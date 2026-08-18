# RFC-019: tooling dinámico de diagnóstico

**Estado:** propuesta de planificación para Tondo 0.1
**Propietario:** toolchain/runtime
**Contrato:** [`docs/contracts/diagnostic-tooling.md`](../contracts/diagnostic-tooling.md)

## Resumen

Tondo necesita diagnosticar los fallos que más tiempo consumen en aplicaciones
reales: races, retención de memoria/recursos y terminaciones fatales. La
propuesta añade perfiles dinámicos opt-in al CLI y un analizador de dumps, con
una semántica común entre la VM y el futuro backend nativo. No añade keywords,
no duplica APIs sync/async y no convierte la stdlib en un framework de
instrumentación.

La motivación toma como referencia los límites conocidos de los detectores
dinámicos: Go documenta que su race detector observa únicamente rutas
ejecutadas y tiene un coste sustancial, mientras que ThreadSanitizer y
LeakSanitizer separan instrumentación de runtime y dejan claro que no son
pruebas estáticas ni sustitutos de un diseño de ownership. Tondo conserva esa
honestidad en sus reportes y gates:

- [Go race detector](https://go.dev/doc/articles/race_detector)
- [Clang ThreadSanitizer](https://clang.llvm.org/docs/ThreadSanitizer.html)
- [Clang LeakSanitizer](https://clang.llvm.org/docs/LeakSanitizer.html)
- [Go diagnostics](https://go.dev/doc/diagnostics)

## Decisiones

1. **Un perfil, una frontera:** `tondo run/test --diagnostics ...` activa
   instrumentación; `tondo dump analyze` analiza artefactos. El spell final se
   fija en `DIAG-SPEC-001`, pero no habrá comandos paralelos por cada backend.
2. **Runtime primero:** la VM hosted implementa los eventos y el formato
   lógico antes de que `NATIVE-001` compare backends. El backend nativo debe
   conservar esos observables y aportar source maps, unwind y registros.
3. **Evidencia por intento:** cada test y retry tiene un proceso y un reporte
   propios. Race/leak/crash no comparten estado entre suites, shards o retries.
4. **Sin APIs duplicadas:** `std` sigue describiendo operaciones de programa;
   el runtime posee hooks privados y el CLI posee controles opt-in. Un futuro
   hook público necesita un RFC independiente.
5. **Privacidad cerrada:** los dumps no se suben, los payloads de usuario se
   redactan por defecto y cada campo no disponible queda señalado, nunca
   inventado.

## Fases de implementación

| Fase | Bloques | Resultado verificable |
|---|---|---|
| D0 Contrato | `DIAG-SPEC-001` | Schema de reporte/dump, perfiles, identidad, privacidad, exit status y negativos |
| D0.5 Fronteras runtime | `STD-CONC-001`, `STD-SYNC-001`, `STD-EXEC-001`, `STD-NET-001` | Contratos de eventos que consumirá la instrumentación, sin implementar todavía los owners |
| D1 Instrumentación VM | `DIAG-RUNTIME-001` | Registro de task/thread, eventos de memoria/sync, roots, recursos, source maps y quiescencia |
| D2 Detectores | `RACE-001`, `LEAK-001` | Corpus positivo/negativo, reducción de reportes, límites y coste medido |
| D3 Dumps | `DUMP-001` | Captura segura, redacción, fixtures `.tdump`, analizador human/JSON y corrupción rechazada |
| D4 Runner | `DIAG-TEST-001` | Artifacts por intento, retries aislados, sharding, JUnit/JSON y clasificación unsupported |
| D5 CI | `DIAG-CI-001` | Lanes opt-in, fuzzing, regression corpus, budgets y promotion gate sin alterar baseline normal |
| D6 Native | `NATIVE-001`, `NATIVE-MEM-ADR-001`, `NATIVE-ABI-001`, `DIAG-NATIVE-001` | Paridad VM/native de eventos, unwind, source maps, roots, cleanup y dumps |
| D7 Owners B | `DIAG-STDLIB-001` | Adapters y corpus de channel/sync/executor/net sobre VM y nativo antes de S1 |

Cada fase mantiene tests ejecutables y evidencia separada. Una fase no puede
declarar soporte por compilar un stub, incluir una ruta en el inventario o
producir un reporte sin una observación positiva/negativa real.

## Modelo técnico

### Race

El runtime registra accesos, task/thread IDs, stacks de creación y edges de
happens-before. `Ref[T]`, `Pointer[T]`, `unsafe`, FFI, locks, channels,
atomics, `spawn`, `Join` y suspensión son fronteras explícitas del modelo. La
campaña reporta conflictos observados y no intenta demostrar ausencia global.

### Leaks

El detector toma snapshots de roots/retainers tras quiescencia y mantiene un
ledger de recursos afines y allocations FFI. La recuperación de un objeto
inalcanzable por el GC no es una fuga; sí lo son la retención sin owner, el
recurso sin operación terminal y el crecimiento sostenido fuera de una policy
de caché declarada.

### Crash dump

El capturador registra un envelope `.tdump` versionado. La ruta de señal no
asume asignación ni locks; un helper escribe el objeto cuando el host lo
permite. El analizador nunca ejecuta el programa ni consulta red y rechaza
versiones, hashes o secciones corruptas.

## Integración con planes existentes

- `PERF-001` conserva la baseline sin instrumentación; cada perfil publica su
  propio coste y presupuesto.
- La spec de testing conserva logs, tags, artifacts, JUnit, retries y shards;
  el diagnóstico se añade como descriptor por intento, no como un segundo
  runner.
- `NATIVE-001` compara debugging y diagnóstico junto a corrección,
  rendimiento, memoria, distribución y mantenimiento.
- `NATIVE-MEM-ADR-001` incluye roots, retain/release, recursos, ciclos,
  cancelación y tasks abandonadas.
- `NATIVE-ABI-001` incluye calling convention, unwind, source maps, IDs de
  task/thread y hooks internos, sin prometer ABI FFI pública.
- `STD-0.1B` expone channels/sync/executor/net normales; no se bloquea el
  diseño con una API detectora duplicada.

## Riesgos y mitigaciones

| Riesgo | Mitigación |
|---|---|
| Sobrecoste hace inviable CI | Lanes explícitas, budgets registrados y campañas pequeñas reproducibles |
| Falsos positivos por GC/caches | Separar unreachable, retained y resource ledger; policy declarada |
| Diferencias VM/native | Oracle de valores/errores/orden/cleanup y evento común antes de promover |
| Fugas de secretos en dumps | Redacción por defecto, sin upload y hashes/metadata en vez de payloads |
| Detector incompleto se interpreta como prueba | `observed-only`, limitations obligatorias y estado unsupported explícito |
| Hooks públicos congelan el runtime | Hooks privados primero; cualquier superficie pública requiere RFC propia |

## Criterio de promoción

El tooling entra en el baseline Tondo 0.1 cuando D0–D5 tienen contratos,
fixtures, negativos, fuzzing acotado, reportes reproducibles y CI verde; D6
añade la paridad del backend nativo antes de Gate N1. Hasta entonces todos los
bloques permanecen abiertos en el tracker aunque la VM tenga prototipos
internos.
