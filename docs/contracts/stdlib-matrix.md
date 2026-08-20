# Matriz normativa de `stdlib` 0.1A

`STD-MATRIX-ALL-001` coordina la trazabilidad completa del catálogo actual de
`STD-0.1A`. Su fuente machine-readable es
[`testing/stdlib-matrix.json`](../../testing/stdlib-matrix.json), generada de
forma reproducible por
[`scripts/stdlib-matrix-generate.sh`](../../scripts/stdlib-matrix-generate.sh)
y validada por
[`scripts/stdlib-matrix-check.sh`](../../scripts/stdlib-matrix-check.sh).

La matriz incluye exactamente los 21 owners del contrato de integración y el
owner adicional `std.bytes` (22 en total; incluidos `std.time` y `std.env`), 214
firmas indexadas por la auditoría pública y 171 requisitos de owner. `std.meta`,
`std.reflect`, `std.bytes`, `std.time` y `std.env` añaden sus contratos
executable A0 y seis requisitos de evidencia cada uno sin crear una segunda
API pública; `std.core` conserva su contrato de grupo y añade evidencia por
celda para sus nueve firmas intrínsecas; `std.text` añade evidencia por celda
para sus quince firmas Unicode y UTF-8; `std.collections` añade evidencia por
celda para sus dieciocho firmas de `Array`, `Map` y `Set`; `std.iter` añade
evidencia por celda para sus cuatro firmas de `Iterator`; `std.math` añade
evidencia por celda para sus nueve firmas escalares; `std.format` añade
evidencia por celda para sus cinco firmas de `Display` y builder; `std.io`
añade evidencia por celda para sus cuatro firmas de Reader/Writer y límites;
`std.path` añade evidencia por celda para sus diez firmas de paths léxicos y
`std.console` añade evidencia por celda para sus siete firmas de streams y
output capability-gated; `std.fs` añade evidencia por celda para sus catorce
firmas de filesystem, handles afines y operaciones atómicas capability-gated.
Los placeholders sintéticos anteriores
quedan reemplazados por filas reales.
`std.0.1B` permanece
como catálogo futuro cerrado:
sus módulos aparecen solo en `catalogs.future_modules` y no se convierten en
requisitos implícitos de la fase actual.

## Celdas obligatorias

Cada fila contiene referencias explícitas a las seis celdas:

```text
SPEC → IMPL/HOST → MODEL/TEST/FUZZ → PERF → CONF → DOC
```

Las filas de firma apuntan a la fila correspondiente de
`testing/stdlib-public-api.json` para conservar los gaps de HIR, lowering y
caso público sin duplicar evidencia. Las filas de requisito apuntan al
contrato del owner y a su `test_matrix`. Las seis celdas se materializan como
`stage_refs`; el registro del owner contiene el estado y las razones de cada
celda. `dimensions_ref` enlaza cada fila con las dimensiones públicas de su
owner, evitando una cifra agregada entre módulos incompatibles.

`verified` solo significa que esa celda tiene evidencia suficiente. `partial`,
`pending`, `gap` y `not-applicable` exigen una razón no vacía. La matriz
permanece `open-gaps` mientras exista cualquier fila no completamente
verificada; no hay waivers silenciosos ni promoción implícita por tener
tests de kernel.

## Reproducibilidad y promoción

El checker regenera la matriz en un directorio temporal y compara el resultado
byte a byte con el archivo versionado. También exige que todas las firmas de la
auditoría pública estén presentes exactamente una vez, que cada requisito
tenga owner, que las dimensiones PERF tengan owner group y que todas las
referencias apunten a paths existentes. El test negativo elimina un owner y
una razón de estado para demostrar que el gate falla cerrado.

`std.core` conserva `HOST` como `not-applicable` porque sus valores
`Option`/`Result` y su dispatch estático pertenecen al lowering compiler/VM.
El corpus de admission fuzz, los fixtures runtime y la auditoría pública se
enlazan desde su evidencia de owner; `STD-A-FUZZ-001` promueve el fuzz
owner-aware y la captura de rendimiento por owner sigue abierta.

`std.text` conserva también `HOST` como `not-applicable`: `String` es un valor
intrínseco portable y sus quince operaciones no consultan capabilities ni el
entorno. Los fixtures Unicode/UTF-8, el corpus bounded y la auditoría pública
quedan enlazados por `STD-A-TEXT-EVIDENCE-001`; `STD-A-FUZZ-001` promueve el
fuzz owner-aware y el baseline de coste por owner sigue abierto.

`std.collections` conserva `HOST` como `not-applicable`: `Array`, `Map` y `Set`
son valores intrínsecos portables. El fixture de runtime, las properties de
COW eager/compartido, el orden de inserción y el admission corpus enlazan las
dieciocho firmas; `STD-A-FUZZ-001` promueve el fuzz owner-aware y los
baselines de memoria/hash por owner siguen abiertos.

