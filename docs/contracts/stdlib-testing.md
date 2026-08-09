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
`--update-snapshots` o cambia `tondo-test-report-0.1/7` y
`tondo-junit-report-0.1/4`. El formato de assertion solo identifica el payload
acotado de expected/actual en un fallo `P0007`; no crea un archivo adicional.
Temp directories no tienen formato de persistencia: solo dejan el path
operativo fuera del reporte.

La promoción exige equivalencia VM/backend para las comparaciones, errores,
ownership y límites; diff bytewise estable; tolerancia float independiente de
CPU; consumo explícito de Option/Result; cleanup de recursos incluso bajo
fallo, cancelación y timeout; replay exacto con el algoritmo cerrado; shrinking
acotado y sin captura de pánicos; ausencia de capabilities no declaradas; y
ninguna alteración del núcleo sellado de `TONDO_TESTING_SPEC.md`.
