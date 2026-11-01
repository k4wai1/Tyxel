//! Atlas Lógico v1: 512 glifos con máscaras 8×8 en `u64` y codepoints de ancho 1.
//!
//! Endianness de la matriz: bit = (row * 8) + col; la fila 0 es la superior y
//! ocupa los 8 bits menos significativos (ARCHITECTURE.md §2).

#[cfg(test)]
use unicode_width::UnicodeWidthChar;

pub const ATLAS_LEN: usize = 512;

#[inline]
pub const fn mask_bit(row: u32, col: u32) -> u64 {
    1u64 << (row * 8 + col)
}

#[inline]
pub const fn mask_is_set(mask: u64, row: u32, col: u32) -> bool {
    (mask >> (row * 8 + col)) & 1 == 1
}

#[inline]
pub const fn mask(id: u16) -> u64 {
    MASKS[id as usize]
}

#[inline]
pub const fn codepoint(id: u16) -> char {
    CODEPOINTS[id as usize]
}

#[inline]
pub const fn mask_count(id: u16) -> u32 {
    MASKS[id as usize].count_ones()
}

pub fn id_of_codepoint(c: char) -> Option<u16> {
    for (i, cp) in CODEPOINTS.iter().enumerate() {
        if *cp == c {
            return Some(i as u16);
        }
    }
    None
}

// ===== Ayudas de construcción (const fn) =====

const fn fill(m: u64, r0: u32, r1: u32, c0: u32, c1: u32) -> u64 {
    let mut out = m;
    let mut r = r0;
    while r < r1 {
        let mut c = c0;
        while c < c1 {
            out |= 1u64 << (r * 8 + c);
            c += 1;
        }
        r += 1;
    }
    out
}

const fn bit(r: u32, c: u32) -> u64 {
    1u64 << (r * 8 + c)
}

// Tallos desde el centro. w: 0 = luz (2 bits) / 1 = negro (4 bits).
const fn stem_n(w: u32) -> u64 {
    if w == 0 {
        fill(0, 0, 4, 3, 5)
    } else {
        fill(0, 0, 4, 2, 6)
    }
}
const fn stem_s(w: u32) -> u64 {
    if w == 0 {
        fill(0, 4, 8, 3, 5)
    } else {
        fill(0, 4, 8, 2, 6)
    }
}
const fn stem_e(w: u32) -> u64 {
    if w == 0 {
        fill(0, 3, 5, 4, 8)
    } else {
        fill(0, 2, 6, 4, 8)
    }
}
const fn stem_w(w: u32) -> u64 {
    if w == 0 {
        fill(0, 3, 5, 0, 4)
    } else {
        fill(0, 2, 6, 0, 4)
    }
}
// Cuartos diagonales desde el centro hacia cada esquina (pendiente exacta 1:1).
const fn stub_dr() -> u64 {
    bit(4, 4) | bit(5, 5) | bit(6, 6) | bit(7, 7)
}
const fn stub_ur() -> u64 {
    bit(3, 4) | bit(2, 5) | bit(1, 6) | bit(0, 7)
}
const fn stub_dl() -> u64 {
    bit(4, 3) | bit(5, 2) | bit(6, 1) | bit(7, 0)
}
const fn stub_ul() -> u64 {
    bit(3, 3) | bit(2, 2) | bit(1, 1) | bit(0, 0)
}

// Tallos dobles (dos líneas de 2 bits desplazadas alrededor del centro).
const fn stem_n_d() -> u64 {
    fill(0, 0, 4, 1, 3) | fill(0, 0, 4, 5, 7)
}
const fn stem_s_d() -> u64 {
    fill(0, 4, 8, 1, 3) | fill(0, 4, 8, 5, 7)
}
const fn stem_e_d() -> u64 {
    fill(0, 1, 3, 4, 8) | fill(0, 5, 7, 4, 8)
}
const fn stem_w_d() -> u64 {
    fill(0, 1, 3, 0, 4) | fill(0, 5, 7, 0, 4)
}

const fn diag_ur_bl() -> u64 {
    // ╱ : r + c == 7

    let mut m = 0u64;
    let mut k = 0u32;
    while k < 8 {
        m |= bit(k, 7 - k);
        k += 1;
    }
    m
}
const fn diag_ul_br() -> u64 {
    // ╲ : r == c

    let mut m = 0u64;
    let mut k = 0u32;
    while k < 8 {
        m |= bit(k, k);
        k += 1;
    }
    m
}

