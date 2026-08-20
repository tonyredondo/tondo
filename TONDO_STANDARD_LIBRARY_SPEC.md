# Tondo Standard Library: especificación base

**Línea de desarrollo de la librería:** `draft`

**Revisión del documento:** `draft.3`

**Estado:** borrador normativo; Tondo y STD-0.1 todavía no
se han publicado

**Edición de lenguaje compatible:** Tondo 0.1

**Última actualización:** 2026-08-07

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
- Las variantes culturales o dependientes de locale de `std.format`; el
  formatting estático sobre `Display`, sus builders y sus límites están fijados
  en `docs/contracts/stdlib-core.md`.
- Los métodos concretos de strings, colecciones e iteradores.
- Las declaraciones exhaustivas de consola, filesystem, procesos,
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
2. **Regular:** las mismas reglas de error, ownership y suspendible se aplican en
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
| [`TONDO_LANGUAGE_SPEC.md`](./TONDO_LANGUAGE_SPEC.md) | Sintaxis, tipos, ownership, suspendible, módulos, imports, prelude, intrinsics, `derive` y límites de reflection |
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
- Añadir o retirar `suspendible` o `unsafe`.
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
- No se añade un sufijo `Async`; `suspendible` ya forma parte de la firma.
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

Todas las operaciones se declaran con `fn`. El compilador infiere un efecto de
suspensión cuando el cuerpo llama a una operación `suspends`, usa `await`, itera
un `AsyncIterator` o registra cleanup suspendible. Las llamadas suspendibles
ordinarias esperan automáticamente y devuelven el resultado lógico; no crean un
wrapper `Task`/`Future` ni una segunda API.

La interfaz pública imprime el efecto como `suspends` después del outcome y lo
incluye en el hash ABI. Una declaración sin cuerpo debe escribirlo; una
implementación con cuerpo puede declararlo o dejar que el compilador lo infiera.
El marcador explícito fija la promesa pública aunque la ruta actual complete de
inmediato. `@sync`/`@nosuspend` garantiza que una función no suspende, es
incompatible con `suspends` y rechaza cualquier llamada suspendible, incluso
cuando aparece sin `await`.

Una operación síncrona:

- Retorna sin punto de suspensión.
- No espera de forma oculta a una task Tondo.
- No ejecuta una callback suspendible.
- No bloquea indefinidamente un worker cooperativo.

### 9.2 Una forma por operación

STD-0.1 no duplica automáticamente cada operación como `read` y `readAsync`.
El módulo elige una forma canónica según el efecto real:

- Cálculo y transformación de memoria: síncrono.
- Espera potencialmente no acotada de host: función suspendible, esperada
  automáticamente en la forma ordinaria; `await` queda disponible como spelling
  explícito y es obligatorio para consumir un handle pendiente.
- Construcción inerte de un plan: síncrona.
- Operación que solo consulta metadata ya materializada: síncrona.

Si una forma bloqueante es necesaria, su módulo, nombre y documentación hacen
visible esa decisión; no comparte nombre con una operación suspendible de
semántica diferente.

### 9.3 `spawn`, `Join` y `oneshot`

`spawn call()` devuelve un `Join[T, E]` afín; `spawn thread call()` solicita un
thread del sistema operativo. Ambos se consumen con `await handle` y están
sujetos a ownership estructurado: un scope debe esperar, cancelar, detach o
transferir cada handle. `cancel` solicita cancelación y el `Join` aún requiere
`await handle`; `detach`
consume el handle y lo entrega a un supervisor, sin permitir préstamos locales.

`std.async.oneshot[T, E]` divide una operación en `Waiter` y `Completer`. El
waiter se consume una vez; `Waiter.wait()` es una llamada suspendible ordinaria
y por tanto espera implícitamente, mientras que un valor `Waiter` o `Join` sin
consumir solo puede convertirse en resultado mediante `await handle`. Completar,
fallar o cancelar es atómico y una segunda finalización produce
`AlreadyCompleted`. No hay callbacks ni scheduler implícito.

`AsyncIterator[T]` y `for item in source` cubren streams lazy con backpressure
cuando la fuente no tiene un iterador síncrono. Cada elemento espera un `next`,
el efecto se infiere y el cierre ocurre al salir. `for await item in source`
permanece como spelling explícito opcional para desambiguar una fuente que
implementa ambos protocolos. La materialización solo ocurre mediante
`collect(limit:)`, con un límite finito y sin publicar arrays parciales. La
adaptación de `std.channel.Channel` pertenece a STD-0.1B y no es una
dependencia de esta superficie A.

La superficie nominal mínima de `std.async` es:

~~~tondo pseudocode
pub type Join[T, E]
pub type Waiter[T, E]
pub type Completer[T, E]
pub type AlreadyCompleted

pub fn oneshot[T, E](): (Waiter[T, E], Completer[T, E])
pub fn Waiter.wait(var self): T ! E suspends
pub fn Completer.complete(var self, value: T): Unit ! AlreadyCompleted
pub fn Completer.fail(var self, error: E): Unit ! AlreadyCompleted
pub fn Completer.cancel(var self): Unit ! AlreadyCompleted

pub trait AsyncIterator[T] {
    fn next(mut self): T? suspends
}

pub fn AsyncIterator.collect[T](var self, limit: Int): Array[T] ! CollectionError suspends
~~~

`Join` no expone constructor, poller ni callback: solo nace de `spawn` y se
consume con `await`. `Waiter.wait` es la operación suspendible de la pareja y
se espera implícitamente en una llamada ordinaria;
`Completer` puede completarse desde otro task o thread que cumpla `Send`. La
segunda finalización no cambia el resultado de la primera y devuelve
`AlreadyCompleted` de forma atómica.

La superficie ejecutable y sus siete requisitos verificables están indexados en
[`testing/stdlib-async.json`](./testing/stdlib-async.json) y el documento
normativo fuente es [`docs/contracts/stdlib-async.md`](./docs/contracts/stdlib-async.md).

### 9.4 Scheduler y backpressure

Una API suspendible de host:

- No bloquea el único worker cooperativo mientras espera I/O.
- Mantiene vivos sus argumentos y roots durante la suspensión.
- Respeta backpressure y límites finitos.
- No crea tasks detached; el caller usa `spawn` si necesita concurrencia.
- Publica los puntos de cancelación.
- Completa o limpia todo recurso antes de entregar su outcome terminal.

La implementación puede usar event loops, workers o primitivas del sistema
siempre que esos detalles no cambien el contrato.

### 9.5 Cancelación

Una operación cancelable documenta:

- En qué puntos observa la señal.
- Qué datos parciales pueden haberse emitido.
- Qué cleanup completa antes de regresar.
- Si el error previo, el pánico o la cancelación tiene prioridad.
- Si el caller puede reintentar de forma segura.

No se promete preempción de código CPU. La cancelación continúa siendo
cooperativa.

### 9.6 Timeouts y deadlines

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

La frontera general de decodificación exige un encoding explícito y devuelve
error ante input inválido. `String(Bytes)` es la conversión explícita y
canónica del encoding UTF-8; tampoco realiza replacement decoding. Los demás
encodings se seleccionan por nombre en `std.encoding` y codificar texto produce
los bytes exactos del encoding elegido.

`Bytes` tiene un único propietario canónico en `std.bytes`; I/O, console,
filesystem, process y testing lo reutilizan.

#### 10.2.1 Contrato cerrado de `std.bytes`

`std.bytes` es la única identidad binaria de STD-0.1. El módulo no es un alias de
`Array[Byte]`: `Bytes` es un valor opaco inmutable, copiable y transferible. Cada
conversión que materializa almacenamiento devuelve una copia independiente y
ninguna operación expone el buffer interno. El builder es el único estado mutable
de esta API y solo puede cambiar mediante un receptor `var self`.

La superficie mínima y canónica es:

```tondo
import std.bytes

fn bytes.empty(): Bytes ! BytesError
fn bytes.fromArray(value: Array[Byte]): Bytes ! BytesError
fn bytes.builder(): BytesBuilder ! BytesError

// Conversiones explícitas del lenguaje; el tipo `Bytes` pertenece a este módulo.
fn Bytes(value: String): Bytes ! BytesError
fn String(value: Bytes): String ! Utf8Error

fn Bytes.length(self): Int
fn Bytes.get(self, index: Int): Byte?
fn Bytes.slice(self, start: Int, end: Int): Bytes ! BytesError
fn Bytes.toArray(self): Array[Byte] ! BytesError
fn Bytes.equal(self, other: Bytes): Bool
fn Bytes.hash(self): UInt64

fn BytesBuilder.length(self): Int
fn BytesBuilder.appendByte(var self, value: Byte): Unit ! BytesError
fn BytesBuilder.append(var self, value: Bytes): Unit ! BytesError
fn BytesBuilder.appendArray(var self, value: Array[Byte]): Unit ! BytesError
fn BytesBuilder.finish(var self): Bytes ! BytesError
```

`Bytes(value)` codifica los bytes UTF-8 ya válidos de `String`; no hace una
segunda validación ni reemplaza caracteres. `String(value)` valida siempre y
devuelve `Utf8Error` sin producir un `String` parcial. `get` es total y devuelve `none`
fuera de rango; `slice` usa el intervalo semiabierto `[start, end)` y devuelve
`BytesError` si los límites no son no negativos o no satisfacen
`start <= end <= length`. `toArray` copia cada elemento. `equal` compara bytes
en orden y `hash` usa FNV-1a de 64 bits, fijado por esta edición para que dos
valores iguales tengan siempre el mismo hash.

`BytesBuilder` comienza vacío. `appendByte`, `append` y `appendArray` son
atómicos: si el límite de bytes del run se excedería, devuelven `BytesError` y
no cambian el builder. `finish` toma una instantánea independiente; el builder
puede seguir utilizándose. El límite publicado por el host se aplica a cada
buffer materializado y nunca permite una reserva silenciosa por encima de
`ResourceLimits.max_vm_heap_bytes`.

`BytesError` es opaco y pertenece a `std.bytes`; sus variantes internas (límite,
rango o elemento inválido) no forman parte de la identidad binaria y pueden
ampliarse sin romper código que solo propaga el error. La API no define
conversiones implícitas, `replacement decoding`, alias mutables ni una segunda
implementación de bytes en `std.process`. `Bytes(value)` y `String(value)` son
las únicas conversiones públicas entre texto y bytes.

