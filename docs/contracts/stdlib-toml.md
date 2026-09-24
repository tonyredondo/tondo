# Contrato de `std.toml`

**Status:** `contract-locked` Tondo 0.1 design (`STD-TOML-001`), with a verified Rust stdlib
kernel, bounded conformance adapters, and executable kernel usage. The public
Tondo `std.toml` API, production host registration, native ABI, and native AOT
lowering are not implemented.

`std.toml` implementa el perfil de datos de TOML v1.1.0 con una frontera
lossless y determinista para Tondo. Es un codec de datos: no es el parser del
manifiesto del toolchain, no interpreta `tondo.toml`, no descubre proyectos y
no consulta el host. El registro machine-readable es
[`testing/stdlib-toml.json`](../../testing/stdlib-toml.json) y este documento
se integra desde [`TONDO_STANDARD_LIBRARY_SPEC.md`](../../TONDO_STANDARD_LIBRARY_SPEC.md).

## Principios del owner

- La versión wire es TOML **1.1.0**. El parser conserva las reglas de claves,
  strings, números, arrays, tablas y arrays de tablas de esa versión; no
  mezcla el dialecto con JSON, YAML ni con la gramática privada del toolchain.
- El documento es UTF-8, case-sensitive y tiene una sola raíz. LF y CRLF son
  equivalentes, los comentarios se descartan y no existen documentos múltiples
  ni marcadores de inicio/fin.
- El modelo dinámico es `TomlValue`, con tablas ordenadas por inserción y
  arrays heterogéneos. No conserva comentarios, spelling numérico, whitespace
  ni identidad de tablas.
- Los cuatro tipos temporales TOML se representan con los valores de
  `std.time`; un offset fijo usa `TomlOffsetDateTime` y nunca se convierte
  implícitamente en una zona del host. Las fracciones superiores a nueve
  dígitos se rechazan para no truncar información al usar el time-base de
  nanosegundos de Tondo.
- Las claves y tablas se resuelven en una máquina explícita de paths. Una
  definición duplicada, una extensión de inline table o una colisión entre
  scalar/table falla antes de publicar un valor.
- Todas las cotas son finitas, parte de `TomlOptions` y no dependen del host.
  El parser y el reader usan frames/worklists explícitos, nunca la pila
  recursiva del host.
- No hay includes, interpolación de environment, macros, schema discovery,
  lookup de locale/TZ ni ejecución. La única suspensión procede del adaptador
  `std.io.Reader`/`Writer`; no hay API async duplicada ni operación `selectable`.

## Superficie pública canónica

These declarations specify the intended Tondo 0.1 surface; they are not yet
callable from Tondo source. The current executable Rust kernel is described in
the usage guide below.