// Líneas discontinuas. kind: 0 = triple, 1 = doble, 2 = central.
const fn dash_h(heavy: bool, kind: u32) -> u64 {
    let (r0, r1) = if heavy { (2u32, 6u32) } else { (3u32, 5u32) };
    let base = fill(0, r0, r1, 0, 8);
    match kind {
        0 => base & !fill(0, r0, r1, 2, 3) & !fill(0, r0, r1, 5, 6),
        1 => base & !fill(0, r0, r1, 3, 4) & !fill(0, r0, r1, 7, 8),
        _ => base & !fill(0, r0, r1, 0, 2) & !fill(0, r0, r1, 6, 8),
    }
}
const fn dash_v(heavy: bool, kind: u32) -> u64 {
    let (c0, c1) = if heavy { (2u32, 6u32) } else { (3u32, 5u32) };
    let base = fill(0, 0, 8, c0, c1);
    match kind {
        0 => base & !fill(0, 2, 3, c0, c1) & !fill(0, 5, 6, c0, c1),
        1 => base & !fill(0, 3, 4, c0, c1) & !fill(0, 7, 8, c0, c1),
        _ => base & !fill(0, 0, 2, c0, c1) & !fill(0, 6, 8, c0, c1),
    }
}

// Líneas dobles discontinuas (254C-254F): edificios Edouble/Vdouble con huecos.
const fn dash2_h(kind: u32) -> u64 {
    let base = fill(0, 1, 3, 0, 8) | fill(0, 5, 7, 0, 8);
    match kind {
        0 => {
            base & !fill(0, 1, 3, 2, 3)
                & !fill(0, 5, 7, 2, 3)
                & !fill(0, 1, 3, 5, 6)
                & !fill(0, 5, 7, 5, 6)
        }
        _ => base & !fill(0, 1, 3, 3, 5) & !fill(0, 5, 7, 3, 5),
    }
}
const fn dash2_v(kind: u32) -> u64 {
    let base = fill(0, 0, 8, 1, 3) | fill(0, 0, 8, 5, 7);
    match kind {
        0 => {
            base & !fill(0, 2, 3, 1, 3)
                & !fill(0, 5, 6, 1, 3)
                & !fill(0, 2, 3, 5, 7)
                & !fill(0, 5, 6, 5, 7)
        }
        _ => base & !fill(0, 3, 5, 1, 3) & !fill(0, 3, 5, 5, 7),
    }
}

// Esquina redondeada: tallo D/R (o N/L) con cuadrado interior sustituido por bits diagonales.
const fn round_corner(v: u64) -> u64 {
    let inner = fill(0, 4, 6, 4, 6);
    (v & !inner) | bit(4, 5) | bit(5, 6)
}

