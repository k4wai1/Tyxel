//! Test E2E de CLI: genera una imagen PNG sintética en memoria, la codifica
//! con `tyxel-enc` y valida `info` / `export` de `tyxel-play`.

use std::process::Command;

fn enc_bin() -> String {
    // En el workspace ambos bins comparten target/: hermano del ejecutable.
    let exe = std::env::current_exe().expect("test exe");
    let dir = exe.parent().unwrap().parent().unwrap(); // deps/ -> debug/
    let p = dir.join("tyxel-enc");
    if !p.exists() {
        // cargo test -p tyxel-play no construye el bin del paquete hermano:
        // hazlo on-demand.
        let st = Command::new(env!("CARGO"))
            .args(["build", "-p", "tyxel-enc", "--color", "never"])
            .output()
            .expect("cargo build -p tyxel-enc");
        assert!(st.status.success(), "build de tyxel-enc falló");
    }
    assert!(p.exists(), "bin tyxel-enc no existe en {}", p.display());
    p.to_str().unwrap().to_string()
}
fn play_bin() -> String {
    let cand = std::env::var("CARGO_BIN_EXE_tyxel-play")
        .or_else(|_| std::env::var("CARGO_BIN_EXE_tyelx_play"))
        .unwrap_or_else(|_| {
            let exe = std::env::current_exe().expect("test exe");
            let dir = exe.parent().unwrap().parent().unwrap();
            let p = dir.join("tyxel-play");
            assert!(
                p.exists(),
                "no encuentro tyxel-play; envars CARGO_BIN no estan"
            );
            p.to_str().unwrap().to_string()
        });
    cand.to_string()
}

fn synth_png(dir: &std::path::Path, tag: &str) -> std::path::PathBuf {
    let w = 128usize;
    let h = 128usize;
    let mut img = image::RgbImage::new(w as u32, h as u32);
    for (x, y, px) in img.enumerate_pixels_mut() {
        let band = (x as usize * 255 / w) as u8;
        let v = ((x + y) % 32 < 16) as u8 * 200;
        *px = image::Rgb([band, v, 255 - band]);
    }
    let path = dir.join(format!("tyxel_e2e_{tag}.png"));
    img.save(&path).expect("guardar PNG");
    path
}

fn run(bin: &str, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(bin).args(args).output().expect("spawn CLI");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn cli_e2e_img_to_info_and_export() {
    let dir = std::env::temp_dir();
    let png = synth_png(&dir, "info");
    let txa_path = dir.join("tyxel_e2e_info.txa");
    let txa_arg = txa_path.to_str().unwrap().to_string();
    let png_arg = png.to_str().unwrap().to_string();
    let _ = std::fs::remove_file(&txa_path);

    // 1. Codificar imagen -> .txa
    let (code, out, err) = run(&enc_bin(), &["img", &png_arg, "-o", &txa_arg]);
    assert_eq!(code, 0, "enc img fallo: stdout={out} stderr={err}");
    assert!(txa_path.exists(), "no se creó el .txa");

    // 2. info sobre el .txa
    let (code, info, _err) = run(&play_bin(), &["info", &txa_arg]);
    assert_eq!(code, 0, "info fallo");
    assert!(info.contains("res"), "info debe listar resolución: {info}");

    // 3. export -> .ansi con secuencias de escape
    let ansi_path = dir.join("tyxel_e2e_info.ansi");
    let ansi_arg = ansi_path.to_str().unwrap().to_string();
    let _ = std::fs::remove_file(&ansi_path);
    let (code, msg, err) = run(&play_bin(), &["export", &txa_arg, "-o", &ansi_arg]);
    assert_eq!(code, 0, "export fallo: {msg} {err}");
    let ansi = std::fs::read(&ansi_path).expect("leer .ansi");
    assert!(!ansi.is_empty());
    // truecolor presente (38;2) para la imagen sintética
    let ansi_str = String::from_utf8_lossy(&ansi).into_owned();
    assert!(
        ansi_str.contains("\x1b[38;2;") || ansi_str.contains("\x1b[48;2;"),
        "el ANSI exportado debe contener truecolor"
    );

    // 4. Codificar imagen -> .txv y leer info
    let txv_path = dir.join("tyxel_e2e_info.txv");
    let txv_arg = txv_path.to_str().unwrap().to_string();
    let (code, out, err) = run(&enc_bin(), &["img", &png_arg, "-o", &txv_arg]);
    assert_eq!(code, 0, "enc img->txv fallo: {out} {err}");
    let (code, info, _err) = run(&play_bin(), &["info", &txv_arg]);
    assert_eq!(code, 0);
    assert!(
        info.contains("LZ4") || info.contains("lz4") || info.contains("compres"),
        "info .txv debe listar compresión: {info}"
    );

    // limpieza
    for p in [png, txa_path, txv_path, ansi_path] {
        std::fs::remove_file(p).ok();
    }
}

#[test]
fn cli_e2e_txv_play_smoke() {
    // Genera un .txvs y lo "reproduce" con --fps-override (termina sin colgar)
    let dir = std::env::temp_dir();
    let png = synth_png(&dir, "play");
    let txv_path = dir.join("tyxel_e2e_play.txv");
    let png_arg = png.to_str().unwrap().to_string();
    let txv_arg = txv_path.to_str().unwrap().to_string();
    let (code, out, err) = run(&enc_bin(), &["img", &png_arg, "-o", &txv_arg]);
    assert_eq!(code, 0, "{out} {err}");
    let (code, out, err) = run(&play_bin(), &["play", &txv_arg, "--fps-override", "120"]);
    assert_eq!(
        code, 0,
        "play debe terminarse limpio: stdout={out} stderr={err}"
    );
    std::fs::remove_file(&png).ok();
    std::fs::remove_file(&txv_path).ok();
}
