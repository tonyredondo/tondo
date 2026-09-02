# Contrato de `std.channel`

**Estado:** contrato `contract-locked` para STD-0.1B, con la guía ejecutable
cerrada por `STD-CHANNEL-DOC-001`. El registro machine-readable está en
[`testing/stdlib-channel.json`](../../testing/stdlib-channel.json) y la
superficie canónica completa en
[`TONDO_STANDARD_LIBRARY_SPEC.md`](../../TONDO_STANDARD_LIBRARY_SPEC.md).
Este cierre fija la semántica; no afirma que el runtime público exista todavía.
La semántica base quedó sellada por `STD-CONC-001`; esta hoja añade la guía
ejecutable sin cambiar ese contrato.

`std.channel` es la única abstracción de productor/consumidor con backpressure
de Tondo. Reutiliza el scheduler y la expresión núcleo `select`, no crea un
segundo tipo `Task`/`Future`, no añade `std.async.select` y no introduce
capabilities ambientales. La implementación necesita primitivas de wakeup,
estado atómico y almacenamiento de VM y backend nativo, por lo que su frontera
`HOST` queda cerrada para el scheduler hosted y el bridge nativo privado por
`STD-CHANNEL-IMPL-001`. Este cierre es target-qualified: no promociona todavía
los símbolos como API pública ni afirma lowering AOT genérico de canales.

## Frontera de implementación verificada

Estado de host: `verified-scheduler-and-native-bridge`; estado nativo:
`verified-native-runtime-abi`.

El compilador registra por identidad nominal `Sender[T]`, `Receiver[T]` y sus
errores, conserva el bound `T: Send` y baja `send`/`receive` seleccionables a
la misma keyword núcleo `select`. La VM hosted ejecuta una cola `VecDeque` con
jobs de scheduler: una llamada que no puede comprometerse se registra, se
desregistra al cancelar y se reintenta sin bloquear un worker cooperativo.
El orden FIFO se aplica al punto de commit, incluido el rendezvous de
`bounded(0)` y el backpressure de capacidades positivas.

El runtime nativo expone únicamente un bridge privado de capacidades opacas
`u64`. Cada identidad usa una celda `Mutex`/`Condvar`, las esperas bloqueantes
se permiten solo en workers nativos y los resultados se transportan por
handles `Result`/`Option` internos. El carrier `ChannelDrain` de
`Receiver.close` conserva los valores pendientes hasta que el lowering nativo
futuro los materialice como `Array[T]`; su layout no es una promesa ABI.

La fixture `tests/runtime/m11-std-channel-impl-001.to` verifica constructor,
FIFO, `Full`, cierre, backpressure y commit de ambos brazos seleccionables.
La sonda independiente
`crates/tondo-native-runtime/examples/channel_conformance.rs` verifica la
misma frontera observable en un proceso nativo fresco. Ambas pruebas son
evidencia de hosted VM y ABI nativo privado sobre el target host; no equivalen
a ejecución AOT Cranelift ni a una publicación de la librería.

## Superficie pública

```tondo
pub type Sender[T]
pub type Receiver[T]

pub enum ChannelError {
    InvalidCapacity
    ResourceLimit
}

pub enum SendError[T] {
    Closed(T)
    ResourceLimit(T)
}

pub enum TrySendError[T] {
    Full(T)
    Closed(T)
    ResourceLimit(T)
}

pub enum TryReceive[T] {
    Item(T)
    Empty
    Closed
}

pub fn bounded[T: Send](capacity: Int): (Sender[T], Receiver[T]) ! ChannelError
pub fn unbounded[T: Send](): (Sender[T], Receiver[T]) ! ChannelError
pub fn Sender.fork(ref self): Sender[T] ! ChannelError
pub fn Sender.send(ref self, value: T): Unit ! SendError[T] selectable
pub fn Sender.trySend(ref self, value: T): Unit ! TrySendError[T]
pub fn Sender.close(self): Unit
pub fn Receiver.fork(ref self): Receiver[T] ! ChannelError
pub fn Receiver.receive(ref self): T? selectable
pub fn Receiver.tryReceive(ref self): TryReceive[T]
pub fn Receiver.close(self): Array[T]
```

`T: Send` es necesario para que un payload atraviese una frontera de task o
thread. `Sender` y `Receiver` son handles no `Copy` y no `Clone`; `fork` es la
única forma pública de obtener otro endpoint de la misma identidad. Ambos
handles son `Send + Share` cuando el parámetro lo permite. El sender satisface
`Discard`: descartarlo ejecuta el cierre de ese endpoint. Un receiver conserva
una obligación terminal y debe cerrarse explícitamente; el compilador rechaza
su abandono implícito.

