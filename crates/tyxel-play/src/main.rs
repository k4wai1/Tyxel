//! Sub-Agente E: reproductor TUI y CLI de inspección del ecosistema Tyxel.
//!
//! Comandos:
//!   tyxel play <FILE> [--loop] [--color ...] [--fps-override N]
//!   tyxel cat <FILE.txa> [--width W] [--height H] [--fit contain|cover] [--color ...]
//!   tyxel export <FILE.txa> -o OUT.ansi [--width ...] [--height ...] [--fit ...] [--color ...]
//!   tyxel info <FILE>
//!   tyxel render <FILE> -o OUT.mp4 [--res 720p|1080p|WxH|auto] [--fps N]
//!                 [--color truecolor|256|16|mono] [--font F.ttf]
//!                 [--audio-from ORIGINAL.mp4] [--scanlines]
//!
//! Nota de plataforma (Android/Termux): el módulo `term` usa `libc`
//! (tcgetattr/tcsetattr) en lugar del crate `termios`, porque el layout que
//! declara termios 0.3 para android (`NCCS = 19`, sin `c_ispeed`/`c_ospeed`)
//! no coincide con el ABI Bionic real (`NCCS = 32`). Todo el `unsafe` queda
//! contenido en este módulo.

use std::collections::HashSet;
use std::io::{self, BufWriter, Write as IoWrite};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand, ValueEnum};
use tyxel_core::atlas;
use tyxel_core::renderer::GlyphAtlas;
use tyxel_core::renderer::{QuantMode, compose_frame, fit_geometry};
use tyxel_core::txa::{self, TxaImage};
use tyxel_core::txv::{self, Txv, TxvFrame};
use tyxel_core::tyxel::{Tyxel, rgb_to_ansi16_dithered, rgb_to_ansi256};

// ============================================================================
// CLI
// ============================================================================

#[derive(Parser)]
#[command(
    name = "tyxel-play",
    version,
    about = "Reproductor TUI/ANSI y CLI de inspección para arte Tyxel (.txa/.txv)"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Reproduce un .txv (o un .txa estático) a pantalla completa TUI.
    Play {
        /// Ruta del archivo .txv o .txa.
        file: PathBuf,
        /// Modo de color de salida.
        #[arg(long, default_value = "auto")]
        color: ColorArg,
        /// Reemplaza el FPS del archivo (N frames por segundo).
        #[arg(long)]
        fps_override: Option<u16>,
        /// Repite el archivo indefinidamente.
        #[arg(long = "loop")]
        loop_play: bool,
    },
    /// Renderiza un .txa a stdout en ANSI (compatible con fastfetch).
    Cat {
        /// Ruta del archivo .txa.
        file: PathBuf,
        /// Ancho en celdas (reescale al vuelo).
        #[arg(long)]
        width: Option<u16>,
        /// Alto en celdas (reescale al vuelo).
        #[arg(long)]
        height: Option<u16>,
        /// Contención del reescalado.
        #[arg(long, default_value = "contain")]
        fit: FitArg,
        /// Modo de color de salida.
        #[arg(long, default_value = "auto")]
        color: ColorArg,
    },
    /// Exporta un .txa a un archivo de escapes ANSI (para `fastfetch --file`).
    Export {
        /// Ruta del archivo .txa de entrada.
        file: PathBuf,
        /// Ruta del .ansi de salida.
        #[arg(short, long)]
        output: PathBuf,
        /// Ancho en celdas (reescale al vuelo).
        #[arg(long)]
        width: Option<u16>,
        /// Alto en celdas (reescale al vuelo).
        #[arg(long)]
        height: Option<u16>,
        /// Contención del reescalado.
        #[arg(long, default_value = "contain")]
        fit: FitArg,
        /// Modo de color de salida.
        #[arg(long, default_value = "auto")]
        color: ColorArg,
    },
    /// Muestra metadatos legibles de un .txa o .txv.
    Info {
        /// Ruta del archivo.
        file: PathBuf,
    },
    /// Renderiza un .txv/.txa a MP4 emulando una terminal virtual (Fase 3).
    Render {
        /// Ruta del archivo .txv o .txa.
        file: PathBuf,
        /// Ruta del MP4 de salida.
        #[arg(short, long)]
        output: PathBuf,
        /// Resolución del lienzo: 720p, 1080p, WxH o auto (cols*8 x rows*16).
        #[arg(long, default_value = "1080p")]
        res: String,
        /// FPS de salida; por defecto el del archivo (30 en .txa).
        #[arg(long)]
        fps: Option<u32>,
        /// Modo de color de la paleta.
        #[arg(long, default_value = "truecolor")]
        color: QuantArg,
        /// Fuente TTF para rasterizar los glifos (default: ~/.termux/font.ttf).
        #[arg(long)]
        font: Option<PathBuf>,
        /// Video original del que copiar la pista de audio.
        #[arg(long)]
        audio_from: Option<PathBuf>,
        /// Atenúa la última fila lógica de cada celda (efecto CRT sutil).
        #[arg(long)]
        scanlines: bool,
    },
}

#[derive(Copy, Clone, ValueEnum)]
enum ColorArg {
    /// Autodetecta según TERM/COLORTERM.
    Auto,
    /// RGB TrueColor 24 bits.
    #[value(name = "truecolor")]
    Truecolor,
    /// Paleta xterm-256.
    #[value(name = "256")]
    C256,
    /// 16 colores ANSI con dithering Bayer.
    #[value(name = "16")]
    C16,
    /// Blanco sobre negro.
    #[value(name = "mono")]
    Mono,
}

#[derive(Copy, Clone, PartialEq, Eq, Default, ValueEnum)]
enum FitArg {
    /// Ajusta dentro del lienzo y rellena con celdas vacías.
    #[default]
    Contain,
    /// Cubre el lienzo completo y recorta el exceso centrado.
    Cover,
}

