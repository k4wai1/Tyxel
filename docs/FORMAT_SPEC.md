# FORMAT_SPEC.md — Especificación de formatos de archivo Tyxel (v1.0)

> Fuente de la verdad byte a byte de `.txa` y `.txv`. Sub-Agente C mantiene este
> documento sincronizado con `txa.rs` / `txv.rs`. Multipartes: todo little-endian
> salvo indicación contraria.

## 1. Formato `.txa` (Tyxel Art — texto plano, estático, editable)

Diseñado para arte estático, edición manual, tuberías UNIX y logos de fastfetch.

### 1.1. Reglas generales

* Sin coordenadas `[x,y]` redundantes ni separadores `|` innecesarios.
* Orden **row-major** (fila por fila).
* Cada celda guarda: `glyph` (carácter del atlas) + `#RRGGBB` de FG + `#RRGGBB` de BG.
* Cada fila del lienzo se separa con la directiva `#ROW <n>` o línea en blanco.
* Las celdas de una fila se separan con espacio o tabulador `\t`.
* RLE textual: sufijo `*N` al final del grupo de celdas para repetir la misma
  celda N veces consecutivas (blanco/Glyph espaciado incluido).
* El glifo especial "espacio vacío" (glyph_id = 0) se escribe como un único
  carácter de espacio que aún así porta colores.

### 1.2. Gramática (implementada en `txa.rs`, ver ADR-011)

```
Archivo    := magic, header*, '---', rows
magic      := "#!TYXEL-ART v1.0"
header     := '#RES' WxH            (obligatorio, columnas x filas)
            | '#GRID 64bit'         (obligatorio)
            | '#COLOR' truecolor|256|16|mono
            | '#NERDFONT' required|optional
            | '#ATLAS' logical-v1   (obligatorio: atlas_id = 0)
rows       := fila (una por línea tras '---'; '#ROW n' se acepta y se omite)
fila       := celda (sep celda)*        sep := espacios/tabuladores
celda      := 'U+' hex4 '#RRGGBB' ['#RRGGBB'] ('*' N)?
            | '*' N                      (repite la última celda N-1 veces más)
hex4       := código Unicode del glifo del atlas (p. ej. U+0020 = espacio)
```

Reglas de compatibilidad (ADR-011):

* El **glifo siempre se escribe `U+XXXX`** (evita la ambigüedad del espacio
  U+0020 como token separado por blancos y hace el archivo operativo en tuberías).
* `from_text` también acepta el **carácter Unicode literal** del glifo (multibyte,
  p. ej. un token `█#RRGGBB`) cuando no es espacio; `to_writer` siempre emite
  `U+XXXX`.
* El RLE `*N` puede ir **pegado al token** (`U+2588#FF0000#000000*12`) o como
  token propio (`*12`); se emite solo para carreras >= 3.
* Si se omite el BG, toma el valor del FG (celda plana).

### 1.3. Ejemplo canónico

Salida real de `to_writer` (tokens `U+XXXX`):

```
#!TYXEL-ART v1.0
#RES 80x24
#GRID 64bit
#COLOR truecolor
#NERDFONT required
#ATLAS logical-v1
---
U+2588#1E1E2E#1E1E2E*12
U+E0B0#1793D1#1E1E2E
U+0020#1793D1#000000
```

* Fila 0: bloque completo █ (U+2588) repetido 12 veces con RLE.
* Fila 1: cuña Powerline izquierda (U+E0B0, ID 165 en el atlas).
* Fila 2: espacio (U+0020) colocado como celda con colores.

## 2. Formato `.txv` (Tyxel Video — contenedor binario)

Diseñado para animación fluida a 30/60 FPS con streaming en una sola pasada de
escritura y seek instantáneo O(1).

### 2.1. Header fijo (32 bytes, little-endian)

| Offset | Tipo | Campo | Descripción |
|---|---|---|---|
| 0 | `[u8; 8]` | `magic` | `b"TYXELVID"` |
| 8 | `u8` | `version` | 1 |
| 9 | `u8` | `flags` | Bit 0: NerdFont requerido; Bits 1-2: modo de color base (0=truecolor, 1=256, 2=16, 3=mono); Bits 3-4: compresión (0=None, 1=LZ4, 2=Zstd) |
| 10 | `u16` | `width` | columnas |
| 12 | `u16` | `height` | filas |
| 14 | `u16` | `fps_num` | numerador FPS (30/60) |
| 16 | `u16` | `fps_den` | denominador FPS (1) |
| 18 | `u32` | `total_frames` | 0 si es stream en vivo |
| 22 | `u16` | `atlas_id` | 0 = Atlas Lógico v1 de 512 glifos |
| 24 | `[u8; 8]` | `reserved` | cero |

### 2.2. Stream de paquetes de fotogramas (Frame Packets)

Cada paquete:

| Campo | Tipo | Descripción |
|---|---|---|
| `frame_type` | `u8` | 0x01 = I-Frame (keyframe completo); 0x02 = P-Frame (delta RLE) |
| `duration_ms` | `u16` | tiempo en pantalla; una imagen estática 2 s => 2000 sin emitir frames duplicados |
| `payload_len` | `u32` | tamaño del bloque comprimido |
| `payload` | `u8[payload_len]` | ver abajo |

**I-Frame:** array contiguo de `width * height` estructuras `Tyxel` (8 bytes c/u,
layout del §1 de ARCHITECTURE.md) comprimido con el método indicado en `flags`.

**P-Frame (delta):** secuencia de operaciones que reconstruyen el frame sobre el
anterior. El cursor lógico recorre el lienzo completo en orden row-major:

* `Op::Skip(n)`: `u8 tag=0x00, u16 n` — salta `n` celdas iguales al frame anterior.
* `Op::Repeat(n, tyxel)`: `u8 tag=0x01, u16 n, Tyxel(8)` — pinta el mismo Tyxel `n` veces.
* `Op::Literal(n, tyxels)`: `u8 tag=0x02, u16 n, n*8 bytes` — copia `n` Tyxel distintos contiguos.

### 2.3. Seek Index Trailer & Footer fijo (final del archivo)

* Al cerrar el archivo el codificador vuelca la tabla de todos los I-Frames:
  `Vec<(u32, u64)>` → `(timestamp_ms, byte_offset)`, ordenada por timestamp.
  La tabla comienza con `u32 count` seguido de `count * 12` bytes.
* Footer fijo de 16 bytes finales:
  * `index_offset: u64` — posición exacta donde inicia el Seek Index Trailer (0 si no hay índice).
  * `end_magic: [u8; 8]` — `b"TYXELEND"`.
