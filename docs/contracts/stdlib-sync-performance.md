# Presupuesto de rendimiento de `std.sync`

Este contrato cierra `STD-SYNC-PERF-001` para el target
`tondo-vm-hosted` (`bytecode-vm`). La medición es una campaña target-qualified:
no mezcla targets, backends ni perfiles y no presenta la ruta AOT nativa como
si fuese la VM hosted. El puente nativo de atomics `u64` y parking por epoch
conserva su propia conformidad ABI; su benchmark AOT queda fuera de este
informe.

## Qué se mide

El probe privado
`process_host::tests::sync_performance_probe` en
[`crates/tondo-compiler/src/process_host.rs`](../../crates/tondo-compiler/src/process_host.rs)
ejecuta el host de referencia real y comprueba sus invariantes mientras mide:

| Grupo | Workloads | Propósito |
| --- | --- | --- |
| Mutex | uncontended 1/8; contended 8/64 | fast path, cola FIFO, park y wakeup |
| RwLock | read/write uncontended 1/8 | coste de lectores y escritores sin espera |
| Semaphore | uncontended 1/8; contended 8/64 | adquisición, devolución y handoff FIFO |
| Condition | contended 8/64 | registro indivisible, `notifyAll` y reacquire |
| Barrier | dos generaciones contended 8/64 | reutilización, elección de `Leader` y wakeups |
| Atomic | load/store/swap/CAS 1/8 | órdenes explícitos y CAS fuerte |
| Once | valor ya publicado 1/8 | lectura lista e idempotencia de publicación |

Cada muestra informa latencia monotónica, operaciones lógicas, waiters
bloqueados, wakeups, violaciones FIFO, memoria lógica y handles vivos. Las
operaciones contended incluyen deliberadamente la preparación y el drenaje de
la cola dentro del round trip medido; las variantes uncontended excluyen la
preparación del fixture del reloj. Esto evita comparar un fast path con una
medición que esconda el coste de registrar waiters.

La memoria es una estimación portable, no RSS: suma el tamaño lógico del
registro de `HostValue`, los `PendingSync` y la capacidad de las colas. Los
headers y la fragmentación del allocator quedan fuera. `live_handles` cuenta
entradas vivas del registro host, no pretende ser una cifra de allocations del
sistema operativo.

## Protocolo reproducible

El contrato machine-readable está en
[`testing/stdlib-sync-performance.json`](../../testing/stdlib-sync-performance.json)
y fija:

- reloj monotónico;
- tres warmups y nueve repeticiones por proceso;
- tres procesos independientes, 27 muestras por workload;
- 32 operaciones por lote en las variantes repetitivas;
- seed declarativa `tondo-stdlib-sync-perf-0.1`;
- outliers conservados en `samples_ns`, sin descartarlos para mejorar la
  mediana, P95 o P99.

[`scripts/stdlib-sync-performance.sh`](../../scripts/stdlib-sync-performance.sh)
comprueba el hash SHA-256 del probe, rechaza por defecto un checkout sucio,
captura target/toolchain/CPU como contexto y escribe
`target/reliability/evidence/stdlib-sync-performance.json`. La identidad del
informe solo usa los campos declarados por el contrato; no incluye PID,
timestamp, path ni frecuencia instantánea de CPU.

## Oracle e invariantes

La corrección no se deduce del tiempo. El probe verifica cada resultado contra
el estado del host y el modelo independiente de
[`crates/tondo-reliability/src/sync_model.rs`](../../crates/tondo-reliability/src/sync_model.rs),
que se ejercita en
[`crates/tondo-reliability/tests/sync_models.rs`](../../crates/tondo-reliability/tests/sync_models.rs).
La campaña falla si ocurre cualquiera de estas condiciones:

- cambia el número de operaciones por workload entre muestras;
- un waiter FIFO se salta su posición o aparece una wakeup espuria;
- los contadores de bloqueo/wakeup no coinciden con la cardinalidad declarada;
- queda un waiter pendiente o una cola no vacía al retornar el probe;
- la memoria lógica o el número de handles cae en un valor imposible;
- se intenta agregar evidencia de targets o backends distintos.

La campaña no fija un umbral absoluto de nanosegundos. Fija la forma de medir y
las leyes que deben permanecer estables; las regresiones de tiempo se comparan
entre informes con el mismo target, backend, perfil, toolchain y hash de probe.

## Ejecución

```bash
scripts/stdlib-sync-performance-test.sh
TONDO_STDLIB_SYNC_PERF_ALLOW_DIRTY=1 \
  scripts/stdlib-sync-performance.sh
```

El segundo comando permite producir evidencia durante una implementación en
curso. El gate CI lo ejecuta sin `ALLOW_DIRTY`, sobre un checkout limpio. La
prueba de contrato utiliza copias temporales para demostrar que se rechazan
muestras insuficientes, workloads duplicados, cardinalidades no declaradas,
hashes incorrectos, presupuestos de fairness alterados, oracles ausentes y
reports fuera de la ruta canónica.
