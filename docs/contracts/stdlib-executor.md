# Contrato de `std.executor`

**Estado:** contrato `contract-locked` para STD-0.1B, cerrado por
`STD-EXEC-001`. El registro machine-readable está en
[`testing/stdlib-executor.json`](../../testing/stdlib-executor.json) y la
superficie común se enlaza desde
[`TONDO_STANDARD_LIBRARY_SPEC.md`](../../TONDO_STANDARD_LIBRARY_SPEC.md).
Este cierre fija la política y la frontera de ejecución; no afirma que el
runtime público de pools o actores esté implementado.

La frontera HOST tiene estado `required-after-native-gate`: el contrato puede
cerrarse ahora para alimentar el diseño del runtime, pero sus workers y
adaptadores no se promocionan antes del backend nativo.

`std.executor` no añade un segundo modelo async. `scope`, `spawn`,
`spawn thread`, `Join`, `Group`, `std.channel` y `std.sync` siguen siendo las
únicas primitivas de tareas, espera, comunicación y memoria. El módulo añade
admisión acotada, ciclo de vida de pools, actores y una frontera explícita para
trabajo bloqueante. Ningún constructor crea un executor global ni hereda
capabilities del ambiente.

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
la capability `threads`. `run` recibe una función no suspendible: no puede
llamar operaciones `suspends`, `spawn`, `select` ni capturar préstamos locales.
La llamada pública sí es suspendible porque espera la admisión y el resultado
sin bloquear el worker cooperativo. Se puede escribir `spawn
blocking_pool.run(job)` para solapar varias operaciones; el resultado sigue
siendo un `Join` normal cuando se hace spawn. `blockingPool` no puede matar un
job bloqueado de forma forzosa: `cancel` cancela lo que aún no empezó y espera
la finalización segura de los jobs host que ya están en ejecución.

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

`ActorRef[M]` es un handle `Copy + Send + Share` a la identidad del actor; copiar
el handle no copia estado ni mailbox. `send` aplica backpressure y es
`selectable`; `trySend` es inmediato. En un brazo perdedor de `select` no se
mueve el mensaje. Un error de envío devuelve el mensaje en la variante
correspondiente (`Saturated`, `Closed`, `Cancelled`, `Terminated` o
`ResourceLimit`).

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

## Capabilities y frontera host

El pool cooperativo no requiere `threads`. `blockingPool` requiere la
capability `threads` en el target y falla estáticamente si no está disponible;
no consulta environment, número de CPUs, variables del proceso ni scheduler
host para elegir defaults. Los números de workers y capacidad son argumentos
del programa o constantes explícitas del caller.

El bridge host solo cruza valores que cumplan las reglas de `Send` y no deja
préstamos, handles de VM, `Ref` locales ni `Pointer` unsafe sin el contrato
explícito correspondiente. La frontera convierte panics, cancelación y errores
host al envelope normal del runtime y libera sus reservas exactamente una vez.
No se ofrece FFI, ABI de layout ni prioridad de worker pública en este bloque.

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

La implementación queda pendiente de
`STD-EXEC-IMPL-001`, `STD-EXEC-HOST-001`, `STD-EXEC-TEST-001`,
`STD-EXEC-PERF-001`, `STD-EXEC-CONF-001` y `STD-EXEC-DOC-001`. Los contratos
runtime-facing de `std.executor`, `std.net` y `std.time` civil ya están cerrados;
`DIAG-RUNTIME-001` puede comenzar cuando se abra la compuerta de diagnóstico,
mientras el siguiente contrato de la lane de stdlib es `STD-LOG-001`.
