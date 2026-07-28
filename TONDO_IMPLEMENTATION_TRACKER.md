# Tondo: tracker de implementación

**Estado:** M10 y Gate G5 cerrados; M10.5 es el siguiente milestone

**Versión del tracker:** 0.86

**Última actualización:** 2026-07-28

**Especificaciones normativas:**

- [Tondo 0.1 publicada](./TONDO_LANGUAGE_SPEC.md)
- [Extensión de testing para Tondo 0.2](./TONDO_TESTING_SPEC.md)

**Objetivo inmediato:** endurecer Tondo 0.1 mediante M10.5 antes de ampliar su
superficie pública. Después se implementa M10.6 contra la extensión normativa
de testing, se especifican e implementan Core + Hosted Standard Library 0.1 y
solo entonces comienza M11 con NATIVE-001. La VM permanece como implementación
de referencia y oracle diferencial del futuro backend nativo.

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
podrá anunciar conformidad completa Tondo 0.1 hasta superar
`tondo-conformance-0.1`.

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
| **G5 — Tondo 0.1 conforme** | Release certificable | Suite de conformidad completa para el target anunciado |
| **H0 — Fiabilidad continua** | Evidencia automatizada y reproducible | Trazabilidad, CI, properties, fuzzing, modelos y métricas |
| **T0 — Testing first-class** | `tondo test` conforme | Edición 0.2, unit/integration tests, aislamiento y reporte estable |
| **S1 — Standard Library 0.1** | Core + Hosted utilizable | Spec estándar, distribución, implementación VM y conformidad |
| **N1 — Backend nativo conforme** | Segunda implementación de producción | Oracle diferencial, runtime nativo y targets verificados |
| **S2 — Standard Library ampliada** | Concurrency + Application | APIs capability-gated con evidencia por módulo |

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

- [ ] **DEC-011 — Contrato de evidencia continua.** Antes de cerrar H0,
  documentar tiers de CI, seeds y reducción, corpus persistente, artefactos de
  fallo, medición de coverage/mutation score, umbrales y excepciones
  justificadas.

- [ ] **DEC-012 — Versionado y distribución de la stdlib.** Antes de publicar
  STD-0.1, fijar compatibilidad, módulos, prelude, PackageIds, hashes,
  capabilities, actualización y coexistencia con la release Tondo 0.1.0.

- [ ] **DEC-013 — Backend nativo y ABI runtime interna.** NATIVE-001 debe
  concluir con un ADR que elija backend y registre targets, calling convention
  interna, unwind, memoria, debug info, toolchain y criterios de portabilidad,
  sin prometer ABI FFI pública.

- [ ] **DEC-014 — Gestión de memoria nativa.** Antes de ARC-001, fijar
  ownership runtime, atomicidad, weak refs, detección de ciclos, interacción con
  COW, async, FFI privilegiada y estrategia de verificación.

- [x] **DEC-015 — Testing first-class y límite de edición.** La especificación
  [`TONDO_TESTING_SPEC.md`](./TONDO_TESTING_SPEC.md) reserva `suite` y `test`
  para Tondo 0.2: `suite` es un contenedor estático con setup léxico y teardown
  por `defer`; `test` es siempre una hoja. Mantiene Tondo 0.1 y
  `tondo-conformance-0.1` inmutables, separa unit overlays de integration roots
  y fija árbol/identidad, capturas `Copy + Send + Share`, envelope estructurado,
  `std.testing.log/failNow/skip`, inferencia de error/async, aislamiento,
  selección, límites, output, exit status, reporte
  `tondo-test-report-0.2/3` y listado `tondo-test-list-0.2/3`. No se introducen
  `TestContext`, attributes, clases, reflection, registro runtime, hooks ni
  retries implícitos.

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
| **M10 — Conformidad y release** | Gate G5: Tondo 0.1 | Completado |
| **M10.5 — Reliability y testing** | Infraestructura de evidencia continua | Siguiente |
| **M10.6 — Testing de usuario y edición 0.2** | Gate T0: `tondo test` conforme | Especificado; implementación pendiente |
| **STD-0.1 — Core + Hosted Standard Library** | API estándar mínima utilizable | Pendiente |
| **M11 — Backend nativo y optimización** | Implementación de producción | Futuro |
| **STD-0.2 — Concurrency + Application Standard Library** | Ecosistema estándar ampliado | Futuro |

Estado observado del workspace:

- Repositorio local: `/media/portable/Tony/Projects/tondo`, branch `main`, con
  upstream en
  `github.com/tonyredondo/tondo`.
- Workspace: `tondo-cli`, `tondo-compiler`, `tondo-conformance`,
  `tondo-reference-adapter` y `tondo-vm`.
- Toolchain utilizado para la validación: Rust 1.93.0 y Cargo 1.93.0; la versión
  mínima soportada queda fijada en Rust 1.93.
- Última validación: 2026-07-28, con formatter check, `cargo check` de todos los
  targets, Clippy con warnings denegados, 685 tests, Rustdoc con warnings
  denegados y metadatos locked. La suite oficial pasa 205 casos y 424
  repeticiones byte-estables.

### 4.1 Ruta crítica

~~~text
M0 -> M1 -> M2 -> M3 -> M4 -> M5 -> M6 -> M7 -> M8 -> M9 -> M10
  -> M10.5 -> M10.6 -> STD-0.1 -> M11 -> STD-0.2
       \________________________________________/
            testing y conformidad continuos
~~~

M4, M5 y M6 pueden investigarse conjuntamente, pero deben integrarse en ese
orden para evitar que collections o closures introduzcan una semántica de copia
incompatible con ownership.

M10.5 es una fase acotada de infraestructura y clasificación, no una pausa
indefinida para perseguir un número arbitrario de tests. Su gate debe existir
antes de ampliar sintaxis. M10.6 implementa después la extensión de testing sin
reescribir Tondo 0.1 y proporciona `tondo test` para probar la propia stdlib.
Cada API de STD-0.1 amplía la matriz generativa y de conformidad y añade
evidencia escrita en Tondo. M11 depende de Gate T0 y STD-0.1 porque el backend
nativo debe implementar una frontera runtime ya especificada y compararse byte
a byte con la VM sobre programas reales. STD-0.2 no bloquea M11.

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
| 16. Mutabilidad, memoria y concurrencia | M5, M7 y M9; APIs en STD-0.2 | G3, G4, M10 y STD-0.2 |
| 17. Operadores | M2, M6 y M8 | G3, G4 y M10 |
| 18. Semántica numérica | M3 y M6 | G3 y M10 |
| 19. Texto y Unicode | M1 para léxico; M6 para runtime | G0, G3 y M10 |
| 20. Ejecutables, scripts y procesos | M3, M7, M8 y M9; API host en STD-0.1 | G2, G4, M10 y STD-0.1 |
| 21. Formato y documentación | M1 y trabajo transversal | G0 y M10 |
| 22. Diagnósticos y tooling | M0, M1, M2, M9 y M10 | Todos los gates |
| 23. Gramática de referencia | M1 | G0 y M10 |
| 24. Ejemplos integrados | Tests de aceptación progresivos | G2, G3, G4 y M10 |
| 25. Características ausentes | Compile-fail distribuido por milestone | M10 |
| 26. Frontera con la stdlib | M6, M8, STD-0.1 y STD-0.2 | G3, G4, M10 y gates STD |
| Extensión de testing Tondo 0.2 | M10.6; helpers en STD-0.1 | T0 y S1 |

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
  tipado; el bootstrap fija un techo seguro de 256 niveles para no agotar la
  pila del host.

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
- La release se identifica expresamente como bootstrap y no conforme.

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
  en este checkpoint todavía no existían guards, cleanup ni fallback
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
- La CLI carga exactamente el plan cerrado, no ejecuta generadores ni busca
  dependencias, emite productos solo tras éxito y evita que estos sobrescriban
  inputs o se solapen entre sí, incluidos aliases de path.