const fn box_mask(o: u32) -> u64 {
    match o {
        // Líneas completas
        0 => fill(0, 3, 5, 0, 8), // ─
        1 => fill(0, 2, 6, 0, 8), // ━
        2 => fill(0, 0, 8, 3, 5), // │
        3 => fill(0, 0, 8, 2, 6), // ┃
        // Discontinuas (interpretación procedural)
        4 => dash_h(false, 0),   // ┄
        5 => dash_h(true, 0),    // ┅
        6 => dash_v(false, 0),   // ┆
        7 => dash_v(true, 0),    // ┇
        8 => dash_h(false, 1),   // ┈
        9 => dash_h(true, 1),    // ┉
        0xA => dash_v(false, 1), // ┊
        0xB => dash_v(true, 1),  // ┋
        // Esquinas ┌┐└┘ y pesos mezclados (semántica estándar)
        0xC => stem_s(0) | stem_e(0),  // ┌
        0xD => stem_s(0) | stem_e(1),  // ┍
        0xE => stem_s(1) | stem_e(0),  // ┎
        0xF => stem_s(1) | stem_e(1),  // ┏
        0x10 => stem_s(0) | stem_w(0), // ┐
        0x11 => stem_s(0) | stem_w(1), // ┑
        0x12 => stem_s(1) | stem_w(0), // ┒
        0x13 => stem_s(1) | stem_w(1), // ┓
        0x14 => stem_n(0) | stem_e(0), // └
        0x15 => stem_n(0) | stem_e(1), // ┕
        0x16 => stem_n(1) | stem_e(0), // ┖
        0x17 => stem_n(1) | stem_e(1), // ┗
        0x18 => stem_n(0) | stem_w(0), // ┘
        0x19 => stem_n(0) | stem_w(1), // ┙
        0x1A => stem_n(1) | stem_w(0), // ┚
        0x1B => stem_n(1) | stem_w(1), // ┛
        // Tees verticales/horizontales con pesos (interpretación procedural)
        0x1C => stem_n(0) | stem_s(0) | stem_e(0),  // ├
        0x1D => stem_n(1) | stem_s(1) | stem_e(0),  // ┝
        0x1E => stem_n(0) | stem_s(0) | stem_e(1),  // ┞
        0x1F => stem_n(1) | stem_s(1) | stem_e(1),  // ┣
        0x20 => stem_n(0) | stem_s(0) | stem_e_d(), // ┠
        0x21 => stem_n(1) | stem_s(1) | stem_e_d(), // ┡
        0x22 => stem_n(0) | stem_s(0) | stem_e_d() | stem_w(1), // ┢
        0x23 => stem_n(1) | stem_s(1) | stem_e_d() | stem_w(1), // ┣ doble
        0x24 => stem_n(0) | stem_s(0) | stem_w(0),  // ┤
        0x25 => stem_n(1) | stem_s(1) | stem_w(0),  // ┥
        0x26 => stem_n(0) | stem_s(0) | stem_w(1),  // ┦
        0x27 => stem_n(1) | stem_s(1) | stem_w(1),  // ┧
        // Tees S con cuartos diagonales (interpretación procedural de ┨┩┪┫)
        0x28 => stem_e(0) | stub_dr(),
        0x29 => stem_e(0) | stub_ur(),
        0x2A => stem_e(0) | stub_dl(),
        0x2B => stem_e(0) | stub_ul(),
        0x2C => stem_s(0) | stem_e(0) | stem_w(0), // ┬
        0x2D => stem_s(0) | stem_e(0) | stem_w(1), // ┭
        0x2E => stem_s(0) | stem_e(1) | stem_w(0), // ┮
        0x2F => stem_s(0) | stem_e(1) | stem_w(1), // ┯
        0x30 => stem_s(1) | stem_e(0) | stem_w(0), // ┰
        0x31 => stem_s(1) | stem_e(0) | stem_w(1), // ┱
        0x32 => stem_s(1) | stem_e(1) | stem_w(0), // ┲
        0x33 => stem_s(1) | stem_e(1) | stem_w(1), // ┳
        0x34 => stem_n(0) | stem_e(0) | stem_w(0), // ┴
        0x35 => stem_n(0) | stem_e(0) | stem_w(1), // ┵
        0x36 => stem_n(0) | stem_e(1) | stem_w(0), // ┶
        0x37 => stem_n(0) | stem_e(1) | stem_w(1), // ┷
        0x38 => stem_n(1) | stem_e(0) | stem_w(0), // ┸
        0x39 => stem_n(1) | stem_e(0) | stem_w(1), // ┹
        0x3A => stem_n(1) | stem_e(1) | stem_w(0), // ┺
        0x3B => stem_n(1) | stem_e(1) | stem_w(1), // ┻
        // Cruces ▒▒ (16 pesos)
        0x3C => stem_n(0) | stem_s(0) | stem_e(0) | stem_w(0), // ┼
        0x3D => stem_n(0) | stem_s(0) | stem_e(0) | stem_w(1),
        0x3E => stem_n(0) | stem_s(0) | stem_e(1) | stem_w(0),
        0x3F => stem_n(0) | stem_s(0) | stem_e(1) | stem_w(1),
        0x40 => stem_n(0) | stem_s(1) | stem_e(0) | stem_w(0),
        0x41 => stem_n(1) | stem_s(0) | stem_e(0) | stem_w(0),
        0x42 => stem_n(0) | stem_s(1) | stem_e(1) | stem_w(0),
        0x43 => stem_n(1) | stem_s(0) | stem_e(1) | stem_w(1),
        0x44 => stem_n(1) | stem_s(1) | stem_e(0) | stem_w(0),
        0x45 => stem_n(0) | stem_s(1) | stem_e(0) | stem_w(1),
        0x46 => stem_n(0) | stem_s(1) | stem_e(1) | stem_w(1),
        0x47 => stem_n(1) | stem_s(0) | stem_e(0) | stem_w(1),
        0x48 => stem_n(1) | stem_s(0) | stem_e(1) | stem_w(0),
        0x49 => stem_n(1) | stem_s(1) | stem_e(1) | stem_w(0),
        0x4A => stem_n(1) | stem_s(1) | stem_e(0) | stem_w(1),
        0x4B => stem_n(1) | stem_s(1) | stem_e(1) | stem_w(1), // ╋
        // Discontinuas dobles (254C-254F)
        0x4C => dash2_h(0),
        0x4D => dash2_h(1),
        0x4E => dash2_v(0),
        0x4F => dash2_v(1),
        // Doble muros (2550-256C)
        0x50 => stem_e_d() | stem_w_d(),              // ═
        0x51 => stem_n_d() | stem_s_d(),              // ║
        0x52 => stem_s(0) | stem_e_d(),               // ╒
        0x53 => stem_s_d() | stem_e(0),               // ╓
        0x54 => stem_s_d() | stem_e_d(),              // ╔
        0x55 => stem_s(0) | stem_w_d(),               // ╕
        0x56 => stem_s_d() | stem_w(0),               // ╖
        0x57 => stem_s_d() | stem_w_d(),              // ╗
        0x58 => stem_n(0) | stem_e_d(),               // ╘
        0x59 => stem_n_d() | stem_e(0),               // ╙
        0x5A => stem_n_d() | stem_e_d(),              // ╚
        0x5B => stem_n(0) | stem_w_d(),               // ╛
        0x5C => stem_n_d() | stem_w(0),               // ╜
        0x5D => stem_n_d() | stem_w_d(),              // ╝
        0x5E => stem_n_d() | stem_s_d() | stem_e(0),  // ╞
        0x5F => stem_n_d() | stem_s_d() | stem_w_d(), // ╟
        0x60 => stem_n_d() | stem_s_d() | stem_e_d() | stem_w(0), // ╠
        0x61 => stem_n_d() | stem_s_d() | stem_e(0) | stem_w(0), // ╡
        0x62 => stem_n_d() | stem_s_d() | stem_e_d(), // ╢
        0x63 => stem_n_d() | stem_s_d() | stem_e_d() | stem_w_d(), // ╣
        0x64 => stem_s_d() | stem_e(0) | stem_w(0),   // ╤
        0x65 => stem_s_d() | stem_e_d() | stem_w(0),  // ╥
        0x66 => stem_s_d() | stem_e_d() | stem_w_d(), // ╦
        0x67 => stem_n(0) | stem_e_d() | stem_w_d(),  // ╧
        0x68 => stem_n_d() | stem_e(0) | stem_w(0),   // ╨
        0x69 => stem_n_d() | stem_e_d() | stem_w(0),  // ╩
        0x6A => stem_n(0) | stem_s_d() | stem_e(0) | stem_w(0), // ╪
        0x6B => stem_n_d() | stem_s_d() | stem_e(0) | stem_w_d(), // ╫
        0x6C => stem_n_d() | stem_s_d() | stem_e_d() | stem_w_d() | stem_e(1) | stem_w(1), // ╬
        // Esquinas redondeadas (256D-2570)
        0x6D => round_corner(stem_s(0) | stem_e(0)), // ╭
        0x6E => round_corner(stem_s(0) | stem_w(0)), // ╮
        0x6F => round_corner(stem_n(0) | stem_w(0)), // ╯
        0x70 => round_corner(stem_n(0) | stem_e(0)), // ╰
        // Diagonales (2571-2573)
        0x71 => diag_ur_bl(),                // ╱
        0x72 => diag_ul_br(),                // ╲
        0x73 => diag_ur_bl() | diag_ul_br(), // ╳
        // Medias líneas ligeras (2574-2577)
        0x74 => stem_w(0), // ╴
        0x75 => stem_n(0), // ╵
        0x76 => stem_e(0), // ╶
        0x77 => stem_s(0), // ╷
        // Medias líneas negras (2578-257B)
        0x78 => stem_e(1), // ╸
        0x79 => stem_n(1), // ╹
        0x7A => stem_w(1), // ╺
        0x7B => stem_s(1), // ╻
        // Medias líneas mixtas (257C-257F)
        0x7C => stem_w(1) | stem_e(0), // ╾
        0x7D => stem_n(1) | stem_s(0), // ╿
        0x7E => stem_n(0) | stem_s(1),
        0x7F => stem_w(0) | stem_e(1),
        _ => 0,
    }
}

