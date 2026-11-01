# TYXEL: Especificación Maestra del Sistema y Directivas para el Agente (OpenCode)

## 1. CONTEXTO DEL PROYECTO Y VISIÓN GENERAL
**Tyxel** (Typographic Pixel) es un ecosistema completo en **Rust (100% Pure Rust)** compuesto por un formato de archivo dual (`.txa` para arte estático en texto plano y `.txv` para video/animación binaria comprimida), un motor de codificación perceptual por sub-celdas de 64 bits y un reproductor TUI de alto rendimiento para emuladores de terminal (Linux/Android).

El objetivo de Tyxel es convertir cualquier imagen o flujo de video en una matriz de celdas tipográficas donde cada celda (**Tyxel**) almacena:
1. Un glifo Unicode (optimizado para bloques geométricos y **Nerd Fonts** de ancho 1).
2. Color de Frente (`FG` en RGB TrueColor).
3. Color de Fondo (`BG` en RGB TrueColor).

Cada celda se evalúa matemáticamente como una cuadrilla de **8×8 sub-píxeles** empacada en un entero nativo de 64 bits (`u64`), permitiendo encontrar el glifo geométrico perfecto en nanosegundos mediante instrucciones de procesador `XOR` y `POPCNT` (`.count_ones()`), con degradación de color en tiempo real en el reproductor (TrueColor -> ANSI 256 -> 16 colores con dithering -> Monocromo) e integración directa con herramientas como `fastfetch`.

---

## 2. ENTORNO DE EJECUCIÓN Y RESTRICCIONES DE HARDWARE (CRÍTICO)
El desarrollo y compilación se ejecutan directamente en **Android (ARM64 / aarch64-linux-android) dentro de Termux** (SoC MediaTek Helio G99, 8 núcleos, 8 GB de RAM).

### Reglas Estrictas para Compilación en Termux:
1. **PROHIBIDO usar dependencias con enlaces a C/C++ complejos (`*-sys` crates):**
   - NO usar `zstd` (requiere compilar C con `cc`/`cmake`). Usar **`ruzstd`** (decodificador puro en Rust) o **`lz4_flex`** (100% Rust, ultrarrápido en ARM64) / `flate2` con backend `miniz_oxide`. Si se requiere Zstd en codificación y `zstd-sys` da problemas en el linker Bionic de Termux, usar `lz4_flex` como motor primario o invocar `zstd` vía tubería si está instalado, pero priorizar **100% Pure Rust** (`lz4_flex` + `ruzstd`).
   - NO vincular `ffmpeg-next` (bindings de C). Para video, `tyxel-enc` debe leer frames crudos (`rawvideo` RGB24) desde `stdin` o un subproceso CLI de `ffmpeg` (`Command::new("ffmpeg")`), y para imágenes estáticas usar el crate `image` con solo los features necesarios (`png`, `jpeg`, `webp`, `gif` puros en Rust).
2. **Control de Memoria y CPU en `cargo`:**
   - Evitar LTO pesado en modo `dev`. Configurar `.cargo/config.toml` para compilaciones incrementales rápidas y en modo `release` habilitar `target-cpu=native` (aprovecha instrucciones NEON y `cnt` de ARMv8.2-A).
3. **Rutas Temporales:**
   - Nunca hardcodear `/tmp`. Usar siempre `std::env::temp_dir()` para respetar el `$PREFIX/tmp` de Termux.

---

## 3. ARQUITECTURA DE MEMORIA Y ALGORITMO CORE (DECISIONES CONGELADAS)

### 3.1. El Struct `Tyxel` (8 Bytes Exactos, Cero UB)
PROHIBIDO usar `#[repr(C, packed)]` por riesgo de *Undefined Behavior* en accesos desalineados en ARM64.
En memoria RAM, `Tyxel` mide exactamente **8 bytes** y se alinea a 8 bytes (un registro completo de 64 bits):

```rust
#[repr(C, align(8))]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Tyxel {
    pub glyph_id: u16, // Índice en el Atlas de Glifos (0..511) -> 2 bytes
    pub fg: [u8; 3],   // Color RGB del Carácter -> 3 bytes
    pub bg: [u8; 3],   // Color RGB del Fondo -> 3 bytes
}
```

Ventaja técnica: Comparar dos Tyxel (`a == b`) para calcular Delta Frames se traduce en una sola instrucción de registro de 64 bits (CMP en ASM). Se proveerá un método `pub fn as_u64(self) -> u64` mediante `u64::from_ne_bytes` seguro (o `bytemuck::Pod`).

