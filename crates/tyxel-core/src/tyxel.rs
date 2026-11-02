// Sub-Agente B: struct Tyxel de 8 bytes exactos (align 8) + conversiones
// de color (truecolor <-> ANSI 256 <-> ANSI 16 con Bayer 4x4).

//! Struct `Tyxel` (8 bytes, alineado a 8) y cuantización de color entera
//! determinista (ANSI 256, ANSI 16, dithering Bayer 4x4).

/// Celda tipográfica: glifo del atlas + color de frente y fondo en RGB TrueColor.
///
/// Ocupa exactamente 8 bytes y está alineada a 8, de modo que la comparación
/// `a == b` se compila a una sola comparación de registro de 64 bits
/// (ideal para Delta Frames). La serialización en RAM y en disco (I-Frame
/// del `.txv`) es la misma: **siempre little-endian**, byte a byte
/// `glyph_id.low, glyph_id.high, fg[0..3], bg[0..3]`.
#[repr(C, align(8))]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Tyxel {
    /// Índice del glifo en el Atlas Lógico (0..512).
    pub glyph_id: u16,
    /// Color RGB del carácter.
    pub fg: [u8; 3],
    /// Color RGB del fondo.
    pub bg: [u8; 3],
}

impl Tyxel {
    /// Bytes en orden de serialización canonico del formato (little-endian):
    /// `[gid.low, gid.high, fg0, fg1, fg2, bg0, bg1, bg2]`.
    #[inline]
    pub fn bytes(&self) -> [u8; 8] {
        [
            self.glyph_id as u8,
            (self.glyph_id >> 8) as u8,
            self.fg[0],
            self.fg[1],
            self.fg[2],
            self.bg[0],
            self.bg[1],
            self.bg[2],
        ]
    }

    /// Entero de 64 bits con la serialización canonica little-endian
    /// (coincide con el layout del I-Frame del `.txv` y con `bytes()`).
    #[inline]
    pub const fn as_u64(self) -> u64 {
        let gid = self.glyph_id as u64;
        let f0 = self.fg[0] as u64;
        let f1 = self.fg[1] as u64;
        let f2 = self.fg[2] as u64;
        let b0 = self.bg[0] as u64;
        let b1 = self.bg[1] as u64;
        let b2 = self.bg[2] as u64;
        gid | (f0 << 16) | (f1 << 24) | (f2 << 32) | (b0 << 40) | (b1 << 48) | (b2 << 56)
    }

    /// Inverso exacto de [`Tyxel::as_u64`] (little-endian).
    #[inline]
    pub const fn from_u64(v: u64) -> Tyxel {
        Tyxel {
            glyph_id: (v & 0xFFFF) as u16,
            fg: [
                ((v >> 16) & 0xFF) as u8,
                ((v >> 24) & 0xFF) as u8,
                ((v >> 32) & 0xFF) as u8,
            ],
            bg: [
                ((v >> 40) & 0xFF) as u8,
                ((v >> 48) & 0xFF) as u8,
                ((v >> 56) & 0xFF) as u8,
            ],
        }
    }

    /// Celda plana: el glifo es invisible y solo importa el fondo.
    #[inline]
    pub const fn is_flat(&self) -> bool {
        self.fg[0] == self.bg[0] && self.fg[1] == self.bg[1] && self.fg[2] == self.bg[2]
    }
}

const _: () = {
    assert!(core::mem::size_of::<Tyxel>() == 8);
    assert!(core::mem::align_of::<Tyxel>() == 8);
};

/// Matriz de Bayer 4x4 canonical en niveles enteros 0..=15
/// (offset por sub-celda, escalado x16 al aplicar sobre el canal RGB).
pub const BAYER_4X4: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// Nivel del cubo 6x6x6 (0..=5) más cercano a un canal de 8 bits.
/// Umbrales en los puntos medios: 0, 95, 135, 175, 215, 255.
#[inline]
fn cube_level(v: u8) -> u32 {
    if v < 48 {
        0
    } else if v < 115 {
        1
    } else if v < 155 {
        2
    } else if v < 195 {
        3
    } else if v < 235 {
        4
    } else {
        5
    }
}

/// Valor exacto de 8 bits de un nivel del cubo (0..=5).
#[inline]
const fn cube_value(level: u32) -> u8 {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    LEVELS[level as usize]
}

/// Tabla de los 16 colores ANSI base (indices 0..=15).
pub const ANSI_16: [[u8; 3]; 16] = [
    [0, 0, 0],
    [128, 0, 0],
    [0, 128, 0],
    [128, 128, 0],
    [0, 0, 128],
    [128, 0, 128],
    [0, 128, 128],
    [192, 192, 192],
    [128, 128, 128],
    [255, 0, 0],
    [0, 255, 0],
    [255, 255, 0],
    [0, 0, 255],
    [255, 0, 255],
    [0, 255, 255],
    [255, 255, 255],
];

