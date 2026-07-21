# Tondo: tracker de implementación

**Estado:** activo  
**Versión del tracker:** 0.31
**Última actualización:** 2026-07-21  
**Especificación base:** [Tondo 0.1-draft.8](./TONDO_LANGUAGE_SPEC.md)  
**Objetivo inmediato:** derivar `Call`, `CallMut` y `CallOnce` (CALL-003).

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
| **M4 — Genéricos, traits y closures** | Sistema estático completo | En curso |
| **M5 — Ownership, préstamos y memoria** | Modelo de valores completo | Pendiente |
| **M6 — Colecciones, números y texto** | Gate G3: alpha utilizable | Pendiente |
| **M7 — Async y concurrencia estructurada** | Tasks conformes | Pendiente |
| **M8 — Scripts y procesos** | Experiencia de scripting | Pendiente |
| **M9 — Unsafe, targets y toolchain** | Gate G4: preview 0.1 | Pendiente |
| **M10 — Conformidad y release** | Gate G5: Tondo 0.1 | Pendiente |
| **M11 — Backend nativo y optimización** | Implementación de producción | Futuro |

Estado observado del workspace:

- Repositorio local: `/tmp/tondo`, branch `main`, con upstream en
  `github.com/tonyredondo/tondo`.
- Workspace: `tondo-cli`, `tondo-compiler` y `tondo-vm`.
- Toolchain utilizado para la validación: Rust 1.93.0 y Cargo 1.93.0; la versión
  mínima soportada aún no está fijada.
- Última validación: 2026-07-21, con formatter check, Clippy sin warnings,
  324 tests, Rustdoc sin warnings, metadatos locked y smoke tests de la CLI
  correctos.

### 4.1 Ruta crítica

~~~text
M0 -> M1 -> M2 -> M3 -> M4 -> M5 -> M6 -> M7 -> M8 -> M9 -> M10
                         \______________________________________/
                           feedback continuo hacia spec y tests
~~~

M4, M5 y M6 pueden investigarse conjuntamente, pero deben integrarse en ese
orden para evitar que collections o closures introduzcan una semántica de copia
incompatible con ownership.

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
| 16. Mutabilidad, memoria y concurrencia | M5, M7 y M9 | G3, G4 y M10 |
| 17. Operadores | M2, M6 y M8 | G3, G4 y M10 |
| 18. Semántica numérica | M3 y M6 | G3 y M10 |
| 19. Texto y Unicode | M1 para léxico; M6 para runtime | G0, G3 y M10 |
| 20. Ejecutables, scripts y procesos | M3, M7, M8 y M9 | G2, G4 y M10 |
| 21. Formato y documentación | M1 y trabajo transversal | G0 y M10 |
| 22. Diagnósticos y tooling | M0, M1, M2, M9 y M10 | Todos los gates |
| 23. Gramática de referencia | M1 | G0 y M10 |
| 24. Ejemplos integrados | Tests de aceptación progresivos | G2, G3, G4 y M10 |
| 25. Características ausentes | Compile-fail distribuido por milestone | M10 |
| 26. Frontera con la stdlib | M6, M8 y spec separada de stdlib | G3, G4 y M10 |

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

- [ ] **CALL-003 — Derivar `Call`, `CallMut` y `CallOnce` desde cuerpo y
  capturas.**

- [ ] **CALL-004 — Implementar closures sync, async y unsafe en la
  representación semántica, aunque sus runtimes se activen después.**

Evidencia observada el 2026-07-21 para GEN-001, GEN-002, TRAIT-001 a TRAIT-006,
CAP-001, CALL-001 y CALL-002:

- Los bodies genéricos bounded y unbounded se comprueban una sola vez con
  parámetros rígidos. Las llamadas explícitas e inferidas cierran todas las
  variables invariantes y pueden reenviar el binder exterior en tipos
  compuestos como `T?` y `Array[T]`.
