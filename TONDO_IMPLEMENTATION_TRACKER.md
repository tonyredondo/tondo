# Tondo: tracker de implementación

**Estado:** M10.5b, Gate H0, `META-FORMAT-001`, `PARSER-STACK-001`, los slices
tempranos `std.bytes`, el time-base de `std.time`, el contrato/implementación
read-only de `std.env`, `ASYNC-DEFER-IMPL-001`, `UTEST-PLAN-001`,
`UTEST-INPUTS-PLAN-001`, `UTEST-RESULT-MODEL-001`, `UTEST-CLI-PARSE-001` y
`UTEST-DISC-001`, `UTEST-OWNERS-001`, `UTEST-DEPS-001`, `UTEST-LEX-001`,
`UTEST-CST-001`, `UTEST-FMT-001`, `UTEST-ID-001`, `UTEST-CAPTURE-001` y
`UTEST-OVERLAY-001`, `UTEST-INTEG-001`, `UTEST-CHECK-001`, `UTEST-LOWER-001` y
`UTEST-CONTROL-001`, `UTEST-RUNTIME-001` y `UTEST-SUITE-001` están
cerrados sobre el
draft actual; Tondo 0.1 sigue en desarrollo y las superficies consolidadas de
metaprogramación, testing y Standard Library deben implementarse y añadirse a
la conformidad del mismo draft antes de publicar la primera versión

**Versión del tracker:** 1.29

**Última actualización:** 2026-07-31

**Especificaciones normativas:**

- [Borrador normativo de Tondo 0.1](./TONDO_LANGUAGE_SPEC.md)
- [Arquitectura base de Standard Library 0.1](./TONDO_STANDARD_LIBRARY_SPEC.md)
- [Contrato de testing para Tondo 0.1](./TONDO_TESTING_SPEC.md)

**Objetivo inmediato:** ejecutar las lanes restantes de Wave 2 después de cerrar
`PARSER-STACK-001`, el prerrequisito portable de sintaxis. `META-FORMAT-001` ya consolidó los formatos del toolchain
con el marcador único `draft`; no existe una lane `/1` frente a otra `/2`.
A partir de aquí avanzan dos lanes:
M10.7 sobre los slices tempranos de `std.meta`/`std.reflect`, y M10.6 sobre
`defer await` y el runner de testing; el slice read-only de `std.env` ya está
implementado;
los slices de `std.bytes` y el contrato de `std.time` ya están cerrados. Gate
G5 solo vuelve a cerrarse cuando ambas lanes forman parte de la conformidad
vigente. Después se completa STD-0.1A, se fijan antes del backend los contratos
runtime-facing de STD-0.1B, comienza M11 y, tras Gate N1, se implementa el resto
de STD-0.1B y se cierra S1. Todo ello pertenece a la primera Standard Library
0.1; los slices y fases son orden de implementación, no versiones públicas.
La VM permanece como implementación de referencia y oracle diferencial del
backend nativo.

> Este documento no define semántica del lenguaje. La especificación es la única
> fuente normativa. El tracker organiza el trabajo de implementación, registra
> decisiones técnicas y permite distinguir entre una característica
> implementada, una característica validada y una implementación conforme.

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
| `ADR-010` | Executor cooperativo de un solo hilo como primer runtime async | El lenguaje no exige una task por thread; permite validar concurrencia estructurada antes del paralelismo |
| `ADR-011` | Copias lógicas correctas antes que copy-on-write | Una copia eager es conforme; COW es una optimización no observable que debe añadirse después |
| `ADR-012` | Pipeline de compilación determinista, inicialmente no incremental | Incrementalidad no debe contaminar la semántica ni retrasar el primer compilador |
| `ADR-013` | Monomorfización como primera estrategia para genéricos y dispatch estático | Encaja con los traits sin vtables y mantiene el bytecode tipado |
| `ADR-014` | Sin formato serializado estable de bytecode durante bootstrap | El bytecode puede ser in-memory hasta que la semántica y el loader estén estabilizados |
| `ADR-015` | Un subconjunto bootstrap es una limitación del toolchain, no una edición ni dialecto de fuente | Las construcciones no implementadas se rechazan; nunca reciben semántica provisional |
| `ADR-016` | Metaprogramación estática mediante `derive` y una ronda hermética de generators Tondo | Elimina boilerplate sin macros textuales, reflection dinámica, plugins nativos ni ejecución ambiental dentro del frontend |

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
  capabilities, actualización explícita y coexistencia verificable con el
  corpus bootstrap de regresión interno; no es una versión ni un release.

- [ ] **DEC-013 — Backend nativo y ABI runtime interna.** `NATIVE-001` elige
  backend y registra targets, debug info, toolchain y portabilidad;
  `NATIVE-ABI-001`, después del ADR de memoria, cierra calling convention,
  unwind y fronteras runtime. Ninguna de ambas promete ABI FFI pública.

- [ ] **DEC-014 — Gestión de memoria nativa.** `NATIVE-MEM-ADR-001` debe
  cerrarla antes de ABI y lowering nativos, fijando ownership runtime,
  atomicidad, weak refs, detección de ciclos, interacción con COW, async,
  threads, FFI privilegiada y estrategia de verificación.

- [x] **DEC-015 — Testing first-class integrado en 0.1.** La especificación
  [`TONDO_TESTING_SPEC.md`](./TONDO_TESTING_SPEC.md) forma parte del mismo
  contrato Tondo 0.1 y reserva `suite` y `test`: `suite` es un contenedor
  estático con setup léxico y teardown por `defer`/`defer await`; `test` es
  siempre una hoja. El corpus bootstrap permanece como evidencia de regresión,
  no como edición, release ni dialecto seleccionable. El contrato separa unit overlays de
  integration roots y fija árbol/identidad, capturas `Copy + Send + Share`,
  envelope estructurado,
  `std.testing.log/tags/failNow/skip/attach/snapshot/withVirtualTime`, inferencia
  de error/async, aislamiento, selección substring/glob/exact, ownership por
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
| **M7 — Async y concurrencia estructurada** | Tasks conformes | Completado |
| **M8 — Scripts y procesos** | Experiencia de scripting | Completado |
| **M9 — Unsafe, targets y toolchain** | Gate G4: preview 0.1 | Completado |
| **M10 — Bootstrap regression corpus** | Baseline ejecutable pre-`derive` | Completado |
| **M10.5 — Reliability y testing** | Infraestructura y hardening continuo de evidencia | Completado |
| **M10.5c — Conformidad del draft** | Una línea de draft y ratchet incremental | Wave 0 cerrado; sellado pendiente en Wave 4 |
| **M10.7 — Metaprogramación estática** | `derive`, generators, meta VM y contribución a G5 | Especificado; implementación pendiente |
| **M10.6 — Testing de usuario Tondo 0.1** | Gate T0 y contribución testing a G5 | En curso; lexer de keywords cerrado |
| **STD-0.1A — Foundation + Hosted** | Base estándar necesaria para meta, testing y backend | Arquitectura base cerrada; slices tempranos y APIs pendientes |
| **M11 — Backend nativo y optimización** | Implementación de producción | Futuro |
| **STD-0.1B — Concurrency + Application** | Contratos runtime antes de M11; implementación tras N1 | Arquitectura base cerrada; contratos y código pendientes |

Estado observado del workspace:

- Repositorio local: `/mnt/media/Tony/Projects/tondo`, branch `main`, con
  upstream en
  `github.com/tonyredondo/tondo`.
- Workspace: `tondo-cli`, `tondo-compiler`, `tondo-conformance`,
  `tondo-reference-adapter`, `tondo-reliability` y `tondo-vm`.
- Toolchain utilizado para la validación: Rust 1.93.0 y Cargo 1.93.0; la versión
  mínima soportada queda fijada en Rust 1.93.
- Última validación del corpus bootstrap implementado, anterior a la ampliación actual
  del draft: 2026-07-29, con formatter check, `cargo check` de todos los
  targets, Clippy con warnings denegados, 848 tests Rust inventariados, Rustdoc
  con warnings denegados y metadatos locked. La suite oficial pasa 205 casos y
  424 repeticiones byte-estables; el inventario completo registra 1.533 casos
  lógicos y 1.752 repeticiones.
- La línea única de draft registra 1.533 tests lógicos y 1.752 repeticiones:
  1.483 ejecutables, 38 contratos `draft-pending`, tres campañas y nueve fences
  no ejecutables. Su matriz conserva 17 requisitos cubiertos y expone
  explícitamente 27 requisitos nuevos o modificados como `draft-pending`.

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
          M10.7/meta                  M10.6/defer-await + testing
             |                                    |
             v                                    v
         META-CONF                                T0
             +-----------------+------------------+
                               v
                       CONF-SEAL -> Gate G5
                               |
                               v
                  resto STD-0.1A -> Gate S1A
                               |
               contratos runtime STD-0.1B
                               |
                               v
                         M11 -> Gate N1
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
antes de ampliar sintaxis. `CONF-DRAFT-001` mantiene el corpus bootstrap como
regresión explícita y el draft como única línea activa; ningún slice nuevo trabaja con el
  gate permanentemente roto ni atribuye esa regresión a reglas nuevas sin evidencia.

`META-FORMAT-001` es el primer cambio de código porque materializa los formatos
`draft` compartidos. Después no existe una dependencia serial entre M10.7 completo
y M10.6 completo. La lane meta requiere la API build-only exacta de `std.meta`
y el contrato de `std.reflect`; la lane testing requiere la identidad estable de
`std.bytes`, el snapshot read-only de `std.env` y el time-base de producción.
Plan/discovery, sintaxis de testing y `defer await` pueden avanzar antes de
terminar esos slices; solo el typecheck que consume sus APIs, materialización de
inputs, virtual time, lifecycle completo y Gate T0 los esperan.

M10.7 y M10.6 ratchetean evidencia al terminar cada wave, no únicamente en
`META-CONF-001` o `UTEST-CONF-001`. T0 cierra testing; G5 espera además toda la
lane meta, ejecuta `CONF-SEAL-001` y vuelve a verificar el corpus consolidado.
El resultado proporciona `tondo test` para completar y probar la propia stdlib.

Cada API posterior de STD-0.1A se implementa como slice vertical y amplía
matriz, conformidad y dogfooding. Antes de `NATIVE-001` deben estar cerrados los
contratos —no necesariamente las implementaciones— de `std.channel`,
`std.sync`, `std.executor` y la frontera host de `std.net`, porque condicionan
memoria, atomics, wakeups, bloqueo y ABI runtime. M11 depende de T0, G5, S1A y
esos contratos. La implementación de STD-0.1B continúa tras N1 y sigue siendo
requisito de la primera publicación STD 0.1.0.

Los números M10.6 y M10.7 son IDs históricos estables, no prioridad
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
| `CONF-SEAL-001` | `META-CONF-001`, `UTEST-CONF-001`, Gate T0 y hashes actuales | STD-0.1A completa |
| Gate G5 vivo | `CONF-SEAL-001` | STD-0.1A completa |
| `NATIVE-001` | Gates G5/S1A y contratos runtime-facing de STD-0.1B | Implementación de STD-0.1B |
| `NATIVE-ABI-001` | `NATIVE-001`, `NATIVE-MEM-ADR-001` y contratos de sync/executor | ABI FFI pública |
| ARC/runtime nativo | `NATIVE-ABI-001` y DEC-014 | Eliminación de retains, COW o escape analysis |
| Gate S1 | N1 y todos los slices A/B conformes | Incrementalidad o LSP |

### 4.1.2 Regla de integración por waves

Cada wave termina con un mini-gate que actualiza inventario, trazabilidad,
tests, cobertura aplicable y conformidad viva. Una wave posterior no utiliza
una API provisional de la anterior. Trabajo de lanes distintas puede ejecutarse
en paralelo; dos cambios que toquen el mismo schema, parser, IR o runtime se
integran en el orden de la tabla anterior.

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
| 21. Formato y documentación | M1 y trabajo transversal | G0 y M10 |
| 22. Diagnósticos y tooling | M0, M1, M2, M9 y M10 | Todos los gates |
| 23. Gramática de referencia | M1 | G0 y M10 |
| 24. Ejemplos integrados | Tests de aceptación progresivos | G2, G3, G4 y M10 |
| 25. Características ausentes | Compile-fail distribuido por milestone | M10 |
| 26. Frontera con la stdlib | M6, M8, STD-0.1A y STD-0.1B | G3, G4, M10, S1A y S1 |
| 27. Metaprogramación estática | M10.7; providers en STD-0.1A | G5 y S1A |
| 28. Testing integrado Tondo 0.1 | Time-base de STD-0.1A + M10.6; helpers en STD-0.1A | T0, G5 y S1A |

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

- [x] **CALL-004 — Implementar closures sync, async y unsafe en la
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
  trait y un receptor async registra la obligación intrínseca `Self: Send`.
- Los defaults se comprueban una sola vez con parámetros rígidos y pueden
  llamar métodos del mismo trait sin lookup global. Las especializaciones de
  método inferidas o explícitas conservan el prefijo del trait y `Self`; los
  corchetes de un index siguen recorriendo su ruta ordinaria.
- El verifier exige correspondencia exacta entre resolución y tabla HIR,
  clasificación de receptor, aridad completa, prefijo genérico, presencia de
  body y requisito async. Los defaults mantienen `Self` genérico y sólo se
  convierten en roots de bytecode cuando un dispatch concreto los selecciona.
- Cada `impl` publica una identidad estable, su cabecera normalizada, binders,
  métodos y contratos instanciados. La coincidencia exige nombre, receptor,
  modos, variadicidad, genéricos, bounds, `async`, `unsafe`, éxito y error
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
  inherentes, asociadas y async, bounds fuente y prelude, y mutaciones en cada
  frontera.
- Un único motor calcula `Copy`, `Discard`, `Equatable`, `Key`, `Send` y
  `Share` mediante resúmenes nominales simbólicos y un punto fijo coinductivo.
  `Copy` implica `Discard`; `Key` implica `Copy`, `Equatable` y `Discard`.
- La tabla completa queda alineada con el interner HIR. Los bounds opacos sólo
  publican lo declarado, los binders genéricos sólo usan constraints visibles y
  un trait con receptor async aporta y exige `Self: Send`.
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
  satisfechos o diferencias de modo, variádico, `async`, `unsafe` y error se
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
  canónicamente `closure`, `unsafe-closure`, `async-closure` o
  `async-unsafe-closure`, y conserva los mismos bits en su firma estructural,
  junto con binders heredados, parámetros completos, body HIR separado y
  capturas sintácticas ordenadas por `LocalId`.
- El outcome se infiere sobre todos los caminos alcanzables y las closures
  anidadas conservan problemas de inferencia independientes. Un tipo de función
  esperado debe coincidir también en `async` y `unsafe`, o produce únicamente
  `E1102`; no existe conversión que añada u oculte un efecto.
- Las capturas conservan `let`/`var`, copian un snapshot owned cuando prueban
  `Copy` y, en caso contrario, mueven el binding exterior al construir el
  entorno. Los free uses de closures anidadas se propagan. Préstamos
  `ref`/`mut`/`var` y el receiver prestado producen `E1402`; parámetros
  variádicos exigen nombre y conservan elemento en la firma y `Array[T]` dentro
  del body. Las firmas async rechazan parámetros `mut`/`var` con el diagnóstico
  normativo `E1609`.
- `Copy`, `Discard`, `Send` y `Share` se derivan componente a componente desde
  las capturas sustituidas; `Equatable` y `Key` se rechazan. OWN-006 elimina la
  restricción ejecutable de capturas `Copy`; OWN-007 deriva `CallOnce` mediante
  descarte estructural o transferencia completa en todas las salidas normales.
- El admission verifier exige correspondencia uno-a-uno entre metadata y
  expresión, identidad generada versus efectos de firma, firma/body, ausencia
  de parámetros async exclusivos, tipo, mutabilidad y binding de cada captura.
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
  el código inalcanzable no contamina la fila exterior. En una closure async,
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
  verifiers rechazan una firma async o unsafe en la operación de llamada
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
  body async o unsafe como root, de modo que CALL-004 no activa prematuramente
  el runtime de M7 ni evita la frontera de M9.
- Las regresiones públicas y unitarias ejecutan closures puras, mutables,
  `CallOnce`, borradas a `fn`, genéricas, opacas, variádicas, anidadas,
  proyectadas, fallibles y bajo presión de GC; también mutan HIR, MIR y bytecode
  para probar cada frontera defensiva. Fixtures adicionales cubren las cuatro
  identidades, mismatch de efectos, `E1609`, protocolo async stateful y rechazo
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
- Los cuatro contratos sync/unsafe/async están representados sin conversión de
  efectos; sólo la firma síncrona segura puede usar la operación de llamada M4.
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
  en el corpus bootstrap todavía no existían guards, cleanup ni fallback
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
| 24.11 | API application clasificada | `std.process.args`, parseo, carga y ejecución son contratos hosted/application aún separados |

---

## 12. M7 — Async y concurrencia estructurada

**Objetivo:** implementar suspensión y concurrencia sin futures implícitos,
tasks detached ni wrappers visibles en las firmas.

- [x] **ASYNC-001 — Typecheckear funciones y closures async.** HIR conserva el
  efecto en la identidad y la firma exacta, comprueba cuerpos nombrados y
  cierres concretos, deriva su protocolo de llamada y rechaza parámetros
  exclusivos con `E1609`.

- [x] **ASYNC-002 — Exigir `await` o `spawn` al invocar trabajo async.** Una
  llamada async se materializa como operación HIR no ejecutable por sí sola y
  solo puede quedar inmediatamente bajo uno de esos dos iniciadores; el resto
  produce `E1601`.

- [x] **ASYNC-003 — Prohibir préstamos y parámetros incompatibles a través de
  suspensión.** El análisis de liveness comprueba `Send` para cada owner vivo,
  reutiliza la frontera de loans de BORROW-006 y emite `E1607` si un préstamo
  exclusivo alcanza `await`; los préstamos estructurados de `spawn` permanecen
  activos hasta consumir su `Join`.

- [x] **ASYNC-004 — Transformar MIR async en frames suspendibles.** MIR y
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
  patterns y contenedores, impide que escape o llegue vivo al final con
  `E1603`, y libera los préstamos de `spawn` solo cuando desaparece el último
  owner del handle. La VM impide consumo doble o desde otro scope.

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

- [x] **MAIN-ASYNC-001 — Implementar `async fn main` y scope raíz.** El driver
  admite una entrada async segura con el mismo outcome lógico que `main`
  síncrono; la task raíz pertenece al executor, pero no crea un scope léxico
  implícito para autorizar `spawn` detached.

- [x] **CONC-TEST-001 — Crear litmus tests con resultados permitidos y
  prohibidos, no con scheduling esperado.** El corpus cubre ejecución
  secuencial y concurrente por propiedades finales: progreso, wake idempotente,
  no escape de `Join`, préstamos liberados tras `await`, cancelación con
  cleanup, pánicos de hermanos y roots vivos bajo GC, sin fijar una traza
  concreta del scheduler.

### Gate de salida de M7

- [x] Ningún hijo sobrevive a su scope.
- [x] Todo `Join` se consume o recibe cleanup estructurado.
- [x] Cancelación no aparece como variante implícita de `E`.
- [x] El executor de un hilo satisface progreso cooperativo.
- [x] El código no depende del orden concreto de scheduling.
- [x] Los roots de frames suspendidos permanecen vivos.

---

## 13. M8 — Scripts, comandos y procesos

**Objetivo:** hacer de Tondo un lenguaje cómodo para scripting sin introducir
shell implícito ni efectos de importación.

### 13.1 Script raíz

- [x] **SCRIPT-001 — Implementar sentencias top-level solo en el archivo raíz
  del modo script.**

- [x] **SCRIPT-002 — Construir un `main` privado implícito.**

- [x] **SCRIPT-003 — Inferir localmente la unión cerrada de errores del script.**

- [x] **SCRIPT-004 — Convertir el `main` implícito en async cuando aparezca
  `await` o `scope` top-level.**

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
- La CLI del corpus bootstrap carga exactamente el plan cerrado, todavía no ejecuta
  generadores ni busca
  dependencias, emite productos solo tras éxito y evita que estos sobrescriban
  inputs o se solapen entre sí, incluidos aliases de path.
- La frontera nativa 0.1 termina deliberadamente en unidades privilegiadas
  fijadas por hash. No se inventan layout, calling convention ni ABI general;
  un adaptador dinámico futuro deberá aportar y fijar ese contrato.

---

## 15. M10 — Corpus bootstrap de regresión

**Objetivo:** convertir la afirmación “implementamos Tondo” en evidencia
versionada y reproducible.

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

### 15.3 Corpus bootstrap reproducible

- [x] **REL-001 — Registrar matriz exacta de target, perfil y capacidades.**

- [x] **REL-002 — Fijar la identidad del compilador, formatter, edición y
  manifest del draft.**

- [x] **REL-003 — Registrar resultados reproducibles de conformidad.**

- [x] **REL-004 — Documentar limitaciones que no contradigan capacidades
  anunciadas.**

- [x] **REL-005 — Verificar que no existe modo oculto que relaje checks.**

- [x] **REL-006 — Congelar el formato público de diagnostics JSON 0.1.**

- [x] **REL-007 — Registrar el corpus bootstrap únicamente después de superar
  todos los grupos aplicables de su manifest histórico.**

### Gate M10 del corpus bootstrap pre-M10.7

- [x] La identidad exacta del toolchain pasa `tondo-conformance-draft`.
- [x] El target y sus capacidades están declarados.
- [x] No hay exclusiones sin justificar por capacidad.

Este cierre es evidencia histórica del corpus bootstrap. La ampliación M10.7
mantiene abierto el Gate G5 de la primera versión publicable, definido en 18.4.
- [x] Los artefactos, resultados y versiones pueden reproducirse.
- [x] La documentación no afirma soporte más amplio que la evidencia.

Evidencia de cierre:

~~~text
suite            = tondo-conformance-draft draft
manifest_sha256  = 6bb8fe5b151ef73f1d49b3d432a51ec18c7a634cf4c9d014eea81d6a351c6ffb
result_sha256    = f07d818482f6d709c4281c2117d432db17019420e6421d81c9ba10c14f48d089
cases            = 205
repetitions      = 424
workspace_tests  = 1533 logical
target           = tondo-vm-hosted
profile          = hosted
capabilities     = [console, process]
~~~

El resultado estructurado del draft se conserva en
`conformance/0.1/results/tondo-reference-draft-tondo-vm-hosted.json`.

---

## 16. M10.5 — Reliability y testing

**Objetivo:** instalar una infraestructura de evidencia continua antes de
ampliar la API pública o duplicar la ejecución en un backend nativo. Este
milestone no cambió la semántica del corpus bootstrap ni reabrió entonces
Gate G5: clasifica la
cobertura actual, automatiza el gate existente y crea las herramientas con las
que cada milestone posterior multiplicará casos reproducibles.

**Límite:** M10.5 no se cierra por alcanzar una cifra arbitraria de tests. Se
cierra cuando inventario, trazabilidad, CI, generación, fuzzing, modelos y
métricas tienen contratos ejecutables. La expansión del corpus continúa dentro
de M10.6, STD-0.1A, M11 y STD-0.1B.

### 16.1 Baseline y trazabilidad normativa

- [x] **TEST-AUDIT-001 — Auditar el corpus 0.1 existente.** La baseline
  observada contiene 685 tests Rust ejecutables y ninguno ignorado, 129
  fixtures internos `.to`, 205 casos y 424 repeticiones de conformidad, 302
  fences Tondo del snapshot de spec 0.1 y 203 fuentes `.to` únicas al descontar
  los 127
  duplicados exactos entre fixtures internos y conformidad. El inventario
  distingue cantidad física, caso lógico, repetición y fuente única.

- [x] **TEST-001 — Materializar un inventario machine-readable.** Añadir una
  herramienta reproducible que enumere por crate, fase, fixture, grupo,
  requisito, oracle, repetición, hash de fuente y target. Debe detectar IDs
  duplicados, sidecars huérfanos, casos no descubiertos y deriva entre el
  manifiesto y el repositorio. También registra documento, edición y estado:
  los ejemplos de `TONDO_TESTING_SPEC.md` se registran como contrato 0.1
  pendiente, pero no cuentan como tests ejecutables ni cobertura del
  corpus bootstrap.

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
  préstamos, control, async y errores sin depender de que el frontend acepte
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
  observación completa alcanza 119.622/132.793 líneas (90,08 %),
  7.866/9.102 funciones (86,42 %) y 169.052/191.782 regiones (88,15 %).
  El gate conserva floors truncados de 9.008, 8.642 y 8.814 basis points y
  floors independientes para parser, checkers, verifiers, heap, ejecución y
  protocolos no confiables. Branch y MC/DC no se interpretan como 0 %: Rust
  1.93.0 con LLVM 21.1.8 publica ambos contadores con cero unidades
  instrumentadas, por lo que permanecen explícitamente no medidos hasta que el
  toolchain produzca una señal estable. Cualquier descenso medido falla aunque
  el porcentaje global permanezca por encima del valor anterior.

- [x] **MUT-002 — Revalidar la resistencia tras el hardening.** La selección
  revisada conserva exactamente 28 mutantes: 27 ejecutables detectados, uno
  inviable, cero timeouts y cero supervivientes. Ninguno de los cuatro archivos
  de producción seleccionados cambió durante el hardening; el reporte fijado se
  verifica contra la nueva baseline antes de publicarla.

### Gate H0 — Infraestructura de fiabilidad

- [x] El gate completo de Tondo 0.1 se ejecuta automáticamente en PR y `main`.
- [x] El inventario y la matriz normativa se validan sin entradas sin
  clasificar para el target del corpus bootstrap; el contrato de testing todavía
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

H0 permanece cerrado para el corpus bootstrap que lo demostró, pero cada
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
  casos válidos e inválidos de profundidad 1.000–4.000 en workers de 64 KiB,
  equivalencia de partición/reconstrucción y token shape tras formatter, además
  de los 2.048 inputs arbitrarios y fuzz targets existentes. La evidencia
  observada en Linux x86_64 pasa; la matriz Linux ARM64/macOS/Windows queda
  como ejecución CI de targets, no como una afirmación local no verificada.

- [x] **CONF-DRAFT-001 — Consolidar una única conformidad de draft.** Mantener
  `conformance/draft/manifest.json` como única identidad activa, usar el corpus
  bootstrap solo como regresión explícita y clasificar los requisitos nuevos o
  modificados como pendientes hasta que sus propios layers tengan evidencia.
  El runner, reliability, matriz, scripts y CI ya no ofrecen selección
  identidades históricas ni fallback entre manifests. El manifest de draft fija los
  hashes de los cuatro specs, el estado abierto, las tareas pendientes y los
  layers futuros; el preflight de sellado sigue siendo no mutante. Añadir tests
  que demuestren selección única, rechazo de nombres de linaje antiguos,
  reproducibilidad y que el gate estricto no presenta la regresión bootstrap
  como conformidad completa.

