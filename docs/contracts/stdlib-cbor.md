# Contrato de `std.cbor`

**Estado:** contrato `contract-locked` para STD-0.1B, cerrado por
`STD-CBOR-001`. The Rust scalar kernel is `verified-stdlib-kernel`
under `STD-CBOR-IMPL-001`; the public compiler API, production host registration,
native ABI, and native AOT lowering remain unclaimed. The production host
boundary is `not-claimed-until-compiler-cbor-abi`.

`std.cbor` implementa el modelo de datos de CBOR definido por RFC 8949. Es un
codec de datos y no un protocolo de aplicación: conserva tags, bytes y valores
simples sin inventar un registro global de tipos. La frontera de Tondo usa un
único data item por documento, aunque el wire model de CBOR permita que un
transporte externo concatene items. El registro machine-readable es
[`testing/stdlib-cbor.json`](../../testing/stdlib-cbor.json) y este documento
se integra desde [`TONDO_STANDARD_LIBRARY_SPEC.md`](../../TONDO_STANDARD_LIBRARY_SPEC.md).

## Principios del owner

- La versión wire es **RFC 8949 / CBOR**. Se aceptan los major types 0 a 7,
  tags, longitudes definidas e indefinidas y valores simples bien formados.
  La API no mezcla CBOR con MessagePack, JSON ni CDDL.
- Un documento es un único data item binario. `parse` y `validate` rechazan
  trailing data; una secuencia de items requiere una API de framing posterior,
  no una interpretación silenciosa del documento.
- El árbol dinámico conserva orden de arrays, orden de pares de map, tags,
  bytes arbitrarios, valores `undefined`, simples no asignados y la precisión
  de los floats mediante variantes explícitas. No conserva la forma de longitud
  definida/indefinida; `CborRaw` conserva los bytes originales cuando esa
  distinción es relevante.
- Los tags son `UInt64` y se preservan sin resolverlos mediante reflection,
  locale, environment, zona horaria o registro mutable. Los tags de tiempo,
  bignum y contenido CBOR tienen helpers explícitos en leaves posteriores, no
  conversiones implícitas del codec.
- La ruta ordinaria acepta representaciones no mínimas y NaN con cualquier
  payload bien formado. `encodeDeterministic` aplica las reglas de preferred
  serialization, prohíbe longitudes indefinidas y ordena maps por los bytes de
  la codificación determinista de sus claves.
- Todas las cotas son finitas, parte de `CborLimits` y se comprueban antes de
  crecer buffers, stacks o colecciones. El parser y el reader/writer usan
  frames y worklists explícitos, nunca la pila recursiva del host.
- No hay includes, schema discovery, ejecución ni capabilities implícitas. La
  única suspensión procede del adaptador `std.io.Reader`/`Writer`; no hay API
  async duplicada ni operación `selectable`.

## Superficie pública canónica

Estas declaraciones son la única superficie pública de `std.cbor` en 0.1.

