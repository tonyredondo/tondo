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
  codecs adicionales, regex, UUID y logging.
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
| `std.csv` | Core | — | 0.1 | CSV RFC 4180 materializado y en streaming |
| `std.hash` | Core | — | 0.1 | Digests MD5, SHA-1 y SHA-2, HMAC, por partes, y hashes no criptográficos |
| `std.http` | Capability-gated | `network` | 0.1 | Cliente y servidor HTTP/1.1 y HTTP/2 con TLS, cuerpos en streaming, multipart, archivos con rangos, Server-Sent Events y WebSockets |
| `std.random` | Core + gated | `entropy` para sembrar desde el sistema | 0.1 | Generadores pseudoaleatorios sembrados, mezcla, elección y bytes de entropía |
| `std.uuid` | Core + gated | `entropy` y/o `civil-clock` para generación | 0.1 | UUID, parsing, formatting y generadores explícitos por versión |
| `std.sync` | Core + gated | `threads` para operaciones cross-thread | 0.1 | Mutex, rwlock, condición, semáforo, once, barrier, atomics y colecciones compartidas linealizables |
| `std.executor` | Core + gated | `threads` para pools bloqueantes | 0.1 | Pools acotados, actores y bridge de trabajo bloqueante sobre las tareas del lenguaje |
| `std.log` | Core | — | 0.1 | Eventos estructurados con niveles, en texto o JSON, sobre un `io.Writer` |
| `std.diagnostics` | Core | — | 0.1 | Contadores y gauges del programa, que `tondo counters` muestra |
| `std.profile` | Core | — | 0.1 | Perfiles que el programa hace de sí mismo, en el formato de pprof, y labels que los dividen |
| `std.embed` | Core | — | 0.1 | Archivos del paquete leídos al compilar y guardados en el ejecutable |
| `std.url` | Core | — | 0.1 | URLs y referencias relativas de RFC 3986: sus partes, su resolución y su query |
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
- `std.log` escribe sobre un `io.Writer` explícito; no existe logger global.
- Los argumentos del programa se obtienen mediante `std.env.args()`.
- `std.env` no lee el entorno durante la compilación.
- `std.embed` lee archivos del paquete durante la compilación, nunca al
  ejecutar, y no concede acceso al filesystem.
- `std.testing` solo existe en source sets de test y no forma parte de producción.
- Streams, canales, red y sincronización solo aparecen bajo sus propietarios
  canónicos; ningún módulo existente los introduce indirectamente.

### 14.2 Estado del borrador

El catálogo y sus propietarios son normativos. Las declaraciones exhaustivas de
las veintinueve superficies permanecen pendientes, salvo el núcleo sellado que
`TONDO_TESTING_SPEC.md` ya fija para `std.testing`, `std.time` y `std.env`.
No existe un contrato bootstrap separado: el corpus consume siempre este
borrador.

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
impl Additive for Duration                 // a + b, a - b, durations.sum()
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

Un bloque `within` (11.17 del lenguaje) pone un plazo a lo que ejecuta. Su
error, y lo que se lee del plazo actual:

~~~tondo pseudocode
pub type TimedOut = {
    limit: Duration
}

pub fn deadline(): Instant?
pub fn remaining(): Duration?
~~~

- `TimedOut` es el error de un `within` cuyo plazo venció; `limit` es el plazo
  que se le dio. `Display` dice `timed out after 2s`.
- `deadline` es el instante en que vence el plazo de la tarea, el más cercano
  de los `within` que la contienen, o `none` sin plazo. `remaining` es lo que
  falta hasta él, cero si ya venció.
- El cliente de `std.http` y las operaciones de red usan el plazo como límite
  de cada espera, y un protocolo que sabe llevarlo al otro lado, como gRPC con
  `grpc-timeout`, lo envía.

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
pub fn hostname(): String
pub fn processId(): Int
pub fn executable(): String
~~~

- `hostname` es el nombre de la máquina, como lo da el sistema; `processId`, el
  identificador del proceso del programa; y `executable`, el path absoluto de
  su ejecutable, con los enlaces simbólicos resueltos.

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
- Si un pánico interrumpe el cierre de un `update`, `tryUpdate`, `updateWhen`
  o `read`, el lock se suelta al desenrollar y el valor queda como lo dejó el
  cierre (15.9 del lenguaje). Ningún lock queda envenenado: la siguiente tarea
  lo toma con normalidad.
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
  secuencialmente consistentes. Con enteros de hasta 64 bits, `Bool`, `Char` y
  `Byte` usa instrucciones atómicas; con otros tipos, los enteros de 128 bits
  incluidos, un lock. `compareExchange` nunca falla de forma espuria:
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

`std.executor` construye políticas de ejecución sobre `spawn`, `Join` y
canales, que siguen siendo las únicas tareas: no publica otro handle de tarea,
ni un executor global, ni una segunda forma de esperar o cancelar.

~~~tondo
import std.executor

let pool = executor.pool(4, 16)?                    // ! ExecutorError
let job = pool.submit(() { fetch(url) })?           // Join[T] ! SubmitError
match pool.trySubmit(() { work() }) {
    ok(other) => other.wait()
    err(executor.SubmitError.Saturated) => retryLater()
    err(executor.SubmitError.Closed) => {}
}
let page = job.wait()
pool.shutdown()

let counter = executor.actor(0, 8, (total: var Int, amount: Int) { total += amount })?
let address = counter.reference()                    // ActorRef[Int]
address.send(5)?                                     // ! SendError
address.trySend(5)?
counter.stop()?                                      // ! E del handler
~~~

Un `Pool` ejecuta como mucho `workers` trabajos a la vez y mantiene como mucho
`capacity` más esperando un worker; es un handle que comparten sus copias.
`executor.pool(workers, capacity)` falla con `ExecutorError.InvalidWorkers`
si `workers < 1` y con `InvalidCapacity` si `capacity < 0`. `submit(job)`
inicia `job`, un `fn(): T`, como tarea que corre cuando hay un worker libre,
y devuelve su `Join[T]`; mientras el pool está lleno, espera. `trySubmit` no
espera y falla con `SubmitError.Saturated`. Tras `shutdown()`, ambos fallan con
`SubmitError.Closed`, también un `submit` que ya esperaba sitio. `shutdown()`
deja de admitir trabajos y espera a que terminen los admitidos. Los trabajos
arrancan según se liberan workers, sin un orden garantizado entre los que
esperan.

Cada trabajo es una tarea ordinaria cuyo `Join` posee quien la envía: el
bloque que la posee la espera al terminar, como a cualquier `Join` (11.11 del
lenguaje), y se cancela con `job.cancel()` o con un `Group`; un trabajo que
espera un worker se cancela en esa espera y no llega a empezar. Por eso el pool
no ofrece una cancelación propia ni exige cerrarse: lo que posee un trabajo es
su `Join`.

Un actor es un dueño de estado con buzón acotado. `executor.actor(state,
capacity, handle)` crea una tarea que empieza con `state` y lo cambia con
`handle: fn(var S, M): !E`, un mensaje cada vez y en el orden en que llegaron;
`capacity` negativa falla con `InvalidCapacity`. `actor.reference()` da un
`ActorRef[M]`, un handle `Copy` y `Send` que no mantiene vivo al actor.
`send(message)` espera mientras el buzón está lleno; `trySend` no espera y
falla con `SendError.Saturated`. Ambos fallan con `SendError.Stopped` cuando
el actor ya paró. Un error de `handle` para al actor.

`actor.stop()` cierra el buzón, deja que el actor procese los mensajes que ya
contiene y espera a que termine; devuelve el error de `handle` si uno lo paró.
Pararlo otra vez devuelve lo mismo. Si su dueño no lo para, el final del bloque
que posee el `Actor` lo hace del mismo modo y espera, igual que con un `Join`
que nadie consumió.

No existe un pool para trabajo bloqueante: cuando una llamada a C se queda en
el sistema, el runtime pone otro worker en su lugar, de modo que una llamada
bloqueante no detiene las tareas (16.16 del lenguaje).

#### 14.4.6 Networking

`std.net` ofrece conexiones TCP, con TLS, listeners, sockets UDP y nombres.
Cada operación espera como cualquier llamada (sección 9): la tarea se aparca
sin ocupar un worker ni un thread mientras la red trabaja, así que un servidor
puede tener una tarea por conexión. Cada operación que espera acepta su propio
`timeout`; sin él, no hay ninguno.

~~~tondo
import std.net

let stream = net.connect("example.com", 80, timeout: time.seconds(5))?
stream.write("GET / HTTP/1.0\r\nHost: example.com\r\n\r\n")?
let reply = stream.readText()?

let secure = net.connectTls("example.com", 443)?

let listener = net.listen("127.0.0.1", 0)?   // puerto 0: uno libre
console.println("{listener.address()}")
for connection in listener {
    spawn detached serve(connection)
}

let socket = net.bind("0.0.0.0", 9000)?
socket.sendTo(data, net.address("127.0.0.1", 9001)?)?
let (packet, from) = socket.receive()?

let addresses = net.resolve("example.com")?
~~~

La superficie es:

~~~tondo pseudocode
pub enum NetError {
    NotFound(String)          // el nombre no resuelve
    Refused
    TimedOut
    Reset
    Closed
    AddressInUse
    Unreachable
    InvalidAddress(String)
    Tls(String)
    Io(String)
}

pub type Address = { ip: String, port: Int, path: String }
pub fn address(text: String, port: Int): Address ! NetError
pub fn unixAddress(path: String): Address
pub fn resolve(host: String, timeout: time.Duration? = none): Array[String] ! NetError

pub type Stream
pub fn connect(host: String, port: Int, timeout: time.Duration? = none): Stream ! NetError
pub fn connectTls(host: String, port: Int, trust: String? = none,
    timeout: time.Duration? = none): Stream ! NetError
pub fn Stream.read(self, max: Int = 65536, timeout: time.Duration? = none): Array[Byte]? ! NetError
pub fn Stream.readLine(self, timeout: time.Duration? = none): String? ! NetError
pub fn Stream.readAll(self, timeout: time.Duration? = none): Array[Byte] ! NetError
pub fn Stream.readText(self, timeout: time.Duration? = none): String ! NetError
pub fn Stream.write(self, text: String, timeout: time.Duration? = none): Unit ! NetError
pub fn Stream.writeBytes(self, data: Array[Byte], timeout: time.Duration? = none): Unit ! NetError
pub fn Stream.closeWrite(self, timeout: time.Duration? = none): Unit ! NetError
pub fn Stream.close(self, timeout: time.Duration? = none): Unit ! NetError
pub fn Stream.reader(self): io.Reader
pub fn Stream.writer(self): io.Writer
pub fn Stream.localAddress(self): Address
pub fn Stream.remoteAddress(self): Address

