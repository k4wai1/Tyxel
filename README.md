# Tyxel - Typographic Pixel

Tyxel converts images and videos into typographic cell matrices for the
terminal. Each cell (a Tyxel) stores one Unicode glyph (Unicode block
elements, box drawing, sextants, Powerline and Braille for Nerd Fonts) plus its
foreground (`fg`) and background (`bg`) TrueColor values. Every cell is encoded
as an 8x8 sub-pixel grid packed into a single `u64`
(`#[repr(C, align(8))] struct { glyph_id: u16, fg: [u8;3], bg: [u8;3] }`),
matched against a 512-glyph atlas with XOR + POPCNT and a 1:2 aspect ratio fix
(terminal cells are twice as tall as wide).

The project is 100 percent Pure Rust: no `*-sys` crates, no C linking anywhere.
The only external binary is `ffmpeg`, used as a subprocess for video sources.

## Build

```bash
cargo build --release
```

Binaries land in `target/release/tyxel-enc` and `target/release/tyxel-play`.

Note: video encoding requires `ffmpeg` on the system (Termux: `pkg install ffmpeg`).

## Room for play: Test folder

The repository ships four real videos plus their ready-made `.txv` files
(encoded at native 30 FPS, 20 rows tall, aspect ratio preserved):

```bash
# Play
./target/release/tyxel-play play Test/X_2039433098266578944_20261029_130834_720x576.txv
./target/release/tyxel-play play Test/X_2111490806053818369_20261018_093309_1920x1080.txv
./target/release/tyxel-play play Test/X_2113639704797356033_20261025_160307_1280x720.txv
./target/release/tyxel-play play Test/YouCut_20261101_061810107.txv

# Inspect: dims, FPS, I/P frame counts, unique glyph count, flat cells ratio
./target/release/tyxel-play info Test/X_2113639704797356033_20261025_160307_1280x720.txv

# Speed control: rescales the cadence of every frame
./target/release/tyxel-play play Test/X_2113639704797356033_20261025_160307_1280x720.txv --fps-override 60

# Print a static .txa to the terminal / export ready-to-consume ANSI
./target/release/tyxel-play cat Test/logo.txa
./target/release/tyxel-play export Test/logo.txa -o logo.ansi

# Render a .txv/.txa to a real MP4 (virtual terminal emulator + H.264)
./target/release/tyxel-play render Test/video.txv -o out.mp4 --res 1080p --fps 30 \
  [--font my.ttf] [--color 256|16|mono] [--audio-from original.mp4] [--scanlines]
```

## Encoding

```bash
# Still images -> .txa (plain text) or .txv (binary container)
./target/release/tyxel-enc img photo.png -o photo.txa
./target/release/tyxel-enc img photo.png -o photo.txv

# Animated GIF (per-frame delay respected)
./target/release/tyxel-enc gif anim.gif -o anim.txv

# Video, automatic aspect ratio (pass only --height or only --width)
./target/release/tyxel-enc video --video clip.mp4 --height 20 -o clip.txv
```

## File formats

| Format | Purpose | Highlights |
| --- | --- | --- |
| `.txa` | static art, plain text | per-cell tokens `U+XXXX#RRGGBB[#RRGGBB][*N]`, RLE rows, human editable, pipe friendly |
| `.txv` | video/animation, binary | 32-byte LE header, I-frames, P-frames with `Skip/Repeat/Literal` deltas, LZ4 payloads, O(1) seek index trailer |

## Player features

- DEC Synchronized Output for tearing-free frames via one atomic `write_all`
  per frame (the whole ANSI frame is built in RAM, then flushed once).
- Dirty-cell rendering: when fewer than 35 percent of cells change, only those
  get cursor-positioned SGR updates.
- Live color degradation: TrueColor to 256 colors to 16 colors with ordered
  Bayer dithering to monochrome, chosen automatically from `COLORTERM`/`TERM`.
- `TerminalGuard`: RAII restoration of raw mode, cursor, alt-screen and colors,
  plus SIGINT/SIGTERM handlers.

## Multicore encoding

Block-to-cell matching is split across a reusable per-thread pool of `Matcher`s
(`std::thread::scope`), one cache per thread; the pool is created once per run.
On a Helio G99 the 1080p encode reaches about 500 percent CPU and the playback
pipeline sustains more than 200 FPS with `--fps-override`.

## Docs (living documentation in `docs/`)

- `ARCHITECTURE.md`: memory contracts, u64 bit endianness, pipeline
- `FORMAT_SPEC.md`: byte-level `.txa`/`.txv` v1.0 specification
- `ATLAS_MAP.md`: the 512-glyph atlas map (ID ranges to codepoints)
- `PROGRESS.md`: project state log
- `DECISIONS.md`: ADR records

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
