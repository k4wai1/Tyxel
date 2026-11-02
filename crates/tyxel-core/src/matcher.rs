// Sub-Agente B: motor de matching. Bipartición cromática de bloques 8x8 RGB,
// búsqueda XOR + POPCNT lineal sin saltos y caché direct-mapped de 64K entradas.

//! Motor de matching: convierte una cuadrilla lógica de 8x8 píxeles RGB
//! (row-major) en un [`Tyxel`].
//!
//! Pipeline (ARCHITECTURE.md §4):
//! 1. Celda plana por baja amplitud por canal -> `glyph_id = 0` y `fg == bg`.
//! 2. Bipartición cromática: eje de mayor amplitud RGB (luminancia si todas las
//!    amplitudes < 24), split por la media, centroides C_0 (fondo) y C_1 (frente),
//!    `target_mask` = 1 donde el píxel está más cerca de C_1.
//! 3. Caché direct-mapped de 64K entradas sin HashMap, indexada por
//!    `((target_mask * 0x9E37_79B9_7F4A_7C15) >> 48)`.
//! 4. Búsqueda lineal branchless sobre las 512 máscaras del atlas con
//!    `XOR + count_ones` doble (normal / invertida); PROHIBIDO el pre-filtro
//!    `popcount(target) ± N`. Empates a favor de la orientación normal.
//! 5. Recolor fino con la máscara real del ganador: `fg = media(máscara==1)`,
//!    `bg = media(máscara==0)`; se intercambian si ganó la orientación invertida.

use crate::atlas;
use crate::tyxel::Tyxel;

const ATLAS_LEN: usize = 512;
const BLOCK_LEN: usize = 64;

/// Celda declarada plana: max-min por canal <= FLAT_AMP en los tres canales.
const FLAT_AMP: u8 = 5;
/// Amplitud debajo de la cual la luminancia perceptual domina la bipartición
/// (los 3 canales amplitudes menores que esto).
const LOW_AMP: u8 = 24;
/// Candidatos por Hamming que se evalúan contra el error cromático real.
const TOP_K: usize = 4;

/// Métricas opcionales de un match (útil para análisis de codificación).
#[derive(Clone, Copy, Debug, Default)]
pub struct MatchMetrics {
    /// Varianza media del bloque (suma de desviaciones cuadradas por canal / 64).
    pub variance: f32,
    /// `count_ones` del `target_mask` (0 en celdas planas).
    pub popcount: u32,
}

/// Matcher con caché direct-mapped de 64K entradas `(target_mask, glyph_id, invert)`.
pub struct Matcher {
    cache: Box<[(u64, u16, bool); 65_536]>,
}

#[inline]
fn slot_of(target_mask: u64) -> usize {
    ((target_mask.wrapping_mul(0x9E37_79B9_7F4A_7C15)) >> 48) as usize
}

/// Sumas, máximos y mínimos por canal RGB de un bloque 8x8.
#[derive(Clone, Copy)]
struct Stats {
    sums: [u32; 3],
    maxes: [u8; 3],
    mins: [u8; 3],
}

impl Stats {
    #[inline]
    fn mean(&self) -> [u8; 3] {
        [
            (self.sums[0] / BLOCK_LEN as u32) as u8,
            (self.sums[1] / BLOCK_LEN as u32) as u8,
            (self.sums[2] / BLOCK_LEN as u32) as u8,
        ]
    }

    #[inline]
    fn amplitude(&self) -> [u8; 3] {
        [
            self.maxes[0] - self.mins[0],
            self.maxes[1] - self.mins[1],
            self.maxes[2] - self.mins[2],
        ]
    }
}

#[inline]
fn stats_of(block: &[[u8; 3]; BLOCK_LEN]) -> Stats {
    let mut s = Stats {
        sums: [0; 3],
        maxes: [0; 3],
        mins: [255; 3],
    };
    for p in block {
        for c in 0..3 {
            let v = p[c];
            s.sums[c] += v as u32;
            if v > s.maxes[c] {
                s.maxes[c] = v;
            }
            if v < s.mins[c] {
                s.mins[c] = v;
            }
        }
    }
    s
}