- [x] **CONF-RATCHET-001 — Hacer incremental la evidencia nueva.** El comando
  `tondo-reliability ratchet check` valida el linaje único de draft y su historial,
  inventario, matriz, baseline de quality y el registro canónico de hashes.
  `ratchet generate` solo escribe el registro después de comprobar todos esos
  bytes; si existen case layers exige reports de coverage y mutation que pasen
  la no-regresión. La Wave 0 no tiene capas ejecutables y registra ambos scopes
  como `not-applicable` con razón explícita. Cada wave futura debe ejecutar este
  mini-gate antes de integrarse; `META-CONF`, `UTEST-CONF` y los gates
  estándar siguen siendo cierres acumulativos.

- [ ] **CONF-SEAL-001 — Sellar una única conformidad Tondo 0.1.** Después de
  `META-CONF-001`, `UTEST-CONF-001` y Gate T0, exigir cero requisitos
  pendientes, fijar hashes actuales de specs/cases/adaptador, reconstruir desde
  un workspace limpio y promover atómicamente el último draft verificado a la
  distribución inmutable del primer release. Probar que no mezcla artefactos
  de otra historia; Gate G5 verifica este
  resultado y no vuelve a generarlo.

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
puros de selección y `defer await` pueden avanzar en lanes independientes.
`PARSER-STACK-001` debe cerrarse antes de `UTEST-CST-001`, para que la nueva
sintaxis no amplíe la ruta recursiva temporal.
`UTEST-CHECK-001` y la ruta de attachments ya tienen la identidad binaria de
`std.bytes`; el checker espera ahora la identidad implementada de `Duration` y
`Instant` del time-base;
la lectura de inputs declarados espera `STD-ENV-SPEC-001`,
`STD-ENV-IMPL-001` y `STD-ENV-CONF-001`;
`UTEST-VTIME-001`, lifecycle temporal y Gate T0 esperan
`STD-TIME-BASE-SPEC-001`, `STD-TIME-BASE-IMPL-001` y
`STD-TIME-BASE-CONF-001`. Son APIs de producción de STD-0.1A, nunca shims
privados del runner. `ASYNC-DEFER-IMPL-001` debe cerrar antes de conducir
teardown de suite; testing no puede sustituirlo por un hook.

**Compatibilidad:** el corpus bootstrap y sus hashes se conservan únicamente
como regresión reproducible. El borrador actual `TONDO_LANGUAGE_SPEC.md`, su
hash y `tondo-conformance-draft` se amplían de forma explícita; no se mantiene
un segundo parser o dialecto. Hasta cerrar M10.6, un
binario declara testing como componente pendiente y no anuncia conformidad
completa Tondo 0.1.

**Orden interno por lanes:**

1. **Plan:** `UTEST-PLAN` → contrato de inputs
   (`UTEST-INPUTS-PLAN`)/discovery/owners/dev-dependencies →
   `UTEST-RESULT-MODEL` → `UTEST-CLI-PARSE`. La materialización de inputs queda en la lane de
   ejecución, después de crear el worker.
2. **Lenguaje:** lexer → CST → formatter; tras unir source classes,
   discovery y dev-dependencies del plan → árbol/capturas →
   overlays/integration. `ASYNC-DEFER-IMPL-001` avanza en paralelo sobre la
   ruta async existente.
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
  `withVirtualTime`/`settle`/`advance`, quiescencia durable, cleanup async,
  interrupción, inputs públicos/secretos, artifact/snapshot stores, update
  explícito, CLI, JSON/JUnit, stdlib boundary, diagnósticos y conformidad sin
  depender de una implementación provisional.

- [x] **ASYNC-DEFER-SPEC-001 — Cerrar `defer await` como efecto general de
  Tondo 0.1.** Especificar grammar, inferencia async solo en
  test/setup/script, requisito `async` explícito en funciones, métodos y cierres,
  única llamada infallible `Unit`, reserva de un operando afín, liveness/`Send`,
  LIFO, no cancelación cooperativa durante unwind, pánico/suprimidos,
  timeout/resource/interrupt y rechazo por capability. No admitir bloque async,
  error propagado, hook de testing ni await oculto. Sus reglas ya están
  incorporadas a la especificación consolidada; implementación y conformidad
  permanecen pendientes.

- [x] **UTEST-EDITION-001 — Consolidar testing en Tondo 0.1.** Añadir `suite`
  y `test` al registry de keywords de la especificación viva, incorporar
  `defer await`, grammar y diagnósticos y renombrar los formatos de tooling a la
  línea única `draft`. Preservar los bytes del corpus bootstrap únicamente como
  regresión por hash, no como edición seleccionable alternativa. La implementación y los tests de
  compilación se ejecutan en las tareas siguientes.

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
  Diagnostics de uso terminan en exit `2`; una forma válida termina
  explícitamente en exit `3` porque `UTEST-CLI-001` aún no conecta la ejecución.
  Evidencia: `docs/contracts/test-cli-plan.md`, cinco tests unitarios del parser
  y un test CLI de ambos límites.

- [ ] **UTEST-INPUTS-001 — Materializar inputs públicos y secretos sin
  filtraciones.** Después de `UTEST-INPUTS-PLAN-001`,
  `UTEST-RUNTIME-001` y `STD-ENV-CONF-001`, resolver los descriptors del plan,
  materializarlos exclusivamente dentro del worker y revocarlos al terminar.
  Probar que valores secretos no entran en plan serializado, cache key de
  compilación, diagnostics, reportes, snapshots, artifacts o productos salvo
  copia explícita del programa, y documentar que el runner no realiza redacción
  heurística. Un fallo de materialización termina con exit `1` sin reporte
  parcial; un fallo de revocación pierde aislamiento y usa exit `3`.

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

- [x] **ASYNC-DEFER-IMPL-001 — Implementar y verificar `defer await`.** Añadir
  la forma 0.1 al CST/formatter sin feature gate, checks de
  firma/efecto/ownership,
  guards async en HIR/MIR/bytecode y conducción LIFO en el executor. Probar
  inferencia de entradas/script, `E1610` en función no async, retorno, error
  exterior, pánico, cancelación, cleanup suprimido, timeout, resource limit,
  interrupción, un owner afín, `Send` y rechazo de bloque/llamada
  fallible/capability mediante `E1608`/`E14xx`. La forma se conecta al parser
  existente; HIR conserva `Await -> AsyncCall`, MIR/bytecode validan el contexto
  `DeferredAsync` y la VM conduce llamadas async de bytecode y resultados async
  de host sin cancelar el cleanup que inició el unwind. Evidencia ejecutable:
  `m10-async-defer-await`, `m10-async-defer-lifo` y `m10-async-defer-cancel`, más
  negativos para función no async, `Join`, awaits anidados y llamada fallible.
  La inmutabilidad se demuestra reproduciendo el corpus bootstrap por sus
  hashes, no manteniendo dos gramáticas en el compilador del draft.

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
  `test_check::check` cierra las entradas privadas `async? fn(): Unit ! E` de
  tests y setups, permite `Unit`/`Never`, prohíbe valores retornados y
  `return` en setup, normaliza la unión de errores y exige `Discard`. Consume
  las pruebas de ownership, préstamos, terminales, `Send`, `Share` y `unsafe`
  sin relajarlas, infiere async desde `await` y tiempo virtual y rechaza
  `std.testing` desde producción con `E2003`. Valida las formas monomórficas de
  `log`, `tags`, `failNow`, `skip`, `attach`, `snapshot`, `withVirtualTime`,
  `settle` y `advance`, incluyendo nombres/media types, duplicados de
  evidencia, `P2005`/`P2006` y la clausura
  `Send + CallOnce[async fn(ref VirtualTime): Unit ! E]`. Evidencia:
  `docs/contracts/test-check.md` y diez tests unitarios deterministas.

### 17.3 Lowering, runtime y CLI

- [x] **UTEST-LOWER-001 — Bajar entradas de test por el pipeline común.** HIR,
  MIR, bytecode y sus admission verifiers representan árbol/parent, entradas de
  setup, snapshots de entorno, identidad, source span, error, async, cleanup,
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
  reutilizar el entorno ni sus snapshots. Conducir `defer await` hasta completar
  dentro de teardown, sin bloquear el worker ni inventar `afterAll`. Evidencia:
  `docs/contracts/test-suite.md` y ocho tests unitarios sobre participación,
  orden exterior-interior, cleanup LIFO, bloqueo causal, skip, continuidad de
  hermanos, teardown y retry con contexto fresco.

- [ ] **UTEST-LIMIT-001 — Hacer límites y timeout terminales reales.** Publicar
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
  presentan como assertion failure ordinario.

- [ ] **UTEST-GLOB-001 — Implementar el selector glob portable.** Parsear
  componentes `::`, `*`, `?` y `**` con la gramática cerrada de la spec,
  rechazar patterns vacíos/no canónicos y no delegar matching al shell,
  filesystem, locale ni normalización. Hacer full match case-sensitive sobre
  IDs Unicode de suite/test mediante un algoritmo dinámico acotado
  `O(pattern_scalars * id_scalars)`, seleccionar subárboles de suites,
  deduplicar la unión y aplicar selección antes de shard/order. Cubrir vectores
  Unicode, cero/muchos componentes para `**`, metacaracteres inválidos,
  coincidencias solapadas y no-match con y sin `--allow-empty`.

- [ ] **UTEST-SHARD-001 — Particionar hojas de forma estable.** Aplicar
  `sha256-mod-v1` después de filter/glob/exact y antes del orden, con índices
  one-based, validación estricta y asignación independiente de discovery order,
  plataforma y cantidad de jobs. Probar unión exacta, disjunción, compilación
  completa, el vector SHA-256 normativo, lifecycle independiente por proceso y
  shard vacío válido sin `--allow-empty` cuando la selección previa no era
  vacía.

- [ ] **UTEST-SCHED-001 — Fijar orden y paralelismo observable.** El default
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

- [ ] **UTEST-VTIME-001 — Implementar tiempo virtual determinista sobre la API
  de producción.** Ejecutar `withVirtualTime` como `CallOnce` async bajo un
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

- [ ] **UTEST-RETRY-001 — Implementar retries explícitos y sin estado
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

- [ ] **UTEST-REPEAT-001 — Implementar repetición completa y aislada.** Parsear
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

- [ ] **UTEST-ARTIFACT-001 — Persistir attachments por intento.** Implementar
  `testing.attach` con copia exacta y linealizada de `std.bytes.Bytes`, gramática
  cerrada de nombre/media type, unicidad por intento, límites y `P2006`. Calcular
  descriptors SHA-256 y escribir `tondo-test-artifacts-0.1/1` con objects
  content-addressed inmutables, deduplicación, manifest canónico y publicación
  atómica. Rechazar symlinks, paths derivados incorrectos, duplicados y
  colisiones; no incluir Base64, upload, ejecución, timestamps ni paths físicos.
  Mantener blobs huérfanos fuera del store lógico tras interrupción y permitir
  su recolección segura.

- [ ] **UTEST-SNAPSHOT-001 — Implementar snapshots textuales explícitos.**
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

- [ ] **UTEST-INTERRUPT-001 — Cerrar la interrupción externa.** En la primera
  señal, detener dispatch, cancelar cooperativamente, conducir cleanup
  incluyendo `defer await` durante el grace period y revocar secretos,
  procesos, handles y recursos; una segunda señal puede forzar terminación.
  Emitir exit `4` solo si se restauró aislamiento y exit `3` en caso contrario.
  No publicar JSON, JUnit, manifest de artifacts ni snapshot update parcial;
  cada output final conserva sus bytes anteriores o permanece ausente. Permitir
  únicamente blobs content-addressed huérfanos y una salida humana marcada
  `interrupted`, nunca un resultado machine-readable presentado como completo.
  La tarea implementa y prueba la transacción coordinator/worker mediante un
  evento de interrupción inyectable después de cerrar stores y reporters; el
  mapping de señales del SO y su prueba pública quedan en `UTEST-CLI-001`.

- [ ] **UTEST-REPORT-001 — Implementar los formatos machine-readable.**
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

- [ ] **UTEST-JUNIT-001 — Exportar JUnit desde el resultado normativo.**
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
  como representación canónica y sin pérdida.

- [ ] **UTEST-CLI-001 — Conectar `tondo test` end-to-end.** Después de cerrar
  plan, lifecycle, algoritmos, stores y reporters, conectar las opciones ya
  parseadas por `UTEST-CLI-PARSE-001` con discovery, compilación completa,
  selección, ejecución, output, publicación atómica y señales del SO sobre la
  transacción de `UTEST-INTERRUPT-001`. `--filter` compara hojas; glob/exact
  aceptan hoja o suite y esta última selecciona su subárbol.
  Verificar colisiones, combinaciones inseguras y exits 0/1/2/3/4 contra el
  modelo único de resultados; ninguna rama de CLI vuelve a implementar
  glob/shard/order/retry/repeat/artifacts/snapshots/reporting. No implementar
  `--tag`, selector regex ni `--fail-fast` bajo este contrato.

### 17.4 Evidencia, conformidad y dogfooding

- [ ] **UTEST-CONF-001 — Ampliar la conformidad del draft Tondo 0.1.** No
  presentar el corpus bootstrap como evidencia de requisitos nuevos. El
  manifiesto draft añade los cincuenta y dos grupos mínimos enumerados por la
  spec de testing y mantiene adaptador público para VM y futuros backends.

- [ ] **UTEST-PROJECTS-001 — Añadir proyectos de aceptación completos.**
  Incluir package unitario, integration roots, dev-dependency, suites anidadas,
  servicio compartido, captura válida/inválida, async/error, fallos de
  setup/teardown, `blocked-setup`, log directo/desde helper/task, `failNow`,
  tags directos/desde helper/task, conflicto `P2002`, skip de hoja/suite,
  `blocked-skip`, `P2001`, deny-skips, pánico/cleanup, host capabilities,
  CODEOWNERS, substring/glob/exact, selección vacía, shards, orden/seed,
  retries de hoja/setup/teardown, aislamiento externo idempotente,
  `flaky-pass`/allow-flaky, campañas repeat, backoff/deadline/debounce con tiempo
  virtual, quiescencia, deadlock/solapamiento/rango, cleanup mediante
  `defer await`, inputs públicos/secretos y sus fallos de
  materialización/revocación, interrupción, attachments y snapshots en
  check/update mode, y reporters JSON/JUnit. Cada proyecto debe poder ejecutarse
  desde una copia en otro path físico con observaciones canónicas iguales salvo
  duración JUnit y material secreto deliberadamente externo.

- [ ] **UTEST-PLATFORM-001 — Validar la matriz declarada.** Linux ejecuta el
  gate canónico completo; Linux ARM64, macOS Intel/ARM64 y Windows ejecutan
  discovery, paths jerárquicos, substring/glob/exact de suite/test, lifecycle,
  envelopes, tags/logs/skips, CODEOWNERS, sharding, orden/seed, workers nuevos
  de retry y repeat, reloj virtual y ties de timers deterministas, cleanup async,
  inputs, interrupción, artifacts/snapshots, aislamiento, timeout, captura y
  reportes JSON/JUnit aplicables además del smoke test de binario.

- [ ] **UTEST-DOGFOOD-001 — Probar componentes Tondo mediante `tondo test`.**
  Antes de Gate T0, mantener una pequeña biblioteca de aceptación escrita en
  Tondo con unit/integration tests y al menos una suite que comparta un recurso
  real. Debe usar `testing.log` y `testing.tags` desde helpers, probar
  `failNow`/skip en casos de aceptación controlados y ejecutar el mismo corpus
  repartido en shards con seed registrada y un retry aislado determinista que
  ejercite `flaky-pass` mediante una fixture externa controlada, sin depender de
  timing y con cleanup final verificable. Debe ejecutar además una campaña
  repeat determinista, adjuntar al menos un artifact, comprobar y actualizar un
  snapshot textual en un fixture aislado, y levantar/cerrar mediante
  `defer await` un servicio de integración. Antes de estabilizar
  `std.process`/`std.net`, ese servicio se implementa enteramente en Tondo sobre
  tasks y la API pública del paquete probado; el dogfood hosted con proceso o
  red reales se añade en STD-0.1A/B y no autoriza un shim de testing. Debe
  probar una API de producción
  con backoff o deadline mediante `withVirtualTime`, sin pausas reales, y
  producir reportes JSON/JUnit con inputs, stores y tiempo virtual separado de
  duración operacional. No sustituye los tests Rust ni la conformidad;
  demuestra que la experiencia pública funciona sin harness privado.

### Gate T0 — Testing first-class conforme

- [ ] El corpus bootstrap, sus manifests, hashes y observations permanece
  verificable como regresión histórica, fuera de los requisitos nuevos.
- [ ] El borrador consolidado Tondo 0.1 incorpora el contrato de testing,
  reserva `suite` y `test` y define `defer await` como cleanup general,
  infallible y verificable sin añadir hooks de testing ni un segundo dialecto.
- [ ] Lexer, CST, parser, formatter, HIR, MIR, bytecode y VM recorren la ruta
  común y sus verifiers aceptan o rechazan árboles suite/test con diagnostics
  exactos.
- [ ] Unit overlays ven privados sin alterar producción; integration roots solo
  ven API pública; `std.testing`, dev-dependencies y operaciones test-only nunca
  entran en productos publicables.
- [ ] Cada entrada recibe un envelope no observable ni falsificable que sigue
  frames/tasks y nunca se deriva de un thread-local del host; tags, logs y
  terminales se atribuyen al nodo exacto sin `TestContext` ni `currentTest()`.
- [ ] Suites ejecutan setup una vez por participación solo para subárboles
  seleccionados, permiten únicamente capturas `let: Copy + Send + Share`,
  hacen teardown tras todos los descendientes y reportan setup, teardown,
  `blocked-setup`, skip y `blocked-skip` sin duplicar causas.
- [ ] Retorno, error, `assert`, `failNow`, skip, pánico, async, cancelación,
  ownership, `defer` y `defer await` conservan cleanup y precedencia; `P2001`,
  resource limits, timeout e interrupción no esconden cleanup observado ni
  rompen aislamiento.
- [ ] Inputs públicos se fijan por bytes/hash y secretos solo por descriptor;
  ningún valor secreto entra en productos, cache keys, reportes o stores
  implícitos, y cada worker materializa y revoca únicamente lo declarado.
- [ ] `Duration`, `Instant`, suspensión, timers y deadlines usan el sustrato
  monotónico de producción; `withVirtualTime` lo sustituye solo dentro de su
  closure, presta un controlador no escapable y conserva el timeout real.
- [ ] Quiescencia durable, `settle`, avance explícito/automático, cola y ties de
  timers son deterministas; esperas externas no se virtualizan y `P2003`,
  `P2004` y `P2005` conservan sus condiciones exactas.
- [ ] `tondo test` implementa discovery, compilación completa, selección,
  substring/glob/exact de suite/test, CODEOWNERS, sharding estable, orden/seed,
  ejecución serial/paralela, retries aislados por rondas, repeat aislado por
  iteraciones, captura, artifacts, snapshots, reporters, interrupción y exit
  codes deny-skips/allow-flaky/empty según contrato; no inventa regex, filtrado
  por tags, retries/repeat implícitos ni fail-fast.
- [ ] Cada retry reutiliza solo el artefacto inmutable, arranca un worker nuevo,
  conserva shard/configuración, respeta el máximo global de jobs y deja
  procesos, recursos rastreados, heap, roots, tasks, handles, envelopes y
  buffers sin supervivientes; cada dominio virtual vuelve al mismo cero.
- [ ] Cada iteración repeat ejecuta el plan completo en worker nuevo, no se
  solapa con otra iteración y, con count mayor que uno, mantiene exit rojo ante
  cualquier non-pass aunque otra oportunidad del mismo nodo pase; count uno
  conserva la policy ordinaria.
- [ ] Attachments y snapshots pertenecen al intento exacto; los stores `/1`
  son canónicos, acotados y atómicos, y snapshot update nunca se activa ni
  elimina entries de forma implícita.
- [ ] Una interrupción deja de despachar, intenta cleanup/revocación y usa exit
  `4` o `3` según aislamiento; nunca publica reportes, manifests o updates
  parciales como completos.
- [ ] El reporte JSON `/7` es canónico y reproducible, conserva todos los
  intentos, iteraciones, dominios y descriptors sin material secreto añadido
  por el runner ni payloads externos embebidos; JUnit `/4` proyecta la misma
  ejecución y policy con duración operacional real y tiempo virtual separado.
  La salida humana no intercala suites/tests o intentos y muestra owners, tags,
  evidence, tiempo virtual, logs, razones y fallos accionables.
- [ ] El grupo de testing de `tondo-conformance-draft` pasa en la VM y la matriz
  de plataformas aplicable está verde.
- [ ] Existe dogfooding escrito en Tondo que usa la superficie pública, sin
  registration APIs, `TestContext`, annotations, reflection, subtests dinámicos
  ni hooks ocultos.

Al cerrar T0 se vuelve a ejecutar la suite completa —incluida
metaprogramación—, `CONF-SEAL-001` promueve el draft verificado y solo
entonces puede cerrarse Gate G5 para el draft consolidado Tondo 0.1.

---

## 18. M10.7 — Metaprogramación estática y reflection

**Objetivo:** implementar la superficie añadida al draft Tondo 0.1 sin abrir el
frontend a ejecución arbitraria. El resultado debe soportar `derive`, generators
de manifest y metadata reflection con código estático, reproducible e
inspeccionable.

**Dependencia:** Gate H0, `CONF-DRAFT-001` y el corpus bootstrap de M10.
`META-FORMAT-001` abre el trabajo compartido. Syntax y modelo pueden avanzar
mientras se cierra `STD-META-SPEC-001`. `META-VM-001` crea después el target
cerrado necesario para ejecutar `STD-META-IMPL-001`; esta implementación espera
también a `META-MODEL-001`. Providers y generators no pueden comenzar hasta
cerrar además `STD-META-CONF-001`.
`PARSER-STACK-001` precede a `META-SYNTAX-001`, para no construir las nuevas
formas sobre la recursión temporal del bootstrap.
`STD-REFLECT-001` precede obligatoriamente a `REFLECT-IMPL-001`. Ninguno de
estos contratos espera al resto de STD-0.1A ni introduce un shim provisional.
La contribución de M10.7 a Gate G5 permanece pendiente hasta incorporar
requisitos, diagnostics y casos en la conformidad viva; el Gate G5 final exige
además M10.6/T0.

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
  El lector actual acepta solo estos records; el corpus bootstrap se mantiene
  fuera de esta frontera como regresión. Cerrado con
  `crates/tondo-compiler/src/toolchain.rs`, `ProjectPlanDraft::parse`, contrato
  `docs/contracts/toolchain-formats-draft.md` y tests de round-trip, canonicalidad,
  grafos, hashes y outputs.

### 18.2 Frontend y modelo semántico

- [ ] **META-SYNTAX-001 — Implementar `derive` end-to-end en syntax.** Lexer,
  CST sin pérdida, parser, recuperación, formatter canónico, documentación y
  AST/HIR soportan una única declaración sin attributes ni body.

- [ ] **META-SEM-001 — Validar solicitudes derive.** Resolver identidades
  exactas de traits/providers, owner nominal, binders, duplicados, superficie
  permitida, bounds generados, coherencia y conflictos con impls manuales.

- [ ] **META-MODEL-001 — Construir el snapshot meta inmutable.** Serializar de
  forma canónica únicamente la clausura de roots autorizada: módulos,
  declaraciones, tipos, bounds, fields/variants públicos, spans y docs; entregar
  al derive solo la vista privada del target autorizado. Excluir bodies,
  valores, layout, direcciones y estado del GC; diagnosticar `E2109` si una
  clausura requiere una salida de la misma ronda.

- [ ] **META-QUERY-001 — Exponer expansiones y procedencia.** Tooling devuelve
  fuente formateada, provider, request/output hashes, bounds introducidos y
  source map sin revelar símbolos privados ajenos al target.

### 18.3 Ejecución hermética

- [ ] **META-VM-001 — Implementar el sustrato target/VM `tondo-meta`.**
  Registrar target, loader y sandbox capaces de ejecutar un programa Tondo
  mínimo con heap nuevo por run, cero capabilities y contadores deterministas
  de steps, memoria viva y output. Esta tarea no embebe una API provisional:
  `STD-META-IMPL-001` compila después el companion especificado sobre este
  sustrato, y solo entonces se habilitan providers.

- [ ] **META-DERIVE-001 — Ejecutar providers derive.** Pasar requests tipados,
  limitar outputs al impl autorizado, validar y formatear fuente, y fusionarla
  solo cuando todos los providers terminan correctamente.

- [ ] **META-GEN-001 — Ejecutar generators de manifest.** Entregar únicamente
  inputs declarados por valor y la clausura pública de roots explícitos, exigir
  todos y solo los outputs cerrados, impedir lectura ambiental, generación
  multi-round y observación de outputs hermanos.

- [ ] **META-ATOMIC-001 — Integrar identidad, cache y productos.** Incluir
  model/provider/request/output hashes en interfaces y artifacts; reutilizar
  cache solo con identidad completa y no publicar fuente, interface o artifact
  parcial ante fallo.

- [ ] **REFLECT-IMPL-001 — Implementar metadata runtime alcanzable.** Generar
  metadata de `typeInfo[T]()` estáticamente, eliminar la no alcanzable y
  demostrar que `TypeId` no escapa como identidad de wire ni habilita value
  reflection.

### 18.4 Evidencia y contribución a Gate G5

- [ ] **META-DIAG-001 — Cubrir `E2101`–`E2109`.** Cada error tiene vecino
  positivo, precedencia, span/ubicación nula correcta, JSON estable y
  diagnostics de provider asociados a inputs o fields relevantes.

- [ ] **META-REPRO-001 — Probar hermeticidad y determinismo.** Variar cwd,
  environment, filesystem order, core count y scheduling; repetir builds,
  comparar outputs byte a byte y demostrar denegación de filesystem, red,
  process, clock, entropy, threads, async, FFI y unsafe.

- [ ] **META-ROBUST-001 — Añadir properties, fuzzing y límites.** Fuzzear
  records draft y revisiones de schema, modelo meta, outputs y source maps; probar cycles imposibles,
  roots que cruzan la frontera de ronda, colisiones, pánicos, budget exhaustion,
  UTF-8 inválido y generadores hostiles sin panic del compilador ni publicación
  parcial.

- [ ] **META-CONF-001 — Extender `tondo-conformance-draft`.** Añadir syntax,
  semantic, tooling, runtime metadata, toolchain y reproducibility cases en la
  línea draft creada por `CONF-DRAFT-001`, sin presentar la regresión bootstrap
  como conformidad completa. Ratchetear su contribución acumulada solo después de actualizar
  inventario, trazabilidad, coverage y mutation evidence para la superficie
  nueva; el sellado conjunto pertenece a `CONF-SEAL-001`.

