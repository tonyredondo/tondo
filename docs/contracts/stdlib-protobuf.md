# Contrato de `std.protobuf`

**Estado:** contrato normativo e implementación portable conforme para
`STD-0.1A`. El owner runtime wire, reader/writer, schema checker y generator
usan la frontera `Encode[Protobuf]`/`Decode[Protobuf]`; la interoperabilidad
wire bidireccional con `prost` queda cerrada en el gate coordinado. La
integración final del generator en el driver sigue siendo un gate posterior.

`std.protobuf` es el único owner de la generación schema-first y del wire
protocol Protocol Buffers. Comparte los traits estáticos `Encode[Protobuf]` y
`Decode[Protobuf]`, pero no expone el árbol dinámico `serialization.Value`: la
inspección sin tipo utiliza `ProtoEvent` y `UnknownField` ligados al wire.
El registro machine-readable
[`testing/stdlib-protobuf.json`](../../testing/stdlib-protobuf.json) fija las
decisiones cerradas y [`scripts/stdlib-protobuf-check.sh`](../../scripts/stdlib-protobuf-check.sh)
las valida dentro de `scripts/test-gate.sh`.

La sección 14.11 de `TONDO_STANDARD_LIBRARY_SPEC.md` sigue siendo la autoridad
del catálogo. Este contrato concreta su frontera operativa sin introducir un
facade universal de codecs, reflection de valores ni una segunda configuración
de proyecto.

## API fuente y de build única

El input de build es TOML, nunca JSON ni una convención ambiental. Un proyecto
declara cada schema y su baseline de evolución con una tabla repetible:

```toml
[protobuf]
version = 1

[[protobuf.schema]]
path = "proto/user.proto"
module = "app.proto.user"
package = "acme.user"
baseline = "proto/baseline/user.proto"
descriptor = "none" # "root" conserva el descriptor estático
```

`path`, `module`, `package`, `baseline` y `descriptor` son obligatorios salvo
`baseline` para un schema nuevo. Los imports se resuelven solo entre paths
declarados; el generador rechaza duplicados, globs, paths absolutos, cambios de
line ending y cualquier lectura fuera del grafo cerrado. `tondo.lock.toml`
fija las identidades de los inputs externos, pero no añade schemas implícitos.

Estas son las únicas firmas runtime del owner y de los tipos generados:

```tondo
pub type ProtoDescriptor[T]
pub type UnknownField = {
    number: UInt32
    wireType: ProtoWireType
    tagBytes: Bytes
    payloadBytes: Bytes
}
pub type UnknownFields = Array[UnknownField]
pub enum ProtoWireType { Varint, Fixed64, LengthDelimited, StartGroup, EndGroup, Fixed32 }
pub enum ProtoEvent {
    StartMessage(String)
    EndMessage
    Field(UInt32, ProtoWireType)
    Varint(UInt64)
    Fixed32(UInt32)
    Fixed64(UInt64)
    StartLengthDelimited(UInt32)
    Bytes(Bytes)
    EndLengthDelimited
    StartPacked(UInt32)
    EndPacked
    Unknown(UnknownField)
}
pub type ProtoPath
pub type ProtoLimits = {
    maxSchemaBytes: Int
    maxImports: Int
    maxGeneratedTypes: Int
    maxGeneratedBytes: Int
    maxMessageBytes: Int
    maxDepth: Int
    maxFields: Int
    maxRepeatedItems: Int
    maxMapEntries: Int
    maxStringBytes: Int
    maxBytesFieldBytes: Int
    maxPackedBytes: Int
    maxUnknownBytes: Int
    maxVarintBytes: Int
    maxEvents: Int
    maxOutputBytes: Int
}
pub enum ProtoWireTypePolicy { PreserveUnknown, Reject }
pub enum ProtoUnknownPolicy { Preserve, Discard }
pub type ProtoDecodeOptions = {
    limits: ProtoLimits
    wireType: ProtoWireTypePolicy
    unknownFields: ProtoUnknownPolicy
    rejectNonMinimalVarints: Bool
}
pub type ProtoEncodeOptions = { limits: ProtoLimits, deterministic: Bool }
pub enum ProtoErrorKind {
    UnexpectedEof, InvalidTag, InvalidWireType, InvalidVarint, InvalidLength,
    InvalidUtf8, TypeMismatch, InvalidPacked, NumberRange, InvalidFieldNumber,
    InvalidGroup, LimitExceeded, IoError, TrailingData, SchemaMismatch
}
pub enum ProtoBuildErrorKind {
    ProtoSyntaxUnsupported, ProtoImportNotDeclared, ProtoNameCollision,
    ProtoFieldNumberConflict, ProtoReservedReuse, ProtoSchemaDrift,
    ProtoWireIncompatible, ProtoGeneratorOutputCollision, ProtoGenerationLimit
}
pub type ProtoError = { kind: ProtoErrorKind, offset: Int?, path: ProtoPath }
pub type ProtoBuildError = { kind: ProtoBuildErrorKind, schema: String, path: String }

pub fn decode[T: Decode[Protobuf]](input: Bytes, options: ProtoDecodeOptions): T ! ProtoError
pub fn encode[T: Encode[Protobuf]](value: T, options: ProtoEncodeOptions): Bytes ! ProtoError
pub fn encodeDeterministic[T: Encode[Protobuf]](value: T, limits: ProtoLimits): Bytes ! ProtoError
pub fn validate[T](input: Bytes, options: ProtoDecodeOptions): Unit ! ProtoError
pub fn descriptor[T](): ProtoDescriptor[T]

pub fn ProtoReader[T].fromBytes(input: Bytes, options: ProtoDecodeOptions): ProtoReader[T] ! ProtoError
pub fn ProtoReader[T].fromReader(var input: Reader, options: ProtoDecodeOptions): ProtoReader[T] ! ProtoError suspends
pub fn ProtoReader[T].next(var self): ProtoEvent? ! ProtoError
pub fn ProtoReader[T].own(var self, event: ProtoEvent): ProtoEvent ! ProtoError
pub fn ProtoReader[T].finish(var self): Unit ! ProtoError

pub fn ProtoWriter[T].toWriter(var output: Writer, options: ProtoEncodeOptions): ProtoWriter[T] ! ProtoError suspends
pub fn ProtoWriter[T].write(var self, event: ProtoEvent): Unit ! ProtoError suspends
pub fn ProtoWriter[T].finish(var self): Unit ! ProtoError suspends

pub fn UnknownFields.count(self): Int
pub fn UnknownFields.discard(var self): Unit
```

Un mensaje generado publica un record nominal y, cuando el schema lo declara,
su enum oneof, enum abierto y `UnknownFields`; todos implementan los traits
comunes de `std.serialization`. `descriptor[T]()` solo existe para un tipo
generado con `descriptor = "root"`; no hace lookup ni conserva metadata de otro
tipo. `ProtoReader[T]` y `ProtoWriter[T]` están ligados a `T`, no aceptan un
descriptor runtime y devuelven `none` exactamente una vez después del frame
raíz. `own` materializa los payloads temporales y cualquier error deja reader o
writer en estado terminal. La ruta a `std.io.Writer` es suspendible y la de `Bytes`
usa la misma máquina sin crear un DOM.

## Alcance y entradas de build

STD-0.1A acepta únicamente schemas con `syntax = "proto3"`. Proto2, Editions,
services, gRPC, ProtoJSON, TextFormat y extensiones de aplicación quedan fuera
de este owner; un import de uno de esos formatos produce un diagnóstico de
build estable. Un `.proto` no se interpreta como código Tondo ni se ejecuta en
runtime.

Cada schema es un input declarado del build, con bytes UTF-8, line endings
canónicos y un path lógico relativo al paquete. Los `import` solo resuelven
otros schemas declarados en el grafo cerrado del proyecto; no consultan el
directorio actual, `PATH`, variables de entorno, red ni una instalación global.
Los well-known types solo se pueden usar si sus `.proto` están declarados como
inputs; no existe un registro ambiental implícito.

El mapping de `package`, path lógico y módulo Tondo se declara explícitamente.
El nombre completo del schema, el número de field y la identidad de su tipo
son las fuentes de identidad; no se derivan del orden de declaraciones, un
hash accidental o reflection. Dos inputs no pueden producir el mismo módulo,
tipo, field identity o path generado. El generator produce siempre los mismos
bytes para el mismo conjunto ordenado de inputs, options y versión de stdlib.