- La frontera nativa 0.1 termina deliberadamente en unidades privilegiadas
  fijadas por hash. No se inventan layout, calling convention ni ABI general;
  un adaptador dinámico futuro deberá aportar y fijar ese contrato.

---

## 15. M10 — Suite de conformidad y release 0.1

**Objetivo:** convertir la afirmación “implementamos Tondo” en evidencia
versionada y reproducible.

### 15.1 Construcción de `tondo-conformance-0.1`

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

### 15.3 Release

- [x] **REL-001 — Publicar matriz exacta de target, perfil y capacidades.**

- [x] **REL-002 — Publicar versión de compilador, formatter, edición y suite.**

- [x] **REL-003 — Publicar resultados reproducibles de conformidad.**

- [x] **REL-004 — Documentar limitaciones que no contradigan capacidades
  anunciadas.**

- [x] **REL-005 — Verificar que no existe modo oculto que relaje checks.**

- [x] **REL-006 — Congelar el formato público de diagnostics JSON 0.1.**

- [x] **REL-007 — Etiquetar Tondo 0.1 únicamente después de superar todos los
  grupos aplicables.**

### Gate G5

- [x] La versión exacta del toolchain pasa `tondo-conformance-0.1`.
- [x] El target y sus capacidades están declarados.
- [x] No hay exclusiones sin justificar por capacidad.
- [x] Los artefactos, resultados y versiones pueden reproducirse.
- [x] La documentación no afirma soporte más amplio que la evidencia.

Evidencia de cierre:

~~~text
suite            = tondo-conformance-0.1 0.1.0
manifest_sha256  = 67f12434001d5d9d17b0f2181afe3ec38cb07d6207e431cca164ec4854f0148b
result_sha256    = d44e8eb853ccdc208b8a8ea044ddd2222a7e5ef148e91edc7c08ebec17425693
cases            = 205
repetitions      = 424
workspace_tests  = 685
target           = tondo-vm-hosted
profile          = hosted
capabilities     = [console, process]
~~~

El resultado estructurado se conserva en
`conformance/0.1/results/tondo-reference-0.1.0-tondo-vm-hosted.json`.

---

## 16. M10.5 — Reliability y testing

**Objetivo:** instalar una infraestructura de evidencia continua antes de
ampliar la API pública o duplicar la ejecución en un backend nativo. Este
milestone no cambia la semántica Tondo 0.1 ni reabre Gate G5: clasifica la
cobertura actual, automatiza el gate existente y crea las herramientas con las
que cada milestone posterior multiplicará casos reproducibles.

**Límite:** M10.5 no se cierra por alcanzar una cifra arbitraria de tests. Se
cierra cuando inventario, trazabilidad, CI, generación, fuzzing, modelos y
métricas tienen contratos ejecutables. La expansión del corpus continúa dentro
de M10.6, STD-0.1, M11 y STD-0.2.

### 16.1 Baseline y trazabilidad normativa

- [x] **TEST-AUDIT-001 — Auditar el corpus 0.1 existente.** La baseline
  observada contiene 685 tests Rust ejecutables y ninguno ignorado, 129
  fixtures internos `.to`, 205 casos y 424 repeticiones de conformidad, 302
  fences Tondo de la spec publicada 0.1 y 203 fuentes `.to` únicas al descontar
  los 127
  duplicados exactos entre fixtures internos y conformidad. El inventario
  distingue cantidad física, caso lógico, repetición y fuente única.

- [ ] **TEST-001 — Materializar un inventario machine-readable.** Añadir una
  herramienta reproducible que enumere por crate, fase, fixture, grupo,
  requisito, oracle, repetición, hash de fuente y target. Debe detectar IDs
  duplicados, sidecars huérfanos, casos no descubiertos y deriva entre el
  manifiesto y el repositorio. También registra documento, edición y estado:
  los ejemplos de `TONDO_TESTING_SPEC.md` se registran como contrato futuro,
  pero no cuentan como tests ejecutables ni cobertura de Tondo 0.1.

- [ ] **TEST-002 — Crear la matriz normativa de cobertura.** Cada requisito
  `debe`/`no puede` de Tondo 0.1 recibe una identidad estable. La matriz conserva
  revisión, heading anchor y hash del texto fuente, y lo clasifica como
  `covered`, `target-not-applicable`, `stdlib-pending` o `toolchain-limit`,
  siempre con evidencia enlazada. Una sección o fence no cuenta por sí mismo
  como cobertura semántica.

- [ ] **TEST-003 — Exigir dimensiones de prueba explícitas.** Para cada regla
  aplicable, la matriz registra caso positivo, rechazo o fallo cuando exista,
  límites materiales, composición con otras reglas, fase que actúa como oracle
  y frontera pública observada. Las excepciones requieren una justificación
  versionada, no una celda vacía.

- [ ] **TEST-004 — Cerrar primero los huecos críticos descubiertos.** Priorizar
  lexer/parser/formatter, resolución, tipos, ownership, HIR/MIR/bytecode
  verifiers, GC, scheduler, procesos y protocolos no confiables. Cada hueco se
  reduce a la fuente o estructura mínima que habría permitido el defecto.

### 16.2 Gate continuo de CI

- [ ] **CI-TEST-001 — Ejecutar el gate estricto en cada cambio.** Un workflow
  de PR y `main` debe ejecutar formatter check, `cargo check` de todos los
  targets, Clippy con warnings denegados, los tests completos, Rustdoc, build
  locked de runner/adaptador, validación del manifiesto y una ejecución de
  conformidad cuyo resultado se compare con la evidencia versionada.

- [ ] **CI-TEST-002 — Separar gate determinista y campañas sin rebajar el
  oracle.** PR y `main` ejecutan el mismo gate obligatorio; el tier nocturno
  añade stress, fuzzing y matrices costosas. Clasificar un caso como campaña no
  puede retirar su regresión determinista del gate ni convertir un fallo en
  warning.

- [ ] **CI-TEST-003 — Definir la matriz multiplataforma de validación.** Linux
  ejecuta el gate canónico; Linux ARM64, macOS Intel/ARM64 y Windows ejecutan
  tests de plataforma y la parte portable aplicable, además del smoke test de
  los binarios. Toda exclusión se justifica por target o capability.

