# Contrato de `std.json`

**Estado:** contrato de API fuente cerrado e implementación typed/dynamic/
streaming disponible para `STD-0.1A`.

`std.json` implementa el modelo JSON de RFC 8259 sobre UTF-8 y reutiliza los
traits estáticos de `std.serialization`. La política canónica y las invariantes
de este documento están reflejadas en el registro machine-readable
[`testing/stdlib-json.json`](../../testing/stdlib-json.json), que valida
`scripts/test-gate.sh` mediante
[`scripts/stdlib-json-check.sh`](../../scripts/stdlib-json-check.sh).

La sección 14.9 de `TONDO_STANDARD_LIBRARY_SPEC.md` continúa siendo la fuente
normativa de catálogo y compatibilidad. Este documento cierra las decisiones
operativas que cada implementación del owner debe respetar.

## Superficie única

El owner tiene tres formas coordinadas, no tres parsers:

1. **Typed:** `Serialize` y `Deserialize` generados o escritos de forma
   estática codifican directamente un `T` a un `Writer` y decodifican desde un
   `Reader`. La ruta de bytes materializada es una comodidad sobre el mismo
   writer/reader y no pasa por un árbol dinámico.
2. **Dynamic:** `JsonValue` representa `null`, boolean, `JsonNumber`, string,
   array y object. `JsonNumber` conserva un token decimal validado y no lo
   reduce a `Float64` al parsearlo. Un object es una secuencia ordenada de
   miembros con claves únicas después de aplicar su política de duplicados.
3. **Streaming:** `JsonReader` produce eventos incrementales y `JsonWriter`
   consume una secuencia estructural válida. Los eventos son vistas UTF-8 y de
   número válidas hasta el siguiente evento; materializar un valor es una
   operación explícita. El reader y el writer conservan un stack explícito y
   acotado por `max_depth`, no el stack de llamadas del host.

El dispatch typed es compile-time y no usa reflection, registro global,
lookup por nombre ni construcción dinámica. Un derive de `Serialize` o
`Deserialize` genera una implementación estática; el codec no inspecciona
metadata en runtime.

## Implementación cerrada del owner

La implementación portable vive en
`crates/tondo-stdlib/src/json_api.rs`. `JsonReader` tokeniza con frames
explícitos de array/object y mantiene el contador de eventos, profundidad,
miembros, strings y números antes de publicar cada evento. `JsonValue` se
construye desde ese mismo flujo; no hay un segundo parser y no se requiere un
DOM para `validate` ni para el camino typed (canonicalize usa explícitamente
el collector dinámico). `JsonNumber` conserva
el lexema decimal validado y las conversiones enteras calculan primero el
valor matemático, sin pasar por `Float64`.

`JsonWriter` valida la máquina root/array/object, orden canónico JCS,
duplicados y límites de salida antes de completar el documento. El encoder
usa una pila explícita de tareas para records, arrays y objetos y solo el
collector dinámico reserva `JsonValue`. `encode_typed` y `decode_typed`
adaptan los traits estáticos comunes a eventos JSON directamente; no usan
reflection, trait objects ni lookup de nombres. La adaptación `fromReader`
del bridge Rust lee de forma acotada para que la superficie Tondo pueda
exponerla como operación async sin cambiar la semántica del parser.

El kernel provisional anterior permanece privado al bridge durante la
migración (`kernel_parse`/`kernel_encode_*`); no es una ruta pública
alternativa y no participa en el dispatch typed.

## API fuente única

Estas son las únicas firmas públicas del owner. Las funciones de módulo son la
ruta cómoda y `JsonReader`/`JsonWriter` son la ruta incremental; no existen
aliases `parseJson`/`readJson`, overloads por defecto ni políticas ambientales.

