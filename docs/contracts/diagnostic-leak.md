# Contrato del detector dinámico de retención y recursos

**Estado:** `implemented` para la VM hosted de Tondo 0.1
(`LEAK-001`). **Superficie:** analizador Rust interno de `tondo-vm`; no es una
API fuente ni una API de la stdlib.

`leaks` consume `tondo-diagnostic-runtime/1` y produce un informe
`tondo-diagnostic-leak/1`. Solo clasifica evidencia observada en la ejecución
que generó la traza. Un resultado `clean` no es una prueba estática de ausencia
de fugas.

## 1. Modelo de memoria gestionada

La VM hosted usa un GC de trazado preciso. Por tanto, un ciclo que deja de ser
alcanzable y que el GC recupera no es una fuga. El detector construye snapshots
únicamente al cerrar una barrera de quiescencia (`Begin` seguido de una raíz y
`End`). Cada raíz conserva sus IDs y retainers lógicos; el payload de usuario
no entra en el informe.

Un objeto gestionado es evidencia de retención cuando aparece en al menos dos
snapshots y la campaña muestra crecimiento sostenido del conjunto retenido. Un
único valor devuelto o una raíz estable no se marca por sí mismo. El finding de
`SustainedGrowth` incluye las métricas de todos los snapshots que sostienen la
clasificación, y cada objeto retenido conserva su ID generacional, tamaño,
owner, primera/última observación y stack de asignación.

La política de crecimiento es deliberadamente conservadora: por defecto exige
tres snapshots y que cada delta aumente objetos retenidos o bytes retenidos.
`LeakConfig.min_growth_snapshots` permite endurecer el mínimo, pero nunca
acepta menos de dos snapshots.

## 2. Recursos afines y memoria nativa

El ledger de `Resource` se analiza separado del heap. Un recurso cuyo estado
final es `Acquired` sin `Released` es un finding `AffineResource`. Los IDs y el
kind se mantienen opacos; cada finding conserva owner, primer/último evento y
stack de adquisición cuando el runtime lo conoce.

Los recursos cuyo kind identifica explícitamente una frontera `ffi`, `native`
o `allocation` se clasifican como `NativeAllocation`. Esta clasificación no
afirma paridad con ARC/FFI del backend nativo: la implementación de esos hooks
pertenece a `DIAG-NATIVE-001`. Recursos liberados no producen findings.

## 3. Quiescencia, límites y fallo cerrado

La traza debe contener al menos un ciclo completo de quiescencia con snapshot
de raíces. Una raíz sin asignación observada, una barrera sin raíces, una
secuencia incompleta o una traza truncada hacen que el informe sea
`unsupported`; nunca se inventa una conclusión limpia.

Los límites del análisis son 100.000 observaciones y 100.000 findings por
defecto. El límite de crecimiento por defecto es de tres snapshots. Al
alcanzar un límite se añade una limitación y el estado pasa a `unsupported`.
Las limitaciones son deterministas y se ordenan antes de publicar el informe.

Los estados son:

- `clean`: no hay findings ni limitaciones;
- `finding`: hay retención, crecimiento o recursos sin terminal y la traza es
  suficiente; o
- `unsupported`: la evidencia está truncada o incompleta, o se alcanzó un
  límite/configuración inválida.

Cada intento se ejecuta en un proceso nuevo. Esta propiedad, garantizada por
`DIAG-TEST-001` y ya fijada en `DIAG-SPEC-001`, evita que retries, shards o
suites hermanas hereden el heap o el ledger de recursos de otro intento.

## 4. API Rust y privacidad

El crate reexporta únicamente tipos de informe y dos funciones puras sobre una
traza:

```rust
detect_leaks(&trace)
detect_leaks_with_config(&trace, LeakConfig { .. })
```

La instrumentación sigue siendo opt-in mediante `execute_with_diagnostics`.
Los eventos de heap y recursos llevan source/stack acotados a
`DiagnosticConfig.max_stack_depth` (256 por defecto). No se incluyen strings,
map keys, bytes de usuario, paths físicos ni red; la identidad es la del
runtime hosted y sus IDs generacionales.

Las pruebas cubren ciclos inalcanzables recuperados, crecimiento monotónico,
retención con stacks, recursos liberados y no liberados, clasificación FFI,
quiescencia ausente, truncado, límites y determinismo. El registro
machine-readable y sus negativos están en `testing/diagnostic-leak.json`,
`scripts/diagnostic-leak-check.sh` y `scripts/diagnostic-leak-test.sh`.

La paridad nativa, los adapters de `channel`/`sync`/`executor`/`net` y la
integración por intento del CLI permanecen en `DIAG-NATIVE-001`,
`DIAG-STDLIB-001` y `DIAG-TEST-001` respectivamente.
