# Contrato del runner de diagnóstico en `tondo test`

**Estado:** `implemented` para la VM hosted de Tondo 0.1
(`DIAG-TEST-001`). **Registro:**
[`testing/diagnostic-test.json`](../../testing/diagnostic-test.json).

Este contrato cierra la integración entre los detectores hosted ya existentes y
el runner público de tests. No crea un segundo runner, no cambia la semántica
de un test y no convierte un reporte limpio en una prueba estática de ausencia.
La campaña normal sigue siendo la fuente de la baseline de coverage, mutation y
performance; `--diagnostics` es un perfil opt-in que añade evidencia.

## 1. Superficie y selección

```text
tondo test --diagnostics race ...
tondo test --diagnostics leaks,crash ...
tondo test --diagnostics all ...
```

Los perfiles aceptados son exactamente `race`, `leaks` y `crash`. `all` se
expande de forma determinista a `race`, `leaks`, `crash`; no puede combinarse
con otro token. Los duplicados, tokens vacíos y perfiles desconocidos son
errores de uso. `--list` no ejecuta código y por tanto rechaza
`--diagnostics`.

El perfil se transmite desde el plan CLI al proceso worker sin configuración de
proyecto, variable de entorno ni keyword. La revisión, el selector, el shard,
el orden/seed, el retry/repeat, los jobs y la lista ordenada de tests forman la
identidad determinista de `run_id`; `source_revision` liga los bytes y paths
lógicos de las fuentes.

## 2. Aislamiento y lifecycle

Cada invocación de worker recibe una identidad de `invocation`. El runner
mantiene un proceso nuevo por intento: un retry, repeat, shard y suite no
reutilizan el collector, heap, scheduler, ledger, roots ni dump de otro
intento. La misma suite puede devolver varios leaves, pero cada leaf se
proyecta en su `attempt_id` y el reporte conserva el shard y la invocation que
lo produjo. Setup, body y teardown se incluyen en la misma observación del
worker; un bloqueo de setup/teardown también recibe un resultado explícito.

El wire protocol de worker cambió a `/2` al añadir `diagnostics`. Un proceso
con formato anterior se rechaza como infraestructura incompleta; nunca se
interpreta como una ejecución sin diagnóstico.

## 3. Reportes por intento

Cada `TestAttempt` puede contener cero o más `DiagnosticRecord` en el orden
cerrado `race`, `leaks`, `crash`. Cada registro usa
`tondo-diagnostic-report/1` y contiene:

- `run_id`, `attempt_id`, `shard`, `profile`, `target`, `backend`, `toolchain`
  y `source_revision`;
- `status`: `clean`, `finding`, `unsupported` o `failed`;
- número de observaciones y una lista ordenada de `limitations`;
- exit status del programa y del comando; y
- política de privacidad fija: payloads omitidos, secretos nunca emitidos,
  paths lógicos y sin subida de red.

`unsupported` es un estado visible, no una lista vacía ni un éxito implícito.
Si el runtime no produjo una traza, si el target carece del perfil o si se
alcanza un límite, el registro conserva la razón en `limitations`. Un hallazgo
dinámico no modifica el estado semántico del test, pero sí determina el exit
status de la campaña: fallo de toolchain `3`, unsupported `2`, finding `1`, y
después el resultado ordinario del test.

## 4. Artifacts y dumps

Los artifacts de diagnóstico se incorporan al mismo `ArtifactStore` del test.
El objeto se identifica por SHA-256 y el descriptor del intento contiene el
nombre, media type, tamaño, hash y referencia `objects/<sha256>`. El runner
verifica el digest antes de publicar; un digest inconsistente es un fallo de
toolchain. El payload no se duplica dentro del JSON/JUnit: los formatos solo
proyectan el descriptor y el registro.

El perfil `crash` puede publicar `diagnostic-crash.tdump` con el envelope
`tondo-dump/1`. El límite del reporte es 16 MiB y el del dump 256 MiB; superar
el límite es `unsupported` con `report-byte-limit`, nunca truncado silencioso.
El dump se analiza offline con `tondo dump analyze`.

## 5. JSON, JUnit y privacidad

El JSON canónico existente conserva `attempts[*].diagnostics` y
`attempts[*].artifacts`. La proyección JUnit mantiene exactamente la misma
información en la propiedad `tondo.diagnostics` y en `tondo.artifacts`; no hay
una semántica alternativa ni un segundo formato de payload.

Los reportes no incluyen payload de usuario, secretos, rutas físicas ni subida
automática. Las limitaciones y redacciones son parte del contrato y se
validan antes de serializar. Los artifacts quedan content-addressed y el
`run_id`/`attempt_id` permite atribuirlos a retry, repeat y shard concretos.

## 6. Evidencia y fronteras

La implementación ejecutable se encuentra en:

- `crates/tondo-cli/src/main.rs`: propagación al worker, aislamiento,
  clasificación, artifacts, exit status e IDs;
- `crates/tondo-cli/src/test_cli.rs`: parser cerrado de perfiles;
- `crates/tondo-compiler/src/driver.rs`: traza opt-in en el pipeline de tests;
- `crates/tondo-compiler/src/test_result.rs`: modelo y validación por intento;
  y
- `crates/tondo-compiler/src/test_junit.rs`: proyección lossless de diagnóstico.

Los gates `scripts/diagnostic-test-check.sh` y
`scripts/diagnostic-test-test.sh` comprueban el contrato, sus negativos y la
frontera `DIAG-CI-001`. El siguiente bloque añade lanes opt-in, corpus
persistent, fuzzing y presupuestos de overhead; no cambia esta superficie ni
la baseline normal.
