# Contrato ejecutable de `std.async` para STD-0.1A

Este documento registra la superficie pública que ejecuta actualmente
`std.async`. La especificación canónica exige `select` núcleo y
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

## Extensión de coordinación planificada para STD-0.1B

La extensión B añade un único agregado homogéneo para coordinar un número
dinámico de hijos ya iniciados. No cambia la superficie ejecutable de STD-0.1A
ni afirma implementación actual:

```tondo
pub type Group[T, E]
pub type Completion[T, E] = {
    index: Int
    outcome: T ! E
}

pub fn group[T, E](): Group[T, E]
pub fn Group.add(var self, job: Join[T, E])
pub fn Group.all(self): Array[T] ! E suspends
pub fn Group.settle(self): Array[T ! E] suspends
pub fn Group.next(var self): Completion[T, E]? selectable
pub fn Group.cancel(self) suspends
```

Mover un `Join` a `add` transfiere al grupo su obligación de cancelación,
espera y cleanup. `all`, `settle` y `cancel` consumen el grupo. `next` retira una
finalización, pero el grupo restante conserva su obligación terminal y debe
consumirse después. El grupo no inicia closures ni constituye un executor.

`all` espera todos los hijos y devuelve valores por orden de inserción. Al
confirmar un error recuperable, solicita cancelar los restantes, drena su
cleanup y devuelve el error del menor índice entre los ya confirmados.
`settle` no cancela por un `E`: devuelve un outcome por posición tras esperar
todos. `next` usa orden real de finalización y conserva el índice estable de
inserción. `cancel` solicita cancelación y drena todos los hijos antes de
regresar. En un grupo vacío, `all` y `settle` devuelven arrays vacíos, `next`
devuelve `none` y `cancel` completa inmediatamente.

Las llamadas directas a esas operaciones suspendibles se esperan de manera
implícita; solo un `Join` se consume con `await`. `Group[Unit, E]` sustituye un
`WaitGroup` y evita contadores `add`/`done` separados de los hijos reales. Un
conjunto fijo heterogéneo conserva sus handles y outcomes nominales separados y
puede esperar la primera finalización mediante `select`; los perdedores siguen
perteneciendo al caller. La stdlib no añade tuples awaitables, variadic generics
heterogéneos ni overloads por aridad.

`STD-ASYNC-GROUP-SPEC-001` cierra este contrato. La superficie no se promueve
hasta completar `STD-ASYNC-GROUP-IMPL-001`, su modelo de estados afín, tests,
fuzzing, presupuestos de rendimiento, conformidad VM/nativa y documentación
ejecutable. `HOST` es no aplicable con razón normativa: `Group` compone el
scheduler y `Join` existentes y no enlaza una primitiva host propia.

## Integración con `select`

La forma final no añade `std.async.select`, builders ni valores `Case`. La
expresión núcleo acepta `await join`, `Waiter.wait` y `Group.next`; `Waiter.wait`
publica ya `selectable` y usa la ABI de registro/commit/rollback de la VM. Un
brazo perdedor no consume el `Join`, waiter o grupo, y la cancelación del scope
desregistra todos los brazos antes del unwind. `Group.next` pertenece todavía a
STD-0.1B y no se considera implementado en esta superficie.

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
