# Contrato de `std.executor`

**Estado:** contrato `contract-locked` para STD-0.1B, cerrado por
`STD-EXEC-001`. El registro machine-readable está en
[`testing/stdlib-executor.json`](../../testing/stdlib-executor.json) y la
superficie común se enlaza desde
[`TONDO_STANDARD_LIBRARY_SPEC.md`](../../TONDO_STANDARD_LIBRARY_SPEC.md).
Este cierre fija la política y la frontera de ejecución; no afirma que el
runtime público de pools o actores esté promocionado.

La frontera HOST tiene estado `verified-hosted-and-target-qualified-native-bridge`;
la observación de implementación queda registrada como
`verified-hosted-blocking-and-native-token-bridge`:
la VM hosted ejecuta jobs bloqueantes en workers aislados y el runtime nativo
expone únicamente un bridge privado de tokens para `x86_64-unknown-linux-gnu`.
La API pública, el layout ABI y el lowering AOT genérico siguen sin
promocionarse.

`std.executor` no añade un segundo modelo async. `scope`, `spawn`,
`spawn thread`, `Join`, `Group`, `std.channel` y `std.sync` siguen siendo las
únicas primitivas de tareas, espera, comunicación y memoria. El módulo añade
admisión acotada, ciclo de vida de pools, actores y una frontera explícita para
trabajo bloqueante. Ningún constructor crea un executor global ni hereda
capabilities del ambiente.

La guía ejecutable de este contrato queda cerrada por `STD-EXEC-DOC-001`. Sus
cinco composiciones se mantienen en
`tests/runtime/m11-std-executor-doc-001.to`, con salida esperada
`executor-doc-ok`; el checker documental reproduce el proyecto temporal con
las capabilities declaradas por el fixture y comprueba también sus sidecars.

## Scopes y pools

Un pool vive dentro del scope que posee su obligación terminal. La forma
canónica es crear el pool, admitir los jobs, consumir cada `Join` y cerrar o
cancelar el pool antes de abandonar el scope. `scope` no oculta el drain:
simplemente hace visible al compilador qué handles, jobs y pools deben quedar
consumidos. Un pool que va a sobrevivir al scope se transfiere de forma
explícita; no se copia ni se descarta implícitamente.

`submit` espera una plaza y devuelve el `Join` existente; `trySubmit` solo
observa la capacidad inmediata. Las dos operaciones usan la misma admisión y
la misma semántica de ownership, por lo que un caller no necesita una familia
paralela de APIs async. El pool cooperativo ejecuta funciones Tondo mediante
el scheduler ya existente; `blockingPool` es la única frontera que reserva
workers host y exige `threads`.

## Frontera de implementación observada (`STD-EXEC-IMPL-001`)

La implementación actual se registra como
`verified-hosted-cooperative-pool-and-actors`. La VM hosted verifica la ruta compiler → HIR
→ MIR → scheduler para el pool cooperativo: valida workers/capacity, aplica
`trySubmit` inmediato, backpressure FIFO de `submit`, materializa el `Join`
existente y drena `shutdown`/`cancel` con cancelación cooperativa. La fixture
[`tests/runtime/m11-std-executor-impl-001.to`](../../tests/runtime/m11-std-executor-impl-001.to)
termina con `executor-ok` y exit `0`; su `.capabilities` sidecar declara
explícitamente `threads`; el informe reproducible se escribe en
`target/reliability/evidence/stdlib-executor-implementation.json`.

Este cierre cubre `STD-EXEC-IMPL-001` para la implementación cooperativa hosted;
la implementación de `BlockingPool` queda cerrada por `STD-EXEC-HOST-001`, sin
promover la API pública. `Pool.actor` crea y conserva
un handle afín. La VM hosted ejecuta el handler como una task interna del
pool, entrega los mensajes en orden FIFO de uno en uno, conserva el estado
mutable al retorno normal y hace terminal al actor cuando el handler devuelve
`E`; `Actor.stop` cierra la mailbox, cancela cooperativamente el handler y
espera su drain. `Actor.ref(ref self)` es la operación explícita y no
consumidora que proyecta ese owner a un `ActorRef[M]`; conserva únicamente la
identidad del actor, no copia estado ni mailbox, y no introduce una conversión
implícita. La VM hosted también verifica la ruta transaccional de
`ActorRef.send` como brazo `selectable`: `prepare` observa el mensaje sin
mutarlo, `commit` linealiza una sola admisión en la mailbox y `rollback`
desregistra el waiter sin consumir el payload. La misma regla se aplica a un
`fs.File` afín y a un brazo `else`; el lowering native AOT de callables no se
promociona por esta observación.

