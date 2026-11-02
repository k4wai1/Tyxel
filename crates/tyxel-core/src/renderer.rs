//! Renderizador de fotogramas Tyxel a framebuffer RGB de alta resolución
//! (Fase 3 — Virtual Terminal Emulator). Toma la cuadrilla de celdas
//! (glyph_id + fg/bg) y compone un `Vec<u8>` RGB24 listo para `ffmpeg`
//! (`-f rawvideo -pix_fmt rgb24 -s WxH`).
//!
//! Dos motores de glifos:
//! - Procedural Hi-DPI (default, sin dependencias): super-sampling 4×4 de la
//!   máscara 8×8 del atlas a una celda `cell_w × cell_h` (aspecto 1:2).
//! - TTF real (opcional, vía `fontdue`, 100% Pure Rust): rasteriza los 512
//!   codepoints del atlas con la fuente del usuario (ej. `~/.termux/font.ttf`)
//!   sobre la misma cuadrícula de cobertura alpha.
//!
//! Composición multinúcleo por bandas de filas de píxeles (`thread::scope`),
//! con modo de color (TrueColor / 256 / 16 con Bayer / mono) y scanlines
//! opcionales.

use crate::atlas;
use crate::tyxel::{ANSI_16, Tyxel, ansi256_to_rgb, rgb_to_ansi16_dithered};
use std::thread;

/// Fondo por defecto estilo terminal (GitHub dark) para el padding del lienzo.
pub const DEFAULT_BG: [u8; 3] = [0x0D, 0x11, 0x17];

/// Modo de color para el render simulado.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum QuantMode {
    #[default]
    TrueColor,
    Ansi256,
    Ansi16,
    Mono,
}

/// Geometría: celdas `(cell_w, cell_h = 2*cell_w)` y desplazamiento centrado.
#[derive(Clone, Copy, Debug)]
pub struct Geometry {
    pub cell_w: usize,
    pub cell_h: usize,
    pub off_x: usize,
    pub off_y: usize,
    pub out_w: usize,
    pub out_h: usize,
}

/// Ajusta `cols × rows` celdas (celda 1:2) al múltiplo entero más grande que
/// cabe en `(target_w, target_h)`, centrado. Nunca produce celdas de 0 px.
pub fn fit_geometry(cols: usize, rows: usize, target_w: usize, target_h: usize) -> Geometry {
    let cols = cols.max(1);
    let rows = rows.max(1);
    let cw = (target_w / cols).min(target_h / (rows * 2)).max(1);
    let ch = cw * 2;
    let canvas_w = cw * cols;
    let canvas_h = ch * rows;
    Geometry {
        cell_w: cw,
        cell_h: ch,
        off_x: target_w.saturating_sub(canvas_w) / 2,
        off_y: target_h.saturating_sub(canvas_h) / 2,
        out_w: target_w,
        out_h: target_h,
    }
}

/// Atlas de cobertura alpha (0..=255) de los 512 glifos al tamaño de celda:
/// `512 * cell_w * cell_h` bytes (~576 KB a 24×48), construido UNA vez por
/// ejecución del render.
pub struct GlyphAtlas {
    pub cell_w: usize,
    pub cell_h: usize,
    alphas: Vec<u8>,
}

impl GlyphAtlas {
    /// Motor procedural Hi-DPI: cada píxel de celda muestrea la máscara 8×8
    /// con una rejilla 4×4 (16 muestras). `bit = (row*8)+col` (ADR del core).
    pub fn procedural(cell_w: usize, cell_h: usize) -> Self {
        let cell_w = cell_w.max(1);
        let cell_h = cell_h.max(2);
        let mut alphas = vec![0u8; atlas::ATLAS_LEN * cell_w * cell_h];
        for id in 0..atlas::ATLAS_LEN {
            let mask = atlas::mask(id as u16);
            let base = id * cell_w * cell_h;
            for py in 0..cell_h {
                for px in 0..cell_w {
                    let mut hits = 0u32;
                    for sy in 0..4u32 {
                        for sx in 0..4u32 {
                            // Centro de la submuestra, mapeado a lógica 8×8.
                            let lx = ((px as u32 * 4 + sx) * 8) / (cell_w as u32 * 4);
                            let ly = ((py as u32 * 4 + sy) * 8) / (cell_h as u32 * 4);
                            let bit = ly.min(7) * 8 + lx.min(7);
                            hits += u32::from((mask >> bit) & 1 == 1);
                        }
                    }
                    alphas[base + py * cell_w + px] = ((hits * 255) / 16) as u8;
                }
            }
        }
        GlyphAtlas {
            cell_w,
            cell_h,
            alphas,
        }
    }