#[derive(Copy, Clone, ValueEnum)]
enum QuantArg {
    /// RGB TrueColor 24 bits.
    #[value(name = "truecolor")]
    Truecolor,
    /// Paleta xterm-256.
    #[value(name = "256")]
    C256,
    /// 16 colores ANSI con dithering Bayer.
    #[value(name = "16")]
    C16,
    /// Blanco sobre negro.
    #[value(name = "mono")]
    Mono,
}

impl QuantArg {
    const fn quant(self) -> QuantMode {
        match self {
            QuantArg::Truecolor => QuantMode::TrueColor,
            QuantArg::C256 => QuantMode::Ansi256,
            QuantArg::C16 => QuantMode::Ansi16,
            QuantArg::Mono => QuantMode::Mono,
        }
    }
}

// ============================================================================
// Errores y exit codes
// ============================================================================

/// Exit code de error de parseo/formato.
const EXIT_PARSE: i32 = 2;
/// Exit code de error de I/O.
const EXIT_IO: i32 = 3;
/// Exit code provocado por SIGINT/SIGTERM.
const EXIT_SIGNAL: i32 = 130;

enum AppError {
    Parse(String),
    Io(String),
}

fn io_err(e: io::Error) -> AppError {
    AppError::Io(e.to_string())
}

fn bail<T>(kind: fn(String) -> AppError, msg: impl Into<String>) -> Result<T, AppError> {
    Err(kind(msg.into()))
}

// ============================================================================
// Módulo terminal: raw mode, alternate screen, TerminalGuard + señales
// ============================================================================

/// Bytes de restauración de terminal.
const RESTORE_SEQ: &[u8; 26] = b"\x1b[?1049l\x1b[?25h\x1b[?2026l\x1b[0m";

/// Indica si la copia estática del estado termios es válida (para el handler).
static SAVED_READY: AtomicBool = AtomicBool::new(false);

/// Copia del estado original de termios para la ruta de señal (libc::termios).
static mut SAVED_TERMIOS: Option<libc::termios> = None;

extern "C" fn signal_restore(_sig: libc::c_int) {
    // Ruta async-signal-safe mínima: restaurar termios, escribir la secuencia
    // de restauración directamente al fd 1 y terminar con código 130.
    unsafe {
        if SAVED_READY.load(Ordering::Relaxed) {
            let saved = core::ptr::addr_of!(SAVED_TERMIOS);
            if !saved.is_null() && (*saved).is_some() {
                let t: libc::termios = (*saved).unwrap_unchecked();
                libc::tcsetattr(0, libc::TCSANOW, &t);
            }
        }
        libc::write(1, RESTORE_SEQ.as_ptr().cast(), RESTORE_SEQ.len());
        libc::_exit(EXIT_SIGNAL);
    }
}

/// RAII: al construir entra a modo raw + pantalla alternativa; al destruirse
/// (salida normal, panic-unwind o drop forzado) restaura todo el estado.
struct TerminalGuard {
    saved: Option<libc::termios>,
    tty: bool,
}

impl TerminalGuard {
    fn enter() -> Result<Self, AppError> {
        // Degradación sin TTY (stdout redirigido / ejecutado dentro de tests):
        // no se activa raw mode ni alt-screen, el Drop solo libera el seq.
        let saved: Option<libc::termios> = unsafe {
            let mut t: libc::termios = core::mem::zeroed();
            if libc::tcgetattr(0, &mut t) != 0 {
                return Ok(TerminalGuard {
                    saved: None,
                    tty: false,
                });
            }
            // Solo ECHO e ICANON: mantenemos ISIG para que Ctrl-C siga
            // generando SIGINT y el handler lo capture con restauración.
            let mut raw = t;
            raw.c_lflag &= !(libc::ECHO | libc::ICANON);
            if libc::tcsetattr(0, libc::TCSANOW, &raw) != 0 {
                return Ok(TerminalGuard {
                    saved: None,
                    tty: false,
                });
            }
            core::ptr::addr_of_mut!(SAVED_TERMIOS).write(Some(t));
            SAVED_READY.store(true, Ordering::SeqCst);
            let h: extern "C" fn(libc::c_int) = signal_restore;
            libc::signal(libc::SIGINT, h as usize);
            libc::signal(libc::SIGTERM, h as usize);
            Some(t)
        };
        let tty = saved.is_some();
        if tty {
            // Entrada de "modo TUI" idempotente: pantalla alternativa + cursor oculto.
            let mut out = io::stdout().lock();
            let _ = out.write_all(b"\x1b[?1049h\x1b[?25l");
            let _ = out.flush();
        }
        Ok(TerminalGuard { saved, tty })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if let Some(saved) = self.saved {
            SAVED_READY.store(false, Ordering::SeqCst);
            unsafe {
                libc::tcsetattr(0, libc::TCSADRAIN, &saved);
            }
        }
        if self.tty {
            let mut out = io::stdout().lock();
            let _ = out.write_all(RESTORE_SEQ);
            let _ = out.flush();
        }
    }
}

// ============================================================================
// Selección de modo de color
// ============================================================================

#[derive(Copy, Clone, PartialEq, Eq)]
enum ColorChoice {
    TrueColor,
    Ansi256,
    Ansi16,
    Mono,
}

fn resolve_color(arg: ColorArg) -> ColorChoice {
    match arg {
        ColorArg::Truecolor => ColorChoice::TrueColor,
        ColorArg::C256 => ColorChoice::Ansi256,
        ColorArg::C16 => ColorChoice::Ansi16,
        ColorArg::Mono => ColorChoice::Mono,
        ColorArg::Auto => {
            let colorterm = std::env::var("COLORTERM")
                .unwrap_or_default()
                .to_ascii_lowercase();
            let term = std::env::var("TERM")
                .unwrap_or_default()
                .to_ascii_lowercase();
            if colorterm.contains("truecolor")
                || colorterm.contains("24bit")
                || term.contains("direct")
            {
                ColorChoice::TrueColor
            } else if term.contains("256color") {
                ColorChoice::Ansi256
            } else if !term.is_empty() && term != "dumb" {
                ColorChoice::Ansi16
            } else {
                ColorChoice::Mono
            }
        }
    }
}

