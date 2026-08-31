
# Contrato de std.sync

Estado: contract-locked para Tondo 0.1, cerrado por STD-SYNC-001. El registro
machine-readable está en testing/stdlib-sync.json y la descripción integrada en
TONDO_STANDARD_LIBRARY_SPEC.md. La superficie de compilador, el parking
cooperativo del host y el puente ABI nativo de atomics/señales están verificados
por `STD-SYNC-HOST-001`. La ejecución de un initializer `Once` como continuación
de VM, su publicación, el despertar de waiters y la limpieza en error, pánico o
cancelación están verificados por `STD-SYNC-TEST-001`. El frontend de los cinco
literales cualificados de colecciones está cerrado por
`STD-SYNC-COLLECTION-FRONTEND-001`; su contrato es
[`testing/stdlib-sync-collection-frontend.json`](../../testing/stdlib-sync-collection-frontend.json)
y su guía normativa [`stdlib-sync-collection-frontend.md`](./stdlib-sync-collection-frontend.md).
La implementación de esos handles para el modelo hosted y el ABI nativo privado
está cerrada por `STD-SYNC-COLLECTION-IMPL-001`; su registro es
[`testing/stdlib-sync-collection.json`](../../testing/stdlib-sync-collection.json)
y su contrato [`stdlib-sync-collection.md`](./stdlib-sync-collection.md). La
iteración directa está cerrada para el VM hosted y el ABI nativo privado por
`STD-SYNC-COLLECTION-ITER-001`; su registro es
[`testing/stdlib-sync-collection-iter.json`](../../testing/stdlib-sync-collection-iter.json)
y su contrato [`stdlib-sync-collection-iter.md`](./stdlib-sync-collection-iter.md).

Implementación host: `scheduler-backed-hosted-model` con puente
`verified-host-parking-native-atomic-epoch-bridge`.
La continuación de initializer está marcada como
`verified-vm-continuation-and-cleanup`.

std.sync es la superficie de memoria compartida de Tondo. Reutiliza el único
modelo de suspensión implícita, spawn, scope, Join y defer; no crea una familia
async, Task, Future ni un scheduler paralelo. Una operación suspends espera
directamente cuando se llama de forma ordinaria y solo puede solaparse si el
programa escribe explícitamente spawn. Ninguna operación de este módulo es
selectable: la selección y el backpressure pertenecen a select y std.channel.

La implementación hosted y la nativa deben aparcar y despertar tasks sin
bloquear un worker cooperativo. La capacidad threads solo es necesaria para
cruzar una frontera de thread del sistema operativo; la ausencia de esa
capacidad produce un diagnóstico estático y nunca una simulación silenciosa de
ejecución cross-thread.

## Estado de implementación

El compilador registra `std.sync` como módulo bootstrap, resuelve sus nominales,
comprueba bounds/efectos y baja llamadas a la identidad host estable. El host de
referencia mantiene estado de mutexes, rwlocks, guards, semáforos, permits,
once, barreras y atomics, con cleanup idempotente y errores nominales. La
contención de locks, condiciones, semáforos y barreras se registra en colas FIFO
por recurso y se reintenta desde el scheduler; ningún worker cooperativo se
duerme ni hace spin dentro del host. La identidad lógica de la task se anuncia
en cada entrada al host, de modo que una adquisición reentrante se diagnostica
sin confundirla con contención de otra task.

El runtime nativo expone un puente privado para atomics `u64` con los cinco
órdenes de `MemoryOrder` y para señales de parking basadas en epoch/`Condvar`.
Los handles siguen siendo capacidades opacas y el puente no expone punteros,
layouts ni IDs físicos de threads. La espera bloqueante de la señal solo es
para workers nativos; la VM hosted usa el estado poll/park cooperativo y falla
cerrado ante un conjunto de esperas sin progreso. El lowering nativo de tipos
genéricos y de las colecciones compartidas consumirá este puente en sus hojas
posteriores.

`Once.getOrInit` conserva el valor listo y diagnostica la reentrada. Cuando el
initializer es un closure de VM, el motor instala una `OnceContinuation` en su
frame, publica el valor solo al retornar y despierta todos los waiters con el
mismo resultado. Error declarado, pánico y cancelación limpian el initializer,
restablecen `uninitialized` y permiten reintentar; ningún waiter queda detached
ni observa un valor parcialmente construido.

