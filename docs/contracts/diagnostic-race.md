# Contrato del detector dinámico de races

**Estado:** `implemented` para la VM hosted de Tondo 0.1
(`RACE-001`). **Superficie:** analizador Rust interno de `tondo-vm`; no es una
API fuente ni una API de la stdlib.

`race` consume `tondo-diagnostic-runtime/1` y produce un informe mínimo
`tondo-diagnostic-race/1`. Solo analiza accesos observados en la ejecución que
generó la traza. Un resultado `clean` significa que no se observó un conflicto
sin orden happens-before en esos caminos; nunca es una prueba estática.

## 1. Modelo

El analizador mantiene un vector clock por task. Cada acceso de memoria avanza
el reloj de su task. Las siguientes observaciones establecen orden:

- `Spawn` copia el reloj del padre al hijo;
- `Wake` incorpora el reloj de quien despierta al task despertado;
- `Join` incorpora el reloj de la tarea completada al waiter;
- `SelectRegister`/`SelectCommit` incorporan los arms seleccionados; y
- las fronteras host y las reservas de loans avanzan el reloj sin inventar
  orden cuando no existe un peer explícito.

Dos accesos de tasks distintos son un conflicto si comparten ubicación, uno es
`Write` o `Move`, y ninguno precede al otro según los relojes. Lectura/lectura,
accesos del mismo task y accesos a locales de tasks distintos no son races.
La detección es determinista y conserva el orden de observación.

## 2. Identidad y evidencia

Cada ubicación compartida usa `storage_id + path_hash`. `storage_id` es la
identidad generacional del handle de heap; `path_hash` es un FNV-1a estable de
las proyecciones resueltas y no contiene payload de usuario en la traza. Un
acceso sin handle compartido se identifica como
`task_id + frame + slot + path_hash`, evitando aliasar dos locales.

Cada `RaceAccess` incluye secuencia, task/thread, tipo de acceso, rango, fuente
y stack acotado. Cada finding conserva los dos accesos, su ubicación, una
creation stack disponible y `missing_happens_before = true`. Los stacks están
limitados por `DiagnosticConfig.max_stack_depth` (256 por defecto).

## 3. Estados y fallo cerrado

El informe tiene uno de estos estados:

- `clean`: no hubo finding ni limitación;
- `finding`: hubo al menos un conflicto observado y la traza fue suficiente; o
- `unsupported`: la traza fue truncada, faltó lifecycle/source map, faltó una
  relación de sincronización referenciada o se alcanzó un límite.

Los límites del detector son 100.000 observaciones y 100.000 findings por
análisis. Superarlos no descarta silenciosamente datos: se añade una
limitación y el estado pasa a `unsupported`. Una configuración inválida
también es `unsupported`. El payload de usuario, los valores de map keys y el
contenido de strings no se incluyen en el informe.

El runtime hospedado cubre memoria gestionada, loans, tareas, scheduler,
suspensión y primitivas internas observables. Los adapters de las APIs públicas
`channel`, `sync`, `executor` y `net`, la paridad nativa y el CLI de reportes
pertenecen a `DIAG-STDLIB-001`, `DIAG-NATIVE-001` y `DIAG-TEST-001`; no se
declaran implementados por este bloque.

## 4. API Rust y verificación

El crate reexporta únicamente tipos de datos y dos funciones puras sobre una
traza:

```rust
detect_races(&trace)
detect_races_with_config(&trace, RaceConfig { .. })
```

La instrumentación permanece opt-in y privada (`execute_with_diagnostics`).
Las pruebas unitarias cubren conflictos positivos, orden por `Join`/`Wake`,
locales, stacks, repetibilidad y límites fail-closed. Los contratos
machine-readable y sus negativos están en
`testing/diagnostic-race.json`, `scripts/diagnostic-race-check.sh` y
`scripts/diagnostic-race-test.sh`.