```tondo
pub enum JsonKind { Null, Bool, Number, String, Array, Object }
pub type JsonMember = { key: String, value: JsonValue }
pub enum JsonValue {
    Null
    Bool(Bool)
    Number(JsonNumber)
    String(String)
    Array(Array[JsonValue])
    Object(Array[JsonMember])
}
pub type JsonNumber
pub type JsonPath
pub type JsonLocation

pub enum JsonEvent {
    StartArray(Int?)
    EndArray
    StartObject(Int?)
    EndObject
    Key(String)
    Null
    Bool(Bool)
    Number(JsonNumber)
    String(String)
}

pub enum JsonDuplicatePolicy { Reject, First, Last }
pub enum JsonUnknownFieldPolicy { Reject, Ignore, Capture }
pub enum JsonNumberPolicy { Exact, Float32, Float64 }
pub type JsonLimits = {
    maxDocumentBytes: Int
    maxDepth: Int
    maxArrayItems: Int
    maxObjectMembers: Int
    maxStringBytes: Int
    maxNumberBytes: Int
    maxEvents: Int
    maxOutputBytes: Int
}
pub type JsonDecodeOptions = {
    limits: JsonLimits
    duplicateKeys: JsonDuplicatePolicy
    unknownFields: JsonUnknownFieldPolicy
    numbers: JsonNumberPolicy
}
pub type JsonEncodeOptions = { limits: JsonLimits, canonical: Bool }
pub enum JsonErrorKind {
    InvalidUtf8, InvalidSyntax, UnexpectedEof, InvalidEscape,
    InvalidUnicodeScalar, InvalidNumber, DuplicateKey, UnknownField,
    MissingField, TypeMismatch, NumberRange, LimitExceeded, IoError,
    TrailingData, CanonicalizationError
}
pub type JsonError = { kind: JsonErrorKind, location: JsonLocation, path: JsonPath }

pub fn parse(input: Bytes, options: JsonDecodeOptions): JsonValue ! JsonError
pub fn decode[T: Deserialize](input: Bytes, options: JsonDecodeOptions): T ! JsonError
pub fn encode[T: Serialize](value: T, options: JsonEncodeOptions): Bytes ! JsonError
pub fn validate(input: Bytes, options: JsonDecodeOptions): Unit ! JsonError
pub fn canonicalize(input: Bytes, options: JsonDecodeOptions): Bytes ! JsonError
pub fn encodeCanonical(value: JsonValue, limits: JsonLimits): Bytes ! JsonError

pub fn JsonNumber.parse(text: String): JsonNumber ! JsonError
pub fn JsonNumber.text(self): String
pub fn JsonNumber.toInt(self): Int64 ! JsonError
pub fn JsonNumber.toUInt(self): UInt64 ! JsonError
pub fn JsonNumber.toFloat32(self): Float32 ! JsonError
pub fn JsonNumber.toFloat64(self): Float64 ! JsonError

pub fn JsonReader.fromBytes(input: Bytes, options: JsonDecodeOptions): JsonReader ! JsonError
pub async fn JsonReader.fromReader(var input: Reader, options: JsonDecodeOptions): JsonReader ! JsonError
pub async fn JsonReader.next(var self): JsonEvent? ! JsonError
pub fn JsonReader.own(var self, event: JsonEvent): JsonEvent ! JsonError
pub async fn JsonReader.finish(var self): Unit ! JsonError

pub fn JsonWriter.toWriter(var output: Writer, options: JsonEncodeOptions): JsonWriter ! JsonError
pub async fn JsonWriter.write(var self, event: JsonEvent): Unit ! JsonError
pub async fn JsonWriter.finish(var self): Unit ! JsonError
```

`parse` es la única construcción dinámica; `decode` exige un `T` estático y
publica el valor solo después de consumir un documento completo. `encode` es la
única comodidad de bytes para typed y `encodeCanonical` es la única operación
que aplica JCS a un `JsonValue`. `JsonReader.next` devuelve `none` exactamente
una vez después de la raíz; `finish` comprueba que no queda un token pendiente.
`JsonWriter` solo acepta eventos en el orden normativo y `finish` publica éxito
una sola vez. En ambos casos un error deja el objeto en estado terminal. La
ruta a un `std.io.Writer` es async; el parser sobre `Bytes` no necesita una API
paralela.

`JsonValue.Object` conserva el orden de inserción y `JsonMember.key` es UTF-8;
no se expone un `Map[String, JsonValue]` alternativo. `JsonNumber.text` copia
el lexema validado y las conversiones numéricas no pasan por `Float64`. Los
payloads de `JsonEvent` son vistas hasta el siguiente `next`; `own` es la única
materialización estable. `JsonError` siempre incluye `JsonPath` y posición sin
copiar el documento en el diagnóstico.

## Sintaxis y Unicode

El parser acepta exactamente la gramática de RFC 8259:

- solo `space`, tab, line-feed y carriage-return son whitespace;
- una operación consume un único documento y rechaza trailing data;
- comentarios, trailing commas, `NaN`, `Infinity` y literales no estándar se
  rechazan;
- el input debe ser UTF-8 válido;
- los escapes de string son los de RFC 8259; `\\uXXXX` combina pares surrogate
  y rechaza un surrogate aislado o un scalar Unicode inválido; y
- los caracteres de control deben estar escapados.

Un reader puede recibir un documento en chunks arbitrarios. Cortar entre bytes
UTF-8, escapes, dígitos o delimitadores nunca cambia el resultado: solo puede
producir un estado pendiente hasta que llegue el siguiente chunk o un
`UnexpectedEof` terminal.

## Números