El build publica código Tondo ordinario, API hash, descriptor estático y
dependencias. El descriptor solo se conserva en el binario cuando una raíz
explícita lo solicita; el decoder no hace lookup por nombre ni carga schemas
después del build.

## Modelo generado

Cada `message` produce un tipo nominal con acceso directo a sus fields,
`Encode`/`Decode` estáticos y una colección `UnknownFields`. El tipo
generado es un valor normal de Tondo: sus copias, moves, préstamos y COW siguen
las reglas del lenguaje y no dependen de un arena o finalizador oculto.

El mapping de tipos escalares es cerrado:

| Protobuf | Tondo generado | Wire |
|---|---|---|
| `int32`, `int64` | `Int32`, `Int64` | varint two's-complement |
| `uint32`, `uint64` | `UInt32`, `UInt64` | varint unsigned |
| `sint32`, `sint64` | `Int32`, `Int64` | ZigZag + varint |
| `fixed32`, `sfixed32` | `UInt32`, `Int32` | I32 little-endian |
| `fixed64`, `sfixed64` | `UInt64`, `Int64` | I64 little-endian |
| `float`, `double` | `Float32`, `Float64` | IEEE I32/I64 |
| `bool` | `Bool` | varint `0`/`1` |
| `string` | `String` | length-delimited UTF-8 |
| `bytes` | `Bytes` | length-delimited bytes |
| `enum` | open enum nominal | varint Int32 |
| `message` | tipo nominal generado | length-delimited |

La presencia es visible en el tipo generado:

- un scalar sin `optional` usa su valor por defecto y omite el valor default en
  el wire; no afirma si fue escrito explícitamente;
- un scalar, `string`, `bytes` o enum `optional` usa `T?` y distingue `None` de
  un valor default;
- un field `message` tiene presencia aunque no diga `optional` y también usa
  `Message?`;
- `repeated T` usa `Array[T]`, conserva el orden de elementos y nunca es
  `None`; y
- `map[K, V]` usa `Map[K, V]`. Las keys permitidas son las integrales, `Bool` y
  `String`; no pueden ser floats, `Bytes`, enums, messages ni otro map.

Un `oneof` genera un enum nominal con `None` y una variante por field. Elegir
una variante limpia la anterior; elegir la variante con un valor default sigue
siendo presencia y se serializa. El wire puede contener varios miembros: el
último que aparece gana, y un message de la misma oneof se fusiona siguiendo
las reglas de Protobuf antes de publicar la variante.

Los enums de proto3 son **abiertos**. El tipo generado conserva siempre el
`Int32` wire, ofrece variantes y operaciones para los números conocidos y una
proyección `known(): KnownEnum?`. Un número desconocido permanece representado
como número; nunca se transforma en sentinel ni se pierde, también dentro de
`repeated` y map values.

`UnknownFields` es una secuencia poseída de records raw. Cada record conserva
el número de field, wire type, bytes de tag y payload, incluidos grupos
deprecated, en el orden de captura. Los fields desconocidos sobreviven a un
decode y a un encode ordinario; `discardUnknown()` es la única operación que
los elimina. Re-encodear conserva los bytes de cada unknown record, pero no
promete conservar su posición relativa frente a fields conocidos.

## Wire format y decode

El wire sigue la especificación Protocol Buffers: un mensaje es una secuencia
de `(tag, value)`; `tag = (field_number << 3) | wire_type`. Los wire types 0
(`VARINT`), 1 (`I64`), 2 (`LEN`) y 5 (`I32`) son válidos para fields conocidos.
Los types 3/4 (`SGROUP`/`EGROUP`) solo se aceptan para unknown fields, con
matching de field number y límite de profundidad. Los demás wire types son
inválidos.

El decoder ordinario acepta varints válidos no mínimos hasta el límite de ancho,
packed y unpacked para cualquier repeated numérico compatible, fields conocidos
fuera de orden, unknown fields y duplicados. El encoder emite varints mínimos.
Un wire type incompatible con un field conocido se conserva como unknown por
defecto para permitir evolución; `DecodeOptions.wire_type = reject` habilita
la validación estricta. Un string siempre valida UTF-8 y `bytes` nunca aplica
transcoding.