// ===== Block Elements U+2580-U+259F (sin 2588) =====

const fn q_ul() -> u64 {
    fill(0, 0, 4, 0, 4)
}
const fn q_ur() -> u64 {
    fill(0, 0, 4, 4, 8)
}
const fn q_ll() -> u64 {
    fill(0, 4, 8, 0, 4)
}
const fn q_lr() -> u64 {
    fill(0, 4, 8, 4, 8)
}

const fn shade(density: u32) -> u64 {
    let mut m = 0u64;
    let mut r = 0u32;
    while r < 8 {
        let mut c = 0u32;
        while c < 8 {
            let s = (r + c) % 4;
            let on = match density {
                1 => s == 0,
                2 => (r + c) % 2 == 0,
                _ => s < 3,
            };
            if on {
                m |= bit(r, c);
            }
            c += 1;
        }
        r += 1;
    }
    m
}

const fn block_mask(o: u32) -> u64 {
    match o {
        0 => fill(0, 0, 4, 0, 8),              // ▀ mitad superior
        1..=7 => fill(0, 8 - o, 8, 0, 8),      // ▁..▇ tramos bajos
        9..=0xF => fill(0, 0, 8, 0, 0x10 - o), // ▉..▏ octantes izquierdos
        0x10 => fill(0, 0, 8, 4, 8),           // ▐ mitad derecha
        0x11 => shade(1),                      // ░
        0x12 => shade(2),                      // ▒
        0x13 => shade(3),                      // ▓
        0x14 => fill(0, 0, 1, 0, 8),           // ▊?? octavo superior
        0x15 => fill(0, 0, 8, 7, 8),           // ▕ octavo derecho
        // Cuadrantes 2596-259F: [UL, UR, LL, LR]
        0x16 => q_ll(),
        0x17 => q_lr(),
        0x18 => q_ul(),
        0x19 => q_ul() | q_ll() | q_lr(),
        0x1A => q_ul() | q_lr(),
        0x1B => q_ul() | q_ur() | q_ll(),
        0x1C => q_ul() | q_ur() | q_lr(),
        0x1D => q_ur(),
        0x1E => q_ur() | q_ll(),
        0x1F => q_ur() | q_ll() | q_lr(),
        _ => 0,
    }
}

