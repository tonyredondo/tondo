# Tondo STD-0.1A core owner contract

Estado: contrato de owner cerrado para la implementación hosted de STD-0.1A.
La implementación inicial puede usar una unidad privilegiada del compilador o
de la VM mientras Tondo no pueda importar la distribución estándar; la unidad
debe conservar exactamente las firmas y los observables de este documento.

## Reglas comunes

- Todas las operaciones son dispatch estático. No hay `Any`, vtables ni
  lookup por nombre.
- Los valores que no declaran `async` no suspenden. Los errores se devuelven
  como `Result` con la sintaxis `T ! E`.
- Los límites de memoria, longitud y pasos son argumentos de options o defaults
  finitos del owner; alcanzar un límite devuelve un error nominal y no publica
  un valor parcial.
- `String` siempre contiene UTF-8 válido. `Bytes` es el valor binario común y
  no se sustituye por otra representación local.

## `std.core` (intrínsecos)

No existe un módulo importable `std.core`; estas operaciones pertenecen al
prelude y a los tipos intrínsecos.

```tondo
pub fn Option.some[T](value: T): T?
pub fn Option.none[T](): T?
pub fn Option.unwrapOr[T](self: T?, fallback: T): T
pub fn Option.map[T, U](self: T?, fn(T): U): U?

pub fn Result.ok[T, E](value: T): T ! E
pub fn Result.err[T, E](error: E): T ! E
pub fn Result.map[T, U, E](self: T ! E, fn(T): U): U ! E
pub fn Result.mapErr[T, E, F](self: T ! E, fn(E): F): T ! F
pub fn Result.unwrapOr[T, E](self: T ! E, fallback: T): T
```

`Display` es el único protocolo de representación textual. `Equatable` y
`Key` son capacidades estáticas cerradas (no traits implementables) y son los
únicos protocolos de igualdad/hash que pueden usar colecciones; un tipo no
obtiene ninguno por tener una representación coincidente.

```tondo
trait Display { fn display(self): String }
// Equatable y Key son capacidades intrínsecas cerradas; no tienen una
// declaración de trait pública ni dispatch dinámico.
```

`Option` y `Result` son valores; mapearlos no captura pánicos ni reordena
errores. `unwrapOr` es total. La construcción de un error no toca el host.

## `std.text`

`String` es UTF-8 inmutable; `Char` es un scalar Unicode y `Byte` un octeto.
Las operaciones de índice trabajan en scalars y nunca en offsets físicos.

```tondo
pub fn String.empty(): String
pub fn String.fromChars(chars: Array[Char]): String ! TextError
pub fn String.length(self): Int                 // scalars
pub fn String.byteLength(self): Int             // UTF-8 bytes
pub fn String.get(self, index: Int): Char?
pub fn String.slice(self, start: Int, end: Int): String ! TextError
pub fn String.contains(self, needle: String): Bool
pub fn String.startsWith(self, prefix: String): Bool
pub fn String.endsWith(self, suffix: String): Bool
pub fn String.find(self, needle: String): Int?
pub fn String.replace(self, old: String, new: String): String
pub fn String.trim(self): String
pub fn String.toLowerAscii(self): String
pub fn String.toUpperAscii(self): String
pub fn String.chars(self): Iterator[Char]

pub enum TextError { InvalidIndex, InvalidBoundary, ResourceLimit }
```

`trim` y las conversiones ASCII no aplican normalización Unicode ni locale.
Las búsquedas son por scalar/substring; `Char` no pretende representar un
grapheme cluster. `String(Bytes)` valida UTF-8 y `Bytes(String)` copia los
bytes UTF-8, sin métodos alternativos de conversión.

## `std.collections` y `std.iter`

Las colecciones siguen semántica de valor, COW interno permitido y mutación
solo mediante `var`/`mut` explícito. `Map` conserva el orden de inserción y
reemplazar una key conserva su primera posición; `Set` conserva el orden de
inserción observable por iteración.

```tondo
pub fn Array.new[T](): Array[T]
pub fn Array.withCapacity[T](capacity: Int): Array[T] ! CollectionError
pub fn Array.length[T](self): Int
pub fn Array.get[T](self, index: Int): T?
pub fn Array.slice[T](self, start: Int, end: Int): Array[T] ! CollectionError
pub fn Array.push[T](var self, value: T): Unit ! CollectionError
pub fn Array.pop[T](var self): T?

pub fn Map.new[K: Key, V](): Map[K, V]
pub fn Map.get[K: Key, V](self, key: K): V?
pub fn Map.insert[K: Key, V](var self, key: K, value: V): V?
pub fn Map.remove[K: Key, V](var self, key: K): V?
pub fn Map.contains[K: Key, V](self, key: K): Bool
pub fn Map.entries[K: Key, V](self): Iterator[(K, V)]

pub fn Set.new[K: Key](): Set[K]
pub fn Set.insert[K: Key](var self, value: K): Bool
pub fn Set.remove[K: Key](var self, value: K): Bool
pub fn Set.contains[K: Key](self, value: K): Bool
pub fn Set.values[K: Key](self): Iterator[K]

pub type Range
pub fn Range.from(start: Int, end: Int): Range
pub fn Range.inclusive(start: Int, end: Int): Range
pub fn Range.step(self, step: Int): Range ! CollectionError
pub trait Iterator[T] {
    fn next(var self): T?
}
pub fn Iterator.map[T, U](self, fn(T): U): Iterator[U]
pub fn Iterator.filter[T](self, fn(T): Bool): Iterator[T]
pub fn Iterator.take[T](self, count: Int): Iterator[T]
pub fn Iterator.collect[T](self): Array[T] ! CollectionError

pub enum CollectionError { InvalidCapacity, InvalidIndex, InvalidStep, ResourceLimit }
```