Una llamada directa a `send` o `receive` espera implícitamente. `await` delante
de esas llamadas produce `E1611`, igual que en el resto del modelo suspendible.
El efecto `selectable` solo describe que la misma operación puede registrarse
en `select`; no crea una API paralela ni permite polling público.

## Capacidad y orden

`bounded(0)` es un rendezvous: un envío solo se compromete con un receiver
compatible. `bounded(n)` para `n > 0` conserva hasta `n` valores comprometidos y
aplica backpressure antes de aceptar el siguiente. Una capacidad negativa
produce `ChannelError.InvalidCapacity`; una capacidad que no puede reservar sus
estructuras produce `ChannelError.ResourceLimit`.

`unbounded()` es explícito y nunca es el default. Su crecimiento está limitado
por el perfil de recursos del runtime; alcanzar ese límite devuelve
`ResourceLimit(value)` y conserva el payload. No existe una cola realmente
infinita ni una reserva oculta que pueda bloquear el proceso por memoria.

Los valores se observan en orden FIFO de commit. La concurrencia puede decidir
qué operación se lineariza primero, pero una vez comprometido el orden no se
reordena por task, prioridad ni orden de finalización de un scheduler.

## Ownership y commit

Un envío mueve `value` únicamente en su punto de linearización (invariante
machine-readable: `moves-only-on-linearization`). Si el canal está
cerrado, no hay capacidad, se cancela la espera o el brazo pierde un `select`,
el caller conserva el valor: la variante correspondiente de `SendError` o
`TrySendError` lo devuelve cuando la operación llega a publicar un error.
`trySend` nunca suspende y devuelve `Full(value)` cuando un buffer acotado está
lleno.

`receive` mueve un valor únicamente al hacer commit. `tryReceive` devuelve
`Item(value)`, `Empty` mientras el canal sigue abierto sin valores disponibles o
`Closed` cuando el input terminó. Un receiver perdedor de `select` no retira
ningún valor.

Los endpoints comparten identidad interna, no contenido duplicado. El cierre
de un endpoint consume exactamente ese owner; usarlo después es un error
estático. No hay callbacks, pollers ni handles detached que puedan ocultar la
obligación de cleanup.

## Cierre y estados

La identidad atraviesa `open`, `sender-closed`, `receiver-closed` y `drained`.
Cerrar un sender decrementa su cuenta; el último sender cierra la entrada, pero
los receivers todavía drenan los valores ya comprometidos y después observan
`none`. Cerrar un receiver decrementa su cuenta; el último receiver cierra la
salida, despierta a todos los senders pendientes con su payload intacto y
devuelve en un `Array[T]` los valores que ya estaban comprometidos en el
buffer, en orden FIFO. Si aún queda otro receiver, el array devuelto está vacío
y los valores siguen perteneciendo al canal.

Ningún cierre descarta silenciosamente un payload, duplica un wakeup ni regresa
antes de terminar el cleanup de las reservas. El cierre consume el endpoint y
no es idempotente; el uso posterior se diagnostica estáticamente. Cuando ambas
direcciones han terminado y el buffer está vacío, el canal queda `drained`.

## `select`, cancelación y fairness

`send` y `receive` implementan el protocolo núcleo de tres fases:

1. `prepare` registra readiness sin mover un payload ni retirar un valor.
2. `commit` de un único brazo hace la linearización y el movimiento atómico.
3. `rollback` de los perdedores desregistra la espera sin mutar el canal.

Cancelar una task o destruir una selección desregistra todos sus brazos antes
del unwind. Un envío cancelado conserva el valor en su scope; una recepción
cancelada conserva el valor en el canal. Los cierres despiertan a los waiters
con el resultado terminal apropiado y no dejan waiters detached.

Dentro de un mismo canal, los waiters compatibles se atienden por orden FIFO de
registro (invariante machine-readable: `FIFO-registration-per-operation`). Un
empate entre operaciones del mismo canal elige el registro
compatible más antiguo. Los empates de varios brazos delegan en la rotación
justa de `select`; no se promete un orden global entre canales diferentes. Bajo
el scheduler cooperativo y contención continuada, una política que permita
starvation no es válida por defecto.

## Iteración async

Cuando `T: Discard`, `Receiver[T]` implementa `AsyncIterator[T]`. El único
`for item in receiver` espera implícitamente cada `next`, conserva el
backpressure de un elemento y termina con `none` tras cerrar el último sender y
drenar el buffer. Salir pronto del `for` cierra el receiver; los elementos
pendientes se descartan solo porque el bound `Discard` lo hace seguro.

La adaptación está implementada y verificada en la VM hosted por
[`STD-CHANNEL-ASYNC-ITER-001`](./stdlib-channel-async-iter.md). El host privado
reutiliza el waiter FIFO de `receive`; no expone otro tipo de stream ni reclama
lowering AOT. `AsyncIterator.collect(limit:)` sigue siendo la extensión genérica
de `std.async`, nunca un método específico de `std.channel`.