```tondo
pub type Toml

pub type TomlOffsetDateTime = {
    local: DateTime
    offset: UtcOffset
}

pub enum TomlValue {
    Null
    Bool(Bool)
    Int(Int64)
    UInt(UInt64)
    Float(Float64)
    Text(String)
    OffsetDateTime(TomlOffsetDateTime)
    LocalDateTime(DateTime)
    LocalDate(Date)
    LocalTime(Time)
    Array(Array[TomlValue])
    Table(Map[String, TomlValue])
}

pub type TomlValueView

pub enum TomlScalar {
    Bool(Bool)
    Int(Int64)
    UInt(UInt64)
    Float(Float64)
    Text(String)
    OffsetDateTime(TomlOffsetDateTime)
    LocalDateTime(DateTime)
    LocalDate(Date)
    LocalTime(Time)
}

pub enum TomlEvent {
    StreamStart
    TableStart(Array[String])
    ArrayTableStart(Array[String])
    TableEnd
    Key(Array[String])
    Scalar(TomlScalar)
    ArrayStart
    ArrayEnd
    InlineTableStart
    InlineTableEnd
    StreamEnd
}

pub type TomlLimits = {
    maxInputBytes: Int
    maxDepth: Int
    maxNodes: Int
    maxTables: Int
    maxArrayElements: Int
    maxKeyBytes: Int
    maxPathSegments: Int
    maxScalarBytes: Int
    maxStringBytes: Int
    maxArrayTableRows: Int
}

pub type TomlOptions = {
    limits: TomlLimits
}

pub enum TomlPathSegment {
    Key(String)
    Index(Int)
}

pub type TomlSpan = {
    startOffset: Int
    endOffset: Int
    startLine: Int
    startColumn: Int
    endLine: Int
    endColumn: Int
}

pub enum TomlErrorKind {
    InvalidLimit
    InvalidUtf8
    InvalidCharacter
    InvalidComment
    InvalidKey
    EmptyKey
    InvalidEscape
    InvalidString
    InvalidNumber
    IntegerOutOfRange
    InvalidDateTime
    DateTimePrecision
    InvalidArray
    InvalidTable
    InvalidTableArray
    DuplicateKey
    DuplicateTable
    TableAfterValue
    InlineTableExtension
    MissingValue
    UnexpectedToken
    TrailingInput
    DepthLimit
    NodeLimit
    TableLimit
    ArrayLimit
    KeyLimit
    ScalarLimit
    StringLimit
    ResourceLimit
    TypeMismatch
    MissingField
    UnknownField
    Io(std.io.IoError)
    Closed
    NoProgress
}

pub type TomlError = {
    kind: TomlErrorKind
    span: TomlSpan
    path: Array[TomlPathSegment]
}

pub type TomlReader
pub type TomlWriter

pub fn TomlLimits.defaults(): TomlLimits
pub fn TomlLimits.create(
    maxInputBytes: Int,
    maxDepth: Int,
    maxNodes: Int,
    maxTables: Int,
    maxArrayElements: Int,
    maxKeyBytes: Int,
    maxPathSegments: Int,
    maxScalarBytes: Int,
    maxStringBytes: Int,
    maxArrayTableRows: Int,
): TomlLimits ! TomlError

pub fn TomlOptions.defaults(): TomlOptions
pub fn TomlOptions.create(limits: TomlLimits): TomlOptions

pub fn parse(input: Bytes, options: TomlOptions): TomlValue ! TomlError
pub fn parseView(input: Bytes, options: TomlOptions): TomlValueView ! TomlError
pub fn validate(input: Bytes, options: TomlOptions): Unit ! TomlError

pub fn decode[T: Decode[Toml]](input: Bytes, options: TomlOptions): T ! TomlError
pub fn encode(value: TomlValue, options: TomlOptions): Bytes ! TomlError
pub fn encode[T: Encode[Toml]](value: T, options: TomlOptions): Bytes ! TomlError
pub fn encodeCanonical(value: TomlValue, limits: TomlLimits): Bytes ! TomlError

pub fn TomlReader.fromBytes(input: Bytes, options: TomlOptions): TomlReader ! TomlError
pub fn TomlReader.fromReader(var input: std.io.Reader, options: TomlOptions): TomlReader ! TomlError suspends
pub fn TomlReader.next(var self): TomlEvent? ! TomlError suspends
pub fn TomlReader.own(var self, event: TomlEvent): TomlEvent ! TomlError
pub fn TomlReader.finish(var self): Unit ! TomlError suspends

pub fn TomlWriter.toWriter(var output: std.io.Writer, options: TomlOptions): TomlWriter ! TomlError suspends
pub fn TomlWriter.write(var self, event: TomlEvent): Unit ! TomlError suspends
pub fn TomlWriter.finish(var self): Unit ! TomlError suspends
```

`TomlOptions` es `Copy + Discard + Send + Share` e inmutable. Los límites
rechazan negativos, cero que no permita representar el documento raíz vacío y
cualquier combinación que pueda overflowear la suma de `maxNodes`,
`maxTables` o `maxArrayElements`. El documento vacío se representa como una
tabla raíz vacía; `parse` nunca acepta bytes posteriores a `StreamEnd` salvo
whitespace y comentarios. `TomlLimits.maxArrayTableRows` acota cada array de
tablas y su suma por documento.

## Sintaxis TOML 1.1.0 y modelo de datos

El parser acepta comentarios con `#`, whitespace de espacio o tab, LF/CRLF,
claves bare/quoted/dotted, strings basic/multiline-basic/literal/
multiline-literal, números decimales/hexadecimales/octales/binarios con
underscores válidos, floats con fracción/exponente e `inf`/`nan`, booleanos
`true`/`false`, los cuatro tipos temporales, arrays multilinea con trailing
comma, inline tables con trailing comma y tablas/arrays de tablas. TOML 1.1
permite `\e` y `\xHH` además de `\b`, `\t`, `\n`, `\f`, `\r`, `\"`, `\\`,
`\uHHHH` y `\UHHHHHHHH`; toda escape no listada es inválida.