La implementación de referencia mantiene una ruta escalar que sirve de oracle.
Una ruta SIMD, de palabra ancha o específica del target puede acelerar copias,
comparación y hashing siempre que preserve exactamente el resultado, el orden,
los errores, los límites y el número observable de operaciones de ownership.

La evidencia executable de este contrato vive en
[`testing/stdlib-bytes.json`](testing/stdlib-bytes.json) y en el registro por
celdas [`testing/stdlib-owner-evidence.json`](testing/stdlib-owner-evidence.json)
(`STD-A-BYTES-EVIDENCE-001`). Sus seis dimensiones cubren forma del catálogo,
ownership/snapshots, UTF-8, atomicidad de builders, límites/rangos y
properties/hot paths. `HOST` es explícitamente `not-applicable` porque el
owner es un intrinsic del compilador/VM, sin provider separado; `STD-A-FUZZ-001`
promueve el fuzz owner-aware y la captura dedicada de rendimiento sigue
pendiente.

El owner intrínseco `std.core` queda cerrado para la evidencia de STD-0.1A
mediante el contrato de grupo [`testing/stdlib-core.json`](testing/stdlib-core.json)
y su registro de nueve celdas en
[`testing/stdlib-owner-evidence.json`](testing/stdlib-owner-evidence.json)
(`STD-A-CORE-EVIDENCE-001`). La evidencia enlaza las nueve firmas de
`Option`/`Result` con protocolos estáticos, instanciación genérica, composición
de mapas y propagación, agregados bytecode y ejecución VM; las pruebas públicas
incluyen los casos de éxito, error, ausencia, límites y especialización
explícita. `HOST` es `not-applicable` porque el owner es compiler/VM-owned y no
consulta capabilities ni el host. El corpus de admission fuzz cubre formas
`Option`/`Result` y protocolos genéricos; `STD-A-FUZZ-001` promueve el fuzz
owner-aware y los baselines de rendimiento por owner quedan pendientes de
promoción.

El owner intrínseco `std.text` queda cerrado para la evidencia de STD-0.1A
mediante el contrato de grupo [`testing/stdlib-core.json`](testing/stdlib-core.json)
y su registro `STD-A-TEXT-EVIDENCE-001` en
[`testing/stdlib-owner-evidence.json`](testing/stdlib-owner-evidence.json).
Las quince firmas de `String` quedan trazadas desde el chequeo y la
especialización HIR hasta el puente compiler/VM: Unicode válido, índices y
slicing por scalar, iteración, búsquedas, transformaciones ASCII y rechazo
atómico de UTF-8 inválido tienen fixtures ejecutables. `HOST` es
`not-applicable` por ser un owner intrínseco sin capability ni host ambiental;
el corpus bounded de UTF-8/admission fuzz está enlazado; `STD-A-FUZZ-001`
promueve el fuzz owner-aware, mientras los baselines de coste y `STD-CONF-001`
permanecen pendientes de promoción.

El owner intrínseco `std.collections` queda cerrado para la evidencia de
STD-0.1A mediante el contrato de grupo [`testing/stdlib-core.json`](testing/stdlib-core.json)
y su registro `STD-A-COLL-EVIDENCE-001` en
[`testing/stdlib-owner-evidence.json`](testing/stdlib-owner-evidence.json).
Las dieciocho firmas de `Array`, `Map` y `Set` quedan trazadas desde HIR/MIR y
los intrinsics bootstrap hasta bytecode, VM y el fixture de runtime: semántica
de valor con COW interno, capacidad y errores atómicos, orden de inserción,
hashing de claves, membership, reemplazo/eliminación e iteración lazy están
cubiertos. `HOST` es `not-applicable` porque el owner es intrínseco y portable;
el admission fuzz y las properties eager/COW aportan cobertura de formas y
ownership; `STD-A-FUZZ-001` promueve el fuzz owner-aware, mientras los
baselines de memoria/hash y `STD-CONF-001` permanecen pendientes de promoción.

El owner intrínseco `std.iter` queda cerrado para la evidencia de STD-0.1A
mediante el contrato de grupo [`testing/stdlib-core.json`](testing/stdlib-core.json)
y su registro `STD-A-ITER-EVIDENCE-001` en
[`testing/stdlib-owner-evidence.json`](testing/stdlib-owner-evidence.json).
Las cuatro firmas de `Iterator` (`map`, `filter`, `take` y `collect`) quedan
trazadas por el protocolo estático HIR, lowering MIR/bytecode y ejecución VM.
El fixture `m11-std-iter-001.to` cubre laziness, consumo único, composición,
callbacks síncronos, closures, rutas calificadas/genéricas, `take(-1)` y
materialización acotada. Las properties y tests de runtime cubren cursores
prestados, dispatch de iteradores de usuario, agotamiento, trazado de fuente y
callbacks, y estados corruptos; `HOST` es `not-applicable` porque no hay
capability ni dependencia ambiental. `STD-A-FUZZ-001` promueve el fuzz
owner-aware; baselines de retención/allocations/materialización y
`STD-CONF-001` permanecen pendientes de promoción.

El owner intrínseco `std.math` queda cerrado para la evidencia de STD-0.1A
mediante el contrato de grupo [`testing/stdlib-core.json`](testing/stdlib-core.json)
y su registro `STD-A-MATH-EVIDENCE-001` en
[`testing/stdlib-owner-evidence.json`](testing/stdlib-owner-evidence.json).
Las nueve firmas escalares se trazan por dispatch HIR estático, puente
`process_host`, frontera nominal `MathError` y fixture público. Las pruebas
cubren redondeo ties-to-even y signed zero, infinitudes, NaN, subnormales,
overflow y dominio/no-finito de `sqrt`, apoyándose en `m6-num-004-ieee.to`,
properties de Float32 y diagnósticos de constantes. La implementación scalar
es el scalar oracle de 0.1 y no existe una ruta SIMD/fast-math separada; un
backend vectorizado futuro sólo puede promocionarse tras demostrar equivalencia
bit a bit. `HOST` es `not-applicable`; `STD-A-FUZZ-001` promueve el fuzz
owner-aware, mientras baselines por owner y `STD-CONF-001` siguen pendientes de
promoción.

El owner capability-gated `std.time` queda cerrado para la evidencia de
STD-0.1A mediante [`testing/stdlib-time.json`](testing/stdlib-time.json) y su
registro de nueve celdas en [`testing/stdlib-owner-evidence.json`](testing/stdlib-owner-evidence.json)
(`STD-A-TIME-EVIDENCE-001`). Sus seis requisitos separan el modelo de
`Duration`/`Instant`/`Timer`, los providers real y virtual, errores y límites,
ciclo de vida de timers y conformidad por capability. El provider real usa el
reloj monotónico de `std::time::Instant`; el virtual está sellado en
`std.testing`, avanza explícitamente y ejecuta el mismo corpus semántico. La
capability `clock` se comprueba en el límite del módulo y el fixture
`tests/runtime/m10-std-time-001.to` atraviesa parser, checker, bytecode, VM y
host. `HOST` es `verified`; `STD-A-FUZZ-001` promueve el fuzz owner-aware y los
baselines de rendimiento por provider permanecen como promoción pendiente y no
se inventan métricas.

El owner capability-gated `std.env` queda cerrado para la evidencia de
STD-0.1A mediante [`testing/stdlib-env.json`](testing/stdlib-env.json) y su
registro de nueve celdas en [`testing/stdlib-owner-evidence.json`](testing/stdlib-owner-evidence.json)
(`STD-A-ENV-EVIDENCE-001`). Sus seis requisitos separan capability y
disponibilidad, snapshot sellado, argv ordenado, nombres/valores textuales y
binarios, ausencia mediante `Option`, copias independientes y límites
atómicos. El proveedor solo recibe un plan runtime explícito: los tests cubren
inputs inyectados, names inválidos, hosts unavailable y aislamiento de `PATH`,
`HOME` y otras variables ambientales. `HOST` es `verified`; `STD-A-FUZZ-001`
promueve el fuzz owner-aware y los baselines de rendimiento por capability
permanecen como promoción pendiente y no se confunden con lectura del entorno
del proceso de compilación.

#### 10.2.2 Contrato cerrado de `std.text`

`std.text` mantiene una sola representación: `String` inmutable y siempre
válido en UTF-8. Los índices y límites de esta API son posiciones de scalar
Unicode, nunca offsets de bytes. `TextError` pertenece al módulo y no publica
una representación de error alternativa.

```tondo
import std.text

fn String.empty(): String
fn String.fromChars(value: Array[Char]): String ! text.TextError
fn String.length(self): Int
fn String.byteLength(self): Int
fn String.get(self, index: Int): Char?
fn String.slice(self, start: Int, end: Int): String ! text.TextError
fn String.contains(self, needle: String): Bool
fn String.startsWith(self, prefix: String): Bool
fn String.endsWith(self, suffix: String): Bool
fn String.find(self, needle: String): Int?
fn String.replace(self, old: String, new: String): String
fn String.trim(self): String
fn String.toLowerAscii(self): String
fn String.toUpperAscii(self): String
fn String.chars(self): String

enum TextError { InvalidIndex, InvalidBoundary, ResourceLimit }
```

`String.chars()` devuelve el mismo valor inmutable: `String` es el witness
intrínseco de `Iterator[Char]`, por lo que un `for` puede consumirlo sin crear
un wrapper de cursor. `fromChars` solo acepta valores `Char` (ya son scalars
válidos), materializa una copia UTF-8 y falla de forma atómica si excede el
límite del run. `slice` usa `[start, end)`, rechaza índices negativos o fuera
de rango con `InvalidIndex`, rechaza `start > end` con `InvalidBoundary` y nunca
publica una cadena parcial. Las búsquedas son por scalar/substring; `trim` y
las conversiones ASCII no aplican normalización Unicode ni locale. La
conversión `String(Bytes)` continúa siendo la única frontera UTF-8 que puede
devolver `Utf8Error` por bytes inválidos.

#### 10.2.3 Contrato cerrado de `std.collections`

`Array[T]`, `Map[K, V]` y `Set[K]` conservan una sola representación de valor
del lenguaje. La implementación puede compartir buffers mediante COW, pero una
asignación, paso por valor o resultado de una operación nunca expone alias
mutables inesperados. Los métodos que cambian una colección exigen un receptor
`var`; los métodos de consulta son puros. `Map` y `Set` requieren `Key` para
garantizar igualdad y hashing estables, preservan el orden de inserción y no
reordenan una entrada al reemplazar su valor.