```tondo
pub type Cbor

pub type CborEntry = { key: CborValue, value: CborValue }
pub type CborTag = { number: UInt64, value: CborValue }
pub type CborFloat16 = { bits: UInt16 }

pub enum CborValue {
    Null
    Undefined
    Bool(Bool)
    Simple(UInt8)
    UInt(UInt64)
    Negative(UInt64)
    Float16(CborFloat16)
    Float32(Float32)
    Float64(Float64)
    Bytes(Bytes)
    Text(String)
    Array(Array[CborValue])
    Map(Array[CborEntry])
    Tag(CborTag)
}

pub type CborValueView
pub type CborRaw
pub type CborPath

pub enum CborEvent {
    StreamStart
    Null
    Undefined
    Bool(Bool)
    Simple(UInt8)
    UInt(UInt64)
    Negative(UInt64)
    Float16(CborFloat16)
    Float32(Float32)
    Float64(Float64)
    Bytes(Bytes)
    Text(String)
    StartBytes(Int?)
    ByteChunk(Bytes)
    EndBytes
    StartText(Int?)
    TextChunk(String)
    EndText
    StartArray(Int?)
    EndArray
    StartMap(Int?)
    MapKey
    EndMap
    Tag(UInt64)
    StreamEnd
}

pub enum CborDuplicatePolicy { Preserve, Reject, First, Last }
pub enum CborUnknownTagPolicy { Preserve, Reject }
pub enum CborNonMinimalPolicy { Accept, Reject }
pub enum CborIndefinitePolicy { Accept, Reject }

pub type CborLimits = {
    maxDocumentBytes: Int
    maxDepth: Int
    maxArrayItems: Int
    maxMapPairs: Int
    maxStringBytes: Int
    maxByteStringBytes: Int
    maxChunks: Int
    maxTags: Int
    maxSimpleValues: Int
    maxEvents: Int
    maxOutputBytes: Int
}

pub type CborDecodeOptions = {
    limits: CborLimits
    dynamicMapDuplicates: CborDuplicatePolicy
    typedMapDuplicates: CborDuplicatePolicy
    unknownTags: CborUnknownTagPolicy
    nonMinimal: CborNonMinimalPolicy
    indefinite: CborIndefinitePolicy
}

pub type CborEncodeOptions = {
    limits: CborLimits
    deterministic: Bool
}

pub enum CborErrorKind {
    UnexpectedEof
    InvalidInitialByte
    InvalidAdditionalInfo
    InvalidBreak
    InvalidUtf8
    InvalidSimpleValue
    InvalidFloat
    InvalidLength
    NonMinimalEncoding
    IndefiniteNotAllowed
    InvalidChunk
    InvalidTag
    UnknownTag
    TypeMismatch
    DuplicateKey
    DeterministicKeyCollision
    OutOfOrderKey
    NumberRange
    LimitExceeded
    TooManyChunks
    TooManyTags
    TrailingData
    IoError
    Closed
    NoProgress
}

pub type CborError = {
    kind: CborErrorKind
    startOffset: Int
    endOffset: Int
    path: Array[CborPath]
}

pub fn CborLimits.defaults(): CborLimits
pub fn CborLimits.create(
    maxDocumentBytes: Int,
    maxDepth: Int,
    maxArrayItems: Int,
    maxMapPairs: Int,
    maxStringBytes: Int,
    maxByteStringBytes: Int,
    maxChunks: Int,
    maxTags: Int,
    maxSimpleValues: Int,
    maxEvents: Int,
    maxOutputBytes: Int,
): CborLimits ! CborError

pub fn CborDecodeOptions.defaults(): CborDecodeOptions
pub fn CborEncodeOptions.defaults(): CborEncodeOptions

pub fn parse(input: Bytes, options: CborDecodeOptions): CborValue ! CborError
pub fn parseView(input: Bytes, options: CborDecodeOptions): CborValueView ! CborError
pub fn decode[T: Decode[Cbor]](input: Bytes, options: CborDecodeOptions): T ! CborError
pub fn encode(value: CborValue, options: CborEncodeOptions): Bytes ! CborError
pub fn encode[T: Encode[Cbor]](value: T, options: CborEncodeOptions): Bytes ! CborError
pub fn validate(input: Bytes, options: CborDecodeOptions): Unit ! CborError
pub fn encodeDeterministic(value: CborValue, limits: CborLimits): Bytes ! CborError
pub fn raw(input: Bytes, options: CborDecodeOptions): CborRaw ! CborError
pub unsafe fn rawUnchecked(input: Bytes): CborRaw

pub fn CborReader.fromBytes(input: Bytes, options: CborDecodeOptions): CborReader ! CborError
pub fn CborReader.fromReader(var input: std.io.Reader, options: CborDecodeOptions): CborReader ! CborError suspends
pub fn CborReader.next(var self): CborEvent? ! CborError suspends
pub fn CborReader.own(var self, event: CborEvent): CborEvent ! CborError
pub fn CborReader.finish(var self): Unit ! CborError suspends

pub fn CborWriter.toWriter(var output: std.io.Writer, options: CborEncodeOptions): CborWriter ! CborError suspends
pub fn CborWriter.write(var self, event: CborEvent): Unit ! CborError suspends
pub fn CborWriter.finish(var self): Unit ! CborError suspends
```

`CborLimits`, `CborDecodeOptions` y `CborEncodeOptions` son `Copy + Discard +
Send + Share` e inmutables. Todos los límites son positivos y se rechazan si
una suma interna puede overflowear. `CborReader` y `CborWriter` son affine,
no `Copy`, no `Share` y solo `Send`; después de `finish` o de un error quedan
terminales y toda operación posterior devuelve `CborError.Closed`.

## Modelo wire y valores

CBOR codifica un data item con un byte inicial: los major types 0 y 1 son
enteros no negativos y negativos, 2 es byte string, 3 text string, 4 array,
5 map, 6 tag y 7 valores simples/floats. Los argumentos usan additional
information de 0 a 27; 31 solo es válido como longitud indefinida o break en
el contexto correspondiente. Los argumentos con valores reservados o una
longitud imposible fallan como `InvalidAdditionalInfo` o `InvalidLength`.

