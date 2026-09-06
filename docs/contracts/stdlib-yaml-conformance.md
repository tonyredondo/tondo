# `std.yaml` VM/native conformance

La lane `std.yaml conformance` es reproducible y target-qualified; este documento fija su frontera.

Este contrato cierra `STD-YAML-CONF-001`. La autoridad machine-readable es
[`testing/stdlib-yaml-conformance.json`](../../testing/stdlib-yaml-conformance.json).
La comparación usa el mismo corpus de seis case IDs en el VM hosted y en un
proceso nativo separado. El probe nativo llama directamente al kernel portable
de [`crates/tondo-stdlib/src/yaml.rs`](../../crates/tondo-stdlib/src/yaml.rs)
y comprueba el estado de la tabla de lifecycle del runtime; no existe todavía
un ABI YAML nativo ni lowering Cranelift/AOT que esta prueba pueda promocionar.

Ejecuta la conformance completa con:

    scripts/stdlib-yaml-conformance.sh

Las mutaciones negativas del contrato se ejercitan con:

    scripts/stdlib-yaml-conformance-test.sh

## Corpus compartido

El mismo corpus (same corpus) y los mismos case IDs se reproducen en ambos
procesos target-qualified.
[`tests/runtime/m11-std-yaml-conformance-001.to`](../../tests/runtime/m11-std-yaml-conformance-001.to)
conserva sus sidecars `.stdout` y `.exit`. Cubre:

- parseo/encoding dinámico y `Encode`/`Decode` tipado de `Array[Int]`;
- interoperabilidad del schema Core, enteros radix, `yes` como texto, binary
  Base64 y orden canónico;
- dos documentos, eventos y lectura con fragmentos de un byte;
- `InvalidBinary` con path `Key`/`Index`, offset, línea y columna estables;
- rechazo atómico por `maxInputBytes` y transición terminal `Closed`; y
- la frontera explícita de scalar/SIMD/AOT.

El proceso nativo emite JSON normalizado para los mismos IDs. Verifica los
bytes dinámicos y canónicos, el tamaño de la ruta tipada, la invariancia de
eventos entre `fromBytes` y `fromReader` con fragmentos de un byte, los errores
estructurados y el cierre del reader. Cada caso vive en un ámbito Rust y se
comprueba `zero live runtime-table objects` antes de publicar su observación.

La salida exacta del VM es la autoridad de líneas observables; la salida JSON
nativa añade sólo campos estructurales que el contrato declara. No se comparan
layouts, punteros, direcciones ni representaciones privadas.

## Fronteras que no se mezclan

Esta conformance reutiliza el mismo scalar stdlib como oracle para evitar un
segundo modelo de wire. Eso demuestra equivalencia del recorrido hosted y del
proceso nativo target-qualified, no una implementación independiente ni una
ABI pública. `native_aot: not-claimed` permanece vigente: no se ejecuta código
generado por Cranelift y no se afirma soporte de targets adicionales. Tampoco
hay ruta SIMD medida; `simd: not-measured-no-optimized-route` es un no-claim,
no un resultado sintético.

El reporte reproducible se escribe en
`target/reliability/evidence/stdlib-yaml-conformance.json`. Incluye hashes del
contrato, fixture, probe, fuentes y logs, las líneas VM y observaciones nativas,
pero no contiene physical paths, timestamps, PIDs ni direcciones.