    /// Motor TTF real: rasteriza los 512 codepoints con `fontdue` (100%
    /// Pure Rust) y compone la cobertura en la celda, centrada y clipped.
    /// Devuelve None si la fuente no carga (el caller degrada a procedural).
    pub fn from_font(font_data: Vec<u8>, cell_w: usize, cell_h: usize) -> Option<Self> {
        let font = fontdue::Font::from_bytes(font_data, fontdue::FontSettings::default()).ok()?;
        let cell_w = cell_w.max(1);
        let cell_h = cell_h.max(2);
        let mut me = GlyphAtlas::procedural(cell_w, cell_h);
        for id in 0..atlas::ATLAS_LEN {
            let c = atlas::codepoint(id as u16);
            if c == '\0' {
                continue;
            }
            let (m, bmp) = font.rasterize(c, cell_h as f32);
            let bm_w = m.width;
            let bm_h = m.height;
            if bm_w == 0 || bm_h == 0 || bmp.len() < bm_w * bm_h {
                continue;
            }
            // Baseline tipográfico al 80% de la altura de celda (em estándar).
            let baseline_y = (0.80_f32 * cell_h as f32).round();
            let top = baseline_y - (m.ymin as f32 + bm_h as f32);
            let top = top.max(0.0) as usize;
            let left = (((cell_w as i64) - (bm_w as i64)) / 2).max(0) as usize;
            let base = id * cell_w * cell_h;
            for y in 0..bm_h {
                let dy = top + y;
                if dy >= cell_h {
                    break;
                }
                for x in 0..bm_w {
                    let dx = left + x;
                    if dx >= cell_w {
                        break;
                    }
                    let a = bmp[y * bm_w + x];
                    if a > 0 {
                        let idx = base + dy * cell_w + dx;
                        me.alphas[idx] = (me.alphas[idx] as u32 + a as u32).min(255) as u8;
                    }
                }
            }
        }
        Some(me)
    }

    #[inline]
    pub fn coverage(&self, id: u16, px: usize, py: usize) -> u8 {
        self.alphas[id as usize * self.cell_w * self.cell_h + py * self.cell_w + px]
    }
}

/// Colores efectivos (fg, bg) del modo elegido para la celda `(cell_x, cell_y)`.
/// El dithering Bayer de Ansi16 se aplica por POSICIÓN DE CELDA (como en
/// terminal real), no por píxel de sub-muestreo.
#[inline]
pub fn effective_colors(
    t: &Tyxel,
    mode: QuantMode,
    cell_x: usize,
    cell_y: usize,
) -> ([u8; 3], [u8; 3]) {
    match mode {
        QuantMode::TrueColor => (t.fg, t.bg),
        QuantMode::Ansi256 => (
            ansi256_to_rgb(crate::tyxel::rgb_to_ansi256(t.fg)),
            ansi256_to_rgb(crate::tyxel::rgb_to_ansi256(t.bg)),
        ),
        QuantMode::Ansi16 => {
            let fi = rgb_to_ansi16_dithered(t.fg, cell_x, cell_y);
            let bi = rgb_to_ansi16_dithered(t.bg, cell_x, cell_y);
            (ANSI_16[fi as usize], ANSI_16[bi as usize])
        }
        QuantMode::Mono => ([255, 255, 255], [0, 0, 0]),
    }
}