`std.iter` conserva `HOST` como `not-applicable`: sus adaptadores y cursores
son intrínsecos portables. El fixture de runtime, las properties de consumo y
agotamiento, el trazado de callbacks y la auditoría pública enlazan las cuatro
firmas; `STD-A-FUZZ-001` promueve el fuzz owner-aware y los baselines de
retención, allocations y materialización por owner siguen abiertos.

`std.math` conserva `HOST` como `not-applicable`: sus nueve operaciones son
intrínsecas escalares y no consultan capabilities ni el entorno. La matriz IEEE,
el corpus de overflow/subnormales, las properties de redondeo y la auditoría
pública enlazan las nueve firmas; el scalar oracle es la referencia normativa y
no hay una ruta SIMD alternativa en 0.1. `STD-A-FUZZ-001` promueve el fuzz
owner-aware; baselines de coste y conformance global siguen abiertos.

`std.format` conserva `HOST` como `not-applicable`: `Display`, `Builder`,
`format` y `join` son intrínsecos portables y no consultan capabilities ni el
entorno. El fixture público, las properties de límites exactos/atomicidad, la
verificación estática y la auditoría de las cinco firmas quedan enlazados por
`STD-A-FMT-EVIDENCE-001`; `STD-A-FUZZ-001` promueve el fuzz owner-aware;
baselines de allocations y materialización y `STD-CONF-001` siguen abiertos.

`std.io` conserva `HOST` como `not-applicable`: Reader/Writer, `IoLimits`,
`readAll` y `writeAll` son protocolos portables y no conceden capabilities por
importarse. La fixture pública y el kernel cubren particiones de chunks, EOF,
short I/O, límites, progreso, errores de `flush` y cancelación; la auditoría de
las cuatro firmas queda enlazada por `STD-A-IO-EVIDENCE-001`. `STD-A-FUZZ-001`
promueve el fuzz owner-aware; baselines de bytes/chunks/work-units y
`STD-CONF-001` siguen abiertos; los adaptadores pertenecen a `std.console`,
`std.fs` y `std.process`.

`std.path` conserva `HOST` como `not-applicable`: es una representación léxica
portable que no consulta el filesystem. El kernel y el fixture público cubren
bytes nativos, UTF-8 estricto, NFC/NFD sin normalización, `.`/`..`, raíces,
extensiones, joins atómicos y el límite de 32 KiB; la auditoría de sus diez
firmas queda enlazada por `STD-A-PATH-EVIDENCE-001`. `STD-A-FUZZ-001` promueve
el fuzz owner-aware; baselines de bytes/componentes/work-units y
`STD-CONF-001` siguen abiertos.

`std.console` conserva `HOST` como `verified`: la capability `console` se
comprueba antes del lowering y el adaptador conserva tokens separados para
stdin, stdout y stderr sobre los protocolos de `std.io`. El fixture y las
pruebas host cubren partial I/O, EOF, LF estable, errores de UTF-8 y handles
incorrectos sin publicar estado parcial; la auditoría de las siete firmas queda
enlazada por `STD-A-CONSOLE-EVIDENCE-001`. `STD-A-FUZZ-001` promueve el fuzz
owner-aware; baselines de bytes/chunks/work-units y `STD-CONF-001` siguen
abiertos explícitamente.

`std.fs` conserva `HOST` como `verified`: la capability `filesystem` se
comprueba antes del lowering y no se concede por importar el módulo. El fixture
de filesystem y las pruebas host cubren handles afines, cleanup normal/unwind,
cancelación, límites atómicos, errores tipados, orden lexicográfico de bytes y
`atomicWrite`; las 14 firmas quedan enlazadas por
`STD-A-FS-EVIDENCE-001`. `STD-A-FUZZ-001` promueve el fuzz owner-aware;
baselines por target y `STD-CONF-001` siguen abiertos explícitamente.

`std.process` conserva `HOST` como `verified`: la capability `process` se
comprueba antes del lowering y no se concede por importar el módulo. Los planes
inertes, handles terminales y el adaptador host quedan enlazados con las 17
firmas públicas por `STD-A-PROC-EVIDENCE-001`; los fixtures cubren argv literal,
shell explícito, las cuatro formas de pipe, backpressure, streams separados y
`combined`, redirección `mergeStderr`, errores, cancelación, cleanup y reaping.
`STD-A-FUZZ-001` promueve el fuzz owner-aware; baselines por target y
`STD-CONF-001` siguen abiertos explícitamente.

