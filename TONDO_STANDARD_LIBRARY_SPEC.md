# Tondo Standard Library: especificación base

**Línea de desarrollo de la librería:** `draft`

**Revisión del documento:** `draft`

**Estado:** borrador normativo; Tondo y STD-0.1 todavía no
se han publicado

**Edición de lenguaje compatible:** Tondo 0.1

**Última actualización:** 2026-07-29

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
19. [Migración desde el bootstrap](#19-migración-desde-el-bootstrap)
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
- La coexistencia temporal con el corpus bootstrap de regresión.

Esta revisión no fija todavía:

- Las declaraciones completas de cada módulo.
- El conjunto exacto de variantes y payloads de cada error.
- Los formatos concretos de `std.format`.
- Los métodos concretos de strings, colecciones e iteradores.
- Las declaraciones exhaustivas de consola, filesystem, environment, procesos,
  networking, concurrencia, calendario civil, codecs adicionales, regex, UUID
  y logging.
- Las declaraciones exhaustivas de cada operación de JSON, MessagePack y
  Protobuf; este documento sí fija sus owners, arquitectura y garantías comunes.
- La representación interna de ningún tipo.

Una firma ilustrativa, un ejemplo del lenguaje o una operación existente en el
bootstrap no se convierte en API de STD-0.1 hasta que su módulo cumpla la
[sección 15](#15-contrato-exigido-a-cada-módulo).

### 1.1 Objetivos

La Standard Library debe ser:

1. **Pequeña:** cada concepto tiene una superficie mínima y justificada.
2. **Regular:** las mismas reglas de error, ownership y async se aplican en
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
- Proporcionar reflection dinámica sobre valores, carga dinámica o una ABI FFI
  pública. La metadata estática y descriptiva de `std.reflect` no es una
  excepción abierta a esta regla.
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
- **Bootstrap** nombra la superficie provisional usada como corpus de regresión
  durante el desarrollo de Tondo 0.1.

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
| [`TONDO_LANGUAGE_SPEC.md`](./TONDO_LANGUAGE_SPEC.md) | Sintaxis, tipos, ownership, async, módulos, imports, prelude, intrinsics, `derive` y límites de reflection |
| [`TONDO_TOOLCHAIN_SPEC.md`](./TONDO_TOOLCHAIN_SPEC.md) | Manifiesto, lockfile, PackageId, target, capabilities, generación meta, interfaces, artefactos y unidades privilegiadas |
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
- Añadir o retirar `async` o `unsafe`.
- Cambiar una operación infallible por fallible o a la inversa.
- Añadir una variante a un enum que el consumidor puede comprobar
  exhaustivamente.
- Retirar una capability o un target anunciado.
- Hacer más débil una garantía pública de orden, atomicidad o complejidad.
- Hacer que código válido seleccione otra declaración por añadir un método.
- Cambiar ownership, capacidades estructurales u obligación terminal.
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
- Bytes de librería.
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
- Las operaciones que consultan o suspenden contra un proveedor monotónico
  requieren `clock`.
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
- No se añade un sufijo `Async`; `async` ya forma parte de la firma.
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
- Acrónimos como palabras: `Utf8Error`, `JsonValue`, `userId`.

Los módulos usan nombres completos cuando una abreviatura no sea universal. Una
abreviatura aceptada no crea también un alias largo.

### 6.3 Parámetros y configuración

Tondo no tiene overload por firma ni parámetros por defecto. La stdlib utiliza:

- Parámetros ordinarios para datos obligatorios.
- Parámetros nombrados cuando mejoran claridad sin cambiar la firma.
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
- Cumplir `Discard`.
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
- Toda obligación terminal.

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

- Es un owner afín cuando desaparecer silenciosamente perdería una obligación.
- Declara una operación terminal exacta.
- Puede utilizarse con `defer`.
- No depende del GC ni de un finalizador de usuario.
- No se oculta dentro de `Ref[T]`, COW o un valor `Copy`.
- Tiene cleanup defensivo de host para abortos del runtime, sin convertir ese
  fallback en semántica de éxito.

Cada tipo afín publica una sola forma terminal canónica para cada outcome. No
ofrece simultáneamente `close`, `dispose` y `release` como sinónimos. Una
operación como `cancel` solo cuenta como terminal si su firma y contrato consumen
el owner y completan todo cleanup requerido.

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
- Qué operación terminal cierra un cursor afín.

No materializa un array de forma oculta detrás de una API anunciada como lazy.

---

## 9. Sincronía, suspensión y cancelación

### 9.1 Suspensión visible

Una operación que puede suspenderse es `async fn`. No devuelve `Task`,
`Future` ni otro wrapper solo para representar async.

Una operación síncrona:

- Retorna sin punto de suspensión.
- No espera de forma oculta a una task Tondo.
- No ejecuta una callback async.
- No bloquea indefinidamente un worker cooperativo.

### 9.2 Una forma por operación

STD-0.1 no duplica automáticamente cada operación como `read` y `readAsync`.
El módulo elige una forma canónica según el efecto real:

- Cálculo y transformación de memoria: síncrono.
- Espera potencialmente no acotada de host: async.
- Construcción inerte de un plan: síncrona.
- Operación que solo consulta metadata ya materializada: síncrona.

Si una forma bloqueante es necesaria, su módulo, nombre y documentación hacen
visible esa decisión; no comparte nombre con una operación suspendible de
semántica diferente.

### 9.3 Scheduler y backpressure

Una API async de host:

- No bloquea el único worker cooperativo mientras espera I/O.
- Mantiene vivos sus argumentos y roots durante la suspensión.
- Respeta backpressure y límites finitos.
- No crea tasks detached.
- Publica los puntos de cancelación.
- Completa o limpia todo recurso antes de entregar su outcome terminal.

La implementación puede usar event loops, workers o primitivas del sistema
siempre que esos detalles no cambien el contrato.

### 9.4 Cancelación

Una operación cancelable documenta:

- En qué puntos observa la señal.
- Qué datos parciales pueden haberse emitido.
- Qué cleanup completa antes de regresar.
- Si el error previo, el pánico o la cancelación tiene prioridad.
- Si el caller puede reintentar de forma segura.

No se promete preempción de código CPU. La cancelación continúa siendo
cooperativa.

### 9.5 Timeouts y deadlines

Un timeout o deadline:

- Se recibe mediante tipo temporal estándar, no como entero sin unidad.
- Utiliza tiempo monotónico para duración operacional.
- No consulta calendario civil.
- No mezcla instantes de proveedores distintos.
- No se convierte en un pánico.
- Declara si incluye cola, suspensión, cleanup o solo la operación principal.

Una API no instala timeouts ambientales invisibles.

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
- `Array[Byte]` es una colección mutable ordinaria.
- `std.bytes.Bytes` es el blob binario inmutable y portable de la stdlib.
- `std.io` define los protocolos portables de lectura/escritura y sus errores;
  importar esos contratos no realiza I/O ni concede una capability.

Decodificar bytes exige encoding explícito y devuelve error ante input inválido.
No se realiza replacement decoding por defecto. Codificar texto produce bytes
exactos del encoding nombrado.

`Bytes` tiene un único propietario canónico en `std.bytes`; I/O, console,
filesystem, process y testing lo reutilizan.

### 10.3 Protocolos de I/O

`std.io` posee los contratos portables compartidos por console, filesystem y
process. El módulo define tipos, traits y errores; no abre streams ni concede
capabilities al importarse.

Sus APIs concretas deberán distinguir:

- Lectura parcial de lectura exacta.
- Escritura aceptada de escritura completamente drenada.
- EOF normal de fallo recuperable.
- Texto de bytes.
- Buffer poseído de préstamo temporal.
- Operación síncrona de punto de suspensión.

Un reader o writer concreto conserva el owner de su módulo y satisface los
protocolos mediante dispatch estático. STD-0.1 no exige type erasure, vtables ni
un stream dinámico común para almacenar implementaciones heterogéneas.

Los protocolos no prometen que toda fuente pueda seek, conocer su longitud,
repetir una lectura o conservar datos después de cancelar. Cada capacidad
adicional aparece como trait o método exacto, no como operación que falla
siempre para ciertos handles.

### 10.4 Paths

`std.path.Path` no es un alias de `String`. Debe poder representar paths nativos
que el host admite aunque no sean Unicode.

La separación es:

- `std.path`: representación y operaciones léxicas.
- `std.fs`: observación y mutación del filesystem.

Formatear un path para diagnóstico no garantiza una representación reversible.
Convertirlo a texto puede fallar o exigir una política explícita. Normalizar
léxicamente no consulta el filesystem, no resuelve enlaces y no afirma
canonicalidad física.

### 10.5 Formato

`Display` es el protocolo mínimo estático usado por interpolación. La stdlib
puede añadir formato controlado mediante `std.format`, pero:

- No introduce reflection general.
- No busca métodos por string.
- No consulta locale sin un valor explícito.
- No convierte console en motor de formato.
- No duplica interpolación con otra sintaxis.
- Distingue output humano de formatos machine-readable.

Una API de console recibe texto o bytes ya definidos por su contrato. La
decisión de añadir newline, separator, flushing o encoding pertenece a esa
operación y nunca es implícita por terminal.

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
- Si bloquea o suspende.
- Si su coste depende de bytes, scalars, elementos o profundidad.

Una garantía utiliza worst-case, amortizada o esperada de forma explícita. No
se degrada en una minor compatible.

Las APIs evitan allocation obligatoria cuando un préstamo o resultado opaco
estático conserva la ergonomía. Tampoco exponen buffers internos de forma que
rompa COW, ownership o seguridad.

Una API de transformación ofrece una ruta de streaming o escritura sobre
`std.io.Writer` cuando materializar el resultado completo no es inherente al
contrato. La operación cómoda que devuelve `Bytes` puede existir además, pero se
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

Los benchmarks registran hardware, OS, target, backend, toolchain, flags, corpus
y varianza. Una optimización se acepta solo con tests de equivalencia y sin una
regresión material no justificada en otra dimensión publicada. Los números
concretos pertenecen al contrato de cada módulo y al tracker; esta arquitectura
no inventa un umbral universal.

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
10. Documentación normativa y ejemplos ejecutables.
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
| `std.bytes` | Core | — | 0.1 | `Bytes`, builders y conversión binaria explícita |
| `std.io` | Core | — | 0.1 | Protocolos de lectura/escritura, buffers, EOF, partial I/O y errores portables |
| `std.math` | Core | — | 0.1 | Matemática escalar portable y semántica IEEE nombrada |
| `std.format` | Core | — | 0.1 | Formatting explícito sobre `Display`, sin reflection |
| `std.serialization` | Core | — | 0.1 | Traits estáticos, eventos estructurales y contratos compartidos de encode/decode |
| `std.reflect` | Core | — | 0.1 | Metadata de tipos retenida explícitamente, sin inspección dinámica de valores |
| `std.meta` | Build-only | — | 0.1 | Modelo semántico inmutable, requests y emisión para providers/generators |
| `std.encoding` | Core | — | 0.1 | Encodings binario-texto, incluidos Base64 y hexadecimal, con APIs materializadas y streaming |
| `std.json` | Core | — | 0.1 | JSON RFC 8259, APIs tipadas, streaming y árbol dinámico explícito |
| `std.messagepack` | Core | — | 0.1 | MessagePack tipado, streaming y árbol dinámico explícito |
| `std.protobuf` | Core + Build-only | — | 0.1 | Wire format y código schema-first generado desde `.proto` |
| `std.yaml` | Core | — | 0.1 | YAML seguro, tipado y streaming sobre `std.serialization`, con límites explícitos |
| `std.toml` | Core | — | 0.1 | TOML tipado y árbol dinámico explícito, preservando errores con spans |
| `std.cbor` | Core | — | 0.1 | CBOR tipado, streaming y modo determinista explícito |
| `std.time` | Core + gated | `clock` para proveedor | 0.1 | Duration, Instant, timers, deadlines, calendario civil y zonas horarias versionadas |
| `std.path` | Core | — | 0.1 | Paths nativos y operaciones puramente léxicas |
| `std.regex` | Core | — | 0.1 | Expresiones regulares Unicode con complejidad y límites declarados |
| `std.uuid` | Core + gated | `entropy` y/o `clock` para generación | 0.1 | UUID, parsing, formatting y generadores explícitos por versión |
| `std.channel` | Core | — | 0.1 | Canales tipados, cierre, backpressure y selección cancelable |
| `std.sync` | Core + gated | `threads` para operaciones cross-thread | 0.1 | Mutex, rwlock, condvar, semáforo y atomics con memoria explícita |
| `std.executor` | Core + gated | `threads` para pools bloqueantes | 0.1 | Configuración de ejecución, actores y bridge de trabajo bloqueante |
| `std.log` | Core + gated | `console`, `filesystem` o `network` por sink | 0.1 | Eventos estructurados, niveles, backpressure y sinks explícitos |
| `std.console` | Capability-gated | `console` | 0.1 | stdin, stdout, stderr, texto, bytes y flushing |
| `std.env` | Capability-gated | `environment` | 0.1 | Argumentos y environment runtime explícitos |
| `std.fs` | Capability-gated | `filesystem` | 0.1 | Filesystem, metadata, iteración y recursos de archivo |
| `std.process` | Capability-gated | `process` | 0.1 | Planes, procesos, pipes, status, output y cancelación |
| `std.net` | Capability-gated | `network` | 0.1 | Direcciones, DNS, sockets, streams, datagrams y frontera TLS |
| `std.testing` | Test-only | ninguna implícita | 0.1 | Control sellado y helpers portables del runner |

### 14.1 Reglas del catálogo

- No existe un módulo importable `std.core`; los nombres intrínsecos continúan
  en el prelude del lenguaje.
- `std.bytes.Bytes` es el propietario binario común. Ningún módulo define otro
  `Bytes`.
- `std.io` posee los protocolos y errores compartidos; los handles concretos y
  sus efectos pertenecen a módulos capability-gated.
- `std.path` no toca el filesystem.
- `std.time.Duration` es usable sin `clock`; consultar tiempo o suspenderse
  contra un proveedor requiere `clock`.
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
- `std.channel`, `std.sync` y `std.executor` reutilizan el único modelo async,
  de ownership y de memoria del lenguaje; ninguna API crea tasks desligadas o
  un segundo tipo de future.
- `std.time` separa estrictamente el reloj monotónico del calendario civil. Los
  datos de zona horaria son inputs versionados de la distribución, nunca una
  consulta ambiental durante compilación.
- `std.net` no concede red por import: cada target debe seleccionar `network` y
  cada operación conserva I/O, timeout y cancelación en su firma.
- `std.uuid` separa representación y parsing core de cualquier generador que
  requiera `entropy` o `clock`.
- `std.log` define eventos puros en core; cada sink declara sus capabilities y
  política de backpressure sin alterar silenciosamente el control del programa.
- Los argumentos de proceso pertenecen a `std.env`; el contrato bootstrap de
  `std.process.args()` se migrará sin compatibilidad implícita.
- `std.testing` solo existe en source sets de test y no forma parte de producción.
- Streams, canales, red y sincronización solo aparecen bajo sus propietarios
  canónicos; ningún módulo existente los introduce indirectamente.

### 14.2 Estado de esta revisión

El catálogo y sus propietarios son normativos. Las declaraciones exhaustivas de
las veintinueve superficies permanecen pendientes, salvo el núcleo sellado que
`TONDO_TESTING_SPEC.md` ya fija para `std.testing`. El bootstrap conserva su
propio contrato separado hasta completar la migración de la sección 19.

La implementación usa dos gates internos sin crear versiones distintas:

- **STD-0.1A / S1A** cierra intrinsics, `std.bytes`, `std.io`, `std.math`,
  `std.format`, `std.serialization`, `std.reflect`, `std.meta`, `std.json`,
  `std.messagepack`, `std.protobuf`, el sustrato monotónico de `std.time`,
  `std.path`, `std.console`, `std.env`, `std.fs`, `std.process` y
  `std.testing`. Es el corpus mínimo que debe existir antes del backend nativo.
- **STD-0.1B / S1** completa `std.encoding`, `std.yaml`, `std.toml`,
  `std.cbor`, calendario civil y zonas de `std.time`, `std.regex`, `std.uuid`,
  `std.channel`, `std.sync`, `std.executor`, `std.log` y `std.net` sobre VM y
  backend nativo.

S1A no es una release ni permite publicar una stdlib incompleta como 0.1.0.
Todos los módulos de ambas fases comparten una distribución, PackageId y
política de compatibilidad; Gate S1 fija sus hashes finales.

### 14.3 Arquitectura común de serialización

Hay dos rutas deliberadamente distintas:

- **Tipada:** un tipo concreto implementa `serialization.Serialize` y/o
  `serialization.Deserialize`; el compilador monomorfiza la llamada y el codec
  accede directamente a su estructura.
- **Dinámica explícita:** cada formato posee su propio enum `Value` para
  documentos cuyo schema no se conoce al compilar.

La ruta tipada no construye un `Value`, no consulta `std.reflect` y no busca
fields por string. La ruta dinámica no introduce `Any`: sus casos posibles son
un enum cerrado y específico del formato.

Todo codec ofrece tres niveles cuando el formato los admite:

1. una operación cómoda que transforma un valor completo a/desde `Bytes`;
2. un `Reader`/`Writer` incremental sobre `std.io`; y
3. una máquina de eventos o tokens para procesar documentos sin materializarlos.

Los tres niveles comparten parser, validador y encoder. Un helper materializado
no mantiene una segunda semántica. Las APIs de streaming soportan chunks
arbitrarios, incluida una secuencia UTF-8, varint o escalar dividida entre dos
chunks, y aplican backpressure del reader/writer.

Cada decoder acepta límites explícitos agrupados en un record: bytes de input,
profundidad, longitud de string/blob, elementos de colección y bytes
materializados. Los defaults estándar son finitos y versionados. Alcanzar un
límite devuelve un error nominal con clase, offset y path estructural; no
produce un pánico ni un valor parcial.

### 14.4 `std.serialization`

`std.serialization` posee los protocolos estáticos compartidos. Su shape lógico
es:

~~~tondo pseudocode
pub trait Serialize {
    fn serialize[E, S: Serializer[E]](
        self,
        var serializer: S,
    ): !E
}

pub trait Deserialize {
    fn deserialize[E, D: Deserializer[E]](
        var deserializer: D,
    ): Self !E
}
~~~

`Serializer[E]` y `Deserializer[E]` son protocolos estáticos de eventos
estructurales: escalares, bytes, secuencias, maps, records, fields, enums y
variants. No son trait objects y no borran el tipo de error del formato. El
contrato exhaustivo de sus operaciones se cerrará antes de implementar
`STD-SER-001`; la decisión normativa ya fijada es que un solo `impl` generado
puede alimentar varios formatos sin reflection.

El receiver `self` de `Serialize` es la observación inmutable ordinaria del
lenguaje: serializar no consume el valor. El serializer/deserializer sí se pasa
como `var` para que avance su estado sin boxing ni allocation por evento.

La distribución registra providers para:

~~~tondo pseudocode
import std.serialization

derive serialization.Serialize + serialization.Deserialize for User
~~~

El comportamiento derivado:

- visita fields en orden de declaración;
- utiliza el spelling declarado de cada field como nombre externo por defecto,
  incluidos fields privados cuya autorización concede `derive`;
- conserva discriminante y payload de enum de forma estructural;
- introduce bounds mínimos sobre parámetros genéricos;
- rechaza en compile time un field sin implementación requerida; y
- construye el valor solo después de validar todos sus componentes.

Renombrar, omitir, aplanar o transformar fields cambia un contrato de wire y no
se esconde en attributes generales. Se expresa con un `impl` manual o con un
tipo DTO explícito. Protobuf no utiliza este derive para inferir field numbers.

### 14.5 `std.reflect`

`std.reflect` implementa la reflection conservada de 27.10. Su núcleo lógico es:

~~~tondo pseudocode
pub fn typeInfo[T](): TypeInfo

pub type TypeInfo
pub type TypeId
pub enum TypeKind {
    Primitive
    Record
    Enum
    Newtype
    Tuple
    Union
    Function
    Applied
    Reference
    Opaque
}
~~~

`TypeInfo` permite consultar nombre calificado, `TypeId`, kind, argumentos
genéricos, fields públicos, variants públicas y capabilities demostradas. Los
descriptores secundarios contienen únicamente nombres, tipos y posición
declarativa; nunca contienen getters, setters, constructores o function
handles.

No hay enumeración global de tipos. `typeInfo[T]()` retiene solo metadata
alcanzable para `T`; el linker puede eliminar el resto. `TypeId` solo compara
identidad dentro del mismo artefacto exacto y no es estable para persistencia,
red, schemas ni caches externas.

Los usos previstos son diagnostics de aplicación, documentación y herramientas
que necesitan describir un tipo conocido estáticamente. JSON, MessagePack y
Protobuf no dependen de este módulo.

### 14.6 `std.meta`

`std.meta` solo está presente en `target = tondo-meta`. Define los valores
inmutables de `GenerateRequest`, `DeriveRequest`, `GenerateResponse`,
`DeriveResponse`, modelo semántico, diagnostics y outputs descritos por el
toolchain.

Además ofrece:

- recorrido canónico de la clausura de roots autorizada: módulos,
  declarations, fields y variants;
- renderizado canónico de identidades, tipos, strings y literals;
- un builder de fuente que maneja escaping e indentación; y
- asociación explícita entre spans generados e inputs de origen.

No ofrece filesystem, environment, process, clock, entropy, red, compiler
callbacks ni mutación del AST que produjo el modelo. Crear un documento fuente
es construir un valor nuevo; el toolchain decide si lo admite, lo formatea y lo
compila.

### 14.7 `std.json`

`std.json` implementa JSON UTF-8 conforme a
[RFC 8259](https://www.rfc-editor.org/rfc/rfc8259.html). Su superficie contiene:

- encode/decode tipado mediante `Serialize` y `Deserialize`;
- `JsonReader`, `JsonWriter` y eventos incrementales;
- `JsonValue` para uso sin schema;
- `JsonNumber`, que conserva una representación numérica validada sin forzar
  pérdida inmediata a `Float64`; y
- opciones nominales de límites, campos desconocidos, duplicados y números no
  representables.

El modo estricto por defecto rechaza trailing data, UTF-8 inválido, escapes
inválidos, profundidad excedida, claves duplicadas y números fuera de la
política solicitada. Ignorar campos desconocidos o elegir una política distinta
requiere `DecodeOptions` explícito; no depende de globals.

El encoder ordinario conserva el orden declarativo de un tipo y el orden de
inserción de `JsonValue.Object`. `encodeCanonical` implementa
[RFC 8785 (JCS)](https://www.rfc-editor.org/rfc/rfc8785.html):
ordena properties según esa norma, usa su serialización de strings y números y
emite UTF-8 sin whitespace. Como JCS restringe el dominio a I-JSON, la operación
devuelve un error si un `JsonNumber` no puede representarse en ese dominio sin
cambiar su valor; nunca redondea silenciosamente. La operación normal no promete
que whitespace o spelling coincidan con el input.

Un error contiene clase estable, byte offset, línea/columna cuando puedan
calcularse y path estructural. No copia automáticamente el documento, el valor
de un field ni datos potencialmente secretos dentro de su mensaje.

### 14.8 `std.messagepack`

`std.messagepack` implementa la
[especificación MessagePack](https://github.com/msgpack/msgpack/blob/master/spec.md)
binaria completa: nil, booleanos, enteros signed/unsigned, floats, strings
UTF-8, binary, arrays, maps y extension values. Un objeto `str` con bytes UTF-8
inválidos produce error; `bin` conserva cualquier secuencia de bytes.

La ruta tipada comparte `Serialize`/`Deserialize` y escribe directamente al
buffer o writer. `MessagePackValue` representa uso dinámico. Como una key
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

### 14.9 `std.protobuf`

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

### 14.10 Reglas de rendimiento de codecs

Los tres codecs:

- decodifican tipos estáticos directamente, sin DOM intermedio;
- escriben a buffers crecientes con reservas comprobadas o a `Writer`;
- utilizan vistas prestadas para tokens o bytes cuando su vida es local y
  segura;
- evitan transcoding cuando input y output ya son UTF-8 válido;
- mantienen fast paths para ASCII, búsqueda de caracteres especiales, varints y
  copias de bytes;
- pueden usar SIMD o kernels nativos bajo las reglas de 11.6; y
- demuestran equivalencia mediante vectores oficiales, fuzzing diferencial,
  round trips, corpus adversario y comparación scalar/optimized.

Los benchmarks separan parse, validate, typed decode, dynamic decode, encode,
streaming y allocation. También miden profundidad hostil, strings con escapes,
maps grandes, unknown fields y chunks mínimos; optimizar el happy path no puede
convertir un input adversario en consumo no acotado.

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

- Síncrona o async.
- Puntos de suspensión.
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

- Ejemplos ejecutables.
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

Una API async prueba además:

- Suspensión real.
- Progreso de otras tasks.
- Cancelación.
- Pánico y error durante cleanup.
- Roots a través de `await`.
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
- Conserva el corpus bootstrap Tondo 0.1 sin mutarlo.
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
- Streams async generales o reactivos; los protocolos acotados de byte I/O de
  `std.io`, sockets de `std.net` y canales de `std.channel` sí pertenecen a
  0.1.
- Deque y priority queues.
- Decimal, BigInt y Complex.
- Matrices, tensors y álgebra lineal.
- `WeakRef`, `Cell` y mutabilidad interior general.
- Dynamic linking y plugins.
- FFI pública, layouts y calling conventions.
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

## 19. Migración desde el bootstrap

El corpus bootstrap de regresión utiliza:

~~~text
PackageId = toolchain:std:0.1-bootstrap
target    = tondo-vm-hosted
profile   = hosted
caps      = [console, process]
~~~

Esos bytes y su suite de conformidad se conservan como evidencia reproducible
del desarrollo, pero no constituyen una release pública de Tondo ni una segunda
línea de la librería.

### 19.1 `std.console`

El bootstrap expone exactamente:

~~~tondo pseudocode
fn print(value: String)
~~~

Es infallible, no añade newline y solo escribe stdout. STD-0.1 deberá fijar
texto, bytes, stdin/stdout/stderr, flushing, error, suspensión y terminal
independiente antes de decidir si conserva esa firma.

No existe obligación de compatibilidad entre el shim provisional y 0.1.0 de la
stdlib. La migración sí debe ser explícita, documentada y comprobable.

### 19.2 `std.process`

El bootstrap expone una superficie cerrada de procesos, `Bytes` opaco y
`process.args()`. STD-0.1:

- Conserva `Command` y `Pipeline` como planes intrínsecos.
- Reutiliza el comportamiento probado cuando siga siendo correcto.
- Migra el owner binario a `std.bytes.Bytes`.
- Migra argumentos runtime a `std.env`.
- Sustituye errores y tipos provisionales por propietarios canónicos.
- Mantiene argv exacto, shell explícito, pipes, backpressure y cleanup.

El corpus bootstrap no se reescribe. Un proyecto adopta STD-0.1
seleccionando el nuevo PackageId y lockfile.

### 19.3 Implementación transitoria

Durante la migración, el compilador puede conservar opcodes o bridges bootstrap
internos. No puede exponer simultáneamente dos identidades públicas ni afirmar
conformidad STD-0.1 hasta que la nueva interfaz completa sea la seleccionada.

---

## 20. Checklist de publicación

STD-0.1 puede publicarse como 0.1.0 únicamente cuando:

- [ ] Cada superficie del catálogo tiene firmas exactas.
- [ ] Prelude y namespace permanecen coherentes con el lenguaje.
- [ ] PackageId, content hash y API hash son finales.
- [ ] La matriz edición/target/perfil/capabilities está versionada.
- [ ] Todas las unidades privilegiadas están fijadas por hash.
- [ ] No existe búsqueda ambiental o de red durante compilación.
- [ ] Cada API documenta errores, pánicos, ownership, async, orden y coste.
- [ ] Todas las capabilities ausentes producen rechazo estático.
- [ ] Los tipos y errores tienen un propietario canónico.
- [ ] Los módulos core no dependen de host.
- [ ] Los recursos afines tienen una operación terminal y cleanup probado.
- [ ] Los ejemplos son ejecutables.
- [ ] La matriz de evidencia cubre las seis dimensiones.
- [ ] La suite estándar pasa sobre la VM.
- [ ] `std.testing` se prueba mediante `tondo test`.
- [ ] Los programas representativos de texto, colecciones, archivos y procesos
      pasan el gate estricto.
- [ ] La distribución es reproducible byte a byte.
- [ ] Providers y generators son herméticos, deterministas y están fijados.
- [ ] JSON, MessagePack y Protobuf pasan interoperabilidad, fuzzing, streaming y
      límites.
- [ ] Cada hot path tiene oracle escalar, equivalencia de kernels y gate de
      rendimiento multidimensional.
- [ ] El corpus bootstrap Tondo 0.1 permanece verificable sin cambios.
- [ ] Todo lo diferido está ausente o identificado fuera de `std`.

Hasta cerrar esta lista, el documento puede ser normativo para las reglas base
sin que exista una release pública completa de STD-0.1.
