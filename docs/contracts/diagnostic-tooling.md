# Contrato de tooling dinámico de diagnóstico

**Estado:** `contract-locked` para Tondo 0.1. `DEC-018` y
`DIAG-SPEC-001` fijan esta superficie; el contrato runtime-facing de
`std.async.Group`, `std.channel`, `std.sync`, `std.executor`, `std.net` y el
calendario civil de `std.time` ya están cerrados por
`STD-ASYNC-GROUP-SPEC-001`, `STD-CONC-001`, `STD-SYNC-001`, `STD-EXEC-001`,
`STD-NET-001` y `STD-CIVIL-TIME-001`. La primera implementación runtime de la
VM hosted está cerrada por `DIAG-RUNTIME-001`; los detectores hosted de races,
retención y dumps lógicos están cerrados por `RACE-001`, `LEAK-001` y
`DUMP-001`, mientras que runner, captura de señales y paridad native
permanecen pendientes en los bloques `DIAG-*` posteriores.

Este documento define la frontera entre el lenguaje, el runtime y las
herramientas que ayudan a encontrar fallos de concurrencia, retención de
memoria y terminaciones fatales. No añade semántica fuente, keywords ni una
segunda familia de APIs síncronas/asíncronas.

## 1. Una superficie de diagnóstico

La dirección pública es una sola superficie de perfiles opt-in:

```text
tondo run  --diagnostics <race|leaks|crash|all>[,...] ...
tondo test --diagnostics <race|leaks|crash|all>[,...] ...
tondo dump analyze <file.tdump> [--format human|json]
```

La grafía y el envelope finales están congelados en el contrato machine-readable
[`testing/diagnostic-tooling.json`](../../testing/diagnostic-tooling.json).
El compilador/runtime instrumenta la ejecución y el CLI materializa reportes y
artefactos cuando los bloques de implementación estén cerrados. `tondo check`
conserva su función estática y no arranca un runtime dinámico. Los perfiles no
requieren un `tondo.json`, una variable de entorno ni un cambio de edición; son
overrides explícitos de una invocación.

Los perfiles son:

| Perfil | Propósito | Resultado de una observación positiva |
|---|---|---|
| `race` | Detectar accesos incompatibles sin una relación happens-before válida | Fallo de la ejecución y reporte con el conflicto |
| `leaks` | Detectar retención de objetos y recursos sin cierre | Fallo de la ejecución y grafo de retención/recursos |
| `crash` | Capturar terminaciones fatales, pánicos no recuperables y abortos | Artefacto `.tdump` analizable, además del diagnóstico normal |
| `all` | Activar los tres perfiles en una campaña | Unión de los reportes, sin ocultar qué perfil observó cada evento |

Un target que no pueda implementar un perfil lo comunica como
`unsupported-diagnostic-profile`; no lo omite silenciosamente ni lo convierte
en un resultado verde.

## 2. Race detector

`race` es un análisis dinámico implementado en la VM hosted por `RACE-001`.
Instrumenta los accesos a memoria compartida y
las operaciones que crean o establecen orden entre unidades de ejecución:

- `Ref[T]`, `Pointer[T]`, regiones mutables, `unsafe` y llamadas FFI;
- `spawn`, `spawn thread`, `Join`, scopes y suspensión implícita;
- channels, `std.sync`, locks, atomics y wakeups del executor; y
- creación, transferencia y terminación de tasks/threads.

El runtime conserva un reloj lógico/vector-clock, identidad generacional de
almacenamiento y metadatos de origen. Un
reporte identifica como mínimo el acceso de cada participante, lectura o
escritura, rango lógico, fuente y source map, task/thread, stack de acceso,
stack de creación y la arista de sincronización que faltaba. La detección se
limita a caminos realmente ejecutados: un resultado limpio no es una prueba
estática de ausencia de races.

La instrumentación se aplica al programa y a los bridges runtime relevantes,
pero no cambia la semántica observable del programa. Las supresiones son
explícitas, acotadas a una identidad de evento y visibles en el reporte; nunca
se aceptan por nombre global o por silenciar stderr. El coste de la campaña se
registra como metadato y no contamina la baseline normal de `PERF-001`.

## 3. Leak y retención

El runtime hosted usa GC de trazado preciso, por lo que “leak” no significa que
un ciclo inalcanzable sobreviva al recolector. El detector separa cuatro
clases:

1. **Retención gestionada:** crecimiento de objetos todavía alcanzables desde
   roots después de una barrera de quiescencia, con snapshot de roots y grafo
   de retenedores. Los objetos inalcanzables que el GC recupera no aparecen
   como falsos positivos.
2. **Recursos afines:** archivos, sockets, procesos, pipes, locks, timers y
   tasks que no alcanzan su operación terminal o cancelación/reaping.