// ============================================================================
// Painter: emisión de escapes con caché de fg/bg entre celdas contiguas
// ============================================================================

#[derive(Copy, Clone, PartialEq, Eq)]
enum Paint {
    /// Estado desconocido / inicial (fuerza la emisión del SGR).
    Unknown,
    Rgb([u8; 3]),
    Idx256(u8),
    Idx16(u8),
    /// Modo monocromo: fg brillante constante + bg negro constante.
    Mono,
}

struct Painter {
    mode: ColorChoice,
    fg: Paint,
    bg: Paint,
}

/// Cercanía perceptual entre dos pints: tolerancia de Manhattan RGB <= 9.
/// Evita repetir SGRs de colores que el ojo no distingue (flush por frame
/// más compacto): solo aplica en tintes TrueColor/256 por índice igual.
#[inline]
fn paints_close(a: &Paint, b: &Paint) -> bool {
    match (a, b) {
        (Paint::Rgb(x), Paint::Rgb(y)) => {
            let m = x[0].abs_diff(y[0]) as u32
                + x[1].abs_diff(y[1]) as u32
                + x[2].abs_diff(y[2]) as u32;
            m <= 9
        }
        (Paint::Mono, Paint::Mono) => true,
        (Paint::Idx256(x), Paint::Idx256(y)) => x == y,
        (Paint::Idx16(x), Paint::Idx16(y)) => x == y,
        _ => false,
    }
}

impl Painter {
    fn new(mode: ColorChoice) -> Self {
        Self {
            mode,
            fg: Paint::Unknown,
            bg: Paint::Unknown,
        }
    }

    /// Olvida el estado de color actual (obliga a reemitir el próximo SGR).
    fn reset(&mut self) {
        self.fg = Paint::Unknown;
        self.bg = Paint::Unknown;
    }

    /// Cuantiza un canal RGB según el modo de color activo.
    fn quant(&self, rgb: [u8; 3], x: u16, y: u16) -> Paint {
        match self.mode {
            ColorChoice::TrueColor => Paint::Rgb(rgb),
            ColorChoice::Ansi256 => Paint::Idx256(rgb_to_ansi256(rgb)),
            // En Ansi16 el dithering Bayer depende de la posición (x, y);
            // dos celdas adyacentes pueden compartir índice y esa vez el
            // caché de SGR también es válido (el terminal ya lo tiene puesto).
            ColorChoice::Ansi16 => {
                Paint::Idx16(rgb_to_ansi16_dithered(rgb, x as usize, y as usize))
            }
            ColorChoice::Mono => Paint::Mono,
        }
    }

    /// Emite fg/bg si cambiaron respecto a la celda anterior y luego el glifo.
    /// El terminal avanza 1 columna al imprimir (glifos de ancho 1).
    fn put<W: IoWrite>(&mut self, w: &mut W, t: &Tyxel, x: u16, y: u16) -> io::Result<()> {
        let nfg = self.quant(t.fg, x, y);
        let nbg = self.quant(t.bg, x, y);
        if !paints_close(&nfg, &self.fg) {
            emit_fg(w, nfg)?;
            self.fg = nfg;
        }
        if !paints_close(&nbg, &self.bg) {
            emit_bg(w, nbg)?;
            self.bg = nbg;
        }
        let id = t.glyph_id % atlas::ATLAS_LEN as u16;
        write!(w, "{}", atlas::codepoint(id))
    }
}

fn emit_fg<W: IoWrite>(w: &mut W, p: Paint) -> io::Result<()> {
    match p {
        Paint::Rgb(c) => write!(w, "\x1b[38;2;{};{};{}m", c[0], c[1], c[2]),
        Paint::Idx256(n) => write!(w, "\x1b[38;5;{}m", n),
        Paint::Idx16(n) => {
            if n < 8 {
                write!(w, "\x1b[{}m", 30 + n)
            } else {
                // Variante brillante por negrita: SGR 1 + 30..37.
                write!(w, "\x1b[1;{}m", 22 + n)
            }
        }
        Paint::Mono => write!(w, "\x1b[1;97m"),
        Paint::Unknown => Ok(()),
    }
}

fn emit_bg<W: IoWrite>(w: &mut W, p: Paint) -> io::Result<()> {
    match p {
        Paint::Rgb(c) => write!(w, "\x1b[48;2;{};{};{}m", c[0], c[1], c[2]),
        Paint::Idx256(n) => write!(w, "\x1b[48;5;{}m", n),
        Paint::Idx16(n) => {
            if n < 8 {
                write!(w, "\x1b[{}m", 40 + n)
            } else {
                // Bright bg vía códigos aixterm (100..107): xterm/konsole/tmux.
                write!(w, "\x1b[{}m", 92 + n)
            }
        }
        Paint::Mono => write!(w, "\x1b[40m"),
        Paint::Unknown => Ok(()),
    }
}

// ============================================================================
// Renderizado de frames (.txv y .txa comparten el mismo camino)
// ============================================================================

/// Umbral para render "dirty cells" con posicionamiento directo de cursor.
const DIRTY_MAX_RATIO: f64 = 0.35;

/// Pinta un frame completo, fila por fila, con caché de SGR entre celdas.
fn render_full<W: IoWrite>(
    out: &mut W,
    cells: &[Tyxel],
    width: u16,
    height: u16,
    painter: &mut Painter,
) -> io::Result<()> {
    painter.reset();
    for y in 0..height {
        write!(out, "\x1b[{};1H", y + 1)?;
        for x in 0..width {
            let i = y as usize * width as usize + x as usize;
            painter.put(out, &cells[i], x, y)?;
        }
    }
    write!(out, "\x1b[0m")
}

