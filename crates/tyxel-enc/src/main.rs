//! tyxel-enc: codificador CLI de Tyxel (100 % Rust puro, Termux/Android aarch64).
//!
//! Subcomandos:
//! - `img`: imagen estática (.png/.jpg/.gif/.webp) -> `.txa` / `.txv` (1 I-Frame).
//! - `gif`: GIF animado -> `.txv` (I-Frame + P-Frames delta, dedup de frames idénticos).
//! - `video`: stdin RGB24 crudo (`--raw-rgb`) o `ffmpeg` como subproceso CLI
//!   (`--video ARCHIVO`) -> `.txv`.
//!
//! Reglas del proyecto: nunca enlaza ffmpeg como librería (solo subproceso binario)
//! y nunca usa `/tmp` literal (respeta `$PREFIX/tmp` vía `std::env::temp_dir()`).

use std::fs::File;
use std::io::{BufReader, BufWriter, ErrorKind, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};
use image::AnimationDecoder;
use image::codecs::gif::GifDecoder;
use image::imageops::FilterType;
use tyxel_core::matcher::Matcher;
use tyxel_core::txa;
use tyxel_core::txv::{ColorMode, Compression, TxvHeader, TxvWriter};
use tyxel_core::tyxel::Tyxel;

// ---------- Errores ----------

type Res<T> = Result<T, EncError>;

#[derive(Debug)]
enum EncError {
    Io(std::io::Error),
    Msg(String),
}

impl From<std::io::Error> for EncError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<image::ImageError> for EncError {
    fn from(e: image::ImageError) -> Self {
        Self::Msg(format!("decodificación de imagen fallida: {e}"))
    }
}

