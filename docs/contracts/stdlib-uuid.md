# Contrato de `std.uuid`

Estado: **contract-locked** para `STD-0.1B` / `STD-ID-001`.

This document locks the normative `std.uuid` design. `STD-UUID-IMPL-001` now
has an explicit-input scalar Rust kernel; its local promotion state and proof
are recorded in section 9. Section 10 records the new hosted provider/public
registration boundary. Section 11 records the verified independent model/fuzz
boundary; section 12 defines the hosted scalar performance campaign.
Conformance and usage documentation remain separate owner leaves.

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

The normative Tondo capabilities are `Copy`, `Discard`, `Equatable`, `Key`,
`Send` and `Share`. Ordering is exposed by `Uuid.compare`; `Eq`, `Ord` and
`Hash` are Rust traits of the kernel, not additional intrinsic Tondo
capabilities. A UUID contains no references, host handles or mutable aliases.
The Rust kernel's read operations use fixed width state, except formatting an
owned string. Hosted transport and managed VM records have separate logical
storage costs; public `toBytes`/`toString` additionally materialize copies.

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
queda cerrado por `STD-ID-001`. The implementation boundary below does not
promote providers, public compiler/VM/native registration, independent testing,
performance, conformance or executable usage documentation.

## 9. Explicit-input Rust kernel

`crates/tondo-stdlib/src/uuid.rs` implements immutable network-order values,
nil/max, strict dashed/URN parsing, canonical formatting, variant/version,
unsigned byte comparison and v4/v5/v7 transformations. The selected route is
`rust-scalar-kernel`. `ready-stdlib-kernel` records focused executable evidence
with `pending-80-percent-per-scope`; `verified-stdlib-kernel` requires the full
functional gate and `verified-80-percent-per-scope` quality evidence. Publication
and exact-SHA CI closure remain separate proof in the tracker.

No provider calls occur in this kernel. Rust `Uuid::v4(&[u8])` requires exactly
16 supplied entropy bytes. `Uuid::v7(i128, &[u8])` checks UTC Unix milliseconds
in `0..=281474976710655`, then requires exactly ten supplied bytes. Both reject
short or excess snapshots as `ProviderMisconfigured`; timestamp range rejection
precedes snapshot length rejection. They overwrite only version/variant bits,
preserve respectively 122/74 entropy bits and publish no partial value. The
input snapshots are trusted boundary data, not proof of entropy quality or
provider capability. Those checks belong to `STD-UUID-HOST-001`.

Rust `Uuid::v5(Uuid, &[u8], UuidLimits)` checks `max_name_bytes` before hashing.
The default is 16 MiB; zero permits an empty name. Names remain opaque, including
non-UTF-8 bytes. Incremental hashing of the namespace and name avoids a combined
input allocation. The exactly pinned `sha1 = "=0.10.6"` dependency uses
`force-soft`; assembly features are not enabled. It uses `digest 0.10.7`,
separately from the workspace's `sha2 0.11` / `digest 0.11` family. This is a
UUIDv5 interoperability dependency, not a general hashing API or a release
license decision.

Text length is checked before lexical scanning. Wrong lengths have
`InvalidTextLength` with no offset; bytes require exact length with
`InvalidBytesLength`. A malformed 45-byte URN prefix reports `InvalidUrnPrefix`
at its first mismatching byte. Characters and separators report the first
invalid absolute byte offset, including the nine-byte prefix. Length, name
limit, snapshot length and timestamp failures have no lexical offset.

The Rust value and `to_bytes()` result contain sixteen inline bytes. Parse,
compare, inspection and generation use bounded stack/hash state. Canonical
formatting builds 36 ASCII bytes on the stack; `try_to_string()` reserves an
owned 36-byte string and maps a failed reservation to `OutOfMemory`. `Display`
writes the same bytes to its caller's formatter, whose allocation policy is
outside the kernel. These facts do not measure allocator calls, RSS or code
size. The Rust return types alone do not demonstrate public Tondo `Bytes`/`String`
materialization or `vm_heap` admission; section 10 supplies the hosted evidence.

The 18 kernel tests cover the RFC 9562 v4/v5/v7 vectors, all 256 variant bytes
crossed with all 16 version nibbles, exact lexical offsets, byte-copy ownership,
unsigned ordering, entropy bit preservation, timestamp boundaries, v5 name
limits, repeated-call behavior and immutable threaded reuse. Provider error
variants are declared, with stable environment-free display, but actual provider
failure normalization is not established by these tests. An independent model,
bounded fuzz campaign and target-qualified performance remain pending.