El frontend de colecciones compartidas ya resuelve por identidad nominal,
preserva la forma lossless `PathExpr + BracketPostfix`, admite aliases de
`std.sync`, vacíos contextuales, `sync.Map[:]`, trailing comma y diagnósticos de
duplicados. El HIR publica únicamente el marcador interno
`std.sync.collectionLiteral`; el verifier lo sella y el lowering MIR lo entrega
al host. `STD-SYNC-COLLECTION-IMPL-001` verifica el runtime
`verified-hosted-vm-and-native-runtime-abi`: el host hosted conserva el orden y
los outcomes mediante jobs listos, y el ABI nativo usa celdas por identidad,
parking epoch, handles opacos, CAS fuerte, límites recuperables y cleanup sin
mantener el lock global durante la espera. No se promociona una API pública ni
se afirma lowering genérico AOT.

## Superficie pública

Tipos normativos:

- pub enum SyncError = { InvalidCapacity, InvalidParties, ResourceLimit, ReentrantLock,
  ReentrantInitialization, Broken }.
- pub type Mutex[T], pub type MutexGuard[T], pub type RwLock[T], pub type ReadGuard[T], pub type WriteGuard[T].
- pub type Condition, pub type Semaphore, pub type Permit, pub type Once[T, E], pub type Barrier.
- pub enum BarrierRole = { Leader, Follower }.
- pub enum MemoryOrder = { Relaxed, Acquire, Release, AcqRel, SeqCst }.
- pub type Atomic[T] y pub type CompareExchange[T] = { Exchanged(T), Mismatch(T) }.
- CollectionError es el error canónico de std.collections.

Superficie de locks y guards:

    pub fn mutex[T: Send](value: T): Mutex[T] ! SyncError
    pub fn Mutex.lock(ref self): MutexGuard[T] ! SyncError suspends
    pub fn Mutex.tryLock(ref self): MutexGuard[T]?
    pub fn MutexGuard.get(ref self): ref T
    pub fn MutexGuard.getMut(mut self): mut T
    pub fn MutexGuard.unlock(self): Unit

    pub fn rwLock[T: Send](value: T): RwLock[T] ! SyncError
    pub fn RwLock.read(ref self): ReadGuard[T] ! SyncError suspends
    pub fn RwLock.tryRead(ref self): ReadGuard[T]?
    pub fn ReadGuard.get(ref self): ref T
    pub fn ReadGuard.unlock(self): Unit
    pub fn RwLock.write(ref self): WriteGuard[T] ! SyncError suspends
    pub fn RwLock.tryWrite(ref self): WriteGuard[T]?
    pub fn WriteGuard.get(ref self): ref T
    pub fn WriteGuard.getMut(mut self): mut T
    pub fn WriteGuard.unlock(self): Unit

Superficie de condición y permits:

    pub fn condition(): Condition ! SyncError
    pub fn Condition.wait[T](ref self, var guard: MutexGuard[T]): MutexGuard[T] suspends
    pub fn Condition.notifyOne(ref self): Unit
    pub fn Condition.notifyAll(ref self): Unit

    pub fn semaphore(capacity: Int): Semaphore ! SyncError
    pub fn Semaphore.acquire(ref self): Permit suspends
    pub fn Semaphore.tryAcquire(ref self): Permit?
    pub fn Permit.release(self): Unit

Superficie de once, barrera y atomics:

    pub fn once[T, E](): Once[T, E]
    pub fn Once.get(ref self): ref T?
    pub fn Once.getOrInit(ref self, init: fn(): T ! E suspends): ref T ! E suspends
    pub fn Once.isReady(ref self): Bool

    pub fn barrier(parties: Int): Barrier ! SyncError
    pub fn Barrier.wait(ref self): BarrierRole ! SyncError suspends

    pub fn atomic[T: Copy + Equatable + Send + Share](value: T): Atomic[T]
    pub fn Atomic.load(ref self, order: MemoryOrder): T
    pub fn Atomic.store(ref self, value: T, order: MemoryOrder): Unit
    pub fn Atomic.swap(ref self, value: T, order: MemoryOrder): T
    pub fn Atomic.compareExchange(ref self, expected: T, desired: T, success: MemoryOrder, failure: MemoryOrder): CompareExchange[T]

