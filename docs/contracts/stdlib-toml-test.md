# std.toml testing contract

STD-TOML-TEST-001 cierra el modelo independiente, el corpus de regresión y
el fuzz bounded del kernel TOML 1.1.0. El registro machine-readable es
[testing/stdlib-toml-test.json](../../testing/stdlib-toml-test.json). El
contrato owner y la frontera de implementación permanecen en
[stdlib-toml.md](./stdlib-toml.md).

## Modelo de referencia acotado

crates/tondo-reliability/src/toml_model.rs implementa un renderer canónico
pequeño y separado del parser, encoder y adapters de tondo-stdlib. Genera
tablas raíz con enteros lossless, floats decimales finitos, strings basic,
arrays heterogéneos y tables inline. Ordena las keys por bytes UTF-8, rechaza
duplicados, keys inválidas, valores no finitos y supera un máximo de 128 nodos.
No lee configuración, reloj, filesystem ni estado del host.

crates/tondo-reliability/tests/toml_models.rs compara los bytes del modelo con
encodeCanonical, parsea el resultado y verifica que el re-encode sea
idempotente para 4.096 seeds. El mismo archivo cubre el corpus negativo, los
spans/path, fragmentos de un byte, el decoder common, límites y lifecycle
terminal del reader/writer.

## Corpus, Unicode y seguridad

La matriz hosted de toml::tests cubre TOML 1.1.0, comentarios y CRLF,
bare/quoted/dotted keys, las cuatro strings, escapes \b, \t, \n, \f, \r, \e,
\x, \u, \U, números decimal/hex/octal/binario, floats incluidos inf/nan,
fechas civiles, offsets fijos, arrays, inline tables, tables y
arrays-of-tables. También cubre Unicode en strings y los rechazos de
duplicados, colisiones, extensiones de inline table, precisión temporal,
overflow, tokens incompletos y entradas trailing.

Los límites de input, depth, nodes, tables, elementos de array, bytes de key,
segmentos de path, scalar, string y filas AOT se prueban como errores atómicos:
no se publica un TomlValue parcial. Los errores conservan span half-open,
línea/columna y path cuando la operación lo conoce. El corpus no habilita
includes, interpolación de environment, timezone lookup, ejecución ni acceso
ambiental.

El target fuzz/fuzz_targets/stdlib_toml.rs consume como máximo 4.096 bytes y
512 acciones deterministas. Cada input se ejecuta dos veces, compara el
renderer independiente con encodeCanonical, parsea y vuelve a codificar el
resultado, y valida la frontera hosted. El seed persistente es
fuzz/corpus/stdlib_toml/seed; el smoke usa 128 runs, seed 4113,
nightly-2026-07-28, timeout de 10 segundos y límite RSS de 4 GiB. El comando
reproducible está en el JSON y en
[scripts/stdlib-toml-fuzz.sh](../../scripts/stdlib-toml-fuzz.sh).

## Frontera de promoción

Este bloque promueve sólo modelo, tests hosted scalar, corpus y fuzz bounded.
No promueve una API pública de Tondo, intrinsic del compilador, registro VM,
ABI nativo, lowering AOT, SIMD ni claims de rendimiento. std.toml sigue
separado de tondo.toml y de los schemas privados del toolchain.

La implementación del kernel permanece en verified-stdlib-kernel, con
host: not-claimed-until-compiler-toml-abi, native_aot_lowering: not-claimed y
public_api_promoted: false. El siguiente leaf es STD-TOML-PERF-001; conformance
y documentación de uso permanecen posteriores.
