# PROGRESS.md — Bitácora de estado del proyecto (fuente de la verdad)

> Regla: actualizar este archivo ANTES de iniciar cualquier tarea y justo después
> de que `cargo test` pase en verde.

Fecha de último estado: 2026-10-31

## Estado global: FASE 2 COMPLETA — proyecto funcional de punta a punta

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
| tyxel-play (CLI reproductor) | LISTA | subcomandos play/cat/export/info; TerminalGuard RAII con degradación sin TTY; DEC 2026; caché fg/bg por fila; dirty-cells < 35 %; frame-skipping; color auto/truecolor/256/16/mono con Bayer |
| tests/golden_tests.rs (integración core) | LISTA | 8 tests |
| tests/cli_e2e.rs (integración CLI en tyxel-play) | LISTA | PNG sintético en memoria → enc img → info/export/play; 2 tests |

## Tests pasando
* `cargo test --workspace`: 52 tests en verde (40 lib core + 8 golden + 2 unit play + 2 e2e CLI).
* clippy --workspace: sin errores; ~18 warnings de estilo en const-fns de
  tyxel-core (`is_multiple_of`/`div_ceil` no son const-stables) — no bloquean.
* Humo manual: PNG 64×64 → `.txa` → `info` (64x32, 3.3 % planas, 7 glifos),
  `cat --width 40 --color 256` (ANSI válido con DEC 2026), `export` (78.5 KB),
  img → `.txv` → `play --fps-override 60` termina limpio sin TTY.

## Bloqueos
* ninguno.

## Posibles siguientes pasos (v1.1)
* Streaming .txv real (TxvReader incremental en vez de parse completo) para
  archivos grandes / streams en vivo.
* Compresión Zstd vía CLI externa (flag reservado en el header).
* Export `.txa` → PNG/virtual ya existe parcial (virtual_pixels); exponer
  `tyxel-play export --png`.
EOF