/// Reconstrucción RGB de un índice xterm-256 (0..=255) por aritmética entera:
/// 0..=15 tabla ANSI base, 16..=231 cubo 6x6x6, 232..=255 rampa de 24 grises.
#[inline]
pub fn ansi256_to_rgb(idx: u8) -> [u8; 3] {
    match idx {
        0..=15 => ANSI_16[idx as usize],
        16..=231 => {
            let i = (idx - 16) as u32;
            let r = i / 36;
            let g = (i % 36) / 6;
            let b = i % 6;
            [cube_value(r), cube_value(g), cube_value(b)]
        }
        _ => {
            let g = 8 + (idx as u32 - 232) * 10;
            [g as u8, g as u8, g as u8]
        }
    }
}

/// Distancia cuadrática entera al candidato índice 256 más próximo por canal RGB.
#[inline]
fn dist2_rgb(a: [u8; 3], b: [u8; 3]) -> u32 {
    let dr = a[0] as i32 - b[0] as i32;
    let dg = a[1] as i32 - b[1] as i32;
    let db = a[2] as i32 - b[2] as i32;
    (dr * dr + dg * dg + db * db) as u32
}

/// Cuantiza RGB TrueColor al índice xterm-256 (cubo 6x6x6 + rampa de 24 grises),
/// eligiendo por distancia entera entre el candidato del cubo y el del gris.
#[inline]
pub fn rgb_to_ansi256(rgb: [u8; 3]) -> u8 {
    let r = cube_level(rgb[0]);
    let g = cube_level(rgb[1]);
    let b = cube_level(rgb[2]);
    let cube_idx = (16 + 36 * r + 6 * g + b) as u8;

    let gray = (rgb[0] as u32 + rgb[1] as u32 + rgb[2] as u32) / 3;
    let gray_idx: u8 = if gray > 238 {
        255
    } else if gray < 8 {
        16
    } else {
        (232 + (gray - 8) * 24 / 231) as u8
    };

    let d_cube = dist2_rgb(rgb, ansi256_to_rgb(cube_idx));
    let d_gray = dist2_rgb(rgb, ansi256_to_rgb(gray_idx));
    if d_gray < d_cube { gray_idx } else { cube_idx }
}

/// Cuantiza RGB a un índice ANSI 16 (0..=15) por vecino más próximo entero,
/// sin dithering (desempates a favor del índice menor).
#[inline]
pub fn rgb_to_ansi16(rgb: [u8; 3]) -> u8 {
    let mut best = 0u8;
    let mut best_d = u32::MAX;
    let mut i = 0;
    while i < 16 {
        let d = dist2_rgb(rgb, ANSI_16[i]);
        if d < best_d {
            best_d = d;
            best = i as u8;
        }
        i += 1;
    }
    best
}

/// Cuantiza RGB a ANSI 16 con Ordered Dithering: la matriz de Bayer 4x4 en la
/// posición `(x, y)` genera un offset +/- sobre cada canal antes de elegir el
/// índice ANSI 16, de modo que regiones grandes promedian al tono correcto.
/// Índices `x`, `y` se toman módulo 4 para teselar la pantalla.
#[inline]
pub fn rgb_to_ansi16_dithered(rgb: [u8; 3], x: usize, y: usize) -> u8 {
    let b = BAYER_4X4[y & 3][x & 3] as i32;
    let shift = b * 16 + 8 - 128;
    let dith = [
        (rgb[0] as i32 + shift).clamp(0, 255) as u8,
        (rgb[1] as i32 + shift).clamp(0, 255) as u8,
        (rgb[2] as i32 + shift).clamp(0, 255) as u8,
    ];
    rgb_to_ansi16(dith)
}

/// Modo de color soportado por el reproductor / export.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ColorMode {
    /// RGB TrueColor (24 bits).
    #[default]
    TrueColor,
    /// Paleta xterm-256 (cubo 6x6x6 + 24 grises).
    Ansi256,
    /// Los 16 colores ANSI base (con dithering Bayer).
    Ansi16,
    /// Blanco y negro puro.
    Mono,
}

