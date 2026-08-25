# Contrato de instrumentación runtime de diagnóstico

**Estado:** `implemented` para la VM hosted de Tondo 0.1 (`DIAG-RUNTIME-001`).
**Superficie:** interna del crate `tondo-vm`; no es una API de Tondo ni de la
stdlib.

Este contrato implementa la capa observable que necesitan los detectores
`RACE-001`, `LEAK-001` y `DUMP-001`. La VM conserva la misma
semántica de valores, errores, orden, cleanup, cancelación y exit status con y
sin instrumentación. El collector solo se crea cuando el runner solicita el
entry point de diagnóstico; la ruta normal no reserva buffers ni genera
eventos.

## 1. Entrada y aislamiento

La frontera Rust es opt-in:

```rust
execute_with_diagnostics(program, entry, host, DiagnosticConfig::default())
```

La ejecución normal (`execute`, `execute_with_limits` y
`execute_with_limits_and_copy_strategy`) devuelve `diagnostics = None`.
`VmExecution.diagnostics` solo contiene una traza cuando se pidió el perfil.
El collector pertenece a una ejecución y nunca se comparte entre procesos,
intentos, shards o suites; el runner futuro debe conservar además el proceso
nuevo exigido por D0 para cada intento.

El módulo `runtime::diagnostics` exporta únicamente tipos de datos para que el
runner pueda consumir la traza. `DiagnosticSession`, los hooks de emisión y las
estructuras del heap permanecen privados al runtime. No hay keyword, anotación,
función de stdlib, variable de entorno ni configuración de proyecto nueva.

## 2. Eventos y contexto

La traza usa `tondo-diagnostic-runtime/1` y mantiene un stream ordenado de
eventos, además de un tail acotado del scheduler, snapshots de roots, ledger de
recursos y source maps observados. Los eventos son metadatos sin payload de
usuario:

| Evento | Datos mínimos |
| --- | --- |
| `Thread` | id estable del thread lógico y `Started`/`Stopped` |
| `Task` | id estable, owner/padre y estado `Created`, `Runnable`, `Running`, `Waiting`, `CancelRequested`, `Complete` o `Consumed` |
| `Memory` | `Read`/`Write`/`Move`, task/frame/slot/profundidad de proyección, identidad de almacenamiento/ruta y `DiagnosticSource`/stack |
| `Synchronization` | spawn, park/wake, join, host start/complete/cancel, loans y select, con peer y source cuando existe |
| `Heap` | identidad generacional del objeto, allocate/replace, bytes estimados y task owner |
| `Roots` | identidades de heap alcanzables en la barrera y retainers lógicos acotados |
| `Resource` | id host opaco, clase cerrada, adquisición/liberación y owner |
| `Scheduler` | enqueue/switch/park/wake/complete y tamaño de cola |
| `Quiescence` | inicio y fin de cada barrera de observación |

Los ids de task son monotónicos dentro de la ejecución (la raíz es `1`). Los
ids de heap combinan índice y generación; un slot reciclado no puede parecer el
mismo objeto. Los nombres de función y los spans son source-map metadata, no
contenido fuente ni payload.

## 3. Roots, retainers y recursos

Los accesos a `BytecodePlace` se registran después de pasar la validación de
ownership/loans. Cada acceso conserva una identidad de almacenamiento
compartido, un hash estable de la ruta proyectada y un stack acotado; los
eventos `Task` conservan el stack de creación. Las operaciones de scheduler y las fronteras de host se
registran en sus helpers centrales, por lo que una nueva instrucción no puede
olvidar una rama equivalente sin que falle su test de cobertura.

La VM toma un snapshot de roots en la finalización y alrededor de la barrera de
quiescencia. Los objetos inalcanzables que el GC recupera no se publican como
roots; el futuro detector de leaks usará esos snapshots para construir el grafo
de retención. Los valores host opacos entran en el ledger cuando se materializan
y salen cuando el cleanup runtime los libera. El ledger conserva estado terminal
y primer/último evento para distinguir una adquisición sin release de una
operación ya cerrada.

## 4. Límites y fallo cerrado

Los defaults son los límites D0: 1.000.000 eventos, 256 frames, 256
retainers por objeto y 4.096 eventos en el tail del scheduler. Los cuatro
valores deben ser positivos. Superar `max_events` devuelve
`VmError::ResourceLimit { resource: "diagnostic events", ... }` y marca la
traza como truncada; no se descarta silenciosamente el exceso. El tail y los
retainers descartan lo más antiguo/excedente solo dejando `truncated = true`.

Esta capa no serializa reportes ni dumps: los límites de 16 MiB y 256 MiB se
aplican en los writers de `DIAG-TEST-001`/`DUMP-001`. El collector no inventa
una conclusión de race: entrega memoria, lifecycle, synchronization, identidad
y stacks para que `RACE-001` calcule vector clocks sobre caminos ejecutados.
`RACE-001` y `LEAK-001` están implementados para la VM hosted; `DUMP-001` sigue
siendo consumidor pendiente.

## 5. Verificación

La evidencia ejecutable está en:

- `crates/tondo-vm/src/runtime/diagnostics.rs`: límites, deduplicación de
  source maps, tail, roots, ledger y corrupción de configuración;
- `crates/tondo-vm/src/runtime/execute.rs`: integración opt-in, lifecycle de
  tareas, scheduler, accesos, heap, host cleanup, quiescencia y aislamiento;
- `scripts/diagnostic-runtime-check.sh`: contrato machine-readable y fronteras
  privadas; y
- `scripts/diagnostic-runtime-test.sh`: negativos de estado, presupuesto,
  módulo y contrato.

Los detectores posteriores deben consumir esta traza sin volver a instrumentar
la VM. La paridad nativa queda explícitamente pendiente de `DIAG-NATIVE-001`.