/// Pinta solo las celdas cuyos Tyxel difieren entre el frame previo y el
/// actual, con posicionamiento directo de cursor por celda sucia.
fn render_dirty<W: IoWrite>(
    out: &mut W,
    prev: &[Tyxel],
    cur: &[Tyxel],
    width: u16,
    painter: &mut Painter,
) -> io::Result<usize> {
    painter.reset();
    let wusize = width as usize;
    let mut n = 0usize;
    for (i, (p, c)) in prev.iter().zip(cur.iter()).enumerate() {
        if p == c {
            continue;
        }
        let y = (i / wusize) as u16;
        let x = (i % wusize) as u16;
        write!(out, "\x1b[{};{}H", y + 1, x + 1)?;
        painter.put(out, c, x, y)?;
        n += 1;
    }
    write!(out, "\x1b[0m")?;
    Ok(n)
}

/// Cuenta celdas cambiadas (utilizado para decidir el modo de render).
fn count_dirty(prev: &[Tyxel], cur: &[Tyxel]) -> usize {
    prev.iter().zip(cur.iter()).filter(|(p, c)| p != c).count()
}

/// DEC Synchronized Output: apertura de la ventana atómica.
fn dec_sync_begin<W: IoWrite>(out: &mut W) -> io::Result<()> {
    out.write_all(b"\x1b[?2026h")
}

/// DEC Synchronized Output: cierre de la ventana atómica.
fn dec_sync_end<W: IoWrite>(out: &mut W) -> io::Result<()> {
    out.write_all(b"\x1b[?2026l")
}

// ============================================================================
// Carga de fuentes (.txv / .txa) y duraciones
// ============================================================================

struct PlaySource {
    width: u16,
    height: u16,
    /// Duración por defecto (ms) cuando `duration_ms == 0`, derivada del fps.
    default_ms: u64,
    /// FPS declarado en el archivo (para escalar cadencias con --fps-override).
    fps_tag: f64,
    frames: Vec<TxvFrame>,
}

/// Duración por defecto por frame en ms: fps_num/fps_den o --fps-override.
fn detect_fps_default(fps_num: u16, fps_den: u16, override_fps: Option<u16>) -> u64 {
    let (num, den) = match override_fps {
        Some(n) if n > 0 => (n as u64, 1),
        _ => (fps_num as u64, fps_den as u64),
    };
    if num == 0 || den == 0 {
        // Fallback seguro ~30 FPS ante metadatos degenerados.
        return 33;
    }
    (den * 1000).div_ceil(num).max(1)
}

fn read_file(path: &Path) -> Result<Vec<u8>, AppError> {
    std::fs::read(path).map_err(io_err)
}

fn is_txv(data: &[u8]) -> bool {
    data.len() >= 8 && data[..8] == txv::TXV_MAGIC
}

/// Carga un .txv o .txa y lo deja listo para el bucle de reproducción.
fn load_play_source(path: &Path, override_fps: Option<u16>) -> Result<PlaySource, AppError> {
    let data = read_file(path)?;
    if is_txv(&data) {
        let txv = Txv::parse(&data).map_err(|e| AppError::Parse(format!("txv: {}", e)))?;
        let h = &txv.header;
        Ok(PlaySource {
            width: h.width,
            height: h.height,
            default_ms: detect_fps_default(h.fps_num, h.fps_den, override_fps),
            fps_tag: (h.fps_num as f64 / h.fps_den.max(1) as f64).max(1.0),
            frames: txv.frames,
        })
    } else {
        let text = String::from_utf8_lossy(&data).into_owned();
        let img = txa::from_text(&text).map_err(|e| AppError::Parse(format!("txa: {}", e)))?;
        Ok(PlaySource {
            width: img.width,
            height: img.height,
            default_ms: detect_fps_default(30, 1, override_fps),
            fps_tag: 30.0,
            frames: vec![TxvFrame {
                frame_type: txv::FRAME_TYPE_I,
                duration_ms: 0,
                cells: img.cells,
            }],
        })
    }
}

// ============================================================================
// Reescalado con --fit contain/cover para cat/export
// ============================================================================

/// Alto en celdas que mantiene el aspecto de píxel (celda 1:2) para un ancho dado.
fn aspect_h(sw: u16, sh: u16, w: u16) -> u16 {
    let swf = (sw as f64).max(1.0);
    let h = ((sh as f64) * (w as f64) / swf).round().max(1.0);
    (h as u16).clamp(1, u16::MAX)
}

/// Ancho en celdas que mantiene el aspecto de píxel (celda 1:2) para un alto dado.
fn aspect_w(sw: u16, sh: u16, h: u16) -> u16 {
    let shf = (sh as f64).max(1.0);
    let w = ((sw as f64) * (h as f64) / shf).round().max(1.0);
    (w as u16).clamp(1, u16::MAX)
}

/// El tamaño de celdas que preserva el aspecto cabiendo (contain) o cubriendo
/// (cover) la caja `max_w x max_h`. Trabajo en píxeles: celda = 8 x 16 px.
fn fit_cells(sw: u16, sh: u16, max_w: u16, max_h: u16, cover: bool) -> (u16, u16) {
    let src = (sw as f64 * 8.0, sh as f64 * 16.0);
    let box_px = (max_w as f64 * 8.0, max_h as f64 * 16.0);
    let sx = box_px.0 / src.0.max(1.0);
    let sy = box_px.1 / src.1.max(1.0);
    let s = if cover { sx.max(sy) } else { sx.min(sy) };
    let cw = ((sw as f64) * s).round().max(1.0) as u16;
    let ch = ((sh as f64) * s).round().max(1.0) as u16;
    (cw.clamp(1, u16::MAX), ch.clamp(1, u16::MAX))
}

