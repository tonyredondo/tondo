# Tondo STD-0.1A core owner contract

Estado: contrato de owner cerrado para la implementación hosted de STD-0.1A.
La implementación inicial puede usar una unidad privilegiada del compilador o
de la VM mientras Tondo no pueda importar la distribución estándar; la unidad
debe conservar exactamente las firmas y los observables de este documento.

## Reglas comunes

- Todas las operaciones son dispatch estático. No hay `Any`, vtables ni
  lookup por nombre.
- Las llamadas a operaciones `suspends` esperan automáticamente en la forma
  ordinaria. Los contratos sin cuerpo escriben el efecto; los cuerpos pueden
  declararlo o inferirlo transitivamente. Siempre se publica como `suspends` en
  la interfaz/ABI; los errores se devuelven como `Result` con `T ! E`.
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

## `std.async`

La fuente normativa completa de este owner es el
[`contrato dedicado de std.async`](./stdlib-async.md); esta sección conserva
solo el resumen para el catálogo Core.

El owner usa el único efecto de suspensión del lenguaje: no publica wrappers
`Task`/`Future` ni duplica operaciones con sufijos async. Los contratos
sin cuerpo escriben `suspends` después del outcome y la interfaz canónica siempre
lo conserva. `Join` solo nace de
`spawn` y se consume mediante `await handle`; la cancelación y el detach son
operaciones terminales estructuradas. `await` delante de una llamada directa es
opcional; para convertir un `Join` o `Waiter` pendiente en su resultado sigue
siendo obligatorio. `Waiter.wait()` es una llamada suspendible directa y espera
implícitamente. La inferencia de cuerpos es transitiva y nunca depende del
nombre de una función; `@sync`/`@nosuspend` rechaza cualquier camino que
suspenda.

```tondo
pub type Join[T, E]
pub type Waiter[T, E]
pub type Completer[T, E]
pub type AlreadyCompleted

pub fn oneshot[T, E](): (Waiter[T, E], Completer[T, E])
pub fn Waiter.wait(var self): T ! E suspends
pub fn Completer.complete(var self, value: T): Unit ! AlreadyCompleted
pub fn Completer.fail(var self, error: E): Unit ! AlreadyCompleted
pub fn Completer.cancel(var self): Unit ! AlreadyCompleted

trait AsyncIterator[T] { fn next(mut self): T? suspends }
pub fn AsyncIterator.collect[T](var self, limit: Int): Array[T] ! CollectionError suspends
```

La finalización de `Completer` es atómica y exactamente una operación gana; las
posteriores devuelven `AlreadyCompleted`. `AsyncIterator` mantiene backpressure,
cierra al salir de `for` (o `for await`) y no materializa un array
implícitamente. `collect(limit:)` es la única materialización y cierra el
cursor en éxito, error, cancelación o unwind. `Channel` no forma parte de
STD-0.1A; su adaptación queda en STD-0.1B.

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
// String itself is the canonical zero-allocation Iterator[Char] witness;
// chars() returns the same immutable String value for use in `for`/Iterator
// contexts rather than allocating a second cursor wrapper.
pub fn String.chars(self): String

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
pub trait Iterator[T] {
    fn next(var self): T?
}
pub fn Iterator.map[T, U](self, fn(T): U): Iterator[U]
pub fn Iterator.filter[T](self, fn(T): Bool): Iterator[T]
pub fn Iterator.take[T](self, count: Int): Iterator[T]
pub fn Iterator.collect[T](self): Array[T] ! CollectionError