- Cada especialización valida sus bounds antes de publicar HIR. `Copy`,
  `Discard`, `Equatable`, `Key`, `Send` y `Share` comparten una prueba
  estructural cerrada; traits fuente, `Display` e `Iterator[T]` usan selección
  estática y prueba recursiva. `Call`, `CallMut` y `CallOnce` permanecen en las
  tareas de closures.
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
  nunca usa el terminador intrínseco; la legalidad final del préstamo `mut`
  pertenece a BORROW-001 en M5.
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
- Cada expresión de cierre sync-safe publica un tipo generado distinto, una
  firma exacta, binders heredados, parámetros completos, un body HIR separado y
  capturas sintácticas ordenadas por `LocalId`. El outcome se infiere sobre
  todos los caminos alcanzables y las closures anidadas conservan problemas de
  inferencia independientes.
- Las capturas conservan `let`/`var`, copian un snapshot owned y propagan free
  uses de closures anidadas. Préstamos `ref`/`mut`/`var` y el receiver prestado
  producen `E1402`; parámetros variádicos exigen nombre y conservan elemento en
  la firma y `Array[T]` dentro del body.
- `Copy`, `Discard`, `Send` y `Share` se derivan componente a componente desde
  las capturas sustituidas; `Equatable` y `Key` se rechazan. El bootstrap
  ejecutable permanece deliberadamente limitado a capturas `Copy`; OWN-006 y
  OWN-007 añadirán moves, disponibilidad y obligaciones afines.
- El admission verifier exige correspondencia uno-a-uno entre metadata y
  expresión, identidad generada, firma/body, capacidad `Copy` y tipo,
  mutabilidad y binding de cada captura. MIR sólo admite copias directas del
  local exterior exacto; bytecode vuelve a comprobar el esquema concreto del
  entorno.
- La VM construye, copia, traza y snapshottea entornos gestionados. Una pila de
  raíces temporales protege capturas compuestas cuando el GC se dispara a mitad
  de una construcción o copia multi-captura.
- Un tipo `fn(...)` esperado ya puede inferir la firma fuente, pero la coerción
  del tipo concreto, la invocación y los protocolos cerrados permanecen en
  CALL-003. Capturas no `Copy` quedan incompletas para M5 y closures
  `async`/`unsafe` para CALL-004, sin semántica provisional ni falso `E1102`.
- El gate acumulado pasa 418 tests, `git diff --check`, formatter check, build
  de todos los targets, Clippy con warnings denegados y Rustdoc con warnings
  denegados.

### Gate de salida de M4

- No existe lookup global abierto de métodos.
- La selección de un `impl` es única y determinista.
- Los casos de overlap, orphan rules y ciclos de constraints tienen sus
  diagnósticos normativos.
- Closures con capturas `Copy + Discard` y genéricos se ejecutan en la VM a
  través del bytecode normal; M5 elimina esa restricción bootstrap aplicando
  moves y obligaciones a capturas afines.
- La monomorfización tiene límites controlados y no puede divergir.

---

## 10. M5 — Ownership, préstamos y gestión automática

**Objetivo:** implementar el modelo que hace a Tondo seguro y predecible sin
lifetimes escritos por el usuario.

### 10.1 Valores y disponibilidad

- [ ] **OWN-001 — Derivar `Copy` y `Discard` para tipos compuestos.**

- [ ] **OWN-002 — Implementar moves de valores no `Copy`.**

- [ ] **OWN-003 — Implementar disponibilidad por flujo.** Un binding debe estar
  disponible en todos los caminos que llegan a cada uso.

- [ ] **OWN-004 — Permitir reposición completa de un `var` movido.**

- [ ] **OWN-005 — Implementar moves parciales y sus restricciones.**

- [ ] **OWN-006 — Implementar captura de closures respetando copia o move.**

- [ ] **OWN-007 — Completar las capacidades derivadas de closures con capturas
  afines y eliminar la restricción bootstrap de M4.**

### 10.2 Préstamos

- [ ] **BORROW-001 — Implementar préstamos `ref`, `mut` y `var` sobre MIR.**

- [ ] **BORROW-002 — Calcular regiones por último uso sin lifetimes de fuente.**