/// Compone la cuadrilla en un framebuffer RGB24 `(out_w × out_h × 3)`,
/// centrado con `bg`, paralelizado por bandas horizontales en `threads`
/// hilos. `cells` debe ser `cols × rows` row-major.
#[allow(clippy::too_many_arguments)]
pub fn compose_frame(
    geo: &Geometry,
    cells: &[Tyxel],
    cols: usize,
    glyph_atlas: &GlyphAtlas,
    mode: QuantMode,
    scanlines: bool,
    bg: [u8; 3],
    threads: usize,
) -> Vec<u8> {
    assert_eq!(
        cells.len(),
        geo_rows_of(cells, cols) * cols,
        "celdas no casan con cols"
    );
    let bytes = geo.out_w * geo.out_h * 3;
    let mut buf = vec![0u8; bytes];
    if bytes == 0 {
        return buf;
    }
    for px3 in buf.chunks_exact_mut(3) {
        px3[0] = bg[0];
        px3[1] = bg[1];
        px3[2] = bg[2];
    }

    // Paleta pre-calculada una vez por frame (celda a celda).
    let palette: Vec<([u8; 3], [u8; 3])> = cells
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let (cx, cy) = (i % cols, i / cols);
            effective_colors(t, mode, cx, cy)
        })
        .collect();

    let rows = cells.len() / cols;
    let canvas_w = geo.cell_w * cols;
    let canvas_h = geo.cell_h * rows;
    let n = threads.max(1).min(geo.out_h.max(1));
    let per = geo.out_h / n;
    let rem = geo.out_h % n;

    thread::scope(|s| {
        let mut handles = Vec::with_capacity(n);
        // Cada banda de filas recibe una sub-slice MUTABLE disjunta del
        // framebuffer: sin sinchronización de hilos y sin races.
        let mut rest: &mut [u8] = &mut buf;
        let mut y0 = 0usize;
        for k in 0..n {
            let band = per + usize::from(k < rem);
            let band_bytes = band * geo.out_w * 3;
            let (band_buf, tail) = rest.split_at_mut(band_bytes);
            rest = tail;
            let g = *geo;
            let ga = glyph_atlas;
            let pal: &[([u8; 3], [u8; 3])] = &palette;
            let c0 = y0;
            let band_rows = band;
            let line = band_buf;
            handles.push(s.spawn(move || {
                for local in 0..band_rows {
                    let row = c0 + local;
                    let ty = row as i64 - g.off_y as i64;
                    if ty < 0 || ty >= canvas_h as i64 {
                        continue; // fila de padding: queda el fondo
                    }
                    let cy = ty as usize / g.cell_h;
                    let ly = ty as usize % g.cell_h;
                    let row_o = local * g.out_w;
                    for tx in 0..canvas_w {
                        let x = g.off_x + tx;
                        if x >= g.out_w {
                            break;
                        }
                        let cx = tx / g.cell_w;
                        let lx = tx % g.cell_w;
                        let id = cells[cy * cols + cx].glyph_id % atlas::ATLAS_LEN as u16;
                        let a = ga.coverage(id, lx, ly) as u32;
                        let (fg, bgc) = pal[cy * cols + cx];
                        let o = (row_o + x) * 3;
                        if a == 0 {
                            line[o] = bgc[0];
                            line[o + 1] = bgc[1];
                            line[o + 2] = bgc[2];
                        } else if a == 255 {
                            line[o] = fg[0];
                            line[o + 1] = fg[1];
                            line[o + 2] = fg[2];
                        } else {
                            let af = a;
                            line[o] = ((bgc[0] as u32 * (255 - af) + fg[0] as u32 * af + 127) / 255)
                                as u8;
                            line[o + 1] = ((bgc[1] as u32 * (255 - af) + fg[1] as u32 * af + 127)
                                / 255) as u8;
                            line[o + 2] = ((bgc[2] as u32 * (255 - af) + fg[2] as u32 * af + 127)
                                / 255) as u8;
                        }
                    }
                    // Scanline: atenúa la última fila lógica de la celda.
                    if scanlines && ly == g.cell_h - 1 {
                        for tx in 0..canvas_w {
                            let o = (row_o + g.off_x + tx) * 3;
                            line[o] = (line[o] as u32 * 215 / 255) as u8;
                            line[o + 1] = (line[o + 1] as u32 * 215 / 255) as u8;
                            line[o + 2] = (line[o + 2] as u32 * 215 / 255) as u8;
                        }
                    }
                }
            }));
            y0 += band;
        }
        for h in handles {
            h.join().expect("composición de frame falló");
        }
    });
    buf
}

