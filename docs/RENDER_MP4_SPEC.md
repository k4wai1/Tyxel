# Especificación de RENDER: .txa/.txv -> MP4 (Fase 3)

La idea central: emular la terminal **en RAM**. El renderer compone imagen RGB24
en tiempo real según la librería `tyxel-core::renderer`, decide la resolución de
la terminal virtual, elige glifos reales (TTF o procedural) y alimenta el codec
H.264 de ffmpeg por stdin — con las mismas Swords que haría el reproductor TUI.

## Pipeline
```
.txv/.txa ──decode──> frames (cells)
                           │
                           ▼
             fit_geometry(cols, rows, W, H)   # celda 1:2, múltiplo entero, centrado
                           │
                           ▼
             GlyphAtlas(size: celda real)     # TTF (--font, default ~/.termux/font.ttf)
                                              #   o procedural 100 % Rust (super-sample 4x4)
                           ▼
             compose_frame(...)               # RGBA blend, multicore (bandas por fila)
                           ▼
             ffmpeg rawvideo rgb24 pipe:0 -> libx264 (veryfast, crf 18, yuv420p)
                           ▼
                   MP4 (opcional: pista de audio de --audio-from)
```

## Contrato de geometria
- `fit_geometry(cols, rows, target_w, target_h)` produce la celda entera más
  grande `cw x ch` con `ch == 2*wc` (fidelidad aspect 1:2) que cabe; padding
  centralizado y pintado con `tyxel_core::renderer::DEFAULT_BG`
  ([0:0x0D, 11, 17]).
- Los 512 glifos usan UNA cobertura `alpha 0..=255` permitir blend de colores:
  bug criticado del texto sólido queda prohibido (ahora antialiasing efectivo).
- Enabled: fusiones por hilo en bandas de FILAS completas
  (una banda no se toca en otros hilos) => composición multicore garantizada.

## Flags CLI en `tyxel-play render`
| flag | valores | efecto |
|------|---------|--------|
| file | `.txa`/`.txv` | fuente de estructuras Tyxel |
| -o/--output | path `.mp4` | destino |
| --res | `720p`, `1080p`, `1280x720`, `auto` | lienzo de la terminal virtual (default `1080p`); espacios pares de yuv420p |
| --fps | número | cadencia de salida; default: `terminación del archivo` (fps tag; .txa = 30) |
| --color | truecolor/256/16/mono | paleta de composición (default truecolor) |
| --font | path .ttf | sube el motor de glifos TTF vía **fontdue** (100 % Pure Rust) |
| --font (sin flag) | (auto) | usa `~/.termux/font.ttf` si existe, sino procedural |
| --audio-from | path mp4 | `ffmpeg -map 0:v:0 -map 1:a:0? -c:a aac -shortest` |
| --scanlines | (flag) | atenúa la última fila lógica de celda a 84 % (215/255) |

## Semántica de duración
Cada entrada del archivo `.txv` (I o P) guarda `duration_ms`:
```
repeats = round(duration_ms * fps / 1000), mínimo 1
```
Esta es la codificación EXACTA de los skips de marcos: si el `.txv` es
30 fps y se pide render con `--fps 60`, cada marco repetido durará
`repeats` marcos de 16.667 ms → mismo tiempo de pantalla e incluso más suave.

- `.txa` = 1 marco de `default_ms` con el `fps` implícito 30.
- El encabezado del [`seek index`] del .txv NO se usa aquí (no hay jumps).

## Convenciones de color
- TrueColor: RGB puro del mensaje, compat con fg/bg.
- 256 colores: vía `tyxel_core::tyxel::rgb_to_ansi256` -> re-mapeo a RGB (en renderer).
- 16 colores: Bayer 4×4 + `rgb_to_ansi16_dithered`.
- Mono: si mean_luma del Tyxel < 0.5 → [255,255,255] sobre [0,0,0]; inverso
  etc. — ver `renderer::effective_colors`.
