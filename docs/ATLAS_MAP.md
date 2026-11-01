# ATLAS_MAP.md — Mapa del Atlas Lógico v1 (512 glifos)

> Fuente de la verdad de los rangos de IDs del atlas. Sub-Agente A lo mantiene
> sincronizado con `atlas.rs`. Endianness de la matriz: **bit = (row * 8) + col**
> (fila 0 = superior, en los 8 bits menos significativos; ver ARCHITECTURE.md §2).

## Reglas globales

* Longitud fija: 512 glifos. Todos con `unicode-width == 1`.
* Tabla `MASKS: [u64; 512]` alineada a 64 bytes (4 KB -> L1).
* ID 0: espacio U+0020, máscara `0x0`.
* ID 1: bloque completo U+2588, máscara `0xFFFF_FFFF_FFFF_FFFF`.
* Ningún otro glifo tiene máscara 0x0 ni máscara de bloque completo (evita duplicados
  tipo ⣿ que competirían con █).
* Glifos "template" de un rango cuyo codepoint es un hueco de Unicode (o el propio
  hueco no tiene dibujo en fuentes) pueden rellenarse de forma programática siempre
  que se documente la lógica aquí. Ante conflicto pureza/512: se rellena con dibujo
  procedural sintético documentado.

## Distribución de rangos de ID (IMPLEMENTADA — atlas.rs verde)

| Rango ID | Código Unicode | Cantidad | Familia / lógica |
|---|---|---|---|
| 0 | U+0020 | 1 | espacio vacío, máscara `0` |
| 1 | U+2588 | 1 | bloque completo `0xFFFF_FFFF_FFFF_FFFF` |
| 2-129 | U+2500-U+257F | 128 | Box Drawing procedural: tallos centrales 2 bits (peso simple/doble/heavy), juntas y tees, discontinuas `dash_h/dash_v` y `dash2_v/h` (254C-254F), diagonales de pendiente exacta 1:1, dobles discontinuas |
| 130-160 | U+2580-U+259F (sin 2588) | 31 | Block Elements: mitades exactas, octavos (tramos), ▐ (0x10), shades ░▒▓ procedural con densidades 1/4, 1/2, 3/4, 2594-2595, cuadrantes UL/UR/LL/LR y combinaciones oficiales 2596-259F (incl. checkerboards ▚ 259A = UL+LR y ▞ 259E = UR+LL; 259D = UR solo) |
| 161-164 | U+25E2-U+25E5 | 4 | Triángulos con hipotenusa progresiva `r+c == 7` |
| 165-180 | U+E0B0-U+E0BF | 16 | Powerline: cuñas escalonadas incorporadas (`crown_mask`), triángulos y flechas |
| 181-195 | U+25D0-U+25DE | 15 | Círculos a distancia: discos/medios/cuartos (`d2 <= 52`), anillos `16 < dd <= 52`, 25D9 = anillo + punto central 2×2 |
| 196-200 | extras | 5 | U+25E6 (bulle, dd 4..18), U+25AA (cuadrado interior 4×4), U+25E0/25E1 (anillo por mitades), U+25C6 (◆ rombo \|(2r-7)\|+\|(2c-7)\| <= 8) |
| 201-454 | U+2801-U+28FE | 254 | Braille: punto = sub-celda 4×2 con SOLO 2 bits centrales encendidos (50 % densidad, márgenes garantizados); 2800/28FF excluidos (28FF duplicaría ║) |
| 455-507 | U+1FB00-U+1FB3B | 53 | Sextantes estándar: v = celdas encendidas (bit0=celda1 arriba-izq), bandas de filas [3, 2, 3] y columnas [4, 4]; excluidos v ∈ {0, 3, 4, 8, 12, 21, 42, 48, 60, 61, 63} por hueco estándar o duplicado (v=3/12/21/42/48 duplican ▀/─/▌/▐/▃ y v=4/8 duplican ╴/╶) |
| 508-511 | U+1FB78/79/7B/7C | 4 | Octantes derechos heredados (mitades de columna derecha por cuartos) |

Notas:
* `NERDFONT_COUNT = 16` (Powerline E0B0..E0BF); el resto son glifos de bloque
  universalmente disponibles, de modo que el atlas funciona incluso sin Nerd Fonts
  completas (solo 16 glifos dependen de fuentes ricas).
* Los dobles discontinuas 254C-254F usan edificios Edouble/Vdouble con huecos
  ("dobles discontinuas", interpretación procedural documentada).

(Según lo que decida el sub-agente A puedan cambiar cuotas internas de rango, pero
el orden de familias y el **techo de 512 glifos fijos** no cambia.)

## Tabla de referencia para el matcher

El matcher busca glifos por máscara, y el render emite el codepoint vía
`CODEPOINTS[tyxel.glyph_id]`. Para integración fastfetch/`.txa` existen helpers:

* `atlas::codepoint(id) -> char`
* `atlas::id_of_codepoint(c) -> Option<u16>`
* `atlas::mask(id) -> u64`

## Tests de invariante (IMPLEMENTADOS en `atlas.rs`)

Todos en verde (`cargo test -p tyxel-core`):

1. `MASKS[0] == 0` y `MASKS[1] == u64::MAX`.
2. Para `i in 2..512`: máscara no vacía y no completa.
3. Máscaras únicas entre los 512 glifos (sin duplicados de Hamming).
4. Todos los codepoints tienen `unicode-width == 1`.
5. Sextantes: `mask_count(id) == MASKS[id].count_ones()` y bandas de filas
   {0..2},{3..4},{5..7} por columnas {0..3}/{4..7}.
6. Braille: bits únicamente en columnas dc*4+1..dc*4+2 y filas dr*2..dr*2+2
   (margen garantizado, nada en filas 0/7).