Las operaciones sin cuerpo declaran sus efectos. El caller no escribe await ante
lock, read, write, wait, acquire, getOrInit o Barrier.wait: la espera directa es
implícita y await call() produce E1611. Solo un Join conserva la forma explícita
await handle. Los cuerpos que llaman a estas operaciones pueden inferir
suspends; la inferencia no depende del nombre. Ninguna firma publica selectable.

Los órdenes de memoria son argumentos constantes de compilación. Una operación
con un orden incompatible se rechaza estáticamente; no existe un orden débil por
defecto ni una conversión silenciosa. En compareExchange, el orden de fallo no
puede ser Release o AcqRel ni más fuerte que el de éxito.

## Handles, guards y cleanup

Los handles de identidad Mutex, RwLock, Condition, Semaphore, Once, Barrier y
Atomic se pueden copiar sin copiar el estado: cada copia apunta a la misma
identidad sincronizada. Sus capacidades Send y Share se derivan de los bounds
del estado protegido y de la implementación intrínseca. Descartar un handle
solo libera esa referencia; no cierra ni reinicia la identidad mientras queden
handles vivos.

MutexGuard, ReadGuard, WriteGuard y Permit son afines: no son Copy ni Clone.
Satisfacen Discard con cleanup determinista, por lo que su descarte, defer,
cancelación o unwind ejecuta la liberación auto-release-exactly-once. unlock()
y release() son formas explícitas terminales. Usar el guard o permit después de
consumirlo es error estático. Un préstamo devuelto por get() o getMut() no puede
escapar del guard que lo protege.

No existe poisoning implícito. Un pánico libera el guard o permit antes del
unwind y el valor protegido conserva únicamente los invariantes que su propio
tipo y errores nominales definan. Si se desea recordar un fallo de inicialización
se almacena explícitamente un Result en Once; el lock no inyecta un estado de
error oculto.

## Mutex y RwLock

Mutex es no reentrante y exclusivo. lock espera en orden de registro hasta que
puede hacer commit; tryLock no suspende y devuelve none si no puede adquirir
inmediatamente. Una adquisición reentrante de la misma identidad por la task
propietaria devuelve SyncError.ReentrantLock sin mutar el lock. La cancelación
antes del commit desregistra al waiter; la cancelación mientras el guard está
vivo libera el guard antes de abandonar el scope. Bajo contención continuada la
implementación no puede producir starvation permanente ni hacer spin ilimitado.

RwLock permite varios lectores simultáneos o un único escritor. Los escritores
reciben preferencia acotada para impedir starvation, sin prometer un orden global
del scheduler. read/write suspenden; tryRead/tryWrite son inmediatas. No se
publica upgrade ni downgrade: se libera el guard actual y se adquiere el nuevo
de forma explícita. La cancelación sigue la misma regla de desregistro y cleanup.

## Condition variables

Condition.wait recibe un MutexGuard por var. Su protocolo es indivisible:
libera el mutex, registra la espera, duerme hasta una notificación o cancelación y
vuelve a adquirir el mismo mutex antes de devolver el guard. Una cancelación no
puede dejar el mutex libre ni hacer escapar un guard parcialmente consumido.

Las wakeups espurias del host se ocultan con hidden-and-rechecked; el programa
debe escribir el bucle de predicado habitual porque una notificación no prueba
que la condición de negocio siga siendo cierta. notifyOne despierta al waiter
compatible más antiguo y notifyAll despierta todos los waiters registrados.
No existe waitAsync, callback, poller ni condición asociada automáticamente a un
lock diferente.

## Semáforos y permits

semaphore(capacity) exige una capacidad positiva y comienza con exactamente esa
cantidad de permits. Una capacidad cero o negativa devuelve
SyncError.InvalidCapacity; la falta de memoria devuelve ResourceLimit. acquire
espera FIFO hasta hacer commit y tryAcquire devuelve none sin suspender. Cada
Permit representa una unidad y solo puede liberarse una vez. Descartarlo libera
la unidad automáticamente; nunca se puede superar la capacidad ni crear un
permit por una ruta distinta de una adquisición.

Cancelar un waiter lo desregistra antes de consumir una unidad. El runtime
despierta al siguiente waiter después del commit y no bloquea un worker
cooperativo. Backpressure entre productores y consumidores usa std.channel, no
una queue de semáforo oculta.

## Once[T, E]

