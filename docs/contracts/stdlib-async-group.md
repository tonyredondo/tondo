# Guía ejecutable de `std.async.Group`

**Estado:** documentación verificada para el contrato STD-0.1B. Esta guía
describe la superficie de `Group[T, E]`, su ownership y el comportamiento que
debe observar un programa Tondo. El fixture y los sidecars enlazados son
ejecutables; la guía no convierte el borrador de la stdlib en una release ni
afirma que el lowering AOT async portable esté terminado.

La autoridad machine-readable es
[`testing/stdlib-async-group.json`](../../testing/stdlib-async-group.json). La
guía general de `std.async` conserva la relación con `Join`, `oneshot`,
`AsyncIterator` y `select` en
[`stdlib-async.md`](./stdlib-async.md); este documento se concentra en la
coordinación de varios hijos homogéneos.

## Cuándo usar `Group`

`Group[T, E]` es el owner afín para un número dinámico de
`Join[T, E]`. Iniciaremos los hijos con `spawn` y transferiremos cada handle al
grupo con `add`. El grupo no crea tareas, no ejecuta closures y no es un
executor. Para un conjunto fijo y heterogéneo se conservan los `Join` separados
y se usa `select`; para un conjunto dinámico homogéneo se usa `Group`.

`Group[Unit, E]` expresa la espera de workers que no devuelven valor. No hay
un contador separado `WaitGroup`, ni una familia paralela `Task`/`Future`, ni
APIs con sufijo `Async`.

## Superficie pública

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

Los cinco ejemplos normativos se identifican de forma estable para que una
herramienta pueda seleccionar una familia sin depender del texto de la guía:

| ID | Superficie | Funciones del fixture |
| --- | --- | --- |
| `fan-out-fan-in-all` | `all` y cancelación tras error | `fan_out_all`, `all_cancels_pending` |
| `settle-mixed-outcomes` | `settle` con éxito y error | `settle_mixed` |
| `next-completion-order` | finalización observable | `next_observes_completion_order` |
| `select-commit-rollback` | commit único y rollback | `next_select_commits_once`, `next_select_rolls_back` |
| `cancel-drain` | cancelación y cleanup | `cancel_drains_pending`, `next_select_drains_cancelled_arm` |

Las llamadas directas a `all`, `settle` y `cancel` esperan implícitamente. No
se escribe `await group.all()`: `await` está reservado para consumir un
`Join` individual. `next` es seleccionable y también espera implícitamente
cuando se llama fuera de `select`. Un `select` registra readiness durante
`prepare`, retira exactamente una finalización solo en `commit` y no modifica
el grupo en `rollback`.

## Ownership y ciclo de vida

`Group` es un valor afín: no es `Copy`, `Clone` ni `Discard`. `Group.add` mueve
el `Join` al grupo y transfiere la obligación de esperar, cancelar y ejecutar
cleanup. El caller no puede volver a usar ese `Join`, añadirlo a otro grupo,
registrarlo en `select` ni esperarlo con `await`. El compilador rechaza cada
uno de esos usos.

El grupo debe terminarse con `all`, `settle` o `cancel`, o transferirse por
movimiento a un owner que pueda satisfacer `Send`. Consumir una finalización
con `next` no consume el grupo: todavía queda una obligación terminal, incluso
si `next` devuelve `none`. Abandonar un grupo vivo al salir de un `scope` es un
error estático; el runtime nativo también rechaza handles inválidos para que
un fallo no pueda dejar el grupo atascado.

El ciclo de vida observable es:

| Estado | Operaciones permitidas | Resultado de la operación |
| --- | --- | --- |
| `open` | `add`, `next`, `all`, `settle`, `cancel` | registra hijos o inicia consumo |
| `waiting` | `next`, terminales | espera una finalización o el drain |
| `ready-to-consume` | `next`, terminales | consume outcomes ya publicados |
| `consumed` | ninguna | cualquier uso posterior es error |

Los hijos se retienen mientras el grupo esté vivo. Cada hijo terminal ejecuta
su cleanup una sola vez y libera su obligación cuando el outcome se consume o
cuando el grupo termina de drenarlo.

## Orden y outcomes

Cada `add` recibe un índice cero-based, monotónico y estable durante toda la
vida del grupo. `all` y `settle` devuelven arrays en orden de inserción; el
scheduler no puede reordenar esos resultados. `next` devuelve la finalización
real más temprana. Si varias finalizaciones están listas en la misma frontera,
el índice de inserción menor rompe el empate. La finalización solo es observable
después de un commit exitoso.

El campo `Completion.index` permite asociar el outcome con la operación que se
registró originalmente. El índice no cambia aunque el scheduler ejecute los
hijos en otro orden.

## `all`: éxito o error principal

`all` espera todos los hijos. En caso de éxito devuelve un `Array[T]` completo
en orden de inserción. Si un hijo produce un error declarado `E`, solicita la
cancelación cooperativa de los hermanos aún pendientes, espera sus cleanups y
devuelve el error del menor índice entre los errores de hijos que hayan
terminado. No publica un array parcial y no sintetiza un `E` para representar
la cancelación de un hermano.

Un pánico no se convierte en un valor de `E`: el grupo drena los hermanos y sus
cleanups y propaga el pánico después del drain. Una cancelación exterior sigue
las reglas del `scope` propietario.

```tondo
import std.async

fn fetch(id: Int): Int ! String suspends {
    id * 10
}

fn fan_out_all(): Array[Int] ! String suspends {
    scope {
        var group = async.group[Int, String]()
        group.add(spawn fetch(1))
        group.add(spawn fetch(2))
        group.all()
    }
}
```

