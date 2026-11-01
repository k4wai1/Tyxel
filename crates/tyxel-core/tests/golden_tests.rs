//! Golden tests de integración Tyxel: patrones sintéticos + round-trips
//! `.txa` / `.txv` (usando `std::env::temp_dir()` para Termux).

use tyxel_core::atlas;
use tyxel_core::matcher::Matcher;
use tyxel_core::txa::{self, TxaImage};
use tyxel_core::txv::{self, Compression, Op, TxvWriter};
use tyxel_core::tyxel::Tyxel;

use std::path::PathBuf;

fn block_pixels(w8: usize, h16: usize, f: impl Fn(usize, usize) -> [u8; 3]) -> Vec<u8> {
    let mut px = vec![0u8; w8 * h16 * 3];
    for y in 0..h16 {
        for x in 0..w8 {
            let p = (y * w8 + x) * 3;
            let c = f(x, y);
            px[p] = c[0];
            px[p + 1] = c[1];
            px[p + 2] = c[2];
        }
    }
    px
}

#[test]
fn flat_block_becomes_space() {
    let mut m = Matcher::new();
    let px = block_pixels(8, 16, |_, _| [30, 32, 48]);
    let cells = m.match_frame(1, 1, &px);
    assert_eq!(cells.len(), 1);
    let t = cells[0];
    assert_eq!(t.glyph_id, 0, "bloque plano -> espacio");
    assert_eq!(t.fg, t.bg);
    assert_eq!(t.bg, [30, 32, 48]);
}

#[test]
fn left_right_split_matches_half_block() {
    let mut m = Matcher::new();
    // mitad izq blanca / mitad der negra (aspecto 2:1: la mitad der = ▌ o ▐ invertida)
    let px = block_pixels(8, 16, |x, _| if x < 4 { [255; 3] } else { [0; 3] });
    let cell = m.match_frame(1, 1, &px)[0];
    let mask = atlas::mask(cell.glyph_id);
    let pop = atlas::mask_count(cell.glyph_id);
    assert_eq!(pop, 32, "mitad del bloque");
    // el glifo debe cubrir una mitad respectiva del bloque
    let win = if cell.fg == [255; 3] {
        // glifo blanco a la izquierda o invertido
        true
    } else {
        assert_eq!(cell.fg, [0; 3]);
        false
    };
    for r in 0u32..8 {
        for c in 0u32..8 {
            let px_white = c < 4;
            assert_eq!(
                atlas::mask_is_set(mask, r, c),
                px_white == win,
                "grid ({r},{c})"
            );
        }
    }
}

#[test]
fn checkerboard_selects_dither_glyph() {
    let mut m = Matcher::new();
    let px = block_pixels(8, 16, |x, y| {
        if ((x / 2) % 2) == ((y / 4) % 2) {
            [240; 3]
        } else {
            [10; 3]
        }
    });
    let cell = m.match_frame(1, 1, &px)[0];
    assert_ne!(cell.glyph_id, 0, "textura fina -> glifo visible");
    let pop = atlas::mask_count(cell.glyph_id);
    assert!(
        (8..=56).contains(&pop),
        "densidad intermedia para un tablero binario, got {pop}"
    );
}

#[test]
fn txa_round_trip_and_rle() {
    let mut img = TxaImage::new(16, 4);
    let t = Tyxel {
        glyph_id: atlas::id_of_codepoint('█').unwrap(),
        fg: [200, 10, 10],
        bg: [10, 10, 20],
    };
    for y in 0..4u16 {
        for x in 0..16u16 {
            img.set(x, y, t);
        }
    }
    let txt = txa::to_text(&img);
    let back = txa::from_text(&txt).unwrap();
    assert_eq!(back, img, "round-trip exacto");
    assert!(txt.contains("*16"), "RLE textual activo");
}

#[test]
fn txa_rejects_row_width_mismatch() {
    let mut img = TxaImage::new(3, 1);
    img.set(0, 0, Tyxel::default());
    img.set(1, 0, Tyxel::default());
    img.set(2, 0, Tyxel::default());
    let txt = txa::to_text(&img);
    let broken = txt.replace("#RES 3x1", "#RES 4x1");
    assert!(txa::from_text(&broken).is_err());
}

