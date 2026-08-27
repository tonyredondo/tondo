# Contrato de diagnóstico nativo

**Estado:** `closed` para `DIAG-NATIVE-001` en Tondo 0.1.

Este contrato demuestra que una ejecución nativa real conserva los observables
lógicos de los detectores hosted. El adaptador genera un objeto y un ejecutable
independiente para cada backend candidato (Cranelift y LLVM), ejecuta el mismo
probe y compara el envelope `tondo-diagnostic-report/1` con el mismo fixture
determinista. No se comparan direcciones, layouts, nombres de símbolos,
stacks físicos ni paths del workspace.

La definición machine-readable es
[`testing/native-diagnostics.json`](../../testing/native-diagnostics.json). La
evidencia de la ejecución se publica en el campo `native_diagnostics` de
`target/reliability/evidence/native-evaluation-runner.json` y se produce con
[`scripts/native-diagnostics.sh`](../../scripts/native-diagnostics.sh).

## 1. Frontera ejecutable

La lane usa el lowering común `tondo-mir-backend/1`; no añade un frontend ni
una semántica paralela. El adapter privado del runtime expone solo durante la
campaña:

| Perfil | Casos | Evidencia mínima |
| --- | --- | --- |
| `race` | `race-conflict`, `race-clean` | dos task IDs lógicos, dos aristas happens-before y source maps |
| `leaks` | `leak-growth`, `leak-clean`, `arc-cycle-reclaimed` | roots/retainers, ledger FFI/recursos y ciclos ARC recuperados |
| `crash` | `crash-dump`, `crash-corruption-rejected`, `crash-limit-enforced` | task/thread IDs, unwind, source maps, ledger y límites fail-closed |

Los casos se ejecutan como procesos nuevos. Un caso finding esperado no hace
verde un backend por devolver un código fijo: el ejecutable debe atravesar el
runtime y sus campos privados, y el runner rechaza cualquier discrepancia
entre Cranelift y LLVM.

## 2. Envelope y privacidad

El envelope compacto conserva identidad (`format`, `profile`, `case`, `mode` y
`status`) y contadores lógicos de tasks, threads, sincronización, roots,
retainers, ciclos, FFI, recursos, unwind y source maps. Los campos
`redacted` y `payloads_omitted` deben ser verdaderos en todos los casos.

No se serializan payloads de usuario, direcciones, punteros, IDs de proceso,
IDs físicos de OS, timestamps ni paths. El adapter falla cerrado si un campo
desconocido aparece o si la captura no puede representar la redacción,
corrupción o límite que el caso exige.

## 3. ABI privada del runtime

El runtime nativo mantiene la captura opt-in como estado de proceso:

```text
tondo_rt_diag_reset()
tondo_rt_diag_probe(profile, mode)
tondo_rt_diag_field(field)
```

Los tres símbolos son internos al compiler/runtime y no forman parte de la FFI
de usuario. `tondo_rt_diag_probe` recorre las mismas transiciones de task,
frame, root, retain/release, cleanup, worker y collector que el runtime
generado. Un perfil o modo desconocido devuelve `unsupported` y no inventa una
observación parcial.

El objeto C de la herramienta es un harness diferencial mínimo: refleja la
misma ABI lógica para poder enlazar el objeto de cada backend y mantener el
oráculo independiente de la representación Rust privada. La implementación
ARC real y sus pruebas viven en
[`native-arc.md`](native-arc.md); el harness no fija su layout.

## 4. Dumps y límites físicos

`crash-dump` verifica el envelope lógico de dump, su cleanup, unwind/source-map
summary, redacción y ledger. `crash-corruption-rejected` y
`crash-limit-enforced` verifican que la captura rechaza corrupción y límites en
vez de publicar datos incompletos como si fueran válidos.

La lane portable no afirma que todos los targets puedan interceptar una señal
fatal ni conservar registros físicos. Esa capacidad se expresa como
`unsupported` hasta que el adaptador de target aporte una ruta
async-signal-safe; la selección de targets y el smoke sobre hardware pertenecen
a `NATIVE-TARGET-001`. Esta limitación explícita no cambia el envelope ni
permite degradar silenciosamente un target que declare soporte.

## 5. Gate y siguiente trabajo

El checker exige el contrato, los ocho casos, ambos backends, el campo de
reporte y los símbolos privados; su test crea variantes negativas para
formato, identidad, privacidad, backend y límites. El runner completo añade la
sección nativa al informe existente y requiere que cada envelope de Cranelift
sea exactamente igual al de LLVM.

Con este bloque cerrado, el siguiente consumidor es
`NATIVE-STD-HOSTED-001`. La conformidad de la stdlib, los adaptadores públicos y
la capacidad física por target no se dan por implementados por la existencia de
este contrato.
