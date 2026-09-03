# Presupuesto de rendimiento de `std.executor`

Este contrato cierra `STD-EXEC-PERF-001` para dos lanes observables y
deliberadamente separadas: la VM hosted (`tondo-vm-hosted`, `bytecode-vm`) y el
bridge privado de tokens del runtime nativo en
`x86_64-unknown-linux-gnu`. La medición no promociona la API pública, un ABI de
layout ni el lowering AOT de callables.

La autoridad machine-readable está en
[`testing/stdlib-executor-performance.json`](../../testing/stdlib-executor-performance.json).
El checker y sus mutaciones negativas son
[`scripts/stdlib-executor-performance-check.sh`](../../scripts/stdlib-executor-performance-check.sh)
y [`scripts/stdlib-executor-performance-test.sh`](../../scripts/stdlib-executor-performance-test.sh).
La campaña reproducible es
[`scripts/stdlib-executor-performance.sh`](../../scripts/stdlib-executor-performance.sh)
y escribe
`target/reliability/evidence/stdlib-executor-performance.json`.

## Qué se mide

El probe hosted es
`runtime::execute::tests::executor_performance::executor_performance_probe`.
Construye el programa verificado de `std.executor`, ejecuta jobs reales en un
`Engine` hijo por worker y observa la admisión, la espera del owner y el
envelope de resultado. El probe nativo es
`tests::native_blocking_performance_probe`; ejercita únicamente la lane privada
de tokens opacos y por tanto no incluye coste de callback ni representa una
ejecución AOT.

Cada lane cubre los mismos seis perfiles lógicos, con su identidad de target:

| Perfil | Configuración | Dimensión principal |
| --- | --- | --- |
| `startup-1` | un worker, un pool vacío | creación de workers y cierre graceful |
| `roundtrip-1` | un job, un worker | scheduling latency y retorno de un resultado |
| `roundtrip-4` | cuatro jobs, cuatro workers | fan-out/fan-in de admisión y completion |
| `throughput-4` | 32 jobs, cuatro workers, capacidad 32 | throughput de la cola acotada |
| `saturation-1` | ocho jobs, un worker, capacidad 1 | backpressure y reintentos explícitos |
| `drain-4` | ocho jobs, cuatro workers, capacidad 8 | shutdown, drain y terminalidad |

El reloj rodea la operación indicada por el perfil. La preparación del
programa y del pool queda fuera de la latencia de operación salvo en
`startup-1`; su coste lógico sí queda reflejado en `logical_memory_bytes` y
`worker_starts`. Los contadores `waits` y `bridge_events` son observaciones del
owner (esperas y completions), no una afirmación de número de wakeups del
kernel.

## Protocolo reproducible

El contrato fija reloj monotónico, tres warmups, nueve repeticiones medidas y
tres procesos independientes: cada workload tiene exactamente 27 muestras.
Los outliers se conservan en `samples_ns`; no se descarta ninguna muestra para
mejorar mediana, P95 o P99. El seed declarativo es
`tondo-stdlib-executor-perf-0.1`.

El runner ejecuta primero el modelo acotado independiente de
`crates/tondo-reliability/src/executor_model.rs` y
`crates/tondo-reliability/tests/models.rs`. Después ejecuta cada probe en tres
procesos frescos, agrega por `workload_id` y comprueba que no se mezclen target,
backend o hash de probe. En un host que no sea `x86_64-unknown-linux-gnu` el
probe nativo emite un marcador explícito de no soporte y el informe conserva
únicamente la lane hosted; no se presenta una ausencia como medición nativa.

## Métricas e invariantes

El informe publica latencia, tail latency, throughput, scheduling,
backpressure, espera/bridge, picos de cola y workers, memoria lógica y handles
vivos. `logical_memory_bytes` es una envolvente portable calculada como estado
del pool + slots de workers + capacidad de envelopes; excluye headers,
fragmentación del allocator y RSS del proceso.

El checker exige que:

- cada workload tenga 27 muestras positivas y cuantiles monotónicos;
- `accepted` y `bridge_events` coincidan con las operaciones declaradas (salvo
  `startup-1`, que no admite jobs);
- `active_peak` nunca supere `workers` y `queued_peak` nunca supere
  `capacity`;
- `saturation-1` observe al menos un rechazo por backpressure y una espera;
- la VM cierre el bridge y el runtime nativo vuelva a cero handles vivos;
- los seis perfiles hosted y los seis nativos permanezcan separados, sin
  agregación entre backends;
- `native_aot` permanezca `not-claimed`.

El contrato no fija un umbral absoluto de nanosegundos. Una regresión sólo es
comparable con otro informe de la misma identidad semántica: suite, workload,
hash de probe, target, backend, perfil, toolchain, flags y revisión fuente.
Modelo, mediciones y cleanup son evidencia de implementación; no alteran la
semántica de `std.executor` ni convierten la lane nativa de tokens en una API
pública.

## Ejecución

```bash
scripts/stdlib-executor-performance-test.sh
CARGO_TARGET_DIR=target-fast \
TONDO_STDLIB_EXECUTOR_PERF_ALLOW_DIRTY=1 \
scripts/stdlib-executor-performance.sh
```

El segundo comando permite obtener evidencia durante una implementación en
curso. En el gate normal se ejecuta sin `ALLOW_DIRTY`, sobre un checkout limpio,
y el hash de cada probe debe coincidir con el contrato.