impl From<&str> for ColorMode {
    fn from(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "truecolor" | "tc" | "rgb" => ColorMode::TrueColor,
            "256" | "ansi256" | "xterm" => ColorMode::Ansi256,
            "16" | "ansi16" => ColorMode::Ansi16,
            _ => ColorMode::Mono,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_and_align() {
        assert_eq!(core::mem::size_of::<Tyxel>(), 8);
        assert_eq!(core::mem::align_of::<Tyxel>(), 8);
    }

    #[test]
    fn roundtrip_key_values() {
        let cases = [
            Tyxel::default(),
            Tyxel {
                glyph_id: 0,
                fg: [0, 0, 0],
                bg: [0, 0, 0],
            },
            Tyxel {
                glyph_id: 1,
                fg: [255, 255, 255],
                bg: [0, 0, 0],
            },
            Tyxel {
                glyph_id: 511,
                fg: [1, 2, 3],
                bg: [4, 5, 6],
            },
            Tyxel {
                glyph_id: 0x1234,
                fg: [128, 64, 32],
                bg: [16, 8, 200],
            },
        ];
        for t in cases {
            assert_eq!(Tyxel::from_u64(t.as_u64()), t);
            assert_eq!(t.bytes(), Tyxel::from_u64(t.as_u64()).bytes());
        }
    }

    #[test]
    fn u64_layout_le() {
        let t = Tyxel {
            glyph_id: 0x0123,
            fg: [1, 2, 3],
            bg: [4, 5, 6],
        };
        assert_eq!(t.bytes(), [0x23, 0x01, 1, 2, 3, 4, 5, 6]);
        assert_eq!(t.as_u64(), 0x0605_0403_0201_0123);
    }

    #[test]
    fn is_flat() {
        assert!(
            Tyxel {
                glyph_id: 7,
                fg: [9, 9, 9],
                bg: [9, 9, 9]
            }
            .is_flat()
        );
        assert!(
            !Tyxel {
                glyph_id: 7,
                fg: [9, 9, 9],
                bg: [9, 9, 8]
            }
            .is_flat()
        );
        assert!(Tyxel::default().is_flat());
    }

    #[test]
    fn ansi256_endpoints() {
        assert_eq!(rgb_to_ansi256([0, 0, 0]), 16);
        assert_eq!(rgb_to_ansi256([255, 255, 255]), 231);
        assert_eq!(rgb_to_ansi256([8, 8, 8]), 232);
        assert_eq!(rgb_to_ansi256([238, 238, 238]), 255);
        assert_eq!(rgb_to_ansi256([250, 250, 250]), 231);
    }

    #[test]
    fn ansi256_cube_grid() {
        for r in 0..=5u32 {
            for g in 0..=5u32 {
                for b in 0..=5u32 {
                    let idx = (16 + 36 * r + 6 * g + b) as u8;
                    assert_eq!(
                        ansi256_to_rgb(idx),
                        [cube_value(r), cube_value(g), cube_value(b)]
                    );
                    assert_eq!(rgb_to_ansi256(ansi256_to_rgb(idx)), idx);
                }
            }
        }
        assert_eq!(ansi256_to_rgb(0), [0, 0, 0]);
        assert_eq!(ansi256_to_rgb(15), [255, 255, 255]);
        assert_eq!(ansi256_to_rgb(232), [8, 8, 8]);
        assert_eq!(ansi256_to_rgb(255), [238, 238, 238]);
        for i in 232..=255u8 {
            let [r, g, b] = ansi256_to_rgb(i);
            assert_eq!(r, g);
            assert_eq!(g, b);
            assert_eq!(rgb_to_ansi256([r, g, b]), i);
        }
    }

    #[test]
    fn ansi16_basic() {
        assert_eq!(rgb_to_ansi16([0, 0, 0]), 0);
        assert_eq!(rgb_to_ansi16([255, 255, 255]), 15);
        assert_eq!(rgb_to_ansi16([255, 0, 0]), 9);
        assert_eq!(rgb_to_ansi16([128, 0, 0]), 1);
        assert_eq!(rgb_to_ansi16([0, 128, 0]), 2);
        assert_eq!(rgb_to_ansi16([0, 0, 255]), 12);
        assert_eq!(rgb_to_ansi16([192, 192, 192]), 7);
    }

    #[test]
    fn ansi16_dithered_valid_and_spread() {
        for y in 0..8usize {
            for x in 0..8usize {
                let idx = rgb_to_ansi16_dithered([128, 128, 128], x, y);
                assert!(idx <= 15);
            }
        }
        let mut seen = [false; 16];
        for y in 0..4usize {
            for x in 0..4usize {
                seen[rgb_to_ansi16_dithered([128, 128, 128], x, y) as usize] = true;
            }
        }
        assert!(seen.iter().filter(|s| **s).count() >= 2);
        assert_eq!(rgb_to_ansi16_dithered([255, 0, 0], 1, 0), 9);
        assert_eq!(rgb_to_ansi16_dithered([0, 0, 0], 1, 0), 0);
    }

    #[test]
    fn color_mode_from_str() {
        assert_eq!(ColorMode::from("truecolor"), ColorMode::TrueColor);
        assert_eq!(ColorMode::from("  256 "), ColorMode::Ansi256);
        assert_eq!(ColorMode::from("ansi16"), ColorMode::Ansi16);
        assert_eq!(ColorMode::from("16"), ColorMode::Ansi16);
        assert_eq!(ColorMode::from("mono"), ColorMode::Mono);
        assert_eq!(ColorMode::from("desconocido"), ColorMode::Mono);
    }
}
