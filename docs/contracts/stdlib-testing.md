# Contrato de `std.testing`

**Estado:** contrato de owner aceptado para `STD-0.1A`; el núcleo sellado del
runner ya está fijado por [`TONDO_TESTING_SPEC.md`](../../TONDO_TESTING_SPEC.md)
y la implementación T0 de estos helpers está integrada en compiler, VM y
runner. La promoción del owner y la conformance completa siguen siendo gates
posteriores de la matriz de la Standard Library.

Este documento cierra la superficie ordinaria que el núcleo de testing deja
pendiente: assertions con valores útiles, diffs de texto, comparación de
floats, consumo explícito de `Option`/`Result`, directorios temporales y datos
generados reproducibles. El registro
[`testing/stdlib-testing.json`](../../testing/stdlib-testing.json) es la forma
machine-readable de las mismas decisiones y
[`scripts/stdlib-testing-check.sh`](../../scripts/stdlib-testing-check.sh) las
comprueba dentro de `scripts/test-gate.sh`.

`TONDO_TESTING_SPEC.md` continúa siendo la autoridad para `test`, `suite`, el
envelope, lifecycle, `log`, `tags`, `failNow`, `skip`, `attach`, `snapshot`,
`withVirtualTime`, `VirtualTime.settle` y `VirtualTime.advance`. Este contrato
no cambia ninguna de esas firmas, sus diagnósticos ni los formatos de reporte,
artifact y snapshot ya publicados en la especificación de testing.

## Frontera y principios

`std.testing` solo aparece en source sets `unit-test` o `integration-test`.
Producción no puede importarlo y un import de testing no concede capabilities
de host. Los helpers de este contrato se dividen en dos grupos:

- **Core y deterministas:** assertions, `TextDiff`, tolerancias y generación.
  No requieren `console`, `filesystem`, `environment`, `clock`, `entropy`,
  `network`, `process` ni `threads`.
- **Recursos temporales:** `tempDirectory` requiere explícitamente la
  capability `filesystem`. No consulta `TMPDIR`, environment, red ni un
  servicio externo; el runner entrega un root aislado al worker.

No existe `TestContext`, `currentTest`, registro runtime, reflection de valores,
selector basado en tags, callback de lifecycle ni una captura recuperable de
pánicos. Un helper que no puede cumplir una comprobación utiliza el mismo
`P0007` de `assert`/`failNow`; no introduce una excepción privada ni un segundo
canal de resultados.

Las entradas, expectativas y actual se evalúan de izquierda a derecha. Las
operaciones que solo observan valores reciben `ref` y nunca mueven el valor del
caller. Las operaciones `assertSome`, `assertOk` y sus pares consumen
explícitamente el wrapper y devuelven su payload: la llamada hace visible que
se ha abierto un `Option` o un `Result` y no inventa un `?` implícito.

## Assertions de valores

La superficie canónica es pequeña y no duplica el `assert` del lenguaje:

~~~tondo
import std.testing

test checksRecords {
    let expected = makeExpected()
    let actual = loadActual()?
    let changed = mutateCopy(actual)

    testing.assertEqual(ref expected, ref actual)
    testing.assertNotEqual(ref expected, ref changed)
}
~~~

Las firmas son:

~~~tondo
pub fn assertEqual[T: Equatable + Display](expected: ref T, actual: ref T)
pub fn assertNotEqual[T: Equatable + Display](expected: ref T, actual: ref T)
pub fn assertTextEqual(expected: String, actual: String)
pub fn diffText(expected: String, actual: String): TextDiff
pub fn TextDiff.render(ref self): String
~~~

`assertEqual` y `assertNotEqual` observan por préstamo, exigen la igualdad
intrínseca y `Display` para producir un diagnóstico humano acotado y fallan con
`P0007` cuando la condición no se cumple. No serializan valores con reflection
ni incluyen sus bytes completos en JSON/JUnit. El orden del mensaje es siempre
`expected` antes de `actual`; el contenido mostrado se trunca conforme al
resource profile del intento.

The hosted VM binds each concrete assertion callable to its statically selected
`Display` implementation, including generic wrappers and function values. A
successful assertion never calls `Display`. A failed `assertEqual` displays
`expected` before `actual`; `assertNotEqual` displays the equal value once.
`assertOk` displays only `E` and `assertErr` only `T`, so the other payload needs
no `Display` bound. Successful unwrapping moves the consumed Result payload and
does not require `Copy`. Failure preserves the owned payload's structural
cleanup. A panic, skip, or resource limit from `Display` keeps its original test
outcome and cleanup semantics.