pub type Tls = { certificate: String, privateKey: String }
pub fn Tls.files(certificate: String, privateKey: String): Tls

pub type Listener                             // Iterator[Stream]
pub fn listen(host: String, port: Int, tls: Tls? = none): Listener ! NetError
pub fn Listener.address(self): Address
pub fn Listener.accept(self, timeout: time.Duration? = none): Stream ! NetError
pub fn Listener.close(self)

pub type Socket
pub fn bind(host: String, port: Int): Socket ! NetError
pub fn Socket.address(self): Address
pub fn Socket.sendTo(self, data: Array[Byte], to: Address): Unit ! NetError
pub fn Socket.receive(self, timeout: time.Duration? = none): (Array[Byte], Address) ! NetError
pub fn Socket.close(self)

pub fn connectUnix(path: String, timeout: time.Duration? = none): Stream ! NetError
pub fn listenUnix(path: String): Listener ! NetError
pub fn bindUnix(path: String? = none): Socket ! NetError

pub type Ip
pub fn ip(text: String): Ip ! NetError
pub fn Ip.isV4(self): Bool
pub fn Ip.isV6(self): Bool
pub fn Ip.isLoopback(self): Bool
pub fn Ip.isPrivate(self): Bool
pub fn Ip.isUnspecified(self): Bool
pub type Network
pub fn network(text: String): Network ! NetError
pub fn Network.contains(self, member: Ip): Bool
pub fn Network.ip(self): Ip
pub fn Network.prefix(self): Int
~~~

- `connect`, `listen` y `bind` aceptan un nombre o una dirección IP;
  `connect` prueba cada dirección del nombre, en el orden del sistema, hasta
  que una responde. `resolve` da esas direcciones a quien quiera elegir.
  `address` solo acepta direcciones IP, en su forma habitual; para un nombre,
  su error indica `resolve`.
- `Stream`, `Listener` y `Socket` son handles compartidos: sus copias usan la
  misma conexión. Las lecturas y escrituras pasan por `reader()` y
  `writer()`, streams de `std.io` (10.3), así que pueden mezclarse con las
  del propio `Stream` y con `csv`, `json.decodeFrom` o cualquier consumidor
  de streams. `read` da lo que ya llegó, hasta `max` bytes, o `none` al
  final; `readAll` y `readText` leen hasta que el otro lado termina.
- Una escritura vuelve cuando la conexión tiene los bytes, que salen en orden
  mientras el programa sigue; espera solo si el otro lado no lee y ya hay
  256 KiB pendientes. Un fallo al enviarlos es el error de la escritura o el
  cierre siguientes. `closeWrite` envía lo pendiente y termina la dirección
  de escritura, y el otro lado lee el final; `close` envía lo pendiente y
  cierra la conexión. La última copia que desaparece la cierra también, y lo
  pendiente se envía por su cuenta.
- `timeout` limita esa operación; al vencer falla con `TimedOut` y la
  conexión sigue utilizable. Una tarea cancelada abandona la operación que
  espera.
- TLS confía en los certificados raíz del sistema y, si el sistema no tiene
  ninguno, en los de Mozilla; `trust` nombra un archivo PEM con más
  certificados en los que confiar. `connectTls` verifica que el certificado es
  para `host` antes de devolver la conexión. Un listener con `tls` acepta en
  claro y cada conexión hace su handshake antes de su primera lectura o
  escritura, cuyo error es si falla; así un cliente lento no retiene a los
  demás. `http.Tls` es `net.Tls`.
- Recorrer un `Listener` da cada conexión nueva hasta que se cierra,
  esperando más allá de los fallos al aceptar; `close` hace que las esperas
  de `accept` fallen con `Closed` y suelta el puerto. Un servidor atiende
  cada conexión con `spawn detached serve(connection)` (11.11 del lenguaje).
- Un datagrama llega entero o no llega, en cualquier orden y como mucho una
  vez; `sendTo` rechaza uno de más de 65 535 bytes. `Display` de una
  dirección es `127.0.0.1:80` o `[::1]:80`.
- Un socket Unix es un archivo del sistema por el que se conectan programas de
  la misma máquina, como el agente de Datadog en
  `/var/run/datadog/apm.socket`. `connectUnix`, `listenUnix` y `bindUnix` son
  `connect`, `listen` y `bind` sobre uno: streams y datagramas, con las mismas
  operaciones. Su dirección tiene `path` y no `ip` ni `port`, y `Display` la
  escribe como su path; la de una conexión TCP o UDP tiene `path` vacío.
  `unixAddress` es la dirección a la que `sendTo` envía un datagrama.
- `listenUnix` y `bindUnix` crean el archivo, y `close` lo borra. Si el path ya
  es un socket sin nadie que escuche, el que dejó un programa que terminó, lo
  reemplazan; otro archivo es `AddressInUse`. `bindUnix` sin path da un socket
  sin nombre, que solo envía.
- `ip` lee una dirección IPv4 o IPv6 en sus formas habituales, y `Display` la
  escribe en su forma canónica, la de RFC 5952 para IPv6, así que dos escrituras
  de la misma dirección son iguales y pueden ser claves. Una IPv6 que contiene
  una IPv4 (`::ffff:10.0.0.1`) es esa IPv4. `isPrivate` es verdadero para las
  redes de uso privado de RFC 1918 y RFC 4193.
- `network` lee una red en notación CIDR, `10.0.0.0/8` o `fd00::/8`, y una
  dirección sola, que es la red de esa dirección; `contains` dice si una
  dirección es suya. Una dirección o red que no se lee es `InvalidAddress`, con
  qué escribir.
- Un provider o generador no puede importar `std.net` (`E2108`).

### 14.5 El prelude y `std.math`

Todo programa tiene, sin importar nada, los métodos que el prelude da a los tipos
intrínsecos y el trait `Ordered`. El prelude está escrito en Tondo sobre
operaciones del runtime; un programa solo compila los métodos que usa. Las
posiciones de un `String` son índices de escalares, como en el lenguaje.

~~~tondo pseudocode
pub trait Ordered { fn compare(self, other: Self): Int }
~~~

`Ordered` está implementado para todos los números, `Byte`, `Char`, `String` y
`Bool`; el `compare` de los floats pone `NaN` después de cualquier otro valor y
lo iguala a sí mismo. Un programa puede implementar `Ordered` para sus tipos.

Los números no tienen métodos del prelude: sus operaciones (`parse`,
`toString`, `abs`, `pow`, las variantes de desbordamiento, los bits, los bytes,
las conversiones nombradas, `floor`, `sqrt`...) y sus constantes son intrinsics
del lenguaje, iguales en todos los anchos (18.9 del lenguaje), y los traits
`Additive`, `Number` e `Integer` también son del lenguaje (18.10).

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
~~~

`Char`: `isDigit`, `isLetter`, `isWhitespace`, `isUpper`, `isLower` e
`isAlphanumeric` (`Bool`); `toUpper` y `toLower`, que cambian el escalar cuando
su otra forma es uno solo; `code(): Int`, `Char.from(code: Int): Char?` y
`compare`.

`Option[T]`: `isSome`, `isNone`, `map(fn(T): U): U?`, `andThen(fn(T): U?): U?`,
`unwrapOr(fallback: T): T`, `orElse(other: T?): T?`, `filter(fn(T): Bool): T?`,
`okOr(error: E): T ! E` y `expect(message: String): T`, que hace pánico con el
mensaje si no hay valor. `Result[T, E]`: `isOk`, `isErr`, `map(fn(T): U): U ! E`,
`mapErr(fn(E): F): T ! F`, `andThen(fn(T): U ! E): U ! E`, `unwrapOr`, `ok(): T?`,
`err(): E?` y `expect`, cuyo pánico dice el mensaje y el error: `loading the app:
LoadError.Missing`. Un pánico dentro de la librería estándar se ubica en la
llamada del programa que llegó a él; la traza conserva el frame de la librería.

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
fn sum(self): T                                     // T: Additive
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
pub fn gcd[T: Integer](a: T, b: T): T
pub fn lcm[T: Integer](a: T, b: T): T
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

`std.serialization` es el protocolo común de todos los formatos: los eventos en
que cada uno lee y escribe sus valores, el error que todos comparten y los traits
con los que cualquier tipo se escribe y se lee en cualquiera de ellos. Cada
formato da un `Source` y un `Sink`; ninguno define traits propios ni convierte
sus documentos a través de otro formato.

~~~tondo pseudocode
pub enum Event {
    Null
    Bool(Bool)
    Int(Int)
    Float(Float)
    Number(String)          // un número tal como lo escribe un formato de texto
    Text(String)
    DateTime(String)        // fecha, hora o ambas en RFC 3339, en un formato que las tiene
    Bytes(Array[Byte])
    BeginList
    EndList
    BeginMap
    Key(String)
    EndMap
}
pub enum ErrorKind { Syntax, Type, Missing, Unknown, Range, Io, Unsupported }
pub enum UnknownFields { Ignore, Reject }
pub type Error = { kind: ErrorKind, message: String, path: String, line: Int, column: Int }

pub trait Source {
    fn next(self): Event? ! Error
    fn skip(self, first: Event): Unit ! Error
    fn failure(self, kind: ErrorKind, message: String): Error
    // con implementación por defecto:
    fn rejectsUnknown(self): Bool
    fn value(self): Event ! Error
    fn nextKey(self): String? ! Error
    fn nextField(self, names: Array[String]): Int? ! Error
    fn unknownField(self, key: String, first: Event): Unit ! Error
    fn beginMap(self, first: Event, name: String): Unit ! Error
    fn beginList(self, first: Event, name: String): Unit ! Error
    fn endList(self): Unit ! Error
    fn required[T](self, value: T?, field: String): T ! Error
    fn mismatch(self, first: Event, expected: String): Error
}

pub trait Sink {
    fn null(self)
    fn bool(self, value: Bool)
    fn int(self, value: Int)
    fn float(self, value: Float)
    fn number(self, digits: String)
    fn text(self, value: String)
    fn bytes(self, value: Array[Byte])
    fn dateTime(self, text: String)    // por defecto, como texto
    fn beginList(self, count: Int)
    fn endList(self)
    fn beginMap(self, count: Int)
    fn key(self, name: String)
    fn endMap(self)
    // con implementación por defecto:
    fn omitsNone(self): Bool
}

pub trait Encode {
    fn encode[S: Sink](self, out: S)
}
pub trait Decode {
    fn decode[S: Source](input: S, first: Event): Self ! Error
}
~~~

