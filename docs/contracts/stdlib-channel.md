# Contrato de `std.channel`

**Estado:** contrato `contract-locked` para STD-0.1B, cerrado por
`STD-CONC-001`. El registro machine-readable está en
[`testing/stdlib-channel.json`](../../testing/stdlib-channel.json) y la
superficie canónica completa en
[`TONDO_STANDARD_LIBRARY_SPEC.md`](../../TONDO_STANDARD_LIBRARY_SPEC.md).
Este cierre fija la semántica; no afirma que el runtime público exista todavía.

`std.channel` es la única abstracción de productor/consumidor con backpressure
de Tondo. Reutiliza el scheduler y la expresión núcleo `select`, no crea un
segundo tipo `Task`/`Future`, no añade `std.async.select` y no introduce
capabilities ambientales. La implementación necesita primitivas de wakeup,
estado atómico y almacenamiento de VM y backend nativo, por lo que su frontera
`HOST` queda pendiente de `NATIVE-001` y de `STD-CHANNEL-IMPL-001`
(`required-after-native-gate`).

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

Para valores afines se usa `receive` y `Receiver.close` explícitamente, de modo
que el caller puede recuperar los mensajes pendientes en vez de perder
ownership. No existe `for await`, `AsyncChannel`, materialización automática ni
un `collect` específico del canal.

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

La implementación pública permanece pendiente de
`STD-CHANNEL-IMPL-001`, `STD-CHANNEL-ASYNC-ITER-001`, su modelo y fuzzing,
presupuestos de rendimiento, conformidad VM/nativa y documentación ejecutable.
El contrato ya puede alimentar `DIAG-RUNTIME-001`, pero no promociona símbolos
runtime antes de cerrar esas leaves.