- [ ] **CI-TEST-004 — Conservar evidencia de fallos reproducibles.** Seeds,
  corpus minimizado, observaciones, logs relevantes y metadatos de target se
  publican como artefactos sin paths físicos, secretos ni estado ambiental
  accidental.

### 16.3 Properties, metamorfismo y fuzzing

- [ ] **PROP-001 — Crear generadores reproducibles y reducibles.** Sustituir
  corpora generados con una única seed fija por generadores que registren la
  seed, puedan reducir el caso fallido y produzcan sintaxis válida, sintaxis
  recuperable y estructuras inválidas controladas bajo presupuestos.

- [ ] **PROP-002 — Generar programas tipados por construcción.** Cubrir
  combinaciones de tipos, operadores, genéricos, traits, patterns, ownership,
  préstamos, control, async y errores sin depender de que el frontend acepte
  ruido aleatorio como programa válido.

- [ ] **META-001 — Añadir properties metamórficas.** Como mínimo: reconstrucción
  CST, idempotencia de formato, alpha-renaming, permutación física de fuentes,
  paréntesis semánticamente neutros, eager frente a COW, presión de GC y
  estabilidad de diagnostics y productos canónicos.

- [ ] **FUZZ-001 — Mantener fuzz targets del frontend.** Lexer, parser y
  formatter deben aceptar bytes no confiables sin panic, no terminación ni
  pérdida de partición; los casos válidos conservan parseo e idempotencia.

- [ ] **FUZZ-002 — Mantener fuzz targets de protocolos.** Manifiesto, lockfile,
  interfaz, artefacto, diagnostics JSON y protocolo del adaptador se decodifican
  bajo límites y nunca consultan entradas ambientales. Todo round-trip canónico
  debe ser estable.

- [ ] **FUZZ-003 — Fuzzear los admission verifiers.** Mutadores estructurados de
  HIR, MIR y bytecode deben explorar tags, índices, CFG, tipos, ownership,
  cleanup y límites. No se introduce por ello un formato bytecode estable en
  disco.

- [ ] **FUZZ-004 — Integrar corpus y campañas.** Cada PR ejecuta smoke fuzzing
  determinista; el tier nocturno amplía tiempo y seeds; todo crash se minimiza,
  se convierte en regresión y entra en el corpus antes de cerrar el defecto.

### 16.4 Modelos, cobertura y resistencia de los tests

- [ ] **MODEL-001 — Modelar valores y colecciones.** Secuencias de operaciones
  sobre `Array`, `Map`, `Set`, `Range`, `String`, slices y copias se comparan
  con modelos puros, incluidos orden, aliasing explícito, errores y límites.

- [ ] **MODEL-002 — Modelar ownership y concurrencia estructurada.** Un modelo
  de estados cubre moves, préstamos, terminales, `defer`, `Join`, cancelación,
  pánico y cleanup. El generador explora transiciones válidas e inválidas y
  verifica la fase exacta que debe rechazarlas.

- [ ] **MODEL-003 — Modelar runtime y host.** GC, ciclos, roots, OOM retry,
  scheduling, pipes y procesos se prueban con umbrales y órdenes perturbados,
  sin convertir contadores privados en semántica observable.

- [ ] **COV-001 — Publicar una baseline de cobertura por riesgo.** Registrar
  líneas, funciones y ramas por crate y, por separado, para parser, checkers,
  verifiers, heap y ejecución. Los umbrales se fijan después de medir la
  baseline; no se excluye código difícil solo para mejorar el porcentaje.

- [ ] **MUT-001 — Medir mutation score en fronteras críticas.** Ejecutar
  mutación automática acotada sobre algoritmos y verifiers; cada mutante
  superviviente se clasifica como test ausente, código equivalente o exclusión
  justificada. El gate posterior impide regresiones del score acordado.

- [ ] **REG-001 — Automatizar la regla de regresión.** Todo bug confirmado
  incorpora el caso mínimo en la frontera pública más baja que habría fallado,
  además de cualquier test interno necesario para localizar la causa.

### Gate H0 — Infraestructura de fiabilidad

- [ ] El gate completo de Tondo 0.1 se ejecuta automáticamente en PR y `main`.
- [ ] El inventario y la matriz normativa se validan sin entradas sin
  clasificar para el target publicado; la extensión 0.2 queda clasificada como
  futura, no omitida ni contada como evidencia verde.
- [ ] Existen generadores con seed reproducible y reducción de fallos.
- [ ] Frontend, protocolos y admission verifiers tienen fuzz targets
  persistentes con corpus versionado.
- [ ] Las familias críticas tienen al menos un modelo o property que compare
  secuencias, no solo ejemplos aislados.
- [ ] Coverage y mutation score publican una baseline revisada y un gate de no
  regresión proporcionado al riesgo.
- [ ] Un fallo de cualquier tier conserva evidencia suficiente para reproducir
  localmente el mismo input y target.
- [ ] El gate estricto y la conformidad continúan verdes después de integrar la
  infraestructura.

---

## 17. M10.6 — Testing de usuario y edición 0.2

**Objetivo:** implementar la extensión
[`TONDO_TESTING_SPEC.md`](./TONDO_TESTING_SPEC.md) como la primera ampliación
de lenguaje posterior a Tondo 0.1. El resultado es una declaración
`test name { ... }` para cada hoja y una declaración `suite name { ... }` para
jerarquía y lifecycle compartido, unit tests con acceso privado controlado,
integration tests contra API pública, control sellado de log/fallo/skip sin
contexto visible y un runner determinista que pueda utilizarse para construir y
validar la stdlib.

**Dependencia:** la implementación comienza únicamente después de Gate H0. El
diseño queda fijado antes para que inventario, matriz normativa y CI conozcan el
próximo contrato, pero no se añade token, parser path, feature flag ni extensión
privada mientras H0 permanezca abierto.

**Compatibilidad:** `TONDO_LANGUAGE_SPEC.md`, su hash normativo y
`tondo-conformance-0.1` permanecen inmutables. Ningún binario puede aceptar
`test` ni `suite` bajo `--edition 0.1`; la nueva superficie se anuncia solo al
seleccionar la edición 0.2.

### 17.1 Spec, edición y plan cerrado

- [x] **UTEST-SPEC-001 — Fijar el contrato normativo de testing.**
  `TONDO_TESTING_SPEC.md` define keywords, grammar, árbol suite/test, formato,
  identidad, source classes, overlays, capturas, lifecycle, envelope,
  `std.testing.log/failNow/skip`, inferencia, aislamiento, resultados, CLI,
  reporte, stdlib boundary, diagnósticos y conformidad sin depender de una
  implementación provisional.

- [ ] **UTEST-EDITION-001 — Materializar Tondo 0.2 como edición separada.**
  Consolidar la especificación de lenguaje 0.2, añadir `suite` y `test` a su
  registry de keywords y diagnósticos y versionar los formatos afectados. La
  edición 0.1 debe seguir seleccionable y byte-compatible; tests de migración
  prueban que identificadores `suite`/`test` válidos en 0.1 reciben `E1005`
  únicamente bajo 0.2.