3. **Memoria nativa/FFI:** asignaciones que cruzan la frontera host y no tienen
   release asociado, con owner y stack de adquisición.
4. **Crecimiento sostenido:** deltas entre snapshots de una campaña repetida,
   distinguiendo caché intencionada de crecimiento sin límite mediante la
   política declarada por el harness.

Cada observación conserva allocation/creation stack, owner lógico, retainers,
estado de cleanup, tamaño y primera/última vista. El detector no reclama
prueba matemática de fuga; clasifica evidencia reproducible y señala la razón
si no puede alcanzar quiescencia. Las campañas se ejecutan en un proceso nuevo
por intento para que una fuga no contamine retries, shards ni suites hermanas.

## 4. Crash dumps `.tdump`

El formato `tondo-dump/1` es un contenedor local, versionado y content-addressed
que conserva:

- razón de terminación, exit status y perfil activo;
- target, backend, toolchain, flags, revisión y debug/source-map IDs;
- stacks de todos los threads y tasks, registros cuando el target los expone y
  información de unwind;
- roots, resumen de heap/retainers y ledger de recursos, sin volcar payloads de
  usuario por defecto;
- últimos eventos de scheduler, suspensión, locks, I/O y panic; y
- redacción aplicada, límites, truncados y capacidad que no estuvo disponible.

La VM hosted captura la traza lógica en el envelope canónico `tondo-dump/1` y
el analizador `tondo dump analyze` produce una vista humana y un JSON estable
sin consultar red ni ejecutar código del dump. No hay subida automática, y los
campos que puedan contener secretos quedan fuera. La captura en una señal o
terminación fatal hace únicamente trabajo async-signal-safe y delega la
serialización a un helper cuando el host lo permite; esa parte física sigue en
`DIAG-NATIVE-001`.

La VM ofrece primero un dump lógico. El backend nativo añade adaptadores de
unwind, símbolos y registros sin cambiar el envelope; si una plataforma no
puede conservar una dimensión, el dump lo declara como `unavailable`.

## 5. Reporte común y runner

Los tres perfiles producen `tondo-diagnostic-report/1`, separado de
`tondo-diagnostics-json/1` (diagnósticos de compilación). El reporte común liga:

```text
run_id + test_id/attempt + shard + profile + target + backend + toolchain
```

e incluye estado, observaciones, limitaciones, hashes de artefactos y paths
lógicos. Los campos de identidad obligatorios son `run_id`, `attempt_id`,
`shard`, `profile`, `target`, `backend`, `toolchain` y `source_revision`; la
ordenación es identidad, tipo de observación y span de origen. Un reporte limpio
solo describe caminos observados y nunca es una prueba estática de ausencia.
`tondo test` enlaza el reporte y los `.tdump` al intento concreto en el artifact
store ya definido; JSON y JUnit proyectan la misma referencia sin duplicar
payloads. Retries, repeat y shards conservan la identidad del test y crean un
proceso limpio por intento.

Las campañas de `race` y `leaks` son lanes explícitas de CI por su coste. Una
campaña instrumentada no sustituye la suite normal ni rebaja el baseline de
cobertura, mutation o rendimiento; añade evidencia adicional y un gate propio.

### 5.1 Exit status y límites

El contrato usa la tabla de salida existente del CLI: `0` indica éxito sin
hallazgo, `1` un hallazgo dinámico observado, `2` un perfil inválido o no
soportado y `3` un fallo del toolchain. `101` se conserva para un panic del
programa cuando no existe un hallazgo dinámico de mayor precedencia. La
precedencia es `toolchain_failure`, `unsupported_or_invalid_profile`,
`finding`, `program_exit_status`; el `program_exit_status` original siempre
queda en el reporte.

Los límites cerrados son tres perfiles por invocación, 16 MiB por reporte,
256 MiB por dump, 100.000 observaciones, 1.000.000 eventos, 256 frames de
stack, 256 retainers por objeto y 4.096 eventos de scheduler. El runtime falla
cerrado al alcanzar un límite y debe registrar truncación o indisponibilidad;
no puede descartar silenciosamente observaciones.

## 6. Frontera con lenguaje y stdlib

No existe `async`, `race`, `leak` o `dump` como keyword. Tampoco se añade una
API pública `std.race`, `std.leaks` o `std.crash` que duplique el CLI. La stdlib
expone únicamente las operaciones normales de memoria, recursos, sincronización
y testing; los hooks internos de instrumentación pertenecen al runtime y al
toolchain. Cualquier hook público futuro deberá tener un caso de uso estable y
pasar por una revisión separada, no ser un escape para acceder al detector.

La instrumentación dinámica complementa, no reemplaza, el análisis estático de
ownership/borrow, `Send`/`Share`, `unsafe` y capacidades. Solo se permite
promover una observación al contrato cuando conserva el mismo oracle de valores,
errores, orden, cleanup, cancelación y exit status que la VM.