```tondo
import std.collections

fn Array.new[T](): Array[T]
fn Array.withCapacity[T](capacity: Int): Array[T] ! CollectionError
fn Array.length[T](self): Int
fn Array.get[T](self, index: Int): T?
fn Array.slice[T](self, start: Int, end: Int): Array[T] ! CollectionError
fn Array.push[T](var self, value: T): Unit ! CollectionError
fn Array.pop[T](var self): T?

fn Map.new[K: Key, V](): Map[K, V]
fn Map.get[K: Key, V](self, key: K): V?
fn Map.insert[K: Key, V](var self, key: K, value: V): V?
fn Map.remove[K: Key, V](var self, key: K): V?
fn Map.contains[K: Key, V](self, key: K): Bool
fn Map.entries[K: Key, V](self): Iterator[(K, V)]

fn Set.new[K: Key](): Set[K]
fn Set.insert[K: Key](var self, value: K): Bool
fn Set.remove[K: Key](var self, value: K): Bool
fn Set.contains[K: Key](self, value: K): Bool
fn Set.values[K: Key](self): Iterator[K]

enum CollectionError { InvalidCapacity, InvalidIndex, InvalidStep, ResourceLimit }
```

`Array.new` y `Map/Set.new` requieren argumentos de tipo explícitos para que no
haya una inferencia dependiente del uso posterior. `withCapacity` rechaza una
capacidad negativa, no representable o por encima del límite del run sin
publicar una reserva parcial. `get` es total y devuelve `none` fuera de rango;
los índices negativos cuentan desde el final. `slice` usa `[start, end)`, exige
límites no negativos y que `start <= end <= length`, y también es atómica.
`push` conserva el orden y devuelve `CollectionError` si no puede reservar; no
modifica el array en ese caso. `pop` devuelve `none` para un array vacío.

`Map.insert` devuelve el valor anterior y mantiene la primera posición de la
key; `remove` devuelve el valor eliminado. `Map.entries` y `Set.values` son
cursores propios lazy: cada target de `for` consume exactamente un elemento y
el cursor no se reinicia implícitamente. `Set.insert` devuelve `false` cuando la
key ya existe y `remove` devuelve `false` cuando está ausente. Los resultados de
consulta y los cursores se materializan por la ruta HIR → MIR → bytecode → VM;
no existe un segundo API host ni una segunda representación de colección.

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
protocolos mediante dispatch estático. `read`, `write`, `flush`, `readAll` y
`writeAll` se declaran como `fn` con `suspends` en sus contratos sin cuerpo; una
implementación con cuerpo puede conservarlo explícitamente o inferirlo. Una
llamada ordinaria espera automáticamente. El caller puede escribir `await` como
forma explícita, pero no hay variantes
`readAsync` ni `writeAsync`. STD-0.1 no exige type erasure, vtables ni
un stream dinámico común para almacenar implementaciones heterogéneas.

La superficie mínima de `std.io` es:

```tondo
enum IoError { Closed, Cancelled, InvalidData, ResourceLimit, Host }
enum ReadResult { Data(Bytes), Eof }
trait Reader {
    fn read(var self, max: Int): ReadResult ! IoError suspends
}
trait Writer {
    fn write(var self, data: Bytes): Int ! IoError suspends
    fn flush(var self): Unit ! IoError suspends
}
fn defaultLimits(): IoLimits
fn limits(maxBytes: Int, maxRead: Int): IoLimits ! IoError
fn readAll[R: Reader](var reader: R, limits: IoLimits): Bytes ! IoError suspends
fn writeAll(var writer: Writer, data: Bytes): Unit ! IoError suspends
type IoLimits
```

`read` puede entregar menos bytes que los solicitados; el único EOF normal es
`ReadResult.Eof`. `readAll` no publica un buffer parcial y comprueba el límite
agregado antes de consumir un handle hosted. `writeAll` drena short writes,
rechaza escritores sin progreso y hace `flush` al finalizar. Los backends que
pueden suspender propagan cancelación como `IoError.Cancelled`; el préstamo de
`Bytes` termina al volver de `write`.

Los protocolos no prometen que toda fuente pueda seek, conocer su longitud,
repetir una lectura o conservar datos después de cancelar. Cada capacidad
adicional aparece como trait o método exacto, no como operación que falla
siempre para ciertos handles.

La evidencia ejecutable `STD-A-IO-EVIDENCE-001` mantiene esta separación:
`std.io` aporta únicamente los protocolos portables y sus límites, mientras
`std.console`, `std.fs` y `std.process` poseen los adaptadores capability-gated.
El corpus de chunks deterministas prueba short I/O, EOF, progreso, errores de
`flush`, límites y cancelación sin inventar una segunda API síncrona/asíncrona.
Las dimensiones de coste son bytes copiados, chunks procesados y work-units;
`STD-A-FUZZ-001` promueve el fuzz owner-aware y baselines por owner y
conformance global se promueven por sus gates propios.

### 10.4 Paths

`std.path.Path` no es un alias de `String`. Debe poder representar paths nativos
que el host admite aunque no sean Unicode.

La representación es un snapshot de bytes con un límite de 32 KiB. `fromBytes`
acepta bytes nativos salvo NUL, `toBytes` devuelve una copia exacta y
`fromString`/`toString` solo aplican la validación UTF-8 necesaria para cruzar
la frontera textual. No hay normalización Unicode, case-folding, resolución de
`.`/`..`, expansión de separadores ni consulta implícita al filesystem:
secuencias NFC/NFD y componentes literales permanecen observables.

La separación es:

- `std.path`: representación y operaciones léxicas.
- `std.fs`: observación y mutación del filesystem.

Formatear un path para diagnóstico no garantiza una representación reversible.
Convertirlo a texto puede fallar o exigir una política explícita. Normalizar
léxicamente no consulta el filesystem, no resuelve enlaces y no afirma
canonicalidad física.

`kind` devuelve `Bool`: `true` para un snapshot absoluto y `false` para uno
relativo. No existe un enum paralelo para esta propiedad binaria.

`join` es una operación atómica sobre un único componente y rechaza un
componente vacío, cualquier `/`, NUL o el resultado que supere el límite. Las
consultas `parent`, `fileName`, `extension`, `kind` e `isEmpty` son puramente
léxicas y tienen resultados deterministas para raíz, path vacío, archivos
ocultos, extensiones vacías y separadores finales.

### 10.5 Filesystem hosted

`std.fs` es la frontera capability-gated para observar y mutar el filesystem.
Todas sus operaciones que pueden tocar el host son `suspendible`; `std.path` sigue
siendo puro y síncrono. Importar `std.fs` no concede `filesystem`: el target debe
declarar explícitamente esa capability y el compilador rechaza el módulo con
`E1008` antes del lowering cuando falta. El owner público es:

```tondo
type File
type Directory
type Metadata
enum OpenMode { Read, Write, ReadWrite, Append, Create, CreateNew }
fn open(path: Path, mode: OpenMode): File ! FsError suspends
fn openDirectory(path: Path): Directory ! FsError suspends
fn metadata(path: Path): Metadata ! FsError suspends
fn File.read(var self, max: Int): Option[Bytes] ! FsError suspends
fn File.write(var self, data: Bytes): Int ! FsError suspends
fn File.flush(var self): Unit ! FsError suspends
fn Directory.list(var self): Array[Path] ! FsError suspends
```

`File` y `Directory` son afines: el owner se revoca en cleanup normal y durante
unwind. El verificador impide el uso posterior en programas seguros y el host
rechaza cualquier token stale o forjado como una violación de la invariante de
runtime; `FsError.Closed` queda disponible para cierres observables que no
invaliden esa invariante. `File` conserva posición entre llamadas y no copia
el descriptor; sus métodos ofrecen la semántica de `Reader`/`Writer`, mientras
`std.io.readAll` y `std.io.writeAll` siguen recibiendo esos handles explícitos.
`list` ordena por bytes nativos; `atomicWrite` usa un temporal en el mismo
directorio, hace flush y rename. Los límites globales de bytes, entradas y
trabajo se comprueban antes de materializar bytes o entradas: un rechazo es
atómico y no publica resultados parciales. El cleanup se ejecuta en las rutas
normales, durante unwind y al cancelar una operación suspendible. El host no
sigue enlaces simbólicos al eliminar un recurso temporal y no incluye paths
físicos ni contenido en `FsError`; la operación de rename no promete
durabilidad de hardware. La evidencia ejecutable de este contrato es
`STD-A-FS-EVIDENCE-001`; `STD-A-FUZZ-001` promueve su fuzz owner-aware, mientras
los baselines por target y la conformance global siguen siendo promoción
posterior.

### 10.6 Formato

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

El contrato executable de `std.format` está en
`docs/contracts/stdlib-core.md`: `Builder`, `format` y `join` son una única
superficie estática sobre `Display`. Los builders comprueban el límite antes de
mutar, y el caso de error no expone materialización parcial. La evidencia
`STD-A-FMT-EVIDENCE-001` enlaza las cinco firmas con HIR/MIR/bytecode/VM, el
fixture runtime, properties de límites y la auditoría pública; fuzz de
operaciones y baselines de allocations/materialización se mantienen como
promoción posterior, no como garantías inventadas.

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
| `std.bytes` | Core | — | 0.1 | `Bytes`, builders y conversión binaria explícita |
| `std.io` | Core | — | 0.1 | Protocolos de lectura/escritura, buffers, EOF, partial I/O y errores portables |
| `std.async` | Core | — | 0.1 | `Join`, `oneshot`, cancelación cooperativa y adaptación `AsyncIterator` |
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
| `std.time` | Core + gated | `clock` para proveedor | 0.1 | Time-base monotónico en STD-0.1A; calendario civil y zonas horarias versionadas en STD-0.1B |
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
- `std.channel`, `std.sync` y `std.executor` reutilizan el único modelo suspendible,
  de ownership y de memoria del lenguaje; ninguna API crea tasks desligadas o
  un segundo tipo de future.
- `std.time` separa estrictamente el reloj monotónico del calendario civil. El
  time-base de `Duration`, `Instant`, timers y deadlines pertenece a STD-0.1A;
  los datos de zona horaria son inputs versionados de la distribución en
  STD-0.1B, nunca una consulta ambiental durante compilación.
- `std.net` no concede red por import: cada target debe seleccionar `network` y
  cada operación conserva I/O, timeout y cancelación en su firma.
- `std.uuid` separa representación y parsing core de cualquier generador que
  requiera `entropy` o `clock`.