### Gate G5 — Primera versión del lenguaje publicable

- [ ] Todo el draft Tondo 0.1, incluidos M10.7 y M10.6, está implementado y
  tiene conformidad aplicable sobre `tondo-vm-hosted`.
- [ ] Gate T0 está cerrado y el grupo de testing forma parte de
  `tondo-conformance-draft`, no de una edición o suite paralela.
- [ ] `CONF-SEAL-001` ha promovido exactamente el draft verificado, sin
  presentar la regresión bootstrap como requisitos nuevos ni dejar pendientes.
- [ ] La suite y sus manifests fijan el hash actual de la spec, no el snapshot
  bootstrap de regresión.
- [ ] No existe una ruta de ejecución ambiental dentro del frontend ni del VM
  meta.
- [ ] La distribución puede describirse como candidata a publicación; este gate
  por sí solo no realiza ni afirma una publicación.

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
runner; no constituye un sexto módulo estándar adelantado. `defer await` es
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

- [ ] **STD-SPEC-001 — Cerrar la integración de
  `TONDO_STANDARD_LIBRARY_SPEC.md`.** Después de los specs por owner, comprobar
  que todas las superficies de STD-0.1A forman un único contrato sin firmas
  duplicadas, huecos de capability ni ciclos. Esta tarea es un cierre agregado,
  no el lugar donde se inventan por primera vez las APIs de cada módulo.

- [x] **STD-MOD-001 — Definir módulos y prelude mínimo.** El contrato base fija
  el catálogo cerrado, un propietario canónico por declaración, `std` único y
  reservado, imports ordinarios, cero inicialización global y ningún nombre
  implícito adicional ni extensión global de métodos.

- [x] **STD-CAP-001 — Fijar la matriz de capabilities.** El contrato base
  clasifica cada superficie como Core, capability-gated, test-only, build-only o
  target-specific, fija `tondo-capabilities-draft`, exige ausencia estática
  `E1008` y conserva el corpus bootstrap de `tondo-vm-hosted` como regresión con
  `[console, process]`. Cada módulo pendiente deberá completar su matriz por
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

- [ ] **STD-CORE-001 — Fijar protocolos y operaciones fundamentales.**
  `Option`, `Result`, `Display`, comparación, `Key` y utilidades de
  error conservan las capacidades y efectos ya definidos por el lenguaje.

- [ ] **STD-TEXT-001 — Especificar texto.** `String`, `Char`, `Byte` y
  sus operaciones fijan construcción, búsqueda, transformación, Unicode,
  límites y costes sin confundir scalar, grapheme ni byte.

- [x] **STD-BYTES-SPEC-001 — Especificar `std.bytes`.** `Bytes`, builders,
  conversiones explícitas `Bytes(String)`/`String(Bytes)` y `Array[Byte]`,
  UTF-8 estricto, slicing, igualdad, hashing y límites tienen una única
  identidad binaria reutilizada por console,
  filesystem, procesos y testing. Base64, hexadecimal y codecs wire-format
  permanecen bajo sus módulos propietarios posteriores.

- [ ] **STD-IO-001 — Especificar `std.io`.** Fijar protocolos estáticos de
  lectura/escritura, buffers, EOF, partial I/O, errores portables, ownership,
  backpressure, suspensión y cancelación sin que importar los contratos conceda
  ninguna capability. Console, archivos y procesos reutilizan esta única
  frontera en vez de inventar streams incompatibles.

- [ ] **STD-MATH-001 — Especificar `std.math`.** Fijar las operaciones escalares
  portables que completan los numéricos intrínsecos, incluidos floor, ceil,
  round, truncate, sqrt y FMA explícita, conservando IEEE, ausencia de fast-math
  observable, dominio, errores y casos límite.

- [ ] **STD-COLL-001 — Especificar colecciones.** `Array`, `Map` y `Set` fijan
  consulta, construcción, actualización funcional, mutación explícita,
  capacidad, orden, combinación y complejidad preservando semántica de valor.

- [ ] **STD-ITER-001 — Especificar ranges e iteración.** `Range`, iteradores y
  combinadores usan dispatch estático, un único elemento por target, evaluación
  lazy acotada y consumo/copia visibles.

- [ ] **STD-FMT-001 — Especificar `std.format`.** Display de tipos compuestos,
  builders y formato estructurado deben reutilizar el protocolo estático sin
  introducir reflection, vtables, lookup abierto ni una segunda interpolación.

- [ ] **STD-SER-001 — Especificar `std.serialization`.** Cerrar las firmas de
  `Serialize`, `Deserialize`, `Serializer[E]` y `Deserializer[E]`, su máquina
  de eventos, derive format-neutral, bounds genéricos, construcción atómica,
  ownership, errores y personalización mediante impl/DTO explícito.

- [ ] **STD-REFLECT-001 — Especificar el contrato exacto de `std.reflect`.**
  Cerrar antes de `REFLECT-IMPL-001` `TypeInfo`, `TypeId`, kinds,
  descriptores, ownership y errores públicos; fijar los oracles de retención
  opt-in, DCE y ausencia de value access, private access, layout, global
  registry o identidad portable. La implementación y su evidencia runtime
  permanecen en M10.7.

- [ ] **STD-META-SPEC-001 — Especificar `std.meta`.** Cerrar request/response,
  modelo inmutable, recorrido canónico, renderizado seguro, source builder,
  ownership, errores y ausencia de capabilities/callbacks antes de construir el
  target `tondo-meta`. Los providers de formatos concretos continúan en sus
  módulos posteriores.

- [ ] **STD-JSON-001 — Especificar JSON out of the box.** Ruta
  typed directa, `JsonValue`/`JsonNumber`, reader/writer/eventos incrementales,
  UTF-8, duplicados, unknown fields, orden/canonical output, límites, errors con
  path y corpus interoperable; nunca materializar DOM para typed decode.

- [ ] **STD-MSGPACK-001 — Especificar MessagePack out of the
  box.** Cubrir todo el data model y extension values, representación mínima,
  maps con keys arbitrarias, streaming, canonical mode, límites y
  interoperabilidad sin reflection.

- [ ] **STD-PROTOBUF-001 — Especificar Protobuf schema-first.**
  Generar desde `.proto` declarado tipos/codecs para proto3, presence, repeated,
  packed, maps, open enums con `Int32` preservado, nested y oneof; preservar
  unknown fields, comprobar evolución y ofrecer encoding determinista sin
  presentarlo como canonical universal. Services/gRPC quedan fuera.

- [ ] **STD-PERF-001 — Fijar contratos de rendimiento.** Cada hot path tiene
  oracle escalar, streaming/bytes-first, límites de allocation/memoria,
  workloads y gates de throughput, latencia, startup, code size y compile time.
  SIMD, word-at-a-time y target multiversioning se permiten solo con
  equivalencia exacta y fallback portable.

- [ ] **STD-TESTING-SPEC-001 — Especificar `std.testing`.** Fijar assertions de
  igualdad, diffs de texto, comparación float con tolerancia, consumo explícito
  de Option/Result, recursos temporales y datos generados que entren realmente
  en 0.1. Cada API declara tipos, ownership, cleanup, formato, seed,
  capabilities y límites; reutiliza sin alterar
  `log/tags/failNow/skip/attach/snapshot/withVirtualTime`,
  `VirtualTime.settle/advance`, el snapshot textual/store/update ya normativos
  ni sus diagnósticos. Los helpers de diff o generación no crean un segundo
  snapshot format, no registran tests, no interpretan tags runtime como
  selectores ni capturan pánicos como excepciones recuperables.

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

- [ ] **STD-CONSOLE-001 — Consolidar consola sobre `std.io`.** Fijar stdout,
  stderr, entrada, flushing, texto/binario, errores y comportamiento async sin
  asumir terminal interactiva ni duplicar los protocolos generales.

- [ ] **STD-PATH-001 — Definir paths portables y nativos.** Separar operaciones
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

- [ ] **STD-FS-001 — Definir filesystem.** Archivos, directorios, metadata,
  enlaces, permisos, atomicidad, iteración y operaciones async declaran
  portabilidad, TOCTOU, cleanup y errores sin esconder bloqueo.

- [ ] **STD-PROC-001 — Estabilizar procesos.** Promover el bridge provisional
  de `Command`, `Pipeline`, `ProcessHandle`, status, output, pipes, shell
  explícito y cancelación a una API versionada que preserve argv exacto.

### 19.4 Implementación y evidencia

- [ ] **STD-META-IMPL-001 — Implementar `std.meta` sobre el target cerrado.**
  Después de `META-VM-001`, materializar el companion meta dentro de la
  distribución candidata, implementar requests, recorrido, renderizado y
  builder en Tondo cuando sea posible y validar su descriptor/content hash. No
  incorpora providers de serialization ni formatos.

- [ ] **STD-META-CONF-001 — Cerrar la evidencia build-only.** Ejecutar
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
  funciones públicas sin alias mutable. La evidencia no muta el manifest
  histórico; el bridge de attachments se prueba después en `UTEST-ARTIFACT-001`.

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

- [ ] **STD-ENV-CONF-001 — Cerrar la evidencia temprana de environment.** Probar
  snapshots vacío y declarado, ausencia, Unicode/bytes, ownership, capability y
  rechazo de consulta ambiental fuera del adaptador; sin snapshot explícito no
  aparece ninguna entrada del host. Clasificación production/test-only,
  materialización/revocación por worker, perfiles secretos y ausencia de
  filtraciones pertenecen a `UTEST-DEPS-001`, `UTEST-INPUTS-001` y Gate T0, no
  a este micro-gate previo al runner.

- [ ] **STD-TIME-BASE-CONF-001 — Cerrar la evidencia del sustrato temporal.**
  Añadir modelos de suma/comparación/overflow de `Duration` e `Instant`, tests de
  identidad/mismatch de proveedor, suspensión, deadline, cancelación, empates de
  timers y disponibilidad por capability. Ejecutar el mismo corpus contra
  proveedor real con tolerancias operacionales explícitas y proveedor virtual
  con observaciones exactas; no usar sleeps reales para verificar el segundo.
  Antes de T0 usa el harness/adaptador público existente; después se vuelve a
  ejecutar mediante `tondo test` como parte de S1A. Debe pasar antes de Gate T0.

- [ ] **STD-IMPL-001 — Coordinar implementación Core por owner.** Instanciar y
  cerrar un subtask `IMPL` por cada módulo de A1/A3, implementando en Tondo
  cuando sea posible. Solo operaciones intrínsecas o dependientes del host
  permanecen privilegiadas; duplicar lógica portable en Rust requiere
  justificación. El checkbox agregado no se cierra mientras falte un owner.

- [ ] **STD-IMPL-002 — Coordinar Hosted por owner.** Instanciar y cerrar un
  subtask `HOST` por cada módulo A2 sobre adaptadores capability-gated. El
  runtime VM no expone una syscall o handle que la API estándar no haya
  validado y tipado; el checkbox agregado no sustituye esos subtasks.

- [ ] **STD-TESTING-IMPL-001 — Implementar `std.testing` sobre T0.** Escribir en
  Tondo toda utilidad portable y reutilizar el bridge privilegiado de T0 sin
  duplicarlo. Confinar a unidades privilegiadas únicamente temp resources,
  captura o aislamiento que requieran host; `attach`, `snapshot` y tiempo
  virtual siguen usando los sinks/controladores sellados de T0. Los helpers
  producen mensajes accionables sin reflection privada y se prueban mediante
  `tondo test`, no mediante registro interno.

- [ ] **STD-TEST-001 — Coordinar modelos y properties por owner.** Cada módulo
  registra y cierra su subtask `MODEL/TEST/FUZZ` con valores normales, vacíos,
  límites, errores, composición, ownership, determinismo y secuencias
  generadas. Los ejemplos del spec estándar son ejecutables; el agregado no
  oculta owners pendientes.

- [ ] **STD-CODEC-CONF-001 — Cerrar evidencia de serialization y formatos.**
  Ejecutar vectores oficiales, interoperabilidad con dos implementaciones
  independientes cuando existan, fuzzing diferencial, round trips, streaming
  con cada boundary, inputs truncados/adversarios, límites, unknown/duplicate
  policies y typed/dynamic equivalence para JSON y MessagePack. Protobuf añade
  wire compatibility, schema evolution y preservación de unknown fields.

- [ ] **STD-PERF-CONF-001 — Coordinar performance por owner.** Cada módulo con
  hot path cierra su subtask `PERF`: comparar scalar/optimized byte a byte y por
  errors; medir allocations, memoria, throughput, tail latency, startup, code
  size y compile-time en hardware y corpora registrados. Rechazar regressions
  materiales no justificadas y conservar resultados versionados.

- [ ] **STD-CONF-001 — Coordinar conformidad por owner.** Cada módulo cierra su
  subtask `CONF`; el agregado extiende el linaje único del draft o un manifiesto
  estándar enlazado de forma explícita, sin mutar el manifest bootstrap. Otro
  implementador debe poder ejecutar los casos mediante un adaptador público y
  distinguir lenguaje de stdlib.

- [ ] **STD-DOC-001 — Cerrar documentación por owner y programas
  representativos.** Cada módulo cierra su subtask `DOC`. Como aceptación
  integrada, transformación de texto, procesamiento de colecciones, JSON
  tipado, MessagePack streaming, Protobuf generado, copia segura de archivos y
  pipeline de procesos usan únicamente APIs especificadas y forman corpus de
  benchmarks.

### Gate S1A — Standard Library 0.1 foundation

- [ ] La spec estándar fija todas las firmas de su catálogo Core + Hosted
  incluidas en STD-0.1A y mantiene cerrado el catálogo posterior de STD-0.1B.
- [ ] Los slices tempranos de meta, reflect, bytes, time-base y env read-only
  conservan las mismas identidades y contratos usados por M10.7/M10.6; S1A no
  sustituye un shim ni mantiene dos propietarios públicos.
- [ ] Cada owner A registra por separado spec, implementación/host, tests/model,
  performance aplicable, conformidad y docs; ninguna tarea umbrella oculta una
  celda pendiente.
- [ ] El sustrato monotónico de `Duration`, `Instant`, suspensión, timers y
  deadlines es único para producción/testing, está modelado y funciona con
  proveedor real o virtual sin cambiar bytecode de usuario.
- [ ] Core se ejecuta sobre la VM sin depender de una ABI nativa.
- [ ] Cada API hosted exige la capability correcta y conserva los claims del
  target candidato Tondo 0.1.
- [ ] `derive` de serialization, JSON, MessagePack y Protobuf schema-first se
  ejecutan sin reflection runtime, DOM intermedio obligatorio ni inputs
  ambientales.
- [ ] Los codecs pasan interoperabilidad, fuzzing, streaming, límites,
  preservación y gates de rendimiento sobre oracle escalar y kernels
  optimizados.
- [ ] Modelos, properties, ejemplos y conformidad estándar cubren sus contratos
  positivos, negativos, límites y composición.
- [ ] La distribución es reproducible, cerrada y versionada.
- [ ] Los programas representativos pasan el gate estricto y proporcionan el
  corpus funcional inicial para NATIVE-001 y PERF-001.
- [ ] `std.testing` está especificado, implementado y probado con su propio
  runner público; un proyecto puede escribir tests útiles usando solo
  `assert` y enriquecerlos mediante imports explícitos, sin crear un segundo
  formato de snapshots, artifacts o generated cases.
- [ ] No se ha congelado una ABI FFI general ni un layout nativo público.

---

## 20. M11 — Backend nativo y optimización

**Objetivo:** añadir una implementación nativa de producción sin introducir una
segunda semántica. Comienza únicamente después de Gates H0, T0, G5 y S1A y de
cerrar los contratos runtime-facing `STD-CONC-001`, `STD-SYNC-001`,
`STD-EXEC-001` y la frontera host/cancelación de `STD-NET-001`. Esos módulos no
se implementan todavía, pero sus requisitos alimentan elección de backend,
memoria y ABI. La VM, la conformidad del lenguaje —incluidos test targets— y la
conformidad de STD-0.1A son los oracles.

**Orden obligatorio:** baseline → selección → memoria/ABI → lowering mínimo →
ARC/ciclos correctos → frontera STD-0.1A → diferencial/targets/empaquetado →
Gate N1. Eliminación de retains, COW, escape analysis, incrementalidad y LSP son
trabajo posterior a N1 y no pueden retrasar el primer backend correcto.

### 20.1 Selección y contrato del backend

- [ ] **PERF-001 — Definir benchmarks y presupuestos antes de implementar.**
  Incluir compilación, tamaño, programas STD-0.1A, throughput, latencia, memoria,
  retain/release, pausas y workloads adversarios; registrar hardware y entorno.
  La evaluación de backend y cada optimización posterior reutilizan esta
  baseline en lugar de cambiar el workload para justificar una decisión.

- [ ] **NATIVE-001 — Elegir backend nativo con una evaluación separada.**
  Comparar Cranelift, LLVM y generación propia usando el MIR real, el corpus de
  conformidad y los programas STD-0.1A. Medir soporte de targets, corrección,
  latencia de compilación, rendimiento, memoria, tamaño, debugging,
  distribución, mantenimiento y licencias; registrar la elección en un ADR.

- [ ] **NATIVE-MEM-ADR-001 — Cerrar DEC-014 antes de la ABI.** Fijar ownership
  runtime, contadores atómicos/no atómicos, `Send`/`Share`, weak refs,
  recolección de ciclos, interacción con async, COW y threads, estrategia de
  pánico/cancelación y oracles de verificación. Prototipar las rutas de riesgo
  necesarias para demostrar que el modelo soporta los contratos runtime-facing
  de STD-0.1B; no prometer layout público.

- [ ] **NATIVE-ABI-001 — Definir una ABI runtime interna y versionada.** Fijar
  después de `NATIVE-MEM-ADR-001` la frontera compilador/runtime necesaria para
  el backend elegido: calls, unwind, frames async, retain/release, roots,
  atomics, wakeups y handles host internos. Esta tarea cierra DEC-013; no
  promete ABI FFI, layout de usuario ni name mangling estables.

- [ ] **NATIVE-002 — Definir y ejecutar un lowering mínimo desde MIR.** Calls,
  pánicos, cleanup, ownership, préstamos, async, source maps y operaciones
  checked conservan identidad verificable hasta código nativo. El primer
  vertical slice ejecuta un programa real antes de ampliar targets u optimizar.

### 20.2 Runtime correcto y frontera estándar

- [ ] **ARC-001 — Implementar ARC correcto en el runtime nativo.** Cubrir
  ownership local/cross-thread, pánicos, cleanup, frames async y terminales
  antes de aplicar eliminación de retains.

- [ ] **ARC-002 — Implementar recolección diferida de ciclos y weak refs
  linealizables.** Validar ciclos independientes, races aplicables, teardown y
  ausencia de resurrección antes de ejecutar conformidad completa.

- [ ] **NATIVE-STD-001 — Implementar la frontera de STD-0.1A.** Core y Hosted
  observan la misma API, capabilities, errores y cleanup que en la VM; ninguna
  optimización puede añadir una ruta pública específica del backend.

### 20.3 Oracle diferencial, targets y empaquetado

- [ ] **NATIVE-CONF-001 — Crear el adaptador nativo de conformidad.** Ejecutar
  lenguaje, test targets y stdlib contra VM y nativo, comparar observaciones
  completas y exigir que ambos superen de forma independiente los manifiestos
  aplicables.

- [ ] **NATIVE-DIFF-001 — Ejecutar differential testing generado.** Programs
  tipados, properties, modelos y regresiones usan ambos backends; cada
  divergencia se reduce antes de decidir cuál implementación contradice el
  contrato.

- [ ] **NATIVE-TARGET-001 — Añadir targets uno a uno.** Cada combinación de
  arquitectura, SO, profile y capability set tiene registry, runner nativo,
  tests de plataforma y artefacto identificable. Cross-compilar no sustituye el
  smoke test sobre la arquitectura destino.

- [ ] **NATIVE-REL-001 — Empaquetar builds reproducibles.** Binarios, runtime,
  STD-0.1A y metadatos declaran versiones y checksums; el paquete candidato no
  depende de paths, reloj ni entorno no declarado. No es todavía el paquete
  final STD 0.1.0, que incorpora STD-0.1B en `REL-0.1-RC-001`.

### Gate N1 — Backend nativo conforme

- [ ] El backend elegido tiene ADR, targets soportados y ABI runtime interna
  explícitos.
- [ ] DEC-014 está cerrado y ARC/ciclos correctos satisfacen los contratos de
  concurrencia ya especificados sin layout público accidental.
- [ ] Todos los programas admitidos atraviesan el MIR verificado común; no
  existe frontend, type checker ni semántica paralela.
- [ ] El adaptador nativo supera lenguaje y STD-0.1A con observaciones
  compatibles con la VM, incluidos los estados y reportes de `tondo test`.
- [ ] Properties, fuzzing diferencial, GC/ARC/ciclos, async, pánicos y cleanup
  pasan bajo stress y sanitización aplicable.
- [ ] Cada target publicado compila y ejecuta un corpus real sobre hardware del
  target.
- [ ] Las optimizaciones aceptadas aportan una mejora medida y conservan todos
  los oracles.
- [ ] Los paquetes nativos son reproducibles y no prometen una ABI pública no
  especificada.

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

1. Antes de M11 se cierran `STD-CONC-001`, `STD-SYNC-001`, `STD-EXEC-001` y la
   frontera runtime/host de `STD-NET-001`; son inputs de DEC-013/014 y de la
   elección de backend, no una autorización para implementarlos.
2. Tras N1 se implementan y conforman todos los módulos B sobre VM y backend
   nativo. Los demás specs B pueden prepararse durante M11 cuando sus
   dependencias A estén estables.

Esta fase no crea una segunda versión: cada módulo pertenece al catálogo cerrado
de STD-0.1 y solo puede considerarse listo tras superar su mini-gate
`SPEC → IMPL/HOST → MODEL/TEST/FUZZ → PERF → CONF → DOC`. STD 0.1.0 no se
publica hasta cerrar el gate final.

| Orden B | Owners | Dependencias duras | Momento |
|---|---|---|---|
| B0 | sync, channel, executor y frontera net | async/memoria/I/O/time A | contratos antes de M11 |
| B1 | `std.sync` | DEC-014 + backend/VM schedulers | implementación tras N1 |
| B2 | `std.channel` | sync + scheduler + ownership `Send` | tras B1 |
| B3 | `std.executor` | sync/channel + bridge bloqueante | tras B2 |
| B4 | civil time | time-base + timezone data versionada | paralelo a B1–B3 |
| B5 | encoding/YAML/TOML/CBOR | bytes + I/O + serialization | paralelo tras N1 |
| B6 | regex/UUID | text; UUID añade clock/entropy | paralelo tras N1 |
| B7 | net | I/O + time + executor/cancelación | después de B3 |
| B8 | log | format + time + I/O y sinks aplicables | después de owners de sinks |
| B9 | integración/distribución | todos los micro-gates | `REL-0.1-RC-001` y S1 |

### 21.1 Concurrencia y tiempo

- [ ] **STD-CONC-001 — Especificar `std.channel`.** Tipos, cierre,
  backpressure, selección cancelable, fairness declarada, ownership de
  `T: Send`, cancelación y ausencia de una keyword `select` implícita quedan
  fijados por API.

- [ ] **STD-SYNC-001 — Especificar `std.sync`.** Mutexes, rwlocks, condvars,
  semáforos y atomics declaran `Send`/`Share`, poisoning si existe, orden de
  memoria y prohibiciones dentro del scheduler.

- [ ] **STD-EXEC-001 — Especificar `std.executor`.** Pools, actores y bridging
  bloqueante no crean un segundo modelo async ni permiten que trabajo host
  bloquee el progreso de tasks Tondo.

- [ ] **STD-CIVIL-TIME-001 — Completar `std.time` con calendario civil.** Añadir
  `Date`, `Time`, `DateTime`, zonas horarias, reglas/versionado de timezone data
  y conversión explícita respecto al sustrato monotónico ya fijado. No
  duplicar `Duration`, `Instant`, suspensión, timers ni deadlines y no hacer que
  compilación consulte reloj.

### 21.2 Aplicación y datos

- [ ] **STD-NET-001 — Especificar `std.net` capability-gated.** Direcciones,
  DNS, sockets, streams, datagrams, TLS boundary, timeouts y cancelación
  exponen errores portables y no realizan I/O implícito.

- [ ] **STD-CODEC-002 — Especificar `std.encoding`, `std.yaml`, `std.toml` y
  `std.cbor`.** Base64, hexadecimal, YAML, TOML y CBOR fijan owner, streaming,
  límites y tratamiento de input no confiable sin duplicar
  `std.serialization`, JSON, MessagePack ni Protobuf de STD-0.1A.

- [ ] **STD-REGEX-001 — Especificar `std.regex`.** Sintaxis, Unicode,
  complejidad y límites evitan comportamiento exponencial no declarado.

- [ ] **STD-ID-001 — Especificar `std.uuid`.** Entropía, reloj,
  representación y versión de generador se solicitan mediante capabilities
  explícitas.

- [ ] **STD-LOG-001 — Especificar `std.log`.** Niveles, fields, formato, sinks,
  backpressure, concurrencia y fallos no alteran el control del programa de
  forma oculta.

### 21.3 Implementación y evidencia

- [ ] **STD-B-IMPL-001 — Coordinar implementación portable por owner.** Cada
  módulo B cierra un subtask `IMPL` reutilizando traits, ownership, async y
  módulos de STD-0.1A; cualquier intrinsic o unidad privilegiada nueva requiere
  contrato y justificación explícitos.

- [ ] **STD-B-HOST-001 — Coordinar adaptadores por owner.** Cada módulo
  aplicable cierra un subtask `HOST`; VM y backend nativo enlazan `clock`,
  `network`, `threads`, `entropy` y capabilities de sinks sin stubs que fallen
  siempre ni efectos concedidos por import.

- [ ] **STD-B-TEST-001 — Coordinar evidencia funcional por owner.** Cada módulo
  cierra `MODEL/TEST/FUZZ`; cubrir cierre y backpressure, memory ordering,
  wakeups, calendario y zonas versionadas, parsers hostiles, regex acotado,
  UUID, logging concurrente, networking parcial/cancelado y límites.

- [ ] **STD-B-PERF-001 — Coordinar rendimiento por owner.** Cada hot path
  cierra `PERF` con throughput, tail latency, memoria, allocations, startup,
  code size y compile time; kernels SIMD o especializados conservan un oracle
  portable byte/error exacto y fallback por target.

- [ ] **STD-B-CONF-001 — Coordinar conformidad y docs por owner.** Cada módulo
  cierra `CONF` y `DOC`; el agregado publica casos portables/capability-gated y
  programas reales en VM/nativo sin mutar evidencia histórica ni crear un
  segundo PackageId de stdlib.