`blockingPool` ya tiene una implementación hosted verificable: cada job se
ejecuta en un worker del sistema operativo con un `Engine` hijo, heap y
adaptador host propios; la tarea cooperativa sólo espera la admisión y el
envelope de resultado. El bridge conserva FIFO, límites, cancelación segura,
shutdown con drain y ausencia de handles/loans del VM en la frontera. En el
runtime nativo, la lane promovida para `x86_64-unknown-linux-gnu` transporta
tokens opacos a workers acotados y prueba admisión, wakeups y lifecycle, pero no
ejecuta callbacks ni define todavía el lowering AOT de callables. No se
promociona API pública ni ABI de layout y `public_api_promoted` permanece
`false`.

## Hardening observable (`STD-EXEC-TEST-001`)

The testing owner is recorded in
[`testing/stdlib-executor-test.json`](../../testing/stdlib-executor-test.json).
Its independent bounded model covers worker limits, FIFO admission, deterministic
round-robin assignment, backpressure, graceful shutdown, cancellation, duplicate
completion races, declared errors, panics, actor mailbox serialization, dead
actors, and exact payload cleanup. The model consumes at most 8 workers, 16 queue
slots, 64 jobs or actor messages, 4 KiB of fuzz input, and 1,024 transitions per
replay; the 4,096-seed campaign must produce the same snapshot on two replays.

The hosted VM adds a real-worker stress case with four workers and 32 admitted
jobs. It asserts sequential admission identifiers, one result envelope per job,
and `Closed` only after graceful drain. The nightly fuzz smoke uses the same
bounded state machine and corpus, and fails on a panic, an invariant violation,
non-terminal work, or a replay divergence. These tests harden the observed hosted
bridge and do not promote a public executor API, native callable lowering, or a
native layout ABI.

## VM/native conformance (`STD-EXEC-CONF-001`)

The shared conformance contract is recorded in
[`testing/stdlib-executor-conformance.json`](../../testing/stdlib-executor-conformance.json)
and its executable explanation in
[`stdlib-executor-conformance.md`](./stdlib-executor-conformance.md). Eight
case IDs are replayed on the hosted VM and on the private native bridge. The
VM fixture runs from a temporary project manifest that explicitly declares
`threads`; the repository fixture carries the same declaration in its
`.capabilities` sidecar, and the standalone source path does not inherit that
capability.

The corpus observes bounded pool admission and saturation, blocking result
transfer, safe cancellation and drain, actor FIFO and terminal errors, the
static capability boundary, and the native AOT boundary. Native observations
for cooperative pools and actors are marked `delegated` because no public
native ABI exists for them. This is an explicit target boundary, not a hidden
stub. The native token lane checks worker lifecycle, managed payload ownership,
safe cancellation and zero live handles on `x86_64-unknown-linux-gnu`.

The missing-capability fixture and driver test require `E1008` for
`executor.blockingPool` without an explicit `threads` target capability. The
conformance report contains source and input hashes, exact VM lines, normalized
native observations, cleanup status and no physical paths, addresses, process
IDs or timestamps. Native AOT callable lowering remains `not-claimed`.

## Superficie pública

