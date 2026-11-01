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
    if best < 24 { 3 } else { axis }
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
    pub fn match_block_metrics(&mut self, block: &[[u8; 3]; BLOCK_LEN]) -> (Tyxel, MatchMetrics) {
        debug_assert_eq!(block.len(), BLOCK_LEN);
        let stats = stats_of(block);
        let mean = stats.mean();
        let amps = stats.amplitude();
        let axis = choose_axis(amps);

        let mut sq = 0u64;
        for p in block {
            for c in 0..3 {
                let d = p[c] as i32 - mean[c] as i32;
                sq += (d * d) as u64;
            }
        }
        let variance = sq as f32 / BLOCK_LEN as f32;

        if amps[0] <= 12 && amps[1] <= 12 && amps[2] <= 12 {
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

        let (target, _c0, _c1) = bipartite(block, axis);

        let slot = slot_of(target);
        let (glyph_id, invert) = if self.cache[slot].0 == target {
            (self.cache[slot].1, self.cache[slot].2)
        } else {
            self.search_and_store(target, slot)
        };

        let winner = atlas::mask(glyph_id);
        let (fg_natural, bg_natural) = recolor(block, winner);
        let (fg, bg) = if invert {
            (bg_natural, fg_natural)
        } else {
            (fg_natural, bg_natural)
        };

        (
            Tyxel { glyph_id, fg, bg },
            MatchMetrics {
                variance,
                popcount: target.count_ones(),
            },
        )
    }

    /// Búsqueda lineal branchless sobre las 512 máscaras del atlas (solo en miss
    /// de caché): `diff_normal = count_ones(target ^ m)` y
    /// `diff_invert = count_ones(!target ^ m)`; gana la menor distancia de
    /// Hamming y los empates favorecen la orientación normal.
    #[cold]
    #[inline(never)]
    fn search_and_store(&mut self, target: u64, slot: usize) -> (u16, bool) {
        let mut best_diff = u32::MAX;
        let mut best_id = 0u16;
        let mut best_invert = false;
        for id in 0..ATLAS_LEN {
            let m = atlas::MASKS[id];
            let diff_normal = (target ^ m).count_ones();
            let diff_invert = ((!target) ^ m).count_ones();
            let (diff, invert) = if diff_invert < diff_normal {
                (diff_invert, true)
            } else {
                (diff_normal, false)
            };
            if diff < best_diff {
                best_diff = diff;
                best_id = id as u16;
                best_invert = invert;
            }
        }
        self.cache[slot] = (target, best_id, best_invert);
        (best_id, best_invert)
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
                out.push(self.match_block(&block));
            }
        }
        out
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

    #[test]
    fn mask_bit_helper_consistency() {
        assert_eq!(mask_bit(0, 0), 1);
        assert_eq!(mask_bit(7, 7), 1u64 << 63);
    }
}
