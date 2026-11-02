# PROGRESS.md — Bitácora de estado del proyecto (fuente de la verdad)

> Regla: actualizar este archivo ANTES de iniciar cualquier tarea y justo después
> de que `cargo test` pase en verde.

Fecha de último estado: 2026-11-02

## Estado global: FASE 3 COMPLETA (render .txv/.txa -> MP4) + Fase 2 cerrada

Nuevo: subcomando `tyxel-play render` con emulador de terminal virtual en RAM (tyxel-core/src/renderer.rs), glifos TTF vía fontdue o procedural 100% Rust, composición multicore por bandas, ffmpeg rawvideo por stdin, audio opcional, scanlines, quantización truecolor/256/16/mono.



## Módulos

| Módulo | Estado | Notas |
|---|---|---|
| Fase 0: Workspace + docs base + lib.rs esqueleto | LISTA | workspace resolver=2, 3 crates, docs/ completos |
| tyxel-core/tyxel.rs (struct 8 B, cuantización) | LISTA | ansi256/ansi16/Bayer incluidos |
| tyxel-core/atlas.rs (512 máscaras) | LISTA | censo exacto 512, máscaras únicas, width==1 |
| tyxel-core/matcher.rs (bipartición + XOR/POPCNT + caché 64K) | LISTA | match_block/match_frame (aspecto 1:2) |
| tyxel-core/txa.rs (texto + RLE `*N` + resizer) | LISTA | tokens `U+XXXX#RRGGBB[#RRGGBB][*N]` (+ literal Unicode de ancho 1, ADR-011) |
| tyxel-core/txv.rs (contenedor binario) | LISTA | header 32 B, I/P deltas, LZ4, Seek Index + TYXELEND |
| tyxel-enc (CLI codificador) | LISTA | subcomandos img/gif/video; calidad GIF por delay real; dedup de frames idénticos con hold_ms; keyframe interval 120; ffmpeg rawvideo subproceso |
| tyxel-play (CLI reproductor) | LISTA | subcomandos play/cat/export/info/render; TerminalGuard RAII con degradación sin TTY; DEC 2026; caché fg/bg por fila; dirty-cells < 35 %; frame-skipping; flush único por frame; color auto/truecolor/256/16/mono con Bayer |
| tyxel-core/renderer.rs (Fase 3) | LISTA | fit_geometry (celda 1:2 múltiplo entero), GlyphAtlas procedural/TTF (fontdue), compose_frame multicore por bandas, quant 256/16-Bayer/mono, scanlines; ver docs/RENDER_MP4_SPEC.md |
| tests/golden_tests.rs (integración core) | LISTA | 8 tests |
| tests/cli_e2e.rs (integración CLI en tyxel-play) | LISTA | PNG sintético en memoria → enc img → info/export/play + E2E render→MP4 con validación ffprobe (3 tests) |

## Tests pasando
* `cargo test --workspace`: 60 tests en verde (47 lib core + 8 golden + 2 unit play + 3 e2e CLI; los 5 r-units nuevos de renderer.rs incluidos en los 47).
* clippy --workspace: sin errores; ~18 warnings de estilo en const-fns de
  tyxel-core (`is_multiple_of`/`div_ceil` no son const-stables) — no bloquean.
* Humo manual: PNG 64×64 → `.txa` → `info` (64x32, 3.3 % planas, 7 glifos),
  `cat --width 40 --color 256` (ANSI válido con DEC 2026), `export` (78.5 KB),
  img → `.txv` → `play --fps-override 60` termina limpio sin TTY.

## Bloqueos
* ninguno.

## Ronda 2 de depuración (fidelidad + multicore + FPS): 2026-11-01

1. Matcher (tyxel-core/src/matcher.rs) reescrito en su núcleo:
   - Umbral plano drástico: solo celda plana si max-min <= 5 en los 3 canales
     (antes <= 12); gradientes suaves y bordes medios ya van al atlas.
   - Bipartición multi-eje: canal dominante + luminancia perceptual
     (77R+150G+29B); ambos targets compiten.
   - Selección final por error cromático REAL (recolor + SAD-RGB) entre los
     top-4 de Hamming; un glifo fino puede ganarle al bloque lleno.
   - Resultado: .txa de humo con 7 glifos distintos -> .txv reales con
     397-456 glifos únicos; planas 3-29% (antes ladrillos).
   - test rich_frame_uses_many_glyphs (>= 40 glifos) y
     parallel_matches_serial (determinismo del pool).
2. Multicore: `Matcher::match_frame_parallel(pool, w, h, px)` con
   std::thread::scope; pool de 8 matchers creado UNA sola vez por ejecución
   (img/gif/video reutilizan el pool entre frames). El 1080p codificó a
   ~500% CPU (5 de 8 núcleos) en el G99.
3. FPS: `--fps` opcional en `tyxel-enc video`; sin valor se detecta el
   framerate nativo con ffprobe (avg_frame_rate) capado a 30. Los 4 videos
   de Test/ re-codificados a 30 FPS nativos.