`Range` es lazy, no materializa un array y rechaza step cero. Cada target de
iteración produce un único elemento; consumir un iterador avanza su estado y no
se reinicia implícitamente. `Array`/`Map`/`Set` nunca exponen buffers mutables.

## `std.math`

Las funciones respetan IEEE-754, no habilitan fast-math y nunca cambian una
excepción de dominio por un valor silencioso.

```tondo
pub fn floor(value: Float): Float
pub fn ceil(value: Float): Float
pub fn round(value: Float): Float
pub fn truncate(value: Float): Float
pub fn sqrt(value: Float): Float ! MathError
pub fn fma(a: Float, b: Float, c: Float): Float
pub fn abs(value: Float): Float
pub fn min(a: Float, b: Float): Float
pub fn max(a: Float, b: Float): Float
pub enum MathError { Domain, NonFinite, ResourceLimit }
```

`sqrt` devuelve `Domain` para valores finitos negativos; NaN e infinitos siguen
las reglas IEEE declaradas. `fma` es la operación fused explícita; el compilador
no puede fusionar `a * b + c` de forma observable por su cuenta.

## `std.format`

```tondo
pub type Builder
pub fn Builder.new(): Builder
pub fn Builder.append(var self, value: String): Unit ! FormatError
pub fn Builder.finish(var self): String ! FormatError
pub fn format[T: Display](value: T): String ! FormatError
pub fn join[T: Display](values: Array[T], separator: String): String ! FormatError
pub enum FormatError { ResourceLimit, InvalidFormat }
```

El builder crece con límites comprobados y reutiliza `BytesBuilder` cuando el
target lo permite. `format` no usa reflection, no inspecciona privados y no
introduce una segunda sintaxis de interpolación.

## `std.io`

```tondo
pub enum IoError { Closed, Cancelled, InvalidData, ResourceLimit, Host }
pub enum ReadResult { Data(Bytes), Eof }
pub trait Reader {
    async fn read(var self, max: Int): ReadResult ! IoError
}
pub trait Writer {
    async fn write(var self, data: Bytes): Int ! IoError
    async fn flush(var self): Unit ! IoError
}
pub fn readAll[R: Reader](var reader: R, limits: IoLimits): Bytes ! IoError
pub type IoLimits
```

`read` puede devolver menos bytes que `max`; `0` solo significa EOF cuando el
resultado es `Eof`. `write` puede hacer partial I/O y devuelve exactamente los
bytes aceptados. `readAll` reserva de forma acotada y nunca devuelve un buffer
parcial junto a éxito. La cancelación ocurre en cada `await` y el writer no
puede retener una vista del `Bytes` después de completar la operación.

## `std.serialization`

```tondo
pub trait Serializer[E] {
    fn null(var self): ! E
    fn bool(var self, value: Bool): ! E
    fn int(var self, value: Int): ! E
    fn float(var self, value: Float): ! E
    fn string(var self, value: String): ! E
    fn bytes(var self, value: Bytes): ! E
    fn startArray(var self, length: Int?): ! E
    fn endArray(var self): ! E
    fn startMap(var self, length: Int?): ! E
    fn key(var self, value: String): ! E
    fn endMap(var self): ! E
}
pub trait Deserializer[E] {
    fn next(var self): SerializationEvent ! E
}
pub enum SerializationEvent { Null, Bool(Bool), Int(Int), Float(Float), String(String), Bytes(Bytes), StartArray(Int?), EndArray, StartMap(Int?), Key(String), EndMap }
pub trait Serialize { fn serialize[E, S: Serializer[E]](self, var serializer: S): ! E }
pub trait Deserialize { fn deserialize[E, D: Deserializer[E]](var deserializer: D): Self ! E }
pub enum SerializationError { UnexpectedEvent, TypeMismatch, LimitExceeded, Io(IoError) }
```

El evento `next` es consumible y no conserva referencias después de la llamada.
Los derives generan un único `impl` estático; los codecs concretos no construyen
un DOM para la ruta tipada. La deserialización publica el record únicamente
después de validar todos sus fields.