| Wire | `CborValue` | Regla |
|---|---|---|
| major 0 | `UInt(UInt64)` | todos los valores de 0 a `2^64-1` |
| major 1 | `Negative(UInt64)` | representa `-1 - magnitude`, sin perder `-2^64` |
| major 2 | `Bytes(Bytes)` | bytes arbitrarios; no se interpreta como UTF-8 |
| major 3 | `Text(String)` | UTF-8 válido; chunks concatenados por el reader |
| major 4 | `Array` | orden y multiplicidad preservados |
| major 5 | `Map(Array[CborEntry])` | claves arbitrarias y pares ordenados |
| major 6 | `Tag(CborTag)` | tag anidado preservado, sin resolverlo |
| major 7, 20/21 | `Bool` | false/true |
| major 7, 22/23 | `Null`/`Undefined` | `undefined` no se convierte en null |
| major 7, 0..19/32..255 | `Simple(UInt8)` | valores no asignados preservados |
| major 7, 25/26/27 | `Float16`/`Float32`/`Float64` | bits y signo preservados en modo ordinario |

Los códigos simples reservados 20 a 23 se representan por sus variantes
dedicadas; 24 se usa como prefijo de un simple explícito y 31 es break, nunca
un valor. Una codificación de simple explícito con un código reservado, un
break fuera de una longitud indefinida o un valor de float no bien formado se
rechaza. `CborFloat16` conserva los 16 bits del wire porque Tondo no promete
un tipo numérico `Float16` global; la conversión a `Float32` es una operación
explícita del owner de runtime.

`CborTag.number` conserva el entero del tag y `value` conserva el item
anidado. Los tags 0/1 (tiempo), 2/3 (bignum), 4/5 (decimal/bigfloat), 24
(CBOR embebido) y cualquier tag privado se tratan igual en el codec. Un
caller puede validar o convertir un tag después de parsearlo; el codec no
consulta una tabla de tipos ni ejecuta el payload.

El árbol dinámico conserva pares de map duplicados y su orden cuando la policy
es `Preserve`; `Reject`, `First` y `Last` son opt-in y `Last` reemplaza el
valor manteniendo la primera posición. `decode[T]` aplica por defecto
`Reject` para maps de records typed. Ningún resultado parcial se publica si
un miembro posterior viola un límite, una policy o la validez del item.

## Longitudes, chunks y streaming

En modo ordinario se aceptan longitudes definidas e indefinidas para arrays,
maps, byte strings y text strings. Una longitud indefinida debe terminar con
un único break; un array/map solo contiene items completos y un string
indefinido solo contiene chunks del mismo tipo. Cada chunk de text debe ser
UTF-8 completo por sí mismo. Los chunks vacíos son válidos y no cambian el
estado; un break dentro de un chunk o un chunk de tipo distinto produce
`InvalidChunk`.

`parse` materializa una sola raíz. `CborReader` emite `StreamStart`, eventos de
items y `StreamEnd`; para una longitud indefinida emite `StartBytes(none)` o
`StartText(none)`, chunks y su `End*`, mientras que una longitud definida puede
emitir `Bytes`/`Text` directamente. `StartArray(none)` y `StartMap(none)`
representan longitudes indefinidas; el entero presente representa una longitud
definida. `Tag(number)` precede al único item anidado y no tiene un `EndTag`.

The public Reader contract borrows byte/text payloads until its next `next`;
`own` materializes a copy for retention beyond that cursor boundary. The Rust
kernel currently returns owned events instead; the usage guide records that
implementation and its buffering costs.
El reader es invariante a fragmentar la entrada en un byte, en la cabecera, en
cualquier frontera de chunk o en bloques grandes. `finish` exige que se haya
consumido exactamente la raíz, permite solo EOF y deja el estado terminal.

`CborWriter` valida balance de frames, que map keys y values estén alternados,
que los chunks solo aparezcan dentro de su string indefinido y que el break
solo cierre el frame correcto. A codec limit or invalid sequence never returns
a successful partial item. External writes follow the partial-I/O contract of
`std.io.Writer`: an accepted prefix or a failed flush cannot be rolled back.
An I/O failure is terminal and returns no successful codec result; writer and
reader cannot be reused.

## Determinismo explícito