/// Rellena con celdas vacías (Tyxel::default) centrando la imagen en w x h.
fn pad_centered(img: TxaImage, w: u16, h: u16) -> TxaImage {
    if img.width >= w && img.height >= h {
        return crop_centered(img, w, h);
    }
    let mut out = TxaImage::new(w, h);
    let x0 = w.saturating_sub(img.width) / 2;
    let y0 = h.saturating_sub(img.height) / 2;
    for y in 0..img.height {
        for x in 0..img.width {
            out.set(x + x0, y + y0, img.get(x, y));
        }
    }
    out
}

/// Recorta el centro de la imagen para dejarla exactamente de w x h.
fn crop_centered(img: TxaImage, w: u16, h: u16) -> TxaImage {
    if img.width <= w && img.height <= h {
        return pad_centered(img, w, h);
    }
    let mut out = TxaImage::new(w, h);
    let x0 = img.width.saturating_sub(w) / 2;
    let y0 = img.height.saturating_sub(h) / 2;
    for y in 0..h {
        for x in 0..w {
            out.set(x, y, img.get(x + x0, y + y0));
        }
    }
    out
}

/// Rellena con celdas vacías para que la imagen cubra el lienzo w x h con el
/// exceso repartido a ambos lados (usada tras el escalado de cover cuando el
/// resultado excede la caja).
fn pad_around(img: TxaImage, w: u16, h: u16) -> TxaImage {
    if img.width > w || img.height > h {
        return crop_centered(img, w, h);
    }
    pad_centered(img, w, h)
}

/// Carga y reescala un .txa según width/height/--fit (--fit solo aplica si se
/// dan ambas dimensiones; con una sola se preserva el aspecto 1:2).
fn load_txa_fitted(
    file: &Path,
    width: Option<u16>,
    height: Option<u16>,
    fit: FitArg,
) -> Result<TxaImage, AppError> {
    let data = read_file(file)?;
    if is_txv(&data) {
        return bail(
            AppError::Parse,
            format!("{} es un .txv; usa `tyxel play`", file.display()),
        );
    }
    let text = String::from_utf8_lossy(&data).into_owned();
    let img = txa::from_text(&text).map_err(|e| AppError::Parse(format!("txa: {}", e)))?;

    match (width, height) {
        (Some(w), Some(h)) => {
            if w == 0 || h == 0 {
                return bail(AppError::Parse, "--width/--height deben ser > 0");
            }
            let (cw, ch) = fit_cells(img.width, img.height, w, h, fit == FitArg::Cover);
            let resized = if (cw, ch) == (img.width, img.height) {
                img
            } else {
                txa::resize(&img, cw, ch)
            };
            match fit {
                FitArg::Contain => Ok(pad_centered(resized, w, h)),
                FitArg::Cover => Ok(pad_around(resized, w, h)),
            }
        }
        (Some(w), None) => {
            if w == 0 {
                return bail(AppError::Parse, "--width debe ser > 0");
            }
            let h = aspect_h(img.width, img.height, w);
            if (w, h) == (img.width, img.height) {
                Ok(img)
            } else {
                Ok(txa::resize(&img, w, h))
            }
        }
        (None, Some(h)) => {
            if h == 0 {
                return bail(AppError::Parse, "--height debe ser > 0");
            }
            let w = aspect_w(img.width, img.height, h);
            if (w, h) == (img.width, img.height) {
                Ok(img)
            } else {
                Ok(txa::resize(&img, w, h))
            }
        }
        (None, None) => Ok(img),
    }
}

// ============================================================================
// Subcomandos
// ============================================================================

/// Espera sin drift hasta `target`: sleep grueso + spin muy corto (últimos
/// ~250 us), para no quemar un núcleo mientras Termux pinta la pantalla.
fn wait_until(target: Instant) {
    loop {
        let now = Instant::now();
        let d = target.saturating_duration_since(now);
        if d.is_zero() {
            return;
        }
        if d > Duration::from_micros(250) {
            std::thread::sleep(d - Duration::from_micros(250));
        } else {
            std::hint::spin_loop();
        }
    }
}

fn cmd_play(
    file: &Path,
    color: ColorArg,
    fps_override: Option<u16>,
    loop_play: bool,
) -> Result<(), AppError> {
    let src = load_play_source(file, fps_override)?;
    let mode = resolve_color(color);
    let _guard = TerminalGuard::enter().map_err(|e: AppError| e)?;
    let mut out = BufWriter::with_capacity(64 * 1024, io::stdout().lock());
    let mut painter = Painter::new(mode);
    // Buffer del fotograma REUTILIZADO (clear + write_all, cero alocaciones
    // por frame): cada frame se arma completo en RAM y se emite con un
    // único write_all + flush al stdout (menos syscalls, tearing cero).
    let mut frame_buf: Vec<u8> = Vec::with_capacity(64 * 1024);

    // Página limpia antes de la primera emisión de frames.
    let _ = out.write_all(b"\x1b[2J");
    let _ = out.flush();

    let mut prev: Option<Vec<Tyxel>> = None;
    let mut start = Instant::now();
    let mut accum_ms: u128 = 0;

    loop {
        for frame in &src.frames {
            // Cadencia efectiva: --fps-override escala TODOS los duration_ms
            // del archivo (factor fps_tag / fps_efectivo), no solo el default.
            let dur_ms: u128 = {
                let raw = if frame.duration_ms == 0 {
                    src.default_ms as f64
                } else {
                    frame.duration_ms as f64
                };
                if let Some(f) = fps_override {
                    if f > 0 && src.fps_tag > 0.0 {
                        (raw * src.fps_tag / f as f64).round().max(1.0) as u128
                    } else {
                        raw as u128
                    }
                } else {
                    raw as u128
                }
            };
            let target = start + Duration::from_millis(accum_ms as u64);
            wait_until(target);

            // Si el reloj real superó la ventana del frame N, NO se emite, pero
            // el slot lógico se consume: el acumulador avanza y el bucle
            // re-sincroniza en el próximo frame con tiempo futuro.
            let now_late = Instant::now().duration_since(target).as_millis();
            if now_late < dur_ms {
                let cur = &frame.cells;
                let expected = src.width as usize * src.height as usize;
                if cur.len() != expected {
                    return bail(
                        AppError::Parse,
                        format!(
                            "frame con {} celdas, se esperaban {} ({}x{})",
                            cur.len(),
                            expected,
                            src.width,
                            src.height
                        ),
                    );
                }
                // ARMADO COMPLETO EN RAM: DEC sync + frame en frame_buf.
                dec_sync_begin(&mut frame_buf).map_err(io_err)?;
                match &prev {
                    Some(p) => {
                        let dirty = count_dirty(p, cur);
                        if (dirty as f64) < DIRTY_MAX_RATIO * expected as f64 {
                            let _ = render_dirty(&mut frame_buf, p, cur, src.width, &mut painter)
                                .map_err(io_err)?;
                        } else {
                            render_full(&mut frame_buf, cur, src.width, src.height, &mut painter)
                                .map_err(io_err)?;
                        }
                    }
                    None => {
                        render_full(&mut frame_buf, cur, src.width, src.height, &mut painter)
                            .map_err(io_err)?;
                    }
                }
                dec_sync_end(&mut frame_buf).map_err(io_err)?;
                // FLUSH ÚNICO por fotograma: la terminal recibe el frame
                // entero en una sola escritura una vez armado.
                out.write_all(&frame_buf).map_err(io_err)?;
                out.flush().map_err(io_err)?;
                prev = Some(cur.clone());
            }
            accum_ms += dur_ms;
        }
        if !loop_play {
            break;
        }
        // Nueva pasada: reiniciamos el reloj maestro; `prev` se conserva para
        // que la transición último->primer frame se renderice por delta.
        start = Instant::now();
        accum_ms = 0;
    }
    out.flush().map_err(io_err)?;
    Ok(())
}