- [ ] **BORROW-003 — Distinguir observación compartida, mutación de extensión
  fija y mutación estructural.**

- [ ] **BORROW-004 — Implementar disjunción estática de regiones de colección.**

- [ ] **BORROW-005 — Insertar checks runtime únicamente cuando el solapamiento
  dependa de datos.**

- [ ] **BORROW-006 — Rechazar préstamos que crucen suspensión o fronteras no
  permitidas.**

### 10.3 Recursos terminales y cleanup

- [ ] **TERM-001 — Implementar el registro cerrado de tipos terminales.**

- [ ] **TERM-002 — Rastrear obligaciones de consumo en todos los caminos
  normales.**

- [ ] **TERM-003 — Implementar `defer` LIFO y desarme al registrar guards
  terminales.**

- [ ] **TERM-004 — Implementar acciones de unwind cerradas para pánico,
  cancelación y teardown estructurado.**

- [ ] **TERM-005 — Probar que cleanup explícito y unwind fallback nunca se
  ejecutan ambos.**

### 10.4 Memoria e identidad

- [ ] **GC-001 — Extender el collector bootstrap a todas las formas
  administradas.** Incluir environments, `Ref`, collections y frames
  suspendibles mediante descriptors de trazado verificables.

- [ ] **GC-002 — Mantener roots en frames, environments, VM host handles y
  estado estructurado.**

- [ ] **GC-003 — Trazar ciclos y recuperar objetos inalcanzables bajo presión.**

- [ ] **GC-004 — Recolectar antes de declarar OOM por heap y reintentar una
  vez.**

- [ ] **REF-001 — Implementar `Ref[T]` con identidad estable y contenido
  trazable.**

- [ ] **REF-002 — Implementar igualdad y `Key` por identidad de `Ref[T]`.**

- [ ] **VALUE-001 — Implementar inicialmente copia lógica eager para valores
  `Copy` compuestos.**

- [ ] **VALUE-002 — Crear tests de equivalencia que permitan sustituir copia
  eager por COW posteriormente sin cambiar observables.**

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

- [ ] **ARRAY-001 — Implementar `Array[T]` con longitud runtime.**

- [ ] **ARRAY-002 — Implementar indexación positiva y negativa con bounds.**

- [ ] **ARRAY-003 — Implementar slicing y normalización de extremos.**

- [ ] **ARRAY-004 — Implementar snapshots lógicos de slices.** La
  representación inicial puede copiar.

- [ ] **ARRAY-005 — Implementar mutación `mut` de extensión fija y `var`
  estructural.**

- [ ] **ARRAY-006 — Implementar aritmética array-array y array-escalar con
  reglas de forma exactas.**

- [ ] **ARRAY-007 — Implementar concatenación y repetición mediante operaciones
  nombradas cuando la stdlib las fije, no mediante nuevos significados de `+`.**

### 11.2 Map, Set, Range e Iterator

- [ ] **MAP-001 — Implementar `Map[K, V]` con orden observable de inserción.**

- [ ] **MAP-002 — Implementar lookup, inserción, reemplazo y eliminación
  preservando el orden normativo.**

- [ ] **MAP-003 — Implementar igualdad independiente del layout interno.**

- [ ] **SET-001 — Implementar `Set[K]` y pertenencia.**

- [ ] **RANGE-001 — Implementar ranges y sus límites de overflow.**

- [ ] **ITER-001 — Implementar el protocolo estático `Iterator[T]` con un único
  elemento por target.**

- [ ] **ITER-002 — Implementar `for`, `for ref`, `for mut` y `for var` sobre las
  fuentes permitidas.**

### 11.3 Numéricos

- [ ] **NUM-001 — Implementar todos los enteros y floats intrínsecos.**

- [ ] **NUM-002 — Implementar la tabla cerrada de conversiones.**

- [ ] **NUM-003 — Implementar overflow, división, resto, shifts y bitwise.**

- [ ] **NUM-004 — Conservar semántica IEEE sin fast-math observable.**

- [ ] **NUM-005 — Implementar `NumericConversionError` y su clasificación
  estable.**

