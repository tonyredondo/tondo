# Contrato de CI para diagnóstico dinámico

**Estado:** `implemented` para la VM hosted de Tondo 0.1
(`DIAG-CI-001`). **Registro:**
[`testing/diagnostic-ci.json`](../../testing/diagnostic-ci.json).

Este contrato convierte los detectores de `RACE-001`, `LEAK-001` y `DUMP-001`
en evidencia repetible de CI. La lane es opt-in: no se ejecuta en cada push ni
forma parte de la baseline normal de coverage, mutation o rendimiento. Cuando
se selecciona un perfil, sin embargo, sus resultados son obligatorios: un perfil
`unsupported`, un límite alcanzado, un hallazgo o un fallo del toolchain hacen
fallar la campaña.

## 1. Selección y aislamiento

La workflow dedicada acepta `race`, `leaks`, `crash` o `all`. `all` se expande
en el orden fijo `race`, `leaks`, `crash`; no puede mezclarse con otro perfil.
Cada lane ejecuta los casos positivos y negativos declarados en el registro y
reutiliza el runner de `DIAG-TEST-001`. No existe una ruta CI paralela al
runner ni una configuración de proyecto.

Cada caso usa un proceso nuevo (`fresh-process`). La campaña no comparte collector,
heap, scheduler, ledger, roots, artefactos o estado de retries entre casos,
lanes o shards. El reporte de la campaña conserva `run_id`, `attempt_id`,
`profile`, `source_revision`, target y toolchain. El informe de una lane solo
puede promoverse si todos sus perfiles requeridos terminaron en `clean`; un
`unsupported` nunca se convierte en verde.

## 2. Corpus de regresión

El corpus persistente vive en `fuzz/corpus/diagnostics/` y tiene tres raíces:

```text
positive/      entradas que deben conservar una observación positiva
negative/      entradas que deben permanecer limpias o rechazar de forma segura
regressions/   entradas minimizadas provenientes de un fallo confirmado
```

Cada perfil tiene al menos una entrada positiva y una negativa. Una entrada
minimizada no se borra al pasar la campaña: se copia a `regressions/`, se
reproduce con `-runs=0` y entra en el gate determinista. El checker rechaza
carpetas ausentes, archivos vacíos, perfiles sin ambos lados o referencias a
tests inexistentes.

El corpus no es una prueba de ausencia global. Los positivos prueban que el
detector conserva una observación conocida; los negativos prueban que no
introduce falsos positivos en las trazas cerradas y que las entradas corruptas
fallan de manera explícita.

## 3. Fuzzing acotado

El target `diagnostics` del workspace `fuzz/` genera trazas bounded de race,
leak y dump, ejecuta cada analizador dos veces y compara el resultado
serializado. El target no recibe payloads de usuario ni acceso a red.

La campaña PR/manual de diagnóstico usa `nightly-2026-07-28`,
`cargo-fuzz 0.13.2`, 128 ejecuciones, semillas fijas, entrada máxima de 64 KiB,
timeout de 10 segundos y RSS máximo de 4 GiB. La campaña extendida mantiene
los mismos límites y usa como máximo 180 segundos por target. Un crash o
resultado no determinista detiene la lane; su input minimizado debe quedar en
`regressions/` antes de promover otra vez.

## 4. Budgets

Cada lane tiene límites fail-closed de eventos, reporte, dump y duración. La
medición de overhead compara la misma fixture en tres procesos limpios sin
instrumentar y con el perfil activo; se conserva la mediana y no se descartan
outliers. Los límites de overhead son específicos de perfil y están en basis
points en el registro:

| Perfil | Overhead máximo por defecto | Duración máxima de una lane |
|---|---:|---:|
| `race` | 100000 bp (10×) | 120 s |
| `leaks` | 50000 bp (5×) | 120 s |
| `crash` | 20000 bp (2×) | 120 s |

Estos presupuestos son de la instrumentación. No modifican ni sustituyen los
umbrales de `PERF-001`, `testing/quality-baseline.json` o la cobertura normal.
Una regresión de overhead falla la lane aunque la suite funcional sea verde;
una mejora en una dimensión no compensa una violación en otra.

## 5. Promotion gate y artefactos

La promoción requiere, en orden:

1. contrato y negativos válidos;
2. corpus positivo/negativo y replay determinista;
3. observación positiva y negativa para cada perfil seleccionado;
4. fuzz smoke con toolchain fijado;
5. presupuestos de overhead y límites respetados; y
6. workflow opt-in verde, con artefactos retenidos.

La workflow [`diagnostics.yml`](../../.github/workflows/diagnostics.yml) solo
sube logs, reportes y entradas minimizadas bajo
`target/reliability/diagnostics-ci/` y `fuzz/artifacts/`. Los payloads, secretos,
rutas físicas y variables ambientales no se suben. Los artefactos se conservan
14 días para una ejecución normal y no son parte de una release.

La campaña publica un resumen JSON con el resultado por perfil, identidad de
toolchain, semillas, conteos del corpus, límites de lane, límites de reportes,
hashes del contrato/corpus y el hash de la baseline normal. Un resumen
incompleto, un perfil `unsupported` o una referencia a baseline normal alterada
hace fallar la promoción.

## 6. Límites y fronteras

La lane hosted no implementa adapters públicos de `std.channel`, `std.sync`,
`std.executor` o `std.net`, ni captura física de registros/unwind. La paridad
lógica nativa ya está cerrada por `DIAG-NATIVE-001`; los adapters públicos y la
capacidad física por target siguen en `DIAG-STDLIB-001` y sus gates.
`DIAG-CI-001` únicamente demuestra que la implementación hosted existente se
ejecuta de manera aislada, reproducible y con límites observables antes de
seleccionar el backend nativo.