### 3.2. El Atlas Híbrido de 512 Glifos (4 KB en Caché L1)
No se deben escanear los 10,000 iconos de Nerd Fonts. El atlas tendrá una longitud fija de 512 glifos (`[u64; 512]`) alineada a 64 bytes (`#[repr(align(64))]`) para caber íntegra en la caché L1 de datos del procesador.
Todos los glifos del atlas DEBEN tener ancho de celda 1 (unicode-width == 1).
Reglas de Generación Procedural del Atlas Lógico (Fase 1 - 100% Independiente de la Fuente):
Cada u64 representa una matriz de 8×8 bits (documentación de endianness: **bit = (row * 8) + col**, fila 0 = los 8 bits menos significativos).
 * Índice 0: Espacio vacío   (U+0020), máscara 0x0000000000000000.
 * Índice 1: Bloque completo █ (U+2588), máscara 0xFFFFFFFFFFFFFFFF.
 * Block Elements (U+2580 – U+259F): Octavos superiores/inferiores/izquierdos/derechos y cuadrantes 2×2 mapeados exactamente a múltiplos de 4×4 bits en la matriz 8×8.
 * Sextantes (U+1FB00 – U+1FB3B): Mapeo de la rejilla lógica 2×3 a la matriz 8×8 usando partición horizontal [4, 4] columnas y partición vertical asimétrica [3, 2, 3] filas (3 + 2 + 3 = 8).
 * Braille (U+2800 – U+28FF): ¡TRAMPA EVITADA! NO escalar por nearest-neighbor completo (porque ⣿ daría 0xFFFFFFFFFFFFFFFF y competiría con █). Cada punto de la matriz Braille 2×4 ocupa una sub-celda de 4×2 bits donde solo se encienden 4 bits centrales (densidad del 50%), dejando margen alrededor. Así el matcher solo elige Braille para texturas punteadas o trazos finos.
 * Box Drawing (U+2500 – U+257F) y Geometría Nerd Fonts / Legacy (U+1FB3C+ y Powerline U+E0B0–U+E0BF): Líneas centrales de 2 bits de grosor, diagonales de pendiente exacta y triángulos de Powerline.

### 3.3. Motor de Matching (Bipartición + XOR + POPCNT + Direct-Mapped Cache)
Para convertir un bloque de 8×8 píxeles RGB de la imagen original a un Tyxel:
 * Corrección de Aspecto (1:2): Las celdas de terminal miden el doble de alto que de ancho. Al muestrear la imagen de entrada para una resolución de W columnas por H filas, la imagen se redimensiona a (W * 8) × (H * 16) (o se muestrea con paso vertical 2:1) para extraer una cuadrilla lógica de 8×8 colores RGB por celda.
 * Bipartición Cromática Rápida:
   * Calcular varianza de color en los 64 píxeles. Si el bloque es casi plano (varianza < umbral), devolver directamente glyph_id = 0 (  ) con bg = color_promedio y fg = bg en O(1) sin buscar en el atlas.
   * Si hay contraste, encontrar el eje de mayor amplitud RGB (R, G o B), dividir los 64 píxeles por la media en dos grupos (C_0 fondo y C_1 frente) y construir la máscara objetivo target_mask: u64 (1 si el píxel está más cerca de C_1, 0 si está más cerca de C_0).
 * Caché Direct-Mapped de 64K entradas (Sin HashMap):
   * Para evitar allocations y cache misses, usar un array fijo en heap `Box<[(u64, u16, bool); 65536]>` indexado por `((target_mask.wrapping_mul(0x9E3779B97F4A7C15)) >> 48) as usize`.
 * Búsqueda Vectorizada sin Saltos (Branchless SIMD-friendly):
   * PROHIBIDO usar el pre-filtro popcount(target) ± 4.
   * Recorrer las 512 máscaras del atlas linealmente:
     ```rust
     let diff_normal = (target_mask ^ glyph_mask).count_ones();
     let diff_invert = ((!target_mask) ^ glyph_mask).count_ones();
     ```
   * Elegir el glyph_id con menor distancia de Hamming. Si gana `diff_invert`, se intercambian fg y bg.
 * Recálculo de Color Fino (Anti-aliasing Cromático):
   * Una vez hallada la máscara real del glifo ganador (`winner_mask`), recalcular fg como el promedio exacto de los píxeles RGB donde winner_mask tiene bit 1, y bg como el promedio donde tiene bit 0.

## 4. ESPECIFICACIÓN DE LOS FORMATOS DE ARCHIVO

