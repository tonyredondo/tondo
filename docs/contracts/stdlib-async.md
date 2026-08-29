# Contrato ejecutable de `std.async` para STD-0.1A

Este documento registra la superficie pública que ejecuta actualmente
`std.async` y el contrato cerrado de `std.async.Group` para STD-0.1B. La
especificación canónica exige `select` núcleo y
`Waiter.wait` publica `selectable`; la VM ya registra, compromete y desregistra
ese adapter junto con los adapters de tiempo. El owner no crea
una segunda familia `Task`/`Future`, no duplica APIs con sufijo `Async` y no
usa `Channel`; el canal pertenece a STD-0.1B.

## Efecto público

Todas las funciones siguen declarándose con `fn`. Una firma sin cuerpo que
pueda suspenderse debe escribir `suspends` después de su outcome. El efecto es
parte de la interfaz y del hash ABI. Una función con cuerpo puede declarar el
efecto o inferirlo transitivamente desde una llamada suspendible, un join, un
`AsyncIterator` o cleanup suspendible. La inferencia nunca se basa en el nombre
de la función.

Una llamada directa suspendible espera automáticamente y conserva el tipo de
resultado lógico; `await` delante de esa llamada produce `E1611`. Un `Join`
pendiente es un handle afín: solo `await handle` lo convierte en su resultado.
`Waiter.wait()` es una llamada directa y también espera implícitamente; su
capacidad pública es `selectable`, por lo que puede aparecer directamente como
brazo de `select`. `@sync`
y `@nosuspend` son garantías negativas y rechazan cualquier camino suspendible.

## Superficie nominal

```tondo
pub type Join[T, E]
pub type Waiter[T, E]
pub type Completer[T, E]
pub type AlreadyCompleted

pub fn oneshot[T, E](): (Waiter[T, E], Completer[T, E])
pub fn Waiter.wait(var self): T ! E selectable
pub fn Completer.complete(var self, value: T): Unit ! AlreadyCompleted
pub fn Completer.fail(var self, error: E): Unit ! AlreadyCompleted
pub fn Completer.cancel(var self): Unit ! AlreadyCompleted
```

## `AsyncIterator` y materialización

```tondo
pub trait AsyncIterator[T] {
    fn next(mut self): T? suspends
}

pub fn AsyncIterator.collect[T](var self, limit: Int): Array[T] ! CollectionError suspends
```

`Join` no tiene constructor, poller ni callback público: solo nace de
`spawn call()` o `spawn thread call()`. Cada handle debe consumirse con
`await`, cancelarse, detached o transferirse antes de salir de su scope. El
resultado de un `Join` conserva el outcome de la llamada (`T ! E`) y el handle
solo puede consumirse una vez.

`oneshot` separa el consumidor (`Waiter`) del productor (`Completer`).
`complete`, `fail` y `cancel` compiten atómicamente; exactamente una operación
gana y las posteriores devuelven `AlreadyCompleted`. `Waiter.wait` consume el
waiter una vez, espera implícitamente en una llamada directa y conserva su
registro de `select` hasta el commit; `Completer` puede transferirse a un task o
thread que satisfaga `Send`.

`AsyncIterator.next` produce como máximo un elemento por llamada. `none` es el
fin normal. La operación es lazy, mantiene backpressure y no materializa una
colección. Al terminar normalmente, por cancelación, por error o por salir de
un `for`, el cursor se cierra exactamente una vez; el cierre es idempotente y
libera sus recursos antes de publicar el outcome terminal.

`collect(limit:)` es la única materialización estándar: `limit` es un máximo
finito de elementos, debe ser no negativo y `0` devuelve un array vacío tras
cerrar el cursor. Alcanzar el límite termina con éxito sin pedir un elemento
adicional; una violación de límites o de capacidad devuelve `CollectionError`
sin publicar un array parcial. El cursor se cierra tanto en éxito como en
error, cancelación o unwind. `collect` no introduce una segunda API para
streams ni depende de `Channel`.

## Contrato de coordinación de STD-0.1B