// ===== Triángulos U+25E2-U+25E5 (hipotenusa exacta) =====

const fn tri_mask(o: u32) -> u64 {
    let mut m = 0u64;
    let mut r = 0u32;
    while r < 8 {
        let mut c = 0u32;
        while c < 8 {
            let on = match o {
                0 => (r as i32 + c as i32) >= 7, // ◢ inferior derecha
                1 => c <= r,                     // ◣ inferior izquierda
                2 => (r as i32 + c as i32) <= 7, // ◤ superior izquierda
                _ => c >= r,                     // ◥ superior derecha
            };
            if on {
                m |= bit(r, c);
            }
            c += 1;
        }
        r += 1;
    }
    m
}

// Rombo (25C6, extra): |2r-7| + |2c-7| <= 8 desde el centro.
const fn diamond_mask() -> u64 {
    let mut m = 0u64;
    let mut r = 0u32;
    while r < 8 {
        let mut c = 0u32;
        while c < 8 {
            let dr = (2 * r as i32 - 7).abs();
            let dc = (2 * c as i32 - 7).abs();
            if dr + dc <= 8 {
                m |= bit(r, c);
            }
            c += 1;
        }
        r += 1;
    }
    m
}

// ===== Círculos U+25D0-U+25DE (discos/anillos a distancia en 1/2 px) =====

const fn d2(r: u32, c: u32) -> i32 {
    let dr = r as i32 * 2 - 7;
    let dc = c as i32 * 2 - 7;
    dr * dr + dc * dc
}

const fn circle_mask(o: u32) -> u64 {
    let mut m = 0u64;
    let mut r = 0u32;
    while r < 8 {
        let mut c = 0u32;
        while c < 8 {
            let dd = d2(r, c);
            let disc = dd <= 52;
            let on = match o {
                0 => disc && c < 4,
                1 => disc && c >= 4,
                2 => disc && r >= 4,
                3 => disc && r < 4,
                4 => disc && r < 4 && c >= 4,
                5 => disc && r >= 4 && c < 4,
                6 => dd >= 40 && dd <= 52 && c < 4,
                7 => dd >= 40 && dd <= 52 && c >= 4,
                8 => disc && dd > 16,
                // 25D9: círculo inverso (anillo + punto central 2×2)
                9 => (dd >= 16 && dd <= 52) || (r == 3 || r == 4) && (c == 3 || c == 4),
                10 => disc && dd > 16 && r < 4,
                11 => disc && dd > 16 && r >= 4,
                12 => dd >= 40 && dd <= 52 && r < 4 && c < 4,
                13 => dd >= 40 && dd <= 52 && r >= 4 && c >= 4,
                14 => disc && dd > 16 && r < 4 && c >= 4,
                _ => disc && dd > 16 && r >= 4 && c < 4,
            };
            if on {
                m |= bit(r, c);
            }
            c += 1;
        }
        r += 1;
    }
    m
}

// ===== Powerline U+E0B0-U+E0BF (cuñas y escaleras procedurales) =====

const LEAD: [u32; 8] = [0, 2, 4, 6, 6, 4, 2, 0];
const STAIR: [u32; 8] = [1, 2, 3, 3, 4, 5, 6, 6];