- [ ] **REL-0.1-RC-001 — Construir el candidato completo de primera
  publicación.** Después de todos los micro-gates A/B, reconstruir toolchain,
  VM, backend, runtime, stdlib runtime y companion meta desde inputs cerrados;
  fijar PackageId, content/API hashes, manifests, checksums y matriz de targets
  finales. Ejecutar conjuntamente G5, H0, T0, N1, todos los checks que componen
  S1, conformidad VM/nativa, programas representativos y reproducibilidad desde
  dos workspaces limpios. Gate S1 se cierra después de verificar este resultado;
  la tarea crea un candidato, pero no publica por sí misma.

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
- [ ] Tondo 0.1, `tondo test`, VM, backend nativo y los programas
  representativos pasan juntos el gate estricto antes de cualquier publicación.
- [ ] `REL-0.1-RC-001` reproduce exactamente el candidato completo y no
  reutiliza el paquete parcial de `NATIVE-REL-001` como si ya incluyera
  STD-0.1B.

---

## 22. Trabajo transversal

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
| `R-020` | Confundir el corpus bootstrap con el contrato del draft | Se reescribe evidencia histórica o se anuncia soporte inexistente | El corpus bootstrap permanece fijado por hash; spec, parser y conformidad del draft incorporan suite/test como trabajo pendiente |
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
| `R-036` | Añadir hooks async exclusivos del runner o esperar cleanup de forma oculta | Segundo lifecycle, orden sorprendente y código distinto entre test y producción | `defer await` general de Tondo 0.1, infallible y verificable; suites reutilizan exactamente esa semántica |
| `R-037` | Embeber, subir o dejar crecer artifacts sin contrato | Filtración de datos, reportes enormes y almacenamiento no reproducible | `attach` explícito, límites, descriptors SHA-256 y store content-addressed local sin Base64 ni upload |
| `R-038` | Hashear, reportar o redactor heurísticamente secretos | El secreto se filtra o la reproducibilidad declarada es falsa | Descriptors/versiones solamente, materialización/revocación por worker y responsabilidad explícita si el programa copia el valor |
| `R-039` | Confundir repeat con retry o solapar iteraciones | Una campaña deja de reproducir el plan y comparte estado entre oportunidades | Modos incompatibles, iteraciones completas secuenciales en workers nuevos, count uno como no-op y cualquier non-pass rojo con count mayor |
| `R-040` | Actualizar o borrar snapshots de forma automática o parcial | CI acepta cambios accidentales y el store queda incoherente tras fallos | Texto exacto, update explícito y restringido, staging total, reemplazo atómico y preservación de entries no alcanzadas |
| `R-041` | Convertir metaprogramación en ejecución arbitraria del compilador | Builds ambientales, no cacheables y con acceso a datos del host | Frontend puro, target `tondo-meta` sin capabilities, una ronda, inputs/outputs/límites/hashes cerrados y VM nueva por run |
| `R-042` | Usar reflection runtime para serializers | Lookup, boxing, metadata global, acceso privado y hot paths difíciles de optimizar | Traits estáticos, derive/codegen directo y `std.reflect` descriptivo sin valores |
| `R-043` | Permitir que código generado eluda coherencia o visibilidad | Impls imposibles de escribir manualmente y APIs con privilegios ocultos | Output Tondo ordinario, owner exacto, vista privada limitada, mismo typecheck y consulta de expansión/source map |
| `R-044` | Optimizar codecs solo para benchmarks felices | Regresiones de allocation/latencia, DoS por inputs hostiles o semántica distinta entre SIMD/scalar | Oracle escalar, gates multidimensionales, corpus adversario, límites y equivalencia byte/error exacta por kernel |
| `R-045` | Inferir Protobuf desde records o reflection | Field numbers inestables, presencia perdida y evolución wire incompatible | Schema-first desde `.proto` fijado, unknown fields preservados y checks de evolución en build time |
| `R-046` | Usar el manifest bootstrap como evidencia del draft | El gate queda roto durante meses o se atribuyen casos antiguos a reglas nuevas | `CONF-DRAFT-001`, selección explícita del linaje draft, requisitos pendientes honestos, ratchet por wave y `CONF-SEAL-001` después de T0/meta |
| `R-047` | Cerrar bloques enormes con una sola tarea umbrella | No se conoce el estado real por módulo y los fallos aparecen al final | Micro-gates verticales, modelo único de resultados y estado SPEC/IMPL/TEST/PERF/CONF/DOC por owner |
| `R-048` | Usar la recursión del host como pila del parser | Un input válido o malicioso aborta antes del límite tipado en targets con stacks pequeños | Guarda portable temporal; `PARSER-STACK-001` migra toda profundidad controlada por fuente a frames explícitos y conserva solo presupuestos configurables |

---

## 24. Cola inmediata

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
    gate estricto lo valida en cada ejecución.
21. [x] **Wave 1 — Formatos draft.** Implementar `META-FORMAT-001` con
    parse/canonicalización/round-trip y rechazo de records no draft. El mini-gate
    queda cerrado: manifest, lockfile, interface, artifact y descriptor estándar
    usan una única forma draft; el corpus bootstrap permanece como regresión.
22. [ ] **Wave 2 — Prerrequisitos y frontends, en paralelo.**
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
      `ASYNC-DEFER-IMPL-001` avanza en paralelo y se une antes del lowering.
    Mini-gate: cada frontend baja hasta su primer IR verificable, los cinco
    slices tempranos tienen owner definitivo y la conformidad viva ratchetea.
23. [ ] **Wave 3 — Vertical slices ejecutables.**
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
24. [ ] **Wave 4 — Features y cierre del lenguaje.**
    - Meta: diagnostics, reproducibilidad, robustez y `META-CONF-001`.
    - Testing sobre worker estable: `UTEST-VTIME-001`, `UTEST-RETRY-001`,
      `UTEST-REPEAT-001`, `UTEST-ARTIFACT-001` y `UTEST-SNAPSHOT-001`
      avanzan en paralelo; su unión alimenta `UTEST-REPORT-001 →
      UTEST-JUNIT-001 → UTEST-INTERRUPT-001 → UTEST-CLI-001`.
    - Aceptación testing: `UTEST-CONF-001`, `UTEST-PROJECTS-001`,
      `UTEST-PLATFORM-001` y `UTEST-DOGFOOD-001`; después se cierra T0.
    - Unión final: `(META-CONF-001 + UTEST-CONF-001 + T0) →
      CONF-SEAL-001 → G5`.
    Mini-gate: T0 y después G5 verdes sobre hashes actuales; la regresión
    bootstrap queda separada de la conformidad del draft.
25. [ ] **Wave 5 — STD-0.1A por layers.** Cerrar `STD-PERF-001` y el spec de
    cada owner antes de su implementación; ejecutar A1 valores/protocolos, A2
    host, A3 serialization/codecs y A4 helpers de testing, cerrando el
    micro-gate de cada owner antes de sus consumidores. Owners independientes
    avanzan en paralelo. `STD-SPEC-001`, `STD-TEST-001`,
    `STD-CODEC-CONF-001`, `STD-PERF-CONF-001`, `STD-CONF-001` y
    `STD-DOC-001` cierran A5 y Gate S1A.
26. [ ] **Wave 6 — Contratos que condicionan el backend.** Cerrar
    `STD-CONC-001`, `STD-SYNC-001`, `STD-EXEC-001` y la frontera runtime de
    `STD-NET-001`. Mini-gate: DEC-013/014 reciben requisitos completos sin
    implementar todavía STD-0.1B.
27. [ ] **Wave 7 — M11 correcto antes que optimizado.** Ejecutar
    `PERF-001 → NATIVE-001 → NATIVE-MEM-ADR-001 → NATIVE-ABI-001 →
    NATIVE-002 → ARC-001 → ARC-002 → NATIVE-STD-001 → NATIVE-CONF-001 /
    NATIVE-DIFF-001 → targets → NATIVE-REL-001`. Cerrar Gate N1.
28. [ ] **Wave 8 — Completar STD-0.1B y candidato 0.1.** Terminar specs B,
    implementar cada módulo sobre VM/nativo, ejecutar models/fuzz/perf/conf/doc
    y `REL-0.1-RC-001`; cerrar Gate S1. Optimizaciones post-N1 avanzan solo por
    evidencia y no bloquean el candidato salvo que un presupuesto publicado lo
    exija.

Resumen topológico:

~~~text
CONF-DRAFT
  -> FORMAT draft
  -> {PARSER-STACK -> meta/test syntax
      meta prerequisites + meta frontend
      bytes + env + time + test plan/frontend + defer-await}
  -> {meta runtime
      test runtime + algorithms}
  -> {META-CONF + T0} -> CONF-SEAL -> G5
  -> STD-0.1A / S1A
  -> STD-0.1B runtime contracts
  -> native correctness / N1
  -> STD-0.1B implementation + REL-0.1-RC / S1
~~~

M4, M5, M6, M7, M8, M9, el corpus bootstrap M10, M10.5, M10.5b y Gates G4/H0
quedan cerrados. Gate G5 está abierto únicamente por M10.7 y M10.6 dentro del
draft no publicado. `CONF-DRAFT-001` y `CONF-RATCHET-001` están cerrados; la
acción inmediata es completar las lanes pendientes de Wave 2. `PARSER-STACK-001`
ya está cerrado antes de añadir formas sintácticas. Ninguna tarea posterior necesita
volver a decidir el orden global: sus prerequisitos están en 4.1.1 y en la wave
correspondiente.

---

## 25. Historial del tracker

### 1.28 — 2026-07-31

- Se completa `UTEST-RUNTIME-001` con `tondo_compiler::test_runtime` y el
  contrato `docs/contracts/test-runtime.md`. El runner crea un bootstrap,
  worker/heap/executor, envelope y registro de recursos nuevos por hoja,
  captura pánicos, ejecuta cleanup LIFO, revoca handles aunque sobrevivan
  referencias obsoletas y ordena los resultados por ID. Se fijan los estados
  de retorno, skip, error, pánico, límite, timeout e infraestructura, junto a
  la frontera de reloj monotónico/virtual. Catorce tests cubren aislamiento,
  revocación, terminales, cleanup, snapshots, paralelismo y retries.

### 1.27 — 2026-07-31

- Se completa `UTEST-CONTROL-001` con `tondo_compiler::test_control` y el
  contrato `docs/contracts/test-control.md`. El envelope privado mantiene
  logs, tags, streams, artifacts, snapshots, virtual time, terminales y
  presupuestos por intento con operaciones atómicas e idempotentes; no expone
  contexto ni identidad al lenguaje. Se fijan `P2001`, `P2002`, `P2004`,
  `P2006`, `P2007`, `P2008`, `P0007`, el orden de skip de tasks estructuradas y
  el rechazo `E2003` de intrinsics desde producción. Dieciséis tests cubren
  límites, precedencia, aislamiento y revocación.

### 1.26 — 2026-07-31

- Se completa `UTEST-LOWER-001` con `tondo_compiler::test_lower` y el
  contrato `docs/contracts/test-lower.md`. El lowering consume exclusivamente
  contratos ya comprobados, ordena el árbol por spans, conserva parent,
  snapshots de entorno, identidad, dominios, async y cleanup, y copia la
  operación sellada a HIR/MIR/bytecode con un admission verifier común.
  `main` queda fuera del target, las identidades y hashes son canónicos y
  nueve tests cubren operaciones, orden, metadatos, límites y tampering.

### 1.25 — 2026-07-31

- Se completa `UTEST-CHECK-001` con `tondo_compiler::test_check` y el contrato
  `docs/contracts/test-check.md`. El checker adapta los facts ordinarios sin
  rebajar ownership/capabilities, infiere el contrato Unit/Never y la unión
  `Discard`, sella las operaciones de `std.testing`, valida virtual time y
  mantiene `E2003` para producción. Diez tests cubren formas válidas,
  diagnósticos, evidencia, async y opacidad del controlador.

### 1.24 — 2026-07-31

- Se completa `UTEST-INTEG-001` con `tondo_compiler::test_integration` y el
  contrato `docs/contracts/test-integration.md`. Cada fuente bajo `tests/`
  recibe un consumidor sintético content-addressed, imports públicos
  explícitos, helpers privados locales y referencias resueltas sin friend scope
  ni dependencia implícita entre roots. La construcción por lote es estable y
  ocho tests cubren identidades, paths, visibilidad, imports, helpers y
  aislamiento.

### 1.23 — 2026-07-31

- Se completa `UTEST-OVERLAY-001` con `tondo_compiler::test_overlay`. La fase
  de producción queda sellada con su prueba de resolución/semántica/coherencia,
  fuentes exactas y hashes de interfaz, capabilities, coherence y artefacto.
- El overlay unitario admite únicamente la companion del mismo paquete y
  módulo, acceso privado a producción, helpers privados, imports públicos
  explícitos y el árbol suite/test separado. No reabre bodies, no modifica el
  producto de producción ni puede publicar, importar su companion, cambiar
  coherence o acceder a privados externos. Se añaden once tests deterministas
  y `docs/contracts/test-overlay.md`.

### 1.21 — 2026-07-31

- Se completa `UTEST-ID-001` con `tondo_compiler::test_tree`, un builder
  compile-time y host-free que consume CST y metadata cerrada, ordena las
  fuentes por identidad estable y expone el árbol pre-order con identidad
  interna, ID visible, parent y spans.
- Se implementan `E2001`, `E2002`, `E2004` y warnings `W1004`, incluidos
  duplicados de hermanos entre archivos, rechazo de reapertura/mezcla, suites
  vacías y nombres `_`. La permutación de entradas conserva nodos,
  diagnostics y rangos; `docs/contracts/test-tree.md` fija el contrato y
  doce tests nuevos lo cubren.

### 1.22 — 2026-07-31

- Se completa `UTEST-CAPTURE-001` con `tondo_compiler::test_capture`: la
  frontera semántica consume bindings y usos ya resueltos, verifica ancestoría
  estática y exige `let` con `Copy + Send + Share` sin obligación terminal.
- Cada descendiente recibe una descripción de snapshot inmutable por binding;
  `var`, préstamos, moves, capabilities no satisfechas, terminales y usos fuera
  de la cadena de padres emiten `E2005` con el uso primario y la declaración
  relacionada. La adaptación de facts queda cerrada a los resúmenes HIR y se
  añaden nueve tests unitarios y el contrato `docs/contracts/test-capture.md`.

### 1.20 — 2026-07-31

- Se completa `UTEST-FMT-001`: `SuiteBlock` usa el layout estructural de
  declaraciones para insertar una línea vacía entre setup y miembros, y entre
  miembros consecutivos; los cuerpos de `test` conservan las reglas ordinarias
  de bloques y las suites anidadas mantienen su indentación.
- Se añaden tres tests de formatter para layout canónico, nesting, bodies y
  setups vacíos/multiline, comentarios/documentación, declaraciones adyacentes,
  recovery e idempotencia. `docs/contracts/formatter.md` fija la regla de
  separación y el gate completo queda en verde.

### 1.19 — 2026-07-31

- Se completa `UTEST-CST-001` con nodos lossless `TestDecl`, `SuiteDecl` y
  `SuiteBlock` en CST/AST. `suite` conserva setup ordinario antes del primer
  miembro y solo admite `test`/`suite` directos después, incluidos niveles
  anidados.
- El parser reconoce las formas en Module e ImportedModule, rechaza el uso en
  Script, modifiers, declaraciones dentro de tests y setup posterior al primer
  miembro, y recupera hasta el siguiente miembro sin perder bytes ni nodos
  válidos. El formatter comparte el nuevo inventario y trata `SuiteBlock` como
  un bloque de llaves forzadas.
- Se añaden ocho tests de parser para vistas AST, modos, recovery, nesting,
  modifiers y reconstrucción exacta; la suite completa del compilador queda en
  verde.

### 1.18 — 2026-07-31

- Se completa `UTEST-LEX-001` reservando `suite` y `test` como keywords en todos
  los source forms, con normalización NFC, independencia de origen y rechazo
  como nombres de paquete. La evidencia quedó fijada en lexer, package y los
  artefactos de fiabilidad generados.

### 1.17 — 2026-07-31

- Se completa `UTEST-DEPS-001` con un grafo puro de interfaces de
  dev-dependencies. El record exige `PackageId`, path y SHA-256 exactos del
  plan; faltantes, adicionales, duplicados, targets de producción y ciclos
  terminan antes de materializar el grafo.
- Los aliases de test solo resuelven desde fuentes unit/integration. El lookup
  de producción falla explícitamente y no consulta ni mezcla el subgrafo.
  `production_identity` usa exclusivamente inputs de producción para conservar
  la independencia del artefacto publicable.
- Se añade `docs/contracts/test-dependencies.md` y nueve pruebas de compilador
  para metadata, orden, edges, ciclos, visibilidad y no interferencia.

### 1.16 — 2026-07-31

- Se completa `UTEST-OWNERS-001` con la resolución pura de CODEOWNERS. La
  selección `auto` respeta `.github/CODEOWNERS`, `CODEOWNERS` y
  `docs/CODEOWNERS`; `none` y paths explícitos conservan sus fronteras y un
  candidato inválido no cae silenciosamente a otro archivo.
- Se implementan UTF-8 sin BOM, CRLF, comentarios, owners opacos, el subset
  portable de `*`, `?`, `**`, anclaje, segment matching, trailing `/`, última
  regla, source/hash y guards de filesystem. No hay red, permisos ni consultas
  externas.
- Se añade `docs/contracts/test-owners.md` y nueve pruebas de compilador para
  selección, parsing, matching, hashing, errores y fuentes generadas.

### 1.15 — 2026-07-31

- Se completa `UTEST-DISC-001` con una frontera pura de discovery. La
  enumeración del host aporta solo paths, módulo y atestaciones de archivo;
  el clasificador no abre ni sigue paths y falla cerrado ante entradas no
  regulares, escapes por symlink, paths no canónicos y colisiones.
- Se fijan la precedencia convencional `tests/` → integration,
  `_test.to` → unit y roots explícitos, el orden por bytes UTF-8 y los inputs
  `source:<class>:<physical-path>`. La reconciliación compara la identidad
  completa contra el plan y reporta faltantes/adicionales antes de compilar.
- Se añade `docs/contracts/test-discovery.md` y ocho pruebas de compilador para
  precedencia, determinismo, guards de filesystem, identidades de módulo,
  duplicados y deriva de plan.

### 1.14 — 2026-07-31

- Se completa `UTEST-CLI-PARSE-001`: el binario reconoce `tondo test` y su
  parser puro normaliza la superficie completa del comando sin ejecutar fuentes
  ni workers. Se validan selectores exclusivos, paths lógicos, CODEOWNERS,
  shard/order/seed, límites y duración, retry/repeat explícitos, reports,
  artifacts, snapshot update y flags de policy.
- Las incompatibilidades y la deriva de sintaxis son errores de uso exit `2`.
  Una invocación válida deja claro que el runner todavía no está conectado y
  usa exit `3`, sin generar un resultado falso. Se añaden
  `docs/contracts/test-cli-plan.md` y evidencia unit/integration.

### 1.13 — 2026-07-31

- Se completa `UTEST-RESULT-MODEL-001` con `TestResultTree`, que concentra
  attempts, outcomes, causalidad, policy y summary en una única forma validada
  para todos los reporters. La derivación de `passed`, `flaky-pass`, fallos,
  bloqueos y contadores se hace una vez; el parser rechaza schema drift y
  referencias o estados inconsistentes.
- Se añade el protocolo puro `tondo-test-worker-0.1/1` con frames de hello/run,
  cancel/shutdown y ready/started/attempt/finished/cancelled/closed/error.
  `ProtocolSession` comprueba handshake, secuencias independientes, límites,
  cancelación, cleanup/closure y errores fatales sin ejecutar cuerpos ni tocar
  el host.
- Se documentan las fronteras en `TONDO_TESTING_SPEC.md` y
  `docs/contracts/test-result-model.md`, con siete tests de compilador.

### 1.12 — 2026-07-31

- Se completa `UTEST-INPUTS-PLAN-001` con el record value-free
  `tondo-test-input-plan-draft` y el parser puro `TestInputPlan`. Cada input
  queda ligado a un source y profile; los públicos solo llevan hash, los
  secretos solo metadata de provider/descriptor/versión y una capability
  habilitada opcional.
- El parser exige cobertura exacta de referencias, orden canónico, hash del
  plan, digests separados para contenido público y perfil secreto, conteo y
  estado de reproducibilidad. Nunca acepta ni serializa valores de inputs.
- Se añaden `docs/contracts/test-input-plan.md`, la sección 4.4 del toolchain
  spec y cinco pruebas de canonicalización, colisiones, deriva, capabilities,
  secretos y ausencia de canal de valores. La materialización/revocación queda
  explícitamente para `UTEST-INPUTS-001` dentro del worker.

### 1.11 — 2026-07-31

- Se completa `UTEST-PLAN-001` con el record puro
  `tondo-test-plan-draft` y `TestProjectPlan`. El plan liga hashes exactos de
  manifest/lockfile y cierra las tres source classes, roots físico-lógicos,
  inputs nombrados, dependencias de desarrollo y PackageIds, sin inferir roots
  por common-prefix ni leer el host.
- La misma frontera normaliza selector, shard, orden/seed, policy,
  reporters, stores de artifacts/snapshots, target/capabilities, el catálogo
  `std.time@monotonic-v1` y límites positivos. Rechaza campos desconocidos,
  duplicados, deriva de producción, hashes inválidos, configuraciones
  incompatibles y datos de fuente; discovery, inputs, CODEOWNERS y workers
  quedan explícitamente para sus tareas consumidoras.
- Se añade `docs/contracts/test-plan.md` y cobertura unitaria para clases,
  canonicalización, identidad, límites, stores, ausencia de bytes y todos los
  rechazos de forma material.

### 1.10 — 2026-07-31

- Se completa `ASYNC-DEFER-IMPL-001`. El parser ya admitía `defer` seguido de
  una expresión; el frontend ahora permite únicamente la forma especial
  `defer await <async-call>` dentro de una función async, conserva la llamada
  async en HIR y rechaza `Join`, bloques, awaits anidados, efectos fallibles y
  funciones no async con los diagnósticos normativos.
- MIR y bytecode introducen el contexto de admisión `DeferredAsync` sin crear
  un segundo mecanismo de cleanup. Los operandos siguen capturándose al
  registrar y el guard afín mantiene las mismas reglas `Copy`/`CallOnce`/`Send`.
- La VM drena ambas formas en LIFO. Una llamada async de bytecode reutiliza la
  continuación de frame; una llamada async de host tiene un estado de espera
  dedicado que no se cancela por el unwind que inició el cleanup y vuelve al
  mismo bloque de drain al completar. Retorno, pánico y cancelación conservan
  precedencia y cleanup suprimido.
- Se añaden fixtures de compilación/runtime para orden LIFO, retorno por
  cancelación y diagnósticos negativos; la puerta del bootstrap y la ejecución
  hosted cubren la ruta fuente→HIR→MIR→bytecode→VM.

### 1.09 — 2026-07-31

- Se implementa `STD-ENV-IMPL-001` en el compilador, bytecode, VM y
  `BootstrapHost`: `std.env.snapshot` devuelve un snapshot inmutable y
  cacheado por invocación; `Name.fromText`/`fromBytes`, `Snapshot.arguments`,
  `Snapshot.get`, `Value.asText` y `Value.asBytes` tienen firmas y fronteras
  tipadas. No se consulta `std::env` ni se introduce input ambiental implícito.
- Se añaden pruebas directas de proveedor para argv ordenado, UTF-8 válido,
  bytes inválidos, ausencia, nombres inválidos, host unavailable, límites
  atómicos y recuperación tras un rechazo; el fixture `m10-std-env-001` cubre
  la ruta compilador→VM y la capability `environment` queda cerrada en el
  target hosted. `STD-ENV-CONF-001` sigue pendiente para la evidencia de
  distribución/runner.

### 1.08 — 2026-07-31

- Se amplía la evidencia de `STD-TIME-BASE-IMPL-001`: un corpus común ejecuta
  la misma semántica contra proveedores real y virtual, y añade rechazo de
  dominios extranjeros, deadlines empatados, cancelación, límites atómicos y
  ausencia de la capability `clock`.
- Se cierra `STD-ENV-SPEC-001`. `std.env` queda definido como snapshot runtime
  read-only y sellado por invocación, con `Name`/`Value` explícitos para UTF-8 y
  bytes, argv ordenado, ausencia mediante `Option`, límites y errores
  portables. No hay lectura en compilación, mutación global ni capability
  ambiental implícita en tests.
- Se incorpora `docs/contracts/stdlib-env.md`. `STD-ENV-IMPL-001` y
  `STD-TIME-BASE-CONF-001` siguen pendientes y conservan sus gates de
  implementación/distribución reproducible.

### 1.07 — 2026-07-31

- Se implementa `STD-TIME-BASE-IMPL-001` sobre la frontera async existente de la
  VM. El proveedor real usa `std::time::Instant`; el proveedor virtual queda
  sellado para testing y ambos comparten identidad de operaciones, dominios y
  cleanup.
- Se añaden `Duration`, `Instant`, `Timer`, `DurationError` y `ClockError` al
  catálogo de tipos, capacidades, terminalidad, snapshot y verificación del
  compilador/VM. `Timer` tiene cleanup terminal afín y no puede escapar sin
  `wait`, `cancel` o desregistro estructurado.
- `sleep` y `Timer.wait` son jobs cooperativos; deadlines, mismatch de dominio,
  overflow, retrasos negativos y `ClockError.ResourceLimit` tienen resultados
  tipados. Timers y jobs comparten un límite atómico que se libera en
  cancelación, completion y cleanup.
- Se incorpora el contrato vivo `docs/contracts/stdlib-time.md`, el capability
  `clock` al target hosted y el fixture end-to-end `m10-std-time-001`.
- `STD-TIME-BASE-CONF-001` permanece pendiente hasta cerrar source sets,
  interfaces, unidad privilegiada y hashes reproducibles del slice.

### 1.06 — 2026-07-31

- Se cierra `STD-TIME-BASE-SPEC-001` dentro del mismo draft Tondo 0.1.
- `std.time` separa el time-base monotónico de STD-0.1A del calendario civil de
  STD-0.1B y fija `Duration`, `Instant`, `sleep`, timers one-shot y deadlines
  sin introducir un `Clock` de testing ni un wrapper `Deadline` duplicado.
- Se fijan overflow comprobado, resolución declarada, identidad opaca de
  proveedor/dominio, mismatch determinista, ownership de `Timer`, cancelación y
  puntos de suspensión, además de las entradas source/interface/privileged-unit
  y hashes que debe materializar el plan cerrado.
- `STD-TIME-BASE-IMPL-001` y `STD-TIME-BASE-CONF-001` continúan pendientes; la
  siguiente acción es implementar el proveedor monotónico intercambiable.

### 1.05 — 2026-07-31

- Se elimina la duplicidad pública entre texto y bytes. `Bytes(String)` y
  `String(Bytes)` son ahora las únicas conversiones explícitas; la primera usa
  directamente el UTF-8 válido de `String` y la segunda valida UTF-8 de forma
  estricta.