### 4.1. Formato .txa (Tyxel Art - Texto Plano / Estático / Editable)
Diseñado para imágenes estáticas, edición manual, tuberías UNIX y logos de fastfetch.
 * Sin coordenadas [x,y] redundantes ni tuberías `|` innecesarias.
 * Orden row-major (fila por fila). Cada fila del lienzo se separa con una directiva `#ROW` o línea en blanco, y cada celda ocupa una línea (o separada por tabulador `\t`), soportando RLE de texto (`*N`) para repetir la misma celda N veces consecutivas.

```
#!TYXEL-ART v1.0
#RES 80x24
#GRID 64bit
#COLOR truecolor
#NERDFONT required
#ATLAS logical-v1
---
#ROW 0
 █ #1E1E2E #1E1E2E *12
󰣇 #1793D1 #1E1E2E
 #1793D1 #000000
```

### 4.2. Formato .txv (Tyxel Video - Contenedor Binario con I-Frames, P-Frames y Seek Trailer)
Diseñado para animaciones y video fluido a 30/60 FPS con soporte de streaming (una sola pasada de escritura) y seek instantáneo en O(1).
 * Header Fijo (32 bytes, Little-Endian):
   * magic: [u8; 8] = b"TYXELVID"
   * version: u8 = 1
   * flags: u8 (Bit 0: NerdFont requerido, Bits 1-2: Modo de color base, Bits 3-4: Compresión 0=None, 1=LZ4, 2=Zstd)
   * width: u16 (Columnas)
   * height: u16 (Filas)
   * fps_num: u16 (Numerador FPS, ej. 30 o 60)
   * fps_den: u16 (Denominador FPS, ej. 1)
   * total_frames: u32 (0 si es stream en vivo)
   * atlas_id: u16 (0 = Atlas Lógico v1 de 512 glifos)
   * reserved: [u8; 8]
 * Stream de Paquetes de Fotogramas (Frame Packets):
   * frame_type: u8 (0x01 = I-Frame / Keyframe completo, 0x02 = P-Frame / Delta con RLE)
   * duration_ms: u16 (Tiempo que este frame permanece en pantalla; si una imagen queda estática 2 segundos, duration_ms = 2000 sin emitir frames duplicados).
   * payload_len: u32 (Tamaño del bloque comprimido).
   * payload: [u8; payload_len]
     * En I-Frame: Array contiguo de width * height estructuras Tyxel (8 bytes c/u) comprimido.
     * En P-Frame (Delta): Secuencia de operaciones Delta:
       * Op::Skip(u16): Salta N celdas idénticas al fotograma anterior.
       * Op::Repeat(u16, Tyxel): Pinta el mismo Tyxel N veces seguidas (RLE para fondos).
       * Op::Literal(u16, [Tyxel; N]): Copia N celdas distintas contiguas.
 * Seek Index Trailer & Footer Fijo (Al final del archivo):
   * Al cerrar el archivo, el codificador vuelca la tabla de todos los I-Frames: Vec<(u32 /* timestamp_ms */, u64 /* byte_offset */)>.
   * Footer Fijo (16 bytes finales):
     * index_offset: u64 (Posición exacta donde inicia el Seek Index Trailer; 0 si no hay índice).
     * end_magic: [u8; 8] = b"TYXELEND".