```tondo
pub type Pool
pub type BlockingPool
pub type Actor[S, M, E]
pub type ActorRef[M]

pub enum ExecutorError { InvalidWorkers, InvalidCapacity, ResourceLimit, CapabilityMissing }
pub enum SubmitError { Saturated, Closed, Cancelled, ResourceLimit }
pub enum ActorSendError[M] { Saturated(M), Closed(M), Cancelled(M), Terminated(M), ResourceLimit(M) }

pub fn pool(workers: Int, capacity: Int): Pool ! ExecutorError
pub fn blockingPool(workers: Int, capacity: Int): BlockingPool ! ExecutorError

pub fn Pool.submit[T, E](ref self, job: fn(): T ! E suspends): Join[T, E] ! SubmitError suspends
pub fn Pool.trySubmit[T, E](ref self, job: fn(): T ! E suspends): Join[T, E] ! SubmitError
pub fn Pool.actor[S: Send + Discard, M: Send + Discard, E](ref self, state: S, capacity: Int, step: fn(mut S, M): Unit ! E suspends): Actor[S, M, E] ! ExecutorError
pub fn Actor.ref(ref self): ActorRef[M]
pub fn Pool.shutdown(self): Unit suspends
pub fn Pool.cancel(self): Unit suspends

pub fn ActorRef.send(ref self, message: M): Unit ! ActorSendError[M] selectable
pub fn ActorRef.trySend(ref self, message: M): Unit ! ActorSendError[M]
pub fn Actor.stop(self): Unit ! E suspends

pub fn BlockingPool.run[T, E](ref self, job: fn(): T ! E): T ! E suspends
pub fn BlockingPool.shutdown(self): Unit suspends
pub fn BlockingPool.cancel(self): Unit suspends
```

`pool` crea un pool cooperativo acotado. `workers` debe ser positivo y
`capacity` no negativo; la capacidad cuenta trabajos admitidos que todavía no
han terminado, incluidos los que esperan en la cola. Un pool vacío no implica
un scheduler global: el pool solo admite jobs y los ejecuta mediante el
scheduler Tondo existente. `submit` espera implícitamente hasta que haya
admisión o hasta que el pool se cierre o se cancele. `trySubmit` nunca espera y
devuelve `Saturated` cuando no hay una plaza inmediata. El `Join[T, E]`
devuelto es el handle afín ya definido por el lenguaje y se consume con
`await`; no se publica `Task`, `Future` ni otro resultado pendiente.

El job se transfiere únicamente al hacer commit de la admisión. Si la
admisión falla o se cancela antes del commit, el job permanece propiedad del
caller según la regla transaccional de argumentos afines. Un pool no puede
descartarse implícitamente: debe consumirse con `shutdown`, `cancel` o
transferirse a un scope que mantenga su obligación terminal.

`blockingPool` crea una cola separada de workers del sistema operativo y exige
la capability `threads`. En la VM hosted, `run` recibe una función no
suspendible: no puede llamar operaciones `suspends`, `spawn`, `select` ni
capturar préstamos locales. La llamada pública sí es suspendible porque espera
la admisión y el resultado sin bloquear el worker cooperativo. Se puede
escribir `spawn blocking_pool.run(job)` para solapar varias operaciones; el
resultado sigue siendo un `Join` normal cuando se hace spawn. El runtime nativo
mantiene una lane equivalente de tokens, target-qualified y privada; no
pretende ser todavía el lowering native AOT de ese callable. `blockingPool` no puede
matar un job bloqueado de forma forzosa: `cancel` cancela lo que aún no empezó
y espera la finalización segura de los jobs host que ya están en ejecución.

Los dos tipos de pool son deliberadamente distintos. Esto no duplica una API
síncrona/async: separa una política cooperativa que ejecuta Tondo de una
frontera host que puede bloquear físicamente. No existe una conversión
implícita entre ambos ni un fallback que ejecute trabajo bloqueante dentro de
un worker cooperativo.

## Capacidad, saturación y errores

Los límites son finitos y se validan antes de reservar estado. `workers <= 0`,
`capacity < 0` y cualquier combinación que exceda los límites del runtime
devuelven `InvalidWorkers`, `InvalidCapacity` o `ResourceLimit` sin crear un
pool parcial. La capacidad cero es válida: permite únicamente los trabajos
que puedan tomar un worker inmediatamente y mantiene una cola de espera
vacía. No existe una cola ilimitada por defecto.

