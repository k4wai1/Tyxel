// Sub-Agente C: codec del formato texto plano .txa (RLE textual *N) + resizer.
//! Formato `.txa` (Tyxel Art): texto plano editable con directivas `#`,
//! celdas `GLIFO#RRGGBB#RRGGBB`, repeticiones RLE `*N` y reescalado autónomo
//! (reconstrucción virtual 8×8 + rematching) para `fastfetch`.

use crate::atlas;
use crate::matcher::Matcher;
use crate::tyxel::Tyxel;
use std::fmt;
use std::io::Write;
use unicode_width::UnicodeWidthChar;

pub const TXA_MAGIC: &str = "#!TYXEL-ART v1.0";

/// Lienzo .txa: W×H celdas en orden row-major (y*W + x).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TxaImage {
    pub width: u16,
    pub height: u16,
    pub cells: Vec<Tyxel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TxaError {
    MissingMagic,
    BadDirective(String),
    BadResolution(String),
    DimensionMismatch { declared: u16, rows: u16 },
    RowWidth { row: usize, expect: usize },
    BadCell(String),
    UnknownCodepoint(char),
    BadRle(String),
    BadColor(String),
}

impl fmt::Display for TxaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingMagic => write!(f, "falta la cabecera {TXA_MAGIC}"),
            Self::BadDirective(d) => write!(f, "directiva no reconocida: {d:?}"),
            Self::BadResolution(d) => write!(f, "resolución inválida: {d:?}"),
            Self::DimensionMismatch { declared, rows } => write!(
                f,
                "#RES declara {declared} filas pero el cuerpo tiene {rows}"
            ),
            Self::RowWidth { row, expect } => write!(f, "fila {row}: se esperaban {expect} celdas"),
            Self::BadCell(t) => write!(f, "celda inválida: {t:?}"),
            Self::UnknownCodepoint(c) => write!(f, "glifo {c:?} no está en el atlas"),
            Self::BadRle(t) => write!(f, "repeticion *N inválida: {t:?}"),
            Self::BadColor(t) => write!(f, "color inválido: {t:?}"),
        }
    }
}

impl std::error::Error for TxaError {}

impl TxaImage {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            width,
            height,
            cells: vec![Tyxel::default(); width as usize * height as usize],
        }
    }

    #[inline]
    pub fn get(&self, x: u16, y: u16) -> Tyxel {
        self.cells[y as usize * self.width as usize + x as usize]
    }

    #[inline]
    pub fn set(&mut self, x: u16, y: u16, t: Tyxel) {
        let idx = y as usize * self.width as usize + x as usize;
        self.cells[idx] = t;
    }
}

// ---------- Serialización texto ----------

/// Escribe el formato .txa (con RLE `*N` únicamente para carreras >= 3).
pub fn to_writer<W: Write>(img: &TxaImage, w: &mut W) -> std::io::Result<()> {
    writeln!(w, "{TXA_MAGIC}")?;
    writeln!(w, "#RES {}x{}", img.width, img.height)?;
    writeln!(w, "#GRID 64bit")?;
    writeln!(w, "#COLOR truecolor")?;
    writeln!(w, "#NERDFONT required")?;
    writeln!(w, "#ATLAS logical-v1")?;
    writeln!(w, "---")?;
    for y in 0..u32::from(img.height) {
        let mut row = String::new();
        let mut x = 0usize;
        let base = y as usize * img.width as usize;
        while x < img.width as usize {
            let t = img.cells[base + x];
            let mut run = 1usize;
            while x + run < img.width as usize && img.cells[base + x + run] == t {
                run += 1;
            }
            let cell = format_token(t);
            if run >= 3 {
                row.push_str(&format!("{cell}*{run}"));
                x += run;
            } else {
                for _ in 0..run {
                    row.push_str(&cell);
                    row.push('\t');
                }
                x += run;
                continue;
            }
            if x < img.width as usize {
                row.push('\t');
            }
        }
        writeln!(w, "{row}")?;
    }
    Ok(())
}