Una Once empieza uninitialized, admite un único initializer en initializing y
publica una referencia inmutable en ready. Las llamadas concurrentes a
getOrInit se suspenden mientras otro initializer trabaja y reciben la misma
publicación. get es no bloqueante: devuelve none mientras no haya un valor
listo. La referencia queda ligada al handle Once y no puede guardarse ni cruzar
spawn sin las reglas ordinarias de préstamos.

Si el initializer devuelve E, se despiertan los waiters y el estado vuelve a
uninitialized; el caller decide si reintenta. Un pánico o cancelación sigue la
misma ruta después de completar cleanup. Una llamada recursiva del mismo
initializer devuelve SyncError.ReentrantInitialization en vez de deadlock. Una
Once lista no se puede mutar ni reinicializar. Para una política de error
pegajoso se usa explícitamente Once[Result[T, E], Never].

## Barreras

barrier(parties) exige un número positivo y crea generaciones reutilizables.
Cada wait registra una llegada y suspende hasta que llegan todas las partes. La
última llegada recibe BarrierRole.Leader; las demás reciben
BarrierRole.Follower. Al completarse una generación comienza la siguiente sin
arrastrar llegadas anteriores.

Si una task se cancela mientras espera, la generación se marca rota, despierta a
todos sus participantes y las llamadas afectadas reciben SyncError.Broken. No se
transforma una generación incompleta en éxito ni se deja una espera detached.
No existe reset implícito; una nueva barrera expresa una nueva política de
participantes.

## Atomics y orden de memoria

Atomic[T] restringe T a Copy + Equatable + Send + Share. load, store, swap y
compareExchange son operaciones linealizables y no suspenden. El último devuelve
CompareExchange.Exchanged(previous) cuando compara y escribe, o
CompareExchange.Mismatch(observed) cuando no coincide; nunca falla de forma
espuria y el caso mismatch no escribe.

Relaxed, Acquire, Release, AcqRel y SeqCst deben aparecer de forma explícita. El
compilador valida que un load no use Release, que un store no use Acquire o
AcqRel y que el orden de fallo de CAS sea válido y no más fuerte que el de éxito.
Tondo no publica weakCompareExchange, atomic.wait ni atomic.notify; una espera
de cambio se modela con Condition o channel.

## Colecciones compartidas

La sintaxis y resolución del frontend están cerradas por
`STD-SYNC-COLLECTION-FRONTEND-001`; para el contrato detallado y su evidencia
ejecutable véase [`stdlib-sync-collection-frontend.md`](./stdlib-sync-collection-frontend.md).
La implementación de handles y operaciones para hosted/native ABI tiene un
contrato separado en [`stdlib-sync-collection.md`](./stdlib-sync-collection.md)
y un registro en [`testing/stdlib-sync-collection.json`](../../testing/stdlib-sync-collection.json).
La iteración directa tiene su contrato separado en
[`stdlib-sync-collection-iter.md`](./stdlib-sync-collection-iter.md) y su registro
en [`testing/stdlib-sync-collection-iter.json`](../../testing/stdlib-sync-collection-iter.json).
Las operaciones listadas aquí ya se ejecutan en esos dos carriles; el lowering
genérico AOT permanece target-qualified y sigue sus hojas propias.

Las cinco identidades calificadas son:

    sync.Array[T: Copy + Send + Share]
    sync.Map[K: Key + Send + Share, V: Copy + Send + Share]
    sync.Set[K: Key + Send + Share]
    sync.Stack[T: Send + Discard]
    sync.Queue[T: Send + Discard]