La extensión B añade un único agregado homogéneo para coordinar un número
dinámico de hijos ya iniciados. El contrato machine-readable está en
[`testing/stdlib-async-group.json`](../../testing/stdlib-async-group.json) y
queda cerrado por `STD-ASYNC-GROUP-SPEC-001`. Esto fija la semántica, pero no
afirma por sí solo la conformidad completa. La implementación hosted de VM está
verificada por `STD-ASYNC-GROUP-IMPL-001` y por el fixture ejecutable
`tests/runtime/m11-std-async-group-001.to`; la ruta nativa permanece pendiente
de un scheduler/ABI async nativo y de su corpus de conformidad:

```tondo
pub type Group[T, E]
pub type Completion[T, E] = {
    index: Int
    outcome: T ! E
}

pub fn group[T, E](): Group[T, E]
pub fn Group.add(var self, job: Join[T, E]): Unit
pub fn Group.all(self): Array[T] ! E suspends
pub fn Group.settle(self): Array[T ! E] suspends
pub fn Group.next(var self): Completion[T, E]? selectable
pub fn Group.cancel(self): Unit suspends
```

Mover un `Join` a `add` transfiere al grupo su obligación de cancelación,
espera y cleanup. `Group` es afín: no es `Copy`, `Clone` ni `Discard`; un `Join`
movido ya no puede usarse por el caller y un grupo vivo debe terminarse o
transferirse antes de salir del scope. `all`, `settle` y `cancel` consumen el
grupo. `next` retira una finalización, pero el grupo restante conserva su
obligación terminal incluso cuando devuelve `none`; el caller debe consumirlo
después. El grupo no inicia closures ni constituye un executor.

Los índices son cero-based, monótonos y estables durante la vida del grupo.
`all` espera todos los hijos y devuelve valores por orden de inserción. Al
confirmar un error recuperable, solicita cancelar los restantes, drena su
cleanup y devuelve el error del menor índice entre todos los errores del hijo
que hayan terminado; nunca publica un array parcial. No se sintetiza un `E`
para representar la cancelación. Si un hijo entra en pánico, el grupo drena
cleanup y propaga el pánico después del drain.

`settle` no cancela por un `E`: devuelve un outcome por posición tras esperar
todos, también en orden de inserción. `next` usa orden real de finalización y
conserva el índice estable de inserción; empates se rompen por el índice menor.
Su operación seleccionable no retira nada durante `prepare`; solo el brazo
ganador de `select` retira una finalización en `commit`, y un perdedor hace
`rollback` sin mutación. `cancel` solicita cancelación en orden de inserción y
drena todos los hijos y su cleanup antes de regresar. En un grupo vacío, `all`
y `settle` devuelven arrays vacíos, `next` devuelve `none` inmediatamente y
`cancel` completa inmediatamente.

Las llamadas directas a esas operaciones suspendibles se esperan de manera
implícita; solo un `Join` se consume con `await`. `Group[Unit, E]` sustituye un
`WaitGroup` y evita contadores `add`/`done` separados de los hijos reales. Un
conjunto fijo heterogéneo conserva sus handles y outcomes nominales separados y
puede esperar la primera finalización mediante `select`; los perdedores siguen
perteneciendo al caller. La stdlib no añade tuples awaitables, variadic generics
heterogéneos ni overloads por aridad.

El modelo de estados afín, los tests hosted y el fuzzing ya están verificados
por `STD-ASYNC-GROUP-TEST-001`. El modelo independiente de
[`crates/tondo-reliability/src/group_model.rs`](../../crates/tondo-reliability/src/group_model.rs)
recorre 4.096 seeds con un máximo de 256 operaciones generadas, comprueba
orden, errores, pánicos, cancelación, transferencia, límites, rollback y
cleanup exactamente-una-vez, y se compara consigo mismo para detectar
divergencia de replay. La prueba de integración
[`crates/tondo-reliability/tests/models.rs`](../../crates/tondo-reliability/tests/models.rs)
repite el corpus y exige que toda ejecución termine sin hijos pendientes. El
target independiente
[`fuzz/fuzz_targets/stdlib_async_group.rs`](../../fuzz/fuzz_targets/stdlib_async_group.rs)
usa el mismo oráculo con un límite de 4 KiB/1.024 pasos, corpus persistente y
128 ejecuciones smoke reproducibles; el runner es
[`scripts/stdlib-async-group-fuzz.sh`](../../scripts/stdlib-async-group-fuzz.sh).
El registro completo de estas tres celdas es
[`testing/stdlib-async-group-test.json`](../../testing/stdlib-async-group-test.json).