- `std.log` define eventos puros en core; cada sink declara sus capabilities y
  política de backpressure sin alterar silenciosamente el control del programa.
- Los argumentos de proceso pertenecen a `std.env`; el contrato bootstrap de
  `std.process.args()` se migrará sin compatibilidad implícita.
- `std.env` solo expone un snapshot runtime explícito; no lee environment durante
  compilación ni ofrece mutación implícita.
- `std.testing` solo existe en source sets de test y no forma parte de producción.
- Streams, canales, red y sincronización solo aparecen bajo sus propietarios
  canónicos; ningún módulo existente los introduce indirectamente.

### 14.2 Estado de esta revisión

El catálogo y sus propietarios son normativos. Las declaraciones exhaustivas de
las veintinueve superficies permanecen pendientes, salvo el núcleo sellado que
`TONDO_TESTING_SPEC.md` ya fija para `std.testing`, `std.bytes`, el sustrato
monotónico de `std.time` y el snapshot read-only de `std.env`. El bootstrap
conserva su propio contrato separado hasta completar la migración de la sección
19.

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

### 14.3 `std.time`: sustrato monotónico de STD-0.1A

Esta sección cierra únicamente el time-base necesario para producción y
testing. El calendario civil, las zonas horarias, el reloj de pared y las
conversiones a fechas pertenecen a STD-0.1B y no pueden introducirse como una
dependencia implícita de este contrato.

#### 14.3.1 Valores portables

`Duration` es un valor inmutable cuyo quantum semántico es exactamente un
nanosegundo. Su dominio es un contador firmado de 64 bits con los mismos límites
matemáticos que `Int`; el layout físico no forma parte de la API. Puede ser
negativo, representa cero y no contiene identidad de reloj, epoch, locale ni
zona horaria.

La superficie mínima y canónica es:

~~~tondo pseudocode
pub type Duration

pub enum DurationError {
    Overflow
}

pub fn Duration.fromNanoseconds(value: Int): Duration
pub fn Duration.fromMicroseconds(value: Int): Duration ! DurationError
pub fn Duration.fromMilliseconds(value: Int): Duration ! DurationError
pub fn Duration.fromSeconds(value: Int): Duration ! DurationError

pub fn Duration.toNanoseconds(self): Int
pub fn Duration.add(self, other: Duration): Duration ! DurationError
pub fn Duration.subtract(self, other: Duration): Duration ! DurationError
pub fn Duration.multiply(self, factor: Int): Duration ! DurationError
pub fn Duration.negate(self): Duration ! DurationError
pub fn Duration.isZero(self): Bool
pub fn Duration.isNegative(self): Bool
pub fn Duration.isLessThan(self, other: Duration): Bool
~~~

`DurationError` cumple `Copy`, `Discard`, `Equatable`, `Key`, `Send` y
`Share`. `ClockError` tiene las mismas capacidades; sus variantes no contienen
datos del host ni mensajes localizados.

`fromNanoseconds` y `toNanoseconds` son totales porque `Int` ya tiene el mismo
dominio. Las demás operaciones comprueban el resultado completo antes de
publicarlo y devuelven `DurationError.Overflow`; nunca hacen wrapping,
saturación, truncado silencioso ni pánico. Las conversiones de unidades
multiplican por un factor exacto y no redondean. Un target no puede cambiar el
quantum por usar un reloj con otra resolución.

`Duration` cumple `Copy`, `Discard`, `Equatable`, `Key`, `Send` y `Share`. Sus
operaciones son puras, no requieren `clock`, no asignan y no consultan al host.
La igualdad y el hash observan únicamente el contador de nanosegundos.

#### 14.3.2 Instantes y proveedor

`Instant` es un valor opaco producido por un proveedor monotónico. No tiene
epoch ni conversión implícita a `String`, fecha civil o número. Cada valor
conserva internamente la identidad del proveedor y del dominio que lo creó;
ninguno de esos identificadores es observable desde fuente.

~~~tondo pseudocode
pub type Instant

pub enum ClockError {
    Unavailable
    DomainMismatch
    InvalidDelay
    OutOfRange
    ResourceLimit
}

pub fn now(): Instant ! ClockError
pub fn resolution(): Duration ! ClockError
pub fn deadline(after: Duration): Instant ! ClockError

pub fn Instant.add(self, delta: Duration): Instant ! ClockError
pub fn Instant.subtract(self, delta: Duration): Instant ! ClockError
pub fn Instant.durationSince(self, other: Instant): Duration ! ClockError
pub fn Instant.isBefore(self, other: Instant): Bool ! ClockError
pub fn Instant.isAfter(self, other: Instant): Bool ! ClockError
~~~

`now` es síncrona, no suspende y devuelve instantes no decrecientes. El
proveedor puede devolver dos veces el mismo valor cuando su resolución es más
gruesa que un nanosegundo. `resolution` devuelve una duración positiva y
estable para el proveedor activo; la resolución efectiva y su método de
redondeo se declaran en la matriz del target. La representación sigue siendo
nanosegundos aunque el host use ticks, ciclos u otra unidad.

`durationSince` devuelve la diferencia firmada `self - other`. Todas las
operaciones entre dos `Instant` comprueban que pertenecen al mismo proveedor y
dominio; si no, devuelven `ClockError.DomainMismatch` y nunca comparan por
accidente sus contadores internos. El overflow de `add`, `subtract` o
`deadline` devuelve `ClockError.OutOfRange`. Un `Instant` cumple `Copy`,
`Discard`, `Send` y `Share`, pero no `Equatable` ni `Key`: la igualdad entre
dominios no es una operación booleana válida y debe expresarse mediante las
consultas anteriores.

La capability `clock` garantiza un proveedor monotónico que cumple este
contrato; no habilita reloj de pared ni calendario. Si el proveedor no puede
consultarse, la operación devuelve `ClockError.Unavailable`, nunca usa un
fallback wall-clock y nunca produce un pánico. Consultar el instante no
bloquea ni crea una task.

#### 14.3.3 Suspensión, timers y deadlines

Un deadline es un `Instant` del proveedor activo; STD-0.1 no introduce un
wrapper `Deadline` adicional. `deadline(after)` permite duraciones firmadas
para representar también un deadline ya vencido. Las esperas y timers solo
aceptan retrasos no negativos:

~~~tondo pseudocode
pub fn sleep(delay: Duration): Unit ! ClockError suspends

pub type Timer

pub fn Timer.after(delay: Duration): Timer ! ClockError
pub fn Timer.at(deadline: Instant): Timer ! ClockError
pub fn Timer.wait(self): Unit ! ClockError suspends
pub fn Timer.cancel(self): Unit
~~~

`sleep` es la comodidad sin ownership visible; `Timer` es la forma explícita
cuando el programa necesita armar, transferir o cancelar un timer. No son dos
semánticas de reloj: ambas registran un único evento one-shot en el mismo
proveedor. Un retraso negativo produce `ClockError.InvalidDelay`; cero es válido
y conserva un punto de suspensión suspendible. Un deadline vencido hace que `wait`
termine sin esperar.

`Timer` es afín, no es `Copy`, `Share`, `Equatable` ni `Key`, y puede moverse
entre tasks que cumplan `Send`. `after` y `at` reservan el descriptor de forma
atómica y devuelven `ClockError.ResourceLimit` sin dejar un timer parcial.
`wait` consume el timer, espera hasta que el proveedor observe su deadline y
constituye un punto cooperativo de cancelación. No se despierta antes del
deadline; el proveedor real puede completar después por latencia del target.
`cancel` también consume el timer, es síncrono y completa la desregistración
antes de retornar. Un timer no tiene
reset, repetición ni finalizador de usuario; `wait` o `cancel` son sus únicas
operaciones terminales. Si una cancelación estructurada interrumpe `wait`, el
runtime desregistra el timer antes de propagar la cancelación y no la convierte
en una variante añadida a `ClockError`.

Un timer creado con `Timer.at` rechaza con `DomainMismatch` un instante de otro
proveedor o dominio. La carrera entre expiración y cancelación se resuelve por
el orden de linealización del proveedor; nunca se entregan dos outcomes. Un
timer real no garantiza despertar exactamente en el deadline: no despierta
antes de él y su latencia posterior pertenece al target. El proveedor virtual
de `std.testing` sí observa los deadlines exactamente y conserva el orden
estable de creación fijado en `TONDO_TESTING_SPEC.md`.

#### 14.3.4 Sustitución virtual y separación civil

`testing.withVirtualTime` sustituye únicamente el proveedor monotónico durante
su dominio sellado. El código probado continúa llamando a `std.time.now`,
`deadline`, `sleep` y `Timer` con las firmas anteriores; no recibe un reloj de
test ni una capability adicional. Un `Duration` puede pasar entre dominios,
pero un `Instant` o `Timer` no. Los instantes creados antes de entrar al dominio
virtual se rechazan dentro de él por `DomainMismatch`.

El time-base no consulta `TZ`, locale, epoch Unix, hora civil ni red. `Date`,
`Time`, `DateTime`, zonas horarias, resolución de calendario y conversiones
entre calendario e instante se especificarán e implementarán en STD-0.1B con
datos versionados. Ninguna API de STD-0.1A puede aceptar un `Instant` donde
espere una fecha civil o viceversa.

#### 14.3.5 `std.env`: snapshot runtime explícito

`std.env` es una superficie capability-gated por `environment`. Su única
responsabilidad en STD-0.1A es leer un snapshot inmutable de argumentos y
variables entregado por el adaptador de ejecución. No consulta el host durante
la compilación, no convierte el environment en input implícito del proyecto y
no ofrece mutación global. `set`, `remove`, `clear`, herencia ambiental y
resolución de nombres mediante locale quedan fuera de esta versión.

El proveedor obtiene el snapshot una vez en la frontera de invocación. En un
target de producción puede construirlo a partir del proceso, mientras que un
adaptador sellado puede entregarlo desde un plan de inputs; la fuente Tondo ve
la misma API y nunca distingue esas dos procedencias. Un snapshot vacío es
válido y no es un error. La ausencia de una entrada se representa con `none`,
no con una excepción ni con un valor vacío.

~~~tondo pseudocode
pub enum EnvError {
    Unavailable
    InvalidName
    ResourceLimit
}

pub type Name

pub enum Value {
    Text(String)
    Bytes(bytes.Bytes)
}

pub type Snapshot