/// Eje de mayor amplitud RGB (0=R, 1=G, 2=B); si todas las amplitudes
/// son < 24 se prefiere la luminancia (3).
#[inline]
fn choose_axis(amps: [u8; 3]) -> u32 {
    let mut axis = 0u32;
    let mut best = amps[0];
    if amps[1] > best {
        axis = 1;
        best = amps[1];
    }
    if amps[2] > best {
        axis = 2;
        best = amps[2];
    }
    if best < LOW_AMP { 3 } else { axis }
}

#[inline]
fn axis_value(p: [u8; 3], axis: u32) -> u32 {
    match axis {
        0 => p[0] as u32,
        1 => p[1] as u32,
        2 => p[2] as u32,
        _ => (p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000,
    }
}

#[inline]
fn dist2_rgb(a: [u8; 3], b: [u8; 3]) -> u32 {
    let dr = a[0] as i32 - b[0] as i32;
    let dg = a[1] as i32 - b[1] as i32;
    let db = a[2] as i32 - b[2] as i32;
    (dr * dr + dg * dg + db * db) as u32
}

/// Inserta un candidato en el top-K por Hamming (mantienen orden ascendente,
/// O(K) por mis de caché: K=4).
#[inline]
fn insert_candidate(cands: &mut [(u32, u16, bool); TOP_K], diff: u32, id: u16, invert: bool) {
    if diff >= cands[TOP_K - 1].0 {
        return;
    }
    for k in 0..TOP_K {
        if diff < cands[k].0 {
            for m in ((k + 1)..TOP_K).rev() {
                cands[m] = cands[m - 1];
            }
            cands[k] = (diff, id, invert);
            break;
        }
        if diff == cands[k].0 {
            return; // empate: gana el primero encontrado (id menor)
        }
    }
}

/// Bipartición por la media del eje elegido: centroides C_0 (píxeles con valor
/// <= media) y C_1 (> media), y `target_mask` con bit 1 donde el píxel está más
/// cerca de C_1.
fn bipartite(block: &[[u8; 3]; BLOCK_LEN], axis: u32) -> (u64, [u8; 3], [u8; 3]) {
    let mut sum_v = 0u32;
    for p in block {
        sum_v += axis_value(*p, axis);
    }
    let mean_v = sum_v / BLOCK_LEN as u32;

    let mut c0_sum = [0u32; 3];
    let mut c1_sum = [0u32; 3];
    let mut n0 = 0u32;
    let mut n1 = 0u32;
    for p in block {
        if axis_value(*p, axis) > mean_v {
            n1 += 1;
            for c in 0..3 {
                c1_sum[c] += p[c] as u32;
            }
        } else {
            n0 += 1;
            for c in 0..3 {
                c0_sum[c] += p[c] as u32;
            }
        }
    }
    let c0 = [
        (c0_sum[0] / n0.max(1)) as u8,
        (c0_sum[1] / n0.max(1)) as u8,
        (c0_sum[2] / n0.max(1)) as u8,
    ];
    let c1 = [
        (c1_sum[0] / n1.max(1)) as u8,
        (c1_sum[1] / n1.max(1)) as u8,
        (c1_sum[2] / n1.max(1)) as u8,
    ];

    let mut target = 0u64;
    for (i, p) in block.iter().enumerate() {
        if dist2_rgb(*p, c1) < dist2_rgb(*p, c0) {
            target |= 1u64 << i;
        }
    }
    (target, c0, c1)
}

/// Recolor fino (anti-aliasing cromático): con la máscara real del glifo ganador,
/// `fg = media de los píxeles donde el bit es 1` y `bg = media donde es 0`.
/// Devuelve `(fg, bg)` en la orientación natural para evitar swaps internos.
fn recolor(block: &[[u8; 3]; BLOCK_LEN], winner_mask: u64) -> ([u8; 3], [u8; 3]) {
    let mut fg_sum = [0u32; 3];
    let mut bg_sum = [0u32; 3];
    let (mut nf, mut nb) = (0u32, 0u32);
    for (i, p) in block.iter().enumerate() {
        if (winner_mask >> i) & 1 == 1 {
            nf += 1;
            for c in 0..3 {
                fg_sum[c] += p[c] as u32;
            }
        } else {
            nb += 1;
            for c in 0..3 {
                bg_sum[c] += p[c] as u32;
            }
        }
    }
    let avg = |s: [u32; 3], n: u32| -> [u8; 3] {
        if n == 0 {
            [0; 3]
        } else {
            [(s[0] / n) as u8, (s[1] / n) as u8, (s[2] / n) as u8]
        }
    };
    (avg(fg_sum, nf), avg(bg_sum, nb))
}

impl Matcher {
    /// Matcher nuevo con la caché vacía (inicializada a `(0, 0, false)`: un
    /// `target_mask` genuinamente igual a 0 caería en un hit permisible con
    /// glifo 0, equivalente a una celda plana).
    pub fn new() -> Self {
        Matcher {
            cache: Box::new([(0u64, 0u16, false); 65_536]),
        }
    }

    /// Match básico de un bloque lógico 8x8 RGB row-major.
    #[inline]
    pub fn match_block(&mut self, block: &[[u8; 3]; BLOCK_LEN]) -> Tyxel {
        self.match_block_metrics(block).0
    }

    /// Match con métricas adicionales (`variance` y `popcount` del objetivo).
    ///
    /// Pipeline: (1) plano solo si el rango por canal es <= `FLAT_AMP` en los
    /// tres; (2) bipartición multi-eje (canal dominante + luminancia), cada
    /// objetivo pasa por Hamming top-K y el ganador final se elige por el
    /// error cromático real de la reconstrucción. La caché direct-mapped
    /// sigue keyed por `target_mask`.
    pub fn match_block_metrics(&mut self, block: &[[u8; 3]; BLOCK_LEN]) -> (Tyxel, MatchMetrics) {
        debug_assert_eq!(block.len(), BLOCK_LEN);
        let stats = stats_of(block);
        let mean = stats.mean();
        let amps = stats.amplitude();

        let mut sq = 0u64;
        for p in block {
            for c in 0..3 {
                let d = p[c] as i32 - mean[c] as i32;
                sq += (d * d) as u64;
            }
        }
        let variance = sq as f32 / BLOCK_LEN as f32;

        if amps[0] <= FLAT_AMP && amps[1] <= FLAT_AMP && amps[2] <= FLAT_AMP {
            return (
                Tyxel {
                    glyph_id: 0,
                    fg: mean,
                    bg: mean,
                },
                MatchMetrics {
                    variance,
                    popcount: 0,
                },
            );
        }

        let dominant = choose_axis(amps);
        let axes: [u32; 2] = if dominant == 3 { [3, 3] } else { [dominant, 3] };

        let mut best_err = u32::MAX;
        let mut best_t = Tyxel::default();
        let mut best_pop = 0u32;
        for axis in axes {
            let (target, _c0, _c1) = bipartite(block, axis);
            // Máscara degenerada (una sola partida de bits) = plano real.
            if target == 0 || target == u64::MAX {
                continue;
            }
            let slot = slot_of(target);
            let (glyph_id, invert) = if self.cache[slot].0 == target {
                (self.cache[slot].1, self.cache[slot].2)
            } else {
                let (_err, gid, inv) = Self::refine_target(block, target);
                self.cache[slot] = (target, gid, inv);
                (gid, inv)
            };

            let winner = atlas::mask(glyph_id);
            let (fg_natural, bg_natural) = recolor(block, winner);
            let (fg, bg) = if invert {
                (bg_natural, fg_natural)
            } else {
                (fg_natural, bg_natural)
            };
            let mut err = 0u32;
            for (i, p) in block.iter().enumerate() {
                let ref_c = if (target >> i) & 1 == 1 { fg } else { bg };
                err += dist2_rgb(*p, ref_c);
            }
            if err < best_err {
                best_err = err;
                best_t = Tyxel { glyph_id, fg, bg };
                best_pop = target.count_ones();
            }
        }

        if best_err == u32::MAX {
            return (
                Tyxel {
                    glyph_id: 0,
                    fg: mean,
                    bg: mean,
                },
                MatchMetrics {
                    variance,
                    popcount: 0,
                },
            );
        }

        (
            best_t,
            MatchMetrics {
                variance,
                popcount: best_pop,
            },
        )
    }

    /// Ganador real de un `target` fijo (solo leaf del miss de caché):
    /// Hamming top-`TOP_K` sobre las 512 máscaras y después recomposición
    /// cromática real de cada candidato (recolor + SAD-RGB) — un glifo fino
    /// puede ganarle al bloque lleno si reproduce mejor los colores.
    #[cold]
    #[inline(never)]
    fn refine_target(block: &[[u8; 3]; BLOCK_LEN], target: u64) -> (u32, u16, bool) {
        let mut cands: [(u32, u16, bool); TOP_K] = [(u32::MAX, 0, false); TOP_K];
        for id in 0..ATLAS_LEN {
            let m = atlas::MASKS[id];
            let diff_normal = (target ^ m).count_ones();
            let diff_invert = ((!target) ^ m).count_ones();
            let (diff, invert) = if diff_invert < diff_normal {
                (diff_invert, true)
            } else {
                (diff_normal, false)
            };
            insert_candidate(&mut cands, diff, id as u16, invert);
        }
        let mut best = (u32::MAX, 0u16, false);
        for &(diff, id, invert) in cands.iter() {
            if diff == u32::MAX {
                break;
            }
            let mask = atlas::mask(id);
            let (mut fg, mut bg) = recolor(block, mask);
            if invert {
                std::mem::swap(&mut fg, &mut bg);
            }
            let mut err = 0u32;
            for (i, p) in block.iter().enumerate() {
                let ref_c = if (target >> i) & 1 == 1 { fg } else { bg };
                err += dist2_rgb(*p, ref_c);
            }
            if err < best.0 {
                best = (err, id, invert);
            }
        }
        best
    }

    /// Codifica una imagen virtual RGB24 de `(width*8) x (height*16)` píxeles
    /// (`pixels.len() == width*height*8*16*3`) en `width*height` celdas.
    ///
    /// Corrección de aspecto 1:2 (ADR-006): por cada fila lógica `gy` de la
    /// cuadrilla 8x8 se promedian verticalmente las dos filas contiguas
    /// `row*2` y `row*2+1` (muestreo 2:1), de modo que las máscaras del atlas
    /// se comparan en un espacio cuadrado de 8x8.
    pub fn match_frame(&mut self, width: usize, height: usize, pixels: &[u8]) -> Vec<Tyxel> {
        assert!(
            pixels.len() == width * height * 8 * 16 * 3,
            "se esperaba RGB24 de {width}x{height} celdas ({}), hay {} bytes",
            width * height * 8 * 16 * 3,
            pixels.len(),
        );
        let img_w = width * 8;
        let mut block = [[0u8; 3]; BLOCK_LEN];
        let mut out = Vec::with_capacity(width * height);
        for cy in 0..height {
            for cx in 0..width {
                sample_block(pixels, img_w, cx, cy, &mut block);
                out.push(self.match_block(&block));
            }
        }
        out
    }

    /// Igual que [`match_frame`] pero reparte las celdas entre un pool
    /// reutilizable de matchers (uno por hilo, cada uno con su caché propia).
    /// Los matchers DEBEN crearse una sola vez y sobrevivir entre frames:
    /// alocarlos por frame destroza rendimiento (1 MB de caché por hilo).
    pub fn match_frame_parallel(
        pool: &mut [Matcher],
        width: usize,
        height: usize,
        pixels: &[u8],
    ) -> Vec<Tyxel> {
        if pool.len() <= 1 {
            return pool[0].match_frame(width, height, pixels);
        }
        assert!(
            pixels.len() == width * height * 8 * 16 * 3,
            "se esperaba RGB24 de {width}x{height} celdas"
        );
        use std::thread;
        let total = width * height;
        let n = pool.len().min(total);
        let per = total / n;
        let rem = total % n;
        let img_w = width * 8;
        thread::scope(|s| {
            let mut handles = Vec::with_capacity(n);
            let mut start = 0usize;
            for (k, m) in pool.iter_mut().enumerate().take(n) {
                let cells = per + usize::from(k < rem);
                let end = start + cells;
                let px: &[u8] = pixels;
                let wc = width;
                handles.push((
                    s.spawn(move || {
                        let mut block = [[0u8; 3]; BLOCK_LEN];
                        let mut out = Vec::with_capacity(cells);
                        for idx in start..end {
                            let cy = idx / wc;
                            let cx = idx % wc;
                            sample_block(px, img_w, cx, cy, &mut block);
                            out.push(m.match_block(&block));
                        }
                        out
                    }),
                    start,
                    end,
                ));
                start = end;
            }
            let mut result = vec![Tyxel::default(); total];
            for (h, s0, e0) in handles {
                let part = h.join().expect("hilo de matching");
                result[s0..e0].copy_from_slice(&part);
            }
            result
        })
    }
}

/// Extrae el bloque lógico 8x8 de la celda (cx, cy): por cada fila lógica gy
/// promedia verticalmente las filas físicas *2 y *2+1 (aspecto 1:2, ADR-006).
#[inline]
fn sample_block(
    pixels: &[u8],
    img_w: usize,
    cx: usize,
    cy: usize,
    block: &mut [[u8; 3]; BLOCK_LEN],
) {
    for gy in 0..8usize {
        let r0 = (cy * 8 + gy) * 2;
        let r1 = r0 + 1;
        for gx in 0..8usize {
            let c = cx * 8 + gx;
            let i0 = (r0 * img_w + c) * 3;
            let i1 = (r1 * img_w + c) * 3;
            block[gy * 8 + gx] = [
                ((pixels[i0] as u16 + pixels[i1] as u16 + 1) / 2) as u8,
                ((pixels[i0 + 1] as u16 + pixels[i1 + 1] as u16 + 1) / 2) as u8,
                ((pixels[i0 + 2] as u16 + pixels[i1 + 2] as u16 + 1) / 2) as u8,
            ];
        }
    }
}

impl Default for Matcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atlas::mask_bit;

    fn fill(c: [u8; 3]) -> [[u8; 3]; 64] {
        [c; 64]
    }

    fn half_split(top: [u8; 3], bottom: [u8; 3]) -> [[u8; 3]; 64] {
        let mut b = [[0u8; 3]; 64];
        for (i, p) in b.iter_mut().enumerate() {
            *p = if i / 8 < 4 { top } else { bottom };
        }
        b
    }

    // Tests agnósticos del Atlas: estadística, eje, target_mask y caché.

    #[test]
    fn flat_block() {
        let mut m = Matcher::new();
        let t = m.match_block(&fill([255, 0, 0]));
        assert_eq!(t.glyph_id, 0);
        assert_eq!(t.fg, [255, 0, 0]);
        assert_eq!(t.bg, [255, 0, 0]);
        assert!(t.is_flat());
        let mt = m.match_block_metrics(&fill([20, 20, 40]));
        assert_eq!(mt.1.popcount, 0);
        assert!(mt.0.is_flat());
    }

    #[test]
    fn axis_choice() {
        let mut g_only = fill([10, 10, 0]);
        for (i, p) in g_only.iter_mut().enumerate() {
            if i % 8 < 4 {
                p[1] = 200;
            }
        }
        let s = stats_of(&g_only);
        let (target, c0, c1) = bipartite(&g_only, 1);
        assert_eq!(choose_axis(s.amplitude()), 1);
        assert!(target != 0, "target no debe ser degenerado aquí");
        assert_eq!(c1[1], 200);
        assert_eq!(c0[1], 10);

        let luma_low = half_split([10, 10, 10], [25, 25, 25]);
        let s = stats_of(&luma_low);
        assert_eq!(choose_axis(s.amplitude()), 3);
        let (_, c0, c1) = bipartite(&luma_low, 3);
        assert_eq!(c0, [10, 10, 10]);
        assert_eq!(c1, [25, 25, 25]);

        let r_only = half_split([10, 0, 0], [200, 0, 0]);
        assert_eq!(choose_axis(stats_of(&r_only).amplitude()), 0);
    }

    #[test]
    fn target_mask_construction() {
        let b = half_split([10, 0, 0], [200, 0, 0]);
        let s = stats_of(&b);
        assert_eq!(choose_axis(s.amplitude()), 0);
        let (target, c0, c1) = bipartite(&b, 0);
        assert_eq!(target, 0xFFFF_FFFF_0000_0000);
        assert_eq!(c0, [10, 0, 0]);
        assert_eq!(c1, [200, 0, 0]);
    }

    #[test]
    fn slot_spread() {
        for t in [1u64, 0xFFFF_FFFF_0000_0000, u64::MAX, 0xAAAA_AAAA_AAAA_AAAA] {
            assert!(slot_of(t) < 65_536);
        }
        assert_ne!(slot_of(1), slot_of(0xFFFF_FFFF_0000_0000));
    }

    // Tests que dependen de que atlas.rs compile (Sub-Agent A).

    #[test]
    fn winner_hamming_close() {
        let mut m = Matcher::new();
        let t = m.match_block(&half_split([255, 0, 0], [0, 0, 0]));
        let target = 0x0000_0000_FFFF_FFFFu64;
        let wm = atlas::mask(t.glyph_id);
        let dn = (target ^ wm).count_ones();
        let di = ((!target) ^ wm).count_ones();
        assert!(dn.min(di) < 16, "hamming demasiado grande: {dn}/{di}");
        assert!(
            (t.fg == [255, 0, 0] && t.bg == [0, 0, 0])
                || (t.fg == [0, 0, 0] && t.bg == [255, 0, 0]),
            "colores incorrectos: {t:?}"
        );
    }

    #[test]
    fn cache_hit_miss_and_repetition() {
        let mut m = Matcher::new();
        let b = half_split([255, 0, 0], [0, 0, 0]);
        let (target, _, _) = bipartite(&b, 0);
        let slot = slot_of(target);
        let r1 = m.match_block(&b);

        let (_t, gid, _inv) = m.cache[slot];
        assert_eq!(m.cache[slot].0, target, "el miss debe llenar la ranura");
        assert_eq!(gid, r1.glyph_id);

        let r2 = m.match_block(&b);
        assert_eq!(r1, r2, "determinista tras el hit de caché");
    }

    #[test]
    fn frame_shape_and_sample() {
        let (w, h) = (2usize, 2usize);
        let mut px = vec![0u8; w * h * 8 * 16 * 3];
        for (i, chunk) in px.chunks_exact_mut(3).enumerate() {
            let row = i / (w * 8);
            let col = i % (w * 8);
            chunk[0] = 255;
            if row >= 16 && col % 8 < 4 {
                chunk[0] = 0;
            }
        }
        let mut m = Matcher::new();
        let cells = m.match_frame(w, h, &px);
        assert_eq!(cells.len(), w * h);
        assert_eq!(cells[0].fg, [255, 0, 0]);
        for c in &cells {
            assert_eq!(c.fg[1], 0);
            assert_eq!(c.fg[2], 0);
            assert_eq!(c.bg[1], 0);
            assert_eq!(c.bg[2], 0);
        }
    }

    #[test]
    #[should_panic]
    fn frame_rejects_bad_len() {
        let mut m = Matcher::new();
        let _ = m.match_frame(2, 2, &[0u8; 10]);
    }

    /// Test de riqueza: círculos + diagonales + gradiente
    /// de 4 colores deben despertar >= 40 glifos distintos del atlas de 512
    /// y una fracción plana baja. Ejercicio además el pool paralelo.
    #[test]
    fn rich_frame_uses_many_glyphs() {
        let (w, h) = (24usize, 16usize);
        let img_w = w * 8;
        let img_h = h * 16;
        let mut px = vec![0u8; img_w * img_h * 3];
        for y in 0..img_h {
            let yy = y as f64 / (img_h as f64 - 1.0);
            for x in 0..img_w {
                let xx = x as f64 / (img_w as f64 - 1.0);
                // Gradiente diagonal: R crece en x, G crece en y, B decrece.
                let mut r = 30.0 + 180.0 * xx;
                let mut g = 30.0 + 150.0 * yy;
                let mut b = 190.0 - 150.0 * xx;
                // Franjas diagonales finas (periodo 4 px): bordes nítidos.
                let d = x as i32 + y as i32;
                if d % 4 < 2 {
                    r *= 0.4;
                    g *= 0.4;
                    b *= 0.4;
                }
                // Ajedrez grueso de inversión regional (bloques 8x16).
                let chk = ((x / 8) ^ (y / 16)) & 1 == 1;
                if chk {
                    let t = r;
                    r = b * 0.9;
                    b = t * 0.8;
                    g = 255.0 - g;
                }
                // Círculo relleno brillante (bodegón con borde limpio).
                let dx = x as f64 - 60.0;
                let dy = (y as f64 - 40.0) * 0.5;
                if dx * dx + dy * dy < 28.0 * 28.0 {
                    r = 245.0;
                    g = 240.0;
                    b = 10.0;
                }
                // Textura de ruido fino determinista (LFSR-like): rompe la
                // monotonía de los gradientes suaves entre franjas.
                let seed = (x * 31 + y * 57) as u64;
                let n = (seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 56) as f64;
                let jitter = (n - 128.0) / 384.0;
                r += jitter * 42.0;
                g -= jitter * 42.0;
                b += jitter * 42.0;
                let i = (y * img_w + x) * 3;
                px[i] = r.round().clamp(0.0, 255.0) as u8;
                px[i + 1] = g.round().clamp(0.0, 255.0) as u8;
                px[i + 2] = b.round().clamp(0.0, 255.0) as u8;
            }
        }
        let mut pool: Vec<Matcher> = (0..4).map(|_| Matcher::new()).collect();
        let cells = Matcher::match_frame_parallel(&mut pool, w, h, &px);
        assert_eq!(cells.len(), w * h);
        let mut glyphs = std::collections::HashSet::new();
        let mut flat = 0usize;
        for c in &cells {
            glyphs.insert(c.glyph_id);
            if c.glyph_id == 0 {
                flat += 1;
            }
        }
        assert!(
            glyphs.len() >= 40,
            "se esperaban >= 40 glifos distintos, hubo {}: {:?}",
            glyphs.len(),
            {
                let mut ids: Vec<u16> = glyphs.iter().copied().collect();
                ids.sort();
                ids
            }
        );
        // El patrón no debe aplastarse a ladrillos: <= 40% plano.
        assert!(
            flat * 4 <= cells.len(),
            "demasiadas celdas planas: {} de {}",
            flat,
            cells.len()
        );
    }

    /// La división en catholicas del pool debe reproducir exactamente el mismo
    /// resultado que el matcher secuencial (determinismo por hilo).
    #[test]
    fn parallel_matches_serial() {
        let (w, h) = (8usize, 4usize);
        let img_w = w * 8;
        let img_h = h * 16;
        let mut px = vec![0u8; img_w * img_h * 3];
        for (i, v) in px.iter_mut().enumerate() {
            let p = i % (img_w * 3);
            *v = ((p * 37 + (i / (img_w * 3)) * 17) % 256) as u8;
        }
        let mut single = Matcher::new();
        let want = single.match_frame(w, h, &px);
        let mut pool: Vec<Matcher> = (0..6).map(|_| Matcher::new()).collect();
        let got = Matcher::match_frame_parallel(&mut pool, w, h, &px);
        assert_eq!(want, got);
    }

    #[test]
    fn mask_bit_helper_consistency() {
        assert_eq!(mask_bit(0, 0), 1);
        assert_eq!(mask_bit(7, 7), 1u64 << 63);
    }
}
