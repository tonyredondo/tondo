# Presupuesto de rendimiento de `std.async.Group`

Este contrato cierra `STD-ASYNC-GROUP-PERF-001` para el target
`tondo-vm-hosted` (`bytecode-vm`). No constituye evidencia de la ruta AOT ni
de un scheduler async nativo: esas rutas siguen siendo leaves de conformidad
independientes.

## Qué se mide

El probe es el test privado
`runtime::execute::tests::group_performance_probe` en
[`crates/tondo-vm/src/runtime/execute.rs`](../../crates/tondo-vm/src/runtime/execute.rs).
Cada workload usa grupos de 1, 8 y 64 hijos y cubre:

| Operación | Estado | Medición |
| --- | --- | --- |
| `add` | hijos aún vivos | inserción de todos los hijos y crecimiento del buffer |
| `all` | todos listos | fan-in de resultados en orden de inserción |
| `settle` | todos listos | fan-in de un outcome por hijo |
| `next` | uno listo | extracción por orden de finalización |
| `next` | ninguno listo | poll pendiente, park, wakeup y extracción |
| `cancel` | hijos pendientes | solicitud cooperativa, drain terminal y consumo |

La latencia se mide alrededor de la operación observable; la preparación del
fixture queda fuera del reloj. La preparación sí queda reflejada en los
contadores de allocations y almacenamiento, de modo que el coste de crear el
owner y registrar sus hijos no desaparece del informe.

## Protocolo reproducible

El contrato machine-readable está en
[`testing/stdlib-async-group-performance.json`](../../testing/stdlib-async-group-performance.json)
y fija:

- reloj monotónico;
- tres warmups y nueve repeticiones por proceso;
- tres procesos independientes, 27 muestras mínimas por workload;
- una operación lógica por muestra;
- seed declarativa `tondo-async-group-perf-0.1`;
- outliers conservados en `samples_ns`, sin descartarlos para mejorar la
  mediana o los percentiles.

El runner
[`scripts/stdlib-async-group-performance.sh`](../../scripts/stdlib-async-group-performance.sh)
comprueba el hash SHA-256 del probe, rechaza por defecto un workspace sucio,
captura toolchain/target/CPU sin usarlos como identidad semántica y genera
`target/reliability/evidence/stdlib-async-group-performance.json`. El informe
incluye mediana, P95, P99, throughput derivado y los contadores invariantes de
cada workload.

## Contadores e interpretación

`VmStatistics` expone contadores específicos de Group para que las cifras no
se infieran de tiempos: número de adds y polls por operación, entradas de
hijos inspeccionadas, parks y wakeups, pasadas de cancelación, allocations del
heap VM, creaciones del estado runtime, crecimientos de los buffers de hijos y
waiters, máximo de hijos y bytes lógicos reservados.

`group_peak_state_bytes` es una medida lógica y portable:

```text
size_of(RuntimeGroupState)
+ children.capacity() * size_of(RuntimeGroupChild)
+ waiters.capacity() * size_of(usize)
```

No pretende representar headers ni fragmentación del allocator del proceso.
La cifra permite comparar cardinalidades y targets con el mismo contrato sin
confundir una estimación portable con RSS del sistema.

El probe exige además que:

- `add` registre exactamente la cardinalidad y no ejecute un poll terminal;
- `all` y `settle` hagan un único poll listo;
- `next` listo haga un poll y `next` pendiente haga el par pendiente/listo,
  con exactamente un park y un wakeup;
- `cancel` haga el poll de solicitud y el poll posterior al drain;
- cada workload conserve el mismo máximo de hijos, cree un estado Group y
  consuma o elimine su owner antes de devolver.

El contrato no fija un umbral absoluto de nanosegundos. Fija la forma de la
medición y los invariantes que deben permanecer estables; las regresiones se
comparan entre informes con el mismo target, backend, perfil, toolchain y
probe hash.

## Ejecución

```bash
scripts/stdlib-async-group-performance-test.sh
TONDO_ASYNC_GROUP_PERF_ALLOW_DIRTY=1 \
  scripts/stdlib-async-group-performance.sh
```

El segundo comando produce evidencia local aun durante una implementación en
curso. En CI y en el gate normal se ejecuta sin `ALLOW_DIRTY`, sobre un
checkout limpio. La prueba de contrato muta copias temporales para demostrar
que el gate rechaza muestras insuficientes, cardinalidades no declaradas,
hashes de probe incorrectos, workloads ausentes e invariantes alteradas.