Para valores afines se usa `receive` y `Receiver.close` explícitamente, de modo
que el caller puede recuperar los mensajes pendientes en vez de perder
ownership. No existe `for await`, `AsyncChannel`, materialización automática ni
un `collect` específico del canal.

## Costes y límites

El coste de una operación que ya puede comprometerse es proporcional al
trabajo de su cola local: `send`/`receive` sobre un buffer acotado hacen una
inserción o extracción FIFO y `trySend`/`tryReceive` no crean una espera. El
registro y la cancelación de waiters pueden recorrer la cola de operaciones
compatibles; no se presenta esa ruta como una garantía de latencia constante.
`select` inspecciona sus brazos en la rotación del núcleo y el coste crece con
el número de brazos.

La memoria lógica de un canal acotado queda limitada por su capacidad más los
waiters registrados. `unbounded()` sigue estando sujeto al límite de recursos
del runtime y devuelve `ResourceLimit(value)` antes de aceptar un payload que
no pueda conservar. El cierre libera endpoints, waiters y valores pendientes
según las reglas de `Cierre y estados`; no hay polling oculto, buffer infinito
ni executor implícito.

La campaña de rendimiento de
[STD-CHANNEL-PERF-001](./stdlib-channel-performance.md) mide la VM hosted y
no convierte estas reglas en una promesa de rendimiento nativo o AOT. Los
costes del bridge nativo privado y de cualquier lowering Cranelift futuro deben
medirse en un target comparable antes de promocionarse.

## Ejemplos ejecutables de composición

La guía de uso se verifica con
[`tests/runtime/m11-std-channel-doc-001.to`](../../tests/runtime/m11-std-channel-doc-001.to)
y sus sidecars `.stdout`/`.exit`. El fixture conserva una sola forma canónica:
constructores `bounded`, endpoints explícitos, llamadas directas que esperan,
`select` núcleo y `scope` para joins. Las cinco familias son:

| ID | Función del fixture | Composición que fija |
| --- | --- | --- |
| `fan-out-fan-in` | `fan_out_fan_in` | `Sender.fork`, dos productores y un receiver que suma resultados |
| `pipeline-backpressure` | `pipeline_backpressure` | dos canales acotados, etapa intermedia y backpressure de un elemento |
| `select-cancel-safe` | `select_cancel_safe` | `select` sobre un envío listo y cierre que despierta un sender bloqueado |
| `close-and-drain` | `close_and_drain` | último receiver, drenado FIFO y recuperación de ownership |
| `discardable-iteration` | `discardable_iteration` | `for value in receiver`, cierre del sender y descarte seguro |

Los ejemplos usan las firmas canónicas
`pub fn Sender.send(ref self, value: T): Unit ! SendError[T] selectable`,
`pub fn Receiver.receive(ref self): T? selectable`,
`pub fn Sender.close(self): Unit` y
`pub fn Receiver.close(self): Array[T]`. El primer ejemplo hace explícito el
`fork` para que ningún endpoint se copie implícitamente; el pipeline muestra
que el productor puede continuar mientras la etapa libera capacidad al
consumidor. `close-and-drain` deja el array vacío para el primer receiver y
devuelve `[11, 12]` al último, por lo que la regla de cierre no queda solo en
prosa.

`select-cancel-safe` verifica en la VM hosted el commit de un brazo listo y,
en un rendezvous separado, que cerrar el receiver despierta el `send` pendiente
con `Closed(value)` sin perder el payload. La preparación/rollback de varios
brazos y la rotación de empates están cubiertas por el modelo independiente y
la conformance; esta guía no inventa una API de selección ni afirma que el
bridge nativo privado publique `select`.

La comprobación reproducible es:

~~~text
scripts/stdlib-channel-doc-check.sh
cargo run -q -p tondo-cli --locked -- run tests/runtime/m11-std-channel-doc-001.to
channel-doc-ok
~~~

El checker valida el documento, el registro
[`testing/stdlib-channel.json`](../../testing/stdlib-channel.json), las cinco
funciones del fixture y los sidecars (`exit = 0`, stdout exacto
`channel-doc-ok`). También exige que el siguiente bloque sea
`STD-EXEC-IMPL-001`. Esta evidencia es una guía ejecutable de la VM hosted; no
promociona símbolos runtime, un layout ABI ni lowering AOT.

## Reliability model and fuzzing

STD-CHANNEL-TEST-001 is closed at the independent model and regression
boundary. The complete testing contract is
[testing/stdlib-channel-test.json](../../testing/stdlib-channel-test.json) and
its executable explanation is
[docs/contracts/stdlib-channel-test.md](./stdlib-channel-test.md).