#[inline]
fn geo_rows_of(cells: &[Tyxel], cols: usize) -> usize {
    cells.len() / cols.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tyxel::Tyxel;

    fn t(id: u16, fg: [u8; 3], bg: [u8; 3]) -> Tyxel {
        Tyxel {
            glyph_id: id,
            fg,
            bg,
        }
    }

    #[test]
    fn geometry_fit_71x20_in_1080p() {
        let g = fit_geometry(71, 20, 1920, 1080);
        assert_eq!(g.cell_w, 27); // min(1920/71=27, 1080/40=27)
        assert_eq!(g.cell_h, 54);
        assert_eq!(g.off_x, (1920 - 27 * 71) / 2);
        assert_eq!(g.off_y, 0);
        let g2 = fit_geometry(4, 2, 64, 64);
        assert_eq!(g2.cell_w, 16); // min(64/4=16, 64/4=16)
        assert_eq!(g2.cell_h, 32);
        assert_eq!((g2.off_x, g2.off_y), (0, 0));
    }

    #[test]
    fn compose_block_center_and_padding() {
        let geo = fit_geometry(1, 1, 24, 40); // celda 20x40? no: min(24,20)=20 → padding x2
        let ga = GlyphAtlas::procedural(geo.cell_w, geo.cell_h);
        let cells = [t(1, [255, 0, 0], [0, 0, 0])];
        let buf = compose_frame(
            &geo,
            &cells,
            1,
            &ga,
            QuantMode::TrueColor,
            false,
            [10, 20, 30],
            2,
        );
        assert_eq!(buf.len(), 24 * 40 * 3);
        let o = ((geo.off_y + geo.cell_h / 2) * 24 + geo.off_x + geo.cell_w / 2) * 3;
        assert_eq!(&buf[o..o + 3], &[255, 0, 0]);
        // La esquina (0,0) es padding (off_x > 0) → fondo [10,20,30].
        assert!(geo.off_x > 0);
        assert_eq!(&buf[0..3], &[10, 20, 30]);
    }

    #[test]
    fn parallel_match_and_shapes() {
        let ga = GlyphAtlas::procedural(4, 8);
        // geometría exactamente compatible con el atlas de 4x8.

        let cells: Vec<Tyxel> = (0..8u32)
            .map(|i| t((i % 126 + 2) as u16, [200, 100, 50], [10, 10, 10]))
            .collect();
        let geo = fit_geometry(4, 2, 64, 64);
        let a = compose_frame(
            &geo,
            &cells,
            4,
            &ga,
            QuantMode::TrueColor,
            false,
            [0, 0, 0],
            1,
        );
        let b = compose_frame(
            &geo,
            &cells,
            4,
            &ga,
            QuantMode::TrueColor,
            false,
            [0, 0, 0],
            7,
        );
        assert_eq!(a, b);
    }

    #[test]
    fn quant_modes_change_palette() {
        let geo = fit_geometry(1, 1, 16, 32);
        let ga = GlyphAtlas::procedural(geo.cell_w, geo.cell_h);
        let cells = [t(1, [123, 45, 67], [10, 10, 10])];
        let tc = compose_frame(
            &geo,
            &cells,
            1,
            &ga,
            QuantMode::TrueColor,
            false,
            [0, 0, 0],
            1,
        );
        let p8 = compose_frame(
            &geo,
            &cells,
            1,
            &ga,
            QuantMode::Ansi256,
            false,
            [0, 0, 0],
            1,
        );
        assert_ne!(tc, p8);
        let m = compose_frame(&geo, &cells, 1, &ga, QuantMode::Mono, false, [0, 0, 0], 1);
        let o = ((geo.off_y + geo.cell_h / 2) * 16 + geo.off_x + geo.cell_w / 2) * 3;
        assert_eq!(&m[o..o + 3], &[255, 255, 255]);
    }

    #[test]
    fn scanlines_darken_last_row_per_cell() {
        let geo = fit_geometry(1, 1, 8, 16);
        let ga = GlyphAtlas::procedural(geo.cell_w, geo.cell_h);
        let cells = [t(1, [255, 255, 255], [255, 255, 255])];
        let buf = compose_frame(
            &geo,
            &cells,
            1,
            &ga,
            QuantMode::TrueColor,
            true,
            [0, 0, 0],
            1,
        );
        let last = geo.off_y + geo.cell_h - 1;
        let o = (last * 8 + geo.off_x + 0) * 3;
        assert_eq!(buf[o], (255 * 215 / 255) as u8); // ~85% del brillo
        let first = (geo.off_y * 8 + geo.off_x) * 3;
        assert_eq!(buf[first], 255);
    }
}