pub fn snapshot(): Snapshot ! EnvError
pub fn Name.fromText(value: String): Name ! EnvError
pub fn Name.fromBytes(value: bytes.Bytes): Name ! EnvError
pub fn Snapshot.arguments(self): Array[Value]
pub fn Snapshot.get(self, name: Name): Option[Value] ! EnvError
pub fn Value.asText(self): Option[String]
pub fn Value.asBytes(self): bytes.Bytes
~~~

`Name.fromText` codifica el nombre en UTF-8 sin normalización Unicode;
`Name.fromBytes` permite consultar exactamente bytes que no sean UTF-8. Un nombre no puede estar
vacío ni contener `NUL` o `=`; la validación produce `InvalidName` antes de
consultar el proveedor. Las dos variantes que representan los mismos bytes
identifican la misma entrada. El proveedor conserva los bytes originales del
nombre y del valor, y devuelve `Value.Text` solo cuando los bytes del valor
son UTF-8 válido; de lo contrario devuelve `Value.Bytes`. `asText` no intenta
reparar ni reemplazar encoding y `asBytes` siempre devuelve una copia lógica
independiente.

`Snapshot.arguments` conserva el orden y la cardinalidad del vector de argv.
Cada elemento usa la misma política `Text`/`Bytes`; no se elimina el elemento
cero ni se interpreta ningún argumento como una ruta o un comando. El array y
los bytes devueltos no son vistas mutables del snapshot. `Snapshot` es
inmutable, `Send` y `Share`, pero no `Copy`; `Name` y `Value` son valores
copiables cuando sus payloads lo son. Ningún método devuelve un handle al
almacenamiento del host.

El proveedor aplica un límite de bytes y de entradas antes de construir el
snapshot. Si no puede capturarlo, devuelve `Unavailable`; si supera el límite,
devuelve `ResourceLimit` sin publicar un snapshot parcial. Dos llamadas durante
una misma invocación observan el mismo snapshot sellado y no pueden ver
cambios externos a mitad de ejecución. La capability `environment` es
necesaria tanto para `snapshot` como para cualquier operación que consuma un
snapshot; omitirla produce `E1008` en el límite de módulos. `Duration`, `Bytes`
y las conversiones de texto siguen siendo APIs independientes y no se importan
por `std.env`.

El runner de testing materializa únicamente los inputs públicos o secretos
declarados por el plan. Los públicos se fijan por bytes y hash; los secretos se
materializan solo dentro del worker y se representan fuera de él por su
descriptor/version. `std.env` no redacciona copias que el programa escriba
explícitamente en logs, snapshots, artifacts o salida, y el runner no realiza
redacción heurística. Un target que no ofrezca `environment` no puede importar
`std.env`, aunque el programa se ejecute bajo tests.

#### 14.3.6 Plan cerrado, hashes y disponibilidad

El slice se versiona como `std.time.monotonic-0.1` dentro de la única
distribución `toolchain:std:draft`. El plan cerrado debe contener, cada uno con
su SHA-256 lowercase, los bytes de:

1. el source set core que define `Duration` y `DurationError`;
2. el source set gated `clock` que define `Instant`, `ClockError`, `Timer` y
   sus firmas;
3. la interfaz pública `std.time` resultante;
4. la unidad privilegiada del proveedor monotónico real, con sus hashes de
   firma, contrato de seguridad e implementación; y
5. el descriptor y corpus del proveedor virtual usado por la conformidad,
   que pertenece al artefacto de test y no a un bridge ambiental del frontend.

Estas entradas utilizan las categorías existentes de `tondo-toolchain` (`source`,
`dependency-interface` y `privileged-unit`); no se añade un path físico ni una
segunda identidad de `std.time`. El `content_hash` de la distribución y su
`api_hash` cubren el slice completo. La implementación hosted ya materializa
el proveedor real/virtual y la frontera suspendible descrita en
`docs/contracts/stdlib-time.md`; mientras no existan los bytes y hashes
reproducibles del plan cerrado, `STD-TIME-BASE-CONF-001` permanece pendiente y
`std.time` no se anuncia como una superficie distribuida estable.

### 14.4 Contratos cerrados de los owners STD-0.1A

Las firmas exhaustivas de los owners de valores y host están en
[`docs/contracts/stdlib-core.md`](./docs/contracts/stdlib-core.md) y
[`docs/contracts/stdlib-hosted.md`](./docs/contracts/stdlib-hosted.md). Esos
documentos forman parte del mismo contrato, no una API alternativa: cada
declaración que aparezca aquí o en un ejemplo debe coincidir byte a byte en
nombre, visibilidad, parámetros, modo, outcome y capability. La distribución
canónica incluye ambos source sets y sus hashes.

La integración normativa de STD-0.1A queda así:

La relación entre estos owners, sus source sets, dependencias y capabilities
no se duplica en otro catálogo: la fuente machine-readable única es
[`testing/stdlib-spec.json`](./testing/stdlib-spec.json), validada por
`scripts/stdlib-spec-check.sh`. El orden del catálogo es topológico y el gate
rechaza owners duplicados, contratos ausentes, aliases, defaults implícitos y
ciclos. Este cierre integra el contrato de los owners; no convierte los
contratos `closed-contract` en implementaciones publicadas.

La trazabilidad de implementación por firma se mantiene separada en
[`docs/contracts/stdlib-public-api-audit.md`](./docs/contracts/stdlib-public-api-audit.md)
y [`testing/stdlib-public-api.json`](./testing/stdlib-public-api.json). La
matriz debe demostrar `contrato → HIR → lowering → host/VM → caso público`;
un path Rust, una prueba documental o un alias bootstrap no son evidencia
suficiente. Mientras existan filas `open-gaps`, el catálogo sigue siendo un
contrato de desarrollo y no una publicación.

La coordinación del grupo Core ya implementado se registra por separado en
[`docs/contracts/stdlib-implementation-coordination.md`](./docs/contracts/stdlib-implementation-coordination.md)
y [`testing/stdlib-implementation-coordination.json`](./testing/stdlib-implementation-coordination.json).
Este registro exige evidencia completa para sus owners y conserva los gaps de
la auditoría global como trabajo pendiente; no relaja el contrato de
promoción S1A.

La coordinación Hosted se registra por separado en
[`docs/contracts/stdlib-hosted-implementation-coordination.md`](./docs/contracts/stdlib-hosted-implementation-coordination.md)
y [`testing/stdlib-hosted-implementation-coordination.json`](./testing/stdlib-hosted-implementation-coordination.json).
Este registro comprueba las cuatro superficies `std.console`, `std.path`,
`std.fs` y `std.process`, sus bridges, capabilities y 48 firmas públicas. La
capability vacía de `std.path` es intencional: sus operaciones son puramente
léxicas. El cierre Hosted mantiene abiertos los gaps públicos de codecs y
owners build-only, sin relajar `--strict`.

La coordinación normativa completa de STD-0.1A vive en
[`docs/contracts/stdlib-matrix.md`](./docs/contracts/stdlib-matrix.md) y
[`testing/stdlib-matrix.json`](./testing/stdlib-matrix.json). Incluye una fila
por firma y por requisito de owner, el owner intrínseco `std.bytes`, las
dimensiones públicas de performance y las seis celdas
`SPEC → IMPL/HOST → MODEL/TEST/FUZZ → PERF → CONF → DOC`. Las celdas
pendientes exigen razón y referencia; la matriz no adelanta requisitos de
STD-0.1B ni convierte evidencia de kernel en una API publicada.

La dimensión `FUZZ` de S1A queda cerrada por `STD-A-FUZZ-001`. El target
owner-aware `fuzz/fuzz_targets/stdlib_owners.rs` enruta cada entrada a uno de
los 22 owners con límites explícitos, corpus y oráculo propios; el contrato
machine-readable `testing/stdlib-fuzz.json` conserva semillas, replay,
minimización y persistencia de regresiones. `scripts/stdlib-fuzz-check.sh`
comprueba que no falte ninguna ruta ni corpus y que la evidencia de cada owner
esté promovida. Esta clausura de fuzz no adelanta las dimensiones
independientes de performance ni conformance.

| Owner | Source set | Estado | Dependencias directas |
|---|---|---|---|
| `std.core` (intrínsecos) | `stdlib-core` | cerrado | lenguaje |
| `std.text` | `stdlib-core` | cerrado | `std.bytes` |
| `std.collections` | `stdlib-core` | cerrado | `std.core` |
| `std.iter` | `stdlib-core` | cerrado | colecciones |
| `std.math` | `stdlib-core` | cerrado | numéricos intrínsecos |
| `std.format` | `stdlib-core` | cerrado | `Display`, `std.bytes` |
| `std.io` | `stdlib-core` | cerrado | `Bytes` |
| `std.async` | `stdlib-core` | cerrado | `Join`, `oneshot`, `AsyncIterator` |
| `std.serialization` | `stdlib-core` | cerrado | `std.io`, `std.bytes` |
| `std.console` | `stdlib-hosted` | cerrado | `std.io`, capability `console` |
| `std.path` | `stdlib-hosted` | cerrado | `String`, `Bytes` |
| `std.fs` | `stdlib-hosted` | cerrado | `std.path`, `std.io`, `filesystem` |
| `std.process` | `stdlib-hosted` | cerrado | `std.io`, `std.bytes`, `process` |

Los contratos de JSON, MessagePack, Protobuf y `std.testing` mantienen sus
registros de owner existentes y reutilizan exactamente `Encoder`,
`Decoder`, `Reader`, `Writer` y `Bytes` de estos source sets. `std.path`
no adquiere `filesystem` por importar; `std.console`, `std.fs` y `std.process`
solo comprueban su capability en el límite del adaptador. Ningún contrato
introduce una segunda representación de error, stream o colección.

La implementación hosted inicial puede residir en unidades privilegiadas
mientras el compilador no sea capaz de compilar estos módulos en Tondo. Esa
decisión es de bootstrap y no cambia el owner, la semántica ni la identidad de
la API. Cada unidad debe publicar su source hash, modelo, tests, oracle escalar
y conformance antes de cerrar S1A.

### 14.5 Arquitectura común de serialización

Hay dos rutas deliberadamente distintas:

- **Tipada:** un tipo concreto implementa `serialization.Encode[C]` y/o
  `serialization.Decode[C]`; el compilador monomorfiza la llamada y el codec
  accede directamente a su estructura.
- **Dinámica explícita:** JSON y MessagePack comparten `serialization.Value`
  (reexportado como `json.Value`/`messagepack.Value`) para documentos cuyo
  schema no se conoce al compilar; Protobuf conserva un modelo de wire propio.