El caso completo, incluido el error y la prioridad por índice, se ejecuta en
`all_cancels_pending` y en `main` del fixture
[`m11-std-async-group-001.to`](../../tests/runtime/m11-std-async-group-001.to).

## `settle`: conservar cada resultado

`settle` espera todos los hijos sin cancelar a los hermanos por un error
recuperable y devuelve un outcome por posición. Cada elemento se inspecciona
con `match` como `ok(value)` o `err(error)`. El array siempre tiene la misma
longitud que el número de hijos registrados y mantiene el orden de inserción.

```tondo
import std.async

fn fetch(id: Int): Int ! String suspends {
    id * 10
}

fn unavailable(_: Int): Int ! String suspends {
    err("unavailable")
}

fn fan_out_settle(): Array[Int ! String] suspends {
    scope {
        var group = async.group[Int, String]()
        group.add(spawn fetch(1))
        group.add(spawn unavailable(2))
        let outcomes = group.settle()
        outcomes
    }
}
```

`settle_mixed` en el fixture verifica un éxito y un error sin array parcial.
Si un hijo entra en pánico, `settle` también drena todos los cleanups antes de
propagarlo.

## `next`: finalización incremental y `select`

`next` retira una sola finalización. Si todavía no terminó ningún hijo,
suspende hasta recibir una notificación terminal. Cuando no quedan
finalizaciones devuelve `none`, pero el grupo sigue siendo afín y debe
consumirse después con `cancel` o transferirse.

```tondo
import std.async

fn fetch(id: Int): Int ! String suspends {
    id * 10
}

fn consume_as_completed(): Unit suspends {
    scope {
        var group = async.group[Int, String]()
        group.add(spawn fetch(1))
        group.add(spawn fetch(2))

        match group.next() {
            some(completion) => assert(completion.index >= 0)
            none => ()
        }
        _ = group.cancel()
    }
}
```

En un `select`, el brazo ganador retira exactamente una finalización. Un brazo
perdedor hace rollback sin quitarla ni transferir ownership:

```tondo pseudocode
let selected = select {
    group.next() => 1
    waiter.wait() => 2
}
```

Los ejemplos `next_observes_completion_order`, `next_select_commits_once` y
`next_select_rolls_back` del fixture cubren, respectivamente, índices
`[1, 0]`, commit único y rollback no mutante.

## `cancel`: terminar sin publicar un error

`cancel` solicita cancelación en orden de inserción, drena todos los hijos
vivos y no regresa hasta que sus cleanups terminan. Su outcome público es
`Unit`; no inventa una variante de `E` para los hijos cancelados. En un grupo
vacío termina inmediatamente. La operación no es idempotente: consumir el
grupo dos veces es un error estático.

```tondo
import std.async

fn worker_without_value(): Unit ! String suspends {}

fn stop_workers(): Unit suspends {
    scope {
        var group = async.group[Unit, String]()
        group.add(spawn worker_without_value())
        _ = group.cancel()
    }
}
```

`cancel_drains_pending` y `next_select_drains_cancelled_arm` son los ejemplos
ejecutables de cancelación y cleanup de un brazo seleccionado.

## Coste y límites

El perfil actual admite como máximo 64 hijos por grupo. `add` es O(1)
amortizado; `all`, `settle` y el drain de `cancel` recorren O(n) hijos. `next`
puede inspeccionar O(n) entradas para encontrar una finalización lista y usa
una cola de notificaciones para evitar polling ocupado. Todas las operaciones
son acotadas por los límites de recursos del runtime.

El coste lógico del estado se calcula sin confundirlo con RSS ni con headers del
allocator:

```text
size_of(RuntimeGroupState)
+ children.capacity() * size_of(RuntimeGroupChild)
+ waiters.capacity() * size_of(usize)
```

El contrato de rendimiento fija cardinalidades 1/8/64, 27 muestras por
workload, P50/P95/P99, throughput, allocations, scans, wakeups y cleanup en
[`stdlib-async-group-performance.md`](./stdlib-async-group-performance.md).
La conformidad del ABI nativo usa el mismo corpus en
[`stdlib-async-group-conformance.md`](./stdlib-async-group-conformance.md);
ninguno de esos informes afirma lowering AOT async portable.

## Verificación ejecutable

El fixture público contiene los ejemplos de `all`, `settle`, `next`,
`select`, cancelación, errores y grupo vacío. Sus sidecars fijan el resultado
observable:

```text
cargo run -q -p tondo-cli -- run tests/runtime/m11-std-async-group-001.to
stdout: group-ok
exit: 0
```

La comprobación documental y el fixture se ejecutan con:

```bash
scripts/stdlib-async-group-doc-check.sh
```

La conformance separada ejecuta los mismos ocho casos contra VM y el ABI del
runtime nativo:

```bash
scripts/stdlib-async-group-conformance.sh
```

El checker documental rechaza cambios de firma, secciones ausentes, comandos
no ejecutables, sidecars faltantes, un fixture que no produce `group-ok` y
claims que presenten esta guía como una release. `STD-ASYNC-GROUP-DOC-001`
queda así cerrado junto con `SPEC`, `IMPL`, `MODEL/TEST/FUZZ`, `PERF` y
`CONF`; la promoción pública y el lowering AOT async siguen siendo decisiones
posteriores del tracker.

## Diagnóstico privado

Los hooks opcionales de diagnóstico usan el namespace
`std.async.group` y pueden registrar `group.create`, `group.add`,
`group.select.prepare`, `group.select.commit`, `group.select.rollback`,
`group.child.cancel-request`, `group.child.terminal`, `group.drain` y
`group.consume`. Incluyen identidad lógica de run/task/group, índice estable,
secuencia, estado y revisión de fuente; omiten payloads por defecto. No son
una API pública ni una vía alternativa para coordinar tareas.