- [ ] **UTEST-PLAN-001 — Extender el project plan con source classes de test.**
  Representar exactamente `production`, `unit-test` e `integration-test`,
  dev-dependencies, roots, paths lógicos, target, capabilities y límites. El
  hash de un artefacto de producción no puede depender de entradas test-only.

- [ ] **UTEST-DISC-001 — Implementar descubrimiento convencional y explícito.**
  Soportar `_test.to` dentro de production roots y `.to` bajo `tests/`, con la
  precedencia, case-sensitivity, orden y overrides cerrados del spec. Detectar
  colisiones, fuentes sin clasificar, symlink escapes y deriva entre discovery
  y plan materializado antes de compilar.

- [ ] **UTEST-DEPS-001 — Separar dev-dependencies del grafo de producción.**
  Fijarlas por PackageId/hash en lockfile, impedir imports desde producción y
  demostrar mediante interfaces y artefactos comparados que añadir o cambiar
  una dev-dependency no altera el producto publicable. `std.testing` y cualquier
  operación de control deben quedar ausentes de interfaces y productos de
  producción.

### 17.2 Frontend y semántica estática

- [ ] **UTEST-LEX-001 — Añadir `suite` y `test` a las keywords de edición
  0.2.** Lexer, token registry, Unicode/NFC, diagnostics y reconstrucción CST
  deben continuar dependiendo de la edición declarada, nunca de un path físico
  o del comando que invoca el parser.

- [ ] **UTEST-CST-001 — Parsear `test` y `suite` sin pérdida.** Añadir
  `test identifier block` y `suite identifier suite-block` a CST/AST y
  recovery. El suite-block admite setup ordinario seguido solo de miembros
  estáticos; se rechazan modifiers, parámetros, firmas, nodos bajo control de
  flujo, sentencias posteriores al primer miembro y todas las alternativas
  ausentes. Edición 0.1 conserva su parseo anterior.

- [ ] **UTEST-FMT-001 — Formatear suites y tests canónicamente.** Cubrir bodies
  y setups vacíos/multiline, nesting, comentarios, documentación, separación
  setup-members, declaraciones adyacentes, recovery e idempotencia en ambas
  ediciones. `fmt` no depende de discovery runtime.

- [ ] **UTEST-ID-001 — Construir el árbol estático suite/test.** La identidad
  interna usa PackageId + source class + module path + ordered node path + kind;
  la visible usa `package::unit|integration::path::suite...::test`. Registrar
  parents, rechazar suites vacías `E2004`, nombres hermanos duplicados `E2002`
  y cualquier intento de reabrir/mezclar suites. Orden, warnings y source
  ranges deben ser deterministas entre archivos permutados.

- [ ] **UTEST-CAPTURE-001 — Comprobar entornos de suite.** Un descendiente solo
  captura bindings ancestrales `let: Copy + Send + Share` mediante snapshot.
  Rechazar con `E2005` `var`, préstamos, moves, valores afines/terminales y
  cualquier bypass a través de suites anidadas. Constantes y funciones de
  módulo continúan resolviéndose por nombre.

- [ ] **UTEST-OVERLAY-001 — Implementar el overlay unitario sellado.** Resolver
  y comprobar producción primero, después permitir lectura privada y helpers
  privados sin reabrir bodies, añadir exports ni cambiar interfaces. Casos
  negativos deben demostrar que un overlay no repara producción inválida,
  altera coherence o entra en el grafo production.

- [ ] **UTEST-INTEG-001 — Implementar integration roots aislados.** Cada root
  recibe PackageId sintético de consumidor, imports explícitos y únicamente
  visibilidad pública sobre el paquete probado. No existe flag friend ni
  reutilización accidental del scope unitario.

- [ ] **UTEST-CHECK-001 — Inferir el contrato exacto del body.** Comprobar cada
  test como entrada privada `async? fn(): Unit ! E` y cada setup de suite como
  otra entrada privada `async? fn(): Unit ! E` sin `return`; inferir una unión
  cerrada `E: Discard`, admitir `fail`, `?`, `await` y `scope` donde corresponda
  y reutilizar todos los checks ordinarios de tipos, ownership, préstamos,
  terminales, `defer`, `Send`, `Share` y `unsafe`. Resolver el módulo test-only
  `std.testing` con las tres firmas exactas, tipos `Unit`/`Never` y `E2003` al
  cruzar a producción.

### 17.3 Lowering, runtime y CLI

- [ ] **UTEST-LOWER-001 — Bajar entradas de test por el pipeline común.** HIR,
  MIR, bytecode y sus admission verifiers representan árbol/parent, entradas de
  setup, snapshots de entorno, identidad, source span, error, async, cleanup,
  `TestLog`, `TestFailNow` y `TestSkip` sin crear un segundo frontend o una ruta
  de ejecución no verificada. `main` nunca se ejecuta en un test target.

- [ ] **UTEST-CONTROL-001 — Implementar el envelope sellado de ejecución.** Cada
  suite/test recibe node ID, log/stdout/stderr sinks, cancelación y límites en
  estado privado del runtime, nunca como valor o thread-local Tondo. Helpers,
  closures y tasks estructuradas heredan el enlace; verifiers rechazan
  operaciones forjadas o presentes en artefactos de producción. Implementar
  `log`, `failNow` con `P0007`, `skip`, precedencia de cleanup y `P2001` sin
  exponer `TestContext`, `currentTest()` ni identidad del nodo. Un skip de hijo
  marca la entrada completa, cancela el resto del scope y se propaga a la task
  propietaria con la prioridad determinista fijada por el lenguaje.

- [ ] **UTEST-RUNTIME-001 — Ejecutar cada hoja en una raíz aislada.** Estado,
  roots, heap observable, tasks, handles, pánicos, logs, stdout, stderr,
  envelopes y presupuestos no cruzan hojas salvo snapshots de suite comprobados.
  Retorno, skip, error, pánico, resource limit, timeout e infrastructure
  producen exactamente los estados normativos; los terminales cooperativos
  completan unwind y cleanup, mientras una terminación forzada garantiza
  aislamiento sin fingir que ejecutó `defer`.

- [ ] **UTEST-SUITE-001 — Implementar lifecycle jerárquico de suite.** Ejecutar
  setup una vez y solo si existe una hoja seleccionada, conservar su entorno y
  guards, entrar de fuera hacia dentro y hacer teardown de dentro hacia fuera
  después de todos los descendientes. Un fallo de setup bloquea solo su
  subárbol, ejecuta cleanup realmente observable y permite continuar hermanos;
  un fallo de teardown no reescribe resultados ya emitidos. Un skip de setup
  produce `skipped`/`blocked-skip`; un fallo durante su cleanup prevalece y
  convierte descendientes en `blocked-setup`.