`std.serialization` conserva `HOST` como `not-applicable`: es el protocolo de
eventos portable compartido por los codecs y sus providers de derive son
herméticos y build-only. `STD-A-SER-EVIDENCE-001` enlaza traits, frames,
`Value`/`Raw`, límites, chunking, publicación atómica, records, enums,
genéricos, attributes, source maps y diagnostics con el kernel y sus tests.
`STD-A-FUZZ-001` promueve el fuzz del event protocol; baselines por target y
`STD-CONF-001` siguen abiertos explícitamente.

`std.json` conserva `HOST` como `not-applicable`: es un codec portable con
parser y writer de frames explícitos, y el bridge del compilador no consulta
capabilities ni el target. `STD-A-JSON-EVIDENCE-001` enlaza sus 22 firmas con
las rutas typed/dynamic/streaming, `JsonNumber`, JCS, límites, fragmentos de un
byte, el fuzz owner-aware de `STD-A-FUZZ-001` e interoperabilidad `serde_json`.
Las pruebas por
operación y los baselines de allocations/memoria por target permanecen
parciales, al igual que `STD-CONF-001`.

`std.messagepack` conserva `HOST` como `not-applicable`: es un codec binario
portable con frames explícitos, claves arbitrarias y extensiones preservadas.
`STD-A-MSGPACK-EVIDENCE-001` enlaza sus 18 firmas con el modelo wire, formas
no mínimas, floats, binary/UTF-8, ext/timestamp, determinismo, límites,
fragmentos de un byte, el fuzz owner-aware de `STD-A-FUZZ-001` e interoperabilidad
`rmpv`. Las pruebas
por operación y los baselines de allocations/memoria por target permanecen
parciales, al igual que `STD-CONF-001`.

`std.protobuf` conserva `HOST` como `not-applicable`: el wire es portable y el
generator es build-only, hermético y alimentado únicamente por `tondo.toml` y
su grafo declarado. `STD-A-PROTOBUF-EVIDENCE-001` enlaza sus 15 firmas con
schema-first proto3, presencia, repeated/packed, maps, oneof, enums abiertos,
unknown fields/grupos, evolución, descriptor raíz, determinismo, límites,
fragmentos de un byte, el fuzz owner-aware de `STD-A-FUZZ-001` e interoperabilidad
`prost`. Las
pruebas por operación/schema y los baselines de allocations/memoria por target
permanecen parciales, al igual que `STD-CONF-001`.

`std.testing` es test-only y conserva `HOST` como `verified`: el worker bridge
es el único adaptador y la producción no puede importar el módulo. El leaf
`STD-A-TESTING-EVIDENCE-001` enlaza las 25 firmas públicas con assertions,
diffs, tolerancias, consumo de `Option`/`Result`, temporales aislados,
generación determinista, shrinking sellado, control del runner y fixtures de
dogfooding. `STD-A-FUZZ-001` ha promovido la ruta owner-aware de `std.testing`;
las dimensiones de coste no capturadas y `STD-CONF-001` siguen parciales con
razones explícitas.

`STD-TEST-001` añade `testing/stdlib-test-coordination.json`, un registro
generado que liga las 214 firmas y 171 requisitos a 66 leyes de modelo,
comandos de test y campañas de fuzz. El registro exige que cada superficie
pública tenga una ley; `STD-A-FUZZ-001` promueve las 22 rutas owner-aware y
mantiene performance y conformance como dimensiones independientes. La prueba Rust
`stdlib_owner_models` y los negativos del checker verifican la clausura sin
crear un segundo owner ni una API alternativa.

`STD-CONF-001` añade `testing/stdlib-conformance-coordination.json`, que
materializa una fila `CONF` por cada una de las 385 filas de esta matriz. Cada
owner conserva sus comandos, referencias, casos de interoperabilidad cuando
existen y el estado derivado de sus filas; `std.async` aporta siete filas de
contrato, con cinco callable auditadas y la implementación genérica pendiente;
los demás gaps continúan `partial` sin sobreafirmar conformance.
El checker regenera y cruza la identidad de todas las filas, exige razones y
referencias existentes y mantiene `promotion.status=not-promoted`; los tests
negativos y el test Rust `stdlib_conformance_coordination` rechazan omisiones,
sobreclaims y drift. La matriz sigue `open-gaps` por celdas de promoción
pendientes; el siguiente bloque de implementación global es
`NATIVE-PUBLISH-SPEC-001` (con `NATIVE-TARGET-DESC-001`,
`NATIVE-ARTIFACT-001` y `NATIVE-LINK-PLAN-001` ya cerrados).

`STD-DOC-001` añade `testing/stdlib-documentation.json`, el índice que enlaza
cada owner con sus documentos, fronteras `kernel`/`bridge`/`public_api` y
programas representativos. La auditoría de API conserva `complete`/`partial`/
`not-applicable` sin inferir estados desde el kernel; los fixtures runtime
exigen sidecars y los providers sin runtime aplicable llevan una razón. Esta
documentación es para el draft no publicado y no cambia el estado `open-gaps`
de la matriz.
