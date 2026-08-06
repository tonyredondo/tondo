# Contrato de `std.messagepack`

**Estado:** contrato de API fuente cerrado para `STD-0.1A`; la implementación
typed, dynamic y streaming del owner está disponible y permanece en promoción
con evidencia de conformance independiente pendiente.

`std.messagepack` implementa el modelo binario de la especificación MessagePack
y reutiliza los traits estáticos de `std.serialization`. El registro
machine-readable [`testing/stdlib-messagepack.json`](../../testing/stdlib-messagepack.json)
describe las decisiones cerradas y
[`scripts/stdlib-messagepack-check.sh`](../../scripts/stdlib-messagepack-check.sh)
las valida dentro de `scripts/test-gate.sh`.

La sección 14.10 de `TONDO_STANDARD_LIBRARY_SPEC.md` sigue siendo la fuente
normativa del catálogo. Este documento fija la frontera operativa del owner sin
inventar un segundo modelo de serialization ni afirmar una canonicalización
universal que MessagePack no define.

## API fuente única

Las siguientes firmas son la única superficie pública de `std.messagepack`.
`encode`/`decode` son las operaciones materializadas y los readers/writers son
la misma máquina en modo incremental; no hay aliases por formato ni defaults
ambientales.

```tondo
pub type MessagePackEntry = { key: MessagePackValue, value: MessagePackValue }
pub enum MessagePackValue {
    Nil
    Bool(Bool)
    Int(Int64)
    UInt(UInt64)
    Float32(Float32)
    Float64(Float64)
    String(String)
    Binary(Bytes)
    Array(Array[MessagePackValue])
    Map(Array[MessagePackEntry])
    Ext(MessagePackExt)
}
pub type MessagePackExt = { typeCode: Int8, payload: Bytes }
pub type MessagePackTimestamp = { seconds: Int64, nanoseconds: Int32 }
pub type MessagePackPath

pub enum MessagePackEvent {
    Nil, Bool(Bool), Int(Int64), UInt(UInt64), Float32(Float32), Float64(Float64),
    String(String), Binary(Bytes), StartArray(Int?), EndArray,
    StartMap(Int?), MapKey, EndMap, Ext(MessagePackExt)
}
pub enum MessagePackDuplicatePolicy { Preserve, Reject, First, Last }
pub enum MessagePackUnknownExtensionPolicy { Preserve, Reject }
pub enum MessagePackNonMinimalPolicy { Accept, Reject }
pub type MessagePackLimits = {
    maxDocumentBytes: Int
    maxDepth: Int
    maxArrayItems: Int
    maxMapPairs: Int
    maxStringBytes: Int
    maxBinaryBytes: Int
    maxExtBytes: Int
    maxEvents: Int
    maxOutputBytes: Int
}
pub type MessagePackDecodeOptions = {
    limits: MessagePackLimits
    dynamicMapDuplicates: MessagePackDuplicatePolicy
    typedMapDuplicates: MessagePackDuplicatePolicy
    nonMinimal: MessagePackNonMinimalPolicy
    unknownExtensions: MessagePackUnknownExtensionPolicy
}
pub type MessagePackEncodeOptions = {
    limits: MessagePackLimits
    deterministic: Bool
}
pub enum MessagePackErrorKind {
    UnexpectedEof, InvalidTag, InvalidUtf8, InvalidLength, NonMinimalEncoding,
    InvalidExtension, TypeMismatch, DuplicateKey, NumberRange,
    DeterministicKeyCollision, OutOfOrderKey, LimitExceeded, IoError, TrailingData
}
pub type MessagePackError = { kind: MessagePackErrorKind, offset: Int, path: MessagePackPath }

pub fn decodeValue(input: Bytes, options: MessagePackDecodeOptions): MessagePackValue ! MessagePackError
pub fn decode[T: Deserialize](input: Bytes, options: MessagePackDecodeOptions): T ! MessagePackError
pub fn encodeValue(value: MessagePackValue, options: MessagePackEncodeOptions): Bytes ! MessagePackError
pub fn encode[T: Serialize](value: T, options: MessagePackEncodeOptions): Bytes ! MessagePackError
pub fn validate(input: Bytes, options: MessagePackDecodeOptions): Unit ! MessagePackError
pub fn encodeDeterministic(value: MessagePackValue, limits: MessagePackLimits): Bytes ! MessagePackError

pub fn MessagePackTimestamp.fromExt(value: MessagePackExt): MessagePackTimestamp ! MessagePackError
pub fn MessagePackTimestamp.toExt(self): MessagePackExt ! MessagePackError

pub fn MessagePackReader.fromBytes(input: Bytes, options: MessagePackDecodeOptions): MessagePackReader ! MessagePackError
pub async fn MessagePackReader.fromReader(var input: Reader, options: MessagePackDecodeOptions): MessagePackReader ! MessagePackError
pub async fn MessagePackReader.next(var self): MessagePackEvent? ! MessagePackError
pub fn MessagePackReader.own(var self, event: MessagePackEvent): MessagePackEvent ! MessagePackError
pub async fn MessagePackReader.finish(var self): Unit ! MessagePackError

pub fn MessagePackWriter.toWriter(var output: Writer, options: MessagePackEncodeOptions): MessagePackWriter ! MessagePackError
pub async fn MessagePackWriter.write(var self, event: MessagePackEvent): Unit ! MessagePackError
pub async fn MessagePackWriter.finish(var self): Unit ! MessagePackError
```