- [ ] **UTEST-LIMIT-001 — Hacer límites y timeout terminales reales.** Publicar
  defaults finitos, aplicar `--timeout` por hoja y por fase setup/teardown sin
  contar la espera de descendientes, cargar logs/stdout/stderr al mismo
  presupuesto de output, registrar valores efectivos y garantizar que una
  entrada no cooperativa no continúa tras `timeout`. OOM, abort o pérdida de
  aislamiento nunca se presentan como assertion failure ordinario.

- [ ] **UTEST-CLI-001 — Añadir `tondo test`.** Implementar manifest/default
  discovery, `--filter`, `--exact`, `--list`, `--jobs`, `--timeout`,
  `--test-format`, `--show-output`, `--deny-skips` y `--allow-empty`, incluidas
  exclusiones, parsing estricto y exit codes 0/1/2/3. Primero compila toda la
  suite y el selector solo limita ejecución. `--filter` compara hojas;
  `--exact` acepta una hoja o una suite y en este último caso selecciona su
  subárbol. No implementar `--fail-fast` bajo este contrato.

- [ ] **UTEST-SCHED-001 — Fijar orden y paralelismo observable.** El default
  ejecuta hojas con jobs=1 en orden de ID y respeta el bracketing de suites.
  Jobs explícitos limitan conjuntamente setup/test/teardown y pueden cambiar
  completion order, pero setup precede hijos, teardown los espera y cada
  envelope conserva sus logs/streams. Resultados y reporte final permanecen
  ordenados y nunca intercalan nodos.

- [ ] **UTEST-REPORT-001 — Implementar los formatos machine-readable.**
  Serializar `tondo-test-report-0.2/3` y `tondo-test-list-0.2/3` con arrays
  separados de suites/tests, parents, paths, phase, `blocked_by`, `failure`,
  `skip`, logs, streams, policy deny-skips, selection, resource profile e
  invariantes exactas de summary y status/phase. No incluir reloj, duración,
  PID, paths físicos ni direcciones. Fallos de compilación continúan usando
  diagnostics estructurados y no ejecutan setup ni bodies.

### 17.4 Evidencia, conformidad y dogfooding

- [ ] **UTEST-CONF-001 — Crear una suite de conformidad Tondo 0.2 nueva.** No
  mutar manifests, cases, hashes ni observations de 0.1. La suite 0.2 cubre los
  veintinueve grupos mínimos enumerados por la spec de testing y tiene adaptador
  público para VM y futuros backends.

- [ ] **UTEST-PROJECTS-001 — Añadir proyectos de aceptación completos.**
  Incluir package unitario, integration roots, dev-dependency, suites anidadas,
  servicio compartido, captura válida/inválida, async/error, fallos de
  setup/teardown, `blocked-setup`, log directo/desde helper/task, `failNow`,
  skip de hoja/suite, `blocked-skip`, `P2001`, deny-skips, pánico/cleanup, host
  capabilities, filtros, selección vacía y reporter JSON. Cada proyecto debe
  poder ejecutarse desde una copia en otro path físico con observaciones
  canónicas iguales.

- [ ] **UTEST-PLATFORM-001 — Validar la matriz publicada.** Linux ejecuta el
  gate canónico completo; Linux ARM64, macOS Intel/ARM64 y Windows ejecutan
  discovery, paths jerárquicos, filtros de suite/test, lifecycle, envelopes,
  logs/skips, aislamiento, timeout, captura y reporte aplicables además del
  smoke test de binario.

- [ ] **UTEST-DOGFOOD-001 — Probar componentes Tondo mediante `tondo test`.**
  Antes de Gate T0, mantener una pequeña biblioteca de aceptación escrita en
  Tondo con unit/integration tests y al menos una suite que comparta un recurso
  real. Debe usar `testing.log` desde un helper y probar `failNow`/skip en casos
  de aceptación controlados. No sustituye los tests Rust ni la conformidad;
  demuestra que la experiencia pública funciona sin harness privado.

### Gate T0 — Testing first-class conforme

- [ ] Tondo 0.1, su spec, suite, diagnostics y comportamiento permanecen
  inmutables y pasan Gate H0.
- [ ] La edición 0.2 consolidada incorpora el contrato de testing y solo ella
  reserva `suite` y `test`.
- [ ] Lexer, CST, parser, formatter, HIR, MIR, bytecode y VM recorren la ruta
  común y sus verifiers aceptan o rechazan árboles suite/test con diagnostics
  exactos.
- [ ] Unit overlays ven privados sin alterar producción; integration roots solo
  ven API pública; `std.testing`, dev-dependencies y operaciones test-only nunca
  entran en productos publicables.
- [ ] Cada entrada recibe un envelope no observable ni falsificable que sigue
  frames/tasks y nunca se deriva de un thread-local del host; logs y terminales
  se atribuyen al nodo exacto sin `TestContext` ni `currentTest()`.
- [ ] Suites ejecutan setup una vez solo para subárboles seleccionados, permiten
  únicamente snapshots `let: Copy + Send + Share`, hacen teardown tras todos los
  descendientes y reportan setup, teardown, `blocked-setup`, skip y
  `blocked-skip` sin duplicar causas.
- [ ] Retorno, error, `assert`, `failNow`, skip, pánico, async, cancelación,
  ownership y `defer` conservan cleanup y precedencia; `P2001`, resource limits
  y timeout no esconden cleanup de usuario no observado ni rompen aislamiento.
- [ ] `tondo test` implementa discovery, compilación completa, selección,
  selección exacta de suite/test, ejecución serial/paralela, captura, exit codes
  deny-skips y selección vacía según contrato; no inventa fail-fast.
- [ ] El reporte JSON es canónico y reproducible; la salida humana no intercala
  suites/tests y muestra logs, razones y fallos accionables.
- [ ] La suite Tondo 0.2 pasa en la VM y la matriz de plataformas aplicable está
  verde.
- [ ] Existe dogfooding escrito en Tondo que usa la superficie pública, sin
  registration APIs, `TestContext`, annotations, reflection, subtests dinámicos
  ni hooks ocultos.

---

## 18. STD-0.1 — Core + Hosted Standard Library

**Objetivo:** especificar e implementar la primera API estándar utilizable
sobre la VM antes de fijar decisiones del runtime nativo. La especificación de
la stdlib es independiente de la especificación del lenguaje; una API
ilustrativa no se vuelve pública por aparecer en un ejemplo.

**Dependencia:** no comienza implementación pública hasta cerrar Gates H0 y T0.
El diseño puede investigarse antes, pero ninguna firma se congela ni distribuye
como estable sin sus modelos, tests y contrato de capability. La única excepción
es el núcleo test-only `std.testing.log/failNow/skip`, cuyas firmas y bridge
quedan fijados y ejecutables en T0 porque forman parte del contrato del runner.
STD-0.1 amplía ese mismo módulo; no crea un segundo harness.

### 18.1 Contrato y distribución

- [ ] **STD-SPEC-001 — Crear `TONDO_STANDARD_LIBRARY_SPEC.md`.** Fijar versión,
  módulos, firmas, tipos de error, ownership, complejidad, orden, Unicode,
  bloqueo/suspensión, cancelación, capabilities, disponibilidad por target y
  ejemplos verificables.