La superficie aún no se promueve hasta completar presupuestos de rendimiento,
conformidad VM/nativa y documentación ejecutable. `STD-ASYNC-GROUP-IMPL-001`
cierra la implementación hosted de VM y `STD-ASYNC-GROUP-TEST-001` cierra
`MODEL`, `TEST` y `FUZZ`; la implementación nativa y la conformidad cruzada
siguen siendo leaves separadas. `HOST = not-applicable`: `Group` compone el
scheduler y `Join` existentes y no enlaza una primitiva host propia.

### Eventos observables para diagnóstico

El runtime futuro puede registrar los eventos internos
`std.async.group/{create,add,select.prepare,select.commit,select.rollback,
child.cancel-request,child.terminal,drain,consume}`. Cada evento lleva como
mínimo `run_id`, `task_id`, `group_id`, `child_index`, `event_sequence`, `state`
y `source_revision`; los payloads de usuario se omiten por defecto. Estos son
hooks privados consumidos por `DIAG-RUNTIME-001`, no una API pública de
instrumentación.

## Integración con `select`

La forma final no añade `std.async.select`, builders ni valores `Case`. La
expresión núcleo acepta `await join`, `Waiter.wait` y `Group.next`; `Waiter.wait`
publica ya `selectable` y usa la ABI de registro/commit/rollback de la VM. Un
brazo perdedor no consume el `Join`, waiter o grupo, y la cancelación del scope
desregistra todos los brazos antes del unwind. `Group.next` está contractualmente
cerrado en STD-0.1B. Su operación hosted de VM está implementada junto con
`all`, `settle` y `cancel`; el owner no se promueve hasta cerrar sus leaves
`PERF`, `CONF` y `DOC` y la evidencia nativa que corresponda.

## Estado de implementación de STD-0.1A

`STD-A-ASYNC-IMPL-001` cierra las dos rutas de consumo sin duplicar la API:

- una llamada directa a `cursor.collect(...)` usa el lowering MIR que
  conserva el cursor y el buffer bajo el cleanup normal de la función;
- `spawn cursor.collect(...)` transporta el witness estático de
  `AsyncIterator.next` por HIR, MIR y bytecode y ejecuta el mismo protocolo en
  un task estructurado, suspendiendo entre polls y sin crear un array
  intermedio;
- el task mantiene el cursor y los elementos como roots mientras está
  suspendido y libera ese estado exactamente una vez en éxito, límite cero,
  error de capacidad, cancelación o unwind;
- los límites negativos y los fallos de capacidad publican `CollectionError`
  como `ResultErr` y nunca publican un array parcial; al alcanzar el límite no
  se solicita otro `next`;
- al salir de un scope se solicita cancelación cooperativa al task, se reanuda
  el frame suspendido para ejecutar su unwind y solo entonces se cierra el
  owner. El `Join` sigue siendo el único camino para observar un resultado.

La prueba pública de estas rutas es
[`tests/runtime/m11-std-async-impl-001.to`](../../tests/runtime/m11-std-async-impl-001.to);
los límites directos permanecen además en
[`tests/runtime/m11-std-async-iter-001.to`](../../tests/runtime/m11-std-async-iter-001.to)
y el driver cubre el mismo flujo con `tick()`, cancelación de scope y
rechazo de loans exclusivos en `spawn`.

## Límites y exclusiones

- La cancelación es cooperativa y observable al alcanzar el siguiente punto
  de suspensión; nunca deja un handle o cursor vivo sin dueño.
- Los loans exclusivos pueden atravesar una espera secuencial, pero no pueden
  escapar a `spawn`, `spawn thread` ni a un `Completer` compartido.
- `for item in source` selecciona `AsyncIterator` cuando no existe iterador
  síncrono. Si ambos protocolos existen, `Iterator` tiene precedencia; no existe
  `for await`. La iteración no añade `Channel` a STD-0.1A.
- No hay callbacks, polling público, scheduler implícito ni wrappers de tarea.

La implementación del cursor genérico, `collect(limit:)`, cancelación y
backpressure queda verificada en `STD-A-ASYNC-IMPL-001`; `STD-A-FUZZ-001`
promueve la ruta owner-aware y rendimiento y conformidad global siguen siendo
leaves independientes de S1A.