The diagnostic retains at most 1024 bytes per displayed value, ending on a
UTF-8 boundary, followed by `...<truncated>` when necessary. Intrinsic scalar
and Array display streams directly into this prefix; the iterative Array walk
admits 64 logical bytes per pending cursor. A user implementation executes
normally under the phase limits and its returned String is then copied into the
bounded prefix. Each prefix buffer and the complete assertion message are
admitted to the calling phase before construction. A rejected admission
publishes a resource failure instead of a partial assertion diagnostic. The
profile binds `static-display-utf8-prefix-1024-frame-64/1`. Diagnostic ownership
and publication follow the shared transport and evidence accounting in
[`test-limits.md`](test-limits.md). Bytecode
verification rejects missing or mismatched Display metadata and nonlocal,
asynchronous, or incorrectly typed dispatch targets.

`assertTextEqual` usa exactamente igualdad bytewise de `String` y, si falla,
añade el `TextDiff` acotado al mensaje de `P0007`. No llama a `snapshot`, no
abre el snapshot store y no transforma la comprobación en una actualización.
Para inspeccionar sin fallar, `diffText` devuelve un valor puro que el caller
puede observar o adjuntar explícitamente.

No hay aliases `equals`, `same`, `expect`, `assertEq` ni macros equivalentes.
Una comparación de dominio distinta de `Equatable` continúa siendo una función
normal del módulo dueño.

## Diff de texto

`TextDiff` es un diagnóstico temporal, no un formato de snapshots. Su forma
lógica es:

~~~tondo
pub enum TextDiffHunk {
    Equal(String)
    Delete(String)
    Insert(String)
}

pub type TextDiff = {
    equal: Bool
    hunks: Array[TextDiffHunk]
    expectedBytes: Int
    actualBytes: Int
    truncated: Bool
}
~~~

Las líneas se separan por `LF` sin normalizar `CR`, Unicode o whitespace. El
algoritmo es un shortest-edit-script de Myers sobre líneas, con empate estable
por el offset esperado menor y, después, el offset actual menor. Hunks
adyacentes del mismo kind se fusionan y conservan los bytes de sus líneas,
incluido el terminador cuando estaba presente. La entrada vacía y el texto sin
terminador final son casos normales.

`TextDiff.render` produce el formato acotado
`tondo-test-text-diff-0.1/1`: cabecera `--- expected`, cabecera `+++ actual`,
hunks unificados y una marca final `... truncated ...` cuando el límite impide
mostrar todo. No incluye paths físicos, timestamps, hashes ocultos ni el
snapshot esperado completo. El formato se usa solo para diagnóstico humano o
para un valor que el caller adjunte; no tiene store, update, key de snapshot ni
semántica de aceptación.

El diff tiene límites de bytes de entrada, líneas, hunks y bytes de salida.
Cuando se alcanza uno, devuelve `truncated: true` con los prefijos ya calculados
y nunca reserva el resto para intentar completar la salida. Dos entradas
iguales siempre producen `equal: true` y un array de hunks vacío. Los helpers no
dependen del locale ni de una implementación de regex.

