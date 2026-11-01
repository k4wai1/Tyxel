# DECISIONS.md — Registro de Decisiones Técnicas (ADR)

Formato: ID, Estado, Contexto, Decisión, Consecuencias. Una vez decidido, **no se re-debate**.

## ADR-001 — Pure Rust obligatorio (lz4_flex + ruzstd)
* **Estado:** ACEPTADO
* **Contexto:** Compilación en Termux aarch64-android sin NDK pelearse con `cc`/`cmake`.
* **Decisión:** Prohibido `*-sys` crates con C. Compresión: `lz4_flex` (enc/dec) primario;
  `ruzstd` solo decodificación si se necesita compatibilidad.
* **Consecuencias:** throughput algo menor que zstd/lib C, pero builds limpios.

## ADR-002 — Struct `Tyxel` de 8 bytes con `#[repr(C, align(8))]`
* **Estado:** ACEPTADO
* **Contexto:** comparación de celdas para delta-frames debe ser O(1); UB por `packed`.
* **Decisión:** glyph_id u16 + fg 3 B + bg 3 B, alineado a 8; conversión con
  `u64::from_ne_bytes` (sin bytemuck en Fase 1).
* **Consecuencias:** `glyph_id` limita al atlas a 512 (coherente).

## ADR-003 — Endianness de bitmask: bit = row*8 + col, fila 0 = LSB
* **Estado:** ACEPTADO
* **Contexto:** documentar claramente la geometría de la máscara `u64`.
* **Decisión:** fila 0 (superior de la celda) en los 8 bits menos significativos.
* **Consecuencias:** helper `mask_bit()` canónico en `atlas.rs`; cualquier otra
  convención es bug de integración.

## ADR-004 — Atlas lógico fijo de 512 índices
* **Estado:** ACEPTADO
* **Contexto:** no escanear 10k iconos de Nerd Fonts; 4 KB caben en L1.
* **Decisión:** `[u64; 512]` alineado a 64 B; Braille con trampa neutralizada
  (puntos a 50 % de densidad con margen).
* **Consecuencias:** juega bien con `glyph_id: u16`.

## ADR-005 — Motor de matching XOR/POPCNT lineal branchless
* **Estado:** ACEPTADO
* **Contexto:** el pre-filtro popcount ± 4 descarta glifos correctos cuando la
  máscara objetivo tiene mucho ruido; la búsqueda branchless sí gana a HashMaps
  en ARM64.
* **Decisión:** loop lineal de 512 con `XOR + count_ones` doble (normal/invertida);
  detrás de caché direct-mapped 64K `Box<[(u64, u16, bool); 65536]>`.
* **Consecuencias:** determinista O(512) por celda con hit-rate alto tras calentarse.

## ADR-006 — Corrección de aspecto 1:2 en el muestreo
* **Estado:** ACEPTADO
* **Contexto:** las celdas de terminal miden el doble de alto que de ancho.
* **Decisión:** imagen de entrada redimensionada a (W*8) x (H*16) y muestreada a
  cuadrillas 8×8 lógicas por celda.
* **Consecuencias:** las máscaras se comparan en un espacio cuadrado.

## ADR-007 — Degradación de color con Bayer 4×4 en 16 colores
* **Estado:** ACEPTADO
* **Contexto:** terminales limitadas necesitan legibilidad en 16 colores.
* **Decisión:** ANSI 256 por cubo 6×6×6 + 24 grises entero rápido; 16/mono por
  quantización con Ordered Dithering (matriz Bayer 4×4) antes de elegir el índice ANSI 16.
* **Consecuencias:** determinista y sin dependencias.

## ADR-008 — Video: `.txv` guarda I-Frames completos (8 B/celda) + P-Frames RLE con
Skip/Repeat/Literal
* **Estado:** ACEPTADO
* **Contexto:** seek O(1) y streaming en una pasada.
* **Decisión:** índice de I-Frames al final (timestamp_ms, byte_offset) + footer 16 B.
* **Consecuencias:** specs en FORMAT_SPEC.md §2.

## ADR-009 — Atlas: exclusión de sextantes duplicados y extras de relleno
* **Estado:** ACEPTADO
* **Contexto:** al validar con tests se detectaron máscaras duplicadas entre familias:
  v=3/12/21/42/48 de sextantes duplican ▀/─/▌/▐/▃ y v=4/8 duplican ╴/╶.
* **Decisión:** sextantes válidos = 53 (v ∈ 1..62 menos {3,4,8,12,21,42,48,60,61});
  el hueco que queda se rellena con 25E1 (anillo inferior) y ◆ 25C6 (rombo propio)
  además de 25E0/25E6/25AA en el bloque "extras" (IDs 196..=200).

## ADR-010 — Semántica de Ops delta en `.txv`
* **Estado:** ACEPTADO
* **Contexto:** la primera versión de `apply_frame` copiaba los Skips con un cursor
  "solo-skips", rompiendo la correspondencia posicional cuando un Literal/Repeat
  se intercalaba entre dos Skips (detectado por golden test).
* **Decisión:** Skip(n) en la posición absoluta `p = out.len()` copia
  `prev[p..p+n]`: las ops describen la reconstrucción fila a fila con la misma
  geometría que emite el codificador greedy (runs de cambios + RLE interno +
  fusión de repeticiones que también sea válida aunque cur[i]==prev[i]).

## ADR-011 — Formato `.txa`: tokens de celda sin ambigüedad
* **Estado:** ACEPTADO
* **Contexto:** el glifo espacio (U+0020) es imposible de colocar como token suelto
  separado por espacios.
* **Decisión:** cada celda es un token `U+XXXX#RRGGBB[#RRGGBB][*N]`; el RLE `*N`
  puede ir pegado al token o como token propio, y las filas se separan con `\t`
  (writer) o espacios (parser). El RLE se emite solo para carreras >= 3.