El modo ordinario es interoperable y tolerante: acepta encoding no mínimo,
longitudes indefinidas y cualquier representación de float bien formada. El
modo `encodeDeterministic` de Tondo se alinea con los requisitos core de RFC
8949, pero es una policy de la API, no una afirmación de canonicalización
universal:

1. enteros, longitudes y argumentos de tags usan el additional information más
   corto;
2. arrays, maps y strings usan longitudes definidas;
3. un float usa el formato 16, 32 o 64 más corto que conserva exactamente su
   valor y su signo; todos los NaN se normalizan a un quiet-NaN binario-16
   único (`0x7e00`), mientras `-0.0` se conserva;
4. tags y la secuencia de tags se conservan en el orden semántico;
5. las claves de cada map se ordenan lexicográficamente por los bytes de su
   `encodeDeterministic(key)`; y
6. dos claves con el mismo encoding determinista producen
   `DeterministicKeyCollision` antes de escribir, sin desempate por hash o
   dirección.

El writer determinista exige que el caller suministre las claves ya ordenadas
y rechaza `OutOfOrderKey`. El encoder materializado puede ordenar un map
acotado. `CborRaw` no se puede introducir en un encode determinista sin
decodificarlo: el modo determinista siempre valida la estructura y no permite
que bytes opacos oculten una longitud indefinida, un NaN no normalizado o una
clave fuera de orden.

## Límites y seguridad

`maxDocumentBytes` se comprueba antes de aceptar cada chunk; `maxDepth` antes
de push; `maxArrayItems`/`maxMapPairs` antes de append; `maxStringBytes` y
`maxByteStringBytes` antes de materializar; `maxChunks`, `maxTags` y
`maxSimpleValues` antes de cada incremento; `maxEvents` antes de cada evento;
`maxOutputBytes` antes de crecer el writer. La suma de límites y los tamaños
de additional information se comprueban con aritmética checked.

Las rutas de error usan `Array[CborPath]` con segmentos estables
`ArrayIndex`, `MapEntry`, `MapKey`, `MapValue` y `Tag`. Cada error conserva un
span half-open de bytes `[startOffset,endOffset)`; no hay columnas inventadas
para un formato binario ni snippets que puedan filtrar payloads. Un input
hostil no puede consumir la pila del host, publicar un árbol parcial ni
convertir un tag desconocido en una llamada dinámica.

## Typed, raw y ownership

`Encode[Cbor]` y `Decode[Cbor]` se generan en compile time y escriben/leen
directamente sobre la máquina del codec. `decode` typed no crea un
`CborValue` intermedio; `encode` typed no requiere un DOM. Los adapters para
records, enums, `@name` y `@ignore` obedecen el contrato común de
`std.serialization`; no existe un segundo sistema de reflection en CBOR.

`raw` valida un único data item y devuelve `CborRaw` con los bytes exactos,
incluyendo forma de longitud, orden y float. `rawUnchecked` es `unsafe` y solo
promete almacenamiento opaco; no puede entrar en `decode`, `validate` ni en
el modo determinista sin una validación posterior. A standalone `CborValueView`
borrows the original validated encoded input, whose owner must outlive the
view. It is independent of the Reader cursor and may survive the function
that created it while that input remains alive.

## Separación de alcance y promoción

RFC 8949 section 3.3 requires every two-byte simple encoding `f8 00` through
`f8 1f` to be rejected as `InvalidSimpleValue`, including when non-minimal
integer and length arguments are accepted. The independent bounded model,
persistent wire corpus and fuzz boundary are documented in
[stdlib-cbor-test.md](./stdlib-cbor-test.md) and registered by
[testing/stdlib-cbor-test.json](../../testing/stdlib-cbor-test.json).

`std.cbor` no interpreta `tondo.toml`, `tondo.test.toml` ni
`tondo.lock.toml`, no modifica el package graph y no tiene includes,
environment interpolation, locale, timezone lookup, schema discovery ni
capabilities requeridas. CBOR es un codec binario general; COSE, CDDL,
CBOR-LD, tags de IP y tags de tiempo con política de aplicación son owners o
protocolos posteriores.

The design contract and negative checks are
[testing/stdlib-cbor.json](../../testing/stdlib-cbor.json),
[scripts/stdlib-cbor-check.sh](../../scripts/stdlib-cbor-check.sh) and
[scripts/stdlib-cbor-test.sh](../../scripts/stdlib-cbor-test.sh).
`STD-CBOR-IMPL-001` verifies the bounded scalar Rust kernel: dynamic values,
exact validated raw bytes, borrowed encoded-byte views, static
primitive/collection codecs, owned event reader/writer, finite limits and
explicit deterministic encoding. Its fourteen focused tests and strict Clippy
are run by
[scripts/stdlib-cbor-implementation.sh](../../scripts/stdlib-cbor-implementation.sh).
Generated Tondo record/enum decoding, the public compiler API, production host
registration, native ABI and native AOT lowering remain unimplemented.

