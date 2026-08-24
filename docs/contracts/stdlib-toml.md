# Contrato de `std.toml`

**Estado:** contrato `contract-locked` para STD-0.1B, cerrado por
`STD-TOML-001`. La implementación de VM/host y el backend nativo permanecen
pendientes de sus leaves posteriores a `NATIVE-001`.

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

Estas declaraciones son la única superficie pública de `std.toml` en 0.1.

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
underscores numéricos; `TomlValueView` es un view inmutable prestado para una
operación y deja de ser válido al avanzar el reader o retornar de la operación
que lo creó.

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
reader/writer quedan terminales y toda operación posterior devuelve `Closed`.

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
son `Send` y deben terminar con `finish`. Un input no se retiene después de
`fromBytes`; el streaming mantiene solo frames, la tabla de paths acotada y el
token actual.

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
profundidad, número de tablas/filas y coste de rechazo adversarial. No se
publican claims de rendimiento antes de `STD-TOML-PERF-001`.

## Separación del toolchain

`std.toml` es un codec de datos general. El toolchain usa `tondo.toml`,
`tondo.test.toml` y `tondo.lock.toml` como inputs con schemas privados y
fronteras de capabilities propias. Esos archivos se validan por los módulos de
proyecto/CLI, se normalizan a registros internos cerrados y no se importan
desde `.to`. `std.toml` no puede cambiar el package graph, seleccionar
capabilities, resolver dependencias, leer la red ni ejecutar el compiler por
el hecho de parsear un documento con esas keys.

## Exclusiones deliberadas y leaves posteriores

Este contrato no incluye TOML 1.0 como dialecto alternativo, documentos
múltiples, includes, macros, `environment-interpolation`, locale, TZ, comments
preservados, edición round-trip, schema discovery, valores binarios implícitos,
segundos intercalares, offsets fuera de `std.time`, fracciones de más de nueve
dígitos, futures duplicadas ni `selectable`.

La implementación, host, corpus de tests/fuzzing, rendimiento, conformance y
documentación de uso permanecen pendientes de:

```text
STD-TOML-IMPL-001
STD-TOML-TEST-001
STD-TOML-PERF-001
STD-TOML-CONF-001
STD-TOML-DOC-001
```