La ruta tipada no construye un `Value`, no consulta `std.reflect` y no busca
fields por string. La ruta dinámica no introduce `Any`: sus casos posibles son
un enum cerrado y documentado, y el codec rechaza las variantes que su wire
format no admite.

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

La ruta dinámica común pertenece a `std.serialization` y se expone como
`serialization.Value` (JSON y MessagePack pueden reexportarlo como `Value`). Sus
variantes son `Null`, `Bool`, `Int`, `UInt`, `Float`, `Text`, `Bytes`,
`Object(ordered Map[String, Value])`, `Map(ordered Array[(Value, Value)])` y
`Extension(tag, Bytes)`. JSON solo produce las variantes de su modelo y rechaza
`Bytes`, `Extension` y mapas con claves no textuales salvo una opción explícita;
MessagePack conserva bytes, extensiones y claves arbitrarias. Protobuf no usa
este árbol: su inspección dinámica es un modelo de wire propio con fields
desconocidos.

`Value` es poseído, mutable y tiene copia lógica independiente. `ValueView` es
prestado e inmutable y solo vive hasta el siguiente evento o hasta que termina
la entrada; `parseView` lo entrega sin materialización. `clone()` siempre produce
una copia lógica; copy-on-write es una optimización interna no observable.
`Raw`/`RawView` son bytes opacos específicos de cada codec. `raw(bytes)` valida
antes de construir `Raw`; `rawUnchecked(bytes)` solo existe en `unsafe`.

La derivación estática utiliza `Encode[C]` y `Decode[C]`, donde `C` es el codec
(`Json`, `MessagePack` o `Protobuf`), no interfaces runtime:

~~~tondo pseudocode
pub trait Encode[C] {
    fn encode[E, S: Encoder[C, E]](value: Self, var encoder: S): Unit ! E
}
pub trait Decode[C] {
    fn decode[E, D: Decoder[C, E]](var decoder: D): Self ! E
}
~~~

Un mismo tipo puede implementar varios codecs. `@name("wire_name")` cambia el
nombre común; `@json(base64)`, `@messagepack(binary)` y `@proto(number)` afinan
un codec concreto. `@proto(number)` es obligatorio para cada field Protobuf y
los números nunca se infieren; los números 19000..19999 están reservados.
`@ignore` es simétrico y omite el field al codificar y decodificar, por lo que
un field ignorado debe ser `Option[T]` y se reconstruye como `none`.
`@json(base64)` convierte `Bytes` tipado a/desde texto Base64 (RFC 4648;
URL-safe debe nombrarse explícitamente); `parse` dinámico conserva el texto
original. Las anotaciones se resuelven en compile time y no requieren
reflection de valores.

Un `Encode[C]` o `Decode[C]` derivado añade `Discard` únicamente a los
parámetros genéricos usados en payloads. En encode es necesario porque el
writer puede fallar antes de consumir todos los fields recibidos por valor; en
decode permite limpiar valores parciales si falla la validación de un field o
cierre posterior. Una implementación manual puede manejar tipos affine si
consume todos los valores pendientes en cada salida de error; el compilador
aplica exactamente el mismo análisis de ownership al código generado y al
escrito a mano.

### 14.6 `std.serialization`

`std.serialization` posee los protocolos estáticos compartidos. La forma
normativa exhaustiva está en
[`docs/contracts/stdlib-serialization.md`](./docs/contracts/stdlib-serialization.md)
y se resume aquí para mantener la tabla de owners junto al lenguaje:

~~~tondo pseudocode
pub trait Encoder[C, E] {
    fn null(var self): Unit ! E
    fn bool(var self, value: Bool): Unit ! E
    fn int(var self, value: Int64): Unit ! E
    fn uint(var self, value: UInt64): Unit ! E
    fn float32(var self, value: Float32): Unit ! E
    fn float64(var self, value: Float64): Unit ! E
    fn string(var self, value: String): Unit ! E
    fn bytes(var self, value: Bytes): Unit ! E
    fn base64(var self, value: Bytes): Unit ! E
    fn startArray(var self, length: Int?): Unit ! E
    fn endArray(var self): Unit ! E
    fn startMap(var self, length: Int?): Unit ! E
    fn mapKey(var self): Unit ! E
    fn endMap(var self): Unit ! E
    fn startRecord(var self, name: String, fields: Int?): Unit ! E
    fn field(var self, name: String): Unit ! E
    fn endRecord(var self): Unit ! E
    fn startEnum(var self, name: String, variant: String): Unit ! E
    fn endEnum(var self): Unit ! E
}

pub trait Decoder[C, E] {
    fn peek(var self): SerializationEvent? ! E
    fn next(var self): SerializationEvent ! E
    fn base64(var self): Bytes ! E
    fn own(var self, event: SerializationEvent): SerializationEvent ! E
    fn reject(var self, error: SerializationError): E
}

pub trait Encode[C] {
    fn encode[E, S: Encoder[C, E]](value: Self, var encoder: S): Unit ! E
}

pub trait Decode[C] {
    fn decode[E, D: Decoder[C, E]](var decoder: D): Self ! E
}
~~~

`SerializationEvent` contiene scalars con anchura explícita, arrays, maps con
`MapKey`, records (`StartRecord`/`Field`) y enums (`StartEnum`). La máquina de
eventos exige una raíz única, fields únicos, claves y payloads completos,
longitudes exactas cuando se declaran y cierres balanceados. Los frames son
explícitos y acotados; no se usa la pila de llamadas del host.

`Encoder.base64` y `Decoder.base64` son la única operación común para la policy
de bytes representados como Base64 RFC 4648 canónico. El derive JSON la activa
solo con `@json(base64)`; no construye `Value` ni acepta alfabetos/padding
alternativos.

`Encode` recibe el valor por ownership como parámetro asociado, de modo que un
tipo affine se consume una sola vez y no requiere un receiver ficticio. El
encoder/decoder se pasa como `var` para que avance su estado sin boxing ni
allocation por evento. `peek` observa el siguiente evento sin consumirlo y
devuelve `none` únicamente cuando no queda ninguno; permite componer `Option`,
arrays y maps sin buffering ni retroceso. Los payloads de texto/bytes de
`next` son vistas hasta el siguiente evento y `own` es la única materialización
estable. `reject` traduce un `SerializationError` estructural al error nominal
exacto del codec; no borra el tipo, no crea una union y no habilita
conversiones implícitas.

La distribución registra providers para:

~~~tondo pseudocode
import std.serialization

derive serialization.Encode[Json] + serialization.Decode[Json]
    + serialization.Encode[MessagePack] + serialization.Decode[MessagePack]
    for User
~~~

El comportamiento derivado:

- el encode visita fields en orden de declaración; el decode usa una máquina
  estática de slots y acepta cualquier orden de fields;
- utiliza el spelling declarado de cada field como nombre externo por defecto,
  incluidos fields privados cuya autorización concede `derive`;
- conserva discriminante y payload de enum de forma estructural;
- introduce bounds mínimos sobre parámetros genéricos;
- rechaza en compile time un field sin implementación requerida; y
- consume cada field conocido como máximo una vez y publica el valor solo tras
  validar todos sus componentes; `DuplicateField`, `UnknownField` y
  `MissingField` son fallos distintos;
- reconstruye `Option[T]` ausentes como `none` y consume fields `@ignore`
  aplicando su policy antes de publicar `none`;
- para MessagePack materializa records y enums como maps de claves string; para
  Protobuf usa tokens `#number` que el adapter baja al field tag sin inferir
  desde el orden.

Renombrar, omitir, aplanar o transformar fields cambia un contrato de wire y no
se esconde en attributes generales. Se expresa con un `impl` manual o con un
tipo DTO explícito. Protobuf no utiliza este derive para inferir field numbers.
El error común conserva tipo, path, offset y límites; cada codec puede añadir
sus variantes nominales sin cambiar la máquina compartida.

La evidencia ejecutable de este owner es `STD-A-SER-EVIDENCE-001`: enlaza el
contrato común con los traits `Encoder`/`Decoder` y `Encode`/`Decode`, la máquina
de eventos con frames explícitos, `Value`/`ValueView`/`Raw`, paths, límites,
chunking y publicación atómica. Los providers herméticos de derive conservan
la identidad del codec, generan output determinista, source maps y diagnostics
reproducibles para records, enums, newtypes, genéricos y attributes. `HOST` es
`not-applicable`; `STD-A-FUZZ-001` promueve el fuzz owner-aware del protocolo,
mientras los baselines de coste y `STD-CONF-001` continúan como promoción
posterior.

La evidencia ejecutable de `std.json` es `STD-A-JSON-EVIDENCE-001`. Cierra las
rutas typed, dynamic y streaming sobre el mismo parser de frames explícitos,
incluyendo `JsonNumber` exacto, `Value`/`ValueView`/`Raw`, límites antes de
crecer, políticas de duplicados y errores terminales. La suite comprueba
Unicode, números, JCS/RFC 8785, canonicalización, fragmentación de un byte,
interoperabilidad bidireccional con `serde_json` y los adaptadores de
`Encode`/`Decode` sin DOM. `HOST` es `not-applicable`: el bridge del compilador
no introduce una capability ni semántica dependiente del target. El fuzz
dedicado por operación, los baselines de allocations/memoria por target y
`STD-CONF-001` siguen explícitamente como promoción posterior.

### 14.7 `std.reflect`

`std.reflect` implementa reflection descriptiva, estática y retenida de forma
explícita. No es un sistema de objetos dinámico ni una puerta lateral hacia el
compilador. Su superficie completa es:

~~~tondo pseudocode
pub fn typeInfo[T](): TypeInfo

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
    Discard
    Equatable
    Key
    Send
    Share
}

pub type TypeInfo
pub type FieldInfo
pub type VariantInfo
pub type ParameterInfo
pub type FunctionInfo
~~~

`TypeInfo` es un handle de valor inmutable, barato de copiar, `Send + Share` y
respaldado por metadata read-only del artefacto. Expone exactamente:

- `id(): TypeId`, `qualifiedName(): String` y `kind(): TypeKind`;
- `genericArguments(): Array[TypeInfo]` en orden de declaración;
- `capabilities(): Set[TypeCapability]` con solo capacidades demostradas;
- `fields(): Array[FieldInfo]` para records y payloads record públicos;
- `variants(): Array[VariantInfo]` para enums públicos;
- `tupleElements(): Array[TypeInfo]` para tuple y union, preservando el orden
  canónico del tipo; y