- Se retiran las funciones y métodos de conversión con nombres alternativos de
  `std.bytes`; `Array[Byte]`, builders, slicing y observación binaria conservan
  sus operaciones específicas.
- Compiler, VM hosted, proceso, fixtures, documentación y tests validan la
  nueva superficie sin modificar la conformidad histórica `conformance/0.1`.

### 1.04 — 2026-07-31

- Se cierra la lane temprana `STD-BYTES-SPEC-001 → STD-BYTES-IMPL-001 →
  STD-BYTES-CONF-001` sin alterar el manifest histórico `conformance/0.1`.
- `std.bytes` incorpora `Bytes`, `BytesBuilder`, `BytesError`, constructores,
  conversiones copiadas, UTF-8 estricto, slicing totalizado, igualdad, FNV-1a
  estable y builders con receptor `var` verificado en la frontera host.
- La VM aplica `max_vm_heap_bytes` por buffer, hace atómicos los appends y
  usa `Bytes(String)`/`String(Bytes)` como conversiones canónicas. El fixture
  `m10-std-bytes-001` y los tests directos del host cubren límites, moves/copies,
  errores y ausencia de alias mutable; attachments continúan esperando T0.
- Se añade `docs/contracts/stdlib-bytes.md` y el catálogo normativo de firmas a
  ambas especificaciones. La siguiente lane es `STD-TIME-BASE-SPEC-001` o
  `STD-ENV-SPEC-001`, según el consumidor que se priorice.

### 1.03 — 2026-07-31

- Se cierra `PARSER-STACK-001`. El parser conserva un camino recursivo fijo y
  usa continuaciones explícitas para expresiones, delimitadores, bloques,
  loops, llamadas, records, tipos, patterns y recuperación profunda; el CST
  también recorre tokens con una pila explícita.
- Se elimina la guarda interna dependiente del stack. El único límite lógico
  restante es `ParseLimits.max_nesting_depth`; la documentación y ADR-004
  describen memoria O(depth), equivalencia de CST/formatter y el comportamiento
  portable en stacks pequeños.
- Se añaden pruebas de profundidad válida e inválida, límites lógicos,
  recuperación de cierres ausentes y round-trip del formatter. La siguiente
  integración es la lane pendiente de Wave 2, no una nueva versión del lenguaje.

### 1.02 — 2026-07-31

- Se elimina la confusión entre una línea de desarrollo y una versión publicada.
  Tondo tiene un único draft activo; no existe una lane `/1` frente a otra
  `/2`, ni un selector `checkpoint`/`live`, ni un segundo parser.
- Los formatos actuales del toolchain, la identidad del compilador, el runner
  de conformidad, la matriz y el ratchet usan el marcador `draft`. El corpus
  bootstrap anterior queda solo como regresión reproducible y los documentos
  históricos se conservan como arqueología, no como contratos seleccionables.
- El inventario, el resultado del adaptador y el ratchet se regeneran contra
  `conformance/draft/manifest.json`; la publicación de una primera versión
  futura será una decisión posterior y no forma parte de este estado.

### 1.01 — 2026-07-30

- Se cierra `META-FORMAT-001`. El nuevo módulo puro `toolchain` valida y
  canonicaliza manifest/lockfile `/2`, interfaces y artefactos `/2`, y el
  descriptor estándar `/1`; comprueba grafos runtime/meta, providers,
  generators, límites, paths, colisiones, hashes de contenido y
  `build_hash` antes de enumerar entradas.
- `ProjectPlanDraft::parse` expone la frontera explícita sin modificar los
  lectores `/1` ni el oráculo histórico. Se añade el contrato
  `docs/contracts/toolchain-formats-draft.md` y cobertura unitaria de rechazo y
  round-trip.
- Wave 1 queda cerrada. La siguiente acción es `PARSER-STACK-001` en Wave 2;
  la sintaxis `derive` sigue bloqueada hasta disponer de una pila de parser
  portable.

### 1.00 — 2026-07-30

- Se implementa `CONF-RATCHET-001` con los comandos
  `tondo-reliability ratchet generate/check` y el registro canónico
  `testing/conformance-ratchet.json`.
- El mini-gate valida inventario, matriz, quality baseline, linaje vivo e
  historial content-addressed; exige reports de coverage/mutation cuando hay
  case layers y registra `not-applicable` explícito en Wave 0.
- La revisión 2 del antiguo manifest que entonces se denominaba `live` conserva
  la revisión 1 bajo su directorio histórico y retira únicamente
  `CONF-RATCHET-001` de las tareas pendientes. Es arqueología del tracker, no
  una ruta que el toolchain actual pueda seleccionar.
- El gate estricto ejecuta el ratchet en cada validación y la siguiente tarea
  pasa a ser `META-FORMAT-001`.

### 0.99 — 2026-07-30

- Se cierra `CONF-DRAFT-001` con selección obligatoria entre el corpus
  bootstrap y el draft, snapshot verificable del tag `v0.1.0`, manifest draft
  content-addressed y preflight de sellado no mutante.
- Reliability hereda evidencia histórica solo cuando coinciden el ID estable y
  el hash exacto del requisito; las 27 reglas nuevas o modificadas permanecen
  `draft-pending` y una declaración de layer sin evidencia ejecutable no puede
  convertirlas en `covered`.
- El gate validaba entonces ambos linajes sin fallback, conservaba cada
  manifest como artefacto CI y probaba que la reproducción y ejecución de los
  205 casos del corpus bootstrap no leían el spec draft.
- `CONF-RATCHET-001` pasa a ser el siguiente nodo antes de
  `META-FORMAT-001`.

### 0.98 — 2026-07-30

- El límite interno del parser recursivo baja temporalmente a 128 para que ARM64
  y hosts con stacks pequeños produzcan `T0002` en lugar de abortar, conservando
  256 como presupuesto lógico configurado.
- `PARSER-STACK-001` programa la solución estructural antes de ampliar la
  gramática: pila explícita para toda profundidad controlada por fuente,
  equivalencia del CST y diagnostics, pruebas con stacks pequeños y eliminación
  posterior de la guarda interna sin retirar límites de recursos configurables.

### 0.97 — 2026-07-29

- Se reemplaza la cola serial por un DAG de ocho waves con prerequisitos,
  lanes paralelas y mini-gate de evidencia al finalizar cada unión.
- `CONF-DRAFT-001`/`CONF-RATCHET-001` separan el corpus bootstrap inmutable de
  la conformidad del draft antes de tocar M10.7 o M10.6; `CONF-SEAL-001` queda como
  unión explícita posterior a T0/META-CONF, evitando cerrar una tarea temprana
  con trabajo diferido hasta G5.
- STD-0.1 adelanta únicamente `std.meta`, el contrato de `std.reflect`,
  `std.bytes`, el time-base y la lectura declarada de `std.env`; así elimina los
  ciclos que obligaban a meta y testing a consumir APIs programadas después de
  T0.
- M10.6 separa parsing de CLI, modelo de resultados y wiring final; elimina los
  restos de dos ediciones, divide contrato/materialización de inputs y ordena
  plan, frontend, runtime, algoritmos, features, reporters, interrupción y CLI.
- M11 pasa a corrección antes que optimización: contratos de
  channel/sync/executor/net y DEC-014 preceden ABI/lowering; ARC/ciclos preceden
  conformidad, la baseline precede la selección de backend y ARC
  optimization/COW/escape/incremental/LSP quedan post-N1.
- STD-0.1A/B usan micro-gates por owner y `REL-0.1-RC-001` reconstruye el
  candidato completo después de ambas fases sin confundirlo con el paquete
  parcial de N1.

### 0.96 — 2026-07-29

- Se elimina la frontera editorial 0.2 antes de la primera publicación:
  `suite`, `test`, `defer await`, sus diagnósticos y los formatos de testing
  pasan al único contrato Tondo 0.1. El checkpoint interno `v0.1.0` conserva sus
  hashes y resultados como evidencia histórica, sin convertirse en dialecto.
- M10.6 amplía `tondo-conformance-0.1`; Gate G5 requiere ahora M10.7 y T0. Los
  formatos vivos son `tondo-test-report-0.1/7`,
  `tondo-test-list-0.1/6`, `tondo-junit-report-0.1/4`,
  `tondo-test-artifacts-0.1/1` y `tondo-snapshot-store-0.1/1`.
- Toda la Standard Library prevista se consolida en 0.1.0. STD-0.1A cierra la
  foundation necesaria para M11 y STD-0.1B completa concurrency/application
  después de N1; Gate S1 exige ambas fases antes de publicar. Las menciones 0.2
  de entradas históricas inferiores describen borradores ya superados y no
  forman parte de la ruta activa.

### 0.95 — 2026-07-29

- Se corrige el estado del producto: Tondo 0.1 continúa en desarrollo y no ha
  sido publicado. El tag/checkpoint 0.1.0 conserva evidencia histórica, pero no
  constituye una release pública ni congela el draft.
- Se cierra DEC-016 y se añade M10.7 para `derive`, modelo semántico meta,
  generators de manifest, VM hermético, formatos toolchain `/2`, expansión
  inspeccionable y reflection metadata-only. Gate G5 vuelve a estar pendiente
  hasta implementar y validar esa superficie.
- STD-0.1 incorpora `std.serialization`, `std.reflect`, `std.meta`, `std.json`,
  `std.messagepack` y `std.protobuf`. Los codecs tipados usan código generado
  directo; Protobuf es schema-first y ningún formato depende de reflection de
  valores.
- Se fijan oracles escalares, kernels SIMD/word-at-a-time opcionales,
  multiversioning, streaming, límites y gates multidimensionales de rendimiento
  junto a interoperabilidad, fuzzing y corpus adversario.
- La ruta inmediata comienza en META-FORMAT-001 y termina M10.7 antes de volver
  al sustrato temporal, T0, S1 y el backend nativo.

### 0.94 — 2026-07-29

- Se crea `TONDO_STANDARD_LIBRARY_SPEC.md` como contrato normativo de
  arquitectura, sin fingir firmas de módulos todavía abiertas. Fija versionado
  conservador, una sola `std` por grafo, PackageId/hash, actualización explícita,
  prelude mínimo, propietarios canónicos, availability/capabilities, errores,
  ownership, async, determinismo, distribución, conformidad y migración
  inmutable desde bootstrap.
- Se cierran DEC-012, `STD-FOUNDATION-SPEC-001`, `STD-MOD-001`,
  `STD-CAP-001`, `STD-ERR-001` y `STD-DIST-001`. El catálogo 0.1 reserva métodos
  de intrínsecos, `std.bytes`, `std.io`, `std.math`, `std.format`, `std.time`,
  `std.path`, `std.console`, `std.env`, `std.fs`, `std.process` y
  `std.testing`; las firmas concretas permanecen pendientes.
- `Bytes` adquiere `std.bytes` como propietario futuro único y los argumentos
  runtime migrarán de la superficie bootstrap de process a `std.env`. La
  extensión de testing referencia ya la identidad binaria canónica.
- `STD-TIME-BASE-SPEC-001` sigue siendo la acción inmediata; no se implementan
  todavía tiempo, testing de usuario ni otra API estándar.

### 0.93 — 2026-07-29

- El workspace se migra y verifica íntegramente en el SSD montado en
  `/mnt/media`; branch, remotes, objetos Git y contenido permanecen idénticos y
  la copia anterior se retira solo después de la comparación exacta.
- El hardening alcanza 1.507 casos lógicos y 1.726 repeticiones. La baseline
  sube a 90,08 % de líneas, 86,42 % de funciones y 88,15 % de regiones, con
  ratchets actualizados globales y por riesgo; branch y MC/DC se registran como
  no medidos porque el toolchain fijado no instrumenta ninguna unidad.
- Un mapa de evidencia revisado enlaza por separado las seis dimensiones de las
  reglas de resultados opacos con tests ejecutables y frontera pública. La
  matriz pasa a 17 requisitos cubiertos y 271 límites de trazabilidad
  explícitos, sin convertir proximidad documental en evidencia.
- La campaña de mutación conserva 27/27 mutantes ejecutables detectados, uno
  inviable, cero timeouts y cero supervivientes.

### 0.92 — 2026-07-29

- Se completa M10.5b con 1.445 casos lógicos y 1.664 repeticiones. La
  conformidad versionada completa se ejecuta dentro de la misma instrumentación
  y se añaden contratos cerrados para CLI, artefactos, manifiestos, adaptador,
  semántica, bytecode, runtime y tooling de fiabilidad.
- La baseline revisada sube a 86,11 % de líneas, 83,62 % de funciones y
  84,45 % de regiones, con ratchets globales y separados para parser, checkers,
  verifiers, heap, ejecución y protocolos no confiables.
- La selección de mutación conserva 27/27 mutantes ejecutables detectados, uno
  inviable y ningún timeout o superviviente. La cola sigue en
  `STD-TIME-BASE-SPEC-001`; el hardening no inicia STD-0.1 ni M10.6.

### 0.91 — 2026-07-29

- Se completa M10.5 y Gate H0 con un inventario machine-readable de 1.400 casos
  lógicos y 1.619 repeticiones, y una matriz de 300 requisitos normativos con
  identidades, clasificación, seis dimensiones y evidencia o waiver explícito.
- El gate estricto queda automatizado en PR y `main`; Linux ARM64, macOS
  Intel/ARM64 y Windows prueban la superficie portable, mientras las campañas
  deterministas de fuzzing y el tier nocturno conservan artefactos
  reproducibles.
- Se añaden generadores reducibles, properties metamórficas, modelos de
  colecciones, ownership, concurrencia y memoria, y tres fuzz targets con seis
  semillas revisadas.
- La baseline inicial cubre el 81,11 % de líneas, 78,60 % de funciones y
  79,25 % de regiones. La campaña acotada de mutación detecta los 27 mutantes
  ejecutables de 28; el restante es inviable y no existe ningún superviviente.
- Dos mutantes supervivientes iniciales se convierten en regresiones públicas
  para admisión de unidades privilegiadas y line endings aislados. La cola
  avanza, sin iniciar aún el trabajo, a `STD-TIME-BASE-SPEC-001`.

### 0.90 — 2026-07-29

- La extensión de testing sube a `0.2-draft.7`. Tondo 0.2 incorpora
  `defer await` como cleanup general e infallible, sin crear hooks async de
  testing; el documento fija el snapshot exacto de Tondo 0.1 que consume.
- Se fijan inputs públicos por bytes/hash y secretos únicamente por
  descriptor/version, los tres estados de reproducibilidad y la respuesta
  transaccional a interrupciones con exits `4`/`3`.
- `testing.attach` registra bytes por intento en
  `tondo-test-artifacts-0.2/1`; `testing.snapshot` compara texto exacto contra
  `tondo-snapshot-store-0.2/1`, y solo `--update-snapshots` puede stagear y
  publicar cambios atómicos sin borrar entries no alcanzadas.
- `--repeat N` se separa de retry: reejecuta el plan completo en iteraciones
  secuenciales con workers nuevos, es incompatible con retry/update y mantiene
  exit rojo ante cualquier oportunidad no exitosa cuando `N > 1`; `N = 1`
  conserva la policy ordinaria.
- Los formatos incompatibles suben a `tondo-test-report-0.2/7`,
  `tondo-test-list-0.2/6` y `tondo-junit-report-0.2/4`, con inputs, iteraciones,
  stores y descriptors que no embeben material secreto añadido por el runner,
  bytes de artifacts ni valores completos de snapshots.
- M10.6 añade `ASYNC-DEFER-*`, `UTEST-INPUTS-001`,
  `UTEST-REPEAT-001`, `UTEST-ARTIFACT-001`, `UTEST-SNAPSHOT-001` y
  `UTEST-INTERRUPT-001`; amplía aceptación, plataformas, dogfooding y gates,
  registra R-035 a R-040 y eleva la conformidad mínima de 45 a 52 grupos.

### 0.89 — 2026-07-29

- La extensión de testing sube a `0.2-draft.6` y añade tiempo virtual opt-in
  mediante `testing.withVirtualTime`, un controlador prestado `VirtualTime` y
  las únicas operaciones `settle`/`advance`; no añade keyword, `TestContext`,
  flag global ni reloj inyectado en el código probado.
- El dominio usa la API monotónica de producción, una cola determinista,
  quiescencia durable, avance automático al próximo timer y avance explícito
  exacto. I/O, procesos y calendario siguen siendo reales, y timeout/límites del
  runner nunca usan tiempo virtual.
- Se fijan `P2003` para deadlock interno, `P2004` para solapamiento y `P2005`
  para duración/rango inválido. Cada retry empieza en el mismo cero sin timers,
  secuencias ni contadores heredados.
- El JSON incompatible sube a `tondo-test-report-0.2/6` e incorpora
  `virtual_time` por intento; la lista permanece `/5`. JUnit sube a
  `tondo-junit-report-0.2/3`, conserva duración real y expone
  `tondo.virtual_time` por separado.
- STD-0.1 extrae un time-base mínimo de `Duration`, `Instant`, suspensión, timers
  y deadlines antes de M10.6; calendario civil permanece en STD-0.2. El tracker
  añade `UTEST-VTIME-001`, tres tareas `STD-TIME-BASE-*`, R-033/R-034 y eleva la
  conformidad mínima de 40 a 45 grupos.

### 0.88 — 2026-07-28

- La extensión de testing sube a `0.2-draft.5` y añade `--glob` como tercer
  selector mutuamente excluyente: match completo sobre IDs con componentes
  `::`, `*`, `?` y `**`, gramática portable cerrada, Unicode scalar,
  deduplicación de subárboles y complejidad acotada sin regex ni expansión del
  host.
- `--retry N` habilita rondas adicionales explícitas solo para error, pánico y
  timeout. Las unidades reejecutan lifecycle ancestral o subárbol de suite,
  absorben causas descendientes, conservan shard/seed/inputs y arrancan workers
  nuevos que solo reutilizan el artefacto compilado inmutable.
- Cada nodo conserva intentos separados y un intento decisivo. Un éxito
  posterior es `flaky-pass`, falla por default y solo `--allow-flaky` modifica
  el exit/JUnit; no se borran failures, tags, logs ni streams previos.
- Los formatos incompatibles suben a `tondo-test-report-0.2/5`,
  `tondo-test-list-0.2/5` y `tondo-junit-report-0.2/2`. JSON incorpora
  plan/rondas de retry, causalidad por intento y summaries; JUnit mantiene un
  testcase agregado por hoja y proyecta flakiness sin multiplicar intentos.
- M10.6 añade `UTEST-GLOB-001` y `UTEST-RETRY-001`, amplía runtime, lifecycle,
  CLI, scheduling, reportes, aceptación, plataformas, dogfooding y Gate T0, y
  registra R-030 a R-032. La conformidad mínima pasa de 35 a 40 grupos.

### 0.87 — 2026-07-28

- La extensión de testing sube a `0.2-draft.4` y añade la cuarta operación
  sellada `testing.tags(Map[String, String])`: merge idempotente por nodo,
  conflicto `P2002`, propagación del envelope por helpers/tasks, uso permitido
  durante cleanup y prohibición de emplear tags runtime como selectores.
- Ownership se resuelve estáticamente desde CODEOWNERS con precedencia y
  matching cerrados, última regla aplicable, owners opacos, source/hash
  reportados y ausencia de red, permisos remotos o efecto sobre producción.
- El runner incorpora sharding estable `sha256-mod-v1` y orden
  canónico/aleatorio reproducible mediante `id-byte-order-v1`,
  `sha256-tree-v1`, seed registrada y `execution_plan`.
- Los schemas incompatibles suben a `tondo-test-report-0.2/4` y
  `tondo-test-list-0.2/4`. `tondo-junit-report-0.2/1` proyecta la misma
  ejecución para CI con properties, lifecycle sintético y duración operacional,
  mientras JSON permanece canónico y sin pérdida.
- M10.6 añade tareas separadas para ownership, sharding y JUnit y amplía
  planning, frontend, runtime, scheduling, aceptación, plataformas, dogfooding
  y Gate T0. La conformidad mínima pasa de 29 a 35 grupos.

### 0.86 — 2026-07-28

- La extensión de testing sube a `0.2-draft.3` y fija el núcleo test-only
  `std.testing.log`, `failNow` y `skip` con firmas exactas. No añade keywords,
  parámetros de test, `TestContext`, `currentTest()` ni fail-fast global.
- Cada suite/test se ejecuta bajo un envelope privado con node ID,
  logs/streams, cancelación y límites. El enlace sigue frames, helpers, closures
  y tasks estructuradas, nunca se expone como valor ni se obtiene mediante un
  thread-local.
- Skip es cooperativo, exige razón y solo se confirma después de cleanup. Una
  suite omitida produce `blocked-skip`; un fallo de cleanup prevalece y produce
  `blocked-setup`. `P2001` impide omitir desde defer/unwind/teardown y
  `--deny-skips` permite convertir skips en exit `1` sin falsificar estados.
- Los schemas incompatibles suben a `tondo-test-report-0.2/3` y
  `tondo-test-list-0.2/3`, con policy deny-skips, `skip`, logs y contadores de
  skipped/blocked-skip. La conformidad mínima pasa de 24 a 29 grupos.
- M10.6 añade UTEST-CONTROL-001 y amplía frontend, runtime, lifecycle, límites,
  CLI, scheduling, reporte, aceptación, plataformas y Gate T0. STD-0.1 reutiliza
  el control ya ejecutable de T0 y solo amplía `std.testing`.

### 0.85 — 2026-07-28

- La extensión de testing sube a `0.2-draft.2`: `suite` es un contenedor
  estático y léxico; `test` permanece exclusivamente como hoja ejecutable.
  Suites pueden anidarse, ejecutan setup una vez solo para descendientes
  seleccionados y hacen teardown por `defer` después de todos ellos.
- Se fijan capturas ancestrales `let: Copy + Send + Share`, prohibición de
  estado mutable/afín compartido, IDs jerárquicos, exact match de suite como
  selección de subárbol y ausencia de subtests dinámicos o lifecycle hooks.
- Los fallos de setup y teardown quedan como resultados propios de la suite; un
  fallo de setup produce `blocked-setup` sin duplicar la causa y un fallo de
  teardown conserva los resultados de las hojas. Timeouts se aplican por hoja y
  fase activa, no a la espera de descendientes.
- Los schemas incompatibles se versionan como `tondo-test-report-0.2/2` y
  `tondo-test-list-0.2/2`, con arrays separados de suites/tests, parent/path,
  phase, `blocked_by` e invariantes de summary.
- M10.6 añade comprobación de capturas y lifecycle jerárquico a frontend,
  lowering, runtime, scheduling, conformidad, aceptación, plataformas y Gate
  T0. La ruta inmediata continúa empezando en TEST-001 y Gate H0.

### 0.84 — 2026-07-28

- Se crea `TONDO_TESTING_SPEC.md` como extensión normativa de la edición 0.2:
  una única declaración `test name { ... }`, inferencia local de error y async,
  unit overlays privados, integration roots públicos, aislamiento, límites,
  selección, output, reporte `tondo-test-report-0.2/1` y listado
  `tondo-test-list-0.2/1`.
- Tondo 0.1 y `tondo-conformance-0.1` permanecen inmutables. `test` se reserva
  únicamente en 0.2 y la implementación debe conservar ambas ediciones sin
  aceptar extensiones silenciosas.
- Se añade M10.6 y Gate T0 después de H0 y antes de STD-0.1. El milestone cubre
  edición, project plan, lexer/CST/formatter, semántica, HIR/MIR/bytecode,
  runtime, CLI, reporte, conformidad nueva, plataformas y dogfooding.
- STD-0.1 incorpora la especificación e implementación de `std.testing` sobre
  el runner público. M11 pasa a depender de H0, T0 y S1 y debe conservar los
  resultados de test en su oracle diferencial.

### 0.83 — 2026-07-28

- Una auditoría separa los 685 tests Rust de casos, repeticiones y fuentes
  independientes: registra 129 fixtures internos, 205 casos/424 repeticiones de
  conformidad, 302 fences y 203 fuentes `.to` únicas tras descontar 127
  duplicados exactos.
- Se inserta M10.5 como milestone acotado de fiabilidad. Gate H0 exige
  inventario, trazabilidad normativa, CI completo, properties, fuzzing,
  modelos, coverage, mutation score y reproducción de fallos antes de ampliar
  la superficie pública.
- Se crea STD-0.1 para Core + Hosted Standard Library con spec independiente,
  capabilities, distribución reproducible, implementación VM, modelos y
  conformidad. El checkpoint Tondo 0.1.0 existente no se reescribe.
- M11 conserva su ID histórico, pero NATIVE-001 depende ahora de H0 y S1. La VM
  y STD-0.1 se convierten en oracle y corpus diferencial del backend nativo.
- Concurrency + Application Standard Library se separa como STD-0.2 después de
  Gate N1 para impedir que APIs no esenciales bloqueen la implementación
  nativa.
- La cola inmediata comienza en TEST-001; este cambio reorganiza trabajo y no
  modifica semántica, compilador, runtime ni la evidencia de conformidad
  registrada.

### 0.82 — 2026-07-28

- Se cierra M10 y Gate G5 con `tondo-conformance-0.1` versión `0.1.0`: 205
  casos distribuidos en los diez grupos obligatorios y 424 repeticiones
  completas para `tondo-vm-hosted` / `hosted` / `[console, process]`.
- El manifiesto
  `67f12434001d5d9d17b0f2181afe3ec38cb07d6207e431cca164ec4854f0148b`
  fija todos los inputs. Dos ejecuciones completas producen bytes idénticos y
  el resultado
  `d44e8eb853ccdc208b8a8ea044ddd2222a7e5ef148e91edc7c08ebec17425693`.
- Quedan cubiertos los 78 errores con vecino positivo, las once clases de
  pánico, los nueve warnings `core`, formato idempotente, queries semánticas,
  runtime, 32 repeticiones por litmus concurrente, host, los cuatro escenarios
  privados de memoria, determinismo con orden inverso y documentación
  normativa.
- Diagnostics JSON 0.1 queda congelado con schema, IDs, ranges, children,
  fixes y orden exactos. El perfil `core` solo añade warnings; un test verifica
  que nunca relaja errores. El feature privado de conformidad no introduce un
  dialecto ni cambia la ruta pública.
- La revisión final corrige dos dependencias ambientales descubiertas por la
  puerta completa: los fixtures declaran su warning profile mediante sidecar y
  el spec queda anclado a la raíz real del workspace, con una regresión que
  impide aceptar copias externas accidentales.
- Formatter, check, Clippy con warnings denegados, 685 tests y Rustdoc pasan con
  Rust/Cargo 1.93.0. El estado se etiqueta internamente como `v0.1.0`; no es una
  publicación de Tondo y M11 queda fuera de ese checkpoint.

### 0.81 — 2026-07-28