`STD-CBOR-TEST-001` verifies the independent model, 61 valid and 30 invalid
retained wire vectors, nine integration tests, 4096 deterministic seeds,
all 65536 binary16 patterns and bounded 128-run fuzz with unchanged sanitizers.
See [stdlib-cbor-test.md](./stdlib-cbor-test.md) and its register linked above.

`STD-CBOR-PERF-001` records fifteen bounded unoptimized Rust test-profile
routes, three independent processes and 27 retained samples per workload in
[testing/stdlib-cbor-performance.json](../../testing/stdlib-cbor-performance.json)
and [stdlib-cbor-performance.md](./stdlib-cbor-performance.md).
Clean-source measurement, exact independent oracles and report negatives pass.
Its logical resource models do not promote production VM, native ABI/AOT,
SIMD or allocator measurements.

`STD-CBOR-CONF-001` verifies seven shared cases through a private test-only
bytecode host callable and a native Rust process using the same kernel.
The independent model checks 61 valid and 30 invalid vectors before dispatch.
Exact typed/dynamic/event paths, raw/views, errors, terminal limits and
deterministic output are compared by
[testing/stdlib-cbor-conformance.json](../../testing/stdlib-cbor-conformance.json)
and [stdlib-cbor-conformance.md](./stdlib-cbor-conformance.md).
This is a bounded adapter comparison, not an independently implemented native
CBOR ABI or source-level public API.

The implementation, test, performance and conformance leaves passed their
required functional and provenance-bound quality gates. The latest conformance
campaign measures 91.5681% global lines, every global and risk-scope
line/function/region dimension meets the unchanged 80% floor, and all six
selected critical mutants are caught. `STD-CBOR-DOC-001` verifies the
executable usage below after the canonical example, documentation/owner
negatives and full functional/quality gates pass. Its renewed campaign also
measures 91.5681% global lines, all coverage dimensions at or above 80% and
six critical mutants caught. The register is `verified-rust-kernel-usage`;
the next owner block is `STD-REGEX-IMPL-001`.

## Executable usage guide for `std.cbor`

This guide describes the bounded Rust scalar kernel. The Tondo declarations
above remain the public language contract; compiler imports, production host
registration, the native CBOR ABI and native AOT lowering are unimplemented.
The executable examples use Rust methods and synchronous `std::io` adapters.
They do not establish execution of the public Tondo `suspends` methods.

### Values and byte preservation

Use `parse` for an owned `CborValue`, or `decode_typed` for existing primitive
and collection implementations of the common static `Decode[Cbor]` protocol.
`encode_typed` writes the corresponding static `Encode[Cbor]` events without
building a dynamic tree. Generated Tondo record/enum adapters are not part of
this kernel promotion.

Dynamic maps are ordered `CborEntry` pairs with arbitrary keys. Their default
duplicate policy is `Preserve`; typed maps default to `Reject`. `First` and
`Last` must be selected explicitly. For a dynamic map, `Last` keeps the first
pair position; a typed collection has its own iteration order.
`Negative(n)` represents `-1 - n`, including `-2^64`, and `Undefined` remains
distinct from `Null`. Rust `Float32` and `Float64` variants contain bit patterns;
`Float16(CborFloat16 { bits })` preserves binary16 bits.

Ordinary encoding preserves float widths, bits, tags and pair order, but it
uses minimal integer/length arguments and definite containers. Parsing loses
the original defined/indefinite length form. A semantic round trip therefore
does not promise identical input bytes. Use validated `raw` when the original
wire representation matters. It checks one complete item and copies its bytes;
the guide does not use `raw_unchecked`.

In the common Rust protocol, `Vec<u8>` is an array of integers. Use
`serialization::Bytes` for a CBOR byte string. `String` encodes UTF-8 text;
binary bytes do not undergo UTF-8 interpretation.

### Tags and deterministic encoding

Tags preserve their unsigned number and nested value. The kernel does not
interpret time, bignum, decimal or application tags. The default
`CborUnknownTagPolicy::Preserve` retains them; `Reject` rejects every tag because
there is no built-in tag registry.

