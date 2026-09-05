# Contrato de tests de `std.encoding`

`STD-ENCODING-TEST-001` cierra el modelo independiente, los vectores,
las fronteras de chunk, los errores byte-exactos, los límites y el fuzz
acotado de Base64 y hexadecimal en el draft Tondo 0.1. El registro
machine-readable es [`testing/stdlib-encoding-test.json`](../../testing/stdlib-encoding-test.json).

Este bloque es una frontera de fiabilidad, no una nueva API ni un runtime
alternativo. El modelo de referencia no llama al codec de producción: calcula
las tablas, el padding, los bits de relleno, los nibbles y los offsets por
separado. La implementación scalar de `std.encoding` y sus regresiones de la
VM hosted se comparan con ese oráculo. El runtime nativo público, el lowering
AOT genérico y SIMD siguen sin reclamar.

## Modelo wire independiente

`crates/tondo-reliability/src/encoding_model.rs` mantiene las policies y reglas
de Base64 standard/URL-safe, padding requerido/omitido y hex lower/upper/Any.
Comprueba los vectores RFC 4648, las representaciones canónicas, alfabetos
mezclados, whitespace, padding intermedio o excesivo, bits de relleno no cero,
prefijos/separadores hex y nibbles incompletos. Los errores del modelo
contienen únicamente el kind wire y el número de bytes observados antes del
fallo.

La misma prueba compara la salida materializada y la salida incremental para
chunks de un byte, fronteras de quantum, chunks grandes y chunks vacíos.
Los límites de input/output se comprueban antes de publicar bytes, y los casos
de error verifican que el handle queda terminal y devuelve `Closed` después de
`finish` o de un fallo.

## Fuzz reproducible y alcance

El target `stdlib_encoding` consume como máximo 4.096 bytes y 512 acciones.
Cada caso se ejecuta dos veces; se comparan la referencia independiente, la
ruta scalar materializada y las máquinas incrementales para las seis policies.
La semilla estable está en `fuzz/corpus/stdlib_encoding/seed` y el smoke
reproducible en `scripts/stdlib-encoding-fuzz.sh` usa 128 ejecuciones,
`nightly-2026-07-28`, seed 4105, timeout de 10 segundos y RSS de 4 GiB.

El bloque promueve modelo, tests y fuzz para la ruta scalar/hosted ya existente.
No promueve rendimiento, equivalencia SIMD, runtime nativo público ni lowering
AOT; esos resultados pertenecen a `STD-ENCODING-PERF-001` y a las leaves
posteriores de conformance.