Los bare keys son ASCII `A-Z`, `a-z`, `0-9`, `_` y `-`; las quoted keys
admiten Unicode y strings basic/literal de una sola línea. Las partes de una
dotted key conservan su spelling semántico como `String`; `1234` es una key,
nunca un número. Las claves vacías quoted son válidas pero se mantienen bajo
las mismas cotas que cualquier otra key.

El schema dinámico es:

| Wire TOML | `TomlValue` | Regla de pérdida |
|---|---|---|
| string | `Text` | se descartan delimitadores y escapes |
| integer | `Int` o `UInt` | se exige representación lossless de 64 bits |
| float | `Float` | `inf`/`nan` se conservan como IEEE-754 |
| boolean | `Bool` | solo lowercase en el wire |
| offset date-time | `OffsetDateTime` | conserva fecha/hora local y offset fijo |
| local date-time | `LocalDateTime` | no representa un instante |
| local date | `LocalDate` | calendario civil de `std.time` |
| local time | `LocalTime` | precisión nanosegundo, sin zona |
| array | `Array` | puede ser heterogéneo y conserva orden |
| table/inline table | `Table` | keys únicas, orden de inserción |

Los enteros negativos y positivos representables en `Int64` usan `Int`; un
entero no negativo mayor que `Int64.max` usa `UInt64`. Valores fuera de esos
rangos producen `IntegerOutOfRange`, no wraparound. La raíz siempre es
`Table`; no existe un TOML escalar como documento completo.

TOML permite arrays heterogéneos, pero un destino typed `Array[T]` exige que
cada elemento sea decodificable como `T`. Los arrays de tablas se representan
como `Array` de `Table`; el parser conserva el orden de sus filas y nunca los
aplana. Una inline table queda cerrada al terminar sus llaves: intentar
añadirle una dotted key o una tabla posterior produce `InlineTableExtension`.

Las comments se descartan del valor. No se preservan lexical spelling,
indentación, orden global de declaraciones, delimitadores de string ni
underscores numéricos. The designed Tondo `TomlValueView` has an operation-bound
borrow; the current Rust `TomlValueView` borrows the input slice until its Rust
lifetime ends and is not tied to a reader advance.

## Fecha/hora y precisión

Los cuatro tipos del wire se interpretan sin locale:

- offset date-time usa `TomlOffsetDateTime { local: DateTime, offset: UtcOffset }`;
  `Z` es offset cero y otros offsets no crean una `TimeZone` ni consultan el
  bundle de zonas;
- local date-time usa `DateTime`, local date usa `Date` y local time usa `Time`;
- se acepta `T` o un espacio donde TOML 1.1 lo permite y segundos omitidos se
  normalizan a `00`;
- la fracción tiene uno a nueve dígitos y se normaliza a nanosegundos; más de
  nueve dígitos produce `DateTimePrecision` en vez de truncarse;
- no se aceptan segundos intercalares, años fuera del rango de `std.time` ni
  offsets fuera de `UtcOffset`.

La ruta typed decodifica a `TomlOffsetDateTime`, `DateTime`, `Date` y `Time`
respectivamente. No convierte un offset en `UtcDateTime` automáticamente: la
conversión requiere que el programa elija explícitamente cómo tratar el
offset. Los tipos usan el calendario civil ya cerrado de `std.time` y no
requieren `civil-clock` ni `clock`.

## Claves, tablas y duplicados

La máquina mantiene un trie de paths dentro de los límites declarados. Cada
key/value define exactamente una hoja; una segunda definición de la misma key,
una tabla definida dos veces, una tabla después de haber sido convertida en
scalar o una array-of-tables incompatible produce un error con el span de la
segunda declaración. Los super-tables implícitos se materializan sin emitir
datos adicionales.

`[table]` abre una tabla ordinaria, `[[table]]` añade una nueva fila al array
de tablas y las dotted keys crean solo los padres que todavía no existen.
Definir explícitamente un padre ya creado implícitamente es válido cuando no
hay colisión; redefinir una tabla cerrada no lo es. Las inline tables son
inmutables y no pueden mezclarse con headers posteriores. No hay merge,
include, herencia, interpolación ni resolución de aliases.

## Ruta typed, annotations y errores