- `Encode` y `Decode` están implementados para todos los números, `Byte` (un
  entero de 0 a 255), `Bool`, `String`, `T?`, `Array[T]`, `Map[String, V]` y `time.DateTime` (texto RFC 3339
  en UTC, o el `DateTime` del formato), y el compilador los implementa
  para cada record, enum y newtype del programa cuyas partes los implementan,
  cuando el programa trae `std.serialization` directamente o por un formato. Una
  implementación escrita en el programa sustituye a la automática.
- Un record es un mapa con sus campos en orden de declaración; `@name("texto")`
  cambia la clave de un campo y `@ignore` lo omite, y debe ser opcional. `none`
  es `Null`; un formato sin null (`omitsNone`) omite el campo, y un campo
  opcional ausente se decodifica como `none`. Un campo obligatorio ausente da
  `Missing`; uno desconocido se ignora salvo que el `Source` lo rechace.
- Un record se lee miembro a miembro con `nextField(names)`, donde `names`
  son sus claves en orden de declaración: da el índice en `names` de la
  clave del miembro siguiente, leído hasta su valor, o `none` al final del
  mapa. Un miembro cuya clave no está en `names` se salta entero, o falla
  como desconocido si el `Source` los rechaza, y da `names.length()`. La
  implementación por defecto usa `nextKey`, `value` y `unknownField`; un
  formato que lee las claves en su sitio las compara ahí, sin crear una
  cadena para cada una, y salta los valores desconocidos sin crear sus
  eventos.
- Un newtype es su valor. Una variante sin payload es su nombre como texto; una
  con payload es un mapa de una sola clave, el nombre, cuyo valor es el payload,
  una lista si tiene varios componentes o un mapa si tiene campos.
- Un entero que cabe en `Int` se escribe con `int`, y otro, como un `UInt`
  grande o uno de 128 bits, con `number` y sus dígitos: ningún entero pasa por
  un float. Un `Float32` finito se escribe con `number` y sus dígitos más
  cortos, `0.1`, y otro con `float`.
- `Decode` acepta para un número cualquiera de `Int`, `Float` y `Number`, y para
  un entero da `Range` cuando no cabe en su tipo.
- Un `Error` lleva dónde ocurrió como camino (`$.users[2].age`), línea y
  columna, y se muestra como `mensaje at camino (line l, column c)`, sin la
  posición cuando no la hay, como al escribir. `ErrorKind` se muestra con su
  nombre.
- Un `Sink` que no puede representar un valor lo informa al terminar el
  documento, con `Unsupported`.

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
    Int128
    UInt
    UInt8
    UInt16
    UInt32
    UInt128
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
- `ParameterInfo`: `position(): Int`, `name(): String`, `typeInfo(): TypeInfo` y
  `mode(): ParameterMode` (`Value | Ref | Mut | Var`).
- `FunctionInfo`: `parameters(): Array[ParameterInfo]`, `outcome(): TypeInfo`,
  `variadic(): Bool` y `isUnsafe(): Bool`. El resultado incluye el `Result` o
  `Option` completo.

`TypeInfo`, `FieldInfo`, `VariantInfo`, `MethodInfo` y `ParameterInfo` dan las
anotaciones del programa de lo que describen (27.13 del lenguaje):

~~~tondo pseudocode
pub fn TypeInfo.annotation[A: Reflect](self): A?
pub fn TypeInfo.annotations(self): Array[Value]
// y las mismas en FieldInfo, VariantInfo, MethodInfo y ParameterInfo
~~~

`annotation[A]()` es el valor de la anotación de tipo `A`, `none` sin ella;
`annotations()`, todas, en el orden en que están escritas.

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
pub enum DeclarationKind { Record, Enum, Newtype, Function }
pub type FieldView = { name: String, typeRef: TypeRef, private: Bool, annotations: Array[AnnotationView] }
pub type VariantView = {
    name: String
    fields: Array[FieldView]
    elements: Array[TypeRef]
    annotations: Array[AnnotationView]
}
pub type ParameterView = { name: String, typeRef: TypeRef, annotations: Array[AnnotationView] }
pub type AnnotationView
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
pub fn Declaration.parameters(self): Array[ParameterView]
pub fn Declaration.result(self): TypeRef?
pub fn Declaration.annotations(self): Array[AnnotationView]

pub fn AnnotationView.name(self): String
pub fn AnnotationView.to[T: serialization.Decode](self): T ! Error

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
- Una función pública es una declaración `Function`: `parameters()` da sus
  parámetros y `result()` su resultado, con el `Result` o la `Option`
  completos; las de un tipo dan `[]` y `none`.
- Las anotaciones del programa (27.13 del lenguaje) de una declaración, un
  campo, una variante o un parámetro están en el orden en que se escriben, con
  todos sus campos, también los que toman su valor por defecto.
  `AnnotationView.name()` es su tipo como lo escribe el módulo de la
  declaración, `binding.Check`, y `to[T]()` su valor como `T`, un record del
  provider con los campos que lee, porque un provider no importa los módulos
  del programa: con `type Check = { required: Bool, min: Int? }`,
  `let check: Check = annotation.to()?`. Un `T` de otra forma falla con
  `Error.Provider`.
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
pub alias ErrorKind = serialization.ErrorKind
pub alias JsonError = serialization.Error
pub enum UnknownFields { Ignore, Reject }

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

- `Encode` y `Decode` son los de `std.serialization` (14.7), con la forma que
  allí se define: un record es un objeto, `none` es `null` y una variante con
  payload es un objeto de una clave. `Value`, `Number` y `Raw` los implementan
  también. Un `Reader` es un `Source` y un `Writer` es un `Sink`; los bytes se
  escriben como un string base64.
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
  recibe valores, claves y corchetes y `finish()` da el texto. Como `Source` y
  `Sink` son la base de `encode` y `decode`.
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