- `function(): FunctionInfo?` para funciones.

Consultar una vista que no corresponde al kind devuelve la colección vacía o
`none`; no inventa un `ReflectError`. `typeInfo[T]()` tampoco falla en runtime:
un `T` no describible se rechaza durante compilación. Por tanto `std.reflect`
no publica un tipo de error runtime en 0.1. Esta ausencia es parte del contrato,
no una omisión provisional.

`FieldInfo` contiene únicamente `name`, `type`, ordinal declarativo y docs
retenidas; `VariantInfo`, nombre, ordinal y descriptores de su payload;
`ParameterInfo`, posición, tipo y modo (`value`, `ref`, `mut` o `var`); y
`FunctionInfo`, parámetros, outcome, variadicidad y flags `suspendible`/`unsafe`.
Todas las colecciones devueltas son valores inmutables canónicos. Sus elementos
no dependen de direcciones, vtables ni del orden de un registro global.

Solo se publican fields, variants y firmas visibles desde el punto donde se
formó `typeInfo[T]()`. Un field privado no aparece ni siquiera si la consulta
se escribió en su propio módulo. Los descriptores nunca contienen getters,
setters, constructores, function handles, offsets, tamaño, alineación, ABI,
direcciones, discriminantes físicos, estado del GC ni bytes de un valor.

`TypeId` es una identidad opaca por artefacto. Admite igualdad y hash para ser
key durante esa ejecución exacta; no admite constructor desde enteros/string,
orden total, acceso a bits ni encoding estándar. Copiarlo a disco, red,
schemas, caches o IPC carece de significado. Dos artefactos pueden asignar IDs
distintos al mismo fuente y dos toolchains no prometen IDs comparables.

#### Retención y DCE

Cada instanciación concreta de `typeInfo[T]()` crea una raíz explícita de
metadata. La clausura conserva solo `T` y los tipos nombrados por sus
descriptores públicos alcanzables. No conserva cuerpos, valores, private
members ni tipos no alcanzables. Genéricos se retienen por instanciación
concreta, no como un registro abierto de todas sus posibles aplicaciones.

Si no existe una raíz `typeInfo[T]()` alcanzable, el linker puede eliminar toda
la metadata de `T`. Si elimina el código que contenía la consulta, puede
eliminar también su clausura. Añadir reflection sobre `A` no retiene metadata
de un `B` no alcanzable. Estos tres casos —raíz viva, raíz eliminada y tipo no
alcanzable— son los oracles normativos de DCE para `REFLECT-IMPL-001`.

No existe `allTypes`, lookup por nombre/ID, registro global, carga dinámica,
reflection de valores, `get`/`set`/`invoke`, construcción dinámica ni
attributes ejecutables. El único punto de entrada es el genérico estático
`typeInfo[T]()`. La implementación puede resolver y compactar toda su clausura
en compile time; una consulta no ejecuta búsqueda, allocation obligatoria ni
locking global.

Los usos previstos son diagnostics de aplicación, documentación y herramientas
que necesitan describir un tipo conocido estáticamente. JSON, MessagePack y
Protobuf no dependen de este módulo: usan impls generados y dispatch estático.

La evidencia machine-readable de este contrato vive en
`testing/stdlib-reflect.json` y en la matriz por celdas de
`testing/stdlib-owner-evidence.json`, bajo
`STD-A-REFLECT-EVIDENCE-001`. Esa evidencia separa las raíces explícitas, la
clausura pública, privacidad, identidad local al artefacto, ausencia de
reflection de valores y los límites de coste; `HOST` es
`not-applicable` porque el módulo es metadata-only. La matriz no convierte
estas pruebas en una promesa de reflection runtime ni cierra por sí sola los
gates globales de conformidad o rendimiento.

### 14.8 `std.meta`

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

La evidencia executable de este owner se registra en
`testing/stdlib-meta.json` y `testing/stdlib-owner-evidence.json`. El registro
mantiene separadas las celdas `MODEL`, `TEST` y `FUZZ`, marca `HOST` como
`not-applicable` por la naturaleza build-only de `tondo-meta` y deja visibles
los presupuestos de compile-time y tamaño de fuente generada hasta su captura
de promoción. No se interpreta esta evidencia como una publicación de la
stdlib ni como una API runtime.

### 14.9 `std.json`