`submit` es la operación con backpressure. `trySubmit` es el único probe
inmediato y no se usa para implementar una segunda familia de funciones
`submitAsync`. Tras `shutdown`, todas las nuevas admisiones devuelven `Closed`;
tras `cancel`, los waiters de admisión reciben `Cancelled`. Un rechazo nunca
consume silenciosamente un job.

El pool mantiene como máximo `workers` jobs en ejecución. Los jobs admitidos
pero todavía no ejecutados conservan su orden FIFO de admisión, salvo que el
contrato de actor indique el orden de su mailbox. El scheduler puede rotar
workers para evitar starvation, pero no puede cambiar la semántica observable
de `Join`, los errores o el cleanup. Saturación no es un error del programa
ejecutado y no se transforma en un panic.

## Actores y mailboxes

`Pool.actor` crea un owner de estado y una mailbox acotada. `step` recibe el
estado mutable y un mensaje por vez; el actor procesa como máximo un mensaje
simultáneamente y conserva el orden FIFO de commit de la mailbox. `S` y `M`
son `Send + Discard` para que cancelación, shutdown y panic puedan drenar el
estado y los mensajes sin perder ownership. El handler puede suspenderse, pero
no crea un executor ni un task detached.

`ActorRef[M]` es un handle `Copy + Send + Share` a la identidad del actor.
`Actor.ref(ref self)` lo obtiene mediante un préstamo compartido no consumidor;
copiar el handle no copia estado ni mailbox. `send` aplica backpressure y es
`selectable`; `trySend` es inmediato. En un brazo perdedor de `select` no se
mueve el mensaje: el registro conserva una reserva del payload, el único brazo
ganador lo mueve al commit y la selección `else`, la cancelación o un perdedor
desregistran el waiter y dejan el valor en el binding del caller. Un error de
envío devuelve el mensaje en la variante correspondiente (`Saturated`, `Closed`,
`Cancelled`, `Terminated` o `ResourceLimit`). Esta garantía está observada en
la VM hosted cooperativa; no afirma una implementación equivalente en AOT.

El actor es afín y debe terminarse con `stop`. `stop` cierra la mailbox,
solicita cancelación cooperativa, drena el cleanup y espera la finalización del
handler. Los mensajes pendientes se descartan solo porque `M: Discard`. Si el
handler devuelve `E`, el actor se vuelve terminal y `stop` propaga ese error
después del drain; un panic se propaga según el protocolo normal de unwind.
Después de la terminalización, los refs existentes solo pueden observar
`Terminated(message)` y no pueden reactivar el actor.

Un actor no es una alternativa a `Group`: para fan-out/fan-in se usan
`spawn` + `Group`; para productor/consumidor con backpressure se usa
`std.channel`; el actor solo encapsula estado serializado detrás de una
mailbox.

## Shutdown, cancelación y scopes

`shutdown` es graceful: cierra la admisión, deja terminar los jobs aceptados,
drena actores y workers y solo entonces vuelve. `cancel` cierra la admisión,
solicita cancelación a jobs cooperativos y actores, drena todos los cleanups y
espera a los workers. Ninguna operación publica antes de que termine el drain.
Un job cooperativo que no alcanza un punto de suspensión puede retrasar la
cancelación; esto es observable y no se resuelve matando tasks.

Salir de un scope con un pool vivo es un error estático salvo que el pool se
transfiera. `shutdown` y `cancel` son terminales, no idempotentes, y consumir
el pool dos veces es error estático. Los `Join` devueltos por `submit` conservan
su obligación ordinaria: el caller debe esperarlos, cancelarlos, transferirlos
o detached antes de abandonar su scope, incluso si el pool ya completó su
drain.

La cancelación de un `BlockingPool` no interrumpe una llamada host arbitraria.
Los jobs que todavía están en cola no empiezan; los que ya están en un worker
terminan de forma normal o devuelven su error host. El pool espera ambos casos
antes de publicar éxito de `cancel`.

## Costes y límites