## 5. REPRODUCTOR TUI, DEGRADACIÓN EN TIEMPO REAL Y FASTFETCH
 * Renderizado Atómico Libre de Tearing:
   * Envolver cada fotograma emitido al `BufWriter<StdoutLock>` con DEC Synchronized Output (`\x1b[?2026h` ... `\x1b[?2026l`).
   * Mantener registro del último color fg y bg emitidos en la fila para no repetir secuencias `\x1b[38;2;R;G;Bm` si el color no cambió entre celdas contiguas.
   * En los P-Frames, si el número de celdas que cambiaron es menor al 35% de la pantalla, usar posicionamiento directo de cursor (`\x1b[Y;XH`) solo sobre las regiones sucias.
 * Frame Skipping con Acumulación de Deltas en RAM:
   * Reloj maestro sin drift: `target_time = start_instant + Duration::from_millis(accumulated_ms)`.
   * Espera híbrida: `thread::sleep` hasta 2 ms antes del objetivo y `std::hint::spin_loop()` el resto.
   * Si el stdout de la terminal hace cuello de botella y el reloj real ya superó el tiempo del Frame N, el reproductor NO salta la decodificación del P-Frame N: aplica los deltas del Frame N sobre el backbuffer en memoria RAM (toma microsegundos), avanza al Frame N+1, aplica sus deltas en RAM y solo emite por ANSI a la terminal el frame puesto al día.
 * Degradación de Color en Tiempo Real + Banner de Advertencia:
   * Detectar capacidades con variables COLORTERM y TERM (o flag --color truecolor|256|16|mono).
   * Si el archivo pide truecolor y la terminal soporta menos, mostrar durante 2 segundos una barra superior TUI de advertencia y degradar en vivo:
     * 256 colores: Cubo 6×6×6 + rampa gris de 24 tonos por aritmética entera rápida.
     * 16 colores / Mono: Cuantización con Ordered Dithering (Matriz de Bayer 4×4) sobre los canales RGB antes de elegir el índice ANSI 16.
 * Seguridad de Terminal (Drop Guard + Signal Handling):
   * Implementar un struct TerminalGuard que en su Drop y ante SIGINT/SIGTERM restaure el modo raw, muestre el cursor (`\x1b[?25h`), cierre el buffer alternativo (`\x1b[?1049l`) y resetee colores (`\x1b[0m`).
 * Integración con fastfetch y Reescalado Autónomo:
   * Comando `tyxel cat <archivo.txa> [--width W] [--height H] [--fit contain|cover]`: Como cada Tyxel guarda su glyph_id (cuya máscara 8×8 se conoce en el atlas) y sus colores fg/bg, tyxel puede reconstruir una imagen virtual de (width*8) × (height*8) desde el propio .txa, reescalarla al tamaño pedido por el usuario para fastfetch y volver a codificarla al vuelo en milisegundos.
   * Comando `tyxel export --ansi <archivo.txa> -o logo.ansi`: Pre-renderiza las secuencias de escape listas para `fastfetch --raw` o `fastfetch --file`.

## 6. ESTRUCTURA DEL REPOSITORIO Y SISTEMA DE DOCUMENTACIÓN VIVA
El agente debe crear y mantener la siguiente estructura de archivos desde el primer paso:
```
tyxel/
├── Cargo.toml                 # Workspace root (resolver = "2")
├── .cargo/
│   └── config.toml            # Optimizaciones para ARM64 / Termux
├── AGENTS.md                  # Este documento + reglas de coordinación de agentes
├── docs/
│   ├── ARCHITECTURE.md        # Contratos de memoria, endianness de bits u64 y pipeline
│   ├── FORMAT_SPEC.md         # Especificación byte a byte de .txa y .txv (v1.0)
│   ├── ATLAS_MAP.md           # Tabla de los 512 glifos (Rangos ID -> Codepoint -> Lógica)
│   ├── PROGRESS.md            # Bitácora de estado actual, tareas completadas y bloqueos
│   └── DECISIONS.md           # Registro de decisiones técnicas (ADR) para no re-debatir
├── crates/
│   ├── tyxel-core/            # Librería sin I/O pesado: Struct Tyxel, Atlas 512, Matcher, Parsers .txa/.txv
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── tyxel.rs       # Struct Tyxel (8 bytes align 8) y conversiones de color/Bayer
│   │       ├── atlas.rs       # Generador del Atlas Lógico de 512 máscaras u64
│   │       ├── matcher.rs     # Bipartición RGB + XOR/POPCNT + Direct-Mapped Cache 64K
│   │       ├── txa.rs         # Serializer / Parser de formato texto .txa + Resizer
│   │       └── txv.rs         # Serializer / Parser de formato binario .txv + Delta RLE + Seek Index
│   ├── tyxel-enc/             # Binario CLI de codificación (Imagen/Video -> .txa / .txv)
│   │   ├── Cargo.toml
│   │   └── src/main.rs
│   └── tyxel-play/            # Binario CLI reproductor TUI, `tyxel cat` y `tyxel export`
│       ├── Cargo.toml
│       └── src/main.rs
└── tests/
    └── golden_tests.rs        # Tests de patrones sintéticos y round-trip (.txa / .txv)
```

Reglas de Mantenimiento de Documentación (Obligatorio para el Agente):
 * `docs/PROGRESS.md` es la fuente de la verdad del estado del proyecto. Antes de iniciar cualquier tarea y justo después de que `cargo test` pase en verde, el agente DEBE actualizar `docs/PROGRESS.md` marcando qué módulo está listo, qué tests pasan y cuál es el siguiente paso.
 * `docs/FORMAT_SPEC.md` y `docs/ATLAS_MAP.md` deben escribirse ANTES o DURANTE la fase 1 para que cualquier sub-agente paralelo consulte los offsets de bytes y rangos de IDs del atlas sin tener que leer todo el código fuente.