fn cmd_cat(
    file: &Path,
    width: Option<u16>,
    height: Option<u16>,
    fit: FitArg,
    color: ColorArg,
) -> Result<(), AppError> {
    let img = load_txa_fitted(file, width, height, fit)?;
    let mode = resolve_color(color);
    let mut out = BufWriter::with_capacity(64 * 1024, io::stdout().lock());
    let mut painter = Painter::new(mode);
    // Sin alternate screen ni raw mode: impresión directa en la corriente de
    // texto actual (pensado para fastfetch / tuberías contra la terminal).
    dec_sync_begin(&mut out).map_err(io_err)?;
    render_full(&mut out, &img.cells, img.width, img.height, &mut painter).map_err(io_err)?;
    dec_sync_end(&mut out).map_err(io_err)?;
    writeln!(out).map_err(io_err)?;
    out.flush().map_err(io_err)?;
    Ok(())
}

fn cmd_export(
    file: &Path,
    output: &Path,
    width: Option<u16>,
    height: Option<u16>,
    fit: FitArg,
    color: ColorArg,
) -> Result<(), AppError> {
    let img = load_txa_fitted(file, width, height, fit)?;
    let mode = resolve_color(color);
    let mut buf = Vec::with_capacity(64 * 1024);
    {
        let mut painter = Painter::new(mode);
        dec_sync_begin(&mut buf).map_err(io_err)?;
        render_full(&mut buf, &img.cells, img.width, img.height, &mut painter).map_err(io_err)?;
        dec_sync_end(&mut buf).map_err(io_err)?;
        buf.push(b'\n');
    }
    std::fs::write(output, &buf).map_err(io_err)?;
    println!(
        "tyxel-play: exportado {} bytes a {}",
        buf.len(),
        output.display()
    );
    Ok(())
}

/// Rango de codepoints Nerd Font del atlas lógico (Powerline U+E0B0..U+E0BF).
const NERDFONT_CP_RANGE: core::ops::RangeInclusive<u32> = 0xE0B0..=0xE0BF;

/// ¿El glifo (id normalizado al atlas) es del grupo Nerd Font?
fn glyph_is_nerdfont(glyph_id: u16) -> bool {
    let id = glyph_id % atlas::ATLAS_LEN as u16;
    NERDFONT_CP_RANGE.contains(&(atlas::codepoint(id) as u32))
}