#[test]
fn txv_round_trip_to_temp_file() {
    let h = txv::TxvHeader {
        width: 8,
        height: 4,
        fps_num: 60,
        fps_den: 1,
        compression: Compression::Lz4,
        ..Default::default()
    };
    let n = h.cells_per_frame();
    // Zigzag movido por frames: I-Frame + 2 P-Frames.
    let f0: Vec<Tyxel> = (0..n)
        .map(|i| Tyxel {
            glyph_id: atlas::id_of_codepoint('▒').unwrap(),
            fg: [i as u8, 0, (i % 7) as u8],
            bg: [0, 0, 20],
        })
        .collect();
    let mut f1 = f0.clone();
    for t in f1.iter_mut() {
        std::mem::swap(&mut t.fg, &mut t.bg);
    }
    let mut path: PathBuf = std::env::temp_dir();
    path.push(format!("tyxel_golden_{:?}.txv", std::process::id()));
    {
        let file = std::fs::File::create(&path).expect("crear .txv temporal");
        let mut w = TxvWriter::new(file, h).unwrap();
        w.push_iframe(&f0, 16).unwrap();
        w.push_pframe(&f0, &f1, 16).unwrap();
        let (frames, _index) = w.finish().unwrap();
        assert_eq!(frames, 2);
    }
    let data = std::fs::read(&path).unwrap();
    let doc = txv::Txv::parse(&data).unwrap();
    assert_eq!(doc.frames[0].cells, f0, "I-Frame decodificado");
    assert_eq!(doc.frames[1].cells, f1, "P-Frame aplicado sobre I");
    assert_eq!(doc.frames[0].duration_ms, 16);
    assert_eq!(data[data.len() - 8..], txv::TXV_END_MAGIC);
    std::fs::remove_file(&path).ok();
}

#[test]
fn txv_op_kinds_sane() {
    let a = vec![
        Tyxel {
            glyph_id: 0,
            fg: [1, 2, 3],
            bg: [4, 5, 6],
        },
        Tyxel {
            glyph_id: 0,
            fg: [1, 2, 3],
            bg: [4, 5, 6],
        },
        Tyxel {
            glyph_id: 3,
            fg: [9, 9, 9],
            bg: [0, 0, 0],
        },
        Tyxel {
            glyph_id: 7,
            fg: [1, 1, 1],
            bg: [2, 2, 2],
        },
    ];
    let b = vec![
        a[0],
        a[1],
        Tyxel {
            glyph_id: 7,
            fg: [1, 1, 1],
            bg: [2, 2, 2],
        },
        Tyxel {
            glyph_id: 7,
            fg: [1, 1, 1],
            bg: [2, 2, 2],
        },
    ];
    let ops = txv::delta_ops(&a, &b);
    let rebuilt = txv::apply_frame(&a, &ops);
    assert_eq!(rebuilt, b);
    assert!(ops.contains(&Op::Skip(2)));
    assert!(ops.iter().any(|o| matches!(o, Op::Repeat(2, _))));
}

#[test]
fn txa_virtual_reconstruction_follows_glyph() {
    let mut img = TxaImage::new(1, 1);
    let id = atlas::id_of_codepoint('▌').unwrap(); // mitad izquierda
    img.set(
        0,
        0,
        Tyxel {
            glyph_id: id,
            fg: [10, 10, 10],
            bg: [250, 250, 250],
        },
    );
    let px = txa::virtual_pixels(&img);
    assert_eq!(px.len(), 8 * 16 * 3, "(w*8)x(h*16) RGB24");
    let p_top_left = px.as_slice().get(0..3).unwrap().to_vec();
    assert_eq!(p_top_left, [10, 10, 10], "fg en la mitad izquierda");
    let p_right = px
        .as_slice()
        .get((4 + 8) * 3..(4 + 8) * 3 + 3)
        .unwrap()
        .to_vec();
    assert_eq!(p_right, [250, 250, 250], "bg en la mitad derecha");
}