`Decode[Toml]` visita fields en orden de declaración al codificar y acepta
cualquier orden wire al decodificar. `@name` y `@ignore` conservan la
semántica común de `std.serialization`; no existe un conjunto paralelo de
annotations TOML. Un field ausente solo se reconstruye como `none` cuando el
destino es `Option[T]`; los demás producen `MissingField`. Fields desconocidos,
duplicados, tipos temporales incompatibles y arrays heterogéneos incompatibles
producen `UnknownField`, `DuplicateKey` o `TypeMismatch` con path estable.

`TomlError.span` es half-open en bytes UTF-8 y contiene línea/columna 1-based
de inicio y fin. `path` contiene `Key`/`Index` semánticamente estables; los
errores de límites, configuración o estado usan span cero y path vacío. No se
publica un resultado parcial. Tras un error estructural, I/O, límite o estado,
reader/writer quedan terminales. The designed Tondo interface returns `Closed`
after a terminal error; the current Rust kernel may repeat the stored error,
as recorded in the executable guide.

## Streaming, eventos y ownership

`parse`, `parseView`, `validate`, `decode` y `encode` son collectors de la
misma máquina que alimenta `TomlReader`/`TomlWriter`. `TomlReader.next` emite
`StreamStart`, headers, keys, valores estructurales y `StreamEnd`; un
`TableEnd` aparece antes de cada header siguiente y antes de `StreamEnd`.
`Key(path)` siempre precede al valor que define. `ArrayTableStart(path)` abre
una fila nueva y se cierra con `TableEnd`. El writer valida este balance, la
unicidad de paths y la regla de inline tables antes de escribir.

`fromReader`, `next`, `finish`, `toWriter`, `write` y `finish` son las únicas
operaciones suspendibles. `fromBytes` es pura. `TomlReader` y `TomlWriter` son
handles afines: no son `Copy`, `Share` ni `Clone`, pueden transferirse cuando
son `Send` y deben terminar con `finish`. The designed Tondo `fromBytes` does
not retain the caller's input and its streaming interface is specified to keep
bounded frames, paths, and the current token. The current Rust reader instead
buffers the whole input and event list; the Rust writer buffers events until
`finish`.

Dividir el input en chunks arbitrarios (incluido el corpus `one-byte-chunks`),
incluso dentro de UTF-8, escapes,
strings multilinea, números, arrays o headers, produce los mismos eventos,
valores, spans y errores que un buffer contiguo. Un chunk vacío no cambia el
estado. `finish` es obligatorio y terminal; `NoProgress` se devuelve cuando un
writer no avanza. Los límites durante un evento son atómicos: el evento no se
consume si no puede aceptarse completo.

## Encoding y canonicalidad

`encode` emite un documento TOML único, UTF-8, LF, sin comentarios y con una
representación segura para volver a obtener el mismo modelo. Conserva el
orden de inserción de tablas/keys materializadas; los records typed siguen el
orden de declaración. Usa bare keys cuando son legales y quoted keys en otro
caso, strings basic cuando el escape es inequívoco, y literal strings cuando
reducen escapes sin cambiar el valor.

`encodeCanonical` es la política reproducible de Tondo, no una canonicalización
universal de todos los parsers TOML: ordena keys por sus bytes UTF-8 en cada
tabla, conserva el orden de arrays y filas de array-of-tables, usa headers
ordinarios y `[[...]]` deterministas, normaliza números/fechas a su spelling
canónico y emite `inf`, `-inf` y `nan` en lowercase. Nunca emite comentarios,
inline-table ambiguo, trailing whitespace ni una expansión dependiente del
host.

Antes de publicar bytes se comprueban `maxInputBytes`, límites de nodes,
tables, arrays, keys, scalars y `ResourceLimits.max_vm_heap_bytes`. Un exceso
devuelve el error correspondiente sin reservar una estructura ilimitada ni
publicar bytes parciales.

## Seguridad, rendimiento y portabilidad

La ruta escalar es el oráculo normativo. SIMD/multiversioning solo puede
acelerar UTF-8, delimitadores, números y copia de strings después de demostrar
igualdad de valores, spans, paths, errores, límites, eventos, terminalidad y
ownership. El dispatch depende únicamente del target declarado y del tamaño
del chunk.