Use `encode_deterministic` when the application needs this explicit policy:
minimal arguments, definite lengths, shortest exact floats, one quiet binary16
NaN (`0x7e00`), preserved negative zero and bytewise sorted deterministic map
keys. Equivalent key encodings fail with `DeterministicKeyCollision`; there is
no arbitrary tie-break. This policy is not a universal CBOR canonical form.

The materialized encoder sorts bounded maps and normalizes parsed indefinite
values into definite output. A deterministic `CborWriter` instead requires
already ordered keys and definite event frames; it rejects indefinite frames
and reports `OutOfOrderKey` for key-order violations.

### Policies and limits

Start from `CborDecodeOptions::default()` and `CborEncodeOptions::default()`.
Decode accepts well-formed non-minimal arguments and indefinite lengths by
default; either policy can explicitly reject them. Malformed two-byte simple
values below 32 remain invalid under both non-minimal policies.

`CborLimits::default()` supplies finite bounds for document/output bytes, depth,
array items, map pairs, string/byte payloads, chunks, tags, simple values and
events. Set limits appropriate to the input, then validate them with
`CborLimits::create`. Zero or values above `isize::MAX` fail. Invalid input,
policy violations and resource rejection return `CborError`, with no successful
partial value, raw item, view or reader.

### Errors, ownership and terminal streams

`CborError` carries a nominal kind, a half-open byte span and stable
`CborPath` segments. Binary input has byte offsets rather than text columns.
The executable nested UTF-8 example checks bytes `4..7` and the exact
array/map/value/tag path; human error wording is not the identity.

`parse_view` lends the original validated encoded bytes, whose owner must
outlive the view. It is independent of a Reader cursor. `view.own()` returns
an owned value by reparsing. Validated `CborRaw` owns its byte copy.

The current Rust `CborReader::from_reader` buffers the bounded document and
then all validated events. `next()` clones an owned event, and `own()` clones
it again. These owned Rust payloads are distinct from the borrowed public
Reader payload contract specified above. `StreamEnd` closes the event stream;
one subsequent `next()` returns `None`, further calls return `Closed`, and
`finish()` must still be called. Early finish fails and is terminal.

`CborWriter::to_writer` validates events and retains encoded bytes before
`finish()` writes and flushes the sink. Errors and finish are terminal; later
operations return `Closed`. A limit or invalid event before finish leaves the
sink untouched. External I/O is not transactional: a sink can accept a prefix
and then fail, or accept all bytes and fail to flush. The returned operation
fails with `IoError` or `NoProgress`; it cannot undo those external writes.

### Costs and performance

`parse` builds a DOM. `validate` currently builds and drops that same DOM, so
`parse_view` is not an allocation-free parser despite retaining borrowed bytes.
Its `own()` repeats parsing; `raw` additionally copies the validated document.
Typed decode buffers CBOR events and common protocol events without a dynamic
CBOR tree. Reader construction buffers input and owned events, and each event
delivery/copy has the cloning cost described above. Reader event storage and
Writer output/event buffers are retained until the Rust objects are dropped,
including after terminal `finish()`. Neither adapter promises constant-memory
incremental parsing or immediate sink delivery.

The target-qualified
[scalar performance contract](./stdlib-cbor-performance.md) records fifteen
bounded unoptimized Rust test-profile routes and 27 retained samples per route.
Latency and throughput are measured. Its copy/allocation/peak-memory metrics
are declared logical models, not allocator calls, RSS or a production native
codec benchmark. Native codec handles, SIMD and code size remain unmeasured.

### Executable kernel example

[cbor_usage.rs](../../crates/tondo-stdlib/examples/cbor_usage.rs) checks six
independent usage paths: materialized and typed values; tags, float preservation
and deterministic encoding; raw bytes and borrowed-view costs; fragmented
events and terminal lifecycle; exact errors and finite limits; and external
partial I/O. All assertions run through `scripts/stdlib-cbor-doc-check.sh`,
which compiles and executes this canonical source with the workspace lockfile.
Its complete successful stdout is `cbor-doc-ok`.

### Promotion boundary

`STD-CBOR-DOC-001` documents executable Rust-kernel usage. The independent
model, bounded fuzz, scalar performance campaign and private VM/native Rust
process comparison retain their own qualified evidence. This guide does not
implement or promote the public Tondo API, generated Tondo record/enum codecs,
production VM registration, a native CBOR ABI, native AOT or SIMD.
The next owner block after verified documentation is `STD-REGEX-IMPL-001`.