- [ ] **STD-MOD-001 — Definir módulos y prelude mínimo.** Imports y nombres
  implícitos deben ser cerrados, deterministas y compatibles con los
  namespaces del lenguaje; no existe extensión global de métodos.

- [ ] **STD-CAP-001 — Versionar la matriz de capabilities.** Core permanece
  target-neutral. Toda API hosted declara su capability y fallo de admisión. La
  release `tondo-vm-hosted` 0.1.0 y su manifiesto `[console, process]` no se
  reescriben al ampliar el registro o añadir otro target/capability set.

- [ ] **STD-ERR-001 — Definir errores portables.** Los errores exponen
  clasificación nominal y datos portables; códigos, mensajes y payloads del SO
  no se convierten accidentalmente en semántica estable.

- [ ] **STD-DIST-001 — Definir distribución reproducible.** Fuentes Tondo,
  unidades privilegiadas y metadatos de la stdlib se fijan por versión y hash,
  entran en el plan cerrado y no requieren red ni búsqueda ambiental durante
  compilación.

### 18.2 Core Standard Library

- [ ] **STD-CORE-001 — Fijar protocolos y operaciones fundamentales.**
  `Option`, `Result`, `Display`, comparación, `Key` y utilidades de
  error conservan las capacidades y efectos ya definidos por el lenguaje.

- [ ] **STD-TEXT-001 — Especificar texto y bytes.** `String`, `Char`, `Byte` y
  `Bytes` fijan construcción, búsqueda, transformación, encoding/decoding,
  invalid UTF-8, límites y costes sin confundir scalar, grapheme ni byte.

- [ ] **STD-COLL-001 — Especificar colecciones.** `Array`, `Map` y `Set` fijan
  consulta, construcción, actualización funcional, mutación explícita,
  capacidad, orden, combinación y complejidad preservando semántica de valor.

- [ ] **STD-ITER-001 — Especificar ranges e iteración.** `Range`, iteradores y
  combinadores usan dispatch estático, un único elemento por target, evaluación
  lazy acotada y consumo/copia visibles.

- [ ] **STD-FMT-001 — Especificar formatting.** Display de tipos compuestos,
  builders y formato estructurado deben reutilizar el protocolo estático sin
  introducir reflection, vtables ni lookup abierto.

- [ ] **STD-TESTING-SPEC-001 — Especificar `std.testing`.** Fijar assertions de
  igualdad, diffs de texto, comparación float con tolerancia, consumo explícito
  de Option/Result, recursos temporales, snapshots y datos generados que entren
  realmente en 0.1. Cada API declara tipos, ownership, cleanup, formato, seed,
  actualización, capabilities y límites; reutiliza sin alterar
  `log/failNow/skip`, no registra tests ni captura pánicos como excepciones
  recuperables.

### 18.3 Hosted Standard Library

- [ ] **STD-CONSOLE-001 — Consolidar consola y streams.** Fijar stdout, stderr,
  entrada, flushing, texto/binario, errores y comportamiento async sin asumir
  terminal interactiva.

- [ ] **STD-PATH-001 — Definir paths portables y nativos.** Separar operaciones
  léxicas de acceso al host, preservar bytes no Unicode cuando el target lo
  admita y no prometer una representación común falsa.

- [ ] **STD-ENV-001 — Definir argumentos y environment.** Acceso explícito,
  snapshots, Unicode/bytes, ausencia y mutación quedan capability-gated; no son
  inputs implícitos de compilación.

- [ ] **STD-FS-001 — Definir filesystem.** Archivos, directorios, metadata,
  enlaces, permisos, atomicidad, iteración y operaciones async declaran
  portabilidad, TOCTOU, cleanup y errores sin esconder bloqueo.

- [ ] **STD-PROC-001 — Estabilizar procesos.** Promover el bridge provisional
  de `Command`, `Pipeline`, `ProcessHandle`, status, output, pipes, shell
  explícito y cancelación a una API versionada que preserve argv exacto.

### 18.4 Implementación y evidencia

- [ ] **STD-IMPL-001 — Implementar Core en Tondo cuando sea posible.** Solo las
  operaciones intrínsecas o dependientes del host permanecen privilegiadas;
  duplicar lógica portable en Rust requiere justificación.

- [ ] **STD-IMPL-002 — Implementar Hosted sobre adaptadores capability-gated.**
  El runtime VM no expone una syscall o handle que la API estándar no haya
  validado y tipado.

- [ ] **STD-TESTING-IMPL-001 — Implementar `std.testing` sobre T0.** Escribir en
  Tondo toda utilidad portable y reutilizar el bridge privilegiado de T0 sin
  duplicarlo. Confinar a unidades privilegiadas únicamente temp resources,
  captura o aislamiento que requieran host. Los helpers producen mensajes
  accionables sin reflection privada y se prueban mediante `tondo test`, no
  mediante registro interno.

- [ ] **STD-TEST-001 — Añadir modelos y properties por módulo.** Cada API prueba
  valores normales, vacíos, límites, errores, composición, ownership,
  determinismo y secuencias generadas. Los ejemplos del spec estándar son
  ejecutables.

- [ ] **STD-CONF-001 — Versionar la conformidad de stdlib.** Extender mediante
  una suite o manifiesto nuevo sin mutar `tondo-conformance-0.1`; otro
  implementador debe poder ejecutar los casos mediante un adaptador público.

- [ ] **STD-DOC-001 — Entregar programas representativos.** Como mínimo,
  transformación de texto, procesamiento de colecciones, copia segura de
  archivos y pipeline de procesos deben usar únicamente APIs publicadas y
  actuar como aceptación y corpus de benchmarks.

### Gate S1 — Standard Library 0.1

- [ ] La spec estándar fija todas las firmas Core + Hosted incluidas en 0.1 y
  clasifica explícitamente lo diferido a STD-0.2.
- [ ] Core se ejecuta sobre la VM sin depender de una ABI nativa.
- [ ] Cada API hosted exige la capability correcta y conserva los claims del
  target Tondo 0.1.0 ya publicado.
- [ ] Modelos, properties, ejemplos y conformidad estándar cubren sus contratos
  positivos, negativos, límites y composición.
- [ ] La distribución es reproducible, cerrada y versionada.
- [ ] Los programas representativos pasan el gate estricto y proporcionan el
  corpus funcional inicial para NATIVE-001 y PERF-001.
- [ ] `std.testing` está especificado, implementado y probado con su propio
  runner público; un proyecto puede escribir tests útiles usando solo
  `assert` y enriquecerlos mediante imports explícitos.
- [ ] No se ha congelado una ABI FFI general ni un layout nativo público.

---

## 19. M11 — Backend nativo y optimización

**Objetivo:** añadir una implementación nativa de producción sin introducir una
segunda semántica. Comienza únicamente después de Gates H0, T0 y S1; la VM, la
conformidad del lenguaje —incluidos test targets— y la conformidad de stdlib son
sus oracles.