const fn crown_mask(o: u32) -> u64 {
    let mut m = 0u64;
    let mut r = 0u32;
    while r < 8 {
        let l = LEAD[r as usize];
        let l2 = if l > 0 { l - 1 } else { 0 };
        let s = STAIR[r as usize];
        let mut c = 0u32;
        while c < 8 {
            let on = match o {
                0 => c <= l,
                1 => c >= l + 1,
                2 => c >= 7 - l,
                3 => c <= 6 - l,
                4 => c <= l2,
                5 => c >= l2 + 2,
                6 => c >= 8 - l,
                7 => c <= 7 - l,
                8 => c <= s,
                9 => c >= 7 - s,
                0xA => c >= s + 2,
                0xB => (c as i32) <= 5 - s as i32,
                0xC => c <= s + 2,
                0xD => c >= s + 1,
                0xE => c >= 8 - s,
                _ => c <= 6 - s,
            };
            if on {
                m |= bit(r, c);
            }
            c += 1;
        }
        r += 1;
    }
    m
}

// ===== Braille U+2801-U+28FE (punto = bloque 4×2 central, margen garantizado) =====
// ⣿ (U+28FF, 8 puntos): coincide con ║ doble y se excluye por duplicado de máscara.

const DOT_POS: [(u32, u32); 8] = [
    (0u32, 0u32), // dot1
    (1, 0),       // dot2
    (2, 0),       // dot3
    (0, 1),       // dot4
    (1, 1),       // dot5
    (2, 1),       // dot6
    (3, 0),       // dot7
    (3, 1),       // dot8
];

const fn braille_mask(d: u32) -> u64 {
    let mut m = 0u64;
    let mut k = 0u32;
    while k < 8 {
        if (d >> k) & 1 == 1 {
            let (dr, dc) = DOT_POS[k as usize];
            m |= fill(0, dr * 2, dr * 2 + 2, dc * 4 + 1, dc * 4 + 3);
        }
        k += 1;
    }
    m
}

// ===== Sextantes U+1FB00-U+1FB3B (orden estándar: cp-1FB00+1 = máscara de 6 bits) =====
// bandas verticales [3, 2, 3]; celdas: bit0-1 tanda sup, bit2-3 tanda media, bit4-5 inferior.
// Excluidos: v=0 (vacío), v=60/61 (≲ no en el estándar), v=63 (U+2588), y los 4 que
// duplican máscaras (v=3 ▀, v=21 ▌, v=42 ▐, v=48 ▃).

const fn sext_mask(v: u32) -> u64 {
    let mut m = 0u64;
    let mut k = 0u32;
    while k < 6 {
        if (v >> k) & 1 == 1 {
            let r0 = match k / 2 {
                0 => 0u32,
                1 => 3,
                _ => 5,
            };
            let r1 = match k / 2 {
                0 => 3u32,
                1 => 5,
                _ => 8,
            };
            let (c0, c1) = if k % 2 == 0 {
                (0u32, 4u32)
            } else {
                (4u32, 8u32)
            };
            m |= fill(0, r0, r1, c0, c1);
        }
        k += 1;
    }
    m
}

const fn sextant_cp_mask(v: u32) -> Option<(u32, u64)> {
    // Devuelve (cp, mask) o None si v queda excluido.
    // v=4/8 (banda media izq/der) duplican ╴/╶; v=3/21/42/48 duplican ▀▌▐▃; v=12 duplica ─.
    match v {
        0 | 3 | 4 | 8 | 12 | 21 | 42 | 48 | 60 | 61 | 63 => None,
        _ => Some((0x1FB00 + v - 1, sext_mask(v))),
    }
}

// Bandas laterales Derechos de octantes (1FB78..1FB7C, sin duplicados existentes).
const fn right_oct_mask(n: u32) -> u64 {
    // n = 2, 3, 5 barras de columnas a la derecha; 6 = col 7 + fila 7.
    match n {
        2 => fill(0, 0, 8, 6, 8),
        3 => fill(0, 0, 8, 5, 8),
        5 => fill(0, 0, 8, 3, 8),
        _ => fill(0, 0, 8, 7, 8) | fill(0, 7, 8, 0, 8),
    }
}

// ===== Ensamblador =====

