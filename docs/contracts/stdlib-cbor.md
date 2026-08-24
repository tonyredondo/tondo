# Contrato de `std.cbor`

**Estado:** contrato `contract-locked` para STD-0.1B, cerrado por
`STD-CBOR-001`. La implementación de VM/host y el backend nativo permanecen
pendientes de sus leaves posteriores a `NATIVE-001`.

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

Los payloads de bytes y texto son vistas hasta el siguiente `next`; `own`
materializa una copia y es la única forma de conservarlos fuera de ese límite.
El reader es invariante a fragmentar la entrada en un byte, en la cabecera, en
cualquier frontera de chunk o en bloques grandes. `finish` exige que se haya
consumido exactamente la raíz, permite solo EOF y deja el estado terminal.

`CborWriter` valida balance de frames, que map keys y values estén alternados,
que los chunks solo aparezcan dentro de su string indefinido y que el break
solo cierre el frame correcto. Un error de I/O, límite o secuencia no escribe
un miembro parcial según el contrato de `std.io.Writer`; writer y reader
entran en estado terminal y no se reutilizan.

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
el modo determinista sin una validación posterior. `CborValueView` puede
prestar slices del input, pero su vida termina al avanzar el reader o salir de
la operación que la devuelve.

## Separación de alcance y promoción

`std.cbor` no interpreta `tondo.toml`, `tondo.test.toml` ni
`tondo.lock.toml`, no modifica el package graph y no tiene includes,
environment interpolation, locale, timezone lookup, schema discovery ni
capabilities requeridas. CBOR es un codec binario general; COSE, CDDL,
CBOR-LD, tags de IP y tags de tiempo con política de aplicación son owners o
protocolos posteriores.

El contrato machine-readable, la documentación y los checks negativos son
[`testing/stdlib-cbor.json`](../../testing/stdlib-cbor.json),
[`scripts/stdlib-cbor-check.sh`](../../scripts/stdlib-cbor-check.sh) y
[`scripts/stdlib-cbor-test.sh`](../../scripts/stdlib-cbor-test.sh). El contrato
queda cerrado como diseño B0; implementación, host, fuzzing, rendimiento,
conformance y documentación de uso permanecen pendientes de
`STD-CBOR-IMPL-001`, `STD-CBOR-TEST-001`, `STD-CBOR-PERF-001`,
`STD-CBOR-CONF-001` y `STD-CBOR-DOC-001`.