The bounded reference model tracks endpoint identity, FIFO waiter registration,
select prepare/rollback/commit, cancellation, terminal close, an affine payload
ledger and one wakeup per completed waiter. It covers rendezvous, finite
backpressure, explicit unbounded resource limits, simultaneous readiness,
multiple producers and consumers, abandoned operations and structured cleanup.
The integration suite replays 4096 deterministic seeds and reruns the existing
hosted VM and private native ABI regressions. The libFuzzer target caps each
input at 4096 bytes and 512 transitions; the observed smoke completed 128 runs
with seed 4104.

This evidence proves model robustness and the current hosted/native regression
boundary only. It does not change native_aot_lowering: not-claimed or
public_api_promoted: false, and it does not replace the separate PERF or CONF
leaves. `STD-CHANNEL-PERF-001`, `STD-CHANNEL-CONF-001` and the executable
documentation leaf `STD-CHANNEL-DOC-001` are now closed.

## VM/native conformance

`STD-CHANNEL-CONF-001` closes the shared observable corpus in
[`testing/stdlib-channel-conformance.json`](../../testing/stdlib-channel-conformance.json)
and [`stdlib-channel-conformance.md`](./stdlib-channel-conformance.md). A
fresh hosted VM process emits eight ordered case lines, including bounded FIFO,
rendezvous wakeup, terminal drain, closed/error payloads, invalid capacity,
hosted `select`, close wakeup and deferred panic cleanup. A fresh native
process runs the same case IDs through the private opaque endpoint ABI and
checks normalized result tags, payload preservation, waiter cleanup and zero
live objects.

The native bridge does not expose a select API, so the `select-commit` case
records that target boundary while the hosted implementation and scheduler
tests prove the core prepare/commit/rollback path. The separate panic fixture
checks exit 101 and that `defer receiver.close()` emits its cleanup marker
before propagation. This conformance evidence covers the host target and
private ABI only; it does not claim Cranelift AOT lowering, a public FFI layout
or native fast paths.

## Performance baseline

`STD-CHANNEL-PERF-001` records the scheduler-owned hosted VM baseline in
[`testing/stdlib-channel-performance.json`](../../testing/stdlib-channel-performance.json)
and [`stdlib-channel-performance.md`](./stdlib-channel-performance.md). The
probe measures nine explicit 1:1, n:1 and n:m workloads with rendezvous,
buffered, unbounded, backpressure and close-wakeup behavior. Three warmups and
nine repetitions in each of three independent processes produce 27 monotonic
samples per workload, including median, tail latency, throughput, logical
memory, queue peak, backpressure, wakeups and cleanup counters.

The report is target-qualified to `tondo-vm-hosted` / `bytecode-vm`. Its
logical memory excludes allocator overhead and RSS, and its fixture is outside
timed latency. The independent bounded channel model runs before the probe;
the probe additionally checks FIFO commit order, intact affine payloads,
one-wakeup-per-waiter and zero live endpoints after cleanup. Native runtime
contention, native AOT lowering and algorithmic fast paths remain unmeasured:
`native AOT` is `not-claimed`, and fast paths are deferred to a comparable
native-targeted campaign. This baseline is evidence, not a public API
promotion.

## Eventos privados de diagnóstico

La implementación puede emitir en el namespace `std.channel` los eventos
`channel.create`, `channel.sender.fork`, `channel.receiver.fork`,
`channel.send.prepare`, `channel.send.commit`, `channel.send.rollback`,
`channel.receive.prepare`, `channel.receive.commit`, `channel.receive.rollback`,
`channel.sender.close`, `channel.receiver.close`, `channel.wake` y
`channel.drain`. Cada registro lleva como
mínimo `run_id`, `task_id`, `channel_id`, `endpoint_id`, `event_sequence`,
`state`, `capacity`, `queued` y `source_revision`. Los payloads se omiten por
defecto y los hooks no son una API pública; `DIAG-RUNTIME-001` los consume para
diagnóstico estructurado.

## Exclusiones y promoción

El contrato excluye `sendAsync`/`receiveAsync`, `waitSend`/`waitReceive`,
`AsyncChannel`, pollers, callbacks, builders de selección, una cola ilimitada
por defecto, clones implícitos, un executor propio y cualquier API pública de
`select`. La keyword núcleo sigue siendo la única representación de selección.

La guía ejecutable de `STD-CHANNEL-DOC-001` queda cerrada: fija orden, cierre,
cancelación, fairness, costes y cinco composiciones con fixture y sidecars
verificados. El contrato ya puede alimentar `DIAG-RUNTIME-001`, pero no
promociona símbolos runtime ni lowering AOT. El siguiente bloque secuencial es
`STD-EXEC-IMPL-001`, que deberá reutilizar Group, async estructurado y channels
sin crear un segundo tipo `Task` público.