fn cmd_info(file: &Path) -> Result<(), AppError> {
    let data = read_file(file)?;
    if is_txv(&data) {
        let txv = Txv::parse(&data).map_err(|e| AppError::Parse(format!("txv: {}", e)))?;
        let h = &txv.header;
        let default_ms = detect_fps_default(h.fps_num, h.fps_den, None);
        let total_ms: u64 = txv
            .frames
            .iter()
            .map(|f| {
                if f.duration_ms == 0 {
                    default_ms
                } else {
                    f.duration_ms as u64
                }
            })
            .sum();
        let n_i = txv
            .frames
            .iter()
            .filter(|f| f.frame_type == txv::FRAME_TYPE_I)
            .count();
        let n_p = txv
            .frames
            .iter()
            .filter(|f| f.frame_type == txv::FRAME_TYPE_P)
            .count();
        let comp = match h.compression {
            txv::Compression::None => "none",
            txv::Compression::Lz4 => "lz4",
            txv::Compression::Zstd => "zstd",
        };
        println!("tipo: tyxel-video (.txv)");
        println!("resolucion: {}x{} celdas", h.width, h.height);
        println!("fps: {}/{}", h.fps_num, h.fps_den);
        println!("compresion: {}", comp);
        println!("frames: {}", txv.frames.len());
        println!("i-frames: {}", n_i);
        println!("p-frames: {}", n_p);
        println!("duracion: {:.2} s", total_ms as f64 / 1000.0);
        println!("tamano: {} bytes", data.len());
        // Riqueza de glifos del video completo (todas las celdas de todos
        // los frames, materializadas por el parser).
        let mut glyphs = HashSet::new();
        let mut total_cells = 0usize;
        let mut flat_cells = 0usize;
        for f in &txv.frames {
            let prev_len = glyphs.len();
            for &t in &f.cells {
                total_cells += 1;
                let id = t.glyph_id % atlas::ATLAS_LEN as u16;
                glyphs.insert(id);
                if id == 0 || id == 1 {
                    flat_cells += 1;
                }
            }
            let _ = glyphs.len() - prev_len;
        }
        println!("glifos_unicos: {}", glyphs.len());
        println!(
            "planas(espacio/bloque): {:.1}%",
            if total_cells > 0 {
                flat_cells as f64 * 100.0 / total_cells as f64
            } else {
                0.0
            }
        );
        println!("atlas_id: {}", h.atlas_id);
        println!(
            "nerd_fonts: {}",
            if h.nerdfont { "requerida" } else { "no" }
        );
        println!("seek_index: {} entradas", txv.index.len());
    } else {
        let text = String::from_utf8_lossy(&data).into_owned();
        let img = txa::from_text(&text).map_err(|e| AppError::Parse(format!("txa: {}", e)))?;
        let total = img.cells.len();
        let flat = img.cells.iter().filter(|t| t.is_flat()).count();
        let mut glyphs = HashSet::new();
        let mut nerd = false;
        for &t in &img.cells {
            let id = t.glyph_id % atlas::ATLAS_LEN as u16;
            glyphs.insert(id);
            if glyph_is_nerdfont(id) {
                nerd = true;
            }
        }
        let flat_pct = if total == 0 {
            0.0
        } else {
            flat as f64 / total as f64 * 100.0
        };
        println!("tipo: tyxel-art (.txa)");
        println!("resolucion: {}x{} celdas", img.width, img.height);
        println!("celdas: {}", total);
        println!("celdas_planas: {:.1}%", flat_pct);
        println!("glifos_distintos: {}", glyphs.len());
        println!("nerd_fonts: {}", if nerd { "requerida" } else { "no" });
        println!("formato: Tyxel-Art {}", tyxel_core::FORMAT_VERSION);
    }
    Ok(())
}

// ============================================================================
// Fase 3: render a MP4 (emulador de terminal virtual + ffmpeg rawvideo)
// ============================================================================

/// Convierte un especto de resolucion (`720p`, `1080p`, `1280x720`, `auto`) en
/// dimensiones del lienzo RGB24. `yuv420p` exige dimensiones pares.
fn parse_res(spec: &str, cols: u16, rows: u16) -> Result<(u32, u32), AppError> {
    let bad = || {
        AppError::Parse(format!(
            "resolucion invalida: '{}' (usa 720p|1080p|WxH|auto)",
            spec
        ))
    };
    let (w, h) = match spec {
        "720p" => (1280_u32, 720_u32),
        "1080p" => (1920, 1080),
        "auto" | "native" => (cols as u32 * 8, rows as u32 * 16),
        _ => {
            let (a, b) = spec.split_once('x').ok_or_else(bad)?;
            (
                a.trim().parse().map_err(|_| bad())?,
                b.trim().parse().map_err(|_| bad())?,
            )
        }
    };
    if w == 0 || h == 0 || w > 8192 || h > 8192 {
        return Err(bad());
    }
    // Redondeo hacia arriba para garantizar paridad de yuv420p.
    let even = |v: u32| v + (v & 1);
    Ok((even(w), even(h)))
}

/// Resuelve el atlas de cobertura: TTF pedido por el usuario, o
/// `~/.termux/font.ttf` si existe, o el atlas procedural 100% Rust.
fn resolve_render_atlas(
    font: Option<&Path>,
    cell_w: usize,
    cell_h: usize,
) -> Result<GlyphAtlas, AppError> {
    let candidates: Vec<PathBuf> = match font {
        Some(p) => vec![p.to_path_buf()],
        None => {
            let mut v = Vec::new();
            if let Some(home) = std::env::var_os("HOME") {
                let p = Path::new(&home).join(".termux").join("font.ttf");
                if p.exists() {
                    v.push(p);
                }
            }
            v
        }
    };
    for p in &candidates {
        let data = match std::fs::read(p) {
            Ok(d) => d,
            Err(_) => continue,
        };
        if let Some(a) = GlyphAtlas::from_font(data, cell_w, cell_h) {
            return Ok(a);
        }
        eprintln!(
            "tyxel-play: aviso: fuente '{}' no legible, fallback procedural",
            p.display()
        );
    }
    Ok(GlyphAtlas::procedural(cell_w, cell_h))
}

/// Opciones del subcomando `render` (agrupadas para clippy::too_many_arguments).
struct RenderOpts<'a> {
    output: &'a Path,
    res: &'a str,
    fps: Option<u32>,
    color: QuantArg,
    font: Option<&'a Path>,
    audio_from: Option<&'a Path>,
    scanlines: bool,
}

