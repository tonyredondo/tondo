# Contrato de `std.async` para STD-0.1A

Este documento es la fuente normativa de la superficie pública de `std.async`.
El owner comparte el único efecto de suspensión de Tondo: `suspends`. No crea
una segunda familia `Task`/`Future`, no duplica APIs con sufijo `Async` y no
usa `Channel`; el canal pertenece a STD-0.1B.

## Efecto público

Todas las funciones siguen declarándose con `fn`. Una firma sin cuerpo que
pueda suspenderse debe escribir `suspends` después de su outcome. El efecto es
parte de la interfaz y del hash ABI. Una función con cuerpo puede declarar el
efecto o inferirlo transitivamente desde una llamada suspendible, `await`, un
`AsyncIterator` o cleanup suspendible. La inferencia nunca se basa en el nombre
de la función.

Una llamada directa suspendible espera automáticamente y conserva el tipo de
resultado lógico; `await` delante de esa llamada es opcional. Un `Join` o un
`Waiter` pendiente es un handle afín: solo `await handle` lo convierte en su
resultado. `@sync` y `@nosuspend` son garantías negativas y rechazan cualquier
camino suspendible, incluso si el caller omitió `await`.

## Superficie nominal

```tondo
pub type Join[T, E]
pub type Waiter[T, E]
pub type Completer[T, E]
pub type AlreadyCompleted

pub fn oneshot[T, E](): (Waiter[T, E], Completer[T, E])
pub fn Waiter.wait(var self): T ! E suspends
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
waiter una vez y declara `suspends`; `Completer` puede transferirse a un task o
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

## Límites y exclusiones

- La cancelación es cooperativa y observable al alcanzar el siguiente punto
  de suspensión; nunca deja un handle o cursor vivo sin dueño.
- Los loans exclusivos pueden atravesar una espera secuencial, pero no pueden
  escapar a `spawn`, `spawn thread` ni a un `Completer` compartido.
- `for item in source` selecciona `AsyncIterator` cuando no existe iterador
  síncrono; `for await item in source` solo desambigua una fuente que expone
  ambos protocolos. Ninguna forma añade `Channel` a STD-0.1A.
- No hay callbacks, polling público, scheduler implícito ni wrappers de tarea.

La implementación del cursor genérico, `collect(limit:)`, cancelación y
backpressure se cierra en `STD-A-ASYNC-IMPL-001`; este documento fija sus
contratos y permite que las tareas dependientes implementen exactamente esta
superficie.