`std.messagepack` lee y escribe la
[especificación MessagePack](https://github.com/msgpack/msgpack/blob/master/spec.md)
binaria sobre un decodificador y un codificador del runtime.

~~~tondo
import std.messagepack

let bytes = messagepack.encode(packet)
let back: Packet = messagepack.decode(bytes)?
let document = messagepack.parse(bytes)?
~~~

La superficie es:

~~~tondo pseudocode
pub enum Value {
    Nil
    Bool(Bool)
    Int(Int)
    Float(Float)
    Text(String)
    Bytes(Array[Byte])
    Timestamp(String)       // RFC 3339 en UTC
    Array(Array[Value])
    Map(Map[String, Value])
}
pub alias MessagePackError = serialization.Error
pub alias UnknownFields = serialization.UnknownFields

pub fn decode[T: serialization.Decode](input: Array[Byte], unknownFields: UnknownFields = UnknownFields.Ignore): T ! MessagePackError
pub fn encode[T: serialization.Encode](value: T): Array[Byte]
pub fn parse(input: Array[Byte]): Value ! MessagePackError

pub type Writer                                   // un serialization.Sink
pub fn writer(): Writer
pub fn writerTo(stream: io.Writer): Writer
pub fn Writer.nil(self)
pub fn Writer.bool(self, value: Bool)
pub fn Writer.int(self, value: Int)
pub fn Writer.float(self, value: Float)
pub fn Writer.string(self, value: String)
pub fn Writer.bytes(self, value: Array[Byte])
pub fn Writer.beginArray(self, count: Int)
pub fn Writer.beginMap(self, count: Int)
pub fn Writer.value[T: serialization.Encode](self, value: T)
pub fn Writer.finish(self): Array[Byte]
pub fn Writer.flush(self): Unit ! MessagePackError

pub fn Value.isNil(self): Bool
pub fn Value.get(self, key: String): Value?
pub fn Value.at(self, index: Int): Value?
pub fn Value.find(self, path: String): Value?
pub fn Value.asText(self): String?
pub fn Value.asInt(self): Int?
pub fn Value.asFloat(self): Float?
pub fn Value.asBool(self): Bool?
pub fn Value.asBytes(self): Array[Byte]?
pub fn Value.asTimestamp(self): String?
pub fn Value.asArray(self): Array[Value]?
pub fn Value.asMap(self): Map[String, Value]?
impl Display for Value                     // como JSON
~~~

- El codificador usa la forma más corta de cada entero y longitud; el
  decodificador acepta todas las formas. Un `str` es texto y debe ser UTF-8; un
  `bin` son bytes (`serialization.Bytes` en un tipo); la extensión timestamp
  (-1) es un `DateTime`, que `time.DateTime` lee y escribe. Otra extensión es un
  error.
- Las claves de un mapa son texto, o enteros leídos como sus dígitos. Un entero
  sin signo que no cabe en `Int` es un `Number`: un `UInt` en un campo `UInt`,
  un error para un campo `Int`, un `Float` en `Value`. Un entero escrito con
  `number` es el entero del formato si cabe en 64 bits, con o sin signo; uno
  mayor no tiene forma en MessagePack, y `encode` hace pánico con lo
  ocurrido.
- Un `Encode` escrito a mano que declara otro número de elementos de los que
  escribe es un error de programa: `encode` hace pánico con lo ocurrido.
- Los errores de lectura llevan el camino y el byte donde ocurrieron.
- Un `Writer` escribe por partes: valores sueltos, la cabecera de un array o
  de un mapa con su número de elementos, que siguen, y con `value` cualquier
  valor con `Encode`. `finish` da los bytes escritos, y `writerTo` los entrega
  a un stream de `std.io` a medida que crecen, con `flush` al final. No
  comprueba que lleguen tantos elementos como dice una cabecera, así que un
  array puede escribirse por trozos: un envío que acumula sus elementos con
  `value` y solo al final sabe cuántos son escribe su cabecera en otro
  `Writer` y la pone delante.

### 14.12 `std.protobuf`

> **Aplazado.** Protobuf se diseñará junto con gRPC, que es para lo que la
> mayoría lo usa, cuando `std.http` exponga lo que gRPC necesita: trailers de
> HTTP/2, streaming en los dos sentidos y deadlines. Hasta entonces
> `import std.protobuf` es un error que lo dice, y el texto que sigue es el
> del proyecto anterior.

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

`std.yaml` lee y escribe un subset seguro y portable de
[YAML 1.2](https://yaml.org/spec/1.2.2/) sobre un parser y un emisor del
runtime. No reproduce los dialectos históricos de YAML ni convierte el parser en
una superficie de ejecución.

~~~tondo
import std.yaml

type Server = { host: String, port: Int, tls: Bool? }

let servers: Array[Server] = yaml.decode(text)?
let back = yaml.encode(servers)
let every = yaml.decodeAll[Server](stream)?
~~~

La superficie es:

~~~tondo pseudocode
pub enum Value {
    Null
    Bool(Bool)
    Int(Int)
    Float(Float)
    Text(String)
    Array(Array[Value])
    Map(Map[String, Value])
}
pub alias YamlError = serialization.Error
pub alias UnknownFields = serialization.UnknownFields

pub fn decode[T: serialization.Decode](text: String, unknownFields: UnknownFields = UnknownFields.Ignore): T ! YamlError
pub fn decodeAll[T: serialization.Decode](text: String, unknownFields: UnknownFields = UnknownFields.Ignore): Array[T] ! YamlError
pub fn encode[T: serialization.Encode](value: T): String
pub fn encodeAll[T: serialization.Encode](values: Array[T]): String
pub fn parse(text: String): Value ! YamlError
pub fn parseAll(text: String): Array[Value] ! YamlError

pub fn Value.isNull(self): Bool
pub fn Value.get(self, key: String): Value?
pub fn Value.at(self, index: Int): Value?
pub fn Value.find(self, path: String): Value?      // claves con puntos: `server.port`
pub fn Value.asText(self): String?
pub fn Value.asInt(self): Int?
pub fn Value.asFloat(self): Float?                 // también de un entero
pub fn Value.asBool(self): Bool?
pub fn Value.asArray(self): Array[Value]?
pub fn Value.asMap(self): Map[String, Value]?
impl Display for Value                             // como documento YAML
~~~

- El schema implícito es Core: `null`, `~` y un escalar vacío son null; `true` y
  `false` (también `True`, `TRUE`...) son los únicos booleanos; enteros
  decimales, `0o` y `0x`, y floats finitos. Cualquier otro escalar, y todo escalar
  entre comillas, es texto: `yes`, `on` y las fechas también. Un entero que no
  cabe en `Int` es un `Number`.
- Solo se aceptan los tags core (`!!str`, `!!int`, `!!float`, `!!bool`, `!!null`,
  `!!seq`, `!!map`); un tag propio es un error. Las claves son texto y no se
  repiten en un mapa; `<<` no tiene semántica de merge. `.inf` y `.nan` son un
  error, y un float no finito se escribe `null`.
- Un alias repite el valor de su ancla, que debe estar completa antes; un
  documento no puede superar el millón de valores con sus alias expandidos.
- `decode` exige exactamente un documento; `decodeAll` y `parseAll` leen todos
  los que separa `---`, y `encodeAll` los escribe así. Un texto que necesita
  comillas se escribe entre ellas, y uno de varias líneas como bloque `|`.
- Los errores llevan el camino, la línea y la columna del valor.

### 14.14 `std.toml`

`std.toml` lee y escribe documentos [TOML 1.1](https://toml.io/en/v1.1.0) UTF-8
para datos de aplicación, sobre un parser y un escritor del runtime. No es el
parser de `tondo.toml` del toolchain. Los comentarios se descartan y no hay
includes, interpolación de entorno ni ejecución.

~~~tondo
import std.toml

type Server = { host: String, port: Int, tls: Bool? }
type Config = { name: String, started: time.DateTime, servers: Array[Server] }

let config: Config = toml.decode(text)?
let back = toml.encode(config)?
let port = toml.parse(text)?.find("servers")
~~~

La superficie es:

~~~tondo pseudocode
pub enum Value {
    Text(String)
    Int(Int)
    Float(Float)
    Bool(Bool)
    DateTime(String)        // tal como se escribió: `1979-05-27T07:32:00Z`
    Array(Array[Value])
    Table(Map[String, Value])
}
pub alias TomlError = serialization.Error
pub alias UnknownFields = serialization.UnknownFields

pub fn decode[T: serialization.Decode](text: String, unknownFields: UnknownFields = UnknownFields.Ignore): T ! TomlError
pub fn encode[T: serialization.Encode](value: T): String ! TomlError
pub fn parse(text: String): Value ! TomlError

pub fn Value.get(self, key: String): Value?
pub fn Value.at(self, index: Int): Value?
pub fn Value.find(self, path: String): Value?      // claves con puntos: `server.port`
pub fn Value.asText(self): String?
pub fn Value.asInt(self): Int?
pub fn Value.asFloat(self): Float?                 // también de un entero
pub fn Value.asBool(self): Bool?
pub fn Value.asDateTime(self): String?
pub fn Value.asArray(self): Array[Value]?
pub fn Value.asTable(self): Map[String, Value]?
impl Display for Value                             // una tabla como documento
~~~

- Cualquier tipo con `serialization.Encode`/`Decode` se lee y se escribe con la
  forma de 14.7: un record es una tabla y un array de records un array de tablas
  (`[[name]]`); un mapa dentro de otra lista es una tabla inline. Las fechas y
  horas de TOML son eventos `DateTime`, que `time.DateTime` lee y escribe; una
  fecha local es medianoche UTC.
- Un documento es una tabla, así que `encode` solo escribe records y mapas. TOML
  no tiene null: un campo `none` se omite, y un `none` en otro lugar es un error
  `Unsupported`.
- Los enteros de TOML son de 64 bits con signo: uno que no cabe en `Int` es un
  error `Unsupported` al escribirlo, y un `Range` al leerlo en un tipo más
  estrecho.
- Los errores de lectura llevan el camino, la línea y la columna del valor; los de
  escritura, el camino.

### 14.15 `std.cbor`

`std.cbor` lee y escribe el modelo de datos de CBOR de
[RFC 8949](https://www.rfc-editor.org/rfc/rfc8949.html) sobre un decodificador y
un codificador del runtime. Una entrada es un único data item: el trailing data
es un error.

~~~tondo
import std.cbor

let bytes = cbor.encode(packet, deterministic: true)
let back: Packet = cbor.decode(bytes)?
~~~

La superficie es la de `std.messagepack` (14.11) con estos nombres:

~~~tondo pseudocode
pub enum Value {
    Null                    // también `undefined`
    Bool(Bool)
    Int(Int)
    Float(Float)
    Text(String)
    Bytes(Array[Byte])
    DateTime(String)        // tags 0 y 1, como RFC 3339 en UTC
    Array(Array[Value])
    Map(Map[String, Value])
}
pub alias CborError = serialization.Error
pub alias UnknownFields = serialization.UnknownFields

pub fn decode[T: serialization.Decode](input: Array[Byte], unknownFields: UnknownFields = UnknownFields.Ignore): T ! CborError
pub fn encode[T: serialization.Encode](value: T, deterministic: Bool = false): Array[Byte]
pub fn parse(input: Array[Byte]): Value ! CborError
// Value.isNull, get, at, find, asText, asInt, asFloat, asBool, asBytes,
// asDateTime, asArray y asMap, y `Display` como JSON
~~~

- Se leen los major types 0 a 7 con longitudes definidas e indefinidas; el texto
  debe ser UTF-8. Los tags 0 (texto RFC 3339) y 1 (segundos desde la época) son
  un `DateTime`; 2 y 3 son enteros, un `Number` cuando no caben en `Int`; cualquier otro tag se
  lee como el item que contiene. Los floats de 16, 32 y 64 bits se leen; un
  simple no asignado es un error. Un contenedor no puede anidarse más de 512
  niveles.
- Las claves de un mapa son texto, o enteros leídos como sus dígitos. Un entero
  que no cabe en `Int` es un `Number`. Un entero escrito con `number` es un
  entero del formato si cabe en 64 bits, con o sin signo, y uno mayor lleva el
  tag 2 o 3.
- `encode` escribe la serialización preferida: cabeceras y floats en su forma
  más corta que conserva el valor, NaN como `f97e00`, longitudes definidas y un
  `DateTime` como tag 0. Con `deterministic`, además, las claves de cada mapa
  siguen el orden de sus codificaciones (RFC 8949, 4.2) y no pueden repetirse.
- Un `Encode` escrito a mano que declara otro número de elementos de los que
  escribe hace que `encode` haga pánico con lo ocurrido.
- Los errores de lectura llevan el camino y el byte donde ocurrieron.

### 14.16 `std.regex`

`std.regex` es el motor de expresiones regulares Unicode de Tondo, no un
dialecto Perl, un parser de glob ni una API de matching sobre bytes arbitrarios.
Opera sobre `String` UTF-8 y fija las tablas Unicode en 16.0.0, la misma versión
que usa la especificación del lenguaje. No normaliza, no consulta locale y no
trata grapheme clusters como una unidad de matching.

La sintaxis es un subconjunto cerrado y regular: literales, clases/rangos,
propiedades `\p`/`\P`, escapes, anchors, alternation, grupos capturantes y no
capturantes, captures con nombre y quantifiers greedy/lazy acotados. `\d`,
`\s`, `\w` y `\b` tienen definiciones Unicode explícitas. Los flags `i`, `m`,
`s` y `U` pueden escribirse en el patrón, para todo lo que sigue (`(?i)abc`) o
para un grupo (`(?i:abc)`), con el significado de las opciones del mismo
nombre; con `-` se quitan (`(?-i)`). Backreferences, look-around, recursion,
conditionals, código embebido, includes, clases dependientes de locale, otros
flags y matching de bytes inválidos se rechazan con un error de compilación; no
se reinterpretan silenciosamente.

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
solapadas. `replaceWith` y `replaceAllWith` reemplazan con lo que una función
da para cada coincidencia, sin interpretar `$`, como `ReplaceAllStringFunc` de
Go. `Regex.escape(text)` da un patrón que encaja con ese texto literal.

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
pub fn Regex.replaceWith(self, input: String, replacement: fn(RegexMatch): String): String ! RegexError
pub fn Regex.replaceAllWith(self, input: String, replacement: fn(RegexMatch): String): String ! RegexError
pub fn Regex.escape(text: String): String
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
- Un provider o generador (27.7 del lenguaje) puede usar `v5`, `parse` y el
  resto, pero no `v4` ni `v7`, que dan otro valor cada vez: llamarlos es
  `E2108`.

### 14.18 `std.log`

`std.log` escribe eventos estructurados, una línea por evento, en texto o JSON.
El logger de la ejecución está en el contexto `log.Context` (11.16 del
lenguaje): las funciones del módulo escriben con él, así que nadie tiene que
pasarlo, y un logger también se crea y se pasa como cualquier valor.

~~~tondo
import std.log

let logger = log.Logger.new(level: log.Level.Debug)
logger.info("server started", ["port": "8080"])
let request = logger.withFields(["request": "{id}"])
request.warn("slow query", ["table": "users", "took": "{elapsed}"])
// 2026-10-23T09:12:03.5Z WARN slow query request=42 table=users took=1.2s
~~~

Con el contexto, `main` pone el logger y un middleware sus campos, y el
código de debajo solo escribe:

~~~tondo pseudocode
fn main() {
    with log.Context { logger: some(log.Logger.new(format: log.Format.Json)) }
    app.listen(port: 8080)
}

fn tagged(request: http.Request, next: http.Handler): http.Reply {
    with log.Context { logger: some(log.current().withFields(["path": request.target])) }
    next(request)
}

fn find(id: Int): User ! NotFound {
    log.debug("looking up", ["id": "{id}"])    // con path y los campos @log
    ...
}
~~~

La superficie es:

~~~tondo pseudocode
pub enum Level { Trace, Debug, Info, Warn, Error }
pub enum Format { Text, Json }
pub type Logger

pub fn Logger.new(output: io.Writer? = none, level: Level = Level.Info,
    format: Format = Format.Text, timestamps: Bool = true): Logger
pub fn Logger.withFields(self, fields: Map[String, String]): Logger
pub fn Logger.isEnabled(self, level: Level): Bool
pub fn Logger.log(self, level: Level, message: String, fields: Map[String, String] = [:])
pub fn Logger.trace(self, message: String, fields: Map[String, String] = [:])
pub fn Logger.debug(self, message: String, fields: Map[String, String] = [:])
pub fn Logger.info(self, message: String, fields: Map[String, String] = [:])
pub fn Logger.warn(self, message: String, fields: Map[String, String] = [:])
pub fn Logger.error(self, message: String, fields: Map[String, String] = [:])
pub fn Logger.flush(self): Unit ! io.IoError

pub context Context = {
    logger: Logger? = none
}

pub fn current(): Logger
pub fn trace(message: String, fields: Map[String, String] = [:])
pub fn debug(message: String, fields: Map[String, String] = [:])
pub fn info(message: String, fields: Map[String, String] = [:])
pub fn warn(message: String, fields: Map[String, String] = [:])
pub fn error(message: String, fields: Map[String, String] = [:])
~~~

- Sin `output`, el logger escribe en la salida de error. Escribe los eventos de
  su nivel o superiores; `isEnabled` evita construir campos costosos.
- `withFields` da el mismo logger con más campos en cada evento; los campos de
  la llamada se añaden detrás y ganan en una clave repetida.
- `current` es `Context.logger`, o sin él un logger como `Logger.new()`, que
  escribe en la salida de error desde `Info`; `trace`, `debug`, `info`, `warn`
  y `error` del módulo escriben con `current()`.
- Cada evento, de cualquier logger, lleva también los campos `@log` de los
  contextos de la tarea (11.16 del lenguaje): detrás de los del logger y antes
  de los de la llamada, que ganan en una clave repetida.
- `Text` escribe `tiempo NIVEL mensaje clave=valor ...`, con el valor entre
  comillas y escapado como JSON si está vacío o tiene espacios, `=`, comillas o
  saltos de línea. `Json` escribe un objeto por línea con `time`, `level`,
  `message` y los campos, en ese orden.
- El tiempo es `time.utcNow()` en RFC 3339; `timestamps: false` lo omite, para
  salidas que se comparan en tests.
- Cada evento es una sola escritura en el stream, así que las líneas de tareas
  distintas no se mezclan. Un stream que falla pierde el evento sin interrumpir
  el programa; `flush` devuelve el error.

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
pub fn htmlEscape(text: String): String
pub fn htmlUnescape(text: String): String
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
- `htmlEscape` escribe `&`, `<`, `>`, `"` y `'` como `&amp;`, `&lt;`, `&gt;`,
  `&#34;` y `&#39;`, así que el texto va seguro en el contenido o en un atributo
  entre comillas de HTML. `htmlUnescape` lee las referencias con nombre de HTML5
  y las numéricas, y deja como está lo que no es una.

### 14.21 `std.hash`

`std.hash` calcula digests, HMAC (RFC 2104) y hashes no criptográficos:

~~~tondo pseudocode
pub type Digest
pub fn Digest.bytes(self): Array[Byte]
pub fn Digest.hex(self): String

pub fn md5(data: Array[Byte]): Digest
pub fn sha1(data: Array[Byte]): Digest
pub fn sha256(data: Array[Byte]): Digest
pub fn sha384(data: Array[Byte]): Digest
pub fn sha512(data: Array[Byte]): Digest
pub fn hmacSha256(key: Array[Byte], data: Array[Byte]): Digest
pub fn hmacSha512(key: Array[Byte], data: Array[Byte]): Digest

pub enum Algorithm { Md5, Sha1, Sha256, Sha384, Sha512 }
pub type Hasher
pub fn hasher(algorithm: Algorithm): Hasher
pub fn hmac(algorithm: Algorithm, key: Array[Byte]): Hasher
pub fn Hasher.update(self, data: Array[Byte])
pub fn Hasher.finish(self): Digest

pub fn fnv32(data: Array[Byte]): UInt32
pub fn fnv32a(data: Array[Byte]): UInt32
pub fn fnv64(data: Array[Byte]): UInt
pub fn fnv64a(data: Array[Byte]): UInt
pub fn xxhash64(data: Array[Byte], seed: UInt = 0): UInt
pub fn murmur3(data: Array[Byte], seed: UInt32 = 0): UInt128
~~~

- `Display` de un `Digest` es su hexadecimal minúsculo:
  `hash.sha256(text.bytes())` se interpola directamente. MD5 y SHA-1 existen
  para los formatos y protocolos que los nombran, como UUIDv5, Git o un
  `Content-MD5`; para seguridad se usa SHA-256 o más.
- Un `Hasher` calcula un digest o un HMAC por partes, sin tener los datos
  juntos: `update` con cada parte y `finish` dan lo mismo que la función de
  una vez sobre su concatenación, y después puede seguir recibiendo partes. Es
  un handle, como un `Writer`: sus copias comparten el cálculo.
- `fnv32`, `fnv32a`, `fnv64` y `fnv64a` son FNV-1 y FNV-1a; `xxhash64` es
  XXH64; y `murmur3` es MurmurHash3 x64 de 128 bits, con su primera mitad en
  los 64 bits altos. Dan los valores de sus implementaciones de referencia,
  así que coinciden con los de otros lenguajes. Son para repartir, muestrear o
  identificar, nunca para seguridad.

### 14.22 `std.random`

`std.random` da valores pseudoaleatorios de un generador explícito; no hay
estado global:

~~~tondo pseudocode
pub type Random
pub fn Random.seeded(seed: Int): Random
pub fn Random.new(): Random
pub fn Random.int[T: Integer](mut self, range: Range[T]): T
pub fn Random.float(mut self): Float
pub fn Random.bool(mut self): Bool
pub fn Random.bytes(mut self, count: Int): Array[Byte]
pub fn Random.shuffled[T](mut self, items: Array[T]): Array[T]
pub fn Random.choice[T](mut self, items: Array[T]): T?
pub fn entropy(count: Int): Array[Byte]
~~~

- El generador es SplitMix64: la misma semilla da los mismos valores en
  cualquier máquina. `Random.new()` toma la semilla de la entropía del sistema.
- `int` da cada valor del rango con la misma probabilidad, sin sesgo, en
  cualquier entero: `dice.int(1..=6)`, o `ids.int(1u64..=UInt64.max)` para
  un id de 64 bits. Un rango vacío es un pánico. `float` da un valor de `[0, 1)` con 53 bits; `shuffled` es
  Fisher-Yates sobre una copia; `choice` da `none` sin elementos.
- El generador no sirve para secretos: claves, tokens y nonces usan
  `random.entropy(count)`, que lee la entropía del sistema.
- Un provider o generador (27.7 del lenguaje) puede usar `Random.seeded`, pero
  no `Random.new` ni `entropy`: llamarlos es `E2108`.

### 14.23 `std.csv`

`std.csv` lee y escribe valores separados por comas (RFC 4180) registro a
registro, así que un archivo de cualquier tamaño pasa sin cargarse entero:

~~~tondo
let input = fs.open("sales.csv", fs.OpenMode.Read)?.reader()
for record in csv.records(input) {
    let sale = record?
    total += Int.parse(sale.getOr("amount", "0"))?
}

let out = csv.Writer.new(fs.open("report.csv", fs.OpenMode.Write)?.writer())
out.write(["region", "total"])?
~~~

La superficie es:

~~~tondo pseudocode
pub enum CsvError {
    Malformed(Int, Int)            // línea y columna
    Unterminated(Int)              // línea donde empieza el campo
    FieldCount(Int, Int, Int)      // línea, campos de la cabecera y del registro
    Io(io.IoError)
}

pub type Rows                      // Iterator[Array[String] ! CsvError]
pub type Records                   // Iterator[Map[String, String] ! CsvError]
pub type Writer

pub fn rows(input: io.Reader, separator: Char = ','): Rows
pub fn records(input: io.Reader, separator: Char = ','): Records
pub fn parse(text: String, separator: Char = ','): Array[Array[String]] ! CsvError
pub fn parseRecords(text: String, separator: Char = ','): Array[Map[String, String]] ! CsvError
pub fn Writer.new(output: io.Writer, separator: Char = ','): Writer
pub fn Writer.write(self, fields: Array[String]): Unit ! io.IoError
pub fn format(rows: Array[Array[String]], separator: Char = ','): String
pub fn formatRecords(records: Array[Map[String, String]], separator: Char = ','): String
~~~

- `rows` y `records` leen un registro cada vez que se les pide; `parse` y
  `parseRecords` leen un texto entero con ellos. En `records` el primer
  registro es la cabecera, y cada registro es un mapa de sus nombres a sus
  campos, en el orden de la cabecera.
- Un campo entre comillas puede contener el separador, comillas dobladas
  (`""`) y saltos de línea. Se acepta `\n` o `\r\n` entre registros, se saltan
  las líneas vacías y se quita el BOM UTF-8 del principio.
- Una comilla dentro de un campo que no empieza con ella, o texto tras una
  comilla de cierre, es `Malformed`; un campo entre comillas que no se cierra,
  `Unterminated`; en `records`, un registro con otro número de campos que la
  cabecera, `FieldCount`. Tras un error, el iterador no da más registros.
- `Writer.write` y `format` escriben una línea terminada en `\n` por registro y
  ponen entre comillas el campo que contiene el separador, una comilla o un
  salto de línea. `formatRecords` escribe como cabecera los nombres del primer
  registro.

### 14.24 `std.http`

`std.http` es un cliente HTTP/1.1 y HTTP/2 sobre TLS. Una petición espera
como cualquier llamada (sección 9): la tarea no ocupa un worker mientras la red
trabaja, así que muchas tareas pueden tener peticiones en curso.

~~~tondo
import std.http

let user: User = http.get("https://api.example.com/users/42")?.json()?

let github = http.Client.new(
    baseUrl: "https://api.github.com",
    headers: ["Authorization": "Bearer {token}"],
    timeout: time.seconds(30),
)
let repo: Repo = github.get("/repos/acme/tondo", query: ["ref": "main"])?.json()?
github.post("/upload", body: http.Body.multipart([
    http.Part.field("title", "Report"),
    http.Part.file("report", path: "report.pdf"),
]))?

for event in github.post("/v1/chat", body: http.Body.json(prompt))?.events() {
    console.print(event?.data)
}
~~~

La superficie es:

~~~tondo pseudocode
pub enum HttpError {
    InvalidUrl(String)
    Connect(String)
    Timeout
    Status(Response)
    Body(String)
    Protocol(String)
}

pub type Client
pub fn Client.new(baseUrl: String = "", headers: Map[String, String] = [:],
    timeout: time.Duration? = none, connectTimeout: time.Duration? = none,
    redirects: Int = 10, socket: String? = none, cookies: Bool = false): Client
pub fn Client.get(self, url: String, query: Map[String, String] = [:],
    headers: Map[String, String] = [:], timeout: time.Duration? = none): Response ! HttpError
pub fn Client.head(...)                       // como get
pub fn Client.delete(...)                     // como get
pub fn Client.post(self, url: String, body: Body? = none, query: Map[String, String] = [:],
    headers: Map[String, String] = [:], timeout: time.Duration? = none): Response ! HttpError
pub fn Client.put(...)                        // como post
pub fn Client.patch(...)                      // como post
pub fn Client.request(self, method: String, url: String, body: Body? = none, ...): Response ! HttpError
pub fn get(url: String, ...): Response ! HttpError   // y head, delete, post, put, patch

pub type Body
pub fn Body.text(text: String, contentType: String = "text/plain; charset=utf-8"): Body
pub fn Body.bytes(bytes: Array[Byte], contentType: String = "application/octet-stream"): Body
pub fn Body.json[T: serialization.Encode](value: T): Body
pub fn Body.form(fields: Map[String, String]): Body
pub fn Body.multipart(parts: Array[Part]): Body
pub fn Body.stream(input: io.Reader, length: Int? = none,
    contentType: String = "application/octet-stream"): Body

pub type Part
pub fn Part.field(name: String, value: String): Part
pub fn Part.file(name: String, path: String, contentType: String = ""): Part
pub fn Part.bytes(name: String, bytes: Array[Byte], fileName: String, contentType: String = ""): Part
pub fn Part.stream(name: String, input: io.Reader, fileName: String,
    contentType: String = "", length: Int? = none): Part

pub type Response
pub fn Response.status(self): Int
pub fn Response.reason(self): String
pub fn Response.url(self): String
pub fn Response.version(self): String
pub fn Response.header(self, name: String): String?
pub fn Response.headerValues(self, name: String): Array[String]
pub fn Response.headers(self): Map[String, String]
pub fn Response.cookie(self, name: String): String?
pub fn Response.cookies(self): Map[String, String]
pub fn Response.text(self): String ! HttpError
pub fn Response.bytes(self): Array[Byte] ! HttpError
pub fn Response.json[T: serialization.Decode](self): T ! HttpError
pub fn Response.body(self): io.Reader
pub fn Response.lines(self): Lines            // Iterator[String ! HttpError]
pub fn Response.events(self): Events          // Iterator[Event ! HttpError]
pub fn Response.copyTo(self, output: io.Writer): Int ! HttpError
pub type Event = { event: String, data: String, id: String? }

pub enum Message { Text(String), Binary(Array[Byte]) }
pub type WebSocket                            // Iterator[Message ! HttpError]
pub fn WebSocket.connect(url: String, headers: Map[String, String] = [:],
    timeout: time.Duration? = none): WebSocket ! HttpError
pub fn WebSocket.send(self, message: Message): Unit ! HttpError
pub fn WebSocket.sendText(self, text: String): Unit ! HttpError
pub fn WebSocket.sendBytes(self, bytes: Array[Byte]): Unit ! HttpError
pub fn WebSocket.receive(self): Message? ! HttpError
pub fn WebSocket.close(self): Unit ! HttpError
~~~

- No hay cliente global. Las funciones del módulo abren una conexión por
  petición; un `Client` guarda sus conexiones para las siguientes, y su
  `baseUrl` y sus cabeceras van con cada petición, cuyas cabeceras ganan.
  `query` se añade a la URL codificado como en `encoding.urlEncode`. Con
  `socket`, cada petición va por ese socket Unix (14.4.6) en vez de por la red,
  y la URL solo da el path y la cabecera `Host`:
  `http.Client.new(baseUrl: "http://agent", socket: "/var/run/datadog/apm.socket")`.
- Con `cookies`, el cliente guarda las cookies que le dan las respuestas y las
  envía con las peticiones siguientes, como un navegador: a su dominio y su
  path, mientras no caducan, y las `Secure` solo por HTTPS. `Response.cookie` y
  `cookies` leen las que da una respuesta, decodificadas como las de
  `Request.cookie`.
- Una petición devuelve la respuesta cuando llega su cabecera. Un estado de
  400 o más es `HttpError.Status`, que lleva la respuesta para leer su cuerpo;
  así `http.get(url)?.json()?` es correcto sin comprobar el estado.
- El cuerpo llega mientras se lee, una sola vez: entero con `text`, `bytes` o
  `json`, que decodifica mientras llega; como `io.Reader` con `body`; por líneas
  con `lines`; como Server-Sent Events con `events`; o hacia un stream con
  `copyTo`. Una segunda lectura falla con `HttpError.Body`, que dice que se
  guarde lo que dio la primera. `Display` de una respuesta es su estado:
  `200 OK`.
- `text` y `lines` leen el texto en el `charset` de su `Content-Type`: UTF-8
  si no nombra ninguno, y Windows-1252 para `windows-1252` y para las
  etiquetas de Latin-1 y ASCII (`iso-8859-1`, `latin1`, `us-ascii`), como los
  navegadores. Con otro charset, o con bytes que no son el UTF-8 que esperan,
  fallan con `HttpError.Body`, que dice que se lean sus bytes con `bytes`.
  `events` lee UTF-8, como manda Server-Sent Events.
- `Body.json` usa `std.json`, cuyas implementaciones automáticas existen para
  los tipos del programa en cuanto este importa `std.http`. `Body.stream` y
  `Part.stream` leen el stream mientras envían; sin `length`, el cuerpo va por
  chunks. `Part.file` lee el archivo mientras envía, con su nombre y el content
  type de su extensión.
- La conexión negocia HTTP/2 por ALPN cuando el servidor lo ofrece, sigue hasta
  `redirects` redirecciones, descomprime gzip, deflate y brotli y respeta los
  proxies del entorno. TLS confía en los certificados raíz del sistema y, si el
  sistema no tiene ninguno, en los de Mozilla.
- `timeout` limita la petición entera, incluido el cuerpo; `connectTimeout`,
  solo la conexión. Una tarea cancelada abandona su petición.
- `WebSocket.connect` abre una conexión de RFC 6455, con TLS para `wss://`, y
  `timeout` limita su handshake. Una tarea puede enviar mientras otra recibe;
  los pings se contestan solos. `receive` da el siguiente mensaje de texto o
  binario, o `none` cuando la conexión se cerró, y recorrer el socket con `for`
  recibe hasta entonces. `Display` de un mensaje es su texto, o `3 bytes`.

~~~tondo
let socket = http.WebSocket.connect("wss://example.com/chat")?
socket.sendText("hola")?
for message in socket {
    match message? {
        http.Message.Text(text) => console.println(text)
        http.Message.Binary(bytes) => save(bytes)
    }
}
~~~
- `trust` nombra un archivo PEM con certificados en los que el cliente confía
  además de los del sistema, como los de la CA de una empresa o de desarrollo.
- Un provider o generador no puede importar `std.http` (`E2108`).

#### 14.24.1 Servidores

Un `App` reúne rutas, middleware y documentación; `listen` sirve hasta que su
tarea se cancela y `start` sirve en una tarea propia y devuelve el `Server`.
Cada petición se atiende en una tarea, así que un handler puede esperar sin
bloquear a los demás.

~~~tondo
var app = http.App.new()
app.get("/users/:id", (request) {
    let id = request.intParam("id")?            // 400 si no es un entero
    match users.find(id) {
        some(user) => http.Reply.json(user)
        none => fail http.Problem.notFound("no user {id}")
    }
})
app.post("/users", (request) {
    let input: NewUser = request.json()?        // 400 si no es un NewUser
    http.Reply.json(users.add(input), status: 201)
})
app.use((request, next) {
    let reply = next(request)?
    reply.header("x-served-by", "tondo")
})
app.files("/downloads", "public")               // GET y HEAD con rangos
app.embedded("/", Site)                         // los de un embed.Files (14.27)
app.uploads("/downloads", "public")             // PUT reanudable y POST multipart
app.listen(port: 8080)?
~~~

La superficie es:

~~~tondo pseudocode
pub type Problem = { status: Int, message: String }
pub fn Problem.new(status: Int, message: String): Problem
pub fn Problem.badRequest(message: String): Problem
pub fn Problem.unauthorized(message: String = "authentication is required"): Problem
pub fn Problem.forbidden(message: String = "this is not allowed"): Problem
pub fn Problem.notFound(message: String = "not found"): Problem
pub fn Problem.conflict(message: String): Problem
pub fn Problem.from[E: Display](error: E): Problem          // 500

pub type Request
pub fn Request.method(self): String
pub fn Request.path(self): String
pub fn Request.route(self): String?
pub fn Request.version(self): String
pub fn Request.remoteAddress(self): String
pub fn Request.clientIp(self): String
pub fn Request.cookie(self, name: String): String?
pub fn Request.cookies(self): Map[String, String]
pub fn Request.param(self, name: String): String
pub fn Request.intParam(self, name: String): Int ! Problem
pub fn Request.query(self, name: String): String?
pub fn Request.queryValues(self, name: String): Array[String]
pub fn Request.intQuery(self, name: String): Int? ! Problem
pub fn Request.header(self, name: String): String?
pub fn Request.headers(self): Map[String, String]
pub fn Request.text(self): String ! Problem
pub fn Request.bytes(self): Array[Byte] ! Problem
pub fn Request.json[T: serialization.Decode](self): T ! Problem
pub fn Request.form(self): Map[String, String] ! Problem
pub fn Request.uploads(self): Uploads                       // Iterator[Upload ! Problem]
pub fn Request.body(self): io.Reader
pub fn Request.saveTo(self, path: String): Int ! Problem
pub type Upload                                             // name, fileName, contentType
pub fn Upload.text(self): String ! Problem
pub fn Upload.bytes(self): Array[Byte] ! Problem
pub fn Upload.saveTo(self, path: String): Int ! Problem

pub type Reply
pub fn Reply.text(text: String, status: Int = 200): Reply
pub fn Reply.html(html: String, status: Int = 200): Reply
pub fn Reply.json[T: serialization.Encode](value: T, status: Int = 200): Reply
pub fn Reply.bytes(bytes: Array[Byte], contentType: String = "application/octet-stream", status: Int = 200): Reply
pub fn Reply.stream(input: io.Reader, contentType: String = "application/octet-stream", status: Int = 200): Reply
pub fn Reply.file(path: String, download: String? = none): Reply
pub fn Reply.events(events: Receiver[Event]): Reply
pub fn Reply.redirect(url: String, status: Int = 302): Reply
pub fn Reply.empty(status: Int = 204): Reply
pub fn Reply.notFound(): Reply
pub fn Reply.header(self, name: String, value: String): Reply
pub fn Reply.cookie(self, name: String, value: String, maxAge: time.Duration? = none,
    path: String = "/", domain: String = "", secure: Bool = false, httpOnly: Bool = true,
    sameSite: SameSite = SameSite.Lax): Reply
pub enum SameSite { Lax, Strict, None }
pub fn Reply.status(self): Int

pub type App
pub fn App.new(): App
pub fn App.get(mut self, path: String, handler: fn(Request): Reply ! Problem)   // y post, put, patch, delete
pub fn App.route(mut self, method: String, path: String, handler: fn(Request): Reply ! Problem, operation: String = "")
pub fn App.use(mut self, middleware: fn(Request, fn(Request): Reply ! Problem): Reply ! Problem)
pub fn App.trustProxies(mut self, networks: ...net.Network)
pub fn App.mount(mut self, prefix: String, other: App)
pub fn App.files(mut self, prefix: String, directory: String, listing: Bool = false)
pub fn App.embedded(mut self, prefix: String, files: embed.Files)
pub fn App.uploads(mut self, prefix: String, directory: String)
pub fn App.websocket(mut self, path: String, handler: fn(WebSocket): Unit ! HttpError)
pub fn App.openApi(mut self, path: String = "/openapi.json", title: String = "API", version: String = "1.0")
pub fn App.listen(self, port: Int = 8080, host: String = "0.0.0.0", tls: Tls? = none,
    socket: String? = none): Unit ! HttpError
pub fn App.start(self, port: Int = 0, host: String = "127.0.0.1", tls: Tls? = none,
    socket: String? = none): Server ! HttpError
pub type Server
pub fn Server.address(self): String
pub fn Server.url(self): String
pub fn Server.stop(self)
pub alias Tls = net.Tls
~~~

- Una ruta es un método y un path cuyos segmentos `:nombre` son parámetros y
  cuyo último puede ser `*nombre`, el resto del path. Entre las rutas que
  encajan gana la de más segmentos fijos, así que `/users/me` gana a
  `/users/:id`; `HEAD` usa la ruta `GET`. Un path sin ruta es `404`, y uno con
  rutas de otros métodos, `405` con `Allow`. Los parámetros llegan
  decodificados.
- `Request.route()` es la plantilla de la ruta que atiende la petición, como se
  registró y con el prefijo de `mount`: `/users/:id` para `GET /users/42`;
  `none` si ninguna la atiende. El servidor elige la ruta antes de llamar al
  middleware, así que este ya la ve antes de `next`. Es el nombre de la
  petición para un tracer o una métrica, que no crece con cada valor de un
  parámetro como el path.
- Con `socket`, `listen` y `start` atienden en ese socket Unix (14.4.6) en vez
  de en un puerto, y `Server.address()` es su path.
- `Request.clientIp()` es la dirección IP del cliente, sin puerto. Tras un
  proxy o un balanceador, la conexión llega de él, y la dirección del cliente
  viene en `X-Forwarded-For`, que cualquier cliente puede escribir. Por eso
  `App.trustProxies` nombra las redes de los proxies propios: cuando la
  conexión viene de una de ellas, la dirección es la última de
  `X-Forwarded-For` que no es de esas redes, o `X-Real-Ip`, como `ClientIP` de
  Gin. Sin `trustProxies` no se cree a ninguna cabecera, y es la dirección de
  la conexión.
- `Request.cookie(name)` y `cookies()` leen las cookies que envía el cliente, y
  `Reply.cookie` añade una a la respuesta, con `Set-Cookie`: `maxAge` cero la
  borra, y sin `maxAge` dura lo que la sesión del navegador; `httpOnly` la
  oculta al JavaScript de la página y `secure` la envía solo por HTTPS. El
  valor se escribe con `encoding.urlEncode` y se lee con `urlDecode`, como hace
  Gin, así que cualquier texto cabe en una cookie.
- Un handler devuelve una `Reply` o falla con un `Problem`, que el servidor
  envía como `application/problem+json` (RFC 9457). Con un estado de 500 o más
  el cliente solo ve `internal server error` y el mensaje va a la salida de
  error. Los helpers de `Request` fallan con un `Problem` 400 que explica qué
  falta, así que `?` basta para validar la entrada.
- El servidor ejecuta cada petición dentro de un `recover` (15.9 del
  lenguaje). Un handler o un middleware que entra en pánico responde `500`
  como un `Problem` de 500, el informe del pánico va a la salida de error con
  el método y el path de la petición en su `data`, y el servidor sigue
  atendiendo las demás. Los pánicos que no se recuperan (15.9) terminan el
  programa igual que en cualquier otro sitio. `Request.text` lee el
  `charset` como `Response.text`.
- El middleware rodea las rutas de su app y la respuesta a los paths sin ruta;
  el primero añadido es el exterior. `mount` añade las rutas de otra app bajo
  un prefijo, con el middleware de esa app solo alrededor de ellas.
- `Reply.file` envía un archivo con el content type de su extensión, `ETag` y
  `Last-Modified`. Responde a `Range` (también `bytes=-n`) con `206` y la parte
  pedida, así que una descarga se reanuda, a `If-None-Match` y
  `If-Modified-Since` con `304`, y a un rango fuera del archivo con `416`;
  `If-Range` mantiene el rango solo si el archivo no cambió. `download` lo
  envía como adjunto con ese nombre. `App.files` sirve así los archivos de una
  carpeta, sin dejar salir de ella un path con `..` (`403`). Con `listing`, el
  path de una carpeta responde con una página HTML de sus entradas, primero
  las carpetas, cada una con un enlace para abrirla y cada archivo con su
  tamaño y un enlace para descargarlo; sin `listing`, es `404`.
- `App.embedded` sirve igual los archivos de un `embed.Files` (14.27), desde la
  memoria del programa: el content type de su extensión, un `ETag` de su
  contenido, `304`, rangos y `416`. El path de una carpeta responde con su
  `index.html`, si lo tiene, y si no es `404`.
- `App.uploads` recibe archivos en una carpeta: `PUT` escribe el cuerpo en ese
  archivo, desde el byte que diga `Content-Range: bytes inicio-fin/total`, así
  que una subida se reanuda tras un `HEAD` que dice cuánto llegó; `POST` con un
  formulario multipart guarda cada parte con archivo bajo esa carpeta con su
  nombre. Ambos responden con el JSON de lo escrito (`path` y `size`), `201`
  para un archivo nuevo.
- `Reply.events` envía cada `Event` del canal como Server-Sent Events hasta que
  el canal se cierra o el cliente se va. `App.websocket` acepta WebSockets en
  un path y el handler habla con cada uno hasta que termina.
- `listen` y `start` sirven HTTP/1.1 y HTTP/2; con `tls`, sobre TLS con ALPN.
  `Server.stop` deja de aceptar conexiones y espera a que el servidor pare;
  las peticiones en curso terminan.

#### 14.24.2 Handlers tipados y OpenAPI

Un handler de `get`, `post`, `put`, `patch` o `delete` con path literal puede
tomar parámetros con tipo en vez de la petición. El compilador escribe el
adaptador que los lee y convierte el resultado en una respuesta, así que la
validación es la del tipo y sus errores son `400`:

~~~tondo
app.get("/users/:id", (id: Int) { users.find(id) })                 // User?: 404 sin valor
app.get("/users", (page: Int?, search: String) { users.list(page, search) })
app.post("/users", (input: NewUser) { users.add(input) })          // el cuerpo JSON
app.delete("/users/:id", (id: Int) { users.remove(id) })           // Unit: 204
app.get("/report", report)                                         // una función con nombre
app.openApi(title: "Users", version: "1.0")                        // GET /openapi.json
~~~

- Un parámetro de tipo `http.Request` es la petición. Uno que se llama como un
  segmento `:nombre` de la ruta es ese parámetro, `String` o `Int`. Uno
  `String` o `Int`, u opcional de ellos, es un parámetro de query: sin `?` es
  obligatorio. Cualquier otro tipo es el cuerpo JSON (`std.json`), uno por
  handler y nunca en `GET` ni en `DELETE`; un parámetro que no encaja es un
  error de compilación que dice de dónde se puede leer.
- El resultado se convierte: una `Reply` se envía tal cual; un `String`, como
  texto; `Unit`, como `204`; `T?`, como `T` o `404` sin valor; cualquier otro,
  como JSON. Un handler `T ! http.Problem` falla con su problema, y uno
  `T ! E` con otro error lo envía como `Problem.from(error)`, un `500`. Un
  closure sin tipo esperado declara su resultado para poder fallar:
  `(): String ! StoreError { ... }`.
- Cada ruta con path literal, tipada o no, lleva su operación OpenAPI 3.1:
  parámetros, cuerpo y respuestas, con los JSON Schemas de sus tipos tal como
  `std.json` los escribe. `App.openApi` sirve el documento de todas, también
  las de las apps montadas, en `path`.

### 14.25 `std.diagnostics`

`std.diagnostics` da al programa contadores propios, que `tondo counters`
muestra junto a los del runtime (7.5 del toolchain):

~~~tondo
import std.diagnostics

let served = diagnostics.counter("requests served")
served.add()
diagnostics.gauge("queue length").set(12)
~~~

La superficie es:

~~~tondo pseudocode
pub type Counter
pub type Gauge

pub fn counter(name: String): Counter
pub fn gauge(name: String): Gauge
pub fn Counter.add(self, amount: Int = 1)
pub fn Counter.value(self): Int
pub fn Gauge.set(self, value: Int)
pub fn Gauge.add(self, amount: Int)
pub fn Gauge.value(self): Int
~~~

- Un nombre da siempre el mismo contador, desde cualquier tarea, así que el
  programa no tiene que guardarlo ni pasarlo. Pedir con `counter` el nombre de
  un gauge, o al revés, y un nombre vacío son pánicos que dicen qué pedir.
- Un `Counter` solo crece, y `tondo counters` muestra su ritmo por segundo y
  su total; `add` con un número negativo es un pánico, porque lo que baja es
  un `Gauge`. Un `Gauge` es un nivel que sube y baja.
- Cambiar o leer un contador es una operación atómica, sin locks.

### 14.26 `std.profile`

`std.profile` deja que el programa haga perfiles de sí mismo, como los de
`tondo profile` (7.15 del toolchain): dónde gasta su tiempo de procesador, qué
asigna y retiene, y dónde están sus tareas; para guardarlos o enviarlos a un
servicio, como hace un profiler continuo. Y labels que los dividen:

~~~tondo
import std.fs
import std.profile

fn serve(request: Request): Response ! fs.FsError {
    let cpu = profile.startCpu()
    let response = profile.withLabels(["endpoint": request.path], () { handle(request) })
    fs.writeBytes("cpu.pprof", cpu.stop().pprof())?
    response
}
~~~

La superficie es:

~~~tondo pseudocode
pub type Profile
pub type CpuProfile

pub fn startCpu(): CpuProfile
pub fn CpuProfile.stop(self): Profile
pub fn startMemory()
pub fn memory(): Profile
pub fn tasks(): Profile
pub fn Profile.pprof(self): Array[Byte]
pub fn Profile.folded(self): String
pub fn withLabels[T: ~Copy](labels: Map[String, String], body: fn(): T): T
~~~

- `startCpu` muestrea dónde están los hilos del programa cada milisegundo de
  procesador, hasta `stop`; solo hay uno a la vez, y otro `startCpu` antes de
  `stop` es un pánico que lo dice. Sus muestras llevan la etiqueta `thread` y
  los labels de la tarea que corría.
- `startMemory` muestrea desde entonces lo que el programa asigna, una
  asignación por cada 512 KiB de media, como `TONDO_DIAGNOSTICS=1`; `memory`
  da la memoria viva y lo que asignó cada pila, como `tondo profile
  --memory`. Sin muestreo, `memory` es un pánico que dice cómo encenderlo.
- `tasks` da dónde está cada tarea, con la etiqueta `state`.
- `pprof` da el perfil en el formato de pprof, comprimido con gzip, con la
  función, el archivo y la línea de cada marco, los del runtime y los de C
  por su nombre; `folded`, una línea por pila, sus marcos desde el más externo
  unidos por `;` y cuánto tomó.
- `withLabels` ejecuta `body` con esos labels, además de los que la tarea ya
  tenía, que los del mismo nombre reemplazan; las tareas que lanza los
  heredan. Los labels son un contexto de `std.profile` (11.16 del lenguaje):
  `withLabels` lo cambia mientras corre `body`, y los campos `@label` de los
  contextos de la tarea se añaden a los suyos, como si se hubieran dado en un
  `withLabels`. Da lo que da `body`: un cuerpo que puede fallar da su resultado,
  que se propaga con `?`. Los perfiles de `std.profile`, los de `tondo
  profile` y los de `tondo test --profile` llevan los labels. El runtime
  guarda un número limitado de conjuntos de labels distintos, así que los que
  cambian en cada petición, como el de un span, no crecen sin fin: la muestra
  de un conjunto que ya se reemplazó no lleva labels.

### 14.27 `std.embed`

`std.embed` guarda en el ejecutable archivos del paquete, leídos al compilar:
páginas, plantillas, datos o un sitio entero, que el programa usa sin
buscarlos en el disco al ejecutar:

~~~tondo
import std.embed
import std.http

const Banner: String = embed.text("banner.txt")
const Logo: Array[Byte] = embed.bytes("static/logo.png")
const Site: embed.Files = embed.files("static")

fn main(): !http.HttpError {
    var app = http.App.new()
    app.get("/banner", (request) { http.Reply.text(Banner) })
    app.embedded("/", Site)
    app.listen(port: 8080)?
}
~~~

La superficie es:

~~~tondo pseudocode
pub type Files

pub fn text(path: String): String
pub fn bytes(path: String): Array[Byte]
pub fn files(path: String): Files
pub fn Files.read(self, path: String): Array[Byte]?
pub fn Files.text(self, path: String): String?
pub fn Files.paths(self): Array[String]
pub fn Files.list(self, directory: String): Array[String]
pub fn Files.tag(self, path: String): String?
~~~

- `text`, `bytes` y `files` se evalúan al compilar: su argumento es un
  literal de string y su resultado una constante (6.5 del lenguaje), que vale
  en una `const` y en cualquier expresión. El ejecutable lleva los bytes entre
  sus datos, sin copiarlos al arrancar ni al usarlos, y al ejecutar no lee el
  disco.
- La ruta es relativa al directorio del archivo que la escribe, con `/` en
  todos los sistemas, y no sale del paquete: del directorio de su
  `tondo.toml`, o del de un programa de un solo archivo.
- `text` lee un archivo UTF-8 y `bytes` cualquiera. `files` lee un directorio
  y todo lo que hay debajo, salvo los archivos y directorios cuyo nombre
  empieza por `.`.
- Un archivo que no existe, un directorio donde va un archivo o al revés, una
  ruta que sale del paquete, un archivo que no es UTF-8 en `text` y un
  argumento que no es un literal son `E1904`; el error dice qué hay en ese
  directorio y qué escribir.
- `Files.paths()` da las rutas de sus archivos, relativas al directorio
  embebido, con `/` y en orden. `read` y `text` dan el contenido de una de
  ellas, `none` si no está, y `text` también si no es UTF-8. `list` da lo que
  hay directamente en una carpeta, `""` la de arriba, con `/` al final de las
  subcarpetas. `tag` da un resumen del contenido de un archivo, el `ETag` con
  que `std.http` lo sirve.
- Los archivos embebidos son entradas del build: `tondo run`, `tondo test` y
  `--watch` compilan de nuevo cuando cambian (6 y 7.10 del toolchain).
- Un provider o un generador no embebe: lo que lee le llega como input (27.5
  del lenguaje), y `std.embed` es `E2108`.

### 14.28 `std.url`

`std.url` lee y escribe URLs y referencias relativas como las define RFC 3986:
las de un enlace, una redirección o una petición.

~~~tondo
import std.url

let page = url.parse("https://ada@example.com:8443/docs/a%20b?q=tondo&tag=x#intro")?
// page.scheme "https", page.host "example.com", page.port some(8443),
// page.path "/docs/a b", page.param("q") some("tondo")
let next = page.resolve("../img/logo.png?v=2")?   // https://ada@example.com:8443/img/logo.png?v=2
let moved = page with { path: "/v2{page.path}", query: url.encodeQuery(["q": "a b"]) }
~~~

La superficie es:

~~~tondo pseudocode
pub type Url = {
    scheme: String           // "https"; vacío en una referencia relativa
    user: String
    password: String?
    host: String             // sin corchetes en IPv6
    port: Int?
    path: String             // decodificado
    query: String            // como se escribe, sin `?`
    fragment: String         // decodificado, sin `#`
}
pub type UrlError = { message: String, offset: Int }

pub fn parse(text: String): Url ! UrlError
pub fn Url.isAbsolute(self): Bool
pub fn Url.resolve(self, reference: String): Url ! UrlError
pub fn Url.param(self, name: String): String?
pub fn Url.params(self, name: String): Array[String]
pub fn encodeQuery(values: Map[String, String]): String
impl Display for Url
~~~

- `parse` acepta una URL absoluta o una referencia relativa (`/docs`,
  `../a.png`, `?page=2`, `#top`). Decodifica `path` y `fragment`; `Display` los
  vuelve a codificar donde hace falta, así que un `Url` cambiado con `with`
  escribe una URL válida. Una URL que no se lee, como un puerto que no es un
  número o un esquema con un carácter que no admite, es `UrlError` con el
  offset de lo que falla.
- `resolve` da la URL de una referencia vista desde esta, como un navegador
  resuelve un enlace (RFC 3986, 5.2), con sus `.` y `..` quitados.
- `param` y `params` leen la query como un formulario, con `+` como espacio:
  el primer valor de un nombre, o todos. `encodeQuery` escribe una query así,
  en el orden del mapa.

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
- `Cell` y mutabilidad interior general; `WeakRef[T]` pertenece al prelude
  de la edición (26.3 del lenguaje).
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
  fallo del host cuenta como final de la entrada. Mientras espera la entrada
  del programa no ocupa un thread; si la tarea se cancela, deja de esperar al
  instante y lo que llegue queda para la siguiente lectura.
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
  llena, en cada línea si es una terminal o el programa es un build de
  depuración, antes de leer la entrada y al terminar el programa; un programa nunca necesita llamar a `flush`. La salida
  de error no se acumula. `flush` entrega lo pendiente de la salida actual y
  devuelve el primer fallo de entrega ocurrido desde el anterior `flush`.
- Un fallo de entrega que el programa no observe se reporta al terminar en
  stderr como `error: standard output: …` y el proceso termina con estado 1,
  salvo que ya termine con otro estado de error o pánico. Escribir en una pipe
  cuyo lector ya cerró termina el proceso sin mensaje, como cualquier
  herramienta del sistema; en un programa que ya usó la red (14.24), esa
  escritura falla con `Closed` en vez de terminarlo.

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
pub fn Process.signal(self, signal: Signal)

pub enum Signal { Interrupt, Terminate, Hangup, User1, User2 }
pub fn signals(wanted: ...Signal): Receiver[Signal]
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

Señales:

- `Interrupt` (Ctrl-C) y `Terminate` (`kill`, `docker stop`, Kubernetes)
  cancelan el programa sin que escriba nada (20.10 del lenguaje): un servidor
  deja de aceptar, sus tareas ejecutan sus `defer` y termina con `130` o
  `143`.
- `signals(...)` entrega en un canal las señales pedidas que llegan, en vez de
  su efecto: ya no cancelan ni terminan el programa, que decide qué hacer,
  como recargar su configuración con `Hangup`. Las que el canal no ha leído
  se juntan: dos iguales seguidas son una.
- `Process.signal` envía una señal a los procesos de un hijo; `cancel` sigue
  siendo la forma de terminarlo y esperarlo.

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