- M10 queda en curso con un corpus portátil de 202 casos y un manifiesto
  canónico que fija fuentes, expectativas, spec, fixtures y registry por
  SHA-256. Quedan cerrados CONF-001 a CONF-006, la cobertura primaria/vecina de
  los 78 errores, el perfil `core`, las once clases de pánico y la idempotencia
  byte a byte del formatter.
- `semantic-queries` publica schemas 0.1 cerrados para tipos, símbolos,
  referencias, firmas, cierres, opacos, iteradores, capacidades, terminales,
  borrows, loans, checks dinámicos, estados afines, `Join`, `unsafe`, azúcar y
  AST formateada. Los IDs request-local nunca aparecen en el wire format.
- El runner valida cardinalidad, tags, keys, IDs, spans y orden, y reaplica
  cualquier fix `safe` sobre el snapshot exacto antes de exigir un segundo
  check sin errores.
- La consulta MIR descubrió y corrigió una regresión de refinamiento de tags:
  un loan regional es metadata de acceso y no una identidad distinta para el
  discriminante de `Enum.Variant(ref payload)`.
- Los cuatro casos semánticos, sus regresiones focalizadas y Clippy de los
  crates afectados pasan. CONF-007 y la auditoría de los grupos ejecutables son
  el siguiente límite; G5 continúa abierto.

### 0.80 — 2026-07-28

- Se completa M9 con funciones, closures y bloques `unsafe` que conservan su
  efecto hasta la VM. Las seis operaciones raw tienen formas dot y calificadas,
  tipos y aridades cerrados, verificación independiente y una lista exhaustiva
  de UB; las capturas seguras de `Pointer` y el uso raw fuera de región fallan
  con `E1702` y `E1701`.
- El target `tondo-vm-hosted` publica un registro versionado y únicamente las
  capacidades `console` y `process`. Source sets condicionados se resuelven
  antes del lexer y una API estándar ausente produce `E1008`.
- Se implementa un plan de proyecto puro con manifiesto y lockfile estrictos,
  PackageIds y aliases exactos, hashes SHA-256, resolución offline, inputs de
  generador declarados y unidades privilegiadas canónicas.
- Interfaces y artefactos versionados fijan identidad de compilador, edición,
  target, perfil, capacidades, features, source sets, módulos, API pública y
  dependencias transitivas. Sus bytes son canónicos, los builds son
  deterministas y el `build_hash` se vuelve a derivar al admitir un artefacto.
- La CLI admite proyectos cerrados y productos canónicos sin sobrescribir
  manifiesto, lockfile, fuentes, interfaces de dependencia, inputs de generador,
  unidades privilegiadas ni otro producto, incluso mediante aliases de path.
- La puerta G4 pasa con 659 tests, `git diff --check`, formatter check,
  `cargo check` para todos los targets, Clippy con warnings denegados y Rustdoc
  con warnings denegados. M10 permanece sin iniciar; CONF-001 es el siguiente
  límite de trabajo.

### 0.79 — 2026-07-28

- Se completa M8 con scripts raíz de sentencias top-level, `main` privado
  implícito, inferencia cerrada de errores, promoción async contextual y
  shebang exclusivo del root. Los scripts no pueden importarse ni mezclarse
  con un `main` explícito.
- `std.process` queda cerrado por capacidad de target. `Command` y `Pipeline`
  son planes inertes; `|` admite exactamente sus cuatro combinaciones y
  `cmd` conserva argv sin parsing, expansión ni shell implícito. El shell solo
  se alcanza mediante `process.shell`.
- Las operaciones terminales `start`, `status`, `output`, `run`, `check` y
  `cancel` usan `ProcessHandle` afín, resultados nominales y bytes con
  decodificación UTF-8 estricta. `check` conserva los estados por etapa y trata
  el cierre esperado de un pipe por un consumidor satisfactorio sin ocultar
  fallos posteriores.
- El host conecta pipes del sistema con backpressure, drena stdout y stderr
  concurrentemente y ejecuta waits bloqueantes fuera del executor cooperativo.
  Cancelación, pánico, unwind y destrucción del host convergen en terminación y
  reap idempotentes; los grupos de procesos Unix incluyen descendientes de un
  shell explícito.
- La CLI expone exactamente los argumentos posteriores a `--` mediante
  `process.args()`. Fixtures de runtime, compile-fail y tests directos cubren
  argv literal, las cuatro formas de pipe, procesos inexistentes, exit status,
  discriminación de errores, backpressure, cancelación, pánico, cleanup y el
  ejemplo normativo 24.17.
- La puerta acumulada pasa con 635 tests, formatter check, `cargo check` para
  todos los targets, Clippy con warnings denegados y Rustdoc con warnings
  denegados. M9 queda expresamente fuera de esta entrega; UNSAFE-001 es el
  siguiente límite de trabajo.

### 0.78 — 2026-07-28

- Se completa M7 con llamadas async explícitamente iniciadas, cierres async,
  liveness `Send`, fronteras de loans y lowering verificado a `Await`, `Spawn`,
  `EnterTaskScope` y `DrainScopes` en MIR y bytecode.
- La VM incorpora un executor cooperativo monohilo con cola idempotente, frames
  suspendibles, `async fn main`, scopes propietarios y `Join[T, E]` afín sin
  exponer un wrapper `Task` en las firmas.
- Cancelación continúa siendo un canal interno distinto de `E`. Salidas no
  locales y pánicos cancelan hijos, esperan todos sus cleanups y propagan el
  pánico primario por orden de creación después de recoger los suprimidos.
- Los préstamos estructurados de `spawn` exigen `Send + Share`, permanecen
  activos mientras exista su `Join` y se liberan exactamente al consumirlo o
  durante teardown. Los roots incluyen frames aparcados y resultados de hijos
  completados.
- Fixtures positivos, negativos y runtime cubren `E1601`, `E1602`, `E1603` y
  `E1605` a `E1611`, no escape y consumo de handles, progreso, wakeups
  duplicados, cancelación, pánicos, closures async, GC bajo presión y
  scheduling no observable. M8 queda expresamente fuera de esta entrega;
  SCRIPT-001 es el siguiente límite de trabajo.
- La puerta acumulada pasa con 620 tests, formatter check, `cargo check` para
  todos los targets, Clippy con warnings denegados y Rustdoc con warnings
  denegados.

### 0.77 — 2026-07-28

- Se cierra VARIADIC-002. El spread conserva `Array[T]` hasta runtime, copia el
  owner completo solo cuando `T: Copy`, mueve el array afín en caso contrario y
  transfiere sus elementos al pack sin una segunda copia lógica. Fixtures
  cubren forma nombrada, métodos, funciones indirectas, closures, genéricos,
  orden, mutación independiente, owner afín, bytecode adversarial y GC.
- OPT-COW-001 mide 195 copias lógicas sobre workloads source-to-VM de Array,
  Map y Set: eager recorre 33.280 elementos superiores. OPT-COW-002 adopta
  buffers compartidos conservadores y detachment verificado por `is_unique`,
  reduciendo esos recorridos a cero sin cambiar el bound lógico de memoria.
- OPT-COW-003 conserva eager como referencia ejecutable y compara contra COW
  las ocho pruebas black-box de semántica de valor, tanto con GC normal como
  con umbral uno. Se añaden contadores de trabajo internos sin convertirlos en
  observables del lenguaje.
- El fixture integrado ejecuta los ejemplos puros 24.3, 24.5, 24.7, 24.8,
  24.13 y 24.15. Los restantes ejemplos síncronos de G3 quedan clasificados por
  sus dependencias explícitas de core/hosted stdlib. M4, M5, M6 y Gate G3 pasan
  a completados; ASYNC-001 es el siguiente límite de trabajo.

### 0.76 — 2026-07-28

- Se cierra VARIADIC-001 con un único parámetro final homogéneo `...T`. La firma
  retiene el elemento y el body recibe exactamente un `Array[T]` inmutable,
  incluido un array vacío cuando la llamada no proporciona elementos.
- HIR conserva asociación y orden textual; MIR y bytecode retienen para cada
  elemento su tipo, modo por valor y acceso Copy o Move. La VM materializa un
  array nuevo, mantiene elementos y publicación pendientes como raíces y usa
  la misma ruta para llamadas directas, métodos, closures, genéricos e
  indirectas.
- Los fixtures ejecutan packs vacíos y poblados, prefijo fijo, inferencia,
  métodos, closures explícitas y contextuales, función nombrada uniforme,
  efectos ordenados, valores gestionados y elementos afines `CallOnce`.
  También fijan heterogeneidad como `E1102`, pack sin nombre como `E1115`,
  mutación del binding como `E1411`, reutilización tras move como `E1401`,
  bytecode adversarial y observables idénticos con GC desde la primera
  asignación.
- El spread explícito permanece abierto como VARIADIC-002 porque todavía debe
  cerrar la transferencia completa de un `Array[T]` afín sin copiar sus
  elementos.

### 0.75 — 2026-07-26

- Se cierra TEXT-003 con interpolación normal y multilínea ejecutable. Los
  segmentos se dedentan antes de decodificar escapes y llaves duplicadas, y
  cada hueco completa su conversión `Display` antes de evaluar el siguiente.
- `Display.display(ref T): String` conserva selección estática: escalares y
  `String` cierran a una operación intrínseca de bytecode; los tipos de usuario
  monomorfizan hacia su implementación concreta, sin trait objects, reflection
  ni type packs.
- HIR, MIR y bytecode verifican tipos, aridad de segmentos, asociación del
  receptor y préstamo compartido. La VM consume ese préstamo, mantiene
  temporales vivos, preflights el tamaño del resultado y publica un único
  `String` UTF-8 de forma atómica.
- Los fixtures cubren formatos escalares, `Display` explícito y genérico,
  temporales, preservación del valor observado, orden de efectos, escapes,
  llaves, dedentación, `E1105`, bytecode adversarial y GC con umbral inicial
  uno.

### 0.74 — 2026-07-26

- Se cierra TEXT-002 con `String[Int]: Char` y slicing
  `String[start:end:step]: String` en HIR, MIR, bytecode verificado, evaluación
  constante y VM.
- El acceso cuenta valores escalares Unicode y reutiliza el normalizador
  matemático de arrays para índices negativos, límites omitidos, clipping,
  strides, `Int.min`, `Int.max`, `P0001` y `P0002`.
- El HIR y ambos verificadores impiden que un índice o slice de String forme una
  ubicación, préstamo o destino de escritura; la inmutabilidad no depende de
  que el frontend haya construido bytecode honesto.
- El `Length` interno admite Array o String y se prueba con texto multibyte; el
  nombre de la futura API pública sigue reservado a la especificación de
  stdlib.

### 0.73 — 2026-07-26

- Se cierra TEXT-001 conservando `String` como objeto gestionado UTF-8
  inmutable, con igualdad y orden exactos sin normalización automática,
  pertenencia por `Char` e iteración escalar en tiempo lineal.
- El cursor de texto avanza con un offset interno que siempre cae en una
  frontera UTF-8; ese detalle no cruza el bytecode ni es observable desde
  fuente.
- El verificador vuelve a decodificar literales inmediatos `String` y `Char` y
  rechaza delimitadores, escapes, escalares o sustitutos Unicode malformados
  antes de ejecutar una instrucción.
- Se cierra TEXT-004 manteniendo separados `String`, `Char`, `Byte` y
  `Array[Byte]`, sin reservar una representación intrínseca para el futuro
  `Bytes` de la stdlib.

### 0.72 — 2026-07-26

- Se cierra NUM-004 ejecutando `Float32` con operaciones binarias reales de
  precisión simple y `Float64` con precisión doble, redondeando en cada frontera
  semántica sin contracción FMA.
- Constantes nombradas y literales flotantes se verifican contra su precisión.
  El pool usa bits canónicos de `f64` como envoltura: todo `Float32` no-NaN debe
  ser la ampliación exacta de un binario32; el payload concreto de NaN permanece
  no normativo.
- La aceptación compara evaluación constante y runtime para ties-to-even,
  gradual underflow, overflow a infinito, NaN, cero con signo y expresiones
  deliberadamente sensibles a FMA en ambas precisiones.
- Los valores `Float32` entregados por el bootstrap host se normalizan en la
  frontera antes de entrar en el grafo de valores de la VM.

### 0.71 — 2026-07-26

- Se cierra NUM-003 corrigiendo `<<` para descartar bits altos en cada ancho,
  manteniendo `>>` aritmético o lógico según signo y reservando `P0010`
  exclusivamente para conteos inválidos.
- Aritmética, división, resto, complemento, bitwise y asignaciones compuestas se
  prueban en fuente y VM. La matriz de bordes cubre todos los anchos, overflow
  firmado/sin signo, cero, mínimo dividido por `-1` y su resto representable.
- La aceptación negativa conserva `P0003`, `P0005`, `P0010` y rechaza dominios
  de operador fuera del conjunto cerrado antes de generar bytecode.
- Las constantes enteras se vuelven a validar contra su ancho en la frontera de
  bytecode; una constante `Byte` se materializa como byte y no como `Int`.

### 0.70 — 2026-07-26

- Se cierra NUM-002 con la matriz exhaustiva de 121 parejas numéricas, incluyendo
  identidades canónicas, conversiones totales, conversiones comprobadas y
  rechazo cerrado de cualquier escalar no numérico.
- Se cierra NUM-005 con `NumericConversionError` como unión intrínseca cerrada:
  sus tres valores y patrones atraviesan HIR, MIR, bytecode verificado, VM y
  evaluación constante sin depender de metadata nominal inventada.
- La aceptación pública cubre `Byte`, propagación con `?`, límites, NaN,
  infinito, los tres errores, constantes `ok`/`err`, variantes desconocidas y
  exhaustividad. El verificador rechaza discriminantes o payloads forjados.

### 0.69 — 2026-07-26

- Se cierra NUM-001 sobre una única representación canónica por ancho: `Int64`
  comparte identidad con `Int` y `Float64` con `Float`; los demás enteros y
  `Float32` conservan tipos distintos a través de todo el pipeline.
- La aceptación pública cubre mínimos y máximos de cada entero, ambos floats,
  sufijos, inferencia por tipo esperado y los aliases canónicos. Los rechazos
  cubren overflow literal, overflow contextual y ausencia de promoción implícita.

### 0.68 — 2026-07-26

- Se cierra ITER-001 con selección estática del `Iterator[T]` único por target,
  receptor `mut`, discriminación explícita de `Option[T]` y ejecución real de
  un cursor nominal. Una regresión pública conserva `E1113` para dos elementos
  incompatibles sobre el mismo target.
- Se cierra ITER-002 con un único lowering para `for`, `for ref`, `for mut` y
  `for var`. El patrón selecciona el cursor own/ref/mut y conserva permisos
  exactos por hoja; las fuentes compartidas y exclusivas siguen una matriz
  cerrada, se evalúan una vez y liberan sus regiones en agotamiento y toda salida.
- La VM escribe directamente sobre elementos de array y sobre el valor real de
  cada entrada de map, nunca sobre la tuple efímera del elemento. `mut` valida
  extensión fija y `var` permite reemplazo; claves, cardinalidad y orden del map
  permanecen inmutables durante el recorrido.
- Las regresiones cubren source checking, modos y capacidades, coherencia HIR,
  regiones y posiciones MIR, bytecode adversarial, arrays anidados, patterns
  mixtos, maps, reborrows, `break`, `continue`, `return`, acceso al owner y
  liberación posterior.
- El gate acumulado pasa 591 tests: 521 unitarios del compilador, 27 de la VM y
  43 de CLI/integración. También pasan formatter check, compilación de todos los
  targets, Clippy con warnings como errores y rustdoc estricto, con compilación
  incremental desactivada por el ICE conocido de Rust 1.93.

### 0.67 — 2026-07-26

- Se cierran SET-001 y RANGE-001 sobre las representaciones ya verificadas en
  HIR, MIR, bytecode y VM. Set conserva el primer orden de inserción, deduplica
  entradas constantes y dinámicas, consulta pertenencia con la misma igualdad
  de `Key` y compara contenido independientemente del orden.
- Range conserva extremos y clase inclusiva/exclusiva sin materialización. Los
  cursores terminan al emitir un máximo inclusivo, por lo que no fabrican un
  sucesor desbordado; `Char` avanza por escalares, salta `D800...DFFF` y admite
  `U+10FFFF` como último valor.
- El corpus público añade casos end-to-end de orden, deduplicación dinámica,
  `W1011`, pertenencia, igualdad, ranges vacíos, `Int.min/max`, `UInt64.max`,
  hueco surrogate y máximo Unicode, además de rechazos de `Set` sin `Key` y
  `Range[Byte]`.
- El gate acumulado continúa en 587 tests y pasa formatter check, compilación
  de todos los targets, Clippy con warnings como errores, rustdoc estricto y la
  suite workspace locked con compilación incremental desactivada.

### 0.66 — 2026-07-26

- Se cierran MAP-001..MAP-003 sobre una representación ordenada explícita:
  inserción nueva añade al final, reemplazo conserva posición, eliminación
  conserva el orden relativo y reinserción vuelve a añadir al final. Igualdad
  compara contenido independientemente de ese orden observable.
- `Map.remove` pasa a ser una operación intrínseca cerrada con firma
  `Map[K, V].remove(var self, key: K): V?`. La forma calificada exige
  `Map.remove(var map, key)`, infiere los tipos del receptor y rechaza listas de
  tipos en el owner. La ausencia devuelve `none`; un valor presente se
  transfiere sin requerir `V: Copy`.
- HIR reserva el receptor `var` antes de evaluar la clave. MIR lo conserva como
  un `MapRemove` no panicking sobre una región exacta y bytecode vuelve a
  verificar map, clave, resultado, modo y lender. La VM enraíza el valor
  extraído durante reemplazo del objeto y construcción de `some`.
- El corpus black-box cubre lookup, inserción, reemplazo, eliminación ausente y
  presente, reinserción, ambas formas de llamada, genéricos sin `Copy`, orden e
  igualdad. Un fixture separado repite la transferencia bajo GC forzado; tests
  de mutación de IR prueban que MIR y bytecode rechazan degradar `var` a `ref`.
- El gate acumulado queda en 587 tests; también pasan formatter check,
  compilación de todos los targets, Clippy con warnings como errores y rustdoc
  estricto, siempre con compilación incremental desactivada por el ICE conocido
  de Rust 1.93.

### 0.65 — 2026-07-26

- Se cierra ARRAY-007 con una única superficie canónica:
  `Array[T: Copy].concat(self, other)` y `repeat(self, count)`. `+` y `*`
  conservan exclusivamente su semántica numérica; las formas calificadas
  conservan el receptor `self` sin modificador como
  `Array.concat(left, right)` y `Array.repeat(values, count)`.
- Typed HIR registra `ArraySequence { kind, array, argument }`, observa el
  receptor, transfiere el argumento `Copy` y rechaza tipos sin `T: Copy`.
  MIR y bytecode retienen el orden receptor-argumento en un `Invoke` checked;
  ambos verificadores vuelven a derivar kind, firma, capacidad y modo
  `Borrow`, por lo que una mutación del IR no puede convertirlo en otra
  operación.
- La VM valida la longitud matemática antes de copiar: un conteo negativo
  produce `P0011`, una longitud fuera de `Int` produce `P0005` y una reserva
  físicamente imposible permanece agotamiento de recursos. Repetir un array
  vacío con cualquier conteo no negativo termina inmediatamente.
- El fixture `m6-array-007-sequences` prueba concat, repeat, formas calificadas,
  receptor `ref`, cero, vacío con `Int.max`, independencia de arrays anidados e
  identidad `Ref`. Fixtures separados fijan `P0011`, `P0005`, el bound `Copy` y
  el modo explícito del receptor; las pruebas internas añaden precedencia de
  evaluación, GC con umbral uno y HIR/MIR/bytecode adversarial.
- La puerta integral queda verde con Rust incremental desactivado: formato,
  `cargo check`, build, Clippy con warnings como errores, rustdoc con warnings
  como errores y 584 pruebas de todos los targets (514 del compilador, 27 de la
  VM, 12 de CLI y 31 de integración/especificación), además de
  `git diff --check`.

### 0.64 — 2026-07-26

- Se cierra ARRAY-006 sin introducir operadores ni tipos auxiliares. La
  expectativa interna `ArithmeticPeer` fija una única hoja numérica y acepta
  cualquier forma array compatible; variables y llamadas reciben la misma
  inferencia que los literales y un contexto `Array[Int32]` tipa sus hojas sin
  conversión implícita.
- MIR y bytecode clasifican una operación aritmética como checked cuando
  cualquiera de sus operandos es `Array`. Sus verificadores rechazan una
  codificación pure forjada, incluida la forma escalar-array con escalar
  izquierdo `Float`.
- La VM realiza un preflight recursivo completo antes de la primera hoja. El
  fixture `m6-array-006-shape-preflight` coloca deliberadamente un overflow
  antes de una incompatibilidad profunda y observa `P0006`, fijando la
  precedencia y la garantía fuerte de las variantes in-place. El evaluador
  constante aplica el mismo preflight mediante un worklist iterativo antes de
  calcular hojas.
- El fixture `m6-array-006-arithmetic` cubre los cinco operadores enteros,
  ambos sentidos de broadcasting escalar, arrays anidados, `Float`, `Float32`
  y las cinco asignaciones compuestas. La ejecución interna repite arrays
  anidados bajo un umbral inicial de GC igual a uno.
- La puerta completa pasa con 578 tests —508 del núcleo del compilador, 27 de
  la VM y 43 de CLI/integración—, formatter canónico de los nuevos fixtures,
  `cargo fmt --check`, `cargo check`, `cargo build`, Clippy y rustdoc con
  warnings como error, y `git diff --check`.

### 0.63 — 2026-07-26

- Se cierra ARRAY-005 reutilizando una sola jerarquía de permisos:
  `ref < mut < var`. No se añade una segunda clase de array, un tipo slice ni
  sintaxis estructural paralela.
- El fixture público `m6-array-005-mutation` demuestra escritura de longitud
  fija sobre propietario y slice, reemplazo `var` que cambia longitud y
  reborrow `var` de un elemento completo dentro de un `mut Array[Array[T]]`.
- El fixture compile-fail `m6-array-005-permissions` fija `E1411` para el
  reemplazo raíz de un `mut Array[T]` y `E1407` para intentar prestar un slice
  como `var`.
- La VM ejecuta `ensure_mut_array_extent` antes de toda escritura raíz a través
  de un préstamo `mut Array[T]`. El reemplazo permanece en la pila temporal de
  roots mientras una región pueda materializarse; una longitud distinta
  produce un error de invariante antes de cualquier escritura.
- Una mutación adversarial de bytecode sustituye el resultado in-place por un
  `Array` de otra longitud: el programa continúa siendo tipado y verificable,
  pero la defensa runtime impide publicar el cambio. El mismo camino pasa bajo
  umbral inicial de GC igual a uno.
- La puerta completa pasa con 575 tests —505 del núcleo del compilador, 27 de
  la VM y 43 de CLI/integración—, formatter canónico de los nuevos fixtures,
  `cargo fmt --check`, `cargo check`, `cargo build`, Clippy y rustdoc con
  warnings como error, y `git diff --check`.

### 0.62 — 2026-07-25

- Se cierra ARRAY-004 con una sola rutina de snapshot lógico para slice directo
  y materialización por un parámetro `ref`. La implementación eager copia cada
  elemento mediante el copiador exhaustivo ya fijado por ADR-011, sin exponer
  storage, handles, refcounts ni una decisión de COW.
- El fixture estable `tests/runtime/value-copy/slice-snapshot.to` prueba
  separación tras escrituras en ambas direcciones, elementos anidados,
  asignación solapada, identidad `Ref`, materialización a través de préstamo y
  mutación de la región original mediante `mut`.
- El mismo fixture se ejecuta con límites ordinarios y con umbral inicial de GC
  igual a uno; ambas ejecuciones producen la misma observación completa del
  driver y los mismos sidecars.
- MIR y bytecode vuelven a demostrar `Array[T]: Copy` en toda operación Slice
  que materializa otro propietario. Una mutación adversarial a `Array[Join[…]]`
  es rechazada, mientras la proyección `ref values[:]` equivalente permanece
  válida porque solo reserva una región.
- La puerta completa pasa con 574 tests —504 del núcleo del compilador, 27 de
  la VM y 43 de CLI/integración—, formatter canónico del nuevo fixture,
  `cargo fmt --check`, `cargo check`, `cargo build`, Clippy y rustdoc con
  warnings como error, y `git diff --check`.

### 0.61 — 2026-07-25

- Se cierra ARRAY-003 con una única función
  `normalize_array_slice_indices` compartida por evaluación constante y VM.
  Los defaults dependen del signo del paso; un end omitido conserva su
  sentinel estructural y sigue siendo distinto de un `-1` explícito.
- La matriz cubre slices completos y parciales, clipping, pasos positivos y
  negativos, `[::-1]`, `[:-1:-1]`, array vacío, ambos extremos de `Int` y
  `Int.min` como paso. Paso cero conserva `P0002` y una mutación adversarial a
  un bound `Bool` es rechazada por el verificador antes de ejecutar.
- HIR exige `Int` independientemente para start/end/step. Los slices almacenados
  siguen requiriendo `T: Copy`, mientras `ref`/`mut` conserva una región sobre
  el array original; MIR y bytecode reutilizan los operandos ya evaluados sin
  normalizarlos prematuramente.
- Los fixtures públicos `m6-array-003-slice.to` y
  `m6-array-003-zero-step.to` fijan el resultado observable y el pánico. Ambos
  son puntos fijos del formatter.
- La puerta completa pasa con 573 tests —503 del núcleo del compilador, 27 de
  la VM y 43 de CLI/integración—, `cargo fmt --check`, `cargo check`,
  `cargo build`, Clippy y rustdoc con warnings como error, y
  `git diff --check`.

### 0.60 — 2026-07-25

- Se cierra ARRAY-002 con una única función `normalize_array_index` compartida
  por evaluación constante y todos los caminos del VM. La transformación usa
  distancia desde el final y no puede desbordar al simular `n + i`.
- La matriz source-to-VM cubre lectura, escritura y préstamo con `0`, `n - 1`,
  `-1` y `-n`; vacío, `n`, `-n - 1`, `Int.min` e `Int.max` producen
  `P0001`. Una escritura simple demuestra además que el RHS se completa antes
  de validar bounds.
- El chequeo semántico exige `Int`, solo materializa `T: Copy` y permite
  observar un elemento afín mediante préstamo. Una mutación adversarial del
  operando a `Bool` queda rechazada por el verificador de bytecode.
- Cinco fixtures públicos fijan el caso válido, bounds positivo, negativo y
  extremo, además del orden RHS-antes-de-bounds.
- La puerta completa pasa con 569 tests —500 del núcleo del compilador, 26 de
  la VM y 43 de CLI/integración—, formatter canónico de todos los fixtures,
  `cargo fmt --check`, `cargo check`, `cargo build`, Clippy y rustdoc con
  warnings como error, y `git diff --check`.