`JsonNumber` almacena un token decimal validado con su signo, dígitos y
exponente, sin conversión intermedia a `Float64`. Puede conservar el spelling
de entrada para la ruta dinámica; el valor matemático se usa para conversiones
typed. Una conversión a entero exige un valor matemáticamente integral y dentro
del rango destino. Una conversión a float exige un valor finito y la política
de redondeo explícita del tipo; overflow, underflow no representable y pérdida
silenciosa producen `NumberRange`.

El signo de cero se conserva en `JsonNumber` hasta una serialización que aplique
RFC 8785. `encodeCanonical` usa exactamente la serialización JCS y puede
normalizar el spelling o `-0` como exige esa norma; nunca redondea o cambia un
valor para conseguir una salida canónica. Un número arbitrario que no pueda
entrar en el dominio I-JSON de JCS produce `CanonicalizationError`.

## Objects y políticas

La configuración por defecto es deliberadamente estricta:

- claves duplicadas producen `DuplicateKey`;
- un field desconocido durante decode typed produce `UnknownField`;
- un field desconocido solo puede ignorarse o capturarse si el caller pasa
  `DecodeOptions` explícito; `capture` requiere un campo `extras` declarado por
  el tipo; y
- un field requerido ausente produce `MissingField`. Un field `Option[T]`
  ausente se convierte en `none`; no se inventan defaults desde el codec.

Las políticas explícitas de duplicados son `reject`, `first` y `last`. `last`
reemplaza el valor pero conserva la posición del primer miembro, de modo que
la ordenación observable no dependa del layout interno. No existe una política
ambiental ni global y dos opciones incompatibles se rechazan antes de leer el
documento.

El encoder ordinario mantiene el orden declarativo de un record y el orden de
inserción de un `JsonValue.Object`. El encoder canonical aplica RFC 8785: los
nombres se ordenan según JCS, strings y números usan su representación JCS y
no se emite whitespace. Un `JsonWriter` canonical de streaming exige que el
caller entregue las claves en ese orden; si no puede hacerlo, falla antes de
publicar el miembro fuera de orden. `encodeCanonical` para un valor dinámico
puede ordenar una estructura acotada antes de escribirla.

## Reader, writer y ownership

`JsonReader` es incremental y no conserva todo el documento. Su estado contiene
solo el stack de contenedores, el token pendiente, límites y el scratch acotado
para un string o número. Una llamada a `next` invalida las vistas del evento
anterior; `JsonEvent.own` crea un valor estable cuando el caller lo necesita.

`JsonWriter` valida la máquina de estados (root, array, object/key/value),
escapa strings estrictamente y escribe directamente al `std.io.Writer`. El
writer no hace buffering ilimitado para completar un documento y, tras un error
de I/O, límite o estado, queda terminal y no anuncia éxito posterior. La ruta
typed puede escribir un campo y continuar sin construir `JsonValue` ni un DOM.

El parser de valores y el decoder typed usan la misma máquina léxica y el mismo
oráculo de errores. El decoder typed no materializa un DOM intermedio; un
collector dinámico es el único que reserva el árbol. Todas las reservas
comprueban el límite prospectivo antes de crecer y un fallo no publica un
resultado parcial.

## Límites y errores

Cada perfil de ejecución proporciona límites finitos para documento de entrada,
profundidad, miembros de object, elementos de array, bytes de string, bytes de
número, eventos y bytes de salida. El límite se valida antes de leer o reservar
la siguiente unidad. El parser usa un stack explícito, por lo que una entrada
profunda falla con `LimitExceeded` y no con overflow del stack del host.

Los errores estables son `InvalidUtf8`, `InvalidSyntax`, `UnexpectedEof`,
`InvalidEscape`, `InvalidUnicodeScalar`, `InvalidNumber`, `DuplicateKey`,
`UnknownField`, `MissingField`, `TypeMismatch`, `NumberRange`,
`LimitExceeded`, `IoError`, `TrailingData` y `CanonicalizationError`. Cada error
incluye clase, offset de byte, línea/columna calculables y un `JsonPath`
estructural formado por segmentos de clave o índice. El texto de diagnóstico
no copia automáticamente el input ni secretos potenciales.

## Corpus y promoción

El owner ya está implementado contra este contrato y sus identidades
reproducibles viven en el corpus y en las pruebas de `tondo-stdlib`. El gate
`STD-CODEC-CONF-001` añadirá vectores oficiales y comparación con al menos dos
implementaciones independientes cuando estén disponibles; ese gate no puede
sustituirse por round-trips internos.

La aceptación de `STD-JSON-001` exige que el contrato machine-readable pase,
que cada clase de corpus tenga un owner y que la implementación demuestre
equivalencia typed/dynamic en los observables declarados. La salida canónica,
el orden, el path de error, los límites y la ausencia de DOM son parte del
contrato; el rendimiento se mide además bajo `STD-PERF-001`.