`MessagePackValue.Map` conserva pares y orden; no se ofrece un segundo
`Map[String, ...]`. `decode` typed nunca crea ese valor dinámico. `next` devuelve
`none` exactamente una vez después de la raíz, `own` copia vistas de string,
binary o ext y reader/writer quedan terminales tras error o `finish`. El modo
determinista se activa solo mediante `MessagePackEncodeOptions.deterministic`
o `encodeDeterministic`; exige claves ordenadas y rechaza colisiones de bytes.

## Modelo completo

`MessagePackValue` representa todos los tipos del wire model:

- `Nil` y `Bool`;
- `Int` signed y `UInt` unsigned, sin pasar por `Float64`;
- `Float32` y `Float64`, conservando sus bits en la ruta ordinaria;
- `String` UTF-8, `Binary` como bytes arbitrarios;
- `Array` ordenado; y
- `Map` como secuencia ordenada de pares `(MessagePackValue, MessagePackValue)`.

Una clave de map puede ser cualquier valor MessagePack, incluidos arrays,
maps, binarios y extensiones. Por eso el árbol dinámico no se representa como
`Map[String, Value]` ni se fuerza una función de hash para tipos que no la
tienen. La ruta dinámica conserva todos los pares y puede observar claves
duplicadas; un map typed aplica su policy de claves antes de publicar el
resultado.

`Ext` contiene un `type_code` signed de 8 bits y un payload `Bytes` sin
interpretar. Se admiten los tamaños fixed 1/2/4/8/16 y ext8/ext16/ext32. La
extensión timestamp estándar se convierte solo mediante una operación
explícita a `MessagePackTimestamp` (segundos Unix signed y nanosegundos); una
extensión desconocida permanece `Ext` y nunca se interpreta mediante
reflection, un registro global o una heurística de nombre.

## Typed y streaming

`Serialize` y `Deserialize` generan dispatch estático. El encode typed escribe
directamente a `std.io.Writer` y el decode typed consume un `Reader` sin crear
un `MessagePackValue` intermedio. Las sobrecargas materializadas con
`BytesBuilder` son una comodidad sobre la misma máquina.

`MessagePackReader` emite eventos incrementales para nil, bool, enteros,
floats, string, binary, arrays, maps y ext. Los payloads de string, binary y
ext son vistas válidas hasta el siguiente evento; `own` es la única conversión
explícita a almacenamiento estable. `MessagePackWriter` valida la secuencia
array/map, escribe prefijos y payloads directamente y queda terminal después
de un error de I/O, límite o estado.

Reader, writer y decoder usan un stack explícito de contenedores, acotado por
`max_depth`; una entrada profunda produce un error de recursos y nunca consume
el stack de llamadas del host. Fragmentar el input en un byte, en cada frontera
de token, dentro de UTF-8 o en chunks grandes conserva exactamente los mismos
eventos y valores.

## Representación mínima y determinismo

El encoder ordinario conserva el orden de arrays y de pares de map, acepta
representaciones wire válidas no mínimas y conserva los bits de floats y los
payloads de ext. El encoder determinista de Tondo (`encodeDeterministic`) no es
una promesa de canonical MessagePack interoperable:

1. usa siempre la forma válida más corta para enteros y longitudes;
2. usa `float32` solo cuando conserva exactamente el valor y los signos
   relevantes; en otro caso usa `float64`;
3. representa todos los NaN con un único quiet-NaN y conserva distintos `-0.0`
   y `0.0`;