### 0.59 — 2026-07-25

- Se cierra ARRAY-001 sin introducir prematuramente una API de stdlib. Un
  `Array[T]` no codifica longitud estática; el valor conserva el recuento
  runtime a través de construcción, copia, llamadas y retornos.
- El contrato interno `Length(Array[T]) : Int` sirve a los patterns y queda
  rederivado tanto por MIR como por el verificador de bytecode. Una mutación
  adversarial sustituye el operando por `Int` y se rechaza antes de ejecutar.
- El fixture `m6-array-001-runtime-length.to` observa nueve casos de longitud
  bajo un único `Array[Int]`, incluidos vacío, copia y cruces de función, usando
  únicamente semántica pública.
- La puerta completa pasa con 566 tests —498 del núcleo del compilador, 25 de
  la VM y 43 de CLI/integración—, formatter canónico del fixture,
  `cargo fmt --check`, `cargo check`, `cargo build`, Clippy y rustdoc con
  warnings como error, y `git diff --check`.

### 0.58 — 2026-07-25

- Se cierra VALUE-002 con seis fixtures públicos e independientes para valor,
  separación tras escritura, identidad, iteración, pánico y presión de GC.
- `Fixture::run_with_limits` permite ejecutar exactamente la misma fuente y los
  mismos sidecars bajo perfiles distintos. La comparación usa la observación
  completa del driver y excluye deliberadamente toda estadística o identidad
  física de la VM.
- El corpus eager pasa también con umbral inicial de GC igual a uno sin alterar
  los límites ordinarios de objetos o bytes. Una futura implementación COW
  deberá ejecutar los fixtures sin modificarlos y producir el mismo oráculo.
- La puerta completa pasa con 565 tests —497 del núcleo del compilador, 25 de
  la VM y 43 de CLI/integración—, formatter canónico de todos los fixtures,
  `cargo fmt --check`, `cargo check`, `cargo build`, Clippy y rustdoc con
  warnings como error, y `git diff --check`.

### 0.57 — 2026-07-25

- Se cierra VALUE-001 sobre el walker eager ya construido por las verticales de
  ownership y GC. Todas las formas administradas `Copy` se duplican
  recursivamente bajo roots precisos; String y `Ref[T]` conservan sus dos únicas
  reglas explícitas de sharing.
- Una matriz source-to-VM compara construcción y copia para tuple, array, map,
  set, closure, newtype, record, los tres payloads de enum, Option, Result,
  union, range, String, `Ref` y agregados anidados. Mutaciones posteriores
  prueban separación de tuple/array, record, newtype y map; el cursor own prueba
  además copia independiente de su source y posición.
- La puerta completa pasa con 564 tests —497 del compilador, 25 de la VM y 42
  de CLI/integración—, `cargo fmt --check`, `cargo check`, `cargo build`,
  Clippy con warnings como error, rustdoc con warnings como error y
  `git diff --check`.

### 0.56 — 2026-07-25

- Se completan REF-001 y REF-002 como una vertical slice única. El frontend
  reconoce construcción explícita, contextual e inferida, exige `Discard`,
  rechaza identidad constante y conserva `.value` como place compartido de
  solo lectura.
- MIR y bytecode incorporan agregados y proyecciones nominalmente separados.
  Sus verificadores rechazan aridad/tipo falsos, move del payload, escritura y
  préstamos `mut`/`var`, de modo que un programa manipulado no puede convertir
  identidad segura en alias mutable.
- La VM asigna una sola celda, copia únicamente su handle y traza el payload
  durante presión real de GC. Igualdad, Map y Set observan identidad aun cuando
  el contenido es una función sin `Equatable` ni `Key`.
- La puerta completa pasa con 562 tests —496 del compilador, 24 de la VM y 42
  de CLI/integración—, `cargo fmt --check`, `cargo check`, `cargo build`,
  Clippy con warnings como error, rustdoc con warnings como error y
  `git diff --check`.

### 0.55 — 2026-07-25

- Se completa GC-004 con una puerta única de capacidad comprobada para límites
  por objetos y bytes. Una petición realiza como máximo una colección completa,
  reevalúa capacidad y publica una vez o devuelve OOM.
- Allocation mantiene el objeto pendiente fuera del heap y solo incrementa
  estadísticas tras publicarlo. Replacement protege su propio target durante
  GC; un éxito recupera garbage antes de crecer y un OOM conserva descriptor,
  generation, payload y bytes del slot anteriores.
- Las regresiones cubren éxito recuperable y agotamiento real en ambos límites,
  cardinalidad exacta de colección y atomicidad de replacement.
- La puerta completa pasa con 554 tests —488 del compilador, 24 de la VM y 42
  de CLI/integración—, `cargo fmt --check`, `cargo check`, `cargo build`,
  Clippy con warnings como error, rustdoc con warnings como error y
  `git diff --check`.

### 0.54 — 2026-07-25

- Se completa GC-003 mediante el adaptador privado requerido por la
  especificación, conectado al `Heap` y al collector distribuidos, sin un modo
  alternativo ni una operación fuente oculta.
- Un ciclo mixto atraviesa una celda `Ref`, una colección y un environment de
  closure. Un root conserva sus tres nodos durante presión sostenida, mientras
  32 ciclos independientes se recuperan; retirar el root hace recuperable el
  original.
- Esta es la frontera honesta anterior a REF-001: values y captures actuales no
  pueden crear back-edges por fuente. La futura identidad pública reutilizará
  esta misma ruta de trazado.
- La puerta completa pasa con 551 tests —488 del compilador, 21 de la VM y 42
  de CLI/integración—, `cargo fmt --check`, `cargo check`, `cargo build`,
  Clippy con warnings como error, rustdoc con warnings como error y
  `git diff --check`.

### 0.53 — 2026-07-25

- Se completa GC-002 con transiciones explícitas entre frames, cleanups, roots
  temporales, objetos pendientes y edges administrados. Move, muerte de slot,
  retirada de cleanup, pop de frame y cierre de scope retiran cada fuente.
- Constantes y resultados host compuestos, operandos evaluados de izquierda a
  derecha, mapas dinámicos, record updates, copias recursivas, proyecciones,
  slices, aritmética elevada, variádicos y calls conservan cada valor completado
  durante cualquier asignación posterior.
- El walker de fallback publica el owner retirado y todos sus hijos pendientes
  hasta completar o fallar. Environments siguen siendo objetos trazados
  ordinarios; el host solo conserva snapshots detached y los futuros frames
  suspendidos permanecen una frontera explícita de M7.
- La puerta completa pasa con 550 tests —488 del compilador, 20 de la VM y 42
  de CLI/integración—, `cargo fmt --check`, `cargo check`, `cargo build`,
  Clippy con warnings como error, rustdoc con warnings como error y
  `git diff --check`.

### 0.52 — 2026-07-25

- Se completa GC-001 con descriptors de trazado derivados y verificados por la
  VM, independientes de metadata de tracing del compilador.
- Todo objeto administrado conserva el ID de su descriptor; altas, mutaciones,
  copias, host materialization y marking usan la misma forma cerrada y rechazan
  discrepancias antes de publicar estado.
- Environments prueban callable y captures exactos; nominales y sums prueban
  aridad y layout; collections, cursores, `Ref` y witnesses opacos conservan
  todos sus edges administrados.
- Cada función obtiene un descriptor exacto de slots, validado al crear el
  frame y directamente reutilizable cuando M7 introduzca suspensión. El
  registro de esos futuros containers como roots permanece en GC-002/M7.
- La puerta completa pasa con 545 tests —487 del compilador, 16 de la VM y 42
  de CLI/integración—, `cargo fmt --check`, `cargo check`, `cargo build`,
  Clippy con warnings como error, rustdoc con warnings como error y
  `git diff --check`.

### 0.51 — 2026-07-25

- Se completa TERM-005 con una obligación de cardinalidad independiente:
  todo guard terminal explícito sustituye exactamente un fallback y ningún
  fallback puede rearmarse sobre él.
- La VM hace atómica la publicación del cleanup: una captura fallida conserva
  el fallback, una sustitución inválida no muta el ledger y los roots
  temporales se liberan en todas las rutas.
- Las mutaciones de MIR y bytecode prueban el rechazo del doble armado; la
  ejecución cubre retarget, aggregates terminales, handoff a llamada,
  agotamiento natural de iteración, salida normal, pánico y fallo de registro.
- La puerta completa pasa con 541 tests —487 del compilador, 12 de la VM y 42
  de CLI/integración—, `cargo fmt --check`, `cargo check`, `cargo build`,
  Clippy con warnings como error, rustdoc con warnings como error y
  `git diff --check`.

### 0.50 — 2026-07-25

- Se completa TERM-004 con `RegisterFallback` y `DrainUnwind` sobre el mismo
  ledger LIFO de TERM-003. Las transiciones compartidas siguen handoffs y el
  retorno normal elimina únicamente marcadores anormales.
- MIR y bytecode exigen cobertura en parámetros, capturas, stores, resultados
  de llamada y valores de iteración; la especialización elimina fallbacks
  genéricos cerrados como no terminales y rechaza cualquier `Potential`
  ejecutable.
- La VM ejecuta el teardown estructural inverso de tuples, sums, colecciones,
  nominales, closures y cursores, preserva el pánico principal y consulta el
  contrato sellado para raíces directas. El estado activo y suspendible de
  `JoinTeardown` permanece asignado explícitamente a M7.
- La puerta completa pasa con 538 tests —484 del compilador, 12 de la VM y 42
  de CLI/integración—, `cargo fmt --check`, `cargo check`, `cargo build`,
  Clippy con warnings como error, rustdoc con warnings como error y
  `git diff --check`.

### 0.49 — 2026-07-25

- Se completa TERM-003 con acciones `defer` tipadas y estrictamente síncronas,
  captura inmediata de operands `Copy` y un único guard afín completo que puede
  retargetearse o desarmarse únicamente mediante una transferencia confirmada.
- MIR y bytecode incorporan un ledger LIFO explícito y verificadores
  independientes de scope, registro, guard, move, lifetime y drain. Todas las
  salidas normales y de pánico atraviesan los scopes exactos sin abandonar una
  entrada activa.
- La VM captura, enraíza y ejecuta los defers en LIFO; un pánico de cleanup no
  detiene los restantes y se conserva como principal o suprimido según la causa
  previa.
- La iteración intrínseca own transfiere elementos destructivamente y conserva
  el resto en el cursor. El agotamiento natural desarma únicamente el guard de
  una colección terminal; `break` y los demás exits mantienen el remainder, y
  la monomorfización elimina el marker en especializaciones no terminales.
- La puerta completa pasa con 533 tests —479 del compilador y 12 de la VM, más
  42 de CLI e integración—, `cargo fmt --check`, `cargo check`, `cargo build`,
  Clippy con warnings como error, rustdoc con warnings como error y
  `git diff --check`.

### 0.48 — 2026-07-25

- Se completa TERM-002 con un dataflow independiente de disponibilidad afín que
  clasifica cada owner terminal como vivo o reservado y trata `Potential` de
  forma conservadora. Un bound `Discard` elimina la obligación; un resultado
  `Copy` diferido nunca autoriza duplicación.
- La transferencia confirmada cubre argumentos, agregados, closures,
  asignaciones, resultados, `return`, `fail`, `?`, `break` y `continue`. Un
  binding se restaura si el destino no se completa; un temporal ya construido
  conserva su owner y una observación normal que lo abandonaría emite `E1404`.
- Patrones y bucles transfieren obligaciones por componente. Wildcards y
  bindings `ref` de un consumo usan owners ocultos para distinguir salida normal
  de divergencia; el cursor intrínseco propio se desarma solo al agotarse y un
  cursor de trait conserva su contrato terminal.
- Los slots capturados participan en joins y sobrescrituras sin convertirse en
  una obligación nueva por invocación. `E1408` cubre reemplazos de locals,
  capturas, destinos prestados y campos terminales mediante `with`; mover antes
  y reponer completamente un `var` continúa siendo válido.
- Un slice almacenable exige elementos `Copy`, cerrando la duplicación de
  ownership, mientras `ref` y `mut` conservan su modelo de préstamo. HIR vuelve
  a derivar registro y dataflow en admisión; MIR y bytecode preservan los moves
  aceptados, sin inventar aún guards o cleanup. `E1404` queda como diagnóstico
  único de salida terminal normal y elimina el solapamiento histórico `E1604`.
- La puerta completa pasa con 518 tests —464 del compilador y 12 de la VM—,
  `cargo fmt --check`, `cargo check`, `cargo build`, Clippy con warnings como
  error, rustdoc con warnings como error y `git diff --check`.

### 0.47 — 2026-07-25

- Se completa TERM-001 con un registro sellado que distingue la raíz terminal
  intrínseca `Join` de los tipos que solo la contienen estructuralmente. El
  contrato conserva por separado su operación visible `await`, su fallback de
  teardown estructurado y la única excepción de unwind que puede suspender.
- HIR deriva `Absent`, `Potential` o `Present` mediante resúmenes nominales
  simbólicos de punto fijo. Tuples, unions, options, results, colecciones,
  nominales y entornos de closure propagan ownership; `Ref`, `Pointer` y cursores
  prestados no lo adquieren. Un genérico deja de ser potencial únicamente bajo
  un bound que implique `Discard`.
- La tabla queda alineada con el interner y se expone mediante `HirProgram` y
  `SemanticModel`. El admission verifier la recalcula y demuestra que un tipo
  presente no puede ser `Copy` ni `Discard`.
- `tondo-vm` conserva su propio registro y análisis sobre tipos concretos,
  layouts nominales y capturas. El verifier no confía en metadata HIR y rechaza
  un witness opaco forjado que intente esconder una obligación terminal.
- TERM-001 no genera cleanup ni consume recursos: TERM-002 conserva la
  responsabilidad exclusiva del dataflow normal, y TERM-003/004 poblarán
  después los edges ya reservados.
- La puerta completa pasa con 513 tests —459 del compilador y 12 de la VM—,
  `cargo fmt --check`, `cargo check`, `cargo build`, Clippy con warnings como
  error, rustdoc con warnings como error y `git diff --check`.

### 0.46 — 2026-07-22

- Se completa BORROW-006. HIR admite `for ref` solo sobre lugares estables
  `Array`, `Map` y `Set`, conserva la colección como región compartida durante
  todo el bucle, exige `Copy` a los bindings por valor y diagnostica fuentes
  temporales, protocolos de usuario, movimiento o mutación solapada.
- MIR y bytecode representan el avance prestado con una posición y una
  proyección `IteratorElement`, nunca copiando el elemento para construir el
  cursor. Los índices de fuentes anidadas se congelan al entrar al bucle. Ambos
  verificadores rederivan el origen único, la cadena de regiones, el productor
  de posición y la frontera que permite solo `Region ref`.
- La VM ejecuta arrays, maps y sets prestados, incluidos reborrows, patrones de
  map, `break`, `continue` y uso posterior del dueño. El unwind elimina las
  reservas del frame abandonado y el runtime valida que el cursor y la
  colección anclada conservan la misma identidad.
- Las regresiones incluyen diagnósticos HIR, HIR/MIR/bytecode adversarial y
  ejecución end-to-end. La política de suspensión queda cerrada sin fingir M7:
  `await` sigue siendo una superficie incompleta hasta que tenga terminador,
  frame y análisis `Send` explícitos.
- El gate acumulado pasa 507 tests, incluidos 457 tests de la librería del
  compilador; también pasan `git diff --check`, formatter check, check y build
  de todos los targets, Clippy y Rustdoc con warnings denegados.

### 0.45 — 2026-07-22

- Se completa BORROW-005. HIR distingue obligaciones dinámicas de reserva y de
  acceso, las admite como snapshots completos y mantiene `E1403` para todo
  solapamiento inevitable.
- MIR ejecuta un análisis posterior a liveness sobre el conjunto exacto de
  préstamos activos. `ValidateLoan`, `Index`/`Slice` y `ValidatePlaces`
  transportan listas `against` canónicas solo para relaciones `Runtime`; las
  rutas demostrablemente disjuntas no pagan una comparación de solapamiento.
- Los verificadores de MIR y bytecode recalculan las relaciones, rechazan IDs
  ausentes, extra, duplicados o inactivos, enlazan cada validación con su reserva
  o acceso y prohíben cambiar los temporales de índice/bounds mientras el testigo
  está pendiente. El desensamblador expone estos IDs.
- La VM normaliza índices positivos y negativos, strides, intervalos, regiones
  de patrón y claves de map antes de comparar rutas. Una intersección produce
  `P0004`; bounds, entrada de map ausente y step cero conservan sus pánicos y el
  unwind invalida reservas previas antes de que pueda ejecutarse el callee.
- Las regresiones end-to-end cubren éxito y solapamiento para índices, slices,
  strides negativos, lectura y escritura posteriores, regiones de patrón y
  claves dinámicas de map; también fijan orden de pánicos y bytecode/MIR
  adversarial. El gate acumulado pasa 501 tests, incluidos 451 tests de la
  librería del compilador; también pasan `git diff --check`, formatter check,
  check y build de todos los targets, Clippy y Rustdoc con warnings denegados.

### 0.44 — 2026-07-22

- Se completa BORROW-004 con una representación canónica de regiones para
  índices y slices de array, elementos prefijo y restos de patrón. Literales y
  constantes enteras no negativas alimentan una prueba conservadora de
  intervalos y progresiones; bounds negativos o dinámicos se reservan para el
  check normativo de BORROW-005.
- HIR distingue `Disjoint`, `Overlap` y `Runtime`: admite regiones incompatibles
  demostrablemente disjuntas, diagnostica con `E1403` el solapamiento inevitable
  y deja incompleta únicamente la pareja incompatible dependiente de datos. Un
  préstamo dinámico aislado y regiones dinámicas exclusivamente `ref` ya cruzan
  la frontera ejecutable.
- MIR valida bounds y step mediante CFG con unwind antes de cada reserva. Sus
  move paths y los del bytecode recuperan constantes solo desde temporales de
  definición única y vuelven a probar la disjunción sin confiar ciegamente en
  HIR. El verificador rechaza bytecode forjado que convierte regiones disjuntas
  en solapadas.
- La VM ejecuta escrituras a través de índices, slices contiguos o strided y
  regiones nacidas de patrones. Toda validación de escritura transporta un
  testigo prestado del reemplazo, de modo que también puede comprobar la forma
  cuando un parámetro aparentemente raíz oculta un slice del caller. Bounds y
  step cero conservan sus pánicos de lenguaje antes de entrar en el callee.
- Las regresiones cubren índices, intervalos, residuos de stride, índice frente
  a slice, regiones dinámicas aisladas, shared/shared dinámico, prefijos/restos
  de patrón, escritura efectiva, unwind y bytecode adversarial.
- El gate acumulado pasa 497 tests, incluidos 447 tests de la librería del
  compilador; también pasan `git diff --check`, formatter check, check y build
  de todos los targets, Clippy y Rustdoc con warnings denegados.

### 0.43 — 2026-07-22

- Se completa BORROW-003 con una separación verificable entre observación
  compartida, escritura de extensión fija y reemplazo estructural. Una
  asignación raíz `mut` solo se admite para formas exteriores estáticamente
  fijas; las raíces `Array`, `Map`, `Set`, genéricas u opacas producen `E1411`
  y usan `var` para reemplazo arbitrario. El reemplazo de contenido de igual
  longitud sigue disponible mediante asignación de slice y las operaciones
  `mut self` conservan su contrato explícito.
- HIR registra `PreserveExtent` o `Replace` en cada asignación y su verificador
  rederiva de forma independiente permisos, proyecciones y forma exterior antes
  de admitir MIR. Los cuerpos divergentes no fabrican una escritura inexistente.
- MIR, bytecode y VM exigen que una reborrow `var` nacida de `mut` termine en un
  sublugar estricto, completo y reemplazable. Campos, slots y elementos de array
  existentes conservan expresividad; raíces, slices, restos de array, entradas
  potenciales de map y proyecciones opacas no elevan permisos. Bytecode
  centraliza esa clasificación para que verificador y ejecutor no diverjan.
- Las regresiones cubren raíces fijas y dinámicas, genéricos, operaciones
  in-place, slices `mut`/`var`, HIR forjado y reborrows estructurales válidas e
  inválidas en MIR y bytecode.
- El gate acumulado pasa 492 tests, incluidos 443 tests de la librería del
  compilador; también pasan `git diff --check`, formatter check, check y build
  de todos los targets, Clippy y Rustdoc con warnings denegados.
- La cola avanza a BORROW-004.

### 0.42 — 2026-07-22

- Se completa BORROW-002 para bindings `ref` de patrones sobre lugares fijos.
  HIR conserva la identidad fuente real a través de aliases anidados y calcula
  conjuntos `live-after` por camino para bloques, operandos ordenados,
  cortocircuito, `if`, guards y arms de `match`, loops y transfers. Los facts de
  uso excluyen código inalcanzable; `break` toma la salida exacta y `continue`
  el backedge aunque estén anidados dentro de otra expresión.
- MIR distingue loans `CallLocal` y `Region`. Los lugares prestados conservan
  un ancla `source_loan`; un análisis backward inserta releases tras el último
  uso o en bridges de edges específicos, siempre cerrando primero regiones
  anidadas y sin exponer lifetimes en el lenguaje fuente.
- Los verificadores MIR y bytecode rederivan el orden acíclico, la contención
  del path, el modo compartido, la cadena fuente activa, los joins exactos y la
  imposibilidad de consumir una región como argumento. La VM repite las
  defensas dinámicas sobre lectura, move, escritura, validación, reserva y call.
- Las regresiones cubren liberación secuencial y por rama, joins, backedges,
  orden de argumentos, regiones anidadas, reborrow call-local, escritura
  posterior válida, cierre de un hijo abandonado antes de su padre, transfers
  anidados, código inalcanzable y bytecode forjado con cierre prematuro.
  Patrones de array y cursores `for ref` siguen honestamente incompletos para
  BORROW-004/005.
- El gate acumulado pasa 489 tests, incluidos 441 tests de la librería del
  compilador; también pasan `git diff --check`, formatter check, check y build
  de todos los targets, Clippy y Rustdoc con warnings denegados.
- La cola avanza a BORROW-003.

### 0.41 — 2026-07-22

- Se completa BORROW-001 con una única representación explícita para argumentos
  no-value: cada loan conserva modo y lugar, se reserva después de evaluar su
  argumento y se consume al iniciar la llamada. Un `Borrow` vuelve a quedar
  limitado a observaciones inmediatas y callees indirectos.
- HIR aplica permisos `ref`/`mut`/`var`, reborrowing monotónico y conflictos en
  orden de evaluación. MIR y bytecode verifican conjuntos activos exactos,
  aliasing de proyecciones fijas, no escape y release explícito en `return`,
  `fail`, `?`, `break` y `continue`; los límites de loop impiden liberar un loan
  exterior por accidente.
- La VM pasa loans como referencias internas al frame prestador, normaliza
  paths para detectar aliasing, lee y escribe a través de reborrows y limpia
  reservas durante unwind. La ABI host sigue cerrada a parámetros prestados y
  los loans de index/slice permanecen detrás de BORROW-004/005.
- Las regresiones cubren `ref` temporal, `mut`/`var` raíz, proyecciones y
  capturas de closure, campos disjuntos, llamadas anidadas, reborrow, `?`,
  transfers de loop, release inactivo, reservas duplicadas, acceso conflictivo
  y operandos forjados fuera de una llamada.
- El gate acumulado pasa 486 tests, incluidos 438 tests de la librería del
  compilador; también pasan `git diff --check`, formatter check, check y build
  de todos los targets, Clippy y Rustdoc con warnings denegados.
- La cola avanza a BORROW-002 para inferir regiones generales por último uso.

### 0.40 — 2026-07-22

- Se completa OWN-007 con una prueba must-transfer separada de la unión de
  disponibilidad: `CallOnce` exige `Discard` o transferencia completa de cada
  captura en toda salida normal, `return`, `fail` y propagación `?`; panic y
  divergencia no fabrican una salida normal.
- HIR vuelve a derivar la prueba con bounds abiertos. MIR y bytecode la repiten
  por intersección sobre el CFG normal, distinguen moves parciales de una
  extracción completa de newtype y eliminan la prueba si el slot se repone.
- La frontera monomorfizada reevalúa `Discard` sobre tipos, nominales, closures y
  testigos opacos concretos. Así una closure genérica conservadora puede ganar
  `CallOnce` para `T = Int` sin alterar sus decisiones de Copy/Move ni confiar en
  metadata HIR.
- Las regresiones cubren capturas terminales observadas, transferencia en todas
  frente a solo algunas ramas, `fail`, `?`, newtypes, metadata HIR falsificada,
  rederivación MIR/bytecode y especialización genérica concreta. El gate
  acumulado pasa 480 tests, formatter check, `git diff --check`, check y build de
  todos los targets, Clippy y Rustdoc con warnings denegados.
- La cola avanza a BORROW-001. TERM-002 conserva la obligación posterior de
  seguir un recurso transferido a otro owner y rechazar su abandono al salir de
  scope.

### 0.39 — 2026-07-22

- Se completa OWN-006 aplicando a cada captura la decisión contextual única de
  ownership: un tipo con prueba `Copy` conserva el binding exterior y cualquier
  otro tipo se mueve al entorno cuando se construye la closure.
- HIR incorpora construcción de entornos y slots capturados al mismo análisis
  de disponibilidad. Un move alcanzable desde el entorno elimina `Call` y
  `CallMut`; una observación afín sigue siendo repetible y construir una closure
  anidada transfiere sus capturas sin ejecutar su body.
- MIR conserva operandos Copy/Move exactos, rederiva protocolos desde sus move
  paths y bytecode repite ambas pruebas antes de ejecución. La VM reutiliza sus
  campos opcionales y raíces temporales para ejecutar moves de capturas, incluso
  durante una construcción multi-captura bajo presión de GC.
- Regresiones HIR, MIR, bytecode y end-to-end cubren invalidación exterior,
  observación repetible, propagación anidada, metadata falsificada y ejecución
  de closures opacas afines. El gate acumulado pasa 475 tests, formatter check,
  `git diff --check`, check y build de todos los targets, Clippy con
  warnings denegados y Rustdoc con warnings denegados.
- La cola avanza a OWN-007 para cerrar la prueba de obligaciones terminales de
  `CallOnce`; no queda otra forma de transferencia de captura pendiente.

### 0.38 — 2026-07-22

- Se completa OWN-005 sin exponer un estado fuente persistentemente
  “parcialmente movido”. Una extracción affine ordinaria desde campo, tuple
  slot, índice, slice, receptor o préstamo produce `E1406`; la destructuración
  completa consume primero el owner, mientras `.value` de newtype cuenta como
  proyección completa solo sobre owner o temporal movible.
