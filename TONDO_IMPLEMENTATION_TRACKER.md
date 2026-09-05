# Tondo: tracker de implementación

**Estado:** M0–M10.7 conservan su implementación y los gates vivos H0 y T0
validan el borrador actual. G5 permanece abierto hasta el primer proceso real
de release; no existe un candidato pre-release ni una revisión histórica que
mantener. `STD-CODEC-PUBLIC-001` ya cerró la auditoría pública global: las 214
firmas tienen ruta contract → HIR → lowering → host/VM → caso público y los
tres owners sin runtime están indexados como `build-only`/`not-applicable` con
razón verificable. STD-0.1A/S1A está sellado como draft técnico; la
distribución VM, las dimensiones de rendimiento y la conformidad pública están
promovidas con evidencia ejecutada y un bundle content-addressed independiente.
`STD-A-FUZZ-001` ya cerró la dimensión
FUZZ para los 22 owners y no se sobreafirma un cierre global.
La forma TLF para agentes ya tiene spec y estudio léxico, pero encoder, decoder,
source maps, CLI y evaluación de generación permanecen pendientes. Tondo 0.1
sigue en desarrollo y no ha sido publicado.

Gate N1 ya está cerrado por el contrato composicional `testing/native-n1.json` y
la evidencia hash-bound `target/reliability/evidence/native-n1.json`: Cranelift
queda promovido para el producto AOT primario
`x86_64-unknown-linux-gnu`. La entrada física Linux ARM64 queda como smoke de
candidato hasta completar allí la campaña AOT completa; Windows y macOS siguen
siendo probes de portabilidad. Este cierre no publica Tondo, no cierra G5/S1 ni
promete una ABI pública.

El producto primario previsto para Tondo 0.1 es un ejecutable nativo AOT
(`native-aot`). `tondo-vm-hosted` permanece como implementación de referencia,
oráculo diferencial y opción bootstrap/hosted; no es una segunda semántica del
lenguaje. JIT no es un perfil de producto ni una dimensión de `DEC-013` en
0.1. `DEC-013` selecciona Cranelift para la ruta AOT de
`x86_64-unknown-linux-gnu`; LLVM se conserva como comparativa experimental.
La decisión se limita al mismo MIR, runtime, stdlib, target y protocolo de
medición; la promoción efectiva queda registrada únicamente por Gate N1.
El contrato del lenguaje sigue siendo independiente del recolector; para la
ruta AOT nativa `NATIVE-MEM-ADR-001` fija `hybrid-arc-cycle-collector`, mientras
la VM conserva el tracing GC de referencia de `ADR-009`.

El tooling dinámico de diagnóstico también queda dentro del plan 0.1: race
detection, detección de retención/leaks y crash dumps conservan una única
frontera compilador/runtime/CLI, con evidencia por intento y paridad VM/native.
`DIAG-SPEC-001` es el prerrequisito explícito de la evaluación nativa y
`DIAG-RUNTIME-001` ya cerró la instrumentación interna de la VM hosted;
`RACE-001`, `LEAK-001`, `DUMP-001` y `DIAG-TEST-001` ya cerraron sus lanes
hosted; `DIAG-CI-001` y `DIAG-NATIVE-001` también están cerrados, con paridad
lógica ejecutable entre Cranelift y LLVM. La captura de señales físicas sigue
siendo una capacidad declarada por target.

**Última actualización:** 2026-09-05

**Especificaciones normativas:**

- [Borrador normativo de Tondo 0.1](./TONDO_LANGUAGE_SPEC.md)
- [Arquitectura base de Standard Library 0.1](./TONDO_STANDARD_LIBRARY_SPEC.md)
- [Contrato normativo del toolchain 0.1](./TONDO_TOOLCHAIN_SPEC.md)
- [Contrato de testing para Tondo 0.1](./TONDO_TESTING_SPEC.md)

**Contratos normativos por owner:**

- [Contrato global de baseline de rendimiento previo al backend](./docs/contracts/performance.md)
- [Contrato de tooling dinámico de diagnóstico](./docs/contracts/diagnostic-tooling.md)
- [Contrato de instrumentación runtime de diagnóstico](./docs/contracts/diagnostic-runtime.md)
- [Contrato del detector dinámico de races](./docs/contracts/diagnostic-race.md)
- [Contrato del detector dinámico de retención y recursos](./docs/contracts/diagnostic-leak.md)
- [Contrato del dump lógico de diagnóstico](./docs/contracts/diagnostic-dump.md)
- [Contrato operativo de rendimiento de Standard Library 0.1](./docs/contracts/stdlib-performance.md)
- [Contrato de owner de `std.json`](./docs/contracts/stdlib-json.md)
- [Contrato de owner de `std.messagepack`](./docs/contracts/stdlib-messagepack.md)
- [Contrato de owner de `std.protobuf`](./docs/contracts/stdlib-protobuf.md)
- [Contrato de owner de `std.encoding`](./docs/contracts/stdlib-encoding.md)
- [Contrato de tests de `std.encoding`](./docs/contracts/stdlib-encoding-test.md)
- [Contrato de rendimiento de `std.encoding`](./docs/contracts/stdlib-encoding-performance.md)
- [Contrato de conformance VM/native de `std.encoding`](./docs/contracts/stdlib-encoding-conformance.md)
- [Contrato de owner de `std.yaml`](./docs/contracts/stdlib-yaml.md)
- [Contrato de owner de `std.serialization`](./docs/contracts/stdlib-serialization.md)
- [Contrato de owner de `std.testing`](./docs/contracts/stdlib-testing.md)
- [Contrato de owner de `std.async`](./docs/contracts/stdlib-async.md)
- [Contrato de owner de `std.sync`](./docs/contracts/stdlib-sync.md)
- [Contrato de rendimiento de `std.sync`](./docs/contracts/stdlib-sync-performance.md)
- [Contrato de rendimiento de colecciones de `std.sync`](./docs/contracts/stdlib-sync-collection-performance.md)
- [Contrato de owner de `std.executor`](./docs/contracts/stdlib-executor.md)
- [Matriz normativa de owners y firmas de stdlib](./docs/contracts/stdlib-matrix.md)
- [Contrato de campañas de generación del runner](./docs/contracts/test-generation.md)
- [Contrato de fast gate y tiers de evidencia](./docs/contracts/fast-gate.md)
- [Contrato de alcance de evaluación native AOT](./docs/contracts/native-aot-scope.md)
- [Contrato de memoria de productos native AOT](./docs/contracts/native-aot-memory.md)
- [Contrato de coordinación de implementación STD-0.1A](./docs/contracts/stdlib-implementation-coordination.md)
- [Contrato de coordinación Hosted STD-0.1A](./docs/contracts/stdlib-hosted-implementation-coordination.md)
- [Contrato de owners Core STD-0.1A](./docs/contracts/stdlib-core.md)
- [Contrato de owners Hosted STD-0.1A](./docs/contracts/stdlib-hosted.md)
- [Contrato de distribución VM STD-0.1A](./docs/contracts/stdlib-distribution.md)
- [Contrato del seal S1A STD-0.1A](./docs/contracts/stdlib-s1a-seal.md)
- [Contrato de promoción del backend nativo Gate N1](./docs/contracts/native-n1.md)

**RFCs de planificación:**

- [RFC-019 — tooling dinámico de diagnóstico](./docs/rfc/019-diagnostic-tooling.md)

**Companion normativo con conformidad separada:**

- [Tondo LLM Form](./TONDO_LLM_FORM_SPEC.md)

G5 inventaría y sella exactamente lenguaje, testing y toolchain al preparar la
primera release. La stdlib
mantiene su conformidad separada en S1A/S1; fijar su spec por hash nunca se
confunde con conformarla. TLF tampoco cambia la semántica `.to`: Gate L0 produce
un bundle companion separado. El futuro candidato del lenguaje fijará G5 y S1;
solo fijará L0 cuando se construya además una distribución TLF, sin convertirla
en requisito de Tondo 0.1.

**Objetivo inmediato:** continuar la implementación de `STD-0.1B` de Wave 8 tras
cerrar Gate N1. `STD-ASYNC-GROUP-IMPL-001`, `STD-ASYNC-GROUP-TEST-001`,
`STD-ASYNC-GROUP-PERF-001`, `STD-ASYNC-GROUP-CONF-001` y
`STD-ASYNC-GROUP-DOC-001` ya están cerrados para la VM hosted y el ABI del
runtime nativo; `STD-SYNC-IMPL-001`, `STD-SYNC-TEST-001` y
`STD-SYNC-PERF-001` ya están cerrados para la superficie del compilador, el
modelo hosted determinista y la campaña de rendimiento target-qualified;
`STD-SYNC-COLLECTION-FRONTEND-001` también está cerrado para la sintaxis,
resolución nominal, HIR/MIR boundary y diagnóstico, y
`STD-SYNC-COLLECTION-IMPL-001` queda cerrado para la ejecución en la VM hosted
y el ABI nativo privado (sin promoción de una API pública ni lowering AOT
genérico). `STD-SYNC-COLLECTION-ITER-001` queda cerrado para el `for` directo
finito en la VM hosted y el ABI nativo privado, sin reclamar lowering AOT
genérico. `STD-SYNC-COLLECTION-TEST-001` queda cerrado para los modelos
secuenciales independientes, histories de linearización, cursores, aliases,
cleanup y fuzz acotado. `STD-SYNC-COLLECTION-PERF-001` queda cerrado para la
campaña target-qualified de rendimiento de la VM hosted: 31 workloads, tres
procesos independientes, 27 muestras por workload y métricas de latencia,
throughput, allocations, memoria lógica, retries, wakeups, parking y handles
vivos. `STD-SYNC-COLLECTION-CONF-001` queda cerrado para la equivalencia
observable VM/native de ocho casos, incluidos aliases, outcomes, orden,
cursores, snapshots, límites, cleanup y capability `threads`; la campaña no
promueve fast paths nativos ni lowering AOT genérico. `STD-SYNC-CONF-001` y la
conformance global ya están cerrados. `STD-SYNC-DOC-001` también queda cerrado
con la guía ejecutable; `STD-CHANNEL-PERF-001` queda cerrado para la línea
base de rendimiento hosted; `STD-CHANNEL-CONF-001` también queda cerrado para
la equivalencia observable VM/native; `STD-CHANNEL-DOC-001` queda cerrado con
la guía ejecutable de composición. `STD-EXEC-IMPL-001` queda cerrado para la
implementación cooperativa hosted de pools, adquisición explícita de `ActorRef`,
handlers y envíos `selectable`. `STD-EXEC-HOST-001` también queda cerrado: la
VM hosted ejecuta `BlockingPool.run` en workers aislados con bridge de host y
el runtime nativo aporta una lane privada de tokens para
`x86_64-unknown-linux-gnu`; el lowering native AOT de callables y la API pública
siguen sin promocionarse. `STD-EXEC-TEST-001` queda cerrado con el modelo
acotado, replay de 4.096 semillas, stress real del bridge y smoke fuzz
reproducible. `STD-EXEC-PERF-001` queda cerrado con una campaña 3 x 9
target-qualified para la VM hosted y la lane privada nativa de tokens; el
`STD-EXEC-CONF-001` queda cerrado por el corpus común VM/native y la capability
estática `threads`; `STD-EXEC-DOC-001` queda cerrado por la guía ejecutable y
sus cinco composiciones. `STD-ENCODING-IMPL-001` queda cerrado para la ruta
scalar de la stdlib y el bridge VM hosted, con materialización, `Reader`/`Writer`,
handles afines y errores tipados; `native_aot_lowering: not-claimed` permanece
explícito. `STD-ENCODING-TEST-001` queda cerrado. `STD-ENCODING-PERF-001`
queda cerrado con un baseline scalar de la VM hosted: 16 workloads
materializados e incrementales, 3 warmups, 9 repeticiones y 3 procesos
independientes, con mediana/P95/P99, bytes copiados, allocations, memoria
lógica y cleanup de handles. `STD-ENCODING-CONF-001` queda cerrado por una
corpus común VM/native de seis casos, con interoperabilidad, streaming,
errores/offsets, límites y cleanup hash-bound; la sonda usa el mismo kernel
scalar y mantiene `native_aot_lowering: not-claimed` y
`simd: not-measured-no-optimized-route`. `STD-ENCODING-DOC-001` queda cerrado
por la guía ejecutable de policies, errores, costes y ejemplos; el siguiente
bloque de ese slice es `STD-YAML-IMPL-001`. El modelo y
tests/fuzz hosted de Group están respaldados por
`STD-ASYNC-GROUP-TEST-001`. El slice
ejecutable de `select` ya está cerrado en la VM hosted —frontend,
semántica tipada, lowering verificable, runtime cooperativo, ownership
branch-sensitive, adapters `Waiter`/time, modelo/tests deterministas y
presupuestos de rendimiento reproducibles y corpus de conformidad completo ya
están cerrados. `STD-A-PERF-001` está promovido: diez owners portables tienen
las ocho dimensiones en seis workloads y los doce owners restantes tienen
fronteras normativas `not-applicable` hacia `PERF-001`. `STD-A-CONF-001`
también está promovido: sus 22 owners, 385 filas y 206 casos del draft tienen
observación ejecutada. `STD-A-DIST-001` también está promovido: dos snapshots
limpios producen el mismo paquete VM content-addressed, con instalación,
ejecución y desinstalación verificadas. `STD-S1A-SEAL-001` ha cerrado el
bundle técnico del draft con auditoría estricta y cero celdas aplicables
abiertas; `DIAG-SPEC-001` ya cerró su contrato D0 y los contratos de
`std.async.Group`, `std.channel`, `std.sync`, `std.executor`, `std.net` y el
calendario civil de `std.time` ya están cerrados como contratos runtime-facing.
`std.encoding`, `std.yaml`, `std.toml`, `std.cbor`, `std.regex`, `std.uuid` y
`std.log` ya han cerrado sus fronteras contractuales B0. `DIAG-RUNTIME-001`
ya consume esos contratos de observabilidad en la VM hosted; `RACE-001` y
`LEAK-001`, `DUMP-001` y `DIAG-TEST-001` ya cerraron sus detectores, writer,
integración y evidencia por intento. El grafo
activo se valida con `TRACKER-LINT-001`; con el contrato D0, Wave 6 y la
instrumentación D1, DUMP y el runner de tests cerrados, `DIAG-CI-001` ya está
cerrado antes de la evaluación
coordinada de `NATIVE-001`. El cierre coordinado de
`STD-CODEC-PUBLIC-001` ya verificó su superficie, pero el conteo global
214/214 ya incluye los tres adapters `selectable` de DEC-020;
y los tres owners build-only tienen una frontera explícita; no se fabrican
funciones runtime para ellos. `NATIVE-TARGET-DESC-001` y
`NATIVE-ARTIFACT-001`, `NATIVE-LINK-PLAN-001` y `NATIVE-PUBLISH-SPEC-001` están
cerrados como contratos puros, y `PERF-001` ya fija el contrato de benchmark y
baseline previo al backend. Con la conformance pública, la distribución, el
seal S1A y el contrato D0 promovidos como evidencia técnica del draft,
los contratos runtime-facing B0 se han cerrado en la VM y en las dos rutas
nativas candidatas.
`NATIVE-001` cerró la frontera de evidencia inicial y `DEC-013` ya seleccionó
Cranelift para el target admitido: la slice de selección runtime se cerró en
`NATIVE-SELECT-001`, el adaptador común está cerrado en
`NATIVE-BACKEND-ADAPTER-001` y `NATIVE-002` cerró la coordinación mínima de
lowering. `ARC-001` y `ARC-002` cerraron ownership, cleanup, ciclos y weak
refs en el runtime nativo; `DIAG-NATIVE-001` cerró la paridad lógica de
diagnóstico; `NATIVE-THREAD-001` cerró la lane física de workers OS. Las
fronteras Core y Hosted de STD-0.1A están cerradas y la evidencia de enlace,
targets, conformance y distribución ya está disponible para comparar
Cranelift y LLVM. La campaña AOT completa, la normalización de artefactos
enlazados, memoria, calidad y rendimiento ya tienen evidencia cerrada;
Gate N1 queda cerrado por su informe compositivo. Las cifras rápidas
anteriores no se usan para la decisión final: la campaña cerrada mide el
producto enlazado completo y no mezcla buffers de código Cranelift con el
objeto completo de LLVM.
La campaña `NATIVE-AOT-MEM-001` ya está cerrada: ambos productos enlazados
ejecutan el corpus completo y un workload instrumentado en tres procesos
frescos, con tres warmups y nueve muestras por proceso; la evidencia registra
allocations, bytes asignados/live/pico, ARC local/atómico, ciclos, weak
upgrades, pausas, presión de worker y RSS, manteniendo la semántica de la VM
como oráculo. `NATIVE-AOT-QUALITY-001` ya tiene su compuerta completa
implementada en el
contrato `testing/native-aot-quality.json`, la campaña
`scripts/native-aot-quality.sh` y su suite de mutaciones negativas. La
compuerta usa seis mutantes críticos deterministas (uno por frontera) además
de los doce oráculos contractuales; la campaña completa de 30 mutantes queda
explícitamente reservada para un carril posterior de rendimiento de calidad y
no sustituye la evidencia AOT ya cerrada.
FUZZ está promovido para los 22 owners; la distribución está promovida y el
seal S1A está cerrado como bundle técnico del draft, sin sobreafirmar G5, N1,
TLF ni una publicación.
`CONF-GAP-IMPL-001` y `CONF-LAYER-RESULT-001` mantienen T0 verificable sobre el
árbol actual. `CONF-SEAL-FINAL-001` queda reservado para el primer candidato
real; los límites `TL01-26-*` pertenecen a S1A. Los contratos
runtime-facing de STD-0.1B quedan ahora desbloqueados por Gate N1; M11 conserva
la evidencia nativa promovida sin convertirla en un release. Todo pertenece a la primera versión 0.1; los slices
son orden de implementación, no versiones públicas. La
VM permanece como implementación de referencia y oracle diferencial del
backend nativo. La lane TLF puede avanzar en paralelo porque solo depende del
frontend/formatter ya cerrados; no reemplaza esas prioridades ni bloquea el
futuro candidato base. Su bundle L0 se publicará únicamente como companion
opcional.

`NATIVE-AOT-SCOPE-001`, `NATIVE-AOT-LOWER-001`, `NATIVE-AOT-BINARY-001`,
`NATIVE-AOT-MEM-001`, `NATIVE-AOT-QUALITY-001` y `NATIVE-AOT-PERF-001` ya están
cerrados. `DEC-013` seleccionó Cranelift para el target admitido y el informe
compositivo de Gate N1 promueve esa implementación únicamente para
`x86_64-unknown-linux-gnu`; no existe promoción automática para otros targets.

> Este documento no define semántica del lenguaje. La especificación es la única
> fuente normativa. El tracker organiza el trabajo de implementación, registra
> decisiones técnicas y permite distinguir entre una característica
> implementada, una característica validada y una implementación conforme.

## 0. Contrato vigente de Tondo 0.1

Tondo todavía no se ha publicado. Existe un único contrato vivo: este tracker,
las especificaciones actuales y el corpus ejecutable actual evolucionan juntos.
Git conserva el historial; el compilador, los adaptadores y CI no mantienen
sintaxis, manifiestos, snapshots ni rutas de compatibilidad de borradores
anteriores.

### Suspensión y concurrencia

- No existe una familia `async fn`, `async unsafe fn` ni un wrapper público
  `Task`/`Future`. Todas las funciones son `fn`; `suspends` es un efecto postfix
  denotable en firmas y tipos de función, obligatorio en contratos sin cuerpo e
  inferible en cuerpos.
- Una llamada directa a una operación `suspends` espera implícitamente y
  devuelve su resultado lógico. `await call()` es inválido con `E1611` porque
  duplica una decisión que ya pertenece al compilador.
- `selectable` implica suspensión y añade una entrada atómica de
  prepare/register/commit/rollback. La llamada ordinaria también espera
  implícitamente; la expresión núcleo `select` es el único contexto que registra
  la operación sin esperarla primero. No existe `std.async.select` ni tipos
  públicos `Case`.
- `spawn call()` crea una task ligera; `spawn thread call()` crea un thread del
  sistema operativo. Ambos devuelven el mismo `Join[T, E]` afín y se consumen
  mediante `await`; `return spawn call()` es una expresión ordinaria.
- Al salir un scope cada child debe esperarse, cancelarse, detached o
  transferirse. `cancel` solicita y luego requiere observar la finalización;
  `detach` consume el handle y prohíbe capturar préstamos locales. El unwind
  cancela y espera children no transferidos.
- `oneshot[T, E]` separa `Waiter` y `Completer`, completa una sola vez y devuelve
  `AlreadyCompleted` en una segunda finalización. `AsyncIterator[T]` es el
  protocolo lazy con backpressure; el único `for` selecciona `Iterator[T]` si
  está disponible y, en otro caso, espera `AsyncIterator[T]` implícitamente.
  `collect(limit:)` es la materialización explícita.
- STD-0.1B añadirá `Group[T, E]` para coordinación homogénea `all`, `settle`,
  `next` y cancelación drenada; sustituye tanto contadores `WaitGroup` como
  agregadores `WhenAll`/`WhenAny` sin introducir otro handle de tarea.
  `std.channel` cubrirá productor/consumidor mediante `send`/`receive`
  `selectable`; `select` también acepta timers, one-shots, `Group.next` y un
  número fijo de `Join` heterogéneos;
  `std.sync` las primitivas y colecciones compartidas —incluidos array/map/set
  linealizables y stack/queue—, y `std.executor` pools, actores y el bridge
  bloqueante. Todo reutiliza `suspends`, `spawn`, `Join`, scopes y
  `AsyncIterator`.
- `await` se reserva para consumir trabajo pendiente representado por un
  `Join[T, E]`. `Waiter.wait()` y cualquier otra operación suspendible directa
  usan la espera implícita normal.
- `defer cleanup()` y `defer { cleanup() }` infieren suspensión exactamente como
  el resto del lenguaje. Dentro de `defer` no se permite escribir `await`,
  `spawn` ni `scope`; el cleanup conserva LIFO, `Send`, cancelación y unwind.

### Serialization y protocolos

- `std.serialization` posee los traits genéricos estáticos `Encode[C]` y
  `Decode[C]`; no hay interfaces runtime, reflection de valores ni DOM en la
  ruta tipada. Un mismo tipo puede implementar varios codecs.
- JSON y MessagePack comparten `serialization.Value` (aliases de módulo), con
  `ValueView` prestado, `Raw` opaco y copy-on-write no observable. `parse` es la
  ruta dinámica; `decode[T]` y `encode(value)` son las rutas tipadas/dinámicas
  unificadas.
- `@name`, `@json`, `@messagepack`, `@proto(number)`, `@ignore` y
  `@json(base64)` se resuelven en compile time. Protobuf exige números de field
  explícitos y conserva su modelo de wire (`ProtoEvent`/`UnknownField`), sin
  convertirlo en `Value`.
- Los defaults de límites son finitos; límites no acotados y `rawUnchecked` solo
  existen en `unsafe`. Streaming reutiliza el único `Reader`/`Writer`; no hay
  APIs síncronas/suspendibles duplicadas.

### Regla de actualización pre-release

- Un cambio de lenguaje actualiza en el mismo bloque especificaciones,
  contratos, registros machine-readable, lowering, APIs públicas, fixtures y
  evidencia derivada.
- No se conserva una segunda spelling ni un adapter de migración para una forma
  descartada. Los tests negativos pueden comprobar que esa forma se rechaza,
  pero nunca compilarla por compatibilidad.
- El corpus de conformidad es vivo y se regenera contra el árbol actual. Proofs,
  candidatos y snapshots inmutables comenzarán con el primer proceso real de
  release; antes de él solo añaden coste y divergencia.

---

## 1. Resultado que buscamos

El primer resultado importante no debe ser un parser aislado ni un backend
incompleto. Debe ser una **vertical slice real**:

~~~text
fuente .to
  -> parseo
  -> resolución
  -> type checking
  -> MIR
  -> bytecode
  -> ejecución en la VM
  -> diagnóstico o exit status observable
~~~

Al alcanzar ese punto, el toolchain deberá ofrecer:

~~~text
tondo fmt <archivo>
tondo check <archivo>
tondo run <archivo>
~~~

El primer compilador podrá llamarse **bootstrap** o **experimental**, pero no
podrá anunciar conformidad completa del draft hasta superar
`tondo-conformance-draft`.

### 1.1 Definición del primer compilador

Consideraremos que existe un primer compilador cuando:

- Acepte fuente Tondo real, no un dialecto temporal.
- Produzca un CST sin pérdida y pueda formatear el archivo canónicamente.
- Resuelva un módulo raíz y sus nombres locales.
- Compruebe un subconjunto coherente del sistema de tipos.
- Baje el programa a una IR explícita y después a bytecode.
- Ejecute el bytecode en una VM propia.
- Implemente `main`, llamadas, variables, control de flujo, records, enums,
  `match`, `Option`, `Result`, `?`, aritmética comprobada, `assert` y `panic`.
- Produzca diagnósticos humanos y JSON con códigos, spans y orden determinista.
- Rechace explícitamente las características todavía no soportadas, sin
  reinterpretarlas ni cambiar su semántica.
- Pase los tests correspondientes a la superficie implementada.

No es necesario para este primer gate:

- Backend nativo.
- ARC ni recolección específica de ciclos.
- Copy-on-write optimizado.
- Compilación incremental.
- LSP.
- Gestor de paquetes.
- ABI estable.
- Executor multithread.
- FFI general.
- Librería estándar completa.

### 1.2 Hitos de producto

| Gate | Resultado | Alcance |
|---|---|---|
| **G0 — Frontend fiable** | `tondo fmt` y parseo recuperable | Léxico, CST, parser, formatter y diagnósticos de sintaxis |
| **G1 — Análisis semántico** | `tondo check` útil | Nombres, tipos y control de flujo del núcleo bootstrap |
| **G2 — Primer compilador** | `tondo run` ejecuta bytecode | Vertical slice síncrona, segura y deliberadamente parcial |
| **G3 — Alpha utilizable** | Núcleo síncrono completo | Genéricos, traits, ownership, préstamos y colecciones |
| **G4 — Preview 0.1** | Superficie del lenguaje completa | Async, scripts, procesos, targets y `unsafe` |
| **G5 — Tondo 0.1 conforme** | Primera versión publicable, todavía no publicada | Suite de conformidad completa para el draft final y el target anunciado |
| **H0 — Fiabilidad continua** | Evidencia automatizada y reproducible | Trazabilidad, CI, properties, fuzzing, modelos y métricas |
| **T0 — Testing first-class** | `tondo test` conforme | Tondo 0.1, unit/integration tests, aislamiento y reporte estable |
| **S1A — Standard Library 0.1 foundation** | Core + Hosted utilizable | Base necesaria para testing y backend nativo |
| **N1 — Backend nativo conforme** | Segunda implementación de producción | Oracle diferencial, runtime nativo y targets verificados |
| **S1 — Standard Library 0.1 completa** | Primera stdlib publicable | Foundation + Concurrency + Application conformes |
| **L0 — Tondo LLM Form** | Transporte compacto, reversible y medido para agentes | Codec, source maps, CLI, properties, fuzzing y evaluación multi-modelo |

---

## 2. Cómo se mantiene este tracker

### 2.1 Estados

- `[ ]` significa pendiente.
- `[x]` significa terminado y con evidencia verificable.
- Una tarea pendiente puede añadir `EN CURSO` o `BLOQUEADA` junto a su ID.
- Un milestone solo se cierra cuando cumple su gate completo; no basta con que el
  código exista.

### 2.2 Tres ejes distintos

Cada característica debe poder responder por separado:

1. **Implementada:** existe una ruta real de compilación o ejecución.
2. **Validada:** existen tests que prueban los casos positivos, negativos y los
   bordes materiales.
3. **Conforme:** supera los casos aplicables de la suite versionada oficial.

No se utilizará “soportado” como abreviatura ambigua de los tres estados.

### 2.3 Definición de terminado para una tarea

Una tarea solo se marca como terminada cuando:

- El comportamiento está conectado a la ruta pública real.
- No depende de un modo oculto o de datos prefabricados para tests.
- Tiene tests positivos y negativos proporcionados al riesgo.
- Sus diagnósticos observables tienen código y spans correctos.
- No deja `TODO`, panic temporal, feature stub silencioso ni ruta alternativa
  contradictoria.
- La documentación técnica afectada está actualizada.
- Se han ejecutado y observado las comprobaciones mínimas relevantes.

### 2.4 Relación con cambios del spec

Durante la implementación aparecerán preguntas que el análisis en papel no
puede descubrir. Se seguirá este proceso:

1. Reducir el caso a un programa Tondo mínimo.
2. Determinar si el spec ya contiene una respuesta.
3. Clasificarlo como bug del compilador, ambigüedad editorial o decisión
   semántica ausente.
4. No inventar una regla privada dentro del compilador.
5. Si falta una decisión semántica, registrar una propuesta `SPEC-NNN` con
   alternativas y efectos.
6. Cuando la decisión se acepte, actualizar conjuntamente spec, tests y
   compilador.

Una corrección editorial no debe convertirse accidentalmente en una edición
nueva del lenguaje.

---

## 3. Decisiones técnicas de partida

Estas decisiones buscan maximizar la velocidad de aprendizaje y minimizar la
cantidad de infraestructura necesaria antes del primer programa ejecutable.

### 3.1 Decisiones aceptadas como baseline

| ID | Decisión | Motivo |
|---|---|---|
| `ADR-001` | Implementar el compilador en **Rust** | Buen control de memoria, enums adecuados para IRs, ecosistema de tooling y frontera `unsafe` explícita |
| `ADR-002` | Lexer y parser escritos a mano | La gramática es deliberadamente determinista y contiene ambigüedades contextuales que deben preservarse hasta resolución |
| `ADR-003` | CST sin pérdida como representación sintáctica primaria | Formatter, diagnósticos, fixes y tooling deben observar exactamente la misma fuente |
| `ADR-004` | Recursive descent para declaraciones y Pratt parser para expresiones | Mantiene el parser pequeño, local y fácil de diagnosticar |
| `ADR-005` | Pipeline `CST -> HIR -> HIR tipado -> MIR -> bytecode` | Separa sintaxis, significado, tipos, ownership y ejecución |
| `ADR-006` | Bytecode por slots/registros explícitos, no una operand stack opaca | Se aproxima al MIR, simplifica debugging, spans, roots y movimientos |
| `ADR-007` | VM interpretada como primer backend | Permite validar semántica antes de asumir el coste de LLVM, Cranelift o generación nativa propia |
| `ADR-008` | `Value` explícito y legible antes que NaN-boxing u otras representaciones compactas | La representación bootstrap debe favorecer corrección e inspección |
| `ADR-009` | GC preciso, no móvil y stop-the-world para la VM bootstrap | Satisface memoria automática y ciclos con un runtime inicial pequeño |
| `ADR-010` | Executor cooperativo de un solo hilo como primer runtime de suspensión | El lenguaje no exige una task por thread; permite validar concurrencia estructurada antes del paralelismo |
| `ADR-011` | Copias lógicas correctas antes que copy-on-write | Una copia eager es conforme; COW es una optimización no observable que debe añadirse después |
| `ADR-012` | Pipeline de compilación determinista, inicialmente no incremental | Incrementalidad no debe contaminar la semántica ni retrasar el primer compilador |
| `ADR-013` | Monomorfización como primera estrategia para genéricos y dispatch estático | Encaja con los traits sin vtables y mantiene el bytecode tipado |
| `ADR-014` | Sin formato serializado estable de bytecode durante bootstrap | El bytecode puede ser in-memory hasta que la semántica y el loader estén estabilizados |
| `ADR-015` | Un subconjunto bootstrap es una limitación del toolchain, no una edición ni dialecto de fuente | Las construcciones no implementadas se rechazan; nunca reciben semántica provisional |
| `ADR-016` | Metaprogramación estática mediante `derive` y una ronda hermética de generators Tondo | Elimina boilerplate sin macros textuales, reflection dinámica, plugins nativos ni ejecución ambiental dentro del frontend |
| `ADR-017` | Trabajo de procesos bloqueante fuera del executor cooperativo | Conserva progreso suspendible y cancelación estructurada sin fingir I/O no bloqueante |
| `ADR-018` | TLF es un formato de transporte, no una segunda semántica | Conserva el pipeline `.to`, reutiliza formatter/diagnósticos y exige expansión explícita antes del lexer ordinario |

### 3.2 Decisiones que deben documentarse antes de su milestone

- [x] **DEC-001 — Contrato exacto de la CLI.** Fijar exit codes, escritura en
  stdout/stderr, selección de target, `--diagnostic-format`, modo script y
  comportamiento ante varios archivos.

- [x] **DEC-002 — Modelo interno de fuente.** Fijar `SourceId`, paths lógicos,
  offsets en bytes, line index, NFC y remapping de archivos virtuales.

- [x] **DEC-003 — Forma del CST.** Documentar nodos, tokens, trivia, nodos de
  error y representación de construcciones contextuales.

- [x] **DEC-004 — Representación de tipos.** Definir interning, identidad
  nominal, sustituciones, aliases expandidos, uniones normalizadas y tipos de
  inferencia.

- [x] **DEC-005 — Contrato HIR/MIR.** Decidir qué invariantes debe haber
  demostrado cada fase y dónde se representan moves, loans, cleanup y puntos de
  suspensión.

- [x] **DEC-006 — Modelo de objetos de la VM.** Fijar roots, heap objects,
  tracing, strings, environments, `Ref[T]`, payloads de enum y collections.

- [x] **DEC-007 — Frontera bootstrap de la stdlib.** Definir el shim mínimo para
  consola y host sin congelar prematuramente la futura API estándar.

- [x] **DEC-008 — Targets iniciales.** Nombrar el target de la VM y el primer
  perfil `hosted`, con sus capacidades declaradas.

- [x] **DEC-009 — Estrategia de tests extraídos del spec.** Fijar cómo se
  descubren fences, fixtures, edición, capacidades y expectativas
  compile-pass/compile-fail.

- [x] **DEC-010 — Presupuesto de recursos del compilador.** Fijar límites
  defensivos para profundidad sintáctica, tipos recursivos, expansión de
  genéricos, resolución de traits y tamaño de diagnostics JSON.

- [x] **DEC-011 — Contrato de evidencia continua.** Antes de cerrar H0,
  documentar tiers de CI, seeds y reducción, corpus persistente, artefactos de
  fallo, medición de coverage/mutation score, umbrales y excepciones
  justificadas.

- [x] **DEC-012 — Versionado y distribución de la stdlib.** El contrato base
  [`TONDO_STANDARD_LIBRARY_SPEC.md`](./TONDO_STANDARD_LIBRARY_SPEC.md) fija una
  sola distribución `std` por grafo, versionado conservador incluso antes de
  1.0, PackageId y hashes exactos, prelude mínimo, catálogo cerrado,
  capabilities y actualización explícita; no es una versión ni un release.

- [x] **DEC-013 — Seleccionar backend nativo y fijar la ABI runtime interna.**
  `NATIVE-001` cerró la frontera de evidencia y la campaña AOT completa quedó
  cerrada con probe MIR común, runner físico, artefactos enlazados
  normalizados, memoria, calidad, rendimiento, targets y paquete reproducible.
  La decisión selecciona **Cranelift** para native AOT en
  `x86_64-unknown-linux-gnu`; LLVM queda como comparativa experimental y
  `custom` fuera del ranking. El registro explica la ponderación de
  integración Rust, mantenimiento/distribución, rendimiento y tamaño, y fija
  que no existe fallback silencioso ni ABI FFI pública, en
  [`docs/adr/019-native-backend-selection.md`](docs/adr/019-native-backend-selection.md)
  y [`testing/native-selection.json`](testing/native-selection.json). La
  selección quedó validada por Gate N1 para el target primario; las compuertas
  posteriores verifican STD-0.1B y la distribución, no reabren el backend.

- [x] **DEC-014 — Gestión de memoria nativa.** Cerrada por
  `NATIVE-MEM-ADR-001` antes de la ABI y el lowering nativos: fija ownership
  runtime, atomicidad, weak refs, detección de ciclos, interacción con COW,
  async, threads, FFI privilegiada, roots/retainers, ledger de recursos y la
  estrategia de verificación de `LEAK-001`. La decisión canónica y sus
  negativos ejecutables están en `docs/contracts/native-memory.md`,
  `docs/adr/020-native-memory-and-runtime-abi.md`,
  `testing/native-memory.json` y los gates `scripts/native-memory-{check,test}.sh`.

- [x] **DEC-018 — Diagnóstico dinámico sin APIs paralelas.**
  La decisión acepta una única superficie opt-in para `race`, `leaks` y
  `crash`, conserva intacto `tondo-diagnostics-json/1`, prohíbe keywords y
  APIs paralelas en `std`, exige `unsupported-diagnostic-profile` explícito y
  mantiene payloads redactados por defecto. La decisión queda fijada por
  [`docs/contracts/diagnostic-tooling.md`](./docs/contracts/diagnostic-tooling.md),
  RFC-019 y `testing/diagnostic-tooling.json`; no afirma implementación runtime.

- [x] **DEC-015 — Testing first-class integrado en 0.1.** La especificación
  [`TONDO_TESTING_SPEC.md`](./TONDO_TESTING_SPEC.md) forma parte del mismo
  contrato Tondo 0.1 y reserva `suite` y `test`: `suite` es un contenedor
  estático con setup léxico y teardown por `defer`; `test` es siempre una hoja.
  El corpus vivo no es una edición, release ni dialecto seleccionable. El
  contrato separa unit overlays de
  integration roots y fija árbol/identidad, capturas `Copy + Send + Share`,
  envelope estructurado,
  `std.testing.log/tags/failNow/skip/attach/snapshot/withVirtualTime`, inferencia
  de error/suspensión, aislamiento, selección substring/glob/exact, ownership por
  CODEOWNERS, sharding, orden aleatorio reproducible, retries y repeat explícitos
  en workers nuevos, tiempo virtual opt-in sobre la API monotónica de
  producción, inputs públicos/secretos, interrupción, artifacts
  content-addressed, snapshots versionados, límites, output, exit status y
  reportes `tondo-test-report-0.1/7`, `tondo-test-list-0.1/6` y
  `tondo-junit-report-0.1/4`. No se introducen `TestContext`, attributes,
  clases, reflection, registro runtime, hooks, selección regex o por tags
  runtime, retries/repeat implícitos, actualización automática de snapshots ni
  un reloj exclusivo de testing.

- [x] **DEC-016 — Metaprogramación y reflection.** Tondo 0.1 incorpora una
  declaración `derive` cerrada y generators declarados por manifest. Ambos
  ejecutan programas Tondo fijados en un VM `tondo-meta` sin capabilities,
  contra `tondo-meta-model-0.1/1`, en una única ronda y con inputs, outputs,
  roots semánticos, presupuestos y hashes exactos. Los programas viven en un
  grafo meta separado y el frontend permanece puro. `std.reflect`
  conserva solo metadata descriptiva solicitada; no existe `Any`, inspección de
  valores, acceso privado runtime, lookup por string ni dependencia reflectiva
  de serializers.

### 3.2.1 Feedback por impacto

- [x] **DEC-017 — Feedback por impacto y gates por tier.** Cada bloque usa
  `scripts/fast-gate.sh` y selecciona el mínimo tier suficiente. Cambios
  exclusivamente documentales —specs, tracker y evidencia generada— usan
  `scripts/documentation-gate.sh`: fences, conformance documental, matriz
  normativa, grafo del tracker y contratos de stdlib, sin suite completa,
  coverage ni mutation testing. Código Rust de producción aislado usa check/test
  del paquete y exige cobertura de líneas ejecutables nuevas al 100 % y mutación
  del diff; tests externos o cambios confinados a un módulo inline de tests
  auditado ejecutan solo los targets de test afectados —`--lib` cuando todo el
  cambio Rust está en ese módulo inline— sin recalcular métricas del producto;
  fronteras compartidas escalan a `scripts/test-gate.sh`. Pushes y pull requests
  usan este clasificador sobre todo el rango `before..head` del evento, nunca
  solo `HEAD^`; solo una frontera compartida, límite de wave, cambio de baseline,
  release candidate o afirmación portable exige el tier `full`, que
  conserva test-gate, matriz portable, fuzzing y quality-gate. La evidencia de
  fast gate vive en `target/reliability/fast-gate/` y es siempre efímera.

- [x] **DEC-019 — `suspends` visible sin duplicar APIs.** El efecto es postfix,
  forma parte del tipo y del hash ABI, y aparece siempre en interfaces y
  tooling. Los contratos sin cuerpo deben escribirlo; un cuerpo puede inferirlo
  o fijarlo explícitamente como promesa estable. La llamada ordinaria espera de
  forma implícita y `spawn` es la única iniciación concurrente. Los préstamos
  exclusivos pueden atravesar una espera secuencial con `Send`, pero nunca se
  transfieren a `spawn` o `spawn thread`.

- [x] **DEC-020 — Integrar selección atómica en el núcleo.** `select` es una
  expresión y `selectable` una capacidad postfix más fuerte que `suspends`,
  visible en tipos, interfaz y ABI. Los brazos aceptan llamadas seleccionables o
  `await Join`, un `else` final opcional, fairness rotatoria y ownership por
  rama. La stdlib aporta operaciones y entrypoints atómicos, no un selector,
  builders, macros, objetos `Case` ni APIs duplicadas.

### 3.3 Estructura inicial recomendada

Mantener pocos crates durante bootstrap:

~~~text
tondo/
  Cargo.toml
  crates/
    tondo-cli/
    tondo-compiler/
    tondo-vm/
  tests/
    spec/
    compile-pass/
    compile-fail/
    runtime/
  docs/
    architecture.md
    adr/
~~~

Dentro de `tondo-compiler`, comenzar con módulos internos y extraer crates solo
cuando exista una frontera estable:

~~~text
source
syntax
diagnostics
resolve
hir
types
mir
bytecode
driver
~~~

No crear inicialmente un crate por cada fase. La modularidad lógica es
necesaria; la fragmentación del workspace no.

---

## 4. Dashboard

| Milestone | Resultado principal | Estado |
|---|---|---|
| **M0 — Fundación** | Repo reproducible, CLI y arquitectura | Completado |
| **M1 — Fuente, parser y formatter** | Gate G0 | Completado |
| **M2 — Semántica bootstrap** | Gate G1 | Completado |
| **M3 — MIR, bytecode y VM** | Gate G2: primer compilador | Completado |
| **M4 — Genéricos, traits y closures** | Sistema estático completo | Completado |
| **M5 — Ownership, préstamos y memoria** | Modelo de valores completo | Completado |
| **M6 — Colecciones, números y texto** | Gate G3: alpha utilizable | Completado |
| **M7 — Async y concurrencia estructurada** | Tasks conformes + selección núcleo | Suspensión/tasks, frontend, semántica tipada, lowering verificable, selector cooperativo, ownership branch-sensitive, modelo/tests, adapters `selectable`, baseline de rendimiento y conformidad VM del selector cerrados |
| **M8 — Scripts y procesos** | Experiencia de scripting | Completado |
| **M9 — Unsafe, targets y toolchain** | Gate G4: preview 0.1 | Completado |
| **M10 — Corpus ejecutable** | Conformidad viva pre-`derive` | Completado |
| **M10.5 — Reliability y testing** | Infraestructura y hardening continuo de evidencia | Completado |
| **M10.5c — Infraestructura de conformidad** | Un único draft vivo y su ratchet | Completado; T0 verificable sobre el árbol actual |
| **M10.7 — Metaprogramación estática** | `derive`, generators, meta VM y contribución a G5 | Completado |
| **M10.6 — Testing de usuario Tondo 0.1** | Implementación de `tondo test` y contribución a G5 | Completado; incluido en el gate T0 vivo |
| **DIAG — Tooling dinámico** | Race detector, leak/retention detector, crash dumps y runner integrado | Hosted y paridad native cerrados; capacidades físicas por target |
| **STD-0.1A — Foundation + Hosted** | Base estándar necesaria para meta, testing y backend | S1A sellado como draft técnico; bundle reproducible, auditoría 214/214, matriz sin gaps aplicables y claims de publicación/backend/TLF desactivados |
| **M11 — Backend nativo y optimización** | Implementación de producción | Gate N1 cerrado para Cranelift/x86_64 GNU; optimizaciones posteriores pendientes |
| **STD-0.1B — Concurrency + Application** | Contratos runtime antes de M11; implementación tras N1 | Arquitectura base cerrada; implementación desbloqueada y pendiente |
| **TLF — Forma para agentes** | Transporte compacto hacia Tondo canónico | Spec y estudio exploratorio completados; reproducción, implementación, evaluación y bundle L0 pendientes |

Estado observado del workspace:

- Repositorio local: `/mnt/media/Tony/Projects/tondo`, branch
  `main`, con
  upstream en
  `github.com/tonyredondo/tondo`.
- Workspace: `tondo-cli`, `tondo-compiler`, `tondo-conformance`,
  `tondo-reference-adapter`, `tondo-reliability`, `tondo-stdlib` y `tondo-vm`.
- Toolchain utilizado para la validación: Rust 1.93.0 y Cargo 1.93.0; la versión
  mínima soportada queda fijada en Rust 1.93.
- Los conteos de tests, casos, requisitos y evidencia se derivan del árbol
  actual mediante `testing/inventory.json`, la matriz normativa y los gates;
  no se copian aquí cifras ligadas a una revisión antigua.
- Coverage y mutation se validan contra `testing/quality-baseline.json`. El
  tracker describe el umbral y la procedencia, pero los reports generados son la
  única fuente para las mediciones de cada árbol.

### 4.1 Grafo de dependencias y ruta crítica

~~~text
M0 -> M1 -> M2 -> M3 -> M4 -> M5 -> M6 -> M7 -> M8 -> M9 -> M10
  -> M10.5 -> M10.5b -> CONF-DRAFT-001 -> CONF-RATCHET-001
                                                |
                                                v
                                         META-FORMAT-001
                                                |
                              +-----------------+------------------+
             |                                    |
             v                                    v
  std.meta + std.reflect contract     std.bytes + std.env + std.time base
             |                                    |
             v                                    v
          M10.7/meta                  M10.6/defer inference + testing
             |                                    |
             v                                    v
         META-CONF                    T0 implementation
             +-----------------+------------------+
                               v
              select/selectable frontend + VM adapters
                               |
                               v
       +-----------------------+------------------------+
       |                                                |
       v                                                v
 matrix all specs -> gap audit -> final seal -> G5   STD-0.1A APIs -> S1A
       |                                                |
       +-----------------------+------------------------+
                               v
               contratos runtime STD-0.1B
                               |
                               v
                         M11 -> AOT campaign -> DEC-013 -> Gate N1
                               |
                               v
               implementación STD-0.1B -> Gate S1

Cada unión ratchetea inventario, matriz, conformidad viva y H0.
~~~

M4, M5 y M6 pueden investigarse conjuntamente, pero deben integrarse en ese
orden para evitar que collections o closures introduzcan una semántica de copia
incompatible con ownership.

M10.5 y su hardening M10.5b son fases acotadas de infraestructura,
clasificación y cierre de huecos reales, no una pausa
indefinida para perseguir un número arbitrario de tests. Su gate debe existir
antes de ampliar sintaxis. `CONF-DRAFT-001` mantiene el draft y su corpus
actual como única línea activa; ningún slice nuevo trabaja con el gate
permanentemente roto ni atribuye casos obsoletos a reglas nuevas.

`META-FORMAT-001` es el primer cambio de código porque materializa los formatos
`draft` compartidos. Después no existe una dependencia serial entre M10.7 completo
y M10.6 completo. La lane meta requiere la API build-only exacta de `std.meta`
y el contrato de `std.reflect`; la lane testing requiere la identidad estable de
`std.bytes`, el snapshot read-only de `std.env` y el time-base de producción.
Plan/discovery, sintaxis de testing y `defer` pueden avanzar antes de
terminar esos slices; el typecheck que consume sus APIs, la materialización de
inputs, virtual time, lifecycle completo y Gate T0 quedaron condicionados a su
cierre y ya lo incorporan.

M10.7 y M10.6 ratchetean evidencia al terminar cada wave, no únicamente en
`META-CONF-001` o `UTEST-CONF-001`. T0 se demuestra siempre contra la matriz,
el corpus y el árbol actuales. `CONF-SEAL-FINAL-001` solo creará un bundle
inmutable cuando exista un primer proceso real de release. El resultado
existente de `tondo test` permite completar y probar la propia stdlib.

La decisión DEC-020 reabre de forma explícita la superficie M7: lexer, parser,
CST, formatter, tipos de efecto, HIR/MIR/bytecode, scheduler, ownership y los
adaptadores `Waiter`/time deben cerrar `ASYNC-SELECT-VM-CONF-001` antes de G5 y
S1A. No se reetiqueta la implementación anterior como conforme ni se mantiene
la sintaxis descartada de selección por librería.

Cada API posterior de STD-0.1A se implementa como slice vertical y amplía
matriz, conformidad y dogfooding. Antes de `NATIVE-001` deben estar cerrados los
contratos —no necesariamente las implementaciones— de `std.channel`,
`std.sync`, `std.executor` y la frontera host de `std.net`, porque condicionan
memoria, atomics, wakeups, bloqueo y ABI runtime. M11 depende de T0, G5, S1A y
esos contratos. La implementación de STD-0.1B continúa tras N1 y sigue siendo
requisito de la primera publicación STD 0.1.0.

TLF es una lane transversal independiente después de G0. No modifica `.to`, no
condiciona el backend ni bloquea S1A; sí debe completar codec, mapas, CLI,
properties, fuzzing, benchmark reproducible, evaluación de generación/reparación
y bundle content-addressed antes del candidato de release. Su evidencia no
cuenta como conformidad del lenguaje base.

Los números M10.6 y M10.7 son IDs estables de planificación, no prioridad
cronológica. Este DAG y la cola de la sección 24 son la autoridad para ordenar
el trabajo.

### 4.1.1 Dependencias duras

| Consumidor | Prerrequisito obligatorio | No necesita esperar |
|---|---|---|
| Cualquier cambio del draft | `CONF-DRAFT-001` y H0 verde | Un nuevo Gate G5 |
| Nuevas formas de parser de M10.7/M10.6 | `PARSER-STACK-001` | Resto de meta o testing runtime |
| Plan draft, meta y testing de proyecto | `META-FORMAT-001` | Meta VM completa |
| `META-VM-001` | contrato exacto de `std.meta` | Su implementación completa |
| `STD-META-IMPL-001` | meta VM y `META-MODEL-001` | Derive/generators |
| Derive y generators | meta VM + implementación/conformidad de `std.meta` | JSON/MessagePack/Protobuf |
| `REFLECT-IMPL-001` | contrato público exacto de `std.reflect` | Serializers o reflection de valores |
| `STD-ENV-IMPL-001` | `STD-ENV-SPEC-001` y `STD-BYTES-CONF-001` | Mutación de environment |
| `UTEST-ID-001` | project plan, discovery y dev-dependencies cerrados | Worker o reporters |
| `UTEST-CHECK-001` y attachments | spec + implementación + evidencia de `std.bytes`; tipos del time-base para el checker | Resto de `std.io` y calendario civil |
| `UTEST-INPUTS-001` | `UTEST-INPUTS-PLAN-001`, `UTEST-RUNTIME-001` y `STD-ENV-CONF-001` | Mutación de environment |
| `UTEST-VTIME-001` y Gate T0 | spec + implementación + evidencia del time-base | Calendario civil |
| Lifecycle de suites | `ASYNC-DEFER-IMPL-001`, lowering y worker aislado | Retry, JUnit o snapshot update |
| Gate T0 evidencial | `UTEST-SPEC-EVIDENCE-001` y matriz multi-spec sin huecos de testing aplicables | STD-0.1A completa |
| `ASYNC-SELECT-SEMA-001` | `ASYNC-SELECT-FRONTEND-001` y DEC-020 | Canales o backend nativo |
| `ASYNC-SELECT-LOWER-001` | semántica de efecto/arms y scheduler M7 existente | STD-0.1B |
| `STD-A-SELECTABLE-IMPL-001` | lowering/ownership de `select`, `Waiter` y time-base implementados | `Group` o canales |
| `ASYNC-SELECT-VM-CONF-001` | frontend, lowering, ownership, adapters, modelo/tests y presupuesto de rendimiento del selector | Backend nativo |
| Gate G5 vivo | `DOC-TEST-001`, `DOC-TEST-CONF-001`, `CONF-MATRIX-ALL-001`, `CONF-GAP-AUDIT-001`, `CONF-GAP-IMPL-001`, `CONF-LAYER-RESULT-001`, `QUALITY-EVIDENCE-BIND-001` y `CONF-SEAL-FINAL-001` | STD-0.1A completa |
| `DIAG-SPEC-001` | `PERF-001`, contrato CLI/testing y RFC-019 | Implementación de detectores |
| Contratos runtime-facing B0 | `DIAG-SPEC-001`, foundations STD-0.1A y `ASYNC-SELECT-VM-CONF-001` | `DIAG-RUNTIME-001` o backend nativo |
| `DIAG-RUNTIME-001` | `DIAG-SPEC-001`, contratos B0, VM hosted, async/threads/unsafe y source maps | Backend nativo |
| `RACE-001` / `LEAK-001` / `DUMP-001` | `DIAG-RUNTIME-001` y sus respectivos fixtures/negativos | Modelo de memoria nativo o implementación de owners B |
| `DIAG-TEST-001` | Detectores `RACE-001`, `LEAK-001`, `DUMP-001` y runner de retries/shards | CI específico |
| `DIAG-CI-001` | `DIAG-TEST-001`, `PERF-001`, fuzzing y corpus persistente | Selección de backend |
| `NATIVE-001` | `NATIVE-PRODUCT-SPEC-001`, target/artifact/link/publish specs, Gates G5/S1A, `select` VM conforme, contratos runtime-facing B0 y `DIAG-CI-001` | Campaña AOT y decisión `DEC-013` |
| `NATIVE-AOT-SCOPE-001` | `NATIVE-001`, `PERF-001` y `NATIVE-MEM-ADR-001` | Lowering AOT completo |
| `NATIVE-AOT-LOWER-001` | Alcance AOT, ABI y lowering mínimo común | Binarios enlazados comparables, memoria y calidad |
| `NATIVE-AOT-BINARY-001` | Lowering AOT, plan de enlace y empaquetado reproducible | Rendimiento AOT completo |
| `NATIVE-AOT-MEM-001` | Lowering AOT, ARC/ciclos y diagnóstico nativo | Rendimiento AOT completo |
| `NATIVE-AOT-QUALITY-001` | Lowering AOT, conformidad/differential y diagnóstico nativo | Rendimiento AOT completo |
| `NATIVE-AOT-PERF-001` | Binarios, memoria, calidad y `PERF-001` | `DEC-013` |
| `NATIVE-ABI-001` | `NATIVE-BACKEND-ADAPTER-001`, `NATIVE-001`, `NATIVE-MEM-ADR-001`, contratos de sync/executor y hooks `RACE`/`LEAK`/`DUMP` | ABI FFI pública |
| `DIAG-NATIVE-001` | `NATIVE-002`, memoria/ABI/lowering nativos, lane `NATIVE-THREAD-001` ya cerrada y detectores VM | Conformidad N1 |
| ARC/runtime nativo | `NATIVE-ABI-001` y DEC-014 | Eliminación de retains, COW o escape analysis |
| `NATIVE-LINK-001`/`NATIVE-CLI-001` | target/artifact/link schemas, lowering, ARC/ciclos y `NATIVE-STD-001` | Optimizaciones post-N1 |
| `DIAG-STDLIB-001` | owners B implementados y `DIAG-NATIVE-001` | Gate S1 |
| Gate S1 | N1, todos los slices A/B conformes, `DIAG-STDLIB-001` y `STD-S1-SEAL-001` | `REL-0.1-RC-001` |
| Gate L0 | benchmark reproducible, codec TLF, maps/diagnostics/CLI, properties/fuzz, evaluación, conformidad y bundle L0 | `TLF-REL-001` companion; nunca el candidato base |
| `REL-0.1-RC-001` | G5, T0, N1 y S1 | supply chain e instalación |
| `REL-SUPPLY-001` / `REL-INSTALL-001` | candidato exacto y decisión humana de licencia donde aplique | `REL-PUBLISH-001` |
| `REL-PUBLISH-001` | RC, supply/install, CI verde y autorización explícita | publicación externa 0.1 |

### 4.1.2 Regla de integración por waves

Cada wave termina con un mini-gate que actualiza inventario, trazabilidad,
tests, cobertura aplicable y conformidad viva. Una wave posterior no utiliza
una API provisional de la anterior. Trabajo de lanes distintas puede ejecutarse
en paralelo; dos cambios que toquen el mismo schema, parser, IR o runtime se
integran en el orden de la tabla anterior.

### 4.1.3 Prioridad transversal de diagnóstico

La lane `DIAG` se ejecuta antes de seleccionar el backend nativo y conserva el
principio de una superficie pequeña. El orden es contractual para el trabajo,
no una promesa de implementación ya cerrada:

| ID | Prioridad | Alcance | Estado |
|---|---:|---|---|
| `DIAG-SPEC-001` | P0 | Profiles, envelope, dumps, identidad, privacidad, límites y CLI | Contrato cerrado; runtime D1 separado |
| `DIAG-RUNTIME-001` | P0 | Eventos de memoria/sync, task/thread registry, roots/retainers, recursos, source maps, scheduler y quiescencia en VM hosted | Implementado; registro y contrato D1 cerrados |
| `RACE-001` | P0 | Race detector dinámico con happens-before, stacks y corpus positivo/negativo | Implementado en VM hosted; paridad lógica nativa cerrada; adapters públicos pendientes |
| `LEAK-001` | P0 | Retención GC, recursos afines, FFI y snapshots de crecimiento | Implementado en VM hosted; paridad lógica nativa cerrada; adapters públicos pendientes |
| `DUMP-001` | P0 | Captura lógica `.tdump`, redacción y analizador human/JSON | Implementado VM hosted; paridad lógica nativa cerrada; señal física por target |
| `DIAG-TEST-001` | P0 | Intentos aislados, retries, shards, artifacts JSON/JUnit | Implementado VM hosted |
| `DIAG-CI-001` | P0 | Lanes, fuzzing, budgets y promotion gate | Implementado hosted; workflow opt-in promovida |
| `DIAG-NATIVE-001` | P0 | Paridad ejecutable de envelopes entre Cranelift y LLVM | Cerrado; ocho casos, redacción, ARC, FFI, recursos, unwind, source maps, corrupción y límites |

`DIAG-SPEC-001` y `DIAG-RUNTIME-001` bloquearon `NATIVE-001`. `RACE-001`,
`LEAK-001` y `DUMP-001` están cerrados en VM hosted; `DIAG-NATIVE-001` demuestra
la paridad lógica ejecutable en ambos candidatos y conserva la declaración
explícita de capacidades físicas por target después del cierre de Gate N1.

### 4.2 Mapa de cobertura del spec

Esta tabla evita que una característica quede fuera del tracker por encontrarse
entre dos subsistemas:

| Capítulo normativo | Implementación principal | Validación final |
|---|---|---|
| 5. Código fuente y léxico | M1 | G0 y M10 |
| 6. Programas, módulos y paquetes | M2 para módulos; M9 para toolchain | G1, G4 y M10 |
| 7. Declaraciones, nombres y visibilidad | M2 | G1 y M10 |
| 8. Sistema de tipos | M2, M4, M5 y M6 | G3 y M10 |
| 9. Tipos compuestos | M2; runtime en M3 | G2 y M10 |
| 10. Colecciones intrínsecas | M6 | G3 y M10 |
| 11. Funciones, métodos y cierres | M2, M4 y M7 | G3, G4 y M10 |
| 12. Genéricos y traits | M4 | G3 y M10 |
| 13. Expresiones y control | M2; cleanup en M5 | G1, G3 y M10 |
| 14. Patrones y `match` | M2; lowering en M3 | G1, G2 y M10 |
| 15. Errores y pánicos | M2, M3 y M5 | G2, G3 y M10 |
| 16. Mutabilidad, memoria y concurrencia | M5, M7 y M9; APIs en STD-0.1B | G3, G4, M10 y S1 |
| 17. Operadores | M2, M6 y M8 | G3, G4 y M10 |
| 18. Semántica numérica | M3 y M6 | G3 y M10 |
| 19. Texto y Unicode | M1 para léxico; M6 para runtime | G0, G3 y M10 |
| 20. Ejecutables, scripts y procesos | M3, M7, M8 y M9; API host en STD-0.1A | G2, G4, M10 y S1A |
| 21. Formato y documentación | M1, `DOC-TEST-001` y trabajo transversal | G0, `DOC-TEST-CONF-001` y G5 |
| 22. Diagnósticos y tooling | M0, M1, M2, M9 y M10 | Todos los gates |
| 23. Gramática de referencia | M1 | G0 y M10 |
| 24. Ejemplos integrados | Tests de aceptación progresivos | G2, G3, G4 y M10 |
| 25. Características ausentes | Compile-fail distribuido por milestone | M10 |
| 26. Frontera con la stdlib | M6, M8, STD-0.1A y STD-0.1B | G3, G4, M10, S1A y S1 |
| 27. Metaprogramación estática | M10.7; providers en STD-0.1A | G5 y S1A |
| 28. Testing integrado Tondo 0.1 | Time-base de STD-0.1A + M10.6; helpers en STD-0.1A | T0, G5 y S1A |
| Tondo LLM Form | Lane TLF sobre lexer/CST/formatter existentes | L0 y `TLF-REL-001` |

---

## 5. M0 — Fundación del proyecto

**Objetivo:** poder desarrollar y validar el compilador con un loop corto,
reproducible y sin decisiones arquitectónicas implícitas.

- [x] **BOOT-001 — Fijar la revisión inicial del lenguaje.** La implementación
  comienza contra `TONDO_LANGUAGE_SPEC.md` revisión `0.1-draft.8`.

- [x] **BOOT-002 — Crear este tracker.**

- [x] **BOOT-003 — Crear el workspace Rust mínimo.** Incluir
  `tondo-cli`, `tondo-compiler` y `tondo-vm`, sin dependencias de backend nativo.

- [x] **BOOT-004 — Fijar la versión mínima de Rust y el toolchain.** El build
  limpio debe utilizar un toolchain declarado, no el que casualmente exista en
  una máquina.

- [x] **BOOT-005 — Crear la CLI vacía con los comandos `fmt`, `check` y `run`.**
  Los comandos todavía pueden devolver un diagnóstico explícito de feature no
  implementada, pero no aparentar éxito.

- [x] **BOOT-006 — Definir el driver de compilación.** Una única API debe recibir
  fuentes, edición, target, perfil, capacidades y opciones diagnósticas.

- [x] **BOOT-007 — Implementar el modelo de fuente y spans.** Offsets en bytes,
  line index lazy, paths lógicos, archivos virtuales y orden estable.

- [x] **BOOT-008 — Implementar el contenedor de diagnósticos.** Debe aceptar
  primary span, `related`, notas y fixes antes de que exista el primer error
  concreto.

- [x] **BOOT-009 — Crear el harness de tests.** Soportar fixtures inline,
  compile-pass, compile-fail, snapshots humanos, JSON estructurado y runtime.

- [x] **BOOT-010 — Añadir comprobaciones locales reproducibles.** Como mínimo:

  ~~~text
  cargo fmt --check
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  ~~~

- [x] **BOOT-011 — Escribir `docs/architecture.md`.** Debe describir las fases,
  invariantes, ownership de datos del compilador y qué estructuras pueden
  sobrevivir entre fases.

- [x] **BOOT-012 — Registrar ADR-001 a ADR-015.** Una decisión puede evolucionar,
  pero no debe quedar únicamente en conversaciones.

### Gate de salida de M0

- El workspace compila desde limpio.
- La CLI muestra ayuda y errores de uso deterministas.
- Un test puede proporcionar una fuente virtual y observar diagnostics JSON.
- Existe una única ruta del driver, aunque las fases todavía sean stubs
  explícitos.
- Las decisiones arquitectónicas iniciales están versionadas.

---

## 6. M1 — Fuente, lexer, parser y formatter

**Objetivo:** alcanzar G0 con una representación sintáctica fiable que pueda
servir simultáneamente al compilador, formatter, diagnósticos y tooling.

### 6.1 Fuente y léxico

- [x] **LEX-001 — Validar UTF-8 y conservar bytes originales.** Implementar
  `E0001` sin sustitución silenciosa de secuencias inválidas.

- [x] **LEX-002 — Normalizar identificadores según el contrato NFC.** Conservar
  spelling y span originales para diagnósticos y formatter.

- [x] **LEX-003 — Tokenizar trivia y newlines.** Whitespace y comentarios deben
  permanecer en el CST aunque no lleguen al HIR.

- [x] **LEX-004 — Implementar keywords, nombres contextuales y operadores.**
  Separar keywords léxicas de nombres reservados contextuales.

- [x] **LEX-005 — Implementar literales.** Enteros, sufijos, floats, chars,
  strings, escapes, multiline e interpolación.

- [x] **LEX-006 — Implementar shebang condicionado al modo script.**

- [x] **LEX-007 — Implementar `E0002` y `E0003` con recuperación local.**

Evidencia observada el 2026-07-21:

- El driver público ejecuta el lexer para todas las fuentes y no añade `T0001`
  cuando ya existe un error léxico normativo.
- Las tablas `XID` y NFC están fijadas exactamente a Unicode 16.0.0.
- La suite cubre reconstrucción byte a byte, UTF-8 inválido, NFC, las 41
  keywords, maximal munch, comentarios anidados, `NL`, todos los literales,
  interpolación, shebang, recuperación y límites explícitos.
- Los 295 fences Tondo de `TONDO_LANGUAGE_SPEC.md` se lexan sin diagnósticos y
  conservan una partición física exacta.

### 6.2 CST y parser

- [x] **PARSE-001 — Definir el inventario de nodos CST.** Todo token debe
  pertenecer al árbol, incluido trivia y tokens inesperados recuperados.

- [x] **PARSE-002 — Implementar declaraciones y tipos mediante recursive
  descent.**

- [x] **PARSE-003 — Implementar expresiones mediante Pratt parsing.** La tabla
  debe corresponder exactamente a la precedencia normativa.

- [x] **PARSE-004 — Preservar ambigüedades contextuales.** En particular,
  corchetes de índice o argumentos genéricos, record o bloque, cierre o grupo y
  formas de `for`.

- [x] **PARSE-005 — Implementar patrones y `match`.**

- [x] **PARSE-006 — Implementar modo módulo, script y fragmento.**

- [x] **PARSE-007 — Diseñar recuperación sin cascadas.** Un error temprano no
  debe fabricar tipos ni eliminar declaraciones posteriores independientes.

- [x] **PARSE-008 — Implementar `E0004`, `E0005` y `E0006`.**

- [x] **PARSE-009 — Crear una fachada AST tipada sobre el CST.** No duplicar
  texto, trivia ni spans.

Evidencia observada el 2026-07-21:

- `SyntaxKind` cubre el inventario cerrado y cada token físico o sintético
  pertenece al CST en orden de árbol; `syntax::ast` ofrece una vista comprobada
  para cada kind sin crear un segundo árbol.
- El recursive descent cubre declaraciones, tipos, patterns, `match`, los tres
  source forms públicos y las superficies aisladas usadas por los doc-tests.
- El Pratt parser coincide con la precedencia normativa, conserva los nodos
  preliminares contextuales y emite `E0005` para las familias no asociativas.
- `E0004`, `E0005` y `E0006` atraviesan el driver público y preemptan `T0001`;
  archivos importados se fuerzan siempre a forma módulo.
- La recuperación conserva tokens inesperados, inserta missing tokens de ancho
  cero, suprime cascadas por línea y mantiene métodos o declaraciones
  independientes posteriores.
- Los 295 fences Tondo del spec alcanzan una superficie sintáctica válida o el
  código esperado. Todos los bytes individuales, 2.048 entradas binarias
  deterministas y el límite profundo se resuelven sin crash ni pérdida de
  fuente.
- El límite request-wide de nodos, diagnostics y nesting produce rechazo
  tipado. `PARSER-STACK-001` eliminó la guarda interna de 128 niveles: el
  presupuesto configurado es ahora el único límite lógico y se carga contra
  frames explícitos, con recuperación y reconstrucción lossless seguras en
  stacks pequeños.

### 6.3 Formatter

- [x] **FMT-001 — Implementar el modelo de documentos del formatter normativo.**

- [x] **FMT-002 — Implementar layout, indentación, continuaciones y trailing
  commas.**

- [x] **FMT-003 — Implementar placement de comentarios y doc comments.**

- [x] **FMT-004 — Preservar shebang y distinguir módulo, script y fragmento.**

- [x] **FMT-005 — Ejecutar el corpus mínimo de formato del spec byte a byte.**

- [x] **FMT-006 — Probar idempotencia.** Para toda entrada válida del corpus,
  `F(F(source)) == F(source)`.

- [x] **FMT-007 — Probar estabilidad con entradas inválidas recuperables.** El
  formatter no debe perder tokens ni convertir código inválido en código válido
  con significado inventado.

Evidencia observada el 2026-07-21:

- El renderer normativo usa grupos deterministas, 100 scalars Unicode, cuatro
  espacios, `LF`, ausencia de whitespace final y exactamente un salto final.
- Listas, records, bloques, operadores, cadenas postfix, comentarios, doc
  comments, imports, shebang y los tres source forms comparten el CST lossless.
- `tondo fmt` produce fuente canónica en stdout sin modificar el archivo;
  `tondo fmt --check` comprueba silenciosamente el fixed point.
- El corpus mínimo coincide byte a byte, los 295 fences se procesan según su
  superficie normativa y todo fence sintácticamente válido se formatea,
  reparsa y vuelve a formatear con resultado idéntico.
- Una entrada léxica, sintáctica o materialmente limitada se rechaza sin emitir
  fuente parcial ni fabricar un programa válido.

### 6.4 Robustez

- [x] **ROBUST-001 — Fuzzear lexer y parser.** Cualquier secuencia de bytes debe
  producir árbol/diagnóstico o rechazo válido, nunca crash del proceso.

- [x] **ROBUST-002 — Fuzzear idempotencia del formatter sobre árboles válidos.**

- [x] **ROBUST-003 — Probar límites de nesting y tamaño.** El rechazo por
  recursos debe ser controlado.

Evidencia observada el 2026-07-21:

- Los 256 inputs de un byte y 2.048 secuencias binarias pseudoaleatorias con
  seed fija terminan de forma controlada y conservan la partición física.
- 512 programas válidos generados por gramática alcanzan un fixed point del
  formatter después de parsear y reparsar.
- Los límites request-wide de bytes, archivos, tokens, nodos, diagnostics y
  nesting se rechazan mediante `T0002`; el formatter nunca entrega output
  parcial tras ese rechazo.

### Gate G0

**Estado:** cerrado el 2026-07-21.

- Todos los ejemplos sintácticamente válidos del spec se parsean.
- Todos los casos sintácticos inválidos aplicables producen su código estable.
- El CST reproduce exactamente la secuencia de tokens de entrada.
- El corpus normativo de formato coincide byte a byte.
- El formatter es idempotente.
- Human diagnostics y JSON comparten los mismos datos estructurados.

---

## 7. M2 — Resolución y semántica bootstrap

**Objetivo:** alcanzar G1: `tondo check` debe comprender un subconjunto coherente
del lenguaje, no limitarse a verificar sintaxis.

### 7.1 Paquetes, módulos y nombres

- [x] **RESOLVE-001 — Recibir un grafo de paquetes ya cerrado.** Durante
  bootstrap el driver puede construirlo en memoria; el schema del manifiesto no
  pertenece todavía a este milestone.

- [x] **RESOLVE-002 — Implementar `PackageId` e identidad nominal completa.**

- [x] **RESOLVE-003 — Implementar módulos distribuidos entre archivos e imports
  acíclicos.**

- [x] **RESOLVE-004 — Implementar namespaces separados para tipos, valores,
  módulos y miembros.**

- [x] **RESOLVE-005 — Implementar visibilidad `pub`/`priv` y validación de APIs
  públicas.**

- [x] **RESOLVE-006 — Prohibir shadowing y redeclaraciones según sus scopes.**

- [x] **RESOLVE-007 — Resolver sin depender del orden textual ni del orden de
  archivos.**

- [x] **RESOLVE-008 — Implementar `E1001` a `E1008`.**

### 7.2 Representación de tipos

- [x] **TYPE-001 — Crear el interner de tipos canónicos.**

- [x] **TYPE-002 — Implementar escalares, `Unit`, `Never` y tipos función.**

- [x] **TYPE-003 — Implementar tuples, aliases, newtypes, records y enums.**

- [x] **TYPE-004 — Implementar uniones estructurales normalizadas.** Aplanado,
  deduplicación y reducción de `Never` deben ser deterministas.

- [x] **TYPE-005 — Implementar `Option[T]`, `Result[T, E]`, `T?`, `T ! E` y
  `!E` como formas equivalentes normativas.**

- [x] **TYPE-006 — Implementar asignabilidad exacta, invariancia y conversiones
  numéricas explícitas del subconjunto bootstrap.**

- [x] **TYPE-007 — Implementar inferencia local bidireccional.** El solver
  request-local invariante tiene rollback, occurs-check, contexto de resultado,
  restricciones por argumento y cierre obligatorio sin variables irresueltas.
  No introduce inferencia global ni Hindley-Milner general; la resolución de
  bounds y la monomorfización completa continúan en M4.

- [x] **TYPE-008 — Implementar recursión productiva y rechazo de aliases
  recursivos.**

Evidencia observada el 2026-07-21:

- El driver público baja todas las expresiones de tipo y firmas a un único HIR
  semántico antes de emitir `T0001`; `E1104`, `E1106`, `E1107`, `E1110`,
  `E1115` y `E1117` preemptan correctamente ese marcador.
- Aliases transparentes, genéricos, bounds, `Self`, receivers, variádicos,
  resultados opacos, tipos nominales y todas las grafías de `Option`/`Result`
  comparten la representación canónica documentada.
- La productividad usa SCCs y un punto fijo mínimo con sustitución genérica;
  los recorridos profundos, sustituciones, unificación y serialización usan
  worklists explícitas y respetan el presupuesto de nodos.
- El lowering produce el mismo snapshot al invertir el orden de inserción de
  archivos de un módulo.
- El gate acumulado observado es de 176 tests, formatter check, Clippy con
  warnings denegados y Rustdoc sin warnings.

### 7.3 Declaraciones y control de flujo

- [x] **CHECK-001 — Typecheckear constantes, bindings, funciones y métodos
  inherentes no genéricos.**

- [x] **CHECK-002 — Typecheckear bloques, `if`, las tres formas de `for`,
  `break`, `continue` y `return`.**

- [x] **CHECK-003 — Implementar `fail`, construcción de `Result` y propagación
  mediante `?`.**

- [x] **CHECK-004 — Implementar widening válido de uniones de error.**

- [x] **CHECK-005 — Implementar patrones, irrefutabilidad, guards y
  exhaustividad de `match`.**

- [x] **CHECK-006 — Implementar asignación simple y múltiple con evaluación
  previa del RHS.**

- [x] **CHECK-007 — Implementar análisis de reachability y `Never`.**

- [x] **CHECK-008 — Implementar descarte explícito `_ = expression` y rechazo
  inicial de resultados no `Unit` descartados.**

- [x] **CHECK-009 — Implementar las consultas semánticas mínimas del apartado
  22.5 para símbolos, tipos y firmas.**

- [x] **CHECK-010 — Typecheckear accesos, llamadas, literales y actualización
  `with` de records, variantes y operadores del subconjunto bootstrap.**

Evidencia observada el 2026-07-21:

- El HIR tipado asigna tipo, categoría, span e identidad resuelta a cada
  expresión del subconjunto completado y materializa coerciones contextuales.
- Constantes, bindings simples, funciones y métodos inherentes no genéricos se
  comprueban por la ruta pública. Las constantes acíclicas conservan su valor
  normalizado; cada SCC cíclica produce un único `E1902` estable por identidad
  lógica y no por orden de archivos.
- Bloques, `if`, `match`, los tres `for`, `break`, `continue`, `return`, `fail`
  y llamadas conservan un resumen explícito de finalización normal separado de
  su tipo contextual. Cada loop tiene identidad propia y consume únicamente
  sus breaks alcanzables.
- Un `for {}` sin salida del mismo loop es `Never`; breaks muertos, destinados
  a loops anidados o posteriores a otra transferencia no lo convierten en
  `Unit`. Headers divergentes, joins completos de ramas y coerciones de `Never`
  propagan el flujo sin heurísticas interprocedurales.
- Un worklist HIR top-down emite `W1006` siguiendo el orden de evaluación de
  statements, destinos, RHS, operandos, argumentos, branches, arms y headers,
  sin entrar en subárboles ya inalcanzables. Warnings no preemptan la siguiente
  fase del driver; errores semánticos sí.
- `_ = expression` tiene una sentencia HIR propia; `_` dentro de asignación
  múltiple conserva su posición de hoja. Ambos exigen `Discard`, mientras una
  expresión no `Unit` abandonada implícitamente produce `E1303`.
- La derivación bootstrap de `Discard` es estructural y coinductiva: atraviesa
  colecciones y nominales genéricos sin expandir recursión transformadora,
  propaga la obligación terminal de `Join` y acepta bounds `Discard`, `Copy` o
  `Key`. Parámetros `_` prestados no adquieren ownership ni exigen capacidad.
- `E1105` por descarte inválido preempta `T0001`; opacos, cursores y closures
  todavía sin contrato publicado se difieren explícitamente al milestone de
  capacidades/ownership.
- `CompilationOutput` conserva un snapshot semántico request-owned desde la
  resolución. Los rechazos parciales publican únicamente las fases realmente
  completadas; errores léxicos/sintácticos y `fmt` no fingen un modelo.
- Las consultas públicas cubren tipos contextuales de expresión, entidades y
  declarations, referencias, firmas globales y de métodos, miembros de enum y
  unión, firma directa y conjunto cerrado de errores de una llamada.
- Usos de fields y variantes se registran en el token exacto donde el checker
  los desambigua. Rangos visibles toleran trivia del CST, coerciones exteriores
  ganan los empates y las referencias se ordenan por identidad lógica, no por
  `FileId` ni orden de inserción.
- IDs de arena permanecen locales al snapshot; identidad nominal completa y
  serialización canónica siguen siendo la frontera estable de tooling. Los
  hechos de ownership, borrows, closures y capacidades de 22.5 continúan
  diferidos a sus análisis reales.
- `some`, `none`, `ok`, `err`, la elevación de éxito, `fail` y ambos canales de
  `?` están implementados sin doble envoltura de `Result`. El widening cerrado
  distingue inyección de un error y ampliación de una unión-subconjunto.
- Las fuentes intrínsecas de `for` conservan su protocolo cerrado. Un nominal
  exige ahora un `Iterator[T]` visible o implementado; HIR fija el elemento y la
  firma de `next`, y la ausencia real produce `E1206`.
- `E1101`, `E1102`, `E1109`, `E1115`, `E1116`, `E1205`, `E1206`, `E1301` a
  `E1304`, `E1405`, `E1407`, `E1411` y `E1901` a `E1903` preemptan `T0001` en
  el driver. El
  presupuesto conjunto de expresiones/patrones HIR produce `T0002`.
- Todos los patrones de 23.26 producen HIR tipado. La matriz iterativa demuestra
  irrefutabilidad, arms inalcanzables y exhaustividad sobre dominios finitos,
  arrays vacío/no-vacío y dominios abiertos; guards no cuentan como cobertura.
- Paths importados, argumentos y aliases genéricos, literales decodificados,
  bindings prestados, control transfers directos y recuperación sin cascadas
  tienen regresiones específicas. El análisis de patrones tiene presupuesto
  propio y una prueba con un prefijo de array de 4.096 elementos.
- `E1201` a `E1204` preemptan `T0001` y el agotamiento del análisis produce
  `T0002` por la ruta pública.
- Asignación simple, compuesta y múltiple conserva destinos resueltos antes del
  RHS, coerciones por hoja y escritura izquierda-derecha. Campos, slots de
  tupla, índices, slices y entradas de map retienen sus operandos sin
  reevaluación; `mut` y `var` producen requisitos de extensión distintos.
- Los once operadores de asignación, swaps anidados, contexto parcial, overlap
  estático normalizado, todos los modos de mutabilidad y la política de maps
  tienen regresiones. `E1405` y el nuevo `E1411` son observables por el driver.
- Literales `Array`, `Map` y `Set`, todos los constructores nominales y `with`
  tienen HIR explícito, sustitución genérica invariante, validación de forma y
  orden de evaluación. Construcción, actualización, acceso y métodos respetan
  visibilidad entre módulos sin enumerar campos privados omitidos.
- Las llamadas conservan orden textual y asocian cada argumento a receptor,
  parámetro fijo, elemento variádico o spread. Dot-call, forma calificada,
  operaciones asociadas y fields función comparten un único HIR; `mut self` y
  `var self` validan la capacidad de la ubicación.
- Las llamadas genéricas explícitas e inferidas materializan una
  `SpecializedFunction`; resultado esperado, argumentos, options y variádicos
  restringen el solver. Conflictos producen `E1102` y soluciones incompletas o
  ambiguas producen `E1101`.
- La tabla numérica cerrada materializa conversiones identity, total y checked;
  estas últimas producen `NumericConversionError` por el canal `Result`.
- `..` y `..=` producen `Range[T]` solo para extremos discretos idénticos. `in`
  distingue array, clave de map, set, range y carácter de string, conserva orden
  izquierda-derecha y contextualiza colecciones vacías inequívocas.
- La evaluación constante cerrada usa HIR tipado y nunca ejecuta bodies Tondo.
  Materializa escalares, agregados, nominales, options, results, colecciones,
  ranges y funciones nombradas especializadas; respeta cortocircuito, slicing
  Python, aritmética vectorizada e IEEE 754. Trabajo runtime produce `E1901` y
  pánicos o errores recuperables conocidos producen `E1903`.
- Claves constantes repetidas de map producen `E1116`; valores repetidos de set
  producen `W1011` y se normalizan conservando el primero; comparaciones con NaN
  conocido producen `W1008`. Expresiones dinámicas no se ejecutan ni se adivinan.
- El gate acumulado observado es de 248 tests. `cargo fmt --check`, Clippy con
  warnings denegados, la suite workspace locked y Rustdoc con warnings
  denegados pasan después de habilitar la aceptación pública de `tondo check`.

- [x] **CONST-001 — Implementar evaluación constante cerrada.** Debe resolver
  constantes, rangos de literales y claves duplicadas sin ejecutar código
  Tondo arbitrario.

- [x] **CONST-002 — Detectar ciclos y ordenar errores constantes
  determinísticamente.**

### Gate G1

**Estado:** cerrado el 2026-07-21.

- `tondo check` acepta programas bootstrap positivos de varios módulos.
- Los errores de nombre, visibilidad, tipo, control y pattern matching tienen
  códigos normativos y spans precisos.
- El resultado no cambia al permutar archivos de un módulo.
- Las uniones y sustituciones tienen una serialización canónica estable.
- Los fixtures compile-pass y compile-fail de la superficie implementada pasan.

Evidencia observada el 2026-07-21:

- La suite workspace contiene 248 tests y pasa completa con `--locked`.
- `cargo fmt --all -- --check`, Clippy para todos los targets con
  `-D warnings` y Rustdoc workspace con `-D warnings` pasan.
- La CLI acepta silenciosamente `tests/compile-pass/bootstrap-semantics.to` con
  exit 0, rechaza el overflow constante con `E1903` y mantiene `run` detrás del
  marcador explícito `T0001` hasta M3.

---

## 8. M3 — MIR, bytecode y VM

**Objetivo:** alcanzar G2 y poder afirmar que Tondo tiene un primer compilador.

### 8.1 HIR tipado y MIR

- [x] **MIR-001 — Definir las invariantes del HIR tipado.** Toda expresión debe
  tener tipo, símbolo resuelto y categoría de valor.

Evidencia observada el 2026-07-21:

- ADR-016 y `docs/contracts/mir.md` fijan la admisión HIR, el reparto de
  responsabilidades y la ubicación explícita de moves, loans, cleanup y
  suspensión sin delegarlos al backend.
- Todo HIR completo y sin errores atraviesa `verify_typed_hir` incluso durante
  `tondo check`. Snapshots parciales siguen disponibles para tooling, pero no
  pueden entrar en MIR.
- El verificador comprueba tipos canónicos, arenas topológicas y alineadas,
  identidades resueltas, categorías `Value`/`Place`, constantes, callables,
  patterns, campos, loops y metadatos de flujo. Sus cinco regresiones negativas
  mutan HIR válido para probar cada frontera material.
- La suite workspace acumulada contiene 253 tests y pasa con `--locked`; el
  formatter check y Clippy para todos los targets con warnings denegados pasan.

- [x] **MIR-002 — Bajar a un CFG explícito.** Blocks, terminators, locals y
  temporales no deben depender de la forma del AST.

- [x] **MIR-003 — Bajar `if`, `for`, `match`, `return`, `fail` y `?`.**

- [x] **MIR-004 — Representar `Never` y ramas sin sucesor normal.**

- [x] **MIR-005 — Introducir cleanup edges desde el principio.** Aunque las
  obligaciones terminales lleguen en M5, el MIR no debe necesitar rediseño para
  `defer`, pánico o cancelación.

- [x] **MIR-006 — Crear un verificador interno del MIR.** Ningún backend debe
  aceptar CFG roto, tipos inconsistentes o locals fuera de vida.

- [x] **MIR-007 — Conservar source spans a través de lowering.**

Evidencia observada el 2026-07-21:

- Todo HIR completo baja a funciones deterministas con locals tipados, blocks,
  terminators y unwind blocks explícitos. La cobertura incluye cortocircuito,
  las tres formas de `for`, los cinco iterables bootstrap, patterns y guards,
  `Never`, transfers, propagación, llamadas, construcción, colecciones,
  accesos, slices y asignación atómica de múltiples destinos.
- Las lecturas indexadas y sliced son operaciones checked con unwind; los
  payloads solo son proyectables bajo un `SwitchTag` dominante; las llamadas
  conservan callable, especialización, receiver, modos y asociación variádica.
- El verificador prueba CFG y cleanup, tipos y proyecciones instanciadas,
  aridad/modos de calls, inicialización definida, storage lifetime, refinamiento
  de tags, return place y spans. Las regresiones negativas mutan MIR válido para
  demostrar rechazo de edges, proyecciones, usos, calls, tags y presupuestos
  inválidos.
- Los límites de funciones, blocks, locals, statements y pasos de dataflow
  están conectados a `CompilationRequest`; su agotamiento produce `T0002` antes
  de bytecode. `tondo run` atraviesa lowering y verificación antes del marcador
  deliberado `T0001` de la siguiente fase.
- La suite workspace acumulada contiene 269 tests y pasa completa para todos
  los targets con `--locked`; formatter check, Clippy y Rustdoc con warnings
  denegados pasan. El smoke test de `tondo check` termina con exit 0 y el de
  `tondo run` alcanza exactamente `T0001` tras verificar MIR.

### 8.2 Bytecode

- [x] **BC-001 — Definir instrucciones por slots.** Loads, stores, constants,
  arithmetic, branches, calls, construction, projection y returns.

- [x] **BC-002 — Mantener una tabla de tipos y spans por función.**

- [x] **BC-003 — Implementar un verificador de bytecode.** Validar índices,
  tipos de operands, destinos de branch y aridad de llamadas.

- [x] **BC-004 — Generar bytecode determinista desde el mismo MIR.**

- [x] **BC-005 — Crear un disassembler solo de tooling.** Debe ayudar a tests y
  debugging sin convertirse en ABI estable.

Evidencia observada el 2026-07-21:

- El formato propiedad de `tondo-vm` representa todos los tipos, declaraciones
  nominales, callables, constantes, slots, places, operaciones, terminators,
  cleanup edges y spans necesarios para la superficie HIR/MIR bootstrap, sin
  conservar IDs ni interner del compilador.
- El lowering asigna índices densos de forma determinista, crea tablas locales
  ordenadas de tipos y spans y vuelve a admitir el resultado mediante el
  verificador independiente de la VM antes de entregarlo al runtime.
- El verificador rechaza índices, layouts instanciados, tipos, proyecciones,
  calls, edges, storage lifetime, inicialización y refinamiento de tags
  inválidos. Sus análisis usan worklists y un presupuesto explícito; el driver
  traduce el agotamiento a `T0002`.
- Las regresiones cubren bytecode mutado, aridad incorrecta, payload sin
  discriminante dominante, límites de construcción/dataflow y la bajada de
  asignaciones, colecciones, conversiones, Option/Result, llamadas
  variádicas/métodos y los cinco iterables bootstrap.
- El desensamblador es texto determinista de tooling y el contrato documenta
  expresamente que no existe formato serializado ni loader estable.
- La suite workspace acumulada contiene 278 tests y pasa completa para todos
  los targets con `--locked`; formatter check, Clippy y Rustdoc con warnings
  denegados pasan. `tondo check` termina con exit 0 y `tondo run` alcanza
  exactamente el marcador `T0001` después de verificar MIR y bytecode.

### 8.3 VM bootstrap

- [x] **VM-001 — Implementar frames, slots, llamadas y retorno.**

- [x] **VM-002 — Implementar `Bool`, enteros, floats, `Unit`, strings bootstrap,
  tuples, records y enums.**

- [x] **VM-003 — Implementar aritmética comprobada y clases de pánico
  normativas.**

- [x] **VM-004 — Implementar branches, loops y pattern dispatch.**

- [x] **VM-005 — Implementar `assert` y `panic` con ubicación y stack trace
  cuando haya símbolos.**

- [x] **VM-006 — Implementar `main` síncrono, exit status y frontera de error.**

- [x] **VM-007 — Crear un shim bootstrap de `std.console.print`.** Debe quedar
  aislado de la futura API estándar y documentado como provisional.

- [x] **VM-008 — Implementar el heap preciso, no móvil y mark-and-sweep
  bootstrap.** Debe recorrer roots de frames y objetos existentes, aunque M5
  amplíe después el universo trazable y sus pruebas bajo presión.

- [x] **VM-009 — Probar que bytecode inválido se rechaza antes de ejecutar.**

### 8.4 Programas de aceptación de G2

- [x] **ACCEPT-001 — Programa sin I/O.**

  ~~~tondo
  fn add(left: Int, right: Int): Int {
      left + right
  }

  fn main() {
      assert(add(20, 22) == 42)
  }
  ~~~

- [x] **ACCEPT-002 — `Hello, world`.**

  ~~~tondo
  import std.console

  fn main() {
      console.print("Hello, world")
  }
  ~~~

- [x] **ACCEPT-003 — Enum, `match`, `Result` y `?`.**

- [x] **ACCEPT-004 — Loop, checked overflow y panic con span.**

- [x] **ACCEPT-005 — Dos módulos con visibilidad e identidad nominal.**

Evidencia observada el 2026-07-21:

- La VM usa frames iterativos, slots tipados y continuaciones explícitas; ejecuta
  scalars, strings, tuples, records, enums, options, results, colecciones,
  branches, loops, pattern dispatch, llamadas, retornos y unwind sin recurrir al
  stack Rust para llamadas Tondo.
- Las diez clases bootstrap `P0001` a `P0010` tienen identidad y nombre estables.
  Los tests cubren overflow, división por cero, bounds, step cero, shift
  inválido, overlap dinámico, shape de arrays, claves dinámicas duplicadas,
  `assert` y `panic`. `assert` conserva la representación fuente de la condición
  a través de HIR, MIR y bytecode para el mensaje por defecto.
- `main` síncrono valida unicidad, privacidad, aridad, genéricos, `unsafe`,
  outcome y `Discard` del error. `Unit`, `ok(Unit)`, error no manejado y pánico
  terminan respectivamente con 0, 0, 1 y 101.
- `std.console.print(String): Unit` es un host op tipado, provisional y gated por
  la capability cerrada `console`; sin ella el módulo no existe y el import
  produce `E1008`. La salida exacta no añade newline.
- El heap preciso, no móvil y generacional conserva roots, recupera ciclos,
  rechaza handles stale y recolecta bajo presión antes de OOM. La ejecución
  verifica todo el bytecode antes de seleccionar un frame o invocar al host; un
  test mutado demuestra cero llamadas host.
- Los fixtures `g2-001` a `g2-004` recorren la ruta pública y el caso
  multimódulo `g2-005` ejecuta bytecode mientras prueba además `E1102` para
  identidad nominal y `E1501` para privacidad. Los smoke tests del binario
  confirman exits 0/101, `P0005` y `Hello, world` byte por byte.
- `cargo test --workspace --all-targets --locked` pasa 307 tests; también pasan
  `git diff --check`, formatter check, Clippy con warnings denegados y Rustdoc
  con warnings denegados.

### Gate G2

- `tondo fmt`, `tondo check` y `tondo run` utilizan el mismo frontend.
- Los cinco programas de aceptación atraviesan la ruta completa.
- La VM no ejecuta bytecode sin verificar.
- Overflow, división inválida, bounds implementados y `panic` no dependen de
  modo debug/release.
- Los diagnósticos runtime contienen código, nombre estable y ubicación.
- El build se identifica expresamente como bootstrap y no conforme.

---

## 9. M4 — Genéricos, traits, funciones y closures

**Objetivo:** completar el modelo de abstracción estática sin introducir objetos
dinámicos ni dispatch oculto.

- [x] **GEN-001 — Implementar parámetros genéricos invariantes e inferencia de
  argumentos desde argumentos y tipo esperado.**

- [x] **GEN-002 — Implementar constraints e instanciación monomorfizada.**

- [x] **TRAIT-001 — Implementar declaración de trait y métodos por defecto.**

- [x] **TRAIT-002 — Implementar `impl`, orphan rules y coincidencia exacta del
  contrato.**

- [x] **TRAIT-003 — Detectar impls solapados antes de resolver constraints.**

- [x] **TRAIT-004 — Implementar el control de terminación por cambio de tamaño.**

- [x] **TRAIT-005 — Implementar dispatch estático, llamadas calificadas y
  métodos visibles a través de constraints.**

- [x] **TRAIT-006 — Implementar resultados opacos `impl Bound` con un único
  testigo concreto.**

- [x] **CAP-001 — Implementar las capacidades intrínsecas `Copy`, `Discard`,
  `Equatable`, `Key`, `Send` y `Share` como contratos cerrados.**

- [x] **CALL-001 — Implementar funciones como valores y coerción exacta a
  `fn(...)`.**

- [x] **CALL-002 — Implementar closures y captura por valor.**

- [x] **CALL-003 — Derivar `Call`, `CallMut` y `CallOnce` desde cuerpo y
  capturas.**

- [x] **CALL-004 — Implementar closures sync, suspendibles y unsafe en la
  representación semántica, aunque sus runtimes se activen después.**

Evidencia observada el 2026-07-21 para GEN-001, GEN-002, TRAIT-001 a TRAIT-006,
CAP-001 y CALL-001 a CALL-004:

- Los bodies genéricos bounded y unbounded se comprueban una sola vez con
  parámetros rígidos. Las llamadas explícitas e inferidas cierran todas las
  variables invariantes y pueden reenviar el binder exterior en tipos
  compuestos como `T?` y `Array[T]`.
- Cada especialización valida sus bounds antes de publicar HIR. `Copy`,
  `Discard`, `Equatable`, `Key`, `Send` y `Share` comparten una prueba
  estructural cerrada; traits fuente, `Display` e `Iterator[T]` usan selección
  estática y prueba recursiva. `Call`, `CallMut` y `CallOnce` usan una prueba
  cerrada independiente para funciones, closures, genéricos y opacos.
- La monomorfización se ejecuta entre MIR verificado y bytecode. Parte de todos
  los callables no genéricos y de function values constantes, sigue referencias
  transitivas, sustituye todos los tipos de firma y body y deduplica por
  callable más vector concreto de argumentos.
- El bytecode ejecutable publica callables de aridad genérica cero y calls sin
  type pack runtime. Las plantillas nominales genéricas permanecen únicas para
  que el verifier compruebe fields y variants con argumentos concretos.
- Recursión con la misma sustitución converge por deduplicación. Recursión que
  expande tipos termina en `T0002`; los límites cero, el presupuesto de
  obligaciones y el de nodos de tipo especializados tienen fallos controlados.
- Las regresiones ejecutan en la VM identidades, forwarding explícito,
  constantes función, records y fields genéricos, indexación de arrays y
  discriminantes de `Option`, con instancias `Int` y `String` separadas y orden
  determinista.
- Cada trait publica una tabla determinista de métodos requeridos, asociados y
  defaults. `Self` ocupa una posición genérica oculta después de los binders del
  trait y un receptor suspendible registra la obligación intrínseca `Self: Send`.
- Los defaults se comprueban una sola vez con parámetros rígidos y pueden
  llamar métodos del mismo trait sin lookup global. Las especializaciones de
  método inferidas o explícitas conservan el prefijo del trait y `Self`; los
  corchetes de un index siguen recorriendo su ruta ordinaria.
- El verifier exige correspondencia exacta entre resolución y tabla HIR,
  clasificación de receptor, aridad completa, prefijo genérico, presencia de
  body y requisito suspendible. Los defaults mantienen `Self` genérico y sólo se
  convierten en roots de bytecode cuando un dispatch concreto los selecciona.
- Cada `impl` publica una identidad estable, su cabecera normalizada, binders,
  métodos y contratos instanciados. La coincidencia exige nombre, receptor,
  modos, variadicidad, genéricos, bounds, `suspends`, `unsafe`, éxito y error
  exactos; un default puede omitirse o sustituirse.
- Las orphan rules se aplican después de expandir aliases y usan el constructor
  nominal exterior. Los protocolos cerrados no admiten `impl` manual, mientras
  `Display` e `Iterator[T]` exponen contratos prelude implementables.
- Los bodies de implementación atraviesan el checker ordinario. El admission
  verifier reconstruye cada contrato desde el trait y vuelve a comprobar IDs,
  binders, propiedad, cobertura y correspondencia uno-a-uno con callables.
- La coherencia agrupa por identidad de trait y compara la cabecera completa
  con ámbitos de binders independientes y una sola sustitución multi-raíz. Los
  bounds positivos no participan y aliases, shorthands y uniones llegan ya
  normalizados.
- Una cabecera ordinaria unificable produce `E1111`. `Iterator[T]` unifica
  primero el target y distingue una duplicación `E1111` de dos elementos
  funcionalmente incompatibles `E1113`; ambos diagnósticos apuntan al `impl`
  posterior y relacionan el anterior en orden lógico estable.
- El verifier repite la prueba de coherencia antes de MIR. Las regresiones
  cubren scopes alfa independientes, occurs checks, uniones sin orden, bounds
  ignorados, aliases, instanciaciones distintas, no cascada, orden de archivos,
  mutación del HIR y diagnósticos JSON públicos.
- La terminación convierte cada bound abierto de un `impl` genérico en una
  arista entre consultas canónicas, excluye las capacidades cerradas y deriva
  matrices `<`/`=`/`?` por subterm estructural sin depender de tipos concretos
  futuros.
- Un worklist satura matrices dentro de cada SCC de identidades de trait y
  rechaza con `E1112` toda matriz idempotente sin descenso diagonal. El
  diagnóstico reconstruye una ruta completa y estable con spans relacionados;
  las aristas acíclicas no necesitan descenso.
- Construcción, recorridos de tipos, composición, idempotencia y expansión del
  testigo consumen un presupuesto explícito y fallan como `T0002`. El verifier
  reconstruye independientemente el grafo y vuelve a demostrar terminación
  antes de MIR.
- Las regresiones cubren descenso, adaptadores acíclicos, ciclos iguales,
  mutuos, permutaciones, crecimiento, múltiples SCC, álgebra de composición,
  precedencia frente a overlap, orden lógico, HIR mutado y límite público.
- El lookup de método ordinario prioriza inherentes y sólo después consulta los
  traits visibles por constraints; nunca escanea impls globales. Una colisión
  produce `E1004` y exige calificación explícita, también entre traits fuente y
  prelude.
- Las llamadas calificadas cierran argumentos del trait, `Self` y genéricos del
  método, respetan modos de receptor y módulos importados, y prueban la consulta
  completa. La ausencia de implementación o de un bound sustituido produce
  `E1105`.
- HIR representa los contratos prelude con `PreludeTraitFunction` y verifica
  aridad, tipos canónicos y firma exacta. MIR conserva el operando estático y
  vuelve a verificar su receptor y outcome antes de bytecode.
- La monomorfización sustituye la consulta alcanzada, selecciona un único impl,
  distingue override de default, verifica igualdad exacta de firmas y encola
  sólo el callable destino. El bytecode contiene llamadas directas sin vtables,
  witnesses ni type packs runtime; source traits, defaults y bounds genéricos
  tienen regresiones que ejecutan en la VM.
- `for` distingue protocolo intrínseco y `Iterator[T]` de usuario. El segundo
  evalúa la fuente una vez, llama estáticamente a `next`, ramifica sobre `T?` y
  nunca usa el terminador intrínseco; BORROW-001 representa ya su receptor como
  un loan `mut` call-local verificado.
- `impl Bound` sólo se admite como éxito superior de funciones libres,
  inherentes y asociadas. El parser recupera las posiciones prohibidas con
  `E0004` sin fabricar un tipo opaco ni perder progreso.
- Cada declaración publica una familia nominal estable formada por su identidad
  y argumentos genéricos invariantes. El canal `! E` permanece exterior y las
  especializaciones concretas conservan identidades opacas distintas.
- El checker infiere un único testigo exacto para todos los éxitos normales
  alcanzables. `Never` y `err` no aportan testigo; no se inventan option lifts,
  uniones ni coerciones de función, y los contenedores vacíos usan el mismo
  contexto de inferencia. Ausencia, conflicto o ciclos producen `E1117`.
- Todos los bounds publicados se demuestran contra el testigo bajo los binders
  de la declaración. Los callers sólo obtienen esa superficie; los métodos
  inherentes y la representación concreta no atraviesan la frontera pública de
  HIR ni el desensamblado.
- HIR y MIR conservan un sello `Assignability::Opaque`; bytecode lo representa
  como una coerción verificada de coste cero. La VM reenvía el valor sin wrapper,
  allocation, vtable, witness table ni type pack runtime.
- Los tres verifiers rechazan bounds duplicados o falsos, testigos genéricos,
  `Never` o cíclicos, familias duplicadas y sellos alterados. Las regresiones
  cubren resultados fallibles, familias genéricas, funciones libres,
  inherentes, asociadas y suspendibles, bounds fuente y prelude, y mutaciones en cada
  frontera.
- Un único motor calcula `Copy`, `Discard`, `Equatable`, `Key`, `Send` y
  `Share` mediante resúmenes nominales simbólicos y un punto fijo coinductivo.
  `Copy` implica `Discard`; `Key` implica `Copy`, `Equatable` y `Discard`.
- La tabla completa queda alineada con el interner HIR. Los bounds opacos sólo
  publican lo declarado, los binders genéricos sólo usan constraints visibles y
  un trait con receptor suspendible aporta y exige `Self: Send`.
- La formación de `Map`, `Set` y `Ref`, la igualdad estructural, membership,
  map lookup, política de duplicados y discard consumen la misma prueba cerrada.
  Las regresiones cubren genéricos, nominals recursivos y toda la matriz
  intrínseca positiva y negativa.
- El admission verifier reconstruye la tabla y vuelve a probar cada consumo;
  MIR comprueba que sus operaciones coinciden y el verifier VM deriva otra vez
  las capacidades desde el catálogo bytecode cerrado. La igualdad runtime de
  maps y sets ignora el orden de inserción.
- Las funciones libres y operaciones asociadas sin receptor producen un valor
  uniforme con firma exacta. Una función genérica se especializa explícitamente
  o desde un único contexto `fn(...)`; parámetros abiertos, ambiguos, bounds no
  satisfechos o diferencias de modo, variádico, `suspends`, `unsafe` y error se
  rechazan antes de MIR.
- Los valores asociados infieren o fijan los argumentos del owner y del método.
  Las operaciones asociadas de traits exigen `Self` explícito y prueba estática;
  los receiver methods nunca crean bound methods. Módulos y privacidad conservan
  las mismas reglas que una llamada por nombre, y las llamadas indirectas sólo
  admiten argumentos posicionales.
- El verifier HIR rechaza funciones genéricas abiertas, aridad incompleta y una
  firma especializada forjada. MIR conserva operandos estáticos o lecturas de
  valores con el mismo tipo estructural, y bytecode vuelve a verificar la
  llamada indirecta exacta.
- La monomorfización enraíza valores de función dentro de constantes y aplica
  también ahí el dispatch estático de traits. La VM ejecuta funciones libres,
  asociadas, de trait, locales, parámetros y constantes sin vtable ni type pack
  runtime.
- Cada expresión de cierre publica un tipo generado distinto. CALL-004 elige
  canónicamente `closure`, `unsafe-closure`, `suspendible-closure` o
  `unsafe-suspendible-closure`, y conserva los mismos bits en su firma estructural,
  junto con binders heredados, parámetros completos, body HIR separado y
  capturas sintácticas ordenadas por `LocalId`.
- El outcome se infiere sobre todos los caminos alcanzables y las closures
  anidadas conservan problemas de inferencia independientes. Un tipo de función
  esperado debe coincidir también en `suspends` y `unsafe`, o produce únicamente
  `E1102`; no existe conversión que añada u oculte un efecto.
- Las capturas conservan `let`/`var`, copian un snapshot owned cuando prueban
  `Copy` y, en caso contrario, mueven el binding exterior al construir el
  entorno. Los free uses de closures anidadas se propagan. Préstamos
  `ref`/`mut`/`var` y el receiver prestado producen `E1402`; parámetros
  variádicos exigen nombre y conservan elemento en la firma y `Array[T]` dentro
  del body. Las firmas suspendibles rechazan parámetros `mut`/`var` con el diagnóstico
  normativo `E1609`.
- `Copy`, `Discard`, `Send` y `Share` se derivan componente a componente desde
  las capturas sustituidas; `Equatable` y `Key` se rechazan. OWN-006 elimina la
  restricción ejecutable de capturas `Copy`; OWN-007 deriva `CallOnce` mediante
  descarte estructural o transferencia completa en todas las salidas normales.
- El admission verifier exige correspondencia uno-a-uno entre metadata y
  expresión, identidad generada versus efectos de firma, firma/body, ausencia
  de parámetros suspendibles exclusivos, tipo, mutabilidad y binding de cada captura.
  HIR vuelve a decidir Copy/Move, MIR sólo admite esa transferencia directa
  desde el local exterior exacto y bytecode vuelve a comprobar esquema,
  capacidad y disponibilidad del entorno.
- La VM construye, mueve, copia, traza y snapshottea entornos gestionados. Una
  pila de raíces temporales protege capturas compuestas o afines cuando el GC se
  dispara a mitad de una construcción, move o copia multi-captura.
- El análisis de cada body alcanzable deriva su fila exacta `Call`/`CallMut`/
  `CallOnce`: una escritura, paso mutable o `CallMut` sobre una captura impide
  `Call`, y un move impide también `CallMut`. Construir una closure anidada no
  ejecuta su body, pero sí mueve en ese punto las capturas afines que necesita;
  el código inalcanzable no contamina la fila exterior. En una closure suspendible,
  escribir el entorno impide también `CallMut`. `CallOnce` exige que cada
  captura pruebe `Discard` o abandone su slot de entorno en toda terminación
  normal, `return`, `fail` y salida fallida de `?`; los joins intersectan esa
  prueba y una reposición vuelve a armar la obligación.
- Funciones, closures concretas y callable bounds genéricos u opacos comparten
  una firma estructural exacta. Un contrato ambiguo produce `E1115`, un
  protocolo inaccesible `E1407`, y la coerción contextual a `fn(...)` exige
  `Call`, firma idéntica —incluidos efectos— y entorno `Copy + Send + Share`, o
  produce `E1108`.
- El admission verifier HIR vuelve a derivar protocolos, selección de cada call
  y erasures. MIR crea un cuerpo `MirFunctionId::Closure` con entorno oculto en
  el parámetro cero, proyecta capturas y confina el `Borrow` de cierre al callee
  inmediato; el verifier MIR repite firma, protocolo y forma de acceso. Los tres
  verifiers rechazan una firma suspendible o unsafe en la operación de llamada
  síncrona segura.
- La monomorfización crea instancias de callable para closures, incluidos bodies
  genéricos, y las carga al mismo presupuesto `T0002`. El catálogo bytecode
  contiene identidad, entorno, esquema, protocolos y body; su verifier deriva
  otra vez capacidades y protocolos, resuelve testigos opacos y rechaza
  metadata, firmas, accesos o erasures forjados. Tras sustituir binders, el
  lowering especializa `CallOnce` con el `Discard` concreto sin cambiar los
  moves decididos por el body genérico.
- La VM inserta el entorno como argumento oculto, conserva `Call`/`CallMut` por
  préstamo superficial y aplica a `CallOnce` la copia o move que ya seleccionó
  el caller. Un move toma el owner del cierre y los moves del body vacían los
  fields opcionales correspondientes. Callee y argumentos permanecen en raíces
  temporales durante toda la preparación y el GC puede ejecutarse bajo presión
  sin invalidar el entorno.
- Los cuatro tipos de closure pueden construirse, copiarse, trazarse,
  snapshottearse, descartarse y borrarse a su firma uniforme exacta. El verifier
  bytecode rechaza calls con efectos y la entrada pública de la VM rechaza un
  body suspendible o unsafe como root, de modo que CALL-004 no activa prematuramente
  el runtime de M7 ni evita la frontera de M9.
- Las regresiones públicas y unitarias ejecutan closures puras, mutables,
  `CallOnce`, borradas a `fn`, genéricas, opacas, variádicas, anidadas,
  proyectadas, fallibles y bajo presión de GC; también mutan HIR, MIR y bytecode
  para probar cada frontera defensiva. Fixtures adicionales cubren las cuatro
  identidades, mismatch de efectos, `E1609`, protocolo suspendible stateful y rechazo
  de llamadas/entries con efectos. OWN-006 añade capturas afines observadas y
  movidas, propagación anidada, metadata forjada y construcción bajo presión de
  GC; OWN-007 añade observación terminal, transferencia total frente a parcial,
  `return`, `fail`, `?`, extracción completa de newtypes y especialización
  monomorfizada; `await`/`spawn` siguen en M7 y la ejecución unsafe en M9.
- El gate acumulado pasa 480 tests, `git diff --check`, formatter check, build
  de todos los targets, Clippy con warnings denegados y Rustdoc con warnings
  denegados.

### Gate de salida de M4

- No existe lookup global abierto de métodos.
- La selección de un `impl` es única y determinista.
- Los casos de overlap, orphan rules y ciclos de constraints tienen sus
  diagnósticos normativos.
- El gate original de M4 ejecuta closures con capturas `Copy + Discard` y
  genéricos a través del bytecode normal; OWN-006 extiende esa misma vertical a
  capturas afines mediante moves verificados, sin introducir otro runtime.
- Los cuatro contratos sync/unsafe/suspendible están representados sin conversión
  de efectos; sólo la firma síncrona segura puede usar la operación de llamada M4.
- La monomorfización tiene límites controlados y no puede divergir.

---

## 10. M5 — Ownership, préstamos y gestión automática

**Objetivo:** implementar el modelo que hace a Tondo seguro y predecible sin
lifetimes escritos por el usuario.

### 10.1 Valores y disponibilidad

- [x] **OWN-001 — Derivar `Copy` y `Discard` para tipos compuestos.** HIR y
  bytecode usan el mismo contrato estructural cerrado para escalares,
  funciones, tuples, unions, options, results, nominales recursivos, genéricos,
  opacos, colecciones, references, pointers, closures y cursores intrínsecos.
  Los cursores conservan ahora un tipo interno explícito
  `cursor[own,C]`/`cursor[ref,C]`/`cursor[mut,C]`; MIR y bytecode no pueden
  confundirlo con `C`, y la VM realiza una copia lógica independiente cuando el
  contrato la permite. El cursor exclusivo es afín y solo cumple `Discard`.

- [x] **OWN-002 — Implementar moves de valores no `Copy`.** MIR selecciona
  `Copy` o `Move` con el grafo de capacidades y los bounds exactos de cada body,
  conserva la decisión al monomorfizar y la vuelve a probar defensivamente.
  Parámetros, locals, retornos, argumentos por valor, agregados, cursores own y
  callees `CallOnce` transfieren ya valores afines. Las observaciones inmediatas
  usan un `Borrow` no almacenable en vez de una copia ficticia; los argumentos
  `ref`/`mut`/`var` usan los loans explícitos de BORROW-001. OWN-003 rechaza ya
  sus usos posteriores y joins inconsistentes.

- [x] **OWN-003 — Implementar disponibilidad por flujo.** HIR conserva el span
  del primer move, emite `E1401` al reutilizar un owner no disponible y une
  ramas con disponibilidad en todos los predecesores. Bucles, `break`,
  `continue`, divergencia, patterns y scopes participan en un fixed point
  determinista. Los verificadores HIR, MIR y bytecode vuelven a probar el
  contrato; MIR/bytecode invalidan locals/slots completos tras un `Move` y
  OWN-005 conserva la granularidad de proyecciones.

- [x] **OWN-004 — Permitir reposición completa de un `var` movido.** Solo una
  asignación simple al binding directo declarado con `var` crea una nueva
  definición sin leer el valor anterior. El RHS debe completarse antes de la
  escritura; ramas y loops conservan la prueba en todos sus caminos. `let`,
  parámetros, compound assignment y campos/índices siguen exigiendo una raíz
  disponible. MIR/bytecode ya materializan el write como definición y la VM
  valida un destino directo sin leer su slot movido.

- [x] **OWN-005 — Implementar moves parciales y sus restricciones.** HIR
  conserva el modelo fuente de propietario completo, registra y vuelve a probar
  un modo uniforme `Copy`/`Observe`/`Consume` por `match`, rechaza con `E1406`
  proyecciones afines o ubicaciones prestadas no transferibles y difiere todo
  binding afín hasta que su guard haya tenido éxito. MIR y bytecode usan move
  paths tipados con joins conservadores, siblings disjuntos y reposición de
  subárboles; la VM ejecuta destructuración affine de tuples, records, enums,
  options, arrays con `..rest` y la extracción completa de newtypes.

- [x] **OWN-006 — Implementar captura de closures respetando copia o move.** La
  construcción aplica la misma prueba contextual que cualquier transferencia:
  copia una captura `Copy` y mueve la afín, invalidando el binding exterior. El
  body posee sus slots de entorno, los moves alcanzables reducen la fila a
  `CallOnce`, y HIR, MIR y bytecode rederivan disponibilidad, operandos y
  protocolos. La VM toma campos movidos y protege construcciones multi-captura
  con raíces temporales incluso bajo presión de GC.

- [x] **OWN-007 — Completar las capacidades derivadas de closures con capturas
  afines y probar las obligaciones terminales de `CallOnce`.** HIR conserva una
  unión de owners no disponibles y, en paralelo, la intersección de capturas
  transferidas sobre toda salida normal, `return`, `fail` y `?`. Una captura
  `Discard` no añade obligación; cualquier otra debe salir completamente del
  entorno, incluida la extracción `.value` de newtype, y una escritura posterior
  rearma la prueba. MIR y bytecode repiten el must-analysis sobre sus CFG
  normales. Bytecode especializa además el `Discard` concreto de closures
  genéricas y opacas antes de verificar la fila ejecutable exacta. TERM-002
  sigue además el owner receptor después de un handoff interno y rechaza su
  abandono posterior.

### 10.2 Préstamos

- [x] **BORROW-001 — Implementar préstamos `ref`, `mut` y `var` sobre MIR.**
  HIR valida permisos, reborrowing y reservas en orden, acepta temporales solo
  para `ref`, diagnostica conflictos con `E1403` y lvalues exclusivos inválidos
  con `E1407`. MIR y bytecode poseen tablas densas de loans, `ReserveLoan`,
  `Loan` y `ReleaseLoan`; sus verificadores propagan el conjunto activo exacto
  por CFG,
  exigen consumo por la llamada o liberación explícita, rechazan escapes,
  accesos solapados y permisos crecientes. La VM normaliza identidad de
  frame/slot/proyección, ejecuta lectura y escritura a través del lender,
  soporta reborrow y campos disjuntos, y limpia reservas en transfers tempranos
  y unwind. Un reemplazo raíz `mut` de forma dinámica produce `E1411`: el
  reemplazo arbitrario usa `var`, mientras las operaciones con contrato de
  extensión fija siguen disponibles.

- [x] **BORROW-002 — Calcular regiones por último uso sin lifetimes de fuente.**
  Los bindings `ref` de patrones sobre lugares fijos conservan la proyección
  fuente exacta y una región compartida inferida por último uso. HIR calcula
  liveness sensible a ramas, joins, orden de evaluación y backedges; MIR
  materializa reservas, releases en sentencias o edges y cadenas de reborrow
  sin crear referencias locales ni lifetimes de fuente. Los verificadores MIR
  y bytecode vuelven a probar identidad, contención, orden acíclico, actividad
  y cierre exacto padre-hijo, y la VM defiende la cadena activa en cada acceso.
  Los usos inalcanzables no prolongan regiones y `break`/`continue` conservan la
  liveness específica de su destino. BORROW-004 extiende estas regiones a
  elementos y restos de patrones de array, y BORROW-006 cierra los cursores
  prestados compartidos y exclusivos con regiones dinámicas y fronteras
  verificadas.

- [x] **BORROW-003 — Distinguir observación compartida, mutación de extensión
  fija y mutación estructural.** HIR clasifica cada escritura como reemplazo o
  preservación de extensión y su verificador rederiva el permiso antes de MIR.
  La asignación raíz mediante `mut` exige un tipo exterior estáticamente fijo;
  `Array`, `Map`, `Set`, tipos genéricos y opacos usan `var` para reemplazo
  arbitrario y reciben `E1411` en caso contrario. MIR, bytecode y VM permiten
  elevar una reborrow `mut` a `var` únicamente sobre un sublugar estricto,
  completo y estructuralmente reemplazable.

- [x] **BORROW-004 — Implementar disjunción estática de regiones de colección.**
  HIR representa índices, slices, elementos de patrón y restos con una región
  canónica, reconoce índices y bounds constantes no negativos, y decide
  disjunción por intervalos, congruencias de stride y posiciones de patrón. Un
  préstamo de región aislado y cualquier conjunto solo `ref` son ejecutables;
  un solapamiento incompatible inevitable produce `E1403`; una pareja
  incompatible dependiente de datos se conserva como obligación explícita para
  BORROW-005. MIR y bytecode recuperan constantes desde temporales de definición
  única y rederivan la misma prueba. `ValidatePlaces` comprueba bounds y step
  antes de reservar, y la VM vuelve a comparar las rutas normalizadas reales.

- [x] **BORROW-005 — Insertar checks runtime únicamente cuando el solapamiento
  dependa de datos.** HIR admite como completas las obligaciones dinámicas de
  reserva y acceso sin ocultar solapamientos inevitables. Un análisis MIR
  posterior a liveness adjunta IDs `against` solo a conflictos `Runtime`:
  `ValidateLoan` protege la reserva, `Index`/`Slice` su lectura atómica y
  `ValidatePlaces` la lectura o escritura posterior. Los verificadores de MIR y
  bytecode rederivan el conjunto exacto, exigen consumo inmediato del testigo y
  estabilidad de índices/bounds. La VM normaliza índices negativos, slices y
  claves, eleva `P0004` únicamente si las rutas reales intersectan, preserva
  bounds/step y limpia reservas por unwind antes de entrar en el callee.

- [x] **BORROW-006 — Rechazar préstamos que crucen suspensión o fronteras no
  permitidas.** `for ref` acepta lugares estables `Array`, `Map` o `Set`;
  patrones con `mut` o `var` aceptan lugares estables escribibles `Array` o
  `Map`. La colección mantiene durante todo el bucle una región compartida o
  exclusiva según el cursor y cada binding prestado queda limitado a su
  iteración; los bindings por valor deben ser `Copy`. MIR congela una sola vez
  los índices que identifican una fuente anidada, representa el avance sin
  copia mediante una posición `Int` y `IteratorElement`, enlaza cursor, origen,
  región y posición de forma canónica, y libera hijos y colección en backedges,
  salidas, retorno y unwind. `IteratorNext` admite regiones compartidas y la
  cadena fuente exacta del cursor; toda región exclusiva debe pertenecer a esa
  cadena. Los verificadores rechazan loans call-local o exclusivos ajenos,
  claves mutables de map y proyecciones redirigidas. M7 reutiliza esta frontera
  en `Await`: excluye loans call-local y exclusivos, exige `Send` al estado vivo
  y conserva por separado los préstamos estructurados ligados a `Join`.

### 10.3 Recursos terminales y cleanup

- [x] **TERM-001 — Implementar el registro cerrado de tipos terminales.** HIR
  registra `Join[T, E]` como la única raíz intrínseca actual, con una operación
  consumidora `await` y una acción cerrada de teardown estructurado. Una
  derivación existencial separada de `Discard` clasifica cada tipo como
  `Absent`, `Potential` o `Present` a través de compuestos, nominales recursivos,
  genéricos, closures y cursores. El admission verifier reconstruye toda la
  tabla; el verifier bytecode la vuelve a derivar desde su catálogo independiente
  y rechaza resultados opacos que oculten un token terminal.

- [x] **TERM-002 — Rastrear obligaciones de consumo en todos los caminos
  normales.** HIR mantiene owners `live`/`reserved` para todo estado
  `Present` o `Potential`, incluidos genéricos sin `Discard`, compuestos,
  temporales observados, slots capturados, patrones, closures y cursores de
  iteración. Los handoffs se confirman solo al completar destino, llamada,
  agregado o salida; un control anterior restaura el binding y conserva el
  fallback del temporal ya construido. `match` y `for` materializan owners
  ocultos para wildcards y préstamos terminales, la iteración intrínseca propia
  desarma su cursor solo al agotarse y los slices almacenables exigen elementos
  `Copy`. Toda salida normal pendiente produce `E1404`; una escritura que
  perdería el owner anterior —incluidos captura, préstamo y `with`— produce
  `E1408`. El admission verifier reconstruye registro y dataflow antes de MIR;
  en el corpus temprano todavía no existían guards, cleanup ni fallback
  ejecutable.

- [x] **TERM-003 — Implementar `defer` LIFO y desarme al registrar guards
  terminales.** HIR asigna IDs estables a los scopes, valida acciones síncronas
  infalibles `Unit`, captura operands `Copy` y permite un único owner afín
  completo. MIR y bytecode materializan `RegisterDefer`, `RetargetCleanup`,
  `DisarmCleanup` y `DrainDefers`; sus verificadores independientes demuestran
  scopes exactos, LIFO, guard único, transiciones inmediatas, lifetimes y
  ausencia de entradas abandonadas. La VM drena en salidas normales y pánico,
  conserva la prioridad de pánico y adjunta los secundarios como suprimidos, y
  mantiene snapshots y guards como roots. La iteración own mueve elementos de
  `Array`, `Map` y `Set`,
  conserva el resto para una salida temprana y desarma el guard exactamente en
  el edge de agotamiento natural; la especialización elimina el marker de un
  genérico cuando su colección cerrada resulta no terminal. Si un guard
  genérico se cierra como `Copy`, la misma especialización lo convierte en un
  snapshot de registro y elimina únicamente sus transiciones ya vacías.

- [x] **TERM-004 — Implementar acciones de unwind cerradas para pánico,
  cancelación y teardown estructurado.** MIR y bytecode registran fallbacks en
  parámetros/capturas propietarios y en cada resultado terminal de store,
  llamada o iteración, los especializan a presencia concreta y los verifican
  independientemente. `DrainUnwind` consume el ledger unificado en LIFO durante
  pánico; la VM recorre compounds, nominales, colecciones, environments y
  cursores en orden inverso de construcción y despacha raíces directas mediante
  el registro sellado. Los retornos normales omiten el fallback después de la
  prueba TERM-002 y nunca omiten un `defer` explícito. La identidad
  `JoinTeardown` y su ruta anormal quedan cerradas; M7 aporta el estado de task,
  la suspensión y la cancelación necesarias para ejecutarla sobre un `Join`
  activo hasta consumir su child exactamente una vez.

- [x] **TERM-005 — Probar que cleanup explícito y unwind fallback nunca se
  ejecutan ambos.** MIR exige que un guard terminal `Present` o `Potential`
  sustituya exactamente un fallback; el bytecode vuelve a derivar `Present` y
  exige la misma cardinalidad. Ambos verificadores rechazan el rearmado
  inverso y toda superposición. La VM conserva el fallback durante la captura,
  valida la sustitución antes de mutar el ledger y publica el cleanup explícito
  solo al completarla. Tests de mutación y ejecución cubren retarget, agregado,
  llamada consumidora, agotamiento de iteración, salida normal, pánico y fallo
  durante el registro sin construir estado suspendible de `Join`.

### 10.4 Memoria e identidad

- [x] **GC-001 — Extender el collector bootstrap a todas las formas
  administradas.** La VM deriva un catálogo sellado desde bytecode verificado
  para strings, agregados, colecciones, nominales, sums, environments,
  cursores, `Ref` y witnesses opacos. Cada heap slot conserva su descriptor;
  allocation, copy, mutation y marking validan la misma forma. Cada función
  obtiene además un descriptor exacto de slots que reutilizan sin cambios los
  frames activos y suspendidos.

- [x] **GC-002 — Mantener roots en frames, environments, frontera host y
  estado estructurado.** Frames y cleanups publican valores vivos; scopes
  temporales protegen evaluaciones, copias, materialización y walkers hasta su
  publicación o error; environments siguen edges ordinarios del heap. El host
  intercambia snapshots sin handles; M7 añade como roots explícitos cada frame
  aparcado y cada resultado de child completado todavía no consumido.

- [x] **GC-003 — Trazar ciclos y recuperar objetos inalcanzables bajo presión.**
  Un adaptador privado de test usa el allocator, descriptors, roots y trigger de
  presión reales para construir `Ref -> Array -> Closure -> Ref`. Conserva un
  ciclo publicado durante 32 rondas, recupera ciclos independientes no
  enraizados sin invocar una colección especial y recupera también el retenido
  al retirar su último root. REF-001 conserva para sí la construcción de
  identidad pública.

- [x] **GC-004 — Recolectar antes de declarar OOM por heap y reintentar una
  vez.** Objetos y bytes usan una única puerta de capacidad con suma comprobada.
  Cada petición ejecuta como máximo una colección completa y después publica
  una sola vez o devuelve OOM. Allocation no contabiliza un objeto rechazado;
  replacement protege internamente su target, lo conserva si no cabe y no
  contabiliza el payload rechazado.

- [x] **REF-001 — Implementar `Ref[T]` con identidad estable y contenido
  trazable.** `Ref(value)` acepta un único operando posicional por valor,
  demuestra `T: Discard` y crea una celda administrada nueva. Copiar el
  resultado conserva el handle sin copiar `T`; `.value` es una proyección
  compartida, de solo lectura e inmovible. HIR, MIR y bytecode sellan forma,
  tipo, acceso y préstamos, y la celda reutiliza el descriptor y collector
  verificados.

- [x] **REF-002 — Implementar igualdad y `Key` por identidad de `Ref[T]`.**
  El comparador de valores reconoce primero el mismo handle y nunca compara el
  payload de dos celdas distintas. Map y Set reutilizan esa igualdad para
  reemplazo, lookup, deduplicación y pertenencia, incluso cuando `T` no es
  `Equatable` ni `Key`.

- [x] **VALUE-001 — Implementar inicialmente copia lógica eager para valores
  `Copy` compuestos.** Un único walker exhaustivo duplica recursivamente tuples,
  arrays, maps, sets, closures, nominales, sums, uniones, ranges y cursores own
  bajo roots temporales y conserva el descriptor original. String comparte
  storage inmutable y `Ref[T]` comparte identidad deliberadamente; ningún otro
  compuesto `Copy` comparte estado mutable. La matriz ejecutable cubre todas
  las formas administradas, payloads anidados, separación tras escritura y la
  copia independiente del estado de cursor.

- [x] **VALUE-002 — Crear tests de equivalencia que permitan sustituir copia
  eager por COW posteriormente sin cambiar observables.** Un corpus black-box
  separado fija valor, independencia tras escritura, identidad `Ref`,
  iteración, pánico y presión de GC exclusivamente mediante la observación
  pública del driver. Los mismos casos pasan con límites ordinarios y con
  umbral inicial de GC igual a uno; handles, allocations, schedule y
  representación no forman parte del oráculo.

### Gate de salida de M5

- No existe use-after-move en código aceptado.
- Ningún alias mutable ilegal llega a runtime sin un check permitido por el
  spec.
- Los recursos terminales se consumen en cada salida normal.
- Pánico y cancelación ejecutan exactamente las acciones cerradas previstas.
- Un root nunca se reclama y un ciclo sin roots se recupera bajo presión.
- `Ref[T]` preserva identidad sin exponer direcciones.

---

## 11. M6 — Colecciones, números y texto

**Objetivo:** completar el núcleo síncrono seguro y alcanzar G3.

### 11.1 Arrays

- [x] **ARRAY-001 — Implementar `Array[T]` con longitud runtime.** El tipo
  canónico contiene únicamente `T`; construcción, copia, llamada y retorno
  preservan el vector ordenado y su longitud. Los patterns observan esa forma
  mediante `Length(Array[T]) : Int`, sellado otra vez por el verificador de
  bytecode. Un fixture público cubre longitudes distintas bajo el mismo tipo.

- [x] **ARRAY-002 — Implementar indexación positiva y negativa con bounds.**
  Evaluación constante y VM comparten un único normalizador sin suma signed
  intermedia. Lecturas, escrituras, préstamos y validación de lugares aceptan
  `0..n - 1` y `-1..-n`; el resto produce `P0001`. El verificador sella
  `Array[T]` + `Int` + `T` antes de ejecutar.

- [x] **ARRAY-003 — Implementar slicing y normalización de extremos.** HIR,
  MIR y bytecode conservan start/end/step como tres operandos `Int` opcionales,
  sin convertir omisiones en sentinels. Evaluación constante y todos los
  caminos de la VM comparten `normalize_array_slice_indices`: aplica defaults
  según el signo, desplaza solo extremos negativos explícitos, recorta sin
  panic y avanza sin overflow incluso con `Int.min`. Paso cero produce
  `P0002`; el verificador rechaza bases, resultados o bounds incompatibles.

- [x] **ARRAY-004 — Implementar snapshots lógicos de slices.** Slice directo y
  materialización por un préstamo compartido usan un único
  `copy_array_snapshot`. La VM eager crea otro `Array` y copia lógicamente cada
  elemento; contenido ordinario queda separado y `Ref[T]` conserva identidad.
  MIR y bytecode rederivan `Array[T]: Copy` solo para materialización, mientras
  las proyecciones `ref`/`mut` pueden cubrir elementos afines sin crear otro
  propietario. El corpus black-box fija estos observables sin comprometer COW.

- [x] **ARRAY-005 — Implementar mutación `mut` de extensión fija y `var`
  estructural.** HIR conserva la separación introducida por BORROW-003:
  `mut Array[T]` admite índices, slices y operaciones in-place sin cambiar
  longitud; `var Array[T]` puede reemplazar el propietario completo, y ninguna
  región parcial puede obtener `var`. La VM defiende además el postcontrato
  dinámico de toda escritura raíz a través de `mut`: compara ambas longitudes
  antes de publicar, mantiene el reemplazo como root durante una posible
  materialización y deja `var` como único permiso que puede redimensionar.

- [x] **ARRAY-006 — Implementar aritmética array-array y array-escalar con
  reglas de forma exactas.** Una expectativa de peer aritmético infiere la hoja
  numérica sin fijar la forma y trata igual literales, bindings y llamadas. MIR
  y bytecode exigen `Invoke` checked si cualquiera de los operandos es array.
  Evaluación constante y VM validan toda la forma recursiva antes de calcular
  hojas; runtime reutiliza la aritmética escalar para enteros y floats,
  construye un resultado separado y solo después permite publicar una variante
  in-place.

- [x] **ARRAY-007 — Implementar concatenación y repetición mediante operaciones
  nombradas, no mediante nuevos significados de `+` o `*`.** La especificación
  fija `Array[T: Copy].concat(self, other)` y `repeat(self, count)`, incluidas
  sus formas calificadas con el mismo `self` implícitamente compartido. HIR
  conserva una única operación cerrada; MIR y bytecode la exigen como `Invoke`
  checked con receptor prestado y rederivan tipos y `Copy`. La VM preflighta
  `P0011` y `P0005`, construye un valor lógico nuevo con copia recursiva,
  preserva identidad `Ref` y mantiene todos los temporales vivos bajo GC.

### 11.2 Map, Set, Range e Iterator

- [x] **MAP-001 — Implementar `Map[K, V]` con orden observable de inserción.**
  Literales, copias e iteración conservan una secuencia explícita de entradas;
  ninguna frontera observable depende de la tabla o estrategia de búsqueda
  interna.

- [x] **MAP-002 — Implementar lookup, inserción, reemplazo y eliminación
  preservando el orden normativo.** Lookup devuelve `V?` con `V: Copy`;
  asignación indexada inserta al final o reemplaza en posición. La nueva
  operación intrínseca `Map.remove(var self, key): V?` transfiere valores sin
  `Copy`, conserva el orden restante y no modifica el map ante ausencia. HIR,
  MIR y bytecode rederivan firma, receptor `var` y origen exacto de región.

- [x] **MAP-003 — Implementar igualdad independiente del layout interno.**
  Igualdad compara pertenencia clave-valor y cardinalidad, no posición,
  capacidad, semilla ni representación; la iteración continúa observando el
  orden de inserción.

- [x] **SET-001 — Implementar `Set[K]` y pertenencia.** La construcción evalúa
  entradas de izquierda a derecha, conserva la primera posición de inserción y
  deduplica claves iguales aunque solo puedan compararse en runtime. `in`
  reutiliza la igualdad de `Key`; la igualdad de sets compara pertenencia y
  cardinalidad sin observar el orden. Duplicados constantes producen `W1011`
  sin impedir ejecución.

- [x] **RANGE-001 — Implementar ranges y sus límites de overflow.** `..` y
  `..=` conservan extremos discretos idénticos, pertenencia e iteración lazy.
  Los ranges descendentes están vacíos; un extremo inclusivo se marca agotado
  al emitirlo sin calcular sucesor. La VM cubre `Int.min/max`, `UInt64.max`,
  salta surrogates de `Char` y termina en `U+10FFFF`.

- [x] **ITER-001 — Implementar el protocolo estático `Iterator[T]` con un único
  elemento por target.** La selección conserva la dependencia funcional
  target→`T`: dos `impl Iterator[...]` unificables para el mismo tipo producen
  `E1113`. `for` evalúa una vez el cursor concreto, invoca estáticamente
  `next(mut self): T?`, ramifica sobre `none`/`some` y mueve o copia cada
  elemento conforme a sus capacidades sin borrar el tipo del cursor.

- [x] **ITER-002 — Implementar `for`, `for ref`, `for mut` y `for var` sobre las
  fuentes permitidas.** El patrón completo selecciona `cursor[own,C]`,
  `cursor[ref,C]` o `cursor[mut,C]`, mientras cada hoja conserva su modo exacto.
  `for ref` observa `Array`/`Map`/`Set`; `for mut` y `for var` escriben a través
  de `Array`/`Map` estables sin permitir `Set`, `Range`, `String`, temporales ni
  iteradores de usuario. En maps solo el valor puede ser exclusivo. `mut`
  conserva extensión y `var` reemplaza el elemento; ninguna forma altera la
  colección conducida por el cursor. HIR, MIR, bytecode y VM rederivan origen,
  permisos, regiones, posición, write-through y cleanup en cada salida.

### 11.3 Numéricos

- [x] **NUM-001 — Implementar todos los enteros y floats intrínsecos.** Los diez
  tipos canónicos conservan ancho y signo desde literals contextuales o con
  sufijo hasta HIR, MIR, bytecode y VM. `Int64`/`Float64` son aliases exactos de
  `Int`/`Float`, no representaciones duplicadas, y todos los límites se validan
  antes de construir el valor.

- [x] **NUM-002 — Implementar la tabla cerrada de conversiones.** Las 121
  parejas ordenadas entre los once escalares numéricos se clasifican como
  identidad, total o comprobada. Las parejas no numéricas se rechazan y la
  clasificación se rederiva en HIR, MIR y bytecode antes de ejecutar.

- [x] **NUM-003 — Implementar overflow, división, resto, shifts y bitwise.**
  La aritmética comprueba el ancho exacto, división/resto conservan `P0003` y
  la excepción mínimo/`-1`, los conteos inválidos conservan `P0010`, y los
  shifts válidos transforman el patrón de ancho fijo sin convertir el descarte
  de bits altos en overflow. Operadores simples y compuestos comparten lowering.

- [x] **NUM-004 — Conservar semántica IEEE sin fast-math observable.**
  `Float32` redondea como binario32 después de cada operación y `Float64` como
  binario64. Constantes y ejecución coinciden en ties-to-even, subnormales,
  infinidades, NaN y cero con signo; expresiones `a * b + c` conservan dos
  redondeos salvo una futura operación FMA explícita.

- [x] **NUM-005 — Implementar `NumericConversionError` y su clasificación
  estable.** `OutOfRange`, `NotFinite` y `NotIntegral` son discriminantes
  intrínsecos cerrados con valores y patrones nombrados, exhaustividad, lowering
  completo y tags verificados. Las conversiones comprobadas constantes
  conservan el mismo `Result` que la VM.

### 11.4 Texto

- [x] **TEXT-001 — Implementar `String` UTF-8 inmutable.** Cada valor mantiene
  UTF-8 válido por construcción, copia con semántica de valor, compara y ordena
  secuencias escalares exactas sin normalización e itera `Char` en tiempo
  lineal mediante offsets internos de byte no observables.

- [x] **TEXT-002 — Implementar longitud, indexación y slicing por escalares
  Unicode según el spec.** Longitud cuenta escalares, el índice `Int` produce
  `Char`, el slice produce `String` y ambos comparten exactamente la
  normalización negativa, clipping, extremos y pánicos de arrays sin exponer
  offsets UTF-8 ni lugares mutables.

- [x] **TEXT-003 — Implementar `Char`, escapes e interpolación mediante
  `Display`.** Los segmentos normales y multilínea se dedentan y decodifican
  una sola vez; los huecos se evalúan de izquierda a derecha y se convierten
  mediante selección estática. Escalares y `String` usan el intrinsic cerrado
  del bootstrap; los tipos de usuario llaman su `impl Display` concreto con un
  préstamo compartido que conserva temporales y valores afines. El formato
  compuesto de colecciones continúa perteneciendo a la futura core stdlib,
  como exige la separación normativa entre lenguaje y librería.

- [x] **TEXT-004 — Separar claramente texto y `Byte`; `Bytes` permanece en la
  stdlib.** `String`, `Char`, `Byte` y `Array[Byte]` conservan identidades
  distintas sin coerciones implícitas; el bytecode no introduce un descriptor
  intrínseco provisional para `Bytes`.

### 11.5 Variádicos y spread

- [x] **VARIADIC-001 — Implementar variádico homogéneo final `...T`.** Un único
  parámetro final por valor conserva `T` en la firma y expone `Array[T]`
  inmutable en el body. Funciones, métodos, closures explícitas o contextuales,
  genéricos y valores de función comparten la misma ruta; cero o más elementos
  se evalúan de izquierda a derecha, se copian o mueven individualmente y se
  materializan como un pack gestionado y enraizado.

- [x] **VARIADIC-002 — Implementar spread `...array` y materialización lógica de
  `Array[T]`.** HIR, MIR y bytecode conservan una asociación final única y el
  acceso contextual al array completo. La VM drena el snapshot Copy o el owner
  afín movido hacia el pack sin volver a copiar cada elemento. Formas
  posicionales y nombradas comparten runtime; la optimización como vista
  temporal continúa siendo opcional.

### 11.6 Optimización posterior al gate de corrección

- [x] **OPT-COW-001 — Medir el coste de copia eager con workloads reales.** Tres
  workloads Tondo source-to-VM reproducibles ejecutan 195 copias lógicas
  read-heavy de Array, Map y Set. Eager recorre 33.280 elementos superiores;
  la evidencia y el comando exacto quedan en
  `docs/measurements/m6-cow.md`.

- [x] **OPT-COW-002 — Introducir storage compartido y `is_unique` solo si el
  perfil demuestra valor.** `SharedBuffer` usa `Arc<Vec<_>>`; cada slot
  almacenado de Array, Map o Set debe ser escalar, String o Ref para compartir.
  Los wrappers compuestos continúan separados, cada write aplica
  `Arc::make_mut`, `is_unique` observa owners físicos y el límite de heap carga
  capacidad lógica completa para conservar el bound eager.

- [x] **OPT-COW-003 — Ejecutar los mismos tests observables contra copia eager y
  COW.** Las ocho pruebas `tests/runtime/value-copy/` se bajan una vez y se
  ejecutan con ambas estrategias, con GC normal y desde la primera asignación.
  Retorno o pánico, stdout, valores, identidad, iteración y write independence
  deben coincidir; contadores de representación quedan fuera del oracle.

### Gate G3 — superado

- Los ejemplos síncronos seguros de los capítulos 24.1 a 24.13 y 24.15 se
  compilan o se clasifican explícitamente si dependen de una API de stdlib aún
  provisional.
- Arrays, maps y sets conservan semántica de valor.
- El orden de `Map` es determinista.
- Los operadores numéricos y vectorizados respetan tipos, forma y orden de
  evaluación.
- El runtime recupera ciclos que atraviesen `Ref`, closures o collections.
- La suite completa del núcleo síncrono seguro pasa.

Clasificación explícita de los ejemplos integrados exigidos por este gate:

| Ejemplos | Estado G3 | Evidencia o frontera pendiente |
|---|---|---|
| 24.3, 24.5, 24.7, 24.8, 24.13 y 24.15 | Ejecutados | `tests/runtime/m6-g3-integrated-examples.to` |
| 24.12 | Núcleo ejecutado; API ilustrativa clasificada | Variádicos, spread y closures se ejecutan en `m6-variadic-001.to` y `m6-variadic-002.to`; solo `Array.append` pertenece a la futura core stdlib |
| 24.4 | API ilustrativa clasificada | Slicing y aritmética se ejecutan en los fixtures ARRAY-003/006; `Array.isEmpty`, `Array.length` como método y Display compuesto pertenecen a core stdlib |
| 24.6 | API ilustrativa clasificada | Map, orden e iteración se ejecutan en MAP-001..003; `Map.getOr` y consola pertenecen a stdlib |
| 24.1 y 24.2 | API hosted clasificada | `std.fs`, Bytes, decoders y métodos de String son stubs fijados en el manifiesto C.6, no superficie del lenguaje |
| 24.9 y 24.10 | API core/domain clasificada | `Array.append`, Deque y `run` son contratos provisionales de stdlib o del fixture |
| 24.11 | API application clasificada | `std.env.snapshot().arguments()`, parseo, carga y ejecución son contratos hosted/application aún separados |

---

## 12. M7 — Async y concurrencia estructurada

**Objetivo:** implementar una única suspensión inferida: sin modificador fuente
`async`, sin `await` sobre llamadas directas y con `suspends` publicado. Los
adapters de streams y el worker OS nativo permanecen como leaves independientes.

- [x] **ASYNC-001 — Typecheckear funciones y closures suspendibles.** HIR conserva el
  efecto en la identidad y la firma exacta, comprueba cuerpos nombrados y
  cierres concretos, deriva su protocolo de llamada y rechaza parámetros
  exclusivos con `E1609`.

- [x] **ASYNC-002 — Bajar la espera implícita de llamadas suspendibles.** Una
  llamada directa se materializa como una operación HIR `Await` sin spelling
  fuente adicional. `await call()` se rechaza con `E1611`; `spawn` es la única
  iniciación que conserva un `Join`; `@sync` y `@nosuspend` rechazan la llamada
  directa y producen `E1601`.

- [x] **ASYNC-003 — Prohibir préstamos y parámetros incompatibles a través de
  suspensión.** El análisis de liveness comprueba `Send` para cada owner vivo,
  reutiliza la frontera de loans de BORROW-006 y emite `E1607` si un préstamo
  exclusivo alcanza `await`; los préstamos estructurados de `spawn` permanecen
  activos hasta consumir su `Join`.

- [x] **ASYNC-004 — Transformar MIR suspendible en frames suspendibles.** MIR y
  bytecode poseen terminadores separados `Await`, `Spawn` y `DrainScopes`,
  además de `EnterTaskScope`; el executor aparca el vector de frames tipados de
  cada task sin recurrir al stack de Rust y lo restaura al reanudarla.

- [x] **EXEC-001 — Implementar executor cooperativo single-thread.** La VM
  mantiene una cola FIFO de tasks ejecutables y cede después de cada quantum de
  bytecode, sin crear un thread del sistema operativo por task.

- [x] **EXEC-002 — Definir wakeups idempotentes y garantía de progreso.** Cada
  task conserva un bit de cola y una única transición `Waiting -> Runnable`;
  dependencias repetidas, wakes duplicados y entradas obsoletas no duplican
  ejecución. Quedarse sin runnable antes de terminar la raíz es una violación
  defensiva del runtime.

- [x] **SCOPE-001 — Implementar `scope` como propietario estructurado.** Cada
  entrada crea estado runtime con owner y lista ordenada de hijos. El lowering
  drena exactamente el sufijo léxico abandonado antes de sus defers y la VM
  verifica owner, anidamiento y cierre único.

- [x] **SPAWN-001 — Implementar `spawn` y `Join[T, E]`.** Los argumentos se
  preparan en la task propietaria, se transfieren a un frame hijo mediante
  `CallOnce`, y el resultado inmediato es un handle afín ligado a la identidad
  runtime del hijo y de su scope.

- [x] **JOIN-001 — Tratar `Join` como obligación terminal y consumirlo mediante
  `await`.** HIR rastrea su procedencia a través de bindings, asignaciones,
  patterns y contenedores, exige consumo, cancelación, detach o transferencia
  afín explícita, y libera los préstamos de `spawn` solo cuando desaparece el
  último owner del handle. La VM impide consumo doble o desde otro scope. La
  antigua regla que prohibía que `Join` escapase fue sustituida por
  `ASYNC-JOIN-RETURN-001`; esta entrada conserva el identificador estable,
  pero su estado canónico es el contrato de ownership vigente.

- [x] **CANCEL-001 — Implementar cancelación cooperativa en los puntos
  normativos.** La petición se observa al entrar o abandonar `scope` y en
  `await`/`spawn`; viaja por un canal interno `Cancelled`, ejecuta unwind y
  nunca se inyecta en el tipo de error `E`.

- [x] **CANCEL-002 — Implementar cleanup de hijos al salir del scope.** Una
  salida no local solicita cancelación a cada hijo vivo, aparca al owner hasta
  que todos terminan, ejecuta sus defers/fallbacks y consume estructuralmente
  cualquier resultado pendiente antes de cerrar el scope.

- [x] **PANIC-ASYNC-001 — Propagar pánicos de tareas según el contrato
  estructurado.** El primer hijo que paniquea cancela hermanos y despierta al
  owner; este espera todo el cleanup y propaga un primario estable por orden de
  creación, anexando los demás como suprimidos.

- [x] **SEND-001 — Comprobar `Send` en transferencia a tasks.** HIR exige la
  capacidad en callee, argumentos propios, resultados, errores y valores vivos
  a través de suspensión; MIR y bytecode vuelven a derivar el contrato cerrado.

- [x] **SHARE-001 — Comprobar `Share` para observación concurrente.** Un
  argumento `ref` lanzado exige `Send + Share`, conserva una identidad de loan
  que puede cruzar tasks y bloquea escritura/movimiento del origen hasta
  consumir el `Join`.

- [x] **MAIN-ASYNC-001 — Implementar `main` suspendible por inferencia y scope raíz.** El driver
  admite una entrada segura con el mismo outcome lógico que `main`
  síncrono; la task raíz pertenece al executor, pero no crea un scope léxico
  implícito para autorizar `spawn` detached. Evidencia: script raíz con
  `async.oneshot` y espera directa.

- [x] **CONC-TEST-001 — Crear litmus tests con resultados permitidos y
  prohibidos, no con scheduling esperado.** El corpus cubre ejecución
  secuencial y concurrente por propiedades finales: progreso, wake idempotente,
  no escape de `Join`, préstamos liberados tras `await`, cancelación con
  cleanup, pánicos de hermanos y roots vivos bajo GC, sin fijar una traza
  concreta del scheduler.

- [x] **ASYNC-INFER-001 — Migrar el frontend al efecto suspendible inferido.**
  La ruta pública propaga la capacidad desde llamadas `suspends`, `await`,
  `spawn`, scopes, iteración asíncrona y cleanup; la compatibilidad del token
  `async` se elimina: léxicamente es un identificador ordinario y `async fn` no
  forma parte de la gramática. Las llamadas directas bajan a espera implícita,
  las interfaces publican `suspends` y no se generan wrappers `Task`/`Future`.
  Evidencia: tests de inferencia transitiva, `AsyncIterator`, script raíz y
  one-shot.

- [x] **ASYNC-IMPLICIT-WAIT-001 — Unificar el lowering de espera directa.**
  Resolver primero la firma pública `suspends`, insertar `Await` para una
  llamada ordinaria y reservar el operador fuente `await` para `Join[T, E]`.
  Probar `E1611`, evaluación izquierda-a-derecha, liveness, `unsafe`,
  `@nosuspend` y ausencia de doble espera. Evidencia: llamadas directas a
  `Waiter.wait`, `AsyncIterator.next`, script raíz y cleanup de `defer`
  comparten el lowering implícito verificado.

- [x] **ASYNC-EFFECT-API-001 — Publicar el efecto `suspends`.** Generar el
  marcador después del outcome en interfaces, hashes ABI, diagnósticos e IDE;
  cargarlo antes de typecheckear clientes y rechazar cualquier drift de efecto.
  No introducir APIs duplicadas. `canonical_interface` y el hash de interfaz
  usan la forma estable `fn(...): T suspends`.

- [x] **ASYNC-SUSPENDS-DENOTE-001 — Hacer denotable y comprobable el efecto.**
  Reservar `suspends`, aceptarlo después del outcome en funciones, métodos de
  trait/impl y tipos `fn`, exigirlo en contratos bodyless suspendibles y
  permitir que un cuerpo lo infiera o lo fije como promesa. Las implementaciones
  de trait conservan el efecto exactamente; `@sync`/`@nosuspend` entra en
  conflicto con el marcador. La interfaz pública siempre lo muestra. Las
  llamadas secuenciales admiten préstamos `mut`/`var` `Send` hasta completar la
  espera; `spawn` y `spawn thread` los rechazan con `E1609`. Lexer, parser, HIR,
  verificador y tests cubren firmas, closures contextuales, loans, drift y
  ausencia de contaminación entre métodos homónimos. Solo una llamada
  `suspends` es candidata a `spawn`; scope, `CallOnce`, `Send` y `Share` siguen
  validándose en el call site.

- [x] **ASYNC-JOIN-RETURN-001 — Hacer transferible `Join` fuera del scope.**
  Sustituir la regla descartada `join-escapes` por ownership afín: `await`,
  `cancel`, `detach` o `return`/move son las únicas terminaciones; validar
  teardown, cancelación y unwind cuando el handle se transfiere al caller.
  Evidencia: retorno directo desde un scope, consumo por el caller y
  validación de teardown/cancelación.

- [x] **ASYNC-THREAD-SPAWN-001 — Unificar task y thread bajo `spawn`.**
  Implementar `spawn thread call()` con el mismo `Join[T, E]`, capacidades
  `Send`, cleanup y diagnostics; retirar la API pública paralela `Thread.start`.
  La VM bootstrap conserva una única cola cooperativa y propaga la lane
  `Thread` hasta el terminador; el worker OS real pertenece al backend nativo y
  queda trazado como `NATIVE-THREAD-001`, sin duplicar el contrato fuente.

- [x] **ASYNC-ONESHOT-001 — Implementar completion one-shot.**
  Añadir `Waiter`/`Completer`, finalización atómica, `AlreadyCompleted`,
  cancelación y pruebas de carreras sin callbacks ni scheduler adicional.
  Evidencia: finalización normal/fallida, segunda finalización, cancelación,
  wake de waiter pendiente y spawn sobre waiter.

- [x] **ASYNC-ITER-001 — Implementar `AsyncIterator` y la iteración implícita.**
  Añadir el protocolo estático, hacer que `for` espere cada `next` cuando no
  exista `Iterator[T]`, rechazar `for await` y demostrar que no se crea un array
  intermedio. Si existen ambos protocolos, `Iterator[T]` tiene precedencia.
  `collect(limit:)` y las reglas
  genéricas de cierre/backpressure quedan separadas en `ASYNC-ITER-EXT-001`;
  la adaptación concreta de `std.channel.Receiver[T]` bajo `T: Discard`
  pertenece a STD-0.1B y queda cerrado en `STD-CHANNEL-ASYNC-ITER-001`, con la
  frontera hosted/AOT registrada en su contrato específico.

- [x] **ASYNC-ITER-EXT-001 — Completar streams genéricos en `std.async`.**
  `AsyncIterator.collect(limit:)` queda implementado como lowering MIR genérico
  sobre el único `next` del protocolo: reserva capacidad una vez, respeta
  `limit == 0`, rechaza límites negativos mediante `CollectionError`, no pide
  un `next` adicional al alcanzar el límite y propaga errores sin publicar un
  array parcial. El estado del cursor permanece bajo ownership estructurado y
  se drena por los caminos normal, error, cancelación y unwind; no se añade
  `Channel`, executor ni una segunda abstracción de stream. La cobertura
  ejecutable vive en `tests/runtime/m11-std-async-iter-001.to`, el driver y
  `scripts/stdlib-async-test.sh`; el ABI de `AsyncIterator[T]` no cambia.

- [x] **ASYNC-SELECT-FRONTEND-001 — Implementar la sintaxis canónica de
  `select`.** Reservar `select`/`selectable`, añadir CST/AST lossless, gramática
  de brazos `let pattern = operation => body`, `else` único y final, recovery,
  formatter estable y doc-test pseudocode. Eliminar cualquier camino o fixture
  que modele selección mediante `async.select`, builders o valores `Case`; no
  conservar compatibilidad del borrador descartado. Evidencia: el lexer reserva
  ambos keywords; el parser conserva `SelectExpr`, `SelectArm` y
  `SelectElseArm` en el CST/AST lossless, recupera brazos mal formados y no
  crea ninguna ruta de selección basada en `async.select`; el formatter emite
  brazos canónicos por línea. Los
  tests unitarios cubren la forma con binding, `await Join`, control-transfer,
  `selectable` en firmas/cierres, recovery, reparseo e idempotencia de tokens.

- [x] **ASYNC-SELECT-SEMA-001 — Tipar `selectable` y los brazos.** `FunctionType`
  conserva la capacidad fuerte en tipos, sustitución, equivalencia, interfaces
  canónicas y hash ABI; traits e implementaciones exigen el mismo contrato y
  los cierres pueden adoptar el efecto contextual. La única relación adicional
  es el debilitamiento cerrado `selectable` → `suspends` (`EffectWeakening`),
  sin wrapper ni conversión inversa y sin inferencia transitiva de la capacidad.
  El checker valida brazos directos `selectable`, `await Join`, propagación `?`
  después de preparar la operación, bindings irrefutables, unificación de
  cuerpos y `@sync`/`@nosuspend`; el contexto del brazo suprime la espera
  implícita y da prioridad a `E1612`, mientras `E1613` cubre patrones/forma y
  `E1614` contratos incompatibles. El parser conserva marcadores duplicados
  para que `suspends selectable` produzca `E1614` y no un error sintáctico.
  Evidencia: tests `select_semantics_*`, `selectable_*`, el test de tipos de
  debilitamiento, contratos de trait y el gate focalizado de `tondo-compiler`.
  La expresión aún queda como recovery marcada incompleta hasta
  `ASYNC-SELECT-LOWER-001`; este bloque no afirma ejecución HIR/MIR/VM.

- [x] **ASYNC-SELECT-LOWER-001 — Bajar selección a HIR/MIR y bytecode
  verificable.** Representar prepare/register/commit y teardown de rollback
  en el estado runtime, registro
  izquierda-a-derecha, un único ganador, `else` como snapshot no bloqueante y
  cleanup explícito de perdedores. El verificador debe rechazar bytecode que
  salte fases, comprometa más de un brazo, observe el resultado antes de commit
  o publique una tabla de brazos sin límites comprobados. Evidencia: HIR
  ejecutable `Select` con operaciones por brazo, wrappers `?` reproducidos en
  el cuerpo ganador; MIR y bytecode con protocolo de fases
  `BeginSelect → RegisterSelectArm×N → CommitSelect` (capacidad ≤64 verificada)
  y análisis de flujo dedicado en ambos verificadores que rechaza regiones
  reentradas, registros fuera de orden, instrucciones interleaved antes del
  commit, tablas que no coinciden con los registros y commits huérfanos; tests
  positivos de desensamblado y negativos de bytecode forjado en
  `bytecode/lower.rs`. El selector cooperativo permanece en
  `ASYNC-SELECT-RUNTIME-001`: la VM ya consume ese protocolo; no se atribuye
  aquí la ejecución cooperativa ni la política de ownership.

- [x] **ASYNC-SELECT-RUNTIME-001 — Implementar el selector cooperativo.** La
  VM mantiene un registro por brazo, espera sin bloquear en `TaskWait::Select`,
  despierta de forma idempotente ante completions simultáneos, arbitra con
  rotación y compromete un único ganador. `else` es snapshot no bloqueante;
  resultados normales, errores, pánicos y cancelación pasan por el mismo
  cleanup. Los brazos `Call`, host async, one-shot, `AsyncIterator.collect` y
  `Join` se integran con el scheduler existente, sin scheduler paralelo.

- [x] **ASYNC-SELECT-OWN-001 — Hacer branch-sensitive el ownership de
  selección.** El análisis de disponibilidad conserva cada `Join` durante el
  registro y lo transfiere únicamente al cuerpo ganador; los cuerpos de
  perdedores y `else` lo mantienen disponible. La VM consume solo el owner
  ganador, cancela/descarte las tareas que pertenecen al selector y deja los
  `Join` perdedores awaitables. Se validan además los tipos de registration y
  payload en MIR/bytecode y se conserva la afinidad en pánico/cancelación.

- [x] **ASYNC-SELECT-TEST-001 — Modelar, probar y fuzzear el núcleo de
  selección.** `SelectModel` en `crates/tondo-reliability/tests/models.rs`
  recorre 4.096 seeds deterministas y comprueba exactamente un commit,
  rotación sin starvation, wake idempotente, cancelación/cleanup único y
  owners perdedores. Los tests HIR/MIR/bytecode cubren los negativos
  `E1612`–`E1615`, límites de forma y payload; los tests VM cubren 0/1/N,
  ready/pending, `else`, pánico/cancelación, `Join` heterogéneo y one-shot.
  La fixture ejecutable `tests/runtime/m11-std-async-selectable-001.to` cubre
  `Waiter.wait` y `Timer.wait` en el camino público, sin fijar un scheduling
  concreto.

- [x] **ASYNC-SELECT-PERF-001 — Fijar presupuestos del selector VM.**
  `scripts/async-select-performance.sh` ejecuta el probe real
  `runtime::execute::tests::select_performance_probe` en tres procesos
  independientes, con tres warmups y nueve mediciones por proceso (27 muestras
  por workload). Registra latencia ready/pending, throughput, allocations
  gestionadas, bytes de frame, registros, scans, wakeups y p95/p99 para
  1/2/8/64 brazos, además de la comparación `direct-ready-1`. Los contadores
  del VM demuestran cero allocations gestionadas por operación, frame lineal
  en el número de brazos, dos pasadas exactas en pending y un wakeup por
  operación pendiente. El informe reproducible se escribe en
  `target/reliability/evidence/async-select-performance.json`; el contrato y
  sus negativos están en `testing/async-select-performance.json` y
  `scripts/async-select-performance-test.sh`.

- [x] **ASYNC-SELECT-VM-CONF-001 — Conformar `select` sobre la VM.** El corpus
  completo 0.1 se ejecuta por parser → formatter → interfaz → bytecode
  verificado → VM. La capa `conformance/draft/layers/async-select.json` enlaza
  parser/HIR/lowering/VM con los requisitos de selección; el contrato
  `testing/async-select-conformance.json` y
  `scripts/async-select-conformance.sh` ratchetean 206/206 casos, los tres
  casos públicos de selección y 32 observaciones exactas idénticas por caso.
  El informe reproducible vive en
  `target/reliability/evidence/async-select-conformance.json`. Esto cierra la
  conformidad hosted de la VM; la paridad nativa de selección runtime queda
  cerrada por `NATIVE-SELECT-001`.

- [x] **NATIVE-THREAD-001 — Mapear la lane `Thread` a workers OS en el backend
  nativo.** La VM bootstrap conserva semántica cooperativa determinista; el
  runtime nativo lanza un worker OS seguro por `spawn thread call()` y mantiene
  el mismo `Join`/`Send`/cancelación/cleanup. `Join`, `await` y el `select-take`
  ganador cruzan una barrera de finalización antes de consumir el valor; la
  identidad observable es lógica y path-free, nunca un ID físico. El runner
  diferencial usa `pthread_create`/`pthread_join` en ambos candidatos y prueba
  estado, ejecución única, worker distinto, join, cancelación y selección.
  La lane física de threads sigue entregando un valor eager al handoff; el
  coordinador `NATIVE-002` cubre por separado el cuerpo diferido mínimo de
  llamadas directas a tasks. La coordinación completa del scheduler queda
  pendiente.
  Evidencia: `testing/native-thread.json`,
  `docs/contracts/native-thread.md`, `scripts/native-thread-{check,test}.sh`,
  `tondo-native-runtime` y el campo `native_thread_runs` de
  `target/reliability/evidence/native-evaluation-runner.json`.

### Gate de salida de M7

- [x] Ningún hijo sobrevive a su scope bajo las reglas de transferencia de 1.67.
- [x] Todo `Join` se consume, se transfiere, se cancela o se detached de forma
  explícita.
- [x] Una llamada directa suspendible espera exactamente una vez;
  `await call()` se rechaza con `E1611` y `@sync`/`@nosuspend` rechaza la forma
  directa con `E1601`.
- [x] Interfaces y clientes usan el marcador `suspends` con hashes ABI
  coherentes y el cambio de efecto se diagnostica como drift.
- [x] Cancelación no aparece como variante implícita de `E`.
- [x] El executor de un hilo satisface progreso cooperativo.
- [x] El código no depende del orden concreto de scheduling.
- [x] Los roots de frames suspendidos permanecen vivos tras la migración de
  efectos inferidos.
- [x] `selectable` forma parte de tipos/interfaz/ABI y una llamada ordinaria
  conserva la espera implícita sin duplicar API.
- [x] `select` compromete exactamente un brazo y conserva owners perdedores;
  modelo/tests, adapters, presupuesto de rendimiento y conformidad VM
  promocionada están cerrados para el target hosted. La slice nativa de
  selección runtime está cerrada por `NATIVE-SELECT-001`; el adaptador común
  de MIR está cerrado por `NATIVE-BACKEND-ADAPTER-001` y las capacidades de
  identidad/source maps están cerradas en `NATIVE-LOWER-DEBUG-001`.

---

## 13. M8 — Scripts, comandos y procesos

**Objetivo:** hacer de Tondo un lenguaje cómodo para scripting sin introducir
shell implícito ni efectos de importación.

### 13.1 Script raíz

- [x] **SCRIPT-001 — Implementar sentencias top-level solo en el archivo raíz
  del modo script.**

- [x] **SCRIPT-002 — Construir un `main` privado implícito.**

- [x] **SCRIPT-003 — Inferir localmente la unión cerrada de errores del script.**

- [x] **SCRIPT-004 — Inferir suspensión del `main` implícito.** Una llamada
  suspendible directa, `await` explícito, iteración `AsyncIterator` o `scope`
  top-level hace suspendible el `main` implícito sin escribir `async` en la
  firma.

- [x] **SCRIPT-005 — Prohibir importar un script y mezclarlo con `main`
  explícito.**

- [x] **SCRIPT-006 — Implementar shebang sin convertirlo en sintaxis de módulo.**

### 13.2 Command y Pipeline

- [x] **PROC-001 — Implementar `Command` y `Pipeline` como planes inertes
  `Copy + Send + Share`.**

- [x] **PROC-002 — Implementar únicamente las cuatro combinaciones cerradas de
  `|`.**

- [x] **PROC-003 — Garantizar que construir un plan no inicia procesos.**

- [x] **PROC-004 — Definir en la stdlib las operaciones terminales `start`,
  `status`, `output`, `run` y `check` antes de implementarlas públicamente.**

- [x] **PROC-005 — Pasar programa y argumentos sin parsing de shell.**

- [x] **PROC-006 — Ofrecer shell solo mediante una API nombrada y explícita.**

- [x] **PROC-007 — Modelar handles, streams y ownership one-shot como recursos
  terminales.**

- [x] **PROC-008 — Integrar cancelación y cleanup con el scope raíz.**

- [x] **PROC-009 — Traducir exit status y errores de spawn a tipos nominales de
  stdlib.**

- [x] **PROC-010 — Rechazar la API antes de ejecutar cuando el target no
  anuncie capacidad `process`.**

### Gate de salida de M8

- [x] El ejemplo 24.17 funciona sin invocar un shell implícito.
- [x] Un import nunca ejecuta código.
- [x] No quedan procesos huérfanos al terminar, cancelar o panicar un scope.
- [x] Los argumentos conservan exactamente sus caracteres.
- [x] Los pipes aplican backpressure y no bloquean el executor cooperativo.

---

## 14. M9 — Unsafe, targets, interfaces y toolchain

**Objetivo:** completar la superficie 0.1 y alcanzar G4 sin prometer una ABI que
el lenguaje excluye.

### 14.1 Unsafe y Pointer

- [x] **UNSAFE-001 — Implementar funciones, closures y bloques `unsafe`.**

- [x] **UNSAFE-002 — Permitir operaciones de `Pointer[T]` únicamente dentro de
  una frontera unsafe válida.**

- [x] **UNSAFE-003 — Comprobar estáticamente toda precondición comprobable.**

- [x] **UNSAFE-004 — Documentar la lista cerrada de comportamiento indefinido
  que puede introducir una operación raw.**

- [x] **UNSAFE-005 — Impedir que código seguro observe direcciones como
  identidad ordinaria.**

- [x] **FFI-001 — Diseñar unidades privilegiadas y wrappers nativos sin añadir
  atributos semánticos generales a `.to`.**

### 14.2 Targets y capacidades

- [x] **TARGET-001 — Implementar edición, target, perfil y capacidades como
  inputs explícitos.**

- [x] **TARGET-002 — Resolver source sets antes de lexear.**

- [x] **TARGET-003 — Rechazar imports o APIs ausentes para el target.**

- [x] **TARGET-004 — Registrar target, perfil, capacidades, features y source
  sets en artefactos e interfaces.**

### 14.3 Paquetes e interfaces

- [x] **PKG-001 — Escribir la especificación separada del manifiesto y
  lockfile.**

- [x] **PKG-002 — Implementar resolución cerrada y offline durante
  compilación.**

- [x] **PKG-003 — Fijar aliases locales y PackageIds transitivos exactos.**

- [x] **IFACE-001 — Definir el formato versionado de interfaces compiladas.**

- [x] **IFACE-002 — Incluir hash de API, edición, target y dependencias.**

- [x] **IFACE-003 — Rechazar interfaces incompatibles antes del type checking
  consumidor.**

- [x] **BUILD-001 — Verificar builds deterministas bajo entradas idénticas.**

- [x] **BUILD-002 — Verificar que la compilación no consulta red, reloj ni
  entorno no declarados.**

### Gate G4

- [x] Toda sintaxis y semántica de fuente 0.1 tiene una ruta implementada.
- [x] El target VM `hosted` declara exactamente sus capacidades.
- [x] Las capacidades ausentes fallan en compilación.
- [x] Código seguro permanece libre de UB.
- [x] Las interfaces incompatibles no se enlazan por parecido nominal.
- [x] Los ejemplos integrados del spec se compilan con sus fixtures o stdlib
  correspondiente.

Evidencia de cierre:

- Los cuatro efectos de callable, las regiones léxicas `unsafe`, las seis
  operaciones raw de `Pointer[T]` y los diagnósticos `E1701`/`E1702` atraviesan
  HIR, MIR y bytecode con verificación independiente. `Pointer[T]` continúa sin
  `Equatable`, `Key`, `Send` ni `Share`; observar o reconstruir una dirección
  exige una operación nombrada dentro de `unsafe`.
- [`TONDO_TOOLCHAIN_SPEC.md`](./TONDO_TOOLCHAIN_SPEC.md) fija los formatos
  estrictos de manifiesto, lockfile, interfaz, artefacto y unidad privilegiada.
  El plan puro selecciona source sets antes del lexer, acepta únicamente bytes
  declarados con su SHA-256 y no posee una superficie de I/O ambiental.
- Interfaces y artefactos usan identidades canónicas sin colisiones ambiguas,
  fijan compilador, edición, PackageIds, target, perfil, capacidades, features,
  módulos, source sets y dependencias transitivas. El artefacto vuelve a derivar
  su propio `build_hash` al decodificarse y rechaza cualquier manipulación.
- La CLI del corpus vivo carga exactamente el plan cerrado, todavía no ejecuta
  generadores ni busca
  dependencias, emite productos solo tras éxito y evita que estos sobrescriban
  inputs o se solapen entre sí, incluidos aliases de path.
- La frontera nativa 0.1 termina deliberadamente en unidades privilegiadas
  fijadas por hash. No se inventan layout, calling convention ni ABI general;
  un adaptador dinámico futuro deberá aportar y fijar ese contrato.

---

## 15. M10 — Corpus ejecutable vivo

**Objetivo:** convertir la afirmación “implementamos Tondo” en evidencia
derivada y reproducible sobre el árbol actual.

### 15.1 Construcción de `tondo-conformance-draft`

- [x] **CONF-001 — Crear un manifiesto versionado y machine-readable de casos.**

- [x] **CONF-002 — Extraer y clasificar fences normativos del spec.**

- [x] **CONF-003 — Implementar fixtures del apéndice C sin exponerlos a
  programas normales.**

- [x] **CONF-004 — Crear grupo de lexing, parsing y formato.**

- [x] **CONF-005 — Crear grupos compile-pass y compile-fail.**

- [x] **CONF-006 — Crear grupo de consultas semánticas y fixes JSON.**

- [x] **CONF-007 — Crear grupo runtime.**

- [x] **CONF-008 — Crear grupo de concurrencia.**

- [x] **CONF-009 — Crear grupo `hosted`.**

- [x] **CONF-010 — Crear adaptador privado de memoria.** Debe probar roots,
  ciclos, presión y reintento previo a OOM usando el collector real.

### 15.2 Cobertura

- [x] **DIAG-001 — Tener al menos un caso primario para cada código `E`.**

- [x] **DIAG-002 — Tener casos positivos que demuestren que cada check no
  rechaza programas vecinos válidos.**

- [x] **WARN-001 — Cubrir el perfil de warnings `core`.**

- [x] **PANIC-001 — Cubrir cada clase normativa `P`.**

- [x] **FMT-CONF-001 — Validar resultados byte a byte e idempotencia.**

- [x] **QUERY-CONF-001 — Validar schema, IDs, orden, spans, related y fixes.**

- [x] **DETERMINISM-001 — Repetir builds con orden físico de archivos
  perturbado.**

- [x] **MEM-CONF-001 — Probar reachability y ciclos bajo presión.**

- [x] **CONC-CONF-001 — Repetir litmus tests con límites calibrados.**

### 15.3 Corpus vivo reproducible

- [x] **REL-001 — Registrar matriz exacta de target, perfil y capacidades.**

- [x] **REL-002 — Fijar la identidad del compilador, formatter, edición y
  manifest del draft.**

- [x] **REL-003 — Registrar resultados reproducibles de conformidad.**

- [x] **REL-004 — Documentar limitaciones que no contradigan capacidades
  anunciadas.**

- [x] **REL-005 — Verificar que no existe modo oculto que relaje checks.**

- [x] **REL-006 — Congelar el formato público de diagnostics JSON 0.1.**

- [x] **REL-007 — Regenerar el corpus vivo únicamente después de superar todos
  los grupos aplicables de su manifest actual.**

### Gate M10 del corpus vivo pre-M10.7

- [x] La identidad exacta del toolchain pasa `tondo-conformance-draft`.
- [x] El target y sus capacidades están declarados.
- [x] No hay exclusiones sin justificar por capacidad.

Este gate valida el corpus que vive en el árbol actual. La ampliación M10.7
mantiene abierto el Gate G5 de la primera versión publicable, definido en 18.4.
- [x] Los artefactos, resultados y versiones pueden reproducirse.
- [x] La documentación no afirma soporte más amplio que la evidencia.

La evidencia se produce de nuevo durante el gate y no se conserva como un
resultado versionado dentro del repositorio. El manifest vivo y los reports
efímeros registran identidad, target, perfil, capabilities, casos y
observaciones del árbol que se está validando.

---

## 16. M10.5 — Reliability y testing

**Objetivo:** instalar una infraestructura de evidencia continua antes de
ampliar la API pública o duplicar la ejecución en un backend nativo. Este
milestone no cambia la semántica del corpus vivo: clasifica la
cobertura actual, automatiza el gate existente y crea las herramientas con las
que cada milestone posterior multiplicará casos reproducibles.

**Límite:** M10.5 no se cierra por alcanzar una cifra arbitraria de tests. Se
cierra cuando inventario, trazabilidad, CI, generación, fuzzing, modelos y
métricas tienen contratos ejecutables. La expansión del corpus continúa dentro
de M10.6, STD-0.1A, M11 y STD-0.1B.

### 16.1 Baseline y trazabilidad normativa

- [x] **TEST-AUDIT-001 — Auditar el corpus 0.1 existente.** El inventario
  derivado distingue cantidad física, caso lógico, repetición y fuente única;
  sus cifras se regeneran y no se duplican como una fotografía histórica en
  este tracker.

- [x] **TEST-001 — Materializar un inventario machine-readable.** Añadir una
  herramienta reproducible que enumere por crate, fase, fixture, grupo,
  requisito, oracle, repetición, hash de fuente y target. Debe detectar IDs
  duplicados, sidecars huérfanos, casos no descubiertos y deriva entre el
  manifiesto y el repositorio. También registra documento, edición y estado:
  los ejemplos de `TONDO_TESTING_SPEC.md` se registran como contrato 0.1
  pendiente, pero no cuentan como tests ejecutables ni cobertura del corpus.

- [x] **TEST-002 — Crear la matriz normativa de cobertura.** Cada requisito
  `debe`/`no puede` de Tondo 0.1 recibe una identidad estable. La matriz conserva
  revisión, heading anchor y hash del texto fuente, y lo clasifica como
  `covered`, `target-not-applicable`, `stdlib-pending` o `toolchain-limit`,
  siempre con evidencia enlazada. Una sección o fence no cuenta por sí mismo
  como cobertura semántica.

- [x] **TEST-003 — Exigir dimensiones de prueba explícitas.** Para cada regla
  aplicable, la matriz registra caso positivo, rechazo o fallo cuando exista,
  límites materiales, composición con otras reglas, fase que actúa como oracle
  y frontera pública observada. Las excepciones requieren una justificación
  versionada, no una celda vacía.

- [x] **TEST-004 — Cerrar primero los huecos críticos descubiertos.** Priorizar
  lexer/parser/formatter, resolución, tipos, ownership, HIR/MIR/bytecode
  verifiers, GC, scheduler, procesos y protocolos no confiables. Cada hueco se
  reduce a la fuente o estructura mínima que habría permitido el defecto.

### 16.2 Gate continuo de CI

- [x] **CI-TEST-001 — Ejecutar el gate estricto en cada cambio.** Un workflow
  de PR y `main` debe ejecutar formatter check, `cargo check` de todos los
  targets, Clippy con warnings denegados, los tests completos, Rustdoc, build
  locked de runner/adaptador, validación del manifiesto y una ejecución de
  conformidad cuyo resultado se compare con la evidencia versionada.

- [x] **CI-TEST-002 — Separar gate determinista y campañas sin rebajar el
  oracle.** PR y `main` ejecutan el mismo gate obligatorio; el tier nocturno
  añade stress, fuzzing y matrices costosas. Clasificar un caso como campaña no
  puede retirar su regresión determinista del gate ni convertir un fallo en
  warning.

- [x] **CI-TEST-003 — Definir la matriz multiplataforma de validación.** Linux
  ejecuta el gate canónico; Linux ARM64, macOS Intel/ARM64 y Windows ejecutan
  tests de plataforma y la parte portable aplicable, además del smoke test de
  los binarios. Toda exclusión se justifica por target o capability.

- [x] **CI-TEST-004 — Conservar evidencia de fallos reproducibles.** Seeds,
  corpus minimizado, observaciones, logs relevantes y metadatos de target se
  publican como artefactos sin paths físicos, secretos ni estado ambiental
  accidental.

### 16.3 Properties, metamorfismo y fuzzing

- [x] **PROP-001 — Crear generadores reproducibles y reducibles.** Sustituir
  corpora generados con una única seed fija por generadores que registren la
  seed, puedan reducir el caso fallido y produzcan sintaxis válida, sintaxis
  recuperable y estructuras inválidas controladas bajo presupuestos.

- [x] **PROP-002 — Generar programas tipados por construcción.** Cubrir
  combinaciones de tipos, operadores, genéricos, traits, patterns, ownership,
  préstamos, suspensión y errores sin depender de que el frontend acepte
  ruido aleatorio como programa válido.

- [x] **META-001 — Añadir properties metamórficas.** Como mínimo: reconstrucción
  CST, idempotencia de formato, alpha-renaming, permutación física de fuentes,
  paréntesis semánticamente neutros, eager frente a COW, presión de GC y
  estabilidad de diagnostics y productos canónicos.

- [x] **FUZZ-001 — Mantener fuzz targets del frontend.** Lexer, parser y
  formatter deben aceptar bytes no confiables sin panic, no terminación ni
  pérdida de partición; los casos válidos conservan parseo e idempotencia.

- [x] **FUZZ-002 — Mantener fuzz targets de protocolos.** Manifiesto, lockfile,
  interfaz, artefacto, diagnostics JSON y protocolo del adaptador se decodifican
  bajo límites y nunca consultan entradas ambientales. Todo round-trip canónico
  debe ser estable.

- [x] **FUZZ-003 — Fuzzear los admission verifiers.** Programas tipados y
  plantillas estructuradas atraviesan HIR y MIR mediante el driver público; el
  mutador estructural de bytecode explora tags, índices, tipos y límites contra
  el verifier directo. Los tests internos conservan la cobertura exhaustiva de
  CFG, ownership y cleanup sin exponer constructores inválidos como API ni
  introducir un formato bytecode estable en disco.

- [x] **FUZZ-004 — Integrar corpus y campañas.** Cada PR ejecuta smoke fuzzing
  determinista; el tier nocturno amplía tiempo y seeds; todo crash se minimiza,
  se convierte en regresión y entra en el corpus antes de cerrar el defecto.

### 16.4 Modelos, cobertura y resistencia de los tests

- [x] **MODEL-001 — Modelar valores y colecciones.** Secuencias de operaciones
  sobre `Array`, `Map`, `Set`, `Range`, `String`, slices y copias se comparan
  con modelos puros, incluidos orden, aliasing explícito, errores y límites.

- [x] **MODEL-002 — Modelar ownership y concurrencia estructurada.** Un modelo
  de estados cubre moves, préstamos, terminales, `defer`, `Join`, cancelación,
  pánico y cleanup. El generador explora transiciones válidas e inválidas y
  verifica la fase exacta que debe rechazarlas.

- [x] **MODEL-003 — Modelar runtime y host.** GC, ciclos, roots, OOM retry,
  scheduling, pipes y procesos se prueban con umbrales y órdenes perturbados,
  sin convertir contadores privados en semántica observable.

- [x] **COV-001 — Publicar una baseline de cobertura por riesgo.** Registrar
  líneas, funciones y regiones instrumentadas por crate y, por separado, para
  parser, checkers, verifiers, heap y ejecución. Los umbrales se fijan después
  de medir la baseline; no se excluye código difícil solo para mejorar el
  porcentaje.

- [x] **MUT-001 — Medir mutation score en fronteras críticas.** Ejecutar
  mutación automática acotada sobre algoritmos y verifiers; cada mutante
  superviviente se clasifica como test ausente, código equivalente o exclusión
  justificada. El gate posterior impide regresiones del score acordado.

- [x] **REG-001 — Automatizar la regla de regresión.** Todo bug confirmado
  incorpora el caso mínimo en la frontera pública más baja que habría fallado,
  además de cualquier test interno necesario para localizar la causa.

### 16.5 M10.5b — Hardening de cobertura y oracles

- [x] **TEST-HARDEN-001 — Cerrar los huecos observables de mayor retorno.**
  La suite versionada completa se ejecuta dentro del proceso instrumentado y se
  añaden contratos positivos, negativos y de borde para CLI, artefactos,
  manifiestos, protocolo del adaptador, consultas y snapshots semánticos,
  bytecode, valores gestionados y tooling de fiabilidad. El inventario resultante
  contiene 1.507 casos lógicos y 1.726 repeticiones; no se cuenta un subprocess
  opaco como cobertura de las rutas que ejecuta.

- [x] **COV-002 — Elevar y ratchetear la baseline sin exclusiones.** La
  recaptura vigente alcanza 199.747/220.585 líneas (90,55 %),
  13.505/15.562 funciones (86,78 %) y 286.225/322.383 regiones (88,78 %).
  El gate conserva floors truncados de 9.055, 8.678 y 8.878 basis points y
  floors independientes para parser, checkers, verifiers, heap, ejecución y
  protocolos no confiables. Branch y MC/DC no se interpretan como 0 %: Rust
  1.93.0 con LLVM 21.1.8 publica ambos contadores con cero unidades
  instrumentadas, por lo que permanecen explícitamente no medidos hasta que el
  toolchain produzca una señal estable. Cualquier descenso medido falla aunque
  el porcentaje global permanezca por encima del valor anterior.

- [x] **MUT-002 — Revalidar la resistencia tras el hardening.** La compuerta
  vigente ejecuta una muestra crítica determinista de seis mutantes, uno por
  frontera revisada: todos son ejecutables, detectados, sin timeouts ni
  supervivientes. La selección completa de 30 mutantes permanece especificada
  como workload del carril de rendimiento y no se presenta como evidencia de
  esta compuerta acotada.

### Gate H0 — Infraestructura de fiabilidad

- [x] El gate completo de Tondo 0.1 se ejecuta automáticamente en PR y `main`.
- [x] El inventario y la matriz normativa se validan sin entradas sin
  clasificar para el target del corpus vivo; el contrato de testing todavía
  pendiente queda clasificado, no omitido ni contado como evidencia verde.
- [x] Existen generadores con seed reproducible y reducción de fallos.
- [x] Frontend, protocolos y admission verifiers tienen fuzz targets
  persistentes con corpus versionado.
- [x] Las familias críticas tienen al menos un modelo o property que compare
  secuencias, no solo ejemplos aislados.
- [x] Coverage y mutation score publican una baseline revisada y un gate de no
  regresión proporcionado al riesgo.
- [x] Un fallo de cualquier tier conserva evidencia suficiente para reproducir
  localmente el mismo input y target.
- [x] El gate estricto y la conformidad continúan verdes después de integrar la
  infraestructura.

### 16.6 M10.5c — Preparación portable y linaje único del draft

H0 permanece cerrado para el corpus vivo que lo demuestra, pero cada
ampliación del mismo draft necesita un frontend portable y evidencia activa.
Antes de ampliar la gramática de M10.7 o M10.6:

- [x] **PARSER-STACK-001 — Eliminar la dependencia del stack nativo.**
  La ruta efectiva mantiene solo un descenso recursivo fijo y pasa a frames
  explícitos para Pratt prefix/postfix/infix, grupos, arrays, bloques, loops,
  constructores, llamadas, records, tipos, patterns, recuperación de cierres
  ausentes y traversal lossless del CST. No se crea un segundo AST ni cambia la
  gramática, precedencia o shape del CST; se eliminó
  `MAX_SAFE_RECURSIVE_PARSER_DEPTH` y `ParseLimits.max_nesting_depth` es ahora
  el único presupuesto lógico, cargado contra los frames. La batería cubre
  casos válidos e inválidos de profundidad 1.000–4.000 en workers pequeños
  (64 KiB en POSIX y 256 KiB en Windows por el overhead del ABI/harness),
  equivalencia de partición/reconstrucción y token shape tras formatter, además
  de los 2.048 inputs arbitrarios y fuzz targets existentes. La evidencia
  observada en Linux x86_64 pasa; la matriz Linux ARM64/macOS/Windows queda
  como ejecución CI de targets, no como una afirmación local no verificada.

- [x] **CONF-DRAFT-001 — Consolidar una única conformidad de draft.** Mantener
  `conformance/draft/manifest.json` como única identidad activa y
  `conformance/0.1` como único corpus ejecutable vivo. Los requisitos todavía
  no demostrados permanecen pendientes hasta que sus layers tengan evidencia.
  El runner, reliability, matriz, scripts y CI no seleccionan manifests viejos
  ni reescriben sintaxis. Ambos manifests fijan directamente las specs y casos
  actuales; Git conserva el pasado. Los tests demuestran selección única,
  reproducibilidad y que el gate no presenta un requisito pendiente como
  conformidad completa.

- [x] **CONF-RATCHET-001 — Hacer incremental la evidencia nueva.** El comando
  `tondo-reliability ratchet check` valida el único draft vivo,
  inventario, matriz, baseline de quality y el registro canónico de hashes.
  `ratchet generate` solo escribe el registro después de comprobar todos esos
  bytes; si existen case layers exige reports de coverage y mutation que pasen
  la no-regresión. La Wave 0 no tiene capas ejecutables y registra ambos scopes
  como `not-applicable` con razón explícita. Cada wave futura debe ejecutar este
  mini-gate antes de integrarse; `META-CONF`, `UTEST-CONF` y los gates
  estándar siguen siendo cierres acumulativos.

- [x] **QUALITY-EVIDENCE-BIND-001 — Ligar quality evidence al árbol medido.**
  El runner de quality debe calcular antes y después un digest canónico de
  fuentes, tests, `Cargo.lock`, flags y toolchain; coverage y mutation registran
  ese digest junto a sus reports. `tondo-reliability` publica bindings cerrados
  `tondo-quality-report-binding/1`, valida el hash raw y la identidad actual
  antes de aceptar cada report, y `ratchet` conserva el digest de provenance por
  scope. El quality gate ejecuta el protocolo alrededor de llvm-cov y
  cargo-mutants; la conformance seal exige ambos hashes y rechaza evidencia sin
  identidad. Cambiar `target`/temporales no altera el árbol, pero cambiar una
  fuente, script, flag o toolchain sí. El baseline conserva además el origen de
  su captura. Este gate no cierra STD-IMPL-001 ni convierte la auditoría pública
  de la stdlib en completa.

- [x] **FAST-GATE-001 — Acortar el ciclo sin rebajar calidad.** Añadir el
  manifiesto `testing/fast-gate.json`, el clasificador de impacto y el gate
  ejecutable `scripts/fast-gate.sh`. El gate mantiene caches incrementales
  locales mediante `CARGO_TARGET_DIR`, ejecuta solo crates afectados cuando la
  frontera es local, comprueba líneas nuevas con `cargo llvm-cov`, ejecuta
  `cargo mutants --in-diff` en serie y escala a `test-gate.sh` ante una frontera
  compartida. `scripts/fast-gate-test.sh` cubre el clasificador y las decisiones
  de cobertura positiva/negativa; `docs/contracts/fast-gate.md` separa la
  evidencia local de la completa. La CI expone `fast` para push/PR y `full`
  para wave boundaries/manual y nightly; ningún cambio al umbral global ni a
  la ratchet queda implícito.

- [x] **DOC-TEST-001 — Implementar `tondo doc-test` por la ruta pública.**
  El comando exacto `tondo doc-test --edition 0.1 <markdown>` usa el scanner
  Markdown interno de fences, el registry normativo y el fixture manifest
  fijados por hash, y cubre `tondo`/`fragment`/`script`/`compile-fail`/
  `pseudocode`. La ruta llama al mismo lexer, parser, formatter y checker de
  referencia que conformance, no descubre proyectos ni introduce un parser
  Markdown general. Publica un array JSON compacto, determinista y con el
  orden de schema de 21.6 solo después de validar todos los fences; el script
  de gate lo escribe mediante un temporal y rename atómico. Las pruebas del
  CLI cubren opciones, UTF-8/CRLF, offsets, fixtures, errores y ausencia de
  output parcial; `DOC-TEST-CONF-001` conserva la conformidad exhaustiva como
  tarea separada.

- [x] **DOC-TEST-CONF-001 — Cerrar conformidad de ejemplos verificables.**
  Probar UTF-8 y CR/LF, headers, fences truncados, offsets de byte, fixtures,
  errores exactos, formatting, determinismo y ausencia de output parcial sobre
  los documentos normativos y un corpus hostil. El runner documental solo
  parsea, formatea y typecheckea; cada ejemplo que afirme comportamiento
  runtime enlaza además un caso de aceptación público de `tondo test` o de
  conformidad. No existe un harness documental paralelo. El gate ejecuta el
  mismo comando público sobre lenguaje, testing, toolchain, stdlib y TLF: 365
  fences pasan en el orden de sus documentos. Los 21 fences tipados están
  clasificados por `(file, fence_byte, source_sha256)`; 20 enlazan evidencia
  runtime pública ejecutable y uno declara de forma revisable que solo comprueba
  superficie estática. El corpus hostil cubre errores exactos, UTF-8, CR/LF,
  headers, cierres, offsets multibyte, formato, determinismo y output atómico.

- [x] **CONF-MATRIX-ALL-001 — Extender la matriz normativa a los tres contratos
  G5.** Inventariar requisitos estables de lenguaje, testing y toolchain con
  identidad, riesgo y seis dimensiones de evidencia. Un documento incluido en
  un futuro bundle no cuenta como cubierto solo por estar fijado por hash, y un fence no
  se considera test por ser parseable. La matriz de stdlib es
  `STD-MATRIX-ALL-001` y alimenta S1A/S1, no G5.
  La matriz v2 fija el conjunto ordenado de las tres specs y asigna namespaces
  `TL01`, `TT01` y `TC01`; cada fila conserva documento, hash/revisión, heading,
  riesgo, fase, estado y seis dimensiones. El inventario resultante contiene
  421 requisitos: 309 de lenguaje, 81 de testing y 31 de toolchain. Las 112
  filas especializadas nuevas quedan como límites explícitos hasta que la
  auditoría enlace evidencia ejecutable; ni el hash del documento ni sus
  fences cuentan como cobertura.

- [x] **CONF-GAP-AUDIT-001 — Clasificar todos los límites existentes.** Revisar
  individualmente los 344 `toolchain-limit` y los 22 `draft-pending` actuales y
  asignar una de tres salidas verificables: implementación existente sin
  trazabilidad, requisito no aplicable con razón normativa, o funcionalidad
  realmente ausente. No se permiten waivers agregadas por sección ni usar un
  ejemplo vecino como prueba implícita.
  `testing/normative-gap-audit.json` conserva una fila ordenada por cada uno de
  los 366 requisitos, ligada al hash exacto del texto y de las tres specs. El
  resultado es 364 implementaciones existentes sin la traza completa de seis
  dimensiones, dos no aplicables (`TL01-2-2-R001` y `TT01-13-R001`) con razón
  normativa individual y cero ausencias. Cada fila implementada nombra una ruta
  real y un ID ejecutable del inventario; el validador rechaza omisiones,
  extras, drift, paths inválidos, tests no ejecutables y formas ambiguas. El
  ratchet vivo revalida el registro junto a matriz e inventario sin convertir
  la clasificación en cobertura.

- [x] **CONF-GAP-IMPL-001 — Cerrar la trazabilidad descubierta por la
  auditoría.** Cada ausencia generaría una tarea leaf enlazada al requisito, una
  ruta pública real y tests positivos, negativos, de borde y composición. La
  auditoría no encontró ausencias: los casos ya implementados requieren añadir
  identidad y evidencia revisada, sin reescribir código que ya funciona. El
  registro conserva sus 366 decisiones y permite que una fila auditada avance
  únicamente a `covered` o, para los dos no aplicables, a
  `target-not-applicable`; eliminar una decisión auditada falla por identidad.

  - [x] **CONF-GAP-IMPL-TC-001 — Toolchain.** Los 31 requisitos `TC01` tienen
    evidencia explícita en las seis dimensiones. Los oracles ejercitan los
    records cerrados, resolución, proyectos convencionales, planes de test,
    inputs y meta-generación; la frontera pública usa las layers existentes de
    finalización, meta y testing. La matriz pasa de 45 a 76 filas cubiertas.
  - [x] **CONF-GAP-IMPL-TT-001 — Testing.** Las 80 filas aplicables `TT01`
    enlazan ahora su layer pública exacta y pruebas internas separadas de
    rechazo, borde, composición y oracle. La fila `TT01-13-R001`, que enumera
    funcionalidad deliberadamente ausente, se reconoce por identidad auditada
    y avanza a `target-not-applicable`; una prueba estructural exige el reparto
    exacto 80+1 y evidencia completa en las seis dimensiones aplicables.
  - [x] **CONF-GAP-IMPL-TL-001 — Lenguaje.** Las 253 filas aplicables `TL01`
    enlazan su prueba auditada con rechazo, borde, composición y oracle internos,
    más casos públicos de conformidad elegidos por la sección normativa. El
    no-goal `TL01-2-2-R001` se clasifica por identidad exacta. Una prueba
    estructural exige 298 requisitos cubiertos, ocho no aplicables y tres
    fronteras reservadas a la conformidad independiente de stdlib.

- [x] **CONF-LAYER-RESULT-001 — Ejecutar y atestar cada caso de layer.** El
  resultado de `tondo-conformance run` debe quedar ligado al hash del draft e
  incluir una observación
  verificable por cada caso `meta`, `testing` y `finalization`. Referenciar un
  nombre existente en el inventario no acredita ejecución. El schema rechaza
  layers/casos/evidencias ausentes, extra, duplicados, reordenados o ligados a
  otro árbol y el report incluye el resultado compuesto exacto.
  La implementación captura el árbol antes de `cargo test`, exige un único
  `ok` real por evidencia Rust y compone todas las layers con el corpus vivo.
  El formato fija manifests, inventario, árbol y observaciones; el validador
  rechaza cualquier divergencia frente al ratchet.

- [ ] **CONF-SEAL-FINAL-001 — Sellar el primer candidato real.**
  Exigir que los requisitos aplicables a lenguaje, testing y toolchain no
  conserven `toolchain-limit`, `draft-pending` ni ausencias; ejecutar resultados
  frescos y compuestos de `CONF-LAYER-RESULT-001`,
  `QUALITY-EVIDENCE-BIND-001`, `DOC-TEST-CONF-001`, coverage/mutation y Gate
  T0; después crear por primera vez un bundle `candidate` y comprobarlo sin
  consultar el draft vivo. Solo esta tarea, ejecutada como parte del primer
  release real, habilita el cierre final de G5; S1A usa su propia matriz y su
  propio gate. Hasta entonces no existe candidato ni directorio sellado.

---

## 17. M10.6 — Testing de usuario Tondo 0.1

**Objetivo:** implementar
[`TONDO_TESTING_SPEC.md`](./TONDO_TESTING_SPEC.md) como parte de la primera
versión de Tondo. El resultado es una declaración
`test name { ... }` para cada hoja y una declaración `suite name { ... }` para
jerarquía y lifecycle compartido, unit tests con acceso privado controlado,
integration tests contra API pública, control y metadata sellados de
log/tags/fallo/skip/attachments/snapshots sin contexto visible, ownership
resuelto desde CODEOWNERS, selección glob portable, sharding estable, orden
aleatorio reproducible, retries y repetición proactiva explícitos y aislados con
historial honesto, tiempo virtual opt-in sobre las APIs monotónicas de
producción, inputs públicos/secretos explícitos, interrupción fiable y reportes
JSON/JUnit desde una sola invocación. El runner resultante puede utilizarse para
completar y validar la stdlib.

**Dependencia:** M10.6 empieza después de H0, `CONF-DRAFT-001` y
`META-FORMAT-001`, pero no espera a que M10.7 esté completo. Plan,
discovery/dev-dependencies, lexer/CST/formatter, árbol estático, algoritmos
puros de selección y `defer` pueden avanzar en lanes independientes.
`PARSER-STACK-001` debe cerrarse antes de `UTEST-CST-001`, para que la nueva
sintaxis no amplíe la ruta recursiva temporal.
`UTEST-CHECK-001` y la ruta de attachments usan la identidad binaria de
`std.bytes`; el checker usa las identidades implementadas de `Duration` e
`Instant` del time-base. La lectura de inputs declarados quedó cerrada por
`STD-ENV-SPEC-001`, `STD-ENV-IMPL-001` y `STD-ENV-CONF-001`;
`UTEST-VTIME-001`, el lifecycle temporal y Gate T0 quedaron cerrados sobre
`STD-TIME-BASE-SPEC-001`, `STD-TIME-BASE-IMPL-001` y
`STD-TIME-BASE-CONF-001`. Son APIs de producción de STD-0.1A, nunca shims
privados del runner. La semántica funcional de `defer` ya conduce el
teardown de suite; `ASYNC-DEFER-IMPL-001` cerró sus fixtures de hardening y
testing no lo sustituye por un hook.

**Evolución pre-release:** el corpus y su manifest se regeneran con el borrador
actual `TONDO_LANGUAGE_SPEC.md`; no se conserva un segundo parser, dialecto,
snapshot ni hash histórico. Hasta cerrar M10.6, un
binario declara testing como componente pendiente y no anuncia conformidad
completa Tondo 0.1.

**Orden interno por lanes:**

1. **Plan:** `UTEST-PLAN` → contrato de inputs
   (`UTEST-INPUTS-PLAN`)/discovery/owners/dev-dependencies →
   `UTEST-RESULT-MODEL` → `UTEST-CLI-PARSE`. La materialización de inputs queda en la lane de
   ejecución, después de crear el worker.
2. **Lenguaje:** lexer → CST → formatter; tras unir source classes,
   discovery y dev-dependencies del plan → árbol/capturas →
   overlays/integration. `ASYNC-DEFER-IMPL-001` ya está cerrado sobre la ruta
   de suspensión existente; `ASYNC-ITER-EXT-001` ya cerró el lowering genérico
   y la siguiente leaf async es la conformance completa de `std.async`.
3. **Ejecución:** check → lowering y modelo de resultados en paralelo →
   envelope → worker → inputs/lifecycle/límites.
4. **Algoritmos puros:** glob → shard → order/scheduler, después de identidad y
   plan pero sin esperar I/O de artifacts o reporters.
5. **Features sobre el worker estable:** tiempo virtual, retry/repeat,
   attachments y snapshots.
6. **Cierre:** JSON → JUnit → transacción de interrupción → wiring completo de
   CLI → proyectos/plataformas/dogfooding → Gate T0.

Plan y frontend se unen antes de `UTEST-ID-001`; la semántica estática completa
se une antes de `UTEST-CHECK-001`. Ejecución y algoritmos se unen antes de
retry/repeat; todos los productores de outcomes se unen antes de congelar los
reporters.

### 17.1 Spec 0.1 y plan cerrado

- [x] **UTEST-SPEC-001 — Fijar el contrato normativo de testing.**
  `TONDO_TESTING_SPEC.md` define keywords, grammar, árbol suite/test, formato,
  identidad, source classes, overlays, capturas, lifecycle, envelope,
  `std.testing.log/tags/failNow/skip/attach/snapshot/withVirtualTime`, inferencia,
  aislamiento, resultados,
  ownership, selección substring/glob/exact, sharding, orden reproducible,
  retries por rondas, repeat por iteraciones y workers nuevos,
  `withVirtualTime`/`settle`/`advance`, quiescencia durable, cleanup suspendible,
  interrupción, inputs públicos/secretos, artifact/snapshot stores, update
  explícito, CLI, JSON/JUnit, stdlib boundary, diagnósticos y conformidad sin
  depender de una implementación provisional.

- [x] **ASYNC-DEFER-SPEC-001 — Inferir cleanup suspendible en `defer`.** La
  forma única `defer expression` o `defer { ... }` infiere suspensión desde sus
  llamadas, sin `async` ni `await` escritos. Exige outcome infalible `Unit`,
  reserva operands afines, comprueba liveness/`Send` y conserva LIFO,
  cancelación, unwind, pánicos suprimidos, timeout/resource/interrupt y
  capabilities. Rechaza `await`, `spawn` y `scope` explícitos en el cleanup.

- [x] **UTEST-EDITION-001 — Consolidar testing en Tondo 0.1.** Añadir `suite`
  y `test` al registry de keywords de la especificación viva, incorporar
  la inferencia de `defer`, grammar y diagnósticos y usar una única línea
  `draft`. El corpus se migra y regenera con el lenguaje vivo; no se preservan
  bytes de borradores anteriores. La implementación y los tests de compilación
  se ejecutan en las tareas siguientes.

- [x] **UTEST-PLAN-001 — Extender el project plan con source classes de test.**
  `tondo-test-plan-draft` y `TestProjectPlan` representan exactamente
  `production`, `unit-test` e `integration-test`, dev-dependencies, roots
  físicos/lógicos explícitos, paths lógicos, raíz de repositorio, referencias a
  inputs y CODEOWNERS, selector, shard, orden, seed, policy, retry/repeat,
  reporters, artifact store, snapshot stores, target, capabilities, catálogo
  temporal estándar y límites. El parser es puro: valida hashes de manifest y
  lockfile, exige que production coincida con el proyecto activo, normaliza la
  forma canónica y no lee fuentes, inputs, CODEOWNERS ni el host. La
  participación exacta de inputs se cierra en `UTEST-INPUTS-PLAN-001`; el hash de
  un artefacto de producción continúa independiente de metadata o fuentes
  test-only. No existe un “source root” inferido por common-prefix: discovery
  solo enumerará roots declarados y los convertirá en entradas cerradas antes
  del frontend.

- [x] **UTEST-INPUTS-PLAN-001 — Cerrar inputs sin ejecutar workers.**
  `tondo-test-input-plan-draft` y `TestInputPlan` validan, contra el hash del
  `TestProjectPlan`, nombres de input únicos, source, profile
  (`build`/`runtime`/`both`), visibilidad pública/secreta, capability habilitada
  y los estados `closed`, `secret-dependent-versioned` y
  `secret-dependent-unversioned`. Los inputs públicos fijan un SHA-256 de
  contenido; los secretos fijan únicamente provider, descriptor y versión
  opcional. `public_sha256`, `secret_profile_sha256`, `secret_count` y la
  reproducibilidad se calculan sobre listas canónicas y no contienen valores.
  La frontera es pura, rechaza campos desconocidos, colisiones, deriva del
  plan, hashes/capabilities inválidos y mezclas público/secreto, y no lee el
  host ni materializa valores. Evidencia: `docs/contracts/test-input-plan.md`
  y cinco tests unitarios en `tondo-compiler`.

- [x] **UTEST-RESULT-MODEL-001 — Fijar el modelo interno de ejecución.**
  `TestResultTree` implementa el report format `tondo-test-report-0.1/7` como
  una única representación validada de nodes, participation, phase, attempt,
  iteration, retry unit, outcome agregado, causalidad, `blocked_by`, policy y
  summary. `assemble` deriva status/decisive attempt/counts una sola vez y
  `parse` rechaza IDs/parents/causas rotas, fases incompatibles, payloads
  incoherentes, hashes inválidos, schema drift y summary inconsistente.
  `CoordinatorFrame`/`WorkerFrame` y `ProtocolSession` fijan
  `tondo-test-worker-0.1/1`, handshake, secuencias por dirección, límites
  positivos, run units, cancelación con ACK, shutdown/closed y errores fatales.
  El módulo es puro: no ejecuta bodies, consulta host ni transporta valores
  secretos. Evidencia: `docs/contracts/test-result-model.md` y siete tests
  unitarios que cubren agregados/flaky, summary, causalidad, canonicalización,
  handshake, secuencias, límites, cancelación, cierre y schema desconocido.

- [x] **UTEST-CLI-PARSE-001 — Implementar parsing y normalización de CLI.**
  `tondo_cli::test_cli::parse` añade `tondo test` y convierte su vector de
  argumentos a `TestCliPlan` sin discovery, I/O, compilación ni workers.
  Cierra selectores filter/glob/exact, CODEOWNERS, shard, order/seed, list,
  jobs, timeout, retry/repeat, artifacts, formats/reports y policies; conserva
  la presencia explícita de retry/repeat para aplicar las incompatibilidades
  de los valores cero/uno. Valida paths, globs, ranges, overflow, duplicados,
  report collisions y combinaciones de list/update antes de compilar.
  Diagnostics de uso terminan en exit `2`; el parser es una frontera sin
  efectos y el runner consume el plan cerrado en `UTEST-CLI-BACKEND-001`.
  Evidencia: `docs/contracts/test-cli-plan.md`, cinco tests unitarios del parser
  y tests CLI de los límites de uso y ejecución.

- [x] **UTEST-INPUTS-001 — Materializar inputs públicos y secretos sin
  filtraciones.** Después de `UTEST-INPUTS-PLAN-001`,
  `UTEST-RUNTIME-001` y `STD-ENV-CONF-001`, resolver los descriptors del plan,
  materializarlos exclusivamente dentro del worker y revocarlos al terminar.
  Probar que valores secretos no entran en plan serializado, cache key de
  compilación, diagnostics, reportes, snapshots, artifacts o productos salvo
  copia explícita del programa, y documentar que el runner no realiza redacción
  heurística. Un fallo de materialización termina con exit `1` sin reporte
  parcial; un fallo de revocación pierde aislamiento y usa exit `3`. Evidencia:
  `docs/contracts/test-input-runtime.md` y siete tests unitarios sobre selección
  build/runtime, hash público, errores de proveedor, contención de pánicos,
  revocación idempotente, metadata sin secretos y acceso a inputs no
  materializados.

- [x] **UTEST-DISC-001 — Implementar descubrimiento convencional y explícito.**
  `tondo_compiler::test_discovery` recibe entradas enumeradas por el host y
  aplica, sin I/O, la precedencia de `tests/`, `_test.to` y roots explícitos,
  con paths slash-separated canónicos y comparación case-sensitive. Ordena por
  bytes UTF-8 del path físico, rechaza fuentes no regulares, symlink escapes,
  colisiones físicas o de nodo lógico y asigna inputs estables
  `source:<class>:<physical-path>`. `reconcile_plan` exige igualdad exacta con
  la identidad cerrada del plan antes de compilar. Evidencia:
  `docs/contracts/test-discovery.md` y ocho tests unitarios del compilador.

- [x] **UTEST-OWNERS-001 — Resolver ownership de tests desde CODEOWNERS.**
  `tondo_compiler::test_owners` implementa `auto`, `none` y path explícito sin
  I/O, con precedencia estricta, paths canónicos, guards de regular/readable y
  symlink, UTF-8 sin BOM, gramática portable, glob case-sensitive y última
  regla aplicable. Conserva owners opacos y duplicados, devuelve source/hash,
  resuelve por path lógico y deja `[]` para una fuente generada sin origin.
  Evidencia: `docs/contracts/test-owners.md` y nueve tests unitarios del
  compilador.

- [x] **UTEST-DEPS-001 — Separar dev-dependencies del grafo de producción.**
  `tondo_compiler::test_dependencies` valida records de interfaz por alias,
  PackageId, path y hash exactos; limita edges transitivos al subgrafo de test
  o `toolchain:std:0.1-bootstrap`, rechaza ciclos/overlap con producción y
  expone aliases solo a unit/integration. `production_identity` deja explícita
  la huella de inputs productivos sin plan ni records de test. Evidencia:
  `docs/contracts/test-dependencies.md` y nueve tests unitarios del compilador.

### 17.2 Frontend y semántica estática

- [x] **UTEST-LEX-001 — Añadir `suite` y `test` a las keywords Tondo 0.1.**
  `TokenKind` y `from_keyword` las registran como keywords reservadas. El lexer
  las clasifica en Module, ImportedModule, Script y Fragment, mantiene la
  normalización NFC de identificadores vecinos, no emite diagnostics y conserva
  la partición/reconstrucción byte a byte. La misma clasificación es
  independiente de `SourceId`, ruta lógica y origen físico/virtual; `Name`
  rechaza ambas como nombres de usuario. Evidencia: tests unitarios de
  `syntax::lexer` y `package`, además de la suite completa de `tondo-compiler`.

- [x] **UTEST-CST-001 — Parsear `test` y `suite` sin pérdida.** `SyntaxKind`
  y la fachada AST tipada exponen `TestDecl`, `SuiteDecl` y `SuiteBlock`; el
  parser acepta las dos formas canónicas en Module e ImportedModule, conserva
  setup ordinario y miembros directos anidados, y mantiene la partición y
  reconstrucción byte a byte. El recovery rechaza modifiers, declaraciones
  dentro de tests/bloques ordinarios, y sentencias después del primer miembro
  sin perder los miembros posteriores; Script conserva el rechazo de la forma.
  Evidencia: ocho tests de parser con AST, modos de fuente, recovery,
  modifiers, nesting y reconstrucción lossless, además de la suite completa de
  `tondo-compiler`.

- [x] **UTEST-FMT-001 — Formatear suites y tests canónicamente.** El formatter
  emite spacing y bloques canónicos para `test`/`suite`, mantiene bodies y
  setups vacíos/multiline, nesting, comentarios y documentación, y separa el
  setup de sus miembros y las declaraciones adyacentes con las reglas de
  módulos. La salida es idempotente, el recovery inválido no fabrica bytes y
  `fmt` no depende de discovery runtime. Evidencia: tres tests de integración
  dedicados en `formatter_spec`, además de la suite completa del compilador y
  el gate oficial del formatter.

- [x] **ASYNC-DEFER-IMPL-001 — Inferir suspensión en toda forma de `defer`.**
  La semántica funcional, CST/formatter, checks de
  firma/efecto/ownership, guards HIR/MIR/bytecode y conducción LIFO ya existen
  y quedan sellados por fixtures del draft actual. La evidencia cubre
  inferencia desde llamada y bloque, entrada y script (`m10-defer-inferred`), la forma
  canónica `fn ... suspends`, `E1601` en `@sync`/`@nosuspend`, retorno, error
  exterior, pánico con cleanup suprimido, cancelación, timeout forzado,
  resource limit, interrupción, owner afín, `Send`, bloque, `Join`, await
  anidado, llamada no suspendible, llamada fallible y capability (`E1608`,
  `E1611`, `E1401`, `E1605`, `E1410` y `E1008`). Runtime añade LIFO, cancelación
  de hijos, host-backed `ProcessHandle.cancel` y script inferred; HIR conserva
  `Await` sobre una llamada suspendible, MIR/bytecode validan `DeferredAsync` y
  la VM mantiene el cleanup durante panic/cancelación sin relabelar un fallo de
  recurso `T0002`. Evidencia adicional: los tests de driver
  `async_deferred_cleanup_*`, el modelo de timeout
  `cleanup_runs_lifo_after_error_and_is_skipped_for_forced_termination` y la
  transacción de interrupción que espera el cleanup antes de una salida segura.
  Los tests negativos fijan el rechazo del prefijo explícito `await` dentro de
  `defer`, no una ruta de compatibilidad.

- [x] **UTEST-SUSPENSION-CONTRACT-001 — Fijar el contrato suspendible del
  runner.** Revisar check, lowering, worker y fixtures de test para
  que una llamada suspendible directa espere implícitamente, `await` directo
  produzca `E1611`, `Join` conserve consumo explícito y `Waiter.wait()` use la
  llamada directa normal; además,
  `@sync`/`@nosuspend` produzca `E1601`. El test harness no puede introducir una
  API `async` paralela ni depender de que el body escriba `await` para inferir
  suspensión. Añadir compile-pass, compile-fail y runtime tests con hash de
  interfaz `suspends` antes de marcar la conformidad de testing. Cerrado con la
  ruta de atributos del parser/CST, la frontera `no_suspend` del HIR/checker,
  el backend de conformance y fixtures canónicos `fn`; la observación de
  interfaz fija `api_hash` y `content_hash` para la forma directa.
  Evidencia: `tests/compile-pass/m7-direct-suspension-call.to`,
  `tests/compile-fail/m7-await-direct-call.to`,
  `tests/compile-fail/m7-suspension-sync-boundary.to`,
  `tests/runtime/m7-suspension-inferred.to` y
  `crates/tondo-reference-adapter/tests/suspension_contracts.rs`. Todos los
  fixtures usan la gramática vigente.

- [x] **UTEST-ID-001 — Construir el árbol estático suite/test.** La identidad
  interna usa PackageId + source class + module path + ordered node path + kind;
  la visible usa `package::unit|integration::path::suite...::test`. Registrar
  parents, rechazar suites vacías `E2004`, nombres hermanos duplicados `E2002`
  y cualquier intento de reabrir/mezclar suites. Orden, warnings y source
  ranges son deterministas entre archivos permutados. `test_tree::build`
  conserva el parent identity, spans de declaración/nombre y warnings `W1004`,
  y devuelve diagnósticos ordenados de forma estable para `E2001`, `E2002` y
  `E2004`. Evidencia: `docs/contracts/test-tree.md` y doce tests unitarios
  sobre nesting, IDs unit/integration, duplicados cross-file, suites vacías,
  producción, permutación, descarte y spans.

- [x] **UTEST-CAPTURE-001 — Comprobar entornos de suite.** Un descendiente solo
  captura bindings ancestrales `let: Copy + Send + Share` mediante snapshot.
  Rechazar con `E2005` `var`, préstamos, moves, valores afines/terminales y
  cualquier bypass a través de suites anidadas. Constantes y funciones de
  módulo continúan resolviéndose por nombre. `test_capture::build` valida la
  cadena de padres, facts de capabilities/terminales provenientes de HIR,
  acceso ordinario por valor y slots de snapshot deterministas por hijo.
  Evidencia: `docs/contracts/test-capture.md` y nueve tests unitarios sobre
  snapshots válidos, nesting, modos de binding/uso, capabilities, terminales,
  diagnósticos y entradas inválidas.

- [x] **UTEST-OVERLAY-001 — Implementar el overlay unitario sellado.** Resolver
  y comprobar producción primero, después permitir lectura privada y helpers
  privados sin reabrir bodies, añadir exports ni cambiar interfaces. Casos
  negativos demuestran que un overlay no repara producción inválida, altera
  coherence ni entra en el grafo production. `test_overlay::ProductionSeal`
  exige resolución, comprobación semántica y coherencia completas más hashes
  de interfaz/capabilities/coherence/artefacto; `from_resolved` filtra un
  conjunto explícito de fuentes de producción. `test_overlay::build` solo
  acepta `UnitTest`, ordena imports/helpers/referencias de forma determinista,
  conserva el árbol de tests separado y rechaza exports públicos, colisiones,
  self-imports, visibilidad privada importada, mutaciones de coherence y
  referencias desconocidas. Evidencia: `docs/contracts/test-overlay.md` y
  once tests unitarios, incluidos el adapter del resolver y la invariancia de
  los hashes de producción.

- [x] **UTEST-INTEG-001 — Implementar integration roots aislados.**
  `test_integration::build` deriva un `PackageId` sintético estable a partir del
  paquete probado y el path lógico `tests/*.to`, conserva el nombre del paquete
  probado únicamente en el prefijo visible y mantiene el consumidor separado.
  Los imports son explícitos y solo admiten el paquete probado o
  dev-dependencies del grafo cerrado; interfaces privadas, paquetes
  desconocidos, alias duplicados, self-imports y miembros duplicados se
  rechazan. Los roots solo pueden declarar helpers privados propios, nunca
  exports públicos ni acceso al scope unitario. `build_many` ordena por path y
  rechaza roots duplicados. Evidencia: `docs/contracts/test-integration.md` y
  ocho tests unitarios deterministas.

- [x] **UTEST-CHECK-001 — Inferir el contrato exacto del body.**
  `test_check::check` cierra las entradas privadas `fn(): Unit ! E` de
  tests y setups, permite `Unit`/`Never`, prohíbe valores retornados y
  `return` en setup, normaliza la unión de errores y exige `Discard`. Consume
  las pruebas de ownership, préstamos, terminales, `Send`, `Share` y `unsafe`
  sin relajarlas, infiere suspensión desde `await` y tiempo virtual y rechaza
  `std.testing` desde producción con `E2003`. Valida las formas monomórficas de
  `log`, `tags`, `failNow`, `skip`, `attach`, `snapshot`, `withVirtualTime`,
  `settle` y `advance`, incluyendo nombres/media types, duplicados de
  evidencia, `P2005`/`P2006` y la clausura
  `Send + CallOnce[fn(ref VirtualTime): Unit ! E]` con efecto suspendible
  inferido. Evidencia:
  `docs/contracts/test-check.md` y diez tests unitarios deterministas.

### 17.3 Lowering, runtime y CLI

- [x] **UTEST-LOWER-001 — Bajar entradas de test por el pipeline común.** HIR,
  MIR, bytecode y sus admission verifiers representan árbol/parent, entradas de
  setup, snapshots de entorno, identidad, source span, error, suspensión,
  cleanup,
  `TestLog`, `TestTags`, `TestFailNow`, `TestSkip`, `TestAttach`,
  `TestSnapshot`, entrada/salida de dominio, `VirtualTimeSettle` y
  `VirtualTimeAdvance` sin crear un segundo frontend o una ruta de ejecución no
  verificada. `main` nunca se ejecuta en un test target. `test_lower::lower`
  ordena por span, conserva snapshots/cleanup y `test_lower::verify` exige
  streams HIR/MIR/bytecode idénticos, identidad canónica y hash de artefacto.
  Evidencia: `docs/contracts/test-lower.md` y nueve tests unitarios.

- [x] **UTEST-CONTROL-001 — Implementar el envelope sellado de ejecución.** Cada
  suite/test recibe node ID, tag/log/artifact/snapshot/stdout/stderr sinks,
  cancelación y límites en estado privado del runtime, nunca como valor o
  thread-local Tondo. Helpers,
  closures y tasks estructuradas heredan el enlace; verifiers rechazan
  operaciones forjadas o presentes en artefactos de producción. Implementar
  `log`, merge atómico e idempotente de `tags`, primera key conflictiva estable,
  `P2002` ante valores incompatibles, `failNow` con `P0007`, `skip`, precedencia
  de cleanup y `P2001` sin exponer `TestContext`, `currentTest()` ni identidad
  del nodo. Los tags no se heredan entre nodos ni intervienen en
  discovery/selección/sharding/orden. Artifacts/snapshots usan registros
  separados por intento y no exponen paths/store al programa. Un skip de hijo
  marca la entrada completa,
  cancela el resto del scope y se propaga a la task propietaria con la prioridad
  determinista fijada por el lenguaje. El mismo envelope mantiene un registro
  por intento de dominios virtuales, pero no expone su node ID, sinks o policy al
  controlador temporal prestado. `test_control::EnvelopeHandle` implementa
  la frontera, `admit_operation` mantiene `E2003` fuera de producción y el
  reporte conserva evidencia ordenada sin paths ni contexto observable.
  Evidencia: `docs/contracts/test-control.md` y dieciséis tests unitarios.

- [x] **UTEST-RUNTIME-001 — Ejecutar cada hoja en una raíz aislada.** Estado,
  roots, heap observable, tasks, handles, pánicos, tags, logs, attachments,
  comprobaciones de snapshot, stdout, stderr, envelopes y presupuestos no cruzan
  hojas salvo snapshots de entorno de suite comprobados.
  Retorno, skip, error, pánico, resource limit, timeout e infrastructure
  producen exactamente los estados normativos; los terminales cooperativos
  completan unwind y cleanup, mientras una terminación forzada garantiza
  aislamiento sin fingir que ejecutó `defer`. Exponer un bootstrap de worker que
  pueda crear una VM realmente nueva desde el artefacto inmutable y rastrear
  procesos/recursos de host hasta revocarlos, sin serializar heap, roots ni
  handles como snapshot reutilizable. El executor recibe el proveedor monotónico
  real o virtual mediante una frontera interna única; el bytecode de usuario y
  las llamadas de `std.time` no cambian entre ambos. El protocolo de worker
  expone una única fase sellada de inicialización/revocación y ejecuta primero
  con snapshot de environment vacío; `UTEST-INPUTS-001` conecta después las
  fuentes declaradas sin cambiar el protocolo ni serializar valores de vuelta.
  `test_runtime::RuntimeRunner` crea bootstrap/envelope/registry nuevos por
  hoja, captura pánicos y proyecta estados terminales, ejecuta cleanup LIFO y
  revoca todos los recursos antes de devolver resultados ordenados por ID.
  Evidencia: `docs/contracts/test-runtime.md` y catorce tests unitarios.

- [x] **UTEST-SUITE-001 — Implementar lifecycle jerárquico de suite.** Ejecutar
  setup una vez por participación y solo si existe una hoja seleccionada,
  conservar su entorno y guards, entrar de fuera hacia dentro y hacer teardown
  de dentro hacia fuera después de todos los descendientes. Un fallo de setup
  bloquea solo su subárbol, ejecuta cleanup realmente observable y permite
  continuar hermanos; un fallo de teardown no reescribe resultados ya emitidos.
  Un skip de setup produce `skipped`/`blocked-skip`; un fallo durante su cleanup
  prevalece y convierte descendientes en `blocked-setup`. La misma máquina de
  lifecycle debe admitir una participación posterior en un worker de retry sin
  reutilizar el entorno ni sus snapshots. Conducir `defer` hasta completar
  dentro de teardown, sin bloquear el worker ni inventar `afterAll`. Evidencia:
  `docs/contracts/test-suite.md`; el lowerer de participación, HIR/MIR/bytecode,
  verifiers y VM ejecutan el árbol público compilado en un único worker con
  envelopes sellados por nodo. Tests de compiler y CLI acceptance cubren scope
  compartido, orden exterior-interior, cleanup LIFO, pánico aislado de hoja,
  bloqueo por el ancestro causal más cercano, skip/`blocked-skip`, continuidad
  de hermanos, fases setup/teardown y retry de suite completa con contexto
  fresco.

- [x] **UTEST-LIMIT-001 — Hacer límites y timeout terminales reales.** Publicar
  defaults finitos, aplicar `--timeout` por hoja y por fase setup/teardown sin
  contar la espera de descendientes, cargar tags/logs/stdout/stderr al mismo
  presupuesto de output, artifacts/snapshot actual a límites separados de
  cantidad/bytes y dominios/timers/cola/descriptores virtuales a presupuestos
  finitos de trabajo, memoria y metadata. Aplicar deltas sin cambios parciales,
  registrar valores efectivos, fijar grace period de interrupción y garantizar
  que una entrada no cooperativa no continúa tras `timeout`. Cada intento obtiene
  presupuestos nuevos bajo el mismo resource profile; timeout,
  CPU/instrucciones, memoria y output siempre usan recursos reales aunque el
  intento abra tiempo virtual. OOM, abort o pérdida de aislamiento nunca se
  presentan como assertion failure ordinario. Evidencia:
  `docs/contracts/test-limits.md` y ocho tests unitarios sobre defaults, hashes,
  validación, reservas atómicas, deltas duplicados, pausas de timeout, timeout
  desactivado, regresión de reloj y grace period de interrupción.

- [x] **UTEST-GLOB-001 — Implementar el selector glob portable.** Parsear
  componentes `::`, `*`, `?` y `**` con la gramática cerrada de la spec,
  rechazar patterns vacíos/no canónicos y no delegar matching al shell,
  filesystem, locale ni normalización. Hacer full match case-sensitive sobre
  IDs Unicode de suite/test mediante un algoritmo dinámico acotado
  `O(pattern_scalars * id_scalars)`, seleccionar subárboles de suites,
  deduplicar la unión y aplicar selección antes de shard/order. Cubrir vectores
  Unicode, cero/muchos componentes para `**`, metacaracteres inválidos,
  coincidencias solapadas y no-match con y sin `--allow-empty`. Evidencia:
  `docs/contracts/test-glob.md` y ocho tests unitarios sobre full-match,
  Unicode, globstar, gramática inválida, deduplicación, selección de suites,
  hojas individuales, selección vacía y árboles malformados.

- [x] **UTEST-SHARD-001 — Particionar hojas de forma estable.** Aplicar
  `sha256-mod-v1` después de filter/glob/exact y antes del orden, con índices
  one-based, validación estricta y asignación independiente de discovery order,
  plataforma y cantidad de jobs. Probar unión exacta, disjunción, compilación
  completa, el vector SHA-256 normativo, lifecycle independiente por proceso y
  shard vacío válido sin `--allow-empty` cuando la selección previa no era
  vacía. Evidencia: `docs/contracts/test-shard.md` y ocho tests unitarios sobre
  parsing estricto, vector SHA-256 normativo, independencia del discovery
  order, unión/disjunción, shard vacío, shard único, selección inválida y
  constantes del protocolo.

- [x] **UTEST-SCHED-001 — Fijar orden y paralelismo observable.** El default
  usa `id-byte-order-v1`; random usa `sha256-tree-v1` con seed hexadecimal
  explícita o generada y registrada. Ordenar hermanos sin romper la atomicidad
  estructural ni el bracketing de suites y materializar `execution_plan` como
  prioridad de dispatch. Verificar los digests normativos. Con jobs=1 una seed
  reproduce el orden exacto; con jobs=N reproduce la prioridad, no completion
  timing. Jobs explícitos limitan conjuntamente setup/test/teardown; cada
  envelope conserva tags/artifacts/snapshots/logs/streams y los arrays finales
  permanecen canónicos
  y nunca intercalan nodos. El mismo límite global gobierna workers e intentos
  de todas las rondas de retry. Dentro de un dominio virtual, usar una cola
  determinista por secuencia de creación/wake y ordenar timers empatados por
  creación sin cambiar el scheduler de producción fuera de tests. Iteraciones
  repeat son secuenciales y cada una vuelve a aplicar el mismo límite global.
  Evidencia: `docs/contracts/test-schedule.md` y ocho tests unitarios sobre
  seeds canónicas, vectores SHA-256, atomicidad de suites, repetibilidad,
  árboles inválidos, límite global de jobs, cola virtual y constantes de
  protocolo.

- [x] **UTEST-VTIME-001 — Implementar tiempo virtual determinista sobre la API
  de producción.** Ejecutar `withVirtualTime` como `CallOnce` suspendible bajo un
  dominio por intento/fase; prestar `ref VirtualTime`, prohibir escape y todo
  solapamiento dentro del mismo envelope y desmontar siempre tras retorno, error,
  pánico, skip o cancelación.
  `settle` conduce hasta quiescencia durable sin mover tiempo; `advance` exige
  duración no negativa, visita deadlines hasta un target exacto y no lo
  sobrepasa. El avance automático salta al próximo timer solo cuando raíz y
  tasks están duraderamente bloqueadas; el catálogo incluye timers, joins y
  sincronización enteramente local y excluye filesystem, red, procesos,
  syscalls, reloj civil y callbacks externos. Implementar cola/timer ties
  deterministas, cero, múltiples dominios secuenciales, `P2003` deadlock,
  `P2004` solapamiento y `P2005` rango/overflow. Mantener timeout y límites reales,
  y probar instantes/deadlines de otro dominio, backoff, debounce, deadline,
  cancelación, reprogramación infinita/livelock, espera externa y cleanup sin
  pausas wall-clock.

  Evidencia implementada en `crates/tondo-compiler/src/test_virtual_time.rs`,
  integrada en `test_control::VirtualTime`, con cola/timer deterministas,
  dominios aislados por envelope, deadlock/espera externa/livelock y rango
  `P2003/P2004/P2005`. Contrato y límites: `docs/contracts/test-virtual-time.md`;
  cobertura ejercitada por los tests unitarios del módulo y del envelope. La
  superficie pública está conectada de extremo a extremo mediante
  `std.testing.withVirtualTime`, `VirtualTime.settle` y `advance`: el frontend
  infiere closures suspendibles, conserva `Send + CallOnce` mediante una coerción
  consumible verificada en HIR/MIR/bytecode, presta el controlador opaco y
  rechaza `spawn withVirtualTime` con `E1601`. El VM sustituye y restaura la
  pareja proveedor/dominio de `std.time`, conduce las mismas tasks y timers y
  cierra la frontera también al desenrollar un pánico. Los tests públicos del
  driver y del host prueban un `BytesBuilder` affine, `time.sleep` bajo
  `settle` sin espera real, dos dominios secuenciales, evidencia one-based con
  avance automático/explícito, restauración de instantes exteriores,
  revocación, espera externa `P2003`, solapamiento `P2004`, rango/overflow
  `P2005` y cleanup por `P0008`.

- [x] **UTEST-RETRY-001 — Implementar retries explícitos y sin estado
  heredado.** Parsear `--retry N` con default cero y máximo finito; ejecutar la
  ronda inicial completa antes de planificar rondas adicionales solo para
  `failed-error`, `failed-panic` y `timeout`. Construir unidades hoja con
  lifecycle ancestral y unidades suite con el subárbol seleccionado original;
  absorber causas descendientes bajo la suite elegible exterior, ordenarlas por
  primera hoja del plan y conservar shard, target, inputs, seed, order,
  capabilities, limits y artefacto. Cada unidad arranca un proceso worker nuevo
  con VM/heap/roots/executor/tasks/handles/envelopes/buffers/budgets/temp nuevos,
  revoca recursos rastreados y nunca cruza shard. No reintentar compile errors,
  skips, resource-limit ni infrastructure; no añadir delays, historial o
  annotations, ni reintroducir esos terminales indirectamente desde un agregado
  previo. Preservar todos los intentos con `iteration: 1` y referencia de
  ronda/unidad, derivar intento decisivo y `flaky-pass`; este último falla por
  default y `--allow-flaky` solo cambia policy de salida. Cada worker empieza
  sin dominios ni registros de attachments/snapshots y cada dominio vuelve al
  mismo cero virtual, sin heredar timers, task order, contadores ni tiempo
  avanzado. El snapshot store esperado permanece como input inmutable y los
  attachments de cada intento permanecen separados. Rechazar la combinación
  con repeat o snapshot update.

  Evidencia implementada en `crates/tondo-compiler/src/test_retry.rs`: política
  finita, planner de hojas/suites con absorción ancestral, contexto canónico y
  campaña runtime por workers frescos. `docs/contracts/test-retry.md` fija el
  contrato; los tests cubren causas elegibles/no elegibles, flaky-pass,
  aislamiento de workers, orden y combinaciones incompatibles.

- [x] **UTEST-REPEAT-001 — Implementar repetición completa y aislada.** Parsear
  `--repeat N` con default uno y `N >= 1`; rechazar retry, allow-flaky, list y
  snapshot update. Ejecutar cada iteración completa de forma secuencial en un
  proceso worker nuevo, sin recompilar y conservando selección, shard,
  `execution_plan`, seed, inputs, stores, capabilities, limits y orden. Dentro
  de cada iteración respetar `--jobs`, pero no solapar dos iteraciones.
  Registrar `iteration: 1..N`, `round: 0` y `unit: null`; cualquier intento no
  `passed` mantiene exit `1` bajo `N > 1` aunque otra iteración pase; `N = 1`
  conserva exactamente la policy ordinaria. Probar revocación de procesos,
  secretos y recursos, cero virtual, registros y presupuestos nuevos,
  determinismo del replay y ausencia de un modo implícito o
  `allow-repeat-flaky`.

  Evidencia implementada en `crates/tondo-compiler/src/test_repeat.rs`: policy
  finita y validación de combinaciones, contexto canónico y campaña secuencial
  sobre workers frescos con reportes por iteración. `docs/contracts/test-repeat.md`
  fija los campos `iteration/round/unit`, la política de salida y la ausencia
  de estado virtual o recursos entre iteraciones; los tests cubren cleanup,
  skips, fallos, virtual time y no solapamiento.

- [x] **UTEST-ARTIFACT-001 — Persistir attachments por intento.** Implementar
  `testing.attach` con copia exacta y linealizada de `std.bytes.Bytes`, gramática
  cerrada de nombre/media type, unicidad por intento, límites y `P2006`. Calcular
  descriptors SHA-256 y escribir `tondo-test-artifacts-0.1/1` con objects
  content-addressed inmutables, deduplicación, manifest canónico y publicación
  atómica. Rechazar symlinks, paths derivados incorrectos, duplicados y
  colisiones; no incluir Base64, upload, ejecución, timestamps ni paths físicos.
  Mantener blobs huérfanos fuera del store lógico tras interrupción y permitir
  su recolección segura.

  Evidencia implementada en `crates/tondo-compiler/src/test_artifacts.rs`:
  blobs SHA-256 inmutables, deduplicación, manifiesto canónico y publicación
  atómica, límites `P2006`, validación de symlinks/escapes y GC seguro de
  huérfanos. `docs/contracts/test-artifacts.md` fija la ausencia de paths
  físicos/Base64/timestamps; los tests cubren límites, colisiones, orden,
  atomicidad y reclamación.

- [x] **UTEST-SNAPSHOT-001 — Implementar snapshots textuales explícitos.**
  Resolver `(node_id, name)` contra `tondo-snapshot-store-0.1/1`, comparar el
  `String` exacto, registrar `matched/missing/mismatched` y producir
  `P2007`/`P2008` con diffs humanos acotados. Parsear, validar, ordenar y
  hashear el store canónico; preservar entries stale y stores de paquetes no
  seleccionados y rechazar symlinks o escapes de package root.
  `--update-snapshots` stagea `created/updated`, exige jobs uno y orden canónico,
  rechaza shard/retry/repeat/allow-flaky y solo publica por reemplazo atómico
  tras una invocación completa sin otros estados no exitosos. Probar
  `snapshot_policy.published: false`, ausencia de update/borrado implícito y
  separación de registros entre intentos.

  Evidencia implementada en `crates/tondo-compiler/src/test_snapshots.rs`:
  store canónico ordenado/hasheado, checks exactos con diff acotado,
  `P2007/P2008`, stage de created/updated, preservación de stale entries,
  policy serial/canónica y publicación atómica con validación de symlinks y
  escapes. `docs/contracts/test-snapshots.md` documenta el contrato; los tests
  cubren parseo, matching, no-update implícito, separación del stage y rutas.

- [x] **UTEST-INTERRUPT-001 — Cerrar la interrupción externa.** En la primera
  señal, detener dispatch, cancelar cooperativamente, conducir cleanup
  incluyendo `defer` durante el grace period y revocar secretos,
  procesos, handles y recursos; una segunda señal puede forzar terminación.
  Emitir exit `4` solo si se restauró aislamiento y exit `3` en caso contrario.
  No publicar JSON, JUnit, manifest de artifacts ni snapshot update parcial;
  cada output final conserva sus bytes anteriores o permanece ausente. Permitir
  únicamente blobs content-addressed huérfanos y una salida humana marcada
  `interrupted`, nunca un resultado machine-readable presentado como completo.
  La tarea implementa y prueba la transacción coordinator/worker mediante un
  evento de interrupción inyectable después de cerrar stores y reporters; el
  mapping de señales del SO y su prueba pública quedan en `UTEST-CLI-001`.
  Evidencia implementada en `crates/tondo-compiler/src/test_interrupt.rs` y
  `docs/contracts/test-interrupt.md`: la primera solicitud corta dispatch y
  staging, exige ACK de cleanup/revocación antes de cerrar workers, usa el
  grace period de `LimitProfile`, separa exit `4` de pérdida de aislamiento en
  exit `3`, conserva outputs previos y restringe huérfanos a hashes
  content-addressed. Seis tests cubren la ruta segura, expiración, segunda
  solicitud, ACK incompleto, ledger de outputs y validación de reloj/identidad.

- [x] **UTEST-REPORT-001 — Implementar los formatos machine-readable.**
  Implementar una sola vez `tondo-test-json-v1` y serializar con ella
  `tondo-test-report-0.1/7` y `tondo-test-list-0.1/6`, con arrays separados de
  suites/tests, parents, source, owners, paths, estado agregado,
  intento decisivo e historial por intento de phase, `blocked_by` causal,
  iteración/ronda/unidad, `failure`, `skip`, tags, artifacts, snapshots,
  dominios `virtual_time`, logs y streams.
  Incluir por dominio índice, `elapsed_ns` decimal sin pérdida y contadores de
  avance automático/explícito/settle; incluir además policy, ownership, inputs
  públicos/perfil secreto, selector incluido glob, shard, order, seed,
  algoritmos, `execution_plan`, retry, repeat, artifact store, snapshot policy,
  resource profile y las invariantes exactas de summary/attempts y sus
  contadores de evidencia. No añadir valores secretos desde el materializador
  de inputs ni incluir bytes de attachments, valores completos de snapshots,
  reloj o duración wall-clock, PID, paths físicos ni direcciones; un programa
  que copie un secreto a un canal observable conserva la advertencia de la
  spec.
  `--test-format json` y `--report json=path` producen bytes idénticos para la
  misma ejecución. Fallos de compilación continúan usando diagnostics
  estructurados, no consumen intentos y no ejecutan setup ni bodies.
  Evidencia implementada en `crates/tondo-compiler/src/test_report.rs` y
  `docs/contracts/test-report.md`: `tondo-test-json-v1` serializa reportes y
  listas compactas con LF único, metadata de invocación, nodos separados y
  summary derivado; el parser rechaza deriva canónica, secretos, referencias
  rotas, source classes inconsistentes y payloads no válidos. Ocho tests
  unitarios cubren round-trip, canonicalización, lista descriptor-only,
  policies, identidad y rechazo de schema.

- [x] **UTEST-JUNIT-001 — Exportar JUnit desde el resultado normativo.**
  Proyectar la misma ejecución como `tondo-junit-report-0.1/4`, XML 1.0 UTF-8,
  con un testcase agregado por hoja, testcases sintéticos únicos para fallos de
  lifecycle y flaky suite, y
  `tondo.retry/repeat/decisive_attempt/attempts`. Mapear `flaky-pass` a failure
  por default y omitir solo ese outcome bajo `--allow-flaky`, sin cambiar
  identidad, conteo ni historial; repeat con count mayor que uno permanece rojo
  si cualquier oportunidad no pasa. Proyectar streams decisivos, scalars no
  representables, carrier vacío, owners, perfil de inputs sin valores secretos,
  shard, order, seed,
  artifact/snapshot descriptors, policy, `tondo.virtual_time`, conteos por
  identidad y duración real sumada por intentos. No embeber bytes ni valores de
  snapshot. Publicar cada path atómicamente, rechazar colisiones y mantener JSON
  como representación canónica y sin pérdida. Evidencia implementada en
  `crates/tondo-compiler/src/test_junit.rs` y
  `docs/contracts/test-junit.md`: el proyector genera XML `/4` determinista,
  conserva las properties `tondo.*`, emite casos sintéticos de lifecycle,
  flaky y plan vacío, aplica la política de repeat-instability, escapa XML 1.0
  y comprueba tiempos por intento con aritmética checked. Seis tests unitarios
  cubren metadata/duración, estados y repeat, lifecycle/flaky, plan vacío,
  errores de timings y scalars XML.

- [x] **UTEST-BACKEND-001 — Conectar una hoja `test` al backend VM.** El
  driver expone `Operation::Test`, descubre el ID seleccionado, inyecta solo
  el `main` privado de la hoja y conserva imports, declaraciones normales y
  setup de suites. El resultado atraviesa resolver, HIR, MIR, bytecode y VM;
  no existe una ruta de ejecución basada en callbacks Rust. Assertion failures,
  panics, límites y un `main` de producción conservan diagnósticos y exits
  normativos. Evidencia en `crates/tondo-compiler/src/test_backend.rs`,
  `crates/tondo-compiler/src/driver.rs` y
  `docs/contracts/test-backend.md`.

- [x] **UTEST-CLI-BACKEND-001 — Conectar el runner mínimo al backend.** `tondo
  test` materializa el proyecto cerrado, descubre hojas del
  paquete raíz, aplica selección all/filter/glob/exact, shard, order y jobs,
  ejecuta cada hoja en un worker nuevo sobre `Operation::Test`, ensambla el
  resultado canónico y publica JSON/JUnit de forma atómica. `--list`,
  `--show-output`, `--allow-empty`, `--deny-skips` y exits 0/1/2/3 quedan
  cubiertos. Esta base también materializa campañas reales de retry/repeat,
  conserva evidencia por intento, resuelve CODEOWNERS y publica el store de
  attachments content-addressed. La cola de timeout wall-clock y snapshots
  queda cerrada por `UTEST-CLI-001` con defaults en memoria, sidecar opcional y
  workers de proceso.

- [x] **TOOLCHAIN-PROJECT-001 — Hacer la CLI convention-first.** `check`,
  `run` y `test` aceptan el directorio actual o `--project <dir>` sin usar
  manifiestos JSON. La CLI descubre `src/`, `src/main.to`, `tests/` y módulos por
  paths ordenados; lee el `tondo.toml` opcional para package/edition,
  target/profile/capabilities/features y aliases, y materializa un grafo
  interno equivalente al `ProjectPlan` cerrado. Un `tondo.lock.toml` generado
  es obligatorio para dependencias externas y los proyectos sin dependencias
  usan un lockfile equivalente en memoria. `tondo.test.toml` es el sidecar
  humano único; no existe un sidecar JSON. Symlinks,
  `target/`, `vendor/` y directorios ocultos no entran en discovery. La
  representación JSON interna queda privada al compilador y no es una ruta
  de configuración. Evidencia en
  `crates/tondo-cli/src/project_discovery.rs`, `main.rs`,
  `docs/contracts/project-discovery.md` y las pruebas CLI de proyecto/TOML.

- [x] **UTEST-CLI-001 — Conectar `tondo test` end-to-end.** `tondo test`
  materializa un plan canónico opinionado desde el proyecto cuando no existe
  sidecar; `--test-plan <path>` o un `tondo.test.toml` adyacente seleccionan un
  plan avanzado, cuyos hashes de proyecto y forma canónica se verifican antes
  de compilar. Los overrides efímeros de CLI permiten selector, shard,
  orden/seed, jobs, retry, repeat y outputs sin editar el TOML, pero no pueden
  ampliar límites ni capabilities. Cada hoja se ejecuta en un proceso worker nuevo; el
  coordinator importa evidencia y aplica el timeout wall-clock monotónico con
  terminación del proceso, incluidos retry y repeat. Las snapshot stores se
  cargan como inputs cerrados; `--update-snapshots` usa una
  `SnapshotUpdateStage` y rename atómico solo tras una campaña completamente
  verde. El artifact store del plan base fija el formato y el límite, mientras
  `--artifacts` solo puede reubicar físicamente la salida. Un `--timeout none`
  explícito se rechaza frente al límite cerrado. JSON/JUnit, artifacts,
  CODEOWNERS, selección, shards, orden, jobs y exits 0/1/2/3 quedan cubiertos
  por tests unitarios e integración. El plan de usuario solo admite TOML;
  JSON queda restringido a informes y fronteras internas.
  Evidencia en `crates/tondo-cli/src/main.rs`,
  `crates/tondo-compiler/src/test_control.rs`,
  `crates/tondo-compiler/src/test_runtime.rs` y los contratos de
  `test-cli-plan`/`test-plan`. Las señales del SO siguen en
  `UTEST-INTERRUPT-001`; no se implementan `--tag`, selector regex ni
  `--fail-fast` bajo este contrato.

### 17.4 Evidencia, conformidad y dogfooding

- [x] **UTEST-CONF-001 — Ampliar la conformidad del draft Tondo 0.1.** No
  presentar casos vecinos como evidencia de requisitos nuevos. El
  manifiesto draft añade los cincuenta y dos grupos mínimos enumerados por la
  spec de testing y mantiene adaptador público para VM y futuros backends.

- [x] **UTEST-PROJECTS-001 — Añadir proyectos de aceptación completos.**
  Incluir package unitario, integration roots, dev-dependency, suites anidadas,
  servicio compartido, captura válida/inválida, async/error, fallos de
  setup/teardown, `blocked-setup`, log directo/desde helper/task, `failNow`,
  tags directos/desde helper/task, conflicto `P2002`, skip de hoja/suite,
  `blocked-skip`, `P2001`, deny-skips, pánico/cleanup, host capabilities,
  CODEOWNERS, substring/glob/exact, selección vacía, shards, orden/seed,
  retries de hoja/setup/teardown, aislamiento externo idempotente,
  `flaky-pass`/allow-flaky, campañas repeat, backoff/deadline/debounce con tiempo
  virtual, quiescencia, deadlock/solapamiento/rango, cleanup mediante
  `defer`, inputs públicos/secretos y sus fallos de
  materialización/revocación, interrupción, attachments y snapshots en
  check/update mode, y reporters JSON/JUnit. Cada proyecto debe poder ejecutarse
  desde una copia en otro path físico con observaciones canónicas iguales salvo
  duración JUnit y material secreto deliberadamente externo.
  El corpus versionado `acceptance/projects/testing-acceptance` y
  `acceptance/projects/testing-control` cruza discovery convencional, unit e
  integration roots, suites anidadas, helper compartido, `std.testing`
  sellado, `failNow`, skip, logs, tags, CODEOWNERS, selección, shards, seed y
  reporters JSON/JUnit a través del binario real. La prueba copia el proyecto
  a dos raíces físicas y compara los bytes canónicos. Los modelos de lifecycle,
  retries/repeat, tiempo virtual, inputs, interrupción, artifacts/snapshots y
  sus fallos quedan enlazados por los 52 grupos de `UTEST-CONF-001`; el host VM
  tiene además cobertura directa de logs, tags, attachment y snapshot, y
  `std.testing` permanece ausente de producción.

- [x] **UTEST-PLATFORM-001 — Validar la matriz declarada.** Linux ejecuta el
  gate canónico completo; Linux ARM64, macOS Intel/ARM64 y Windows ejecutan
  discovery, paths jerárquicos, substring/glob/exact de suite/test, lifecycle,
  envelopes, tags/logs/skips, CODEOWNERS, sharding, orden/seed, workers nuevos
  de retry y repeat, reloj virtual y ties de timers deterministas, cleanup async,
  inputs, interrupción, artifacts/snapshots, aislamiento, timeout, captura y
  reportes JSON/JUnit aplicables además del smoke test de binario.
  `.github/workflows/test.yml` conserva el gate estricto en Linux x86_64 y
  ejecuta `scripts/platform-test.sh` sobre runners nativos Linux ARM64, macOS
  Intel/ARM64 y Windows x86_64. El gate portable recorre todos los tests Rust,
  incluidos los contratos de lifecycle, scheduler, reloj virtual, inputs,
  captura y reporters, y después ejecuta el proyecto público de aceptación con
  seed fija mediante el binario de cada plataforma. Los resultados JSON/JUnit
  se validan como no vacíos y se retienen como artifacts identificados por
  target; el mismo binario debe superar además `--version` y Hello World.

- [x] **UTEST-DOGFOOD-001 — Probar componentes Tondo mediante `tondo test`.**
  Antes de Gate T0, mantener una pequeña biblioteca de aceptación escrita en
  Tondo con unit/integration tests y al menos una suite que comparta un recurso
  real. Debe usar `testing.log` y `testing.tags` desde helpers, probar
  `failNow`/skip en casos de aceptación controlados y ejecutar el mismo corpus
  repartido en shards con seed registrada y un retry aislado determinista que
  ejercite `flaky-pass` mediante una fixture externa controlada, sin depender de
  timing y con cleanup final verificable. Debe ejecutar además una campaña
  repeat determinista, adjuntar al menos un artifact, comprobar y actualizar un
  snapshot textual en un fixture aislado, y levantar/cerrar mediante
  `defer` un servicio de integración. Antes de estabilizar
  `std.process`/`std.net`, ese servicio se implementa enteramente en Tondo sobre
  tasks y la API pública del paquete probado; el dogfood hosted con proceso o
  red reales se añade en STD-0.1A/B y no autoriza un shim de testing. Debe
  probar una API de producción
  con backoff o deadline mediante `withVirtualTime`, sin pausas reales, y
  producir reportes JSON/JUnit con inputs, stores y tiempo virtual separado de
  duración operacional. No sustituye los tests Rust ni la conformidad;
  demuestra que la experiencia pública funciona sin harness privado. Cerrado
  con los tres proyectos bajo `acceptance/projects/`: el corpus real recorre
  unit e integration tests, suites, control, evidence, shards, retry, repeat y
  reporters a través del binario; `integration_root` prueba la API pública de
  producción `answerAfterBackoff` con `withVirtualTime`, `settle` y 25 ns
  virtuales, y la aceptación exige esa evidencia tanto en JSON como en JUnit.

- [x] **UTEST-SPEC-EVIDENCE-001 — Cerrar la trazabilidad completa del spec de
  testing.** Los 38 fences que el inventario clasifica hoy como
  `draft-contract` deben mapearse a casos públicos existentes, convertirse en
  aceptación ejecutable o declararse ilustrativos/no normativos con una razón
  individual. La matriz multi-spec debe demostrar cada contrato aplicable de
  `TONDO_TESTING_SPEC.md`; el número de grupos del manifest no sustituye esa
  correspondencia. La matriz viva clasifica cada requisito `TT01` y exige
  evidencia ejecutable o una razón normativa individual para cualquier no-goal.

### Gate T0 — Testing first-class conforme

- [x] El corpus vivo, su manifest y sus observaciones se regeneran y validan
  contra el mismo borrador que consume el compilador.
- [x] El borrador consolidado Tondo 0.1 incorpora el contrato de testing,
  reserva `suite` y `test` y define `defer` con inferencia de cleanup,
  infallible y verificable sin añadir hooks de testing ni un segundo dialecto.
- [x] Lexer, CST, parser, formatter, HIR, MIR, bytecode y VM recorren la ruta
  común y sus verifiers aceptan o rechazan árboles suite/test con diagnostics
  exactos.
- [x] Unit overlays ven privados sin alterar producción; integration roots solo
  ven API pública; `std.testing`, dev-dependencies y operaciones test-only nunca
  entran en productos publicables.
- [x] Cada entrada recibe un envelope no observable ni falsificable que sigue
  frames/tasks y nunca se deriva de un thread-local del host; tags, logs y
  terminales se atribuyen al nodo exacto sin `TestContext` ni `currentTest()`.
- [x] Suites ejecutan setup una vez por participación solo para subárboles
  seleccionados, permiten únicamente capturas `let: Copy + Send + Share`,
  hacen teardown tras todos los descendientes y reportan setup, teardown,
  `blocked-setup`, skip y `blocked-skip` sin duplicar causas.
- [x] Retorno, error, `assert`, `failNow`, skip, pánico, async, cancelación,
  ownership, `defer` y `defer` conservan cleanup y precedencia; `P2001`,
  resource limits, timeout e interrupción no esconden cleanup observado ni
  rompen aislamiento.
- [x] Inputs públicos se fijan por bytes/hash y secretos solo por descriptor;
  ningún valor secreto entra en productos, cache keys, reportes o stores
  implícitos, y cada worker materializa y revoca únicamente lo declarado.
- [x] `Duration`, `Instant`, suspensión, timers y deadlines usan el sustrato
  monotónico de producción; `withVirtualTime` lo sustituye solo dentro de su
  closure, presta un controlador no escapable y conserva el timeout real.
- [x] Quiescencia durable, `settle`, avance explícito/automático, cola y ties de
  timers son deterministas; esperas externas no se virtualizan y `P2003`,
  `P2004` y `P2005` conservan sus condiciones exactas.
- [x] `tondo test` implementa discovery, compilación completa, selección,
  substring/glob/exact de suite/test, CODEOWNERS, sharding estable, orden/seed,
  ejecución serial/paralela, retries aislados por rondas, repeat aislado por
  iteraciones, captura, artifacts, snapshots, reporters, interrupción y exit
  codes deny-skips/allow-flaky/empty según contrato; no inventa regex, filtrado
  por tags, retries/repeat implícitos ni fail-fast.
- [x] Cada retry reutiliza solo el artefacto inmutable, arranca un worker nuevo,
  conserva shard/configuración, respeta el máximo global de jobs y deja
  procesos, recursos rastreados, heap, roots, tasks, handles, envelopes y
  buffers sin supervivientes; cada dominio virtual vuelve al mismo cero.
- [x] Cada iteración repeat ejecuta el plan completo en worker nuevo, no se
  solapa con otra iteración y, con count mayor que uno, mantiene exit rojo ante
  cualquier non-pass aunque otra oportunidad del mismo nodo pase; count uno
  conserva la policy ordinaria.
- [x] Attachments y snapshots pertenecen al intento exacto; los stores `/1`
  son canónicos, acotados y atómicos, y snapshot update nunca se activa ni
  elimina entries de forma implícita.
- [x] Una interrupción deja de despachar, intenta cleanup/revocación y usa exit
  `4` o `3` según aislamiento; nunca publica reportes, manifests o updates
  parciales como completos.
- [x] El reporte JSON `/7` es canónico y reproducible, conserva todos los
  intentos, iteraciones, dominios y descriptors sin material secreto añadido
  por el runner ni payloads externos embebidos; JUnit `/4` proyecta la misma
  ejecución y policy con duración operacional real y tiempo virtual separado.
  La salida humana no intercala suites/tests o intentos y muestra owners, tags,
  evidence, tiempo virtual, logs, razones y fallos accionables.
- [x] El grupo de testing de `tondo-conformance-draft` pasa en la VM, la matriz
  de plataformas aplicable está verde y `UTEST-SPEC-EVIDENCE-001` demuestra
  todos los contratos normativos de testing sin `draft-pending`.
- [x] Existe dogfooding escrito en Tondo que usa la superficie pública, sin
  registration APIs, `TestContext`, annotations, reflection, subtests dinámicos
  ni hooks ocultos.

La implementación funcional y el gate T0 vivo están completos. La suite
completa —incluida metaprogramación— alimenta el resultado compuesto del árbol
actual; el primer candidato G5 se construirá únicamente al preparar la release.

---

## 18. M10.7 — Metaprogramación estática y reflection

**Objetivo:** implementar la superficie añadida al draft Tondo 0.1 sin abrir el
frontend a ejecución arbitraria. El resultado debe soportar `derive`, generators
de manifest y metadata reflection con código estático, reproducible e
inspeccionable.

**Dependencia:** Gate H0, `CONF-DRAFT-001` y el corpus vivo de M10.
`META-FORMAT-001` abre el trabajo compartido. Syntax y modelo pueden avanzar
mientras se cierra `STD-META-SPEC-001`. `META-VM-001` crea después el target
cerrado necesario para ejecutar `STD-META-IMPL-001`; esta implementación espera
también a `META-MODEL-001`. Providers y generators no pueden comenzar hasta
cerrar además `STD-META-CONF-001`.
`PARSER-STACK-001` precede a `META-SYNTAX-001`, para no construir las nuevas
formas sobre la recursión temporal del bootstrap.
`STD-REFLECT-001` precede obligatoriamente a `REFLECT-IMPL-001`. Ninguno de
estos contratos espera al resto de STD-0.1A ni introduce un shim provisional.
La contribución de M10.7 y M10.6/T0 forma parte de la conformidad viva. Cada
cambio pre-release actualiza spec, corpus, manifest, tests y evidencia en el
mismo árbol; no se mantiene una versión anterior en paralelo.

**Orden interno:**

1. `META-FORMAT-001` y el linaje único del draft.
2. En paralelo: `META-SYNTAX → META-SEM → META-MODEL`,
   `STD-META-SPEC-001 → META-VM-001` y `STD-REFLECT-001`.
3. Bootstrap meta: `(META-MODEL-001 + META-VM-001) →
   STD-META-IMPL-001 → STD-META-CONF-001`.
4. Unión: `META-DERIVE-001` y `META-GEN-001`.
5. Integración: `META-ATOMIC-001`, `META-QUERY-001` y `REFLECT-IMPL-001`.
6. Evidencia incremental y cierre: diagnostics, reproducibilidad, robustez y
   `META-CONF-001`.

Cada unión ejecuta `CONF-RATCHET-001`; la tarea final no descubre por primera
vez los errores de los slices anteriores.

### 18.1 Contrato y formatos

- [x] **META-SPEC-001 — Fijar el modelo del lenguaje.** Reservar `derive`,
  gramática, ownership del target, providers exactos, autorización privada
  limitada, una ronda, `tondo-meta-model-0.1/1`, sandbox, presupuestos,
  identidad/cache, diagnostics `E2101`–`E2109` y frontera de reflection.

- [x] **META-TOOLCHAIN-SPEC-001 — Fijar el plan de generación.** Fijar los
  formatos draft de manifest, lockfile, interface y artifact; separar frontend puro de
  orquestador; separar los grafos runtime/meta; declarar programs, inputs,
  roots, outputs, límites, hashes, target `tondo-meta`, fusión atómica y
  ausencia de outputs parciales.

- [x] **REFLECT-ARCH-001 — Fijar reflection metadata-only.** `std.reflect`
  retiene `TypeInfo`/`TypeId` solo por solicitud estática, expone únicamente
  estructura pública y no ofrece `Any`, value access, constructors, invocation,
  private members, layout ni enumeración global.

- [x] **META-FORMAT-001 — Implementar formatos toolchain draft.** Parsear,
  validar y canonicalizar los records draft y el descriptor estándar draft;
  rechazar campos, providers, meta packages, roots, límites, paths, outputs y
  hashes inconsistentes antes de ejecutar código.
  El lector actual acepta solo estos records; el corpus vivo se mantiene
  fuera de esta frontera como regresión. Cerrado con
  `crates/tondo-compiler/src/toolchain.rs`, `ProjectPlanDraft::parse`, contrato
  `docs/contracts/toolchain-formats-draft.md` y tests de round-trip, canonicalidad,
  grafos, hashes y outputs.

### 18.2 Frontend y modelo semántico

- [x] **META-SYNTAX-001 — Implementar `derive` end-to-end en syntax.** Lexer,
  CST sin pérdida, parser, recuperación, formatter canónico, documentación y
  AST/HIR soportan una única declaración sin attributes ni body. La forma
  `derive [T] Trait + Trait for Target` usa nodos `DeriveDecl`,
  `DeriveTraitList` y `DeriveTarget`, conserva trivia y normaliza espacios sin
  introducir una segunda sintaxis; la resolución semántica permanece en
  `META-SEM-001`.

- [x] **META-SEM-001 — Validar solicitudes derive.** Resolver identidades
  exactas de traits/providers, owner nominal, binders, duplicados, superficie
  permitida, bounds generados, coherencia y conflictos con impls manuales.
  `std::meta::validate_derive_requests` produce un plan determinista y
  all-or-nothing; `validate_hir_derive_requests` conserva spans y no ejecuta
  providers hasta las fases posteriores.

- [x] **META-MODEL-001 — Construir el snapshot meta inmutable.** Serializar de
  forma canónica únicamente la clausura de roots autorizada: módulos,
  declaraciones, tipos, bounds, fields/variants públicos, spans y docs; entregar
  al derive solo la vista privada del target autorizado. Excluir bodies,
  valores, layout, direcciones y estado del GC; diagnosticar `E2109` si una
  clausura requiere una salida de la misma ronda. Cerrado con el modelo privado
  `MetaSnapshot` de `crates/tondo-compiler/src/meta.rs`, JSON/hash canónicos,
  validación de duplicados/orden/UTF-8 y round-trip que audita la ausencia de
  datos ejecutables o de runtime.

- [x] **META-QUERY-001 — Exponer expansiones y procedencia.** Tooling devuelve
  fuente formateada, provider, request/output hashes, bounds introducidos y
  source map sin revelar símbolos privados ajenos al target.

### 18.3 Ejecución hermética

- [x] **META-VM-001 — Implementar el sustrato target/VM `tondo-meta`.**
  Registrar target, loader y sandbox capaces de ejecutar un programa Tondo
  mínimo con heap nuevo por run, cero capabilities y contadores deterministas
  de steps, memoria viva y output. Esta tarea no embebe una API provisional:
  `STD-META-IMPL-001` compila después el companion especificado sobre este
  sustrato, y solo entonces se habilitan providers.

- [x] **META-DERIVE-001 — Ejecutar providers derive.** Pasar requests tipados,
  limitar outputs al impl autorizado, validar y formatear fuente, y fusionarla
  solo cuando todos los providers terminan correctamente.

- [x] **META-GEN-001 — Ejecutar generators de manifest.** Entregar únicamente
  inputs declarados por valor y la clausura pública de roots explícitos, exigir
  todos y solo los outputs cerrados, impedir lectura ambiental, generación
  multi-round y observación de outputs hermanos.

- [x] **META-ATOMIC-001 — Integrar identidad, cache y productos.** Incluir
  model/provider/request/output hashes en interfaces y artifacts; reutilizar
  cache solo con identidad completa y no publicar fuente, interface o artifact
  parcial ante fallo.

- [x] **REFLECT-IMPL-001 — Implementar metadata runtime alcanzable.** Generar
  metadata de `typeInfo[T]()` estáticamente, eliminar la no alcanzable y
  demostrar que `TypeId` no escapa como identidad de wire ni habilita value
  reflection.

### 18.4 Evidencia y contribución a Gate G5

- [x] **META-DIAG-001 — Cubrir `E2101`–`E2109`.** Cada error tiene vecino
  positivo, precedencia, span/ubicación nula correcta, JSON estable y
  diagnostics de provider asociados a inputs o fields relevantes.

- [x] **META-REPRO-001 — Probar hermeticidad y determinismo.** Variar cwd,
  environment, filesystem order, core count y scheduling; repetir builds,
  comparar outputs byte a byte y demostrar denegación de filesystem, red,
  process, clock, entropy, threads, async, FFI y unsafe.

- [x] **META-ROBUST-001 — Añadir properties, fuzzing y límites.** Fuzzear
  records draft y revisiones de schema, modelo meta, outputs y source maps; probar cycles imposibles,
  roots que cruzan la frontera de ronda, colisiones, pánicos, budget exhaustion,
  UTF-8 inválido y generadores hostiles sin panic del compilador ni publicación
  parcial.

- [x] **META-CONF-001 — Extender `tondo-conformance-draft`.** Añadir syntax,
  semantic, tooling, runtime metadata, toolchain y reproducibility cases en la
  línea draft creada por `CONF-DRAFT-001`, sin presentar la regresión bootstrap
  como conformidad completa. Ratchetear su contribución acumulada solo después de actualizar
  inventario, trazabilidad, coverage y mutation evidence para la superficie
  nueva; el sellado del primer artefacto publicable pertenece exclusivamente a
  `CONF-SEAL-FINAL-001`.

### Gate G5 — Primer candidato completo del lenguaje

- [x] Todo el draft Tondo 0.1, incluidos M10.7 y M10.6, está implementado y
  tiene conformidad aplicable sobre `tondo-vm-hosted`; la matriz multi-spec no
  conserva límites aplicables ni contratos pendientes.
- [x] Gate T0 está cerrado y el grupo de testing forma parte de
  `tondo-conformance-draft`, no de una edición o suite paralela.
- [x] `tondo doc-test` aplica el contrato completo de 21.6 y valida los
  ejemplos normativos sin harness paralelo ni resultados parciales.
- [x] La suite y su manifest fijan el hash actual de la spec y no conservan un
  snapshot pre-release paralelo.
- [x] No existe una ruta de ejecución ambiental dentro del frontend ni del VM
  meta.
- [ ] `CONF-GAP-AUDIT-001`, cualquier leaf de `CONF-GAP-IMPL-001`,
  `CONF-LAYER-RESULT-001`, `QUALITY-EVIDENCE-BIND-001` y
  `CONF-SEAL-FINAL-001` están cerrados; solo entonces la distribución puede
  describirse como candidata a publicación. El gate no publica por sí solo.

---

## 19. STD-0.1A — Foundation + Hosted Standard Library

**Objetivo:** especificar e implementar la primera API estándar utilizable
sobre la VM antes de fijar decisiones del runtime nativo. La especificación de
la stdlib es independiente de la especificación del lenguaje; una API
ilustrativa no se vuelve pública por aparecer en un ejemplo.

La arquitectura, identidad, catálogo y reglas comunes están fijadas en
[`TONDO_STANDARD_LIBRARY_SPEC.md`](./TONDO_STANDARD_LIBRARY_SPEC.md). Cerrar esa
base no cierra ninguna firma de módulo salvo el núcleo sellado que ya pertenece
a la especificación de testing.

La fase A contiene métodos intrínsecos, `std.bytes`, `std.io`, `std.math`,
`std.format`, `std.serialization`, `std.reflect`, `std.meta`, JSON,
MessagePack, Protobuf, el sustrato monotónico de `std.time`, `std.path`,
`std.console`, `std.env`, `std.fs`, `std.process` y `std.testing`. La fase B
completa el mismo catálogo 0.1 con calendario civil, encodings y codecs
adicionales, regex, UUID, canales, sincronización, executors, logging y red.

**Dependencia:** tras H0 y `CONF-DRAFT-001` se adelantan cinco slices exactos,
sin abrir el resto de la stdlib:

- `STD-META-SPEC-001` antes de `META-VM-001`, y su implementación/conformidad
  antes de providers o generators;
- `STD-REFLECT-001` antes de `REFLECT-IMPL-001`;
- `STD-BYTES-SPEC-001 → STD-BYTES-IMPL-001 → STD-BYTES-CONF-001` ya está cerrado;
  su identidad se reutiliza en typecheck y artifacts de testing;
- `STD-TIME-BASE-SPEC-001 → STD-TIME-BASE-IMPL-001 →
  STD-TIME-BASE-CONF-001` antes de tiempo virtual;
- `STD-ENV-SPEC-001 → STD-ENV-IMPL-001 → STD-ENV-CONF-001` antes de
  materializar y leer inputs declarados.

El resto de la implementación pública comienza después de T0, aunque sus specs
pueden avanzar por layers cuando sus owners anteriores estén cerrados. Ninguna
firma se congela ni distribuye como estable sin modelos, tests y contrato de
capability. Junto a esos slices, M10.6 implementa el núcleo test-only
`std.testing.log/tags/failNow/skip/attach/snapshot/withVirtualTime`, cuyas firmas
y bridge quedan fijados y ejecutables en T0 porque forman parte del contrato del
runner; no constituye un sexto módulo estándar adelantado. `defer` es
semántica general de Tondo 0.1, no una API de ese módulo. STD-0.1A completa esos
mismos módulos; no crea reloj, snapshot engine, artifact store ni harness
paralelos.

**Layers de implementación:**

1. **A0 — Prerrequisitos:** meta, contrato reflect, bytes, time-base y acceso
   environment de solo lectura necesario para inputs declarados.
2. **A1 — Valores y protocolos:** core, texto, colecciones, iteradores, math,
   format e I/O.
3. **A2 — Host:** path → console/env → filesystem/process.
4. **A3 — Datos:** serialization → JSON/MessagePack; Protobuf añade además meta
   schema-first.
5. **A4 — Experiencia de test:** helpers portables de `std.testing` sobre T0.
6. **A5 — Cierre:** performance, conformidad, documentación y distribución.

Cada módulo atraviesa `SPEC → IMPL/HOST → MODEL/TEST/FUZZ → PERF → CONF → DOC`
antes de considerarse listo. Las tareas umbrella de 19.4 coordinan y verifican
esos micro-gates; no sustituyen el estado individual de un módulo. Un owner
solo bloquea a sus consumidores explícitos: owners independientes de una misma
layer pueden avanzar en paralelo.

| Orden A | Owners | Dependencias duras | Desbloquea |
|---|---|---|---|
| A0.1 | `std.meta` | formatos draft, modelo meta normativo y target meta bootstrap | derive y generators |
| A0.2 | `std.reflect` contract | modelo de tipos público | metadata runtime |
| A0.3 | `std.bytes` | tipos intrínsecos existentes | attachments, I/O y codecs |
| A0.4 | `std.time` monotónico | async/executor VM existente | tiempo virtual y deadlines |
| A0.5 | `std.env` read-only | `String` intrínseco + `std.bytes` + capability `environment` | inputs públicos/secretos de workers |
| A1.1 | core, text, collections, iterator, math | lenguaje ya conforme | protocolos portables |
| A1.2 | format | `Display`, core/text | diagnostics, log y serializers |
| A1.3 | I/O | bytes + error/async contracts | console, fs, process y streaming |
| A2.1 | path | text/bytes | filesystem y paths host |
| A2.2 | console + resto de env | format + I/O; path donde aplique | programas hosted |
| A2.3 | filesystem/process | bytes + I/O + path + time-base | corpus real y dogfooding |
| A3.1 | serialization | core + format + I/O contracts | codecs tipados |
| A3.2 | JSON/MessagePack | serialization + bytes + I/O | datos runtime |
| A3.3 | Protobuf | serialization + bytes + I/O + meta | schema-first generado |
| A4 | helpers `std.testing` | T0 + owners anteriores que use cada helper | dogfooding completo |
| A5 | integración | todos los micro-gates | S1A y corpus de M11 |

### 19.1 Contrato y distribución

- [x] **STD-FOUNDATION-SPEC-001 — Crear la especificación base de la stdlib.**
  Fijar relación con lenguaje/toolchain/testing, versionado, PackageId,
  convivencia con bootstrap, namespace y prelude, propietario canónico,
  catálogo de módulos, availability/capabilities, forma de API, errores,
  ownership, async, determinismo, costes, bindings privilegiados, distribución
  reproducible, conformidad y checklist de publicación. Mantener pendientes las
  firmas concretas de módulos y no anunciar STD-0.1 como publicada.

- [x] **STD-TIME-BASE-SPEC-001 — Extender la especificación estándar con el
  sustrato temporal mínimo.** `std.time` fija en STD-0.1A `Duration` firmado con
  quantum de nanosegundo y overflow explícito, `Instant` monotónico, consulta
  síncrona, `sleep`, timer one-shot y deadlines representados por `Instant`.
  El contrato declara ownership (`Copy`/`Send`/`Share` para valores y handles
  afines para timers), resolución, identidad opaca de proveedor y dominio,
  rechazo de mezcla, cancelación cooperativa, puntos de suspensión, capability
  `clock`, errores y disponibilidad por target. El plan cerrado debe fijar con
  SHA-256 los source sets, interfaz, unidad privilegiada real y descriptor del
  proveedor virtual; no existe bridge ambiental ni segunda API de testing.
  Calendario/reloj civil queda separado en STD-0.1B. La implementación y la
  conformidad permanecen pendientes y esta tarea habilita M10.6 sin anunciar
  disponibilidad en `tondo-vm-hosted`.

- [x] **STD-SPEC-001 — Cerrar la integración de
  `TONDO_STANDARD_LIBRARY_SPEC.md`.** `testing/stdlib-spec.json` es ahora el
  catálogo machine-readable único de owners, source sets, dependencias,
  capabilities y reglas de API; `scripts/stdlib-spec-check.sh` comprueba orden
  topológico, contratos presentes, owner único, ausencia de aliases/defaults y
  enlace con el spec canónico. Es un cierre agregado y mantiene los codecs
  typed pendientes en los bloques A3 de implementación.

- [x] **STD-MOD-001 — Definir módulos y prelude mínimo.** El contrato base fija
  el catálogo cerrado, un propietario canónico por declaración, `std` único y
  reservado, imports ordinarios, cero inicialización global y ningún nombre
  implícito adicional ni extensión global de métodos.

- [x] **STD-CAP-001 — Fijar la matriz de capabilities.** El contrato base
  clasifica cada superficie como Core, capability-gated, test-only, build-only o
  target-specific, fija `tondo-capabilities-draft`, exige ausencia estática
  `E1008` y conserva el corpus vivo de `tondo-vm-hosted` como oracle con
  `[console, environment, process]`. Cada módulo pendiente deberá completar su matriz por
  target antes de publicarse.

- [x] **STD-ERR-001 — Definir errores portables.** El contrato base separa
  Option, Result, pánico y error de toolchain; exige errores públicos nominales,
  cerrados y con payloads portables; y excluye códigos, mensajes y payloads del
  SO de la semántica estable. Las variantes concretas permanecen en cada módulo.

- [x] **STD-DIST-001 — Definir distribución reproducible.** El contrato base
  fija versión, PackageId, content/API hashes, fuentes Tondo, source sets,
  interfaces, providers/generators, unidades privilegiadas, oracles,
  conformidad y documentación como una distribución inmutable, canónica,
  cerrada y sin red ni búsqueda ambiental durante compilación. Sus bytes finales
  se materializan al cerrar S1.

### 19.2 Core Standard Library

- [x] **STD-CORE-001 — Fijar protocolos y operaciones fundamentales.**
  `Option`, `Result`, `Display`, comparación, `Key` y utilidades de
  error conservan las capacidades y efectos ya definidos por el lenguaje.

- [x] **STD-TEXT-001 — Especificar texto.** `String`, `Char`, `Byte` y
  sus operaciones fijan construcción, búsqueda, transformación, Unicode,
  límites y costes sin confundir scalar, grapheme ni byte.

- [x] **STD-BYTES-SPEC-001 — Especificar `std.bytes`.** `Bytes`, builders,
  conversiones explícitas `Bytes(String)`/`String(Bytes)` y `Array[Byte]`,
  UTF-8 estricto, slicing, igualdad, hashing y límites tienen una única
  identidad binaria reutilizada por console,
  filesystem, procesos y testing. Base64, hexadecimal y codecs wire-format
  permanecen bajo sus módulos propietarios posteriores.

- [x] **STD-IO-001 — Especificar `std.io`.** Fijar protocolos estáticos de
  lectura/escritura, buffers, EOF, partial I/O, errores portables, ownership,
  backpressure, suspensión y cancelación sin que importar los contratos conceda
  ninguna capability. Console, archivos y procesos reutilizan esta única
  frontera en vez de inventar streams incompatibles.

- [x] **STD-MATH-001 — Especificar `std.math`.** Fijar las operaciones escalares
  portables que completan los numéricos intrínsecos, incluidos floor, ceil,
  round, truncate, sqrt y FMA explícita, conservando IEEE, ausencia de fast-math
  observable, dominio, errores y casos límite.

- [x] **STD-COLL-001 — Especificar colecciones.** `Array`, `Map` y `Set` fijan
  consulta, construcción, actualización funcional, mutación explícita,
  capacidad, orden, combinación y complejidad preservando semántica de valor.

- [x] **STD-ITER-001 — Especificar ranges e iteración.** `Range`, iteradores y
  combinadores usan dispatch estático, un único elemento por target, evaluación
  lazy acotada y consumo/copia visibles.

- [x] **STD-FMT-001 — Especificar `std.format`.** Display de tipos compuestos,
  builders y formato estructurado deben reutilizar el protocolo estático sin
  introducir reflection, vtables, lookup abierto ni una segunda interpolación.

- [x] **STD-SER-001 — Completar la especificación de
  `std.serialization`.** Cerrar las firmas de `Encode[C]`, `Decode[C]`,
  `Encoder[C, E]` y `Decoder[C, E]`, su máquina de eventos, derive
  format-neutral, `Value`/`ValueView`/`Raw`, bounds genéricos, construcción
  atómica, ownership, errores, límites y personalización mediante anotaciones
  compile-time/impl explícito. Cerrado con
  el contrato canónico [`docs/contracts/stdlib-serialization.md`](./docs/contracts/stdlib-serialization.md),
  su registro machine-readable y un validador iterativo de eventos que cubre
  arrays, maps con claves arbitrarias, records, fields, enums, variants,
  límites, duplicados y cierres terminales. La implementación typed de los
  codecs y el provider derive permanecen en `STD-SER-IMPL-001` y
  `STD-DERIVE-SER-001`.

- [x] **STD-REFLECT-001 — Especificar el contrato exacto de `std.reflect`.**
  Cerrar antes de `REFLECT-IMPL-001` `TypeInfo`, `TypeId`, kinds,
  descriptores, ownership y errores públicos; fijar los oracles de retención
  opt-in, DCE y ausencia de value access, private access, layout, global
  registry o identidad portable. La implementación y su evidencia runtime
  permanecen en M10.7.

- [x] **STD-META-SPEC-001 — Especificar `std.meta`.** Cerrar request/response,
  modelo inmutable, recorrido canónico, renderizado seguro, source builder,
  ownership, errores y ausencia de capabilities/callbacks antes de construir el
  target `tondo-meta`. Los providers de formatos concretos continúan en sus
  módulos posteriores. Cerrado con `docs/contracts/std-meta.md` y los records
  puros `MetaRequest`/`MetaResponse`/`MetaSourceBuilder` de `meta.rs`; el builder
  consume el request, valida UTF-8 y paths/módulos declarados, aplica límites y
  no publica respuestas parciales.

- [x] **STD-JSON-001 — Especificar JSON out of the box.** Ruta
  typed directa `Encode[Json]`/`Decode[Json]`, `Value`/`ValueView`/`Raw`,
  `JsonNumber`, reader/writer/eventos incrementales,
  UTF-8, duplicados, unknown fields, orden/canonical output, límites, errors con
  path y corpus interoperable; nunca materializar DOM para typed decode. Cerrado
  con [`docs/contracts/stdlib-json.md`](./docs/contracts/stdlib-json.md), el
  registro [`testing/stdlib-json.json`](./testing/stdlib-json.json) y el check
  [`scripts/stdlib-json-check.sh`](./scripts/stdlib-json-check.sh), integrado en
  `scripts/test-gate.sh`. El contrato exige dispatch estático, `JsonNumber`
  decimal exacto, policies estrictas por defecto, reader/writer con stack
  explícito y límites finitos; la conformidad RFC y la interoperabilidad quedan
  preparadas para `STD-CODEC-CONF-001`.

- [x] **STD-MSGPACK-001 — Especificar MessagePack out of the
  box.** Cubrir todo el data model y extension values, representación mínima,
  maps con keys arbitrarias, streaming, canonical mode, límites y
  interoperabilidad sin reflection. Cerrado como contrato del owner con
  [`docs/contracts/stdlib-messagepack.md`](./docs/contracts/stdlib-messagepack.md),
  el registro [`testing/stdlib-messagepack.json`](./testing/stdlib-messagepack.json)
  y el check [`scripts/stdlib-messagepack-check.sh`](./scripts/stdlib-messagepack-check.sh),
  integrado en `scripts/test-gate.sh`. El contrato fija `Value` común con
  maps de pares ordenados y claves arbitrarias, `Ext`/timestamp explícitos,
  typed dispatch sin DOM, reader/writer con stack acotado, policies de
  duplicados y extensiones, `Value` común y `encodeDeterministic` sin afirmar una
  canonicalización universal. La interoperabilidad queda preparada para
  `STD-CODEC-CONF-001`.

- [x] **STD-PROTOBUF-001 — Especificar Protobuf schema-first.**
  Generar desde `.proto` declarado tipos/codecs para proto3, presence, repeated,
  packed, maps, open enums con `Int32` preservado, nested y oneof; preservar
  unknown fields, comprobar evolución y ofrecer encoding determinista sin
  presentarlo como canonical universal. Services/gRPC quedan fuera. Cerrado
  como contrato del owner con
  [`docs/contracts/stdlib-protobuf.md`](./docs/contracts/stdlib-protobuf.md),
  el registro [`testing/stdlib-protobuf.json`](./testing/stdlib-protobuf.json) y
  el check [`scripts/stdlib-protobuf-check.sh`](./scripts/stdlib-protobuf-check.sh),
  integrado en `scripts/test-gate.sh`. El contrato fija proto3 schema-first,
  mapping de presencia y escalares, open enums numéricos, maps last-wins,
  packed/unpacked, unknown raw fields, parser streaming con frame, evolución
  contra baseline TOML y `encodeDeterministic` propio de Tondo sin reflection
  runtime; Protobuf conserva una API de protocolo separada de `Value`. La
  conformidad wire e interoperabilidad quedan preparadas para
  `STD-CODEC-CONF-001`.

- [x] **STD-PERF-001 — Fijar contratos de rendimiento.** Cada hot path tiene
  oracle escalar, streaming/bytes-first, límites de allocation/memoria,
  workloads y gates de throughput, latencia, startup, code size y compile time.
  SIMD, word-at-a-time y target multiversioning se permiten solo con
  equivalencia exacta y fallback portable. Cerrado con
  [`docs/contracts/stdlib-performance.md`](./docs/contracts/stdlib-performance.md),
  el registro canónico [`testing/stdlib-performance.json`](./testing/stdlib-performance.json)
  y el check determinista [`scripts/stdlib-performance-check.sh`](./scripts/stdlib-performance-check.sh),
  integrado en `scripts/test-gate.sh`. El contrato fija identidad de workload,
  protocolo de 27 muestras mínimas, observables exactos, presupuestos por
  dimensión y la secuencia design → capture → compare → promote; no convierte
  una cifra dependiente de máquina en semántica del lenguaje.

- [x] **STD-TESTING-SPEC-001 — Especificar `std.testing`.** Fijar assertions de
  igualdad, diffs de texto, comparación float con tolerancia, consumo explícito
  de Option/Result, recursos temporales y datos generados que entren realmente
  en 0.1. Cada API declara tipos, ownership, cleanup, formato, seed,
  capabilities y límites; reutiliza sin alterar
  `log/tags/failNow/skip/attach/snapshot/withVirtualTime`,
  `VirtualTime.settle/advance`, el snapshot textual/store/update ya normativos
  ni sus diagnósticos. Los helpers de diff o generación no crean un segundo
  snapshot format, no registran tests, no interpretan tags runtime como
  selectores ni capturan pánicos como excepciones recuperables. Cerrado como
  contrato del owner con
  [`docs/contracts/stdlib-testing.md`](./docs/contracts/stdlib-testing.md), el
  registro [`testing/stdlib-testing.json`](./testing/stdlib-testing.json) y el
  check [`scripts/stdlib-testing-check.sh`](./scripts/stdlib-testing-check.sh),
  integrado en `scripts/test-gate.sh`. La superficie fija assertions estáticas,
  `TextDiff` line-based acotado, tolerancias Float/Float32, consumo de
  `Option`/`Result`, `TempDirectory` con cleanup terminal y `Generator`/`Shrink`
  con replay por seed y caso; el núcleo sellado y los formatos de runner,
  snapshots y artifacts no cambian. Wave 5 avanza a `STD-TESTING-IMPL-001`.

### 19.3 Hosted Standard Library

- [x] **STD-TIME-BASE-IMPL-001 — Implementar el proveedor monotónico
  intercambiable.** `std.time` usa una única frontera interna para proveedor real
  y virtual; la VM implementa consulta, suspensión, timer y deadline sin que el
  bytecode de usuario conozca cuál se seleccionó. El proveedor real usa reloj
  monotónico del target, respeta cancelación y nunca consulta calendario. El
  virtual solo puede seleccionarlo el dominio sellado de testing y no concede
  capabilities adicionales. La implementación actual cubre `Duration`,
  `Instant`, `Timer`, `now`, `resolution`, `deadline`, `sleep`, operaciones
  aritméticas y comparativas, cancelación cooperativa y límites atómicos de
  recursos. La cobertura directa está en `process_host` y el fixture
  `tests/runtime/m10-std-time-001.to`; el corpus común real/virtual, los
  dominios extranjeros, empates de deadline, límites y capability `clock` se
  validan en `process_host` y `driver`; la evidencia de distribución y
  conformance queda en `STD-TIME-BASE-CONF-001`.

- [x] **STD-CONSOLE-001 — Especificar consola sobre `std.io`.** Fijar stdout,
  stderr, entrada, flushing, texto/binario, errores y comportamiento async sin
  asumir terminal interactiva ni duplicar los protocolos generales.

- [x] **STD-PATH-001 — Definir paths portables y nativos.** Separar operaciones
  léxicas de acceso al host, preservar bytes no Unicode cuando el target lo
  admita y no prometer una representación común falsa.

- [x] **STD-ENV-SPEC-001 — Definir argumentos y environment.** `std.env` queda
  como una API read-only capability-gated por `environment`, con un
  `Snapshot` sellado por invocación, `Name`/`Value` explícitos para texto y
  bytes, argv ordenado, ausencia mediante `Option`, validación de nombres,
  límites y errores `Unavailable`/`ResourceLimit`. No hay lectura durante
  compilación, input ambiental implícito ni mutación global; el plan de testing
  materializa públicos por hash y secretos por descriptor/version dentro del
  worker. El contrato vivo está en `docs/contracts/stdlib-env.md` y en la
  sección 14.3.5 de la especificación estándar.

- [x] **STD-FS-001 — Definir filesystem.** Archivos, directorios, metadata,
  enlaces, permisos, atomicidad, iteración y operaciones async declaran
  portabilidad, TOCTOU, cleanup y errores sin esconder bloqueo.

- [x] **STD-PROC-001 — Especificar procesos.** Fijar `Command`, `Pipeline`,
  `ProcessHandle`, status, output, pipes, shell explícito y cancelación como una
  API versionada que preserve argv exacto. La promoción completa del bridge
  pertenece a `STD-PROC-IMPL-001`.

### 19.4 Implementación y evidencia

- [x] **STD-META-IMPL-001 — Implementar `std.meta` sobre el target cerrado.**
  Después de `META-VM-001`, materializar el companion meta dentro de la
  distribución candidata, implementar requests, recorrido, renderizado y
  builder en Tondo cuando sea posible y validar su descriptor/content hash. No
  incorpora providers de serialization ni formatos.

- [x] **STD-META-CONF-001 — Cerrar la evidencia build-only.** Ejecutar
  round-trips canónicos, source maps, errores, límites, budgets y ausencia
  efectiva de filesystem, environment, process, clock, entropy, network,
  threads, FFI y unsafe. Debe pasar antes de `META-DERIVE-001` o
  `META-GEN-001`.

- [x] **STD-BYTES-IMPL-001 — Implementar la identidad binaria común.**
  La VM hosted implementa `Bytes`, `BytesBuilder`, conversiones, UTF-8, slicing,
  equality/hash y límites con semántica de valor y sin alias mutable. El owner
  canónico es `std.bytes`; texto y bytes usan las conversiones explícitas del
  lenguaje `Bytes(String)` y `String(Bytes)`.

- [x] **STD-BYTES-CONF-001 — Cerrar la evidencia temprana de bytes.** El fixture
  runtime `m10-std-bytes-001`, los tests directos del host y la suite completa de
  `tondo-compiler` cubren vacío, builders, límites, slicing, equality/hash,
  conversión `String`/`Array[Byte]`, UTF-8 inválido, moves/copies y paso por
  funciones públicas sin alias mutable. La evidencia actualiza el manifest
  vivo; la integración de attachments se prueba después en `UTEST-ARTIFACT-001`.

- [x] **STD-ENV-IMPL-001 — Implementar el acceso declarado de environment.**
  La VM hosted expone únicamente el snapshot entregado por el adaptador mediante
  la API y capability normales; distingue ausencia, texto UTF-8 y bytes sin
  globals ni consulta ambiental implícita. El snapshot se cachea por invocación,
  conserva argv y orden de entradas, valida `Name.fromText`/`Name.fromBytes`,
  aplica límites atómicos y publica `EnvError` tipados. El adaptador acepta el
  snapshot vacío como caso base; `UTEST-INPUTS-001` conectará después su
  materialización por worker. La evidencia ejecutable está en
  `process_host` (snapshot sellado, Unicode/raw bytes, ausencia, unavailable,
  nombres inválidos, límites sin publicación parcial), el fixture
  `m10-std-env-001` y la capability test del driver.

- [x] **STD-ENV-CONF-001 — Cerrar la evidencia temprana de environment.** Probar
  snapshots vacío y declarado, ausencia, Unicode/bytes, ownership, capability y
  rechazo de consulta ambiental fuera del adaptador; sin snapshot explícito no
  aparece ninguna entrada del host. Clasificación production/test-only,
  materialización/revocación por worker, perfiles secretos y ausencia de
  filtraciones pertenecen a `UTEST-DEPS-001`, `UTEST-INPUTS-001` y Gate T0, no
  a este micro-gate previo al runner.

- [x] **STD-TIME-BASE-CONF-001 — Cerrar la evidencia del sustrato temporal.**
  Añadir modelos de suma/comparación/overflow de `Duration` e `Instant`, tests de
  identidad/mismatch de proveedor, suspensión, deadline, cancelación, empates de
  timers y disponibilidad por capability. Ejecutar el mismo corpus contra
  proveedor real con tolerancias operacionales explícitas y proveedor virtual
  con observaciones exactas; no usar sleeps reales para verificar el segundo.
  Antes de T0 usa el harness/adaptador público existente; después se vuelve a
  ejecutar mediante `tondo test` como parte de S1A. Debe pasar antes de Gate T0.

#### 19.4.1 Tareas leaf de la auditoría pública

Estas tareas no invalidan los kernels ni los detalles internos existentes. Cierran la
diferencia entre “hay código Rust que prueba una operación” y “la API Tondo
completa del owner existe por la ruta pública”. Cada una debe actualizar la
matriz de owner por firma, no limitarse a citar un archivo que contiene alguna
parte del módulo.

#### 19.4.2 Contrato ABI canónico

Las leaves A3 solo pueden marcarse `[x]` cuando cumplen todos estos puntos:

- `STD-SER-IMPL-001`: publicar `Encode[C]`/`Decode[C]` y
  `Encoder[C, E]`/`Decoder[C, E]` desde la ruta Tondo, con adapters typed sin DOM.
- `STD-DERIVE-SER-001`: generar derives por codec, anotaciones `@name`,
  `@ignore`, `@json(base64)` y `@proto(number)` con source maps deterministas.
- `STD-JSON-API-001`/`STD-MSGPACK-API-001`: migrar a `parse`, `decode[T]`,
  `encode(value)` y aliases de `serialization.Value`, `ValueView` y `Raw`.
- `STD-PROTOBUF-API-001`: conservar `ProtoEvent`/`UnknownField` como API de
  protocolo y usar únicamente `Encode[Protobuf]`/`Decode[Protobuf]` para tipos
  generados; no introducir un DOM `Value` para Protobuf.
- `STD-JSON-IMPL-001`, `STD-MSGPACK-IMPL-001` y `STD-PROTOBUF-IMPL-001`:
  conectar las tres rutas (bytes, streaming y typed) a esos símbolos públicos,
  preservar límites finitos y demostrar tests de anotaciones, Base64, vistas,
  Raw y errores estructurados. Los símbolos Rust internos no forman una segunda
  API Tondo ni se mantienen como superficie de compatibilidad.

- [x] **STD-JSON-API-001 — Fijar la API fuente exacta de JSON.** Publicar
  `parse -> Value`, `decode[T: Decode[Json]]`, `encode[T: Encode[Json]]`,
  `ValueView`/`parseView`, `Raw`, `JsonReader`/`JsonWriter`, options y limits
  nominales, errores con path y estado terminal; no hay aliases ni defaults
  ambientales. Cerrado con el catálogo de firmas único en la sección 14.9, el
  contrato machine-readable `testing/stdlib-json.json`, el check owner y la
  matriz de auditoría por firma. `parseView` y `raw`/`rawUnchecked` quedan como
  entry points explícitos (la última variante solo en `unsafe`); sus rutas HIR,
  lowering y caso público se mantienen abiertas, de forma visible, para
  `STD-JSON-IMPL-001` y no se sustituyen por aliases del bridge Rust.

- [x] **STD-MSGPACK-API-001 — Fijar la API fuente exacta de MessagePack.**
  Publicar `parse -> Value`, `decode[T: Decode[MessagePack]]`,
  `encode[T: Encode[MessagePack]]`, `ValueView`/`Raw`, ext/timestamp, policies,
  options/limits, eventos, readers/writers, ownership, errores y terminalidad
  bajo el owner común de serialization e I/O. Cerrado con el catálogo único de
  firmas en la sección 14.10, el contrato machine-readable
  `testing/stdlib-messagepack.json` y el checker del owner, incluyendo las
  aliases `Value`/`ValueView`/`Raw` y la frontera estática
  `Encode[MessagePack]`/`Decode[MessagePack]`. Cualquier helper Rust es un
  detalle interno; la ruta HIR/lowering/caso público sigue visible para
  `STD-MSGPACK-IMPL-001` y no se sustituye por ese detalle.

- [x] **STD-PROTOBUF-API-001 — Fijar la API fuente y de build de Protobuf.**
  Publicar `[[protobuf.schema]]` en `tondo.toml`, mapping hermético, baseline
  de evolución, descriptor root explícito, tipos generated,
  `Encode[Protobuf]`/`Decode[Protobuf]`, `ProtoReader[T]`/`ProtoWriter[T]`,
  `ProtoEvent`, options, limits, errors y terminalidad. Cerrado con la sección
  14.11, el contrato machine-readable `testing/stdlib-protobuf.json` y el
  checker del owner: el input es exclusivamente `tondo.toml`, la identidad del
  schema y baseline es hermética y la ruta estática
  `Encode[Protobuf]`/`Decode[Protobuf]` queda separada de
  `serialization.Value`. `ProtoEvent`/`UnknownField` son la inspección wire;
  cualquier helper Rust es un detalle interno. La ruta HIR/lowering/caso público permanece visible para
  `STD-PROTOBUF-IMPL-001` y no se sustituye por el bridge.

- [x] **STD-CORE-IMPL-001 — Publicar los protocolos Core completos.** Conectar
  por dispatch estático las operaciones cerradas de `Option` y `Result`, junto
  con `Display`, `Equatable` y `Key`, incluyendo genéricos, ownership y errores.
  Los constructores intrínsecos existentes no sustituyen `map`, `mapErr` y
  `unwrapOr` ni sus pruebas de composición. Cerrado con la ruta pública HIR →
  MIR para `map`, `mapErr` y `unwrapOr`, y con cobertura end-to-end en
  `tests/runtime/m11-std-core-002.to`; las capacidades `Display`, `Equatable`
  y `Key` permanecen estáticas y cerradas, sin vtable ni reflection.

- [x] **STD-TEXT-IMPL-001 — Completar la API pública de texto.** Añadir
  `String.empty`, `fromChars`, `slice` y `chars`, además de validar todas las
  operaciones ya conectadas contra scalars Unicode, UTF-8 inválido, límites y
  costes. El bridge actual de búsqueda/transformación no constituye por sí solo
  el owner completo. Cerrado con `TextError` nominal, construcción segura desde
  `Array[Char]`, slicing por scalar con límites atómicos y `chars()` como
  identidad del `String` iterable (sin wrapper de cursor adicional), además de
  cobertura hosted end-to-end en `m11-std-text-002`.

- [x] **STD-COLL-IMPL-001 — Completar constructores y operaciones de
  colecciones.** Publicar `Array.new/withCapacity/push/pop`, las operaciones
  cerradas de `Map` y `Set`, y sus iteradores con semántica de valor, orden y
  errores exactos. Reutilizar los intrinsics del lenguaje sin crear dos APIs ni
  dos representaciones. Cerrado con constructores genéricos explícitos,
  `CollectionError` atómico, operaciones directas sobre los buffers COW del
  runtime, orden observable de `Map`/`Set`, cursores own lazy para `entries` y
  `values`, materialización host de `Map`/`Set` y cobertura hosted en
  `tests/runtime/m11-std-collections-001.to`.

- [x] **STD-ITER-IMPL-001 — Implementar los combinadores estáticos.** Conectar
  `map`, `filter`, `take` y `collect` sobre un único `Iterator[T]`, con lazy
  evaluation, consumo visible, límites y errores de colección. Cerrado con
  adaptadores own encadenables en la VM que retienen callbacks síncronos,
  consumen fuentes intrínsecas sin arrays intermedios, tratan `take(n < 0)` como
  vacío y devuelven `CollectionError` al alcanzar el límite de materialización.
  La ruta HIR → MIR → bytecode acepta sintaxis de método y las formas
  cualificadas `std.iter.*`; `tests/runtime/m11-std-iter-001.to` cubre map,
  filter, take, collect, callbacks, rangos, encadenamiento y tipos explícitos.

- [x] **STD-FMT-IMPL-001 — Exponer `std.format` a programas Tondo.** Conectar
  `Builder`, `format` y `join` a `Display`, con crecimiento acotado, error
  atómico y tests end-to-end. Cerrado con tipos intrínsecos dedicados,
  resolución HIR, operaciones MIR/bytecode verificadas y ejecución VM. El
  builder host conserva el límite global, rechaza cada append sin mutación
  parcial y `format`/`join` seleccionan `Display` estático, incluyendo callbacks
  para implementaciones de usuario. `tests/runtime/m11-std-format-001.to` y la
  prueba host de límites cubren la ruta pública completa.

- [x] **STD-IO-IMPL-001 — Completar protocolos y helpers de I/O.** Mantener los
  handles `Reader`/`Writer` existentes y añadir `readAll`/`IoLimits`, partial
  I/O, EOF, cancelación, límite prospectivo y cleanup por la ruta pública.
  Cerrado con `IoLimits` validable y default seguro, `readAll`/`writeAll`
  expuestos desde `std.io`, operaciones `Reader`/`Writer` async con jobs
  inmediatos cancelables en el host, short reads/writes, EOF y errores tipados.
  `readAll` comprueba el límite agregado antes de consumir stdin, y el cleanup
  público retira los tokens `Reader`/`Writer` para que no puedan reutilizarse.
  La prueba kernel cubre progreso parcial, EOF, límites, cancelación y flush;
  `tests/runtime/m11-std-io-001.to` valida la cadena HIR → MIR → bytecode → VM
  con `await`, lectura acotada y escritura drenada.

- [x] **STD-FS-IMPL-001 — Completar filesystem hosted.** `open`,
  `openDirectory`, `metadata`, `File`/`Directory` y sus operaciones async ya
  atraviesan HIR, MIR, bytecode, VM y bootstrap host con `FsError` nominal.
  `File` conserva el descriptor y la posición, expone `read`/`write`/`flush`,
  `Directory.list` mantiene orden por bytes nativos y ambos handles se revocan
  en cleanup normal/unwind. `readAll/writeAll/list/rename/atomicWrite` existentes
  también quedan registrados como awaitables y mantienen límites y atomicidad.
  La fixture `tests/runtime/m11-std-fs-001.to` y la prueba host cubren apertura,
  metadata, directorio, partial writes, lectura, errores y cleanup.

- [x] **STD-PROC-IMPL-001 — Alinear procesos con el contrato público.** La
  superficie hosted expone `command/shell/pipe` (con `cmd` documentado solo como
  alias bootstrap interno), `Command.run/output/check/start`,
  `ProcessHandle.wait/cancel`, `Command/Pipeline.mergeStderr` y
  `ProcessOutput.stdout/stderr/combined/statuses`. La captura mantiene stdout y
  stderr separados y ofrece la secuencia intercalada observada por el host;
  `mergeStderr` implementa la redirección tipada equivalente a `|&`/`2>&1 |`.
  Pipes usan backpressure del sistema operativo, los lectores se drenan
  concurrentemente, cancelación y errores limpian/reaparecen todos los hijos y
  la prueba host cubre argumentos exactos, streams separados, combined,
  redirección, formas de pipeline, backpressure y errores nominales.

- [x] **STD-SER-IMPL-001 — Implementar `std.serialization` tipada.** La ruta
  fuente del compilador publica `Encode[C]`/`Decode[C]` y los protocolos
  `Encoder[C, E]`/`Decoder[C, E]` como contratos prelude abiertos del módulo
  compiler-owned `std.serialization`. Cada método conserva aridad, receiver,
  modos `var`/`ref`, resultado `Result` y bounds del codec; las llamadas
  cualificadas y por constraint usan la selección HIR/MIR estática sin
  `Value`, reflection runtime, vtables ni lookup por nombre. El verificador HIR
  deriva de nuevo las firmas de Encode/Decode y de los 23 métodos de
  Encoder/Decoder antes de producir MIR. Los kernels Rust siguen siendo
  adaptadores internos; los entry points de JSON/MessagePack/Protobuf y los
  providers derive se cierran en sus leaves posteriores. Evidencia: tests HIR
  de lowering y de comprobación de implementaciones/calls en SSD.

- [x] **STD-DERIVE-SER-001 — Implementar providers de derive de
  serialization.** Registrar providers build-only para
  `Encode[C]`/`Decode[C]`, generar impls Tondo deterministas para
  records/enums/newtypes/genéricos y probar diagnostics, source maps, límites,
  campos privados y ausencia de reflection runtime. Resolver en compile time
  `@name`, `@json(...)`, `@messagepack(...)`, `@proto(number)`, `@ignore` y
  `@json(base64)` de forma simétrica; no inferir números Protobuf ni leer el
  entorno. El provider recibe solo el MetaSnapshot sellado. Cerrado con
  identidad especializada (`Encode[Json]`, `Encode[MessagePack]`,
  `Encode[Protobuf]` y sus decoders), headers y bounds codec-específicos,
  source maps atómicos y políticas de fields `@name`/`@ignore`; las
  anotaciones `@json(base64)`, `@messagepack(binary)` y `@proto(number)` quedan
  validadas en el snapshot. Su transformación wire y las políticas completas
  de record se mantienen explícitamente abiertas en
  `STD-CODEC-DERIVE-POLICY-001`; validar una anotación no cuenta como haberla
  aplicado. El driver ejecuta los providers sobre el snapshot
  sellado, publica sus fuentes de manera atómica y realiza exactamente una
  segunda pasada ordinaria del frontend con derives desactivados; los
  diagnósticos de código generado vuelven al span del target original. El
  protocolo incluye `Decoder.reject(SerializationError): E`, y un derive
  genérico de Encode o Decode exige `Discard` solo a los parámetros realmente
  usados: el writer puede fallar antes de consumir todos los fields recibidos
  por valor y el decoder debe poder limpiar valores parciales.

- [x] **STD-JSON-IMPL-001 — Implementar las tres rutas de JSON.** Publicar
  `parse -> Value`, `decode[T: Decode[Json]]` y
  `encode[T: Encode[Json]]`, además de `ValueView`/`parseView`, `Raw` y
  `JsonReader`/`JsonWriter` con stack explícito. Cubrir chunking, Unicode,
  paths, policies, RFC 8259/JCS, Base64 anotado, límites finitos y ausencia de
  DOM en la ruta typed. Cerrado con `json_api.rs`: reader/writer de frames
  explícitos, `JsonNumber` decimal exacto, vistas input-backed, `Raw` validado,
  typed dispatch directo por `Encode[Json]`/`Decode[Json]`, políticas de
  duplicados y errores terminales con path. `serde_json` permanece aislado como
  oracle interno y no sustituye la ruta typed; los gaps de exposición
  HIR/VM quedan registrados por `STD-PUBLIC-API-AUDIT-001`.

- [x] **STD-JSON-PUBLIC-001 — Cerrar la superficie Tondo exacta de JSON.** El
  primer corte conecta HIR → bytecode → VM → host para `Value`, `ValueView`,
  `Raw`, `JsonNumber`, `JsonReader` y `JsonWriter`, preserva los genéricos
  explícitos y hace terminales los readers/writers tras `finish` o error. La
  fixture pública se ejecuta desde el test de aceptación de la CLI. La leaf no
  se cierra todavía: `rawUnchecked` ya atraviesa HIR → VM → host como callable
  realmente `unsafe`, sin validar los bytes ni admitir llamadas desde código
  safe. `JsonLimits`, `JsonDecodeOptions`, `JsonEncodeOptions` y los tres enums
  de policy ya son tipos compiler-owned cerrados, construibles desde Tondo, y
  todas las operaciones salvo el dispatch typed respetan su aridad normativa y
  aplican realmente límites y policies. `JsonEvent`, `JsonErrorKind`,
  `JsonLocation`, `JsonPath` y `JsonError` usan ahora el sistema nominal normal
  de Tondo de extremo a extremo; la representación intrínseca provisional se
  eliminó y el host materializa records/enums por nombre público, ordinal y
  orden de declaración verificados. El cierre añade una unidad fuente
  compiler-owned que implementa el adapter typed con los protocolos públicos,
  redirige los targets nominales a los impls derive y recorre records, enums
  unit/tuple/record y composiciones `Option`/`Array`/`Map` sin reflection ni
  DOM. Los enums JSON usan un object externally tagged único y el reader
  detecta trailing data antes de publicar `T`. `Decoder.peek` es no consumidor
  y permite composición estática sin rewind. La fixture CLI cubre round-trip y
  formas inválidas; la auditoría global verifica ahora las 214/214 firmas,
  incluidas las 23/23 de `std.json`. Quedan fuera de esta leaf únicamente los
  gates de rendimiento, fuzzing, conformance y promoción de S1A.

- [x] **STD-CODEC-DERIVE-POLICY-001 — Completar semántica wire de derives.**
  Sustituir la decodificación posicional de records por una máquina estática
  independiente del orden que detecte missing/duplicate/unknown fields,
  reconstruya `Option` ausentes como `none` y aplique la policy explícita del
  codec sin DOM. Consumir realmente `@json(base64)`, `@messagepack(binary)` y
  `@proto(number)` en ambos sentidos; cubrir fields renombrados/ignorados,
  payloads de enum, genéricos affine, orden permutado y fallos parciales. La
  leaf no puede cerrarse por validar attributes ni por tests del kernel Rust.
  Cerrado con `serialization_derive.rs`: los records generan slots `seen` y
  `Option[field]`, aceptan orden arbitrario y distinguen `MissingField`,
  `DuplicateField` y `UnknownField`; los fields `Option` ausentes y los
  ignorados publican `none` después de consumir su payload. JSON aplica
  Base64 RFC 4648 mediante operaciones estáticas `Encoder/Decoder.base64`,
  MessagePack usa maps string-keyed y `@messagepack(binary)` para binarios, y
  Protobuf transporta `@proto(number)` como tokens `#N` que el adapter baja a
  tags wire. La fixture CLI `m11-std-codecs-001.to` prueba rename/ignore,
  Base64, orden permutado, missing/unknown/duplicate y payloads de enum; los
  tests Rust cubren el adapter Protobuf order-independent, el mapa MessagePack,
  el Base64 canónico y el mapping nominal de errores. Contratos y gates:
  `docs/contracts/stdlib-serialization.md`, `testing/stdlib-serialization.json`,
  `testing/stdlib-json.json`, `scripts/stdlib-serialization-check.sh` y
  `scripts/stdlib-serialization-test.sh`.

- [x] **STD-MSGPACK-IMPL-001 — Implementar MessagePack completo.** Publicar
  `parse -> Value`, `decode[T: Decode[MessagePack]]`,
  `encode[T: Encode[MessagePack]]`, `ValueView`/`Raw` y streaming para todo el
  modelo wire, claves arbitrarias, ext/timestamp, floats bit-exact, policies y
  encoding determinista. El codec debe consumir los protocolos comunes sin
  materializar un `Value` en la ruta typed; el corpus cubre fragmentación,
  non-minimal, duplicados, límites y terminalidad. Cerrado con el owner
  portable de `messagepack_api.rs`, la vista input-backed `parseView`, `Raw`
  validado y `unsafe rawUnchecked`; la ruta canónica `Encode[MessagePack]` /
  `Decode[MessagePack]` usa `MessagePackWriter`/`MessagePackReader` sin DOM; los
  helpers Rust son detalles internos de implementación.

- [x] **STD-PROTOBUF-IMPL-001 — Implementar Protobuf schema-first.** Publicar
  `Encode[Protobuf]`/`Decode[Protobuf]`, `ProtoReader[T]`/`ProtoWriter[T]`,
  `ProtoEvent` y `UnknownField` como API de wire separada, sin convertir
  Protobuf en `serialization.Value`. El checker proto3 valida números y
  evolución por baseline TOML; el encoder/reader usan frames explícitos,
  límites finitos, atomicidad terminal y preservación de unknown fields. La
  conformance externa y la integración del generator son gates posteriores.
  Cerrado con el
  owner portable de `protobuf_api.rs`, la ruta estática
  `Encode[Protobuf]`/`Decode[Protobuf]`, checker schema-first y generator
  determinista; `ProtoReader` y `ProtoWriter` aplican ahora `max_events` a cada
  evento antes de materializarlo o aceptarlo, manteniendo terminalidad y
  preservación de unknown fields.

- [x] **STD-TESTING-SHRINK-001 — Completar generación y shrinking público.**
  `crates/tondo-compiler/src/test_generation.rs` conecta los helpers públicos
  `Generator`/`Shrink` con `RuntimeRunner`: materializa casos por
  `Generator.forCase`, conserva orden por `caseIndex`, permite replay y limita
  casos, candidatos y profundidad antes de reservar. El primer fallo se reduce
  en orden estable y cada candidato usa un worker nuevo; los casos son una vista
  efímera de tooling y no crean `TestEntry`, suites ni subtests dinámicos.

- [x] **STD-PUBLIC-API-AUDIT-001 — Verificar firma por firma todos los owners
  A.** `scripts/stdlib-public-api-audit.sh` genera y valida
  `testing/stdlib-public-api.json` con la cadena contract signature → símbolo
  HIR → lowering → host/VM → caso público. `--check` detecta drift y mantiene
  visibles los huecos; `--strict` falla ante cualquiera. No acepta un path Rust
  aislado, un fixture que llama otra operación, una prueba documental ni un
  alias bootstrap. El registro actual está verificado en `214/214`, con cero
  gaps; el bundle `STD-S1A-SEAL-001` fija la promoción técnica del draft sin
  convertirla en una release.

- [x] **STD-IMPL-001 — Coordinar implementación Core por owner.** Cierra cuando
  `STD-CORE-IMPL-001`, `STD-TEXT-IMPL-001`, `STD-COLL-IMPL-001`,
  `STD-ITER-IMPL-001`, `STD-FMT-IMPL-001`, `STD-IO-IMPL-001`,
  `STD-SER-IMPL-001`, los owners Core ya completos y
  `STD-PUBLIC-API-AUDIT-001` no dejan ninguna firma sin ruta pública dentro
  del grupo coordinado. La evidencia reproducible está en
  `testing/stdlib-implementation-coordination.json` y su checker: verifica
  las 64 firmas Core, la etapa `IMPL/HOST` y las rutas de implementación/tests
  de los ocho owners Core/serialization. El auditor global está ahora en
  `214/214` firmas verificadas y cero gaps; los owners build-only quedan
  excluidos mediante una razón explícita y no son un waiver.

- [x] **STD-IMPL-002 — Coordinar Hosted por owner.** Cierra tras
  `STD-FS-IMPL-001`, `STD-PROC-IMPL-001` y la auditoría pública, conservando los
  bridges correctos de path/console y capabilities. El registro
  `testing/stdlib-hosted-implementation-coordination.json` y su checker
  verifican los cuatro owners Hosted, sus capabilities exactas, las etapas
  `IMPL/HOST`, las celdas de evidencia y las 48/48 firmas públicas. `std.path`
  queda explícitamente `HOST not-applicable` por ser puramente léxico; no se
  inventa un provider. La auditoría global está en `214/214`, sin gaps; los
  tres owners build-only conservan su frontera `not-applicable` explícita.

- [x] **STD-CODEC-PUBLIC-001 — Cerrar la exposición pública restante de codecs
  y owners build-only.** Las 32 firmas restantes de MessagePack y Protobuf
  quedan trazadas hasta HIR, lowering, VM/host y casos públicos: la matriz
  `testing/stdlib-public-api.json` verifica 214/214 filas bajo `--strict`.
  `MessagePackValue` tiene la ruta dinámica `encode` además de la ruta typed
  con bounds, y la fixture `m11-std-codecs-001.to` compila y ejecuta parse,
  parseView, decode/encode typed y dynamic, validate, determinismo, raw,
  timestamp/ext, readers/writers y Protobuf descriptor/readers/writers/
  unknown fields. `std.meta`, `std.reflect` y `std.serialization` quedan
  indexados explícitamente como `build-only` con runtime `not-applicable`,
  paths compiler-owned y razones normativas; no se inventa una `pub fn`
  runtime. `scripts/stdlib-public-api-audit.sh --strict`, los negativos del
  auditor, `scripts/stdlib-codec-conformance.sh` y el smoke CLI pasan.

- [x] **STD-TESTING-IMPL-001 — Implementar `std.testing` sobre T0.** El runtime,
  temp resources, generators, diffs, tolerancias y control sellado se conservan.
  `TestingShrink` está conectado desde la resolución HIR y lowering hasta el
  host VM con el protocolo `Shrink` sellado, candidatos deterministas y límites
  de 4.096 candidatos/64 niveles; una implementación de usuario se rechaza con
  `E1114`. El runner público ejercita `shrink`, replay, `Generator.forCase`,
  draws y los helpers de valor; la prueba de host cubre determinismo,
  deduplicación, anidamiento, NaN, tipos no soportados y atomicidad. El owner
  queda implementado, pero promoción/conformance y la auditoría global de
  firmas siguen abiertas.

#### 19.4.2 Evidencia leaf por owner

Cada tarea siguiente produce un record machine-readable del owner con campos
separados `SPEC`, `IMPL`, `HOST`, `MODEL`, `TEST`, `FUZZ`, `PERF`, `CONF` y
`DOC`. Un campo no aplicable lleva razón normativa; no se crea una task vacía.
Los campos aplicables enlazan artefactos y casos exactos, no solo directorios o
tests vecinos. Así se conserva granularidad por owner sin multiplicar tareas
administrativas que no implementan comportamiento.

- [x] **STD-A-META-EVIDENCE-001 — Cerrar evidencia de `std.meta`.**
  `testing/stdlib-meta.json` fija el contrato A0, los seis requisitos del
  owner y los límites de compilación/generación; `testing/stdlib-owner-evidence.json`
  registra por separado `SPEC`, `IMPL`, `HOST`, `MODEL`, `TEST`, `FUZZ`, `PERF`,
  `CONF` y `DOC`. `HOST` es explícitamente `not-applicable` por la frontera
  build-only `tondo-meta`; el modelo, las pruebas y el corpus de fuzz enlazan
  `meta.rs`, `std_meta.rs`, `meta_robust.rs` y el test de conformance. Los
  presupuestos compile-time/generados quedan declarados como promoción
  pendiente, sin inventar una captura runtime.
- [x] **STD-A-REFLECT-EVIDENCE-001 — Cerrar evidencia de `std.reflect`.**
  `testing/stdlib-reflect.json` fija los seis requisitos del owner y sus
  límites de metadata estática; `testing/stdlib-owner-evidence.json` separa
  raíces explícitas, clausura pública, privacidad, identidad local al artefacto,
  ausencia de reflection de valores y documentación. `HOST` es explícitamente
  `not-applicable` por la frontera metadata-only; link-work y tamaño de
  descriptores quedan como presupuestos de promoción pendientes, y la
  conformidad global continúa visible en la matriz.
- [x] **STD-A-BYTES-EVIDENCE-001 — Cerrar evidencia de `std.bytes`.**
  `testing/stdlib-bytes.json` fija el contrato A0, sus límites, invariantes,
  corpora y seis requisitos ejecutables; `testing/stdlib-owner-evidence.json`
  separa identidad/snapshots, builders, UTF-8, límites/rangos,
  properties/hot paths, conformidad y docs. `HOST` es explícitamente
  `not-applicable` por ser un intrinsic compiler/VM-owned; `STD-A-FUZZ-001`
  promueve el fuzz owner-aware y la captura de rendimiento sigue como
  promoción pendiente sin inventar métricas.
- [x] **STD-A-TIME-EVIDENCE-001 — Cerrar evidencia del time-base.**
  `testing/stdlib-time.json` fija el contrato A0 capability-gated, sus cinco
  límites, nueve invariantes, tres corpora y seis requisitos ejecutables.
  `testing/stdlib-owner-evidence.json` separa modelo, provider HOST real y
  virtual, límites/errores, ciclo de vida de timers, conformidad por `clock` y
  documentación. El provider real usa `std::time::Instant`; el virtual es
  sellado, parte de cero y solo avanza explícitamente. `process_host` ejecuta
  el corpus común para ambos providers y `driver` prueba capability ausente,
  sustitución virtual, settle, cancelación, dominios y cleanup. `HOST` queda
  `verified`; `STD-A-FUZZ-001` promueve el fuzz owner-aware y la captura de
  rendimiento por provider permanece como promoción pendiente.
  `scripts/stdlib-time-check.sh` y
  `scripts/stdlib-time-test.sh` validan el contrato y sus negativos.
- [x] **STD-A-ENV-EVIDENCE-001 — Cerrar evidencia de `std.env`.**
  `testing/stdlib-env.json` fija el contrato A0 capability-gated, sus tres
  límites, nueve invariantes, tres corpora y seis requisitos ejecutables.
  `testing/stdlib-owner-evidence.json` separa snapshot/modelo, adaptador HOST,
  argv ordenado, nombres y valores raw/text, ausencia mediante `Option`, copias,
  límites atómicos, aislamiento ambiental y documentación. `process_host` usa
  solo el plan de inputs entregado en runtime; las pruebas cubren proveedor
  unavailable, nombres inválidos, entradas inyectadas, límites sin estado
  parcial y ausencia de lecturas de `PATH`/`HOME`. `HOST` queda `verified`;
  `STD-A-FUZZ-001` promueve el fuzz owner-aware y la captura de rendimiento por
  capability permanece como promoción pendiente. `scripts/stdlib-env-check.sh` y
  `scripts/stdlib-env-test.sh` validan el contrato y sus negativos.
- [x] **STD-A-CORE-EVIDENCE-001 — Cerrar evidencia Core.** `testing/stdlib-owner-evidence.json`
  registra las nueve celdas del owner intrínseco `std.core`: protocolos y
  genéricos en HIR, composición `Option`/`Result`, dispatch y agregados
  bytecode/MIR, fixtures runtime y auditoría pública de las nueve firmas.
  `HOST` es explícitamente `not-applicable` por la frontera compiler/VM-owned.
  El corpus de admission fuzz cubre formas `Option`/`Result` y protocolos
  genéricos; `STD-A-FUZZ-001` promueve el fuzz owner-aware, mientras la captura
  de rendimiento por owner mantiene su frontera PERF-001 y la ejecución pública
  de conformance está promovida por `STD-A-CONF-001`.
- [x] **STD-A-TEXT-EVIDENCE-001 — Cerrar evidencia de texto.**
  `testing/stdlib-owner-evidence.json` registra las nueve celdas del owner
  intrínseco `std.text` y enlaza las quince firmas de `String` con el contrato
  de grupo, HIR/lowering, puente compiler/VM, fixtures Unicode, slicing por
  scalar, iteración y rechazo atómico de UTF-8 inválido. `HOST` es explícitamente
  `not-applicable`; el corpus bounded de UTF-8/admission fuzz y la auditoría
  pública quedan verificados como evidencia disponible; `STD-A-FUZZ-001`
  promueve el fuzz owner-aware y `STD-A-CONF-001` promueve la ejecución pública;
  el coste sigue en la frontera PERF-001. `scripts/stdlib-text-test.sh` cubre negativos de contrato y
  forma/cobertura de la API.
- [x] **STD-A-COLL-EVIDENCE-001 — Cerrar evidencia de colecciones.**
  `testing/stdlib-owner-evidence.json` registra las nueve celdas del owner
  intrínseco `std.collections` y enlaza las dieciocho firmas de `Array`, `Map`
  y `Set` con HIR/MIR, intrinsics bootstrap, bytecode, VM y el fixture
  `m11-std-collections-001.to`. La evidencia cubre COW y semántica de valor,
  capacidad/errores atómicos, protocolo `Key`, hashing, orden de inserción,
  membership, reemplazo/eliminación e iteración lazy. `HOST` es explícitamente
  `not-applicable`; admission fuzz y properties eager/COW quedan ejecutables;
  `STD-A-FUZZ-001` promueve el fuzz owner-aware y `STD-A-CONF-001` promueve la
  ejecución pública; los baselines de memoria/hash siguen su frontera PERF-001.
  `scripts/stdlib-collections-test.sh`
  valida negativos del contrato, símbolos, runtime, properties y las 18/18 filas
  de la auditoría pública.
- [x] **STD-A-ITER-EVIDENCE-001 — Cerrar evidencia de iteradores.** `testing/stdlib-owner-evidence.json`
  registra las nueve celdas del owner intrínseco `std.iter` y enlaza las cuatro
  firmas de `Iterator` (`map`, `filter`, `take`, `collect`) con el protocolo HIR,
  lowering MIR/bytecode, VM, fixture runtime y auditoría pública. La evidencia
  cubre laziness, consumo único, composición, callbacks síncronos, closures,
  rutas calificadas/genéricas, `take` negativo, `collect` acotado, cursores
  prestados, iteradores de usuario, agotamiento y trazado de callbacks. `HOST`
  es explícitamente `not-applicable`; admission fuzz y properties de cursor son
  ejecutables; `STD-A-FUZZ-001` promueve el fuzz owner-aware y
  `STD-A-CONF-001` promueve la ejecución pública; los baselines de
  retención/allocations/materialización siguen su frontera PERF-001.
  `scripts/stdlib-iter-test.sh` valida negativos de contrato, símbolos, fixture,
  properties y las 4/4 filas de la auditoría pública.
- [x] **STD-A-MATH-EVIDENCE-001 — Cerrar evidencia matemática.** `testing/stdlib-owner-evidence.json`
  registra las nueve celdas del owner intrínseco `std.math` y enlaza sus nueve
  firmas escalares con dispatch HIR, puente `process_host`, `MathError`, kernel
  portable y fixture público. La evidencia cubre IEEE-754, ties-to-even, cero
  con signo, infinitudes, NaN, subnormales, overflow, dominio/no-finito de
  `sqrt`, properties Float32, diagnósticos de constantes y la auditoría 9/9.
  El scalar oracle es la ruta normativa 0.1 y no existe un camino SIMD o
  fast-math alternativo; cualquier vectorización futura deberá probar
  equivalencia bit a bit. `HOST` es `not-applicable`; `STD-A-FUZZ-001` promueve
  el fuzz owner-aware y `STD-A-CONF-001` promueve la ejecución pública; los
  baselines de coste siguen su frontera PERF-001.
  `scripts/stdlib-math-test.sh` valida contratos negativos, símbolos, corpus,
  ausencia de rutas SIMD/fast-math y las 9/9 filas públicas.
- [x] **STD-A-FMT-EVIDENCE-001 — Cerrar evidencia de formatting.** `std.format`
  queda trazado en nueve celdas del owner intrínseco: las cinco firmas de
  `Display`/builder pasan por HIR/MIR/bytecode/VM, fixture público y auditoría
  de API. El corpus cubre vacío, límites exactos, separadores, errores de
  `Display`, receivers inválidos y atomicidad; `HOST` es `not-applicable`.
  `STD-A-FUZZ-001` promueve el fuzz owner-aware; baselines de
  allocations/materialización permanecen visibles como promoción posterior,
  sin inventar métricas.
  `scripts/stdlib-format-test.sh` valida contrato, símbolos, corpus, docs y
  las 5/5 filas públicas.
- [x] **STD-A-IO-EVIDENCE-001 — Cerrar evidencia de I/O portable.** Las cuatro
  firmas de `std.io` quedan trazadas en nueve celdas del owner portable:
  Reader/Writer, `IoLimits`, `readAll` y `writeAll` pasan por HIR/lowering,
  bytecode/VM, fixture público y auditoría de API. El kernel prueba particiones
  deterministas de chunks, partial I/O, EOF, límites, progreso cero,
  sobreescrituras, errores de `flush` y cancelación sin éxito parcial. `HOST` es
  `not-applicable`; los adaptadores pertenecen a `std.console`, `std.fs` y
  `std.process`. `scripts/stdlib-io-test.sh` valida contrato, símbolos, corpus,
  docs y las 4/4 filas públicas; `STD-A-FUZZ-001` promueve el fuzz owner-aware
  y los baselines de coste quedan visibles como promoción posterior.
- [x] **STD-A-PATH-EVIDENCE-001 — Cerrar evidencia de paths.** Las diez
  firmas de `std.path` quedan trazadas por contrato hosted, HIR/lowering,
  bytecode/VM, kernel portable, fixture público y auditoría de API. `Path` es
  un snapshot léxico de bytes con límite de 32 KiB: el corpus determinista
  cubre bytes nativos inválidos, UTF-8, NFC/NFD, `.`/`..`, raíces, archivos
  ocultos, extensiones vacías, separadores finales, joins rechazados y
  atomicidad de errores sin consultar el filesystem. `toBytes` devuelve una
  copia exacta y la prueba de host confirma que la frontera conserva esos
  bytes. `HOST` es `not-applicable`; `STD-A-FUZZ-001` promueve el fuzz
  owner-aware y `STD-A-CONF-001` promueve la ejecución pública; los baselines por
  owner siguen su frontera PERF-001.
  `scripts/stdlib-path-test.sh` valida contrato, símbolos, corpus, ausencia de
  capability, docs y las 10/10 filas públicas.
- [x] **STD-A-CONSOLE-EVIDENCE-001 — Cerrar evidencia de consola.** Las siete
  firmas de `std.console` quedan trazadas con el modelo único de
  `std.io.Reader`/`Writer`, tokens distintos para stdin/stdout/stderr, frontera
  estática de capability `console`, partial I/O, EOF, LF estable, flush
  explícito, errores UTF-8 atómicos y mensajes host opacos. HIR/lowering,
  bytecode/VM, host, fixture `m11-std-console-001.to`, auditoría pública 7/7 y
  la matriz de evidencia quedan enlazados. `HOST` es `verified`;
  `STD-A-FUZZ-001` promueve el fuzz owner-aware y `STD-A-CONF-001` promueve la
  ejecución pública; los baselines de bytes/chunks/work-units siguen su
  frontera PERF-001. `scripts/stdlib-console-test.sh` valida negativos,
  símbolos, corpus, capability, documentación y todas las filas públicas.
- [x] **STD-A-FS-EVIDENCE-001 — Cerrar evidencia de filesystem.** Las catorce
  firmas públicas de `std.fs` quedan trazadas por contrato hosted, capability
  `filesystem`, modelo de handles afines, HIR/lowering, bytecode/VM y el
  adaptador `process_host`. El fixture `m11-std-fs-001.to` y las pruebas host
  cubren bytes nativos, orden de directorios, modos, EOF/short I/O, errores
  tipados y redactados, límites de materialización, `atomicWrite`, cancelación,
  tokens stale y cleanup normal/unwind. La frontera estática rechaza imports
  sin capability antes del lowering, y la auditoría pública mantiene 14/14.
  `HOST` queda `verified`; `STD-A-FUZZ-001` promueve el fuzz owner-aware y
  `STD-A-CONF-001` promueve la ejecución pública; los baselines por target
  siguen su frontera PERF-001.
- [x] **STD-A-PROC-EVIDENCE-001 — Cerrar evidencia de procesos.** Las diecisiete
  firmas públicas quedan trazadas por el contrato hosted, capability `process`,
  planes inertes `Command`/`Pipeline`, handles terminales, HIR/lowering,
  bytecode/VM y el adaptador `process_host`. Las pruebas M8 cubren argv literal,
  shell explícito, las cuatro formas de pipe, backpressure por encima de la
  ventana del kernel, stdout/stderr separados, `combined`, redirección
  `mergeStderr`, estados de salida, errores de spawn, cancelación, panic/unwind
  y reaping. La auditoría pública mantiene 17/17 y `HOST` queda `verified`;
  `STD-A-FUZZ-001` promueve el fuzz owner-aware y `STD-A-CONF-001` promueve la
  ejecución pública; los baselines por target siguen su frontera PERF-001.
- [x] **STD-A-SER-EVIDENCE-001 — Cerrar evidencia de serialization.** El
  protocolo común queda trazado por `Encoder`/`Decoder`, `Encode`/`Decode`, la
  máquina de eventos con frames explícitos, `Value`/`ValueView`/`Raw`, paths,
  construcción atómica y streaming. Los providers de derive canónicos quedan
  enlazados con la frontera `tondo-meta`, source maps y diagnostics reproducibles;
  las pruebas cubren records, enums, newtypes, genéricos, attributes, límites,
  duplicados, longitudes, chunking y publicación sin valores parciales. `HOST`
  es `not-applicable`; `STD-A-FUZZ-001` promueve el fuzz owner-aware del
  protocolo y `STD-A-CONF-001` promueve la ejecución pública; los baselines de
  coste siguen su frontera PERF-001.
- [x] **STD-A-JSON-EVIDENCE-001 — Cerrar evidencia de JSON.** Las rutas
  typed/dynamic/streaming quedan trazadas por el parser y writer de frames
  explícitos, `JsonNumber` exacto, límites, políticas, errores terminales,
  JCS/RFC 8785, fragmentos de un byte y la interoperabilidad bidireccional con
  `serde_json`. `HOST` es no aplicable; `STD-A-FUZZ-001` promueve el fuzz
  owner-aware y `STD-A-CONF-001` promueve la ejecución pública; los baselines de
  allocations/memoria por target siguen su frontera PERF-001.
- [x] **STD-A-MSGPACK-EVIDENCE-001 — Cerrar evidencia de MessagePack.** Las
  rutas typed/dynamic/streaming quedan trazadas por el modelo wire completo,
  formas no mínimas, enteros y bits de floats, binary/UTF-8, claves arbitrarias,
  ext/timestamp, determinismo, límites, fragmentos de un byte y la
  interoperabilidad bidireccional con `rmpv`. `HOST` es no aplicable;
  `STD-A-FUZZ-001` promueve el fuzz owner-aware y `STD-A-CONF-001` promueve la
  ejecución pública; los baselines de allocations/memoria por target siguen su
  frontera PERF-001.
- [x] **STD-A-PROTOBUF-EVIDENCE-001 — Cerrar evidencia de Protobuf.** La
  frontera TOML schema-first y las rutas wire typed/streaming quedan
  trazadas por proto3, presencia, repeated/packed, maps, oneof, enums abiertos,
  unknown fields/grupos, evolución, descriptor raíz, determinismo, límites,
  fragmentos de un byte y la interoperabilidad bidireccional con `prost`.
  `HOST` es no aplicable; `STD-A-FUZZ-001` promueve el fuzz owner-aware de
  schema/operaciones y `STD-A-CONF-001` promueve la ejecución pública; los
  baselines de allocations/memoria por target siguen su frontera PERF-001.
- [x] **STD-A-TESTING-EVIDENCE-001 — Cerrar evidencia de `std.testing`.**
  El contrato pasa a `closed-contract` y el leaf de evidencia registra las
  nueve celdas. Las 25 firmas públicas quedan enlazadas con assertions,
  `TextDiff`, tolerancias, consumo de `Option`/`Result`, temporales aislados,
  generación replayable, shrinking sellado, control terminal y virtual time.
  El bridge `HOST` es verificado por el worker; los proyectos de aceptación
  dogfoodéan importación test-only, hooks de control, retries/repeats,
  selección/sharding y JSON/JUnit. `STD-A-FUZZ-001` promueve el fuzz
  owner-aware y `STD-A-CONF-001` promueve la ejecución pública; las dimensiones
  de coste conservan la frontera PERF-001 explícita.

- [x] **STD-MATRIX-ALL-001 — Construir la matriz normativa de stdlib.**
  `testing/stdlib-matrix.json` contiene 22 owners (incluidos los owners
  intrínsecos `std.bytes` y capability-gated `std.time`/`std.env`), 214 firmas y 171
  requisitos de owner. Cada fila
  enlaza explícitamente `SPEC → IMPL/HOST → MODEL/TEST/FUZZ → PERF → CONF →
  DOC`, conserva las dimensiones públicas de PERF y queda `open-gaps` cuando
  una celda es `partial`, `pending`, `gap` o `not-applicable` sin
  sobreafirmar evidencia. `scripts/stdlib-matrix-check.sh` regenera y
  compara byte a byte la matriz, valida owners, firmas, requisitos, stages,
  razones y paths; `scripts/stdlib-matrix-test.sh` conserva dos fixtures
  negativos. La matriz coordina STD-0.1A sin introducir el catálogo cerrado
  STD-0.1B en G5; `STD-TEST-001`, `STD-DOC-001` y `STD-A-CONF-001` quedan
  cerrados para sus respectivas dimensiones; `STD-A-DIST-001` promueve la
  distribución VM reproducible y el sellado S1A queda explícitamente cerrado
  como bundle técnico del draft.

- [x] **STD-TEST-001 — Coordinar modelos y properties por owner.**
  `testing/stdlib-test-coordination.json` liga los 22 owners A, las 214 firmas
  públicas y los 171 requisitos de owner a 66 leyes de modelo, comandos de test
  y campañas de fuzz. `scripts/stdlib-test-coordination-check.sh` regenera el
  registro y lo compara con la auditoría pública, la matriz normativa y
  `stdlib-owner-evidence`; sus tests negativos rechazan superficies sin ley,
  owners sin comandos, firmas ausentes o rutas de fuzz incompletas. El test Rust
  `stdlib_owner_models` ejecuta la misma clausura y verifica que cada superficie
  pública tenga un modelo, incluyendo requisitos de owners sin filas de firma.
  Los campos `MODEL` y `TEST` quedan verificados; `STD-A-FUZZ-001` promueve los
  22 campos `FUZZ` owner-aware con corpus y seeds explícitos. La siguiente
  coordinación es `STD-CONF-001`.

- [x] **STD-CODEC-KERNEL-001 — Validar los kernels de formatos existentes.**
  `scripts/stdlib-codec-conformance.sh` prueba los kernels materializados y el
  bridge `validate/canonicalize`; esta evidencia se conserva como oracle parcial.

- [x] **STD-CODEC-CONF-001 — Cerrar evidencia de serialization y formatos.**
  Después de las cinco tareas A3, `scripts/stdlib-codec-conformance.sh` ejecuta
  las rutas typed/dynamic/streaming y los casos derive/schema-first de los
  owners, además del harness externo bidireccional
  [`crates/tondo-stdlib/tests/codec_conformance.rs`](./crates/tondo-stdlib/tests/codec_conformance.rs).
  `serde_json`, `rmpv` y `prost` prueban bytes externos y bytes de Tondo; el
  corpus cubre fragmentación de un byte, truncación, rechazo, límites,
  paths, preservación de extensiones/unknown fields y ausencia de DOM o
  reflection en las rutas typed. El target `stdlib_codecs` añade un smoke de
  fuzzing bounded y la evidencia se registra en
  [`testing/stdlib-codec-conformance.json`](./testing/stdlib-codec-conformance.json).

- [x] **STD-PERF-KERNEL-001 — Conservar el probe escalar inicial.** Nueve
  muestras por proceso, tres procesos y cinco kernels proporcionan una baseline
  reproducible para el código ya existente.

- [x] **STD-PERF-CONF-001 — Coordinar performance por owner.**
  [`testing/stdlib-performance-conformance.json`](./testing/stdlib-performance-conformance.json)
  contiene exactamente una fila por cada owner stdlib: las cinco rutas que el
  probe ya puede capturar (`std.json`, `std.messagepack`, `std.protobuf`,
  `std.math` y `std.testing`) declaran operación, workload, oracle y las
  dimensiones realmente observadas (throughput y tail latency); cada owner
  restante queda diferido con una razón explícita hasta tener identidad de hot
  path y baseline revisados. `scripts/stdlib-performance-conformance.sh`
  rechaza owners omitidos, dimensiones sobreafirmadas, entornos incompletos y
  muestras insuficientes; su test negativo conserva esas fronteras. La
  implementación sigue pendiente de los campos PERF completos por owner
  (allocations/memoria, startup, code size y compile time), comparación contra
  baselines revisadas y promoción, pero ya no existe una coordinación implícita
  ni una cifra verde agregada entre targets incompatibles.

- [x] **STD-CONF-001 — Coordinar conformidad por owner.**
  `testing/stdlib-conformance-coordination.json` materializa los 22 owners y
  las 385 filas de `STD-MATRIX-ALL-001` (214 firmas y 171 requisitos), con una
  entrada `CONF` explícita por fila, estado, razón, referencias y comandos.
  `scripts/stdlib-conformance-coordination-check.sh` regenera el registro,
  cruza matriz/API/owner evidence y exige que no existan filas implícitas,
  razones vacías, referencias inexistentes o comandos de script no ejecutables;
  su test negativo rechaza omisiones, sobreclaims y una coordinación siguiente
  obsoleta. `stdlib_conformance_coordination` replica la clausura en Rust.
  La ejecución pública de `STD-A-CONF-001` queda separada en
  `testing/stdlib-conformance.json` y su runner: todos los owners, sidecars y
  los 206 casos del draft se observan antes de promover. La coordinación y la
  matriz quedan promovidas para el draft; `STD-A-DIST-001` también queda
  promovido mediante su paquete reproducible y el sellado S1A permanece
  pendiente.

- [x] **STD-DOC-001 — Cerrar documentación por owner y programas
  representativos.** `testing/stdlib-documentation.json` registra los 22 owners
  con contrato, documentación normativa, estado de `kernel`/`bridge`/`public_api`
  y 32 ejemplos verificables. Hay 26 casos runtime/acceptance con sidecars
  `.exit` y `.stdout`/`.codes`, cuatro casos externos de codecs y dos casos de
  compiler/meta; `std.meta` y `std.reflect` declaran explícitamente que no les
  aplica un caso runtime. El registro distingue 14 APIs auditadas completas,
  cuatro parciales (incluidos los codecs y `std.serialization`) y cuatro
  intrínsecas/build-only sin filas públicas, sin promover gaps. El checker
  regenera el JSON, cruza matriz/conformance/API/owner evidence, valida
  comandos y sidecars, y sus negativos rechazan owners, ejemplos, sidecars o
  claims de API ausentes; `stdlib_documentation` replica la clausura en Rust.
  `docs/contracts/stdlib-s1a.md` fija el vocabulario de fronteras y mantiene el
  claim de draft no publicado; no se afirma una release ni una matriz verde.

Las coordinaciones anteriores prueban que cada gap tiene identidad, no que el
gap esté cerrado. Las siguientes leaves son las únicas que pueden promocionar
S1A; su estado se deriva de los registros machine-readable y no de este texto:

- [x] **STD-A-ASYNC-API-001 — Cerrar la superficie pública de `std.async`.**
  `docs/contracts/stdlib-async.md` y `testing/stdlib-async.json` fijan el
  efecto denotable `suspends`, la regla de inferencia solo en cuerpos presentes,
  `Join`, `Waiter`/`Completer`, `AsyncIterator`, `collect(limit:)`, cierre y
  backpressure sin `Channel`, que pertenece a STD-0.1B. La auditoría pública
  verificó las cinco firmas callable ejecutables de esa base (214/214 en total);
  DEC-020 ya está incorporada en la auditoría y en los hashes de interfaz;
  `STD-A-SELECTABLE-IMPL-001` queda cerrado, mientras la
  implementación genérica de iteradores, materialización y cancelación queda
  explícitamente en `STD-A-ASYNC-IMPL-001`.

- [x] **STD-A-ASYNC-IMPL-001 — Implementar y probar `std.async` completo.**
  Consumir `ASYNC-DEFER-IMPL-001` (cerrado) y `ASYNC-ITER-EXT-001`: la ruta
  pública ejecuta one-shot, streams genéricos, cancelación cooperativa, cierre
  por liberación del owner, límites y backpressure. `spawn cursor.collect(...)`
  transporta el witness concreto de `AsyncIterator.next` por HIR → MIR →
  bytecode → VM y conserva cursor/buffer como roots hasta el outcome terminal;
  límites negativos o de capacidad devuelven `CollectionError` sin array
  parcial y el límite alcanzado no hace un poll adicional. Los tests cubren
  cancelación al salir de `scope`, loans secuenciales y el rechazo de loans
  exclusivos en `spawn`.

- [x] **STD-A-SELECTABLE-IMPL-001 — Migrar one-shot y time-base al protocolo
  seleccionable.** `Waiter.wait`, `time.sleep` y `Timer.wait` usan ahora
  `selectable` en contratos, interfaces, hashes, HIR, lowering, runtime y
  auditorías públicas, sin renombrar métodos ni duplicarlos. La VM registra
  sus operaciones como brazos atómicos, conserva ownership, cancela/descarte
  perdedores y mantiene el resultado de la llamada directa. Evidencia:
  `testing/stdlib-public-api.json` (214/214), los checkers de `std.async` y
  `std.time`, tests estáticos de HIR y la fixture pública
  `tests/runtime/m11-std-async-selectable-001.to` (one-shot + timer).

- [x] **STD-A-FUZZ-001 — Cerrar todas las celdas FUZZ aplicables de S1A.** El
  target owner-aware `fuzz/fuzz_targets/stdlib_owners.rs` enruta las 22
  superficies mediante selector reproducible, límites fijos y oráculos
  ejecutables. `testing/stdlib-fuzz.json` registra target, corpus, seed,
  límites, replay y persistencia de regresiones; `scripts/stdlib-fuzz-check.sh`
  y su suite negativa comprueban rutas, corpus no vacíos, selectores y la
  evidencia promovida. `testing/stdlib-owner-evidence.json` y las matrices
  regeneradas muestran `FUZZ=verified` para los 22 owners (22/22, 0 partial).

- [x] **STD-A-PERF-001 — Capturar presupuestos completos por owner S1A.** El
  probe owner-aware (`crates/tondo-stdlib/examples/stdlib_performance_probe.rs`)
  cubre diez kernels portables (`std.bytes`, `std.format`, `std.io`, `std.json`,
  `std.math`, `std.messagepack`, `std.path`, `std.protobuf`,
  `std.serialization`, `std.testing`) en las seis workloads normativas, con
  27 muestras por identidad. El reporte captura las ocho dimensiones
  `throughput`, `tail_latency`, `allocations_per_operation`,
  `allocated_bytes_per_operation`, `peak_memory`, `startup`, `code_size` y
  `compile_time`; las asignaciones portables usan el oracle estable
  `logical-owned-buffer` y la campaña registra ambiente, binario, compilación,
  memoria y arranque. Los doce owners intrínsecos, build-only o host-provider
  sin hot path portable quedan `not-applicable` con razón normativa y frontera
  `PERF-001`; no hay dimensiones diferidas ni estados parciales. Los gates
  `scripts/stdlib-performance-{check,report,conformance}.sh`, sus negativos,
  `testing/stdlib-owner-evidence.json` y la matriz regenerada cierran la
  promoción.

- [x] **STD-A-CONF-001 — Ejecutar conformidad pública completa de S1A.**
  `scripts/stdlib-conformance.sh` valida y ejecuta el contrato público,
  conserva la procedencia del árbol y registra hashes de cada comando. Ejecuta
  los 22 owner commands, compara los sidecars de los 22 fixtures runtime
  únicos (incluido `args-unix`/`args-windows` cuando aplica), y ejecuta el
  corpus draft completo de 206 casos con el adapter de referencia. La evidencia
  queda en `target/reliability/evidence/stdlib-conformance.json`, mientras
  `testing/stdlib-conformance.json` conserva la identidad reproducible de 385
  filas/214 firmas/171 requisitos. `scripts/stdlib-conformance-check.sh` y su
  suite negativa ratifican revisión, hashes, owners, filas y matriz `CONF`;
  `STD-A-CONF-001` está promovido para el draft, sin convertirlo en release.

- [x] **STD-A-DIST-001 — Construir la distribución VM de STD-0.1A.**
  `scripts/stdlib-distribution.sh` crea dos snapshots limpios con `git archive`
  y produce el mismo paquete USTAR content-addressed en ambos. El paquete
  `tondo-std-0.1` incluye fuentes, interfaces, units/providers, el
  `PackageId` `toolchain:std:0.1-bootstrap`, hashes, manifests TOML, docs,
  matriz derivada de capabilities, ejemplos y el VM CLI. Su manifest verifica
  cada ruta, tamaño y SHA-256, y liga API, owner evidence y matriz normativa.
  La prueba extrae la distribución en una instalación separada, verifica los
  hashes antes de ejecutar `examples/m11-std-core-001.to` con `bin/tondo`,
  elimina los snapshots fuente y confirma que el ejemplo sigue funcionando;
  finalmente desinstala solo el package root y conserva un workspace vacío.
  `scripts/stdlib-distribution-check.sh` y
  `scripts/stdlib-distribution-test.sh` cierran el contrato y sus negativos.
  La evidencia queda en `target/reliability/evidence/stdlib-distribution/`;
  es una promoción del draft, no una publicación.

- [x] **STD-S1A-SEAL-001 — Sellar Gate S1A desde evidencia derivada.** Exigir
  `STD-A-ASYNC-IMPL-001`, `STD-A-SELECTABLE-IMPL-001`,
  `ASYNC-SELECT-VM-CONF-001`, `STD-A-FUZZ-001`, `STD-A-PERF-001`,
  `STD-A-CONF-001`, `STD-A-DIST-001`, auditoría pública estricta y cero celdas
  aplicables abiertas en la matriz. Emitir un bundle content-addressed separado
  de G5, del backend nativo y de TLF; no editar estados a mano. El contrato
  `testing/stdlib-s1a-seal.json`, `scripts/stdlib-s1a-seal.sh` y su verificador
  independiente producen y validan `tondo-stdlib-s1a-<payload-sha256>.tar`,
  ligado al Git HEAD limpio, a los reportes frescos y a la distribución VM;
  el bundle declara `public_release=false`, `g5=false`, `native_backend=false`
  y `tlf=false`.

### Gate S1A — Standard Library 0.1 foundation

- [x] La spec estándar fija todas las firmas de su catálogo Core + Hosted
  incluidas en STD-0.1A y mantiene cerrado el catálogo posterior de STD-0.1B.
- [x] Los slices tempranos de meta, reflect, bytes, time-base y env read-only
  conservan las mismas identidades y contratos usados por M10.7/M10.6; S1A no
  sustituye un shim ni mantiene dos propietarios públicos.
- [x] Cada owner A registra por separado spec, implementación/host, tests/model,
  performance aplicable, conformidad y docs; ninguna tarea umbrella oculta una
  celda pendiente. `PERF` está promovido o tiene una frontera normativa
  `not-applicable`, `CONF` está promovido mediante `STD-A-CONF-001` y la
  distribución VM está promovida mediante `STD-A-DIST-001`; el sello S1A es un
  bundle técnico independiente ya verificado.
- [x] El sustrato monotónico de `Duration`, `Instant`, suspensión, timers y
  deadlines es único para producción/testing, está modelado y funciona con
  proveedor real o virtual sin cambiar bytecode de usuario.
- [x] Toda la superficie Core se ejecuta sobre la VM sin depender de una ABI
  nativa; no basta con intrinsics y kernels parciales.
- [x] `select` y los adapters `Waiter`/time ejecutan prepare/commit/rollback
  sobre la VM con ownership y fairness conformes; una firma `suspends` antigua
  no satisface esta celda.
- [x] Cada API hosted exige la capability correcta y conserva los claims del
  target candidato Tondo 0.1.
- [x] `derive` de serialization, JSON, MessagePack y Protobuf schema-first se
  ejecutan sin reflection runtime, DOM intermedio obligatorio ni inputs
  ambientales.
- [x] Los codecs pasan interoperabilidad, fuzzing, streaming, límites,
  preservación y gates de rendimiento sobre oracle escalar y kernels
  optimizados.
- [x] Modelos, properties, ejemplos y conformidad estándar cubren sus contratos
  positivos, negativos, límites y composición.
- [x] La distribución de STD-0.1A es reproducible, cerrada y versionada con
  firmas, units y providers realmente implementados; `STD-A-DIST-001` prueba
  dos snapshots limpios, instalación, ejecución y desinstalación sin depender
  del árbol fuente.
- [x] `STD-S1A-SEAL-001` verifica cero gaps aplicables y reproduce el bundle
  desde inputs cerrados; ninguna coordinación `[x]` sustituye esta compuerta.
- [x] Los programas representativos pasan el gate estricto y proporcionan el
  corpus funcional inicial para `PERF-001`, `DIAG-*` y `NATIVE-001`.
- [x] `std.testing` está especificado, implementado y probado con su propio
  runner público; un proyecto puede escribir tests útiles usando solo
  `assert` y enriquecerlos mediante imports explícitos, sin crear un segundo
  formato de snapshots, artifacts o generated cases.
- [x] No se ha congelado una ABI FFI general ni un layout nativo público.

---

## 20. M11 — Backend nativo y optimización

**Objetivo:** añadir una implementación nativa de producción sin introducir una
segunda semántica. Comienza únicamente después de Gates H0, T0, G5 y S1A,
`DIAG-CI-001` y de cerrar los contratos runtime-facing
`STD-ASYNC-GROUP-SPEC-001`, `STD-CONC-001`,
`STD-SYNC-001`, `STD-EXEC-001` y la frontera host/cancelación de
`STD-NET-001`. Esos módulos no se implementan todavía, pero sus requisitos y
los perfiles `DIAG-*` alimentan elección de backend, memoria, debugging y ABI.
La VM, la conformidad del lenguaje —incluidos test targets— y la conformidad
de STD-0.1A son los oracles.

**Orden obligatorio:** UX de producto → target/artifact/link/publish schemas →
baseline → `DIAG-SPEC-001` → contratos runtime-facing B0 →
`DIAG-RUNTIME-001` → detectores/runner/CI → selección → memoria/ABI →
lowering por slices → `NATIVE-THREAD-001` (cerrado) → ARC/ciclos correctos
→ fronteras Core/Hosted de STD-0.1A → link/CLI → conformidad por slices →
diferencial/targets/empaquetado → Gate N1. Eliminación de retains,
COW, escape analysis, incrementalidad y LSP son trabajo posterior a N1 y no
pueden retrasar el primer backend correcto.

### 20.1 Selección y contrato del backend

- [x] **NATIVE-PRODUCT-SPEC-001 — Fijar la experiencia pública del backend.**
  La sección 10.1 de `TONDO_TOOLCHAIN_SPEC.md` define una sola forma
  `tondo build`, target/backend seleccionados por el plan, output físico no
  semántico, publicación atómica y `tondo run` sobre el mismo producto. El
  compilador puro emite un plan de enlace cerrado y el driver interno no usa
  shell, `PATH` ni flags ambientales; no se promete object format, linker
  configurable, dynamic linking o ABI FFI pública. Esta tarea cierra solo la
  UX pública; los schemas ejecutables del producto se cierran en las leaves
  siguientes.

- [x] **NATIVE-TARGET-DESC-001 — Definir el target descriptor nativo.** El
  formato `tondo-native-target-descriptor-draft` fija schema, canonicalización,
  identidad de backend y target/triple, object format, runtime ABI,
  capabilities/features, flags deterministas, driver/linker y artefactos de
  toolchain por SHA-256. `NativeTargetDescriptor` valida referencias de tipo,
  rechaza campos/path/expansiones ambientales y calcula la identidad de los
  bytes canónicos sin consultar `PATH` ni entorno. El contrato y negativos
  ejecutables viven en `testing/native-target-descriptor.json`,
  `docs/contracts/native-target-descriptor.md` y los dos scripts de gate. Este
  cierre solo fija inputs para artifact/link/publish; la selección de Cranelift
  se registra en `DEC-013`/`testing/native-selection.json` y este contrato no
  cierra S1A.

- [x] **NATIVE-ARTIFACT-001 — Definir la clausura de artefactos nativos.**
  `tondo-native-artifact-draft` extiende la clausura semántica del artifact
  draft con un grafo canónico de objetos, runtime, stdlib, unidades
  privilegiadas, producto final, productores y hashes. `NativeArtifact` exige
  inputs/intermediarios/output distinguibles, una única salida de `link`,
  reachability completa, DAG de producers, hashes de target/source y
  `artifact_hash` recalculable; no serializa layout, paths físicos ni ABI FFI.
  El contrato, negativos, docs y gates viven en
  `testing/native-artifact.json`, `docs/contracts/native-artifact.md` y los
  scripts dedicados. El siguiente bloque es `NATIVE-LINK-PLAN-001`.

- [x] **NATIVE-LINK-PLAN-001 — Definir el plan de enlace canónico.**
  `tondo-native-link-plan-draft` es un record cerrado, versionado y validable
  con inputs en orden semántico (objetos, unidades privilegiadas, runtime y
  stdlib), driver exacto hash-pinned con argumentos ordenados, output lógico y
  límites positivos. `NativeLinkPlan::validate_against` cruza descriptor y
  artifact para rechazar mezcla de targets, inputs, driver, formato o producto
  inconsistentes; paths físicos, shell, `PATH` y entorno quedan prohibidos.
  La identidad `plan_hash` se recalcula sobre el fingerprint canónico y la
  identidad de bytes es independiente. El contrato machine-readable, docs,
  negativos y gates viven en `testing/native-link-plan.json`,
  `docs/contracts/native-link-plan.md` y los scripts dedicados. El siguiente
  bloque es `NATIVE-PUBLISH-SPEC-001`.

- [x] **NATIVE-PUBLISH-SPEC-001 — Cerrar publicación y consumo del producto.**
  `tondo-native-publish-plan-draft` y
  `tondo-native-published-product-draft` son records canónicos, hash-pinned y
  reproducibles. `NativePublishPlan` cruza target/artifact/link, fija staging
  sibling, sincronización de archivos, commit atómico del par producto/receipt,
  colisiones, preservación del par anterior, límites y cleanup. El receipt se
  valida antes de decodificar por tamaño y después se liga a los bytes reales
  mediante SHA-256 y byte count antes de `tondo run`; directorios, symlinks,
  paths físicos, entorno y shell quedan fuera del contrato. La matriz de
  fallos entre validación, staging, fsync, commit, interrupción, cleanup y
  consumo queda enumerada en `testing/native-publish.json` y
  `docs/contracts/native-publish.md`; las decisiones puras están cubiertas por
  los scripts dedicados y los tests de `toolchain.rs`. La integración física
  del orquestador sigue reservada a `NATIVE-001`. Las leaves de la evaluación
  AOT ya están cerradas; Gate N1 ya está cerrado para Cranelift en el target
  primario.

- [x] **PERF-001 — Definir benchmarks y presupuestos antes de implementar.**
  El contrato global `tondo-performance/1` fija 14 workloads hash-pinned: cuatro
  de compilación y diez de runtime, de los que dos son límites adversarios,
  incluyendo el corpus
  STD-0.1A de core, collections, text, codecs, I/O, process y bytes, además de
  async y presión de memoria. Cada caso tiene clase, límites positivos finitos,
  dimensiones aplicables y backend explícito; el fixture se verifica por
  SHA-256. El protocolo fija reloj monotónico, 3 warmups, 9 muestras en 3
  procesos (27 mínimo), median/p95/p99 y entorno reproducible. Los presupuestos
  cubren compile time, code size, startup, throughput, latencia, allocations,
  bytes asignados, memoria pico, retain/release y pausas, comparando únicamente
  la misma identidad y sin agregación entre targets/backends. La VM hosted es
  la baseline requerida; `native-aot` usa Cranelift como backend seleccionado
  para el target admitido, con LLVM conservado como comparativa experimental, y
  el carril rápido de `NATIVE-001` solo captura compile-time/code-size y no se
  usa para la decisión final. La captura completa de productos enlazados queda
  cerrada por `NATIVE-AOT-PERF-001`; no se inventan cifras en el contrato.
  El registro, documentación, negativos y gates viven
  en `testing/performance.json`, `docs/contracts/performance.md`,
  `scripts/performance-check.sh` y `scripts/performance-test.sh`, integrados en
  `scripts/test-gate.sh`. Este contrato y el seal S1A desbloquearon
  `DIAG-SPEC-001`; su contrato D0 está ahora cerrado y la campaña AOT completa
  también está cerrada; `NATIVE-001` y `DEC-013` han cerrado la selección de
  Cranelift después de la compuerta de diagnóstico y el adaptador común ya está cerrado;
  la lane física de `NATIVE-THREAD-001` y `NATIVE-002` están cerradas;
  `ARC-001`, `ARC-002` y `DIAG-NATIVE-001` están cerrados; la frontera Core
  nativa también está cerrada y Gate N1 ya compone su evidencia de promoción.

- [x] **DIAG-SPEC-001 — Cerrar el contrato unificado de diagnóstico dinámico.**
  Fijar profiles `race`, `leaks` y `crash`, el envelope
  `tondo-diagnostic-report/1`, el dump `tondo-dump/1`, exit status, límites,
  privacidad, identidad por target/backend/toolchain y la forma CLI de
  `tondo run/test --diagnostics` y `tondo dump analyze`. Mantener intacto el
  schema de diagnostics de compilación y no añadir keywords ni APIs paralelas
  en `std`. El registro `testing/diagnostic-tooling.json`, los checks
  `scripts/diagnostic-contract-check.sh`/`diagnostic-contract-test.sh`, el
  contrato y RFC congelan la superficie y sus negativos; la instrumentación
  VM hosted queda implementada en `DIAG-RUNTIME-001`; los detectores hosted
  están cerrados en `RACE-001`, `LEAK-001`, `DUMP-001` y `DIAG-TEST-001`, y su
  paridad lógica nativa queda cerrada por `DIAG-NATIVE-001`.

- [x] **DIAG-RUNTIME-001 — Exponer instrumentación interna verificable.**
  Después de los contratos runtime-facing B0, la VM hosted registra task/thread
  IDs estables, accesos `Read`/`Write`/`Move` a `BytecodePlace`, sincronización
  (spawn, park/wake, host, loans y select), roots/retainers, ledger de recursos
  opacos, source maps, eventos de scheduler y barreras de quiescencia. La
  instrumentación es opt-in mediante `execute_with_diagnostics`, no cambia la
  semántica normal y devuelve una traza `tondo-diagnostic-runtime/1` acotada.
  El collector falla cerrado al superar `max_events`; tail, roots y retainers
  informan truncación. Tests unitarios y runtime cubren límites, corrupción de
  configuración, aislamiento/determinismo, privacidad sin payloads y ruta
  normal sin collector. La evidencia machine-readable está en
  `testing/diagnostic-runtime.json`, el contrato en
  `docs/contracts/diagnostic-runtime.md` y los checks en
  `scripts/diagnostic-runtime-check.sh`/`diagnostic-runtime-test.sh`.

- [x] **RACE-001 — Implementar el detector dinámico de races hosted.** La VM
  registra identidad generacional de almacenamiento, hash estable de rutas,
  stacks de acceso/creación, lifecycle de tasks, spawn/wake/join/select y las
  fronteras internas de scheduler; `crates/tondo-vm/src/runtime/race.rs`
  analiza la traza con vector clocks y relaciones happens-before. Emite
  `clean`/`finding`/`unsupported`, conserva ambos accesos y la creation stack,
  y falla cerrado ante truncado, contexto ausente o límites de 100.000
  observaciones/findings. Tests positivos/negativos cubren conflicto, Join/Wake,
  locales, determinismo y límites; la evidencia está en
  `testing/diagnostic-race.json`, `docs/contracts/diagnostic-race.md` y
  `scripts/diagnostic-race-{check,test}.sh`. El alcance es la VM hosted y las
  primitivas internas; adapters públicos channel/sync/executor/net siguen en
  `DIAG-STDLIB-001` y la paridad lógica nativa está cerrada en
  `DIAG-NATIVE-001`.

- [x] **LEAK-001 — Implementar el detector de retención y recursos.** La VM
  hosted consume snapshots de roots/retainers cerrados por quiescencia y separa
  objetos gestionados todavía alcanzables, recursos afines sin terminal,
  asignaciones identificadas como FFI/native y crecimiento sostenido. Exige tres
  snapshots crecientes por defecto para marcar retención, conserva IDs
  generacionales, tamaños, owners y stacks de asignación/adquisición, y no
  marca ciclos que el GC recupera ni un único valor devuelto. Truncado,
  quiescencia incompleta, raíces sin asignación y límites producen
  `unsupported`. La evidencia está en
  `crates/tondo-vm/src/runtime/leak.rs`,
  `testing/diagnostic-leak.json`,
  `docs/contracts/diagnostic-leak.md` y
  `scripts/diagnostic-leak-{check,test}.sh`; ARC/ciclos/FFI nativos reales y
  sus envelopes comparables quedan cubiertos por `DIAG-NATIVE-001`.

- [x] **DUMP-001 — Implementar crash dumps y analizador lógico hosted.** El
  writer `DumpArtifact` captura un `.tdump` versionado con razón,
  target/backend/toolchain, identidad por intento, stacks lógicos de tasks y
  threads, roots/heap summary, resource ledger, scheduler tail y
  source-maps/retainers opcionales. El envelope canónico está content-addressed
  con SHA-256, aplica redacción por defecto, rechaza formatos/secciones/hashes
  corruptos y el CLI `tondo dump analyze` ofrece vistas human/JSON offline.
  Registros físicos y la ruta async-signal-safe se declaran por target; la
  paridad lógica de dump, unwind/source-map summary, redacción, corrupción y
  límites queda cerrada en `DIAG-NATIVE-001`.

- [x] **DIAG-TEST-001 — Integrar perfiles en `tondo test`.** `--diagnostics`
  acepta únicamente `race`, `leaks`, `crash` o `all`; cada retry, repeat,
  shard y suite se ejecuta en un worker limpio y recibe un `run_id`/
  `attempt_id` determinista. Los `DiagnosticRecord` por intento preservan
  identidad, estado `clean`/`finding`/`unsupported`/`failed`, limitaciones,
  exit statuses y política de privacidad; los dumps `.tdump` se incorporan al
  artifact store SHA-256 con descriptor compartido por JSON/JUnit. Los límites
  de 16 MiB por reporte y 256 MiB por dump son fail-closed, un worker `/1`
  antiguo se rechaza como infraestructura y setup/teardown bloqueados no
  pierden su evidencia. La evidencia ejecutable vive en
  `testing/diagnostic-test.json`, `docs/contracts/diagnostic-test.md` y
  `scripts/diagnostic-test-{check,test}.sh`; la siguiente frontera es
  `NATIVE-001`.

- [x] **DIAG-CI-001 — Cerrar las lanes y gates de diagnóstico.** La workflow
  opt-in ejecuta `race`, `leaks`, `crash` o `all` con procesos frescos, corpus
  positivo/negativo persistente, fuzzing acotado con toolchain fijado, budgets
  fail-closed y promotion gate. La campaña no modifica el baseline de
  coverage/mutation/performance normal y nunca acepta verde cuando el perfil
  requerido está `unsupported`. La evidencia ejecutable vive en
  `testing/diagnostic-ci.json`, `docs/contracts/diagnostic-ci.md`,
  `scripts/diagnostic-ci-{check,test}.sh`, `scripts/diagnostic-ci.sh`,
  `scripts/diagnostic-fuzz.sh` y `.github/workflows/diagnostics.yml`.

- [x] **NATIVE-001 — Evaluar candidatos nativos y registrar la decisión.**
  Cerrado como frontera de evidencia y decisión para el target admitido:
  Cranelift y LLVM consumen el mismo probe
  `tondo-native-mir-probe/1`/`tondo-mir-backend/1` y el target
  `x86_64-unknown-linux-gnu`. El carril rápido captura tres muestras de
  compile-time/code-size por fixture; el runner físico pasa 118 casos escalares,
  3 managed, 21 runtime, 8 select, 5 threads, 14 `std.core`, 1 lowering y 8
  diagnósticos en ambos candidatos contra el oráculo VM. La matriz y los hashes
  de informes quedan en `testing/native-selection.json`,
  `docs/contracts/native-selection.md`,
  `scripts/native-selection-{check,capture}.sh` y
  `target/reliability/evidence/native-selection.json`. `DEC-013` selecciona
  Cranelift para native AOT en este target; LLVM queda como comparativa
  experimental, sin fallback silencioso. La evidencia AOT se cerró mediante
  `NATIVE-AOT-SCOPE-001` → lowering completo → artefactos enlazados
  normalizados → memoria/calidad → rendimiento completo. Gate N1 ya promueve la
  implementación seleccionada para el target primario.

- [x] **NATIVE-BACKEND-ADAPTER-001 — Sustituir el smoke adapter por lowering
  real común.** La primera slice ya consume `tondo-mir-backend/1` desde el MIR
  verificado y conecta Cranelift y LLVM al mismo lowering común de escalares,
  operadores lógicos, conversiones checked, calls directas y host/prelude,
  carriers gestionados/agregados opacos, control-flow normal (incluidos
  locals acarreados por loops), `Option`/`Result` tags, traps y edges de
  cleanup/ownership/async/select. Los elementos cuyo storage o ABI aún no está
  cerrado producen trap y estado `unsupported`, nunca una aproximación
  silenciosa; las proyecciones de storage y `IteratorNext` quedan
  explícitamente en esa frontera para las futuras ABI nativas de valores y
  colecciones. El runner opt-in
  `native-evaluation-runner/1` genera y ejecuta objetos Cranelift/LLVM con
  `cc` explícito: 118 casos escalares, 3 managed-result, 21 runtime-contract,
  8 de selección y 5 de workers-thread comparan resultados, errores y traps con el oráculo MIR y una
  invocación directa de la VM. La cobertura de compilación, oráculo y límites
  está fijada por tests unitarios y por el informe hash-bound; el carril rápido
  sigue midiendo solo compile-time/code-size y Cranelift es ahora la ruta
  seleccionada para el target admitido, con promoción bloqueada por N1. La evidencia física de
  `NATIVE-THREAD-001` y la coordinación mínima de `NATIVE-002` están cerradas;
  `ARC-001`, `ARC-002` y `DIAG-NATIVE-001` están cerrados. La evidencia de
  `NATIVE-STD-CORE-001` añade catorce casos nativos (Option/Result); las
  compuertas AOT ya están cerradas y alimentan el Gate N1 ya promovido.

- [x] **NATIVE-MEM-ADR-001 — Cerrar DEC-014 antes de la ABI.** La decisión
  queda cerrada como `hybrid-arc-cycle-collector`: contadores no atómicos para
  valores no compartidos, atómicos al cruzar `Send`/`Share`, trial deletion bajo
  presión/quiescencia, weak edges gestionados por runtime, roots explícitos de
  stack/task/thread/async-frame/host-handle, cleanup determinista de recursos,
  cancelación antes del estado terminal y COW solo tras comprobar unicidad. El
  contrato no expone layout. La decisión typed, su identidad canónica y sus
  negativos están en `docs/contracts/native-memory.md`,
  `testing/native-memory.json`, `crates/tondo-compiler/src/toolchain.rs` y
  `scripts/native-memory-{check,test}.sh`; la instrumentación del runtime y la
  paridad lógica native/VM están cerradas por `DIAG-NATIVE-001`; la capacidad
  física por target y los leaves de stdlib siguen pendientes.

- [x] **NATIVE-ABI-001 — Definir una ABI runtime interna y versionada.**
  `tondo-native-runtime-abi/1` fija la calling convention de direct calls
  verificados, el result record scalar/runtime, edges MIR de retain/release y
  terminales de recursos, unwind normal/abort, frames y wakers async, IDs de
  source/task/thread/crash, handles host opacos y visibilidad solo
  compiler/runtime. La ABI no promete FFI, layout de usuario ni name mangling.
  El contrato typed y sus negativos están en `docs/contracts/native-abi.md`,
  `testing/native-abi.json`, `crates/tondo-compiler/src/toolchain.rs` y
  `scripts/native-abi-{check,test}.sh`.

- [x] **NATIVE-LOWER-CALLS-001 — Lowering de ABI y llamadas.** La slice
  ejecuta funciones escalares reales con parámetros, retornos y direct calls
  ordinal-resueltos en Cranelift y LLVM, comparados contra VM y oráculo; los
  argumentos que son borrows directos de escalares se leen sin exponer
  punteros. El result record administrado (con tags y payloads `Option`/`Result`)
  y la ruta de llamadas host comparten el mismo carrier opaco y tienen casos
  ejecutables en `tools/native-evaluation/src/main.rs`, con observaciones VM en
  `crates/tondo-compiler/examples/native_mir_probe.rs`. Los objetivos
  indirectos, protocolos no soportados y targets desconocidos producen trap
  fail-closed. Evidencia: `scripts/native-evaluation-runner.sh` y
  `target/reliability/evidence/native-evaluation-runner.json`.

- [x] **NATIVE-LOWER-CONTROL-001 — Lowering de control y operaciones checked.**
  Branches, loops, joins, overflow e invalid shifts de la slice escalar se
  bajan y se comparan con la VM; la dispatch de tags para `Option`/`Result`,
  los explicit-panic traps, `assert` checked y bounds checked tienen lowering
  y pruebas propias en Cranelift, LLVM y el oráculo. El corpus de pánicos cubre
  overflow signed, división/remainder inválidos, shifts fuera de rango,
  asserts y límites negativos/pasados del final. Evidencia: los tests
  `checked_bounds_share_trap_policy_across_oracle_cranelift_and_llvm` y
  `native_control_panic_corpus_covers_arithmetic_shift_assert_and_bounds_edges`
  de `tools/native-evaluation/src/main.rs`.

- [x] **NATIVE-LOWER-CLEANUP-001 — Lowering de pánico y cleanup.** El ABI y la
  decisión de memoria exigen cleanup exactamente una vez y cancellation antes
  del estado terminal. Los edges runtime de `frame-enter`, `register-defer`,
  `frame-cleanup` y `frame-leave` se bajan en ambos backends; el runner ejecuta
  salidas normales idempotentes y abortadas, verificando el estado terminal y
  el código de doble cleanup. La ruta completa de `defer` fuente conserva su
  rechazo explícito hasta el lowering de ownership, pero ya no se aproxima ni
  se omite silenciosamente. Evidencia: casos `cleanup-exactly-once` y
  `cleanup-abort` de `run_native_runtime_probe` y las pruebas del runtime
  nativo.

- [x] **NATIVE-LOWER-OWNERSHIP-001 — Lowering de ownership y préstamos.** La
  frontera de ABI reserva los edges MIR y la política de memoria define
  retains/releases, weak edges y COW; la slice nativa materializa los retains,
  releases y `cow-clone` como llamadas runtime sobre handles opacos. La prueba
  `ownership-cow` comparte un valor, clona solo al detectar aliasing, libera el
  original exactamente dos veces y valida tag/payload del clon en Cranelift y
  LLVM; los borrows directos siguen siendo lecturas verificadas y las
  proyecciones/escapes se rechazan. Evidencia: `run_native_runtime_probe`,
  `tondo-native-runtime` y el contrato de memoria.

- [x] **NATIVE-LOWER-ASYNC-001 — Lowering de async estructurado.** El contrato
  fija publicación de roots antes de suspender y el registro frame/task/waker.
  Las operaciones runtime de `scope-enter`, `scope-spawn`, `task-spawn`,
  `task-poll`, `task-wake`, `await`, `scope-join` y `scope-cancel` se bajan en
  Cranelift y LLVM. El runner ejecuta await de una tarea despertada, join
  estructurado, cancelación de scope antes del estado terminal y transición
  pending→ready, con cancelación propagada a las tareas del scope. Evidencia:
  casos `async-await`, `async-structured-join`, `async-scope-cancel` y
  `async-task-progress` de `run_native_runtime_probe`, además del negativo
  `async-cancel-wake-rejected` y las pruebas unitarias de transiciones inválidas
  del runtime seguro.

- [x] **NATIVE-SELECT-001 — Implementar selección atómica nativa.** La misma
  máquina prepare/register/commit/rollback de la VM está bajada a la ABI nativa
  con un límite de 64 brazos y una linearización de un solo lock. Integra
  wakeups de task/thread y adapters de time/one-shot/Join, conserva fairness
  round-robin y ownership por rama, y rechaza fases, fuentes, capacidades y
  registros duplicados inválidos. El commit pendiente no hace polling ni
  bloquea workers; `else`, rollback y `take` mantienen las reglas de ownership
  de la VM. Ocho casos ejecutables (`select-ready-join`, `select-pending-wakeup`,
  `select-round-robin`, `select-rollback-ownership`, `select-oneshot`,
  `select-time`, `select-thread-join`, `select-else`) pasan en Cranelift y LLVM
  y se comparan con el corpus VM `ASYNC-SELECT-VM-CONF-001`. Evidencia:
  `testing/native-select.json`, `docs/contracts/native-abi.md`,
  `docs/contracts/native-evaluation.md`, `scripts/native-select-{check,test}.sh`
  y el campo `native_select_runs` de
  `target/reliability/evidence/native-evaluation-runner.json`. El shim C es
  solo un arnés diferencial determinista; no sustituye al runtime ni decide el
  backend final.

- [x] **NATIVE-LOWER-DEBUG-001 — Preservar identidad y source maps.** El
  lowering normalizado emite `tondo-mir-debug/1` con inventario de fuentes
  lógicas path-free y hash de contenido, símbolos MIR→native, regiones de
  función/bloque/statement/terminator, sucesores de unwind e identidades
  deterministas de task/thread. El driver resuelve nombres sin serializar
  `SourceId` ni paths físicos; Cranelift/LLVM validan la metadata antes de
  generar código y LLVM conserva registros debug path-free junto a cada
  símbolo. Faltas, duplicados, rangos fuera de límites, unwind desconocido y
  tipos de ejecución inválidos fallan cerrado. Evidencia: `docs/contracts/native-lowering-debug.md`,
  `testing/native-lowering-debug.json`, los tests unitarios de `mir.rs` y
  `tools/native-evaluation/src/main.rs`, más los informes generados por
  `native-evaluation`/`native-evaluation-runner`.

- [x] **NATIVE-002 — Coordinar el lowering mínimo desde MIR.** Cerrado tras
  `NATIVE-LOWER-CALLS-001`, `NATIVE-LOWER-CONTROL-001`,
  `NATIVE-LOWER-CLEANUP-001`, `NATIVE-LOWER-OWNERSHIP-001`,
  `NATIVE-LOWER-ASYNC-001`, `NATIVE-LOWER-DEBUG-001` y
  `NATIVE-SELECT-001` y `NATIVE-THREAD-001`. El coordinador consume el MIR
  normalizado una vez y lo baja por Cranelift y LLVM con la misma metadata;
  `spawn call()` directo publica un handle `Pending` y el primer `Join` evalúa
  el cuerpo una sola vez, lo completa mediante `tondo_rt_task_complete` y lo
  consume por `await`. El alcance de esta slice queda limitado a capturas
  escalares constantes e inmutables; capturas mutables, closures, storage
  proyectado y la lane física de `thread` fallan cerrado o conservan su barrera
  explícita. El runner diferencial añade el caso `deferred-task-call` a la
  evidencia Cranelift/LLVM y mantiene las 21 runtime, 8 select y 5 thread
  cases previas. Evidencia: `testing/native-lowering.json`,
  `docs/contracts/native-lowering.md`, `scripts/native-lowering-{check,test}.sh`,
  `crates/tondo-native-runtime/src/lib.rs` y el campo
  `native_lowering_runs` de `target/reliability/evidence/native-evaluation-runner.json`.

### 20.2 Runtime correcto y frontera estándar

- [x] **ARC-001 — Implementar ARC correcto en el runtime nativo.** Cerrado en
  `crates/tondo-native-runtime/src/lib.rs`: los valores no compartidos usan
  contadores `u32` comprobados y los que cruzan `Send`/`Share` migran a
  `AtomicU32` con actualización acquire/release. Las aristas de payload se
  retienen al publicarse y se transfieren al consumir o se liberan antes de
  cancelación, scope-drop y terminales; frames normales y abortados comparten
  cleanup exacto, scopes retienen/cancelan hijos, `select` retiene registros y
  los workers conservan un runtime-root hasta su terminal lógico. Los tests del
  runtime cubren ownership local/cross-thread, pánicos/unwind, cleanup,
  frames async, selección y terminales sin leaks; la evidencia contractual es
  `testing/native-arc.json` y `docs/contracts/native-arc.md`.

- [x] **ARC-002 — Implementar recolección diferida de ciclos y weak refs
  linealizables.** Cerrado en `crates/tondo-native-runtime/src/lib.rs`: el
  collector de trial deletion conserva ciclos anclados por roots y recupera
  componentes sin owners tanto en quiescencia explícita como bajo presión de
  256 allocations; los weak handles usan metadata de tombstone y
  `weak_upgrade` tiene una linealización acquire que impide la resurrección.
  Las upgrades concurrentes liberan exactamente un strong por éxito, no hay
  finalizers de usuario, y los workers cancelan antes de descartar su estado.
  La evidencia ejecutable es `testing/native-arc.json`,
  `scripts/native-arc-{check,test}.sh` y los casos
  `arc-rooted-cycle-preservation`, `arc-cycle-pressure-and-quiescence` y
  `arc-weak-upgrade-linearization`.

- [x] **DIAG-NATIVE-001 — Demostrar paridad nativa de diagnóstico.** Cerrado
  después de `NATIVE-002` y `ARC-002`, con `NATIVE-THREAD-001` ya cerrado. El
  runner `scripts/native-diagnostics.sh` ejecuta el corpus contra objetos y
  procesos reales de Cranelift y LLVM, tomando los contratos hosted como oracle
  y comparando exactamente el envelope portable `tondo-diagnostic-report/1`.
  Los ocho casos (`race-conflict`, `race-clean`, `leak-growth`, `leak-clean`,
  `arc-cycle-reclaimed`, `crash-dump`, `crash-corruption-rejected` y
  `crash-limit-enforced`) verifican IDs lógicos de task/thread,
  happens-before, roots/retainers ARC, ciclos recuperados, allocations FFI,
  ledger de recursos, unwind, source maps, redacción, corrupción y límites.
  `testing/native-diagnostics.json` y `docs/contracts/native-diagnostics.md`
  fijan la ABI privada, el reporte y los negativos; `native_diagnostics` en
  `target/reliability/evidence/native-evaluation-runner.json` conserva la
  evidencia de ambos backends. No se exigen layouts o stacks físicos idénticos,
  y una captura de señal física solo se declara cuando el target la soporta.
  `NATIVE-STD-CORE-001` queda cerrado con la evidencia descrita a continuación;
  el siguiente bloque es `NATIVE-STD-HOSTED-001`.

- [x] **NATIVE-STD-CORE-001 — Implementar la frontera Core de STD-0.1A.** Cerrado
  con el MIR nativo normalizado y la evidencia ejecutable de ambos candidatos.
  `Option.some`, `Option.none`, `Option.unwrapOr`, `Option.map`, `Result.ok`,
  `Result.err`, `Result.unwrapOr`, `Result.map` y `Result.mapErr` conservan los
  tags, payloads, fallbacks, callbacks por rama y ownership observable de la VM.
  Las proyecciones de payload de profundidad uno se etiquetan como
  `option-value`, `result-ok-value` o `result-err-value` y cruzan únicamente
  `tondo_rt_result_payload`; las proyecciones de storage siguen fail-closed.
  Los callbacks nombrados se resuelven a llamadas directas durante la
  normalización, sin inventar una ABI de function pointers. El fixture
  `tests/native/native-std-core-001.to` y `native_std_core_runs` prueban los
  catorce casos en procesos Cranelift y LLVM frescos, comparándolos con el
  oráculo MIR y las observaciones VM. El contrato es
  `testing/native-std-core.json`, con checks estáticos/negativos en
  `scripts/native-std-core-{check,test}.sh`; la siguiente frontera es
  `NATIVE-STD-HOSTED-001`.

- [x] **NATIVE-STD-HOSTED-001 — Implementar la frontera Hosted de STD-0.1A.**
  Cerrado con una frontera runtime real y acotada: capabilities explícitas
  (`console`, `filesystem`, `process`, `clock`), handles host afines opacos,
  buffers inmutables sin punteros, I/O parcial, cancelación terminal, errores
  tipados y cleanup exactamente una vez. El proveedor bootstrap usa fixtures
  deterministas y no consulta PATH, cwd, entorno ni descriptores ambientales;
  la ABI fail-closed valida también handles stale, índices y límites. La
  evidencia ejecutable queda en `testing/native-std-hosted.json`,
  `docs/contracts/native-std-hosted.md`,
  `scripts/native-std-hosted-{check,test}.sh` y el informe generado
  `target/reliability/evidence/native-std-hosted.json`; el siguiente bloque es
  `NATIVE-STD-001`.

- [x] **NATIVE-STD-001 — Coordinar la frontera completa de STD-0.1A.** Cerrado
  con una coordinación ejecutable de `std.core` y `std.hosted`: ambos owners
  validan sus contratos de forma independiente y después comparan carrier
  `tondo_rt_result_new/tag/payload`, tags de error, admission de capabilities,
  ownership y cleanup. Cranelift y LLVM quedan registrados como rutas del
  mismo `tondo-mir-backend/1`, sin API pública específica del backend ni lookup
  ambiental. La evidencia queda en `testing/native-std.json`,
  `docs/contracts/native-std.md`, `scripts/native-std-{check,test}.sh` y
  `target/reliability/evidence/native-std.json`; el siguiente bloque es
  `NATIVE-LINK-001`.

- [x] **NATIVE-LINK-001 — Implementar el plan de enlace cerrado.** Cerrado con
  una prueba física sobre los contratos tipados: descriptor, artifact y
  `NativeLinkPlan` se validan antes de resolver inputs; el driver absoluto se
  invoca directamente con argumentos ordenados, sin shell ni búsqueda de PATH,
  y el producto se comprueba antes de publicarse. Dos workspaces limpios
  generan el mismo ejecutable y SHA-256 con `--build-id=none`; un driver
  relativo, hashes divergentes, límites y salidas no válidas fallan cerrado.
  La evidencia está en `testing/native-link.json`,
  `docs/contracts/native-link.md`, `scripts/native-link-{check,test}.sh` y
  `target/reliability/evidence/native-link.json`; sigue `NATIVE-CLI-001`.

- [x] **NATIVE-CLI-001 — Conectar `tondo build` y `tondo run` nativo.** Cerrado
  con el comando `build` en la CLI compartiendo discovery TOML/lock y el
  frontend común: publica atómicamente el artifact canónico y un envelope
  `tondo-native-build/1` con backend `cranelift` seleccionado y promoción
  `pending-gate-n1`. `run` conserva stdout, stderr, argv, exits y diagnostics
  en la ruta existente, sin flags `--native`/`--vm` ni semánticas duplicadas.
  La integración verifica repetición byte-a-byte, output existente, argumentos
  y rechazo de opciones prohibidas; los productos parciales se limpian. Evidencia en
  `testing/native-cli.json`, `docs/contracts/native-cli.md`,
  `scripts/native-cli-{check,test}.sh` y el código de `crates/tondo-cli`;
  sigue `NATIVE-CONF-ADAPTER-001`.

### 20.3 Oracle diferencial, targets y empaquetado

- [x] **NATIVE-CONF-ADAPTER-001 — Crear el adaptador nativo.** Cerrado con el
  protocolo `tondo-native-observation/1`: recibe el probe común
  `tondo-mir-backend/1`, backend/target/capabilities explícitos y emite
  observaciones normalizadas de valores, errores, diagnostics, lifecycle de
  tests y cleanup. Backend, target, capability y shapes desconocidos fallan
  cerrado; los informes no contienen paths físicos. Cada owner se ejecuta por
  separado para Cranelift y LLVM. Evidencia en
  `testing/native-conf-adapter.json`, `testing/native-conf-probe.json`,
  `docs/contracts/native-conf-adapter.md` y
  `scripts/native-conf-adapter-{check,test}.sh`; siguen las tres hojas
  `NATIVE-CONF-*`.

- [x] **NATIVE-CONF-LANGUAGE-001 — Ejecutar conformidad base nativa.** Cerrado
  con tres casos del probe común (`scalar`, `Result` con error y panic) en
  Cranelift y LLVM, comparados contra el oráculo VM de forma independiente.
  Backend/target son explícitos, los tags/diagnostics deben coincidir y las
  rutas físicas se redactan. Evidencia en `testing/native-conf-language.json`,
  `docs/contracts/native-conf-language.md`,
  `scripts/native-conf-language-{check,test}.sh` y
  `target/reliability/evidence/native-conf-language.json`.

- [x] **NATIVE-CONF-TESTING-001 — Ejecutar test targets nativos.** Cerrado con
  los casos de pass/fail/aislamiento del protocolo del runner: logs, `P0007`,
  exits, fresh-process y cleanup exactamente una vez se observan de forma
  independiente en Cranelift y LLVM frente al oráculo VM. Evidencia en
  `testing/native-conf-testing.json`, `docs/contracts/native-conf-testing.md`,
  `scripts/native-conf-testing-{check,test}.sh` y
  `target/reliability/evidence/native-conf-testing.json`.

- [x] **NATIVE-CONF-STDLIB-001 — Ejecutar STD-0.1A nativa.** Cerrado con los
  owners `std.core` y `std.hosted`, las capabilities `console`, `filesystem`,
  `process` y `clock`, y los casos de core, hosted y cleanup en Cranelift y
  LLVM. Cada observación conserva el oráculo VM, bytes parciales, tags de
  error y release exactamente una vez; los informes no contienen paths físicos.
  Evidencia en `testing/native-conf-stdlib.json`,
  `docs/contracts/native-conf-stdlib.md`,
  `scripts/native-conf-stdlib-{check,test}.sh` y
  `target/reliability/evidence/native-conf-stdlib.json`; sigue
  `NATIVE-CONF-001`.

- [x] **NATIVE-CONF-001 — Coordinar conformidad nativa.** Cerrado con un
  coordinador que ejecuta adaptador, lenguaje, testing y STD-0.1A de forma
  independiente para Cranelift y LLVM, y compara las nueve observaciones con
  el oráculo VM común. La coordinación rechaza hojas ausentes, divergencias,
  duplicados, targets/MIR distintos y paths físicos; la evidencia path-free
  queda en `testing/native-conf.json`, `docs/contracts/native-conf.md`,
  `scripts/native-conf-{check,test}.sh` y
  `target/reliability/evidence/native-conf.json`; sigue
  `NATIVE-DIFF-001`.

- [x] **NATIVE-DIFF-001 — Ejecutar differential testing generado.** Cerrado
  con un generador determinista de las nueve observaciones del probe común:
  cada caso se ejecuta para Cranelift y LLVM, se compara con el oráculo VM y
  se exige igualdad de IDs, valores, tags, diagnostics, cleanup y redacción.
  El harness prueba también que una mutación del oráculo falla cerrado. La
  evidencia está en `testing/native-diff.json`,
  `docs/contracts/native-diff.md`, `scripts/native-diff-{check,test}.sh` y
  `target/reliability/evidence/native-diff.json`. El carril físico completo
  de `native-evaluation-runner.sh` es opt-in y sigue siendo evidencia de
  `NATIVE-001`; las campañas AOT posteriores de alcance, lowering, binarios,
  memoria, calidad y rendimiento ya están cerradas para este target y el
  informe compositivo de Gate N1 promueve Cranelift para el target primario.

- [x] **NATIVE-TARGET-001 — Añadir targets uno a uno.** Cerrado para el primer
  target físico admitido, `x86_64-unknown-linux-gnu`/ELF `release`, con registry
  explícito, capacidades, candidatos, fixture y artefacto identificable. El
  runner valida el descriptor, enlaza con driver absoluto y ejecuta el fixture
  sobre la arquitectura destino en un workspace limpio; cross-compilar no se
  cuenta como smoke. Evidencia en `testing/native-target.json`,
  `docs/contracts/native-target.md`, `scripts/native-target-{check,test}.sh` y
  `target/reliability/evidence/native-target.json`; sigue
  `NATIVE-REL-001`.

- [x] **NATIVE-TARGET-002 — Añadir el target físico Linux ARM64.** Cerrado como
  una entrada de target independiente para
  `aarch64-unknown-linux-gnu`/ELF `release`. El runner nativo
  `ubuntu-24.04-arm` ejecuta el fixture determinista con driver absoluto,
  verifica el producto no vacío, conserva hashes de contrato/descriptor y
  publica un informe path-free junto a los artefactos portables. La lane
  reutiliza el probe Cranelift de host y no cuenta cross-compilation como
  smoke; el target sigue siendo candidato de N1 y no altera la entrada GNU
  x86_64 ni añade soporte musl. Evidencia en
  `testing/native-target-aarch64.json`,
  `scripts/native-target-aarch64-{check,contract-test,test}.sh`,
  `docs/contracts/native-target.md` y el informe `native-target.json` del
  artefacto `portable-testing-linux-aarch64`; Gate N1 queda cerrado por la
  evidencia composicional y ARM64 sigue siendo únicamente un smoke físico de
  candidato hasta completar su corpus AOT.

- [x] **NATIVE-REL-001 — Empaquetar builds reproducibles.** Cerrado con un
  paquete `tondo-native-package/1` que contiene binario, runtime, STD-0.1A,
  metadatos y checksums del target admitido. Dos builds en staging aislado,
  con tar determinista (mtime epoch-zero, owners numéricos cero y entradas
  ordenadas), producen bytes idénticos y rechazan drift, paths, timestamps o
  paquetes parciales. Evidencia en `testing/native-rel.json`,
  `docs/contracts/native-rel.md`, `scripts/native-rel-{check,test}.sh` y
  `target/reliability/evidence/native-rel.json`; el paquete identifica
  Cranelift como backend seleccionado y mantiene `pending-gate-n1` como estado
  histórico del paquete candidato. Gate N1 registra la promoción del backend,
  pero este paquete no se presenta como STD 0.1.0 final.

#### Campaña AOT y promoción del backend (DEC-013 y Gate N1 cerrados)

- [x] **NATIVE-AOT-SCOPE-001 — Fijar el alcance AOT y la matriz de decisión.**
  Cerrado en `testing/native-aot-scope.json` y
  `docs/contracts/native-aot-scope.md`: `native-aot` es el producto primario
  de Tondo 0.1, `tondo-vm-hosted` es la implementación de referencia/oráculo y
  JIT queda fuera del producto y de `DEC-013`. La matriz separa compilación,
  binario enlazado, arranque, runtime, memoria, diagnóstico, mantenimiento y
  distribución; liga cada observación a la misma identidad de target,
  toolchain, runtime y stdlib; y exige tamaños stripped/debug/section del
  producto final, sin mezclar buffers de código con objetos completos. También
  fija la política collector-neutral del lenguaje, el
  `hybrid-arc-cycle-collector` de AOT y el tracing GC de la VM. Los negativos
  de producto, candidatos, memoria, métricas, protocolo, selección y N1 pasan
  en `scripts/native-aot-scope-{check,test}.sh`; `DEC-013` selecciona
  Cranelift para el target admitido, sin convertir este contrato de alcance en
  una compuerta de promoción. Las leaves posteriores hasta
  `NATIVE-AOT-PERF-001` ya están cerradas y Gate N1 compone su evidencia.

- [x] **NATIVE-AOT-LOWER-001 — Completar el lowering AOT del MIR admitido.**
  Extender el lowering común más allá de la slice mínima de `NATIVE-002` hasta
  todo el corpus admitido por Tondo 0.1: storage concreto de valores y
  colecciones, proyecciones, closures y capturas mutables, calls indirectas
  verificadas, async/select/thread y cleanup/ownership completos. Cada caso
  debe ejecutar el mismo MIR en Cranelift y LLVM, comparar observables con la
  VM y fallar cerrado cuando una capacidad no esté admitida; no se acepta un
  stub que solo cambie el contador de funciones. Cerrado con el corpus AOT
  ejecutable de `testing/native-aot-lowering.json`: el runtime de handles ahora
  materializa arrays, sets, records y closures; las proyecciones leen campos,
  las capturas mutables se actualizan con `aggregate-set` y las llamadas
  indirectas pasan por ordinales de función verificados. Los 7 casos de storage,
  proyección, closure/captura, llamada directa/indirecta, metadata, set y
  ownership se ejecutan junto a los 20 casos de cleanup/async/select/thread
  existentes, siempre desde el mismo MIR en ambos candidatos y contra el
  oráculo de referencia. La evidencia conserva el inventario por familia y dos
  traps explícitos para storage opaco no admitido; no se modifica el alcance de
  N1 ni se promociona automáticamente el backend seleccionado. El siguiente bloque es
  `NATIVE-AOT-BINARY-001`.

- [x] **NATIVE-AOT-BINARY-001 — Medir el producto enlazado de forma comparable.**
  Generar para cada candidato el ejecutable AOT completo con el mismo target,
  runtime, stdlib, linker y perfil. Capturar por separado bytes del producto
  stripped, bytes con debug, secciones relevantes, startup y reproducibilidad,
  y ligar cada medida al receipt y a los hashes de inputs. Eliminar la actual
  asimetría de `NATIVE-001` (buffer de código Cranelift frente a objeto LLVM),
  rechazar productos parciales y conservar una comparación path-free. Cerrado
  con `testing/native-aot-binary.json`: cada candidato construye dos veces el
  mismo ejecutable con 29 funciones en su inventario (28 casos admitidos: 7 de
  storage/ABI y 21 de runtime, más un trap explícito para la capacidad no
  admitida), ejecuta el producto stripped en tres procesos frescos, captura
  bytes debug/stripped y secciones ELF, y publica receipts ligados a MIR,
  runtime, stdlib, target, linker, strip, readelf, flags y toolchain. Los
  hashes y secciones coinciden entre builds y ninguna ruta física entra en la
  evidencia; las compuertas de calidad y rendimiento AOT ya están cerradas y
  la evidencia alimenta el informe compositivo de Gate N1 para Cranelift.

- [x] **NATIVE-AOT-MEM-001 — Capturar memoria y ARC en AOT.** Cerrado con
  `testing/native-aot-memory.json`: cada candidato construye y ejecuta un
  producto AOT enlazado con el mismo MIR/target/runtime/stdlib/perfil, valida
  el corpus completo antes de publicar contadores y ejecuta tres warmups y
  nueve muestras en cada uno de tres procesos frescos (27 por candidato).
  La evidencia separa la semántica de la VM de las observaciones native y
  captura allocations, bytes asignados/live/pico, retain/release local y
  atómico, ciclos recuperados, weak upgrades, pausas de colección, presión de
  worker OS y RSS. La instrumentación es process-local y fail-closed: un
  resultado, trap, cleanup o byte vivo divergente invalida la muestra; no se
  declara todavía N1. La evidencia de memoria alimenta la campaña de
  rendimiento AOT ya cerrada y la promoción de Cranelift en Gate N1.

- [x] **NATIVE-AOT-QUALITY-001 — Ejecutar la compuerta completa de calidad AOT.**
  Cerrado con `testing/native-aot-quality.json`: la campaña reutiliza la misma
  entrada MIR/target/runtime/stdlib/perfil, ejecuta el inventario AOT completo
  en Cranelift y LLVM contra la VM y el intérprete MIR normalizado, y exige
  cero divergencias y cero `unsupported` admitidos. La evidencia incluye las
  nueve hojas de conformance, differential generado con mutación fail-closed,
  cinco targets de fuzz de owners y el target de diagnósticos (128 ejecuciones,
  límites fijos y regresiones replayed), ASan/UBSan con wrapper absoluto, y la
  compuerta workspace de cobertura/mutación manteniendo la baseline versionada
  (actualmente 90,71%) y un suelo de política de 90,55%.
  `scripts/native-aot-quality-check.sh` valida un resumen reproducible y sin
  rutas físicas; `scripts/native-aot-quality-test.sh` muta los 12 oráculos
  críticos y rechaza también reportes incompletos, baseline alterado o
  divergencias. Su evidencia de calidad es el prerrequisito directo de la
  campaña de rendimiento AOT.

- [x] **NATIVE-AOT-PERF-001 — Capturar el rendimiento AOT completo.** Cerrado
  con `testing/native-aot-performance.json` y
  `docs/contracts/native-aot-performance.md`. La campaña ejecutable
  `scripts/native-aot-performance.sh` reutiliza el runner de calidad y mide
  productos enlazados completos de Cranelift y LLVM para el target del runner:
  27 builds aislados y 27 lanzamientos medidos por candidato, cada cohorte con
  tres warmups, nueve repeticiones y tres procesos frescos. La baseline separada
  del intérprete MIR conserva 27 muestras sobre ocho casos soportados y marca
  explícitamente los 20 casos no temporizados por esa referencia. El informe
  path-free conserva cada muestra y publica median/p95/p99 de compile/link y
  del tiempo end-to-end de build, medido por muestra desde el inicio de la
  generación de código hasta la creación, cálculo de metadatos y validación
  final del binario stripped; no se reconstruye sumando percentiles
  independientes de fases. También publica tamaño debug/stripped/.text,
  startup, throughput y latencia; las dimensiones
  de allocations, memoria, retain/release y pausas se ligan a los 27 samples
  ya validados por `NATIVE-AOT-MEM-001`. La VM y el intérprete MIR normalizado
  son el oráculo separado, no se mide JIT, no se agregan targets y no se
  selecciona backend automáticamente. `scripts/native-aot-performance-check.sh`
  y `scripts/native-aot-performance-test.sh` cubren el contrato y sus
  mutaciones negativas. El informe fue la entrada final de `DEC-013`, que
  seleccionó Cranelift; Gate N1 ya promueve esa ruta para el target primario
  admitido.

### Gate N1 — Backend nativo conforme

- [x] La campaña `NATIVE-AOT-SCOPE-001`, `NATIVE-AOT-LOWER-001`,
  `NATIVE-AOT-BINARY-001`, `NATIVE-AOT-MEM-001`,
  `NATIVE-AOT-QUALITY-001` y `NATIVE-AOT-PERF-001` está cerrada sin
  divergencias ni dimensiones omitidas.
- [x] El backend elegido tiene ADR, targets soportados y ABI runtime interna
  explícitos.
- [x] `tondo build` y `tondo run` atraviesan el mismo plan de enlace cerrado y
  el producto ejecutable supera smoke tests reales sin inputs ambientales.
- [x] DEC-014 está cerrado y ARC/ciclos correctos satisfacen los contratos de
  concurrencia ya especificados sin layout público accidental.
- [x] Todos los programas admitidos atraviesan el MIR verificado común; no
  existe frontend, type checker ni semántica paralela.
- [x] El adaptador nativo supera lenguaje y STD-0.1A con observaciones
  compatibles con la VM, incluidos los estados y reportes de `tondo test`.
- [x] Properties, fuzzing diferencial, GC/ARC/ciclos, async, pánicos y cleanup
  pasan bajo stress y sanitización aplicable.
- [x] `DIAG-NATIVE-001` demuestra race/leaks/dumps reales y nunca degrada un
  perfil requerido a `unsupported` por silencio.
- [x] Cada target publicado compila y ejecuta un corpus real sobre hardware del
  target.
- [x] Las optimizaciones aceptadas aportan una mejora medida y conservan todos
  los oracles.
- [x] Los paquetes nativos son reproducibles y no prometen una ABI pública no
  especificada.

El gate queda cerrado por `scripts/native-n1.sh`, que genera el informe
`target/reliability/evidence/native-n1.json` y lo valida con
`scripts/native-n1-check.sh`; `scripts/native-n1-test.sh` demuestra que las
mutaciones de contrato, procedencia, calidad, target, privacidad y claims
fallan cerrado. La promoción es exclusivamente para Cranelift en
`x86_64-unknown-linux-gnu`. ARM64 conserva un smoke físico de candidato y no
se convierte en target publicado hasta completar su campaña AOT independiente.

### 20.4 Trabajo posterior a Gate N1

Estas tareas no pertenecen al camino de corrección de N1. Se priorizan por
medición y pueden avanzar junto a la implementación de STD-0.1B sin alterar su
semántica:

- [ ] **ARC-003 — Implementar eliminación de retain/release mediante análisis
  de último uso.**

- [ ] **COW-NATIVE-001 — Portar al runtime nativo la política COW ya validada.**
  Reevaluar con perfiles nativos si conviene ampliar las formas compartibles;
  no duplicar una semántica ni asumir que el layout de la VM será definitivo.

- [ ] **ESCAPE-001 — Implementar escape analysis y stack allocation.**

- [ ] **INCR-001 — Añadir compilación incremental conservando resultados
  deterministas.** Una compilación limpia y un cache hit deben producir
  productos y diagnósticos observacionalmente idénticos.

- [ ] **LSP-001 — Construir LSP sobre las consultas semánticas existentes, no
  sobre un segundo frontend.**

---

## 21. STD-0.1B — Concurrency + Application Standard Library

**Objetivo:** completar la primera stdlib sin convertir APIs de aplicación en
nueva semántica del lenguaje. La fase tiene dos momentos:

1. Después de `DIAG-SPEC-001` y antes de los detectores/runner de M11 se cierran
   `STD-CONC-001`, `STD-SYNC-001`,
   `STD-EXEC-001` y la
   frontera runtime/host de `STD-NET-001`; son inputs de DEC-013/014 y de la
   elección de backend, no una autorización para implementarlos. Sus contratos
   describen también los eventos que consume `DIAG-RUNTIME-001`; no añaden APIs
   públicas para manejar el detector.
2. Tras N1 se implementan y conforman todos los módulos B sobre VM y backend
   nativo. Los demás specs B pueden prepararse durante M11 cuando sus
   dependencias A estén estables.

Esta fase no crea una segunda versión: cada módulo pertenece al catálogo cerrado
de STD-0.1 y solo puede considerarse listo tras superar su mini-gate
`SPEC → IMPL/HOST → MODEL/TEST/FUZZ → PERF → CONF → DOC`. STD 0.1.0 no se
publica hasta cerrar el gate final.

| Orden B | Owners | Dependencias duras | Momento |
|---|---|---|---|
| B0 | async group, sync, channel, executor y frontera net | `select` VM conforme + async/memoria/I/O/time A + `DIAG-SPEC-001` | contratos antes de los detectores/runner de M11 |
| B1 | `std.async.Group` | `Join` + scopes + scheduler VM/nativo | implementación hosted + conformance ABI runtime tras N1; lowering AOT async portable queda pendiente |
| B2 | `std.sync` | DEC-014 + backend/VM schedulers | tras B1 |
| B3 | `std.channel` | sync + scheduler + ownership `Send` | tras B2 |
| B4 | `std.executor` | group/sync/channel + bridge bloqueante | tras B3 |
| B5 | civil time | time-base + timezone data versionada | paralelo a B1–B4 |
| B6 | encoding/YAML/TOML/CBOR | bytes + I/O + serialization | paralelo tras N1 |
| B7 | regex/UUID | text; UUID añade civil-clock/entropy | paralelo tras N1 |
| B8 | net | I/O + time + executor/cancelación | después de B4 |
| B9 | log | format + time + I/O y sinks aplicables | después de owners de sinks |
| B10 | integración/distribución | todos los micro-gates | S1 y después `REL-0.1-RC-001` |

### 21.1 Concurrencia y tiempo

- [x] **STD-ASYNC-GROUP-SPEC-001 — Cerrar `std.async.Group`.** El registro
  [`testing/stdlib-async-group.json`](./testing/stdlib-async-group.json) fija
  `group/add/all/settle/next/cancel`, firmas con efectos, estados afines,
  transferencia de `Join`, índices estables, orden de inserción frente a
  finalización, prioridad de errores por índice, cancelación drenada, grupo
  vacío y rollback no mutante de `next` cuando pierde `select`. Los negativos
  ejecutables están en `scripts/stdlib-async-group-check.sh` y
  `scripts/stdlib-async-group-test.sh`, ambos integrados en `test-gate.sh`.
  `HOST = not-applicable` porque compone el scheduler existente; no se añaden
  `WaitGroup`, tuples awaitables, overloads por aridad ni otro `Task`/`Future`.
  La implementación hosted, la evidencia de medición, la conformance ABI y la
  documentación ejecutable quedan cerradas por sus leaves B1; el lowering AOT
  async portable y la promoción pública permanecen pendientes.

- [x] **STD-CONC-001 — Especificar `std.channel`.** El registro
  [`testing/stdlib-channel.json`](./testing/stdlib-channel.json) y el contrato
  [`docs/contracts/stdlib-channel.md`](./docs/contracts/stdlib-channel.md)
  cierran `Sender`/`Receiver`, `ChannelError`, `SendError` y `TryReceive`,
  capacidades `bounded(0/N)` y `unbounded` explícito, `fork`, backpressure,
  recuperación de payload afín al no comprometer un envío, estados de cierre y
  drenado, cierre terminal del receiver que devuelve mensajes pendientes,
  adaptación `AsyncIterator` bajo `T: Discard`, `send`/`receive` `selectable`
  sobre el `select` núcleo, commit/rollback cancel-safe, fairness FIFO de
  registros, ownership `T: Send`, cancelación y eventos privados para
  `DIAG-RUNTIME-001`. Los negativos ejecutables son
  `scripts/stdlib-channel-check.sh` y `scripts/stdlib-channel-test.sh`,
  integrados en `test-gate.sh`. Este contrato no define selector, casos,
  macros ni operaciones paralelas de selección; la implementación
  target-qualified se registra por separado en `STD-CHANNEL-IMPL-001`, sin
  convertir este contrato en una API pública ni reclamar lowering AOT.

- [x] **STD-SYNC-001 — Especificar `std.sync`.** El registro
  [`testing/stdlib-sync.json`](./testing/stdlib-sync.json) y el contrato
  [`docs/contracts/stdlib-sync.md`](./docs/contracts/stdlib-sync.md) cierran
  `Mutex`/`RwLock`, guards afines con cleanup `Discard` exactamente una vez,
  condition variables con release/reacquire cancel-safe, `Semaphore`/`Permit`,
  `Once[T, E]`, `Barrier` y atomics con `MemoryOrder` constante y CAS fuerte.
  Fijan `Send`/`Share`, ausencia de poisoning implícito, fairness, progreso y
  prohibición de bloqueo de workers cooperativos; una operación suspendible
  espera implícitamente y ninguna de estas primitivas es `selectable`. El mismo
  contrato cierra además `sync.Array/Map/Set` linealizables, `sync.Stack` LIFO y
  `sync.Queue` FIFO MPMC, bounds de ownership, snapshots de un punto de
  linearización, `for` directo `AsyncIterator` por valor con horizonte finito,
  orden, literals por identidad y separación respecto a `std.channel`. Los
  negativos ejecutables están en `scripts/stdlib-sync-check.sh` y
  `scripts/stdlib-sync-test.sh`, integrados en `test-gate.sh`. La superficie de
  primitivas, host y parking está cerrada; el frontend de literales está
  cerrado por `STD-SYNC-COLLECTION-FRONTEND-001`. La ejecución hosted y el ABI
  nativo privado de colecciones quedan cerrados por
  `STD-SYNC-COLLECTION-IMPL-001`; la iteración directa queda cerrada por
  `STD-SYNC-COLLECTION-ITER-001` para hosted y ABI nativo privado. Las
  optimizaciones algorítmicas, la conformance final y el lowering AOT siguen
  pendientes de las leaves `STD-SYNC-COLLECTION-*` tras `NATIVE-001`.

- [x] **STD-EXEC-001 — Especificar `std.executor`.** El registro
  [`testing/stdlib-executor.json`](./testing/stdlib-executor.json) y el contrato
  [`docs/contracts/stdlib-executor.md`](./docs/contracts/stdlib-executor.md)
  cierran pools cooperativos acotados, actores con mailbox FIFO, el bridge de
  trabajo bloqueante con capability `threads`, backpressure, saturación,
  shutdown, cancelación y cleanup. `submit` reutiliza `Join` y el scheduler
  estructurado existente; `selectable` solo aparece en el envío de mensajes del
  actor. El bridge no admite funciones suspendibles, no bloquea workers
  cooperativos, no hereda estado ambiental y nunca fuerza la terminación de un
  job host. Los checks negativos están en
  `scripts/stdlib-executor-check.sh` y
  `scripts/stdlib-executor-test.sh`, integrados en `test-gate.sh`. La
  implementación permanece pendiente de las leaves `STD-EXEC-*` después de
  `NATIVE-001`; la instrumentación hosted de `DIAG-RUNTIME-001` ya está
  cerrada y la siguiente frontera son los detectores de M11.

- [x] **STD-CIVIL-TIME-001 — Cerrar el contrato civil de `std.time`.** El
  registro [`testing/stdlib-civil-time.json`](./testing/stdlib-civil-time.json)
  y el contrato [`docs/contracts/stdlib-civil-time.md`](./docs/contracts/stdlib-civil-time.md)
  fijan `Date`, `Time`, `DateTime`, `UtcDateTime`, parsing/formatting
  canónico, aritmética gregoriana comprobada, `ZoneId`, bundle de timezone
  versionado por `(version, sha256)`, `TimeZone`, gaps/folds con policy
  explícita, `civil-clock` separado de `clock` y `CivilAnchor` para la única
  conversión monotónica/civil. No hay consulta de `TZ`/locale durante build,
  fallback UTC, `select`, API async duplicada ni un segundo `Duration` o
  `Instant`. Los checks negativos están en
  `scripts/stdlib-civil-time-check.sh` y
  `scripts/stdlib-civil-time-test.sh`, integrados en `test-gate.sh`. El
  contrato está cerrado, pero la implementación, el bundle host, tests,
  rendimiento, conformance y documentación de uso siguen pendientes de las
  leaves `STD-CIVIL-TIME-*`; la instrumentación hosted de
  `DIAG-RUNTIME-001` ya está cerrada.

### 21.2 Aplicación y datos

- [x] **STD-NET-001 — Especificar `std.net` capability-gated.** El registro
  [`testing/stdlib-net.json`](./testing/stdlib-net.json) y el contrato
  [`docs/contracts/stdlib-net.md`](./docs/contracts/stdlib-net.md) cierran la
  superficie de direcciones, DNS explícito, TCP con partial I/O y split afín,
  UDP datagram-atómico, TLS con `PlatformRoots`, límites finitos, deadlines
  monotónicos, cancelación cooperativa, cleanup y eventos privados. `network`
  es capability obligatoria, `clock` solo se solicita para deadlines; no hay
  I/O por import, retries, proxies ambientales, resolución implícita,
  futures duplicados, TLS inseguro ni `std.net.select`. `accept`, lectura TCP y
  recepción UDP son los únicos adapters `selectable`; el resto usa la
  suspensión implícita única de Tondo. Los checks negativos están en
  `scripts/stdlib-net-check.sh` y `scripts/stdlib-net-test.sh`, integrados en
  `test-gate.sh`. La implementación y el host siguen pendientes de las leaves
  `STD-NET-*` tras `NATIVE-001`; la instrumentación hosted de
  `DIAG-RUNTIME-001` ya está cerrada.

- [x] **STD-ENCODING-001 — Especificar `std.encoding`.** El registro
  [`testing/stdlib-encoding.json`](./testing/stdlib-encoding.json), el contrato
  [`docs/contracts/stdlib-encoding.md`](./docs/contracts/stdlib-encoding.md) y
  sus checks negativos cierran Base64 RFC 4648 estándar/URL-safe, padding
  requerido/omitido, hexadecimal con policies de case, canonicalidad estricta,
  APIs materializadas y de streaming sobre `Bytes`/`std.io`, límites
  acumulados, offsets de error, terminalidad, atomicidad del carry y la
  frontera scalar/SIMD. No hay whitespace permisivo, autodetección, MIME,
  owner binario alternativo ni API async duplicada. La implementación, host,
  tests de corpus, rendimiento, conformance y documentación de uso permanecen
  pendientes de las leaves `STD-ENCODING-*` posteriores a `NATIVE-001`; la
  instrumentación hosted de `DIAG-RUNTIME-001` ya está cerrada.

- [x] **STD-YAML-001 — Especificar `std.yaml`.** El contrato
  [`docs/contracts/stdlib-yaml.md`](./docs/contracts/stdlib-yaml.md) y el
  registro [`testing/stdlib-yaml.json`](./testing/stdlib-yaml.json) fijan el
  subset seguro de YAML 1.2 Core: UTF-8, block/flow collections, scalars
  quoted/block, documentos múltiples explícitos y tags core cerrados. El modelo
  dinámico es `YamlValue` con objects de keys textuales y la ruta tipada usa
  `Encode[Yaml]`/`Decode[Yaml]` sin árbol intermedio. Anchors y aliases se
  resuelven como copias lógicas dentro de un documento; ciclos, forward refs,
  `<<`, custom tags, timestamps implícitos, mappings no textuales y código son
  rechazados. `YamlReader`/`YamlWriter` comparten eventos, frames explícitos,
  chunk-boundary invariance, límites finitos de input/profundidad/nodos/aliases/
  expansión/scalars/collections y errores con offset, línea, columna y path.
  No hay lookup ambiental, API async duplicada ni operación `selectable`.
  Los checks negativos `scripts/stdlib-yaml-check.sh` y
  `scripts/stdlib-yaml-test.sh` están integrados en `test-gate.sh`. La ruta
  scalar y el bridge VM hosted buffered quedan cerrados por
  `STD-YAML-IMPL-001`, con typed/dynamic codecs, límites, errores y lifecycle
  de reader/writer; la API pública y el lowering AOT nativo siguen sin
  reclamar. Los corpus ampliados, fuzzing, rendimiento, conformance y
  documentación de uso quedan pendientes de las leaves `STD-YAML-*`; la
  instrumentación hosted de `DIAG-RUNTIME-001` ya está cerrada.

- [x] **STD-TOML-001 — Especificar `std.toml`.** El contrato
  [`docs/contracts/stdlib-toml.md`](./docs/contracts/stdlib-toml.md) y el
  registro [`testing/stdlib-toml.json`](./testing/stdlib-toml.json) fijan el
  perfil TOML 1.1.0: UTF-8 case-sensitive, raíz única, claves bare/quoted/
  dotted, strings basic/literal multilinea, números con bases y underscores,
  floats `inf`/`nan`, los cuatro tipos fecha/hora, arrays heterogéneos, inline
  tables cerradas, tables y arrays-of-tables. El modelo `TomlValue` conserva
  orden de tablas y arrays; los tipos temporales reutilizan `std.time` y un
  offset fijo vive en `TomlOffsetDateTime`, sin lookup de zona ni reloj.
  Duplicados, colisiones scalar/table, extensión de inline table y valores
  fuera de los rangos lossless son rechazos atómicos con `TomlSpan` half-open,
  línea, columna y path estable. Fracciones temporales de más de nueve dígitos
  se rechazan para no truncar nanosegundos. `TomlReader`/`TomlWriter` exponen
  eventos balanceados con frames/worklists explícitos, invariancia al
  chunking, límites finitos y suspensión solo en la frontera `std.io`; no hay
  API `selectable`, includes, interpolación de environment ni semántica de
  `tondo.toml`. Los negativos están en `scripts/stdlib-toml-check.sh` y
  `scripts/stdlib-toml-test.sh`, integrados en `test-gate.sh`. La implementación,
  host, tests/fuzzing, rendimiento, conformance y documentación de uso quedan
  pendientes de `STD-TOML-IMPL-001` y sus leaves posteriores a `NATIVE-001`.

- [x] **STD-CBOR-001 — Especificar `std.cbor`.** El contrato
  [`docs/contracts/stdlib-cbor.md`](./docs/contracts/stdlib-cbor.md) y el
  registro [`testing/stdlib-cbor.json`](./testing/stdlib-cbor.json) fijan el
  wire model de RFC 8949: major types 0–7, `UInt` y negativos por magnitud,
  bytes, texto UTF-8, arrays, maps de claves arbitrarias, tags `UInt64`,
  simples, `null`, `undefined` y floats `Float16`/`Float32`/`Float64`. Se
  aceptan longitudes definidas e indefinidas solo para los cuatro containers
  permitidos; break fuera del frame actual, chunks de tipo incorrecto y
  trailing data son rechazos atómicos. `CborValue` conserva tags, orden de
  arrays/maps, duplicados según policy, bytes, simples no asignados y bits de
  floats; `CborRaw` conserva la codificación exacta y `CborValueView` presta
  payloads hasta el siguiente evento. `CborReader`/`CborWriter` exponen
  frames explícitos para chunks y tags, son invariantes al chunking, acotados
  por límites finitos y nunca usan la pila recursiva del host. El modo
  ordinario acepta formas no mínimas y NaN sin normalizar; `encodeDeterministic`
  usa preferred serialization, longitudes definidas, el float más corto que
  conserva valor/signo, un quiet-NaN estable y orden lexicográfico por bytes
  deterministas de las claves, rechazando colisiones. Los negativos están en
  `scripts/stdlib-cbor-check.sh` y `scripts/stdlib-cbor-test.sh`, integrados
  en `test-gate.sh`. La implementación, host, tests/fuzzing, rendimiento,
  conformance y documentación de uso quedan pendientes de
  `STD-CBOR-IMPL-001`, `STD-CBOR-TEST-001`, `STD-CBOR-PERF-001`,
  `STD-CBOR-CONF-001` y `STD-CBOR-DOC-001`.

- [x] **STD-REGEX-001 — Especificar `std.regex`.** El registro
  [`testing/stdlib-regex.json`](./testing/stdlib-regex.json) y el contrato
  [`docs/contracts/stdlib-regex.md`](./docs/contracts/stdlib-regex.md) fijan un
  dialecto regular cerrado sobre `String` UTF-8, Unicode 16.0.0, propiedades
  versionadas, classes/rangos, anchors, captures, quantifiers greedy/lazy y
  replacement seguro. Backreferences, look-around, recursion, conditionals,
  locale, normalización, grapheme matching, includes, callbacks y bytes UTF-8
  inválidos se rechazan explícitamente. `Regex` es inmutable y shareable;
  `isMatch`, `isFullMatch`, `match`, `findAll`, `replace` y `replaceAll` son
  síncronos, leftmost, no solapados y progresan un scalar en matches vacíos.
  Los spans son offsets UTF-8 half-open en límites de scalar; los errores son
  nominales y atómicos. El motor exige autómatas finitos o una prueba
  equivalente de complejidad lineal, worklists explícitos y límites de patrón,
  programa, input, pasos, matches, replacement y output. Los negativos están en
  `scripts/stdlib-regex-check.sh` y `scripts/stdlib-regex-test.sh`, integrados
  en `test-gate.sh`. La implementación, tests/fuzzing, rendimiento,
  conformance y documentación de uso permanecen pendientes de
  `STD-REGEX-IMPL-001`, `STD-REGEX-TEST-001`, `STD-REGEX-PERF-001`,
  `STD-REGEX-CONF-001` y `STD-REGEX-DOC-001`; la instrumentación hosted de
  `DIAG-RUNTIME-001` ya está cerrada.

- [x] **STD-ID-001 — Especificar `std.uuid`.** El registro
  [`testing/stdlib-uuid.json`](./testing/stdlib-uuid.json), el contrato
  [`docs/contracts/stdlib-uuid.md`](./docs/contracts/stdlib-uuid.md) y sus
  negativos ejecutables fijan un valor inmutable de 128 bits interoperable con
  RFC 9562, bytes de red big-endian, parse/format dashed y URN, `nil`/`max`,
  variant/version observables y las traits `Copy`, `Eq`, `Ord`, `Hash`, `Send` y
  `Share`. Solo se generan v4, v5 y v7: v4 requiere `entropy`, v5 es puro y
  determinista sobre namespace+name bytes, y v7 requiere `civil-clock` y
  `entropy` para su timestamp Unix en milisegundos y sus 74 bits restantes.
  Se rechazan compact/braced text, UUID como secreto, v1/v2/v3/v6/v8 de
  generación, timezone lookup, collision registries, retries y estado global.
  El parser conserva UUID externos de cualquier variante/versión; los errores
  son nominales, bounded y atómicos, y no hay API async ni `selectable`. La
  implementación, providers, tests/fuzzing, rendimiento, conformance y
  documentación de uso permanecen pendientes de `STD-UUID-IMPL-001`,
  `STD-UUID-HOST-001`, `STD-UUID-TEST-001`, `STD-UUID-PERF-001`,
  `STD-UUID-CONF-001` y `STD-UUID-DOC-001`; la instrumentación hosted de
  `DIAG-RUNTIME-001` ya está cerrada.

- [x] **STD-LOG-001 — Especificar `std.log`.** El registro
  [`testing/stdlib-log.json`](./testing/stdlib-log.json), el contrato
  [`docs/contracts/stdlib-log.md`](./docs/contracts/stdlib-log.md) y sus
  negativos ejecutables fijan `LogLevel` (`Trace`–`Error`), eventos inmutables,
  fields ordenados con `LogValue` y `Redacted`, timestamp civil opcional sin
  reloj ambiental, y los formatos `Text` y `JsonLines` con el schema
  `tondo-log-event-0.1/1`. El protocolo estático `LogSink` ofrece `write`,
  `flush` y `close`; `ConsoleSink` y `FileSink` declaran `console` y
  `filesystem`, mientras un sink de red usa un writer/transporte explícito con
  `network`. `Block`, `Reject` y `Drop` hacen visible el backpressure y los
  receipts; la concurrencia se lineariza por sink y `close` drena de forma
  terminal. No hay logger global, fallback a stderr, queue ilimitada, worker
  oculto, retry, rotation, sampling ni redacción heurística. `emit`, `flush` y
  `close` son `suspends` inferibles, sin `logAsync`, polling ni `select`. La
  implementación, bridges de host, tests/fuzzing, rendimiento, conformance y
  documentación de uso permanecen pendientes de `STD-LOG-IMPL-001`,
  `STD-LOG-HOST-001`, `STD-LOG-TEST-001`, `STD-LOG-PERF-001`,
  `STD-LOG-CONF-001` y `STD-LOG-DOC-001`; la instrumentación hosted de
  `DIAG-RUNTIME-001` ya está cerrada.

### 21.3 Implementación y evidencia

Cada owner mantiene celdas independientes `SPEC`, `IMPL`, `HOST`, `TEST/FUZZ`,
`PERF`, `CONF` y `DOC`. Cuando no existe frontera host, el registro del owner
marca `HOST = not-applicable` con razón; no se crea un stub ni se omite la
dimensión. `TEST/FUZZ` produce subregistros separados `MODEL`, `TEST` y `FUZZ`;
la task puede ser una sola porque pertenece a un único owner, pero el gate
rechaza cualquiera de los tres ausente. En `DOC`, “ejemplo ejecutable” significa
ejemplo verificable por doc-test más un acceptance runtime enlazado, no que el
doc runner ejecute efectos. Las tareas coordinadoras de 21.3.14 solo agregan
estas leaves.

#### 21.3.1 Coordinación de `std.async`

- [x] **STD-ASYNC-GROUP-IMPL-001 — Implementar `Group[T, E]` en la VM hosted.**
  `Join` homogéneos se transfieren sin duplicar frame ni outcome; la ruta
  tipada implementa `add/all/settle/next/cancel`, la entrada seleccionable de
  `next`, grupos vacíos, prioridad determinista de errores y cancelación
  drenada sobre el scheduler cooperativo único. El fixture
  `tests/runtime/m11-std-async-group-001.to` prueba fan-in, orden real de
  finalización, `settle`, `select` con commit/rollback y cleanup de cancelación.
  `HOST` es `not-applicable`: no existe bridge ni primitiva host propia. La
  frontera del runtime nativo queda verificada por `STD-ASYNC-GROUP-CONF-001`;
  el lowering AOT async y el scheduler portable siguen siendo trabajo
  posterior y no se ocultan como completados por esta evidencia hosted.
- [x] **STD-ASYNC-GROUP-TEST-001 — Modelar, probar y fuzzear grupos.** El
  registro separado
  [`testing/stdlib-async-group-test.json`](./testing/stdlib-async-group-test.json)
  cierra las celdas `MODEL`, `TEST` y `FUZZ`. El modelo independiente
  `crates/tondo-reliability/src/group_model.rs` y su corpus de 4.096 seeds
  comprueban secuencias de add/next/terminal, éxito/error/pánico/cancelación,
  finalizaciones simultáneas con tie-break por índice, probe/commit/rollback de
  `select`, transferencia entre scopes, grupo vacío, límites, cleanup único y
  ausencia de hijos o handles perdidos. `tests/models.rs` repite cada secuencia
  con replay determinista y el target `stdlib_async_group` mantiene un corpus
  persistente, límite de 4 KiB/1.024 pasos y 128 ejecuciones smoke; los tests
  hosted de VM y el fixture público siguen siendo los oráculos consumidores.
- [x] **STD-ASYNC-GROUP-PERF-001 — Medir grupos.** El contrato
  [`testing/stdlib-async-group-performance.json`](./testing/stdlib-async-group-performance.json)
  y el probe hosted cubren `add`, fan-in `all`, `settle`, `next` listo/pendiente
  y cancelación para cardinalidades 1/8/64. Cada workload conserva 27 muestras
  (3 warmups, 9 repeticiones, 3 procesos) y reporta mediana/P95/P99,
  throughput, allocations del heap, escaneos, wakeups, pasadas de cancelación y
  bytes lógicos reservados. `VmStatistics` aporta los contadores explícitos y
  el runner conserva outliers y verifica hashes/invariantes; `group_peak_state_bytes`
  excluye headers del allocator y no se presenta como RSS. La evidencia solo
  cierra `tondo-vm-hosted`/`bytecode-vm`; el ABI del runtime nativo queda
  cerrado por `STD-ASYNC-GROUP-CONF-001`, mientras AOT y scheduler async
  portable siguen pendientes de sus propias leaves.
- [x] **STD-ASYNC-GROUP-CONF-001 — Conformar grupos.** El contrato
  [`testing/stdlib-async-group-conformance.json`](./testing/stdlib-async-group-conformance.json)
  ejecuta ocho casos con los mismos identificadores y observables contra el
  fixture hosted de VM y el ABI del runtime nativo. La evidencia verifica
  orden de finalización, prioridad por índice de inserción, `all`/`settle`/
  `next`/`cancel`, drenado de errores y pánicos, cleanup exactamente una vez,
  proceso fresco por probe y rechazo de handles afines inválidos. El informe
  hash-bound queda en
  `target/reliability/evidence/stdlib-async-group-conformance.json`. Esto
  cierra la frontera del runtime nativo, no afirma aún lowering AOT async ni
  portabilidad del scheduler; el siguiente trabajo de la superficie es el
  lowering AOT async portable, fuera de esta conformance.
- [x] **STD-ASYNC-GROUP-DOC-001 — Documentar grupos.** La guía normativa
  [`docs/contracts/stdlib-async-group.md`](./docs/contracts/stdlib-async-group.md)
  publica firmas, ownership, orden, errores, cancelación, costes y cinco
  familias de ejemplos ejecutables de `all`, `settle`, `next`, `select` y
  cancelación sin `WaitGroup`. El registro
  [`testing/stdlib-async-group.json`](./testing/stdlib-async-group.json)
  enlaza el fixture `tests/runtime/m11-std-async-group-001.to`, sus sidecars y
  `scripts/stdlib-async-group-doc-check.sh`; sus negativos se prueban con
  `scripts/stdlib-async-group-doc-test.sh`, ambos integrados en `test-gate.sh`.
  El cierre es documental y no promociona lowering AOT async ni una release.

#### 21.3.2 `std.sync`

- [x] **STD-SYNC-IMPL-001 — Implementar la superficie portable de sync.**
  El compilador registra y baja mutex, rwlock, guards, condición,
  semáforo/permit, once, barrier y atomics con ownership, cleanup sin poisoning
  implícito y memory ordering fijados por `STD-SYNC-001`. La VM hosted ejecuta
  el modelo determinista de estado, errores nominales, cleanup idempotente y
  fast paths no contendedidos; el fixture
  `tests/runtime/m11-std-sync-impl-001.to` produce `sync-ok`. El parking/wakeup
  cooperativo para contención y el puente ABI nativo se cierran en
  `STD-SYNC-HOST-001`; no se introduce un contador `WaitGroup` paralelo a
  `Group[Unit, E]`.
- [x] **STD-SYNC-HOST-001 — Implementar parking y atomics del host.** El host
  anuncia la unidad lógica de cada llamada, registra waiters en colas FIFO por
  recurso y reintenta mutex, rwlock, condición, semáforo y barrera desde el
  scheduler sin bloquear un worker cooperativo ni fingir progreso
  single-thread. `Condition.wait` libera y reacquirea el guard de forma
  indivisible; cancelación y rotura de barrera desregistran y despiertan sin
  dejar waiters detached. El runtime nativo verifica el carril privado de
  `AtomicU64` con los cinco órdenes de memoria y señales epoch de parking sobre
  `Condvar`, con operaciones concurrentes entre workers y handles opacos. La
  ejecución del closure de `Once.getOrInit` se cubre en
  `STD-SYNC-TEST-001` mediante una continuación de VM verificable, sin
  simularla en este bloque.
- [x] **STD-SYNC-TEST-001 — Modelar y endurecer sync.** El registro separado
  [`testing/stdlib-sync-test.json`](./testing/stdlib-sync-test.json) cierra las
  celdas `MODEL`, `TEST` y `FUZZ`. Los modelos independientes de
  `crates/tondo-reliability/src/sync_model.rs` cubren litmus de memoria,
  publicación release/acquire, FIFO, wakeups perdidos, liberación de
  guard/permit, pánico/cancelación, once/barrier, teardown y límites bajo
  scheduling adversario. El fixture
  `tests/runtime/m11-std-sync-test-001.to` prueba la continuación real de
  `Once.getOrInit`, su memoización y el cleanup de sus resultados; el target
  `stdlib_sync` mantiene corpus persistente, 4 KiB/1.024 pasos y 128 ejecuciones
  smoke reproducibles. La frontera de sanitización es explícita: estos modelos,
  VM y bridge están escritos en Rust seguro (`forbid(unsafe_code)`), por lo que
  ASan/UBSan no aplican aquí; la campaña hosted y la frontera AOT se reportan
  por separado.
- [x] **STD-SYNC-PERF-001 — Medir sync.** La VM hosted tiene 20 workloads
  uncontended/contended de Mutex, RwLock, Condition, Semaphore, Barrier,
  Atomic y Once, con 27 muestras por workload en tres procesos independientes.
  El informe fija latencia, P95/P99, throughput, fairness FIFO, memoria lógica,
  handles vivos y contadores de wakeups contra el host y el oracle independiente
  de `std.sync`. El cierre es target-qualified para `tondo-vm-hosted`; no
  sobreafirma rendimiento AOT nativo ni agrega targets distintos.
- [x] **STD-SYNC-COLLECTION-FRONTEND-001 — Implementar literales concurrentes.**
  Cerrado el lexer/parser preliminar, CST lossless, formatter, resolución por
  identidad de declaración, tipos, HIR/MIR boundary y diagnósticos para las
  cinco formas cualificadas. Los aliases de `std.sync` conservan la identidad,
  los paths de usuario no pueden optar al azúcar y posición de tipo frente a
  expresión resuelve `sync.Array[T]` sin heurísticas de runtime. Se cubren
  vacíos contextuales, map `[:]`, entrada única y múltiple, trailing comma,
  duplicados, recovery y round-trip sin keywords ni aliases del prelude. La
  evidencia ejecutable está en
  [`testing/stdlib-sync-collection-frontend.json`](./testing/stdlib-sync-collection-frontend.json),
  [`docs/contracts/stdlib-sync-collection-frontend.md`](./docs/contracts/stdlib-sync-collection-frontend.md),
  `scripts/stdlib-sync-collection-frontend-check.sh` y
  `scripts/stdlib-sync-collection-frontend-test.sh`; el marcador tipado queda
  consumido por `STD-SYNC-COLLECTION-IMPL-001`.
- [x] **STD-SYNC-COLLECTION-IMPL-001 — Implementar colecciones compartidas.**
  Cerrada la ejecución de handles `Copy + Discard + Send + Share` para las
  cinco identidades nominales en la VM hosted y el ABI nativo privado:
  `Array` de longitud fija, `Map`/`Set` con orden de inserción, `Stack` LIFO y
  `Queue` FIFO MPMC. La implementación valida identidad y handles stale o
  forjados, mantiene estado compartido por alias, aplica límites recuperables,
  snapshots coherentes, compare-exchange fuerte y cleanup compatible con
  ownership/GC. El runtime nativo usa una celda `RwLock` por array/map/set,
  `Mutex` por stack/queue y parking por época sin conservar el lock global de
  tablas durante la espera; la VM hosted conserva su scheduler determinista de
  worker único. La evidencia ejecutable está en
  [`testing/stdlib-sync-collection.json`](./testing/stdlib-sync-collection.json),
  [`docs/contracts/stdlib-sync-collection.md`](./docs/contracts/stdlib-sync-collection.md),
  `scripts/stdlib-sync-collection-check.sh` y
  `scripts/stdlib-sync-collection-test.sh`, integrados en los gates de
  `std.sync`. Los fast paths algorítmicos CAS/sharding, el lowering AOT
  genérico y la promoción de una API pública no se declaran completados; la
  estrategia nativa queda target-qualified para una campaña posterior y no se
  infiere del baseline hosted de `STD-SYNC-COLLECTION-PERF-001`.
- [x] **STD-SYNC-COLLECTION-ITER-001 — Implementar `for` concurrente directo.**
  Reconocer solo las cinco identidades cerradas de `std.sync` y bajar el header
  ordinario por valor a `cursor[sync,C]: AsyncIterator[T]`, copiando el handle y
  capturando en O(1) un horizonte estructural finito. Excluir altas posteriores
  a crear el cursor, omitir retiradas aún no observadas, linearizar cada `next`,
  preservar el orden normativo y no repetir una generación aun con resize, ABA
  o reclamación. El cursor no copia contenidos, no mantiene locks durante el
  body y libera toda protección en agotamiento, `break`, error, retorno, pánico
  o cancelación.
  Rechazar cualquier binding `ref`/`mut`/`var`; stack/queue exigen
  `T: Copy + Send + Share` y nunca consumen elementos. Mantener `snapshot()`
  como la única iteración de estado global coherente y no añadir `scan`, `live`,
  `for await` ni otro protocolo público. Cerrado para la VM hosted y el ABI
  nativo privado: el checker infiere `AsyncIterator` por valor para las cinco
  identidades nominales, rechaza préstamos y exige `Copy + Send + Share` en
  `Stack`/`Queue`; el cursor captura un horizonte estructural O(1), no
  materializa ni mantiene locks durante el body y excluye inserciones o
  reinserciones posteriores mediante generaciones. La evidencia está en
  [`testing/stdlib-sync-collection-iter.json`](./testing/stdlib-sync-collection-iter.json),
  [`docs/contracts/stdlib-sync-collection-iter.md`](./docs/contracts/stdlib-sync-collection-iter.md),
  `scripts/stdlib-sync-collection-iter-check.sh` y
  `scripts/stdlib-sync-collection-iter-test.sh`. El runtime AOT genérico y una
  API pública de cursor no se declaran completados.
- [x] **STD-SYNC-COLLECTION-TEST-001 — Modelar y fuzzear colecciones
  compartidas.** Cerrado con modelos secuenciales independientes acotados para
  `Array`, `Map`, `Set`, `Stack` y `Queue`; operaciones de orden, límites,
  duplicados, reemplazo/reinserción, CAS y snapshots; búsqueda exhaustiva de
  histories de linearización con precedencia de tiempo real; cursores directos
  con horizonte finito y generaciones; aliases, retención de la fuente,
  handles/cursors stale y cleanup exactamente una vez; y replay/fuzz
  determinista, panic-free y leak-free. La evidencia vive en
  `testing/stdlib-sync-collection-test.json`,
  `docs/contracts/stdlib-sync-collection-test.md`,
  `crates/tondo-reliability/tests/sync_collection_models.rs` y el target
  `stdlib_sync_collections`; las regresiones hosted/native se ejecutan en los
  carriles existentes. El bloque no promueve fast paths, sanitizers, API pública
  ni lowering AOT genérico. La campaña PERF hosted queda cerrada por separado
  y no altera este oráculo.
- [x] **STD-SYNC-COLLECTION-PERF-001 — Medir colecciones compartidas.** Cerrada
  la campaña target-qualified de la VM hosted para los cinco owners con 31
  workloads sobre cardinalidades/unidades lógicas 1/8/64, tres procesos
  independientes y 27 muestras por workload. El probe mide latencia mediana y
  de cola, throughput, allocations lógicas, memoria lógica, retries, wakeups,
  parking y handles vivos para fast path 1:1, lecturas/escrituras
  independientes, hot key/slot, MPMC lógico, cursor directo, snapshot y resize.
  El oráculo independiente y los invariantes rechazan operaciones inestables,
  contadores de contención hosted no nulos, jobs/waiters pendientes, copias o
  tablas de visitados en el cursor y retención de locks durante el body. Se
  selecciona `single-worker-ready-job-collection-baseline` para este target;
  los fast paths algorítmicos nativos, la contención real y el lowering AOT
  genérico permanecen sin reclamar hasta una campaña target-qualified
  comparable.
  La evidencia está en
  [`testing/stdlib-sync-collection-performance.json`](./testing/stdlib-sync-collection-performance.json),
  [`docs/contracts/stdlib-sync-collection-performance.md`](./docs/contracts/stdlib-sync-collection-performance.md),
  `scripts/stdlib-sync-collection-performance-check.sh`,
  `scripts/stdlib-sync-collection-performance-test.sh` y
  `scripts/stdlib-sync-collection-performance.sh`, integrados en
  `scripts/test-gate.sh`.
- [x] **STD-SYNC-COLLECTION-CONF-001 — Conformar colecciones compartidas.**
  Cerrado el mismo corpus observable en VM y nativo con ocho casos: literales y
  aliases, bounds y outcomes CAS, linearización Map/Set, `for` débil finito,
  orden de recorridos y snapshots, LIFO/FIFO, límites, cleanup y capability
  `threads`. La fixture hosted emite las mismas líneas que el probe ABI nativo;
  los tests existentes conservan la suspensión inferida, el rechazo de
  préstamos, los bounds `Copy + Send + Share`, las generaciones de cursor y la
  equivalencia aritmética sobre snapshots. La evidencia ejecutable queda en
  [`testing/stdlib-sync-collection-conformance.json`](./testing/stdlib-sync-collection-conformance.json),
  [`docs/contracts/stdlib-sync-collection-conformance.md`](./docs/contracts/stdlib-sync-collection-conformance.md),
  `scripts/stdlib-sync-collection-conformance-check.sh`,
  `scripts/stdlib-sync-collection-conformance-test.sh` y
  `scripts/stdlib-sync-collection-conformance.sh`. El cierre es target-qualified
  para la VM hosted y el ABI nativo privado: no promueve fast paths nativos,
  API pública de cursor ni lowering AOT genérico.
- [x] **STD-SYNC-CONF-001 — Conformar sync.** Cerrado el corpus común de ocho casos en VM hosted y el puente nativo privado.
  Incluye órdenes y CAS de atomics, parking con timeout/wake-one/wake-all,
  cleanup de handles, publicación Once y generaciones de barrera como modelos
  de puente, lifecycle de workers y consumo obligatorio de
  `STD-SYNC-COLLECTION-CONF-001`. La ausencia de `threads` queda fijada por
  el diagnóstico estático `E1008` y su fixture compile-fail. La evidencia y
  los negativos están en `testing/stdlib-sync-conformance.json`,
  `docs/contracts/stdlib-sync-conformance.md` y sus tres scripts; el cierre
  es target-qualified y no reclama locks nativos públicos ni lowering AOT.
- [x] **STD-SYNC-DOC-001 — Documentar sync.** La guía ejecutable en
  [`docs/contracts/stdlib-sync.md`](./docs/contracts/stdlib-sync.md) publica
  ordering y límites de deadlock, ausencia de poisoning, cancelación, cleanup y
  costes sin defaults ocultos. El fixture
  `tests/runtime/m11-std-sync-doc-001.to` ejecuta los seis casos de decisión:
  los cinco literales cualificados y el array fijo, CAS con órdenes explícitos,
  `for` directo débil frente a snapshots coherentes, orden LIFO/FIFO y la
  elección explícita entre queue no bloqueante y channel con
  espera/backpressure. `scripts/stdlib-sync-doc-check.sh` y
  `scripts/stdlib-sync-doc-test.sh` mantienen el contrato y sus negativos. El
  cierre es target-qualified para la VM hosted y no promociona lowering AOT,
  locks nativos públicos ni una release; el siguiente bloque es
  `STD-CHANNEL-IMPL-001`.

#### 21.3.3 `std.channel`

- [x] **STD-CHANNEL-IMPL-001 — Implementar canales tipados.** Cerrada la
  superficie nominal de `Sender[T]`/`Receiver[T]` en compiler, VM hosted y ABI
  nativo privado: `bounded(0/N)`, `unbounded` explícito, FIFO, backpressure,
  `fork`, try-operations, cierre y devolución terminal de pendientes. Las
  llamadas `send`/`receive` se registran en el scheduler y participan en la
  keyword núcleo `select` sin tasks desligadas; el commit mueve el payload una
  sola vez y todo fallo devuelve el valor afín intacto. La fixture
  `tests/runtime/m11-std-channel-impl-001.to` cubre además ambos brazos
  seleccionables; `crates/tondo-native-runtime/examples/channel_conformance.rs`
  comprueba el mismo corpus observable en un proceso nativo fresco. El
  registro y la evidencia están en `testing/stdlib-channel.json` y
  `target/reliability/evidence/stdlib-channel-implementation.json`; el cierre
  es target-qualified, no promociona una API pública y no reclama lowering AOT.
  El siguiente bloque es `STD-CHANNEL-ASYNC-ITER-001`.
- [x] **STD-CHANNEL-ASYNC-ITER-001 — Adaptar canales a `AsyncIterator`.**
  Cerrada la vista consumible bajo `T: Discard` sobre el protocolo ya cerrado:
  el compilador selecciona un witness privado `next(mut self)` y la VM hosted
  reutiliza el waiter FIFO de `receive`, conserva backpressure y cierra el
  receiver en salida normal o temprana. El `collect` genérico marca el endpoint
  antes de su primer poll para que cancelación y cleanup descarten sólo payloads
  `Discard`; los payloads afines reciben `E1105` y siguen usando `receive`/`close`.
  No se crea otro tipo de stream, no se promueve API pública y no se reclama
  lowering AOT. La fixture, negativa y evidencia están en
  `testing/stdlib-channel-async-iter.json` y
  `docs/contracts/stdlib-channel-async-iter.md`; `STD-CHANNEL-PERF-001` queda
  cerrado para su campaña hosted.
- [x] **STD-CHANNEL-TEST-001 — Modelar y fuzzear canales.** Cerrado el modelo
  independiente con ledger de ownership afín, colas FIFO de waiters, select
  prepare/rollback/commit, cancelación, cierre terminal, fairness y wakeups
  exactamente una vez. La suite de integración cubre buffers 0/N, unbounded
  acotado, productores/consumidores múltiples, ready simultáneo, `else`,
  abandono, drenaje FIFO y cleanup sobre 4096 seeds deterministas; el target
  libFuzzer completó 128 ejecuciones con seed 4104. Se mantienen las
  regresiones hosted VM/ABI nativo privado y la frontera `native_aot_lowering:
  not-claimed`. El contrato y la evidencia están en
  `testing/stdlib-channel-test.json`,
  `docs/contracts/stdlib-channel-test.md` y
  `scripts/stdlib-channel-test-check.sh`; la conformance de canales queda
  cerrada en el bloque siguiente.
- [x] **STD-CHANNEL-PERF-001 — Medir canales.** Cerrada la campaña
  target-qualified del VM hosted con nueve workloads explícitos 1:1, n:1 y
  n:m: rendezvous, buffers, unbounded burst, backpressure y wakeups al cerrar
  el último receiver. Tres procesos independientes aportan 27 muestras por
  workload, con latencia/throughput, P95/P99, memoria lógica, queue peak,
  allocations, backpressure, wakeups y handles vivos. El oráculo independiente
  de `channel_model` se ejecuta antes de cada campaña y la sonda verifica FIFO
  por commit, payloads afines intactos, un wakeup por waiter y cleanup sin
  endpoints vivos. El contrato y el informe son
  `testing/stdlib-channel-performance.json`,
  `docs/contracts/stdlib-channel-performance.md` y
  `target/reliability/evidence/stdlib-channel-performance.json`; el baseline
  queda limitado a `tondo-vm-hosted` / `bytecode-vm`, mantiene
  `native_aot: not-claimed` y difiere fast paths a una campaña nativa
  comparable. El siguiente bloque de ese slice es la conformance compartida.
- [x] **STD-CHANNEL-CONF-001 — Conformar canales.** Cerrado con un corpus de
  ocho casos y un fixture de pánico: la VM hosted emite líneas ordenadas para
  FIFO, rendezvous, errores con payload intacto, capacidad inválida, drenado,
  wakeups y cleanup diferido; la sonda nativa ejecuta los mismos IDs mediante
  el ABI privado y exige cero waiters/handles vivos. El caso de `select` queda
  explícitamente target-qualified porque el bridge nativo no publica una API
  de selección. La evidencia está en
  `testing/stdlib-channel-conformance.json`,
  `docs/contracts/stdlib-channel-conformance.md` y
  `target/reliability/evidence/stdlib-channel-conformance.json`; no reclama
  lowering AOT ni layout nativo público. La guía documental queda cerrada en
  `STD-CHANNEL-DOC-001`; el siguiente bloque es `STD-EXEC-IMPL-001`.
- [x] **STD-CHANNEL-DOC-001 — Documentar canales.** La guía normativa
  [`docs/contracts/stdlib-channel.md`](./docs/contracts/stdlib-channel.md)
  fija superficie, orden FIFO por commit, cierre, cancelación, fairness y
  costes explícitos sin inventar APIs paralelas ni prometer lowering AOT. El
  registro [`testing/stdlib-channel.json`](./testing/stdlib-channel.json)
  enlaza el fixture
  `tests/runtime/m11-std-channel-doc-001.to`, sus sidecars y
  `scripts/stdlib-channel-doc-check.sh`; las cinco familias
  `fan-out-fan-in`, `pipeline-backpressure`, `select-cancel-safe`,
  `close-and-drain` y `discardable-iteration` producen `channel-doc-ok` en
  la VM hosted. `scripts/stdlib-channel-doc-test.sh` rechaza secciones,
  estado, ejemplos, stdout y promoción stale; el cierre es target-qualified,
  no promociona símbolos runtime, ABI público ni lowering AOT. El siguiente
  bloque es `STD-EXEC-IMPL-001`.

#### 21.3.4 `std.executor`

- [x] **STD-EXEC-IMPL-001 — Implementar la superficie cooperativa hosted de
  executor, pools y actores.** Reutiliza Group, async estructurado, channels y
  sync; pools y mailboxes son acotados y no crean un segundo `Task` público. La
  VM hosted verifica la admisión cooperativa, backpressure, `Join`,
  shutdown/cancel, la creación/stop de un actor, `Actor.ref` y handlers con
  estado, FIFO, cancelación y propagación de error. La ruta transaccional
  `selectable` de `ActorRef.send` también queda verificada: prepare observa el
  payload, commit hace una única linearización en mailbox y rollback conserva
  el mensaje del caller y desregistra el waiter. El bridge de `BlockingPool`,
  sus workers host y el lowering native AOT permanecen explícitamente en los
  leaves posteriores; este cierre es target-qualified para la VM hosted y no
  promociona una API pública.
- [x] **STD-EXEC-HOST-001 — Implementar workers host.** La VM hosted enlaza
  `BlockingPool` a un bridge privado con workers OS, heaps hijos por job,
  adaptador de host propietario, admisión FIFO acotada, shutdown con drain y
  cancelación segura sin bloquear el scheduler cooperativo. El runtime nativo
  añade una lane target-qualified de tokens opacos para
  `x86_64-unknown-linux-gnu`, con lifecycle, wakeups, cancelación de cola y
  ownership ARC; no se reclama lowering native AOT ni ABI público. Evidencia:
  `scripts/stdlib-executor-implementation.sh` y
  `target/reliability/evidence/stdlib-executor-implementation.json`. El
  siguiente bloque es `STD-EXEC-PERF-001`.
- [x] **STD-EXEC-TEST-001 — Modelar y endurecer executor.** El modelo
  independiente cubre fairness, starvation, saturación, cancelación, pánicos,
  actores muertos, rechazo de trabajo, shutdown drenado, límites y races con
  schedulers deterministas; el replay de 4.096 semillas, el stress hosted de
  4 workers/32 jobs y el smoke libFuzzer de 128 ejecuciones (seed 4103, entrada
  máxima 4 KiB, 1.024 pasos) pasan. Evidencia: `testing/stdlib-executor-test.json`,
  `crates/tondo-reliability/src/executor_model.rs`,
  `crates/tondo-reliability/tests/models.rs`,
  `crates/tondo-vm/src/runtime/execute.rs` y
  `scripts/stdlib-executor-fuzz.sh`. El siguiente bloque es
  `STD-EXEC-PERF-001`.
- [x] **STD-EXEC-PERF-001 — Medir executor.** La campaña reproducible `3 x 9`
  cubre startup, roundtrip, throughput, saturación y drain en seis workloads
  hosted y seis de la lane privada nativa `x86_64-unknown-linux-gnu`. Conserva
  muestras, mediana/P95/P99, backpressure, esperas/completions del bridge,
  picos de cola/workers, memoria lógica y handles vivos, con el modelo
  independiente como oracle. Evidencia:
  `testing/stdlib-executor-performance.json`,
  `docs/contracts/stdlib-executor-performance.md`,
  `scripts/stdlib-executor-performance.sh` y
  `target/reliability/evidence/stdlib-executor-performance.json`. El siguiente
  bloque es `STD-EXEC-CONF-001`; native AOT y ABI público siguen
  `not-claimed`.
- [x] **STD-EXEC-CONF-001 — Conformar executor.** El corpus común de ocho
  casos compara líneas observables de la VM hosted con resultados normalizados
  del bridge native privado, cubre lifecycle, transferencia ARC, cancelación
  segura, actores y la frontera explícita de `threads` (`E1008`) sin lookup
  ambiental ni stubs silenciosos. Evidencia: `testing/stdlib-executor-conformance.json`,
  `docs/contracts/stdlib-executor-conformance.md`,
  `scripts/stdlib-executor-conformance.sh` y
  `target/reliability/evidence/stdlib-executor-conformance.json`; el native
  AOT de callables y ABI público siguen `not-claimed`; la guía documental
  ejecutable queda cerrada en el siguiente bloque.
- [x] **STD-EXEC-DOC-001 — Documentar executor.** La guía canónica cubre
  scopes y pools, actores y mailboxes, trabajo bloqueante, cancelación,
  shutdown, costes lógicos y los cinco patrones `scoped-join`,
  `bounded-backpressure`, `actor-mailbox`, `blocking-bridge` y
  `cancel-and-drain`. La fixture
  `tests/runtime/m11-std-executor-doc-001.to` se ejecuta en un proyecto que
  declara `clock` y `threads`, termina con `executor-doc-ok` y exit `0`; el contrato
  `testing/stdlib-executor.json`, los sidecars y
  `scripts/stdlib-executor-doc-check.sh` /
  `scripts/stdlib-executor-doc-test.sh` verifican la guía, sus enlaces y sus
  negativos. Esto documenta comportamiento y costes observados en VM hosted y
  en la lane nativa target-qualified sin promocionar API pública, ABI de layout
  ni lowering native AOT. El siguiente bloque owner es `DIAG-RUNTIME-001`.

#### 21.3.5 Calendario civil de `std.time`

- [ ] **STD-CIVIL-TIME-IMPL-001 — Implementar calendario y zonas.** Publicar
  Date, Time, DateTime, parsing/formatting, aritmética y conversiones de zona
  sobre el time-base único, con reglas y errores versionados.
- [ ] **STD-CIVIL-TIME-HOST-001 — Integrar datos temporales sellados.** Empaquetar
  la base de zonas por versión/hash y enlazar clock civil solo mediante la
  capability declarada; nunca consultar timezone ambiental durante build.
- [ ] **STD-CIVIL-TIME-TEST-001 — Modelar calendario civil.** Cubrir calendarios,
  leap years, gaps/folds DST, cambios históricos, límites, round-trips y
  providers versionados.
- [ ] **STD-CIVIL-TIME-PERF-001 — Medir tiempo civil.** Fijar parsing,
  formatting, lookup de zona, conversiones, memoria y tamaño de datos.
- [ ] **STD-CIVIL-TIME-CONF-001 — Conformar tiempo civil.** Ejecutar el corpus
  portable sobre VM/nativo con los mismos datos sellados y rechazo de versiones
  o domains incompatibles.
- [ ] **STD-CIVIL-TIME-DOC-001 — Documentar tiempo civil.** Separar claramente
  monotónico/civil, gaps/folds, datos versionados, costes y ejemplos.

#### 21.3.6 `std.encoding`

- [x] **STD-ENCODING-IMPL-001 — Implementar Base64 y hexadecimal.** Cerrada la
  ruta scalar bytes-first de `std.encoding` y su integración en la VM hosted:
  APIs materializadas e incrementales, policies Base64/hex estrictas, decode
  atómico, adapters `std.io.Reader`/`Writer`, límites, errores tipados,
  terminalidad y cleanup afín. La fixture
  `tests/runtime/m11-std-encoding-impl-001.to` y los tests directos del kernel,
  compiler host y materialización nominal VM pasan. No se reclama runtime nativo,
  SIMD ni lowering AOT; esas fronteras siguen sujetas a sus leaves posteriores.
- [x] **STD-ENCODING-TEST-001 — Probar y fuzzear encodings.** El modelo
  independiente y la ruta scalar/hosted cubren vectores RFC 4648, alfabetos y
  padding, cada frontera de chunk, casos inválidos de Base64/hex, límites,
  terminalidad y errores byte-exactos; el target `stdlib_encoding` queda
  acotado a 4.096 bytes/512 pasos con smoke reproducible de 128 ejecuciones.
  No se reclama runtime nativo público, SIMD ni lowering AOT.
- [x] **STD-ENCODING-PERF-001 — Medir encodings.** Fijar throughput, tail,
  allocations, memoria y dispatch multiversion por tamaño/target. Cerrado el
  baseline hosted scalar con
  `testing/stdlib-encoding-performance.json`,
  `docs/contracts/stdlib-encoding-performance.md`,
  `scripts/stdlib-encoding-performance-check.sh`,
  `scripts/stdlib-encoding-performance-test.sh` y
  `scripts/stdlib-encoding-performance.sh`. El probe real cubre 16 workloads,
  tres procesos independientes, 27 muestras por workload y exige
  `scalar-fixed-target`; native ABI, SIMD, tamaño de código y lowering AOT
  permanecen explícitamente no medidos.
- [x] **STD-ENCODING-CONF-001 — Conformar encodings.** La corpus común de
  seis casos verifica interoperabilidad VM/native, policies Base64/hex,
  fragmentación de streaming, errores byte-exactos con offsets, límites
  atómicos, terminalidad y cleanup de handles. El runtime nativo usa el
  kernel scalar de `std.encoding` mediante un ABI privado target-qualified;
  no se reclaman SIMD optimizado, Cranelift/AOT ni layout FFI.
- [x] **STD-ENCODING-DOC-001 — Documentar encodings.** La guía ejecutable
  [`docs/contracts/stdlib-encoding.md`](./docs/contracts/stdlib-encoding.md)
  publica una única forma por policy, errores observables, costes, ownership y
  ejemplos materializados/streaming. La fixture
  `tests/runtime/m11-std-encoding-doc-001.to`, sus sidecars y los runners
  `scripts/stdlib-encoding-doc-check.sh`/`scripts/stdlib-encoding-doc-test.sh`
  verifican seis familias y cierran la documentación hosted; runtime nativo
  público, SIMD optimizado y lowering AOT permanecen sin reclamar. El siguiente
  bloque es `STD-YAML-IMPL-001`.

#### 21.3.7 `std.yaml`

- [x] **STD-YAML-IMPL-001 — Implementar YAML seguro.** Cerrada la ruta YAML 1.2
  Core scalar y el bridge VM hosted buffered sobre `std.serialization`: parseo
  dinámico/typed, tags y aliases acotados, límites, errores con ubicación/path,
  codificación normal/canónica y lifecycle de `YamlReader`/`YamlWriter`. La
  fixture `tests/runtime/m11-std-yaml-impl-001.to`, los tests directos del
  kernel, compiler host y materialización nominal VM pasan. No se promociona
  una API pública ni se reclama runtime nativo o lowering AOT; el siguiente
  bloque es `STD-YAML-TEST-001`.
- [ ] **STD-YAML-TEST-001 — Probar y fuzzear YAML.** Cubrir corpus interoperable,
  Unicode, anchors/aliases, tags, duplicados, chunks, bombs, nesting y límites.
- [ ] **STD-YAML-PERF-001 — Medir YAML.** Fijar throughput, tail, memoria,
  allocations y comportamiento adversario sin optimizar solo documentos planos.
- [ ] **STD-YAML-CONF-001 — Conformar YAML.** Comparar typed/dynamic/streaming,
  interoperabilidad y errores/path sobre VM y nativo.
- [ ] **STD-YAML-DOC-001 — Documentar YAML.** Enumerar el subset seguro,
  policies, límites, costes y ejemplos ejecutables.

#### 21.3.8 `std.toml`

- [ ] **STD-TOML-IMPL-001 — Implementar TOML.** Publicar typed, árbol dinámico y
  parser con spans sobre serialization, preservando fecha/hora, duplicados y
  construcción atómica sin compartir semántica con el manifest del toolchain.
- [ ] **STD-TOML-TEST-001 — Probar y fuzzear TOML.** Cubrir corpus oficial,
  Unicode, números, fechas, tablas, arrays, duplicados, chunks aplicables,
  límites y spans exactos.
- [ ] **STD-TOML-PERF-001 — Medir TOML.** Fijar parsing/encoding, tail, memoria,
  allocations y documentos adversarios.
- [ ] **STD-TOML-CONF-001 — Conformar TOML.** Verificar interoperabilidad,
  typed/dynamic, errores/spans y equivalencia VM/nativo.
- [ ] **STD-TOML-DOC-001 — Documentar TOML.** Separar data format y
  `tondo.toml`, fijar policies, costes y ejemplos ejecutables.

#### 21.3.9 `std.cbor`

- [ ] **STD-CBOR-IMPL-001 — Implementar CBOR.** Publicar typed, dynamic y
  streaming con tags, longitudes definidas/indefinidas y modo determinista
  explícito sobre serialization.
- [ ] **STD-CBOR-TEST-001 — Probar y fuzzear CBOR.** Cubrir vectores RFC,
  floats/NaN, tags, maps, chunks, forms no mínimas, nesting, límites y
  preservación definida por policy.
- [ ] **STD-CBOR-PERF-001 — Medir CBOR.** Fijar throughput, tail, memoria,
  allocations y coste del modo determinista.
- [ ] **STD-CBOR-CONF-001 — Conformar CBOR.** Verificar interoperabilidad,
  typed/dynamic/streaming, determinismo y equivalencia VM/nativo.
- [ ] **STD-CBOR-DOC-001 — Documentar CBOR.** Explicar tags, determinismo,
  preservación, límites, costes y ejemplos ejecutables.

#### 21.3.10 `std.regex`

- [ ] **STD-REGEX-IMPL-001 — Implementar regex acotado.** Publicar compile,
  match, find y replace con sintaxis/Unicode cerrados y memoria/tiempo sometidos
  a límites; ninguna entrada válida activa backtracking exponencial oculto.
- [ ] **STD-REGEX-TEST-001 — Modelar y fuzzear regex.** Cubrir parser,
  automata/oracle, Unicode, vacíos, captures, replace, límites y patrones/input
  hostiles.
- [ ] **STD-REGEX-PERF-001 — Medir regex.** Fijar compile/match throughput,
  tail, memoria y tamaño de automata sobre corpus normal y adversario.
- [ ] **STD-REGEX-CONF-001 — Conformar regex.** Ejecutar vectores portables y
  equivalencia VM/nativo con los mismos límites y errores.
- [ ] **STD-REGEX-DOC-001 — Documentar regex.** Publicar sintaxis exacta,
  Unicode, captures, complejidad, límites y ejemplos ejecutables.

#### 21.3.11 `std.uuid`

- [ ] **STD-UUID-IMPL-001 — Implementar UUID.** Publicar representación,
  parse/format y generadores de las versiones fijadas, separando operaciones
  puras de civil-clock/entropy.
- [ ] **STD-UUID-HOST-001 — Integrar proveedores de UUID.** Enlazar entropy y
  clock declarados con límites y fallos nominales, sin RNG o reloj global
  implícitos.
- [ ] **STD-UUID-TEST-001 — Probar UUID.** Cubrir vectores por versión,
  canonical text, inválidos, orden aplicable, providers deterministas,
  colisiones modeladas y límites.
- [ ] **STD-UUID-PERF-001 — Medir UUID.** Fijar parse/format/generation,
  allocations, memoria y coste de providers.
- [ ] **STD-UUID-CONF-001 — Conformar UUID.** Verificar operaciones core y
  capabilities civil-clock/entropy sobre VM/nativo con providers sellados.
- [ ] **STD-UUID-DOC-001 — Documentar UUID.** Explicar versiones, seguridad de
  generación, providers, errores, costes y ejemplos ejecutables.

#### 21.3.12 `std.net`

- [ ] **STD-NET-IMPL-001 — Implementar networking portable.** Publicar
  direcciones, DNS, streams, datagrams y frontera TLS con partial I/O,
  deadlines y cancelación sobre `std.io`/executor.
- [ ] **STD-NET-HOST-001 — Implementar adaptadores de red.** Enlazar sockets,
  resolver y proveedor TLS declarados por target, sin I/O por import, fallback
  bloqueante oculto ni errores crudos del SO.
- [ ] **STD-NET-TEST-001 — Modelar y endurecer networking.** Cubrir fragmentación,
  backpressure, DNS, half-close, cancelación, timeouts, TLS boundary, teardown,
  límites y fallos host reproducibles.
- [ ] **STD-NET-PERF-001 — Medir networking.** Fijar throughput, tail, memoria,
  allocations, conexiones y cancelación con loopback/provider controlado.
- [ ] **STD-NET-CONF-001 — Conformar networking.** Ejecutar casos portables,
  capability-gated y de integración sobre targets reales VM/nativos.
- [ ] **STD-NET-DOC-001 — Documentar networking.** Publicar ownership,
  partial I/O, DNS/TLS, timeout, cancelación, errores, costes y ejemplos.

#### 21.3.13 `std.log`

- [ ] **STD-LOG-IMPL-001 — Implementar logging estructurado.** Publicar eventos,
  niveles, fields, filters y sinks explícitos con backpressure/fallo visible y
  sin globals ambientales.
- [ ] **STD-LOG-HOST-001 — Implementar sinks capability-gated.** Enlazar
  console, filesystem y network por unidades declaradas, con flush, rotación o
  entrega exactamente según el contrato del sink.
- [ ] **STD-LOG-TEST-001 — Probar logging.** Cubrir orden, concurrencia,
  backpressure, fallos, cancelación, redacción declarada, límites y teardown de
  sinks.
- [ ] **STD-LOG-PERF-001 — Medir logging.** Fijar disabled/enabled cost,
  throughput, tail, allocations, buffers y presión de sinks.
- [ ] **STD-LOG-CONF-001 — Conformar logging.** Verificar eventos core y sinks
  capability-gated en VM/nativo sin que fallos cambien control ocultamente.
- [ ] **STD-LOG-DOC-001 — Documentar logging.** Publicar fields, filters,
  sinks, backpressure, fallos, costes y ejemplos ejecutables.

#### 21.3.14 Coordinación de STD-0.1B

- [ ] **STD-B-OWNER-MATRIX-001 — Materializar celdas B por owner.** Generar un
  record canónico para cada módulo de 21.3.1–21.3.13 con `SPEC`, `IMPL`, `HOST`,
  `MODEL`, `TEST`, `FUZZ`, `PERF`, `CONF` y `DOC`. `HOST = not-applicable`
  requiere razón normativa y `MODEL/TEST/FUZZ` conservan identidades separadas;
  no se crean stubs ni tasks `HOST-NA` administrativas.

- [ ] **STD-B-IMPL-001 — Coordinar implementación portable por owner.** Cada
  task `*-IMPL-001` de 21.3.1–21.3.13 está cerrada y enlazada firma por firma;
  cualquier intrinsic o unidad privilegiada nueva tiene contrato y
  justificación explícitos.

- [ ] **STD-B-HOST-001 — Coordinar adaptadores por owner.** Cada módulo
  aplicable cierra su task `*-HOST-001`; los demás registran `not-applicable`
  con razón. VM y backend nativo enlazan `clock`, `network`, `threads`,
  `entropy` y sinks sin stubs que fallen siempre ni efectos por import.

- [ ] **STD-B-TEST-001 — Coordinar evidencia funcional por owner.** Cada módulo
  cierra su task `*-TEST-001` y registra MODEL/TEST/FUZZ por separado; ninguna
  evidencia interna sustituye un caso público del owner.

- [ ] **STD-B-PERF-001 — Coordinar rendimiento por owner.** Cada hot path
  cierra su task `*-PERF-001` con throughput, tail latency, memoria,
  allocations, startup, code size y compile time; SIMD conserva oracle y
  fallback portable.

- [ ] **STD-B-CONF-001 — Coordinar conformidad por owner.** Cada módulo cierra
  su task `*-CONF-001`, y `STD-B-OWNER-MATRIX-001` no conserva celdas aplicables
  vacías; el agregado publica casos portables/capability-gated y programas
  reales en VM/nativo regenerando la evidencia del árbol actual.

- [ ] **STD-B-DOC-001 — Coordinar documentación por owner.** Cada módulo cierra
  su task `*-DOC-001`, con firmas, errores, pánicos, ownership, async, orden,
  coste y ejemplos ejecutables; el agregado no crea un segundo PackageId.

- [ ] **DIAG-STDLIB-001 — Integrar diagnóstico con los owners concurrentes.**
  Después de `STD-B-IMPL-001`, `STD-B-HOST-001`, `STD-B-TEST-001` y
  `DIAG-NATIVE-001`, conectar channel/sync/executor/net a los eventos privados
  ya fijados. Ejecutar corpus positivo/negativo de happens-before, recursos,
  cancelación, sockets y dumps sobre VM y nativo; no añadir APIs públicas de
  detector ni aceptar `unsupported` para un owner que entra en S1.

- [ ] **STD-S1-SEAL-001 — Sellar la distribución conforme de Standard Library
  0.1.** Después de los micro-gates A/B, `DIAG-STDLIB-001` y Gate N1, reconstruir únicamente la
  stdlib, runtime/units/providers que posee, manifests, PackageId, content/API
  hashes y matriz de targets. Ejecutar conformidad VM/nativa y reproducibilidad
  desde dos workspaces limpios. Este bundle cierra S1 sin depender de L0 ni del
  empaquetado global del lenguaje.

- [ ] **REL-0.1-RC-001 — Construir el candidato completo de primera
  publicación.** Después de G5, T0, N1 y S1, reconstruir toolchain, VM,
  backend, runtime, stdlib runtime y companion meta desde inputs cerrados;
  fijar PackageId, content/API hashes, manifests, checksums, matriz de targets y
  provenance. Ejecutar conjuntamente G5, H0, T0, N1, el bundle exacto de
  `STD-S1-SEAL-001`, conformidad VM/nativa,
  programas representativos y reproducibilidad desde dos workspaces limpios.
  La tarea compone gates ya independientes y crea un candidato, pero no publica
  por sí misma. Si existe L0, se referencia como companion opcional mediante
  `TLF-REL-001`; su ausencia o cambio no modifica la identidad del candidato
  base.

- [ ] **REL-SUPPLY-001 — Cerrar licencia y cadena de suministro.** Antes de
  distribuir fuera del repositorio, registrar la decisión humana de licencia,
  añadir `LICENSE`/notices aplicables, generar SBOM y provenance verificables,
  auditar licencias de dependencias y firmar checksums/artefactos sin incluir
  secretos ni paths ambientales. No se elige una licencia por inferencia.

- [ ] **REL-INSTALL-001 — Verificar instalación y ciclo de actualización.**
  Desde los artefactos exactos de `REL-0.1-RC-001`, probar descarga, checksum,
  instalación limpia, `tondo --version`, hello world, `tondo test`, upgrade,
  rollback y uninstall en cada plataforma publicada, sin depender del checkout
  ni dejar estado global tras el test.

- [ ] **REL-PUBLISH-001 — Publicar Tondo 0.1 con autorización explícita.**
  Requiere `REL-0.1-RC-001`, `REL-SUPPLY-001`, `REL-INSTALL-001` y CI verde sobre
  los bytes finales. Crear tag/release, adjuntar artefactos, checksums,
  firmas/SBOM, notas y documentación solo tras una orden humana específica;
  verificar consumo externo y conservar un procedimiento de retirada sin
  reescribir tags ni artefactos ya publicados.

### Gate S1 — Standard Library 0.1 completa

- [ ] Cada módulo publicado tiene spec, capability matrix, implementación,
  modelos, properties, fuzzing, ejemplos y conformidad versionada.
- [ ] VM y backend nativo producen observaciones compatibles para todos los
  módulos aplicables.
- [ ] Límites de recursos y tratamiento de inputs no confiables están fijados y
  probados.
- [ ] Los módulos diferidos permanecen ausentes o experimentales de forma
  explícita; ningún nombre ilustrativo se anuncia como estable.
- [ ] La distribución STD 0.1.0 reúne STD-0.1A y STD-0.1B con un único PackageId,
  content/API hashes finales y matriz de targets reproducible.
- [ ] `STD-S1-SEAL-001` reproduce y verifica el bundle de stdlib sin requerir
  TLF ni el candidato global.
- [ ] Tondo 0.1, `tondo test`, VM, backend nativo y los programas
  representativos pasan juntos el gate estricto antes de cualquier publicación.
- [ ] El candidato fija G5 y S1; cualquier bundle L0 se enlaza como companion
  opcional, sin ampliar la semántica ni reutilizar evidencia incompatible.

---

## 22. Trabajo transversal

### 22.0 Estructura normativa

- [x] **SPEC-STRUCTURE-001 — Validar identidades estructurales de los specs.**
  `tondo-reliability check/generate` valida UTF-8, fences Markdown cerrados,
  headings ATX CommonMark de nivel 1–6, cierres opcionales, indentación 0–3,
  rutas jerárquicas únicas y números cuya profundidad/prefijo coincide con sus
  padres en lenguaje, testing, toolchain, stdlib y TLF. También liga el hash
  base declarado por testing a los bytes actuales del lenguaje y rechaza un
  estado documental que contradiga el runner funcional. Tests cubren títulos
  raíz, normalización, padres incorrectos, headings dentro de fences y fences
  truncados.

- [x] **TRACKER-LINT-001 — Validar el tracker como grafo.** Parsear tasks
  canónicas y gates, exigir IDs únicos, referencias exactas en campos de
  dependencia, ausencia de abreviaturas ambiguas y DAG acíclico. Los resúmenes
  de cerradas/abiertas y la cola topológica se derivan; no se mantienen cifras
  manuales ni se interpretan menciones históricas como dependencias activas.

### 22.1 Diagnósticos

Todo milestone debe:

- Emitir el código normativo más específico de la fase fiable más temprana.
- Mantener información estructurada como fuente única; la representación humana
  y JSON son vistas.
- Evitar cascadas que dependan de tipos o ownership inventados.
- Conservar paths lógicos y offsets de bytes.
- Ordenar diagnostics, related y fixes según el apartado 22.6.
- Añadir códigos propios solo bajo un prefijo distinto al registro normativo.

### 22.2 Determinismo

Desde M0:

- No depender del iteration order de hash maps internos para output observable.
- Ordenar símbolos, diagnostics, módulos e instanciaciones explícitamente.
- No leer red, reloj, locale o entorno como input implícito.
- Mantener paths físicos fuera de hashes y diagnostics normativos.
- Sembrar aleatoriedad de tests de forma reproducible y registrar la seed al
  fallar.

### 22.3 Testing

La pirámide prevista:

1. Unit tests de estructuras y algoritmos.
2. Golden tests de lexer, CST, formatter y diagnostics.
3. Compile-pass y compile-fail.
4. Runtime tests contra programas Tondo.
5. Property tests y fuzzing.
6. Tests de regresión para cada bug.
7. Suite oficial de conformidad.

Cada bug semántico debe terminar con un programa Tondo mínimo que habría fallado
antes de la corrección. Gate H0 convierte los puntos 5 y 6 en infraestructura
ejecutable para el toolchain. M10.6 añade después el runner público para
programas Tondo; STD-0.1A, M11 y STD-0.1B deben extender ambas fronteras, no
crear harnesses paralelos.

### 22.4 Seguridad y robustez

- Tratar fuente, bytecode, interfaces y manifiestos como inputs no confiables.
- Validar bytecode aunque lo haya producido el propio compilador.
- Evitar recursión del host sin límite al recorrer sintaxis o tipos.
- Limitar tamaño de instanciación genérica y resolución de traits.
- No ejecutar comandos durante compilación.
- No consultar red durante compilación.
- Mantener shell explícito y separado de argumentos.
- Probar parser, loader y JSON con fuzzing.

### 22.5 Rendimiento

Antes de G3, priorizar corrección y claridad. No introducir:

- NaN-boxing.
- JIT.
- ARC optimizado.
- COW complejo.
- Query engine incremental.
- Paralelismo del compilador.

Después de G3, medir como mínimo:

- Tiempo de cold `check`.
- Tiempo de `fmt`.
- Memoria pico del frontend.
- Número y tamaño de monomorfizaciones.
- Dispatches de bytecode por segundo.
- Pausas y memoria viva del GC.
- Coste de copias lógicas de arrays y maps.

Una optimización solo se acepta si conserva los mismos tests observables y aporta
una mejora medida.

### 22.6 Disciplina de librería estándar

La stdlib continúa siendo una especificación separada. El compilador solo debe
anticipar lo que el lenguaje ya declara intrínseco. STD-0.1A y STD-0.1B
convierten el siguiente orden en milestones con gates de una misma versión; esta
sección conserva las reglas que se aplican a ambas fases.

Orden recomendado:

1. **Bootstrap host shim:** `std.console.print`, únicamente para ejecutar los
   primeros programas.
2. **Time-base spec:** `Duration`, `Instant` monotónico, suspensión, timers y
   deadlines compartidos por producción y testing.
3. **Core stdlib spec:** métodos exactos de `String`, `Array`, `Map`, `Set`,
   `Range`, iterators, `Bytes`, protocolos de I/O, matemática, formatting y
   helpers portables de testing, más serialization, JSON, MessagePack y
   Protobuf.
4. **Hosted stdlib spec:** consola, environment, paths, filesystem y procesos.
5. **Concurrency stdlib spec:** channels, mutexes, atomics, actors y pools.
6. **Application stdlib:** calendario civil, networking, encodings,
   YAML/TOML/CBOR, regex, UUID y logging.

Los nombres ilustrativos del spec del lenguaje no deben implementarse como API
pública definitiva hasta ser fijados por la especificación estándar.

### 22.7 Tondo LLM Form

TLF se implementa como codec puro delante del frontend ordinario. No añade
tokens a `.to`, no crea otro AST y no puede consultar resolución o tipos para
decodificar. La cadena de trabajo es:

~~~text
TLF-RESEARCH-001 -> TLF-SPEC-001 -> TLF-CODEC-001
  -> {TLF-CANON-001 + TLF-MAP-001 + TLF-DIAG-001}
  -> TLF-CLI-001
  -> {TLF-PROP-001 + TLF-FUZZ-001 + TLF-EVAL-001} --+
                                                            |
TLF-RESEARCH-001 -> TLF-BENCH-REPRO-001 --------------------+
                                                            |
                                                            v
                 TLF-CONF-001 -> TLF-BUNDLE-001 -> Gate L0
~~~

- [x] **TLF-RESEARCH-001 — Medir shapes compactos con tokenizers reales.** El
  estudio [`docs/measurements/tlf-token-study.md`](./docs/measurements/tlf-token-study.md)
  deduplica 154 fuentes Tondo, usa el lexer real y compara cinco tokenizers. La
  cinta léxica elegida reduce 16,18 % de tokens agregados sin aliases ni
  diccionario; el spike conserva los códigos léxicos/sintácticos en 154/154
  expansiones. No afirma todavía calidad de generación.

- [ ] **TLF-BENCH-REPRO-001 — Hacer reproducible el estudio léxico.** Añadir
  harness versionado, manifest SHA-256 del corpus deduplicado, revisiones o
  hashes exactos de los cinco tokenizers, algoritmos de cada candidato y
  resultados machine-readable de los que se derive el Markdown. Un cambio de
  corpus, tokenizer o transformación invalida el resultado previo y el check
  debe detectar drift sin acceso implícito a red.

- [x] **TLF-SPEC-001 — Cerrar el contrato del formato.** La especificación
  [`TONDO_LLM_FORM_SPEC.md`](./TONDO_LLM_FORM_SPEC.md) fija identidad draft,
  source forms, `;` como `NL` lógico, canonicalización, expansión, comentarios,
  source maps, límites, CLI prevista y gates. ADR-018 conserva una sola
  semántica Tondo.

- [ ] **TLF-CODEC-001 — Implementar encoder y decoder puros.** Crear una
  frontera propia en `tondo-compiler` sobre el lexer/CST sin duplicar el parser.
  La expansión produce bytes `.to` solo al terminar y verifica los invariantes
  `E(P(s))` y complejidad lineal bajo límites.

- [ ] **TLF-CANON-001 — Implementar la forma canónica.** Emitir separadores
  mínimos, omitir el `NL` opcional posterior a `{` y el terminal, conservar
  docs/shebang y demostrar fixed point byte a byte. No aceptar configuración de
  estilo ni perfiles por tokenizer.

- [ ] **TLF-MAP-001 — Componer source maps.** Mapear tokens, `;`, whitespace
  insertado y edits del formatter en offsets UTF-8; probar Unicode, strings,
  interpolación, comments y spans generados sin fallback al archivo completo.

- [ ] **TLF-DIAG-001 — Cerrar diagnósticos y patches.** Reservar `E22xx`, fijar
  precedencia, limits, ubicación primaria TLF y fixes aplicables. Los errores
  Tondo conservan su código normativo después de remapping.

- [ ] **TLF-CLI-001 — Exponer `tondo llm`.** Implementar
  `encode/decode/check/fmt`, source form explícito, streams y exits atómicos.
  `check` reutiliza el frontend normal; ningún comando de proyecto descubre
  `.tlf`.

- [ ] **TLF-PROP-001 — Probar round-trip y equivalencia diferencial.** Añadir
  goldens y properties sobre toda la gramática, comparar formatter, diagnostics,
  MIR observable y ejecución original/expandida, y registrar regresiones
  minimizadas.

- [ ] **TLF-FUZZ-001 — Endurecer input hostil.** Fuzzear codec, strings,
  comments, interpolaciones, UTF-8, separadores, nesting, límites y source maps;
  ninguna entrada puede abortar, crecer sin cota o publicar output parcial.

- [ ] **TLF-EVAL-001 — Medir programas correctos por token.** Ejecutar una
  matriz reproducible multi-modelo que incluya coste de enseñar TLF, validez en
  primer intento, typecheck, aceptación, reparaciones y tokens totales frente a
  Tondo canónico/minificado. Un ahorro bruto sin mejora total no promociona el
  formato.

- [ ] **TLF-CONF-001 — Sellar Gate L0.** Publicar vectores independientes,
  corpus adversario, hashes del formato/medición y evidencia portable de codec,
  maps, CLI, properties, fuzzing y evaluación. Requiere el harness reproducible
  de `TLF-BENCH-REPRO-001` y no atribuye evidencia TLF a G5.

- [ ] **TLF-BUNDLE-001 — Construir el bundle content-addressed de L0.** Fijar
  spec TLF, identidad del formato, codec/maps/CLI, golden y fuzz corpora,
  harnesses, manifests, resultados léxicos/multi-modelo y frontend Tondo usado
  como oracle. Verificar el bundle sin consultar el workspace vivo. Este bundle
  es separado del linaje G5 y no bloquea el candidato base.

- [ ] **TLF-REL-001 — Empaquetar TLF como companion opcional.** Después de
  `TLF-BUNDLE-001`, producir artefactos y checksums que declaren tanto la
  identidad L0 como el rango exacto de toolchains Tondo compatibles. Puede
  acompañar a `REL-0.1-RC-001`, pero nunca cambia su hash, sus gates ni su
  disponibilidad.

---

## 23. Registro de riesgos

| ID | Riesgo | Efecto | Mitigación |
|---|---|---|---|
| `R-001` | Intentar implementar toda la superficie antes de ejecutar nada | Meses sin feedback semántico real | Gates verticales G0, G1 y G2 |
| `R-002` | Parser y formatter construidos sobre árboles distintos | Divergencias, pérdida de comentarios y fixes frágiles | CST sin pérdida compartido |
| `R-003` | Comprometer una representación runtime demasiado pronto | Reescritura al llegar ownership o async | `Value` explícito, bytecode por slots y ADR de objetos |
| `R-004` | Implementar COW antes de validar copias lógicas | Complejidad y bugs de aliasing | Copia eager primero, COW medido después |
| `R-005` | Implementar ARC y collector de ciclos en el bootstrap | El runtime bloquea al lenguaje | Mark-and-sweep simple en la VM |
| `R-006` | Posponer cleanup edges en el MIR | Rediseño al añadir `defer`, terminales y cancelación | Modelarlos desde M3 |
| `R-007` | Mezclar borrow checking con type checking ad hoc | Diagnósticos inestables y análisis incompleto | Dataflow separado sobre MIR tipado |
| `R-008` | Tratar async como wrapper de retorno | Contradice el modelo visible de Tondo | Lowering a frames después del type checking |
| `R-009` | Congelar accidentalmente APIs ilustrativas de stdlib | Compatibilidad prematura | Shim aislado y spec estándar separada |
| `R-010` | Introducir feature gates en fuente para el bootstrap | Crear dialectos Tondo incompatibles | Rechazo explícito del toolchain |
| `R-011` | Fijar códigos estables sin tests de precedencia | Cascadas y cambios incompatibles | Golden tests por código y fase |
| `R-012` | Monomorfización sin límites | Explosión de código o compilación no terminante | Métrica decreciente, límites y diagnostics |
| `R-013` | Usar hash iteration para outputs | Builds y diagnostics no reproducibles | Orden explícito en cada frontera observable |
| `R-014` | Añadir executor multithread demasiado pronto | Bugs de memoria y scheduling difíciles de aislar | Executor cooperativo single-thread inicial |
| `R-015` | Considerar terminado lo que solo compila | Falsa sensación de soporte | Estados separados de implementación, validación y conformidad |
| `R-016` | Medir calidad por cantidad bruta de tests | Duplicados y ejemplos grandes ocultan reglas sin oracle | Inventario por fuente única, requisito, property, modelo y mutación |
| `R-017` | Empezar el backend nativo antes de Gate H0 | Dos runtimes divergen sin poder localizar la causa | CI, fuzzing, modelos y oracle VM antes de NATIVE-001 |
| `R-018` | Diseñar runtime/ABI antes de conocer memoria y concurrencia 0.1 | Atomicidad, wakeups, bloqueo y layouts obligan a rehacer el backend o limitan la API | S1A y contratos runtime-facing de channel/sync/executor/net antes de NATIVE-001; DEC-014 antes de ABI/lowering |
| `R-019` | Convertir la stdlib 0.1 en un proyecto ilimitado | El backend queda bloqueado por APIs de aplicación no esenciales | Catálogo cerrado, cinco slices tempranos estrictos, S1A antes de M11 y solo contratos B runtime-facing antes de N1; ninguna fase amplía scope sin revisar spec y tracker |
| `R-020` | Separar el corpus del contrato vivo | Spec, parser y casos divergen o se anuncia soporte inexistente | Spec, parser, manifest y corpus se actualizan juntos; el gate rechaza cualquier drift |
| `R-021` | Permitir que unit tests cambien la compilación de producción | Código solo correcto bajo test y artefactos distintos | Unidad production sellada antes del overlay y comparación exacta de productos |
| `R-022` | Ocultar flakiness mediante paralelismo o retries | Suites verdes no reproducibles | Tiempo virtual para causalidad temporal; una ejecución por default; retries explícitos, historial completo y `flaky-pass` rojo salvo `--allow-flaky`; orden/seed/paralelismo reportados |
| `R-023` | Convertir testing en attributes, reflection, context parameters y hooks especiales | Segundo sublenguaje con boilerplate y semántica oculta | Dos roles canónicos: `suite` contenedor y `test` hoja; envelope sellado sin valor visible; helpers, fixtures y doubles como Tondo/stdlib ordinarios |
| `R-024` | Convertir suites en globals mutables u orden implícito | Data races, tests dependientes y resultados distintos bajo `--exact` | Capturas `let: Copy + Send + Share`, ownership del recurso en la suite, sin dependencias ni orden semántico entre hojas y lifecycle reportado |
| `R-025` | Implementar logs/control con un global o thread-local | Eventos atribuidos al test incorrecto bajo async, migración o paralelismo | Envelope por raíz que sigue frames/tasks; operaciones selladas y revalidadas por HIR/MIR/bytecode |
| `R-026` | Permitir que skips escondan regresiones o cleanup fallido | CI verde con cobertura real ausente o recursos sin cerrar | Razón obligatoria, sin ignored estático, cleanup antes de confirmar, fallo con precedencia y `--deny-skips` |
| `R-027` | Usar tags runtime como autoridad de discovery o scheduling | Ejecutar el body cambia qué tests existen, el shard o su orden | Tags solo en el envelope posterior al dispatch; selección, ownership, sharding y orden usan metadata estática |
| `R-028` | Particionar u ordenar mediante hashes o iteration order del host | Shards solapados, huecos y seeds que no reproducen entre plataformas | Algoritmos versionados sobre IDs UTF-8, vectores de conformidad y `execution_plan` reportado |
| `R-029` | Tratar JUnit como representación normativa sin pérdida | Consumidores CI descartan jerarquía, metadata o estados Tondo | JSON `/7` es canónico; JUnit `/4` es una proyección versionada de la misma ejecución y CI puede emitir ambos |
| `R-030` | Reintentar dentro del mismo runtime o restaurar una fixture parcial | Un intento pasa por heap, tasks, handles o buffers heredados y no confirma flakiness real | Worker nuevo por unidad; solo artefacto inmutable reutilizable; recursos rastreados revocados antes de completar |
| `R-031` | Tratar un éxito posterior como pass ordinario | CI verde oculta una regresión intermitente y pierde la evidencia inicial | Estado `flaky-pass`, todos los intentos en JSON/JUnit y exit `1` por default; `--allow-flaky` es policy explícita |
| `R-032` | Añadir regex o delegar globs al host | Dialectos, expansión accidental, complejidad no acotada y selección distinta por plataforma | Un glob propio de match completo con `*`/`?`/`**`, gramática cerrada, DP acotada y vectores multiplataforma |
| `R-033` | Crear un reloj o `Duration` exclusivo de testing | Los tests validan otra API y la semántica diverge de producción | Time-base de STD-0.1 antes de T0; mismo bytecode y frontera monotónica con proveedor real o virtual |
| `R-034` | Tratar I/O externo o timeout del runner como tiempo virtualizable | Saltos prematuros, hangs o tests instantáneos que fallan en producción | Catálogo cerrado de bloqueo durable; I/O/civil continúan reales y timeout/límites siempre usan recursos reales |
| `R-035` | Publicar un resultado parcial tras interrupción | CI consume una ejecución incompleta como evidencia válida o acepta snapshots a medias | Stop dispatch, cleanup/revocación acotados, exits `4`/`3` y publicación atómica solo tras completar |
| `R-036` | Añadir hooks async exclusivos del runner o esperar cleanup de forma oculta | Segundo lifecycle, orden sorprendente y código distinto entre test y producción | `defer` general de Tondo 0.1, infallible y verificable; suites reutilizan exactamente esa semántica |
| `R-037` | Embeber, subir o dejar crecer artifacts sin contrato | Filtración de datos, reportes enormes y almacenamiento no reproducible | `attach` explícito, límites, descriptors SHA-256 y store content-addressed local sin Base64 ni upload |
| `R-038` | Hashear, reportar o redactor heurísticamente secretos | El secreto se filtra o la reproducibilidad declarada es falsa | Descriptors/versiones solamente, materialización/revocación por worker y responsabilidad explícita si el programa copia el valor |
| `R-039` | Confundir repeat con retry o solapar iteraciones | Una campaña deja de reproducir el plan y comparte estado entre oportunidades | Modos incompatibles, iteraciones completas secuenciales en workers nuevos, count uno como no-op y cualquier non-pass rojo con count mayor |
| `R-040` | Actualizar o borrar snapshots de forma automática o parcial | CI acepta cambios accidentales y el store queda incoherente tras fallos | Texto exacto, update explícito y restringido, staging total, reemplazo atómico y preservación de entries no alcanzadas |
| `R-041` | Convertir metaprogramación en ejecución arbitraria del compilador | Builds ambientales, no cacheables y con acceso a datos del host | Frontend puro, target `tondo-meta` sin capabilities, una ronda, inputs/outputs/límites/hashes cerrados y VM nueva por run |
| `R-042` | Usar reflection runtime para serializers | Lookup, boxing, metadata global, acceso privado y hot paths difíciles de optimizar | Traits estáticos, derive/codegen directo y `std.reflect` descriptivo sin valores |
| `R-043` | Permitir que código generado eluda coherencia o visibilidad | Impls imposibles de escribir manualmente y APIs con privilegios ocultos | Output Tondo ordinario, owner exacto, vista privada limitada, mismo typecheck y consulta de expansión/source map |
| `R-044` | Optimizar codecs solo para benchmarks felices | Regresiones de allocation/latencia, DoS por inputs hostiles o semántica distinta entre SIMD/scalar | Oracle escalar, gates multidimensionales, corpus adversario, límites y equivalencia byte/error exacta por kernel |
| `R-045` | Inferir Protobuf desde records o reflection | Field numbers inestables, presencia perdida y evolución wire incompatible | Schema-first desde `.proto` fijado, unknown fields preservados y checks de evolución en build time |
| `R-046` | Tratar un manifest anterior como evidencia del draft | El gate queda roto o se atribuyen casos obsoletos a reglas nuevas | Un solo manifest vivo, requisitos pendientes honestos y ratchet por wave |
| `R-047` | Cerrar bloques enormes con una sola tarea umbrella | No se conoce el estado real por módulo y los fallos aparecen al final | Micro-gates verticales, modelo único de resultados y estado SPEC/IMPL/TEST/PERF/CONF/DOC por owner |
| `R-048` | Usar la recursión del host como pila del parser | Un input válido o malicioso aborta antes del límite tipado en targets con stacks pequeños | Guarda portable temporal; `PARSER-STACK-001` migra toda profundidad controlada por fuente a frames explícitos y conserva solo presupuestos configurables |
| `R-049` | Tratar un path existente como prueba de implementación del owner | Un kernel parcial cierra decenas de firmas que ningún programa Tondo puede llamar | Matriz firma → HIR → lowering → runtime → caso público y `STD-PUBLIC-API-AUDIT-001` fail-closed |
| `R-050` | Confundir bundle sellado con conformidad completa | El candidato fija bytes reproducibles pero omite requisitos sin evidencia o specs no inventariados | Matriz multi-spec, clasificación individual de límites y `CONF-SEAL-FINAL-001` como único cierre de G5 |
| `R-051` | Optimizar TLF por caracteres o por un solo tokenizer | Aliases crípticos fragmentan tooling y el ahorro desaparece en errores/reparaciones | Tokens Tondo intactos, formato único, benchmark multi-tokenizer y Gate L0 por programas correctos y tokens totales |
| `R-052` | Implementar `select` como helper, macro o carrera de tasks | La espera implícita consume el primer caso, los perdedores producen efectos, se pierden payloads/wakeups y async parece una librería paralela | Keyword núcleo, capacidad `selectable`, ABI prepare/commit/rollback, ownership por rama, modelo independiente y paridad VM/nativa |

---

## 24. Cola inmediata

### 24.1 Fuente canónica del grafo

`testing/tracker-graph.json` es el manifiesto de dependencias activo. Las
declaraciones de tareas y su estado (`[x]`/`[ ]`) se leen de las checklists
normativas del documento; no existe una segunda sección histórica. El
manifiesto solo enumera aristas no raíz en `task_dependencies` y
`gate_dependencies`: toda tarea o gate activo que no aparece en esos mapas es
una raíz explícita con `depends_on: []`. Cada referencia debe ser un ID exacto,
sin prefijos, comodines ni abreviaturas. El linter deriva los conteos, la cola
de trabajo lista y el orden topológico, y rechaza duplicados, referencias
desconocidas, dependencias repetidas, auto-dependencias y ciclos.

La compuerta reproducible es:

```text
cargo run -p tondo-reliability --locked -- tracker lint --root .
```

`--json` expone el informe derivado para CI y tooling; no existe un segundo
resumen manual que pueda divergir del tracker.

Los puntos 1–19 conservan la secuencia ya completada. A partir del 20 la unidad
de integración es una **wave vertical**. Una tarea puede empezar tan pronto
como estén cerrados sus prerequisitos duros; la wave posterior no se integra ni
se declara terminada antes del mini-gate anterior. Así, specs, algoritmos puros
y spikes explícitamente independientes pueden avanzar sin convertir el orden de
gates en una barrera artificial.

1. [x] Crear el repositorio y workspace Rust mínimo.
2. [x] Escribir `architecture.md` y los ADR de partida.
3. [x] Fijar contrato de CLI, source model y diagnostics JSON.
4. [x] Crear el harness que pueda ejecutar casos extraídos del spec.
5. [x] Implementar lexer con trivia, spans y errores léxicos.
6. [x] Implementar CST sin pérdida y parser recuperable.
7. [x] Implementar el formatter normativo y su corpus.
8. [x] Implementar resolución de nombres y representación canónica de tipos.
9. [x] Implementar el subconjunto semántico de G1.
10. [x] Diseñar MIR con cleanup edges antes de escribir la VM.
11. [x] Implementar bytecode verificado por slots.
12. [x] Implementar la VM y ejecutar los programas de aceptación de G2.
13. [x] Auditar cantidad física, casos lógicos, repeticiones, fuentes únicas y
    técnicas de testing de Tondo 0.1.
14. [x] Ejecutar **TEST-001** y crear el inventario machine-readable.
15. [x] Ejecutar **TEST-002** y **TEST-003** para materializar trazabilidad y
    dimensiones normativas.
16. [x] Ejecutar **CI-TEST-001** a **CI-TEST-004** y convertir el gate existente
    en evidencia continua.
17. [x] Añadir generadores, properties, fuzz targets y modelos de M10.5.
18. [x] Medir coverage y mutation score, cerrar huecos críticos y superar H0.
19. [x] Ejecutar **STD-FOUNDATION-SPEC-001** y cerrar **DEC-012** sin fingir
    que las APIs de módulo o STD-0.1 completa ya están publicadas.
20. [x] **Wave 0 — Evidencia del draft.** `CONF-DRAFT-001` y
    `CONF-RATCHET-001` están cerrados; el manifest draft es la única identidad
    activa. El registro
    `testing/conformance-ratchet.json` fija hashes de manifest, inventario,
    matriz y quality baseline; no atribuye las capas pendientes como pass y el
    gate de quality lo valida con reports frescos y hashes semánticos portables;
    el gate estricto valida los registros deterministas sin exigir herramientas
    de coverage/mutation.
21. [x] **Wave 1 — Formatos draft.** Implementar `META-FORMAT-001` con
    parse/canonicalización/round-trip y rechazo de records no draft. El mini-gate
    queda cerrado: manifest, lockfile, interface, artifact y descriptor estándar
    usan una única forma draft; el corpus se regenera para esa forma.
22. [x] **Wave 2 — Prerrequisitos y frontends, en paralelo.**
    - [x] Base portable: cerrar `PARSER-STACK-001`; lexer y planes que no modifican
      descenso sintáctico pueden avanzar en paralelo, y
      `META-SYNTAX-001` y `UTEST-CST-001` esperan la pila explícita.
    - Lane meta: `STD-META-SPEC-001 → META-VM-001`,
      `META-SYNTAX-001 → META-SEM-001 → META-MODEL-001` y
      `STD-REFLECT-001` avanzan en paralelo; después
      `(META-VM-001 + META-MODEL-001) → STD-META-IMPL-001 →
      STD-META-CONF-001`.
    - Lane testing estándar: `STD-BYTES-SPEC-001 →
      STD-BYTES-IMPL-001 → STD-BYTES-CONF-001` está cerrada; `STD-TIME-BASE-SPEC-001`
      también está cerrado y sus tareas `IMPL → CONF` avanzan en paralelo con
      `STD-ENV-SPEC-001`; desde el spec de bytes puede avanzar `STD-ENV-SPEC-001`, pero
      `(STD-BYTES-CONF-001 + STD-ENV-SPEC-001) → STD-ENV-IMPL-001 →
      STD-ENV-CONF-001`. En paralelo,
      `STD-TIME-BASE-IMPL-001 →
      STD-TIME-BASE-CONF-001`.
    - Lane testing plan: `UTEST-PLAN-001 →
      (UTEST-INPUTS-PLAN-001 + UTEST-DISC-001 + UTEST-OWNERS-001 +
      UTEST-DEPS-001) → UTEST-CLI-PARSE-001`.
    - Lane lenguaje: `UTEST-LEX-001 → UTEST-CST-001 → UTEST-FMT-001`; tras
      unir formatter con plan/discovery/dev-dependencies,
      `UTEST-ID-001 → UTEST-CAPTURE-001`, y después
      `UTEST-OVERLAY-001` y `UTEST-INTEG-001` cierran en paralelo.
      `ASYNC-DEFER-IMPL-001` ya está cerrado y no añade una lane pendiente al
      lowering.
    Mini-gate: cada frontend baja hasta su primer IR verificable, los cinco
    slices tempranos tienen owner definitivo y la conformidad viva ratchetea.
23. [x] **Wave 3 — Vertical slices ejecutables.**
    - Meta: `META-DERIVE-001 + META-GEN-001 → META-ATOMIC-001`, seguido de
      `META-QUERY-001` y `REFLECT-IMPL-001`.
    - Testing core: `UTEST-CHECK-001 → UTEST-LOWER-001` y
      `UTEST-RESULT-MODEL-001` avanzan en paralelo; su unión alimenta
      `UTEST-CONTROL-001 → UTEST-RUNTIME-001`. Desde ahí,
      `UTEST-INPUTS-001` avanza en paralelo con
      `UTEST-SUITE-001 → UTEST-LIMIT-001`, y los tres cierran el worker estable.
    - Testing puro: `UTEST-GLOB-001 → UTEST-SHARD-001 →
      UTEST-SCHED-001`, después de plan e identidad.
    Mini-gate: derive y un test mínimo recorren rutas públicas end-to-end; no
    existen shims, productos parciales ni estado cruzado entre workers.
24. [x] **Wave 4 — Features y cierre del lenguaje.**
    - Meta: diagnostics, reproducibilidad, robustez y `META-CONF-001`.
    - Testing sobre worker estable: `UTEST-VTIME-001`, `UTEST-RETRY-001`,
      `UTEST-REPEAT-001`, `UTEST-ARTIFACT-001` y `UTEST-SNAPSHOT-001`
      avanzan en paralelo; su unión alimenta `UTEST-REPORT-001 →
      UTEST-JUNIT-001 → UTEST-INTERRUPT-001 → UTEST-CLI-001`.
    - Aceptación testing: `UTEST-CONF-001`, `UTEST-PROJECTS-001`,
      `UTEST-PLATFORM-001` y `UTEST-DOGFOOD-001`; después se cierra T0.
    - Unión de implementación: `META-CONF-001 + UTEST-CONF-001` alimenta el
      resultado vivo conjunto sin crear un bundle pre-release.
    Mini-gate observado: meta/testing ejecutan sus rutas públicas sobre el
    mismo corpus. El cierre evidencial de T0 y la preparación de G5 se asignan a
    `DOC-TEST-001`, `DOC-TEST-CONF-001`, `UTEST-SPEC-EVIDENCE-001`, `CONF-MATRIX-ALL-001`,
    `CONF-GAP-AUDIT-001`, `CONF-GAP-IMPL-001`, `CONF-LAYER-RESULT-001`,
    `QUALITY-EVIDENCE-BIND-001`; `CONF-SEAL-FINAL-001` queda para el primer
    candidato real.
25. [x] **Wave 4.5 — Suspensión inferida canónica.** Completar
    `ASYNC-INFER-001`, `ASYNC-IMPLICIT-WAIT-001`, `ASYNC-EFFECT-API-001`,
    `ASYNC-SUSPENDS-DENOTE-001`, `ASYNC-JOIN-RETURN-001`,
    `ASYNC-THREAD-SPAWN-001`, `ASYNC-ONESHOT-001` y `ASYNC-ITER-001`, junto a la actualización de `SCRIPT-004`, del ABI de
    bytecode y de los contratos de I/O. El mini-gate exige `rg` sin firmas
    fuente `async fn`, rechazo `E1611` de `await` sobre llamadas directas,
    metadatos `suspends` con hash estable, `Join` transferible, one-shot,
    inferencia en `defer` y el único `for` sobre `AsyncIterator`. `async` es un
    identificador ordinario; no hay fixtures ni adapters de compatibilidad.
    `ASYNC-ITER-EXT-001` continúa como leaf explícita; `NATIVE-THREAD-001`,
    `NATIVE-002`, `ARC-001`, `ARC-002` y `DIAG-NATIVE-001` están cerradas y la
    frontera Core nativa está cerrada y la siguiente es `NATIVE-STD-HOSTED-001`.

#### Evidencia de Wave 4.5

El estado siguiente se basa en los tests ejecutables actuales y el gate local
completo; los conteos se regeneran y no son contratos fijados a un commit:

| Entrada | Estado auditado | Evidencia observada |
| --- | --- | --- |
| `ASYNC-001..004` | Cerradas | `cargo test -p tondo-compiler --lib`: 1.126/1.126; tests de inferencia de suspensión, diagnósticos de parámetros exclusivos, liveness `Send`, lowering/verificación de `Await`/`Spawn`/frames y ejecución de roots suspendidos. |
| `SPAWN-001`, `JOIN-001` | Cerradas | Tests de `scope`/cleanup, `direct_suspension_is_inferred_and_join_can_cross_a_function_boundary`, `join_can_be_returned_as_an_explicit_scope_handoff`, consumo afín y rechazo de doble consumo; el contrato de ownership se completa en `ASYNC-JOIN-RETURN-001`. |
| `SCRIPT-004` | Cerrada | `script_entry_infers_suspension_for_direct_waiter_calls`, `script_entry_executes_sync_and_async_top_level_work` y `tests/runtime/m10-defer-script.to`, además del gate de fixtures. |
| `ASYNC-INFER-001`, `ASYNC-IMPLICIT-WAIT-001`, `ASYNC-EFFECT-API-001`, `ASYNC-SUSPENDS-DENOTE-001`, `ASYNC-JOIN-RETURN-001`, `ASYNC-THREAD-SPAWN-001`, `ASYNC-ONESHOT-001`, `ASYNC-ITER-001` | Cerradas | El efecto público es denotable en contratos y tipos, continúa infiriéndose en cuerpos, y el gate cubre inferencia, préstamos secuenciales, rechazo en `spawn`, `Join`, one-shot, thread lane e iteración async. |
| `ASYNC-DEFER-IMPL-001` | **Cerrada** | Fixtures canónicos y de script cubren retorno, error exterior, pánico/supresión, LIFO, cancelación, host-backed cleanup e inferencia; negativos cubren `E1601`, `E1608`, `E1611`, `E1401`, `E1605`, `E1410` y `E1008`. Los tests de driver prueban `T0002`, capability y precedencia de panic; `test_runtime` fija timeout forzado y `test_interrupt` exige acknowledgement de cleanup antes de exit 4. |
| `UTEST-SUSPENSION-CONTRACT-001` | Cerrada | Parser/CST acepta `@sync`/`@nosuspend`; HIR/checker preserva `fn` + inferencia, espera implícita y `E1601`; fixtures compile-pass/compile-fail/runtime canónicos y `crates/tondo-reference-adapter/tests/suspension_contracts.rs` fijan `E1611` para `await call()`, llamada directa a `Waiter.wait()` y hashes de interfaz `suspends`. |
| `STD-A-ASYNC-IMPL-001` | **Cerrada** | Implementación VM completa de `std.async`: ruta directa y `spawn` de `collect`, cursor genérico, límites, cancelación cooperativa, liberación terminal y loans; rendimiento y conformance global siguen en sus leaves S1A. |
| `STD-A-FUZZ-001` | **Cerrada** | Target owner-aware con 22 rutas, corpus y seeds reproducibles, límites de entrada/source/RSS/timeout, oráculos de no-panic e invariantes por owner, replay de minimizados y campañas smoke/nightly integradas; `FUZZ=verified` 22/22. |
| `STD-CHANNEL-IMPL-001` | **Cerrada** | Compiler, VM hosted y bridge nativo privado verifican endpoints nominales, bounded/unbounded, FIFO, backpressure, fork, cierres, drenado terminal, cancelación y send/receive seleccionables; fixture y sonda nativa hash-bound, sin API pública ni lowering AOT. |
| `STD-CHANNEL-ASYNC-ITER-001` | **Cerrada** | Witness privado `Receiver[T] -> AsyncIterator[T]` bajo `T: Discard`, lowering de `for` y `collect` genérico, scheduler FIFO, cleanup/cancelación y negativa affine `E1105` verificados en la VM hosted; ABI nativa y lowering AOT permanecen sin reclamar. |
| `STD-EXEC-IMPL-001` | **Cerrada** | La VM hosted verifica la superficie cooperativa de pools y actores: admisión/backpressure FIFO, `Join`, lifecycle, handlers con estado, `Actor.ref` y `ActorRef.send` `selectable` con prepare/commit/rollback transaccional; workers host, runtime nativo y lowering AOT permanecen sin reclamar. |
| `STD-ENCODING-IMPL-001` | **Cerrada** | El kernel scalar verifica Base64/hex materializado e incremental, canonicalidad estricta, límites y terminalidad; el compiler y la VM hosted verifican options, `Reader`/`Writer`, handles afines, errores y fixture `m11-std-encoding-impl-001`. Runtime nativo, SIMD y lowering AOT permanecen sin reclamar. |
| `STD-ENCODING-TEST-001` | **Cerrada** | Modelo independiente de Base64/hex, vectores RFC 4648, fronteras de chunk, errores y offsets, límites atómicos, lifecycle y fuzz `stdlib_encoding` reproducible (128 runs, seed 4105); la frontera native AOT permanece sin reclamar. |
| `STD-ENCODING-CONF-001` | **Cerrada** | Corpus compartida VM/native de seis casos sobre handles opacos: interoperabilidad Base64/hex, streaming por fragmentos, errores y offsets, límites/terminalidad, cleanup y frontera explícita `simd: not-measured-no-optimized-route`; native AOT y layout FFI permanecen sin reclamar. |
| `STD-ENCODING-DOC-001` | **Cerrada** | Guía ejecutable con seis familias de ejemplos para la única forma por policy, errores, costes, ownership y materialización/streaming; fixture, sidecars, checker y negativos verificados. Cierre documental hosted sin reclamar runtime nativo público, SIMD optimizado ni lowering AOT. |
| `NATIVE-THREAD-001` | **Cerrado** | Worker OS seguro, barrera de `Join`, cancelación, identidad lógica y smoke diferencial Cranelift/LLVM en `testing/native-thread.json`; la coordinación deferred de tasks queda cerrada por `NATIVE-002`, sin cambiar la barrera física de threads. |
| `NATIVE-002` | **Cerrado** | Coordinador MIR común para Cranelift/LLVM y smoke `deferred-task-call`: handle pendiente antes del cuerpo, completado único en `Join` y consumo por `await`; capturas mutables/closures/storage nativo completo siguen fuera de alcance. |

El gate oficial (`bash scripts/test-gate.sh`) se ejecutó después de esta
reconciliación y selló la evidencia final de workspace, conformance, reliability,
doc-tests, rustdoc y contratos de stdlib. La evidencia de calidad instrumentada
alcanza 90,71% de líneas (`9071` bp), 87,21% de funciones y 89,08% de regiones;
la baseline se recapturó sobre este árbol después de la evolución de los bloques
de `select` y conserva esos floors sin exclusiones. La compuerta oficial de
mutación ejecuta seis mutantes críticos deterministas, uno por frontera: seis
detectados, cero supervivientes, cero timeouts y cero inviables; por tanto el
score de la muestra es 100% (`10000` bp). El gate usa un staging hermano de
`CARGO_TARGET_DIR`, no copia artefactos de `target` y limita la ejecución a
un worker para evitar presión de memoria. La campaña completa de 30 mutantes
queda reservada al carril de rendimiento. `testing/conformance-ratchet.json` se
regenera contra el
único corpus vivo y sus case layers actuales.

Esta tabla es la fuente de reconciliación del estado actual.
26. [x] **Wave 5 — STD-0.1A por layers.** Los contratos, slices A0 y kernels
    iniciales están cerrados; la auditoría pública verifica 214/214 firmas y ya
    incorpora el delta `selectable` de DEC-020. Las dimensiones de evidencia y
    promoción de S1A quedaron cerradas por el bundle técnico reproducible. El
    orden de cierre fue:
    - lenguaje/select: `ASYNC-SELECT-FRONTEND-001 → ASYNC-SELECT-SEMA-001 →
      ASYNC-SELECT-LOWER-001`; después runtime y ownership cierran en paralelo,
      alimentan `STD-A-SELECTABLE-IMPL-001` —que además exige los ya cerrados
      `STD-A-ASYNC-IMPL-001` y `STD-TIME-BASE-CONF-001`— y continúa con
      `ASYNC-SELECT-TEST-001 →
      ASYNC-SELECT-PERF-001 → ASYNC-SELECT-VM-CONF-001` (tests/model,
      performance y conformidad VM hosted cerrados);
    - A1: `STD-CORE-IMPL-001`, `STD-TEXT-IMPL-001`, `STD-COLL-IMPL-001`,
      `STD-ITER-IMPL-001`, `STD-FMT-IMPL-001` y `STD-IO-IMPL-001`;
    - A2: `STD-FS-IMPL-001` y `STD-PROC-IMPL-001`, preservando path/console;
    - A3 spec: `STD-SER-001` y
      `STD-JSON-API-001 / STD-MSGPACK-API-001 / STD-PROTOBUF-API-001 →
      STD-SPEC-001`;
    - A3 ABI migration: `STD-SER-IMPL-001 → STD-DERIVE-SER-001` must first
      land the `Encode[C]`/`Decode[C]` and `Value` contracts; only then:
      `STD-JSON-IMPL-001 / STD-MSGPACK-IMPL-001 / STD-PROTOBUF-IMPL-001`;
    - A3 implementation (after ABI migration):
      `STD-JSON-IMPL-001 / STD-MSGPACK-IMPL-001 / STD-PROTOBUF-IMPL-001`,
      después `STD-JSON-PUBLIC-001 → STD-CODEC-DERIVE-POLICY-001` y las
      superficies públicas equivalentes de MessagePack/Protobuf. Sus firmas
      ya tienen auditoría pública completa; quedan sus gates de codec,
      rendimiento, fuzzing y promoción;
    - A4: `STD-TESTING-SHRINK-001 → STD-TESTING-IMPL-001`; y
    - A5: `STD-PUBLIC-API-AUDIT-001` (214/214 regenerado con DEC-020) → leaves
      `STD-A-*-EVIDENCE` →
      STD-TEST-001 / STD-CODEC-CONF-001 / STD-PERF-CONF-001 →
      STD-MATRIX-ALL-001 → STD-CONF-001 → STD-DOC-001 →
      (`STD-A-ASYNC-API-001` (cerrado) → `ASYNC-ITER-EXT-001` →
      `STD-A-ASYNC-IMPL-001`; `ASYNC-DEFER-IMPL-001` cerrado) +
      `STD-A-FUZZ-001` + `STD-A-PERF-001` + `STD-A-CONF-001` +
      `STD-A-DIST-001` → `STD-S1A-SEAL-001`; el bundle técnico queda reproducible
      y verificado, por lo que Wave 5 termina sin abrir G5 ni una publicación.
    Los owners independientes pudieron avanzar en paralelo; el seal cerró S1A
    únicamente después de que cada firma contractual atravesó una ruta pública
    real.
27. [x] **Wave 6 — Contratos que condicionan el backend.** Después de
    `ASYNC-SELECT-VM-CONF-001` y `DIAG-SPEC-001`, están cerrados
    `STD-ASYNC-GROUP-SPEC-001`, `STD-CONC-001`, `STD-SYNC-001`,
    `STD-EXEC-001`, `STD-NET-001`, `STD-CIVIL-TIME-001`, `STD-ENCODING-001`,
    `STD-YAML-001`, `STD-TOML-001`, `STD-CBOR-001`, `STD-REGEX-001`,
    `STD-ID-001` y `STD-LOG-001`; la instrumentación hosted de
    `DIAG-RUNTIME-001` ya está cerrada y la siguiente frontera contractual son
    los detectores de M11. Mini-gate: DEC-013/014 reciben requisitos completos sin
    implementar todavía STD-0.1B.
28. [x] **Wave 7 — M11 correcto antes que optimizado.** Con Wave 6 cerrada y
    `DUMP-001`, `DIAG-TEST-001` y `DIAG-NATIVE-001` cerrados, continuar
    `NATIVE-BACKEND-ADAPTER-001` (cerrado) → `NATIVE-MEM-ADR-001` →
    `NATIVE-ABI-001` → leaves `NATIVE-LOWER-*` → `NATIVE-THREAD-001` (cerrado) →
    NATIVE-SELECT-001 (cerrado) → NATIVE-002 (cerrado) → ARC-001 (cerrado) → ARC-002 (cerrado) →
    DIAG-NATIVE-001 (cerrado) → NATIVE-STD-CORE/HOSTED → NATIVE-STD-001 →
    NATIVE-LINK-001 → NATIVE-CLI-001 → leaves NATIVE-CONF-* → NATIVE-CONF-001 /
    NATIVE-DIFF-001 → targets → NATIVE-REL-001 →
    `NATIVE-AOT-SCOPE-001` → `NATIVE-AOT-LOWER-001` →
    (`NATIVE-AOT-BINARY-001` + `NATIVE-AOT-MEM-001` + `NATIVE-AOT-QUALITY-001`) →
    `NATIVE-AOT-PERF-001` (cerrado) → `DEC-013` → Gate N1 (cerrado por
    `scripts/native-n1.sh`; Cranelift promovido para x86_64 GNU y ARM64
    conservado como smoke de candidato).
29. [ ] **Wave 8 — Completar STD-0.1B y candidato 0.1.** Terminar specs B,
    cerrar para cada owner las leaves `IMPL`, `HOST` aplicable, `TEST/FUZZ`,
    `PERF`, `CONF` y `DOC` de 21.3.1–21.3.13, y después los coordinadores
    `STD-B-OWNER-MATRIX-001`, `STD-B-*` y `STD-S1-SEAL-001`; cerrar Gate S1. Solo después componer
    `REL-0.1-RC-001` con G5/T0/N1/S1 y después `REL-SUPPLY-001` /
    `REL-INSTALL-001` / `REL-PUBLISH-001`. Optimizaciones post-N1 avanzan por evidencia
    y no bloquean el candidato salvo que un presupuesto publicado lo exija.

Lane transversal TLF, independiente del orden de Waves 5–8:

- [x] `TLF-RESEARCH-001 → TLF-SPEC-001`.
- [ ] `TLF-BENCH-REPRO-001` puede avanzar desde research; en paralelo,
  `TLF-CODEC-001 → (TLF-CANON-001 + TLF-MAP-001 + TLF-DIAG-001) →
  TLF-CLI-001 → (TLF-PROP-001 + TLF-FUZZ-001 + TLF-EVAL-001) →
  TLF-CONF-001 → TLF-BUNDLE-001`.

Puede avanzar mientras se cierran stdlib/conformidad porque solo consume el
frontend estable. Gate L0 no es prerrequisito de S1A, G5 o N1, pero sí de
`TLF-REL-001`; el candidato base no depende de TLF.

Resumen topológico:

~~~text
CONF-DRAFT
  -> FORMAT draft
  -> {PARSER-STACK -> meta/test syntax
      meta prerequisites + meta frontend
      bytes + env + time + test plan/frontend + defer inference}
  -> {meta runtime
      test runtime + algorithms}
  -> {META-CONF + testing implementation}
  -> {doc-test + matrix all specs + gap audit + final seal -> T0/G5
      STD-0.1A leaf implementations + public API audit -> S1A}
  -> STD-0.1B runtime contracts
  -> native build/link/CLI correctness / N1
  -> STD-0.1B leaves -> S1 -> REL-0.1-RC -> supply/install/publish

G0 -> TLF spec + reproducible benchmark -> codec/maps/CLI
   -> properties/fuzz/eval -> conformance -> L0 bundle -> TLF companion
~~~

M4, M5, M6, la base suspendible de M7, M8, M9, el corpus vivo M10, M10.5,
M10.5b y Gates G4/H0 quedan cerrados como implementación/infraestructura. La
extensión núcleo `select` de M7 está implementada en frontend, semántica,
lowering, VM, ownership, modelo y adapters; su presupuesto y conformidad VM
hosted están ratcheteados. M10.7 y la implementación
funcional de M10.6 permanecen cerradas. `CONF-DRAFT-001` también permanece
cerrada. La auditoría mantiene T0 verificable sobre el árbol actual y G5 abierto
hasta el primer candidato real. Wave 5/S1A queda cerrada como draft técnico
por `STD-S1A-SEAL-001`;
la superficie ejecutable está verificada en 214/214 firmas y FUZZ está promovido
22/22; la auditoría ya incluye los efectos `selectable`, la conformidad hosted
del selector está ratcheteada y la slice de selección runtime nativa está
cerrada por `NATIVE-SELECT-001`; `DEC-013` seleccionó Cranelift para el target
admitido y las dimensiones N1
siguen abiertas; las capacidades de identidad/source maps
quedan demostradas por `NATIVE-LOWER-DEBUG-001`.
`CONF-GAP-IMPL-001` y `CONF-LAYER-RESULT-001` producen la trazabilidad y el
resultado compuesto vivos. `CONF-SEAL-FINAL-001` permanece pendiente para el
primer release. `STD-IMPL-001`, `STD-IMPL-002` y `STD-CODEC-PUBLIC-001` están cerrados;
Wave 6 continúa con los contratos runtime-facing B0 después del seal S1A y del
contrato D0 de diagnóstico; `STD-ASYNC-GROUP-SPEC-001`, `STD-CONC-001`,
`STD-SYNC-001`, `STD-EXEC-001`, `STD-NET-001`, `STD-CIVIL-TIME-001`,
`STD-ENCODING-001`, `STD-YAML-001`, `STD-TOML-001`, `STD-CBOR-001`,
`STD-REGEX-001`, `STD-ID-001` y `STD-LOG-001` ya tienen registros y negativos
ejecutables; `STD-ASYNC-GROUP-IMPL-001`, `STD-ASYNC-GROUP-TEST-001` y
`STD-ASYNC-GROUP-PERF-001`, `STD-ASYNC-GROUP-CONF-001` y
`STD-ASYNC-GROUP-DOC-001` ya están cerrados para VM hosted y el ABI del
runtime nativo; quedan pendientes el lowering AOT async portable y los
detectores de M11.
`STD-IMPL-001` y `STD-IMPL-002` quedan ahora cerrados por sus gates de
coordinación; `NATIVE-TARGET-DESC-001`, `NATIVE-ARTIFACT-001`,
`NATIVE-LINK-PLAN-001`, `NATIVE-PUBLISH-SPEC-001` y `PERF-001` quedan cerrados
como contratos puros. `NATIVE-AOT-PERF-001` queda cerrado con evidencia
repetida y path-free; Gate N1 queda cerrado por su informe compositivo y
promueve Cranelift únicamente para el target primario x86_64 GNU. El frontend de
colecciones compartidas ya está cerrado por `STD-SYNC-COLLECTION-FRONTEND-001`;
la ejecución hosted y el ABI nativo privado quedan cerrados por
`STD-SYNC-COLLECTION-IMPL-001`; `STD-SYNC-COLLECTION-TEST-001` también está
cerrado para el modelo/test/fuzz acotado y `STD-SYNC-COLLECTION-PERF-001`
queda cerrado para la línea base de rendimiento hosted; la conformance
observable de colecciones, la conformance global y la guía ejecutable de
`std.sync` quedan cerradas por `STD-SYNC-COLLECTION-CONF-001`,
`STD-SYNC-CONF-001` y `STD-SYNC-DOC-001`; el siguiente bloque crítico es
`STD-CHANNEL-IMPL-001`;
`STD-SYNC-HOST-001`,
`STD-SYNC-TEST-001` y `STD-SYNC-PERF-001` ya cerraron la frontera de
parking/atomics, la continuación de `Once`, el modelo hosted determinista y el
presupuesto de rendimiento target-qualified. ARM64 conserva una clasificación
de smoke de candidato hasta completar su corpus AOT y la ruta async nativa.
`TRACKER-LINT-001` está cerrado y su informe deriva los
conteos directamente del tracker. `STD-A-ASYNC-API-001` ya
cerró su contrato y auditoría, `ASYNC-DEFER-IMPL-001` cerró su hardening y
`ASYNC-ITER-EXT-001` cerró el lowering genérico de `collect(limit:)` con
evidencia runtime, `STD-A-ASYNC-IMPL-001` cerró su ejecución estructurada y
`STD-A-FUZZ-001` cerró las 22 rutas owner-aware de fuzz y
`STD-A-PERF-001` promovió los baselines portables y fronteras target-qualified.
`STD-A-CONF-001` promovió la ejecución pública (22 owners, 385 filas y 206
casos del draft) con evidencia hash-bound. `STD-A-DIST-001` promovió el
paquete VM reproducible (dos snapshots, instalación, ejecución y
desinstalación). `STD-S1A-SEAL-001` cerró el bundle técnico del draft y
`DIAG-SPEC-001` cerró el contrato D0 y `DIAG-RUNTIME-001` cerró la
instrumentación VM hosted; `STD-ASYNC-GROUP-SPEC-001`, `STD-CONC-001`,
`STD-SYNC-001`, `STD-EXEC-001`, `STD-NET-001`, `STD-CIVIL-TIME-001`,
`STD-ENCODING-001`, `STD-YAML-001`, `STD-TOML-001`, `STD-CBOR-001`,
`STD-REGEX-001`, `STD-ID-001` y `STD-LOG-001` cerraron trece fronteras B0;
`RACE-001`, `LEAK-001`, el writer lógico de `DUMP-001` y la integración de
`DIAG-TEST-001` y `DIAG-CI-001` ya están cerrados en hosted. `NATIVE-001`
mantiene la evidencia reproducible y registra la selección de Cranelift, con
mediciones rápidas y diferenciales reales de Cranelift/LLVM; la campaña AOT completa de producto,
memoria, calidad y rendimiento también está cerrada; el adaptador común, su metadata de
identidad, la lane física de `NATIVE-THREAD-001` y la coordinación mínima de
`NATIVE-002` están cerrados y `ARC-001`, `ARC-002` y `DIAG-NATIVE-001` ya están
implementados. La frontera AOT está cerrada hasta `NATIVE-AOT-PERF-001`;
la frontera AOT y Gate N1 están cerrados para el target primario; el backend
seleccionado queda promovido únicamente para x86_64 GNU. El frontend de
colecciones compartidas está cerrado y la ejecución hosted/ABI nativo privado
de `STD-SYNC-COLLECTION-IMPL-001` ya tiene evidencia ejecutable;
`STD-SYNC-COLLECTION-TEST-001` también está cerrado para el modelo/test/fuzz
acotado, `STD-SYNC-COLLECTION-PERF-001` queda cerrado para la línea base
hosted y `STD-SYNC-COLLECTION-CONF-001` queda cerrado para la equivalencia
observable VM/native; `STD-SYNC-CONF-001` y `STD-SYNC-DOC-001` también quedan
cerrados para el corpus común VM/native-bridge y la guía ejecutable; la adaptación
hosted `Receiver[T] -> AsyncIterator[T]` de `STD-CHANNEL-ASYNC-ITER-001` también
está cerrada; `STD-CHANNEL-TEST-001` queda cerrado para modelo, regresiones y
fuzz acotado; `STD-CHANNEL-PERF-001` queda cerrado para la línea base hosted y
`STD-CHANNEL-DOC-001` queda cerrado para la guía ejecutable de composición.
`STD-EXEC-IMPL-001` queda cerrado para la implementación cooperativa hosted de
su superficie y `STD-EXEC-HOST-001` queda cerrado para el bridge hosted y la
lane nativa target-qualified; `STD-EXEC-TEST-001` queda cerrado con el modelo,
replay, stress y fuzz acotados. `STD-EXEC-PERF-001` queda cerrado por la
campaña target-qualified hosted/native 3 x 9 y su presupuesto reproducible;
`STD-EXEC-CONF-001` queda cerrado por el corpus común VM/native y la capability
estática `threads`; `STD-EXEC-DOC-001` queda cerrado por su guía ejecutable,
fixture y negativos; el siguiente trabajo crítico es `DIAG-RUNTIME-001`. La
implementación de
superficie, el parking hosted, el puente nativo escalar, el modelo/test/fuzz,
el presupuesto de rendimiento y la conformance del ABI del runtime nativo de
Group ya tienen fronteras explícitas, mientras la iteración directa, el
lowering AOT async y la promoción pública siguen pendientes.

---