### 19.1 Selección y contrato del backend

- [ ] **NATIVE-001 — Elegir backend nativo con una evaluación separada.**
  Comparar Cranelift, LLVM y generación propia usando el MIR real, el corpus de
  conformidad y los programas STD-0.1. Medir soporte de targets, corrección,
  latencia de compilación, rendimiento, memoria, tamaño, debugging,
  distribución, mantenimiento y licencias; registrar la elección en un ADR.

- [ ] **NATIVE-002 — Definir lowering desde MIR sin introducir una segunda
  semántica.** Calls, pánicos, cleanup, ownership, préstamos, async, source maps
  y operaciones checked conservan identidad verificable hasta código nativo.

- [ ] **NATIVE-ABI-001 — Definir una ABI runtime interna y versionada.** Fijar
  únicamente la frontera compilador/runtime necesaria para el backend elegido;
  no prometer todavía ABI FFI, layout de usuario ni name mangling estables.

- [ ] **NATIVE-STD-001 — Implementar la frontera de STD-0.1.** Core y Hosted
  observan la misma API, capabilities, errores y cleanup que en la VM; ninguna
  optimización puede añadir una ruta pública específica del backend.

### 19.2 Oracle diferencial y targets

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
  stdlib y metadatos declaran versiones y checksums; la release no depende de
  paths, reloj ni entorno no declarado.

### 19.3 Runtime y optimización medida

- [ ] **PERF-001 — Definir benchmarks y presupuestos antes de optimizar.**
  Incluir compilación, tamaño, programas STD-0.1, throughput, latencia, memoria,
  retain/release, pausas y workloads adversarios; registrar hardware y entorno.

- [ ] **ARC-001 — Implementar ARC en el runtime nativo.**

- [ ] **ARC-002 — Implementar recolección diferida de ciclos y weak refs
  linealizables.**

- [ ] **ARC-003 — Implementar eliminación de retain/release mediante análisis
  de último uso.**

- [ ] **COW-NATIVE-001 — Portar al runtime nativo la política COW ya validada.**
  Reevaluar con perfiles nativos si conviene ampliar las formas compartibles;
  no duplicar una semántica ni asumir que el layout de la VM será definitivo.

- [ ] **ESCAPE-001 — Implementar escape analysis y stack allocation.**

### 19.4 Tooling posterior al gate nativo

- [ ] **INCR-001 — Añadir compilación incremental conservando resultados
  deterministas.** Una compilación limpia y un cache hit deben producir
  productos y diagnósticos observacionalmente idénticos.

- [ ] **LSP-001 — Construir LSP sobre las consultas semánticas existentes, no
  sobre un segundo frontend.**

### Gate N1 — Backend nativo conforme

- [ ] El backend elegido tiene ADR, targets soportados y ABI runtime interna
  explícitos.
- [ ] Todos los programas admitidos atraviesan el MIR verificado común; no
  existe frontend, type checker ni semántica paralela.
- [ ] El adaptador nativo supera lenguaje y STD-0.1 con observaciones
  compatibles con la VM, incluidos los estados y reportes de `tondo test`.
- [ ] Properties, fuzzing diferencial, GC/ARC/ciclos, async, pánicos y cleanup
  pasan bajo stress y sanitización aplicable.
- [ ] Cada target publicado compila y ejecuta un corpus real sobre hardware del
  target.
- [ ] Las optimizaciones aceptadas aportan una mejora medida y conservan todos
  los oracles.
- [ ] Los paquetes nativos son reproducibles y no prometen una ABI pública no
  especificada.

---

## 20. STD-0.2 — Concurrency + Application Standard Library

**Objetivo:** ampliar la stdlib después del backend nativo conforme, sin
convertir un conjunto de utilidades conveniente en nueva semántica del
lenguaje. Cada módulo puede publicarse de forma incremental solo tras superar
su mini-gate de spec, implementación portable/host, models, properties,
capabilities, documentación y conformidad.

### 20.1 Concurrencia y tiempo

- [ ] **STD-CONC-001 — Especificar canales y selección cancelable.** Tipos,
  cierre, backpressure, fairness declarada, ownership de `T: Send`, cancelación
  y ausencia de una keyword `select` implícita quedan fijados por API.

- [ ] **STD-SYNC-001 — Especificar sincronización compartida.** Mutexes, rwlocks,
  condvars, semáforos y atomics declaran `Send`/`Share`, poisoning si existe,
  orden de memoria y prohibiciones dentro del scheduler.

- [ ] **STD-EXEC-001 — Especificar pools, actores y bridging bloqueante.** La API
  no crea un segundo modelo async ni permite que trabajo host bloquee el
  progreso de tasks Tondo.

- [ ] **STD-TIME-001 — Especificar tiempo y timers.** Separar reloj monotónico y
  civil, declarar resolución, overflow, timezone data, suspensión y
  cancelación; compilación continúa sin consultar reloj.

### 20.2 Aplicación y datos

- [ ] **STD-NET-001 — Especificar networking capability-gated.** Direcciones,
  DNS, sockets, streams, datagrams, TLS boundary, timeouts y cancelación exponen
  errores portables y no realizan I/O implícito.

- [ ] **STD-CODEC-001 — Especificar codecs y texto estructurado.** Encodings,
  Base64 y formatos binarios fijan streaming, límites y tratamiento de input no
  confiable.

- [ ] **STD-JSON-001 — Especificar JSON.** Árbol, parser/serializer, streaming,
  números, duplicados, orden y límites tienen oracle diferencial y corpus de
  interoperabilidad.

- [ ] **STD-REGEX-001 — Especificar regex.** Sintaxis, Unicode, complejidad y
  límites evitan comportamiento exponencial no declarado.

- [ ] **STD-ID-001 — Especificar UUID y generación de identidad.** Entropía,
  reloj y representación se solicitan mediante capabilities explícitas.

- [ ] **STD-LOG-001 — Especificar logging estructurado.** Niveles, fields,
  formato, sinks, backpressure, concurrencia y fallos no alteran el control del
  programa de forma oculta.

### Gate S2 — Standard Library ampliada

- [ ] Cada módulo publicado tiene spec, capability matrix, implementación,
  modelos, properties, fuzzing, ejemplos y conformidad versionada.
- [ ] VM y backend nativo producen observaciones compatibles para todos los
  módulos aplicables.
- [ ] Límites de recursos y tratamiento de inputs no confiables están fijados y
  probados.
- [ ] Los módulos diferidos permanecen ausentes o experimentales de forma
  explícita; ningún nombre ilustrativo se anuncia como estable.

---

## 21. Trabajo transversal

### 21.1 Diagnósticos

Todo milestone debe:

- Emitir el código normativo más específico de la fase fiable más temprana.
- Mantener información estructurada como fuente única; la representación humana
  y JSON son vistas.
- Evitar cascadas que dependan de tipos o ownership inventados.
- Conservar paths lógicos y offsets de bytes.
- Ordenar diagnostics, related y fixes según el apartado 22.6.
- Añadir códigos propios solo bajo un prefijo distinto al registro normativo.