### 11.4 Texto

- [ ] **TEXT-001 — Implementar `String` UTF-8 inmutable.**

- [ ] **TEXT-002 — Implementar longitud, indexación y slicing por escalares
  Unicode según el spec.**

- [ ] **TEXT-003 — Implementar `Char`, escapes e interpolación mediante
  `Display`.**

- [ ] **TEXT-004 — Separar claramente texto y `Byte`; `Bytes` permanece en la
  stdlib.**

### 11.5 Variádicos y spread

- [ ] **VARIADIC-001 — Implementar variádico homogéneo final `...T`.**

- [ ] **VARIADIC-002 — Implementar spread `...array` y materialización lógica de
  `Array[T]`.** La optimización como vista temporal puede esperar.

### 11.6 Optimización posterior al gate de corrección

- [ ] **OPT-COW-001 — Medir el coste de copia eager con workloads reales.**

- [ ] **OPT-COW-002 — Introducir storage compartido y `is_unique` solo si el
  perfil demuestra valor.**

- [ ] **OPT-COW-003 — Ejecutar los mismos tests observables contra copia eager y
  COW.**

### Gate G3

- Los ejemplos síncronos seguros de los capítulos 24.1 a 24.13 y 24.15 se
  compilan o se clasifican explícitamente si dependen de una API de stdlib aún
  provisional.
- Arrays, maps y sets conservan semántica de valor.
- El orden de `Map` es determinista.
- Los operadores numéricos y vectorizados respetan tipos, forma y orden de
  evaluación.
- El runtime recupera ciclos que atraviesen `Ref`, closures o collections.
- La suite completa del núcleo síncrono seguro pasa.

---

## 12. M7 — Async y concurrencia estructurada

**Objetivo:** implementar suspensión y concurrencia sin futures implícitos,
tasks detached ni wrappers visibles en las firmas.

- [ ] **ASYNC-001 — Typecheckear funciones y closures async.**

- [ ] **ASYNC-002 — Exigir `await` o `spawn` al invocar trabajo async.**

- [ ] **ASYNC-003 — Prohibir préstamos y parámetros incompatibles a través de
  suspensión.**

- [ ] **ASYNC-004 — Transformar MIR async en frames suspendibles.**

- [ ] **EXEC-001 — Implementar executor cooperativo single-thread.**

- [ ] **EXEC-002 — Definir wakeups idempotentes y garantía de progreso.**

- [ ] **SCOPE-001 — Implementar `scope` como propietario estructurado.**

- [ ] **SPAWN-001 — Implementar `spawn` y `Join[T, E]`.**

- [ ] **JOIN-001 — Tratar `Join` como obligación terminal y consumirlo mediante
  `await`.**

- [ ] **CANCEL-001 — Implementar cancelación cooperativa en los puntos
  normativos.**

- [ ] **CANCEL-002 — Implementar cleanup de hijos al salir del scope.**

- [ ] **PANIC-ASYNC-001 — Propagar pánicos de tareas según el contrato
  estructurado.**

- [ ] **SEND-001 — Comprobar `Send` en transferencia a tasks.**

- [ ] **SHARE-001 — Comprobar `Share` para observación concurrente.**

- [ ] **MAIN-ASYNC-001 — Implementar `async fn main` y scope raíz.**

- [ ] **CONC-TEST-001 — Crear litmus tests con resultados permitidos y
  prohibidos, no con scheduling esperado.**

### Gate de salida de M7

- Ningún hijo sobrevive a su scope.
- Todo `Join` se consume o recibe cleanup estructurado.
- Cancelación no aparece como variante implícita de `E`.
- El executor de un hilo satisface progreso cooperativo.
- El código no depende del orden concreto de scheduling.
- Los roots de frames suspendidos permanecen vivos.

---

## 13. M8 — Scripts, comandos y procesos

**Objetivo:** hacer de Tondo un lenguaje cómodo para scripting sin introducir
shell implícito ni efectos de importación.

### 13.1 Script raíz