4. conserva el orden de arrays y extensiones;
5. ordena los pares de map por los bytes de `encodeDeterministic(key)`; y
6. rechaza dos claves cuyo encoding determinista sea idéntico, sin desempate
   dependiente del layout o del hash interno.

Un `MessagePackWriter` determinista exige claves ya ordenadas y rechaza una
clave fuera de orden o una colisión antes de escribir el miembro. La operación
materializada puede ordenar un map acotado. Un decoder ordinario no rechaza una
representación no mínima salvo que `DecodeOptions` active explícitamente el
modo `reject`; interoperabilidad no se sacrifica por una optimización local.

## Policies de maps y extensiones

La policy dinámica por defecto es `preserve`: todos los pares del wire se
conservan aunque sus claves se repitan. `reject`, `first` y `last` son policies
explícitas para callers que necesiten una vista tipo diccionario; `last`
reemplaza el valor manteniendo la posición del primer par. En un map typed la
policy por defecto es `reject`, porque publicar silenciosamente una clave
duplicada perdería información. El modo determinista siempre rechaza la
colisión de bytes de clave.

Las extensiones desconocidas se preservan por defecto. Un caller puede pedir
`reject` de forma explícita antes de leer un payload. La conversión de timestamp
comprueba rango y nanosegundos, y no crea un `Instant` ni adelanta el calendario
civil; el owner de tiempo decide sus propias conversiones.

## Límites y errores

Cada perfil proporciona límites finitos para documento, profundidad, elementos
de array, pares de map, bytes de string, binary y ext, eventos y output. Los
prefijos de longitud se comprueban antes de reservar o leer el payload y los
contadores se validan antes de incrementarse. Un fallo de límite o de
materialización no publica un valor parcial.

Los errores estables son `UnexpectedEof`, `InvalidTag`, `InvalidUtf8`,
`InvalidLength`, `NonMinimalEncoding`, `InvalidExtension`, `TypeMismatch`,
`DuplicateKey`, `NumberRange`, `DeterministicKeyCollision`, `OutOfOrderKey`,
`LimitExceeded`, `IoError` y `TrailingData`. Las extensiones desconocidas no
son error mientras la policy sea `preserve`. Cada error contiene offset de
byte, path estructural y, cuando proceda, el contexto de map-key/map-value; no
copia automáticamente payloads grandes o secretos en el diagnóstico.

## Implementación del owner

`crates/tondo-stdlib/src/messagepack_api.rs` contiene la ruta ejecutable del
owner. `parse_one` consume tags con un `Vec<DecodeFrame>` explícito, comprueba
longitudes antes de reservar y aplica las policies de duplicados, extensiones y
formas no mínimas antes de publicar el árbol. El encoder usa una pila de tareas
para arrays y maps; el modo determinista normaliza NaN, ordena por bytes del
encoding de la clave y rechaza colisiones. La API compatible `Value`/`encode`/
`decode` continúa disponible para el bridge, mientras `encode_value`,
`decode_value`, `encode_typed` y `decode_typed` exponen las opciones cerradas.

`MessagePackReader` genera eventos desde la misma decodificación acotada,
acepta bytes, chunks y el adaptador bounded de `Read`, y queda terminal tras un
error o `finish`. `MessagePackWriter` valida la secuencia de eventos con otra
pila explícita y solo publica bytes después de cerrar la raíz. Los adaptadores
typed pasan por `std.serialization` y no introducen una tabla de reflection en
runtime. El corpus unitario cubre el modelo wire, políticas, timestamp,
fragmentación, límites, determinismo, terminalidad y round-trip typed.

## Corpus e interoperabilidad

Antes de implementar el owner deben existir identidades reproducibles para el
modelo completo de MessagePack, tags y longitudes no mínimas, floats y bits,
UTF-8 frente a binary, maps con claves arbitrarias, ext/timestamp, fragmentos,
límite y determinismo. `STD-CODEC-CONF-001` añadirá vectores de la
especificación y comparación con al menos dos implementaciones independientes.
Los round-trips contra el propio codec no bastan para demostrar wire
compatibility.

La promoción exige que typed y dynamic coincidan en los observables declarados,
que los eventos sean independientes del chunking, que el modo determinista sea
estable frente a layouts distintos y que ningún camino dependa de reflection.
La medición de allocations, memoria, throughput y latencia queda además sujeta
al contrato `STD-PERF-001`.