El parser debe usar frames/worklists explícitos para arrays, tablas, dotted
keys y arrays-of-tables. No puede usar recursión del host para inputs anidados,
ni una tabla global entre documentos (TOML solo tiene una raíz). Los baselines
medirán throughput, tail latency, allocations, bytes copiados, memoria,
profundidad, número de tablas/filas y coste de rechazo adversarial. Do not
publish unqualified performance claims. `STD-TOML-PERF-001` records only the
target-qualified Rust scalar kernel baseline; it does not establish SIMD, VM,
or AOT performance.

## Separación del toolchain

`std.toml` es un codec de datos general. El toolchain usa `tondo.toml`,
`tondo.test.toml` y `tondo.lock.toml` como inputs con schemas privados y
fronteras de capabilities propias. Esos archivos se validan por los módulos de
proyecto/CLI, se normalizan a registros internos cerrados y no se importan
desde `.to`. `std.toml` no puede cambiar el package graph, seleccionar
capabilities, resolver dependencias, leer la red ni ejecutar el compiler por
el hecho de parsear un documento con esas keys.

## Implementación del kernel

STD-TOML-IMPL-001 queda cerrado para el kernel determinista de tondo-stdlib.
El estado machine-readable es verified-stdlib-kernel: el módulo
crates/tondo-stdlib/src/toml.rs publica el modelo TomlValue, las cuatro formas
temporales civiles, el parser con spans, duplicados y tablas/AOT, el encoder
canónico, el protocolo de eventos y los handles reader/writer atómicos.
serialization::Toml y lib.rs registran la identidad del codec sin crear una
ruta paralela para el toolchain.

La evidencia focalizada ejecuta los dieciséis casos de toml::tests y clippy
estricto; la construcción es atómica y no se publica un valor o bytes parciales
cuando falla una cota, un token, un duplicado, un span o el balance de eventos.
La implementación usa la misma política para parse, parse_view, validate,
encode, encodeCanonical, TomlReader y TomlWriter.

El límite de ejecución es intencionado y está sellado en el registro:

~~~text
implementation.status: verified-stdlib-kernel
host: not-claimed-until-compiler-toml-abi
native_aot_lowering: not-claimed
public_api_promoted: false
~~~

No existe todavía un intrinsic std.toml en el compilador/HIR/VM ni un ABI
Tondo para conectar std.io.Writer; por eso este bloque no reclama ejecución
hosted, runtime nativo ni lowering AOT, y no introduce una fixture .to
artificial. La ruta typed Rust solo usa std.serialization y no interpreta
tondo.toml. El contrato de tests y su evidencia independiente están en
[testing/stdlib-toml-test.json](../../testing/stdlib-toml-test.json) y
[stdlib-toml-test.md](./stdlib-toml-test.md). STD-TOML-TEST-001 queda cerrado
por el modelo canónico, corpus, chunking, límites, spans y fuzz bounded; no
mueve esta frontera. STD-TOML-PERF-001 mide el kernel Rust en un target concreto,
con la misma separación de ejecución. Su registro y límites de interpretación
están en [testing/stdlib-toml-performance.json](../../testing/stdlib-toml-performance.json)
y [stdlib-toml-performance.md](./stdlib-toml-performance.md).

STD-TOML-CONF-001 compara seis casos de los mismos bytes TOML entre un
callable host privado ejecutado por bytecode verificado en el VM y otro
proceso nativo que llama al kernel Rust. Comprueba typed/dynamic, TOML 1.1,
orden canónico, eventos con fragmentos de un byte, errores con path/span y
rechazo atómico por límite. La frontera exacta, el corpus y el reporte están
en [testing/stdlib-toml-conformance.json](../../testing/stdlib-toml-conformance.json)
y [stdlib-toml-conformance.md](./stdlib-toml-conformance.md). Esta prueba
no registra `std.toml` como API pública en el compilador ni establece ABI o
lowering AOT.

## Exclusiones deliberadas y leaves posteriores

Este contrato no incluye TOML 1.0 como dialecto alternativo, documentos
múltiples, includes, macros, `environment-interpolation`, locale, TZ, comments
preservados, edición round-trip, schema discovery, valores binarios implícitos,
segundos intercalares, offsets fuera de `std.time`, fracciones de más de nueve
dígitos, futures duplicadas ni `selectable`.

La API pública compiler/host, el ABI nativo y el lowering AOT continúan sin
implementarse.

## Executable usage guide for `std.toml`

`STD-TOML-DOC-001` documents the current Rust kernel. Run the example from the
repository root:

```sh
CARGO_TARGET_DIR=target-fast cargo run -q -p tondo-stdlib --example toml_usage --locked
```

It prints exactly `toml-doc-ok` after checking materialized and typed values,
a borrowed source view, buffered events, duplicate-key spans, finite limits,
and terminal reader/writer behavior. The source is
[`crates/tondo-stdlib/examples/toml_usage.rs`](../../crates/tondo-stdlib/examples/toml_usage.rs).
The checker [`scripts/stdlib-toml-doc-check.sh`](../../scripts/stdlib-toml-doc-check.sh)
and its negative tests verify the example and documentation record.

### Data versus project manifest

Use this codec for application TOML data. `tondo.toml`, `tondo.test.toml`, and
`tondo.lock.toml` are toolchain inputs with separate schema and validation.
Parsing their bytes as generic TOML does not validate a project, change a
package graph, select capabilities, or load dependencies. Do not route project
manifests through `std.toml` automatically.

### Policies and limits

Pass `TomlOptions` explicitly. `TomlLimits::defaults()` is finite but permits
up to 64 MiB of input and one million nodes; choose smaller limits for a
bounded input contract. `TomlLimits` also bounds depth, tables, array elements,
key/path/scalar/string bytes, and array-of-table rows. A zero or internally
inconsistent limit is rejected. No environment, filesystem, locale, clock, or
time-zone lookup changes parsing. TOML 1.1.0 has one UTF-8 document, rejects
duplicate definitions and inline-table extension, and does not have a wire
`null` or binary value. A `TomlValue::Null` from the common adapter cannot be
encoded as TOML. Civil dates and local times stay civil; fixed offsets are not
converted through a host time zone. More than nine fractional second digits
are rejected.

### Errors and ownership

`parse` either returns one complete `TomlValue` or a `TomlError`; it never
returns a partial tree. `TomlError` carries a kind, a half-open UTF-8 byte
span, one-based line/column coordinates, and a `Key`/`Index` path where
available. For example, a repeated `first` key on line two reports
`DuplicateKey` at byte offset 10 with path `first`. Callers own returned
values and encoded bytes. A `TomlValueView` borrows the original input; keep
that input alive while using `bytes()` or `clone_value()`.

`TomlReader::from_bytes`, `TomlReader::from_reader`, and
`TomlReader::from_chunks` buffer and
materialize the complete document before exposing events. Consume events
through `StreamEnd`, then call `finish()`; calling `next()` once more first
marks end-of-stream and makes `finish()` return `Closed`. `TomlWriter::to_writer`
buffers events and returns complete bytes from `finish()`, or an error without
partial output. A successful finish closes each handle. After a writer or
reader error, the current Rust kernel can repeat the original terminal error;
callers should discard that handle. The designed Tondo `std.io` streaming
interface is not an executable public language API yet.

### Costs and performance

`parse` materializes the full value. `parse_view` validates by parsing and
dropping a full value before returning a borrowed byte view, and
`clone_value()` parses again; this is not a zero-allocation validation path.
`TomlReader` stores the complete event list, and `TomlWriter` stores events
until finish. `encode` emits insertion order; `encode_canonical` sorts keys
deterministically and discards comments, original whitespace, and numeric
spelling. Canonical encoding is a normalized output form, not a source-editing
round trip. The measured 13-workload, 27-sample hosted Rust scalar baseline is
target-qualified to `x86_64-unknown-linux-gnu` in
[`stdlib-toml-performance.md`](./stdlib-toml-performance.md). Its logical
allocation and memory counters are not OS allocations or RSS. No SIMD, native
ABI, or native AOT performance claim follows from it.

### Executable kernel example

The four functions in `toml_usage.rs` demonstrate `materialized-and-typed`,
`borrowed-view-and-costs`, `buffered-events-and-lifecycle`, and
`errors-and-limits`. They execute the Rust stdlib kernel, including an
incomplete writer stream that fails without publishing bytes. A future `.to`
example would need the missing compiler API and production host bridge;
inventing one here would make a false execution claim.

### Promotion boundary

The guide proves usage of the Rust kernel only. The six-case VM/native
conformance corpus calls the same kernel through private test adapters; it
does not expose `std.toml` to Tondo source. Public compiler API, production
host registration, native ABI, native AOT lowering, and SIMD dispatch remain
unclaimed. After this owner guide, the next STD-0.1B owner leaf is
`STD-CBOR-IMPL-001`.
