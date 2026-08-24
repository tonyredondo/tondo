# Contrato de `std.uuid`

Estado: **contract-locked** para `STD-0.1B` / `STD-ID-001`.

Este documento fija la frontera normativa de `std.uuid`. No afirma que el
runtime, los proveedores de entropía/reloj, los vectores de conformance, los
benchmarks, ni los ejemplos de uso estén implementados. Esas piezas permanecen
en las leaves `STD-UUID-IMPL-001`, `STD-UUID-HOST-001`, `STD-UUID-TEST-001`,
`STD-UUID-PERF-001`, `STD-UUID-CONF-001` y `STD-UUID-DOC-001`.

El contrato sigue [RFC 9562](https://www.rfc-editor.org/rfc/rfc9562.html), que
define el UUID de 128 bits y sustituye RFC 4122. Tondo adopta una superficie
pequeña y explícita: el núcleo representa, valida, formatea y compara valores;
solo los generadores que necesitan host declaran sus capabilities. Un UUID no
es un secreto, una prueba de autenticación ni una garantía matemática de
unicidad.

## 1. Alcance y objetivos

`std.uuid` proporciona:

1. un valor inmutable de 128 bits que puede copiarse, compararse y usarse como
   clave de `Map`/`Set`;
2. conversión estricta entre bytes de red y texto UUID;
3. introspección de variante y nibble de versión, incluso para UUID externos;
4. los valores sentinel `nil` y `max` definidos por RFC 9562;
5. generación v4, v5 y v7 con las políticas de capabilities descritas aquí;
6. errores nominales, límites finitos y publicación atómica; y
7. providers sellados y deterministas para tests sin cambiar las firmas de
   producción.

No existe un registro global de UUIDs ni un retry oculto para “evitar” una
colisión. La unicidad de v4/v7 es probabilística según la calidad y el tamaño
de la entropía suministrada; v5 es determinista dentro de un namespace y un
nombre. Ninguna de esas propiedades sustituye una constraint de base de datos.

## 2. Versiones y variante

### 2.1 Representación común

Todos los valores `Uuid` contienen exactamente 16 bytes. El orden público es
big-endian (network byte order), independiente de la arquitectura del host.
Los 128 bits posibles son representables: `parse` no rechaza un UUID externo
solo porque tenga una versión futura o una variante distinta de RFC 9562.

La variante se obtiene de los bits definidos por RFC 9562:

```tondo
pub enum UuidVariant {
    Rfc9562
    Ncs
    Microsoft
    Future
}
```

`Uuid.version()` devuelve el nibble de versión como `Int` en `0..15`. Para una
variante que no sea `Rfc9562` ese nibble es informativo; no se infiere una
semántica RFC que el layout no garantice.

Los generadores de Tondo siempre producen la variante RFC 9562 (`10` en los
bits de variante). Los valores `nil` y `max` son excepciones intencionadas del
layout normal: todos sus bits son cero o uno respectivamente.

### 2.2 Versiones generables en 0.1

Solo se generan tres versiones:

- **v4:** 122 bits aleatorios procedentes de la capability `entropy`; los bits
  de versión y variante se fijan después de leer el buffer.
- **v5:** SHA-1 sobre `namespace.bytes || name`, con el nombre tratado como
  bytes opacos. Es una operación pura y determinista; no normaliza UTF-8 ni
  concede propiedades de secreto a SHA-1.
- **v7:** los 48 bits más significativos contienen milisegundos desde Unix
  Epoch UTC (sin leap seconds) y los 74 bits restantes son entropía del
  provider. Requiere `civil-clock` y `entropy`.

No se generan v1, v2, v3, v6 ni v8 en 0.1. v1/v6 exponen identidad de nodo y
estado de reloj que Tondo no quiere ocultar; v2 pertenece al perfil DCE; v3
usa MD5; y v8 no define una garantía portable de unicidad. Esas exclusiones no
impiden parsear, conservar, comparar o serializar valores externos de esas
versiones.

v7 usa entropía nueva en cada llamada. No mantiene contador, lock, cache ni
estado global para fabricar una monotonicidad que el reloj no garantiza. Por
tanto, dos valores v7 del mismo milisegundo pueden tener cualquier orden en
los 74 bits bajos; el orden lexicográfico solo ofrece agrupación temporal por
el prefijo del timestamp. Una futura política monotónica sería otro contrato.

## 3. Texto, bytes y canonicalidad

### 3.1 Texto

`Uuid.parse` acepta exactamente:

- la forma dashed de 36 caracteres `8-4-4-4-12`, con dígitos hexadecimales en
  mayúsculas o minúsculas; o
- la misma forma precedida por `urn:uuid:` sin distinguir mayúsculas en el
  prefijo.

No se aceptan whitespace, braces, forma compacta de 32 hexadecimales, guiones
en posiciones distintas, escapes ni otros schemes URI. `Uuid.toString()`
siempre devuelve 36 caracteres dashed, hexadecimales en minúscula y sin URN.
Aceptar mayúsculas al leer y producir una única forma al escribir evita
rechazos de interoperabilidad sin crear varias salidas canónicas.

El parser no hace NFC/NFD/NFKC/NFKD, locale lookup ni case folding sobre el
nombre de un UUID v5. El texto UUID solo contiene ASCII y todos los errores
incluyen la posición byte del primer carácter inválido cuando aplica.

### 3.2 Bytes

`Uuid.fromBytes` exige exactamente 16 bytes y copia el input. `Uuid.toBytes`
devuelve una copia nueva de esos 16 bytes. Nunca se interpreta el layout COM
GUID little-endian ni el endianness nativo del target.

La igualdad y el hash comparan los 16 bytes en orden. `Uuid.compare` devuelve
`-1`, `0` o `1` según la comparación lexicográfica unsigned de esos bytes; no
interpreta timestamp, versión o significado de aplicación.

## 4. API pública

Las declaraciones siguientes son la única superficie pública de este contrato:

~~~tondo
pub type Uuid
pub enum UuidVariant { Rfc9562, Ncs, Microsoft, Future }
pub type UuidErrorKind
pub type UuidError

pub fn Uuid.nil(): Uuid
pub fn Uuid.max(): Uuid
pub fn Uuid.parse(text: String): Uuid ! UuidError
pub fn Uuid.fromBytes(bytes: Bytes): Uuid ! UuidError
pub fn Uuid.toBytes(self): Bytes
pub fn Uuid.toString(self): String
pub fn Uuid.version(self): Int
pub fn Uuid.variant(self): UuidVariant
pub fn Uuid.isNil(self): Bool
pub fn Uuid.isMax(self): Bool
pub fn Uuid.compare(self, other: Uuid): Int

pub fn Uuid.v4(): Uuid ! UuidError
pub fn Uuid.v5(namespace: Uuid, name: Bytes): Uuid ! UuidError
pub fn Uuid.v7(): Uuid ! UuidError
~~~

`Uuid.v4` está disponible únicamente en un source set que declara
`entropy`. `Uuid.v7` declara `civil-clock + entropy`; la lectura del reloj es
la fecha UTC del provider civil, no el `Instant` monotónico de `std.time`.
`Uuid.v5` es pura, aunque devuelve `UuidError.NameLimitExceeded` si el nombre
supera el límite del target. Las tres operaciones son síncronas y no son
`selectable`; no existen `v4Async`, `v7Async`, `UuidFuture` ni un API paralelo.

`Uuid` es `Copy`, `Discard`, `Eq`, `Ord`, `Hash`, `Send` y `Share`. No contiene
referencias, handles host ni aliases mutables. Los métodos de lectura son
allocation-free salvo `toBytes`/`toString`, que deben materializar sus copias
de resultado.

## 5. Generación y capabilities

### 5.1 Entropía

La capability `entropy` es un provider del target que entrega bytes de calidad
criptográfica y devuelve un fallo nominal si no puede hacerlo. El core no usa
clock jitter, addresses, process IDs, counters globales, hashes de memoria ni
fallback pseudorandom. v4 consume un buffer acotado de 16 bytes; v7 consume un
buffer acotado de 10 bytes y descarta únicamente los bits que ocupan versión y
variante. No se reintenta silenciosamente.

La ausencia de `entropy` es un error estático `E1008`; un fallo del provider en
runtime es `UuidError.EntropyUnavailable` o `UuidError.EntropyFailure`.

### 5.2 Reloj civil

La capability `civil-clock` proporciona una lectura UTC con resolución declarada
por el target. v7 convierte esa lectura a milisegundos Unix comprobados. Un
reloj anterior a Unix Epoch, un timestamp negativo o uno que no cabe en 48 bits
produce `TimestampOutOfRange`; no se satura, envuelve ni sustituye la hora por
entropía.

La ausencia de `civil-clock` es estática. El provider no consulta `TZ`, locale,
environment, filesystem, red ni timezone data para construir el timestamp.
`std.testing` puede instalar un provider civil sellado y determinista en su
envelope de test; eso no concede la capability a código de producción.

### 5.3 Name-based v5

`Uuid.v5` concatena los 16 bytes big-endian del namespace con `name`, calcula
SHA-1, conserva sus 128 bits más significativos y vuelve a fijar versión 5 y
variante RFC 9562. El caller decide la codificación canónica del nombre; Tondo
no transforma un `String` implícitamente. Los namespace IDs estándar del RFC
pueden pasarse como valores `Uuid` normales, sin una segunda API de constantes
ambientales.

SHA-1 aquí solo aporta compatibilidad determinista con UUIDv5. `v5` no debe
usarse como password, token, firma, clave ni mecanismo de autenticación; el
contrato no promete resistencia a colisiones criptográficas.

## 6. Errores, límites y atomicidad

`UuidError` es nominal y no expone errno, paths, direcciones, locale, timestamps
del host ni mensajes dependientes del entorno. Sus kinds son:

```text
InvalidTextLength
InvalidCharacter
InvalidSeparator
InvalidUrnPrefix
InvalidBytesLength
NameLimitExceeded
TimestampOutOfRange
EntropyUnavailable
EntropyFailure
ClockUnavailable
ClockFailure
ProviderMisconfigured
ResourceLimit
OutOfMemory
```

Los límites mínimos son:

| Identidad | Unidad | Default | Comprobación |
|---|---:|---:|---|
| `max_text_bytes` | bytes | 45 | antes de parsear |
| `uuid_bytes` | bytes | 16 | antes de copiar |
| `max_name_bytes` | bytes | 16 MiB | antes de SHA-1 |
| `max_entropy_bytes` | bytes por llamada | 16 | antes de invocar provider |
| `vm_heap` | target-defined | target | antes de publicar una copia |

El texto URN máximo es `9 + 36 = 45` bytes. Un input que excede un límite se
rechaza antes de reservar o llamar al provider. No se publica un UUID parcial,
no se devuelve una entropía truncada como éxito y no se conserva estado después
de un fallo de generación.

## 7. Seguridad, ownership y rendimiento

`Uuid` no revela la capability ni el provider que lo creó. Un v7 revela el
milisegundo UTC codificado por diseño; quien necesite ocultar tiempo debe usar
v4 o una representación de aplicación distinta. Ningún UUID se presenta como
secreto.

El camino scalar es el oracle normativo: parsea como una máquina de estados de
longitud fija, compara 16 bytes y formatea con una tabla hex estable. No usa la
pila recursiva ni una tabla global mutable. Fast paths SIMD/word-wide solo se
pueden promover después de demostrar equivalencia byte a byte, errores,
ownership y límites; la implementación debe ser correcta aunque no exista
SIMD. v5 puede usar un SHA-1 especializado para un prefijo fijo de 16 bytes,
pero debe conservar exactamente los vectores RFC.

La generación no mantiene locks ni contadores globales. Los providers son
fronteras host únicas; los fallos se normalizan a `UuidError` y el cleanup de un
provider no puede cambiar un UUID ya publicado.

## 8. Matriz de evidencia y exclusiones

El owner debe demostrar, como mínimo:

| Grupo | Observables obligatorios |
|---|---|
| `representation` | 128 bits, copy/share, nil/max, equality/hash |
| `parse-format` | dashed, URN, case, lowercase canonical, invalid text |
| `bytes-boundary` | network order, exact 16 bytes, copies, host endianness |
| `version-variant` | all nibbles, RFC/NCS/Microsoft/Future, generated bits |
| `v4-entropy` | 122 bits, provider quality/failure, no fallback |
| `v5-vectors` | RFC vectors, namespace/name bytes, determinism, size limit |
| `v7-clock-entropy` | epoch milliseconds, 74 bits, range, no strict monotonic claim |
| `capability-and-provider` | E1008, sealed deterministic providers, normalized errors |
| `ordering-and-limits` | unsigned byte order, map/set keys, fixed budgets, atomic failure |
| `vm-native-parity` | bytes, text, versions, errors, v4/v5/v7 provider observations |

Los corpus mínimos son RFC 9562 vectors, canonical text, malformed text,
name-based v5, v7 timestamp boundaries, provider failures, ordering/hash y
VM/native parity. La conformance no compara punteros ni acepta un UUID porque
otro runtime lo generó: compara bytes, texto, variant/version y errores.

Se excluyen generación v1/v2/v3/v6/v8, MAC/node identity, UUID como secreto,
monotonicidad estricta v7, compact/braced text, timezone lookup, ambient
capabilities, collision registries, hidden retries y cualquier API async.

El contrato machine-readable y los negativos ejecutables son
[`testing/stdlib-uuid.json`](../../testing/stdlib-uuid.json),
[`scripts/stdlib-uuid-check.sh`](../../scripts/stdlib-uuid-check.sh) y
[`scripts/stdlib-uuid-test.sh`](../../scripts/stdlib-uuid-test.sh). El diseño B0
queda cerrado por `STD-ID-001`; la implementación, providers, tests/fuzzing,
rendimiento, conformance y documentación permanecen pendientes de las leaves
`STD-UUID-IMPL-001`, `STD-UUID-HOST-001`, `STD-UUID-TEST-001`,
`STD-UUID-PERF-001`, `STD-UUID-CONF-001` y `STD-UUID-DOC-001`.