La ruta rápida de admisión solo comprueba el estado del pool y reserva un slot
del límite finito; su coste lógico es constante por intento. La cola conserva
como máximo `capacity` trabajos admitidos después de los slots activos, y una
admisión bloqueante puede suspender al caller mientras espera una plaza o un
estado terminal. `trySubmit` no realiza polling oculto: un pool saturado
devuelve inmediatamente `SubmitError.Saturated`.

Cada job admitido conserva un envelope y un `Join` hasta su consumo. El coste
de scheduling, wakeups y cleanup crece con el número de jobs y mensajes, no
con una cola ambiental ilimitada. Una mailbox de actor tiene el mismo límite
finito, mantiene FIFO por commit y ejecuta un handler cada vez; el mensaje se
retiene hasta la linealización o se devuelve dentro del error. Las cifras
físicas de threads, latencia, RSS y allocator no forman parte de este
contrato: el presupuesto reproducible de rendimiento está separado por target
en `stdlib-executor-performance.md`.

`BlockingPool` añade el coste de una reserva y un worker host acotado por job;
la llamada cooperativa paga además la espera del envelope de resultado. La
cancelación no puede ahorrar ese coste para una llamada ya iniciada: espera su
retorno seguro y solo evita iniciar trabajos todavía en cola. Los límites
inrepresentables fallan con `ResourceLimit` antes de reservar estado.

Estas son cotas lógicas y reglas de ownership, no una promesa de tiempos
deterministas ni una afirmación de lowering native AOT. La evidencia observada
es VM hosted y una lane privada target-qualified de tokens nativos; el ABI de
layout y la API pública continúan `not-claimed`.

## Ejemplos ejecutables de composición

Los siguientes patrones son las cinco familias mínimas que cubre la fixture
documental. Cada ejemplo usa la misma superficie canónica y termina sus
consumidores terminales; no introduce wrappers sync/async duplicados.

### `scoped-join`

`scoped_join` mantiene el pool dentro de un `scope`, espera el `Join` y luego
consume el pool con `shutdown`:

```tondo
fn scoped_join(): !(executor.ExecutorError | executor.SubmitError) {
    let pool = executor.pool(1, 1)?
    scope {
        let job = pool.submit(compute)?
        assert(await job == 42)
    }
    pool.shutdown()
}
```

### `bounded-backpressure`

`bounded_backpressure` llena de forma deliberada un pool `1, 1`. El segundo
`trySubmit` debe fallar con `Saturated`; el primer job se espera antes del
drain y no se pierde ningún payload:

```tondo
fn bounded_backpressure(): !(executor.ExecutorError | executor.SubmitError | time.ClockError) suspends {
    let pool = executor.pool(1, 1)?
    scope {
        let first = pool.trySubmit(blocked)?
        match pool.trySubmit(compute) {
            ok(_) => panic("executor documentation accepted saturated work")
            err(executor.SubmitError.Saturated) => {}
            err(error) => panic("unexpected admission error: {error}")
        }
        assert(await first? == 42)
    }
    pool.shutdown()
}
```

### `actor-mailbox`

`actor_mailbox` proyecta explícitamente un `ActorRef`, envía dos mensajes en
orden y termina el owner con `stop`; el handler conserva el estado serializado
de la mailbox:

```tondo
fn actor_mailbox(): !(executor.ExecutorError | executor.ActorSendError[Int]) {
    let pool = executor.pool(1, 2)?
    let actor = pool.actor(0, 2, actor_step)?
    let actor_ref = actor.ref()
    actor_ref.trySend(1)?
    actor_ref.trySend(2)?
    actor.stop()?
    pool.shutdown()
}
```

### `blocking-bridge`

`blocking_bridge` usa la capability `threads` y el `spawn` ordinario para
solapar dos llamadas no suspendibles. El resultado sigue siendo un `Join`
normal y el pool bloqueante se cierra después del scope:

```tondo
fn blocking_bridge(): !(executor.ExecutorError | executor.SubmitError) {
    let pool = executor.blockingPool(1, 1)?
    scope {
        let first = spawn pool.run(blocking_compute)
        let second = spawn pool.run(blocking_compute)
        assert(await first == 42)
        assert(await second == 42)
    }
    pool.shutdown()
}
```

### `cancel-and-drain`