- [ ] **SCRIPT-001 — Implementar sentencias top-level solo en el archivo raíz
  del modo script.**

- [ ] **SCRIPT-002 — Construir un `main` privado implícito.**

- [ ] **SCRIPT-003 — Inferir localmente la unión cerrada de errores del script.**

- [ ] **SCRIPT-004 — Convertir el `main` implícito en async cuando aparezca
  `await` o `scope` top-level.**

- [ ] **SCRIPT-005 — Prohibir importar un script y mezclarlo con `main`
  explícito.**

- [ ] **SCRIPT-006 — Implementar shebang sin convertirlo en sintaxis de módulo.**

### 13.2 Command y Pipeline

- [ ] **PROC-001 — Implementar `Command` y `Pipeline` como planes inertes
  `Copy + Send + Share`.**

- [ ] **PROC-002 — Implementar únicamente las cuatro combinaciones cerradas de
  `|`.**

- [ ] **PROC-003 — Garantizar que construir un plan no inicia procesos.**

- [ ] **PROC-004 — Definir en la stdlib las operaciones terminales `start`,
  `status`, `output`, `run` y `check` antes de implementarlas públicamente.**

- [ ] **PROC-005 — Pasar programa y argumentos sin parsing de shell.**

- [ ] **PROC-006 — Ofrecer shell solo mediante una API nombrada y explícita.**

- [ ] **PROC-007 — Modelar handles, streams y ownership one-shot como recursos
  terminales.**

- [ ] **PROC-008 — Integrar cancelación y cleanup con el scope raíz.**

- [ ] **PROC-009 — Traducir exit status y errores de spawn a tipos nominales de
  stdlib.**

- [ ] **PROC-010 — Rechazar la API antes de ejecutar cuando el target no
  anuncie capacidad `process`.**

### Gate de salida de M8

- El ejemplo 24.17 funciona sin invocar un shell implícito.
- Un import nunca ejecuta código.
- No quedan procesos huérfanos al terminar, cancelar o panicar un scope.
- Los argumentos conservan exactamente sus caracteres.
- Los pipes aplican backpressure y no bloquean el executor cooperativo.

---

## 14. M9 — Unsafe, targets, interfaces y toolchain

**Objetivo:** completar la superficie 0.1 y alcanzar G4 sin prometer una ABI que
el lenguaje excluye.

### 14.1 Unsafe y Pointer

- [ ] **UNSAFE-001 — Implementar funciones, closures y bloques `unsafe`.**

- [ ] **UNSAFE-002 — Permitir operaciones de `Pointer[T]` únicamente dentro de
  una frontera unsafe válida.**

- [ ] **UNSAFE-003 — Comprobar estáticamente toda precondición comprobable.**

- [ ] **UNSAFE-004 — Documentar la lista cerrada de comportamiento indefinido
  que puede introducir una operación raw.**

- [ ] **UNSAFE-005 — Impedir que código seguro observe direcciones como
  identidad ordinaria.**

- [ ] **FFI-001 — Diseñar unidades privilegiadas y wrappers nativos sin añadir
  atributos semánticos generales a `.to`.**

### 14.2 Targets y capacidades

- [ ] **TARGET-001 — Implementar edición, target, perfil y capacidades como
  inputs explícitos.**

- [ ] **TARGET-002 — Resolver source sets antes de lexear.**

- [ ] **TARGET-003 — Rechazar imports o APIs ausentes para el target.**

- [ ] **TARGET-004 — Registrar target, perfil, capacidades, features y source
  sets en artefactos e interfaces.**

### 14.3 Paquetes e interfaces

- [ ] **PKG-001 — Escribir la especificación separada del manifiesto y
  lockfile.**

- [ ] **PKG-002 — Implementar resolución cerrada y offline durante
  compilación.**

- [ ] **PKG-003 — Fijar aliases locales y PackageIds transitivos exactos.**

- [ ] **IFACE-001 — Definir el formato versionado de interfaces compiladas.**

- [ ] **IFACE-002 — Incluir hash de API, edición, target y dependencias.**

