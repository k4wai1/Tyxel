# ARCHITECTURE.md — Contratos de memoria, bits y pipeline

> Estado: base congelada en Fase 0. Los sub-agentes consultan este documento; no lo re-debaten.

## 1. Contrato de memoria del struct `Tyxel`

* Tamaño exacto: **8 bytes** (`glyph_id:u16 + fg:[u8;3] + bg:[u8;3]`).
* Alineación: **8 bytes** (`#[repr(C, align(8))]`).
* PROHIBIDO `packed`: riesgo de UB en accesos desalineados en ARM64.
* Conversión a entero: `pub fn as_u64(self) -> u64` con `u64::from_ne_bytes`
  (native endianness = little en aarch64). La comparación `a == b` de dos Tyxel
  se compila a una sola comparación de registro de 64 bits (ideal para Delta Frames).
* El layout en disco del I-Frame del `.txv` es el mismo layout binario (8 B por celda,
  little-endian): `glyph_id.low, glyph_id.high, fg[0], fg[1], fg[2], bg[0], bg[1], bg[2]`.

## 2. Endianness de la matriz 8×8 en las máscaras `u64`

**bit = (row * 8) + col.** La fila 0 es la **fila superior** de la celda y ocupa los
8 bits menos significativos del `u64`. El pixel (row=r, col=c) es el bit `r*8 + c`.

Helper canónico (en `atlas.rs`):

```rust
#[inline]
pub fn mask_bit(row: u32, col: u32) -> u64 {
    debug_assert!(row < 8 && col < 8);
    1u64 << (row * 8 + col)
}

#[inline]
pub fn mask_is_set(mask: u64, row: u32, col: u32) -> bool {
    (mask >> (row * 8 + col)) & 1 == 1
}
```

## 3. Atlas Lógico de 512 glifos

* Tabla fija `MASKS: [u64; 512]` con `#[repr(align(64))]`: 4 KB -> cabe íntegra en la
  caché L1 de datos.
* Tabla paralela `CODEPOINTS: [char; 512]`: glifos Unicode de **ancho 1** (verificado
  con `unicode-width`).
* Distribución de rangos: ver `ATLAS_MAP.md` (fuente de la verdad de los IDs).
* Invariantes testeados:
  * Máscara 0 = 0x0 (espacio) y máscara 1 = 0xFFFF_FFFF_FFFF_FFFF (bloque completo).
  * Ningún glifo cuyo ID sea distinto de {0, 1} puede tener máscara
    0x0 (invisible) o 0xFFFF_FFFF_FFFF_FFFF (duplicado del bloque completo).

## 4. Motor de matching (pipeline por celda)

Entrada: bloque lógico de 8×8 píxeles RGB muestreado con corrección de aspecto 1:2
(las celdas de terminal miden el doble de alto que de ancho).

1. **Varianza plana:** si la varianza de los 64 píxeles < umbral, devolver
   `glyph_id = 0`, `bg = fg = promedio` en O(1).
2. **Bipartición cromática:** eje RGB de mayor amplitud; split por la media en
   C_0 (fondo) y C_1 (frente); construir `target_mask: u64`.
3. **Caché direct-mapped 64K** (sin HashMap): `Box<[(u64, u16, bool); 65536]>`
   indexada por `((target_mask.wrapping_mul(0x9E37_79B9_7F4A_7C15)) >> 48) as usize`.
   Entrada: `(target_mask, glyph_id, invert)`. Si el `target_mask` coincide, hit.
4. **Búsqueda lineal branchless sobre las 512 máscaras** (PROHIBIDO el pre-filtro
   popcount ± 4):

   ```rust
   let diff_normal = (target_mask ^ glyph_mask).count_ones();
   let diff_invert = ((!target_mask) ^ glyph_mask).count_ones();
   ```

   Gana la menor distancia de Hamming; si gana `diff_invert`, intercambiar fg/bg.
5. **Recálculo fino de color:** con la máscara real del glifo ganador,
   `fg = promedio(pixeles donde máscara==1)`, `bg = promedio(==0)`.

## 5. Pipeline de datos general

```
imagen/video --(tyxel-enc)--> .txa | .txv --(tyxel-play)--> ANSI TrueColor/256/16/mono
                                   \-->(tyxel cat/export)--> fastfetch
```

* Codificación: cada celda = 8×8 sub-píxeles -> matcher -> Tyxel.
* Delta frames: comparación `u64` por celda -> `Op::Skip / Repeat / Literal` (RLE).
* Reproducción: backbuffer en RAM + emisión ANSI por fila con caché de último
  color; P-frames con <35 % de celdas sucias usan posicionamiento de cursor directo.
* Degradación: TrueColor -> ANSI 256 (cubo 6×6×6 + 24 grises) -> ANSI 16 con
  Ordered Dithering (matriz Bayer 4×4) -> monocromo.