4. Rotación: probe del display matrix; intercambia W/H con ±90/±270.
5. `--fps-override` del player reparado: escalaba solo el default_ms y no
   los duration_ms de los I/P-frames; ahora escala TODAS las cadencias
   (factor fps_tag/fps_efectivo).
6. spin del wait_until: 2 ms -> 250 us (no quema un núcleo mientras Termux
   pinta).
7. `tyxel-play info` (.txv) ahora reporta glifos_unicos + % planas.
8. Método de medición: `play --fps-override 240` sobre los .txv reales:
   234-240 FPS conseguidos (la cadencia pedida se respeta; motor sobra).

## Prueba de fuego con videos reales de Test/ (bugfix round 2026-11-01)
Bugs encontrados y corregidos al codificar los 4 videos .mp4 de Test/:
1. `txxel-enc video` exigía `--width` y `--height` a la vez — añadido sondeo
   con `ffprobe` + `derive_cells` para aspecto automático (solo una de las dos
   dimensiones, o ninguna). `--height 20` sobre 1920x1080 → 71x20 exacto.
2. `delta_ops` (tyxel-core/src/txv.rs): la rama del Literal cortaba con
   `seg[0..j]` en vez de `seg[lit_start..j]`, duplicando el prefijo del
   segmento tras un Repeat → frames cortos (998/1000) y glitch visual.
   Este bug no salía en tests golden por falta de patrones Repeat+Literal.
3. `tyxel-enc` escribía `Compression::None` aunque el core soporta LZ4 —
   ahora `Compression::Lz4` (bit flags 3-4).
4. Pánico "failed printing to stdout: Broken pipe" al hacer `tyxel-play info
   file | head` — ambos binarios instalan `SIGPIPE=SIG_DFL` al arrancar.
5. Termux: ffmpeg/ffprobe rotos por libs viejas (`libplacebo 7.360.1`,
   `libx265 4.2`); `pkg upgrade libplacebo libx265` los repara.

También: filtro ffmpeg `scale=WxH:flags=area` (anti-aliasing en downscale).

Resultados de los 4 videos (todos `--height 20 --fps 15`, todos `play` OK):
| video | celdas | frames | I/P | duración | .txv |
|---|---|---|---|---|---|
| 720x576 | 50x20 | 602 | 5/597 | 39.7 s | 3.4 MB |
| 1920x1080 | 71x20 | 737 | 7/730 | 49.0 s | 6.8 MB |
| 1280x720 | 71x20 | 198 | 2/196 | 13.1 s | 2.1 MB |
| YouCut (portrait) | 80x20(?) | 343 | 3/340 | 22.6 s | 3.2 MB |


## Fase 3: `tyxel-play render` (.txv/.txa -> MP4): 2026-11-02 COMPLETA

* `tyxel-core/src/renderer.rs` NUEVO: `fit_geometry` (celda 1:2 entera maximal
  centrada, background #0D1117), `GlyphAtlas::procedural` (super-sampling 4x4,
  alphas 0..=255, ~576 KB con celda 24x48) y `GlyphAtlas::from_font` (fontdue
  0.9, 100 % Pure Rust; baseline al 80 %, centrado horizontal), `effective_colors`
  (truecolor / 256 / 16-Bayer / mono) y `compose_frame` multicore (thread::scope
  + bytes en bandas disjuntas, RUM en buf reutilizable, blending entero).
* `tyxel-play render` NUEVO: flags --res 720p|1080p|WxH|auto (default 1080p),
  --fps (default del archivo; .txa=30), --color, --font (auto ~/.termux/font.ttf),
  --audio-from (map v:0 + 1:a:0? ; aac + -shortest), --scanlines (ultima fila
  logica al 84 %). Feed ffmpeg: `-f rawvideo -pix_fmt rgb24 -i pipe:0 -c:v
  libx264 -preset veryfast -crf 18 -pix_fmt yuv420p`.
* Duracion exacta: repeats = round(duration_ms * fps / 1000) min 1 (un marco
  .txv de 33 ms a --fps 60 => 2 ticks, sin alterar la velocidad percibida).
* Smoke real (Termux):
  * icon.jpg (40x20) -> txa -> MP4 720p, ffprobe h264 1280x720 x1.
  * 720x576.txv (1200 frames, audio): MP4 1280x720x1200 + pista aac OK,
    342 % CPU multicore, 1:08 para 40 s de video.
  * 1280x720.txv --res auto --color 16 --scanlines: 424x240x393 OK.
  * YouCut.txv 2:1 --res 1280x640 --color mono --fps 24: cadencia rescaldada OK.
* Tests: 5 unitarios en renderer.rs (geometria, composicion central/padding,
  paralelo == serial, paletas cuantizadas, scanlines) + E2E render -> ffprobe.
  Total workspace: 60 tests verdes. Cargo.toml de tyxel-core gana `fontdue = "0.9"`.

## Posibles siguientes pasos (v1.1)
* Streaming .txv real (TxvReader incremental en vez de parse completo) para
  archivos grandes / streams en vivo.
* Compresión Zstd vía CLI externa (flag reservado en el header).
* Export `.txa` → PNG/virtual ya existe parcial (virtual_pixels); exponer
  `tyxel-play export --png`.
EOF