## 7. PROTOCOLO DE EJECUCIÓN CON SUB-AGENTES PARALELOS (OPENCODE WORKFLOW)
Para acelerar el desarrollo en Termux sin causar colisiones de archivos ni bloqueos del archivo Cargo.lock, el agente principal debe orquestar a los sub-agentes paralelos bajo estas reglas:
 * Fase 0 (Secuencial - Agente Principal):
   * Inicializar el Workspace (Cargo.toml, .cargo/config.toml), crear la carpeta docs/ con los 5 archivos de documentación inicializados y definir las firmas públicas (interfaces/traits vacíos) en tyxel-core/src/lib.rs.
 * Fase 1 (Paralelismo Triple sobre tyxel-core):
   Cada sub-agente trabaja en archivos disjuntos sin tocar Cargo.toml al mismo tiempo:
   * Sub-Agente A (atlas.rs + docs/ATLAS_MAP.md): Implementa la tabla de 512 codepoints Unicode (ancho 1 verificado) y sus máscaras u64 lógicas (Blocks, Cuadrantes, Sextantes [3,2,3], Braille 50% densidad, Box Drawing y triángulos Nerd Font), con unit tests que verifiquen que ninguna máscara distinta de 0 y 1 sea todo 0 o todo 1.
   * Sub-Agente B (tyxel.rs + matcher.rs): Implementa el struct Tyxel alineado a 8 bytes, la cuantización de color (ANSI 256, ANSI 16 con matriz de Bayer 4×4), la bipartición de bloques 8×8 RGB, la caché directa de 64K entradas y el matcher XOR + count_ones().
   * Sub-Agente C (txa.rs + txv.rs + docs/FORMAT_SPEC.md): Implementa la lectura/escritura del formato .txa (con soporte de repetición *N) y del contenedor .txv (Header 32B, I-Frames, P-Frames con Op::Skip/Repeat/Literal, compresión lz4_flex y Footer con Seek Index).
 * Fase 2 (Paralelismo Doble sobre Binarios CLI):
   Una vez que `cargo test -p tyxel-core` compile y pase al 100%:
   * Sub-Agente D (crates/tyxel-enc): Construye el codificador CLI (con clap derive) capaz de convertir imágenes (.png, .jpg, .gif, .webp) a .txa/.txv y leer flujos de video RGB24 desde ffmpeg para generar .txv con detección automática de I-Frames y P-Frames.
   * Sub-Agente E (crates/tyxel-play): Construye el reproductor e inspector CLI (tyxel play, tyxel cat, tyxel export, tyxel info) con DEC Mode 2026, salto de frames con acumulación en RAM, degradación de color en vivo y TerminalGuard.
 * Regla de Verificación en Termux:
   * Los sub-agentes NO deben ejecutar `cargo build --release` en paralelo al mismo tiempo para no saturar los 8 GB de RAM ni bloquear el directorio target/. Deben usar `cargo check` o `cargo test --lib` ligeros, y el Agente Principal ejecuta la validación final e integración.

## 8. INSTRUCCIÓN INMEDIATA DE ARRANQUE PARA EL AGENTE
Tu primera acción ahora mismo es:
 * Inspeccionar el directorio actual con tus herramientas.
 * Crear la estructura completa del Workspace (Cargo.toml, .cargo/config.toml, carpetas crates/ y docs/).
 * Redactar los documentos base en docs/ (ARCHITECTURE.md, FORMAT_SPEC.md, ATLAS_MAP.md, DECISIONS.md y PROGRESS.md).
 * Implementar el esqueleto compilable de tyxel-core y lanzar la Fase 1 construyendo el struct Tyxel, el generador del Atlas Lógico de 512 glifos y los tests unitarios base. No pidas confirmación para pasos intermedios; avanza hasta que `cargo test -p tyxel-core` pase en verde e informa del progreso.

---

## ANEXO: Consejos prácticos de entorno Termux
1. **Guarda este documento en la raíz del proyecto** como `AGENTS.md` y dile al agente en el chat: *"Lee AGENTS.md y ejecuta la Instrucción Inmediata de Arranque"*.
2. **Paquetes de Termux que ahorran dolores de cabeza:** Como se prohíben crates `*-sys` en C, todo compila limpio con el `rustc` de Termux sin pelearse con `clang` ni cabeceras del Android NDK. Para la prueba de video en `tyxel-enc` solo se necesita el binario normal de `ffmpeg` instalado en Termux (`pkg install ffmpeg`).