### 21.2 Determinismo

Desde M0:

- No depender del iteration order de hash maps internos para output observable.
- Ordenar símbolos, diagnostics, módulos e instanciaciones explícitamente.
- No leer red, reloj, locale o entorno como input implícito.
- Mantener paths físicos fuera de hashes y diagnostics normativos.
- Sembrar aleatoriedad de tests de forma reproducible y registrar la seed al
  fallar.

### 21.3 Testing

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
programas Tondo; STD-0.1, M11 y STD-0.2 deben extender ambas fronteras, no crear
harnesses paralelos.

### 21.4 Seguridad y robustez

- Tratar fuente, bytecode, interfaces y manifiestos como inputs no confiables.
- Validar bytecode aunque lo haya producido el propio compilador.
- Evitar recursión del host sin límite al recorrer sintaxis o tipos.
- Limitar tamaño de instanciación genérica y resolución de traits.
- No ejecutar comandos durante compilación.
- No consultar red durante compilación.
- Mantener shell explícito y separado de argumentos.
- Probar parser, loader y JSON con fuzzing.

### 21.5 Rendimiento

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

### 21.6 Disciplina de librería estándar

La stdlib continúa siendo una especificación separada. El compilador solo debe
anticipar lo que el lenguaje ya declara intrínseco. STD-0.1 y STD-0.2 convierten
el siguiente orden en milestones con gates; esta sección conserva las reglas
que se aplican a ambos.

Orden recomendado:

1. **Bootstrap host shim:** `std.console.print`, únicamente para ejecutar los
   primeros programas.
2. **Core stdlib spec:** métodos exactos de `String`, `Array`, `Map`, `Set`,
   `Range`, iterators, formatting, `Bytes` y helpers portables de testing.
3. **Hosted stdlib spec:** consola, environment, paths, filesystem y procesos.
4. **Concurrency stdlib spec:** channels, mutexes, atomics, actors y pools.
5. **Application stdlib:** time, networking, codecs, JSON, regex, UUID y
   logging.

Los nombres ilustrativos del spec del lenguaje no deben implementarse como API
pública definitiva hasta ser fijados por la especificación estándar.

---

## 22. Registro de riesgos

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
| `R-018` | Diseñar runtime/ABI antes de STD-0.1 | Host calls y layouts condicionan una API todavía provisional | Spec estándar y corpus real antes de seleccionar backend |
| `R-019` | Convertir la stdlib 0.1 en un proyecto ilimitado | El backend queda bloqueado por APIs de aplicación no esenciales | STD-0.1 contiene solo Core + Hosted; Concurrency/Application quedan en STD-0.2 |
| `R-020` | Añadir `suite`/`test` retroactivamente a la edición publicada | Rompe identificadores válidos, hashes y conformidad 0.1 | Keywords exclusivas de 0.2, spec y conformidad nuevas; 0.1 permanece inmutable |
| `R-021` | Permitir que unit tests cambien la compilación de producción | Código solo correcto bajo test y artefactos distintos | Unidad production sellada antes del overlay y comparación exacta de productos |
| `R-022` | Ocultar flakiness mediante paralelismo o retries | Suites verdes no reproducibles | jobs=1 y una ejecución por default; orden/reportes canónicos y paralelismo explícito |
| `R-023` | Convertir testing en attributes, reflection, context parameters y hooks especiales | Segundo sublenguaje con boilerplate y semántica oculta | Dos roles canónicos: `suite` contenedor y `test` hoja; envelope sellado sin valor visible; helpers, fixtures y doubles como Tondo/stdlib ordinarios |
| `R-024` | Convertir suites en globals mutables u orden implícito | Data races, tests dependientes y resultados distintos bajo `--exact` | Capturas `let: Copy + Send + Share`, ownership del recurso en la suite, sin dependencias ni orden entre hojas y lifecycle reportado |
| `R-025` | Implementar logs/control con un global o thread-local | Eventos atribuidos al test incorrecto bajo async, migración o paralelismo | Envelope por raíz que sigue frames/tasks; operaciones selladas y revalidadas por HIR/MIR/bytecode |
| `R-026` | Permitir que skips escondan regresiones o cleanup fallido | CI verde con cobertura real ausente o recursos sin cerrar | Razón obligatoria, sin ignored estático, cleanup antes de confirmar, fallo con precedencia y `--deny-skips` |

---

## 23. Cola inmediata

Estas son las siguientes acciones históricas en orden; G2 ya habilita avanzar a
M4 sin adelantar trabajo de ownership o async.

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
14. [ ] Ejecutar **TEST-001** y crear el inventario machine-readable.
15. [ ] Ejecutar **TEST-002** y **TEST-003** para materializar trazabilidad y
    dimensiones normativas.
16. [ ] Ejecutar **CI-TEST-001** a **CI-TEST-004** y convertir el gate existente
    en evidencia continua.
17. [ ] Añadir generadores, properties, fuzz targets y modelos de M10.5.
18. [ ] Medir coverage y mutation score, cerrar huecos críticos y superar H0.
19. [ ] Implementar M10.6 desde **UTEST-EDITION-001** hasta **UTEST-DOGFOOD-001**
    y superar T0 sin mutar Tondo 0.1.
20. [ ] Comenzar STD-0.1 por **STD-SPEC-001**, incluida la definición exacta
    de `std.testing`, nunca por implementación ad hoc.
21. [ ] Cerrar S1 y comenzar M11 por **NATIVE-001**.
22. [ ] Cerrar N1 antes de iniciar los módulos de STD-0.2.

La ruta autorizada siguiente es:

~~~text
TEST-001
  -> TEST-002/003
  -> CI-TEST-001..004
  -> properties + fuzzing + modelos + métricas
  -> Gate H0
  -> UTEST-EDITION-001..UTEST-DOGFOOD-001
  -> Gate T0
  -> STD-SPEC-001
  -> Gate S1
  -> NATIVE-001
~~~

M4, M5, M6, M7, M8, M9, M10 y Gates G4/G5 quedan cerrados. NATIVE-001 deja de
ser la acción inmediata: M10.6 permanece bloqueado por H0 y M11 por H0, T0 y
S1. No se inicia STD-0.2 antes de Gate N1.

---

## 24. Historial del tracker

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
  conformidad. La release Tondo 0.1.0 existente no se reescribe.
- M11 conserva su ID histórico, pero NATIVE-001 depende ahora de H0 y S1. La VM
  y STD-0.1 se convierten en oracle y corpus diferencial del backend nativo.
- Concurrency + Application Standard Library se separa como STD-0.2 después de
  Gate N1 para impedir que APIs no esenciales bloqueen la implementación
  nativa.
- La cola inmediata comienza en TEST-001; este cambio reorganiza trabajo y no
  modifica semántica, compilador, runtime ni claims de conformidad publicados.

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
  Rust/Cargo 1.93.0. La release se publica como `v0.1.0`; M11 queda fuera de
  esta entrega.

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