La superficie de operaciones es cerrada y no depende de una implementación:

    pub fn sync.Array.length(ref self): Int
    pub fn sync.Array.isEmpty(ref self): Bool
    pub fn sync.Array.get(ref self, index: Int): T? suspends
    pub fn sync.Array.set(ref self, index: Int, value: T): T ! CollectionError suspends
    pub fn sync.Array[T: Copy + Equatable + Send + Share].compareExchange(ref self, index: Int, expected: T, desired: T): CompareExchange[T] ! CollectionError suspends
    pub fn sync.Array.snapshot(ref self): Array[T] ! CollectionError suspends

    pub fn sync.Map.length(ref self): Int
    pub fn sync.Map.isEmpty(ref self): Bool
    pub fn sync.Map.get(ref self, key: K): V? suspends
    pub fn sync.Map.contains(ref self, key: K): Bool suspends
    pub fn sync.Map.insert(ref self, key: K, value: V): V? ! CollectionError suspends
    pub fn sync.Map.remove(ref self, key: K): V? suspends
    pub fn sync.Map[K: Key + Send + Share, V: Copy + Equatable + Send + Share].compareExchange(ref self, key: K, expected: V?, desired: V?): CompareExchange[V?] ! CollectionError suspends
    pub fn sync.Map.snapshot(ref self): Map[K, V] ! CollectionError suspends

    pub fn sync.Set.length(ref self): Int
    pub fn sync.Set.isEmpty(ref self): Bool
    pub fn sync.Set.contains(ref self, key: K): Bool suspends
    pub fn sync.Set.insert(ref self, key: K): Bool ! CollectionError suspends
    pub fn sync.Set.remove(ref self, key: K): Bool suspends
    pub fn sync.Set.snapshot(ref self): Set[K] ! CollectionError suspends

    pub fn sync.Stack.length(ref self): Int
    pub fn sync.Stack.isEmpty(ref self): Bool
    pub fn sync.Stack.push(ref self, value: T): Unit ! CollectionError suspends
    pub fn sync.Stack.pop(ref self): T? suspends
    pub fn sync.Stack[T: Copy + Send + Share].peek(ref self): T? suspends
    pub fn sync.Stack[T: Copy + Send + Share].snapshot(ref self): Array[T] ! CollectionError suspends

    pub fn sync.Queue.length(ref self): Int
    pub fn sync.Queue.isEmpty(ref self): Bool
    pub fn sync.Queue.enqueue(ref self, value: T): Unit ! CollectionError suspends
    pub fn sync.Queue.dequeue(ref self): T? suspends
    pub fn sync.Queue[T: Copy + Send + Share].peek(ref self): T? suspends
    pub fn sync.Queue[T: Copy + Send + Share].snapshot(ref self): Array[T] ! CollectionError suspends

Copiar un handle conserva la misma identidad y no duplica contenido. Todas las
operaciones individuales son linealizables; bajo contención pueden suspender
para aparcar la task, pero no son selectable. CollectionError es el error
canónico de std.collections; no aparece un error paralelo.

sync.Array tiene longitud fija e índices estables: no ofrece resize, inserción,
eliminación, push ni pop. sync.Map y sync.Set conservan el orden de inserción de
su punto de linearización; reemplazar una entrada no la mueve y
eliminar/reinsertar la coloca al final. sync.Stack es LIFO y sync.Queue es FIFO
MPMC. pop/dequeue devuelven none inmediatamente cuando están vacíos; esperar,
limitar capacidad o seleccionar readiness pertenece a std.channel.

Los compareExchange de array y map son fuertes y devuelven el valor observado
en un mismatch. No hay pérdidas, duplicaciones ni fallos espurios.

La construcción corta canónica usa únicamente:

    sync.Array[...]
    sync.Map[...]
    sync.Set[...]
    sync.Stack[...]
    sync.Queue[...]

Los aliases globales SArray, SMap y SSet están prohibidos. Los vacíos de array,
set, stack y queue necesitan tipo esperado; el map vacío se escribe
sync.Map[:]. Los operandos se evalúan de izquierda a derecha y el handle solo se
publica después de construir todo el literal.

### Iteración directa y snapshots

Un for value in sync_owner crea un AsyncIterator con horizonte estructural
finito capturado en O(1). Las inserciones, reinserciones, push y enqueue
posteriores quedan fuera; una retirada anterior a observar una entrada puede
omitirla; cada generación se entrega como máximo una vez. El cursor no mantiene
locks durante el cuerpo, no materializa un array ni asigna memoria proporcional
a la cardinalidad, y termina aunque otros writers continúen.

El header solo admite bindings por valor: for ref, for mut y for var son errores
estáticos. Array recorre índices ascendentes, map/set su orden de inserción
linearizado, stack de cima a base y queue de frente a fondo. Stack y queue
exigen T: Copy + Send + Share para esta observación; iterar nunca consume sus
elementos. La suspensión de next se infiere en el caller.

El recorrido directo es observacional y débilmente consistente, no una vista
global coherente. snapshot() es la frontera explícita suspendible que toma un
único punto de linearización y materializa una colección ordinaria
one-linearization-coherent-value-collection. Igualdad,
serialización, agregaciones exactas y aritmética de colecciones se hacen sobre
ese snapshot; no se inventa una copia que mezcle estados de distintos instantes.