impl std::fmt::Display for EncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EncError::Io(e) => write!(f, "I/O: {e}"),
            EncError::Msg(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for EncError {}

fn err<T>(msg: impl Into<String>) -> Res<T> {
    Err(EncError::Msg(msg.into()))
}

// ---------- CLI ----------

#[derive(Parser)]
#[command(
    name = "tyxel-enc",
    about = "Codificador Tyxel: imagen/GIF/video -> .txa/.txv"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Codifica una imagen estática a .txa o .txv (1 I-Frame).
    Img {
        input: String,
        #[arg(short = 'o', long = "output")]
        output: String,
        #[arg(long)]
        width: Option<u16>,
        #[arg(long)]
        height: Option<u16>,
        #[arg(long, value_enum, default_value = "contain")]
        fit: Fit,
    },
    /// Codifica un GIF animado a .txv.
    Gif {
        input: String,
        #[arg(short = 'o', long = "output")]
        output: String,
        #[arg(long, default_value_t = 64)]
        width: u16,
        #[arg(long)]
        height: Option<u16>,
        #[arg(long)]
        fps: Option<u32>,
        #[arg(long, default_value_t = 120)]
        keyframe_interval: u32,
    },
    /// Codifica video crudo RGB24 (stdin o ffmpeg subproceso) a .txv.
    Video {
        /// Archivo de video para ffmpeg (subproceso CLI, no librería).
        #[arg(long)]
        video: Option<String>,
        /// Lee frames RGB24 crudos desde stdin (requiere --w y --h).
        #[arg(long, default_value_t = false)]
        raw_rgb: bool,
        /// Columnas de la cuadrilla target (default 64 si falta).
        #[arg(long)]
        width: Option<u16>,
        /// Filas de la cuadrilla target.
        #[arg(long)]
        height: Option<u16>,
        /// Ancho en px de los frames fuente (--raw-rgb).
        #[arg(long)]
        w: Option<u32>,
        /// Alto en px de los frames fuente (--raw-rgb).
        #[arg(long)]
        h: Option<u32>,
        #[arg(long, default_value_t = 30)]
        fps: u32,
        #[arg(short = 'o', long = "output")]
        output: String,
        #[arg(long, default_value_t = 120)]
        keyframe_interval: u32,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Fit {
    Contain,
    Cover,
}

fn main() {
    let cli = Cli::parse();
    let res = match cli.cmd {
        Cmd::Img {
            input,
            output,
            width,
            height,
            fit,
        } => encode_img(&input, &output, width, height, fit),
        Cmd::Gif {
            input,
            output,
            width,
            height,
            fps,
            keyframe_interval,
        } => encode_gif(&input, &output, width, height, fps, keyframe_interval),
        Cmd::Video {
            video,
            raw_rgb,
            width,
            height,
            w,
            h,
            fps,
            output,
            keyframe_interval,
        } => encode_video(
            &video,
            raw_rgb,
            width,
            height,
            w,
            h,
            fps,
            &output,
            keyframe_interval,
        ),
    };
    if let Err(e) = res {
        eprintln!("tyxel-enc: error: {e}");
        std::process::exit(1);
    }
}

// ---------- Utilidades ----------

fn is_ext(path: &str, ext: &str) -> bool {
    Path::new(path)
        .extension()
        .map(|e| e.eq_ignore_ascii_case(ext))
        .unwrap_or(false)
}

fn clamp_u16(v: u64, what: &str) -> Res<u16> {
    if v == 0 {
        return err(format!("resolución de cuadrilla nula: {what}"));
    }
    if v > u16::MAX as u64 {
        return err(format!("{what} excede u16: {v}"));
    }
    Ok(v as u16)
}

fn round_div(a: u64, b: u64) -> u64 {
    (a + b / 2) / b
}

/// Elige la resolución de la cuadrilla en celdas (celdas 1:2 ancho:alto).
/// Con `--width W`: H = round(alto_px * W / (ancho_px * 2)).
/// Con `--height H`: W = round(ancho_px * H * 2 / alto_px).
/// Sin ninguno: W = 64 y H según aspecto.
fn derive_cells(pw: u32, ph: u32, width: Option<u16>, height: Option<u16>) -> Res<(u16, u16)> {
    let pw = pw.max(1) as u64;
    let ph = ph.max(1) as u64;
    match (width, height) {
        (Some(w), Some(h)) => Ok((w, h)),
        (Some(w), None) => {
            let h = round_div(ph * w as u64, pw * 2).max(1);
            Ok((w, clamp_u16(h, "alto derivado")?))
        }
        (None, Some(h)) => {
            let w = round_div(pw * h as u64 * 2, ph).max(1);
            Ok((clamp_u16(w, "ancho derivado")?, h))
        }
        (None, None) => {
            let w = 64u64;
            let h = round_div(ph * w, pw * 2).max(1);
            Ok((64, clamp_u16(h, "alto derivado")?))
        }
    }
}

fn rgba_to_rgb_image(img: &image::RgbaImage) -> image::RgbImage {
    let mut out = image::RgbImage::new(img.width(), img.height());
    for (x, y, p) in img.enumerate_pixels() {
        out.put_pixel(x, y, image::Rgb([p[0], p[1], p[2]]));
    }
    out
}

/// Reescalado bilineal (Triangle) con `imageops::resize` (ImageBuffer no
/// tiene `resize_exact` inherente en image 0.25).
fn resize_rgb(src: &image::RgbImage, nw: u32, nh: u32) -> image::RgbImage {
    image::imageops::resize(src, nw, nh, FilterType::Triangle)
}

/// Redimensionado nearest-neighbor RGB24 manual (barato por frame en ARM64).
fn resize_nn_rgb(src: &[u8], sw: usize, sh: usize, dw: usize, dh: usize) -> Res<Vec<u8>> {
    if sw == 0 || sh == 0 {
        return err("dimensiones de fuente nulas");
    }
    let mut out = vec![0u8; dw * dh * 3];
    for y in 0..dh {
        let sy = y * sh / dh;
        for x in 0..dw {
            let sx = x * sw / dw;
            let si = (sy * sw + sx) * 3;
            let di = (y * dw + x) * 3;
            out[di] = src[si];
            out[di + 1] = src[si + 1];
            out[di + 2] = src[si + 2];
        }
    }
    Ok(out)
}

fn txv_header(w: u16, h: u16, fps_num: u16) -> TxvHeader {
    TxvHeader {
        width: w,
        height: h,
        fps_num,
        fps_den: 1,
        total_frames: 0,
        nerdfont: true,
        color: ColorMode::Truecolor,
        compression: Compression::None,
        atlas_id: 0,
    }
}

/// Emisor incremental de frames a un .txv: deduplica frames idénticos
/// (acumula su duración en el siguiente paquete) y fuerza un I-Frame cada
/// `keyframe_interval` paquetes emitidos desde el último I-Frame.
struct FrameEmitter<W: Write> {
    wr: TxvWriter<W>,
    prev: Option<Vec<Tyxel>>,
    hold_ms: u32,
    since_i: u32,
    keyframe_interval: u32,
    emitted: u32,
    iframes: u32,
}

impl<W: Write> FrameEmitter<W> {
    fn new(wr: TxvWriter<W>, keyframe_interval: u32) -> Self {
        Self {
            wr,
            prev: None,
            hold_ms: 0,
            since_i: 0,
            keyframe_interval: keyframe_interval.max(1),
            emitted: 0,
            iframes: 0,
        }
    }

    fn push(&mut self, cur: &[Tyxel], duration_ms: u32) -> std::io::Result<()> {
        if self.prev.as_deref() == Some(cur) {
            self.hold_ms = self.hold_ms.saturating_add(duration_ms);
            return Ok(());
        }
        let dur = self
            .hold_ms
            .saturating_add(duration_ms)
            .min(u16::MAX as u32) as u16;
        let force_i = self.since_i >= self.keyframe_interval;
        match self.prev.as_deref() {
            Some(pv) if !force_i => {
                self.wr.push_pframe(pv, cur, dur)?;
                self.since_i += 1;
            }
            _ => {
                self.wr.push_iframe(cur, dur)?;
                self.since_i = 0;
                self.iframes += 1;
            }
        }
        self.prev = Some(cur.to_vec());
        self.hold_ms = 0;
        self.emitted += 1;
        Ok(())
    }

    fn finish(self) -> std::io::Result<(u32, Vec<(u32, u64)>)> {
        self.wr.finish()
    }
}

/// Escritura de destino para `img`: .txa (texto editable) o .txv (1 I-Frame).
fn write_single_image(output: &str, w: u16, h: u16, cells: &[Tyxel]) -> Res<String> {
    if is_ext(output, "txa") {
        let img = txa::TxaImage {
            width: w,
            height: h,
            cells: cells.to_vec(),
        };
        let mut f = BufWriter::new(File::create(output)?);
        txa::to_writer(&img, &mut f)?;
        f.flush()?;
        Ok(format!(
            "{output}: .txa {w}x{h} celdas ({} Tyxel)",
            cells.len()
        ))
    } else if is_ext(output, "txv") {
        let f = BufWriter::new(File::create(output)?);
        let mut wr = TxvWriter::new(f, txv_header(w, h, 30))?;
        wr.push_iframe(cells, 0)?;
        let (frames, _) = wr.finish()?;
        Ok(format!("{output}: .txv {w}x{h} celdas, {frames} frame(s)"))
    } else {
        err(format!(
            "extensión de salida desconocida (se espera .txa o .txv): {output}"
        ))
    }
}

// ---------- Subcomando img ----------

fn encode_img(
    input: &str,
    output: &str,
    width: Option<u16>,
    height: Option<u16>,
    fit: Fit,
) -> Res<String> {
    let rgb = image::open(input)
        .map_err(|e| EncError::Msg(format!("no se pudo abrir {input}: {e}")))?
        .into_rgb8();
    let (pw, ph) = (rgb.width(), rgb.height());
    let (w, h) = derive_cells(pw, ph, width, height)?;
    let (tw, th) = (w as u32 * 8, h as u32 * 16);

    let resized = if width.is_some() && height.is_some() && fit == Fit::Contain {
        // contain: escala preservando el aspecto y pega centrado sobre negro.
        let scale = (tw as f64 / pw as f64).min(th as f64 / ph as f64);
        let fw = ((pw as f64 * scale).round() as u32).clamp(1, tw);
        let fh = ((ph as f64 * scale).round() as u32).clamp(1, th);
        let fit_img = resize_rgb(&rgb, fw, fh);
        let mut canvas = image::RgbImage::from_pixel(tw, th, image::Rgb([0, 0, 0]));
        for (sx, sy, p) in fit_img.enumerate_pixels() {
            let dx = sx + (tw - fw) / 2;
            let dy = sy + (th - fh) / 2;
            canvas.put_pixel(dx, dy, *p);
        }
        canvas
    } else {
        // cover (o un solo eje): relleno directo a (W*8, H*16) con Triangle.
        resize_rgb(&rgb, tw, th)
    };

    let pixels = resized.into_raw();
    let cells = Matcher::new().match_frame(w as usize, h as usize, &pixels);
    write_single_image(output, w, h, &cells)
}

// ---------- Subcomando gif ----------

/// Convierte un frame GIF (max. compuesto si difiere del lienzo 0) a celdas.
fn gif_frame_cells(
    matcher: &mut Matcher,
    frame: &image::Frame,
    canvas_dims: (u32, u32),
    prev_full: &Option<image::RgbaImage>,
    grid: (u16, u16),
) -> Res<Vec<Tyxel>> {
    let buf = frame.buffer();
    let full: image::RgbaImage = if (buf.width(), buf.height()) == canvas_dims {
        buf.clone()
    } else {
        let mut canvas = match prev_full {
            Some(p) if (p.width(), p.height()) == canvas_dims => p.clone(),
            _ => image::RgbaImage::from_pixel(
                canvas_dims.0,
                canvas_dims.1,
                image::Rgba([0, 0, 0, 255]),
            ),
        };
        for (sx, sy, p) in buf.enumerate_pixels() {
            let dx = sx as i64 + frame.left() as i64;
            let dy = sy as i64 + frame.top() as i64;
            if (dx as u32) < canvas_dims.0 && (dy as u32) < canvas_dims.1 && dx >= 0 && dy >= 0 {
                canvas.put_pixel(dx as u32, dy as u32, *p);
            }
        }
        canvas
    };
    let rgb = rgba_to_rgb_image(&full);
    let (tw, th) = (grid.0 as u32 * 8, grid.1 as u32 * 16);
    let resized = resize_rgb(&rgb, tw, th);
    Ok(matcher.match_frame(grid.0 as usize, grid.1 as usize, &resized.into_raw()))
}

fn frame_delay_ms(frame: &image::Frame) -> u32 {
    let d: Duration = Duration::from(frame.delay());
    let ms = d.as_millis() as u32;
    if ms == 0 { 40 } else { ms }
}

fn encode_gif(
    input: &str,
    output: &str,
    width: u16,
    height: Option<u16>,
    fps: Option<u32>,
    keyframe_interval: u32,
) -> Res<String> {
    if !is_ext(output, "txv") {
        return err(format!("gif requiere salida .txv (hay {output})"));
    }
    let file =
        File::open(input).map_err(|e| EncError::Msg(format!("no se pudo abrir {input}: {e}")))?;
    let decoder = GifDecoder::new(BufReader::new(file))?;
    let mut frames = decoder.into_frames();

    let first = match frames.next() {
        None => return err(format!("GIF sin frames: {input}")),
        Some(Err(e)) => return Err(e.into()),
        Some(Ok(f)) => f,
    };
    let canvas_dims = (first.buffer().width(), first.buffer().height());
    let grid = derive_cells(canvas_dims.0, canvas_dims.1, Some(width), height)?;
    let fps_num = fps
        .filter(|f| *f > 0)
        .map(|f| u16::try_from(f).unwrap_or(u16::MAX))
        .unwrap_or(30);
    let fps_override_ms = fps.filter(|f| *f > 0).map(|f| (1000 / f).max(1));

    let mut matcher = Matcher::new();
    let out_file = BufWriter::new(File::create(output)?);
    let mut emitter = FrameEmitter::new(
        TxvWriter::new(out_file, txv_header(grid.0, grid.1, fps_num))?,
        keyframe_interval,
    );

    let mut prev_full: Option<image::RgbaImage> = None;
    let first_ms = fps_override_ms.unwrap_or_else(|| frame_delay_ms(&first));
    let cells = gif_frame_cells(&mut matcher, &first, canvas_dims, &prev_full, grid)?;
    emitter.push(&cells, first_ms)?;
    prev_full = Some(first.buffer().clone());
    let mut n_read = 1u32;

    for fr in frames {
        let fr = fr?;
        n_read += 1;
        let ms = fps_override_ms.unwrap_or_else(|| frame_delay_ms(&fr));
        let cur = gif_frame_cells(&mut matcher, &fr, canvas_dims, &prev_full, grid)?;
        emitter.push(&cur, ms)?;
        if (fr.buffer().width(), fr.buffer().height()) == canvas_dims {
            prev_full = Some(fr.buffer().clone());
        } else {
            // Reconstrucción del lienzo completo para el próximo frame parcial.
            let mut canvas = match prev_full {
                Some(p) if (p.width(), p.height()) == canvas_dims => p.clone(),
                _ => image::RgbaImage::from_pixel(
                    canvas_dims.0,
                    canvas_dims.1,
                    image::Rgba([0, 0, 0, 255]),
                ),
            };
            for (sx, sy, p) in fr.buffer().enumerate_pixels() {
                let dx = sx as i64 + fr.left() as i64;
                let dy = sy as i64 + fr.top() as i64;
                if (dx as u32) < canvas_dims.0 && (dy as u32) < canvas_dims.1 && dx >= 0 && dy >= 0
                {
                    canvas.put_pixel(dx as u32, dy as u32, *p);
                }
            }
            prev_full = Some(canvas);
        }
    }

    let emitted = emitter.emitted;
    let iframes = emitter.iframes;
    let _ = emitter.finish()?;
    Ok(format!(
        "{output}: .txv {grid0}x{grid1} celdas, {n_read} frames leídos, {emitted} paquetes ({iframes} I-Frames)",
        grid0 = grid.0,
        grid1 = grid.1,
    ))
}

// ---------- Subcomando video ----------

/// Envoltorio que mantiene vivo el proceso ffmpeg mientras se leen sus frames.
struct WithChild {
    stdout: BufReader<ChildStdout>,
    child: Option<Child>,
}

impl Read for WithChild {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.stdout.read(buf)
    }
}

impl Drop for WithChild {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.wait();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn encode_video(
    video: &Option<String>,
    raw_rgb: bool,
    width: Option<u16>,
    height: Option<u16>,
    src_w: Option<u32>,
    src_h: Option<u32>,
    fps: u32,
    output: &str,
    keyframe_interval: u32,
) -> Res<String> {
    if !is_ext(output, "txv") {
        return err(format!("video requiere salida .txv (hay {output})"));
    }
    if fps == 0 {
        return err("--fps debe ser > 0");
    }
    if fps > u16::MAX as u32 {
        return err(format!("--fps excede u16: {fps}"));
    }

    if video.is_some() && raw_rgb {
        return err("video: usa solo una fuente de entrada (--video o --raw-rgb)");
    }
    if video.is_none() && !raw_rgb {
        return err("video: indica una fuente (--video ARCHIVO) o (--raw-rgb con stdin)");
    }

    // Resolución de la fuente y de la cuadrilla target.
    let (from_ffmpeg, frame_src_w, frame_src_h, grid_w, grid_h) = if video.is_some() {
        let (gw, gh) = match (width, height) {
            (Some(w), Some(h)) => (w, h),
            _ => return err("--video requiere --width y --height (cuadrilla target)"),
        };
        (true, 0u32, 0u32, gw, gh)
    } else {
        let (tw, th) = match (src_w, src_h) {
            (Some(w), Some(h)) if w > 0 && h > 0 => (w, h),
            _ => return err("--raw-rgb requiere --w y --h (dimensiones de los frames fuente)"),
        };
        let (gw, gh) = derive_cells(tw, th, width, height)?;
        (false, tw, th, gw, gh)
    };
    let frame_bytes = if from_ffmpeg {
        // ffmpeg entrega ya (W*8)x(H*16) px RGB24 por frame.
        grid_w as usize * 8 * (grid_h as usize * 16) * 3
    } else {
        frame_src_w as usize * frame_src_h as usize * 3
    };

    if from_ffmpeg {
        let path = video
            .as_deref()
            .ok_or_else(|| EncError::Msg(String::from("sin fuente")))?;
        let tw = grid_w as u32 * 8;
        let th = grid_h as u32 * 16;
        let mut child = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-nostdin",
                "-i",
                path,
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgb24",
                "-vf",
                &format!("scale={tw}:{th}"),
                "-r",
                &fps.to_string(),
                "-",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| {
                EncError::Msg(format!(
                    "no se pudo lanzar ffmpeg (instálalo con `pkg install ffmpeg`): {e}"
                ))
            })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| EncError::Msg(String::from("ffmpeg: stdout no disponible")))?;
        let mut src = WithChild {
            stdout: BufReader::new(stdout),
            child: Some(child),
        };
        run_video_loop(
            &mut src,
            true,
            0,
            0,
            grid_w,
            grid_h,
            frame_bytes,
            fps,
            output,
            keyframe_interval,
        )
    } else {
        let mut src = std::io::stdin().lock();
        run_video_loop(
            &mut src,
            false,
            frame_src_w,
            frame_src_h,
            grid_w,
            grid_h,
            frame_bytes,
            fps,
            output,
            keyframe_interval,
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn run_video_loop(
    reader: &mut dyn Read,
    from_ffmpeg: bool,
    src_w: u32,
    src_h: u32,
    grid_w: u16,
    grid_h: u16,
    frame_bytes: usize,
    fps: u32,
    output: &str,
    keyframe_interval: u32,
) -> Res<String> {
    if from_ffmpeg && (grid_w == 0 || grid_h == 0) {
        return err(String::from("cuadrilla target nula"));
    }

    let mut matcher = Matcher::new();
    let out_file = BufWriter::new(File::create(output)?);
    let mut emitter = FrameEmitter::new(
        TxvWriter::new(
            out_file,
            txv_header(grid_w, grid_h, u16::try_from(fps).unwrap_or(u16::MAX)),
        )?,
        keyframe_interval,
    );

    let mut buf = vec![0u8; frame_bytes.max(1)];
    let mut n_read = 0u32;
    let frame_ms = (1000 / fps).max(1);
    loop {
        match reader.read_exact(&mut buf) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.into()),
        }
        let pixels: Vec<u8> = if from_ffmpeg {
            // ffmpeg entrega ya (W*8)x(H*16) px RGB24 listo para matcher.
            buf.clone()
        } else {
            let sw = src_w as usize;
            let sh = src_h as usize;
            resize_nn_rgb(&buf, sw, sh, grid_w as usize * 8, grid_h as usize * 16)?
        };
        let cur = matcher.match_frame(grid_w as usize, grid_h as usize, &pixels);
        emitter.push(&cur, frame_ms)?;
        n_read += 1;
    }

    if n_read == 0 {
        return err("video: no se leyó ningún frame de la fuente");
    }
    let emitted = emitter.emitted;
    let iframes = emitter.iframes;
    let _ = emitter.finish()?;
    Ok(format!(
        "{output}: .txv {grid_w}x{grid_h} celdas, {n_read} frames leídos, {emitted} paquetes ({iframes} I-Frames)"
    ))
}