The hosted kernel uses the linear-space, bidirectional Myers algorithm described
in [Myers, section 4b](https://neil.fraser.name/writing/diff/myers.pdf). Two reusable
frontiers and an explicit partition stack replace the former quadratic LCS
table. Frontier ties retain the earlier first matching expected/actual offsets;
equal-distance crossings use those offsets before their middle-snake coordinates.
Within a replacement, complete `Delete` hunks precede complete `Insert` hunks.
The independent bounded edit-distance oracle checks minimality and exact replay;
separate golden cases fix repeated-line ties, CR/LF bytes and unterminated text.

An allocation-free `TextDiffPlan` lets the hosted runner admit workspace and
result storage before computation. Output limits are checked on complete borrowed
hunk ranges before copying their text; rendering is never used to measure an
intermediate result. As in the existing empty truncated representation, internal
output limits smaller than the header plus truncation marker retain that fixed
envelope. The public default is one MiB, including the envelope.

The hosted compiler exposes `TextDiff` and `TextDiffHunk` as ordinary nominal
declarations. Fields, exhaustive hunk matches, record construction and persistent
record updates use the same rules as user declarations. Both `diff.render()` and
`testing.TextDiff.render(diff)` borrow the receiver. A same-named user record
does not acquire this method. The adapter moves computed hunk strings into the
record; it does not publish an opaque diff handle.

The shared borrowed `TextDiffRenderPlan` also bounds caller-constructed records
to 4096 hunks and one MiB of rendered output. It preserves complete hunk prefixes
and adds the truncation marker when required without changing the source record's
`truncated` field. The other fields remain ordinary caller-supplied metadata;
rendering observes `hunks` and `truncated`. Admission measures exact UTF-8 output
before allocating its string and never constructs an intermediate owned diff.
The returned value retains the shared owned-transport reservation through typed
VM import. Construction and transport use the distinct logical units recorded
in [`test-limits.md`](test-limits.md).

## Floats y tolerancia

La tolerancia se construye una vez y queda validada antes de usarla:

~~~tondo
pub type FloatTolerance

pub enum FloatToleranceError {
    Negative
    NonFinite
    Overflow
}

pub fn FloatTolerance.from(absolute: Float, relative: Float): FloatTolerance ! FloatToleranceError
pub fn assertFloatNear(expected: Float, actual: Float, tolerance: ref FloatTolerance)
pub fn assertFloat32Near(expected: Float32, actual: Float32, tolerance: ref FloatTolerance)
~~~

`absolute` y `relative` deben ser finitos y no negativos. La comprobación para
valores finitos es:

~~~text
abs(actual - expected)
    <= max(absolute, relative * max(abs(expected), abs(actual)))
~~~

La implementación evita overflow intermedio y mantiene el mismo resultado en
VM y backend nativo. `Float32` se ensancha exactamente a `Float` después de
conservar sus bits; no se redondea de nuevo antes de comparar.

Las reglas especiales son cerradas:

- `NaN` nunca satisface `assertFloatNear`, incluso consigo mismo;
- dos infinitos del mismo signo satisfacen la comparación por igualdad exacta;
- un infinito frente a un valor distinto falla; y
- `+0.0` y `-0.0` son iguales bajo la igualdad IEEE de Tondo.

Un `FloatTolerance` es opaco, inmutable, `Copy`, `Equatable` y no tiene
capabilities. Una tolerancia inválida falla al construirla mediante
`FloatToleranceError`; no llega a una assertion ni se convierte en `P0007`.
Una assertion que no alcanza la tolerancia usa `P0007` con expected, actual y
los dos límites, todos acotados por el perfil.

The hosted compiler exposes all three `FloatToleranceError` variants as the
ordinary public enum. The bridge maps the kernel's typed cause directly to
`Negative`, `NonFinite` or `Overflow`; it does not infer a cause from diagnostic
text. Enum construction and exhaustive matching use the normal language rules.
An error result admits 83 logical bytes before construction and allocates no
host registry entry. The shared owned-return protocol accounts for its detached
lifetime through typed VM import. The standard enum implements `Display` with the
qualified form `FloatToleranceError.Negative` (and the corresponding variant
name for the other causes). Qualified calls, calls through a visible `Display`
constraint and string interpolation use that implementation. A same-named user
enum receives no implicit implementation.

No existe un epsilon global, una tolerancia dependiente de la máquina, una
comparación ULP escondida ni una aceptación automática de `NaN`.

## Consumo explícito de `Option` y `Result`

Estas cuatro operaciones son las únicas formas abreviadas y consumen el
wrapper que reciben:

~~~tondo
pub fn assertSome[T](value: T?): T
pub fn assertNone[T](value: T?): Unit
pub fn assertOk[T, E: Display](value: T ! E): T
pub fn assertErr[T: Display, E](value: T ! E): E
~~~

`assertSome` devuelve el `T` de `Some` o termina con `P0007` si recibe `None`.
`assertNone` devuelve `Unit` únicamente para `None`; un `Some` produce el mismo
fallo sin borrar su valor antes de construir el diagnóstico. `assertOk` devuelve
el éxito y muestra el error con `Display` si recibe `Err`; `assertErr` hace lo
dual y muestra el éxito si recibe `Ok`.

El resultado devuelto conserva exactamente su ownership y sus obligaciones.
Si `T` o `E` es afín, el caller continúa siendo responsable de moverlo,
consumirlo o registrarlo con `defer`. Ninguna función devuelve `Option`, añade
una variante de error, absorbe una obligación terminal o transforma
`Option[Result[T, E]]` en `Result[Option[T], E]`. Para comprobar una variante
concreta se consume primero el wrapper y después se utiliza `match` o `==` de
forma ordinaria.

## Recursos temporales

Un test que necesita filesystem puede reservar un root aislado:

~~~tondo
import std.fs
import std.testing

test writesTemporaryDocument {
    let workspace = testing.tempDirectory("document")?
    defer testing.TempDirectory.cleanup(workspace)

    let path = workspace.path()
    fs.writeText(path.join("input.txt"), "hello")?
    testing.assertTextEqual("hello", fs.readText(path.join("input.txt"))?)
}
~~~

Las firmas y el ownership son:

~~~tondo
pub type TempDirectory

pub enum TempError {
    InvalidPrefix
    Unavailable
    PermissionDenied
    LimitExceeded
    IoError
}

pub fn tempDirectory(prefix: String): TempDirectory ! TempError
pub fn TempDirectory.path(ref self): Path
pub fn TempDirectory.cleanup(self)
~~~

`TempDirectory` es opaco, afín, `Send` y no `Share`; su único terminal es
`cleanup`, que consume el owner. La llamada a `path` solo observa y devuelve un
`Path`; no devuelve un handle al allocator del host. El prefijo es ASCII
portable (`[A-Za-z0-9._-]`, como máximo 32 bytes, también puede ser vacío) y no
puede escapar del root.

El runner crea el directorio bajo un root privado del worker mediante un nonce
del host. Ese nombre y el path físico nunca entran en el reporte canónico,
JUnit, snapshots, tags ni la identidad del test. No se consulta `TMPDIR`,
`HOME`, environment o la red. El directorio se puede usar con `std.fs`; no
duplica operaciones de archivos, permisos o paths.

`cleanup` tiene resultado normal `Unit` para poder aparecer en `defer`. Si el
host no puede eliminar recursivamente todo el contenido, la operación se hace
terminal para el envelope como `infrastructure`, conserva el diagnóstico de
cleanup y no finge que el recurso quedó revocado. El runner intenta limpiar el
root después de timeout o terminación forzada; si no puede hacerlo, la
invocación no publica un reporte completo y usa exit `3` según
`TONDO_TESTING_SPEC.md`.

La limpieza no sigue symlinks, rechaza escapes y aplica límites de entries y
bytes. Los paths que el test copie fuera del root, o los bytes que publique con
`attach`, `log`, `snapshot` o output, dejan de estar protegidos por esta API.
No existe `tempFile` duplicado: un archivo temporal es un path dentro de
`TempDirectory` y usa el owner canónico `std.fs`.

### Hosted temporary-root implementation boundary

`TempError` is the public nominal enum, with ordinary construction and exhaustive
matching. The hosted bridge preserves `InvalidPrefix`, `Unavailable`,
`PermissionDenied`, `LimitExceeded` and `IoError` as distinct typed causes. A
missing or invalid explicit root is `Unavailable`; a provider permission failure
is `PermissionDenied`. Bounded tree admission carries a typed limit cause through
I/O, so diagnostic wording cannot change the selected variant. The standard
enum's `Display` implementation produces `TempError.<Variant>`.

An error result admits 73 logical bytes before construction and uses no host
registry entry. The shared owned-return protocol accounts for its detached
lifetime through typed VM import; construction retains its separate charge.

The public CLI coordinator now owns a distinct physical root for each worker
invocation, including retries and repetitions. It passes that root separately
from the hashed input artifact and removes it after reaping the worker. Failed
root cleanup prevents complete JSON, JUnit, snapshot and artifact publication;
prior complete outputs remain intact. An embedded compiler caller must supply
its own root explicitly. Neither route consults ambient temporary-directory
variables. References to `tempDirectory` and `cleanup` require `filesystem`,
including function values and deferred calls.

The hosted root bounds are 1,048,576 entries (including its root), 64 MiB of
regular-file lengths and 64 descendant levels. Traversal rejects observed
symlinks and special files, validates the complete tree before deletion, and
rechecks each deletion step. Ordinary `std.fs` mutations under the supplied
root preflight their predicted growth; atomic replacement admits the staging
file while the old destination still exists. Directory moves check the depth
and size of the resulting subtree.

There is also a 64 MiB budget for admitted write and import bytes over the
worker's lifetime. It counts requested bytes before host I/O, including partial
or failed I/O. Overwrites and removal do not replenish it. A file opened under
the root keeps this budget across rename and unlink. These are logical provider
limits, not RSS or an OS sandbox for arbitrary external processes or general
filesystem paths outside the supplied root. The model identifier is included
in the effective resource-profile hash; physical names are excluded.

`TempDirectory` retains a 32-byte logical descriptor and its native path bytes.
The constructor admits the descriptor, path, result and name scratch before
creation. `path` admits a separate copy in its caller's phase; `cleanup` releases
the original owner's charge. Open file descriptors also participate in phase
collection; nominal temporary errors are ordinary VM values. Full T0 promotion
still needs the remaining host-owner, detached-transport and traversal-scratch
accounting, and the whole-tree gate evidence.

## Datos generados, replay y shrinking

La generación de datos es una utilidad explícita para un test ordinario; no
registra hojas, no crea subtests desde un `for` y no altera el árbol estático.
El generador es core, determinista y no criptográfico:

~~~tondo
pub type Generator
pub type GenerationId = {
    seed: UInt64
    caseIndex: UInt64
}

pub enum GenerationError {
    InvalidBounds
    LimitExceeded
    Exhausted
}

pub fn Generator.new(seed: UInt64): Generator
pub fn Generator.forCase(seed: UInt64, caseIndex: UInt64): Generator
pub fn Generator.id(ref self): GenerationId
pub fn Generator.drawCount(ref self): UInt64
pub fn Generator.nextUInt(mut self): UInt64 ! GenerationError
pub fn Generator.nextBool(mut self): Bool ! GenerationError
pub fn Generator.nextInt(mut self, minimum: Int, maximum: Int): Int ! GenerationError
pub fn Generator.nextBytes(mut self, maximumLength: Int): Bytes ! GenerationError
pub fn Generator.nextText(mut self, maximumBytes: Int): String ! GenerationError

pub trait Shrink {
    fn candidates(self, limit: Int): Array[Self] ! GenerationError
}

pub fn shrink[T: Shrink + Equatable](value: ref T): Array[T] ! GenerationError
~~~

`Generator.forCase(seed, caseIndex)` es la única operación de replay: el mismo
par de valores y la misma secuencia de draws reconstruyen el mismo input. El
índice es cero-based. `Generator.new(seed)` crea el stream del caso `0`; usar
`forCase` evita depender de cuántos casos anteriores se hayan generado.

The hosted compiler exposes `GenerationId` as the ordinary public record above.
Both fields retain their full `UInt64` range. Field access, construction, copying
and record updates use the ordinary nominal rules; an identity is not a host
registry token. `Generator.id` admits 108 logical bytes before constructing its
detached record (three value descriptors and its type name), without advancing
the stream or allocating a registry entry. The VM materializes that record in
its owning phase. The shared owned-return protocol retains the record's charge
through queueing and typed VM import, separately from construction admission.

`GenerationError` is also an ordinary public enum. `InvalidBounds` denotes an
inverted integer range or a negative buffer bound; `LimitExceeded` denotes a
payload or candidate limit; `Exhausted` denotes the fixed generator draw budget.
The bridge transports the typed kernel cause without parsing diagnostic text.
It admits 79 logical bytes before constructing an error result and allocates no
error registry entry. Its `Display` implementation produces
`GenerationError.<Variant>`. Rejected requests preserve the complete generator
state and draw count. The non-Result constructors accept only `UInt64`; malformed
raw host arguments are host contract errors and never invent a Result return.

El algoritmo `xorshift64-7-9-8-v1` está cerrado para 0.1. Inicializa el estado
con `seed XOR 0x9e3779b97f4a7c15`, deriva un caso con suma modular de
`caseIndex * 0x9e3779b97f4a7c15` y aplica, en cada draw, `<<7`, `>>9` y `<<8`
con wrap de `UInt64`. Un estado cero usa el valor no nulo fijo
`0x6a09e667f3bcc909`. `nextInt` usa rejection sampling sin sesgo en el rango
inclusivo; `nextBytes` elige una longitud entre cero y `maximumLength`; y
`nextText` produce únicamente scalars Unicode válidos con UTF-8 canónico.

`GenerationId` se puede serializar o adjuntar explícitamente como
`tondo-test-generation-0.1/1`, con `seed` en 16 dígitos hexadecimales lowercase,
`case` decimal y el algoritmo cerrado. El valor generado nunca se escribe de
forma implícita en JSON/JUnit, snapshot stores o logs. Si un test necesita
replay debe conservar el `GenerationId` o registrarlo de manera explícita.

Los métodos consumen presupuesto de draws y bytes antes de avanzar el estado;
un límite fallido no produce un valor parcial. El generador no consulta reloj,
entropy, environment, filesystem, proceso, red ni threads y no debe utilizarse
para secretos, tokens o claves.

The executable kernel commits a sampled integer or generated buffer only after
the complete operation succeeds. Exhaustion during a multi-draw sample leaves
the generator state and draw count unchanged. `planned_buffer_capacity` selects
the same bytes/text capacity without changing the stream or allocating output,
so the hosted test phase can admit it first. Descriptor and buffer accounting
is specified in `test-limits.md`. A returned `Bytes` supports its receiver
methods even when the source only imports `std.testing`; an explicit `std.bytes`
reference is not required to inspect the generated value.

`Shrink` es un protocolo estático para proponer candidatos, no un executor de
tests. En Tondo 0.1 es un protocolo de prelude sellado por el compilador: no se
puede declarar `impl Shrink` en código de usuario y una implementación manual
produce `E1114` (`closed protocol`). El compilador solo admite las formas
intrínsecas acotadas de enteros, floats, `String` y `Array[T]` cuyos elementos
también sean shrinkables. La implementación es pura, terminante y
determinista; elimina duplicados conservando la primera aparición y devuelve
candidatos en orden de menor complejidad. `shrink` aplica un límite finito y
devuelve `GenerationError` de forma atómica cuando el tipo o el límite no son
válidos; no ejecuta una predicate, no captura pánicos y no convierte fallos en
excepciones recuperables.

`Shrink.candidates(value, limit)` and calls through a visible `Shrink`
constraint execute the same hosted scalar kernel as `testing.shrink`. The method
accepts an explicit candidate count from zero through 4096. Negative counts
return `GenerationError.InvalidBounds`; larger counts return `LimitExceeded`.
Zero still validates the closed input shape and returns an empty array. The
receiver is observed through an immutable loan. The qualified prelude spelling
needs no module import inside a test target; production use is rejected with
`E2003`. The method does not introduce an open trait method lookup.

The hosted implementation validates the complete closed shape and its maximum
depth before cloning candidates. Every string prefix, array prefix and nested
replacement is admitted before copying. Nested temporary candidates retain
their own charge until discarded or moved into an admitted parent. Duplicate
array replacements are compared by borrowing their components before copying;
UTF-8 string prefixes are visited once per character boundary. Successful
candidate order remains equal to the independent kernel. A phase-memory limit
is runner control and cannot be caught as `GenerationError`.

El runner de tooling conecta este helper mediante
[`test-generation.md`](./test-generation.md): materializa una campaña con
`Generator.forCase`, ejecuta los casos en el `RuntimeRunner` ya existente y
reprueba candidatos en workers nuevos durante el shrinking. La campaña conserva
el orden, replay y límites, pero nunca registra subtests dinámicos ni cambia
los formatos de reporte del runner.

## Límites, formatos y promoción

Todas las operaciones cargan el resource profile del intento. Los límites
incluyen bytes de mensajes y `Display`, bytes de entrada/salida y líneas/hunks
del diff, prefijo y árbol temporal, draws y bytes por caso, y candidatos y
profundidad de shrinking. El preflight es atómico: si no cabe, no se publica
un hunk, valor, descriptor o estado de generador parcial.

Los formatos de diagnóstico son `tondo-test-assertion-0.1/1`,
`tondo-test-text-diff-0.1/1` y `tondo-test-generation-0.1/1`. Son valores que el
caller puede materializar o adjuntar; ninguno es un snapshot store, acepta
`--update-snapshots` o cambia `tondo-test-report-0.1/8` y
`tondo-junit-report-0.1/5`. El formato de assertion solo identifica el payload
acotado de expected/actual en un fallo `P0007`; no crea un archivo adicional.
Temp directories no tienen formato de persistencia: solo dejan el path
operativo fuera del reporte.

La promoción exige equivalencia VM/backend para las comparaciones, errores,
ownership y límites; diff bytewise estable; tolerancia float independiente de
CPU; consumo explícito de Option/Result; cleanup de recursos incluso bajo
fallo, cancelación y timeout; replay exacto con el algoritmo cerrado; shrinking
acotado y sin captura de pánicos; ausencia de capabilities no declaradas; y
ninguna alteración del núcleo sellado de `TONDO_TESTING_SPEC.md`.
