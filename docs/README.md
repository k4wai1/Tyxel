# Tyxel — Documentación Viva

Este directorio contiene la documentación viva del proyecto. Regla de oro:
**`PROGRESS.md` es la fuente de la verdad del estado del proyecto**; se actualiza
antes de iniciar cualquier tarea y justo después de que `cargo test` pase en verde.

| Archivo | Contenido |
|---|---|
| `ARCHITECTURE.md` | Contratos de memoria, endianness de bits `u64`, pipeline de codificación |
| `FORMAT_SPEC.md` | Especificación byte a byte de `.txa` y `.txv` (v1.0) |
| `ATLAS_MAP.md` | Tabla de los 512 glifos: rangos de ID -> codepoint -> lógica 8×8 |
| `PROGRESS.md` | Bitácora de estado, tareas completadas y bloqueos |
| `DECISIONS.md` | Registro de decisiones técnicas (ADR) para no re-debatir |

Mantener estos documentos antes o durante cada fase es **obligatorio** para que
cualquier sub-agente paralelo consulte offsets y rangos sin leer todo el código.