pub enum CollectionError { InvalidCapacity, InvalidIndex, InvalidStep, ResourceLimit }
```

Los cuatro combinadores conservan un único protocolo `Iterator[T]` y son
lazy: `map` y `filter` guardan el callback y solo consumen la fuente al pedir
el siguiente elemento; `take` limita ese consumo sin crear una colección
intermedia. Los adaptadores son cursores own afines, se pueden encadenar y su
estado de posición es observable únicamente por consumo. `collect` consume el
cursor una sola vez y materializa el `Array[T]` al final, devolviendo
`CollectionError` si el límite de objetos del runtime impide terminar la
colección. Un `take` con un conteo negativo se comporta como `take(0)` y
produce una colección vacía. Los callbacks son síncronos; una suspensión
inferida no forma parte de este contrato.


`Range` se construye únicamente con los operadores de lenguaje `start .. end`
(final exclusivo) y `start ..= end` (final inclusivo). Es lazy y no materializa
un array; cada target de iteración produce un único elemento. No existe una
segunda familia de constructores nominales (`Range.from`/`Range.inclusive`) ni
un método `Range.step`: esos nombres duplicarían la sintaxis canónica y
restringirían innecesariamente el rango a `Int`, mientras que los operadores
admiten todos los tipos discretos soportados por el lenguaje (`Int`, enteros
sin signo y `Char`). Los pasos son una propiedad de los slices, no de un
`Range`; un range descendente permanece vacío según el contrato del lenguaje.
Consumir un iterador avanza su estado y no se reinicia implícitamente.
`Array`/`Map`/`Set` nunca exponen buffers mutables.
La superficie de `std.collections` está conectada al backend bootstrap por una
única ruta estática HIR → MIR → bytecode → VM: los constructores estáticos usan
los intrinsics del lenguaje, las consultas y mutaciones operan sobre los mismos
objetos COW del runtime y `Map.entries`/`Set.values` devuelven cursores propios
lazy. El conformance hosted está cubierto por
`tests/runtime/m11-std-collections-001.to` y sus sidecars.

## Evidencia del owner intrínseco

`STD-A-CORE-EVIDENCE-001` mantiene la frontera sin host de `std.core` y enlaza
las nueve firmas públicas de `Option` y `Result` con sus símbolos HIR, lowering
MIR, agregados bytecode y ejecución VM. Las pruebas cubren instanciación
genérica explícita e inferida, composición `map`/`mapErr`/`unwrapOr`, patrones
de éxito y error, propagación y semántica de valores. El corpus de admission
fuzz genera formas `Option`/`Result` y protocolos genéricos; `STD-A-FUZZ-001`
promueve la ruta owner-aware y los baselines de rendimiento por owner
permanecen pendientes de promoción. `HOST` es `not-applicable`: el owner es intrínseco y
compiler/VM-owned, sin capability ni consulta ambiental.

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
target lo permite. Cada append comprueba el límite antes de mutar el estado;
un error nunca expone una salida parcial. `format` y `join` usan `Display`
estático, no reflection, no inspeccionan privados y no introducen una segunda
sintaxis de interpolación. El caso vacío, los límites exactos, los separadores,
los errores de `Display` y los receivers inválidos forman parte del corpus
portable. Las dimensiones de coste son bytes materializados, allocations del
builder y work-units; sus baselines por owner se capturan por separado, sin
convertir el número de allocations en una garantía semántica.

## `std.io`

```tondo
pub enum IoError { Closed, Cancelled, InvalidData, ResourceLimit, Host }
pub enum ReadResult { Data(Bytes), Eof }
pub trait Reader {
    fn read(var self, max: Int): ReadResult ! IoError suspends
}
pub trait Writer {
    fn write(var self, data: Bytes): Int ! IoError suspends
    fn flush(var self): Unit ! IoError suspends
}
pub fn defaultLimits(): IoLimits
pub fn limits(maxBytes: Int, maxRead: Int): IoLimits ! IoError
pub fn readAll[R: Reader](var reader: R, limits: IoLimits): Bytes ! IoError suspends
pub fn writeAll(var writer: Writer, data: Bytes): Unit ! IoError suspends
pub type IoLimits
```

`read` puede devolver menos bytes que `max`; `0` solo significa EOF cuando el
resultado es `Eof`. `write` puede hacer partial I/O y devuelve exactamente los
bytes aceptados. `defaultLimits` ofrece una política segura y `limits` rechaza
cotas no positivas. `readAll` comprueba el límite agregado antes de consumir un
handle hosted y nunca devuelve un buffer parcial junto a éxito. `writeAll`
acepta short writes, exige progreso y hace `flush` al completar. La cancelación
se propaga como `IoError.Cancelled` en cada punto de espera del backend y el
writer no puede retener una vista del `Bytes` después de completar la operación.

`STD-A-IO-EVIDENCE-001` cierra las cuatro firmas públicas del owner portable
`std.io` mediante el contrato compartido [`testing/stdlib-core.json`](../../testing/stdlib-core.json).
La evidencia enlaza Reader/Writer, `IoLimits`, HIR/lowering, bytecode, VM y el
fixture `m11-std-io-001.to`; el kernel prueba particiones deterministas de
chunks, short reads/writes, EOF, límites exactos, progreso cero, sobreescrituras,
errores después de datos aceptados, `flush` y cancelación sin publicar éxito
parcial. `HOST` es `not-applicable`: console, filesystem y process poseen los
adaptadores capability-gated y solo reutilizan estos protocolos. Las dimensiones
de coste declaradas son bytes copiados, chunks procesados y work-units; sus
baselines por owner y promoción global de conformance siguen visibles como
trabajo posterior; `STD-A-FUZZ-001` promueve el fuzz owner-aware.

## `std.serialization`

El contrato completo y único está en
[`docs/contracts/stdlib-serialization.md`](./stdlib-serialization.md). Incluye
los protocolos `Encoder[C, E]`/`Decoder[C, E]`, eventos para scalars,
arrays, maps con claves arbitrarias, records y enums, además de `own`, límites,
atomicidad y reglas de `derive`. Los codecs concretos no construyen un DOM en
la ruta typed y la deserialización publica un valor solo después de validar
todos sus componentes.

## Evidencia del owner intrínseco `std.text`

`STD-A-TEXT-EVIDENCE-001` cierra la evidencia ejecutable de las quince firmas
de `String` descritas arriba. El contrato de grupo
[`testing/stdlib-core.json`](../../testing/stdlib-core.json) conserva la
superficie común; `hir/check.rs` y `hir/lower.rs` comprueban y especializan el
dispatch, mientras `process_host.rs`/la VM materializan los valores UTF-8
válidos, límites de scalar y errores de frontera. Los fixtures
`m11-std-text-001.to` y `m11-std-text-002.to` cubren Unicode, índices y slicing
por scalar, iteración sin cursor adicional, transforms ASCII y rechazo
atómico de UTF-8 inválido. `HOST` es `not-applicable`: no hay capability ni
lectura ambiental separada. El corpus bounded de bytes/UTF-8 y el admission
fuzz aportan cobertura de frontera; `STD-A-FUZZ-001` promueve el fuzz
owner-aware y los baselines de coste por owner y la promoción global de
conformance siguen visibles como trabajo posterior.

## Evidencia del owner intrínseco `std.collections`

`STD-A-COLL-EVIDENCE-001` cierra la evidencia ejecutable de las dieciocho
firmas públicas de `Array`, `Map` y `Set`. El contrato de grupo mantiene la
semántica de valor y permite COW interno: el lowering HIR/MIR, el bytecode y la
VM comparten las mismas operaciones y preservan la independencia observable
entre copias. El fixture `m11-std-collections-001.to` cubre capacidades y
errores atómicos, índices, slicing, inserción/reemplazo/eliminación, orden de
inserción, pertenencia e iteración lazy de mapas y sets.

Las claves se comprueban mediante el protocolo `Key`; el runtime conserva el
orden normativo sin usar el layout interno como observable. Las properties de
lowering comparan las rutas eager y COW sobre el mismo corpus y prueban el
detach antes de escribir; el admission fuzz genera las formas intrínsecas de
Array/Map/Set y sus límites. `HOST` es `not-applicable`: las colecciones son un
owner intrínseco portable sin capability ni consulta ambiental. `STD-A-FUZZ-001`
promueve el fuzz owner-aware; los baselines de memoria/hash por owner y la
promoción global de conformance permanecen explícitamente pendientes.

## Evidencia del owner intrínseco `std.iter`

`STD-A-ITER-EVIDENCE-001` cierra la evidencia ejecutable de las cuatro firmas
públicas de `Iterator`: `map`, `filter`, `take` y `collect`. El protocolo
`Iterator[T]` mantiene un cursor own con `next(var self): T?`; el chequeo HIR,
el lowering y la terminación bytecode preservan ese protocolo, mientras la VM
traza la fuente y los callbacks de cada adaptador.

El fixture `m11-std-iter-001.to` cubre composición `map → filter → take`,
callbacks nombrados y closures síncronas, las rutas calificadas y genéricas
estáticas, el consumo acotado de `take(-1)` y la materialización final de
`collect`. Los adaptadores son lazy y de consumo único: crear la cadena no
consume la fuente, cada `next` avanza exactamente el cursor y `collect` publica
un `Array` solo después de terminar sin superar el límite de objetos. Los
callbacks no suspenden implícitamente y un error de materialización no expone
un array parcial.

Las properties de lowering y los tests de runtime cubren iteración intrínseca
prestada, dispatch estático de iteradores de usuario, guards de agotamiento,
trazado de fuente/callbacks y rechazo de descriptores o estados corruptos. El
admission fuzz aporta formas de cursor; `HOST` es `not-applicable` porque el
owner es intrínseco portable sin capability ni consulta ambiental.
`STD-A-FUZZ-001` promueve el fuzz owner-aware; los baselines de
retención/allocations/materialización y la promoción global de conformance
permanecen explícitamente pendientes.

## Evidencia del owner intrínseco `std.math`

`STD-A-MATH-EVIDENCE-001` cierra la evidencia ejecutable de las nueve firmas
escalares de `std.math`. El modelo numérico conserva IEEE-754: los kernels
`floor`, `ceil`, `round`, `truncate`, `abs`, `min`, `max` y `fma` mantienen
infinidades, NaN y cero con signo, mientras `sqrt` distingue `Domain` de
`NonFinite` sin publicar un valor parcial. El lowering HIR y el puente
`process_host` mantienen un dispatch estático por operación y materializan
`MathError` con una frontera nominal.

Las pruebas del owner combinan la matriz de límites del kernel, el fixture
`m11-std-math-001.to`, el corpus IEEE de `m6-num-004-ieee.to`, las properties
de redondeo de `Float32`, los diagnósticos de NaN/overflow de constantes y el
runtime test de la frontera de `sqrt`. El kernel scalar es el scalar oracle
canónico de 0.1: no existe una ruta SIMD alternativa ni fast-math observable;
si un backend futuro vectoriza, debe demostrar equivalencia bit a bit con este
oracle antes de cambiar la ruta. `HOST` es `not-applicable` porque el owner es
intrínseco portable sin capability ni consulta ambiental. `STD-A-FUZZ-001`
promueve el fuzz owner-aware; baselines de coste por owner y la promoción
global de conformance siguen visibles como trabajo posterior.