- Cada `match` HIR conserva un modo uniforme `Copy`, `Observe` o `Consume`, que
  el verificador vuelve a derivar. Tags, forma y guards se prueban mediante
  observación; bindings afines se transfieren únicamente al entrar en el body
  seleccionado, por lo que un guard falso no vacía payloads usados por arms
  posteriores. Un binding de patrón `ref` no puede materializarse, ni siquiera
  cuando su componente cumple `Copy`.
- MIR y bytecode reemplazan el bit de raíz por conjuntos canónicos de move paths
  tipados. Detectan moves duplicados, de ancestros, descendientes y paths
  dinámicos potencialmente solapados; permiten siblings estáticamente disjuntos,
  unen paths no disponibles en CFG/loops y restauran solo el subárbol escrito
  cuando su padre sigue disponible.
- La VM mueve payloads proyectados de forma defensiva y materializa un
  `..rest` affine tomando sus elementos a un nuevo array propietario, con roots
  explícitos durante la asignación. Regresiones HIR, MIR, bytecode y end-to-end
  cubren tuples, records, enums, options, arrays, newtypes, observación y guards.
- El gate acumulado pasa 468 tests, `git diff --check`, formatter check, check y
  build de todos los targets, Clippy con warnings denegados y Rustdoc con
  warnings denegados.

### 0.37 — 2026-07-22

- Se completa OWN-004 derivando la capacidad de reposición desde el `mutable`
  ya retenido por cada binding HIR. Solo `=` sobre un `var` directo evita leer
  el valor anterior; una asignación compuesta, un destino parcial, un `let` o un
  parámetro no adquieren ese permiso por accidente.
- Una escritura que completa elimina el estado movido únicamente en sus caminos
  normales. Los joins siguen exigiendo definición en todos los predecesores:
  reponer en una sola rama no basta, mientras que reponer en ambas ramas o en
  cada backedge de un loop conserva disponibilidad.
- Regresiones cubren reposición lineal, múltiple y desestructurada, ramas,
  loops, RHS inválido, targets parciales e inmutables, mutación defensiva de HIR
  y la ruta pública completa hasta ejecución en VM.
- La validación runtime de una escritura completa ya no intenta leer el slot
  movido de un destino directo. Una proyección continúa resolviendo y leyendo
  su raíz, por lo que no puede reponer parcialmente un agregado no disponible.
- El gate acumulado pasa 461 tests, `git diff --check`, formatter check, check y
  build de todos los targets, Clippy con warnings denegados y Rustdoc con
  warnings denegados.

### 0.36 — 2026-07-22

- Se completa OWN-003 con un análisis estructurado por body que clasifica cada
  acceso como observación o transferencia bajo los bounds `Copy` exactos. El
  estado conserva el primer span de move y produce `E1401` con ubicación
  relacionada para todo uso posterior.
- Los joins unen bindings no disponibles, por lo que un owner solo está
  disponible si lo está en todos los predecesores que completan. `return`,
  `fail` y pánico no contaminan el camino normal; `break`, `continue`, loops
  condicionales, iteradores y cortocircuitos convergen mediante un fixed point
  monotónico.
- La asignación múltiple conserva su semántica atómica: resuelve destinos,
  materializa el RHS completo y restaura destinos directos que estaban
  disponibles. La reposición de un `var` previamente movido permanece aislada
  en OWN-004.
- El admission verifier de HIR repite el análisis. MIR y bytecode tratan un
  `Move` no proyectado como consumo de la definición, intersectan disponibilidad
  en ramas y backedges, y rechazan bytecode mutado antes de ejecutarlo. Los move
  paths proyectados permanecen explícitamente en OWN-005.
- El gate acumulado pasa 459 tests, `git diff --check`, formatter check, check y
  build de todos los targets, Clippy con warnings denegados y Rustdoc con
  warnings denegados.

### 0.35 — 2026-07-22

- Se completa OWN-002 con una decisión contextual y única de ownership:
  `T: Copy` produce `Copy`; un tipo afín, opaco o genérico sin esa prueba produce
  `Move`. El análisis estructural se comparte entre lowering y verifier y se
  cachea por body/tipo.
- Bindings, parámetros propietarios, retornos, argumentos por valor,
  construcciones, cursores intrínsecos y `CallOnce` transfieren ownership sin
  depender de la representación concreta de runtime. `CallOnce` deja de exigir
  el límite bootstrap `Copy`.
- Igualdad, pertenencia, longitud, discriminantes, bases de index/slice,
  callees compartidos/exclusivos, argumentos `ref`/`mut`/`var` y la validación
  previa de un slice write usan `Borrow` inmediato. MIR y bytecode impiden que
  ese acceso escape a storage, agregados, retornos, argumentos por valor u
  operaciones no autorizadas.
- La VM ejecuta moves reales mediante `take_place`; regresiones end-to-end
  transfieren un callable opaco afín a través de una función genérica, lo
  invocan y obtienen `42`. Otras demuestran que igualdad y pertenencia dejan
  disponibles sus operandos afines.
- Los verificadores rechazan un `Copy` falsificado para un tipo contextual no
  `Copy`, modos de argumento con acceso incorrecto y borrows que escapen.
  OWN-003 conserva deliberadamente el análisis de uso posterior, joins y
  disponibilidad por CFG.
- El gate acumulado pasa 451 tests, `git diff --check`, formatter check, check y
  build de todos los targets, Clippy con warnings denegados y Rustdoc con
  warnings denegados.

### 0.34 — 2026-07-22

- Se completa OWN-001 auditando la derivación cerrada ya compartida por
  escalares, estructuras, colecciones, nominales recursivos, genéricos, opacos
  y closures, y eliminando la última forma diferida: los cursores intrínsecos.
- Cada `for` intrínseco conserva en HIR un tipo exacto
  `cursor[own,C]`/`cursor[ref,C]`. MIR y bytecode separan la colección fuente
  del estado de recorrido y sus verificadores rechazan forma, modo o colección
  falsificados.
- Un cursor propio deriva `Copy`, `Discard`, `Send` y `Share` de `C`; uno de
  observación siempre es `Copy + Discard` y exige `C: Send + Share` para las
  dos capacidades concurrentes. Ninguno es `Equatable` ni `Key`.
- HIR conserva ya el modo `ref`, pero su formación ejecutable permanece detrás
  de BORROW-001: MIR/bytecode exigirán un operando `Borrow` real y nunca lo
  aproximan copiando la colección.
- La VM duplica de forma eager el estado lógico de un cursor copiable en un
  objeto de avance independiente: copia el origen propio o conserva el préstamo
  compartido. Las regresiones cubren matrices positivas y negativas, recursión,
  genéricos, modos own/ref, mutación defensiva y ejecución real.
- El gate acumulado pasa 447 tests, `cargo check`, build de todos los targets,
  formatter check, Clippy con warnings denegados y Rustdoc con warnings
  denegados.
- La cola avanza a OWN-002 para introducir moves afines sobre esta base.

### 0.33 — 2026-07-21

- Se completa CALL-004 con identidades generadas y firmas estructurales exactas
  para closures sync, unsafe, async y async-unsafe, sin conversiones implícitas
  entre efectos.
- Las firmas async rechazan parámetros `mut`/`var` mediante `E1609`; HIR vuelve
  a probar identidad, firma y parámetros, y deriva `CallOnce` como único
  protocolo para una closure async que escribe su entorno.
- MIR y bytecode conservan cuerpos, entornos y bits de efecto, pero sus
  operaciones de llamada síncrona segura rechazan firmas con efectos. La VM
  rechaza además seleccionar un callable async o unsafe como entrada raíz.
- Las cuatro formas pueden construirse, borrarse a su firma uniforme exacta,
  copiarse, trazarse y descartarse sin ejecutar su body. `await`/`spawn`
  continúan en M7, la frontera unsafe en M9 y las capturas afines en M5.
- El gate acumulado queda en 445 tests, `cargo check`, formatter check, build de
  todos los targets, Clippy y Rustdoc sin warnings; la cola avanza a OWN-001.

### 0.32 — 2026-07-21

- Se completa CALL-003 con derivación cerrada de `Call`, `CallMut` y `CallOnce`
  desde accesos alcanzables a capturas, selección exacta por llamada y
  diagnósticos diferenciados para contrato, protocolo y erasure inválidos.
- HIR, MIR y bytecode vuelven a probar protocolos, firma y acceso. Cada closure
  tiene un body monomorfizado con entorno oculto, y la VM ejecuta llamadas
  puras, mutables, consumibles, genéricas, opacas y borradas sin vtables.
- `Borrow` queda confinado al callee indirecto; las raíces temporales protegen
  callee, argumentos y copias del entorno frente a colecciones durante la
  invocación.
- Los cuerpos genéricos de closure consumen el presupuesto compartido de
  instanciación, el desensamblador expone schema/protocolos y los verifiers
  rechazan metadata, protocolos, firmas, accesos y erasures falsificados.
- Capturas afines continúan en M5 y los efectos `async`/`unsafe` avanzan a
  CALL-004. El gate acumulado queda en 434 tests, `cargo check`, formatter
  check, build de todos los targets, Clippy y Rustdoc sin warnings.

### 0.31 — 2026-07-21

- Se completa CALL-002 con tipos concretos estables, firmas explícitas o
  inferidas, bodies HIR independientes y capturas sintácticas por valor que
  preservan mutabilidad y binders genéricos.
- HIR y MIR revalidan la correspondencia exacta de cada captura; bytecode y VM
  construyen, copian y trazan el entorno gestionado sin ejecutar el body.
- Las raíces temporales de la VM hacen segura una colección durante
  construcción o copia recursiva de entornos con capturas compuestas.
- La coerción a `fn(...)`, la invocación y los protocolos cerrados avanzan a
  CALL-003; moves de capturas afines siguen perteneciendo a M5 y los efectos
  `async`/`unsafe` a CALL-004.
- El gate acumulado queda en 418 tests, `cargo check`, formatter check, build de
  todos los targets, Clippy y Rustdoc sin warnings; la cola avanza a CALL-003.

### 0.30 — 2026-07-21

- Se completa CALL-001 con valores uniformes para funciones libres y operaciones
  asociadas sin receptor, especialización genérica explícita o contextual
  exacta y rechazo de bound methods implícitos.
- HIR, MIR y bytecode verifican firma, aridad y especialización; llamadas a
  valores pierden etiquetas y mantienen modos, variádico, efectos y outcomes.
- La monomorfización selecciona también las operaciones de trait conservadas en
  constantes y la VM ejecuta todos los orígenes admitidos mediante el mismo
  contrato indirecto.
- El gate acumulado queda en 406 tests, formatter check, build de todos los
  targets, Clippy y Rustdoc sin warnings; la cola avanza a CALL-002.

### 0.29 — 2026-07-21

- Se completa CAP-001 con un motor estructural y coinductivo común para `Copy`,
  `Discard`, `Equatable`, `Key`, `Send` y `Share`, incluidas sus implicaciones,
  bounds genéricos y contratos opacos.
- Formación de colecciones/referencias, igualdad, membership, map lookup,
  duplicados y receptores async consumen una única tabla HIR verificada; MIR y
  bytecode mantienen fronteras de comprobación independientes.
- La VM ejecuta igualdad estructural de nominals y colecciones; maps y sets se
  comparan por contenido, sin hacer observable el orden de inserción.
- El gate acumulado queda en 398 tests, formatter check, build de todos los
  targets, Clippy y Rustdoc sin warnings; la cola avanza a CALL-001.

### 0.28 — 2026-07-21

- Se completa TRAIT-006 con familias opacas por identidad de declaración,
  argumentos genéricos invariantes y un único testigo concreto exacto por body.
- Los bounds publicados se prueban estáticamente; el canal de error sigue
  visible y callers, tooling y dispatch no acceden a la representación privada.
- HIR, MIR y bytecode conservan sellos verificables; la ejecución es un no-op
  sin wrapper, allocation ni dispatch dinámico, y los verifiers rechazan
  metadata, ciclos o coerciones forjados.
- El gate acumulado queda en 389 tests, formatter check, build de todos los
  targets, Clippy y Rustdoc sin warnings; la cola avanza a CAP-001.

### 0.27 — 2026-07-21

- Se completa TRAIT-005 con lookup cerrado por constraints, calificación
  explícita, selección única tras sustitución y prueba recursiva de bounds.
- Traits fuente, defaults, overrides, `Display` e `Iterator[T]` llegan a
  bytecode como callables directos; no existe dispatch dinámico ni metadata de
  witness en runtime.
- Los `for` de usuario conservan su protocolo en HIR, bajan la llamada estática
  a `next` en MIR y ramifican sobre `Option`; los verifiers rechazan aridad,
  firma o protocolo mutados.
- El gate acumulado queda en 370 tests, formatter check, build de todos los
  targets, Clippy y Rustdoc sin warnings; la cola avanza a TRAIT-006.

### 0.26 — 2026-07-21

- Se completa TRAIT-004 con consultas canónicas, matrices normativas de cambio
  de tamaño, SCCs deterministas y saturación iterativa bajo presupuesto.
- Los ciclos idempotentes sin descenso diagonal producen `E1112` con la ruta y
  matriz testigo; las capacidades cerradas no crean aristas y los adaptadores
  acíclicos siguen siendo válidos.
- El admission verifier repite la prueba antes de MIR y las regresiones cubren
  álgebra, descenso, conservación, permutación, crecimiento, múltiples SCC,
  orden de archivos, precedencia diagnóstica, mutación y agotamiento `T0002`.
- El gate acumulado queda en 350 tests, formatter check, build de todos los
  targets, Clippy y Rustdoc sin warnings; la cola avanza a TRAIT-005.

### 0.25 — 2026-07-21

- Se completa TRAIT-003 con unificación first-order multi-raíz cuyos binders
  izquierdo y derecho tienen scopes independientes, occurs checks y matching
  no ordenado de uniones normalizadas.
- La coherencia ignora bounds, compara grupos por identidad de trait y emite
  `E1111` de forma determinista; la dependencia funcional de `Iterator[T]`
  distingue duplicación `E1111` de elemento incompatible `E1113`.
- El admission verifier vuelve a derivar la unicidad de la tabla y las pruebas
  cubren aliases, bounds, no solapamiento, orden lógico, HIR mutado y la ruta
  diagnóstica pública con evidencia relacionada.
- El gate acumulado queda en 339 tests, formatter check, build de todos los
  targets, Clippy y Rustdoc sin warnings; la cola avanza a TRAIT-004.

### 0.24 — 2026-07-21

- Se completa TRAIT-002 con tablas deterministas de implementaciones y métodos,
  cabeceras normalizadas, binders completos y bodies comprobados por la ruta HIR
  ordinaria.
- Orphan rules, protocolos prelude abiertos/cerrados y contratos exactos se
  validan antes de admitir un `impl`; defaults omitidos o sustituidos conservan
  la firma y los bounds del trait.
- El verifier reconstruye los contratos sin confiar en la tabla producida por
  lowering y rechaza mutaciones de IDs, firmas, claves, cobertura o metadata.
- La cola avanza a TRAIT-003; overlap, terminación, selección y dispatch siguen
  deliberadamente fuera de TRAIT-002.

### 0.23 — 2026-07-21

- Se completa TRAIT-001 con tablas HIR deterministas, `Self` contextual oculto,
  métodos requeridos/asociados, defaults y el requisito `Self: Send` de
  receptores async.
- Los defaults se comprueban bajo los binders del trait y resuelven únicamente
  llamadas al mismo contrato; especializaciones explícitas de método fijan solo
  sus argumentos locales sin confundirlas con indexación.
- El admission verifier cierra aridad, ownership de miembros, clasificación de
  receptor y coherencia del body; los defaults no usados permanecen fuera de
  los roots monomorfizados.
- El gate acumulado queda en 324 tests, formatter check, build de todos los
  targets, Clippy y Rustdoc sin warnings; la cola avanza a TRAIT-002.

### 0.22 — 2026-07-21

- Se completan GEN-001 y GEN-002 con bodies genéricos comprobados, inferencia
  invariante, especialización explícita contextual y constraints `Discard`
  ejecutables.
- Un worklist determinista monomorfiza desde roots no genéricos y constantes,
  sustituye toda la superficie MIR, deduplica recursión estable y limita la
  expansión de instancias y tipos con `T0002`.
- El bytecode ejecutable queda completamente concreto sin type packs runtime;
  las plantillas nominales permanecen compactas y verificadas por layout.
- El gate acumulado queda en 318 tests, formatter check, build de todos los
  targets, Clippy y Rustdoc sin warnings; la cola avanza a TRAIT-001.

### 0.21 — 2026-07-21

- Se completan DEC-006, DEC-007, VM-001 a VM-009 y los cinco programas de
  aceptación; G2 queda cerrado como primer compilador bootstrap ejecutable.
- Frames por slots, pánicos normativos, frontera de `main`, consola tipada y GC
  preciso no móvil están conectados al driver y al binario públicos.
- Se corrigen durante la aceptación la atomicidad de asignación de slices, la
  política dinámica de duplicados de `Map`, el mensaje fuente de `assert` y la
  obligación `Discard` del error de `main`.
- El gate acumulado queda en 307 tests, smoke tests G2, formatter check, Clippy
  y Rustdoc sin warnings; la cola avanza a GEN-001 y GEN-002.

### 0.20 — 2026-07-21

- Se completan BC-001 a BC-005 con un bytecode tipado por slots propiedad de la
  VM y lowering determinista desde el MIR verificado.
- Catálogos, layouts nominales, calls, spans, storage lifetime, inicialización,
  refinamiento de tags y cleanup edges se verifican de nuevo en la frontera de
  ejecución con presupuestos explícitos.
- El disassembler queda limitado a tooling in-memory, sin congelar un ABI ni un
  loader durante bootstrap.
- El gate acumulado queda en 278 tests, formatter check, Clippy, Rustdoc y smoke
  tests públicos; la cola avanza a DEC-006 y VM-001.

### 0.19 — 2026-07-21

- Se completan MIR-002 a MIR-007 con lowering determinista de toda la superficie
  HIR bootstrap a un CFG tipado, independiente del CST y del AST.
- Cleanup/unwind, inicialización, storage lifetime, refinamiento de tags,
  places, calls y spans quedan verificados antes de admitir un backend.
- Los presupuestos MIR forman parte de la request y fallan con `T0002`; la ruta
  real de `run` llega al marcador de bytecode únicamente con MIR válido.
- El gate acumulado queda en 269 tests, formatter check, Clippy y Rustdoc sin
  warnings; la cola avanza a BC-001.

### 0.14 — 2026-07-21

- Se completa CHECK-009 con un `SemanticModel` inmutable que conserva fuentes,
  resolución y HIR disponible dentro de `CompilationOutput`.
- Las queries estructuradas cubren expresiones y tipos contextuales, entidades,
  declaraciones, referencias, firmas, enums/uniones y errores cerrados de
  calls; snapshots parciales mantienen una frontera explícita por fase.
- El HIR registra referencias exactas a fields y variantes, y selecciona nodos
  por rango visible sin exigir que tooling conozca la trivia lossless del CST.
- Referencias multiarchivo siguen el orden lógico normativo; newtypes conservan
  ambos namespaces y shorthand de patterns conserva simultáneamente member y
  local.
- El gate acumulado queda en 221 tests, formatter check, Clippy y Rustdoc sin
  warnings; la cola avanza a CHECK-010.

### 0.13 — 2026-07-21

- Se completa CHECK-008 con una sentencia HIR de descarte independiente y hojas
  de descarte preservadas dentro de asignación múltiple.
- `Discard` se deriva mediante resúmenes simbólicos coinductivos sobre tipos
  compuestos, nominales genéricos y recursión transformadora; `Join` produce
  `E1105` a cualquier profundidad.
- Parámetros fijos `_` por valor comparten la obligación, los préstamos no, y
  `Discard`, `Copy` y `Key` prueban genéricos sin asumir bounds ausentes.
- El gate acumulado queda en 212 tests, formatter check, Clippy y Rustdoc sin
  warnings; la cola avanza a CHECK-009.

### 0.12 — 2026-07-21

- Se completa CHECK-007 con `HirFlow`, identidades de loop y resumen bottom-up
  de breaks alcanzables, independiente del tipo producido por una coerción.
- `for {}` distingue breaks propios, anidados y muertos; bloques, calls,
  propagación, `if` y `match` conservan con precisión sus caminos normales.
- `W1006` usa un worklist top-down sobre raíces HIR y el orden real de
  evaluación, sin cascadas dentro de subárboles ya inalcanzables.
- El driver conserva warnings semánticos y continúa hasta la siguiente fase;
  solo los errores preemptan `T0001`.
- El gate acumulado queda en 208 tests, formatter check, Clippy y Rustdoc sin
  warnings; la cola avanza a CHECK-008.

### 0.11 — 2026-07-21

- Se completa CHECK-006 con HIR explícito para asignación simple, compuesta,
  múltiple y descarte, sin reevaluación de lugares ni pérdida del orden del RHS.
- Campos nominales genéricos, slots de tupla, arrays, slices, maps y aritmética
  elevada quedan integrados en la frontera tipada que necesita la asignación.
- La revisión normativa avanza a `0.1-draft.8` para registrar
  `E1411 invalid-assignment-target`; `E1405` normaliza operandos constantes y
  detecta overlap inevitable de rutas y prefijos.
- El gate acumulado queda en 201 tests, formatter check, Clippy y Rustdoc sin
  warnings; la cola avanza a CHECK-007.

### 0.10 — 2026-07-21

- Se completa CHECK-005 con HIR tipado para toda la gramática de patrones,
  guards y `match` exhaustivo.
- La matriz de utilidad cubre dominios algebraicos, nominales, uniones y arrays;
  usa worklist y presupuesto explícitos para no depender del stack del host.
- Paths de patrón importados y genéricos, aliases discriminadores y valores
  literales decodificados comparten la identidad semántica correcta.
- `E1201` a `E1204` y el nuevo límite de análisis quedan conectados al driver.
- El gate acumulado queda en 188 tests, formatter check, Clippy y Rustdoc sin
  warnings; la cola avanza a CHECK-006.

### 0.9 — 2026-07-21

- Se añade el HIR tipado de expresiones con arenas acotadas, categorías
  value/place, bodies, locals y coerciones contextuales explícitas.
- Se completan CHECK-001 a CHECK-004 para el subconjunto no genérico: control
  estructurado, llamadas básicas, `Option`, `Result`, `fail`, `?` y widening
  cerrado de errores.
- Los diagnostics semánticos y el nuevo presupuesto HIR quedan conectados al
  driver público; `none` usa el código normativo `E1304`.
- Se documenta la frontera exacta que aún difiere traits, patrones, accesos,
  assignment, ownership y MIR, sin asignarles semántica provisional.
- El gate acumulado queda en 176 tests, formatter check, Clippy y Rustdoc sin
  warnings; la cola avanza a CHECK-005.

### 0.8 — 2026-07-21

- Se conecta al driver el primer HIR semántico de declaraciones y firmas, con
  lowering canónico de toda la gramática de tipos de fuente.
- Se completan TYPE-001 a TYPE-005 y TYPE-008, incluidos aliases genéricos,
  uniones discriminables, bounds, `Self`, variádicos, opacos y productividad
  recursiva con sustitución real.
- Se implementan los algoritmos de TYPE-006 y TYPE-007, que permanecen en curso
  hasta ser consumidos por el chequeo de expresiones.
- Se corrige la resolución de argumentos genéricos anidados en `PathType` y se
  prueban orden de archivos, recuperación, límites y grafos nominales profundos.
- El gate acumulado queda en 164 tests, formatter check, Clippy y Rustdoc sin
  warnings; la cola avanza al HIR tipado y CHECK-001/CHECK-010.

### 0.7 — 2026-07-21

- Se cierra la resolución determinista sobre un grafo de paquetes cerrado:
  módulos distribuidos, imports exactos, ciclos, namespaces de tipo/valor/módulo
  y todos los diagnósticos `E1001` a `E1008`.
- Se implementan scopes léxicos sin shadowing, bindings de patrones, loops y
  cierres, lvalues, shorthand de records y los contextos explícitos de `Self` y
  `self`.
- Se materializa el namespace de miembros para fields, newtypes, variantes,
  métodos y traits, con visibilidad, `E1501`, `E1503`, `E1504` y `E1505`.
- Se acepta DEC-004 con interning, identidad nominal completa, uniones
  normalizadas, inferencia no serializable y sustituciones que renormalizan.
- El gate acumulado queda en 139 tests, formatter check, Clippy y Rustdoc sin
  warnings; M2/G1 continúa abierto hasta completar lowering y type checking.

### 0.6 — 2026-07-21

- Se completa el formatter canónico sobre el CST lossless, su API pública y la
  integración real de `tondo fmt` y `tondo fmt --check`.
- Se validan comentarios, imports, todos los source forms, el corpus normativo,
  los fences válidos del spec y 512 programas generados por gramática.
- Una regresión generativa aclara y fija en spec que la llave interior restaura
  `NL` dentro de paréntesis o corchetes, manteniendo parseables los records
  multilínea anidados.
- Se cierra M1/G0 con 101 tests, formatter check, Clippy y Rustdoc sin warnings.

### 0.5 — 2026-07-21

- Se completa CST, parser recuperable y fachada AST tipada de M1.
- Se integran `E0004`, `E0005`, `E0006` y límites del parser en el driver.
- Se validan los 295 fences del spec, recuperación local, input binario
  arbitrario y protección efectiva frente a nesting profundo.
- El gate acumulado queda en 70 tests, Clippy y Rustdoc sin warnings.

### 0.4 — 2026-07-21

- Se completa el lexer lossless con Unicode 16.0.0, trivia, literales,
  interpolación, shebang, `NL` lógico y errores `E0001` a `E0003`.
- Se cierra DEC-003 y se valida reconstrucción exacta de los 295 fences Tondo.

### 0.3 — 2026-07-20

- Se completa M0 y su gate de salida.
- Se fija Rust 1.93.0 y se registran los quince ADRs iniciales.
- Se implementan source database, spans, paths NFC y line index lazy.
- Se implementan diagnostics JSON, IDs SHA-256, orden, deduplicación, related,
  fixes y representación humana.
- Se conecta CLI, driver único, target VM hosted, límites y harness de fixtures.
- La validación queda en 20 tests, Clippy y Rustdoc sin warnings.

### 0.2 — 2026-07-20

- Se crea `/tmp/tondo` como repositorio Git sobre branch `main`.
- Se completa el workspace Rust mínimo con los tres crates iniciales.
- Se añade la CLI bootstrap y se verifica que las operaciones no implementadas
  terminan con fallo explícito.
- Se validan formato, Clippy y tests con Rust/Cargo 1.93.0.

### 0.1 — 2026-07-20

- Creación inicial.
- Se fija `TONDO_LANGUAGE_SPEC.md` revisión `0.1-draft.7` como baseline.
- Se define una ruta de bytecode VM antes del backend nativo.
- Se separan bootstrap, alpha, preview y conformidad.
- Se posponen COW, ARC, backend nativo e incrementalidad hasta disponer de
  evidencia y una semántica ejecutable estable.