/// Un comando de resolucion viene en el argumento --res.
fn cmd_render(file: &Path, opts: &RenderOpts) -> Result<(), AppError> {
    let RenderOpts {
        output,
        res,
        fps,
        color,
        font,
        audio_from,
        scanlines,
    } = *opts;
    let src = load_play_source(file, None)?;
    if fps.iter().any(|f| !(1..=240).contains(f)) {
        return Err(AppError::Parse(format!("fps fuera de rango: {:?}", fps)));
    }
    let fps = fps.unwrap_or((src.fps_tag.round().max(1.0)) as u32);
    let (w, h) = parse_res(res, src.width, src.height)?;
    let geo = fit_geometry(
        src.width as usize,
        src.height as usize,
        w as usize,
        h as usize,
    );
    eprintln!(
        "render: {}x{} celdas -> celda {}x{}, lienzo {}x{} (off {},{} canvas {}x{}) @ {} fps",
        src.width,
        src.height,
        geo.cell_w,
        geo.cell_h,
        geo.out_w,
        geo.out_h,
        geo.off_x,
        geo.off_y,
        geo.cell_w * src.width as usize,
        geo.cell_h * src.height as usize,
        fps
    );
    let canvas_w = geo.cell_w * src.width as usize;
    let canvas_h = geo.cell_h * src.height as usize;
    if canvas_w < 1 || canvas_h < 1 {
        return Err(AppError::Parse(format!(
            "la resolucion {}x{} es demasado pequena para {}x{} celdas",
            w, h, src.width, src.height
        )));
    }
    let atlas = resolve_render_atlas(font, geo.cell_w, geo.cell_h)?;
    let quant = color.quant();

    // ffmpeg consume RGB24 crudo por stdin a cadencia fija.
    let mut cmd = process::Command::new("ffmpeg");
    cmd.arg("-y")
        .args(["-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "rgb24"])
        .args(["-s", &format!("{}x{}", w, h)])
        .args(["-r", &fps.to_string(), "-i", "pipe:0"]);
    if let Some(a) = audio_from {
        if !a.exists() {
            return Err(AppError::Io(format!(
                "--audio-from no existe: {}",
                a.display()
            )));
        }
        cmd.arg("-i")
            .arg(a)
            .args(["-map", "0:v:0", "-map", "1:a:0?"]);
    }
    cmd.args([
        "-c:v", "libx264", "-preset", "veryfast", "-crf", "18", "-pix_fmt", "yuv420p",
    ]);
    if audio_from.is_some() {
        cmd.args(["-c:a", "aac", "-shortest"]);
    }
    cmd.arg(output);
    let mut child = cmd
        .stdin(process::Stdio::piped())
        .stderr(process::Stdio::inherit())
        .spawn()
        .map_err(|e| {
            AppError::Io(format!(
                "no pude lanzar ffmpeg (instalalo con `pkg install ffmpeg`): {}",
                e
            ))
        })?;
    let mut writer = child
        .stdin
        .take()
        .ok_or_else(|| AppError::Io("ffmpeg cerro su stdin".to_string()))?;

    let mut emitted = 0_u64;
    for (i, frame) in src.frames.iter().enumerate() {
        let ms = if frame.duration_ms == 0 {
            src.default_ms
        } else {
            frame.duration_ms as u64
        };
        // La duracion del frame manda: repetir el fotograma hasta cubrirla
        // a la cadencia pedida (minimo 1; duracion 0 => un frame).
        let repeats = (((ms as f64) * (fps as f64) / 1000.0).round().max(1.0)) as u64;
        let buf = compose_frame(
            &geo,
            &frame.cells,
            src.width as usize,
            &atlas,
            quant,
            scanlines,
            tyxel_core::renderer::DEFAULT_BG,
            1,
        );
        for _ in 0..repeats {
            writer.write_all(&buf).map_err(io_err)?;
            emitted += 1;
        }
        if i % 60 == 0 || i + 1 == src.frames.len() {
            eprint!(
                "\rrender: frame {}/{} (emitidos {})   ",
                i + 1,
                src.frames.len(),
                emitted
            );
            let _ = io::stderr().flush();
        }
    }
    eprintln!();
    drop(writer);
    let status = child.wait().map_err(io_err)?;
    let secs = emitted as f64 / fps as f64;
    eprintln!(
        "render: {} frames emitidos ({:.2} s) -> ffprobe:",
        emitted, secs
    );
    if !status.success() {
        return Err(AppError::Io(format!(
            "ffmpeg termino con {:?}",
            status.code()
        )));
    }
    let out = process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,codec_name,nb_frames",
        ])
        .args(["-of", "csv=s=x:p=0"])
        .arg(output)
        .output()
        .map_err(io_err)?;
    eprintln!("{}", String::from_utf8_lossy(&out.stdout).trim_end());
    Ok(())
}

// ============================================================================
// main
// ============================================================================

fn run() -> i32 {
    // CLI correcto: morir con SIGPIPE estándar (sin pánico) si el consumidor
    // del stdout se cierra (ej. `| head`).
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let cli = Cli::parse();
    let res = match cli.cmd {
        Cmd::Play {
            ref file,
            color,
            fps_override,
            loop_play,
        } => cmd_play(file, color, fps_override, loop_play),
        Cmd::Cat {
            ref file,
            width,
            height,
            fit,
            color,
        } => cmd_cat(file, width, height, fit, color),
        Cmd::Export {
            ref file,
            ref output,
            width,
            height,
            fit,
            color,
        } => cmd_export(file, output, width, height, fit, color),
        Cmd::Info { ref file } => cmd_info(file),
        Cmd::Render {
            ref file,
            ref output,
            ref res,
            fps,
            color,
            ref font,
            ref audio_from,
            scanlines,
        } => cmd_render(
            file,
            &RenderOpts {
                output,
                res,
                fps,
                color,
                font: font.as_deref(),
                audio_from: audio_from.as_deref(),
                scanlines,
            },
        ),
    };
    match res {
        Ok(()) => 0,
        Err(AppError::Parse(msg)) => {
            eprintln!("tyxel-play: error de formato: {}", msg);
            EXIT_PARSE
        }
        Err(AppError::Io(msg)) => {
            eprintln!("tyxel-play: error de I/O: {}", msg);
            EXIT_IO
        }
    }
}

fn main() {
    process::exit(run());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fps_defaults() {
        assert_eq!(detect_fps_default(30, 1, None), 34);
        assert_eq!(detect_fps_default(60, 1, Some(30)), 34); // override gana
        assert_eq!(detect_fps_default(0, 0, None), 33); // degenerado -> ~30fps
        assert_eq!(detect_fps_default(10, 1, None), 100);
    }

    #[test]
    fn glyph_nerdfont_range_guard() {
        // El id se normaliza módulo ATLAS_LEN; fuera de rango no debe panic.
        assert!(!glyph_is_nerdfont(u16::MAX));
    }
}
