# Tondo Standard Library: especificación base

**Línea de desarrollo de la librería:** `draft`

**Estado:** borrador normativo; Tondo y STD-0.1 todavía no
se han publicado

**Edición de lenguaje compatible:** Tondo 0.1

**Última actualización:** 2026-08-24

---

## Índice

1. [Propósito y estado](#1-propósito-y-estado)
2. [Relación con las demás especificaciones](#2-relación-con-las-demás-especificaciones)
3. [Identidad, versiones y compatibilidad](#3-identidad-versiones-y-compatibilidad)
4. [Namespace, módulos y prelude](#4-namespace-módulos-y-prelude)
5. [Clases de disponibilidad y capabilities](#5-clases-de-disponibilidad-y-capabilities)
6. [Forma de las APIs](#6-forma-de-las-apis)
7. [Ausencia, errores y pánicos](#7-ausencia-errores-y-pánicos)
8. [Ownership, mutación y recursos](#8-ownership-mutación-y-recursos)
9. [Sincronía, suspensión y cancelación](#9-sincronía-suspensión-y-cancelación)
10. [Texto, bytes, paths y formato](#10-texto-bytes-paths-y-formato)
11. [Determinismo, orden y coste](#11-determinismo-orden-y-coste)
12. [Frontera de implementación](#12-frontera-de-implementación)
13. [Distribución reproducible](#13-distribución-reproducible)
14. [Catálogo cerrado de STD-0.1](#14-catálogo-cerrado-de-std-01)
15. [Contrato exigido a cada módulo](#15-contrato-exigido-a-cada-módulo)
16. [Testing y conformidad](#16-testing-y-conformidad)
17. [Evolución de la API](#17-evolución-de-la-api)
18. [Características deliberadamente diferidas](#18-características-deliberadamente-diferidas)
19. [Integración del borrador actual](#19-integración-del-borrador-actual)
20. [Checklist de publicación](#20-checklist-de-publicación)

---

## 1. Propósito y estado

Esta especificación define la arquitectura pública de la Standard Library de
Tondo. Su objetivo es que las APIs concretas puedan crecer sobre una base
pequeña, regular y predecible sin trasladar accidentalmente decisiones del host,
del compilador o de una implementación particular al lenguaje.

Este documento fija de forma normativa:

- La identidad y el versionado de la librería.
- La relación entre versión de stdlib, edición de lenguaje, target, perfil y
  capabilities.
- El namespace reservado `std` y el catálogo de módulos de STD-0.1.
- La frontera entre el prelude, los métodos de tipos intrínsecos y los módulos
  importados.
- Las reglas comunes de nombres, errores, ownership, recursos, suspensión,
  determinismo, coste y portabilidad.
- La distribución cerrada, reproducible y fijada por hashes.
- Los requisitos de documentación, tests y conformidad de cada API.
- La integración con el corpus vivo del borrador actual.

Esta revisión no fija todavía:

- Las declaraciones completas de cada módulo.
- El conjunto exacto de variantes y payloads de cada error.
- Las variantes culturales o dependientes de locale de `std.format`; el
  formatting estático sobre `Display`, sus builders y sus límites están fijados
  en `docs/contracts/stdlib-core.md`.
- Los métodos concretos de strings, colecciones e iteradores.
- Las declaraciones exhaustivas de consola, filesystem, procesos, concurrencia,
  codecs adicionales, regex, UUID y logging. `std.net` es la excepción ya
  cerrada por su contrato owner en la sección 14.4.6.
- Las declaraciones exhaustivas de cada operación de JSON, MessagePack y
  Protobuf; este documento sí fija sus owners, arquitectura y garantías comunes.
- La representación interna de ningún tipo.

Una firma ilustrativa, un ejemplo del lenguaje o una operación interna no se
convierte en API de STD-0.1 hasta que su módulo cumpla la
[sección 15](#15-contrato-exigido-a-cada-módulo).

### 1.1 Objetivos

La Standard Library debe ser:

1. **Pequeña:** cada concepto tiene una superficie mínima y justificada.
2. **Regular:** las mismas reglas de error, ownership y espera se aplican en
   todos los módulos.
3. **Explícita:** I/O, reloj, entorno, proceso y demás efectos aparecen en el
   módulo, la firma y la capability.
4. **Portable sin fingir:** una diferencia inevitable entre targets se declara;
   no se oculta detrás de datos falsamente universales.
5. **Determinista donde sea posible:** ningún orden o dato ambiental se filtra
   por accidente.
6. **Eficiente de forma visible:** una API documenta sus costes y no exige
   allocation o copia innecesarias para ser cómoda.
7. **Amigable para personas, LLMs y herramientas:** un nombre tiene un
   significado canónico y las alternativas no se multiplican.
8. **Implementable por terceros:** el comportamiento se expresa mediante
   contratos y observaciones, no mediante detalles privados de la VM.
9. **Rápida por construcción:** las APIs permiten streaming, préstamos,
   especialización y vectorización sin exigir materializaciones intermedias.

### 1.2 No objetivos

STD-0.1 no intenta:

- Replicar las APIs de un sistema operativo.
- Exponer una clase o wrapper para cada concepto imaginable.
- Convertir todo helper útil en parte del prelude.
- Ocultar fallos de host mediante valores por defecto.
- Proporcionar reflection dinámica sobre valores o carga dinámica. La metadata
  estática y descriptiva de `std.reflect` no es una excepción abierta a esta
  regla. Las funciones de C se declaran con `extern fn` (16.16 del lenguaje);
  `std.ffi` (19.3) solo nombra sus tipos y da memoria y strings para C.
- Congelar el layout, calling convention o estrategia de memoria del backend
  nativo.
- Añadir sintaxis, keywords, coerciones o reglas de inferencia.
- Convertir networking, calendario civil, regex, logging o sincronización
  compartida en semántica implícita del lenguaje; Tondo 0.1 los ofrece como APIs
  estándar explícitas y acotadas.

### 1.3 Lenguaje normativo

En este documento:

- **Debe** expresa un requisito de conformidad.
- **No puede** expresa una prohibición.
- **Puede** expresa una libertad de implementación que no altera observables.
- **STD-0.1** nombra el milestone de producto.
- **0.1.0** nombra únicamente la futura primera versión pública completa de la
  stdlib; no identifica el estado actual.
- **Draft** nombra el único contrato vivo durante el desarrollo de Tondo 0.1.

### 1.4 Inspiraciones aplicadas

La stdlib selecciona ideas, no clones de APIs:

- **Go:** módulos pequeños con owner claro, protocolos `Reader`/`Writer`,
  errores como valores y generación schema-first para Protobuf.
- **Kotlin:** serializers generados en compile time y configuración mediante
  valores nominales, sin reflection de valores en el hot path.
- **Rust:** traits estáticos, iteradores concretos, ownership visible y el
  enfoque de serialization dirigida por tipos.
- **Zig:** costes y allocation visibles, fallback escalar simple y cálculo en
  build time reproducible; Tondo aísla ese cálculo en `tondo-meta` en vez de
  mezclarlo con evaluación constante ordinaria.
- **.NET:** vistas prestadas tipo `Span`, readers/writers incrementales y
  source generation para evitar reflection y materializaciones.
- **Odin:** procedimientos y datos explícitos, sin contextos ambientales
  implícitos.

Cuando dos ecosistemas ofrecen varias formas equivalentes, Tondo conserva una
sola shape que encaje con su sistema de tipos y ownership.

---

## 2. Relación con las demás especificaciones

La Standard Library no redefine el lenguaje ni el toolchain. Los contratos se
reparten así:

| Documento | Autoridad |
|---|---|
| [`TONDO_LANGUAGE_SPEC.md`](./TONDO_LANGUAGE_SPEC.md) | Sintaxis, tipos, ownership, tareas, módulos, imports, prelude, intrinsics, `derive` y límites de reflection |
| [`TONDO_TOOLCHAIN_SPEC.md`](./TONDO_TOOLCHAIN_SPEC.md) | Proyectos, `tondo.toml`, dependencias, `tondo.lock`, caché y comandos |
| [`TONDO_TESTING_SPEC.md`](./TONDO_TESTING_SPEC.md) | Testing Tondo 0.1, runner y núcleo sellado de `std.testing` |
| Este documento | API estándar, reglas comunes, catálogo de módulos y distribución de la stdlib |

Si una API estándar no puede expresarse sin cambiar sintaxis, resolución,
ownership, inferencia o efectos, primero requiere una nueva edición del lenguaje.
La stdlib no puede introducir esa semántica mediante un nombre especial que el
compilador reconozca en secreto.

Si una operación necesita host o implementación privilegiada, su enlace se
describe mediante el toolchain. El binding privilegiado implementa una firma
Tondo ya especificada; no inventa la firma ni amplía sus capabilities.

Si una API elimina boilerplate mediante `derive` o generación, la stdlib define
el trait y el comportamiento observable, el lenguaje limita la autorización y
el modelo semántico, y el toolchain fija provider, inputs, sandbox, hashes y
outputs. El código generado no recibe una semántica paralela ni acceso dinámico
a valores.

### 2.1 Identidad de compatibilidad

Una API estándar nunca se evalúa en abstracto. Su identidad efectiva es la
tupla:

~~~text
stdlib version
+ standard PackageId
+ standard content hash
+ language edition
+ compiler interface format
+ target
+ profile
+ capability registry
+ selected capabilities
+ selected standard source sets
~~~

Dos builds que difieren en uno de estos componentes no comparten
automáticamente interfaces ni artefactos. El toolchain debe admitir la
combinación completa o rechazarla antes de typecheckear al consumidor.

### 2.2 Separación entre contrato y estado de implementación

Cada familia de API mantiene tres estados distintos:

1. **Especificada:** firmas y comportamiento normativo cerrados.
2. **Implementada:** existe una ruta de compilador/runtime que la ejecuta.
3. **Conforme:** supera el corpus público aplicable en su target.

Ninguno implica los otros por sí solo. Un nombre no se anuncia como parte de
0.1.0 hasta alcanzar los tres.

---

## 3. Identidad, versiones y compatibilidad

### 3.1 Versión independiente

La versión de la Standard Library es independiente de:

- La edición del lenguaje.
- La versión del compilador.
- La versión del runtime.
- La versión del formato de interfaz.
- La identidad del target.

Una distribución declara explícitamente qué combinaciones soporta. No se
deduce compatibilidad porque dos componentes compartan `0.1` en su nombre.

### 3.2 Forma de versión

Las releases públicas usan:

~~~text
MAJOR.MINOR.PATCH
~~~

Incluso antes de 1.0, Tondo no utiliza la excepción habitual que permite romper
compatibilidad libremente en cada minor de la serie cero:

- `PATCH` corrige una implementación o documentación sin cambiar una API
  pública válida ni sus observables normativos.
- `MINOR` añade APIs o targets compatibles y puede ampliar garantías sin
  reinterpretar fuente válida.
- `MAJOR` permite cambios incompatibles deliberados.

Un cambio normativo incompatible nunca se disfraza de corrección `PATCH`.

### 3.3 PackageId estándar

Cada build selecciona exactamente un `PackageId` estándar. La distribución
actual del draft utiliza:

~~~text
toolchain:std:draft
~~~

El identificador es inmutable: esos bytes no pueden republicarse con otro
contenido. El lockfile registra además el SHA-256 exacto de la distribución.
PackageId e integridad cumplen funciones distintas y ambos participan en la
identidad del build.

Otro toolchain puede utilizar otro PackageId para su implementación conforme.
Sus tipos nominales no son intercambiables accidentalmente con los de la
distribución de referencia.

El descriptor de la distribución runtime selecciona además, para el grafo meta
separado, su paquete build-only compatible:

~~~text
toolchain:std-meta:draft
~~~

Ese paquete contiene `std.meta` y los providers estándar. El proyecto no repite
la asociación en cada manifest; el lockfile sí materializa ambos PackageIds y
SHA-256 exactos. El companion no forma parte del grafo runtime ni hace visible
`std.meta` al programa final. La separación permite que cada grafo conserve una
única distribución `std`; no autoriza dos versiones dentro del mismo grafo.

### 3.4 Una sola stdlib por grafo

El namespace `std` resuelve a una única distribución en todo el grafo cerrado.
No existen aliases, imports versionados ni dos versiones simultáneas de `std`
dentro del mismo build.

Una dependencia fuente se compila contra la stdlib seleccionada por el plan
cerrado. Una interfaz precompilada producida contra otra identidad estándar se
rechaza o se reconstruye desde fuentes; no se enlaza por parecido estructural.

Esta regla evita que `String`, `Path`, `Duration` o un error estándar cambien de
identidad a mitad del programa.

### 3.5 Compatibilidad de fuente

Son cambios incompatibles, como mínimo:

- Retirar o renombrar un módulo, tipo, función, método, constante o variante.
- Cambiar un parámetro, resultado, error, constraint, receiver o variadicidad.
- Añadir o retirar `unsafe`.
- Cambiar una operación infallible por fallible o a la inversa.
- Añadir una variante a un enum que el consumidor puede comprobar
  exhaustivamente.
- Retirar una capability o un target anunciado.
- Hacer más débil una garantía pública de orden, atomicidad o complejidad.
- Hacer que código válido seleccione otra declaración por añadir un método.
- Cambiar ownership, capacidades estructurales o la acción de salida de un recurso.
- Convertir un dato portable en dato dependiente del host.

Añadir un módulo o una declaración es compatible solo cuando no cambia
resolución ni significado de fuente existente.

### 3.6 Actualización explícita

El compilador nunca selecciona “la última stdlib”. Actualizar exige:

1. Elegir una versión exacta compatible.
2. Cambiar de forma explícita el manifiesto o la configuración de toolchain.
3. Materializar un lockfile nuevo con PackageId y hash.
4. Resolver de nuevo source sets, capabilities e interfaces.
5. Ejecutar la conformidad aplicable.

El comando que automatice estos pasos pertenece a un futuro gestor de paquetes.
La compilación continúa siendo cerrada y offline.

---

## 4. Namespace, módulos y prelude

### 4.1 Namespace reservado

`std` es un namespace reservado por el lenguaje y resuelto por el toolchain.
Un paquete de usuario no puede:

- Llamarse `std`.
- Declarar un alias de dependencia `std`.
- Añadir módulos al namespace `std`.
- Sustituir una declaración estándar mediante orden de búsqueda.

Un import estándar sigue la sintaxis ordinaria:

~~~tondo pseudocode
import std.console
import std.time
~~~

El import introduce únicamente el último segmento, salvo alias explícito. No
hay wildcard imports, apertura global del módulo ni reexport implícito.

### 4.2 Prelude mínimo

La stdlib 0.1.0 no añade nombres al prelude definido por la edición de lenguaje.
En particular, no introduce implícitamente:

- I/O.
- Paths o filesystem.
- Tiempo.
- Environment.
- Formato.
- Testing.
- Errores de host.

Los tipos y funciones de estas familias se obtienen mediante un módulo
calificado o mediante inferencia desde una llamada ya calificada.

Cambiar el prelude exige el proceso de compatibilidad de la especificación del
lenguaje; una minor de stdlib no puede hacerlo.

### 4.3 Propietario canónico

Cada declaración pública tiene un único propietario canónico:

- Un tipo de librería pertenece a un módulo exacto.
- Una función libre pertenece a un módulo exacto.
- Un método inherente pertenece al tipo exacto sobre el que se publica.
- Un trait pertenece a un módulo exacto.
- Un error pertenece al módulo que define su semántica o a un módulo común
  explícito; no se copia con distinta identidad entre módulos.

Los módulos estándar pueden mencionar tipos de otro módulo en sus firmas, pero
no los duplican ni crean aliases públicos solo para ahorrar un import. La
documentación siempre muestra el propietario canónico.

### 4.4 Métodos de tipos intrínsecos

`String`, `Array`, `Map`, `Set`, `Range`, `Option`, `Result` e
`Iterator` pertenecen al lenguaje, pero sus APIs no intrínsecas pertenecen a la
versión estándar seleccionada.

Para esos tipos:

- El toolchain carga los métodos publicados desde la interfaz estándar exacta.
- Los métodos se consideran propiedad canónica del tipo intrínseco, no
  extensiones globales.
- Un paquete de usuario continúa sin poder añadir métodos inherentes a esos
  tipos.
- El compilador no puede atribuir semántica especial a un método salvo que la
  especificación del lenguaje ya lo declare intrinsic.
- La implementación portable debe ser código Tondo cuando sea posible.
- Añadir un método respeta las reglas de compatibilidad de 3.5.

No se publican simultáneamente `array.append(array, value)` y
`array.append(value)` como dos formas equivalentes. Una operación cuyo
propietario natural es un valor se expresa como método; una operación ambiental,
un constructor sin receptor o una combinación simétrica se expresa como función
del módulo.

### 4.5 Grafo de módulos

El grafo estándar es acíclico. Las dependencias siguen esta dirección:

~~~text
superficie intrínseca y módulos core
    -> tipos y abstracciones portables
        -> módulos capability-gated
            -> adaptadores privilegiados del target
~~~

Un módulo core no puede depender de un módulo hosted ni consultar una
capability. `std.testing` puede consumir APIs públicas de producción, pero la
producción no puede depender de `std.testing`.

### 4.6 Ausencia de inicialización global

Importar un módulo estándar:

- No ejecuta código.
- No abre recursos.
- No lee reloj, entorno, filesystem, red o entropía.
- No crea threads ni tasks.
- No registra callbacks.
- No modifica stdout o stderr.

Todo efecto comienza en una llamada visible desde `main`, un script, un test o
una función alcanzada desde ellos.

---

## 5. Clases de disponibilidad y capabilities

### 5.1 Cinco clases

Cada declaración estándar pertenece exactamente a una clase:

| Clase | Disponibilidad |
|---|---|
| **Core** | En todo target que anuncie STD-0.1 |
| **Capability-gated** | Solo cuando el target selecciona la capability exacta |
| **Test-only** | Solo dentro del grafo cerrado de `tondo test` |
| **Build-only** | Solo dentro del target hermético `tondo-meta`; nunca en un artefacto de aplicación |
| **Target-specific** | Solo en una interfaz que identifica expresamente ese target |

STD-0.1 evita APIs target-specific salvo que no exista un contrato portable
honesto. Una operación target-specific permanece en un módulo estándar
claramente documentado o se difiere; no se hace pasar por portable.

### 5.2 Capabilities

STD-0.1 utiliza el registro `tondo-capabilities-draft` definido por el toolchain:

~~~text
clock
civil-clock
console
dynamic-linking
entropy
environment
filesystem
network
process
threads
~~~

Que una capability esté registrada no significa que STD-0.1 la publique ni que
un target la implemente.

Una capability es:

- Parte de la identidad del build.
- Una condición de admisión estática.
- Un límite máximo de efectos que el target puede proporcionar.

No es:

- Garantía de que una operación concreta tendrá éxito.
- Permiso ambiental implícito para cualquier módulo.
- Un booleano consultado dinámicamente por fuente ordinaria.
- Una forma de saltarse errores recuperables.

### 5.3 Ausencia estática

Cuando falta una capability:

- El módulo o la declaración condicionada no existe en la interfaz seleccionada.
- Importarla o utilizarla produce `E1008`.
- El compilador nombra la capability ausente.
- No se instala un stub que siempre devuelve `Unsupported`.
- No se selecciona otra API “parecida”.

Un proyecto que soporte varios targets utiliza source sets explícitos. Tondo no
introduce `#if`, reflection de capabilities ni control de flujo ambiental.

### 5.4 Tipos portables y operaciones hosted

Un módulo puede contener un núcleo core y una extensión capability-gated cuando
la separación conserva una identidad coherente. El caso inicial es `std.time`:

- `Duration` y sus operaciones puras son core.
- Las operaciones que consultan o esperan contra un proveedor monotónico
  requieren `clock`.
- El calendario civil, parsing y aritmética son core cuando el target ha
  seleccionado el bundle de zonas versionado. Consultar la hora de pared
  requiere la capability separada `civil-clock`; relacionarla con un
  `Instant` requiere además `clock`.
- El target y la capability forman parte de la interfaz, por lo que la
  disponibilidad parcial nunca es ambigua.

Esta forma no autoriza a mezclar arbitrariamente declaraciones core y hosted.
Cada módulo debe justificar la separación en su matriz de disponibilidad.

### 5.5 `std.testing` no concede host

`std.testing` es test-only, no una capability. Importarlo no concede:

- `console`.
- `filesystem`.
- `environment`.
- `clock`.
- `process`.
- `network`.
- `threads`.

Sus logs, tags, attachments, snapshots y tiempo virtual utilizan el envelope
sellado del runner. `withVirtualTime` sustituye únicamente el proveedor
monotónico que el programa ya podría utilizar; no habilita otra operación de
host.

---

## 6. Forma de las APIs

### 6.1 Una forma canónica

Antes de publicar dos operaciones parecidas, la especificación debe demostrar
que representan contratos distintos. No se añaden aliases por familiaridad con
otro lenguaje.

En particular:

- No se duplican función libre y método para la misma operación.
- No se añade un sufijo `Async`: una operación que espera es una función ordinaria.
- No se añade un prefijo `try`; `! E` ya muestra que la operación falla.
- No se ofrecen variantes que solo cambian entre tuple y argumentos.
- No se usan booleanos para elegir comportamientos con semántica distinta.
- No se oculta una operación peligrosa detrás de un nombre cómodo.

### 6.2 Convenciones de nombres

Las declaraciones siguen las convenciones del lenguaje:

- Tipos, traits y variantes: `PascalCase`.
- Funciones, métodos, parámetros y fields: `camelCase`.
- Constantes de módulo: `PascalCase`.
- Módulos: una palabra minúscula o `camelCase`.
- Acrónimos como palabras: `Utf8Error`, `Value`, `userId`.

Los módulos usan nombres completos cuando una abreviatura no sea universal. Una
abreviatura aceptada no crea también un alias largo.

### 6.3 Parámetros y configuración

Tondo no tiene overload por firma. La stdlib utiliza:

- Parámetros ordinarios para datos obligatorios.
- Parámetros nombrados cuando mejoran claridad sin cambiar la firma.
- Un parámetro con valor por defecto para una decisión opcional, como
  `fs.remove(path, recursive: true)`.
- Un record `Options` nominal cuando existen varias decisiones opcionales.
- Un enum cuando un valor elige modos semánticamente distintos.
- Un variádico homogéneo únicamente cuando cero o más valores del mismo tipo
  forman naturalmente una secuencia.

Un record de opciones:

- Tiene un propietario de módulo exacto.
- Expone una construcción canónica completa o un valor inicial explícito.
- No lee defaults del entorno.
- No utiliza combinaciones de fields inválidas sin validarlas.

### 6.4 Resultado y shape

Una API devuelve:

- Un valor concreto cuando existe un resultado materializado.
- `T?` cuando la ausencia es normal y no necesita explicación.
- `T ! E` cuando la operación puede fallar de forma recuperable.
- `impl Iterator[T] + Bounds` cuando produce una secuencia lazy de tipo concreto
  oculto.
- Un owner afín cuando transfiere un recurso con cleanup obligatorio.
- `Unit` solo cuando la finalización normal no necesita payload.

No utiliza:

- Enteros centinela.
- Strings vacíos para representar ausencia.
- `null`.
- Un error dentro de un valor de éxito.
- Output parameters para evitar devolver un valor ordinario.

### 6.5 Dispatch estático

Los algoritmos genéricos usan traits y protocolos estáticos. La API no introduce
por conveniencia:

- Reflection de métodos.
- Lookup por string.
- Vtables ocultas.
- Boxing obligatorio de closures.
- `fn(...)` uniforme cuando `Call`, `CallMut` o `CallOnce` conserva mejor el
  callable concreto.

Un resultado `impl Bound` publica todos los bounds que el consumidor necesita y
no permite observar el tipo concreto.

---

## 7. Ausencia, errores y pánicos

### 7.1 Elección

La stdlib utiliza:

- `Option` para ausencia esperada sin causa adicional.
- `Result` para un fallo recuperable que el caller debe manejar o propagar.
- Pánico para una violación de contrato del programador o una invariante rota.
- Error del toolchain para incompatibilidad de build anterior a la ejecución.
- Aborto de implementación únicamente para agotamiento irrecuperable definido
  por el lenguaje.

Filesystem inexistente, permisos, encoding inválido, proceso no creado, stream
cerrado y fallo de reloj no son pánicos.

### 7.2 Errores públicos nominales

Una frontera pública prefiere un error nominal del módulo:

~~~tondo pseudocode
pub enum ReadError {
    // Variantes fijadas por la especificación concreta del módulo.
}
~~~

Las uniones de error son apropiadas para composición local. Una API pública
puede utilizarlas únicamente cuando cada miembro ya es parte deliberada y
estable de su contrato. No depende de conversiones automáticas `From`.

Cada error público debe:

- Ser accesible desde toda firma que lo exponga.
- Tener variantes cerradas y exhaustivas.
- Declarar payloads portables.
- Declarar `Copy`, `Equatable`, `Key`, `Send` o `Share` solo cuando sus payloads
  y semántica lo permiten.
- Separar clasificación programática de representación humana.

Añadir una variante observable es un cambio incompatible.

### 7.3 Datos nativos

Un error portable no convierte en semántica estable:

- `errno`.
- Códigos Win32.
- Textos localizados del sistema.
- Paths físicos no declarados.
- PID, handle o dirección.
- Timestamps wall-clock.

Un módulo puede exponer información nativa mediante una API target-specific
separada. La lógica portable no depende de ella.

### 7.4 Display de errores

El programa decide mediante el tipo, la variante y sus payloads, nunca parseando
`Display`.

Salvo que un módulo lo declare expresamente:

- El texto de `Display` es diagnóstico humano, no protocolo serializado.
- No se localiza según ambiente implícito.
- No incluye valores secretos ni identificadores no deterministas.
- Una versión `PATCH` puede mejorar el texto sin cambiar la clasificación.

Los formatos machine-readable tienen su propia versión y schema.

### 7.5 Pánicos documentados

Cada API enumera todas sus clases de pánico alcanzables desde entrada segura. No
se utiliza “puede panic” como cláusula abierta.

Una API segura:

- Valida todo dato que pueda provenir de entrada no confiable.
- Devuelve error para fallos recuperables.
- Solo panica cuando el caller rompe una precondición local y comprobable o el
  lenguaje ya define esa clase, como bounds u overflow comprobado.
- Nunca produce comportamiento indefinido.

### 7.6 Cancelación no es un error implícito

La cancelación estructurada conserva el contrato del lenguaje. No amplía
automáticamente `E`. Una API puede publicar un error nominal de cancelación solo
cuando solicitar u observar cancelación sea parte explícita de su semántica.

---

## 8. Ownership, mutación y recursos

### 8.1 Firma completa

El contrato de una operación incluye:

- Si recibe el valor, `ref`, `mut` o `var`.
- Si copia, mueve, presta o consume cada argumento.
- Si devuelve storage independiente, una vista o un owner.
- Las capacidades estructurales del resultado.
- La acción de salida de cada recurso que devuelve.

La documentación no puede contradecir una firma usando expresiones vagas como
“puede modificar” o “quizá consume”.

### 8.2 Receptores

Se utiliza:

- `self` para observación por valor compatible con el contrato `Copy` o para
  consumo visible de un owner.
- `ref self` para observación sin copia ni movimiento.
- `mut self` para mutación con extensión fija.
- `var self` para reemplazo o cambio estructural.

Una operación no exige `var` cuando `mut` basta. Una consulta no exige copia
cuando `ref` conserva mejor valores afines.

### 8.3 Semántica de valor

`String`, arrays, maps, sets y otros valores estándar preservan su semántica
lógica aunque una implementación utilice COW, small-value optimization,
interning o fusión.

Una optimización no puede alterar:

- Igualdad.
- Orden de iteración.
- Independencia después de mutar.
- Identidad explícita.
- Pánicos y errores.
- Momento visible de evaluación.
- Cleanup.

### 8.4 Recursos externos

Un archivo, proceso activo, stream one-shot, lock u otro recurso:

- Es un owner afín, salvo que sea un handle compartido como los de `std.sync`.
- Declara su acción de salida (8.10 del lenguaje): qué hace al abandonarse y qué
  hace durante un pánico.
- Ofrece una operación explícita que lo libera antes y devuelve sus errores.
- No depende del GC ni de un finalizador de usuario.

Cada tipo afín publica una sola operación de cierre para cada outcome. No
ofrece simultáneamente `close`, `dispose` y `release` como sinónimos.

### 8.5 Iteradores y vistas

Una API de secuencia elige deliberadamente:

- `Array[T]` para materialización poseída.
- Slice de `Array[T]` para una vista indexable con las reglas del lenguaje.
- `impl Iterator[T]` para producción lazy síncrona.
- Un owner afín cuando iterar mantiene un recurso abierto.

La API documenta:

- Orden.
- Número máximo o conocido de elementos.
- Si consume la fuente.
- Si conserva préstamos.
- Si puede fallar durante iteración.
- Qué acción de salida tiene un cursor que posee un recurso.

No materializa un array de forma oculta detrás de una API anunciada como lazy.

---

## 9. Tareas, esperas y cancelación

### 9.1 Esperas sin colores

Las operaciones de la librería son funciones ordinarias. Una operación que
espera a otra tarea o al host —un `Join`, un canal, un timer, un archivo, un
socket o un proceso— detiene la tarea actual, no el thread, y devuelve su
resultado lógico como cualquier llamada (11.10 de la especificación del
lenguaje). No existen marcas `suspends` ni `selectable`, variantes `Async` ni
wrappers `Task`/`Future`.

### 9.2 Una forma por operación

STD-0.1 no duplica operaciones como `read` y `readAsync`. Cada operación tiene
una sola forma: esperar es la forma normal de una operación que espera, y
construir un plan inerte o consultar metadata ya disponible no espera. La
concurrencia se obtiene con `spawn` sobre cualquier llamada.

### 9.3 Tareas, grupos y canales

`spawn`, `Join[T]`, `Group[T]` y los canales pertenecen al lenguaje y al preludio
(11.11–11.14 de la especificación del lenguaje):

~~~tondo pseudocode
pub fn Join.wait(self): T
pub fn Join.wait(self, timeout: time.Duration): T?
pub fn Join.cancel(self)

pub fn Group.new[T](): Group[T]
pub fn Group.add(var self, job: Join[T])
pub fn Group.wait(self): Array[T]        // Array[R] ! E cuando T es R ! E
pub fn Group.next(var self): T?
pub fn Group.cancel(self)
pub fn Group.length(self): Int
pub fn Group.isEmpty(self): Bool

pub fn channel[T](capacity: Int): (Sender[T], Receiver[T])   // sin capacidad: sin límite
pub fn Sender.send(self, value: T)
pub fn Sender.close(self)
pub fn Sender.copy(self): Sender[T]
pub fn Sender.isClosed(self): Bool
pub fn Receiver.receive(self): T?
pub fn Receiver.receive(self, timeout: time.Duration): T?
pub fn Receiver.isClosed(self): Bool
~~~

`wait` y `receive` admiten el argumento nombrado opcional `timeout`, y `channel`
su capacidad opcional; son formas intrínsecas del preludio, no sobrecargas que
un módulo pueda declarar.

### 9.4 Cancelación

La cancelación es cooperativa (11.15): una tarea cancelada se detiene en su
siguiente espera, ejecuta sus `defer`, libera sus valores y cancela sus propias
tareas pendientes. Cada operación que espera a un recurso externo documenta qué
datos parciales puede haber producido y qué cleanup completa al cancelarse. No se
promete interrumpir un cálculo que no espera.

### 9.5 Timeouts y deadlines

Un timeout se expresa con un `time.Duration`, nunca con un entero sin unidad, y
usa tiempo monotónico. `job.wait(timeout:)` y `receiver.receive(timeout:)` cubren
los casos frecuentes; `select` con `time.sleep` combina un límite con varias
operaciones. Ninguna API instala timeouts ocultos ni convierte un timeout en
pánico.

### 9.6 Scheduler y progreso

El runtime reparte las tareas entre varios threads, uno por núcleo, y las mueve
entre ellos libremente. Una operación de la librería que espera al host no
bloquea su thread: detiene la tarea y deja correr a las demás. La garantía de
progreso y sus límites se definen en 16.14 de la especificación del lenguaje.

---

## 10. Texto, bytes, paths y formato

### 10.1 Unicode

`String` conserva la semántica Unicode de la edición del lenguaje:

- UTF-8 válido.
- Valores escalares, no bytes ni grapheme clusters.
- Sin normalización implícita adicional.
- Sin locale ambiental.

Una API que opera por bytes, scalar, grapheme, palabra o locale lo expresa en
su nombre y contrato. STD-0.1 no promete segmentación por grapheme salvo API
específica.

### 10.2 `String` y bytes

Texto y datos binarios no se convierten implícitamente:

- `String` siempre es UTF-8 válido.
- `Byte` es una unidad binaria nominal.
- `Array[Byte]` es la forma de los datos binarios en toda la librería: archivos,
  procesos, red y codecs leen y escriben `Array[Byte]`. No existe un tipo
  binario aparte.

Las conversiones son explícitas:

```tondo
let data = text.bytes()                 // Array[Byte]: los bytes UTF-8
let text = String.fromBytes(data)?      // String ! text.TextError
```

`bytes()` no falla. `String.fromBytes` valida UTF-8 estricto y devuelve
`TextError.InvalidData` ante bytes inválidos, sin producir un `String` parcial
ni insertar caracteres de reemplazo. Otros encodings pertenecen a
`std.encoding`.

### 10.3 Protocolos de I/O

`std.io` reúne los streams que comparten la consola, los archivos, los procesos
y la memoria. Un `Writer` es un destino de datos y un `Reader` una fuente; los
dos son handles compartidos (`Copy`, `Send` y `Share`): sus copias usan el mismo
stream, cada operación es atómica frente a las demás tareas, y el stream se
cierra cuando desaparece su última copia o con `close()`. Sus operaciones
esperan como cualquier llamada (sección 9), sin variantes `Async`, y no ocupan un
thread del scheduler mientras el host trabaja.

~~~tondo
import std.io

let buffer = io.Buffer()
let out = buffer.writer()
out.writeLine("first")?
out.write("second")?
console.println(buffer.text()?)          // "first\nsecond"

let input = io.reader("one\ntwo")
for {
    match input.readLine()? {
        some(line) => console.println(line),
        none => break,
    }
}
~~~

La superficie es:

~~~tondo pseudocode
pub enum IoError { Closed, InvalidData, Host }
pub type Writer
pub type Reader
pub type Buffer

pub fn Writer.write(self, text: String): Unit ! IoError
pub fn Writer.writeLine(self, text: String): Unit ! IoError
pub fn Writer.writeBytes(self, data: Array[Byte]): Unit ! IoError
pub fn Writer.flush(self): Unit ! IoError
pub fn Writer.close(self): Unit ! IoError

pub fn Reader.readLine(self): String? ! IoError
pub fn Reader.read(self, max: Int): Array[Byte]? ! IoError
pub fn Reader.readText(self): String ! IoError
pub fn Reader.close(self): Unit ! IoError

pub fn Buffer(): Buffer
pub fn Buffer.writer(self): Writer
pub fn Buffer.reader(self): Reader
pub fn Buffer.text(self): String ! IoError
pub fn Buffer.bytes(self): Array[Byte]
pub fn Buffer.length(self): Int
pub fn Buffer.clear(self)

pub fn reader(text: String): Reader
pub fn discard(): Writer
~~~

- `readLine` devuelve la siguiente línea sin su `\n` ni `\r\n`, o `none` al
  final; `read(max)` devuelve hasta `max` bytes, o `none` al final; `readText`
  lee el resto. Las operaciones de texto validan UTF-8 y devuelven
  `InvalidData`.
- Un `Writer` de archivo acumula lo escrito y lo entrega al host al llenarse,
  con `flush()`, con `close()` y al desaparecer su última copia; los errores de
  ese cierre implícito se pierden.
- Escribir en un stream cerrado, o en una pipe cuyo lector ya terminó, devuelve
  `Closed`. `Host` reúne los fallos que el host no permite distinguir.
- Un `Buffer` es memoria compartida: su `writer()` añade al final, su
  `reader()` lee lo que contiene en ese momento, y `text()`, `bytes()`,
  `length()` y `clear()` lo observan o vacían. `io.reader(text)` lee un texto
  fijo y `io.discard()` acepta todo sin guardarlo.
- Los archivos (10.5), la consola (19.1) y los procesos (19.2) ofrecen sus
  streams como `Writer` y `Reader`.

### 10.4 Paths

Un path es un `String`. `std.path` ofrece operaciones léxicas, que no consultan
el filesystem ni resuelven enlaces:

```tondo
import std.path

path.join("docs", "guide.md")       // "docs/guide.md"
path.parent("docs/guide.md")        // some("docs")
path.fileName("docs/guide.md")      // some("guide.md")
path.extension("docs/guide.md")     // some("md")
path.isAbsolute("/etc/hosts")       // true
```

`join` añade un componente, o devuelve el segundo cuando es absoluto. `parent`,
`fileName` y `extension` dan `none` cuando el path no tiene esa parte;
`extension` no incluye el punto. Un nombre de archivo que no es UTF-8 no puede
representarse: las operaciones de `std.fs` que lo encuentran devuelven
`FsError.InvalidName`.

### 10.5 Filesystem

`std.fs` observa y cambia el filesystem. Sus operaciones esperan sin bloquear el
thread: el runtime las ejecuta en threads propios mientras la tarea espera, y las
demás tareas siguen corriendo. No tiene variantes `Async`.

```tondo
import std.fs

let config = fs.readText("config.toml")?
fs.writeText("out.txt", report)?
fs.appendText("app.log", "started\n")?
let image = fs.readBytes("logo.png")?
if fs.exists("cache") {
    fs.remove("cache", recursive: true)?
}
for name in fs.list("docs")? {
    console.println(name)
}

var file = fs.open("big.log", fs.OpenMode.Read)?
for {
    match file.readLine()? {
        some(line) => process(line),
        none => break,
    }
}
```

La superficie es:

```tondo pseudocode
pub enum FsError {
    NotFound
    PermissionDenied
    AlreadyExists
    NotADirectory
    IsADirectory
    InvalidData
    InvalidName
    Host
}
pub enum FileKind { File, Directory, Symlink, Other }
pub type Metadata = { kind: FileKind, size: Int, readOnly: Bool }
pub enum OpenMode { Read, Write, Append, CreateNew, ReadWrite }
pub type File

pub fn readText(path: String): String ! FsError
pub fn readBytes(path: String): Array[Byte] ! FsError
pub fn writeText(path: String, text: String): Unit ! FsError
pub fn writeBytes(path: String, data: Array[Byte]): Unit ! FsError
pub fn appendText(path: String, text: String): Unit ! FsError
pub fn exists(path: String): Bool
pub fn metadata(path: String): Metadata ! FsError
pub fn list(path: String): Array[String] ! FsError
pub fn createDirectory(path: String): Unit ! FsError
pub fn rename(from: String, to: String): Unit ! FsError
pub fn copy(from: String, to: String): Unit ! FsError
pub fn remove(path: String, recursive: Bool = false): Unit ! FsError
pub fn open(path: String, mode: OpenMode): File ! FsError

pub fn File.readLine(var self): String? ! FsError
pub fn File.read(var self, max: Int): Array[Byte]? ! FsError
pub fn File.readText(var self): String ! FsError
pub fn File.write(var self, text: String): Unit ! FsError
pub fn File.writeBytes(var self, data: Array[Byte]): Unit ! FsError
pub fn File.flush(var self): Unit ! FsError
pub fn File.close(self): Unit ! FsError
pub fn File.writer(self): io.Writer
pub fn File.reader(self): io.Reader
```

- `readText`, `readLine`, `File.readText` y `list` validan UTF-8 y devuelven
  `InvalidData` o `InvalidName`.
- `writeText` y `writeBytes` escriben en un archivo temporal del mismo
  directorio y lo renombran: otro lector ve el contenido anterior o el nuevo,
  nunca uno a medias. El rename no promete durabilidad ante un corte de
  energía.
- `exists` no sigue un enlace roto como existente; `metadata` describe la
  entrada misma, sin seguir enlaces. `size` es la longitud en bytes que da el
  host, y `readOnly` el atributo del host, no una predicción de permisos.
- `list` devuelve los nombres de las entradas, sin `.` ni `..`, ordenados por
  bytes.
- `createDirectory` crea también los directorios padre que falten y no falla si
  el directorio ya existe. `remove` borra un archivo, un enlace o un directorio
  vacío; un directorio con contenido da `IsADirectory`, salvo con
  `recursive: true`, que lo borra con su contenido sin seguir enlaces.
- `open` con `Read` lee un archivo existente; `Write` lo crea o lo vacía;
  `Append` lo crea si falta y escribe al final; `CreateNew` falla con
  `AlreadyExists` si ya existe; `ReadWrite` lee y escribe uno existente.
- `readLine` devuelve la siguiente línea sin su `\n` ni `\r\n`, o `none` al
  final; `read(max)` devuelve hasta `max` bytes, o `none` al final;
  `File.readText` lee el resto del archivo.
- Un `File` tiene un único dueño y no es `Copy`. Se cierra solo al abandonarse
  y sus escrituras pendientes se vuelcan entonces; los errores de ese cierre
  implícito se pierden. `close()` cierra en el momento y devuelve sus errores;
  `flush()` vuelca las escrituras pendientes sin cerrar. `writer()` y
  `reader()` entregan el archivo como stream de `std.io` (10.3), por ejemplo
  para redirigir la consola a él; el archivo se cierra con la última copia del
  stream.
- `FsError` no incluye paths ni contenido; `Host` reúne los fallos que el host
  no permite distinguir.

### 10.6 Formato

`Display` es el protocolo mínimo estático usado por interpolación. Los tipos
intrínsecos lo implementan así:

- Enteros y `Byte`: decimal, con `-` para negativos.
- `Bool`: `true` o `false`.
- `Char` y `String`: su propio texto, sin comillas.
- Floats: la forma decimal más corta que vuelve a leerse como el mismo valor y
  que siempre se distingue de un entero: `1.0`, `0.1`, `1e-7`, `1e21`. Los
  valores especiales son `NaN`, `inf` y `-inf`.
- `Array` y `Deque`: `[a, b, c]`; `Map`: `[key: value, ...]` y `[:]` vacío;
  `Set`: `Set[a, b]`. Los elementos usan su propio `Display`.
- `T?`: `some(value)` o `none`, con el `Display` de `T`.
- `T ! E`: `ok(value)` o `err(error)`, con el `Display` de cada uno.
- Tuplas: `(a, b)`, con el `Display` de cada parte.
- Los enums de error de la stdlib: su variante en palabras, como `not found`.

La stdlib puede añadir formato controlado mediante `std.format`, pero:

- No introduce reflection general.
- No busca métodos por string.
- No consulta locale sin un valor explícito.
- No convierte console en motor de formato.
- No duplica interpolación con otra sintaxis.
- Distingue output humano de formatos machine-readable.

Una API de console recibe texto o bytes ya definidos por su contrato. La
decisión de añadir newline, separator, flushing o encoding pertenece a esa
operación y nunca es implícita por terminal.

`std.format` construye texto pieza a pieza sobre `Display`; unir un array es
`Array.join`:

~~~tondo pseudocode
pub type Builder
pub fn Builder.new(): Builder
pub fn Builder.add[T: Display](mut self, value: T)
pub fn Builder.addLine[T: Display](mut self, value: T)   // y un \n
pub fn Builder.isEmpty(self): Bool
pub fn Builder.text(self): String
~~~

El builder guarda las piezas y las une una sola vez, en `text()` o al mostrarlo
con su `Display`, que es ese mismo texto.

---

## 11. Determinismo, orden y coste

### 11.1 Determinismo funcional

Una función core produce el mismo resultado para los mismos valores y la misma
versión estándar. No consulta:

- Reloj.
- Entorno.
- Filesystem.
- Red.
- Entropía.
- Orden de threads.
- Locale.

Un módulo hosted expone sus observaciones como efecto explícito. La
nondeterminación inevitable no puede contaminar una API core.

### 11.2 Orden observable

Toda API que produzca varios elementos define uno:

- Orden canónico concreto.
- Orden de inserción.
- Orden de entrada.
- Orden del pipeline.
- Orden deliberadamente no especificado.

No hereda accidentalmente el orden de un hash table, directorio, scheduler o
API del sistema.

Cuando el orden no pueda ser portable, la documentación lo declara y las suites
de conformidad no exigen uno ficticio. Una API que necesite reproducibilidad
ofrece una operación canónica explícita o devuelve datos que el caller pueda
ordenar sin pérdida.

### 11.3 Igualdad y hashing

La igualdad sigue el lenguaje. Un hash interno puede utilizar seed o layout no
observable, pero:

- No cambia igualdad.
- No cambia el orden normativo de Map o Set.
- No se serializa como identidad estable.
- No aparece en diagnósticos reproducibles.

Una API de hashing público debe especificar algoritmo, versión y bytes exactos;
no reutiliza el hash interno de colecciones.

### 11.4 Complejidad y allocation

Cada operación publica, cuando sea material:

- Complejidad temporal.
- Memoria adicional.
- Si reserva o puede reutilizar storage.
- Si puede activar COW.
- Si materializa una secuencia.
- Si realiza I/O.
- Si espera.
- Si su coste depende de bytes, scalars, elementos o profundidad.

Una garantía utiliza worst-case, amortizada o esperada de forma explícita. No
se degrada en una minor compatible.

Las APIs evitan allocation obligatoria cuando un préstamo o resultado opaco
estático conserva la ergonomía. Tampoco exponen buffers internos de forma que
rompa COW, ownership o seguridad.

Una API de transformación ofrece una ruta de streaming o escritura sobre
`std.io.Writer` cuando materializar el resultado completo no es inherente al
contrato. La operación cómoda que devuelve `Array[Byte]` puede existir además, pero se
define como collector de la misma máquina semántica y no como una implementación
divergente.

### 11.5 Límites

Toda operación sobre input no confiable define:

- Validación.
- Overflow.
- Profundidad o tamaño máximo cuando exista.
- Comportamiento ante agotamiento.
- Presupuesto de output.

Un límite de implementación configurable no se convierte en semántica portable
salvo que la API lo publique.

### 11.6 Implementación de referencia y kernels optimizados

Toda familia crítica de bytes, texto, parsing, hashing o codecs mantiene:

1. una implementación escalar sencilla que actúa como oracle ejecutable;
2. properties y vectores que comparan cualquier ruta optimizada con ese oracle;
3. kernels especializados cuando la evidencia demuestra beneficio; y
4. un fallback portable con exactamente los mismos observables.

Se permiten SIMD, operaciones de palabra ancha, lookup tables, vectorización
automática, specialization, monomorfización y target multiversioning. La
selección de kernel puede depender de arquitectura y CPU disponible, pero nunca
cambia aceptación, resultado, error, orden, overflow ni consumo visible. No
forma parte del API y se realiza como máximo una vez por unidad apropiada, no en
cada byte del hot path.

Un kernel nativo se encapsula en una unidad privilegiada fijada cuando requiere
instrucciones o layout no expresables en Tondo seguro. La ruta escalar continúa
siendo obligatoria para portabilidad, pruebas diferenciales y targets sin esa
instrucción.

### 11.7 Presupuestos de rendimiento

“Rápido” no se acredita con una impresión ni con un único throughput. Cada
módulo crítico publica workloads representativos y gates para:

- throughput y latencia, incluida cola cuando sea material;
- allocations por operación y bytes asignados;
- memoria pico y crecimiento con el input;
- coste de startup y primera llamada;
- tamaño de código o artefacto;
- tiempo de compilación añadido por generadores y monomorfización; y
- comportamiento de inputs pequeños, medianos, grandes y adversarios.

Los benchmarks registran hardware, OS, target, backend, perfil, toolchain, flags,
corpus, revisión de fuente y varianza. El coordinador machine-readable de S1A
mantiene una fila por owner: una fila capturada solo puede declarar las
dimensiones observadas y una fila diferida debe justificar por qué aún no existe
una identidad de hot path revisada. Una optimización se acepta solo con tests de
equivalencia y sin una regresión material no justificada en otra dimensión
publicada. Los números concretos pertenecen al contrato de cada módulo y al
tracker; esta arquitectura no inventa un umbral universal.

---

## 12. Frontera de implementación

### 12.1 Tondo primero

Las operaciones portables se implementan en Tondo siempre que el lenguaje pueda
expresarlas con seguridad y coste razonable. No se duplican en Rust o en el
backend nativo por comodidad.

Pueden ser privilegiadas:

- Acceso a host.
- Primitivas que requieren layout o instrucciones del target.
- Integración con el scheduler.
- Construcción de tipos opacos nativos.
- Operaciones intrínsecas ya declaradas por el lenguaje.

### 12.2 Unidades privilegiadas

Una unidad privilegiada:

- Tiene identidad y hash exactos.
- Declara compiler, target, perfil y capabilities.
- Publica bindings con firma y contrato de seguridad fijados.
- No puede ser seleccionada por un nombre de fuente arbitrario.
- No añade una ABI FFI pública.
- No expone handles raw a código seguro.
- Se admite antes de ejecutar código de usuario.

La firma Tondo es la frontera normativa. El binding no puede aceptar más valores,
devolver otra shape ni convertir un error host en éxito.

### 12.3 Código seguro

Toda API estándar es segura salvo `unsafe fn` explícita. Que su implementación
interna utilice `Pointer`, syscalls o código nativo no traslada precondiciones al
caller.

Un wrapper seguro:

- Valida tamaños, índices, encoding y estado del handle.
- Mantiene roots correctos durante llamadas y suspensión.
- Evita aliasing o lifetime inválidos.
- Traduce fallos recuperables al error nominal publicado.
- Conserva pánicos y cleanup según el lenguaje.

### 12.4 Paridad entre backends

La VM es el oracle inicial. Un backend nativo:

- Consume el mismo MIR verificado.
- Implementa la misma interfaz estándar.
- Produce los mismos valores, errores, pánicos, output y orden normativo.
- Puede diferir en rendimiento, layout, estrategia de memoria y detalles
  expresamente no observables.

Una optimización no autoriza una segunda semántica de stdlib.

### 12.5 Inputs no confiables y secretos

Todo dato procedente de filesystem, environment, console, proceso o cualquier
otro host se trata como no confiable. Una API estándar:

- Valida longitudes, encoding, discriminantes y estado antes de construir un
  valor seguro.
- No interpreta texto como shell, glob, regex, path o formato salvo una
  operación que lo nombre explícitamente.
- No sigue enlaces, canonicaliza o afirma atomicidad de filesystem por
  accidente.
- No convierte una comprobación previa en garantía contra TOCTOU.
- No incluye automáticamente environment, argumentos, contenido de archivo o
  output de proceso dentro de errors o diagnostics.
- No intenta detectar o redactar secretos heurísticamente.
- No transmite datos por red ni los sube como telemetría.

Una API que deba conservar o mostrar datos sensibles hace visible esa decisión
al caller. La capability permite alcanzar la frontera del host; no rebaja estas
obligaciones.

### 12.6 Código generado y specialization

Los providers estándar de `derive` y los generadores de schema son parte de la
distribución fijada. Producen código Tondo ordinario y especializado:

- acceso directo a fields y variants conocidos;
- loops y validaciones concretos para el tipo;
- llamadas estáticas a traits;
- constantes y tablas derivables en build time; y
- fast paths que el optimizador puede inlinear o eliminar.

No pueden introducir lookup por nombre en runtime, un registro global de tipos,
boxing universal ni un árbol dinámico intermedio cuando el caller solicita un
tipo estático. Un backend puede optimizar el resultado como código manual y debe
conservar una expansión inspeccionable para diagnostics y tooling.

---

## 13. Distribución reproducible

### 13.1 Contenido de una distribución

Una distribución estándar completa contiene, de forma cerrada:

1. Versión y PackageId.
2. Matriz de ediciones, targets, perfiles y capabilities soportados.
3. Fuentes Tondo portables.
4. Source sets estándar.
5. Interfaces canónicas.
6. Unidades privilegiadas y sus descriptores.
7. Hashes de cada componente.
8. API hash público.
9. Manifest de conformidad aplicable.
10. Documentación normativa, ejemplos verificables y enlaces a aceptación runtime.
11. Providers `derive`, programas meta y schemas de inputs/outputs.
12. Oracles escalares, vectores de interoperabilidad y corpus de benchmarks.
13. Descriptor canónico `tondo-standard-descriptor-0.1/1` que une runtime,
    companion meta, providers, hashes y límites.

No depende de archivos instalados fuera de su manifest lógico.

### 13.2 Selección por el plan

El manifiesto selecciona una identidad estándar exacta y el lockfile fija sus
bytes. La stdlib:

- No aparece como dependencia de usuario ordinaria.
- No se resuelve por directorio actual.
- No se busca en variables de entorno.
- No se descarga durante compilación.
- No cambia por orden del `PATH`.
- No se sustituye por una copia “compatible” sin cambiar el plan.

### 13.3 Hashes

La distribución utiliza SHA-256 lowercase. Como mínimo se fijan:

- Hash de contenido completo.
- Hash de interfaz pública.
- Hash de cada fuente.
- Hash de cada unidad privilegiada.
- Hash de cada input generado, si existiera.
- Hash de cada provider o programa meta.
- Hash de cada expansión y output generado que forme parte de la distribución.

El hash de contenido cambia ante cualquier byte. El API hash cambia solo ante
la interfaz pública canónica y sus contratos versionados. Cambiar implementación
privada conserva el API hash cuando no cambia ningún observable, pero cambia el
content hash.

### 13.4 Canonicalización

Listas, maps y sets serializados se ordenan por su identidad canónica. Los
formatos:

- Son UTF-8.
- Declaran versión.
- Rechazan campos desconocidos.
- No incluyen timestamps, paths físicos, PID ni orden de lectura.
- Conservan bytes exactos de source y unidades.

Con las mismas entradas declaradas, el toolchain produce el mismo plan,
interfaces y artefactos o el mismo error.

### 13.5 Confianza e integridad

STD-0.1 utiliza hashes de integridad y una instalación de toolchain confiable.
No define todavía firma criptográfica ni transparencia de paquetes. La ausencia
de firma no permite omitir la verificación de hash.

### 13.6 Release inmutable

Una versión publicada es inmutable. Una corrección produce una versión nueva.
No existe `latest`, canal mutable ni reemplazo in-place dentro de un lockfile.

---

## 14. Catálogo cerrado de STD-0.1

STD-0.1 puede publicar únicamente las siguientes superficies. Añadir otra exige
actualizar esta especificación y el tracker antes de implementar.

| Superficie | Clase | Capability | Edición mínima | Responsabilidad |
|---|---|---|---:|---|
| Métodos de tipos intrínsecos | Core | — | 0.1 | Option, Result, String, Array, Map, Set, Range e Iterator |
| `std.io` | Core | — | 0.1 | Protocolos de lectura/escritura, buffers, EOF, partial I/O y errores portables |
| `std.math` | Core | — | 0.1 | Matemática escalar portable y semántica IEEE nombrada |
| `std.format` | Core | — | 0.1 | Formatting explícito sobre `Display`, sin reflection |
| `std.serialization` | Core | — | 0.1 | Protocolo común de encode/decode cuando exista un segundo formato (14.7) |
| `std.reflect` | Core | — | 0.1 | Metadata de tipos retenida explícitamente, sin inspección dinámica de valores |
| `std.meta` | Build-only | — | 0.1 | Modelo semántico inmutable, requests y emisión para providers/generators |
| `std.encoding` | Core | — | 0.1 | Encodings binario-texto, incluidos Base64 y hexadecimal, con APIs materializadas y streaming |
| `std.json` | Core | — | 0.1 | JSON RFC 8259: encode/decode tipados, eventos, `Value` dinámico, JCS y `Raw` |
| `std.messagepack` | Core | — | 0.1 | MessagePack tipado, streaming y árbol dinámico explícito |
| `std.protobuf` | Core + Build-only | — | 0.1 | Wire format y código schema-first generado desde `.proto` |
| `std.yaml` | Core | — | 0.1 | YAML seguro, tipado y streaming sobre `std.serialization`, con límites explícitos |
| `std.toml` | Core | — | 0.1 | TOML tipado y árbol dinámico explícito, preservando errores con spans |
| `std.cbor` | Core | — | 0.1 | CBOR tipado, streaming y modo determinista explícito |
| `std.time` | Core + gated | `clock` monotónico; `civil-clock` para reloj de pared y anclas | 0.1 | Time-base monotónico, calendario civil y zonas horarias versionadas |
| `std.path` | Core | — | 0.1 | Operaciones léxicas sobre paths `String` |
| `std.regex` | Core | — | 0.1 | Expresiones regulares Unicode con complejidad y límites declarados |
| `std.hash` | Core | — | 0.1 | Digests SHA-1, SHA-256 y SHA-512 y HMAC |
| `std.random` | Core + gated | `entropy` para sembrar desde el sistema | 0.1 | Generadores pseudoaleatorios sembrados, mezcla, elección y bytes de entropía |
| `std.uuid` | Core + gated | `entropy` y/o `civil-clock` para generación | 0.1 | UUID, parsing, formatting y generadores explícitos por versión |
| `std.sync` | Core + gated | `threads` para operaciones cross-thread | 0.1 | Mutex, rwlock, condición, semáforo, once, barrier, atomics y colecciones compartidas linealizables |
| `std.executor` | Core + gated | `threads` para pools bloqueantes | 0.1 | Pools acotados, actores y bridge de trabajo bloqueante sobre las tareas del lenguaje |
| `std.log` | Core + gated | `console`, `filesystem` o `network` por sink | 0.1 | Eventos estructurados, niveles, backpressure y sinks explícitos |
| `std.console` | Capability-gated | `console` | 0.1 | stdin, stdout, stderr, texto, bytes y flushing |
| `std.env` | Hosted | — | 0.1 | Argumentos, entorno y directorio de trabajo |
| `std.fs` | Hosted | — | 0.1 | Filesystem, metadata y archivos |
| `std.process` | Capability-gated | `process` | 0.1 | Planes, procesos, pipes, status, output y cancelación |
| `std.net` | Capability-gated | `network` | 0.1 | Direcciones, DNS, sockets, streams, datagrams y frontera TLS |
| `std.testing` | Test-only | ninguna implícita | 0.1 | Control sellado y helpers portables del runner |

### 14.1 Reglas del catálogo

- No existe un módulo importable `std.core`; los nombres intrínsecos continúan
  en el prelude del lenguaje.
- `Array[Byte]` es la forma común de los datos binarios; ningún módulo define
  otro tipo binario.
- `std.io` posee los protocolos y errores compartidos; los handles concretos y
  sus efectos pertenecen a módulos capability-gated.
- `std.path` no toca el filesystem.
- `std.time.Duration` es usable sin `clock`; consultar tiempo o esperar
  contra un proveedor requiere `clock`.
- El calendario civil, parsing y aritmética son core cuando el target ha
  seleccionado el bundle de zonas versionado. Consultar la hora de pared
  requiere la capability separada `civil-clock`; relacionarla con un
  `Instant` requiere además `clock`.
- `std.console` no es un alias de `std.format`.
- `std.serialization` es el único owner de los traits estructurales compartidos;
  no existe un facade universal `std.codec` que oculte el formato.
- `std.reflect` describe tipos y nunca es dependencia de los codecs tipados.
- `std.meta` solo existe para programas del target `tondo-meta` y no concede
  capabilities de host.
- `std.protobuf` ejecuta su compilador de schema en build time; su runtime
  portable continúa siendo core.
- `std.encoding`, `std.yaml`, `std.toml` y `std.cbor` reutilizan `std.io` y
  `std.serialization`; no introducen otro facade universal ni árboles
  heterogéneos compartidos entre formatos.
- `std.sync` y `std.executor` reutilizan las tareas, el ownership y el modelo de
  memoria del lenguaje; ninguna API crea tareas desligadas ni un tipo de future.
- `std.time` separa estrictamente el reloj monotónico del calendario civil. El
  time-base de `Duration`, `Instant`, timers y deadlines pertenece a STD-0.1A;
  los tipos civiles y los datos de zona horaria versionados pertenecen al
  contrato B0 de STD-0.1B, nunca a una consulta ambiental durante compilación.
  `civil-clock` es una capability distinta de `clock`: la primera habilita
  lecturas de pared y la segunda el proveedor monotónico y las anclas que
  relacionan ambos dominios.
- `std.net` no concede red por import: cada target debe seleccionar `network` y
  cada operación conserva I/O, timeout y cancelación en su firma.
- `std.uuid` separa representación y parsing core de cualquier generador que
  requiera `entropy` o `civil-clock`; v7 nunca usa el `clock` monotónico como
  sustituto del reloj UTC.
- `std.log` define eventos puros en core; cada sink declara sus capabilities y
  política de backpressure sin alterar silenciosamente el control del programa.
  FileSink.create waits on the host: it opens its explicit path in the constructor
  and returns opening errors there. STD-LOG-HOST-001 implements the production
  hosted sink boundary, including both command streams and their observation
  adapters. Native logging remains a separate gate.
  Logging Append atomically creates a missing file or appends to an existing
  file; Truncate creates or truncates. Neither mode creates parent directories.
  The existing-only std.fs.OpenMode.Append contract is unchanged.
  Builtin sinks admit explicit queued records, drained by flush, consuming close,
  and Block on a full queue. Records are written separately without a worker.
  Hosted commands preserve the selected stdout/stderr stream and previously
  committed records before runtime diagnostics, including VM resource failure.
  A terminal VM resource failure does not manufacture user cleanup.
- Los argumentos del programa se obtienen mediante `std.env.args()`.
- `std.env` no lee el entorno durante la compilación.
- `std.testing` solo existe en source sets de test y no forma parte de producción.
- Streams, canales, red y sincronización solo aparecen bajo sus propietarios
  canónicos; ningún módulo existente los introduce indirectamente.

### 14.2 Estado del borrador

El catálogo y sus propietarios son normativos. Las declaraciones exhaustivas de
las veintinueve superficies permanecen pendientes, salvo el núcleo sellado que
`TONDO_TESTING_SPEC.md` ya fija para `std.testing`, `std.time`, `std.env` y el
contrato runtime-facing de `std.net` en la sección 14.4.6. No existe un contrato bootstrap separado: el corpus consume
siempre este borrador.

La implementación usa dos gates internos sin crear versiones distintas:

- **STD-0.1A / S1A** cierra intrinsics, `std.io`, `std.math`,
  `std.format`, `std.serialization`, `std.reflect`, `std.meta`, `std.json`,
  `std.messagepack`, `std.protobuf`, el sustrato monotónico de `std.time`,
  `std.path`, `std.console`, `std.env`, `std.fs`, `std.process` y
  `std.testing`. Es el corpus mínimo que debe existir antes del backend nativo.
- **STD-0.1B / S1** completa el contrato e implementación de `std.encoding`,
  `std.yaml`, `std.toml`,
  `std.cbor`, calendario civil y zonas de `std.time`, `std.regex`, `std.uuid`,
  `std.sync`, `std.executor`, `std.log` y `std.net` sobre el backend nativo.

S1A no es una release ni permite publicar una stdlib incompleta como 0.1.0.
Todos los módulos de ambas fases comparten una distribución, PackageId y
política de compatibilidad; Gate S1 fija sus hashes finales.

### 14.3 `std.time`

`std.time` mide intervalos, espera y lee la fecha y hora UTC:

~~~tondo
import std.time

let started = time.now()
time.sleep(time.milliseconds(250))
console.println("took {started.elapsed()}")       // took 250.1ms
console.println("now {time.utcNow()}")            // now 2026-10-22T06:38:28.5Z
~~~

#### 14.3.1 Duraciones

`Duration` es un intervalo con signo, en nanosegundos, del mismo rango que
`Int`. Es `Copy`, `Equatable` y `Key`.

~~~tondo pseudocode
pub type Duration

pub fn nanoseconds(count: Int): Duration
pub fn microseconds(count: Int): Duration
pub fn milliseconds(count: Int): Duration
pub fn seconds(count: Int): Duration
pub fn minutes(count: Int): Duration
pub fn hours(count: Int): Duration

pub fn Duration.nanoseconds(self): Int
pub fn Duration.microseconds(self): Int
pub fn Duration.milliseconds(self): Int
pub fn Duration.seconds(self): Int
pub fn Duration.minutes(self): Int
pub fn Duration.hours(self): Int
pub fn Duration.add(self, other: Duration): Duration
pub fn Duration.subtract(self, other: Duration): Duration
pub fn Duration.multiply(self, factor: Int): Duration
pub fn Duration.isZero(self): Bool
pub fn Duration.isNegative(self): Bool
pub fn Duration.isLessThan(self, other: Duration): Bool
~~~

- Los constructores multiplican por un factor exacto; las lecturas dividen y
  truncan hacia cero. Un resultado que no cabe produce el pánico `P0005`, como
  la aritmética de `Int`.
- `Display` usa la forma de Go: `1h2m3.5s`, `1.5s`, `250ms`, `1.5µs`, `42ns`,
  `0s`, con `-` delante de una duración negativa.

#### 14.3.2 Instantes

~~~tondo pseudocode
pub type Instant

pub fn now(): Instant
pub fn Instant.elapsed(self): Duration
pub fn Instant.since(self, earlier: Instant): Duration
pub fn Instant.add(self, delta: Duration): Instant
~~~

`now` lee un reloj monótono: sus instantes nunca retroceden, aunque cambie la
hora del sistema, y no significan una fecha. `elapsed` es `now().since(self)`;
`since` da la diferencia con signo. Un `Instant` es `Copy`, `Equatable` y
`Key`, pero no tiene `Display`. Un resultado que no cabe produce `P0005`.

#### 14.3.3 Esperas

~~~tondo pseudocode
pub fn sleep(delay: Duration)
~~~

`sleep` detiene la tarea, no el thread, al menos `delay`, y es una operación de
`select` (11.14 del lenguaje); un retraso cero o negativo no espera. Los
timeouts de `wait`, `receive` y `select` reciben también un `Duration`.

#### 14.3.4 Fecha y hora UTC

~~~tondo pseudocode
pub type DateTime = {
    year: Int
    month: Int
    day: Int
    hour: Int
    minute: Int
    second: Int
    nanosecond: Int
}

pub enum TimeError { InvalidDateTime }

pub fn utcNow(): DateTime
pub fn unixTime(): Duration
pub fn utc(sinceEpoch: Duration): DateTime
pub fn parseDateTime(text: String): DateTime ! TimeError
pub fn DateTime.sinceEpoch(self): Duration
~~~

`utcNow` y `unixTime` leen el reloj del sistema, que puede retroceder si
alguien lo ajusta; para medir intervalos se usa `now`. `unixTime` es el tiempo
desde `1970-01-01T00:00:00Z` y `utc` convierte uno de esos tiempos a fecha, en
el calendario gregoriano y sin segundos intercalares. `Display` de `DateTime`
es RFC 3339 en UTC, con la fracción solo cuando no es cero y sin ceros finales:
`2026-10-22T06:38:28Z`, `2026-10-22T06:38:28.5Z`. `sinceEpoch` es la
inversa de `utc`.

`parseDateTime` lee ISO 8601 en la forma extendida de RFC 3339 y la devuelve en
UTC: una fecha `2026-10-22`, que es su medianoche, o una fecha con hora
separada por `T`, `t` o un espacio, `HH:MM` con `:SS` y una fracción
opcionales, y una zona opcional: `Z`, `z`, `+HH:MM`, `+HHMM`, `+HH` o su
negativa; sin zona, la hora es UTC. Una fracción de más de nueve dígitos se
trunca. Un texto con otra forma, una fecha o una hora que no existe, el segundo
60 o un año fuera de 1678–2261, los que caben en un `Duration`, dan
`TimeError.InvalidDateTime`:

~~~tondo
let date = time.parseDateTime("2026-10-22 08:38:28.5+02:00")?
console.println("{date}")                         // 2026-10-22T06:38:28.5Z
let later = time.utc(date.sinceEpoch().add(time.hours(1)))
~~~

Las zonas horarias, la hora local y el formato de fechas en otros calendarios
quedan para una versión posterior.

#### 14.3.5 `std.env`: argumentos y entorno

`std.env` da los argumentos del programa y su entorno, que el programa puede
cambiar:

~~~tondo
import std.env

let files = env.args()
let level = match env.get("LOG_LEVEL") {
    some(value) => value,
    none => "info",
}
env.set("RUST_LOG", "debug")
env.setAll(["CC": "clang", "CFLAGS": "-O2"])
env.remove("TOKEN")
~~~

La superficie es:

~~~tondo pseudocode
pub enum EnvError { NotFound, NotADirectory, PermissionDenied, Host }

pub fn args(): Array[String]
pub fn get(name: String): String?
pub fn vars(): Map[String, String]
pub fn set(name: String, value: String)
pub fn setAll(values: Map[String, String])
pub fn remove(name: String)
pub fn currentDirectory(): String
pub fn setCurrentDirectory(path: String): Unit ! EnvError
pub fn syncNative()
~~~

- `args` devuelve los argumentos sin el nombre del programa, en orden. Los
  bytes que no son UTF-8 se sustituyen por `U+FFFD`, también en `get` y `vars`.
- `vars` devuelve todo el entorno ordenado por nombre.
- `set`, `setAll` y `remove` cambian el entorno de todo el programa, y los
  procesos que se lancen después lo heredan; un `Command` puede además fijar el
  entorno de un solo hijo (19.2). Un nombre vacío o con `=` o `NUL`, o un valor
  con `NUL`, produce el pánico `P0015`.
- El runtime guarda su propia copia del entorno y no llama a `setenv` del
  sistema, que no es seguro con varios threads: el código de C que llama un
  `extern fn` no ve los cambios. `syncNative()` los copia al entorno del sistema; se llama
  antes de que el código nativo lo lea y nunca mientras código nativo corre en
  otra tarea.
- `currentDirectory` y `setCurrentDirectory` leen y cambian el directorio de
  trabajo de todo el programa, contra el que se resuelven los paths relativos de
  `std.fs` y `std.process`.

### 14.4 Concurrencia de aplicación de STD-0.1B

Las tareas, los grupos, los canales y `select` pertenecen al lenguaje y al
preludio (sección 9.3 y 11.11–11.14 de la especificación del lenguaje). Esta
sección describe los módulos que los complementan para el estado compartido.

#### 14.4.4 Sincronización compartida

`std.sync` reúne el estado compartido entre tareas: primitivas de
sincronización y colecciones compartidas. Cada valor de sus tipos es un handle
hacia un estado compartido: copiarlo copia el handle, no el contenido, y todos
cumplen `Copy`, `Send` y `Share` salvo `Permit`. Ninguno cumple
`Equatable` ni `Key`. Las operaciones son linealizables, salvo el recuento de
`sync.Map` y `sync.Set` descrito en 14.4.4.2. Un lock, permiso o barrera
disputado detiene la tarea, no el thread, y la espera termina al cancelar la
tarea (11.15 de la especificación del lenguaje). Ninguna de estas operaciones es
un brazo de `select`: esperar un valor, limitar capacidad o elegir entre fuentes
pertenece a los canales.

El valor protegido nunca sale como préstamo: se observa o cambia dentro de un
cierre que se ejecuta con el lock tomado, y el lock se libera siempre al volver
el cierre, también si la tarea se cancela. Volver a tomar desde la misma tarea
un lock que ya tiene produce el pánico `P0014`.

##### 14.4.4.1 Primitivas

~~~tondo pseudocode
pub fn sync.Mutex(value: T): sync.Mutex[T]
pub fn sync.Mutex.update(self, change: fn(var T): R): R
pub fn sync.Mutex.tryUpdate(self, change: fn(var T): R): R?
pub fn sync.Mutex.updateWhen(self, ready: fn(T): Bool, change: fn(var T): R): R
pub fn sync.Mutex.get(self): T
pub fn sync.Mutex.set(self, value: T)

pub fn sync.RwLock(value: T): sync.RwLock[T]
pub fn sync.RwLock.read(self, observe: fn(T): R): R
pub fn sync.RwLock.update(self, change: fn(var T): R): R
pub fn sync.RwLock.get(self): T
pub fn sync.RwLock.set(self, value: T)

pub fn sync.Semaphore(permits: Int): sync.Semaphore
pub fn sync.Semaphore.acquire(self): sync.Permit
pub fn sync.Semaphore.tryAcquire(self): sync.Permit?
pub fn sync.Permit.release(self)

pub fn sync.Once.new[T](): sync.Once[T]
pub fn sync.Once.getOrInit(self, init: fn(): T): T        // T ! E con init: fn(): T ! E
pub fn sync.Once.get(self): T?
pub fn sync.Once.isReady(self): Bool

pub fn sync.Barrier(parties: Int): sync.Barrier
pub fn sync.Barrier.wait(self): Bool

pub fn sync.Atomic(value: T): sync.Atomic[T]             // T: Equatable
pub fn sync.Atomic.load(self): T
pub fn sync.Atomic.store(self, value: T)
pub fn sync.Atomic.swap(self, value: T): T
pub fn sync.Atomic.compareExchange(self, expected: T, desired: T): sync.CompareExchange[T]
pub fn sync.Atomic.add(self, delta: T): T                 // T entero

pub enum sync.CompareExchange[T] {
    Exchanged(T)
    Mismatch(T)
}
~~~

~~~tondo
import std.sync

let stats = sync.Mutex(Stats { hits: 0, misses: 0 })
stats.update((s) { s.hits += 1 })
let current = stats.get()

let requests = sync.Atomic(0)
let id = requests.add(1)
~~~

- `Mutex.update` ejecuta el cierre con acceso exclusivo al valor y devuelve su
  resultado; `tryUpdate` devuelve `none` sin esperar si otro lo tiene.
  `updateWhen` espera a que `ready` sea cierto, comprobándolo cada vez que el
  valor cambia, y entonces aplica `change` bajo el mismo lock: cubre lo que en
  otros lenguajes hace una variable de condición, sin `notify` ni wakeups
  espurios visibles. `get` devuelve una copia del valor y `set` lo reemplaza.
- `RwLock` admite varios `read` a la vez y un único `update`.
- Ninguna tarea espera indefinidamente un lock disputado: si una tarea lleva
  más de un milisegundo perdiéndolo frente a otras, el lock pasa a entregarse
  en orden de llegada hasta vaciar la cola, también a un escritor frente a
  lectores continuos.
- `Semaphore(n)` reparte `n` permisos. `acquire` espera uno libre. Un `Permit`
  no es `Copy`: el permiso vuelve con `release()` o al
  abandonarse el `Permit`, también al moverlo a una tarea que termina, de modo
  que `spawn fetch(url, limit.acquire())` limita la concurrencia.
- `Once` inicializa su valor una sola vez. Mientras una tarea ejecuta `init`,
  las demás esperan; si `init` falla, se cancela o produce pánico, el `Once`
  sigue vacío y la siguiente llamada lo reintenta.
- `Barrier(n).wait()` espera a que lleguen `n` tareas y devuelve `true` en
  exactamente una de ellas por ronda; después la barrera se reutiliza.
- `Atomic` no expone un orden de memoria: todas sus operaciones son
  secuencialmente consistentes. Con enteros, `Bool`, `Char` y `Byte` usa
  instrucciones atómicas; con otros tipos, un lock. `compareExchange` nunca falla de forma espuria:
  `Exchanged(previous)` si el valor coincidía y se reemplazó, y
  `Mismatch(observed)` si no. `add` devuelve el valor nuevo y produce `P0005`
  si no cabe.
- `Semaphore` y `Barrier` exigen un número positivo; otro produce el pánico
  `P0001`.

##### 14.4.4.2 Colecciones compartidas

~~~tondo pseudocode
pub type sync.Array[T]
pub type sync.Map[K: Key, V]
pub type sync.Set[K: Key]
pub type sync.Stack[T]
pub type sync.Queue[T]

pub fn sync.Array.length(self): Int
pub fn sync.Array.isEmpty(self): Bool
pub fn sync.Array.get(self, index: Int): T?
pub fn sync.Array.set(self, index: Int, value: T): T ! CollectionError
pub fn sync.Array.compareExchange(self, index: Int, expected: T, desired: T): sync.CompareExchange[T] ! CollectionError
pub fn sync.Array.snapshot(self): Array[T]

pub fn sync.Map.length(self): Int
pub fn sync.Map.isEmpty(self): Bool
pub fn sync.Map.get(self, key: K): V?
pub fn sync.Map.contains(self, key: K): Bool
pub fn sync.Map.insert(self, key: K, value: V): V?
pub fn sync.Map.remove(self, key: K): V?
pub fn sync.Map.compareExchange(self, key: K, expected: V?, desired: V?): sync.CompareExchange[V?]
pub fn sync.Map.snapshot(self): Map[K, V]

pub fn sync.Set.length(self): Int
pub fn sync.Set.isEmpty(self): Bool
pub fn sync.Set.contains(self, key: K): Bool
pub fn sync.Set.insert(self, key: K): Bool
pub fn sync.Set.remove(self, key: K): Bool
pub fn sync.Set.snapshot(self): Set[K]

pub fn sync.Stack.length(self): Int
pub fn sync.Stack.isEmpty(self): Bool
pub fn sync.Stack.push(self, value: T)
pub fn sync.Stack.pop(self): T?
pub fn sync.Stack.get(self, index: Int): T?
pub fn sync.Stack.snapshot(self): Array[T]

pub fn sync.Queue.length(self): Int
pub fn sync.Queue.isEmpty(self): Bool
pub fn sync.Queue.push(self, value: T)
pub fn sync.Queue.popFront(self): T?
pub fn sync.Queue.get(self, index: Int): T?
pub fn sync.Queue.snapshot(self): Deque[T]
~~~

Se construyen con los literales calificados de 5.11 de la especificación del
lenguaje (`sync.Map["one": 1]`), vacías con `sync.Queue.new[Job]()` o
`sync.Map.new[String, Int]()`, o a partir de una colección de valor con
`sync.Array(values)`, `sync.Map(entries)`, `sync.Set(keys)`, `sync.Stack(values)`
o `sync.Queue(values)`. Las operaciones usan los nombres de las colecciones de
valor (10.2.3): `sync.Array` tiene longitud fija y no cambia de tamaño;
`sync.Stack` es LIFO, con la cima al final, y `sync.Queue` es FIFO con varios
productores y consumidores. `pop` y `popFront` devuelven `none` al instante si
no hay elementos. Un índice negativo cuenta desde el final. `sync.Array.set` y
`compareExchange` devuelven `CollectionError.InvalidIndex` fuera de rango. En
`sync.Map.compareExchange`, `expected = none` compara con la ausencia y
`desired = none` elimina la entrada.

`snapshot()` copia en una colección de valor el estado de un único instante: por
índice en `sync.Array`, de la base a la cima en `sync.Stack` y del frente al
fondo en `sync.Queue`. `sync.Map` y `sync.Set` no conservan un orden: su
`snapshot()` sigue un orden sin especificar. `for value in shared` recorre el
`snapshot()` tomado al empezar el bucle y no ve los cambios posteriores; solo
admite bindings por valor.

Las colecciones están hechas para la menor contención posible. `sync.Map` y
`sync.Set` leen sin locks y cada escritura bloquea solo la franja de la tabla
donde cae su clave; `sync.Queue` y `sync.Stack` no usan locks; `sync.Array`
bloquea solo la franja de posiciones de cada operación. `length()` e
`isEmpty()` tampoco toman locks: son exactos en `sync.Array`, `sync.Stack` y
`sync.Queue`, y en `sync.Map` y `sync.Set` suman un contador por franja, de
modo que son exactos cuando no hay escrituras concurrentes y, si las hay,
pueden contar unas y no otras. Las demás operaciones son linealizables.
`snapshot()` es O(n).

#### 14.4.5 Executors, pools y actores

`std.executor` construye políticas de ejecución sobre `spawn`, `Group`, canales
y sincronización; no publica otro handle de tarea. Sus pools
tienen capacidad y shutdown explícitos. El bridge bloqueante mueve trabajo a
workers host acotados y su operación lógica espera como cualquier llamada, de
modo que el caller la llama secuencialmente o usa el `spawn` ordinario para solaparla.

Un actor es un owner de estado con mailbox tipado y ciclo de vida estructurado,
no una excepción al ownership. Shutdown, cancelación, saturación, pánico del
worker y rechazo de nuevo trabajo tienen outcomes nominales y drenan el cleanup
antes de completar. Ningún pool hereda capabilities o ambiente que el plan no
haya declarado.

El contrato exhaustivo de este owner está en
[`docs/contracts/stdlib-executor.md`](./docs/contracts/stdlib-executor.md) y su
registro machine-readable en
[`testing/stdlib-executor.json`](./testing/stdlib-executor.json). Ese registro
fija capacidad, backpressure, actores, bridge bloqueante, lifecycle,
capabilities y eventos privados. `STD-EXEC-IMPL-001` queda cerrado para la
implementación cooperativa hosted de pools, `Join`, lifecycle, actores y envíos; `STD-EXEC-HOST-001` queda cerrado para el bridge hosted de
`BlockingPool` y la lane privada de tokens del runtime nativo target-qualified
`x86_64-unknown-linux-gnu`; el informe queda en
`target/reliability/evidence/stdlib-executor-implementation.json`.
`STD-EXEC-TEST-001` queda cerrado para el modelo independiente, las regresiones
hosted, el stress de workers y el smoke fuzz acotado; su manifiesto es
[`testing/stdlib-executor-test.json`](./testing/stdlib-executor-test.json).
`STD-EXEC-PERF-001` queda cerrado por el presupuesto
[`docs/contracts/stdlib-executor-performance.md`](./docs/contracts/stdlib-executor-performance.md)
y el registro
[`testing/stdlib-executor-performance.json`](./testing/stdlib-executor-performance.json):
sus 27 muestras por workload mantienen separados el target hosted, el lane
nativo privado y cualquier lowering AOT que todavía no esté promocionado. El
`STD-EXEC-CONF-001` queda cerrado por el corpus común VM/native y la frontera
estática de `threads`, con contrato
[`testing/stdlib-executor-conformance.json`](./testing/stdlib-executor-conformance.json)
y guía [`docs/contracts/stdlib-executor-conformance.md`](./docs/contracts/stdlib-executor-conformance.md).
`STD-EXEC-DOC-001` queda cerrado por la guía ejecutable de scopes, pools,
actores, trabajo bloqueante, cancelación, shutdown, costes y cinco patrones de
composición. El contrato machine-readable, la fixture
`tests/runtime/m11-std-executor-doc-001.to`, sus sidecars y los checkers
`scripts/stdlib-executor-doc-check.sh` y
`scripts/stdlib-executor-doc-test.sh` mantienen la salida `executor-doc-ok`.
El siguiente leaf del owner es `DIAG-RUNTIME-001`.
El cierre no reclama ABI público ni lowering native AOT de callables. La VM
hosted ya verifica la
adquisición explícita `Actor.ref(ref self): ActorRef[M]` como proyección de
identidad no consumidora y ejecuta handlers cooperativos con mailbox FIFO,
estado mutable conservado y propagación terminal de `E` por `Actor.stop`.
También verifica `ActorRef.send` como operación transaccional:
`prepare` no muta ni consume el mensaje, un único `commit` lo linealiza en la
mailbox y `rollback` desregistra el waiter conservando el payload afín para el
perdedor o `else`. Esta observación mantiene la frontera target-qualified: el
runtime nativo sólo intercambia tokens opacos y los workers host ejecutan cada
job en un `Engine` hijo aislado; el lowering native AOT de callables y la
promoción pública siguen pendientes.

#### 14.4.6 Networking

The adopted hosted network provider is private Tokio 1.53.2, Hickory Resolver
0.26.3, Rustls 0.23.45 with AWS-LC and webpki-roots 1.0.9. Targets explicitly
supply the ordered numeric resolver endpoints in `[target.network]` of
`tondo.toml`; these inputs enter build identity, without inheriting system DNS.
TLS supports versions 1.2 and 1.3 with explicit suites and certificate/name
verification. Dependency and schema adoption does not close the implementation,
host or conformance leaves or establish native ABI/AOT coverage.

`std.net` es un owner capability-gated: el target debe declarar `network` y un
import nunca abre sockets, consulta DNS, lee proxies o certificados, crea tasks
ni toca el entorno. `clock` solo es necesario cuando el caller construye un
`NetOptions` con un `Instant` de deadline; `none` no instala un timeout
ambiental. Las operaciones que esperan siguen el modelo único de Tondo: una
llamada espera sin marcas y el solapamiento usa el `spawn`/`Join`
ordinario, sin `connectAsync`, futures o una API paralela.

El contrato normativo y el registro único son
[`docs/contracts/stdlib-net.md`](./docs/contracts/stdlib-net.md) y
[`testing/stdlib-net.json`](./testing/stdlib-net.json). Fijan:

- valores explícitos para nombres, direcciones y puertos, con DNS solo mediante
  `resolve`, sin resolución implícita en `connect`, retries ni Happy Eyeballs
  ambiental;
- límites finitos y por operación (`NetLimits`/`NetOptions`), errores nominales,
  deadlines monotónicos y cancelación cooperativa con cleanup antes de publicar
  `Timeout` o salir del scope;
- TCP con partial I/O observable, `ReadResult.Eof`, `TcpStream.shutdown` antes
  de `split` y mitades afines de lectura/escritura;
- UDP con datagrams atómicos y `DatagramTooLarge` sin truncación, sin promesas de entrega, orden o unicidad; y
- una frontera TLS target-declared con `PlatformRoots` por defecto, SNI y
  verificación de hostname, sin modo inseguro, downgrade a plaintext ni
  publicación del stream antes del handshake.

Los handles son afines, no se copian ni se comparten; `close` es explícito y la
conformance comprueba ownership, commit/rollback, límites, cancelación,
diagnóstico privado y ausencia de efectos por import. La superficie queda
`contract-locked`. The scalar kernel/private provider is verified separately
from the public hosted integration. The latter implements the 40 operations,
nominal types, explicit resolver configuration, receiving-account admission
and scope cleanup described in
[`stdlib-net-host.md`](docs/contracts/stdlib-net-host.md); its production hosted
boundary is verified after the joint selection, current-source quality,
complete functional checks and exact-source publication CI. The independent
model, regression and bounded-fuzz testing boundary is also verified.
Portable conformance, usage documentation and native ABI/AOT remain separate
`STD-NET-*` owner leaves. The independent bounded test register
is [`testing/stdlib-net-test.json`](testing/stdlib-net-test.json), with its exact
model, controlled hosted replay and fuzz scope in
[`stdlib-net-test.md`](docs/contracts/stdlib-net-test.md).

The verified hosted networking performance baseline is registered in
[testing/stdlib-net-performance.json](testing/stdlib-net-performance.json) and
[stdlib-net-performance.md](docs/contracts/stdlib-net-performance.md). It covers
21 controlled hosted bridge workloads and 27 retained samples each, including
connections, partial I/O, backpressure, explicit DNS/TLS, cancellation and
bounded refusal. Resource counters are selected logical observations; VM
interpreter, native ABI/AOT, SIMD and code-size timing are not promoted.

The public hosted networking conformance register is
[testing/stdlib-net-conformance.json](testing/stdlib-net-conformance.json), with
its bounded native Rust reference and VM-only integration boundary in
[stdlib-net-conformance.md](docs/contracts/stdlib-net-conformance.md). Its
`verified-public-hosted-vm-and-native-kernel-process` state closes its qualified
boundary using current-source quality, clean capture and exact-checkout CI.
The verified usage guide below completes `STD-NET-DOC-001`; the public hosted
logging core and capability-gated hosted sinks are verified; the next owner is
`STD-LOG-TEST-001`.
Public release and full normative trace coverage remain
separate gates.
No native Tondo networking target is supplied by that reference process.

The executable public hosted usage guide in
[stdlib-net.md](docs/contracts/stdlib-net.md) is bound to
`acceptance/projects/net-usage` and checked by `scripts/stdlib-net-doc-check.sh`.
It exercises six paths through the ordinary CLI: address values and keys,
partial TCP I/O and EOF, whole UDP datagrams, errors/deadlines, losing selection
arms and TLS configuration. Real TLS handshakes and successful DNS remain
separate conformance fixtures. Its `verified-public-hosted-usage` state is
supported by source-bound quality, the complete functional gate and
exact-checkout publication CI. Native networking ABI/AOT and public release
remain outside this usage promotion.

#### 14.4.7 `std.encoding`

`std.encoding` es el owner único de los encodings binario-texto de STD-0.1B.
No es un facade de JSON, MessagePack, Protobuf, YAML o TOML: esos módulos
conservan sus propios wire models y solo reutilizan la misma política Base64
cuando una anotación lo solicita. `std.encoding` no requiere capability y su
import no realiza I/O.

El contrato machine-readable y sus checks negativos viven en
[`testing/stdlib-encoding.json`](./testing/stdlib-encoding.json),
[`docs/contracts/stdlib-encoding.md`](./docs/contracts/stdlib-encoding.md),
[`scripts/stdlib-encoding-check.sh`](./scripts/stdlib-encoding-check.sh) y
[`scripts/stdlib-encoding-test.sh`](./scripts/stdlib-encoding-test.sh). La
frontera de fiabilidad del bloque de tests queda registrada en
[`testing/stdlib-encoding-test.json`](./testing/stdlib-encoding-test.json),
[`docs/contracts/stdlib-encoding-test.md`](./docs/contracts/stdlib-encoding-test.md),
[`scripts/stdlib-encoding-test-check.sh`](./scripts/stdlib-encoding-test-check.sh),
[`scripts/stdlib-encoding-test-test.sh`](./scripts/stdlib-encoding-test-test.sh) y
[`scripts/stdlib-encoding-fuzz.sh`](./scripts/stdlib-encoding-fuzz.sh). La
implementación hosted queda registrada además por
[`scripts/stdlib-encoding-implementation-check.sh`](./scripts/stdlib-encoding-implementation-check.sh),
[`scripts/stdlib-encoding-implementation-test.sh`](./scripts/stdlib-encoding-implementation-test.sh),
[`scripts/stdlib-encoding-implementation.sh`](./scripts/stdlib-encoding-implementation.sh)
y la fixture de VM. Estos artefactos cierran el diseño B0 y la ruta scalar
hosted, pero no promocionan una API pública ni un backend nativo.
El baseline de performance queda registrado por
[`testing/stdlib-encoding-performance.json`](./testing/stdlib-encoding-performance.json),
[`docs/contracts/stdlib-encoding-performance.md`](./docs/contracts/stdlib-encoding-performance.md),
[`scripts/stdlib-encoding-performance-check.sh`](./scripts/stdlib-encoding-performance-check.sh),
[`scripts/stdlib-encoding-performance-test.sh`](./scripts/stdlib-encoding-performance-test.sh) y
[`scripts/stdlib-encoding-performance.sh`](./scripts/stdlib-encoding-performance.sh).
Mide únicamente la ruta scalar del bridge VM hosted en 16 workloads; declara
explícitamente como no medidos el ABI nativo, SIMD, multiversionado, tamaño de
código y lowering AOT.
La conformance VM/native queda registrada por
[`testing/stdlib-encoding-conformance.json`](./testing/stdlib-encoding-conformance.json),
[`docs/contracts/stdlib-encoding-conformance.md`](./docs/contracts/stdlib-encoding-conformance.md),
[`scripts/stdlib-encoding-conformance-check.sh`](./scripts/stdlib-encoding-conformance-check.sh),
[`scripts/stdlib-encoding-conformance-test.sh`](./scripts/stdlib-encoding-conformance-test.sh),
[`scripts/stdlib-encoding-conformance.sh`](./scripts/stdlib-encoding-conformance.sh) y
la fixture/sonda compartidas. Cierra seis casos de bytes, streaming, errores,
límites y cleanup sobre VM hosted y el ABI nativo privado; no promociona
Cranelift/AOT, layout FFI ni una ruta SIMD.
La documentación de uso queda registrada por
[`scripts/stdlib-encoding-doc-check.sh`](./scripts/stdlib-encoding-doc-check.sh),
[`scripts/stdlib-encoding-doc-test.sh`](./scripts/stdlib-encoding-doc-test.sh) y
la fixture `tests/runtime/m11-std-encoding-doc-001.to`; `STD-ENCODING-DOC-001`
cierra una única forma por policy, errores, costes y ejemplos materializados o
streaming sin cambiar la frontera hosted/native.

La superficie canónica es:

~~~tondo pseudocode
pub enum Base64Alphabet { Standard, UrlSafe }
pub enum Base64Padding { Required, Omitted }
pub enum HexCase { Lower, Upper, Any }

pub type EncodingError = { kind: EncodingErrorKind, offset: Int }
pub type EncodingLimits = { maxInputBytes: Int, maxOutputBytes: Int }
pub type Base64Options = { alphabet: Base64Alphabet, padding: Base64Padding, limits: EncodingLimits }
pub type HexOptions = { case: HexCase, limits: EncodingLimits }

pub fn EncodingLimits.defaults(): EncodingLimits
pub fn EncodingLimits.create(maxInputBytes: Int, maxOutputBytes: Int): EncodingLimits ! EncodingError
pub fn Base64Options.standard(limits: EncodingLimits): Base64Options
pub fn Base64Options.urlSafe(limits: EncodingLimits): Base64Options
pub fn Base64Options.urlSafeUnpadded(limits: EncodingLimits): Base64Options
pub fn Base64Options.encode(self, input: Array[Byte]): Array[Byte] ! EncodingError
pub fn Base64Options.decode(self, input: Array[Byte]): Array[Byte] ! EncodingError
pub fn Base64Options.encodeTo(self, input: Array[Byte], var output: std.io.Writer): Unit ! EncodingError
pub fn Base64Options.decodeFrom(self, var input: std.io.Reader): Array[Byte] ! EncodingError
pub fn Base64Options.encoder(self): Base64Encoder ! EncodingError
pub fn Base64Options.decoder(self): Base64Decoder ! EncodingError

pub fn HexOptions.lower(limits: EncodingLimits): HexOptions
pub fn HexOptions.upper(limits: EncodingLimits): HexOptions
pub fn HexOptions.anyCase(limits: EncodingLimits): HexOptions
pub fn HexOptions.encode(self, input: Array[Byte]): Array[Byte] ! EncodingError
pub fn HexOptions.decode(self, input: Array[Byte]): Array[Byte] ! EncodingError
pub fn HexOptions.encodeTo(self, input: Array[Byte], var output: std.io.Writer): Unit ! EncodingError
pub fn HexOptions.decodeFrom(self, var input: std.io.Reader): Array[Byte] ! EncodingError
pub fn HexOptions.encoder(self): HexEncoder ! EncodingError
pub fn HexOptions.decoder(self): HexDecoder ! EncodingError
~~~

Base64 usa RFC 4648 estándar o URL-safe, con padding requerido u omitido
seleccionado explícitamente. El encoder es siempre canónico; el decoder es
estricto respecto al alfabeto, padding, bits de relleno, whitespace y datos
posteriores. Hexadecimal emite dos dígitos ASCII por byte; `Lower` y `Upper`
son estrictas y `Any` acepta ambas cases pero siempre emite lowercase. No se
aceptan prefijos `0x`, separadores, MIME line-wrapping, autodetección ni
reemplazo de caracteres.

Las operaciones materializadas son collectors de los mismos estados
incrementales `Base64Encoder`/`Base64Decoder`/`HexEncoder`/`HexDecoder`.
`push` devuelve solo quanta completos, `finish` valida el carry y publica el
último fragmento; dividir por cualquier frontera produce el mismo output y
error. Los handles son afines y quedan terminales tras `finish` o error. Los
límites acumulados de input/output se comprueban antes de publicar; un fallo de
límite en `push` no cambia el carry. Los adapters `Reader`/`Writer` usan
`std.io` y el único modelo de espera de Tondo, sin APIs `Async` duplicadas.

La ruta escalar es el oráculo. SIMD y multiversionado solo pueden promoverse
después de demostrar equivalencia de bytes, errores, offsets, límites y
terminalidad. `STD-ENCODING-IMPL-001` ya verifica la ruta hosted scalar y
`STD-ENCODING-TEST-001` cierra el modelo independiente, los vectores, las
fronteras de chunk, los límites, los errores byte-exactos y el fuzz acotado.
`STD-ENCODING-PERF-001` fija el baseline scalar hosted y
`STD-ENCODING-CONF-001` verifica interoperabilidad VM/native mediante handles
opacos y el mismo kernel scalar. Como no existe una ruta SIMD optimizada,
`simd` permanece `not-measured-no-optimized-route`; native AOT sigue
explícitamente sin reclamar. `STD-ENCODING-DOC-001` queda cerrado por la guía
ejecutable y sus seis familias de ejemplos; el siguiente bloque del owner es
`STD-YAML-IMPL-001`.

### 14.5 El prelude y `std.math`

Todo programa tiene, sin importar nada, los métodos que el prelude da a los tipos
intrínsecos y los traits `Ordered` y `Numeric`. El prelude está escrito en Tondo
sobre operaciones del runtime; un programa solo compila los métodos que usa. Las
posiciones de un `String` son índices de escalares, como en el lenguaje.

~~~tondo pseudocode
pub trait Ordered { fn compare(self, other: Self): Int }
pub trait Numeric { fn zero(): Self; fn plus(self, other: Self): Self }
~~~

`Ordered` está implementado para los enteros, `Byte`, `Char`, `String`, `Bool`
y `Float`, cuyo `compare` pone `NaN` después de cualquier otro valor y lo iguala
a sí mismo; `Numeric`, para `Int` y `Float`. Un programa puede implementar
`Ordered` para sus tipos. `Int`, `Float`, `String` y `Char` tienen además
`compare` como método propio. Los enteros tienen las constantes `max` y `min`
(`Int.max`), y los floats `pi`, `e`, `infinity`, `nan` y `epsilon`.

`String`:

~~~tondo pseudocode
fn split(self, separator: String): Array[String]   // los caracteres con ""
fn lines(self): Array[String]                       // sin \n ni \r\n
fn words(self): Array[String]                       // entre espacios Unicode
fn repeat(self, count: Int): String
fn padStart(self, width: Int, fill: String = " "): String
fn padEnd(self, width: Int, fill: String = " "): String
fn trimStart(self): String
fn trimEnd(self): String
fn removePrefix(self, prefix: String): String
fn removeSuffix(self, suffix: String): String
fn findLast(self, needle: String): Int?
fn count(self, needle: String): Int
fn toLower(self): String                            // reglas de Unicode
fn toUpper(self): String
fn compare(self, other: String): Int
fn chars(self): CharIterator                        // Iterator[Char], perezoso
fn Int.parse(input: String, radix: Int = 10): Int ! text.TextError
fn Float.parse(input: String): Float ! text.TextError
~~~

`Int.parse` acepta un signo y `_` entre dígitos en bases de 2 a 36; `Float.parse`
lee la forma decimal, `inf`, `-inf` y `NaN`. Un texto que no es un número que
cabe da `TextError.InvalidData`.

`Char`: `isDigit`, `isLetter`, `isWhitespace`, `isUpper`, `isLower` e
`isAlphanumeric` (`Bool`); `toUpper` y `toLower`, que cambian el escalar cuando
su otra forma es uno solo; `code(): Int`, `Char.from(code: Int): Char?` y
`compare`.

`Int`: `abs`, `min(other)`, `max(other)`, `clamp(low, high)`, `pow(exponent)`,
cuyo desbordamiento es un pánico como en toda aritmética, `toString(radix: Int =
10)` y `compare`. `Float`: `abs`, `min`, `max`, `clamp`, `floor`, `ceil`,
`round` (la mitad va al par), `truncate`, `sqrt(): Float ! math.MathError`,
`pow(exponent: Float)`, `isNaN`, `isInfinite`, `isFinite`, `toFixed(decimals:
Int): String` y `compare`.

`Option[T]`: `isSome`, `isNone`, `map(fn(T): U): U?`, `andThen(fn(T): U?): U?`,
`unwrapOr(fallback: T): T`, `orElse(other: T?): T?`, `filter(fn(T): Bool): T?`,
`okOr(error: E): T ! E` y `expect(message: String): T`, que hace pánico con el
mensaje si no hay valor. `Result[T, E]`: `isOk`, `isErr`, `map(fn(T): U): U ! E`,
`mapErr(fn(E): F): T ! F`, `andThen(fn(T): U ! E): U ! E`, `unwrapOr`, `ok(): T?`,
`err(): E?` y `expect`.

`Array[T]`, que da arrays nuevos salvo los métodos `mut` y `var`:

~~~tondo pseudocode
fn first(self): T?
fn last(self): T?
fn map[U](self, transform: fn(T): U): Array[U]
fn filter(self, keep: fn(T): Bool): Array[T]
fn flatMap[U](self, transform: fn(T): Array[U]): Array[U]
fn reduce[A](self, initial: A, combine: fn(A, T): A): A
fn any(self, predicate: fn(T): Bool): Bool
fn all(self, predicate: fn(T): Bool): Bool
fn find(self, predicate: fn(T): Bool): T?
fn findIndex(self, predicate: fn(T): Bool): Int?
fn count(self, predicate: fn(T): Bool): Int
fn sum(self): T                                     // T: Numeric
fn min(self): T?                                    // T: Ordered
fn max(self): T?                                    // T: Ordered
fn sorted(self): Array[T]                           // T: Ordered, estable
fn sortedBy[K: Ordered](self, key: fn(T): K): Array[T]
fn sortedWith(self, compare: fn(T, T): Int): Array[T]
fn reversed(self): Array[T]
fn unique(self): Array[T]                           // T: Key
fn take(self, count: Int): Array[T]
fn drop(self, count: Int): Array[T]
fn zip[U](self, other: Array[U]): Array[(T, U)]
fn enumerate(self): Array[(Int, T)]
fn chunks(self, size: Int): Array[Array[T]]
fn windows(self, size: Int): Array[Array[T]]
fn groupBy[K: Key](self, key: fn(T): K): Map[K, Array[T]]
fn partition(self, predicate: fn(T): Bool): (Array[T], Array[T])
fn join(self, separator: String): String            // T: Display
fn sort(mut self)                                   // T: Ordered
fn sortBy[K: Ordered](mut self, key: fn(T): K)
fn reverse(mut self)
fn swap(mut self, i: Int, j: Int)
fn retain(var self, keep: fn(T): Bool)
fn extend(var self, other: Array[T])
fn iter(self): ArrayIterator[T]                     // Iterator[T], perezoso
~~~

`Map[K, V]`: `getOr(key, fallback): V`, `mapValues(fn(V): W): Map[K, W]`,
`filter(fn(K, V): Bool): Map[K, V]` y `merge(other): Map[K, V]`, donde gana
`other`. `Set[T]`: `union`, `intersection`, `difference` e `isSubset`.

`Range[T]` (10.16 del lenguaje) es el valor de `start..end` y `start..=end`,
de enteros o `Char`: implementa `Iterator[T]`, tiene `contains(value: T): Bool`,
que es lo que pregunta `value in range`, `start()`, `end()`, `isInclusive()` e
`isEmpty()`, y `Display` lo escribe como se escribe (`0..10`). Un `Range[Char]`
salta los surrogates.

`Iterator[T]`, sobre cualquier iterador, incluidos los rangos, `String.chars()`,
`Array.iter()`, los cursores de `Map.keys`, `values` y `entries` y los tipos del
programa que lo implementan:

- adaptadores perezosos, que solo piden elementos a su fuente cuando se les
  piden los suyos: `map`, `filter`, `take`, `skip`, `takeWhile`, `skipWhile`,
  `enumerate`, `zip`, `chain` y `flatMap`;
- consumidores: `collect(): Array[T]`, `count`, `any`, `all`, `find`, `reduce` y
  `forEach`. Para sumar, ordenar o unir se usa `collect()` y el método del array.

Estos métodos trabajan sobre una copia del iterador, así que su tipo es `Copy`;
con uno afín, el error dice que se recorra con `for`.

Un cierre que se pasa a estos métodos toma los tipos de sus parámetros de la
llamada: `numbers.map((n) { n * 2 })`, `(0..10).filter((n) { n % 2 == 0 })` o
`text.chars().filter((c) { c.isLetter() })`.

`std.math` da las operaciones escalares con la semántica de IEEE-754 y sin
fast-math. Una operación fuera de su dominio es un error, nunca un `NaN`
silencioso:

~~~tondo pseudocode
pub enum MathError { Domain }
pub fn sqrt(value: Float): Float ! MathError
pub fn ln(value: Float): Float ! MathError
pub fn log10(value: Float): Float ! MathError
pub fn log2(value: Float): Float ! MathError
pub fn asin(value: Float): Float ! MathError
pub fn acos(value: Float): Float ! MathError
pub fn pow(base: Float, exponent: Float): Float
pub fn exp(value: Float): Float
pub fn sin(value: Float): Float
pub fn cos(value: Float): Float
pub fn tan(value: Float): Float
pub fn atan(value: Float): Float
pub fn atan2(y: Float, x: Float): Float
pub fn hypot(x: Float, y: Float): Float
pub fn floor(value: Float): Float
pub fn ceil(value: Float): Float
pub fn round(value: Float): Float                   // la mitad va al par
pub fn roundTiesAway(value: Float): Float           // la mitad se aleja de cero
pub fn truncate(value: Float): Float
pub fn fma(a: Float, b: Float, c: Float): Float     // redondea una vez
pub fn gcd(a: Int, b: Int): Int
pub fn lcm(a: Int, b: Int): Int
~~~

### 14.6 Arquitectura común de serialización

Cada formato ofrece dos rutas:

- **Tipada:** un tipo se codifica y decodifica directamente con los traits del
  formato, sin pasar por un árbol ni consultar reflection. El compilador
  implementa esos traits para cada record, enum y newtype cuyas partes los
  implementan; un `impl` del programa sustituye al automático.
- **Dinámica:** un árbol explícito (`json.Value`) para documentos cuyo schema no
  se conoce al compilar.

Las anotaciones de campo `@name("wire")` e `@ignore` (9.2 del lenguaje) eligen
el nombre de un campo fuera del programa u omiten un campo opcional, en todos
los formatos. Un formato ofrece además lectura y escritura por eventos, que
comparten parser y emisor con las demás rutas.

### 14.7 `std.serialization`

Los traits de codificación viven por ahora en el único formato implementado,
`std.json`. Cuando exista un segundo formato, `std.serialization` reunirá un
protocolo de eventos común y los traits pasarán a ser compartidos, con las
mismas implementaciones automáticas y anotaciones.

### 14.8 `std.reflect`

`std.reflect` lee, escribe y llama por nombre sobre valores cuyo tipo una
herramienta no conoce de antemano, y describe tipos. Las reglas del lenguaje
(conversión implícita a `Value`, visibilidad, conversión por forma, errores de
compilación y coste) están en 27.10 de la especificación del lenguaje. Su
superficie es:

~~~tondo pseudocode
pub type Value
pub type Field = { name: String, value: Value }

pub fn valueOf[T: Reflect](value: T): Value
pub fn get[T: Reflect, S: Reflect](value: S, path: String): T ! ReflectError
pub fn set[S: Reflect](value: mut S, path: String, new: Value): Unit ! ReflectError
pub fn call[R: Reflect, S: Reflect](target: S, method: String, args: ...Value): R ! ReflectError
pub fn callMut[R: Reflect, S: Reflect](target: mut S, method: String, args: ...Value): R ! ReflectError
pub fn convert[T: Reflect, S: Reflect](value: S): T
pub fn typeOf[T: Reflect](value: T): TypeInfo
pub fn typeInfo[T: Reflect](): TypeInfo

pub fn Value.typeName(self): String
pub fn Value.typeInfo(self): TypeInfo
pub fn Value.to[T: Reflect](self): T ! ReflectError
pub fn Value.get(self, path: String): Value ! ReflectError
pub fn Value.set(mut self, path: String, new: Value): Unit ! ReflectError
pub fn Value.call(self, method: String, args: ...Value): Value ! ReflectError
pub fn Value.callMut(mut self, method: String, args: ...Value): Value ! ReflectError
pub fn Value.fields(self): Array[Field]
pub fn Value.elements(self): Array[Value]
pub fn Value.entries(self): Array[(Value, Value)]
pub fn Value.variant(self): String?
pub fn Value.optional(self, name: String): Value? ! ReflectError
impl Display for Value

pub trait Reflect {
    fn reflectGet(self, name: String): Value ! ReflectError
    fn reflectSet(mut self, name: String, value: Value): Unit ! ReflectError
    fn reflectFields(self): Array[Field]?
    fn reflectElements(self): Array[Value]?
    fn reflectEntries(self): Array[(Value, Value)]?
    fn reflectVariant(self): String?
    fn reflectCall(self, method: String, args: Array[Value]): Value ! ReflectError
    fn reflectCallMut(mut self, method: String, args: Array[Value]): Value ! ReflectError
    fn reflectShow(self, nested: Bool): String
    fn reflectFrom(value: Value): Self ! ReflectError
    fn reflectDescription(): TypeDescription
}

pub enum ReflectErrorKind {
    NoField
    NoMethod
    NoValue
    WrongType
    WrongArguments
    OutOfRange
}
pub type ReflectError = { kind: ReflectErrorKind, path: String, message: String }
pub fn ReflectError.noField(typeName: String, name: String, fields: Array[String]): ReflectError
pub fn ReflectError.noMethod(typeName: String, name: String, methods: Array[String]): ReflectError
pub fn ReflectError.wrongType(expected: String, value: Value): ReflectError
pub fn ReflectError.arguments(typeName: String, method: String, expected: Int, found: Int): ReflectError
pub fn ReflectError.privateFields(typeName: String, value: Value): ReflectError
pub fn ReflectError.needsMut(typeName: String, method: String): ReflectError
~~~

`Reflect` tiene una implementación por defecto de cada operación salvo
`reflectShow`; los constructores de `ReflectError`, `TypeDescription` y los
descriptores (`FieldInfo.of`, `VariantInfo.of`, `MethodInfo.of`,
`ParameterInfo.of`, `FunctionInfo.of`) son los que usa el código que genera el
compilador, y un `impl` escrito a mano puede usarlos.

Valores:

- `valueOf`, `get`, `call` y `typeOf` toman el valor por valor, que es una copia
  porque los valores que se reflejan son `Copy`; `set` y `callMut` lo cambian con
  un préstamo `mut`.
- Una ruta separa con `.` nombres de campo, índices de `Array` y `Deque`, índices
  de un payload de tupla, y claves de `Map[String, V]`. Un `Option` con `some`
  es transparente y uno con `none` da `NoValue`. Un newtype es un valor único:
  la ruta no entra en él.
- `get[T]` y `Value.to[T]` dan el valor si es de tipo `T`, o lo convierten por
  forma (27.10 del lenguaje); `to[Value]` es el propio valor. `set` acepta un
  valor del tipo del campo o uno que se convierte a él. Un `Map` cambia entero:
  la reflexión no inserta ni reemplaza una de sus claves.
- `call` y `callMut` buscan un método `pub` por nombre, comprueban el número de
  argumentos y convierten cada uno al tipo de su parámetro, y dan su resultado
  completo como `R`: para un método `T ! E`, `R` es `T ! E`. Se llaman los
  métodos que reciben `self` o `mut self` y parámetros por valor. `call` solo
  llama métodos con `self`; uno con `mut self` necesita `callMut`.
- `fields()` da los campos visibles de un record o del payload record de la
  variante activa; `elements()` los de un `Array`, `Deque`, `Set` o payload de
  tupla; `entries()` los pares de un `Map`; `variant()` el nombre de la variante
  activa de un enum, `Option` o `Result`. Una vista que no corresponde al valor
  está vacía. `optional(name)` da el campo, o `none` si el valor no lo tiene.
- `Display` escribe un escalar como su propio `Display` y un compuesto como
  `User { name: "Ada", age: 36 }`, `[1, 2]`, `["a": 1]` o `Set[1, 2]`, con los
  strings entre comillas dentro de un compuesto y sin los campos `priv`.
- `ReflectError` dice qué ruta o método falló y su mensaje la corrección:
  "`User` has no field `nme`; its fields are `name`, `age` and `address`".

Descripción de tipos:

~~~tondo pseudocode
pub type TypeId: Copy + Equatable + Key + Send + Share

pub enum TypeKind {
    Primitive(PrimitiveKind)
    Record
    Enum
    Newtype
    Tuple
    Union
    Function
    Applied(AppliedKind)
    Reference(ReferenceKind)
    Opaque
}

pub enum PrimitiveKind {
    Bool
    Int
    Int8
    Int16
    Int32
    UInt8
    UInt16
    UInt32
    UInt64
    Float
    Float32
    Byte
    Char
    String
    Unit
    Never
}

pub enum AppliedKind {
    Array
    Map
    Set
    Range
    Option
    Result
    Other
}

pub enum ReferenceKind {
    Ref
    Pointer
}

pub enum TypeCapability {
    Copy
    Equatable
    Key
    Send
    Share
}

pub type TypeInfo
pub type FieldInfo
pub type VariantInfo
pub type MethodInfo
pub type ParameterInfo
pub type FunctionInfo

pub type TypeDescription = {
    kind: TypeKind
    generics: Array[TypeInfo]
    fields: Array[FieldInfo]
    variants: Array[VariantInfo]
    methods: Array[MethodInfo]
    elements: Array[TypeInfo]
    underlying: TypeInfo?
}
pub fn TypeDescription.of(kind: TypeKind): TypeDescription
~~~

`TypeInfo` es un handle inmutable y barato de copiar, que calcula lo que
describe al pedirlo, así que un tipo recursivo no se expande sin fin. Su
`Display` es su nombre y los enums de descripción muestran el nombre de su
variante. Expone:

- `id(): TypeId`, `qualifiedName(): String` y `kind(): TypeKind`;
- `genericArguments(): Array[TypeInfo]` en orden de declaración;
- `capabilities(): Set[TypeCapability]` con solo capacidades que el compilador
  demuestra: hoy `Copy`, `Equatable` y `Key`, porque `Send` y `Share` todavía no
  se comprueban;
- `underlying(): TypeInfo?`, el tipo que envuelve un newtype;
- `fields(): Array[FieldInfo]` de un record;
- `variants(): Array[VariantInfo]` de un enum;
- `methods(): Array[MethodInfo]` con los métodos `pub`;
- `tupleElements(): Array[TypeInfo]` de una tupla o unión, en el orden canónico
  del tipo; y
- `function(): FunctionInfo?` de una función.

Los descriptores secundarios exponen consultas, sin campos públicos:

- `FieldInfo`: `name(): String`, `typeInfo(): TypeInfo`, `ordinal(): Int`,
  `docs(): String?`. Los ordinales conservan la posición declarada aunque se
  omitan campos privados. La documentación son las líneas `///` justo antes del
  campo, sin los marcadores.
- `VariantInfo`: `name(): String`, `ordinal(): Int`,
  `payloadKind(): VariantPayloadKind` (`Unit | Tuple | Record`),
  `tupleElements(): Array[TypeInfo]` y `fields(): Array[FieldInfo]`.
- `MethodInfo`: `name(): String`, `receiver(): ParameterMode` y
  `function(): FunctionInfo`, para los métodos que `call` y `callMut` pueden
  llamar.
- `ParameterInfo`: `position(): Int`, `typeInfo(): TypeInfo` y
  `mode(): ParameterMode` (`Value | Ref | Mut | Var`).
- `FunctionInfo`: `parameters(): Array[ParameterInfo]`, `outcome(): TypeInfo`,
  `variadic(): Bool` y `isUnsafe(): Bool`. El resultado incluye el `Result` o
  `Option` completo.

Consultar una vista que no corresponde al kind devuelve la colección vacía o
`none`. Las colecciones están en orden canónico y no dependen de direcciones ni
de un registro global. Los descriptores no contienen offsets, tamaño, ABI,
direcciones ni bytes de un valor.

`TypeId` es una identidad opaca por artefacto: admite igualdad y hash para ser
key durante esa ejecución; no se construye desde enteros o strings, no tiene
orden ni encoding estándar.

### 14.9 `std.meta`

`std.meta` solo está disponible en el perfil `meta` (27.7 del lenguaje), para
providers de `derive` y generadores. Define las peticiones que reciben, el
builder con el que escriben fuente y sus respuestas.

~~~tondo pseudocode
pub type DeriveRequest
pub type GenerateRequest
pub type Declaration
pub enum DeclarationKind { Record, Enum, Newtype }
pub type FieldView = { name: String, typeRef: TypeRef, private: Bool }
pub type VariantView = { name: String, fields: Array[FieldView], elements: Array[TypeRef] }
pub type GenericView = { name: String, bounds: Array[String] }
pub type TypeRef
pub type Input = { name: String, bytes: Array[Byte], sha256: String }
pub type SourceBuilder
pub type DeriveResponse
pub type GenerateResponse
pub enum Error {
    UnknownInput(String)
    UnknownOutput(String)
    DuplicateOutput(String)
    MissingOutput(String)
    OutputLimit
    Provider(String)
    TypeRendering(String)
}

pub fn DeriveRequest.target(self): Declaration
pub fn DeriveRequest.traitName(self): String
pub fn DeriveRequest.sourceBuilder(self): SourceBuilder
pub fn GenerateRequest.inputs(self): Array[Input]
pub fn GenerateRequest.input(self, name: String): Input ! Error
pub fn GenerateRequest.outputs(self): Array[String]
pub fn GenerateRequest.declarations(self): Array[Declaration]
pub fn GenerateRequest.sourceBuilder(self): SourceBuilder

pub fn Input.text(self): String ! Error

pub fn Declaration.name(self): String
pub fn Declaration.module(self): String
pub fn Declaration.kind(self): DeclarationKind
pub fn Declaration.typeName(self): String
pub fn Declaration.generics(self): Array[GenericView]
pub fn Declaration.fields(self): Array[FieldView]
pub fn Declaration.variants(self): Array[VariantView]
pub fn Declaration.underlying(self): TypeRef?

pub fn TypeRef.identity(self): String

pub fn code(template: String, values: Map[String, String]): String
pub fn SourceBuilder.renderType(mut self, ty: TypeRef): String ! Error
pub fn SourceBuilder.add(mut self, source: String): Unit ! Error
pub fn SourceBuilder.write(mut self, output: String, source: String): Unit ! Error
pub fn SourceBuilder.finish(self): GenerateResponse ! Error
pub fn SourceBuilder.finishDerive(self): DeriveResponse ! Error
pub fn SourceBuilder.error(mut self, message: String)
~~~

- `DeriveRequest.target()` es la vista del tipo autorizado por la declaración
  `derive`, con sus campos privados (27.4); `traitName()` es el trait como lo
  escribe el `derive`. `GenerateRequest.declarations()` da las declaraciones
  públicas de los roots del generador y las que alcanzan sus campos públicos,
  variantes y tipos envueltos; `module()` dice dónde está cada una.
- `Input.text()` lee los bytes como UTF-8, y falla con `Error.Provider` si no
  lo son.
- `Declaration.typeName()` es el tipo con sus parámetros, como lo nombra la
  cabecera de un `impl`: `Page[T]`.
- Un `TypeRef` es el tipo como lo escribe el módulo de la declaración; su
  `identity()` es ese texto, y `renderType` lo escribe tal cual.
- `code` sustituye cada `$nombre` de la plantilla por su valor y `$$` por `$`;
  con una plantilla raw, la plantilla es literalmente el código generado. Un
  `$nombre` sin valor es un pánico, que el toolchain informa como fallo del
  provider.
- Un builder de `derive` admite su fuente una vez con `add`, y `finishDerive`
  exige que la tenga; el toolchain comprueba que sea exactamente el `impl`
  pedido. Uno de generador admite con `write` cada salida declarada una vez, y
  `finish` exige todas. `error` añade un diagnóstico, que impide publicar la
  respuesta.

No ofrece filesystem, environment, procesos, reloj, entropía, red ni callbacks
del compilador: todo lo que un provider o generador necesita llega en la
petición.

### 14.10 `std.json`

`std.json` implementa JSON UTF-8 según
[RFC 8259](https://www.rfc-editor.org/rfc/rfc8259.html), en modo estricto: un
único valor, sin datos detrás, sin claves duplicadas salvo que se permitan y
con una profundidad máxima de 128. Está escrito en Tondo sobre un tokenizador y
un emisor del runtime.

~~~tondo
import std.json

type User = {
    @name("user_id")
    id: Int
    name: String
    email: String?
}

let user: User = json.decode(text)?
let back = json.encode(user, pretty: true)

let document = json.parse(text)?
let city = document["users"][0]["address"]["city"].asText()
~~~

La superficie es:

~~~tondo pseudocode
pub enum Value {
    Null
    Bool(Bool)
    Number(Number)
    Text(String)
    Array(Array[Value])
    Object(Map[String, Value])
}
pub type Number
pub type Raw
pub enum ErrorKind { Syntax, Type, Missing, Unknown, Range, Io }
pub type JsonError = { kind: ErrorKind, message: String, path: String, line: Int, column: Int }
pub enum UnknownFields { Ignore, Reject }

pub trait Encode {
    fn encodeJson(self, out: Writer)
}
pub trait Decode {
    fn decodeJson(input: Reader, first: Event): Self ! JsonError
}

pub fn encode[T: Encode](value: T, pretty: Bool = false, canonical: Bool = false): String
pub fn decode[T: Decode](text: String, unknownFields: UnknownFields = UnknownFields.Ignore, allowDuplicateKeys: Bool = false): T ! JsonError
pub fn encodeTo[T: Encode](value: T, stream: io.Writer, pretty: Bool = false, canonical: Bool = false): Unit ! JsonError
pub fn decodeFrom[T: Decode](stream: io.Reader, unknownFields: UnknownFields = UnknownFields.Ignore, allowDuplicateKeys: Bool = false): T ! JsonError
pub fn parse(text: String): Value ! JsonError
pub fn toValue[T: Encode](value: T): Value
pub fn fromValue[T: Decode](value: Value): T ! JsonError
pub fn number(text: String): Number ! JsonError
pub fn raw(text: String): Raw ! JsonError

pub fn Value.get(self, key: String): Value?
pub fn Value.at(self, index: Int): Value?
pub fn Value.field(self, key: String): Value
pub fn Value.item(self, index: Int): Value
pub fn Value.asText(self): String?
pub fn Value.asInt(self): Int?
pub fn Value.asFloat(self): Float?
pub fn Value.asBool(self): Bool?
pub fn Value.asArray(self): Array[Value]?
pub fn Value.asObject(self): Map[String, Value]?
pub fn Value.isNull(self): Bool
pub fn Number.text(self): String
pub fn Number.toInt(self): Int?
pub fn Number.toFloat(self): Float
pub fn Raw.text(self): String

pub enum Event { Null, Bool(Bool), Number(Number), Text(String), BeginArray, EndArray, BeginObject, EndObject, Key(String) }
pub type Reader
pub type Writer
pub fn reader(text: String, unknownFields: UnknownFields = UnknownFields.Ignore, allowDuplicateKeys: Bool = false): Reader
pub fn readerFrom(stream: io.Reader, unknownFields: UnknownFields = UnknownFields.Ignore, allowDuplicateKeys: Bool = false): Reader
pub fn writer(pretty: Bool = false, canonical: Bool = false): Writer
pub fn writerTo(stream: io.Writer, pretty: Bool = false, canonical: Bool = false): Writer
pub fn Reader.next(self): Event? ! JsonError
pub fn Writer.null(self)
pub fn Writer.bool(self, value: Bool)
pub fn Writer.int(self, value: Int)
pub fn Writer.float(self, value: Float)
pub fn Writer.number(self, value: Number)
pub fn Writer.string(self, value: String)
pub fn Writer.key(self, name: String)
pub fn Writer.beginArray(self)
pub fn Writer.endArray(self)
pub fn Writer.beginObject(self)
pub fn Writer.endObject(self)
pub fn Writer.finish(self): String
pub fn Writer.flush(self): Unit ! JsonError
~~~

Codificación tipada:

- `Encode` y `Decode` están implementados para `Int`, `Float`, `Bool`,
  `String`, `T?`, `Array[T]`, `Map[String, V]`, `Value`, `Number` y `Raw`, y el
  compilador los implementa para cada record, enum y newtype del programa cuyas
  partes los implementan. Una implementación escrita en el programa sustituye a
  la automática.
- Un record es un objeto con sus campos en orden de declaración; `none` es
  `null`, y un campo opcional ausente se decodifica como `none`. Un campo
  obligatorio ausente da `Missing`.
- Un newtype es su valor. Una variante sin payload es su nombre como string; una
  con payload es un objeto de una sola clave, el nombre, cuyo valor es el
  payload, un array si tiene varios componentes o un objeto si tiene campos.
- Los campos desconocidos se ignoran, salvo con
  `unknownFields: UnknownFields.Reject`, que da `Unknown`.
- `pretty` indenta con dos espacios; `canonical` escribe
  [RFC 8785 (JCS)](https://www.rfc-editor.org/rfc/rfc8785.html): claves
  ordenadas por unidades UTF-16, sin espacios y números como doubles. Un float
  no finito se escribe `null`.

Documentos dinámicos:

- `parse` da un `Value`. `document["key"]` y `document[index]` leen un miembro o
  un elemento, o `Null` cuando no existe, de modo que un camino se encadena sin
  comprobar cada paso; `get` y `at` dan `none` en su lugar. `asText`, `asInt`,
  `asFloat`, `asBool`, `asArray` y `asObject` dan el contenido cuando es de ese
  tipo. Un objeto conserva el orden de sus claves.
- Un `Number` guarda el número tal como se escribió, sin perder precisión antes
  de que el programa elija `toInt` o `toFloat`.
- Un `Raw` guarda un valor JSON validado sin interpretarlo, por ejemplo un
  payload que otra parte del programa decodificará, y se escribe tal cual.

Lectura y escritura por eventos:

- Un `Reader` entrega `Event`s en orden y `none` tras el documento; un `Writer`
  recibe valores, claves y corchetes y `finish()` da el texto. Son la base de
  `encode` y `decode` y de las implementaciones escritas a mano, que reciben un
  `Writer`, o un `Reader` y el primer evento de su valor.
- `readerFrom` lee un stream de `std.io` a medida que el documento lo necesita, y
  `writerTo` le entrega la salida a medida que crece; `flush` termina el
  documento y devuelve los fallos de escritura. Solo la parte en curso del
  documento está en memoria, salvo con `canonical`, que ordena las claves al
  final. `decodeFrom` y `encodeTo` son `decode` y `encode` sobre ellos.

Un `JsonError` da su clase, un mensaje, el camino del valor (`$.users[2].age`)
y la línea y columna; su `Display` es
`expected an integer, found a string at $.users[2].age (line 3, column 10)`. No
copia el documento ni el valor de un campo.

### 14.11 `std.messagepack`

`std.messagepack` implementa la
[especificación MessagePack](https://github.com/msgpack/msgpack/blob/master/spec.md)
binaria completa: nil, booleanos, enteros signed/unsigned, floats, strings
UTF-8, binary, arrays, maps y extension values. Un objeto `str` con bytes UTF-8
inválidos produce error; `bin` conserva cualquier secuencia de bytes.

La ruta tipada comparte `Encode`/`Decode` y escribe directamente al
buffer o writer. `Value` representa uso dinámico. Como una key
MessagePack puede ser cualquier valor, un map dinámico conserva sus pares como
secuencia ordenada en lugar de imponer artificialmente `Map[String, Value]`;
así también puede detectar duplicados según la política del caller.

El encoder usa siempre la representación válida más corta para enteros y
longitudes. Conserva orden de entrada en modo ordinario. La operación canónica
define además, de forma propia de Tondo:

- `float32` solo cuando conserva exactamente el valor y sus signos relevantes;
  en otro caso usa `float64`;
- un único bit pattern quiet NaN, manteniendo distintos `-0.0` y `0.0`;
- maps ordenados lexicográficamente por los bytes del encoding canónico de cada
  key; y
- rechazo de dos keys con el mismo encoding canónico, por lo que no existe un
  desempate dependiente del layout.

Así, dos layouts internos no alteran el resultado. Esta operación no se presenta
como parte de la especificación MessagePack ni como compatible con un modo
canónico externo sin nombre.

Extension values conservan type code y payload exactos. La extensión timestamp
estándar puede convertirse explícitamente a un `MessagePackTimestamp` de
segundos Unix y nanosegundos; no adelanta calendario civil ni se convierte en
`Instant`. Una extensión desconocida no se pierde ni se interpreta por
reflection.

La API fuente única de 0.1 es:

~~~tondo pseudocode
pub fn parse(input: Array[Byte], options: MessagePackDecodeOptions): Value ! MessagePackError
pub fn parseView(input: Array[Byte], options: MessagePackDecodeOptions): ValueView ! MessagePackError
pub fn decode[T: Decode[MessagePack]](input: Array[Byte], options: MessagePackDecodeOptions): T ! MessagePackError
pub fn encode(value: Value, options: MessagePackEncodeOptions): Array[Byte] ! MessagePackError
pub fn encode[T: Encode[MessagePack]](value: T, options: MessagePackEncodeOptions): Array[Byte] ! MessagePackError
pub fn validate(input: Array[Byte], options: MessagePackDecodeOptions): Unit ! MessagePackError
pub fn encodeDeterministic(value: Value, limits: MessagePackLimits): Array[Byte] ! MessagePackError
pub fn raw(input: Array[Byte], options: MessagePackDecodeOptions): Raw ! MessagePackError
pub unsafe fn rawUnchecked(input: Array[Byte]): Raw
pub fn MessagePackReader.next(var self): MessagePackEvent? ! MessagePackError
pub fn MessagePackReader.own(var self, event: MessagePackEvent): MessagePackEvent ! MessagePackError
pub fn MessagePackWriter.write(var self, event: MessagePackEvent): Unit ! MessagePackError
pub fn MessagePackWriter.finish(var self): Unit ! MessagePackError
~~~

El catálogo completo de `Value`, ext/timestamp, policies, limits,
paths y errores está en el [contrato fuente de `std.messagepack`](./docs/contracts/stdlib-messagepack.md).

La implementación portable de `std.messagepack` reside en
`crates/tondo-stdlib/src/messagepack_api.rs`: `encode_static`/`decode_static`
son la única frontera del ABI `Encode[MessagePack]`/`Decode[MessagePack]` y
`encode_typed`/`decode_typed` quedan limitados al bridge Rust. El owner mantiene
las alias `Value`, `ValueView` y `Raw` y no publica una segunda superficie por
formato.

La evidencia ejecutable de `std.messagepack` es
`STD-A-MSGPACK-EVIDENCE-001`. Cubre el modelo wire completo, formas no
mínimas, enteros signed/unsigned, bits de floats, UTF-8 frente a binary,
claves arbitrarias, ext/timestamp, policies de duplicados, streaming,
determinismo y límites finitos. La suite verifica que el chunking de un byte no
altera eventos, preserva extensiones desconocidas y compara en ambas
direcciones con `rmpv`. `HOST` es `not-applicable`: el bridge del compilador no
añade capabilities ni semántica de wire dependiente del target.

`STD-A-FUZZ-001` remains partial; the exact component scope is recorded in
`testing/stdlib-fuzz.json`. Performance and public conformance remain separate.

La evidencia ejecutable de `std.protobuf` es `STD-A-PROTOBUF-EVIDENCE-001`.
Combina el wire portable con la frontera build-only schema-first: TOML
declarativo, grafo cerrado de imports, proto3, presencia, repeated/packed,
maps, oneof, enums abiertos, unknown fields/grupos, evolución segura e
incompatible, descriptor raíz, reader/writer schema-bound, determinismo y
límites finitos. La suite comprueba fragmentación de un byte y compatibilidad
bidireccional con `prost`, además de que el generator produzca identidades
estables y errores de schema sin inputs ambientales. `HOST` es
`not-applicable`.

`STD-A-FUZZ-001` remains partial; the exact component scope is recorded in
`testing/stdlib-fuzz.json`. Performance and public conformance remain separate.

La evidencia ejecutable de `std.testing` es `STD-A-TESTING-EVIDENCE-001`.
El módulo es test-only y su bridge `HOST` verificado pertenece al worker del
runner: no concede capabilities ni puede importarse desde producción. La
evidencia enlaza las 25 firmas públicas con assertions de ownership prestado,
diffs acotados, tolerancias finitas, consumo explícito de `Option`/`Result`,
temporales afines bajo raíz privada, generators replayables, shrinking sellado,
control terminal, virtual time y los fixtures de dogfooding del runner.

`STD-A-FUZZ-001` remains partial; the exact component scope is recorded in
`testing/stdlib-fuzz.json`. Performance and public conformance remain separate.

`testing/stdlib-test-coordination.json` links 22 owners, 301 public signatures
and 171 requirements to 66 model laws and their test/campaign references. The
generator preserves each signature's public implementation status and missing
evidence separately from model coverage. A verified model law does not prove
execution of that API from Tondo. The audit reopened `STD-A-FUZZ-001` to verify
the actual dispatch and oracle of each owner campaign; recorded references do
not close that task or global conformance.

The current owner fuzz audit records eight constant frontend checks, three
Rust-reference routes, nine bounded kernel-invariant routes and two compiler
models. All 22 whole-owner FUZZ cells remain partial; the nine component
assertions are retained separately. `STD-TEST-001` remains
open until its declared laws and commands establish the required public
coverage. `stdlib-owner-evidence-generate.sh` synchronizes FUZZ and CONF from
their source contracts without promoting declared cases into observations.

La coordinación `STD-CONF-001` queda registrada en
`testing/stdlib-conformance-coordination.json`: contiene los 22 owners de
`STD-0.1A` y una fila `CONF` explícita para cada firma o requisito de la matriz
(301 firmas y 171 requisitos). Cada fila conserva el estado actual de la
matriz, una razón obligatoria para `partial`/`pending`, referencias
reproducibles y comandos. El registro cruza la matriz normativa, la auditoría
de API, la evidencia de owners, la coordinación de modelos y el harness
externo de codecs; no permite declarar `verified` sin la observación de la
fila ni convierte la coordinación en promoción.

The public meta/reflection boundary additionally has an explicit row plan in
`testing/stdlib-meta-reflect-conformance.json`: 25 ordinary companion callables,
27 hosted descriptor callables and six requirements per owner. Its checker
requires current draft execution observations for every referenced test, with
exact source, inventory and manifest identities. `--plan` validates declarations
only. This scoped conformance does not change the separate FUZZ or performance
cells and cannot promote the other standard-library owners or the S1A seal.

`STD-DOC-001` is recorded in `testing/stdlib-documentation.json`. Each owner
has normative contracts and separate kernel, bridge and public API boundaries.
The API field describes static callable traceability; missing signatures keep
an owner partial, including intrinsic and build-only surfaces. The current
record links 34 examples: 28 runtime cases with exact sidecars, four external
codec cases and two compiler cases. Reflection executes through ordinary
Tondo runtime examples. The meta companion executes provider programs during
compilation and has no runtime host adapter. Documentation coverage describes
the unpublished draft; it does not promote implementation, test/fuzz,
performance or conformance.

### 14.12 `std.protobuf`

[Protobuf](https://protobuf.dev/programming-guides/encoding/) es schema-first.
Un `.proto` entra al build como generator input y el programa estándar fijado
produce tipos, codecs y metadata de schema ordinarios. La versión inicial
soporta proto3, incluido `optional`, repeated, packed fields, maps,
[enums abiertos](https://protobuf.dev/programming-guides/enum/), nested messages
y `oneof`. Services y gRPC se difieren porque requieren contratos de networking
separados.

El mapping de package y module es explícito en la declaración del generador. Dos
schemas no pueden generar el mismo path, tipo o field identity. Los field
numbers proceden solo del schema; nunca del orden de un record Tondo, un hash o
reflection.

Los tipos generados:

- distinguen presencia cuando el schema la distingue;
- validan wire type, varints, longitudes, recursion y límites;
- preservan unknown fields al decodificar y re-encodear salvo descarte
  explícito;
- representan `oneof` mediante un enum cerrado;
- representan cada enum proto3 abierto mediante un tipo nominal que conserva
  siempre su `Int32` wire, operaciones asociadas para los valores conocidos y
  una proyección `known(): KnownEnum?` hacia un enum cerrado generado; un número
  desconocido no se convierte en sentinel ni pierde su payload; y
- no exponen almacenamiento wire mutable.

El encoder ordinario produce bytes válidos y puede elegir un orden permitido.
`encodeDeterministic` ordena fields por número y maps por la regla publicada
para obtener estabilidad con el mismo schema, versión estándar y valor. Esa
garantía no afirma una canonicalización universal de Protobuf ni convierte los
bytes en identidad duradera entre toolchains distintos.

La evolución se comprueba como tarea de build: reutilizar field numbers,
cambiar wire-incompatible types o romper una reserva produce diagnostics
asociados al schema. El runtime no descubre schemas, carga descriptors
ambientales ni genera código en la primera petición.

La declaración de build canónica usa `tondo.toml`:

~~~toml
[protobuf]
version = 1
[[protobuf.schema]]
path = "proto/user.proto"
module = "app.proto.user"
package = "acme.user"
baseline = "proto/baseline/user.proto"
descriptor = "none"
~~~

La API runtime única es:

~~~tondo pseudocode
pub fn decode[T: Decode[Protobuf]](input: Array[Byte], options: ProtoDecodeOptions): T ! ProtoError
pub fn encode[T: Encode[Protobuf]](value: T, options: ProtoEncodeOptions): Array[Byte] ! ProtoError
pub fn encodeDeterministic[T: Encode[Protobuf]](value: T, limits: ProtoLimits): Array[Byte] ! ProtoError
pub fn validate[T](input: Array[Byte], options: ProtoDecodeOptions): Unit ! ProtoError
pub fn descriptor[T](): ProtoDescriptor[T]
pub fn ProtoReader[T].next(var self): ProtoEvent? ! ProtoError
pub fn ProtoReader[T].own(var self, event: ProtoEvent): ProtoEvent ! ProtoError
pub fn ProtoWriter[T].write(var self, event: ProtoEvent): Unit ! ProtoError
pub fn ProtoWriter[T].finish(var self): Unit ! ProtoError
~~~

La implementación portable vive en `crates/tondo-stdlib/src/protobuf_api.rs`:
el checker schema-first, el parser/reader y el writer usan límites y stacks
explícitos, y `encode_static`/`decode_static` atraviesan
`Encode[Protobuf]`/`Decode[Protobuf]` sin materializar `serialization.Value`.
Los helpers Rust `encode`/`decode`, `ProtoValue` y `Raw<Protobuf>` son detalles
internos de implementación y no forman una segunda API Tondo ni introducen un
alias dinámico de `serialization.Value`. La conformance oficial
e interoperabilidad independiente están cerradas por
`STD-CODEC-CONF-001`, cuyo registro coordina `serde_json`, `rmpv` y `prost`,
fragmentación, truncación, límites y preservación de unknown fields.

El mapping generado, el descriptor explícito, la evolución contra baseline TOML,
los eventos y los errores de wire/build están cerrados en el [contrato fuente de
`std.protobuf`](./docs/contracts/stdlib-protobuf.md).

### 14.13 `std.yaml`

`std.yaml` es el owner de un subset seguro y portable de YAML 1.2. No intenta
reproducir los dialectos históricos de YAML ni convierte el parser en una
superficie de ejecución: el schema implícito es Core, las keys dinámicas son
textuales, los tags admitidos son únicamente los tipos core cerrados y no se
aceptan custom tags, includes, merge keys, timestamps implícitos, mappings con
keys no textuales o valores no finitos.

La API expone `YamlValue`/`YamlValueView`, `YamlReader`/`YamlWriter`,
`parse`/`parseAll`, `decode`/`decodeAll`, `validate`, `encode` y
`encodeCanonical`. La ruta tipada usa `Encode[Yaml]`/`Decode[Yaml]` sin
materializar el árbol dinámico; la ruta dinámica usa `Object(Map[String,
YamlValue])` y expande anchors como copias lógicas. Los aliases se permiten solo
con límites explícitos de cantidad y expansión, no pueden formar ciclos ni
cruzar documentos, y `<<` no tiene semántica implícita.

Los readers y writers comparten una máquina de eventos con frames explícitos,
document boundaries, anchors y aliases. `YamlError` conserva kind, byte offset,
línea, columna y path `Key`/`Index`; ningún límite publica un valor parcial. La
entrada es UTF-8, la indentación usa espacios, `true`/`false` son los únicos
booleanos implícitos y fechas/timestamps permanecen texto. El owner no requiere
capability y no consulta el entorno.

El contrato machine-readable, la documentación operativa y sus checks negativos
son [`testing/stdlib-yaml.json`](./testing/stdlib-yaml.json),
[`docs/contracts/stdlib-yaml.md`](./docs/contracts/stdlib-yaml.md),
[`scripts/stdlib-yaml-check.sh`](./scripts/stdlib-yaml-check.sh) y
[`scripts/stdlib-yaml-test.sh`](./scripts/stdlib-yaml-test.sh). El contrato de
fiabilidad es [`testing/stdlib-yaml-test.json`](./testing/stdlib-yaml-test.json),
con documento [`docs/contracts/stdlib-yaml-test.md`](./docs/contracts/stdlib-yaml-test.md),
checks [`scripts/stdlib-yaml-test-check.sh`](./scripts/stdlib-yaml-test-check.sh),
regresiones [`scripts/stdlib-yaml-test-test.sh`](./scripts/stdlib-yaml-test-test.sh)
y fuzz [`scripts/stdlib-yaml-fuzz.sh`](./scripts/stdlib-yaml-fuzz.sh). El contrato
queda cerrado como diseño B0. `STD-YAML-IMPL-001` verifica la ruta scalar y el
bridge VM hosted buffered para parseo/encoding dinámico y tipado, límites,
errores y ciclo de vida de reader/writer; sus artefactos son
[`scripts/stdlib-yaml-implementation-check.sh`](./scripts/stdlib-yaml-implementation-check.sh),
[`scripts/stdlib-yaml-implementation-test.sh`](./scripts/stdlib-yaml-implementation-test.sh),
[`scripts/stdlib-yaml-implementation.sh`](./scripts/stdlib-yaml-implementation.sh),
la fixture de VM y el informe
`target/reliability/evidence/stdlib-yaml-implementation.json`. El cierre no
promociona una API pública, no afirma runtime nativo ni lowering AOT
(`native_aot_lowering: not-claimed`). `STD-YAML-TEST-001` cierra el modelo
independiente, las regresiones scalar/hosted, los límites y el fuzz acotado.
El contrato de rendimiento hosted
[`testing/stdlib-yaml-performance.json`](./testing/stdlib-yaml-performance.json)
queda cerrado por `STD-YAML-PERF-001`: la campaña observa 13 workloads y 27
muestras por workload con latencia, tail, throughput, allocations, memoria
lógica, bytes copiados, profundidad, aliases, expansión, rechazo adversarial y
cleanup de handles. El baseline se limita a `tondo-vm-hosted` /
`bytecode-vm`; runtime nativo, SIMD y lowering AOT siguen sin reclamar.
La conformance VM/native target-qualified está cerrada por
[`testing/stdlib-yaml-conformance.json`](./testing/stdlib-yaml-conformance.json) y
[`docs/contracts/stdlib-yaml-conformance.md`](./docs/contracts/stdlib-yaml-conformance.md):
se comparan seis casos en el VM hosted y en un proceso nativo que reutiliza el
scalar stdlib, sin ABI YAML nativo ni lowering AOT. `native_aot_lowering` sigue
`not-claimed` y no hay ruta SIMD medida.

`STD-YAML-DOC-001` cierra la guía ejecutable de uso en
[`docs/contracts/stdlib-yaml.md`](./docs/contracts/stdlib-yaml.md). La ficha
[`testing/stdlib-yaml.json`](./testing/stdlib-yaml.json), la fixture
`tests/runtime/m11-std-yaml-doc-001.to` y los runners
[`scripts/stdlib-yaml-doc-check.sh`](./scripts/stdlib-yaml-doc-check.sh) /
[`scripts/stdlib-yaml-doc-test.sh`](./scripts/stdlib-yaml-doc-test.sh)
verifican el subset seguro YAML 1.2 Core, la selección explícita de policies,
los límites finitos, costes, ownership, errores y ejemplos materializados y
streaming. La fixture usa únicamente rutas CLI hosted verificadas; la firma
`YamlWriter.toWriter` permanece como contrato estático y cobertura
directa del host hasta que el dispatcher async registre esa operación
(`writer-boundary: static-contract-only-until-async-dispatch`,
`native_aot_lowering: not-claimed`,
`simd: not-measured-no-optimized-route`). El siguiente bloque del owner es
`STD-TOML-TEST-001`.

### 14.14 `std.toml`

`std.toml` es el codec de datos de TOML 1.1.0 y no el parser del toolchain.
Acepta una única raíz TOML UTF-8, claves bare/quoted/dotted, los cuatro tipos
de string, números con bases/underscores, floats finitos y no finitos,
booleanos, fecha/hora civil, arrays heterogéneos, inline tables cerradas,
tables y arrays-of-tables. Los comentarios se descartan y no hay includes,
interpolación de environment, locale, TZ ni ejecución.

La API expone `TomlValue`/`TomlValueView`, `TomlReader`/`TomlWriter`,
`parse`/`parseView`, `decode`, `validate`, `encode` y `encodeCanonical`. Las
fechas usan `std.time`; un offset fijo se conserva en `TomlOffsetDateTime` y
no se convierte implícitamente en zona horaria. La precisión se limita a
nanosegundos: fracciones superiores a nueve dígitos producen un error para no
perder datos. Tables, dotted keys y arrays-of-tables se resuelven con paths
explícitos; duplicados, colisiones scalar/table y extensión de inline tables
se rechazan antes de publicar un valor.

El reader/writer usa eventos con frames y worklists acotados. Los spans son
half-open en bytes UTF-8, con línea/columna y path `Key`/`Index`; el parser es
invariante al chunking y no publica resultados parciales. Solo la frontera de
`std.io.Reader`/`Writer` espera y no existe una API duplicada.

El kernel Rust de tondo-stdlib queda verificado por STD-TOML-IMPL-001 con
TomlValue/TomlValueView, parser con spans, fechas civiles de nanosegundo,
duplicados y tablas/AOT, encoder canónico, eventos y reader/writer atómicos.
La identidad serialization::Toml se registra sin añadir un intrinsic del
compilador. El estado es verified-stdlib-kernel; host/compiler,
native_aot_lowering: not-claimed y public_api_promoted: false. No se reclama
ejecución hosted ni runtime nativo hasta disponer del ABI TOML correspondiente.
STD-TOML-TEST-001 queda verificado por el modelo canónico independiente, la
matriz hosted, el corpus negativo, el reader common con fragmentos de un byte y
el fuzz bounded. La evidencia está en
[testing/stdlib-toml-test.json](./testing/stdlib-toml-test.json) y
[docs/contracts/stdlib-toml-test.md](./docs/contracts/stdlib-toml-test.md);
no promueve compiler/VM/AOT ni claims de rendimiento. La campaña posterior
STD-TOML-PERF-001 fija un baseline reproducible del kernel Rust en
`x86_64-unknown-linux-gnu` cubre trece cargas con 27 muestras cada una,
parse/view, encode normal/canónico, eventos y rechazos acotados. El contrato
[`testing/stdlib-toml-performance.json`](./testing/stdlib-toml-performance.json)
y la guía [`docs/contracts/stdlib-toml-performance.md`](./docs/contracts/stdlib-toml-performance.md)
definen latencia, throughput y contadores lógicos. La medición no promociona
un bridge VM, ABI nativo, SIMD ni AOT; `parseView` todavía materializa y
descarta el valor durante la validación. `STD-TOML-CONF-001` ejecuta seis casos
con bytes compartidos mediante un callable host privado en el bytecode VM y un
proceso nativo separado que llama al mismo kernel Rust. Comprueba typed/dynamic,
TOML 1.1, orden canónico, streaming fragmentado, path/span y rechazo atómico;
el contrato está en
[`testing/stdlib-toml-conformance.json`](./testing/stdlib-toml-conformance.json)
y el límite en
[`docs/contracts/stdlib-toml-conformance.md`](./docs/contracts/stdlib-toml-conformance.md).
El callable es solo de prueba: todavía no existe una API `std.toml` ejecutable
desde Tondo, registro hosted de producción, ABI TOML nativo ni lowering AOT.
`STD-TOML-DOC-001` closes an executable usage guide for the Rust stdlib kernel
in [`docs/contracts/stdlib-toml.md`](./docs/contracts/stdlib-toml.md), backed
by `crates/tondo-stdlib/examples/toml_usage.rs` and
`scripts/stdlib-toml-doc-check.sh`. It separates application TOML from the
toolchain's `tondo.toml`, fixes explicit limits, ownership, error and
buffering costs, and shows materialized, typed, view and event usage. The
example does not claim a source-level Tondo API. The following CBOR owner
section records its verified kernel and remaining owner gates.

El contrato machine-readable, la documentación y los checks negativos son
[testing/stdlib-toml.json](./testing/stdlib-toml.json),
[docs/contracts/stdlib-toml.md](./docs/contracts/stdlib-toml.md),
[testing/stdlib-toml-test.json](./testing/stdlib-toml-test.json),
[docs/contracts/stdlib-toml-test.md](./docs/contracts/stdlib-toml-test.md),
[testing/stdlib-toml-conformance.json](./testing/stdlib-toml-conformance.json),
[docs/contracts/stdlib-toml-conformance.md](./docs/contracts/stdlib-toml-conformance.md),
[scripts/stdlib-toml-check.sh](./scripts/stdlib-toml-check.sh) y
[scripts/stdlib-toml-test.sh](./scripts/stdlib-toml-test.sh). El contrato
queda cerrado como diseño B0 y la conformance acotada está verificada. La
guía de uso del kernel queda cerrada por `STD-TOML-DOC-001`; la API pública
compiler/host y el ABI/AOT nativo siguen fuera de esta promoción.

### 14.15 `std.cbor`

`std.cbor` implementa el modelo de datos de CBOR de RFC 8949. Es un codec
binario general y no un protocolo de aplicación, un compilador CDDL ni una
facade de MessagePack. El documento de la API fija un único data item por
entrada materializada: trailing data se rechaza, aunque un transporte externo
pueda concatenar items con su propio framing.

El wire model acepta los major types 0 a 7: `UInt`, negativos representados
como magnitud `-1 - n`, byte strings, text strings UTF-8, arrays, maps, tags y
simples/floats. Se aceptan longitudes definidas e indefinidas para bytes,
texto, arrays y maps; el break solo cierra el frame indefinido actual. Los
valores `null`, `undefined`, `true` y `false` son variantes distintas, los
simples no asignados se conservan y `Float16` conserva sus bits mediante
`CborFloat16`. Un `CborTag` conserva el número `UInt64` y el item anidado; no
hay resolución implícita de tags de tiempo, bignum, decimal, contenido CBOR o
tags privados.

La ruta dinámica usa `CborValue` con `Array[CborEntry]` para maps, de forma que
las claves pueden ser cualquier data item y el orden/duplicados son
observables. A standalone `CborValueView` borrows validated encoded input;
its owner must outlive the view. This lifetime is independent of a Reader
cursor. The specified public Reader payloads borrow until its next event;
the current Rust kernel instead returns owned events.
`CborRaw` conserva bytes exactos, incluida la forma de longitud y el float
original. `parse`, `parseView`, `validate`, `decode`, `encode`,
`encodeDeterministic` y `raw` comparten el contrato de
`std.serialization`; typed encode/decode se generan en compile time sin DOM
dinámico intermedio.

`CborReader` y `CborWriter` exponen eventos para tags, simples, floats,
containers y chunks indefinidos (`StartBytes`/`ByteChunk`/`EndBytes` y sus
equivalentes de texto). Usan frames/worklists explícitos, son invariantes al
fragmentar la entrada y quedan terminales tras `finish`, EOF inválido, límite
o error de I/O. Solo las fronteras `std.io.Reader`/`Writer` esperan; no hay
API async duplicada.

El modo ordinario acepta formas no mínimas y conserva bits de NaN. El modo
`encodeDeterministic` aplica los requisitos core de RFC 8949: additional
information y floats en la forma preferida más corta, longitudes definidas,
NaN quieto binario-16 estable, arrays/tags en orden semántico y claves de map
ordenadas por los bytes de su encoding determinista. Una colisión o una clave
fuera de orden se rechaza; la API no presenta esta policy como una
canonicalización universal de todos los protocolos CBOR.

Los límites cubren bytes de documento/output, profundidad, items de arrays,
pares de maps, bytes de texto/bytes, chunks, tags, simples y eventos. Los
errores incluyen `CborErrorKind`, span half-open de bytes y path estable de
`ArrayIndex`/`MapEntry`/`MapKey`/`MapValue`/`Tag`; nunca se publica un valor
parcial ni se consume la pila recursiva del host. No hay includes, environment,
locale, timezone lookup, reflection registry ni schema discovery.

El contrato machine-readable, la documentación y los negativos ejecutables
son [`testing/stdlib-cbor.json`](./testing/stdlib-cbor.json),
[`docs/contracts/stdlib-cbor.md`](./docs/contracts/stdlib-cbor.md),
[`scripts/stdlib-cbor-check.sh`](./scripts/stdlib-cbor-check.sh) y
[`scripts/stdlib-cbor-test.sh`](./scripts/stdlib-cbor-test.sh). El diseño B0
queda cerrado por `STD-CBOR-001`. `STD-CBOR-IMPL-001` has a bounded Rust scalar
kernel for dynamic values, static primitive/collection codecs, event
reader/writer, and deterministic encoding. The task closes at that bounded
kernel boundary: focused checks, the full functional gate and provenance-bound
coverage/mutation checks pass. Every global and risk-scope coverage dimension
meets the unchanged 80% floor, and all six selected critical mutants are caught.
Public compiler API, production VM host registration, native ABI and native AOT
lowering remain unclaimed. `STD-CBOR-TEST-001` verifies the independent bounded
model, retained corpus, Rust-kernel regressions and bounded fuzz. Performance
has its bounded Rust scalar baseline, and `STD-CBOR-CONF-001` verifies the
private VM/native Rust process comparison. `STD-CBOR-DOC-001` verifies
executable Rust-kernel usage after the full functional and quality gates.

The independent bounded model and Rust-kernel test/fuzz boundary are registered
in [testing/stdlib-cbor-test.json](./testing/stdlib-cbor-test.json) and defined
in [docs/contracts/stdlib-cbor-test.md](./docs/contracts/stdlib-cbor-test.md).
Their evidence does not promote public compiler, production VM, ABI or AOT
integration. Every two-byte simple encoding with a value below 32 is malformed
under RFC 8949 section 3.3, regardless of the non-minimal decode policy.

The bounded Rust scalar performance protocol is registered in
[testing/stdlib-cbor-performance.json](./testing/stdlib-cbor-performance.json)
and defined in
[docs/contracts/stdlib-cbor-performance.md](./docs/contracts/stdlib-cbor-performance.md).
`STD-CBOR-PERF-001` verifies fifteen target-qualified kernel routes on
`x86_64-unknown-linux-gnu`, using three independent processes and 27 retained
samples per workload. Exact independent oracles, errors and terminal stream
states are checked before timing. Latency and throughput are measured; copies,
allocations and memory describe the explicitly bounded logical resource model.
The register is `verified-stdlib-kernel-baseline` after clean-source capture,
the full functional gate and current quality checks. The unoptimized Rust
test profile does not promote the public Tondo API, production VM bridge,
native ABI/AOT, SIMD or conformance.

The bounded CBOR VM/native-process conformance protocol is registered in
[testing/stdlib-cbor-conformance.json](./testing/stdlib-cbor-conformance.json)
and defined in
[docs/contracts/stdlib-cbor-conformance.md](./docs/contracts/stdlib-cbor-conformance.md).
Its seven shared cases use 61 valid and 30 invalid retained wire vectors,
the independent bounded model, a private verified-bytecode host callable and
a native Rust kernel process. The register is
`verified-hosted-vm-adapter-and-native-stdlib-process` after clean-source exact
comparison, report/owner negatives and the full functional/quality gates.
It verifies typed/dynamic round trips, raw/views, 270 events under fragments
of one, two and seven bytes, exact error spans/paths, atomic limits, terminal
streams, 59 deterministic outputs and two rejected key collisions. The
conformance campaign measures 91.5681% global lines, every coverage dimension
meets the unchanged 80% floor, and all six critical mutants are caught.
This does not promote source-level `std.cbor`, production host registration,
native CBOR ABI or Cranelift lowering.

The [executable Rust-kernel guide](./docs/contracts/stdlib-cbor.md#executable-usage-guide-for-stdcbor)
records tags, preservation and deterministic policy, exact errors, finite
limits, borrowed-input lifetimes, owned buffered events, terminal states and
retention costs. Six paths in
[cbor_usage.rs](./crates/tondo-stdlib/examples/cbor_usage.rs) run through
`scripts/stdlib-cbor-doc-check.sh` with successful stdout `cbor-doc-ok`.
Codec errors never publish a successful partial result; external I/O can
irreversibly accept a prefix or fail to flush, as the example demonstrates.
This `verified-rust-kernel-usage` documentation does not claim public Tondo execution,
production VM registration, native CBOR ABI, native AOT or SIMD.

### 14.16 `std.regex`

`std.regex` es el motor de expresiones regulares Unicode de Tondo, no un
dialecto Perl, un parser de glob ni una API de matching sobre bytes arbitrarios.
Opera sobre `String` UTF-8 y fija las tablas Unicode en 16.0.0, la misma versión
que usa la especificación del lenguaje. No normaliza, no consulta locale y no
trata grapheme clusters como una unidad de matching.

La sintaxis es un subconjunto cerrado y regular: literales, clases/rangos,
propiedades `\p`/`\P`, escapes, anchors, alternation, grupos capturantes y no
capturantes, captures con nombre y quantifiers greedy/lazy acotados. `\d`,
`\s`, `\w` y `\b` tienen definiciones Unicode explícitas. Backreferences,
look-around, recursion, conditionals, código embebido, includes, clases
dependientes de locale, flags embebidos y matching de bytes inválidos se
rechazan con un error de compilación; no se reinterpretan silenciosamente.

`Regex.compile` produce un valor inmutable y reutilizable, que se copia sin
coste y puede compartirse entre tareas. `isMatch`, `isFullMatch`, `find`,
`findAll`, `replace` y `replaceAll` son síncronos y acotados; ninguno suspende.
`find` devuelve la primera coincidencia (`match` es una palabra reservada).
`findAll` implementa de forma perezosa `Iterator[RegexMatch ! RegexError]`:
emite como mucho un error y después queda agotado; las coincidencias anteriores
siguen siendo válidas. La selección usa el inicio más a la izquierda, las
alternativas en el orden del patrón y la prioridad greedy/lazy local de cada
quantifier. Las coincidencias no se solapan; tras una coincidencia vacía la
búsqueda avanza un escalar Unicode, y una coincidencia vacía donde terminó la
anterior se omite, igual en `findAll` que en `replaceAll`. Captures y spans usan
offsets UTF-8 semiabiertos en fronteras de escalar. Una capture cuya rama no
participó es `none`; la capture cero es la coincidencia completa.

El replacement solo admite `$0`, `$N`, `${name}` y `$$`; cualquier otro `$`, o
una capture que el patrón no tiene, es `InvalidReplacement` en su offset.
`replace` modifica la primera coincidencia y `replaceAll` todas las no
solapadas. No existen callbacks de replacement en 0.1: evitar código ejecutable
por match mantiene visibles coste, ownership y errores.

```tondo
pub fn RegexOptions.defaults(): RegexOptions
pub fn RegexLimits.defaults(): RegexLimits
pub fn Regex.compile(pattern: String, options: RegexOptions = …, limits: RegexLimits = …): Regex ! RegexError
pub fn Regex.pattern(self): String
pub fn Regex.captureCount(self): Int
pub fn Regex.captureNames(self): Array[String]
pub fn Regex.isMatch(self, input: String): Bool ! RegexError
pub fn Regex.isFullMatch(self, input: String): Bool ! RegexError
pub fn Regex.find(self, input: String): RegexMatch? ! RegexError
pub fn Regex.findAll(self, input: String): RegexFindIterator ! RegexError
pub fn Regex.replace(self, input: String, replacement: String): String ! RegexError
pub fn Regex.replaceAll(self, input: String, replacement: String): String ! RegexError
pub fn RegexMatch.capture(self, index: Int): RegexCapture?
pub fn RegexMatch.captureName(self, name: String): RegexCapture?
pub fn RegexSpan.slice(self, input: String): String ! RegexError
```

`RegexMatch` y `RegexCapture` tienen `span` y `text`. `RegexOptions` tiene
`caseInsensitive` (simple case folding de Unicode), `multiLine`,
`dotMatchesNewline`, `crlf` (con `multiLine`, `CRLF` es un solo fin de línea) y
`ungreedy`, todos `false` en `defaults()` y en el valor por defecto de
`compile`; Unicode siempre está activo. `RegexLimits` y sus valores por defecto:

| Límite | Default | Se comprueba |
|---|---:|---|
| `maxPatternBytes` | 65536 | antes de parsear |
| `maxSyntaxDepth` | 256 | al parsear |
| `maxCaptureGroups` | 256 | al parsear |
| `maxProgramStates` | 1000000 | al compilar: el programa ocupa hasta 16 bytes por estado |
| `maxInputBytes` | 16777216 | antes de buscar |
| `maxMatches` | 1000000 | antes de emitir cada coincidencia |
| `maxOutputBytes` | 67108864 | al construir el reemplazo |
| `maxReplacementBytes` | 65536 | antes de leer la plantilla |

Si se alcanza un límite no se publica un valor parcial. `RegexError` tiene
`kind`, `phase` (`Compile`, `Match` o `Replace`) y `offset`: el byte del patrón,
input o plantilla donde está el error, o donde se superó el límite. Se muestra
como `regex.<kind> at byte <offset> (<phase>)`. `RegexErrorKind` es
`InvalidSyntax`, `UnexpectedEnd`, `InvalidEscape`, `InvalidUnicodeScalar`,
`InvalidUnicodeProperty`, `InvalidClass`, `InvalidRange`, `InvalidQuantifier`,
`InvalidCaptureName`, `DuplicateCaptureName`, `UnsupportedFeature`,
`PatternLimitExceeded`, `ProgramLimitExceeded`, `InputLimitExceeded`,
`MatchLimitExceeded`, `ReplacementLimitExceeded`, `InvalidReplacement`,
`OutputLimitExceeded` o `InvalidBoundary`. La API no hace panic por input del
usuario.

El motor es el crate `regex` de Rust, enlazado en el runtime: autómatas finitos
(PikeVM, DFA perezoso y un backtracker acotado con memoria de estados
visitados), con tiempo O(m·n) en el tamaño del programa y del input, sin
backtracking exponencial ni recursión en la pila del host. Antes de compilar,
`regex-syntax` valida el dialecto y da el tipo y el offset de cada error. Las
tablas Unicode son las de `regex-syntax` (16.0.0), fijadas por `Cargo.lock`.

### 14.17 `std.uuid`

`std.uuid` da los identificadores de RFC 9562: dieciséis bytes que se escriben
`8-4-4-4-12` en hexadecimal minúscula.

~~~tondo pseudocode
pub type Uuid
pub enum UuidError { InvalidText, WrongLength }

pub fn Uuid.v4(): Uuid
pub fn Uuid.v5(namespace: Uuid, name: String): Uuid
pub fn Uuid.v7(): Uuid
pub fn Uuid.nil(): Uuid
pub fn Uuid.namespaceDns(): Uuid
pub fn Uuid.namespaceUrl(): Uuid
pub fn Uuid.namespaceOid(): Uuid
pub fn Uuid.namespaceX500(): Uuid
pub fn Uuid.parse(text: String): Uuid ! UuidError
pub fn Uuid.fromBytes(bytes: Array[Byte]): Uuid ! UuidError
pub fn Uuid.bytes(self): Array[Byte]
pub fn Uuid.version(self): Int
pub fn Uuid.compare(self, other: Uuid): Int
~~~

- `Uuid` es `Copy`, `Equatable` y `Key`, implementa `Ordered` por sus bytes como
  números sin signo, en orden de red, y `Display` lo escribe en minúsculas con
  guiones.
- `v4` toma 122 bits de la entropía del sistema. `v5` es SHA-1 sobre los bytes
  del namespace seguidos del nombre: el mismo par da siempre el mismo UUID, que
  no es secreto. `v7` pone los milisegundos Unix UTC en los 48 bits altos y 74
  bits de entropía detrás, así que ordena por creación entre milisegundos
  distintos; no guarda contadores ni estado.
- `parse` acepta `8-4-4-4-12` en hexadecimal de cualquier caja, con o sin
  `urn:uuid:` delante; otra forma es `UuidError.InvalidText`. `fromBytes` exige
  dieciséis bytes. Ambos conservan UUIDs de cualquier versión o variante; los
  generadores solo producen la variante de RFC 9562.
- Un provider o generador no puede importar `std.uuid` ni `std.random` (27.7
  del lenguaje): sus valores dependen del momento y de la máquina.

### 14.18 `std.log`

`std.log` separa la construcción pura de un evento de su entrega a un sink.
`LogLevel` es el orden cerrado `Trace < Debug < Info < Warn < Error`; no existe
`Fatal`, porque terminar el programa desde logging ocultaría control de flujo.
`LogEvent` contiene level, target UTF-8 no vacío, mensaje, fields estructurados
y un `DateTime?` opcional. El módulo nunca consulta el reloj por su
cuenta: si el caller quiere timestamp debe obtenerlo explícitamente mediante
`std.time` y pasarlo al evento.

`LogValue` admite `Null`, booleanos, enteros, floats finitos, texto, bytes,
arrays, objects y `Redacted`. `Fields` es un mapa ordenado de claves UTF-8
únicas; duplicados, controles, `NUL`, límites y mutaciones parciales se
rechazan antes de publicar. `Text` escribe una única línea escapada y
`JsonLines` usa el schema `tondo-log-event-0.1/1`, orden raíz fijo y fields en
orden UTF-8; bytes se representan en Base64 RFC 4648 y `Redacted` nunca se
obtiene por heurística.

El protocolo estático `LogSink` ofrece `write`, `flush` y `close`. `Logger` es
un owner explícito de un sink: no existe logger global, fallback implícito a
stderr, worker detached, queue ilimitada o configuración desde environment.
`ConsoleSink` requiere `console`, `FileSink` requiere `filesystem` y un sink
de red implementa el mismo protocolo sobre un writer/transporte explícito con
`network`; DNS, TLS y retry pertenecen a sus owners. `Block` espera capacidad, `Reject` devuelve `LogError.Backpressure` sin consumir el evento y
`Drop` devuelve `LogReceipt.Dropped`. Todo sink es finito y sus llamadas
concurrentes se linearizan por orden de commit; `flush` y `close` conservan
errores, cancelación y cleanup visibles.

The protocol uses a plain `LogSink` trait. `Logger[S: LogSink + Share]`
declares its bounds explicitly.
`LogSink.close(sink: Self)` and `Logger[S].close(logger: Logger[S])` consume
their values. `Logger[S].create` may wait, including for the cleanup of a
sink when construction fails. Builtin sinks are affine; user sinks retain
their own structural capabilities, and the logger owns one logical value.
`LogReceipt.Filtered` identifies an event excluded by the minimum-level
filter without sink or queue admission. Validation and formatting complete
before any write. A later `Io` remains visible, can leave a physical prefix
and makes the sink terminal. Cleanup errors remain visible through terminal
logger close. These guarantees do not require transactional stdout or files.

La superficie no duplica APIs async: `Logger.emit`, `flush` y `close` esperan
como cualquier llamada; no hay
`logAsync`, polling ni `std.log.select`. El core de eventos no requiere host,
las capabilities solo aparecen en los constructores de sinks y la ausencia
produce `E1008`. El oracle scalar recorre valores anidados con frames
explícitos; SIMD y buffers especializados solo se promueven tras equivalencia
de bytes, errores, allocations y límites.

El contrato machine-readable, la documentación normativa y los negativos son
[`testing/stdlib-log.json`](./testing/stdlib-log.json),
[`docs/contracts/stdlib-log.md`](./docs/contracts/stdlib-log.md),
[`scripts/stdlib-log-check.sh`](./scripts/stdlib-log-check.sh) y
[`scripts/stdlib-log-test.sh`](./scripts/stdlib-log-test.sh). El diseño B0
queda cerrado por `STD-LOG-001`. `STD-LOG-IMPL-001` verifies 19 public ordinary
Tondo core operations and the custom `LogSink` protocol, backed by two private
pure construction helpers and bounded scalar formatting. Its
[implementation register](./testing/stdlib-log-implementation.json) and
[contract](./docs/contracts/stdlib-log-implementation.md) record immutable
snapshots, generic affine ownership, backpressure, cancellation, terminal Io
and bounded VM admission. Source-bound quality measures 306,695 of 334,764
lines (91.6153%), preserves every global/risk 80% line/function/region floor
and catches all six selected critical mutants; all 2,879 workspace Rust tests
pass. `STD-LOG-HOST-001` implements public ConsoleSink/FileSink, explicit
bounded record queues, short-write offsets, cancellation, terminal Io and
caller-owned network sinks. Its [host register](./testing/stdlib-log-host.json)
and [contract](./docs/contracts/stdlib-log-host.md) retain 307,658 of 335,744
covered lines (91.634698%), every global/risk 80% dimension, all six critical
mutants caught and 2,898 workspace Rust tests passing. Exact earlier implementation
source `96cd3c62` passes all 364 strict Linux checks in CI run `37897332476`;
normal portable/fuzz skips do not establish global portable success.
The scheduled nightly quality job at `040a93ca` omits the target flag and fails
the final provenance ratchet after its measurements pass. `89130e3f` declares
the target explicitly and its renewed source-bound quality passes. Exact
publication `97eaa29a` passes strict run `37918455600` and all three jobs in
manual nightly run `37918521272`, with checkout, provenance and final quiet
confirmation verified. This qualifies the unchanged nightly workflow.
The command output correction preserves separate streams before runtime
diagnostics and VM resource failure. Wire conformance, reliability observations
and runtime fixtures also retain both streams. The driver/CLI regressions and
all 25 logging tests pass. Fresh source-bound coverage and all six critical
mutants verify the correction at `4e7a5bb0`. Exact publication `1891031a`
passes Linux CI `37937941039` with all 364 named checks, 2,898 Rust tests,
actual checkout and final quiet confirmation verified.
The bounded independent values/queue reference, 34 authored rows, nine kernel
and hosted integrations and maintained fuzz target are recorded in
[testing/stdlib-log-test.json](testing/stdlib-log-test.json) and
[stdlib-log-test.md](docs/contracts/stdlib-log-test.md). Reference-domain
refusal is not Tondo ResourceLimit; finite float rendering shares a Rust
primitive, and calendar/provider validation is not part of this oracle.
The independent testing boundary passes consolidated source-bound quality:
2,921 Rust tests in 80 suites, 308,646/336,737 covered lines (91.657881%), every
global/risk 80% dimension and all six critical mutants caught. Both maintained
fuzz graphs build; 128 runs pass with fixed seed 4113 and active sanitizers.
Exact publication `71020bc2` passes strict Linux CI run `37955205979`: all 367
named checks and 2,921 Rust tests, with actual checkout and final quiet
confirmation verified. Performance, common conformance and
usage remain separate owner blocks: `STD-LOG-PERF-001`,
`STD-LOG-CONF-001` and `STD-LOG-DOC-001`. The full owner and native logging
ABI/AOT are not promoted.

The [logging measurement register](testing/stdlib-log-performance.json) and
[performance contract](docs/contracts/stdlib-log-performance.md) define 18
hosted scalar VM whole-workload routes in the qualified test profile, with
27 retained samples each. Filtered/accepted/dropped/rejected receipts, exact
output, queue pressure and writer failures remain observable. Actual VM/host
counters, modeled queue high water and selected fixture storage stay separate;
no native AOT, RSS, optimized production latency or speedup is claimed.

### 14.19 Reglas de rendimiento de codecs

Los codecs de datos:

- decodifican tipos estáticos directamente, sin DOM intermedio;
- escriben a buffers crecientes con reservas comprobadas o a `Writer`;
- utilizan vistas prestadas para tokens o bytes cuando su vida es local y
  segura;
- evitan transcoding cuando input y output ya son UTF-8 válido;
- mantienen fast paths para ASCII, búsqueda de caracteres especiales, varints y
  copias de bytes;
- mantienen frames/worklists explícitos para YAML, sin usar la pila recursiva del
  host ni expandir aliases sin presupuesto;
- mantienen frames/worklists explícitos para TOML, sin usar la pila recursiva
  del host ni una tabla de paths sin límite;
- mantienen frames/worklists explícitos para CBOR, validando breaks,
  longitudes indefinidas, tags y mapas arbitrarios sin consumir la pila
  recursiva del host;
- ejecutan `std.regex` con el crate `regex`, sin backtracking exponencial, con
  tablas Unicode fijadas y límites de programa, input, matches y output
  comprobados antes de publicar resultados;
- pueden usar SIMD o kernels nativos bajo las reglas de 11.6; y
- demuestran equivalencia mediante vectores oficiales, fuzzing diferencial,
  round trips, corpus adversario, implementaciones externas y comparación
  scalar/optimized. La identidad ejecutable de la conformance de codecs queda
  en `testing/stdlib-codec-conformance.json` y no se confunde con un round-trip
  contra el propio bridge.

Los benchmarks separan parse, validate, typed decode, dynamic decode, encode,
streaming y allocation. También miden profundidad hostil, strings con escapes,
maps grandes, unknown fields y chunks mínimos; optimizar el happy path no puede
convertir un input adversario en consumo no acotado.

### 14.20 `std.encoding`

`std.encoding` escribe datos binarios como texto y los lee de vuelta:

~~~tondo pseudocode
pub enum Base64Alphabet { Standard, Url }
pub enum EncodingError { InvalidCharacter(Int), Truncated, NotUtf8 }

pub fn base64Encode(bytes: Array[Byte], alphabet: Base64Alphabet = Base64Alphabet.Standard): String
pub fn base64Decode(text: String, alphabet: Base64Alphabet = Base64Alphabet.Standard): Array[Byte] ! EncodingError
pub fn hexEncode(bytes: Array[Byte]): String
pub fn hexDecode(text: String): Array[Byte] ! EncodingError
pub fn urlEncode(text: String): String
pub fn urlDecode(text: String): String ! EncodingError
~~~

- Base64 sigue RFC 4648: `Standard` usa `+` y `/` y rellena con `=`; `Url` usa
  `-` y `_` y no rellena. Al leer, el relleno es opcional en ambos.
- `hexEncode` escribe dos dígitos minúsculos por byte; `hexDecode` acepta
  cualquier caja.
- `urlEncode` escribe cada byte del UTF-8 como `%XX` salvo letras, dígitos, `-`,
  `.`, `_` y `~` (RFC 3986), así que sirve para un segmento de path o un valor de
  query. `urlDecode` lee `%XX` y deja `+` como está.
- Un carácter que la codificación no usa es `InvalidCharacter` con su offset en
  bytes; un texto que acaba a mitad de un valor, `Truncated`; lo que `urlDecode`
  lee y no es UTF-8, `NotUtf8`.

### 14.21 `std.hash`

`std.hash` calcula digests y HMAC (RFC 2104):

~~~tondo pseudocode
pub type Digest
pub fn Digest.bytes(self): Array[Byte]
pub fn Digest.hex(self): String

pub fn sha1(data: Array[Byte]): Digest
pub fn sha256(data: Array[Byte]): Digest
pub fn sha512(data: Array[Byte]): Digest
pub fn hmacSha256(key: Array[Byte], data: Array[Byte]): Digest
pub fn hmacSha512(key: Array[Byte], data: Array[Byte]): Digest
~~~

`Display` de un `Digest` es su hexadecimal minúsculo:
`hash.sha256(text.bytes())` se interpola directamente. SHA-1 existe para los
formatos que lo nombran, como UUIDv5 y Git; para seguridad se usa SHA-256.

### 14.22 `std.random`

`std.random` da valores pseudoaleatorios de un generador explícito; no hay
estado global:

~~~tondo pseudocode
pub type Random
pub fn Random.seeded(seed: Int): Random
pub fn Random.new(): Random
pub fn Random.int(mut self, range: Range[Int]): Int
pub fn Random.float(mut self): Float
pub fn Random.bool(mut self): Bool
pub fn Random.bytes(mut self, count: Int): Array[Byte]
pub fn Random.shuffled[T](mut self, items: Array[T]): Array[T]
pub fn Random.choice[T](mut self, items: Array[T]): T?
pub fn entropy(count: Int): Array[Byte]
~~~

- El generador es SplitMix64: la misma semilla da los mismos valores en
  cualquier máquina. `Random.new()` toma la semilla de la entropía del sistema.
- `int` da cada valor del rango con la misma probabilidad, sin sesgo:
  `dice.int(1..=6)`. Un rango vacío, o con más valores que `Int.max`, es un
  pánico. `float` da un valor de `[0, 1)` con 53 bits; `shuffled` es
  Fisher-Yates sobre una copia; `choice` da `none` sin elementos.
- El generador no sirve para secretos: claves, tokens y nonces usan
  `random.entropy(count)`, que lee la entropía del sistema.

---

## 15. Contrato exigido a cada módulo

Antes de considerarse especificado, cada módulo debe incluir:

### 15.1 Identidad

- Nombre canónico.
- Versión desde la que existe.
- Edición mínima y máxima, si existe.
- Clase de disponibilidad.
- Targets, perfiles y capabilities.
- Dependencias estándar directas.

### 15.2 Declaraciones

- Firmas Tondo exactas.
- Visibilidad.
- Tipos nominales y constructibilidad.
- Variantes y payloads de enums.
- Constraints.
- Capacidades estructurales.
- Métodos inherentes y traits.
- Operaciones terminales.

### 15.3 Semántica

- Precondiciones.
- Resultado normal.
- Ausencia.
- Errores y su prioridad.
- Pánicos cerrados.
- Orden de evaluación.
- Orden de output o iteración.
- Mutación, copia, movimiento y préstamos.
- Efectos de host.

### 15.4 Ejecución

- Si espera y en qué puntos.
- Puntos de cancelación.
- Backpressure.
- Cleanup normal y anormal.
- Atomicidad y datos parciales.
- Seguridad de retry.

### 15.5 Coste

- Complejidad.
- Allocation y COW.
- Materialización.
- Límites.
- Resolución o precisión.
- Comportamiento ante overflow y agotamiento.
- Ruta escalar de referencia y rutas optimizadas, cuando existan.
- Workloads, métricas y umbrales de regresión.
- Efecto sobre tamaño de código y tiempo de compilación.

### 15.6 Portabilidad

- Comportamiento común.
- Diferencias declaradas por target.
- Datos nativos no portables.
- Encoding.
- Reproducibilidad.

### 15.7 Evidencia

- Ejemplos verificables por `tondo doc-test`: parse, formato y typecheck cuando
  corresponda, sin ejecución ni efectos.
- Para cualquier comportamiento runtime mostrado, un caso de aceptación
  enlazado que ejecute la API pública mediante `tondo test` o conformidad.
- Casos positivos.
- Rechazos estáticos.
- Fallos recuperables.
- Límites.
- Composición.
- Properties o modelo.
- Adaptador público de conformidad.
- Benchmarks reproducibles cuando la API esté en un hot path.
- Expansión y hashes de providers/generators cuando genere código.

Una sección marcada “implementation-defined” debe enumerar exactamente qué
puede variar y cómo puede observarse. No es una cláusula abierta.

---

## 16. Testing y conformidad

### 16.1 Corpus por API

Cada API pública tiene evidencia en seis dimensiones:

1. Camino positivo.
2. Rechazo o fallo.
3. Límite.
4. Composición.
5. Oracle independiente o modelo.
6. Frontera pública.

Una única prueba puede cubrir varias dimensiones solo cuando la matriz enlaza
la observación exacta; proximidad de código no cuenta como evidencia.

El coordinador de conformance debe tener una fila por cada fila normativa de la
matriz y no puede inferir una observación desde la mera existencia de un
directorio, un kernel o un test vecino. Cada estado `partial`, `pending` o
`gap` lleva razón y referencias; solo `verified` puede entrar en una promoción,
y únicamente cuando el adaptador público haya producido la observación
correspondiente. `testing/stdlib-conformance-coordination.json` y su checker
son la fuente ejecutable de esta clausura administrativa.

La documentación mantiene la misma disciplina: un ejemplo debe ser
reproducible desde un comando registrado y, si es un fixture runtime, conservar
sus sidecars de salida y estado. La existencia de una implementación Rust o de
un documento no convierte por sí sola una firma en API completa; el estado
`public_api` se deriva de `testing/stdlib-public-api.json` y no se puede
promover desde la documentación.

### 16.2 Tests adicionales por clase

Una API core prueba además:

- Determinismo.
- Ownership.
- COW o independencia de writes cuando aplique.
- Valores vacíos y máximos representables.

Una API capability-gated prueba además:

- Import y uso con capability presente.
- Rechazo `E1008` con capability ausente.
- Fallos del host.
- Cleanup.
- Matriz de targets publicada.

Una API que espera prueba además:

- Suspensión real.
- Progreso de otras tasks.
- Cancelación.
- Pánico y error durante cleanup.
- Roots a través de fronteras de suspensión implícitas o explícitas.
- Backpressure y límites.

Un codec o parser prueba además:

- Vectores oficiales e interoperabilidad con al menos dos implementaciones
  independientes cuando existan.
- Fuzzing de parser, decoder, streaming y round trip.
- Chunks en todos los boundaries relevantes.
- Profundidad, longitud, overflow y payloads truncados o mal formados.
- Policies de unknown/duplicate fields.
- Equivalencia typed/dynamic cuando representen el mismo valor.
- Equivalencia byte a byte entre oracle escalar y cada kernel optimizado.
- Ausencia de materialización intermedia en la ruta tipada mediante
  instrumentación de allocations.

Un provider o generador prueba además:

- Determinismo byte a byte.
- Sandbox y denegación de todas las capabilities.
- Presupuestos de pasos, memoria y output.
- Spans y diagnostics de input.
- Colisiones, outputs ausentes/adicionales y fuente generada inválida.
- Expansiones genéricas, campos privados autorizados y conflictos de coherencia.

Un recurso afín prueba además:

- Consumo correcto.
- Doble consumo rechazado.
- Abandono rechazado.
- Unwind.
- Fallback defensivo del host.

### 16.3 Conformidad pública

La suite estándar:

- Es independiente de la implementación.
- Tiene versión y manifest propios.
- Fija la especificación y distribución por hash.
- Se ejecuta mediante un adaptador público.
- Regenera el corpus Tondo 0.1 contra el contrato actual.
- Ejecuta el mismo corpus contra VM y futuro backend nativo.
- Distingue limitación de target de fallo de implementación.

Un target solo anuncia un módulo cuando supera todos sus casos aplicables.

### 16.4 Dogfooding

Cuando `tondo test` exista:

- Las partes portables de stdlib se prueban mediante el runner público.
- Los helpers de `std.testing` se prueban a sí mismos sin registro privado.
- Los ejemplos del spec se ejecutan.
- Los tests privilegiados permanecen separados y justifican su frontera.

No se crea un segundo harness exclusivo para la stdlib.

El diagnóstico dinámico sigue la frontera de
[`docs/contracts/diagnostic-tooling.md`](./docs/contracts/diagnostic-tooling.md):
race detection, retención/leaks y crash dumps pertenecen al compilador, runtime
y CLI. `std.testing` solo conserva el contexto de test, logs, tags y artifacts
que el runner ya define; el catálogo STD-0.1 no añade `std.race`, `std.leaks` ni
`std.crash`, ni duplica APIs sync/async para instrumentarlas. Los hooks internos
que necesite la VM o el backend nativo no forman parte de la superficie pública.
La VM hosted ya implementa la capa interna de observabilidad de
`DIAG-RUNTIME-001`, documentada en
[`docs/contracts/diagnostic-runtime.md`](./docs/contracts/diagnostic-runtime.md);
los adapters de los owners B se incorporan posteriormente en
`DIAG-STDLIB-001`.

---

## 17. Evolución de la API

### 17.1 API estable únicamente

El namespace `std` publicado no contiene APIs experimentales. Una propuesta
todavía inestable vive:

- En una rama de especificación.
- En un paquete ordinario fuera de `std`.
- En un prototipo interno no accesible desde fuente.

No existe `std.experimental`, flag oculto ni variable de entorno que cambie la
API publicada.

### 17.2 Cambios aditivos

Antes de aceptar una adición se comprueba:

- Que pertenece al catálogo o lo actualiza deliberadamente.
- Que no duplica otra forma.
- Que no cambia resolución.
- Que su propietario canónico es claro.
- Que sus capabilities son mínimas.
- Que puede documentarse y probarse por completo.
- Que no obliga a congelar una ABI o layout.

### 17.3 Deprecación

STD-0.1 no introduce sintaxis de deprecación. Una API publicada permanece hasta
una versión mayor. La documentación puede recomendar una alternativa, pero el
toolchain no inventa annotations o warnings sin una especificación separada.

### 17.4 Correcciones

Corregir una divergencia entre implementación y spec restaura el contrato. La
release note identifica:

- Comportamiento incorrecto anterior.
- Comportamiento normativo.
- Versiones y targets afectados.
- Tests que evitan regresión.

Si programas válidos dependían razonablemente del comportamiento documentado,
el cambio se trata como compatibilidad, no como bug conveniente.

---

## 18. Características deliberadamente diferidas

No forman parte de STD-0.1:

- Locale y formatting cultural implícito; calendario civil y zonas horarias sí
  pertenecen a `std.time`.
- RPC y gRPC; Protobuf y la frontera de red sí pertenecen a STD-0.1.
- Formatos adicionales a JSON, MessagePack, Protobuf, YAML, TOML y CBOR.
- Aleatoriedad y entropía de usuario.
- Streams generales o reactivos; los protocolos acotados de byte I/O de
  `std.io`, sockets de `std.net` y los canales del lenguaje sí pertenecen a
  0.1.
- Priority queues.
- Decimal, BigInt y Complex.
- Matrices, tensors y álgebra lineal.
- `WeakRef`, `Cell` y mutabilidad interior general.
- Dynamic linking y plugins.
- Package manager y acceso a registries.

La presencia de la capability correspondiente en
`tondo-capabilities-draft` no adelanta estas APIs.

La división siguiente es solo una secuencia de implementación; ambas fases
pertenecen al mismo contrato y deben cerrarse antes de publicar 0.1.0:

~~~text
STD-0.1A foundation + hosted
    -> backend nativo conforme
        -> STD-0.1B concurrency + application
            -> Gate S1 y publicación STD 0.1.0
~~~

Una necesidad concreta puede promover una familia mediante una revisión del
tracker y de esta especificación, no mediante implementación ad hoc.

---

## 19. Integración del borrador actual

Tondo todavía no tiene una release. La stdlib, el adaptador hosted y el corpus
de conformidad forman una única línea viva y usan el contrato canónico descrito
aquí. Un cambio actualiza en el mismo bloque PackageId, interfaces, lowering,
runtime, tests y manifest; no se conserva un paquete bootstrap, un shim o una
suite anterior por compatibilidad.

### 19.1 `std.console`

`std.console` escribe en la salida y la salida de error del programa y lee su
entrada:

~~~tondo
console.println("hello")
console.eprintln("warning: no config, using defaults")
match console.readLine() {
    some(line) => console.println("read {line}"),
    none => console.println("no more input"),
}
~~~

La superficie es:

~~~tondo pseudocode
pub fn print(text: String)
pub fn println(text: String)
pub fn eprint(text: String)
pub fn eprintln(text: String)
pub fn readLine(): String?
pub fn flush(): Unit ! io.IoError

pub fn stdout(): io.Writer
pub fn stderr(): io.Writer
pub fn stdin(): io.Reader
pub fn setOutput(writer: io.Writer)
pub fn setError(writer: io.Writer)
pub fn setInput(reader: io.Reader)
~~~

- `print` y `println` escriben en la salida actual; `eprint` y `eprintln`, en
  la salida de error. `println` y `eprintln` añaden un único LF. No esperan al
  sistema operativo ni pueden fallar: una llamada es una sentencia ordinaria.
- `readLine` lee la siguiente línea de la entrada actual sin su `\n` ni `\r\n`,
  o `none` al final. Los bytes que no son UTF-8 se sustituyen por `U+FFFD`; un
  fallo del host cuenta como final de la entrada.
- `stdout()`, `stderr()` y `stdin()` devuelven los streams actuales como
  `io.Writer` e `io.Reader`. `setOutput`, `setError` y `setInput` los
  reemplazan para todo el programa, todas sus tareas incluidas; lo que el
  stream anterior tenga pendiente le llega antes:

~~~tondo
let buffer = io.Buffer()
let previous = console.stdout()
console.setOutput(buffer.writer())
report()
console.setOutput(previous)
let captured = buffer.text()?
~~~

- La salida estándar se acumula en un buffer que el runtime entrega cuando se
  llena, en cada línea si es una terminal, antes de leer la entrada y al
  terminar el programa; un programa nunca necesita llamar a `flush`. La salida
  de error no se acumula. `flush` entrega lo pendiente de la salida actual y
  devuelve el primer fallo de entrega ocurrido desde el anterior `flush`.
- Un fallo de entrega que el programa no observe se reporta al terminar en
  stderr como `error: standard output: …` y el proceso termina con estado 1,
  salvo que ya termine con otro estado de error o pánico. Escribir en una pipe
  cuyo lector ya cerró termina el proceso sin mensaje, como cualquier
  herramienta del sistema.

### 19.2 `std.process`

`std.process` lanza procesos. `Command` y `Pipeline` son planes inertes del
lenguaje (20.5 y 20.6 de su especificación): describirlos no ejecuta nada, se
componen con `|` y son `Copy`, `Send` y `Share`.

~~~tondo
import std.process

let log = process.command("git", "log", "--oneline")
let output = (log | process.command("grep", pattern)).output()?
console.print(output.text()?)

process.command("make", "all")
    .directory("build")
    .env("CC", "clang")
    .stdout(process.Output.Append("build.log"))
    .stderr(process.Output.Stdout)
    .check()?
~~~

La superficie es:

~~~tondo pseudocode
pub fn command(program: String, arguments: ...String): Command
pub fn shell(text: String): Command
pub fn exit(code: Int): Never

pub enum Input {
    Inherit
    Null
    Text(String)
    Bytes(Array[Byte])
    File(String)
    Reader(io.Reader)
    Pipe
}
pub enum Output {
    Inherit
    Null
    Capture
    File(String)
    Append(String)
    Writer(io.Writer)
    Stdout
    Pipe
}
pub type ExitStatus = { code: Int, success: Bool }
pub type ProcessOutput = {
    stdout: Array[Byte]
    stderr: Array[Byte]
    combined: Array[Byte]
    statuses: Array[ExitStatus]
}
pub enum ProcessError {
    NotFound
    PermissionDenied
    InvalidData
    Failed(ExitStatus, ProcessOutput)
    Host
}
pub type Process

pub fn Command.directory(self, path: String): Command
pub fn Command.env(self, name: String, value: String): Command
pub fn Command.removeEnv(self, name: String): Command
pub fn Command.clearEnv(self): Command
pub fn Command.stdin(self, input: Input): Command
pub fn Command.stdout(self, output: Output): Command
pub fn Command.stderr(self, output: Output): Command
pub fn Command.mergeStderr(self): Command
pub fn Pipeline.stdin(self, input: Input): Pipeline
pub fn Pipeline.stdout(self, output: Output): Pipeline
pub fn Pipeline.mergeStderr(self): Pipeline

// En Command y en Pipeline:
pub fn run(self): ExitStatus ! ProcessError
pub fn status(self): ExitStatus ! ProcessError
pub fn output(self): ProcessOutput ! ProcessError
pub fn check(self): ProcessOutput ! ProcessError
pub fn start(self): Process ! ProcessError

pub fn ProcessOutput.text(self): String ! ProcessError
pub fn Process.stdin(self): io.Writer
pub fn Process.stdout(self): io.Reader
pub fn Process.stderr(self): io.Reader
pub fn Process.id(self): Int
pub fn Process.wait(self): ExitStatus ! ProcessError
pub fn Process.output(self): ProcessOutput ! ProcessError
pub fn Process.check(self): ProcessOutput ! ProcessError
pub fn Process.cancel(self): ExitStatus ! ProcessError
pub fn Process.detach(self)
~~~

Planes:

- `command` guarda el programa y cada argumento por separado: no hay shell,
  globbing ni expansión de variables. `shell` es el único constructor que usa
  el shell del sistema (`/bin/sh -c` en Unix).
- `|` une el stdout de la última etapa de la izquierda con el stdin de la
  primera de la derecha mediante una pipe del sistema, que aplica
  backpressure. `mergeStderr()` envía además el stderr al mismo sitio que el
  stdout (`|&`, `2>&1`); en un `Pipeline`, el de todas sus etapas.
- `directory`, `env`, `removeEnv` y `clearEnv` fijan el directorio y el
  entorno del hijo, que por lo demás hereda los del programa (14.3.5).
- `stdin`, `stdout` y `stderr` redirigen los streams: `Inherit` usa los del
  programa, `Null` ninguno, `Text` y `Bytes` dan una entrada fija, `File` lee o
  escribe un archivo (`<`, `>`), `Append` añade al final (`>>`), `Reader` y
  `Writer` conectan un stream de `std.io` —como un `io.Buffer` o la consola—,
  `Capture` recoge la salida en el `ProcessOutput`, `Stdout` envía el stderr
  al stdout (`2>&1`) y `Pipe` deja el stream al programa a través del
  `Process`. En un `Pipeline`, `stdin` es la de la primera etapa y `stdout` la
  de la última.

Ejecución:

- `run` usa por defecto los streams del programa, espera y devuelve el estado.
- `status` espera sin entrada y descartando la salida.
- `output` captura stdout y stderr, sin entrada, y devuelve un
  `ProcessOutput`. `check` hace lo mismo y devuelve `ProcessError.Failed` con
  el estado y la salida si alguna etapa no termina con éxito.
- `start` inicia el proceso sin esperarlo; sus streams son los del programa
  salvo los redirigidos. `Process.wait`, `output`, `check` y `cancel` lo
  esperan; `cancel` termina antes todas sus etapas.
- Las redirecciones explícitas prevalecen sobre los valores por defecto de
  cada operación.
- Un código de salida distinto de cero es un dato: solo `check` lo convierte en
  error. El estado de un `Pipeline` tiene éxito cuando lo tienen todas sus
  etapas (`pipefail`), y su código es el de la última etapa que falla, o el de
  la última. Un proceso terminado por una señal tiene el código `128 + señal`,
  como lo informan los shells.
- `combined` reúne stdout y stderr en el orden en que el host los entregó, sin
  inventar un orden por líneas. Los stderr de un `Pipeline` se concatenan en
  orden de etapas.
- `ProcessOutput.text()` decodifica el stdout como UTF-8 y devuelve
  `InvalidData` si no lo es.
- No poder iniciar un programa da `NotFound` o `PermissionDenied`; `Host`
  reúne los demás fallos del host.

Vida de un `Process` (20.9 del lenguaje):

- Abandonar el handle cierra la entrada de sus procesos y espera a que
  terminen, como un `Join`; durante un pánico los cancela. `detach()` los suelta
  para que sigan sin esperarlos, también después de terminar el programa.
- Cancelar la tarea que espera un proceso termina sus etapas, espera a que
  acaben y no deja zombies.
- `exit(code)` entrega la salida pendiente de la consola y termina el programa
  con ese código, sin ejecutar los `defer` ni esperar a otras tareas.

### 19.3 `std.ffi`

`std.ffi` acompaña a las declaraciones `extern fn` y `extern type` (16.16 del
lenguaje): nombra los tipos de C y da memoria y strings que C puede usar.

~~~tondo
import std.ffi

extern fn strlen(text: ffi.CString): ffi.CSize

let length = unsafe { strlen("tondo") }       // 5
let buffer = ffi.allocate[ffi.CChar](64)
unsafe { ffi.free(buffer) }
~~~

Tipos de C, alias de los tipos de Tondo del mismo tamaño y signo en los targets
soportados:

| Alias | Tipo | Alias | Tipo |
|---|---|---|---|
| `CChar` | `Byte` | `CULong` | `UInt64` |
| `CShort` | `Int16` | `CLongLong` | `Int` |
| `CUShort` | `UInt16` | `CULongLong` | `UInt64` |
| `CInt` | `Int32` | `CSize` | `UInt64` |
| `CUInt` | `UInt32` | `CSSize` | `Int` |
| `CLong` | `Int` | `CFloat` | `Float32` |
| `CDouble` | `Float` | `CBool` | `Bool` |

- `CString` es un `char*` que termina en un byte cero: un newtype de
  `Pointer[CChar]`, que se usa como él y al revés. Un `String` pasa donde una
  función de C toma un `CString` como una copia para la llamada.
- `Void` es lo que señala `void*`: un tipo opaco, y todo `Pointer[T]` o
  `CString` se usa como `Pointer[Void]`.
- `VaList` es el `va_list` de C, opaco: una función que C llama lo recibe como
  `Pointer[VaList]` y lo pasa a la función de C que lo formatea.

Funciones:

~~~tondo pseudocode
pub fn sizeOf[T](): Int
pub fn alignOf[T](): Int
pub fn allocate[T: ~Copy](count: Int = 1): Pointer[T]
pub unsafe fn free(pointer: Pointer[Void]?)
pub unsafe fn copy[T](from: Pointer[T], to: Pointer[T], count: Int = 1)
pub fn toCString(text: String): CString
pub unsafe fn fromCString(text: CString): String
pub fn toCChars(text: String, count: Int): Array[CChar]
pub fn fromCChars(chars: Array[CChar]): String
pub unsafe fn bytes(from: Pointer[Byte], count: Int): Array[Byte]
pub fn context[T: ~Copy](value: T): Pointer[Void]
pub unsafe fn fromContext[T](data: Pointer[Void]): T
pub unsafe fn freeContext[T: ~Copy](data: Pointer[Void])
~~~

- `allocate` da memoria a ceros para `count` valores del asignador de C, así
  que C puede liberarla; `free` la devuelve, igual que la de `toCString`,
  `context` o el `malloc` de C, y `none` no hace nada. Un `count` negativo
  produce un pánico.
- `copy` copia `count` valores; las regiones pueden solaparse.
- `toCString` copia el texto con un byte cero al final para que C lo guarde; un
  byte cero dentro produce `P0016`. `fromCString` lee un string de C hasta su
  byte cero, y los bytes que no son UTF-8 pasan a ser U+FFFD.
- `toCChars` y `fromCChars` hacen lo mismo con un array de C de un struct,
  como `@count(16) name: CChar`: el primero rellena con ceros hasta `count` y
  produce un pánico si el texto y un cero no caben.
- `context` guarda una copia del valor en memoria de C y da su dirección, el
  `void*` que las APIs de C devuelven a una función que llaman;
  `fromContext[T]` lee el valor, que debe ser un `T`, y `freeContext[T]` lo
  suelta y devuelve la memoria.

### 19.4 Una sola implementación vigente

El compilador, la VM y los adapters pueden usar detalles internos distintos,
pero solo implementan una identidad pública y un contrato fuente. Cuando el
contrato cambia antes de la primera release, esos detalles, sus tests y la
conformidad se migran en el mismo bloque; no queda un bridge anterior activo.

---

## 20. Checklist de publicación

STD-0.1 puede publicarse como 0.1.0 únicamente cuando:

- [ ] Cada superficie del catálogo tiene firmas exactas.
- [ ] Prelude y namespace permanecen coherentes con el lenguaje.
- [ ] PackageId, content hash y API hash son finales.
- [ ] La matriz edición/target/perfil/capabilities está versionada.
- [ ] Todas las unidades privilegiadas están fijadas por hash.
- [ ] No existe búsqueda ambiental o de red durante compilación.
- [ ] Cada API documenta errores, pánicos, ownership, esperas, orden y coste.
- [ ] Todas las capabilities ausentes producen rechazo estático.
- [ ] Los tipos y errores tienen un propietario canónico.
- [ ] Los módulos core no dependen de host.
- [ ] Los recursos tienen una acción de salida y una operación de cierre probadas.
- [ ] Los ejemplos son ejecutables.
- [ ] La matriz de evidencia cubre las seis dimensiones.
- [ ] La suite estándar pasa sobre la VM.
- [x] `std.testing` se prueba mediante `tondo test`.
- [ ] Los programas representativos de texto, colecciones, archivos y procesos
      pasan el gate estricto.
- [ ] La distribución es reproducible byte a byte.
- [ ] Providers y generators son herméticos, deterministas y están fijados.
- [ ] JSON, MessagePack y Protobuf pasan interoperabilidad, fuzzing, streaming y
      límites.
- [ ] Cada hot path tiene oracle escalar, equivalencia de kernels y gate de
      rendimiento multidimensional.
- [ ] El corpus vivo Tondo 0.1 se regenera y pasa contra la distribución actual.
- [ ] Todo lo diferido está ausente o identificado fuera de `std`.

Hasta cerrar esta lista, el documento puede ser normativo para las reglas base
sin que exista una release pública completa de STD-0.1.