`cancel_and_drain` muestra el consumidor terminal alternativo: la cancelación
rechaza nuevas admisiones, solicita cancelación cooperativa y espera el drain
antes de abandonar el scope:

```tondo
fn cancel_and_drain(): !(executor.ExecutorError | executor.SubmitError) {
    let pool = executor.pool(1, 1)?
    scope {
        let _job = pool.trySubmit(compute)?
        pool.cancel()
        return
    }
}
```

La fixture completa combina esas cinco funciones, imprime una única línea
`executor-doc-ok` y termina con exit `0`. Se puede reproducir con
`scripts/stdlib-executor-doc-check.sh`; la comprobación valida además
`testing/stdlib-executor.json`, los contratos de rendimiento y conformance,
los sidecars `.stdout`, `.exit` y `.capabilities`, y la presencia de las
capabilities `clock` y `threads` en el proyecto temporal.

## Capabilities y frontera host

El pool cooperativo no requiere `threads`. `blockingPool` requiere la
capability `threads` en el target y falla estáticamente si no está disponible;
no consulta environment, número de CPUs, variables del proceso ni scheduler
host para elegir defaults. Los números de workers y capacidad son argumentos
del programa o constantes explícitas del caller.

El bridge hosted solo cruza valores que cumplan las reglas de `Send` y no deja
préstamos, handles de VM, `Ref` locales ni `Pointer` unsafe. Cada worker crea un
heap hijo y devuelve un envelope tipado; las llamadas host se serializan a
través del owner de la VM y observan cancelación sin bloquear un worker
cooperativo. La lane nativa intercambia exclusivamente tokens opacos y no
ofrece callbacks, punteros ni layout ABI. Ninguna de las dos superficies
publica prioridad o identidad física de worker.

## Eventos privados de diagnóstico

El runtime puede emitir en `std.executor` los eventos
`pool.create`, `pool.submit.wait`, `pool.submit.accept`, `pool.submit.reject`,
`pool.worker.start`, `pool.worker.idle`, `pool.worker.stop`, `pool.shutdown`,
`pool.cancel`, `blocking.submit`, `blocking.start`, `blocking.finish`,
`actor.create`, `actor.send.prepare`, `actor.send.commit`,
`actor.send.rollback`, `actor.message.start`, `actor.message.finish` y
`actor.terminate`. Cada registro lleva como mínimo `run_id`, `task_id`,
`pool_id`, `worker_id`, `operation_id`, `event_sequence`, `state` y
`source_revision`; los mensajes y resultados de usuario se omiten por defecto.
Son hooks privados consumidos por `DIAG-RUNTIME-001`, no una API pública de
instrumentación ni una garantía de que el runtime ya los emita.

## Exclusiones y promoción

El contrato excluye `Task`, `Future`, `AsyncPool`, `ExecutorHandle`, executor
global, defaults ambientales, cola ilimitada, force-kill, polling público,
callbacks de completion, prioridades públicas, work-stealing observable,
`submitAsync`, `runAsync`, `waitAsync`, conversión implícita a thread y
fallback bloqueante dentro del scheduler cooperativo.

`STD-EXEC-HOST-001` queda cerrado para la implementación hosted y el bridge
nativo target-qualified descritos arriba. La hardening de comportamiento,
rendimiento, conformance y documentación de uso queda registrada en
`testing/stdlib-executor-performance.json` y
`docs/contracts/stdlib-executor-performance.md` para `STD-EXEC-PERF-001`;
`STD-EXEC-CONF-001` está cerrado en
`testing/stdlib-executor-conformance.json`; `STD-EXEC-DOC-001` queda cerrado
por esta guía y su fixture ejecutable. El siguiente bloque owner es
`DIAG-RUNTIME-001`.
El lowering AOT de callables sigue `not-claimed`. Los contratos runtime-facing de `std.executor`, `std.net`
y `std.time` civil ya están cerrados; `DIAG-RUNTIME-001` puede comenzar cuando
se abra la compuerta de diagnóstico; el contrato `std.log` ya está cerrado y
sus leaves siguen la compuerta nativa.