const fn build_pair() -> ([u64; 512], [char; 512]) {
    let mut masks = [0u64; 512];
    let mut cps = ['\0'; 512];
    let mut i = 0usize;

    // 0: espacio, 1: bloque completo
    masks[i] = 0;
    cps[i] = '\u{0020}';
    i += 1;
    masks[i] = u64::MAX;
    cps[i] = '\u{2588}';
    i += 1;

    // Box Drawing: 128 (IDs 2..=129)
    let mut o = 0u32;
    while o < 128 {
        masks[i] = box_mask(o);
        cps[i] = char::from_u32(0x2500u32 + o).unwrap();
        i += 1;
        o += 1;
    }

    // Block Elements: 31 (IDs 130..=160), sin U+2588
    let mut o = 0u32;
    while o < 32 {
        if o != 8 {
            masks[i] = block_mask(o);
            cps[i] = char::from_u32(0x2580u32 + o).unwrap();
            i += 1;
        }
        o += 1;
    }

    // Triángulos: 4 (IDs 161..=164)
    let mut o = 0u32;
    while o < 4 {
        masks[i] = tri_mask(o);
        cps[i] = char::from_u32(0x25E2u32 + o).unwrap();
        i += 1;
        o += 1;
    }

    // Powerline: 16 (IDs 165..=180)
    let mut o = 0u32;
    while o < 16 {
        masks[i] = crown_mask(o);
        cps[i] = char::from_u32(0xE0B0u32 + o).unwrap();
        i += 1;
        o += 1;
    }

    // Círculos: 15 (IDs 181..=195, U+25D0..U+25DE)
    let mut o = 0u32;
    while o < 15 {
        masks[i] = circle_mask(o);
        cps[i] = char::from_u32(0x25D0u32 + o).unwrap();
        i += 1;
        o += 1;
    }

    // Extras: 5 (IDs 196..=200): 25E6, 25AA, 25E0, 25E1, 25C6
    masks[i] = small_ring();
    cps[i] = '\u{25E6}';
    i += 1;
    masks[i] = fill(0, 2, 6, 2, 6);
    cps[i] = '\u{25AA}';
    i += 1;
    masks[i] = ring_half(false);
    cps[i] = '\u{25E0}';
    i += 1;
    masks[i] = ring_half(true);
    cps[i] = '\u{25E1}';
    i += 1;
    masks[i] = diamond_mask();
    cps[i] = '\u{25C6}';
    i += 1;

    // Braille: 254 (IDs 201..=454, U+2801..U+28FE)
    let mut d = 1u32;
    while d <= 254 {
        masks[i] = braille_mask(d);
        cps[i] = char::from_u32(0x2800u32 + d).unwrap();
        i += 1;
        d += 1;
    }

    // Sextantes: 53 asignaciones (IDs 455..=507)
    let mut v = 1u32;
    while v <= 63 {
        if let Some((cp, m)) = sextant_cp_mask(v) {
            masks[i] = m;
            cps[i] = char::from_u32(cp).unwrap();
            i += 1;
        }
        v += 1;
    }

    // Octantes derechos heredados: 4 (IDs 508..=511): 1FB78, 1FB79, 1FB7B, 1FB7C
    masks[i] = right_oct_mask(2);
    cps[i] = '\u{1FB78}';
    i += 1;
    masks[i] = right_oct_mask(3);
    cps[i] = '\u{1FB79}';
    i += 1;
    masks[i] = right_oct_mask(5);
    cps[i] = '\u{1FB7B}';
    i += 1;
    masks[i] = right_oct_mask(6);
    cps[i] = '\u{1FB7C}';

    (masks, cps)
}

// Anillo grande por mitades (25E0): mitad superior.
const fn ring_half(lower: bool) -> u64 {
    let mut m = 0u64;
    let mut r = 0u32;
    while r < 8 {
        let mut c = 0u32;
        while c < 8 {
            let dd = d2(r, c);
            if dd >= 40 && dd <= 52 && (if lower { r >= 4 } else { r < 4 }) {
                m |= bit(r, c);
            }
            c += 1;
        }
        r += 1;
    }
    m
}

// Pequeño anillo (25E6): bulle blanco (d2 4..=18).
const fn small_ring() -> u64 {
    let mut m = 0u64;
    let mut r = 0u32;
    while r < 8 {
        let mut c = 0u32;
        while c < 8 {
            let dd = d2(r, c);
            if dd >= 4 && dd <= 18 {
                m |= bit(r, c);
            }
            c += 1;
        }
        r += 1;
    }
    m
}

const PAIR: ([u64; 512], [char; 512]) = build_pair();

pub const MASKS: [u64; 512] = PAIR.0;
pub const CODEPOINTS: [char; 512] = PAIR.1;