## 7. Gates y dependencias

La ejecución se divide en bloques del tracker:

| ID | Entrega | Dependencias |
|---|---|---|
| `DIAG-SPEC-001` | Profiles, envelope, identidad, privacidad, límites y CLI | `PERF-001`, contratos CLI/testing |
| `STD-ASYNC-GROUP-SPEC-001` | Contrato de `Group`, ownership, orden, cancelación drenada y eventos privados; sin implementación pública | `DIAG-SPEC-001`, `ASYNC-SELECT-VM-CONF-001`, S1A |
| `STD-CONC-001` | Contrato de canales, ownership, backpressure, cierre, selección cancelable, fairness y eventos privados; sin implementación pública | `DIAG-SPEC-001`, `ASYNC-SELECT-VM-CONF-001`, S1A |
| `STD-SYNC-001` | Contrato de locks, atomics, colecciones compartidas y eventos observables, sin implementación pública | `DIAG-SPEC-001`, foundations STD-0.1A |
| `STD-EXEC-001` / `STD-NET-001` | Contratos runtime-facing y eventos observables, sin implementación pública | `DIAG-SPEC-001`, foundations STD-0.1A |
| `DIAG-RUNTIME-001` | Registro de task/thread, eventos de memoria, roots, recursos, source maps, scheduler y quiescencia en VM hosted | `DIAG-SPEC-001`, contratos runtime-facing B0, VM hosted |
| `RACE-001` | Detector VM sobre tasks, memoria, unsafe y primitivas internas; corpus positivo/negativo | `DIAG-RUNTIME-001` |
| `LEAK-001` | Retención GC y recursos hosted con snapshots reproducibles | `DIAG-RUNTIME-001` |
| `DUMP-001` | Captura lógica `.tdump`, redacción y analizador | `DIAG-SPEC-001`, source maps VM |
| `DIAG-TEST-001` | Integración por intento, retry, shard, JSON/JUnit y artifacts | `RACE-001`, `LEAK-001`, `DUMP-001` |
| `DIAG-CI-001` | Lanes, budgets, fuzzing, regression corpus y promotion gate | `DIAG-TEST-001`, `PERF-001` |
| `DIAG-NATIVE-001` | Paridad nativa de race/leaks/dumps, roots/retainers, threads, unwind y source maps | backend elegido, memoria/ABI/lowering nativos, `NATIVE-THREAD-001`, detectores VM |
| `DIAG-STDLIB-001` | Adapters de detector para channel/sync/executor/net y corpus VM/native | implementaciones STD-0.1B aplicables, `DIAG-NATIVE-001` |

`RACE-001` anterior al backend cubre tasks, memoria, unsafe y primitivas
internas del runtime; no afirma todavía cobertura de las APIs públicas de
channel, sync, executor o net. Esa integración pertenece a
`DIAG-STDLIB-001`, después de implementar sus owners.

`NATIVE-001` no puede seleccionar un backend sin evaluar cómo conserva el
registro de tasks/threads, hooks de memoria/GC, source maps, unwind y crash
dumps. `NATIVE-MEM-ADR-001` y `NATIVE-ABI-001` incorporan estos requisitos; la
implementación de STD-0.1B no publica APIs paralelas para satisfacerlos.
`LEAK-001` no depende del modelo de memoria nativo: primero prueba la VM y el
ledger hosted; `DIAG-NATIVE-001` prueba después ARC/ciclos/FFI nativos. La
implementación hosted está cerrada por `crates/tondo-vm/src/runtime/leak.rs`,
su contrato y su registro machine-readable; no implica soporte nativo ni del
runner de tests.

## 8. Fronteras normativas de `DEC-018`

`DEC-018` acepta una única superficie de diagnóstico opt-in, mantiene intacto
`tondo-diagnostics-json/1`, prohíbe keywords y APIs paralelas en `std`, exige
estado `unsupported-diagnostic-profile` explícito y fija redacción/payloads
fuera del envelope por defecto. La instrumentación VM hosted ejecutable está
documentada por [`diagnostic-runtime.md`](diagnostic-runtime.md), el detector
de races por [`diagnostic-race.md`](diagnostic-race.md) y el detector de
retención por [`diagnostic-leak.md`](diagnostic-leak.md). Crash dumps, runner y
paridad native siguen pendientes.

## 9. No objetivos de esta revisión

- No se promete soporte de todas las plataformas antes de elegir backend.
- No se añade un profiler general, un heap snapshot público ni un uploader.
- No se trata un reporte limpio como prueba estática de ausencia de races o
  fugas.
- No se marca ningún bloque `NATIVE-*` o S1A como implementado por la mera
  existencia de este contrato. `DUMP-001`, al igual que `RACE-001` y
  `LEAK-001`, solo está cerrado por evidencia ejecutable de su propio contrato.