The implementation checker and tests are
[`scripts/stdlib-uuid-implementation-check.sh`](../../scripts/stdlib-uuid-implementation-check.sh)
and [`scripts/stdlib-uuid-implementation-test.sh`](../../scripts/stdlib-uuid-implementation-test.sh).
They validate both local states and reject 96 invalid state/route/dependency
records before running the canonical 18 Rust tests and checking `force-soft`.
The historical kernel-only host state is `not-claimed-until-uuid-host`;
production VM, native ABI and native AOT are not promoted by that kernel proof.
Hosted progression is recorded separately below, followed by TEST, PERF, CONF
and DOC.

The kernel block is `verified-stdlib-kernel`. Its completed source revision
passed the full functional gate and source-bound quality, with 293,081 of
319,953 covered workspace lines
(91.6013%), every global/risk 80% line/function/region floor and all six selected
critical mutants caught. The indexed UUID source coverage includes its unit
tests; it is not a separate production-only coverage campaign. Publication
and exact-SHA CI closure are recorded in the tracker.

## 10. Hosted provider and public registration boundary

`STD-UUID-HOST-001` implements all fourteen public operations in the compiler
and production VM, selecting `hosted-scalar`. The parent register links the
[`host register`](../../testing/stdlib-uuid-host.json) and
[`host contract`](stdlib-uuid-host.md). `ready-production-hosted` requires focused
proof and keeps quality pending; `verified-production-hosted` requires the
source-bound quality and full functional gate. Publication/CI closure remains
a separate tracker step.

The carrier is a private high/low `UInt64` record preserving all 128 bits.
Provider calls use exactly pinned `getrandom 0.4.3` and checked `SystemTime`.
Core/import/v5 remain provider-free; references to v4 and v7 require the declared
capabilities. A sealed Rust envelope fixture exercises once-only providers,
nominal failures and refusal without granting source capabilities. Both public
error fields and intrinsic `Display` work through the real test framework.

The host reserves transport and complete typed result storage before provider
effects. Ordinary synchronous VM execution checks its byte/object heap limits
without creating a test account; test execution additionally reserves its
phase account and joint import pool. This boundary promotes hosted registration
and admission only. Public conformance, native ABI/AOT, performance and usage
remain unpromoted; model/testing progression is recorded below.

The hosted block is locally `verified-production-hosted`: its 23 focused
compiler/VM tests and 86 invalid promotion records pass, along with every full
functional gate step. The frozen workspace source covers 294,459 of 321,402
lines (91.6170%), preserves every global/risk 80% coverage floor and catches all
six selected critical mutants. Publication and exact-SHA CI closure are recorded
in the tracker.

## 11. Independent model, regression and fuzz boundary

`STD-UUID-TEST-001` is verified on the frozen source from `aa0aae4`.
The [test register](../../testing/stdlib-uuid-test.json) and
[test contract](stdlib-uuid-test.md) define a std-only unsigned integer oracle,
an independently computed bounded v5 digest, seventeen valid and thirty-seven
invalid retained vectors, eleven integration tests and a bounded fuzz target.
The model compares 4,096 deterministic seeds and replays a sealed provider
transcript through the production VM. Reference name/provider bounds are not
production limits. Native ABI/AOT, performance, public conformance and usage
are separate later boundaries.

The eleven integration tests, eighteen kernel tests, twenty-three HOST tests,
85 invalid testing records and the 128-run fuzz smoke pass. The single
consolidated quality campaign covers 294,814 of 321,733 workspace lines
(91.6331%), preserves every global/risk 80% floor and catches all six selected
critical mutants. Every full functional gate step passes on the same source,
including the 206-case draft suite and repeated async/select observations.
Published source `27bd0d3` and tracker closure `048730a` pass exact-SHA CI runs
`37463121847` and `37468095223`, respectively, with quiet-interval confirmation.
The next owner boundary is `STD-UUID-PERF-001`.

## 12. Target-qualified hosted scalar performance

The [performance register](../../testing/stdlib-uuid-performance.json) and
[performance contract](stdlib-uuid-performance.md) define 22 direct hosted
bridge workloads with 27 retained samples each. Dashed/URN parse, canonical
format, byte round trips, v4/v5/v7, sealed and explicit OS providers, and ten
bounded refusal routes use independent/authored expectations before timing.
The current state is `measurement-ready`; clean capture, the complete functional
gate and current source-bound quality remain necessary for promotion.

Selected owned identities and retained logical storage include fixture setup,
while actual reply charges and host registry increments are separately observed.
Logical memory is not RSS or allocator instrumentation. Provider requests are
not OS syscalls. The report measures the existing hosted scalar bridge with
reply admission, without executing bytecode or claiming complete VM latency,
native UUID ABI/AOT, SIMD or code-size measurements. CONF and DOC remain open.