pub const NERDFONT_COUNT: usize = 16; // U+E0B0..U+E0BF (Powerline)

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints() {
        assert_eq!(MASKS[0], 0);
        assert_eq!(CODEPOINTS[0], ' ');
        assert_eq!(MASKS[1], u64::MAX);
        assert_eq!(CODEPOINTS[1], '\u{2588}');
    }

    #[test]
    fn no_invisible_nor_full() {
        for i in 2..512 {
            assert!(MASKS[i] != 0, "máscara vacía en id {i}");
            assert!(MASKS[i] != u64::MAX, "máscara completa en id {i}");
        }
    }

    #[test]
    fn unique_masks() {
        for a in 0..512 {
            for b in (a + 1)..512 {
                assert!(MASKS[a] != MASKS[b], "máscaras duplicadas {a} y {b}");
            }
        }
    }

    #[test]
    fn width_one() {
        for c in CODEPOINTS {
            assert_eq!(
                c.width(),
                Some(1),
                "codepoint {c:?} (U+{:04X}) no tiene ancho 1",
                c as u32
            );
        }
    }

    #[test]
    fn no_placeholder() {
        for (i, c) in CODEPOINTS.iter().enumerate() {
            assert!(*c != '\0', "codepoint sin asignar en id {i}");
        }
    }

    #[test]
    fn helpers_consistent() {
        for id in 0..512u16 {
            assert_eq!(mask(id), MASKS[id as usize]);
            assert_eq!(mask_count(id), MASKS[id as usize].count_ones());
        }
        assert_eq!(codepoint(0), ' ');
        assert_eq!(id_of_codepoint('\u{2588}'), Some(1));
        assert_eq!(id_of_codepoint(' '), Some(0));
    }

    #[test]
    fn mask_bit_helpers() {
        assert_eq!(mask_bit(0, 0), 1);
        assert_eq!(mask_bit(7, 7), 1u64 << 63);
        assert!(mask_is_set(1u64 << 63, 7, 7));
        assert!(!mask_is_set(1u64 << 63, 0, 0));
    }

    #[test]
    fn braille_margin_cols() {
        // El braille solo enciende columnas 1, 2, 5, 6 con d3 de 2 filas.
        const COL_MASK: u64 = fill(0, 0, 8, 1, 3) | fill(0, 0, 8, 5, 7);

        for id in 201..455usize {
            let m = MASKS[id];
            assert_eq!(m & !COL_MASK, 0, "id {id} rompe el margen braille");
        }
    }

    #[test]
    fn braille_popcounts() {
        for id in 201..455usize {
            let pc = MASKS[id].count_ones();
            assert!(pc % 4 == 0, "id {id} densidad braille irregular");
            assert!(pc > 0, "id {id} braille vacío");
        }
    }

    #[test]
    fn sextant_bands() {
        // Cada máscara sextante es unión de bandas de filas [0..3), [3..5), [5..8)
        // y columnas [0..4) / [4..8).
        for id in 455..508usize {
            let m = MASKS[id];
            // Toda banda usada debe estar completa en sus filas.
            let bands = [
                fill(0, 0, 3, 0, 8),
                fill(0, 3, 5, 0, 8),
                fill(0, 5, 8, 0, 8),
            ];
            for (bi, band) in bands.iter().enumerate() {
                let usage = m & band;
                let (r0, r1) = match bi {
                    0 => (0u32, 3u32),
                    1 => (3, 5),
                    _ => (5, 8),
                };
                let full_l = fill(0, r0, r1, 0, 4);
                let full_r = fill(0, r0, r1, 4, 8);
                let ok = usage == 0 || usage == full_l || usage == full_r || usage == *band;
                assert!(ok, "banda irregular en sextante id {id}");
            }
        }
    }

    #[test]
    fn atlas_census() {
        let mut counts = [0usize; 9];
        for c in CODEPOINTS {
            let u = c as u32;
            let f = if u == 0x20 || u == 0x2588 {
                0
            } else if (0x2500..0x2580).contains(&u) {
                1
            } else if (0x2580..0x25A0).contains(&u) {
                2
            } else if (0x25E2..=0x25E5).contains(&u) {
                3
            } else if (0xE0B0..=0xE0BF).contains(&u) {
                4
            } else if (0x25D0..=0x25DE).contains(&u) {
                5
            } else if u == 0x25E0 || u == 0x25E1 || u == 0x25E6 || u == 0x25AA {
                6
            } else if (0x2801..=0x28FE).contains(&u) {
                7
            } else {
                8
            };
            counts[f] += 1;
        }
        assert_eq!(counts[0], 2);
        assert_eq!(counts[1], 128);
        assert_eq!(counts[2], 21 + 10); // block elements (sin 2588) + 2590..259F
        assert_eq!(counts[3], 4);
        assert_eq!(counts[4], NERDFONT_COUNT);
        assert_eq!(counts[5], 15);
        assert_eq!(counts[6], 4);
        assert_eq!(counts[7], 254);
        assert_eq!(counts[8], 58);
    }
}