`std.json` implementa JSON UTF-8 conforme a
[RFC 8259](https://www.rfc-editor.org/rfc/rfc8259.html). Su superficie contiene:

- encode/decode tipado mediante `Encode` y `Decode`;
- `JsonReader`, `JsonWriter` y eventos incrementales;
- `Value` para uso sin schema;
- `JsonNumber`, que conserva una representación numérica validada sin forzar
  pérdida inmediata a `Float64`; y
- opciones nominales de límites, campos desconocidos, duplicados y números no
  representables.

El modo estricto por defecto rechaza trailing data, UTF-8 inválido, escapes
inválidos, profundidad excedida, claves duplicadas y números fuera de la
política solicitada. Ignorar campos desconocidos o elegir una política distinta
requiere `DecodeOptions` explícito; no depende de globals.

El encoder ordinario conserva el orden declarativo de un tipo y el orden de
inserción de `Value.Object`. `encodeCanonical` implementa
[RFC 8785 (JCS)](https://www.rfc-editor.org/rfc/rfc8785.html):
ordena properties según esa norma, usa su serialización de strings y números y
emite UTF-8 sin whitespace. Como JCS restringe el dominio a I-JSON, la operación
devuelve un error si un `JsonNumber` no puede representarse en ese dominio sin
cambiar su valor; nunca redondea silenciosamente. La operación normal no promete
que whitespace o spelling coincidan con el input.

Los enums derivados usan un único objeto externally tagged con exactamente un
miembro: el nombre de la variante es la clave; una variante unit usa `null`,
una tuple usa un array de aridad exacta y una record usa un object con sus
fields derivados. Esta forma es cerrada: se rechazan variantes desconocidas,
payloads con forma o aridad distintas y miembros exteriores adicionales.

Un error contiene clase estable, byte offset, línea/columna cuando puedan
calcularse y path estructural. No copia automáticamente el documento, el valor
de un field ni datos potencialmente secretos dentro de su mensaje.

La API fuente única de 0.1 es:

~~~tondo pseudocode
pub fn parse(input: Bytes, options: JsonDecodeOptions): Value ! JsonError
pub fn parseView(input: Bytes, options: JsonDecodeOptions): ValueView ! JsonError
pub fn decode[T: Decode[Json]](input: Bytes, options: JsonDecodeOptions): T ! JsonError
pub fn encode[T: Encode[Json]](value: T, options: JsonEncodeOptions): Bytes ! JsonError
pub fn validate(input: Bytes, options: JsonDecodeOptions): Unit ! JsonError
pub fn canonicalize(input: Bytes, options: JsonDecodeOptions): Bytes ! JsonError
pub fn encodeCanonical(value: Value, limits: JsonLimits): Bytes ! JsonError
pub fn raw(input: Bytes): Raw ! JsonError
pub unsafe fn rawUnchecked(input: Bytes): Raw
pub fn JsonReader.next(var self): JsonEvent? ! JsonError
pub fn JsonReader.own(var self, event: JsonEvent): JsonEvent ! JsonError
pub fn JsonWriter.write(var self, event: JsonEvent): Unit ! JsonError suspends
pub fn JsonWriter.finish(var self): Unit ! JsonError suspends
~~~

Los tipos, options, limits, eventos y errores exhaustivos están cerrados en el
[contrato fuente de `std.json`](./docs/contracts/stdlib-json.md). `next` devuelve
`none` una sola vez tras la raíz y reader/writer son terminales tras error.

### 14.10 `std.messagepack`

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
pub fn parse(input: Bytes, options: MessagePackDecodeOptions): Value ! MessagePackError
pub fn parseView(input: Bytes, options: MessagePackDecodeOptions): ValueView ! MessagePackError
pub fn decode[T: Decode[MessagePack]](input: Bytes, options: MessagePackDecodeOptions): T ! MessagePackError
pub fn encode(value: Value, options: MessagePackEncodeOptions): Bytes ! MessagePackError
pub fn encode[T: Encode[MessagePack]](value: T, options: MessagePackEncodeOptions): Bytes ! MessagePackError
pub fn validate(input: Bytes, options: MessagePackDecodeOptions): Unit ! MessagePackError
pub fn encodeDeterministic(value: Value, limits: MessagePackLimits): Bytes ! MessagePackError
pub fn raw(input: Bytes, options: MessagePackDecodeOptions): Raw ! MessagePackError
pub unsafe fn rawUnchecked(input: Bytes): Raw
pub fn MessagePackReader.next(var self): MessagePackEvent? ! MessagePackError
pub fn MessagePackReader.own(var self, event: MessagePackEvent): MessagePackEvent ! MessagePackError
pub fn MessagePackWriter.write(var self, event: MessagePackEvent): Unit ! MessagePackError suspends
pub fn MessagePackWriter.finish(var self): Unit ! MessagePackError suspends
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
`STD-A-FUZZ-001` promueve el fuzz owner-aware por operación; baselines de
allocations/memoria por target y `STD-CONF-001` siguen como promoción posterior.

La evidencia ejecutable de `std.protobuf` es `STD-A-PROTOBUF-EVIDENCE-001`.
Combina el wire portable con la frontera build-only schema-first: TOML
declarativo, grafo cerrado de imports, proto3, presencia, repeated/packed,
maps, oneof, enums abiertos, unknown fields/grupos, evolución segura e
incompatible, descriptor raíz, reader/writer schema-bound, determinismo y
límites finitos. La suite comprueba fragmentación de un byte y compatibilidad
bidireccional con `prost`, además de que el generator produzca identidades
estables y errores de schema sin inputs ambientales. `HOST` es
`not-applicable`; `STD-A-FUZZ-001` promueve el fuzz owner-aware de
schema/operaciones; baselines de allocations/memoria por target y
`STD-CONF-001` siguen como promoción posterior.

La evidencia ejecutable de `std.testing` es `STD-A-TESTING-EVIDENCE-001`.
El módulo es test-only y su bridge `HOST` verificado pertenece al worker del
runner: no concede capabilities ni puede importarse desde producción. La
evidencia enlaza las 25 firmas públicas con assertions de ownership prestado,
diffs acotados, tolerancias finitas, consumo explícito de `Option`/`Result`,
temporales afines bajo raíz privada, generators replayables, shrinking sellado,
control terminal, virtual time y los fixtures de dogfooding del runner.
`STD-A-FUZZ-001` promueve la ruta owner-aware; baselines completos de coste o
`STD-CONF-001` permanecen parciales y visibles en la matriz.

La coordinación `STD-TEST-001` queda registrada en
`testing/stdlib-test-coordination.json`: sus 22 owners A, 214 firmas públicas y
171 requisitos se vinculan a 66 leyes de modelo, comandos de test y campañas
de fuzz. El registro se genera desde la evidencia de owners, la auditoría de
API y la matriz normativa; cada superficie debe tener una ley de modelo y
`STD-A-FUZZ-001` mantiene las 22 rutas owner-aware promovidas. Esta coordinación
no cierra la conformidad global.

La coordinación `STD-CONF-001` queda registrada en
`testing/stdlib-conformance-coordination.json`: contiene los 22 owners de
`STD-0.1A` y una fila `CONF` explícita para cada firma o requisito de la matriz
(214 firmas y 171 requisitos). Cada fila conserva el estado actual de la
matriz, una razón obligatoria para `partial`/`pending`, referencias
reproducibles y comandos. El registro cruza la matriz normativa, la auditoría
de API, la evidencia de owners, la coordinación de modelos y el harness
externo de codecs; no permite declarar `verified` sin la observación de la
fila ni convierte la coordinación en promoción. `std.async` conserva la
implementación de VM verificada y permanece `partial` únicamente en las celdas
de conformance global, fuzz y rendimiento; su contrato concreto tiene siete
filas, cinco callable auditadas y las rutas directa y `spawn` de `collect(limit:)`
verificadas en `STD-A-ASYNC-IMPL-001`. La promoción sigue
`not-promoted` hasta `STD-DOC-001` y la conformance pública completa.

La coordinación `STD-DOC-001` queda registrada en
`testing/stdlib-documentation.json`. Cada owner tiene un contrato normativo y
una lista de documentos, además de tres fronteras que nunca se mezclan:
`kernel` describe la implementación portable o intrínseca, `bridge` describe
el adaptador compiler/VM/host (o `not-applicable`) y `public_api` refleja
únicamente las firmas de la auditoría pública. Una API con firmas ausentes o
gaps conserva `partial`; una superficie intrínseca/build-only sin filas de
firma declara `not-applicable` con razón. El registro enlaza 31 ejemplos
verificables: cada caso runtime exige su fixture `.exit` y `.stdout`/`.codes`,
los casos externos apuntan al harness independiente y los providers
compiler/meta apuntan a sus tests de build. `std.meta` y `std.reflect` no
tienen caso runtime por diseño y lo declaran de forma explícita. Esta clausura
documental describe el draft actual, no publica una release ni promueve las
matrices de implementación, rendimiento o conformance.

### 14.11 `std.protobuf`

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
pub fn decode[T: Decode[Protobuf]](input: Bytes, options: ProtoDecodeOptions): T ! ProtoError
pub fn encode[T: Encode[Protobuf]](value: T, options: ProtoEncodeOptions): Bytes ! ProtoError
pub fn encodeDeterministic[T: Encode[Protobuf]](value: T, limits: ProtoLimits): Bytes ! ProtoError
pub fn validate[T](input: Bytes, options: ProtoDecodeOptions): Unit ! ProtoError
pub fn descriptor[T](): ProtoDescriptor[T]
pub fn ProtoReader[T].next(var self): ProtoEvent? ! ProtoError
pub fn ProtoReader[T].own(var self, event: ProtoEvent): ProtoEvent ! ProtoError
pub fn ProtoWriter[T].write(var self, event: ProtoEvent): Unit ! ProtoError suspends
pub fn ProtoWriter[T].finish(var self): Unit ! ProtoError suspends
~~~

La implementación portable vive en `crates/tondo-stdlib/src/protobuf_api.rs`:
el checker schema-first, el parser/reader y el writer usan límites y stacks
explícitos, y `encode_static`/`decode_static` atraviesan
`Encode[Protobuf]`/`Decode[Protobuf]` sin materializar `serialization.Value`.
Los helpers Rust `encode`/`decode` son únicamente el bridge de compatibilidad;
`ProtoValue` y `Raw<Protobuf>` también quedan limitados a ese bridge y no
introducen un alias dinámico de `serialization.Value`. La conformance oficial
e interoperabilidad independiente están cerradas por
`STD-CODEC-CONF-001`, cuyo registro coordina `serde_json`, `rmpv` y `prost`,
fragmentación, truncación, límites y preservación de unknown fields.

El mapping generado, el descriptor explícito, la evolución contra baseline TOML,
los eventos y los errores de wire/build están cerrados en el [contrato fuente de
`std.protobuf`](./docs/contracts/stdlib-protobuf.md).

### 14.12 Reglas de rendimiento de codecs

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
  round trips, corpus adversario, implementaciones externas y comparación
  scalar/optimized. La identidad ejecutable de la conformance de codecs queda
  en `testing/stdlib-codec-conformance.json` y no se confunde con un round-trip
  contra el propio bridge.

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

- Síncrona o suspendible.
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

Una API suspendible prueba además:

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

El diagnóstico dinámico sigue la frontera de
[`docs/contracts/diagnostic-tooling.md`](./docs/contracts/diagnostic-tooling.md):
race detection, retención/leaks y crash dumps pertenecen al compilador, runtime
y CLI. `std.testing` solo conserva el contexto de test, logs, tags y artifacts
que el runner ya define; el catálogo STD-0.1 no añade `std.race`, `std.leaks` ni
`std.crash`, ni duplica APIs sync/async para instrumentarlas. Los hooks internos
que necesite la VM o el backend nativo no forman parte de la superficie pública.

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
- Streams suspendible generales o reactivos; los protocolos acotados de byte I/O de
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

`std.console` es el owner capability-gated de los tres streams del proceso.
Importar el módulo no concede la capability `console`; un target que no la
declare rechaza el programa estáticamente con `E1008`. La superficie pública
única es:

~~~tondo
pub fn stdin(): std.io.Reader ! ConsoleError
pub fn stdout(): std.io.Writer ! ConsoleError
pub fn stderr(): std.io.Writer ! ConsoleError
pub fn readLine(input: var std.io.Reader): String? ! ConsoleError suspends
pub fn print(value: String): Unit ! ConsoleError
pub fn println(value: String): Unit ! ConsoleError
pub fn flush(): Unit ! ConsoleError suspends
pub enum ConsoleError { Unavailable, Closed, Cancelled, Io(std.io.IoError) }
~~~

Los handles son tokens distintos y reutilizan los protocolos de `std.io`; no
hay terminal, locale, encoding o newline ambiental implícito. `readLine` solo
avanza el cursor después de aceptar una línea UTF-8 completa, devuelve `none`
en EOF y devuelve un `ConsoleError` tipado sin consumir datos cuando recibe
UTF-8 inválido o un handle de output. `print` y `println` solo anexan al buffer
de salida ordenado del runtime, nunca esperan al sistema operativo y por eso no
declaran `suspends`; `println` añade un único LF. Ninguna de las dos hace flush
implícito. `flush` es la única frontera de entrega suspendible y terminal para
el writer correspondiente. Partial I/O, límites, progreso, cancelación y
cleanup siguen las reglas de `std.io`; el adaptador no introduce una segunda
API sync/async ni duplica buffers. Los mensajes del host se mantienen opacos y
nunca publican rutas o detalles dependientes del sistema operativo.

El shim bootstrap histórico queda como bridge interno de compatibilidad y no
es una segunda identidad pública. La evidencia `STD-A-CONSOLE-EVIDENCE-001`
debe enlazar las siete firmas con HIR/lowering, bytecode/VM, el adaptador de
capability, partial I/O, errores atómicos, fixture, coste y documentación antes
de la promoción de S1A.

### 19.2 `std.process`

El bootstrap expone una superficie cerrada de procesos, `Bytes` opaco y
`process.args()`. STD-0.1:

- Conserva `Command` y `Pipeline` como planes intrínsecos.
- Reutiliza el comportamiento probado cuando siga siendo correcto.
- Migra el owner binario a `std.bytes.Bytes`.
- Migra argumentos runtime a `std.env`.
- Sustituye errores y tipos provisionales por propietarios canónicos.
- Mantiene argv exacto, shell explícito, pipes, backpressure y cleanup.
- `ProcessOutput` conserva `stdout` y `stderr` separados y ofrece además
  `combined`, una captura byte a byte en el orden observado por el host.
- `pipe` equivale a `stdout | stdin`; `Command.mergeStderr()` y
  `Pipeline.mergeStderr()` conectan `stdout + stderr | stdin` y son la forma
  tipada de `|&`/`2>&1 |` sin depender de un shell.
- Las redirecciones shell (`2>&1`, `2>file`, `&>file`, `>/dev/null`) solo se
  interpretan dentro de `shell(...)`. `command(...)` nunca analiza sintaxis de
  shell; el modo script podrá bajar sus operadores de redirección a estos
  planes tipados y exigirá las capabilities de los recursos que abra.

El corpus bootstrap no se reescribe. Un proyecto adopta STD-0.1
seleccionando el nuevo PackageId y lockfile.

La evidencia ejecutable de esta frontera es `STD-A-PROC-EVIDENCE-001`: enlaza
las diecisiete firmas públicas con el contrato hosted, la capability estática,
los planes inertes y handles terminales, HIR/lowering, bytecode/VM, el host de
procesos y los fixtures M8. El gate cubre backpressure, streams separados y
`combined`, redirección de stderr, estados/errores, cancelación y reaping;
`STD-A-FUZZ-001` promueve el fuzz owner-aware, mientras baselines por target y
`STD-CONF-001` siguen siendo promoción posterior.

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
- [ ] Cada API documenta errores, pánicos, ownership, suspendible, orden y coste.
- [ ] Todas las capabilities ausentes producen rechazo estático.
- [ ] Los tipos y errores tienen un propietario canónico.
- [ ] Los módulos core no dependen de host.
- [ ] Los recursos afines tienen una operación terminal y cleanup probado.
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
- [ ] El corpus bootstrap Tondo 0.1 permanece verificable sin cambios.
- [ ] Todo lo diferido está ausente o identificado fuera de `std`.

Hasta cerrar esta lista, el documento puede ser normativo para las reglas base
sin que exista una release pública completa de STD-0.1.
