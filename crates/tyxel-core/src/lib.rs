//! tyxel-core: librería sin I/O pesado del ecosistema Tyxel.
//!
//! Módulos:
//! - [`tyxel`]: struct `Tyxel` de 8 bytes + cuantización de color (ANSI 256/16, Bayer).
//! - [`atlas`]: Atlas Lógico de 512 glifos (máscaras `u64` 8×8).
//! - [`matcher`]: bipartición RGB + XOR/POPCNT + caché direct-mapped 64K.
//! - [`txa`]: codec del formato texto plano `.txa` + resizer.
//! - [`txv`]: codec del contenedor binario `.txv` (I-Frames, P-Frames, seek index).

pub mod atlas;
pub mod matcher;
pub mod txa;
pub mod txv;
pub mod tyxel;

/// Versión del formato de archivo soportado por esta librería.
pub const FORMAT_VERSION: &str = "v1.0";

/// Identificador del Atlas Lógico v1 de 512 glifos (última máscara: ver `atlas`).
pub const ATLAS_ID_LOGICAL_V1: u16 = 0;

/// Número total de glifos del atlas.
pub const ATLAS_LEN: usize = 512;