/// Serializa a String (conveniencia para tests y herramientas).
pub fn to_text(img: &TxaImage) -> String {
    let mut buf = Vec::new();
    to_writer(img, &mut buf).expect("escritura en memoria");
    String::from_utf8(buf).expect("utf8")
}

#[inline]
fn hex(c: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

/// Token de celda: `U+XXXX#RRGGBB#RRGGBB` (evita ambigüedad con espacios).
fn format_token(t: Tyxel) -> String {
    let cp = atlas::codepoint(t.glyph_id);
    format!("U+{:04X}{}{}", cp as u32, hex(t.fg), hex(t.bg))
}

fn parse_hex3(s: &str) -> Result<[u8; 3], TxaError> {
    let s = s
        .strip_prefix('#')
        .ok_or_else(|| TxaError::BadColor(s.to_string()))?;
    if s.len() != 6 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(TxaError::BadColor(s.to_string()));
    }
    let v = u32::from_str_radix(s, 16).map_err(|_| TxaError::BadColor(s.to_string()))?;
    Ok([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

/// Parsea el texto .txa completo.
pub fn from_text(src: &str) -> Result<TxaImage, TxaError> {
    let mut width: Option<u16> = None;
    let mut height: Option<u16> = None;
    let mut in_body = false;
    let mut rows: Vec<Vec<Tyxel>> = Vec::new();

    let mut lines = src.lines().peekable();
    match lines.next() {
        Some(m) if m.trim() == TXA_MAGIC => {}
        _ => return Err(TxaError::MissingMagic),
    }
    for line in lines {
        let line = line.trim_end();
        if !in_body {
            if line.trim() == "---" {
                in_body = true;
                continue;
            }
            if let Some(r) = line.strip_prefix("#RES") {
                let r = r.trim();
                let (w, h) = r
                    .split_once('x')
                    .ok_or_else(|| TxaError::BadResolution(r.to_string()))?;
                width = Some(
                    w.trim()
                        .parse()
                        .map_err(|_| TxaError::BadResolution(r.to_string()))?,
                );
                height = Some(
                    h.trim()
                        .parse()
                        .map_err(|_| TxaError::BadResolution(r.to_string()))?,
                );
                continue;
            }
            if line.starts_with('#') {
                continue; // otras directivas se ignoran en v1.0
            }
            if line.trim().is_empty() {
                continue;
            }
            return Err(TxaError::BadDirective(line.to_string()));
        }
        // Cuerpo: filas (los marcadores #ROW n se aceptan y se omiten).
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(_n) = line.strip_prefix("#ROW") {
            continue;
        }
        rows.push(parse_row(line)?);
    }

    let width = width.ok_or_else(|| TxaError::BadDirective("#RES ausente".into()))?;
    let height = height.ok_or_else(|| TxaError::BadDirective("#RES ausente".into()))?;
    let rows_n = rows.len() as u16;
    if rows.len() != height as usize {
        return Err(TxaError::DimensionMismatch {
            declared: height,
            rows: rows_n,
        });
    }
    for (i, r) in rows.iter().enumerate() {
        if r.len() != width as usize {
            return Err(TxaError::RowWidth {
                row: i,
                expect: width as usize,
            });
        }
    }
    let mut cells = Vec::with_capacity(width as usize * height as usize);
    for r in rows {
        cells.extend(r);
    }
    Ok(TxaImage {
        width,
        height,
        cells,
    })
}

/// Parsea una fila: tokens `U+XXXX#FG[#BG][*N]`.
fn parse_row(line: &str) -> Result<Vec<Tyxel>, TxaError> {
    let mut row: Vec<Tyxel> = Vec::new();
    for tok_raw in line.split_whitespace() {
        let mut tok = tok_raw.to_string();
        // Sufijo RLE pegado al token (`...#RRGGBB*12`) o token propio (`*12`).
        if let Some(rest) = tok.strip_prefix('*') {
            push_run(&mut row, rest)?;
            continue;
        }
        let rep: Option<usize> = match tok.rsplit_once('*') {
            Some((m, n)) => match n.parse::<usize>() {
                Ok(n) if n > 0 => {
                    tok = m.to_string();
                    Some(n)
                }
                _ => None,
            },
            None => None,
        };
        let t = parse_cell_token(&tok)?;
        row.push(t);
        if let Some(n) = rep {
            for _ in 1..n {
                row.push(t);
            }
        }
    }
    Ok(row)
}

fn push_run(row: &mut Vec<Tyxel>, n: &str) -> Result<(), TxaError> {
    let n: usize = n.parse().map_err(|_| TxaError::BadRle(n.to_string()))?;
    if n == 0 || row.is_empty() {
        return Err(TxaError::BadRle(n.to_string()));
    }
    let last = *row.last().expect("no vacío");
    for _ in 1..n {
        row.push(last);
    }
    Ok(())
}

/// Celda combinada `U+XXXX#RRGGBB#RRGGBB`.
fn parse_cell_token(tok: &str) -> Result<Tyxel, TxaError> {
    let mut it = tok.split('#');
    let cp_tok = it
        .next()
        .ok_or_else(|| TxaError::BadCell(tok.to_string()))?;
    // Glifo: `U+XXXX` (canónico) o carácter Unicode literal de ancho 1.
    // U+0020 como literal es ambiguo con la separación por blancos: se rechaza.
    let c = if let Some(h) = cp_tok.strip_prefix("U+") {
        let cp = u32::from_str_radix(h, 16).map_err(|_| TxaError::BadCell(tok.to_string()))?;
        char::from_u32(cp).ok_or(TxaError::UnknownCodepoint('\u{FFFD}'))?
    } else {
        let mut chars = cp_tok.chars();
        let first = chars
            .next()
            .ok_or_else(|| TxaError::BadCell(tok.to_string()))?;
        if chars.next().is_some() {
            return Err(TxaError::BadCell(tok.to_string()));
        }
        if first == ' ' || first.width() != Some(1) {
            return Err(TxaError::UnknownCodepoint(first));
        }
        first
    };
    let id = atlas::id_of_codepoint(c).ok_or(TxaError::UnknownCodepoint(c))?;
    let fg = parse_hex3(&format!("#{}", it.next().unwrap_or_default()))?;
    let bg = match it.next() {
        Some(b) => parse_hex3(&format!("#{b}"))?,
        None => fg,
    };
    Ok(Tyxel {
        glyph_id: id,
        fg,
        bg,
    })
}

// ---------- Reescalado autónomo ----------

/// Reconstruye la imagen virtual RGB24 de `(width*8 × height*16)` píxeles
/// (aspecto 1:2, cada fila lógica duplicada) con el glifo en fg sobre bg.
pub fn virtual_pixels(img: &TxaImage) -> Vec<u8> {
    let w8 = img.width as usize * 8;
    let h16 = img.height as usize * 16;
    let mut px = vec![0u8; w8 * h16 * 3];
    for (idx, t) in img.cells.iter().enumerate() {
        if t.glyph_id as usize >= atlas::ATLAS_LEN {
            continue;
        }
        let m = atlas::mask(t.glyph_id);
        let cx = (idx % img.width as usize) * 8;
        let cy = (idx / img.width as usize) * 8;
        for r in 0u32..8 {
            for c in 0u32..8 {
                let col = if atlas::mask_is_set(m, r, c) {
                    t.fg
                } else {
                    t.bg
                };
                for rep in 0..2 {
                    let p = (((cy + r as usize) * 2 + rep) * w8 + cx + c as usize) * 3;
                    px[p] = col[0];
                    px[p + 1] = col[1];
                    px[p + 2] = col[2];
                }
            }
        }
    }
    px
}

/// Reescala al tamaño pedido con muestreo nearest-neighbor y recodifica con
/// el matcher. Devuelve un TxaImage de (new_w × new_h) listo para fastfetch.
pub fn resize(img: &TxaImage, new_w: u16, new_h: u16) -> TxaImage {
    if img.width == new_w && img.height == new_h {
        return img.clone();
    }
    let src = virtual_pixels(img);
    let sw = img.width as usize * 8;
    let sh = img.height as usize * 16;
    let dw = new_w as usize * 8;
    let dh = new_h as usize * 16;
    let mut dst = vec![0u8; dw * dh * 3];
    for dy in 0..dh {
        let sy = (dy * sh) / dh;
        for dx in 0..dw {
            let sx = (dx * sw) / dw;
            let s = (sy * sw + sx) * 3;
            let d = (dy * dw + dx) * 3;
            dst[d] = src[s];
            dst[d + 1] = src[s + 1];
            dst[d + 2] = src[s + 2];
        }
    }
    let mut m = Matcher::new();
    let cells = m.match_frame(new_w as usize, new_h as usize, &dst);
    TxaImage {
        width: new_w,
        height: new_h,
        cells,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atlas;

    fn sample() -> TxaImage {
        let mut img = TxaImage::new(4, 2);
        let blk = atlas::id_of_codepoint('█').unwrap();
        let dot = atlas::id_of_codepoint('·').unwrap_or(blk);
        for x in 0..4 {
            img.set(
                0,
                0,
                Tyxel {
                    glyph_id: blk,
                    fg: [0x11; 3],
                    bg: [0x00; 3],
                },
            );
            img.set(
                1,
                0,
                Tyxel {
                    glyph_id: blk,
                    fg: [0x11; 3],
                    bg: [0x00; 3],
                },
            );
            img.set(
                2,
                0,
                Tyxel {
                    glyph_id: blk,
                    fg: [0x11; 3],
                    bg: [0x00; 3],
                },
            );
            img.set(
                3,
                0,
                Tyxel {
                    glyph_id: dot,
                    fg: [0xFF; 3],
                    bg: [0x22; 3],
                },
            );
            img.set(x, 1, Tyxel::default());
        }
        img
    }

    #[test]
    fn literal_glyph_accepted() {
        let txt = "#!TYXEL-ART v1.0\n#RES 2x1\n---\n█#AABBCC █#AABBCC"; // literal
        let img = from_text(txt).expect("glifo literal");
        assert_eq!(img.get(0, 0).fg, [0xAA, 0xBB, 0xCC]);
        assert_eq!(img.get(0, 0).glyph_id, atlas::id_of_codepoint('█').unwrap());
        let canonical = from_text(&to_text(&img)).expect("canónico U+XXXX");
        assert_eq!(canonical, img);
        // literal de ancho != 1 o espacio suelto: rechazo
        assert!(from_text("#!TYXEL-ART v1.0\n#RES 1x1\n---\n奇怪#FFFFFF").is_err());
        assert!(from_text("#!TYXEL-ART v1.0\n#RES 1x1\n---\n #FFFFFF").is_err());
    }

    #[test]
    fn txa_round_trip() {
        let img = sample();
        let txt = to_text(&img);
        assert!(txt.starts_with(TXA_MAGIC));
        assert!(txt.contains("*3")); // RLE aplica a la carrera de 3 █
        let back = from_text(&txt).expect("parse");
        assert_eq!(back, img);
    }

    #[test]
    fn rejects_missing_magic() {
        assert!(matches!(from_text("basura"), Err(TxaError::MissingMagic)));
    }

    #[test]
    fn unknown_glyph_rejected() {
        let mut img = sample();
        img.set(
            0,
            0,
            Tyxel {
                glyph_id: 511,
                fg: [1, 2, 3],
                bg: [4, 5, 6],
            },
        );
        let txt = to_text(&img);
        let _ = from_text(&txt); // id 511 existe en el atlas: no debe fallar
    }

    #[test]
    fn resize_changes_dims() {
        let img = sample();
        let out = resize(&img, 2, 1);
        assert_eq!(out.width, 2);
        assert_eq!(out.height, 1);
        assert_eq!(out.cells.len(), 2);
    }
}