- [ ] **IFACE-003 — Rechazar interfaces incompatibles antes del type checking
  consumidor.**

- [ ] **BUILD-001 — Verificar builds deterministas bajo entradas idénticas.**

- [ ] **BUILD-002 — Verificar que la compilación no consulta red, reloj ni
  entorno no declarados.**

### Gate G4

- Toda sintaxis y semántica de fuente 0.1 tiene una ruta implementada.
- El target VM `hosted` declara exactamente sus capacidades.
- Las capacidades ausentes fallan en compilación.
- Código seguro permanece libre de UB.
- Las interfaces incompatibles no se enlazan por parecido nominal.
- Los ejemplos integrados del spec se compilan con sus fixtures o stdlib
  correspondiente.

---

## 15. M10 — Suite de conformidad y release 0.1

**Objetivo:** convertir la afirmación “implementamos Tondo” en evidencia
versionada y reproducible.

### 15.1 Construcción de `tondo-conformance-0.1`

- [ ] **CONF-001 — Crear un manifiesto versionado y machine-readable de casos.**

- [ ] **CONF-002 — Extraer y clasificar fences normativos del spec.**

- [ ] **CONF-003 — Implementar fixtures del apéndice C sin exponerlos a
  programas normales.**

- [ ] **CONF-004 — Crear grupo de lexing, parsing y formato.**

- [ ] **CONF-005 — Crear grupos compile-pass y compile-fail.**

- [ ] **CONF-006 — Crear grupo de consultas semánticas y fixes JSON.**

- [ ] **CONF-007 — Crear grupo runtime.**

- [ ] **CONF-008 — Crear grupo de concurrencia.**

- [ ] **CONF-009 — Crear grupo `hosted`.**

- [ ] **CONF-010 — Crear adaptador privado de memoria.** Debe probar roots,
  ciclos, presión y reintento previo a OOM usando el collector real.

### 15.2 Cobertura

- [ ] **DIAG-001 — Tener al menos un caso primario para cada código `E`.**

- [ ] **DIAG-002 — Tener casos positivos que demuestren que cada check no
  rechaza programas vecinos válidos.**

- [ ] **WARN-001 — Cubrir el perfil de warnings `core`.**

- [ ] **PANIC-001 — Cubrir cada clase normativa `P`.**

- [ ] **FMT-CONF-001 — Validar resultados byte a byte e idempotencia.**

- [ ] **QUERY-CONF-001 — Validar schema, IDs, orden, spans, related y fixes.**

- [ ] **DETERMINISM-001 — Repetir builds con orden físico de archivos
  perturbado.**

- [ ] **MEM-CONF-001 — Probar reachability y ciclos bajo presión.**

- [ ] **CONC-CONF-001 — Repetir litmus tests con límites calibrados.**

### 15.3 Release

- [ ] **REL-001 — Publicar matriz exacta de target, perfil y capacidades.**

- [ ] **REL-002 — Publicar versión de compilador, formatter, edición y suite.**

- [ ] **REL-003 — Publicar resultados reproducibles de conformidad.**

- [ ] **REL-004 — Documentar limitaciones que no contradigan capacidades
  anunciadas.**

- [ ] **REL-005 — Verificar que no existe modo oculto que relaje checks.**

- [ ] **REL-006 — Congelar el formato público de diagnostics JSON 0.1.**

- [ ] **REL-007 — Etiquetar Tondo 0.1 únicamente después de superar todos los
  grupos aplicables.**

### Gate G5

- La versión exacta del toolchain pasa `tondo-conformance-0.1`.
- El target y sus capacidades están declarados.
- No hay exclusiones sin justificar por capacidad.
- Los artefactos, resultados y versiones pueden reproducirse.
- La documentación no afirma soporte más amplio que la evidencia.

---

## 16. M11 — Backend nativo y optimización

Este milestone comienza después de que el bytecode VM haya estabilizado la
semántica. No bloquea el primer compilador ni necesariamente la primera
implementación conforme.