La semántica de duplicados es la del protocolo:

- scalar, string, bytes y enum singulares: último valor gana;
- messages singulares: los occurrences se fusionan field a field;
- repeated: los valores se concatenan en el orden de aparición;
- packed repeated: varios records packed se concatenan y cada payload debe
  contener elementos completos; y
- maps: la última entrada para una key gana.

El decoder valida rangos antes de convertir a un tipo Tondo. Un `Int32` o
`UInt32` no se obtiene truncando un `Int64`, y una longitud nunca se convierte a
`Int` después de reservar memoria. El mensaje completo se decodifica desde
`Bytes` o desde un `Reader` con un frame explícito; al cerrar un frame, bytes
extra producen `TrailingData` salvo que el caller seleccione un stream de
frames.

## APIs typed y streaming

Los impls generados escriben directamente a `std.io.Writer` o a un
`BytesBuilder`; el decode typed consume el schema estático y no crea un DOM
intermedio. La metadata de schema no es una excusa para introducir reflection
en el hot path.

`ProtoReader[T]` es schema-bound y ofrece eventos incrementales para
`start-message`, `end-message`, field number/wire type, scalars,
length-delimited payloads, packed groups y unknown records. El top-level exige
un frame declarado; los submessages y packed payloads tienen límites propios.
Las vistas de string, bytes y raw unknown son válidas hasta el siguiente evento;
`own` es la única operación que las conserva.

`ProtoWriter[T]` comprueba el estado del message, la presencia de oneof, los
límites de longitud y la finalización del frame antes de publicar bytes. Reader,
writer y decoder mantienen un stack explícito acotado por `max_depth`; una
entrada profunda nunca consume el stack de llamadas del host. Cortar el input
en un byte, en una frontera de tag, dentro de un varint o dentro de un payload
produce los mismos eventos o el mismo error que un chunk grande.

## Encoding ordinario y determinista

El encoder ordinario genera bytes válidos siguiendo el field-number order de
los fields conocidos, conserva el orden de repeated y emite unknown records en
su orden de captura después de los conocidos. Los maps pueden conservar el
orden de la colección y, como exige Protobuf, ese orden no es observable como
contrato wire.

`encodeDeterministic` es una operación propia de Tondo para un schema y versión
de stdlib concretos; no es una canonicalización universal de Protobuf:

1. fields conocidos en orden ascendente por número;
2. varints y lengths en la forma mínima;
3. repeated conserva el orden de sus elementos y usa la policy `packed` del
   schema; el decode sigue aceptando ambas formas;
4. maps ordenados por key: `Bool` (`false` antes de `true`), enteros por valor
   numérico y `String` por bytes UTF-8 lexicográficos;
5. oneof solo emite la variante seleccionada, en su número de field;
6. `Float32`/`Float64` conservan exactamente sus bits IEEE, incluidos NaN y
   signed zero;
7. messages anidados aplican recursivamente las mismas reglas; y
8. unknown records se ordenan por `(field_number, wire_type, raw_record_bytes)`
   y se emiten después de los conocidos, conservando cada raw record sin
   reinterpretarlo.

Dos valores con distinta disposición interna pero el mismo schema, fields,
repeated order y unknown raw records producen los mismos bytes deterministas.
La estabilidad no se extiende a otra versión de generator, otra implementación
o un schema diferente.

## Evolución de schemas

La compatibilidad se comprueba en build time contra un baseline de schema
declarado y bloqueado por el proyecto. El baseline es un input TOML del
toolchain; no se consulta un descriptor instalado ni un servicio remoto.

El checker separa tres resultados:

- **wire-safe:** añadir fields, añadir valores de enum, añadir un `optional`,
  reservar un field eliminado y alternar packed/unpacked para un repeated
  numérico compatible;
- **wire-compatible condicionado:** cambios entre tipos con la misma familia
  de wire, `map` y su message repeated equivalente, o enum y ciertos enteros;
  requieren una aceptación explícita porque pueden cambiar rangos, presencia o
  semántica de la aplicación; y