## Modelo, pruebas y límites de diagnóstico

`STD-SYNC-TEST-001` tiene un contrato de pruebas independiente en
[`testing/stdlib-sync-test.json`](../../testing/stdlib-sync-test.json). Los
modelos acotados de `crates/tondo-reliability/src/sync_model.rs` no comparten
estado ni código con la VM: comprueban órdenes de memoria, publicación
release/acquire, colas FIFO, registro atómico de condiciones, handoff de
semáforos, generaciones de barrera, reintentos de `Once`, wakeups y cleanup.
`crates/tondo-reliability/tests/sync_models.rs` reproduce 4.096 seeds y exige
replay determinista, límites finitos y cero waiters pendientes después del
teardown.

El presupuesto de rendimiento target-qualified de `STD-SYNC-PERF-001` está en
[`testing/stdlib-sync-performance.json`](../../testing/stdlib-sync-performance.json)
y [`stdlib-sync-performance.md`](./stdlib-sync-performance.md). Su probe mide
el target `tondo-vm-hosted` con 20 workloads, tres procesos independientes y 27 muestras por
workload. Comprueba latencia, P95/P99, throughput, FIFO, memoria lógica y
cleanup contra el host y el oracle independiente; no mezcla targets ni afirma
resultados AOT nativos.

El fixture ejecutable
`tests/runtime/m11-std-sync-test-001.to` comprueba la continuación real de
`Once.getOrInit`: el closure se ejecuta, el resultado se publica después del
retorno, `get`/`isReady` observan el mismo valor y la segunda llamada no vuelve a
ejecutar el initializer. El target `stdlib_sync` de libFuzzer repite el modelo
con entradas limitadas a 4 KiB y 1.024 transiciones, conservando un corpus de
regresión y comprobando que cada resumen sea reproducible.

La superficie modelada está escrita en Rust seguro y el runtime nativo declara
`#![forbid(unsafe_code)]`; por ello AddressSanitizer/UBSan no añaden una
frontera aplicable a este bloque. La campaña de sanitización y rendimiento de
productos AOT sigue siendo una frontera target-qualified separada; el cierre
de `STD-SYNC-PERF-001` no presenta esta evidencia hosted como una garantía de
otro target.

## Fairness, progreso y diagnóstico

Mutexes, semáforos y condiciones atienden waiters por FIFO de registro, con un
bounded barging documentado para permitir fast paths sin starvation permanente.
El presupuesto de fairness medido es `zero-FIFO-registration-violations`.
La barrera completa todas las llegadas de una generación. El orden del scheduler
no es una promesa de orden global. Backoff, yield o parking son acotados y
suspendibles; el spin ilimitado y el bloqueo inadvertido del executor están
prohibidos.

Los eventos privados del namespace std.sync pueden alimentar
DIAG-RUNTIME-001: creación, espera, adquisición y liberación de locks,
notificaciones, permits, inicialización de once, llegadas y roturas de barrera,
operaciones atómicas y operaciones/snapshots/cursors de colecciones. Cada evento
lleva run_id, task_id, thread_id, resource_id, event_sequence, state, queued y
source_revision; los payloads se omiten por defecto y los hooks no son una API
pública.

## Exclusiones y promoción

El contrato excluye WaitGroup, Task, Future, sufijos Async, locks recursivos,
poisoning implícito, defaults de memory order, CAS débil, espera o notificación
de atomics, scheduler público, spin loops, operaciones de colecciones
selectable, waitPop, waitDequeue, queues ilimitadas ocultas, préstamos en for y
aliases globales SArray/SMap/SSet.

La superficie de compilador, el parking cooperativo hosted, la continuación de
`Once`, la campaña target-qualified de `STD-SYNC-PERF-001`, el frontend de
literales y la implementación de colecciones compartidas para hosted/native ABI
cierran los bloques actualmente implementados. Permanecen pendientes la
iteración directa, `STD-SYNC-COLLECTION-TEST-001`,
`STD-SYNC-COLLECTION-PERF-001`, `STD-SYNC-COLLECTION-CONF-001`,
`STD-SYNC-CONF-001` y `STD-SYNC-DOC-001`. La ABI nativa sigue siendo privada:
este bloque verifica el carrier escalar opaco y su reclamación, no un layout de
tipos genéricos ni lowering AOT.