- [ ] **NATIVE-001 — Elegir backend nativo con una evaluación separada.**
  Comparar Cranelift, LLVM y generación propia usando el MIR real.

- [ ] **NATIVE-002 — Definir lowering desde MIR sin introducir una segunda
  semántica.**

- [ ] **ARC-001 — Implementar ARC en el runtime nativo.**

- [ ] **ARC-002 — Implementar recolección diferida de ciclos y weak refs
  linealizables.**

- [ ] **ARC-003 — Implementar eliminación de retain/release mediante análisis
  de último uso.**

- [ ] **COW-001 — Implementar COW para strings y collections.**

- [ ] **ESCAPE-001 — Implementar escape analysis y stack allocation.**

- [ ] **INCR-001 — Añadir compilación incremental conservando resultados
  deterministas.**

- [ ] **LSP-001 — Construir LSP sobre las consultas semánticas existentes, no
  sobre un segundo frontend.**

- [ ] **PERF-001 — Definir benchmarks representativos y presupuestos antes de
  optimizar.**

---

## 17. Trabajo transversal

### 17.1 Diagnósticos

Todo milestone debe:

- Emitir el código normativo más específico de la fase fiable más temprana.
- Mantener información estructurada como fuente única; la representación humana
  y JSON son vistas.
- Evitar cascadas que dependan de tipos o ownership inventados.
- Conservar paths lógicos y offsets de bytes.
- Ordenar diagnostics, related y fixes según el apartado 22.6.
- Añadir códigos propios solo bajo un prefijo distinto al registro normativo.

### 17.2 Determinismo

Desde M0:

- No depender del iteration order de hash maps internos para output observable.
- Ordenar símbolos, diagnostics, módulos e instanciaciones explícitamente.
- No leer red, reloj, locale o entorno como input implícito.
- Mantener paths físicos fuera de hashes y diagnostics normativos.
- Sembrar aleatoriedad de tests de forma reproducible y registrar la seed al
  fallar.

### 17.3 Testing

La pirámide prevista:

1. Unit tests de estructuras y algoritmos.
2. Golden tests de lexer, CST, formatter y diagnostics.
3. Compile-pass y compile-fail.
4. Runtime tests contra programas Tondo.
5. Property tests y fuzzing.
6. Tests de regresión para cada bug.
7. Suite oficial de conformidad.

Cada bug semántico debe terminar con un programa Tondo mínimo que habría fallado
antes de la corrección.

### 17.4 Seguridad y robustez

- Tratar fuente, bytecode, interfaces y manifiestos como inputs no confiables.
- Validar bytecode aunque lo haya producido el propio compilador.
- Evitar recursión del host sin límite al recorrer sintaxis o tipos.
- Limitar tamaño de instanciación genérica y resolución de traits.
- No ejecutar comandos durante compilación.
- No consultar red durante compilación.
- Mantener shell explícito y separado de argumentos.
- Probar parser, loader y JSON con fuzzing.

### 17.5 Rendimiento

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

### 17.6 Librería estándar

La stdlib completa continúa siendo una especificación separada. El compilador
solo debe anticipar lo que el lenguaje ya declara intrínseco.

Orden recomendado:

1. **Bootstrap host shim:** `std.console.print`, únicamente para ejecutar los
   primeros programas.
2. **Core stdlib spec:** métodos exactos de `String`, `Array`, `Map`, `Set`,
   `Range`, iterators, formatting y `Bytes`.
3. **Hosted stdlib spec:** consola, environment, paths, filesystem y procesos.
4. **Concurrency stdlib spec:** channels, mutexes, atomics, actors y pools.
5. **Application stdlib:** time, networking, codecs, JSON, regex, UUID y
   logging.

Los nombres ilustrativos del spec del lenguaje no deben implementarse como API
pública definitiva hasta ser fijados por la especificación estándar.

---

## 18. Registro de riesgos

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

---

## 19. Cola inmediata

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

La siguiente acción activa es CALL-003: derivar `Call`, `CallMut` y `CallOnce`
desde el body y las capturas ya representadas por CALL-002.

---

## 20. Historial del tracker

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