- **wire-unsafe:** cambiar o reutilizar un field number, quitar una reserva,
  cambiar de familia wire, reutilizar un nombre reservado, cambiar una key de
  map, convertir un repeated no compatible, o dividir/mezclar oneofs.

Un resultado unsafe falla antes de generar cualquier archivo. Un resultado
condicionado solo pasa con una waiver identificada en el baseline. Las
diagnostics incluyen el schema, el tipo, el field number, la versión de origen
y destino y la razón; no imprimen el contenido completo del schema.

## Límites, errors y atomicidad

Cada perfil fija límites finitos para schema, imports, tipos generados, mensaje,
profundidad, fields, repeated items, map entries, string/bytes, packed payload,
unknown bytes, varints, eventos y output. Tags, longitudes y contadores se
validan antes de reservar, crecer o publicar. Un fallo deja el valor destino y
el writer sin cambios observables; un reader o writer que encuentra I/O,
límite o estado inválido queda terminal.

Los errors wire estables son: `UnexpectedEof`, `InvalidTag`,
`InvalidWireType`, `InvalidVarint`, `InvalidLength`, `InvalidUtf8`,
`TypeMismatch`, `InvalidPacked`, `NumberRange`, `InvalidFieldNumber`,
`InvalidGroup`, `LimitExceeded`, `IoError`, `TrailingData` y
`SchemaMismatch`. Los errors de build son: `ProtoSyntaxUnsupported`,
`ProtoImportNotDeclared`, `ProtoNameCollision`, `ProtoFieldNumberConflict`,
`ProtoReservedReuse`, `ProtoSchemaDrift`, `ProtoWireIncompatible`,
`ProtoGeneratorOutputCollision` y `ProtoGenerationLimit`.

Cada error contiene byte offset cuando existe, path estructural y contexto de
schema; no copia payloads grandes, raw unknowns ni secretos en el diagnóstico.
El path raíz es `$`, con segmentos `message`, `field-number`, `repeated-index`,
`map-key`, `map-value`, `oneof-case` y `unknown-field`.

## Implementación del owner

`crates/tondo-stdlib/src/protobuf_api.rs` contiene la implementación portable
del owner. `parse_fields` valida tags, varints, longitudes, grupos y límites con
frames explícitos; `ProtoReader[T]` y `ProtoWriter[T]` comparten la misma
máquina bounded y quedan terminales tras error o `finish`. La ruta canónica
`encode_static`/`decode_static` usa `Encode[Protobuf]`/`Decode[Protobuf]` y el
adaptador de eventos schema-bound sin construir `serialization.Value`. Los
helpers Rust `encode`/`decode` y `ProtoValue`/`Raw<Protobuf>` son detalles
internos del kernel, no la superficie Tondo normativa ni una segunda API
dinámica. Los límites de eventos se
comprueban tanto al materializar eventos del reader como al aceptar eventos del
writer, antes de crecer las colecciones.

## Corpus, interoperabilidad y promoción

El corpus del owner cubre el wire model oficial, todos los escalares, tags y
varints de frontera, UTF-8/bytes, nested, presence, repeated packed/unpacked,
maps, oneof, open enums desconocidos, duplicate/merge, unknown fields y grupos,
frames y fragmentación, límites, evolución segura/insegura y determinismo.

`STD-CODEC-CONF-001` usa el modelo oficial y compara en ambas direcciones con
`prost`: parsea bytes externos, permite que `prost` parse los bytes de Tondo,
comprueba packed repeated, truncación, fragmentación y límites, y conserva el
registro raw de un field desconocido. Un round-trip contra el propio generator
no prueba wire compatibility.

La promoción requiere equivalencia entre generated typed decode y el reader
streaming, preservación de unknowns y open enums, aceptación packed/unpacked,
estabilidad determinista, evolución comprobada y ausencia de reflection en
runtime. Allocations, memoria, throughput, latencia, code size y compile time
quedan además sujetos a `STD-PERF-001`.

Referencias primarias del wire contract: [Encoding]
(https://protobuf.dev/programming-guides/encoding/), [Language Guide
proto3](https://protobuf.dev/programming-guides/proto3/) y [Enum
Behavior](https://protobuf.dev/programming-guides/enum/).
