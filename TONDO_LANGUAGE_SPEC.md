# Tondo: especificación del lenguaje

**Estado:** borrador de diseño 0.1  
**Revisión del Markdown:** 0.1-draft.8 — 2026-07-21  
**Nombre:** Tondo  
**Extensión:** `.to`  
**Lema:** **Pequeño por diseño, completo en la práctica.**

El nombre **Tondo** combina **TON**y con re**DO**ndo. También evoca una forma circular, compacta y completa: una imagen coherente con un lenguaje de núcleo pequeño que busca cubrir los casos habituales sin ceremonias innecesarias.

> Esta especificación define el lenguaje, no su librería estándar. Algunos nombres de tipos intrínsecos y protocolos mínimos se mencionan porque forman parte del sistema de tipos, pero sus APIs completas se especificarán por separado.

En este documento, **debe** expresa un requisito de conformidad, **no puede** expresa una prohibición, **puede** expresa una capacidad permitida y **se recomienda** expresa orientación no obligatoria. Los ejemplos marcados como ilustrativos pueden usar nombres de la futura librería estándar, pero no alteran la gramática.

## Índice

1. [Resumen](#1-resumen)
2. [Objetivos y no objetivos](#2-objetivos-y-no-objetivos)
3. [Inspiraciones](#3-inspiraciones)
4. [Modelo conceptual](#4-modelo-conceptual)
5. [Código fuente y léxico](#5-código-fuente-y-léxico)
6. [Estructura de programas y módulos](#6-estructura-de-programas-y-módulos)
7. [Declaraciones, nombres y visibilidad](#7-declaraciones-nombres-y-visibilidad)
8. [Sistema de tipos](#8-sistema-de-tipos)
9. [Tipos compuestos](#9-tipos-compuestos)
10. [Colecciones intrínsecas](#10-colecciones-intrínsecas)
11. [Funciones, métodos y cierres](#11-funciones-métodos-y-cierres)
12. [Genéricos y traits](#12-genéricos-y-traits)
13. [Expresiones y control de flujo](#13-expresiones-y-control-de-flujo)
14. [Patrones y `match`](#14-patrones-y-match)
15. [Errores recuperables y pánicos](#15-errores-recuperables-y-pánicos)
16. [Mutabilidad, préstamos, memoria y concurrencia](#16-mutabilidad-préstamos-memoria-y-concurrencia)
17. [Operadores](#17-operadores)
18. [Semántica numérica](#18-semántica-numérica)
19. [Texto y Unicode](#19-texto-y-unicode)
20. [Programas ejecutables, scripts y procesos](#20-programas-ejecutables-scripts-y-procesos)
21. [Formato canónico y documentación](#21-formato-canónico-y-documentación)
22. [Diagnósticos y herramientas](#22-diagnósticos-y-herramientas)
23. [Gramática de referencia](#23-gramática-de-referencia)
24. [Ejemplos integrados](#24-ejemplos-integrados)
25. [Características deliberadamente ausentes](#25-características-deliberadamente-ausentes)
26. [Frontera con la librería estándar](#26-frontera-con-la-librería-estándar)

Apéndices:

- [Apéndice A. Referencia rápida](#apéndice-a-referencia-rápida)
- [Apéndice B. Declaración de diseño](#apéndice-b-declaración-de-diseño)
- [Apéndice C. Fixtures normativos de documentación](#apéndice-c-fixtures-normativos-de-documentación)

---

## 1. Resumen

Tondo es un lenguaje de propósito general, compilado, estáticamente tipado y con gestión automática de memoria. Está diseñado para que programas y APIs puedan entenderse localmente, tanto por personas como por herramientas y modelos de lenguaje.

Su superficie pretende combinar:

- La regularidad, los módulos y la legibilidad de Go.
- Los records, procedimientos explícitos y `defer` de Odin.
- `Option`, `Result`, enums algebraicos y `match` exhaustivo de Rust.
- La ergonomía de expresiones, strings y nulabilidad segura de Kotlin.
- El modo de razonamiento directo y la ligereza conceptual de Lua.
- La indexación, slicing y ergonomía de colecciones de Python.
- Las operaciones vectorizadas y la semántica aritmética de NumPy.
- La idea de vistas compactas de los slices de Go y `Span<T>` de C#, sin lifetimes escritos por el usuario.
- La concurrencia estructurada de Swift y Kotlin, manteniendo resultados async ordinarios.
- La composición de procesos de Bash y Nushell sin shell implícito.
- La separación entre referencias seguras con identidad y punteros raw confinados a `unsafe`.

Tondo prioriza:

1. Una única construcción canónica por concepto.
2. Firmas que describen el contrato completo.
3. Inferencia local, nunca “magia” global.
4. Errores recuperables expresados como valores.
5. Inmutabilidad y semántica de valor por defecto.
6. Mutación visible y temporal.
7. Orden de evaluación definido y concurrencia estructurada.
8. Gramática sencilla de analizar y reformatear.
9. Diagnósticos estructurados que una herramienta pueda consumir.
10. Ausencia de comportamiento oculto dependiente del entorno.

Un programa pequeño:

~~~tondo
import std.console

enum AppError {
    InvalidName(String)
}

fn greet(name: String): String ! AppError {
    if name.isEmpty() {
        fail AppError.InvalidName(name)
    }

    "Hola, {name}"
}

fn main(): !AppError {
    let message = greet("Ada")?
    console.print(message)
}
~~~

---

## 2. Objetivos y no objetivos

### 2.1 Objetivos

#### Pequeño

El núcleo debe poder describirse completamente. Una persona debería ser capaz de mantener el modelo mental del lenguaje sin memorizar excepciones históricas, múltiples sistemas de objetos o reglas implícitas de resolución.

#### Predecible

El mismo código debe conservar la misma semántica en todas las compilaciones compatibles. El modo de optimización no puede cambiar overflow, orden de evaluación, igualdad, iteración ni manejo de errores. El interleaving entre tareas concurrentes es deliberadamente no especificado salvo por las relaciones de sincronización visibles; código seguro continúa libre de data races.

#### Local

Para comprender una función deben bastar su cuerpo, su firma, los tipos utilizados y las declaraciones importadas directamente. No existen conversiones arbitrarias, monkey patching, métodos añadidos globalmente ni sobrecarga por búsqueda abierta.

#### Seguro por defecto

En código seguro no hay `null`, referencias colgantes, acceso de memoria sin comprobar, data races autorizadas por defecto ni excepciones invisibles en las firmas. Las operaciones que no pueden ofrecer esas garantías quedan delimitadas por `unsafe`.

#### Conciso sin ser críptico

Se elimina boilerplate mecánico mediante inferencia local, expresiones de bloque, retornos implícitos, records, slices y propagación con `?`. No se eliminan delimitadores que aportan información al lector o al compilador.

#### Amigable para LLMs y herramientas

La sintaxis tiene pocas variantes, el formateador produce una representación canónica y el compilador puede emitir errores con códigos estables, rangos precisos, tipos esperados y reparaciones sugeridas.

### 2.2 No objetivos

Tondo 0.1 no pretende:

- Ser compatible sintácticamente con otro lenguaje.
- Reproducir todo el sistema de ownership de Rust.
- Ofrecer gestión manual de memoria en el lenguaje seguro.
- Imponer a todas las implementaciones una única estrategia interna de gestión
  automática de memoria.
- Tener clases, herencia o jerarquías nominales de objetos.
- Tener metaprogramación arbitraria, macros textuales o evaluación dinámica.
- Tener excepciones recuperables mediante `throw`/`catch`.
- Tener sobrecarga de operadores definida por el usuario.
- Inferir firmas públicas ni tipos a través de módulos.
- Incluir un tipo dinámico universal `Any` u `Object`.
- Incorporar broadcasting multidimensional completo en el núcleo.
- Exponer futures o tasks en todas las firmas asíncronas.
- Permitir tareas separadas, memoria compartida mutable o procesos shell de forma implícita.
- Hacer que los punteros raw formen parte del lenguaje seguro.
- Definir el formato concreto del manifiesto, publicación y descarga de paquetes,
  ni las APIs de red, archivos, fechas, serialización o sistema; sus contratos
  pertenecen al toolchain o a la librería estándar. Esta especificación sí fija
  la identidad semántica y los inputs que esas herramientas deben entregar al
  compilador.

---

## 3. Inspiraciones

### 3.1 Go

Se adoptan:

- Pocos conceptos centrales.
- Un solo bucle.
- Imports explícitos.
- Herramientas oficiales y formato canónico.
- Slices como inspiración de representación.
- Composición en lugar de herencia.
- Canales tipados como inspiración para comunicar tareas e hilos.

No se adoptan:

- Visibilidad basada en mayúsculas.
- `nil`.
- Maps con valores cero ambiguos.
- Interfaces satisfechas de forma completamente implícita.
- Errores que obligan a repetir `if err != nil`.
- Semántica de alias mutable implícita como valor por defecto.
- Lanzamiento de trabajo no estructurado mediante una operación equivalente a `go` sin scope propietario.

### 3.2 Rust

Se adoptan:

- `Option[T]` y `Result[T, E]`.
- Enums con payload.
- Pattern matching exhaustivo.
- Ausencia de `null`.
- Mutabilidad explícita.
- Distinción entre errores recuperables y pánicos.
- Traits e implementaciones explícitas.
- Resultados opacos estáticos para conservar un tipo concreto sin exponer su
  representación.

No se adoptan:

- Lifetimes escritos en firmas normales.
- Borrow checker general con anotaciones de vida.
- Macros como parte necesaria del uso cotidiano.
- Múltiples formas de conversión implícita mediante traits.
- Sobrecarga de operadores por traits.

### 3.3 Kotlin

Se adoptan:

- `if` y `match` como expresiones.
- Strings interpolados.
- Tipos opcionales compactos.
- Inferencia local.
- Ergonomía en llamadas con argumentos nombrados.
- Funciones asíncronas cuyo contrato muestra el valor lógico que producen.
- Concurrencia estructurada y cancelación ligada a scopes.

No se adoptan:

- Plataforma de clases y herencia.
- Referencias anulables.
- Sobrecarga extensa.
- Excepciones como contrato invisible.
- Jobs globales o tareas separadas implícitamente de su llamador.

### 3.4 Lua

Se adoptan:

- Lectura directa.
- Poca ceremonia.
- Adecuación para programas pequeños.

No se adoptan:

- Tipado dinámico como semántica principal.
- Una tabla universal que represente arrays, objetos y maps a la vez.
- Metatables y mutación global del comportamiento.

### 3.5 Odin

Se adoptan:

- Procedimientos y datos explícitos.
- `defer`.
- Énfasis en legibilidad y costos visibles.
- Pocos mecanismos de abstracción.
- Una frontera `unsafe` pequeña y explícita para interoperabilidad de bajo nivel.

No se adoptan:

- Contextos implícitos.
- Punteros o memoria manual en código seguro.

### 3.6 Python y NumPy

Se adoptan:

- Literales claros de colecciones.
- Índices negativos.
- Slices `inicio:fin:paso`.
- Longitud dinámica.
- Maps ordenados por inserción.
- Aritmética elemento a elemento entre arrays.
- Broadcasting de escalares.

No se adoptan:

- Tipado dinámico general.
- Truthiness.
- Mezcla implícita de tipos numéricos.
- `+` como concatenación de listas.
- `*` como repetición de listas.
- Excepciones por ausencia esperable de claves.
- Broadcasting multidimensional implícito dentro del núcleo.

### 3.7 C#

Se adopta conceptualmente:

- La distinción entre paso por valor, `ref` de solo lectura y acceso mutable
  temporal.
- La idea de una vista compacta sobre memoria contigua.

No se adoptan:

- Jerarquía de clases.
- `null`.
- Sobrecarga extensa.
- Dos familias públicas separadas `Span`/`ReadOnlySpan`; Tondo expresa acceso
  compartido mediante `ref`, acceso exclusivo de extensión fija mediante `mut` y
  reemplazo o redimensionado mediante `var`, sin cambiar el tipo `Array[T]`.
- `Task[T]` o `ValueTask[T]` como envoltorio obligatorio en cada firma asíncrona.

### 3.8 Bash y Nushell

Se adoptan:

- Programas pequeños ejecutables directamente como scripts.
- Procesos y pipelines como valores componibles.
- El operador `|` para conectar stdout de un proceso con stdin del siguiente.
- Conversión explícita entre streams de bytes y valores estructurados.

No se adoptan:

- Strings de shell ejecutados implícitamente.
- Expansión textual, quoting contextual o globbing oculto.
- Imports con efectos.
- Códigos de salida no cero convertidos siempre en excepciones o pánicos.

### 3.9 Swift y Zig

Se adoptan conceptualmente:

- `async` como parte explícita del contrato de una función y `await` en cada suspensión.
- El resultado lógico ordinario de una función async, separado del handle creado al lanzarla concurrentemente.
- Lifetimes estructurados para tareas hijas.
- Separación entre semántica del lenguaje, scheduler e implementación de frames async.
- ARC y las comprobaciones de unicidad como inspiración de implementación para
  identidad y buffers copy-on-write, complementados con recolección automática
  de ciclos.

No se adoptan:

- Frames de coroutine, continuations o allocators visibles en firmas cotidianas.
- Cancelación manual obligatoria para cada llamada secuencial.
- Una ABI async fijada prematuramente por la sintaxis fuente.
- La obligación de romper manualmente cada ciclo de referencias fuertes.

---

## 4. Modelo conceptual

### 4.1 Valores e identidad explícita

Records, enums, arrays, maps, sets y strings tienen semántica de valor. Cuando cumplen `Copy`, asignarlos, pasarlos o devolverlos produce una copia lógica y la implementación puede compartir almacenamiento mediante copy-on-write. Si contienen ownership afín, la misma operación mueve el valor completo en lugar de duplicarlo.

La identidad solo existe cuando el tipo la declara. `Ref[T]` crea una referencia segura, no nula y con identidad estable. `Pointer[T]` representa una dirección raw y solo puede operarse dentro de una región `unsafe`. Una copia ordinaria nunca adquiere identidad de referencia de forma implícita.

~~~tondo
var original = [1, 2, 3]
var copy = original

copy[0] = 9

// original continúa siendo [1, 2, 3]
~~~

### 4.2 Inmutabilidad profunda por defecto

`let` crea un binding cuyo valor no puede reasignarse ni modificarse a través de ese binding. `var` permite modificarlo. La mutabilidad no se propaga accidentalmente a copias lógicas.

Los tipos de sincronización o mutabilidad interior pueden cambiar el estado al que apuntan mediante operaciones nombradas, pero esa capacidad forma parte explícita de su contrato. No convierte en mutable el binding, no permite escribir sus campos ordinarios y nunca aparece por aliasing implícito.

### 4.3 Contratos visibles

Una firma declara:

- Parámetros y sus tipos.
- Tipo de éxito, si existe.
- Tipo cerrado de error recuperable, si existe.
- Parámetros de observación prestada mediante `ref`.
- Parámetros de mutación de extensión fija mediante `mut`.
- Parámetros que pueden cambiar de extensión o reemplazarse sin conservarla
  mediante `var`.
- Parámetros genéricos y constraints.
- Capacidad de suspensión mediante `async`, cuando existe.
- Precondiciones inseguras mediante `unsafe`, cuando el llamador debe garantizarlas.

No hay excepciones comprobables fuera de la firma.

### 4.4 Dos clases de fallo

- **Error recuperable:** dato normal representado por `Result`, un enum o una unión.
- **Pánico:** violación de un invariante del programa; no se utiliza para control de flujo.

### 4.5 Un único camino para cada operación

Ejemplos:

- `for` cubre iteración, repetición condicional e infinitos.
- `match` cubre discriminación exhaustiva.
- `pub` expresa exportación; la capitalización no cambia semántica.
- `?` propaga ausencia o error.
- `concat` expresa concatenación; `+` conserva significado aritmético.
- `remove` elimina una clave; asignar `none` no elimina.
- `ref T` expresa observación prestada y temporal; `Ref[T]` expresa identidad
  segura y almacenable; `Pointer[T]` expresa acceso raw inseguro.
- `async fn` expresa suspensión; `spawn` expresa concurrencia.
- Un script raíz se convierte en un único `main` implícito; los módulos importados permanecen libres de efectos.

---

## 5. Código fuente y léxico

### 5.1 Codificación

Los archivos fuente están codificados en UTF-8 válido, sin BOM. Un byte inválido es un error léxico.

Los finales de línea `LF` y `CRLF` son aceptados. El formateador canónico produce `LF`.

Un `CR` que no forme parte de `CRLF` es error léxico, también dentro de un literal
multilínea; un string normal utiliza `\r` cuando necesita ese scalar. Esta regla
evita que la misma secuencia se interprete como contenido o salto según la
plataforma.

Si el último token no termina ya una línea, el lexer inserta un `NL` sintético inmediatamente antes de `EOF`. De este modo una fuente sin salto final se analiza igual que la misma fuente terminada en `LF`; el formateador siempre escribe ese `LF` final.

### 5.2 Espacios y nuevas líneas

Fuera de comentarios y literales, solo el espacio ASCII `U+0020`, el tab
`U+0009` y los finales de línea de 5.1 son whitespace. Otros separadores Unicode
o controles no forman identificadores ni whitespace y producen error léxico.
Espacios y tabs separan tokens. Los tabs no se permiten para indentación en código
formateado; el formateador utiliza cuatro espacios.

No existen sentencias terminadas con punto y coma. `;` no es un token válido en Tondo.

El lexer decide si una nueva línea física produce `NL` mediante un algoritmo cerrado. Los comentarios se ignoran para esta decisión, salvo que una nueva línea contenida en un comentario de línea o bloque continúa siendo una nueva línea física.

Una nueva línea no produce `NL`:

- Dentro de `()` o `[]`, incluidos argumentos genéricos.
- Dentro del contenido léxico de un string.
- Cuando el token significativo anterior es uno de:
  - `,`, `.`, `:`, `=>`.
  - `!`, que solo actúa como separador de resultado en una expresión de tipo.
  - `=`, `+=`, `-=`, `*=`, `/=`, `%=`, `&=`, `^=`, `|=`, `<<=`, `>>=`.
  - `+`, `-`, `*`, `/`, `%`, `<<`, `>>`, `&`, `^`, `|`, `..`, `..=`.
  - `<`, `<=`, `>`, `>=`, `==`, `!=`, `in`, `and`, `or` o `with`.
  - Los prefijos incompletos `not`, `~`, `await`, `spawn`, `fail` o `defer`.
- Cuando el siguiente token significativo es `.`, o uno de los operadores binarios de la lista anterior, desde `+` hasta `with`.

En cualquier otro caso produce exactamente un `NL` aunque existan varias nuevas líneas físicas consecutivas; el parser conserva aparte cuántas líneas y comentarios había para documentación y formato. Cuando se anidan delimitadores, manda el delimitador abierto más interior: entrar en `{}` restaura las nuevas líneas significativas aunque ese body esté dentro de `()` o `[]`, y entrar después en unos `()` o `[]` interiores vuelve a suprimirlas hasta su cierre. Una llave `{` nunca suprime por sí sola el `NL`, porque dentro de un body las nuevas líneas separan elementos. `return` tampoco lo suprime: `return` al final de una línea devuelve `Unit`, y para devolver una expresión esta comienza en esa misma línea.

Estas reglas hacen válidas, entre otras, las continuaciones después de `=` y `=>` sin depender de que una implementación interprete qué parece “incompleto”. El formateador evita comenzar una continuación con operador salvo para el punto de una cadena de accesos.

`else` se escribe en la misma línea lógica que la llave `}` de su rama anterior. Una nueva línea significativa antes de `else` termina el `if`; el formateador canónico siempre emite `} else {` o `} else if ... {`.

~~~tondo
let total =
    subtotal +
    taxes -
    discount

let result = source
    .transform()
    .validate()?
~~~

El valor de un bloque es su última expresión. No se necesita un marcador especial
para distinguirla: una expresión situada inmediatamente antes de la llave de
cierre, ignorando `NL`, es siempre el valor del bloque. Su tipo se comprueba
después contra el contexto; no se reclasifica como sentencia para hacer que el
programa compile. Para evaluar y descartar deliberadamente un valor se escribe
`_ = expression`.

### 5.3 Comentarios

Comentario de línea:

~~~tondo
// Comentario hasta el final de la línea.
~~~

Comentario de bloque anidable:

~~~tondo
/*
    Los comentarios de bloque pueden contener:
    /* otro comentario */
*/
~~~

Comentario de documentación:

~~~tondo
/// Devuelve el usuario asociado al identificador.
///
/// Falla con `UserError.NotFound` si no existe.
pub fn findUser(id: UserId): User ! UserError {
    panic("cuerpo de ejemplo omitido")
}
~~~

Los comentarios de documentación usan Markdown y se asocian con la declaración inmediatamente posterior.

### 5.4 Identificadores

Los identificadores siguen las propiedades Unicode `XID_Start` y `XID_Continue` de **Unicode 16.0.0**. Se normalizan a NFC según esa misma versión antes de compararse. El guion bajo `_` puede iniciar o formar parte de un identificador.

Los identificadores son sensibles a mayúsculas:

~~~tondo
user
User
USER
~~~

son tres nombres diferentes, pero la capitalización no expresa visibilidad ni categoría semántica.

El identificador solitario `_` es un descarte y no crea un binding. Solo es válido en patrones, destinos de descarte y parámetros fijos de función o cierre; no puede nombrar módulos, declaraciones, campos, parámetros genéricos ni variádicos.

Una keyword tampoco es un identificador general, pero puede utilizarse
contextualmente como nombre de **campo** en las posiciones cerradas que ya exigen
uno: declaración, inicialización, actualización, patrón y acceso después de
`.`. Esto permite modelar datos con campos como `type` sin introducir escapes:

~~~tondo
type Event = {
    type: String
}

let event = Event { type: "click" }
let eventType = event.type
~~~

Un campo cuyo nombre sea keyword no admite shorthand porque no puede existir un
binding no calificado con ese nombre; se escribe siempre `type: expression` o
`type: pattern`. La excepción no permite llamar `type` a una variable, parámetro,
función, método, variante, módulo o tipo.

El compilador debe diagnosticar como warning identificadores visualmente confundibles dentro del mismo scope. La comparación utiliza el *confusable skeleton* del perfil general de Unicode Technical Standard #39 correspondiente a Unicode 16.0.0, después de NFC. El modo estricto puede promoverlo a error. Una futura edición puede actualizar Unicode, pero dos compiladores de la misma edición utilizan siempre las mismas tablas.

Convenciones no semánticas:

- Tipos, traits y variantes de enum: `PascalCase`.
- Funciones, métodos, parámetros, campos y variables: `camelCase`.
- Constantes de módulo: `PascalCase`.
- Módulos: una palabra en minúscula cuando sea posible y `camelCase` si son compuestos.
- Los acrónimos se tratan como palabras: `HttpClient`, `JsonValue`, `userId`, no `HTTPClient`, `JSONValue` ni `userID`.

Estas convenciones no participan en resolución ni visibilidad. El linter debe diagnosticarlas y puede promoverlas a error en modo estricto, pero el parser y el sistema de tipos no deducen categorías a partir de mayúsculas.

### 5.5 Palabras reservadas

Las palabras reservadas de Tondo 0.1 son:

~~~text
alias      and        as         async      await
break      const      continue   defer      else
enum       err        fail       false      fn
for        if         impl       import     in
let        match      mut        none       not
ok         or         priv       pub        ref
return     scope      self       some       spawn
trait      true       type       unsafe     var
with
~~~

| Keyword | Función |
|---|---|
| `alias` | Declarar un alias transparente |
| `and` | Conjunción booleana con short-circuit |
| `as` | Asignar alias a un import |
| `async` | Declarar una función o cierre que puede suspenderse |
| `await` | Esperar explícitamente una operación asíncrona |
| `break` | Terminar el bucle actual |
| `const` | Declarar una constante de módulo |
| `continue` | Avanzar a la siguiente iteración |
| `defer` | Registrar cleanup de scope |
| `else` | Rama alternativa de `if` |
| `enum` | Declarar una unión nominal |
| `err` | Construir o reconocer error de `Result` |
| `fail` | Salir de una función por su canal de error |
| `false` | Literal booleano |
| `fn` | Declarar una función nombrada o un tipo de función |
| `for` | Única construcción de bucle |
| `if` | Condicional y expresión condicional |
| `impl` | Implementar un trait o introducir un resultado opaco estático |
| `import` | Importar un módulo |
| `in` | Iteración o pertenencia según contexto |
| `let` | Binding inmutable |
| `match` | Discriminación exhaustiva |
| `mut` | Acceso mutable exclusivo de extensión fija |
| `none` | Ausencia de `Option` |
| `not` | Negación booleana |
| `ok` | Construir o reconocer éxito de `Result` |
| `or` | Disyunción booleana con short-circuit |
| `priv` | Marcar privado un campo de un tipo público |
| `pub` | Exportar una declaración |
| `ref` | Crear o declarar un préstamo compartido temporal |
| `return` | Salir con éxito de una función |
| `scope` | Delimitar la vida de tareas concurrentes estructuradas |
| `self` | Receptor del método actual |
| `some` | Construir o reconocer presencia de `Option` |
| `spawn` | Iniciar una llamada asíncrona dentro de su `scope` propietario |
| `trait` | Declarar un contrato de comportamiento estático |
| `true` | Literal booleano |
| `type` | Declarar un record o newtype nominal |
| `unsafe` | Delimitar operaciones raw o declarar precondiciones del llamador |
| `var` | Binding mutable o préstamo exclusivo con cambio estructural |
| `with` | Construir explícitamente un record actualizado |

`Some`, `None`, `Ok` y `Err` no son palabras reservadas. Las formas canónicas intrínsecas son `some`, `none`, `ok` y `err`.

“Reservada” significa que la palabra no puede actuar como identificador no
calificado. La excepción contextual para nombres de campo de 5.4 no cambia su
token léxico ni habilita escapes generales.

No son keywords en 0.1:

~~~text
catch class dynamic extends finally macro new null override
protected static throw try while yield
~~~

Podrán utilizarse como identificadores. Una versión futura no podrá convertirlos en keywords sin un cambio de versión del lenguaje.

### 5.6 Literales booleanos y ausencia

~~~tondo
true
false
none
~~~

`none` necesita un tipo esperado `T?` o una inferencia inequívoca.

### 5.7 Literales enteros

Formas admitidas:

~~~tondo
42
1_000_000
0b1010_0110
0o755
0xFF_A0
~~~

Los separadores `_` solo pueden aparecer entre dígitos.

La parte entera decimal utiliza `0` o comienza por un dígito de `1` a `9`.
Formas como `00`, `01`, `0_1` o `00.5` son errores léxicos; no expresan octal ni
una grafía alternativa del mismo valor. Los prefijos `0b`, `0o` y `0x` sí pueden
contener ceros iniciales después del prefijo porque su base ya es explícita.

Sufijos opcionales:

~~~tondo
42i8
42i16
42i32
42i64
42u8
42u16
42u32
42u64
~~~

Sin contexto ni sufijo, un literal entero tiene tipo `Int`.

Con tipo esperado, un literal sin sufijo puede adoptar exactamente uno de los enteros intrínsecos `Int8`…`Int64` o `UInt8`…`UInt64`, incluidos `Int` y aliases transparentes de esos tipos. El valor matemático debe caber; en otro caso es error de compilación. Un literal entero no adopta implícitamente `Float`, `Byte` ni un newtype. Por tanto:

~~~tondo
let small: UInt8 = 42
let byte: Byte = Byte(255u8)
let id = UserId(42)
~~~

Los sufijos fijan el tipo antes de aplicar contexto. No existe un sufijo de `Byte`: su construcción explícita conserva visible la frontera entre datos binarios y aritmética.

Un literal numérico seguido inmediatamente por un carácter
`Unicode_XID_Continue` o `_` que no pertenezca a sus dígitos o a un sufijo válido
es un único token numérico mal formado. No se divide para aceptar, por ejemplo,
`42i32extra` como `42i32` seguido de `extra`; debe existir un separador.

### 5.8 Literales flotantes

~~~tondo
3.14
1_000.25
1.0e-9
6.022e23
3.14f32
3.14f64
~~~

Un literal flotante sin contexto tiene tipo `Float`. Con tipo esperado puede adoptar `Float32`, `Float64`, `Float` o un alias transparente de ellos. Nunca adopta un newtype. La conversión decimal-binaria redondea a nearest, ties to even. Si el valor decimal finito redondearía a infinito, el literal es error de compilación; infinito y NaN se obtienen mediante operaciones nombradas.

Debe contener un punto decimal o un exponente; `1` nunca es un `Float` implícitamente.

### 5.9 Literales de caracteres

Un `Char` contiene exactamente un valor escalar Unicode:

~~~tondo
'a'
'ñ'
'λ'
'\n'
'\u{1F642}'
~~~

Un grapheme compuesto por varios valores escalares no cabe en `Char` y debe representarse mediante `String`.

El contenido entre comillas simples debe decodificar exactamente un escalar. Los
escapes válidos son `\n`, `\r`, `\t`, `\\`, `\'`, `\0` y
`\u{HEX}`. La forma Unicode contiene entre uno y seis dígitos hexadecimales, sin
separadores, y su valor debe estar en `0...10FFFF` excluyendo surrogates
`D800...DFFF`. Un escape desconocido, un literal vacío, más de un escalar, una
nueva línea física o un control ASCII sin escape son errores léxicos.

### 5.10 Literales de string

String normal con escapes e interpolación:

~~~tondo
let name = "Ada"
let message = "Hola, {name}. Resultado: {2 + 2}"
~~~

Escapes:

~~~text
\n \r \t \\ \" \0 \u{HEX}
~~~

`\u{HEX}` sigue exactamente las reglas de scalar y número de dígitos de 5.9.
Cualquier otro escape es error. Un string normal de una línea no puede contener
una nueva línea física ni un control ASCII sin escape.

Las llaves literales se duplican:

~~~tondo
"objeto: {{ clave: {value} }}"
~~~

Una interpolación contiene una expresión Tondo completa entre llaves balanceadas. El lexer conserva por separado segmentos de texto y rangos de expresión; el parser aplica la gramática ordinaria a cada rango. Las expresiones se evalúan de izquierda a derecha cuando se construye el string. Un string utilizado como patrón literal no puede contener interpolaciones.

`{{` y `}}` se decodifican como llaves literales. Una `{` no duplicada inicia una
interpolación no vacía y una `}` no duplicada fuera de ella es error. Para buscar
su cierre, el lexer reconoce de forma anidada llaves, strings, chars y comentarios
de la expresión; una llave dentro de uno de esos tokens no altera el balance.

String raw, sin escapes ni interpolación:

~~~tondo
r"C:\users\name"
r"\d+\.\d+"
~~~

En un raw de una línea, la primera `"` posterior al prefijo `r"` cierra el
literal; no existe escape para incluirla y tampoco se admite una nueva línea
física. Backslashes y llaves son texto ordinario.

String multilínea:

~~~tondo
let message = """
    Primera línea
    Segunda línea: {value}
    """
~~~

El valor de un string multilínea se obtiene antes del formateo mediante estas reglas:

1. Los finales físicos `LF` y `CRLF` del literal se normalizan a `\n`.
2. Si el contenido comienza con una nueva línea inmediatamente después de `"""`, esa primera nueva línea estructural no forma parte del valor.
3. Cuando el delimitador de cierre aparece solo tras whitespace desde la última nueva línea, esa última nueva línea estructural tampoco forma parte del valor.
4. El whitespace anterior al delimitador de cierre define el prefijo de indentación. Cada línea no vacía del contenido debe comenzar con ese prefijo exacto; si no, el literal es error. De cada línea vacía se elimina como máximo ese mismo prefijo.
5. El prefijo se elimina antes de procesar escapes e interpolaciones. En la variante raw solo se aplica la dedentación, nunca escapes ni interpolación.
6. Si el delimitador de cierre comparte línea con contenido, no existe prefijo implícito ni eliminación de la última nueva línea.

Así, el ejemplo anterior contiene exactamente `"Primera línea\nSegunda línea: {value}"` antes de evaluar la interpolación. El perfil 0.1 conserva el token multilínea como átomo, salvo la normalización de finales físicos que ya preserva ese mismo valor; nunca reindenta ni “arregla” el literal como efecto secundario.

La combinación `r"""..."""` crea un string multilínea raw.

En un multilínea normal, una comilla escapada no participa en el delimitador y la
primera secuencia `"""` restante lo cierra. En un multilínea raw, la primera
secuencia `"""` lo cierra y todo lo anterior es texto literal. Para representar
esa secuencia se usa un multilínea normal con una comilla escapada o una operación
nombrada que concatene varias piezas raw; no existe un delimitador raw de longitud
variable en 0.1.

### 5.11 Literales de colecciones

Array:

~~~tondo
[1, 2, 3]
~~~

Map:

~~~tondo
[
    "name": "Ada",
    "age": "36",
]
~~~

Map vacío:

~~~tondo
let users: Map[UserId, User] = [:]
~~~

Set:

~~~tondo
Set["read", "write"]
~~~

Los literales vacíos `[]` y `Set[]` requieren contexto de tipo. `[:]` identifica
siempre un map. `Set` es un nombre intrínseco del prelude, no una keyword, y no
puede redeclararse como nombre no calificado.

### 5.12 Orden de evaluación

Toda evaluación es de izquierda a derecha:

- Receptor antes que argumentos.
- Argumentos en orden textual.
- Elementos de arrays, maps y records en orden textual.
- Dentro de una entrada de map, clave antes que valor.
- Operando izquierdo antes que derecho.
- Interpolaciones en orden de aparición.

Una optimización puede fusionar o eliminar trabajo solo si conserva todos los resultados, errores recuperables, pánicos y efectos observables.

---

## 6. Estructura de programas y módulos

### 6.1 Archivo, módulo y paquete

- Un archivo fuente termina en `.to`.
- Todos los archivos de un mismo directorio fuente forman un módulo.
- El path del módulo deriva de su posición dentro del paquete.
- Un paquete contiene uno o más módulos.
- La selección de raíces, dependencias y targets pertenece al manifiesto de herramientas, no a la sintaxis del lenguaje.

No existe una declaración `module` dentro de cada archivo. Esto evita repetir información que ya conoce el sistema de build.

### 6.2 Scope de módulo

Los archivos de un módulo comparten:

- Tipos.
- Funciones.
- Constantes.
- Implementaciones.
- Declaraciones privadas.

Los imports son locales al archivo para que las dependencias sean visibles donde se utilizan.

### 6.3 Imports

Sintaxis canónica:

~~~tondo
import std.fs
import app.models
~~~

El import introduce el nombre final del módulo:

~~~tondo
let data = fs.read(path)?
let user = models.User { name: "Ada" }
~~~

Alias explícito:

~~~tondo
import company.veryLongModule as users
~~~

Los imports forman un encabezado de archivo: aparecen antes de cualquier
declaración o sentencia, ignorando comentarios y líneas vacías. Su orden textual
no afecta resolución porque importar no ejecuta código; el formatter ordena cada
grupo según 21.4.

No existen:

- Imports wildcard.
- Imports que copien todos los símbolos al scope local.
- Imports relativos basados en `..`.
- Imports condicionales según orden o estado global.

Los ciclos entre módulos son un error de compilación. El compilador debe mostrar el ciclo completo.

### 6.4 Declaraciones de nivel superior

Se permiten:

- `import`.
- `const`.
- `type`.
- `alias`.
- `enum`.
- `trait`.
- `impl`.
- `fn`.

En módulos ordinarios no se permiten sentencias ejecutables, `let`, `var` ni inicialización mutable en el nivel superior.

Un archivo raíz compilado en modo script puede contener sentencias de nivel superior según la sección 20. Esas sentencias pertenecen a un `main` implícito; nunca se convierten en inicializadores de módulo.

### 6.5 Constantes

~~~tondo
const MaxRetries: Int = 3
const DefaultHost = "127.0.0.1"
~~~

Una constante debe poder evaluarse en compilación usando:

- Literales.
- Tuplas, records, enums, newtypes, options, results y colecciones formadas únicamente por constantes.
- Valores de función nombrada completamente especializados; obtener el valor no
  ejecuta su body.
- Operadores puros sobre constantes.
- Otras constantes ya resueltas sin ciclos.

Tondo 0.1 no introduce una keyword `comptime` ni permite llamar a funciones de usuario durante evaluación constante. Los constructores nominales y los intrinsics puros enumerados por el lenguaje no cuentan como llamadas de usuario. Una operación que produciría error recuperable o pánico durante evaluación constante es un error de compilación.

Las constantes no pueden realizar I/O, acceder al reloj o al entorno, crear identidad mediante `Ref`, construir punteros, tasks, cursores afines, handles de recursos ni asignar memoria mutable observable. Una constante pública debe declarar su tipo porque ese tipo forma parte de la API del módulo.

### 6.6 Módulos ejecutables

Un target ejecutable `hosted` selecciona un módulo raíz. Ese módulo debe contener
exactamente una función privada `main` válida o un único script raíz, según la
[sección 20](#20-programas-ejecutables-scripts-y-procesos). La ausencia de ambos
produce `E1806`; más de uno produce `E1802`, y una declaración `main` presente
pero inválida produce `E1803`. Otro perfil ejecutable debe definir su entrada de
forma separada y no reutiliza estos diagnósticos para imponer `main`.

Un target de biblioteca no contiene `main`.

### 6.7 Identidad de paquete, edición y resolución

El manifiesto y el lockfile no forman parte de la sintaxis `.to`, pero deben
proporcionar al compilador un grafo cerrado antes de resolver imports. Cada nodo
del grafo tiene un **PackageId** opaco y estable para esa resolución. Su formato
pertenece al toolchain, pero distingue como mínimo el origen lógico, el nombre, la
versión exacta y, cuando exista, la identidad de integridad fijada por el lockfile.
Dos nodos distintos nunca comparten `PackageId`, aunque publiquen el mismo nombre
o los mismos módulos.

La identidad nominal completa de una declaración de módulo incluye:

~~~text
PackageId + module path + namespace + declaration path
~~~

Por ello, tipos procedentes de dos versiones simultáneas de una dependencia son
distintos aunque tengan idéntico spelling y estructura. El manifiesto asigna a
cada dependencia un alias local único y declara el nombre local del paquete
actual. Esos nombres no pueden colisionar entre sí ni con `std`. El primer
segmento de un import resuelve exactamente contra el paquete actual, uno de esos
aliases o el namespace reservado `std`; el resto identifica el module path
interno al paquete resuelto. Un alias es solo spelling local y no participa en
identidad nominal: dos aliases que el toolchain permita resolver al mismo
`PackageId` nombran los mismos tipos. Utilizar dos versiones directamente exige
dos aliases visibles diferentes y, al ser nodos distintos, conserva identidades
distintas. El compilador nunca elige una dependencia por orden de búsqueda,
directorio actual, variable de entorno o versión “más cercana”.

`std` no puede declararse como alias ni como paquete de usuario. La distribución
del toolchain selecciona una implementación estándar exacta compatible con la
edición y el target, le asigna su propio `PackageId` y registra su hash junto al
resto del grafo. Cambiar esa implementación invalida las interfaces que dependan
de su API igual que cambiar cualquier otra dependencia.

Cada paquete selecciona exactamente una edición del lenguaje. Todos sus archivos
se analizan con esa edición; no existen pragmas por archivo. Dependencias de otras
ediciones conservan su propia semántica y se consumen mediante interfaces
compiladas que registran edición, `PackageId`, target, perfil, capacidades,
features y hash de API pública. Una interfaz incompatible se rechaza antes de
typecheckear consumidores.

El lockfile fija todos los `PackageId` transitivos y hashes disponibles. Con el
mismo manifiesto, lockfile, fuentes, edición, target, perfil, capacidades,
features y toolchain, la resolución produce el mismo grafo o el mismo error, sin
consultar red durante la compilación.

### 6.8 Source sets, targets y generación

La selección por sistema operativo, arquitectura o capacidad de host ocurre en
el manifiesto mediante **source sets** declarados, nunca mediante `#if`, imports
condicionales ni constantes ambientales dentro de `.to`. Antes de lexear, el
toolchain resuelve un conjunto concreto de archivos y asigna a cada uno un único
path lógico conforme a 22.6.

Reglas:

- Target, edición, features y source sets activos son entradas declaradas del
  build y forman parte de su identidad.
- Dos archivos activos no pueden producir el mismo path lógico. Los archivos de
  varios source sets sí pueden contribuir al mismo módulo cuando sus paths
  lógicos sean distintos y sus declaraciones ordinarias no colisionen.
- Un import que no exista para el target resuelto produce `E1008`; el compilador
  no busca una variante alternativa implícita.
- Una API pública puede variar entre targets solo si el target forma parte de la
  identidad de su interfaz compilada. Un consumidor nunca enlaza accidentalmente
  una interfaz producida para otro target.
- Un generador declara entradas, salidas y configuración. Sus salidas son fuente
  Tondo ordinaria, reciben paths lógicos y pasan por el mismo lexer, formatter,
  typechecker y reglas de visibilidad.
- La generación no puede leer red, reloj, aleatoriedad o entorno no declarados y
  seguir reclamando un build reproducible. El toolchain registra esas entradas o
  marca el artefacto como no reproducible.

El schema concreto del manifiesto, el algoritmo de versiones y los comandos del
gestor de paquetes pertenecen a la especificación del toolchain; no pueden
cambiar estas identidades ni introducir semántica fuente oculta.

---

## 7. Declaraciones, nombres y visibilidad

### 7.1 Bindings inmutables

~~~tondo
let name = "Ada"
let count: Int = 10
~~~

`let` requiere inicializador. El tipo puede inferirse localmente o declararse. El binding y el valor alcanzado a través de él son inmutables.

### 7.2 Bindings mutables

~~~tondo
var count = 0
count += 1
~~~

`var` requiere inicializador. Permite reasignar y modificar el valor mediante ese binding.

Después de mover su valor, una asignación completa puede reponer el `var`. Hasta completar esa asignación el binding no está disponible; no puede reponerse escribiendo solo un campo o índice.

No existe declaración sin inicialización:

~~~tondo compile-fail E1109
var value: Int // error
~~~

La inicialización definida evita estados “no inicializados” y análisis de asignación parcial.

### 7.3 Desestructuración

`let` y `var` aceptan patrones irrefutables:

~~~tondo
let (left, right) = pair
let Point { x, y } = point
~~~

Un patrón refutable, como una variante concreta de enum, debe consumirse con `match`.

El inicializador se evalúa una vez. Desestructurar un valor `Copy` copia lógicamente sus componentes y deja disponible el origen; desestructurar un valor afín consume el agregado completo y transfiere cada componente enlazado. Un componente omitido solo puede abandonarse si no conserva una obligación terminal.

### 7.4 Ausencia de shadowing

Un binding no puede ocultar otro binding local o parámetro que continúe visible en ese punto:

~~~tondo compile-fail E1003
let valid = true
let value = "10"

if valid {
    let value = 10 // error: `value` ya está visible
}
~~~

Debe utilizarse un nombre que describa la transformación:

~~~tondo
let rawValue = "10"

if valid {
    let value = parse(rawValue)?
    consume(value)
}
~~~

La prohibición incluye bindings creados por `let`, `var`, parámetros, patrones, bucles y cierres. Dos scopes hermanos pueden reutilizar un nombre porque ninguno se encuentra visible dentro del otro. `_` es un descarte y puede aparecer cualquier cantidad de veces.

Ocultar un nombre de módulo importado o una declaración del mismo scope también es error. Los campos accedidos explícitamente mediante `self.field` no son bindings léxicos y pueden compartir nombre con un parámetro.

La prohibición se aplica dentro del namespace que participa en la resolución. La coincidencia permitida entre un tipo y una función descrita en 7.10 no es shadowing porque ambos nombres ocupan namespaces distintos; un binding local sí entra en conflicto con una función, constante u otro valor visible del mismo namespace.

### 7.5 Visibilidad de declaraciones

Todo símbolo de módulo es privado por defecto:

~~~tondo
fn validate(user: User): Bool {
    user.name.length() > 0
}
~~~

`pub` exporta una declaración:

~~~tondo
pub fn loadUser(id: UserId): User ! UserError {
    panic("cuerpo de ejemplo omitido")
}

pub type UserId = Int
~~~

No existe exportación basada en mayúsculas.

### 7.6 Visibilidad de records

Los campos de un record heredan la visibilidad del tipo:

~~~tondo
pub type User = {
    id: UserId
    name: String
    priv passwordHash: String
}
~~~

En este ejemplo:

- `User`, `id` y `name` son públicos.
- `passwordHash` solo es visible dentro del módulo.
- Código externo no puede construir un literal `User` completo porque no puede proporcionar el campo privado.
- El módulo debe ofrecer una función pública de construcción si desea permitir la creación externa.

Un record público es **externamente construible** exactamente cuando todos sus
campos son públicos. Esta propiedad booleana forma parte de su interfaz compilada,
aunque los campos privados que la causan permanezcan ocultos. Añadir el primer
campo privado retira la construcción literal externa y es un cambio incompatible
de fuente. Si el record ya no era externamente construible, añadir o cambiar otro
campo privado puede ser compatible bajo las condiciones siguientes.

Un campo `priv` no puede aparecer en un tipo privado porque sería redundante; es
un error semántico y el diagnóstico propone quitar el modificador. El formatter
solo opera sobre la fuente válida resultante, no convierte esa prohibición en una
regla de estilo distinta.

La visibilidad de un campo se aplica también a acceso, construcción, actualización y patrones. Desde otro módulo, un patrón de record no puede nombrar un campo `priv` y debe terminar en `..` si existen campos no visibles. El patrón no revela sus nombres, tipos ni valores. Las reglas ordinarias de ownership siguen aplicándose: si un campo omitido conserva una obligación terminal, ignorarlo mediante `..` no satisface esa obligación y el patrón es rechazado.

`priv` oculta la representación nominal, no las propiedades semánticas del valor
completo. Por ello:

- Igualdad, hashing y derivación de `Copy`, `Discard`, `Equatable`, `Key`, `Send`,
  `Share` y obligación terminal incluyen también los campos privados.
- La interfaz pública compilada expone únicamente el resultado de cada capacidad y
  si existe una obligación terminal; no expone el nombre, tipo ni posición del
  campo que la causa.
- Si el record es `Equatable` o `Key`, dos valores que solo difieran en un campo
  privado pueden comparar como distintos y producir hashes distintos. La
  documentación pública del tipo debe declarar esta semántica sin revelar la
  representación.
- Añadir o cambiar un campo privado de un record que ya no era externamente
  construible es compatible solo si conserva sus campos públicos, todas las
  capacidades públicas y la semántica documentada de igualdad, clave y ownership.
  De lo contrario es un cambio de contrato público, aunque el campo siga oculto.
- Eliminar el último campo privado añade construcción literal externa. Es una
  ampliación visible de API y requiere como mínimo una versión menor si conserva
  el resto del contrato; si además altera capacidades, igualdad, clave u ownership,
  requiere una versión mayor.

Así un módulo puede encapsular su representación sin fingir que las propiedades
observables del valor dejan de formar parte de su API.

### 7.7 Visibilidad de enums

Las variantes heredan la visibilidad del enum. No hay variantes individualmente privadas en un enum público.

Cuando una API necesita ocultar variantes internas, debe exponer un enum público diferente y realizar una conversión explícita en la frontera del módulo.

### 7.8 Visibilidad de métodos

Los métodos son declaraciones de módulo y son privados por defecto:

~~~tondo
fn User.validate(self): Bool {
    not self.name.isEmpty()
}

pub fn User.displayName(self): String {
    self.name
}
~~~

Solo el módulo que define un tipo puede declarar sus métodos inherentes. Otros módulos utilizan traits.

### 7.9 APIs públicas bien formadas

Una posición externamente observable de una declaración pública no puede mencionar una declaración menos visible. La regla se aplica de forma transitiva a:

- Parámetros, resultados y errores de funciones públicas.
- Parámetros genéricos y constraints públicos.
- Tipos subyacentes de aliases y newtypes públicos.
- Campos públicos de records.
- Payloads de variantes de enums públicos.
- Firmas de métodos públicos y de traits públicos.

El cuerpo de una función, la implementación de un método y los campos `priv` de un record público pueden utilizar tipos privados porque no forman parte de la interfaz accesible. Un campo privado continúa impidiendo la construcción externa del record, pero no filtra su propio tipo.

En un resultado `impl Bound`, los bounds y el error exterior sí forman parte de
la API y deben ser accesibles; el tipo concreto queda deliberadamente oculto por
12.8 y puede ser privado o un entorno de cierre anónimo.

Las capacidades estructurales, obligaciones y constructibilidad externa derivadas
de campos privados sí forman parte de la interfaz pública en la forma opaca
descrita en 7.6. La comprobación de API registra el resultado, no la ruta privada
que lo origina.

Un método inherente `pub` requiere que su tipo propietario también sea público. Una implementación de trait no utiliza `pub`: sus métodos son observables cuando el trait, el tipo y la propia firma son accesibles.

### 7.10 Namespaces

Tondo mantiene namespaces separados para:

- Tipos y traits.
- Valores, funciones, constantes y variantes en contexto.
- Módulos importados.

Los namespaces separados permiten que un tipo no invocable y una función compartan
nombre cuando cada uso es inequívoco. Una excepción evita ambigüedad en la sintaxis
de construcción: ningún nombre de tipo que admita `Name(value)` puede coexistir en
el mismo scope con una función, constante o valor llamado `Name`. Esto incluye
newtypes y tipos numéricos intrínsecos; los aliases se comprueban después de
expandirse. El conflicto se diagnostica en la declaración, no se decide utilizando
el tipo esperado.

Los nombres de tipos, capacidades, traits e intrinsics del prelude enumerados en
26.3 no pueden introducirse como nombres **no calificados** mediante declaraciones
de módulo, bindings, parámetros —incluidos genéricos— o aliases de import, con
independencia del namespace. Esta reserva cerrada garantiza, entre otras cosas,
que `Set[...]`, `Ref(...)` y los constructores numéricos no cambien de categoría
sintáctica por un binding local.

Continúan sin ser keywords. La reserva no alcanza nombres de miembro que siempre
se introducen y utilizan calificados: fields, variantes, métodos inherentes o de
trait y operaciones asociadas pueden llamarse como un nombre del prelude porque
aparecen detrás de un receptor o path propietario. Tampoco afecta a segmentos
intermedios de un path de módulo. Así `packet.String`, `Token.Set` o
`Codec.Result(...)` son inequívocos, mientras un binding local `let String = ...`
continúa siendo error. Los demás nombres importados pueden evitar colisiones
mediante alias de módulo.

Las variantes de enums de usuario se califican siempre:

~~~tondo
Color.Red
LoadError.Io(error)
~~~

No se omite el calificador aunque el tipo esperado ya determine el enum. `Option` y `Result` son las únicas excepciones y utilizan las formas intrínsecas `some`, `none`, `ok` y `err`. Esta regla evita colisiones y mantiene local el origen de cada variante.

### 7.11 Alcance y orden de resolución

Las declaraciones de tipos, funciones, traits, implementaciones y constantes de un módulo se resuelven sin depender del orden de archivos o del orden textual. Los ciclos de tipos válidamente recursivos se permiten; los ciclos de evaluación de constantes se rechazan.

Un binding local:

- Entra en scope después de completar su inicializador.
- Permanece visible hasta el final del bloque.
- No es visible dentro de su propio inicializador.
- No puede quedar oculto mientras permanezca visible.

Los bindings de una rama `match` solo existen en esa rama. El patrón de un `for` solo existe en su body. Un parámetro existe durante todo el cuerpo de su función.

La búsqueda de un nombre local avanza desde el scope más interno hacia:

1. Scopes léxicos exteriores.
2. Parámetros.
3. Declaraciones del módulo.
4. Módulos importados, que siempre requieren calificación.
5. Prelude.

Una ambigüedad en el mismo nivel es error; el compilador no elige por orden.

---

## 8. Sistema de tipos

### 8.1 Propiedades generales

El sistema de tipos es:

- Estático.
- Fuerte.
- Nominal para records, enums, newtypes y resultados opacos por declaración.
- Estructural únicamente para tuplas, funciones y uniones anónimas.
- Sin subtipado de clases.
- Sin conversiones implícitas numéricas.
- Sin tipo superior universal.
- Con inferencia local.
- Con genéricos paramétricos invariantes.

Cada expresión tiene exactamente un tipo estático antes de ejecutar el programa.

### 8.2 Inferencia

El compilador puede inferir:

- Tipo de variables locales desde su inicializador.
- Parámetros genéricos desde argumentos y tipo esperado.
- Tipo de literales desde contexto.
- Firma y tipo concreto de cierres desde el contexto, sus anotaciones y capturas.
- Unión concreta de una expresión cuando existe un tipo esperado.

El compilador no infiere:

- Tipos de parámetros de funciones nombradas.
- Contratos de retorno no `Unit` de funciones nombradas. Un resultado
  `impl Bound` declara el contrato y solo infiere localmente su testigo concreto
  único según 12.8.
- Firmas públicas entre módulos.
- Tipo de colecciones vacías sin contexto.
- Uniones heterogéneas no declaradas.

~~~tondo
let numbers = [1, 2, 3]                 // Array[Int]
let empty: Array[String] = []           // contexto necesario
let mixed: Array[Int | String] = [1, "a"]
~~~

Sin la anotación de `mixed`, el último literal es un error; Tondo no inventa una unión implícita a partir de elementos incompatibles.

### 8.3 Tipos escalares

Tipos de uso general:

| Tipo | Semántica |
|---|---|
| `Bool` | `true` o `false`; no existe truthiness |
| `Int` | entero con signo de 64 bits |
| `Float` | IEEE 754 binario de 64 bits |
| `Byte` | tipo nominal intrínseco con representación `UInt8` para datos binarios |
| `Char` | valor escalar Unicode |
| `String` | secuencia UTF-8 inmutable de valores escalares |
| `Unit` | tipo con un único valor `()` |
| `Never` | tipo sin valores, para expresiones que no retornan |

Tipos de ancho fijo:

~~~text
Int8 Int16 Int32 Int64
UInt8 UInt16 UInt32 UInt64
Float32 Float64
~~~

`Int` tiene siempre 64 bits, incluso en plataformas de 32 bits. `Float` equivale semánticamente a `Float64`. Los nombres cortos no dependen de la arquitectura.

`Int` e `Int64` son el mismo tipo bajo dos grafías; `Float` y `Float64` también. `Int` y `Float` son las formas canónicas en código general. Los nombres con ancho se prefieren cuando el ancho forma parte explícita de un protocolo binario o frontera FFI.

### 8.4 `Unit`

El valor de `Unit` es:

~~~tondo
()
~~~

Una función sin anotación de retorno devuelve `Unit`:

~~~tondo
fn log(message: String) {
    console.print(message)
}
~~~

El final del cuerpo devuelve `()` de forma implícita.

`Unit` tiene exactamente un valor y representa finalización normal sin payload. No significa que la función no pueda retornar.

### 8.5 `Never`

`Never` no tiene valores. Una expresión de tipo `Never` puede utilizarse donde se espera cualquier tipo porque nunca produce un resultado.

`panic` es un intrinsic no sobrecargable del prelude y su firma es normativa.
`terminate` ilustra una posible frontera de librería:

~~~tondo
fn panic(message: String): Never
fn terminate(code: Int): Never
~~~

El contrato completo de `panic` se define en 15.7.

`Never` permite tipar ramas que terminan el programa:

~~~tondo
let value = if valid {
    compute()
} else {
    panic("invalid invariant")
}
~~~

`Unit` y `Never` no son intercambiables: una función `Unit` vuelve normalmente con `()`, mientras que una función `Never` diverge, termina la tarea o finaliza el proceso. Algebraicamente, `T | Never` se reduce a `T`.

### 8.6 Conversiones

No existen conversiones implícitas entre tipos numéricos, strings, enums o newtypes.

La forma de conversión explícita es el constructor del tipo:

~~~tondo
let wide: Int64 = Int64(small)
let small: Int8 = Int8(value)?
let number: Float = Float(integer)
~~~

Reglas:

- Una conversión definida para todos los valores de origen sin posibilidad de fallo devuelve directamente el tipo destino.
- Una conversión que puede perder rango o no representar el valor devuelve `Destino ! NumericConversionError` y requiere `?` o `match`.
- La conversión explícita de entero a float es total y utiliza el redondeo IEEE documentado, aunque pueda perder precisión.
- Una conversión deliberadamente wrapping, saturating o truncating no tiene sintaxis especial; será una operación nombrada de la librería numérica.
- Convertir `Float` a entero exige que sea finito, integral y representable.
- No existe conversión automática de `Bool` a número ni viceversa.

El error intrínseco es cerrado:

~~~tondo
enum NumericConversionError {
    OutOfRange
    NotFinite
    NotIntegral
}
~~~

Una conversión float→entero clasifica primero `NotFinite`, después `NotIntegral` y finalmente `OutOfRange`. Una conversión entre enteros que falla por rango produce `OutOfRange`. Las conversiones no añaden mensajes dependientes de locale ni variantes específicas de plataforma.

### 8.7 Aliases transparentes

`alias` introduce otro nombre para el mismo tipo:

~~~tondo
alias Index = Int
~~~

`Index` e `Int` son intercambiables y no crean una frontera de seguridad. Los aliases no pueden tener métodos propios ni implementaciones distintas.

Un ciclo formado únicamente por aliases es error porque no produce un tipo finito:

~~~tondo compile-fail E1106
alias First = Second
alias Second = First
~~~

### 8.8 Newtypes nominales

`type` aplicado a un tipo existente crea un tipo nominal:

~~~tondo
pub type UserId = Int
pub type Email = String
~~~

Construcción y extracción:

~~~tondo
let id = UserId(42)
let raw: Int = id.value
~~~

El campo intrínseco `value` tiene la misma visibilidad que el newtype, salvo que se marque el tipo como opaco mediante un record con campo privado. No hay conversión implícita entre `UserId` e `Int`.

Si el valor subyacente cumple `Copy`, `.value` lo copia lógicamente. Si es afín,
la proyección consume el newtype completo y solo es válida sobre un binding,
parámetro propietario o temporal movible; no extrae ownership a través de `self`,
`mut`, `var` ni una proyección parcial. El patrón irrefutable `Name(value)` ofrece
la misma extracción al desestructurar un propietario completo.

Un newtype hereda automáticamente las capacidades intrínsecas `Copy`, `Discard`,
`Equatable`, `Key`, `Send` y `Share` que correspondan a su valor, pero no hereda
métodos ni traits implementados por el tipo subyacente.

La forma `Name(value)` es el constructor nominal generado por el compilador y
reserva `Name` en el namespace de valores del mismo scope según 7.10. No es una
función sobrecargable ni puede ocultarse con un binding.

### 8.9 Compatibilidad y asignabilidad

Un valor es asignable a un tipo esperado cuando:

1. Los tipos completos son idénticos después de expandir aliases transparentes.
2. El tipo exacto del valor coincide con un único miembro de una unión normalizada.
3. El valor es una unión normalizada cuyos miembros son un subconjunto de los
   miembros de la unión esperada; el tag existente se conserva.
4. El valor `none` se utiliza donde el tipo esperado directo es `T?`.
5. Un valor `T` se eleva contextualmente a `some(T)` cuando el tipo esperado directo es `T?`.
6. Un error, o cada miembro de una unión de errores, se eleva a una unión destino
   que contiene todo su conjunto normalizado.
7. `Never` aparece en una ruta que no retorna.

El widening de una unión solo ocurre en la posición superior esperada; no crea
covariancia ni se busca recursivamente dentro de arrays, options, results u otros
miembros. Por tanto, en `Int | Int?` un `Int` selecciona siempre el miembro exacto
`Int`, mientras que `none` necesita una anotación o construcción que identifique
el miembro `Int?`. Esta prioridad evita que una misma expresión admita tags
distintos por inferencia contextual.

No se utiliza subtipado nominal ni conversión de “parecido estructural”.

### 8.10 Valores `Copy` y valores afines

`Copy` es una capacidad intrínseca cerrada que significa que un valor puede duplicarse implícitamente conservando su semántica lógica. No implica una copia física inmediata: strings y colecciones pueden utilizar copy-on-write, y copiar un `Ref[T]` duplica la referencia segura, no `T`.

Cuando un valor cumple `Copy`, asignarlo, pasarlo por valor, devolverlo o capturarlo deja el origen disponible. Cuando no cumple `Copy`, la misma operación mueve el valor y el binding de origen deja de estar disponible:

~~~tondo compile-fail E1401
fn invalid(resource: Resource) {
    let pending = resource

    Resource.release(resource) // error: fue movido a `pending`
    Resource.release(pending)
}
~~~

El estado movido solo existe en el análisis estático; no existe un valor “no inicializado” observable en runtime. Un binding movido no puede leerse ni volver a moverse hasta quedar disponible otra vez. Un `var` puede reponerse mediante una asignación completa y queda disponible después de ella; un `let`, un parámetro ordinario y una asignación parcial no pueden reponerse. El análisis de flujo exige que el binding esté disponible en todos los caminos que llegan a cada uso.

Para mantener pequeño el modelo de 0.1, solo puede moverse un binding local o un
parámetro ordinario completo que posea su valor. Un parámetro `ref`, `mut` o `var`
y cualquier receptor representan una ubicación prestada, no un binding
propietario. Desde `ref` o `self` nunca sale ownership; desde `mut` o `var`, un
valor no `Copy` solo puede salir como parte de un reemplazo confirmado que deje la
ubicación disponible. Extraer un campo o índice no `Copy` requiere consumir y
desestructurar el agregado propietario completo, o utilizar una operación
nombrada que reemplace el elemento de forma atómica.

Un argumento `ref` es un préstamo compartido y no mueve el valor. `mut` y `var`
son préstamos exclusivos: `mut` conserva la extensión estructural del lvalue,
aunque puede reemplazar su contenido completo cuando esa extensión se conserva;
`var` permite además cambiar la extensión o reemplazarla sin conservarla, según
16.3. Los tipos intrínsecos u opacos pueden exigir un consumo terminal antes de
abandonar su scope. Todo tipo con esa obligación es necesariamente no `Copy`.
`Join[T, E]` tiene esa obligación. Un cursor concreto conserva exactamente las
capacidades y obligaciones que se deriven de su estado; implementar
`Iterator[T]` no añade, elimina ni oculta ninguna.

Un binding de patrón `ref` sigue siendo un préstamo, no un nuevo propietario. La
obligación terminal permanece íntegra en el scrutinee o colección de origen y
nunca se considera satisfecha al terminar el arm o la iteración.

La obligación terminal se propaga estructuralmente a newtypes, tuples, records,
enums, options, results, uniones, colecciones y entornos de cierre cuyo tipo
pueda contener el valor.
La comprobación es conservadora y no depende del payload o longitud de runtime:
`File?` y `Array[File]` se consumen de forma exhaustiva aunque un valor concreto
sea `none` o esté vacío. Mover o desestructurar un compuesto transfiere las
obligaciones a los nuevos propietarios; nunca las satisface por sí solo.
Sobrescribir, descartar o dejar salir de scope un valor obligado es error, salvo
que una operación terminal declarada lo consuma o un `defer` válido reserve ese
consumo. Un parámetro genérico sin constraint `Discard` se considera
potencialmente obligado: una implementación genérica debe transferirlo o
consumirlo en todos los caminos y no puede abandonarlo de forma implícita. Como
`Copy` implica `Discard`, cualquiera de los dos bounds puede demostrar abandono
seguro, pero `Discard` no exige duplicación.

Todo tipo intrínseco u opaco con obligación terminal declara además exactamente
una **acción de unwind**. No es una operación elegida por el programa ni una API
pública:

- Es infallible desde la semántica Tondo: no devuelve error, no produce pánico y
  no ejecuta cleanup de usuario arbitrario.
- Para tipos ordinarios no suspende código Tondo. La única excepción intrínseca
  es `Join`: su `scope` puede suspender al propietario mientras cancela y espera
  hijos, tal como declara 11.12.
- Libera, cancela o neutraliza el ownership de la forma más conservadora definida
  por el tipo.
- En un compuesto se deriva estructuralmente y se aplica en orden inverso al de
  construcción.
- Un fallo externo durante esa acción se conserva como diagnóstico suprimido; no
  reemplaza la causa del unwind.

Cada token de ownership terminal vivo tiene exactamente una entrada de cleanup
armada. Para un binding o temporal ordinario, adquirir ownership registra en su
scope una entrada de fallback que contiene la acción de unwind. La entrada sigue
los movimientos locales del token. `Join` utiliza la misma regla de unicidad, pero
su entrada pertenece al registro estructural del `scope`, no a la pila léxica,
porque su cleanup puede suspender.

Cada scope léxico conserva sus entradas en orden de registro:

1. Un `defer` ordinario añade una entrada explícita.
2. Registrar un `defer` que consume un token terminal desarma su fallback y añade
   en ese punto un guard explícito. El guard sigue el token según 13.7. El fallback
   y el guard nunca pueden estar armados a la vez.
3. Una operación terminal marca la entrada consumida antes de invocarse. Un
   handoff confirmado desarma la entrada del origen y registra la del nuevo
   propietario; si el handoff falla, la entrada original continúa armada.
4. Al ejecutar cualquier entrada se la desarma primero. Incluso si un cleanup
   posterior produce pánico, ningún token puede limpiarse dos veces.
5. Construir un compuesto confirma simultáneamente los handoffs de sus
   componentes y los pliega en una única entrada del nuevo owner. Su fallback
   estructural recorre los componentes en orden inverso de construcción.
   Desestructurarlo realiza la operación inversa; nunca coexisten las entradas de
   los componentes con la del compuesto.

Las entradas explícitas se ejecutan en LIFO en toda salida que abandone su scope.
Las entradas de fallback ordinarias se omiten en final normal, `return`, `fail`,
`?`, `break` y `continue`: el análisis estático exige antes una operación terminal
visible o un guard y produce `E1404` si falta. En pánico o cancelación, el runtime
sí drena en LIFO todas las entradas todavía armadas, explícitas e intrínsecas. Así
cada token terminal vivo recibe exactamente un cleanup aun cuando no pueda
continuar el control normal.

Al abandonar un `scope` estructurado, primero se drena su registro de hijos y se
esperan sus cleanups; después se drena la pila léxica directa del bloque. Los
scopes léxicos interiores ya atravesados conservan su propio LIFO. Esta es la única
prioridad especial y coincide con 11.12 y 13.7.

Por ejemplo, la acción de unwind de un archivo intenta cerrarlo, la de un proceso
aplica su política explícita de terminación y recolección, y el teardown intrínseco
de un `Join` cancela, espera y limpia cualquier resultado no transferido. Un
teardown estructurado solo puede consumir ownership que el propio constructo había
absorbido. El fallback anormal nunca convierte el cleanup ordinario en implícito.

Los movimientos que forman una llamada, agregado, retorno, `spawn` o asignación múltiple utilizan una **transferencia confirmada**:

1. Evaluar un operando afín reserva su propietario y hace que el binding deje de estar disponible para el resto de la expresión, pero todavía no confirma el handoff.
2. El handoff se confirma cuando el callee ha recibido todos los argumentos, el agregado o destino quedó construido, el retorno fue aceptado por el llamador o el hijo quedó registrado en su `scope`.
3. Si la evaluación termina antes por `?`, `fail`, pánico o cancelación, un binding
   reservado recupera su ownership y su entrada armada; un temporal sin propietario
   anterior drena la suya. Nunca se repiten efectos ya evaluados.

Este protocolo conserva el orden de evaluación de 5.12 y evita una ventana sin propietario entre dos argumentos. La comprobación terminal sigue cubriendo finales de scope y salidas estáticas como `return`, `fail`, `?`, `break` y `continue`.

Cumplen `Copy` automáticamente:

- Escalares, `Unit`, `Never`, strings y valores uniformes `fn(...)`.
- Todo `Ref[T]` bien formado —su formación ya exige `T: Discard`—, y `Pointer[T]`
  con independencia de `T`; copiar un puntero no concede ninguna operación
  segura adicional.
- Tuplas, records, enums, options, results y uniones cuando todos sus componentes son `Copy`.
- Arrays, maps y sets cuando sus elementos almacenados son `Copy`.
- `Range[T]` cuando `T: Copy`.
- El cursor intrínseco `cursor[own,C]` cuando `C: Copy`, y todo
  `cursor[ref,C]`; copiar cualquiera conserva la misma posición inicial pero
  crea un avance lógico independiente.
- Cierres concretos cuando todas sus capturas son `Copy`, según 11.8.
- Los planes inertes `Command` y `Pipeline`.
- Newtypes cuyo valor sea `Copy`.

No cumplen `Copy`:

- `Join[T, E]`.
- Un compuesto que contenga cualquier valor no `Copy`.
- Handles en propiedad que la librería o una implementación intrínseca declare afines.

`Copy` puede utilizarse como constraint genérico, pero un módulo no puede implementarlo manualmente ni alterar su derivación. Una API que necesite compartir identidad copiablemente utiliza `Ref[T]` o un handle explícitamente compartido; una API que necesite ownership único utiliza un valor afín.

`Discard` es la capacidad intrínseca cerrada complementaria a la obligación
terminal:

- Un valor cumple `Discard` exactamente cuando puede alcanzar el final de su scope
  o `_ = value` sin una operación terminal.
- `Copy` implica `Discard`, pero un valor afín también puede ser `Discard`; un
  cursor concreto que solo conserva estado local descartable es un ejemplo
  canónico.
- Newtypes, tuples, records, enums, options, results, uniones y colecciones la
  derivan cuando ninguno de sus componentes posibles conserva una obligación.
- Los cierres concretos la derivan de todas sus capturas según 11.8.
- `cursor[own,C]` la cumple exactamente cuando `C: Discard`;
  `cursor[ref,C]` siempre la cumple porque solo libera su préstamo compartido y
  su posición local.
- Implementar `Iterator[T]` no altera la derivación del cursor: sus campos y su
  contrato opaco deciden si cumple `Discard`. `Join[T, E]` nunca la cumple.
- Un tipo opaco la declara como parte de su contrato. Declarar `Discard` y una
  obligación terminal a la vez es inválido.

Un módulo no puede implementar `Discard` manualmente. Un bound `T: Discard`
permite escribir consumidores genéricos que ignoren de forma segura valores
afines sin exigir la capacidad más fuerte `Copy`.

En tipos nominales recursivos, las capacidades estructurales se calculan como un punto fijo coinductivo: una referencia recursiva al mismo grupo no falla por sí sola, pero cualquier campo alcanzable que no cumpla la capacidad hace fallar al grupo correspondiente. La misma regla se utiliza para `Discard`, `Equatable`, `Key`, `Send` y `Share`.

### 8.11 Igualdad

`Equatable` es una capacidad intrínseca cerrada. `==` y `!=` están disponibles cuando ambos operandos tienen el mismo tipo `Equatable`.

La igualdad observa ambos operandos mediante préstamos inmutables intrínsecos limitados a la operación. Nunca copia ni mueve un valor afín y deja disponibles los dos bindings.

Son `Equatable` automáticamente:

- `Unit`, `Never`, booleanos, enteros, `Byte`, floats, chars y strings.
- Tuplas cuyos elementos son equatables.
- Records cuyos campos son equatables.
- Enums cuyos payloads son equatables.
- Arrays, maps y sets de elementos equatables.
- Options, Results y uniones cuyos miembros son equatables.
- `Ref[T]`, siempre por identidad y con independencia de la igualdad de `T`.

La igualdad de `Float` sigue IEEE 754; `NaN != NaN`. Por esa razón `Float` no puede ser clave de map o set.

La igualdad de maps ignora el orden de inserción y compara pares clave/valor. La igualdad de sets compara pertenencia. La igualdad de arrays respeta longitud, orden y elementos.

`Pointer[T]` no obtiene igualdad segura automáticamente. Comparar direcciones, cuando sea necesario para FFI, utiliza una operación nombrada dentro de `unsafe`.

`Equatable` puede utilizarse como constraint genérico, pero no puede implementarse manualmente. Los tipos de dominio que necesiten otra noción de equivalencia exponen un método o trait nombrado.

### 8.12 Capacidad `Key`

`Map[K, V]` y `Set[K]` requieren `K: Key`. `Key` implica `Copy` y `Equatable`, igualdad total y un hash coherente y estable. La implementación puede mezclar una semilla de seguridad distinta entre procesos sin cambiar igualdad ni orden observable.

Son `Key`:

- `Unit` y `Never`.
- Booleanos.
- Enteros y `Byte`.
- `Char`.
- `String`.
- Newtypes cuyo valor sea `Key`.
- Tuplas de `Key`.
- Records formados enteramente por campos `Key`.
- Enums con payloads `Key`.
- `Option[K]`, `Result[K, E]` cuando ambos componentes sean `Key`, y uniones cerradas cuyos miembros sean `Key`.
- Todo `Ref[T]` bien formado, mediante identidad estable y con independencia de
  que `T` sea `Key`; la restricción separada `T: Discard` pertenece a la formación
  de `Ref`, no a la identidad.

No son `Key`:

- `Float` y `Float32`.
- Arrays.
- Maps.
- Sets.
- Funciones o cierres.
- `Pointer[T]`; una dirección raw puede caducar o reutilizarse.
- Tipos opacos nativos que no declaren la propiedad.

`Key` es una capacidad intrínseca cerrada y puede utilizarse como constraint genérico, pero no es un trait que el usuario pueda implementar con hash o igualdad arbitrarios. Un tipo genérico que contenga `Map[K, V]` o `Set[K]` debe declarar `K: Key`. Esto garantiza comprobación paramétrica y evita que dos módulos discrepen sobre la identidad de una clave.

### 8.13 Representación y ABI

La representación física de records, enums, tuples, unions, colecciones,
cursores y entornos de cierre no forma parte de la semántica fuente 0.1. El
compilador puede:

- Reordenar padding.
- Elegir el ancho de discriminantes.
- Aplicar niche optimization.
- Compartir buffers.
- Insertar indirección en tipos recursivos.
- Inlinear un cierre concreto o materializar su entorno, y representar una
  coerción a `fn(...)` mediante los punteros o indirecciones que necesite.
- Representar un resultado `impl Bound` con su tipo concreto oculto, sin añadir
  por ello una vtable o allocation observable.

No puede cambiar igualdad, orden de evaluación, capacidades derivadas, modo de
llamada ni valores observables.

`pub` exporta una declaración a otros módulos Tondo; no publica por sí solo un
símbolo nativo ni fija name mangling. Tondo 0.1 no promete una ABI binaria estable
para funciones, genéricos, tipos normales, cierres ni interfaces compiladas entre
versiones distintas del compilador. Un artefacto binario o interfaz de módulo
registra como mínimo versión de formato, compilador, edición, target, perfil,
capacidades, features, `PackageId`, hash de API y dependencias exactas; el consumidor
rechaza una combinación incompatible en lugar de intentar enlazarla.

Código que necesite layout, alineación, calling convention o endianness estables
utilizará tipos y declaraciones FFI de una especificación separada. Un record
Tondo normal no debe serializarse ni cruzar una frontera nativa copiando su
memoria.

La especificación FFI 0.1 deberá fijar, sin alterar la gramática segura de esta
edición:

- Tipos con layout explícito, tamaños, alineación, endianness y reglas de padding.
- Importación y exportación de símbolos, calling conventions y enlace estático o
  dinámico.
- Correspondencia de escalares, punteros opcionales, arrays, strings y callbacks.
- Ownership de cada parámetro y resultado, incluido quién retiene, libera o fija
  temporalmente un objeto administrado.
- Registro de threads extranjeros y conservación de roots durante callbacks.
- Comportamiento ante excepciones extranjeras, pánicos Tondo y terminación
  anormal en la frontera.
- Reglas de procedencia, aliasing, mutabilidad, inicialización y validez de
  representaciones recibidas.

Una frontera nativa no puede desenrollar un pánico Tondo a través de frames
extranjeros salvo que una ABI futura lo declare expresamente compatible. La
frontera por defecto completa el unwind Tondo hasta su adaptador y aborta antes de
cruzarlo; un wrapper que necesite recuperación convierte antes el fallo en un
valor o protocolo nativo explícito. De forma simétrica, una excepción extranjera
no entra en frames Tondo: el adaptador la convierte o aborta.

El mecanismo 0.1 para intrinsics y bindings nativos utiliza unidades privilegiadas
o descriptores del toolchain. No añade declaraciones `extern`, atributos
arbitrarios ni pragmas a un archivo `.to` ordinario. Incorporar después nueva
sintaxis fuente para FFI requerirá una edición que la incluya en su gramática.

---

## 9. Tipos compuestos

### 9.1 Tuplas

Una tupla agrupa un número fijo de valores sin asignar nombres semánticos a sus posiciones:

~~~tondo
let point = (10, 20)
let response = (status, headers, body)
~~~

Tipos:

~~~tondo
(Int, Int)
(Status, Map[String, String], String)
~~~

La tupla de cero elementos es `()` y tiene tipo `Unit`. Tondo no tiene tupla de un solo elemento: `(value)` es una expresión agrupada.

Acceso posicional:

~~~tondo
let x = point.0
let y = point.1
~~~

Desestructuración:

~~~tondo
let (x, y) = point
~~~

Las tuplas son estructurales. Dos tuplas son del mismo tipo si tienen la misma longitud y tipos de elementos idénticos en cada posición.

Las tuplas se recomiendan para:

- Retornos locales múltiples.
- Entradas de maps.
- Intercambio temporal de valores.
- Adaptadores internos.

Los records se recomiendan cuando las posiciones tienen significado estable, especialmente en APIs públicas.

### 9.2 Records

Declaración:

~~~tondo
pub type User = {
    id: UserId
    name: String
    email: String?
}
~~~

Construcción:

~~~tondo
let user = User {
    id: UserId(42)
    name: "Ada"
    email: none
}
~~~

Los campos se separan por nuevas líneas o comas. El parser acepta una coma final; el formateador usa nuevas líneas y omite comas en records multilínea.

Cuando el nombre del binding coincide, se permite la forma corta:

~~~tondo
let id = UserId(42)
let name = "Ada"

let user = User {
    id
    name
    email: none
}
~~~

Todos los campos deben inicializarse exactamente una vez. No existen:

- Campos sin inicializar.
- Inicializadores de campos ejecutados ocultamente.
- Propiedades calculadas dentro del record.
- Constructores especiales.
- Herencia de records.

Los invariantes se expresan mediante campos privados y funciones de construcción:

~~~tondo
pub type Email = {
    priv value: String
}

pub fn Email.parse(text: String): Email ! EmailError {
    if not isValidEmail(text) {
        fail EmailError.Invalid(text)
    }

    Email { value: text }
}
~~~

### 9.3 Actualización de records

Tondo 0.1 utiliza actualización funcional explícita mediante la keyword `with`:

~~~tondo
let renamed = user with {
    name: "Grace"
}
~~~

El resultado es un record nuevo. Los campos no mencionados conservan su valor. El operador:

- Solo funciona sobre records.
- Debe actualizar al menos un campo; `value with {}` no es una forma alternativa de copiar.
- No puede actualizar campos privados desde otro módulo.
- Cada nombre debe ser un campo existente y puede aparecer como máximo una vez.
- Evalúa el record base antes que los nuevos valores.
- Evalúa actualizaciones de arriba abajo.
- No modifica el record original cuando este cumple `Copy`.
- Consume un record base afín y transfiere al resultado sus campos no actualizados; el binding base queda no disponible.
- Puede aplicarse a un record que conserve una obligación terminal siempre que
  cada campo actualizado cumpla `Discard`. Los campos no actualizados, incluidos
  los obligados y privados, se transfieren intactos al resultado, que conserva la
  obligación estructural del tipo.
- No puede reemplazar mediante `with` un campo cuyo valor anterior tenga una
  obligación terminal, aunque el valor nuevo tenga la misma. Ese reemplazo debe
  extraer o consumir explícitamente el owner anterior antes de construir el
  record nuevo.

La actualización completa utiliza transferencia confirmada: evalúa primero la
base y después todos los valores nuevos; solo cuando ninguno puede fallar mueve
los campos conservados y publica el resultado. Si la evaluación sale antes por
error, pánico o cancelación, el propietario base se restaura o el temporal ejecuta
su acción de unwind según 8.10. Un guard `defer` asociado al record sigue el
movimiento al resultado como cualquier otro movimiento local del mismo tipo.

En código mutable puede asignarse el resultado:

~~~tondo
var user = initialUser
user = user with { name: "Grace" }
~~~

### 9.4 Enums nominales

Un enum es una unión cerrada y nominal de variantes:

~~~tondo
pub enum Shape {
    Circle(Float)
    Rectangle(Float, Float)
    Point
}
~~~

Construcción:

~~~tondo
let first = Shape.Circle(10.0)
let second = Shape.Rectangle(20.0, 30.0)
let origin = Shape.Point
~~~

Una variante puede tener:

- Ningún payload.
- Payload posicional.
- Payload record.

Todo enum declara al menos una variante. Un conjunto de casos vacío se expresa
con `Never` —o con un newtype nominal sobre él cuando la frontera necesite nombre
propio—, no mediante una segunda construcción de tipo vacío.

Un payload posicional o record contiene al menos un valor. Una variante sin payload se escribe únicamente como `Variant`, nunca `Variant()` ni `Variant {}`.

~~~tondo
enum HttpResult {
    Success {
        status: Int
        body: String
    }
    Redirect(Url)
    Offline
}
~~~

Una variante y su payload forman un único valor. El tag discriminante siempre está disponible para `match`, pero su representación binaria concreta no forma parte del contrato salvo en tipos diseñados para FFI.

### 9.5 Uniones estructurales

`A | B` representa una unión cerrada anónima:

~~~tondo
let identifier: Int | String = 42
~~~

Reglas:

- La unión es independiente del orden: `A | B` equivale a `B | A`.
- Las uniones se aplanan: `A | (B | C)` equivale a `A | B | C`.
- Los miembros duplicados se eliminan.
- `Never | T` equivale a `T`.
- Un valor se inyecta automáticamente si su tipo coincide exactamente con un miembro.
- No se buscan conversiones para encontrar un miembro compatible.
- El tag del miembro forma parte del valor.
- Después de expandir aliases, cada miembro debe ser un tipo nominal o intrínseco expresable como `type_path`. Tuplas, funciones y otros tipos estructurales anónimos se envuelven en un newtype antes de formar parte de una unión.
- Un parámetro de tipo desnudo no puede ser miembro, porque 0.1 no tiene un constraint que garantice un discriminador canónico para todas sus instanciaciones. Un `Either[A, B]` genérico se declara como enum nominal.
- En contexto genérico, se normaliza cada path y se aplica la misma unificación
  sintáctica de primer orden que a las cabeceras de `impl` en 12.6. Si cualquier
  par unifica al sustituir parámetros ligados, la unión se rechaza y se utiliza
  un enum nominal. Esta decisión no depende de búsquedas heurísticas ni de las
  instanciaciones presentes en el programa.

Las uniones estructurales son adecuadas para:

- Errores internos pequeños.
- Valores heterogéneos declarados.
- Adaptadores locales.

Para una API pública estable se prefiere un enum nominal porque:

- Nombra la abstracción.
- Permite renombrar y agrupar casos.
- Evita filtrar tipos internos.
- Permite añadir datos contextuales.

### 9.6 Discriminación de uniones por tipo

Una unión se consume mediante `match`:

~~~tondo
fn render(value: Int | String): String {
    match value {
        Int(number) => "number: {number}"
        String(text) => "text: {text}"
    }
}
~~~

`Int(number)` y `String(text)` son patrones de tipo, no conversiones.

La forma general `MemberType(pattern)` comprueba primero el tag de la unión y aplica después el patrón interior al valor completo del miembro. También funciona con instanciaciones genéricas:

~~~tondo
match value {
    Array[Int](numbers) => sum(numbers)
    Option[String](optional) => renderOptional(optional)
}
~~~

Cuando los miembros son records o enums nominales, sus propios patrones discriminan:

~~~tondo
match error {
    IoError { path: _, reason: _ } => ()
    DecodeError { line: _, column: _ } => ()
}
~~~

### 9.7 `Option[T]`

`Option[T]` expresa presencia o ausencia sin `null`:

~~~tondo
enum Option[T] {
    Some(T)
    None
}
~~~

La forma de tipo canónica es:

~~~tondo
T?
~~~

Constructores y patrones intrínsecos:

~~~tondo
some(value)
none
~~~

Ejemplo:

~~~tondo
fn findUser(users: Array[User], id: UserId): User? {
    for user in users {
        if user.id == id {
            return user
        }
    }

    none
}
~~~

En una función cuyo resultado es `T?`:

- Un valor `T` se eleva a `some(value)`.
- `none` representa ausencia.
- Un valor ya tipado como `T?` se devuelve sin envolver otra vez.

Las options anidadas no se aplanan:

~~~tondo
(Int?)?
~~~

puede distinguir:

- `none` exterior.
- `some(none)`.
- `some(some(42))`.

El formateador exige paréntesis para options anidadas; `Int??` no es forma canónica.

Una unión opcional también se agrupa: `(A | B)?`. Sin paréntesis, `A | B?` significa `A | Option[B]` por la precedencia de tipos.

### 9.8 `Result[T, E]`

`Result[T, E]` separa éxito y error recuperable:

~~~tondo
enum Result[T, E] {
    Ok(T)
    Err(E)
}
~~~

La forma compacta es:

~~~tondo
T ! E
~~~

Cuando el valor de éxito es `Unit`, la forma compacta adicional es:

~~~tondo
!E
~~~

Por tanto:

~~~text
T ! E  = Result[T, E]
!E     = Result[Unit, E]
~~~

Por ejemplo:

~~~tondo
fn readUser(path: Path): User ! IoError
~~~

equivale a:

~~~tondo
fn readUser(path: Path): Result[User, IoError]
~~~

Constructores y patrones intrínsecos:

~~~tondo
ok(value)
err(error)
~~~

En una función `T ! E`, el cuerpo se escribe en términos del valor de éxito `T`:

~~~tondo
fn parsePort(text: String): Int ! ParseError {
    let port = parseInt(text)?

    if port < 1 or port > 65_535 {
        fail ParseError.OutOfRange(port)
    }

    port
}
~~~

No se escribe `ok(port)` en el camino feliz. El compilador eleva el valor final o un `return value` a `ok(value)`. `fail error` devuelve `err(error)`.

### 9.9 Error sin valor de éxito

Cuando el valor de éxito es `Unit`, `!E` evita repetirlo. En una firma, todo resultado fallible sigue apareciendo después de `:`:

~~~tondo
fn save(config: Config): !IoError {
    _ = config
}
~~~

equivale a:

~~~tondo
fn save(config: Config): Unit ! IoError
~~~

Este no es un caso especial de `main`; `!E` es una expresión de tipo válida donde se espere un tipo y no una excepción gramatical reservada a funciones.

### 9.10 Tipos recursivos

Records, enums y newtypes pueden formar grupos recursivos:

~~~tondo
enum Json {
    Null
    Boolean(Bool)
    Number(Float)
    Text(String)
    Array(Array[Json])
    Object(Map[String, Json])
}
~~~

La implementación inserta la indirección necesaria. El usuario no escribe `Box`
ni punteros para hacer finita la representación.

Todo componente fuertemente conexo de tipos nominales recursivos debe ser
**productivo**: calculando desde los escalares y constructores no recursivos, cada
tipo del grupo debe adquirir al menos una forma de construir un valor finito.
Una variante base, `none`, una colección vacía, una función u otro constructor
que no exija materializar inmediatamente el siguiente nodo puede cerrar la
recursión. Tuples, records, payloads, newtypes y `Ref[T]` que exijan ya un valor
del mismo grupo no la cierran por sí solos.

Por ello una recursión puramente inmediata se rechaza:

~~~tondo compile-fail E1107
type Invalid = {
    next: Invalid
}
~~~

También se rechazan `type Invalid = Invalid`, un ciclo solo de newtypes o un enum
cuyas variantes exijan todas otro valor del mismo ciclo. Si se necesita expresar
deliberadamente ausencia de valores se utiliza `Never`, no una declaración
cíclica accidental.

Un newtype recursivo sí es válido cuando existe base finita:

~~~tondo
type Chain = (Int, Chain?)
~~~

El análisis de productividad es un punto fijo mínimo y forma parte del type
checking; la estrategia concreta de indirección sigue sin ser observable.

### 9.11 Tipos opacos nativos

Una implementación o módulo privilegiado puede declarar tipos cuya representación no es visible al lenguaje normal, por ejemplo handles del sistema. Desde código Tondo se comportan como valores nominales y solo exponen las operaciones públicas declaradas.

El mecanismo para declarar intrinsics y enlazar código nativo pertenece a la especificación de implementación y FFI, no a la sintaxis segura 0.1.

---

## 10. Colecciones intrínsecas

### 10.1 Principio general

El núcleo tiene cuatro colecciones conceptuales:

~~~text
Array[T]
Map[K, V]
Set[K]
Range[T]
~~~

`Iterator[T]` es el protocolo estático de consumo secuencial; no es otra
colección ni un tipo de almacenamiento.

No existen simultáneamente `List`, `Vector`, `Slice` y `Span` como sinónimos. `Array[T]` cubre el contenedor dinámico y la vista.

### 10.2 `Array[T]`

`Array[T]` es una secuencia dinámica, ordenada, indexable y con semántica de valor. Cumple `Copy` cuando `T: Copy`; en otro caso se mueve como un agregado afín.

Conceptualmente, un valor contiene:

~~~text
storage
offset
length
stride
~~~

Un literal posee inicialmente un almacenamiento compacto:

~~~tondo
let values = [10, 20, 30, 40, 50]
~~~

Un slice crea en O(1) otro `Array[T]` que describe una región del mismo almacenamiento:

~~~tondo
let middle = values[1:4]
let reverse = values[::-1]
~~~

La compartición es una optimización no observable bajo semántica inmutable. Si una copia lógica se modifica, copy-on-write separa el almacenamiento.

### 10.3 Indexación de arrays

~~~tondo
let first = values[0]
let last = values[-1]
~~~

Para un array de longitud `n`:

- El índice debe tener tipo `Int`.
- Un índice no negativo `i` se refiere directamente a `i`.
- Un índice negativo se normaliza a `n + i`.
- `-1` es el último elemento.
- `-n` es el primero.
- Un índice normalizado fuera de `0 <= i < n` produce un pánico de bounds.

La longitud de todo `Array`, `Map`, `Set` o `String` cabe siempre en `Int`;
`Range` y los cursores no tienen esa limitación porque no necesitan exponer
longitud. La suma conceptual `n + i` se calcula en enteros matemáticos, sin
overflow intermedio, antes de comprobar el rango; por tanto incluso `Int.min`
produce simplemente un índice inválido y el pánico de bounds correspondiente.

Ninguna operación puede envolver una longitud. Un literal cuyo tamaño no sea
representable es error de compilación. En runtime, alcanzar el máximo
representable se trata como agotamiento de recursos de la implementación, igual
que no poder reservar el almacenamiento; una API de librería puede ofrecer además
una variante checked con error recuperable, pero el núcleo no vuelve fallible cada
`append`.

La indexación directa devuelve `T` y por ello requiere `T: Copy`; nunca mueve un
elemento dejando un hueco. Un elemento afín se observa mediante receptor `self`,
`ref values[index]`, `for ref` o APIs callback, y se extrae mediante una operación
consumidora como `remove` o `pop`. La consulta recuperable `get(index): T?`
también requiere `T: Copy`.

La restricción `T: Copy` solo se aplica al materializar el elemento como valor.
Una indexación que permanece como ubicación puede usarse sin copiar: destino de
asignación o swap, argumento `mut` o `var` compatible, receptor
`self`/`mut self`/`var self`, argumento `ref`, o scrutinee estable de un match de
observación. Por ejemplo, `resources[index].status()` o
`inspect(ref resources[index])` prestan ese elemento durante la llamada y no lo
extraen del array. El préstamo termina con la operación y no puede almacenarse.

La diferencia es intencional:

- Un índice directo inválido rompe un invariante del algoritmo.
- Una consulta `get` modela datos no confiables o ausencia esperable.

### 10.4 Slicing

Sintaxis:

~~~tondo
values[start:end]
values[start:]
values[:end]
values[:]
values[start:end:step]
values[::step]
~~~

Reglas:

- `start`, `end` y `step`, cuando aparecen, deben tener tipo `Int`.
- `start` es inclusivo.
- `end` es exclusivo.
- Los índices negativos se calculan desde el final.
- Los extremos se limitan al rango válido en lugar de producir pánico.
- El paso por defecto es `1`.
- Un paso negativo recorre en sentido inverso.
- Un paso `0` produce pánico porque viola el contrato de slicing.
- Los valores por defecto de `start` y `end` dependen del signo del paso, igual que en Python.
- El resultado vuelve a ser `Array[T]`.
- El resultado conserva el almacenamiento mientras sea seguro y puede tener stride distinto de uno.
- Crear un slice lógico que pueda almacenarse como valor requiere `T: Copy`. Una
  colección de elementos afines se recorre o transforma consumiéndola, sin crear
  una segunda vista propietaria de los mismos elementos.

Las formas `ref values[start:end:step]` y `mut values[start:end:step]` son
distintas del slice como valor: crean respectivamente un préstamo compartido o
exclusivo de la región, no un segundo propietario. Por ello pueden utilizarse
aunque `T` no sea `Copy`, quedan ligadas a la llamada y siguen todas las reglas de
16.4. Sin modificador, la expresión continúa siendo un snapshot lógico y exige
`T: Copy`.

La normalización es exacta y se realiza en enteros matemáticos, por lo que ningún
extremo o paso representable como `Int` puede provocar overflow durante el
cálculo. Sea `n` la longitud:

1. Si `step` se omite, vale `1`; si vale `0`, se produce el pánico indicado.
2. Con paso positivo, `start` omitido vale `0` y `end` omitido vale `n`. Cada
   extremo explícito negativo suma `n`; después ambos se limitan a `[0, n]`.
3. Con paso negativo, `start` omitido vale `n - 1` y `end` omitido es un
   centinela anterior al índice `0`. Cada extremo **explícito** negativo suma
   `n`; después ambos se limitan a `[-1, n - 1]`. El centinela omitido no se
   transforma, de modo que `values[::-1]` incluye el elemento `0`, mientras
   `values[:-1:-1]` puede ser vacío.
4. El resultado contiene los índices `start + k * step` que permanezcan antes de
   `end` para paso positivo o después de `end` para paso negativo. Su longitud se
   calcula antes de construir la vista y debe caber en `Int`, como toda longitud
   de colección.

Estas reglas incluyen `Int.min` como paso negativo válido. La representación de la
vista puede usar una forma interna distinta de un stride firmado si fuera necesario;
el valor observable sigue siendo la progresión matemática anterior.

Ejemplos:

~~~tondo
let head = values[:3]
let tail = values[2:]
let alternate = values[::2]
let reversed = values[::-1]
let all = values[-100:100]
~~~

La última expresión produce todo el array porque los límites se recortan.

### 10.5 Mutación de arrays

~~~tondo
var values = [1, 2, 3, 4, 5]

values[0] = 10
values[1:4] += 100
values[::2] *= -1
~~~

Una asignación a slice requiere el mismo número de elementos:

~~~tondo
values[1:3] = [20, 30]
~~~

No puede cambiar la longitud del array. Reemplazar una región por otra longitud es una operación nombrada de colección, no semántica especial de `[]=`.

Las operaciones que cambian la longitud, como `append`, `insert`, `remove` o
`clear`, requieren un receptor `var self` o un parámetro `var Array[T]`. Sobre un
binding local `var`, la dot-call continúa siendo tersa
—`values.append(item)`—; al cruzar una firma libre el contrato aparece como
`append(var values, item)`. Un slice puede prestarse como `ref` o `mut`, nunca
como `var`.

El lado derecho se evalúa por completo y su longitud se valida antes de la primera escritura. Si origen y destino se solapan, el lado derecho actúa como snapshot; una asignación que falla antes de escribir deja el array intacto.

Modificar una variable que comparte almacenamiento con valores inmutables realiza detach antes de escribir. Una vista mutable explícita, descrita en la sección 16, modifica el origen después de separar primero cualquier snapshot lógico anterior. Reemplazar mediante `[]=` uno o varios elementos con obligación terminal es error; se utiliza una operación `replace` o de región que devuelve todos los valores anteriores.

### 10.6 Aritmética de arrays

Los operadores numéricos escalares se elevan de forma cerrada sobre arrays:

~~~tondo
let a = [1, 2, 3]
let b = [10, 20, 30]

let sum = a + b
let product = a * b
let shifted = a + 10
let inverse = 10 - a
~~~

Resultados:

~~~text
sum      = [11, 22, 33]
product  = [10, 40, 90]
shifted  = [11, 12, 13]
inverse  = [9, 8, 7]
~~~

Reglas de elevación:

~~~text
si A op B -> C, entonces:

Array[A] op Array[B] -> Array[C]
Array[A] op B        -> Array[C]
A op Array[B]        -> Array[C]
~~~

Cuando ambos operandos son arrays se elige siempre la operación elemento a elemento, no broadcasting del array derecho como escalar.

Los arrays emparejados deben tener la misma longitud en cada nivel. Una diferencia de forma produce pánico de contrato. Cuando una diferencia sea parte normal del dominio, el programa debe validarla o utilizar una operación checked de la librería.

Los operadores elevados son:

~~~text
+ - * / %
~~~

Solo se elevan cuando la operación de los elementos es una operación numérica intrínseca válida. El usuario no puede redefinirla.

La operación:

- Evalúa operandos de izquierda a derecha.
- Produce un array lógico nuevo y compacto.
- No modifica operandos.
- Puede ser fusionada por el compilador para eliminar temporales.
- Debe conservar orden de evaluación, pánicos y resultados.

`+=`, `-=`, `*=`, `/=` y `%=` realizan la variante in-place sobre un binding
`var` o un préstamo `mut` o `var`.

Una operación in-place tiene garantía fuerte: comprueba forma y calcula todos los resultados que puedan producir pánico antes de hacer observable la primera escritura. Si ocurre overflow, división por cero o incompatibilidad, el array conserva íntegramente su valor anterior. El compilador puede escribir directamente sobre el buffer solo cuando demuestre que preserva esa garantía.

### 10.7 Concatenación, repetición y producto algebraico

`+` nunca concatena arrays y `*` nunca repite arrays. Esas operaciones se nombran:

~~~tondo
a.concat(b)
a.repeat(3)
~~~

Producto escalar, producto matricial y broadcasting multidimensional pertenecen a módulos matemáticos:

~~~tondo
math.dot(a, b)
matrix.matmul(left, right)
~~~

Esto evita que `*` cambie de significado según forma o dimensionalidad.

### 10.8 Igualdad y comparación de arrays

~~~tondo
a == b
a != b
~~~

devuelven un único `Bool` estructural.

Los operadores relacionales `< <= > >=` no están definidos para arrays. Una comparación elemento a elemento utiliza una operación nombrada de la librería y devuelve `Array[Bool]`.

### 10.9 `Map[K, V]`

`Map[K, V]` es un map hash ordenado por inserción, homogéneo y con semántica de valor. `K` siempre es `Copy` porque cumple `Key`; el map completo cumple `Copy` cuando `V: Copy`.

Literal:

~~~tondo
let ages = [
    "ana": 32,
    "leo": 28,
]
~~~

Vacío:

~~~tondo
let ages: Map[String, Int] = [:]
~~~

Las claves deben cumplir `Key`.

### 10.10 Consulta de maps

La ausencia de una clave es normal; por eso:

~~~tondo
let age: Int? = ages["ana"]
~~~

La indexación de lectura devuelve `V?` y requiere `V: Copy`; nunca mueve el valor
almacenado. La futura librería puede ofrecer una consulta afirmativa como
`at(key): V`, también limitada a `V: Copy`, y debe fijar en su propia
especificación la clase de pánico o contrato checked ante ausencia. Un valor por
defecto se obtiene mediante una operación como `getOr`. Maps con valores afines
se observan mediante `for ref` o callbacks de lookup y transfieren ownership
mediante una operación como `remove`. Los nombres y firmas detallados pertenecen
a la librería estándar.

Un map de valores opcionales conserva dos niveles:

~~~tondo
let values: Map[String, Int?] = [:]
let result: (Int?)? = values["answer"]
~~~

No hay flattening implícito.

### 10.11 Mutación de maps

~~~tondo
var ages = ["ana": 32]

ages["leo"] = 28
ages["ana"] = 33
~~~

En contexto de asignación, `map[key] = value` inserta o reemplaza.

Insertar o eliminar una clave cambia la extensión del map y requiere un binding local `var`, un receptor `var self` o un parámetro `var Map[K, V]`. Un préstamo `mut Map[K, V]` solo puede actualizar una clave cuya presencia se afirme mediante una operación nombrada.

La asignación compuesta sobre un índice de map no existe en 0.1. Como la clave podría estar ausente, `map[key] += value` ocultaría una política. Se utiliza una asignación explícita con `getOr` o la API `entry` de la librería. Si `V` puede contener una obligación terminal, `map[key] = value` también es error porque la clave podría reemplazar un valor todavía pendiente; se utiliza una operación que devuelva el valor anterior.

Eliminar se expresa únicamente mediante una operación nombrada:

~~~tondo
let removed: Int? = ages.remove("leo")
~~~

Asignar `none` almacena ausencia cuando `V` es opcional; nunca elimina la clave.

### 10.12 Orden de maps

La iteración sigue orden de inserción:

- Insertar una clave nueva la añade al final.
- Reemplazar su valor conserva la posición.
- Eliminarla y reinsertarla la coloca al final.
- Copiar conserva el orden.
- Una operación de merge conserva primero el orden izquierdo; nuevas claves derechas se añaden en orden derecho.

La tabla hash puede usar una semilla aleatoria de seguridad, pero esa semilla no puede alterar el orden observable.

En un literal con claves repetidas:

- Una duplicación detectable entre constantes produce `E1116`.
- Si `V: Discard` y dos expresiones dinámicas producen la misma clave, la última
  reemplaza el valor y se conserva la posición de la primera inserción.

Si `V` puede conservar una obligación terminal, la validez no depende de probar
que las claves dinámicas sean distintas: todas las claves y valores se evalúan de
izquierda a derecha en temporales y, antes de construir el map, se ejecuta una
comprobación obligatoria de duplicados. Claves distintas construyen el map; una
duplicación produce `P0009 duplicate-dynamic-map-key` antes de transferir ownership al
map, y el unwind libera todos los temporales. Una API que devuelva el valor
reemplazado permite manejar duplicados como dato recuperable.

### 10.13 Iteración de maps

La forma canónica itera entradas:

~~~tondo
for (key, value) in ages {
    console.print("{key}: {value}")
}
~~~

Vistas explícitas permiten iterar solo claves o valores:

~~~tondo
for key in ages.keys() {
    _ = key
}

for value in ages.values() {
    _ = value
}
~~~

La iteración ordinaria de un map `Copy` recorre un snapshot lógico. El programa puede mutar el binding original durante el bucle; copy-on-write mantiene estable el orden y contenido que ve el cursor. Un map afín se mueve al cursor y su binding deja de estar disponible. Modificar en sitio la colección que se está recorriendo requiere un protocolo mutable específico de librería que garantice exclusividad.

`for (key, ref value) in map` utiliza en cambio la iteración observacional de
13.3: copia la clave `Key`, presta el valor y deja disponible el map después del
bucle sin exigir `V: Copy`.

### 10.14 Igualdad y combinación de maps

`==` compara contenido, no orden.

No existen operadores `+`, `|` ni `&` para maps. Sus significados ante claves repetidas serían ambiguos. La combinación es una operación nombrada, como `merge` o `mergeWith`, definida por la librería.

### 10.15 `Set[K]`

`Set[K]` representa pertenencia única y conserva orden de inserción.

~~~tondo
let permissions = Set["read", "write"]

if "read" in permissions {}
~~~

Reglas:

- `K` debe cumplir `Key`.
- El set cumple `Copy` porque todo `Key` es `Copy`.
- Un elemento duplicado no crea una segunda entrada.
- Reinsertar un elemento existente conserva posición.
- Eliminarlo y reinsertarlo lo coloca al final.
- La igualdad compara pertenencia e ignora orden.
- `let`/`var` y copy-on-write siguen las mismas reglas que arrays y maps.

Insertar o eliminar pertenencia cambia la extensión y, a través de una firma, exige `var Set[K]`. La consulta solo observa y las operaciones que no alteren pertenencia pueden utilizar `self` o `mut self` según su contrato.

Un duplicado constante dentro de un literal `Set[...]` produce warning, no error, porque el resultado sigue siendo inequívoco.

Unión, intersección, diferencia y subset son operaciones nombradas. `|` conserva sus significados cerrados de unión de tipos, bitwise o pipeline según contexto y tipos estáticos; nunca opera sobre sets.

### 10.16 `Range[T]`

Un range describe una secuencia discreta sin materializar un array.

Final exclusivo:

~~~tondo
0..10
~~~

Final inclusivo:

~~~tondo
0..=10
~~~

Tipos:

~~~tondo
Range[Int]
Range[Char]
~~~

Los operadores de range requieren extremos del mismo tipo discreto. El paso por defecto es uno. Pasos personalizados y ranges descendentes se construyen mediante APIs nombradas para mantener inequívoca la sintaxis.

Son discretos los enteros intrínsecos con y sin signo y `Char`; `Byte` conserva su papel binario y no participa. Un range ascendente cuyo inicio sea mayor que su final está vacío. La iteración inclusiva termina al emitir el extremo y no calcula un sucesor que pueda desbordar. `Range[Char]` avanza por valores escalares Unicode y salta el intervalo reservado a surrogates.

Un range puede:

- Iterarse.
- Consultar pertenencia con `in`.
- Utilizarse en APIs que acepten rangos.

No es un array y no asigna memoria proporcional a su longitud.

### 10.17 `Iterator[T]`

`Iterator[T]` es un trait intrínseco predefinido e implementable por tipos de
usuario. Describe un cursor concreto que produce valores `T`:

~~~tondo
trait Iterator[T] {
    fn next(mut self): T?
}
~~~

En posición de tipo, `Iterator[T]` solo puede aparecer como constraint, en la
cabecera de un `impl` o dentro del resultado opaco estático de 12.8; no es un tipo
de valor ni borra la representación del cursor. No puede utilizarse por sí solo
como field, parámetro o retorno. Una API devuelve un cursor nominal, utiliza
`impl Iterator[T] + Discard` cuando puede ocultarlo sin obligación terminal, o es
genérica:

~~~tondo
fn drain[T: Discard, I: Discard + Iterator[T]](cursor: I) {
    for item in cursor {
        _ = item
    }
}
~~~

Para cada target normalizado puede existir como máximo una implementación de
`Iterator[T]`, sea cual sea `T`. Esta regla funcional es adicional a la coherencia
general de 12.6 y permite inferir un único tipo de elemento para `for`. Dos
implementaciones como `Iterator[Int] for Cursor` e `Iterator[String] for Cursor`
producen `E1113`, incluso si no se solapan como instanciaciones ordinarias de un
trait.

Para cabeceras genéricas, se unifican primero solo sus targets normalizados. Si
existe un unificador más general, se aplica también a ambos argumentos `T`: si no
quedan idénticos, las cabeceras producen `E1113`; si quedan idénticos, se aplica la
coherencia ordinaria y una duplicación produce `E1111`. Así
`Iterator[T] for Cursor[T]` entra en conflicto funcional con
`Iterator[Int] for Cursor[String]` al sustituir `T = String`.

El protocolo solo exige que `next` reciba un préstamo exclusivo y devuelva
`none` al terminar. No impone que el cursor sea `Copy`, `Discard`, `Send`,
`Share` o terminal. El tipo concreto conserva esas capacidades según 8.10 y
16.13:

- Un cursor puede ser copiable; cada copia representa entonces un cursor lógico
  independiente conforme al contrato de su tipo.
- Un cursor puede ser afín y descartable.
- Un cursor que posea un archivo, conexión u otro recurso conserva su obligación
  terminal y puede no ser `Send`.
- Aunque un cursor concreto cumpla `Share`, avanzar siempre exige un lvalue
  prestado en exclusiva mediante `mut`.
- El protocolo no promete indexación, longitud, finitud ni reinicio.

`for` acepta:

- Arrays.
- Maps.
- Sets.
- Ranges.
- Strings por sus valores `Char`.
- Cualquier valor concreto `I` para el que exista `I: Iterator[T]`.

Para las cinco fuentes intrínsecas, el compilador construye un tipo de cursor
concreto definido por el lenguaje. Para cualquier otra fuente, la propia
expresión ya es el cursor. Los adaptadores de librería también devuelven tipos de
cursor concretos: nominales cuando exponen cleanup o identidad, y opcionalmente
resultados `impl Iterator[T] + Discard` cuando pueden ocultarse. Nunca borran sus
capacidades detrás de un valor `Iterator[T]`.

Los cursores de las cinco fuentes intrínsecas tienen una representación de tipo
interna que no puede escribirse en fuente:

- `cursor[own,C]` conserva en propiedad una colección `C` y su posición. Deriva
  `Copy`, `Discard`, `Send` y `Share` de la capacidad homónima de `C`.
- `cursor[ref,C]` conserva un préstamo compartido de `C` y su posición. Siempre
  cumple `Copy + Discard`; cumple tanto `Send` como `Share` únicamente cuando
  `C: Send + Share`, sin que eso permita al préstamo escapar de las regiones
  autorizadas por 16.13.
- Ninguna de las dos formas cumple `Equatable` ni `Key`: la igualdad observable
  de un estado mutable de recorrido no forma parte del lenguaje.

Una copia admitida comienza en la misma posición y avanza de forma
independiente. Copiar `cursor[own,C]` realiza la copia lógica de `C`; copiar
`cursor[ref,C]` duplica el préstamo compartido, nunca la colección ni su
ownership. Descartar un cursor no consume ni oculta obligaciones que no posea.

`for pattern in expression` evalúa `expression` exactamente una vez, transfiere
el cursor concreto a un propietario interno y llama a `Iterator.next` hasta
recibir `none`. El patrón debe ser irrefutable para el `T` único de la
implementación.

Un cursor concreto terminal requiere que una operación de cleanup quede
reservada antes de entrar en el bucle:

~~~tondo fragment spec.cursor
fn consumeRows(query: Query): !DatabaseError {
    let rows = Database.openRows(query)?
    defer RowCursor.close(rows)

    for row in rows {
        consume(row)
    }
}
~~~

El guard sigue el movimiento hacia el propietario interno del `for`. Al recibir
`none` o ejecutar `break`, el cursor restante se transfiere al slot oculto del
guard, que conserva el orden LIFO y solo ejecuta su operación al abandonar el
scope léxico original. `return`, `fail` y `?` realizan esa misma transferencia
antes de ejecutar los defers del scope. Durante pánico o cancelación se ejecuta el
defer registrado; la acción de unwind del tipo concreto queda para un temporal
cuya transferencia o registro no llegó a confirmarse.

Recibir `none` no elimina por sí solo una obligación terminal de usuario, porque
`next` solo presta el cursor. Los cursores intrínsecos que drenan una colección de
elementos terminales son la única excepción: el lenguaje conoce que el
agotamiento ha transferido todos sus elementos, consume al propietario interno y
desactiva cualquier guard que ya no tenga valor que limpiar. Una salida temprana
de ese drenaje continúa exigiendo un cleanup reservado.

En un header ordinario sin bindings `ref`, evaluar una colección `Copy` para un
`for` crea una copia lógica y deja disponible el origen. Evaluar una colección no
`Copy` mueve la colección completa al cursor y produce sus elementos por
movimiento; el binding original deja de estar disponible. Un cursor concreto se
copia o mueve al iniciar el bucle según sus propias capacidades y mediante la
transferencia confirmada de 8.10. Un guard terminal registrado sobre la colección
o cursor se retargetea al propietario interno sin duplicar ownership.

Un header que contiene bindings `ref` utiliza en cambio el modo observacional de
13.3: presta una colección con almacenamiento estable y nunca consume su
colección ni sus elementos.

Los cursores fallibles implementan `Iterator[T ! E]`. El consumidor decide si
propaga cada error:

~~~tondo
for item in stream {
    let value = item?
    consume(value)
}
~~~

### 10.18 Colecciones heterogéneas

La heterogeneidad requiere una unión explícita:

~~~tondo
let values: Array[Int | String] = [1, "two", 3]

let settings: Map[String, Int | String | Bool] = [
    "port": 8080,
    "host": "localhost",
    "debug": true,
]
~~~

No existe promoción a `Any`.

### 10.19 Garantías de complejidad

Salvo copy-on-write o asignación necesaria:

| Operación | Garantía |
|---|---|
| Índice de `Array` | O(1) |
| Slice de `Array` | O(1) |
| Iteración de `Array` | O(n) |
| Aritmética de `Array` | O(n) sobre el resultado |
| Lookup/insert/delete de `Map` | O(1) esperado amortizado |
| Lookup/insert/delete de `Set` | O(1) esperado amortizado |
| Iteración de `Map` o `Set` | O(n) en orden de inserción |
| Construcción e iteración de `Range` | O(1) y O(n), respectivamente |

Una mutación puede copiar O(n) si el almacenamiento está compartido. Una llamada `mut` sobre almacenamiento exclusivo evita esa copia cuando no se cambia extensión; una llamada `var` permite además redimensionar o reemplazar el almacenamiento.

Las garantías de `String` se especifican por separado porque indexar UTF-8 por valor escalar puede ser O(n).

---

## 11. Funciones, métodos y cierres

### 11.1 Declaraciones de función

Las formas base de firma son:

~~~tondo
fn log(message: String)
fn add(a: Int, b: Int): Int
fn save(config: Config): !IoError
fn load(path: Path): Config ! IoError
fn makeCounter(): impl CallMut[fn(): Int] + Discard
~~~

Equivalencias:

~~~text
sin retorno ni error    Unit
con retorno             T
sin retorno, con error  !E, equivalente a Unit ! E
con retorno y error     T ! E
~~~

Los parámetros siempre declaran tipo. Una función que devuelve algo distinto de `Unit` siempre declara su tipo de éxito.

`impl Bound` declara un éxito concreto pero opaco según 12.8; puede combinarse
con un error exterior como `impl Iterator[Row] + Discard ! IoError`.

`->` no es un token de Tondo. `:` introduce el resultado completo de una función. Dentro de ese tipo, `!` separa el éxito y el error. Una función infallible que devuelve `Unit` omite por completo `: Unit`.

### 11.2 Cuerpo y retorno implícito

La última expresión del cuerpo es su resultado:

~~~tondo
fn add(a: Int, b: Int): Int {
    a + b
}
~~~

`return` permite salida temprana:

~~~tondo
fn absolute(value: Int): Int {
    if value < 0 {
        return -value
    }

    value
}
~~~

En una función fallible, `return value` representa éxito. `fail error` representa error.

Si la última expresión o un `return` ya tiene exactamente el tipo completo declarado —por ejemplo `T ! E` dentro de una función `T ! E`— se devuelve sin envolverlo otra vez. La elevación automática a `ok(...)` solo se aplica a una expresión del tipo de éxito `T`.

### 11.3 Parámetros y argumentos

Llamada posicional:

~~~tondo
connect("localhost", 8080)
~~~

Argumentos nombrados:

~~~tondo
connect(host: "localhost", port: 8080)
~~~

Reglas:

- Los argumentos posicionales deben preceder a los nombrados.
- Después del primer argumento nombrado, todos los siguientes son nombrados.
- Un argumento nombrado debe coincidir exactamente con el parámetro.
- Cada parámetro fijo se proporciona exactamente una vez; el variádico recibe cero o más elementos.
- La evaluación conserva orden textual, no orden de declaración.
- Los nombres de parámetros públicos, salvo el descarte `_`, forman parte del contrato fuente de la API.

Un parámetro fijo escrito `_` descarta su argumento, no crea binding y solo puede
proporcionarse por posición; `_:` nunca es una etiqueta de llamada. En la forma
por valor, su tipo debe cumplir `Discard`. En `_: ref T`, `_: mut T` o
`_: var T`, conserva el contrato de préstamo pero no adquiere ownership ni exige
`T: Discard`; estas formas permiten implementar conscientemente una firma cuyo
parámetro no necesita el body concreto. Un parámetro variádico sí necesita un
nombre ordinario porque ese nombre identifica tanto el pack como su spread
nombrado.

Tondo 0.1 no tiene parámetros por defecto, sobrecarga de funciones por firma ni conversión implícita de tupla a argumentos. Alternativas configurables se expresan mediante un record de opciones.

Un parámetro `ref` observa temporalmente un valor sin copiarlo ni moverlo. El modo
aparece en firma y llamada:

~~~tondo
fn inspect(resource: ref Resource) {
    console.print(resource.status())
}

inspect(ref resource)
~~~

Su semántica completa y sus reglas async se definen en 16.3.

Un único parámetro final puede ser variádico y homogéneo:

~~~tondo
fn log(prefix: String, parts: ...String) {
    for part in parts {
        console.print("{prefix}{part}")
    }
}

log("Info: ", "server", " started")
~~~

Reglas:

- `...T` solo puede aparecer en el último parámetro y como máximo una vez.
- El parámetro tiene semánticamente tipo `Array[T]` y es inmutable dentro de la función.
- Una llamada sin argumentos para esa posición produce un array vacío.
- Los argumentos individuales se evalúan de izquierda a derecha.
- Los variádicos son homogéneos; no se infiere una unión ni se crea un pack heterogéneo.
- Un parámetro variádico no puede ser `ref`, `mut` ni `var`.
- Cada elemento sigue las reglas ordinarias de paso por valor: se copia si cumple `Copy` y se mueve en otro caso.

Un array existente se expande con `...` en la última posición:

~~~tondo
let parts = ["server", " started"]
log("Info: ", ...parts)
~~~

Cuando los parámetros fijos se proporcionan por nombre, el variádico puede proporcionarse mediante un único spread nombrado:

~~~tondo
log(prefix: "Info: ", parts: ...parts)
~~~

El nombre debe coincidir exactamente con el parámetro variádico. No se permiten
elementos variádicos individuales después de un argumento nombrado; se agrupan en
un array y se utiliza la forma anterior. El spread no es una conversión de tupla
y no puede combinarse con `ref`, `mut` ni `var`. En 0.1 solo se permite un spread
y debe ser el último argumento de la llamada.

Expandir un `Array[T]` lo copia lógicamente cuando `T: Copy`; en otro caso consume el array completo y transfiere sus elementos al pack. No existe una expansión que deje disponible un array de elementos afines.

La representación no forma parte de la semántica. El compilador puede pasar una vista contigua sobre argumentos temporales o sobre un `Array[T]` existente. Si el valor escapa, se devuelve o una copia mutable necesita redimensionarse, debe materializar almacenamiento propio sin exponer referencias al stack del llamador:

~~~tondo
fn prepare(parts: ...String): Array[String] {
    var result = parts
    result.append("done")
    result
}
~~~

### 11.4 Recursión

Una función puede llamarse a sí misma y las funciones de un módulo pueden ser mutuamente recursivas. Todas las firmas se conocen antes de comprobar cuerpos; el orden textual de funciones no afecta resolución.

El compilador no garantiza tail-call optimization. Un algoritmo que requiera memoria acotada debe expresarse iterativamente.

### 11.5 Métodos inherentes

Sintaxis:

~~~tondo
pub fn Point.distanceTo(self, other: Point): Float {
    let dx = self.x - other.x
    let dy = self.y - other.y
    math.sqrt(dx * dx + dy * dy)
}
~~~

Un método inherente sobre un tipo genérico declara los parámetros del propietario en el propio path. En esa posición los nombres son binders:

~~~tondo
fn Pair[A: Copy, B: Copy].swapped(self): Pair[B, A] {
    Pair[B, A] {
        first: self.second
        second: self.first
    }
}
~~~

Un método puede añadir después parámetros genéricos propios:

~~~tondo
fn Box[T: Copy].map[U, F: Call[fn(T): U]](
    self,
    transform: F,
): Box[U] {
    Box[U] { value: transform(self.value) }
}
~~~

La lista del propietario debe tener la misma aridad que la declaración nominal y puede añadir constraints para limitar la disponibilidad del método. Solo puede existir una declaración con el mismo nombre para un propietario; Tondo no selecciona entre métodos según constraints.

Ninguna operación inherente puede compartir nombre con una variante de su enum propietario. Esto evita que `Type.Member(...)` pueda significar tanto construcción de variante como llamada asociada.

Un método con receptor no puede compartir nombre con un campo de su tipo propietario. Así `value.name` nunca representa un bound method. Si `name` es un campo de tipo función, `value.name()` llama a ese valor; si no existe tal campo, la misma forma puede resolver un método. La prohibición de colisión impide que ambas interpretaciones sean candidatas a la vez.

Llamada:

~~~tondo
let distance = first.distanceTo(second)
~~~

La notación con punto de un método inherente es azúcar estático para:

~~~tondo
Point.distanceTo(first, second)
~~~

El receptor `self` es un préstamo inmutable limitado a la llamada; no copia ni
mueve el propietario. `mut self` es un préstamo mutable exclusivo que conserva la
extensión estructural del lvalue. `var self` permite además cambiar esa extensión
o reemplazar el receptor sin conservarla. Un método que deba consumir un valor
afín se declara como operación asociada con un parámetro ordinario por valor y se
llama de forma calificada:

~~~tondo
fn Resource.release(resource: Resource) {}

Resource.release(resource)
~~~

Una declaración `fn Type.operation(...)` que no contiene `self`, `mut self` ni `var self` es siempre asociada y nunca participa en dot-call. Un primer parámetro ordinario del mismo tipo no se convierte en receptor implícitamente.

El receptor `self` tiene la misma semántica compartida que un parámetro `ref`,
pero omite el modificador porque la posición de receptor ya declara el préstamo.
`mut self` y `var self` conservan sus permisos exclusivos. Una operación libre o
un segundo valor observado utiliza `ref T`; una función ordinaria sin modificador
continúa recibiendo ownership por valor. `ref self` es redundante y no es sintaxis
válida.

No hay dispatch dinámico oculto. La resolución de `value.method(...)` considera:

1. Métodos inherentes del tipo.
2. Métodos de traits incluidos en los constraints visibles.

Una implementación de trait sobre un tipo concreto no añade sus métodos a una búsqueda abierta por punto. Fuera de un constraint, se utiliza la forma calificada `Trait.method(value, ...)` o `module.Trait.method(value, ...)`. Así, importar un módulo nuevo no puede volver ambigua una llamada existente. Una colisión entre constraints exige también la forma calificada.

Una operación de trait sin receptor hace explícito el tipo implementador como primer argumento genérico de la llamada: `Decode.decode[User](bytes)`. Si el trait es genérico, sus argumentos permanecen junto a su nombre, como en `Codec[Json].decode[User](bytes)`. Los argumentos genéricos declarados por el propio método aparecen después de `Self`. El implementador nunca se infiere solo desde el resultado esperado.

### 11.6 Receptores `mut self` y `var self`

~~~tondo
fn Counter.increment(mut self) {
    self.value += 1
}
~~~

Solo puede llamarse sobre un lvalue mutable:

~~~tondo
var counter = Counter { value: 0 }
counter.increment()
~~~

La llamada crea un préstamo mutable exclusivo durante su ejecución. El método
puede cambiar campos, elementos y valores escalares. También puede asignar a
`self` cuando el contrato del tipo demuestre que el reemplazo conserva la misma
extensión estructural: esto es siempre cierto para escalares, records, tuples y
enums, y exige conservar longitud, región, claves u otra estructura declarada en
colecciones y tipos opacos. Un `let` no puede ser receptor de un método
`mut self`.

Un receptor `var self` permite además cambiar la extensión o reemplazar el valor
sin conservarla:

~~~tondo
fn Array[T].append(var self, value: T) {
    panic("operación intrínseca ilustrativa")
}

var values = [1, 2]
values.append(3)
~~~

`var self` exige un lvalue completo y reemplazable. Puede ser un binding, campo, tuple slot o elemento que almacene un `Array[T]` completo, pero no una región como `values[1:3]` ni un lookup de map potencialmente ausente.

En forma calificada, el préstamo vuelve a quedar explícito: `Counter.increment(mut counter)`, `Reset.reset(mut value)` o `Array.append(var values, item)`. En dot-call, `counter.increment()` y `values.append(item)` son el único azúcar que omite `mut` o `var`, porque el lvalue receptor aparece inmediatamente antes del método. El compilador conoce el contrato desde la firma; no infiere permiso de redimensionado desde el cuerpo.

### 11.7 Funciones como valores

Tipo de función:

~~~tondo
fn(Int): String
fn(Int, Int): Bool
fn(Path): Bytes ! IoError
fn(String, ...String)
fn(ref Resource): Status
fn(mut Array[Float], Float)
fn(var Array[String], String)
async unsafe fn(Pointer[Byte]): Bytes ! IoError
~~~

Las funciones nombradas pueden pasarse como valores si la firma coincide
exactamente. `ref`, `mut`, `var`, `async`, `unsafe`, el variádico, el éxito y el
error forman parte del tipo; los nombres de parámetros no. Un valor cuyo tipo es
literalmente `fn(...)` tiene representación uniforme y cumple siempre
`Copy + Discard + Send + Share`. Esto incluye funciones nombradas especializadas
y cierres que hayan realizado la coerción segura definida en 11.8.

Una función genérica no es un valor polimórfico de primera clase. Antes de
almacenarla o pasarla debe quedar completamente especializada, ya sea de forma
explícita —`identity[Int]`— o por una única solución obtenida del tipo de función
esperado. Si queda algún parámetro libre o existen varias soluciones, el
compilador exige los argumentos entre corchetes. La especialización no ejecuta la
función y produce un valor con una firma monomórfica ordinaria.

Invocar un valor de tipo `unsafe fn(...)` requiere una región `unsafe`, igual que
invocar una función nombrada con ese modificador. El permiso se decide por el tipo
estático del callee y no se pierde al almacenarlo o pasarlo como argumento.

Una llamada a través de un valor de función utiliza solo argumentos posicionales, porque su tipo no conserva etiquetas. Los argumentos nombrados solo se permiten cuando el callee es una función, método u operación asociada resuelta por nombre.

Tondo 0.1 no crea bound methods implícitos: `value.method` no es un valor de función y el acceso a un método debe ir seguido de su llamada. Una función libre o una operación asociada sin `self`, como `Resource.release`, sí puede utilizarse como valor nombrado cuando su firma coincide. Para fijar argumentos se escribe un cierre explícito y se aplican sus reglas de captura.

~~~tondo
fn apply[F: Call[fn(Int): Int]](
    value: Int,
    operation: F,
): Int {
    operation(value)
}
~~~

Una API que necesita conservar un callback de representación uniforme puede
seguir aceptando `fn(...)`. Una API que solo lo invoca debe preferir los bounds
`Call`, `CallMut` o `CallOnce`: así acepta funciones nombradas y cierres con
estado sin asignación ni borrado de capacidades obligatorios.

### 11.8 Cierres

Un cierre no utiliza `fn`. La lista de parámetros seguida de un bloque lo identifica:

~~~tondo
let double = (value: Int): Int {
    value * 2
}
~~~

Cada expresión de cierre crea un tipo concreto anónimo que contiene su entorno y
una firma de llamada conocida. Dos expresiones de cierre distintas tienen tipos
distintos aunque su firma coincida. El tipo no puede escribirse directamente,
pero puede inferirse en un binding local o pasar por un parámetro genérico con uno
de los bounds de llamada definidos más abajo. Una función que lo devuelve sin
coerción utiliza `impl Call... + Discard` según 12.8; el tipo sigue siendo
concreto y no se transforma en trait object.

Cuando existe un tipo de función esperado, las anotaciones inferibles pueden
omitirse y, si el entorno lo permite, el cierre se convierte a esa representación
uniforme:

~~~tondo
let operation: fn(Int): Int = (value) {
    value * 2
}
~~~

Sin firma esperada, cada parámetro declara su tipo. El retorno puede inferirse
desde el cuerpo o escribirse después de `:`. No se utiliza una flecha para
separar parámetros y retorno.

El cierre sin parámetros es `() { ... }`. Los paréntesis son siempre obligatorios, también para un único parámetro.

Un cierre puede anteponer `unsafe` cuando su llamador deba cumplir precondiciones
raw:

~~~tondo
let read: unsafe fn(Pointer[Byte]): Byte = unsafe (address) {
    address.read()
}
~~~

Su cuerpo es una región `unsafe` y llamarlo requiere la misma región explícita
que una función `unsafe` nombrada. `unsafe` forma parte de su firma de llamada.
Un cierre que además pueda suspenderse utiliza la única combinación canónica
`async unsafe (parameters) { ... }`. La región unsafe del llamador debe abarcar
el `await` o el `spawn` que inicia la operación. La captura de punteros conserva
además las reglas explícitas de 16.12.

Un cierre puede recibir préstamos `ref`, `mut` o `var`. Para mantener visible el
contrato, cualquiera de esos parámetros siempre declara su tipo aunque exista
tipo esperado:

~~~tondo
let scale: fn(mut Array[Float], Float) = (
    values: mut Array[Float],
    factor: Float,
) {
    values *= factor
}
~~~

Un cierre que pueda reemplazar o redimensionar utiliza `var` en el tipo, parámetro y llamada:

~~~tondo
let addLine: fn(var Array[String], String) = (
    lines: var Array[String],
    line: String,
) {
    lines.append(line)
}

addLine(var output, "done")
~~~

Un préstamo compartido conserva disponible el origen después de la llamada:

~~~tondo
let describe: fn(ref Resource): String = (
    resource: ref Resource,
) {
    resource.status().display()
}

let text = describe(ref resource)
~~~

Un cierre también puede implementar un tipo de función variádico. Sin tipo esperado, marca su último parámetro con la misma forma `...T`; con tipo esperado, puede omitir la anotación y hereda tanto el elemento como el carácter variádico:

~~~tondo
let countValues = (values: ...String): Int {
    values.length()
}

let countNames: fn(...String): Int = (names) {
    names.length()
}
~~~

Dentro del cierre, `values` y `names` tienen tipo `Array[String]`. Solo puede
existir un parámetro variádico, debe ser el último, necesita nombre y no puede ser
`ref`, `mut` ni `var`; el paso, spread y posible materialización siguen
exactamente las reglas de 11.3.

El conjunto de capturas se obtiene sintácticamente de los bindings locales
exteriores usados libremente por el cuerpo. Funciones, constantes, tipos y
módulos resueltos por nombre no forman parte del entorno. Cada captura ocurre por
valor y exactamente al crear el cierre:

- Si el valor es `Copy`, se captura un snapshot lógico y el binding exterior
  continúa disponible.
- Si no es `Copy`, se mueve al entorno y el binding exterior deja de estar
  disponible.
- La captura conserva si el binding era `let` o `var`. Escribir un `var`
  capturado modifica el snapshot privado del cierre, nunca el binding exterior.
- No existen listas de captura, captura implícita por referencia ni lifetime
  oculto.

Por ejemplo, este cierre posee y modifica su propio contador:

~~~tondo
var count = 0
var next = (): Int {
    count += 1
    count
}

assert(next() == 1)
assert(next() == 2)
assert(count == 0)
~~~

Un préstamo `ref`, `mut` o `var` nunca se captura. Cuando se desea observar o
modificar el valor exterior exacto, el préstamo sigue siendo visible como
parámetro y argumento:

~~~tondo
var count = 0

let increment = (value: mut Int) {
    value += 1
}

increment(mut count)
~~~

Un cierre también puede poseer estado afín. El movimiento ocurre al construirlo
y una llamada que extraiga esa captura consume el cierre:

~~~tondo fragment spec.resource
fn reserveCleanup(): !AcquireError {
    let resource = acquire()?
    let release = () {
        Resource.release(resource)
    }

    defer release()
}
~~~

#### Protocolos de llamada

Tres bounds intrínsecos cerrados describen cómo puede invocarse un callable
concreto. Su argumento es la firma completa, incluidos `ref`, `mut`, `var`,
variádico, éxito, error, `async` y `unsafe`:

~~~text
Call[fn(A): B]
CallMut[fn(A): B]
CallOnce[fn(A): B]
~~~

- `Call[S]` presta el callable de forma compartida y puede repetirse. El cuerpo
  no escribe ni mueve capturas.
- `CallMut[S]` lo presta en exclusiva y puede repetirse. El cuerpo puede modificar
  capturas, pero no moverlas fuera del entorno.
- `CallOnce[S]` recibe el callable por valor. El cuerpo puede mover capturas y la
  invocación consume un callable no `Copy`. Si su entorno conserva obligaciones
  terminales, el body debe satisfacerlas o transferirlas en todos los caminos.

`Call[S]` implica `CallMut[S]`. `Call[S] + Discard` y
`CallMut[S] + Discard` implican `CallOnce[S]`; sin `Discard`, consumir el entorno
podría abandonar una obligación y esa implicación no existe. El compilador puede
derivar `CallOnce` independientemente cuando el body consume o transfiere todas
las capturas terminales. Son protocolos cerrados: un módulo no puede declararlos
ni implementarlos manualmente. Toda función nombrada y todo valor uniforme
`fn(...)` implementan los tres para su firma exacta.

El compilador deriva los protocolos de cada cierre a partir de todos sus caminos
de control. Una captura escrita impide `Call`; una captura movida impide
`Call` y `CallMut`. `CallOnce` exige además que ninguna captura terminal quede
abandonada tras una salida normal, `return`, `fail` o `?`. En un cierre `async`,
escribir una captura también impide `CallMut`: la operación debe poseer su entorno
durante toda posible suspensión y no crea un préstamo exclusivo oculto a través
de `await`. La mutabilidad compartida async se expresa con un tipo de
sincronización explícito.

Una llamada ordinaria prueba, en orden, `Call`, `CallMut` y `CallOnce`, y elige el
primer protocolo que el tipo implemente **y** cuyo acceso permita el lugar del
callee. `CallMut` exige un lvalue `var` o un parámetro `mut`/`var`; si solo existe
un binding `let`, puede caer a `CallOnce` y consumirlo. `CallOnce` mueve el
callable, salvo que sea `Copy`. Si ningún par protocolo/acceso es válido, produce
`E1407`. En código genérico solo están disponibles los bounds escritos y sus
implicaciones cerradas; el compilador no inspecciona un tipo concreto futuro:

~~~tondo
fn invoke[F: CallOnce[fn(Int): String]](
    operation: F,
    value: Int,
): String {
    operation(value)
}
~~~

El protocolo no borra la firma: invocar `Call[unsafe fn(...)]` continúa exigiendo
una región `unsafe`, y una firma `async` continúa exigiendo `await` o `spawn`.
Los modos `ref`, `mut`, `var`, el variádico y el error se comprueban igual que en
una llamada directa.

Las capacidades del tipo anónimo se derivan de su entorno igual que en un record:

- Es `Copy` cuando todas sus capturas son `Copy`.
- Es `Discard` cuando todas sus capturas son `Discard`.
- Conserva obligación terminal si alguna captura la conserva.
- Es `Send` cuando todas sus capturas son `Send`.
- Es `Share` cuando todas sus capturas son `Share`; esto no permite invocar un
  `CallMut` a través de un préstamo compartido.

Una coerción contextual desde el cierre concreto a `fn(...)` solo existe cuando
el cierre implementa `Call` para esa firma y su entorno cumple
`Copy + Send + Share`. La coerción puede elegir una representación indirecta,
pero conserva semántica de valor y no es observable. Si falta alguna condición,
el compilador emite `E1108`; la API debe preservar el tipo concreto mediante un
bound de llamada. No existe coerción inversa ni borrado implícito a otro tipo.

~~~tondo compile-fail E1108
var count = 0
let increment = (): Int {
    count += 1
    count
}

let erased: fn(): Int = increment // requiere Call, pero el cierre es CallMut
~~~

### 11.9 Descarte de resultados

Una expresión de tipo no `Unit` no puede descartarse silenciosamente como sentencia. Debe asignarse, devolverse, consumirse o descartarse explícitamente:

~~~tondo
_ = computeValue()
~~~

Ignorar un `Result` o `Option` sin `_ =` es error. El descarte explícito comunica
intención. Tondo 0.1 no tiene una marca semántica adicional tipo `must_use`; una
futura librería o perfil de lint puede reconocer APIs concretas sin cambiar la
validez del descarte.

`_ = value` exige `value: Discard` y no satisface una obligación terminal.
Descartar de ese modo un `Join`, un handle en propiedad todavía activo o
cualquier compuesto que los contenga es error; debe ejecutarse una operación
terminal concreta.

### 11.10 Funciones asíncronas

`async fn` declara que una función puede suspenderse:

~~~tondo
async fn fetchUser(id: UserId): User ! NetworkError {
    let response = await http.get(userUrl(id))?
    decodeUser(response.body)?
}
~~~

El resultado escrito es el resultado lógico, no un wrapper de ejecución. La firma anterior produce `User ! NetworkError`, no `Task[Result[User, NetworkError]]`. `async` forma parte del tipo de función y del contrato público.

Una llamada asíncrona no puede escribirse como una llamada ordinaria descartable. Debe aparecer como operando de `await` para ejecución secuencial o de `spawn` para ejecución concurrente. Esto impide crear trabajo que nunca se espera accidentalmente.

Una función asíncrona puede ser infallible, fallible y devolver cualquier tipo ordinario:

~~~tondo
async fn flush(): !IoError
async fn read(path: Path): Bytes ! IoError
async fn sleep(duration: Duration)
~~~

Una función, método o cierre `async` no puede declarar parámetros ni receptores
`mut` o `var` en 0.1. Un préstamo exclusivo no puede permanecer activo durante
una suspensión; el código transforma y devuelve un valor, limita la mutación
antes de iniciar la operación async o utiliza un tipo de sincronización explícito.

`async unsafe fn` combina suspensión con precondiciones que debe demostrar el
llamador. El orden de modificadores es siempre `async unsafe fn`; ambos forman
parte del tipo. `unsafe` habilita las operaciones raw catalogadas y hace que su
cuerpo sea una región unsafe, pero no relaja por sí solo `Send`, `Share`,
ownership, cancelación ni la vida estructurada. Si una precondición debe
permanecer cierta a través de suspensiones, la documentación unsafe la expresa
para la duración lógica completa de la llamada. Una frontera que necesite
transportar estado raw no `Send` lo convierte primero en un handle opaco auditado
con el contrato concurrente adecuado.

Sí puede declarar un parámetro `ref T`. En `await operation(ref value)`, el
préstamo compartido dura hasta completar la llamada y el propietario debe
permanecer vivo y sin movimiento ni préstamo exclusivo. `T` debe cumplir `Send`
porque el frame async puede migrar entre workers; no necesita `Share` mientras la
llamada sea secuencial y no exista otro observador concurrente. Lanzar esa llamada
mediante `spawn` añade las reglas estructuradas de 11.12 y exige además `Share`.

Un receptor `self` async equivale a `ref Self` para esta regla y exige
`Self: Send`. En un trait, la presencia de ese método forma parte visible del
contrato y toda implementación debe tener un target `Send`; un llamador genérico
puede obtener ese hecho desde el trait. `spawn` sigue exigiendo `Share` en el
punto de llamada y no convierte el trait completo en `Share`.

Todo valor que permanezca vivo a través de un punto `await` debe cumplir `Send`. El compilador utiliza liveness: un valor no `Send` puede emplearse y terminar antes de la primera suspensión, pero no formar parte del frame suspendido. En una función genérica, mantener un `T` a través de `await` exige el constraint `T: Send`. Esta regla permite que cualquier task suspendida migre entre workers sin introducir una variante “local” del modelo async.

Un cierre concreto se comprueba con las capacidades derivadas de su entorno, no
solo con su firma. Por ello mantenerlo vivo a través de `await` exige que sus
capturas cumplan `Send`, mientras un valor uniforme `fn(...)` ya lo cumple por
construcción.

Los `Join` pertenecientes al `scope` actual son una excepción intrínseca: el runtime los conserva en el estado estructurado del scope mientras la tarea propietaria espera otro hijo. Eso permite tener varios joins pendientes sin convertirlos en valores transferibles; la excepción no autoriza pasarlos a otra task o thread.

### 11.11 `await`

`await` marca cada punto donde la función actual puede suspenderse:

~~~tondo
let user = await fetchUser(userId)?
~~~

La forma canónica anterior equivale a aplicar `?` al resultado obtenido después de esperar. `await operation?` se interpreta como `(await operation)?`, no como `await (operation?)`.

`await`:

- Solo puede aparecer dentro de una función o cierre `async`, o en un script cuyo
  `main` implícito sea async; otro contexto produce `E1610`.
- Acepta una llamada async nombrada, a un valor `async fn(...)`, a un callable
  concreto con el protocolo correspondiente, o un `Join[T, E]` producido por
  `spawn`; cualquier otra forma produce `E1611`.
- Devuelve el valor lógico `T` o `T ! E` de la operación.
- Consume el `Join` cuando su operando es un handle; el handle queda no disponible aunque la espera produzca error o pánico.
- No crea concurrencia por sí mismo; dos awaits consecutivos son secuenciales.
- No propaga errores implícitamente; `?` continúa siendo visible y separado.
- No puede aparecer dentro de un bloque `defer` en 0.1. El cleanup asíncrono debe esperarse explícitamente antes de abandonar el scope.

### 11.12 Concurrencia estructurada con `scope` y `spawn`

`scope` delimita la vida máxima de las tareas que contiene. `spawn` inicia una llamada async de forma concurrente y devuelve un handle `Join[T, E]` ligado a ese scope:

~~~tondo
async fn loadPage(userId: UserId): Page ! LoadError {
    scope {
        let userJob = spawn fetchUser(userId)
        let postsJob = spawn fetchPosts(userId)

        Page {
            user: await userJob?
            posts: await postsJob?
        }
    }
}
~~~

Para una operación infallible, el handle se normaliza como `Join[T, Never]`. `Join` no envuelve el resultado de todas las llamadas async: solo existe cuando `spawn` solicita concurrencia y necesita representar una finalización pendiente.

La región propietaria de `Join` se rastrea por el compilador y no se escribe como parámetro de lifetime en código fuente.

`scope` es una expresión cuyo tipo es el tipo de su bloque. Solo puede aparecer en
un contexto async (`E1610`) porque abandonar el scope puede requerir esperar
cleanup; esa salida cuenta como punto de suspensión para la regla de `Send`. Los
argumentos de cada llamada lanzada se evalúan por completo, de izquierda a
derecha, en la tarea propietaria antes de que el hijo pueda comenzar a
observarlos.

Reglas estructurales:

- `spawn` solo es válido dentro de `scope` (`E1602`) y solo acepta una llamada de
  firma async, ya sea nombrada, uniforme o demostrada por `CallOnce`; otro
  operando produce `E1611`.
- Los tipos de éxito y error de la llamada lanzada deben cumplir `Send`, porque el hijo transfiere su resultado a la tarea propietaria.
- Cada `Join[T, E]` pertenece al scope más interno, es afín y no puede copiarse, devolverse, almacenarse fuera de él ni sobrevivirlo.
- `Join[T, E]` no cumple `Send` ni `Share`: solo la tarea propietaria del `scope` puede esperarlo o cancelarlo.
- Un join se consume exactamente una vez mediante `await` o mediante una operación explícita de cancelación de la futura librería.
- `_ = join` no cuenta como consumo y es error; una tarea no puede abandonarse mediante descarte.
- Alcanzar normalmente el final con un join sin consumir es error de compilación.
- Si un `return`, `fail`, `?`, pánico o cancelación abandona el scope, cada join no consumido entra en teardown estructural: se solicita cancelación si su hijo continúa activo, se espera su cleanup y se consume el resultado si ya había terminado. Este es el propietario estructurado autorizado por 8.10; no convierte las salidas recuperables en cleanup implícito para otros bindings terminales del usuario.
- En esa salida no local, el teardown de hijos es la primera fase de abandonar el
  bloque `scope`: termina todos los hijos y libera sus préstamos estructurados
  antes de ejecutar los `defer` registrados directamente en ese bloque. Los
  defers de scopes léxicos más internos ya atravesados conservan su LIFO normal y
  el borrow checker impide que consuman o muten una región todavía prestada al
  hijo. Esta prioridad evita cerrar un recurso mientras una task aún lo observa.
- Un éxito o error no observado durante ese teardown no sustituye la salida que lo inició. Un valor con obligación terminal ejecuta su acción de unwind; los demás valores se descartan como parte explícita de la política del scope. Un error recuperable se conserva como contexto diagnóstico suprimido. Un pánico de hijo continúa siguiendo la prioridad de 11.14.
- No existe `spawn` separado o detached implícito. Una API de proceso o servicio de larga vida debe poseer explícitamente el scope que mantiene vivo su trabajo.

Todo argumento enviado en propiedad al hijo debe cumplir `Send`. Los argumentos
`Copy` se copian lógicamente y los argumentos afines se mueven al hijo, por lo que
estos últimos no pueden entregarse después a otro hijo. Un argumento explícito
`ref` no se envía en propiedad: crea el préstamo compartido estructurado descrito
abajo. El callee también se transfiere al hijo y se invoca mediante
`CallOnce[async fn(...)]`: una función nombrada se copia y un cierre concreto se
copia o mueve según su entorno. Tanto el callee como cualquier argumento propio
deben cumplir `Send`; `Share` solo es necesario para préstamos compartidos.

Los tipos `T` y `E` de un `Join[T, E]` pueden contener obligaciones terminales: el handle las posee mientras el resultado no se haya transferido mediante `await`. Esto no limita qué trabajo puede lanzarse. La acción de unwind de `Join` conoce su estado y, exactamente una vez, cancela o consume el resultado aplicando recursivamente las acciones necesarias.

A diferencia de la acción no suspendible de un handle ordinario, este teardown
forma parte del runtime async del `scope` y puede suspender a su propietario hasta
que todos los hijos hayan terminado su cleanup. Esa suspensión ya está incluida en
el contrato de salida de `scope`; no introduce `await` oculto en una función
síncrona ni permite acciones terminales async arbitrarias.

El receptor `self` de un método async y los argumentos `ref` conservan su
semántica de préstamo. Con `await` directo, cada préstamo dura hasta que termina
la llamada. Con `spawn receiver.operation(ref other)`, pueden cruzar al hijo como
préstamos compartidos estructurados solo si sus tipos cumplen `Send + Share`.
Cada origen debe ser un lvalue estable que viva hasta consumir el `Join`; durante
ese intervalo no puede moverse ni prestarse con `mut` o `var`, aunque pueden
coexistir otros préstamos `ref`. Un temporal puede prestarse a un `await` directo,
pero no a `spawn`, porque carecería de propietario exterior durante la vida del
hijo.

Ningún hijo recibe un binding exterior por referencia implícita ni un préstamo
`mut` o `var`. Un cierre puede llevar al hijo su entorno por valor conforme a
11.8; sus capturas `Copy` se copian y las afines se mueven al construir o
transferir el cierre. Todo préstamo compartido exterior aparece como receptor
`self` o argumento `ref` en la llamada lanzada. El estado compartido modificable
se expresa mediante tipos de sincronización definidos sobre las capacidades de la
sección 16.

El orden en que el scheduler avanza hijos listos no está especificado. Sí están especificados el orden de evaluación que crea cada hijo, las relaciones establecidas por `await`, canales y locks, y la ausencia de data races en código seguro.

### 11.13 Funciones y cierres async como valores

El tipo de una función asíncrona conserva `async`:

~~~tondo
async fn(UserId): User ! NetworkError
~~~

Un cierre asíncrono antepone `async` a su lista de parámetros:

~~~tondo
let fetch: async fn(UserId): User ! NetworkError = async (id) {
    await fetchUser(id)?
}
~~~

La anotación anterior realiza la coerción uniforme de 11.8 porque el cierre no
captura estado. Sin tipo esperado, conserva un tipo anónimo y puede implementar
`Call`, `CallMut` o `CallOnce` para la firma async correspondiente. `await`
directo utiliza su modo derivado; `spawn` siempre transfiere el callee y utiliza
`CallOnce`, de modo que nunca mantiene un préstamo exclusivo oculto en la tarea
propietaria.

`fn(A): B`, `unsafe fn(A): B`, `async fn(A): B` y
`async unsafe fn(A): B` son cuatro tipos distintos. No existen conversiones
implícitas que añadan u oculten suspensión o responsabilidad unsafe.

### 11.14 Cancelación y pánicos en tareas

La cancelación estructurada es una señal de control del runtime, no una variante añadida automáticamente a cada `E`. Es cooperativa y solo se observa en puntos de cancelación definidos:

- Antes de suspender o al reanudarse en `await`.
- Durante operaciones async de canales, timers, streams y procesos que la librería declare cancelables.
- Al entrar o abandonar un `scope` que ya recibió cancelación.
- En una comprobación explícita de la futura API de cancelación.

Un bucle de CPU sin esos puntos no se cancela por preempción. Debe cooperar mediante una comprobación o yield async explícito. Al observar cancelación:

- Se impide iniciar nuevo trabajo hijo.
- Se desenrollan scopes según el orden único de 8.10: primero termina el registro
  estructural de hijos de cada `scope` y después se drena su cleanup léxico en
  LIFO.
- Se ejecutan tanto los `defer` síncronos registrados como el fallback de cada
  token terminal vivo que no tenga guard, exactamente una vez.
- No se convierte la cancelación en un éxito ni se descartan errores ya observados.
- El error o pánico que originó la salida conserva prioridad sobre cancelaciones derivadas.

Una API que permita solicitar o observar cancelación explícitamente puede devolver un tipo nominal `Cancelled`; esa decisión pertenece a la librería y queda visible en su firma.

Un pánico dentro de un hijo marca inmediatamente el scope como fallido, solicita cancelar a sus hermanos y se propaga como pánico al propietario en su siguiente punto de cancelación o al abandonar el scope. Nunca interrumpe una instrucción síncrona a mitad ni se transforma en `! E`. Si el propietario ejecuta un bucle de CPU sin puntos de cancelación, la observación se retrasa igual que cualquier otra cancelación cooperativa.

Antes de propagarlo se espera el cleanup estructurado de todos los hijos. Si varios hijos entran en pánico antes de terminar el teardown, el hijo creado primero por orden de evaluación proporciona el pánico principal y los demás se adjuntan como diagnósticos suprimidos. Un pánico ya producido por el propietario conserva prioridad. Si el propietario solo tenía pendiente un éxito, `return`, `fail` o cancelación, el primer pánico de hijo prevalece y el resultado pendiente se conserva únicamente como contexto suprimido. Los errores recuperables no observados nunca se convierten en pánicos ni amplían implícitamente `E`; siguen la política de teardown de 11.12.

### 11.15 Efectos visibles

Tondo 0.1 no tiene un sistema general de efectos, pero las firmas reflejan:

- Errores recuperables mediante `! E`.
- Observación prestada mediante `ref`.
- Mutación de extensión fija mediante `mut` y cambio estructural mediante `var`.
- Suspensión mediante `async`.
- Precondiciones raw del llamador mediante `unsafe`.

I/O, reloj y aleatoriedad se distinguen mediante tipos y módulos de librería. La concurrencia se inicia únicamente con `spawn`, y todo acceso raw requiere una región `unsafe`.

---

## 12. Genéricos y traits

### 12.1 Genéricos

~~~tondo
fn first[T: Copy](values: Array[T]): T? {
    values.get(0)
}
~~~

Múltiples parámetros:

~~~tondo
type Pair[A, B] = {
    first: A
    second: B
}
~~~

Los argumentos genéricos utilizan corchetes:

~~~tondo
Pair[String, Int]
parse[User](text)
~~~

El compilador infiere argumentos genéricos cuando existe una única solución:

~~~tondo
let user = parse[User](text)?
let firstNumber = first(numbers)
~~~

Si no puede inferirlos, exige anotación; no elige un valor por defecto.

### 12.2 Invariancia

Todos los parámetros genéricos son invariantes. Aunque `Cat` pudiera convertirse explícitamente a una unión `Cat | Dog`, `Array[Cat]` no es subtipo de `Array[Cat | Dog]`.

La transformación debe ser explícita para conservar semántica de mutación y representación.

### 12.3 Declaración de trait

~~~tondo
pub trait Summary {
    fn summarize(self): String
}
~~~

Un trait declara comportamiento, no estado. Puede contener métodos requeridos y métodos con implementación por defecto:

~~~tondo
trait Empty {
    fn length(self): Int

    fn isEmpty(self): Bool {
        self.length() == 0
    }
}
~~~

Dentro de un trait, `Self` es un tipo contextual que representa el tipo implementador:

~~~tondo
trait Compare {
    fn compare(self, other: ref Self): Order
}
~~~

`Self` también puede aparecer en resultados, errores, parámetros genéricos y métodos sin receptor. Dentro de una implementación equivale exactamente al tipo objetivo. No es una keyword léxica, sino un nombre de tipo contextual reservado: no puede declararse en ningún namespace de usuario y utilizarlo fuera de un trait, su implementación o un método inherente es error.

Un trait no contiene campos, constructores ni inicialización. Puede declarar una operación asociada sin `self`; se invoca mediante `Trait.operation[Implementer](...)` y permite contratos como decoding o fábricas estáticas sin reflection. Especificar el implementador elimina la ambigüedad incluso cuando el método no recibe ni devuelve `Self`.

Un trait que declara un método `async` con receptor `self` tiene el requisito
intrínseco `Self: Send` de 11.10. Toda implementación debe satisfacerlo y, para
comprobación genérica, `T: Trait` permite deducir también `T: Send`. No es
herencia general entre traits: es una condición cerrada de formación de ese
contrato async.

~~~tondo
trait Decode {
    fn decode(bytes: Bytes): Self ! DecodeError
}

fn decodeUser(bytes: Bytes): User ! DecodeError {
    Decode.decode[User](bytes)?
}
~~~

### 12.4 Implementación explícita

~~~tondo
impl Summary for User {
    fn summarize(self): String {
        "{self.name} ({self.id.value})"
    }
}
~~~

No existe satisfacción implícita por coincidencia de métodos. Una implementación debe nombrar el trait y el tipo.

Una implementación genérica declara sus binders inmediatamente después de `impl`:

~~~tondo
impl[T: Display] Display for Box[T] {
    fn display(self): String {
        "Box({self.value})"
    }
}
~~~

Todo parámetro utilizado por el target o el trait debe estar declarado por la
cabecera del `impl`; no existen variables de tipo implícitas. Recíprocamente, cada
binder declarado después de `impl` debe aparecer al menos una vez en el trait
completo o en el target normalizado. Aparecer solo en un bound no lo determina
durante selección y produce `E1114`.

El cuerpo de un `impl` completa exactamente el contrato del trait:

- Debe definir una vez cada método requerido que no tenga implementación por
  defecto.
- Puede omitir un método con implementación por defecto o reemplazarlo una vez.
- No puede declarar métodos adicionales; una operación propia del tipo es un
  método inherente separado.
- Tras sustituir `Self` y los parámetros del trait, cada implementación debe
  coincidir con la firma declarada: nombre, aridad, posiciones de parámetros,
  receptor, préstamos `ref`/`mut`/`var`, posición variádica, parámetros genéricos
  y sus bounds, modificadores `async`/`unsafe` en su orden canónico, éxito y
  error. Los nombres locales de parámetros y binders genéricos pueden diferir o
  usar `_`; las etiquetas públicas de llamada siguen siendo siempre las
  declaradas por el trait.
- Una implementación por defecto se comprueba una sola vez bajo los constraints
  del trait. Dentro de ella, las llamadas a otros métodos del mismo trait se
  resuelven contra `Self`; no dependen de una implementación concreta conocida
  durante esa comprobación.

Estas reglas permiten defaults útiles sin convertir un `impl` en un segundo
namespace abierto ni hacer que su contrato dependa del orden de métodos.

### 12.5 Constraints

~~~tondo
fn renderAll[T: Discard + Summary](values: Array[T]): Array[String] {
    panic("cuerpo de ejemplo omitido")
}
~~~

Múltiples constraints:

~~~tondo
fn compareAndRender[T: Discard + Compare + Summary](left: T, right: T): String {
    panic("cuerpo de ejemplo omitido")
}
~~~

`+` dentro de una lista de constraints significa conjunción de traits o
capacidades intrínsecas; el contexto de tipo lo distingue del operador aritmético.
Después de resolver paths y expandir aliases, una lista es un conjunto no
ordenado: el orden no cambia la firma ni la satisfacción. Repetir el mismo
bound normalizado es `E1115` porque no aporta contrato. El formatter conserva el
orden escrito —no resuelve nombres—, mientras la comparación de APIs, métodos y
resultados opacos utiliza el conjunto normalizado.
`Copy`, `Discard`, `Equatable`, `Key`, `Send` y `Share` son capacidades
estructurales cerradas. `Call[S]`, `CallMut[S]` y `CallOnce[S]` son protocolos
cerrados de callable cuya `S` debe ser un tipo de función completo. Todos son
bounds válidos, pero ningún módulo puede implementarlos manualmente.
La resolución aplica las implicaciones cerradas de 11.8, incluidas
`Call[S] => CallMut[S]` y `(CallMut[S] + Discard) => CallOnce[S]`; no inventa
otras relaciones entre firmas.

`Iterator[T]` es distinto: su contrato está predefinido por el lenguaje, pero los
tipos de usuario sí pueden implementarlo. Se somete a coherencia, terminación y a
la unicidad funcional adicional de 10.17. También puede publicarse como bound de
un resultado opaco `impl Iterator[T] + Discard`, que conserva un cursor concreto.

### 12.6 Coherencia

Una implementación es válida si el módulo actual posee el trait o posee el target.
La propiedad se decide después de expandir aliases:

- El propietario de un trait es el módulo que lo declara.
- El propietario de un tipo nominal —record, enum, newtype o tipo opaco— es el
  módulo que lo declara.
- El propietario de un target es el propietario de su constructor nominal
  exterior. Así `Local[Foreign]` pertenece al módulo de `Local`, pero
  `Foreign[Local]` pertenece al módulo de `Foreign`.
- Aliases, tuples, funciones, options, results, uniones y colecciones intrínsecas
  no crean propiedad local. Un alias de `Foreign` sigue siendo extranjero y
  `Array[Local]` no pertenece al módulo de `Local`.
- Un target sin constructor nominal exterior solo puede implementarse en el
  módulo que posee el trait.

Por tanto, el módulo del trait puede definir su comportamiento para cualquier
target bien formado; un módulo distinto solo puede implementar traits extranjeros
para uno de sus tipos nominales exteriores. Esto impide que dos paquetes externos
creen implementaciones incompatibles para la misma pareja sin prohibir
implementaciones útiles sobre formas estructurales por parte del dueño del trait.

No se permiten:

- Implementaciones solapadas.
- Especialización.
- Implementaciones negativas.

Una cabecera solapada produce `E1111`.

La **cabecera de coherencia** de un `impl` es el par formado por el trait completo,
incluidos sus argumentos genéricos, y el target. Antes de compararlas se expanden
aliases y se normalizan uniones y shorthands. Dos implementaciones se solapan si
sus cabeceras pueden hacerse idénticas mediante una sustitución de parámetros de
tipo. Los bounds positivos no participan en esa decisión: son obligaciones que
debe satisfacer una implementación, nunca selectores para elegir entre dos. Por
tanto `impl[T: A] Trait for Box[T]` y `impl[T: B] Trait for Box[T]` se solapan
aunque el programa actual no contenga todavía un tipo que cumpla `A + B`.

Esta regla es deliberadamente estable frente a código futuro y hace que exista
como máximo una implementación candidata antes de resolver constraints. Dos
targets que normalizan al mismo tipo son la misma cabecera; dos instanciaciones
no unificables del trait, como `Codec[Json]` y `Codec[Xml]`, continúan siendo
pares distintos.

La resolución de traits de usuario es inductiva y debe terminar para cualquier
consulta finita. El compilador normaliza la consulta, selecciona la única cabecera
coherente, sustituye sus parámetros y resuelve después sus bounds. Una consulta
idéntica que reaparece en su propia cadena antes de quedar demostrada no se acepta
coinductivamente: es un ciclo sin prueba y el diagnóstico muestra la cadena
completa. Solo las capacidades intrínsecas cerradas utilizan los puntos fijos
estructurales definidos por el lenguaje.

La validez de un `impl` no depende de la potencia heurística del compilador.
Tondo utiliza la siguiente comprobación normativa de **terminación por cambio de
tamaño**:

1. Una consulta se representa mediante el nombre del trait y una tupla ordenada
   formada por sus argumentos genéricos seguida del target. Aliases, shorthands y
   uniones se normalizan antes de construirla.
2. Por cada `impl` genérico se crea una arista desde la consulta de su cabecera
   hacia cada bound de trait de usuario que deba demostrar. Un bound solo puede
   mencionar parámetros ligados por la cabecera.
3. Para cada arista se construye una matriz. Sus columnas son los componentes de
   la tupla origen y sus filas los de la tupla destino. Cada celda contiene:
   - `<` cuando el término destino es un subterm estructural estricto del término
     origen;
   - `=` cuando ambos términos son idénticos tras renombrar los parámetros ligados;
   - `?` en cualquier otro caso.
4. Sean `A[y, x]` la matriz de una arista `X -> Y` y `B[z, y]` la de
   `Y -> Z`. Su composición es
   `C[z, x] = strongest_y(compose(B[z, y], A[y, x]))`. `compose` devuelve `<`
   si un operando es `<` y el otro es `<` o `=`, devuelve `=` solo para
   `compose(=, =)`, y devuelve `?` en los demás casos. `strongest` elige `<`
   antes que `=` y `=` antes que `?`. Por tanto una ruta demuestra descenso si
   alguna etapa desciende y todas las demás conservan una relación conocida; una
   relación desconocida no aporta prueba.
5. Dentro de cada componente fuertemente conexo del grafo de nombres de trait se
   satura el conjunto finito de matrices bajo esa composición hasta no obtener
   ninguna nueva. Toda matriz idempotente que vuelva al mismo trait debe contener
   al menos un `<` en su diagonal. Si no lo contiene, el componente se rechaza.

“Subterm estructural” significa un nodo alcanzado descendiendo al menos una vez
por argumentos de un constructor de tipo, tuple, función o unión normalizada.
Por ejemplo, `T` es subterm estricto de `Array[T]`, mientras `Array[T]` no lo es
de `T`. La comparación se realiza sobre términos simbólicos y no utiliza tipos
concretos encontrados posteriormente en el programa.

Las aristas entre componentes distintos no necesitan descender porque no pueden
formar un ciclo. Esto permite adaptadores acíclicos como
`impl[T: Summary] Render for T`. Un ciclo que solo permuta parámetros, conserva
su tamaño o introduce constructores termina produciendo una matriz idempotente
sin descenso diagonal y se rechaza.

Los bounds de `Copy`, `Discard`, `Equatable`, `Key`, `Send`, `Share`, `Call`,
`CallMut` y `CallOnce` no crean aristas: se resuelven por sus reglas intrínsecas.
Todos los compiladores conformes construyen las mismas matrices y aceptan el mismo
conjunto de `impl`. El diagnóstico incluye la ruta cíclica y la matriz idempotente
que no demuestra descenso, y utiliza `E1112`.

### 12.7 Dispatch

Los traits de 0.1 se utilizan como constraints genéricos con dispatch estático. Un trait por sí solo no es un tipo de valor:

~~~tondo compile-fail E1110
trait Summary {
    fn summarize(self): String
}

fn consumeDynamic(value: Summary) {} // error: un trait no es un tipo de valor
~~~

Sobre un parámetro `T: Summary`, `value.summarize()` resuelve por el constraint. Sobre un valor concreto, la llamada explícita es `Summary.summarize(value)` o `module.Summary.summarize(value)`. Las implementaciones no participan en una búsqueda global de métodos por nombre.

Una colección heterogénea de comportamientos se expresa mediante:

- Un enum.
- Una unión cerrada.
- Un tipo de función.
- Un record que contenga callbacks explícitos.

Esto evita trait objects, vtables, lifetimes y casts dinámicos en el núcleo inicial.

La única forma en la que un bound aparece en una posición de resultado es el
retorno opaco estático `impl Bound` de 12.8. No convierte el trait en tipo de
valor: cada declaración sigue teniendo un tipo concreto único conocido por el
compilador.

### 12.8 Resultados opacos estáticos

Una función libre, inherente o asociada puede ocultar el nombre de su tipo
concreto de éxito y publicar únicamente los bounds que necesita el consumidor:

~~~tondo
fn makeCounter(): impl CallMut[fn(): Int] + Discard {
    var count = 0

    (): Int {
        count += 1
        count
    }
}

fn enumerate[T: Copy](
    values: Array[T],
): impl Iterator[(Int, T)] + Discard {
    IndexCursor[T] { values, index: 0 }
}
~~~

Cada posición `impl ...` de una declaración define un tipo nominal opaco único.
En una función genérica forma una familia con los mismos parámetros genéricos;
dos llamadas con la misma especialización producen el mismo tipo opaco y
especializaciones distintas conservan sus tipos parametrizados. No existe un
valor existencial ni vtable, y la forma no exige allocation ni dispatch dinámico.
La implementación todavía puede materializar o indireccionar el tipo concreto
por las libertades de representación de 8.13; esa decisión no es observable.

Reglas:

- Solo puede ocupar el éxito superior declarado después de `:` en una función
  libre, inherente o asociada, incluida una función `async`. Puede seguirle un
  canal visible `! E`. No aparece en parámetros, fields, aliases, funciones como
  valores, cierres, métodos de trait ni sus implementaciones.
- La lista contiene uno o más traits, capacidades o protocolos válidos para un
  generic bound. Sus implicaciones cerradas deben demostrar `Discard`; escribir
  `Discard` directamente es la forma habitual y `Copy` también basta porque lo
  implica. Así una firma opaca nunca esconde una obligación terminal ni el nombre
  de su operación de cleanup. Un cursor que posea un recurso devuelve en cambio
  un tipo nominal visible con su operación terminal.
- Todos los caminos normales de `return` y la expresión final deben producir
  exactamente un mismo tipo concreto después de aliases. Los caminos `Never` no
  participan, pero debe existir al menos un camino normal alcanzable que aporte
  el testigo. Una función que solo diverge o sale por error declara `Never` como
  éxito ordinario en vez de `impl Bound`. Dos expresiones de cierre distintas son
  tipos distintos y no se unifican por compartir firma; tampoco se inventa una
  unión o una coerción a `fn(...)`.
- El tipo concreto debe satisfacer todos los bounds publicados bajo los
  parámetros genéricos de la declaración. Un protocolo `Call`, `CallMut` o
  `CallOnce` solo puede proceder de la derivación cerrada de 11.8; no se abre a
  implementaciones manuales.
- Fuera del módulo declarador solo son utilizables los traits, protocolos y
  capacidades publicados. Métodos inherentes y representación del tipo oculto no
  participan en resolución. El valor puede inferirse localmente, pasarse a una
  función genérica o devolverse detrás de otro resultado opaco compatible.
- El módulo propietario puede cambiar la representación concreta sin cambiar la
  identidad opaca de la declaración, siempre que conserve bounds, semántica
  documentada y compatibilidad de versión. Tooling muestra un ID estable de la
  declaración, nunca el nombre privado ni los fields capturados.

Fuera de `decl_outcome_annotation`, `impl Bound` no forma una expresión de tipo y
produce `E0004`. Dentro de una declaración admitida, un conjunto que no demuestre
`Discard`, la ausencia de un testigo normal, retornos con tipos concretos
distintos o un bound no satisfecho producen `E1117`. Esta forma
resuelve factories de cierres con estado y adaptadores concretos sin añadir trait
objects ni borrar `Copy`, `Send`, `Share` o el modo de llamada declarado.

~~~tondo compile-fail E1117
fn hiddenCounter(): impl Call[fn(): Int] {
    (): Int {
        1
    }
}
~~~

Aunque el cierre concreto sea descartable, el contrato opaco debe publicar o
implicar `Discard`; el consumidor no depende de una propiedad escondida.

### 12.9 Límites de los traits

Tondo 0.1 no tiene:

- Tipos asociados.
- Constantes asociadas.
- Genéricos de valores o const generics.
- Higher-kinded types.
- Herencia entre traits.
- Trait aliases.
- Sobrecarga de operadores mediante traits.
- Conversión automática `From`/`Into`.

Estas ausencias mantienen local la resolución. Si una conversión es necesaria, se llama a una función o constructor explícito.

---

## 13. Expresiones y control de flujo

### 13.1 Bloques

Un bloque es una secuencia entre llaves:

~~~tondo
{
    let intermediate = compute()
    transform(intermediate)
}
~~~

Cada bloque crea scope. Su tipo es:

- `Never` si ningún camino alcanza normalmente el final porque todos transfieren control mediante `return`, `fail`, pánico u otra salida, o divergen.
- En otro caso, el tipo de la expresión final cuando existe; solo participan los caminos que alcanzan esa expresión.
- `Unit` cuando algún camino alcanza el final y el bloque termina en declaración, asignación u otra sentencia sin valor.

Una expresión final es tail por posición sintáctica, incluso si produce `Unit` o
no coincide con el resultado esperado. En este último caso se informa el mismatch;
el parser no la convierte en `expression_stmt`. Una expresión anterior debe
terminar como sentencia mediante `NL`, y si no devuelve `Unit` requiere `_ =`.

### 13.2 `if`

La condición debe ser exactamente `Bool`:

~~~tondo
if count > 0 {}
~~~

No son válidos como condición:

~~~tondo compile-fail E1102
let count = 1
let name = "Tondo"
let optional: Int? = some(1)

if count {}       // Int no es Bool
if name {}        // String no es Bool
if optional {}    // Option no es Bool
~~~

`if` es una expresión:

~~~tondo
let label = if active {
    "active"
} else {
    "inactive"
}
~~~

Cuando su valor se utiliza, `else` es obligatorio y todas las ramas deben producir tipos compatibles. Sin `else`, el tipo es `Unit`.

### 13.3 Un único bucle `for`

Tondo no tiene `while`.

Bucle condicional:

~~~tondo
for connection.isOpen() {
    connection.poll()
}
~~~

Iteración:

~~~tondo
for item in items {
    consume(item)
}
~~~

Iteración con patrón:

~~~tondo
for (key, value) in map {}
~~~

Iteración compartida sin copiar ni consumir elementos:

~~~tondo
for ref resource in resources {
    inspect(ref resource)
}

for (key, ref resource) in resourcesById {
    inspectNamed(key, ref resource)
}
~~~

Si el header contiene algún binding `ref`, el `for` entra en modo de
**observación**:

- La fuente debe ser un lvalue estable `Array`, `Map` o `Set`, o una reborrow
  compartida de un parámetro `ref`, `mut` o `var` de una de esas colecciones. Se
  obtiene una vez y permanece prestada durante todo el bucle.
- Cada binding `ref` dura solo la iteración actual. Los demás bindings deben
  corresponder a componentes `Copy`, `_` o forma sin payload; nunca se mueve una
  parte de la colección prestada.
- El origen permanece disponible después del bucle, pero no puede moverse,
  redimensionarse ni recibir un préstamo `mut` o `var` solapado mientras itera.
- `continue` termina los bindings prestados de la iteración actual antes de
  comenzar la siguiente. Cualquier salida del bucle —`break`, `return`, `fail`,
  `?`, pánico o cancelación— termina además el préstamo de la colección antes de
  continuar su cleanup.

`Range`, `String` y cualquier cursor concreto que implemente `Iterator[T]`
producen valores en lugar de ubicaciones de elemento estables y no aceptan
bindings `ref` en el header. Sus elementos ordinarios se copian o mueven según
las reglas existentes.

El patrón del header debe ser irrefutable para el tipo de elemento. Filtrar variantes o elementos se expresa mediante un `match` explícito dentro del bucle.

Un `in` situado al nivel superior del header selecciona siempre la forma iteradora. Cuando una condición de repetición sea una prueba de pertenencia, se agrupa para hacer visible la intención: `for (item in values) { ... }`.

Bucle infinito:

~~~tondo
for {
    processNext()
}
~~~

No existe el `for init; condition; increment` de C. La inicialización aparece antes y la actualización dentro:

~~~tondo
var index = 0

for index < values.length() {
    consume(values[index])
    index += 1
}
~~~

### 13.4 `break` y `continue`

`break` termina el bucle más cercano. `continue` inicia su siguiente iteración.

~~~tondo
for item in items {
    if shouldSkip(item) {
        continue
    }

    if shouldStop(item) {
        break
    }

    consume(item)
}
~~~

Tondo 0.1 no tiene:

- Labels de bucles.
- `break` con valor.
- `else` de bucle.

Los bucles condicionales y de iteración tienen tipo `Unit`. Un `for {}` tiene tipo `Never` cuando ningún `break` alcanzable termina ese mismo bucle; si existe tal salida, su tipo es `Unit`. El análisis no depende de demostrar que una condición ordinaria sea siempre verdadera.

`break` y `continue` no atraviesan una frontera de función o cierre. Un cierre declarado dentro de un bucle no puede controlar el bucle exterior.

### 13.5 `return`

`return` sin valor devuelve `Unit`:

~~~tondo
return
~~~

`return expression` devuelve éxito en funciones normales o fallibles:

~~~tondo
return user
~~~

Dentro de un cierre, `return` termina el cierre más cercano. No atraviesa una frontera de cierre para retornar desde una función exterior. No puede utilizarse fuera de una función, cierre o `main` implícito de script.

### 13.6 `fail`

`fail expression` solo aparece en funciones o cierres con `! E`, incluido el `main` implícito de un script:

~~~tondo
if invalid {
    fail ValidationError.InvalidInput
}
~~~

El tipo del error debe ser `E`, un miembro inyectable o una unión cuyo conjunto
normalizado sea subconjunto de la unión `E`, según 8.9. `fail` tiene tipo `Never`
en la ruta actual.

Como `return`, `fail` termina únicamente la función o cierre más cercano; nunca sale de un callback exterior.

### 13.7 `defer`

`defer` registra trabajo para el final del scope léxico actual:

~~~tondo
let resource = acquire()?
defer Resource.release(resource)

resource.flush()
~~~

También acepta bloque:

~~~tondo
defer {
    console.print("leaving scope")
}
~~~

Reglas:

- Los defers se ejecutan en orden LIFO.
- Se ejecutan al alcanzar el final, `return`, `fail`, `break` o `continue` que abandone el scope.
- En la salida no local de un bloque `scope`, su propietario estructurado completa
  primero el teardown de hijos y libera sus préstamos, según 11.12; después se
  ejecutan los defers registrados directamente en ese bloque en orden LIFO.
- Se ejecutan durante el desenrollado de todo pánico producido por el lenguaje.
- Se ejecutan durante la cancelación estructurada de una task antes de que su join finalice.
- En un defer de llamada, los receptores y argumentos `Copy` se evalúan y capturan al registrar el defer.
- Un bloque defer captura valores `Copy` al registrarse. No puede capturar un valor afín; su cleanup utiliza la forma de llamada.
- Si una llamada diferida consume un binding afín completo, el compilador crea un
  guard terminal asociado a su token de ownership, desarma el fallback intrínseco
  y registra el guard en la posición actual de la pila de cleanup de 8.10. No
  existen dos cleanups activos para el mismo token. El movimiento al callee se
  aplaza hasta salir del scope; mientras tanto el valor puede seguir utilizándose
  mediante métodos `self` y préstamos breves `ref`, `mut` o `var`, y el guard
  sigue un movimiento local hacia otro binding.
- El callee puede ser un cierre concreto `CallOnce`. Si el cierre es afín, su
  valor completo cuenta como el único operando afín de la llamada diferida; sus
  capturas permanecen dentro del entorno y no se cuentan por separado. Esto
  permite reservar `defer cleanup()` cuando `cleanup` posee el recurso que
  consumirá.
- Una llamada diferida puede reservar como máximo un operando afín propietario;
  sus demás receptores y argumentos deben ser `Copy`. Varios recursos se limpian
  con varios `defer` o se agrupan previamente en un único owner con una operación
  terminal propia. Así desactivar o retargetear un guard nunca deja media llamada
  diferida.
- El retarget local solo admite mover ese valor completo a otro binding del mismo
  tipo. Mientras el guard esté activo no puede desestructurarse ni incrustarse en
  otro agregado local, porque la llamada diferida ya no podría reconstruir su
  argumento. El movimiento intrínseco al propietario interno de `for` es la
  excepción definida en 10.17.
- Un handoff confirmado que transfiere el valor fuera del scope del guard —incluidos llamada consumidora, `return`, `spawn` o envío a un canal— desactiva ese guard porque el nuevo propietario adquiere la obligación y su propia entrada de fallback. Si la transferencia no llega a confirmarse, el guard permanece activo. Consumirlo antes mediante una operación terminal también desactiva el guard. No se ejecuta nunca dos veces.
- Un valor con guard activo no puede registrarse para un segundo consumo terminal. El compilador muestra tanto el primer registro como el intento conflictivo.
- Una expresión temporal afín pasada a defer queda en propiedad del defer desde el registro.
- No se difiere un préstamo `ref`, `mut` o `var` ni un receptor `self` afín que
  solo fuera prestado. El cleanup afín se expresa como una operación asociada que
  consume el binding completo; el compilador puede comprobar así un único
  propietario y un orden terminal inequívoco.
- La expresión diferida debe ser infallible y devolver `Unit`.
- Un defer es estrictamente síncrono: no puede contener `await`, `spawn`, una
  expresión `scope` ni una llamada async. No puede crear trabajo estructurado
  nuevo mientras otro scope está terminando.
- Un bloque defer no puede registrar otro `defer` ni ejecutar `return`, `fail` o un `break`/`continue` cuyo destino quede fuera del propio bloque. Sus bucles internos sí pueden controlarse normalmente.
- Un cleanup que pueda fallar debe manejar ese error dentro del bloque defer o realizarse explícitamente antes de salir.
- Todos los defers continúan ejecutándose aunque uno produzca pánico. Sin un pánico previo, el primero según orden LIFO es el principal; los posteriores se adjuntan como suprimidos. Si el scope ya se desenrollaba por otro pánico, ese pánico conserva prioridad. Un pánico de cleanup prevalece sobre un `return`, `fail` o cancelación que todavía no fuera pánico.
- OOM irrecuperable, stack overflow, corrupción del runtime, aborto del proceso y terminación externa no son pánicos Tondo y no garantizan ejecutar defers.

`defer` no es un destructor ni finalizador. Hace visible el punto de adquisición y el cleanup.

### 13.8 Asignación simple y múltiple

La asignación es una sentencia y devuelve `Unit`:

~~~tondo
value = expression
counter += 1
array[index] = item
map[key] = value
~~~

El lado izquierdo debe ser:

- Un binding `var`.
- Un campo alcanzable mediante un lvalue mutable.
- Un índice o slice mutable.
- Un parámetro o receptor `mut` cuando la escritura conserva su extensión.
- Un parámetro o receptor `var`, que también puede reemplazarse o redimensionarse.

Una forma que cumple la gramática de lvalue pero no concede escritura —por
ejemplo un binding `let`, un parámetro ordinario o `ref`, una inserción de map a
través de `mut`, o una proyección potencialmente ausente— produce
`E1411 invalid-assignment-target`. Este código describe el permiso o la clase de
lugar inválidos; una incompatibilidad entre el tipo del lugar y el valor derecho
continúa siendo `E1102`.

Asignar sobre un lugar disponible reemplaza su valor anterior. Si ese valor conserva una obligación terminal, la asignación es error salvo que la misma operación haya movido antes el valor anterior a un temporal que también quede consumido o reasignado. Asignar directamente al binding completo de un `var` movido lo repone; sus campos e índices siguen inaccesibles mientras esté movido.

Una asignación simple resuelve la ubicación izquierda exactamente una vez, evalúa después por completo el lado derecho y solo entonces escribe. Un error o pánico durante la evaluación derecha no modifica el destino. Los bounds, longitudes, conflictos de préstamo y demás precondiciones comprobables se validan antes de reemplazar el valor anterior.

Una asignación puede utilizar un patrón de tupla irrefutable:

~~~tondo
(left, right) = (right, left)
(name, age) = readPerson()
~~~

Los paréntesis son obligatorios. `left, right = right, left` no es sintaxis válida y la asignación encadenada `a = b = value` tampoco existe.

La asignación múltiple es asignación de una tupla, no un segundo mecanismo de retorno. El lado derecho produce un único valor tuple cuya aridad y tipos deben corresponder al patrón izquierdo.

Cada destino `_` consume su componente sin escribirlo y exige que ese componente
cumpla `Discard`; nunca elimina una obligación terminal.

La operación se realiza en fases:

1. Se resuelve cada ubicación del lado izquierdo una sola vez, de izquierda a derecha, sin escribir.
2. Se evalúa por completo el lado derecho y se conserva en temporales.
3. Se comprueba y desestructura la tupla.
4. Se escribe de izquierda a derecha.

Los temporales del lado derecho conservan ownership. Esto permite permutar valores afines sin abandonarlos, por ejemplo `(left, right) = (right, left)`: ambos valores se mueven primero a temporales y después reponen sus bindings. Cada obligación terminal debe seguir teniendo exactamente un propietario al terminar la sentencia.

Una ubicación `mut` o `var` con contenido no `Copy` solo puede moverse en esta
sentencia si recibe simultáneamente otro valor válido. La transferencia no se
confirma hasta que todos los destinos prestados pueden quedar repuestos; por ello
un fallo durante resolución o evaluación conserva sus contenidos anteriores. No
se permite dejar una ubicación prestada movida para reponerla en una sentencia
posterior.

Resolver una ubicación evalúa su receptor, claves e índices, pero no congela una
dirección física que pueda quedar inválida por copy-on-write. Si evaluar el lado
derecho requiriese reasignar o prestar de forma mutable un origen del que depende
una ubicación ya resuelta, el programa se rechaza por conflicto de préstamo. Un
movimiento desde esa ubicación solo admite la permutación confirmada descrita a
continuación. La ubicación de un binding local completo no depende de su valor
actual, por lo que esta restricción no impide intercambiarlo; la ubicación de uno
de sus campos o índices sí depende del agregado base.

Resolver no activa todavía el préstamo de escritura. El lado derecho puede leer
valores `Copy` de esos mismos lugares y actúa como snapshot anterior a toda
escritura. Dentro de una asignación múltiple, un lugar prestado o una proyección
no `Copy` puede aparecer como fuente de movimiento únicamente si ese mismo lugar
aparece también entre los destinos y queda repuesto atómicamente. Esto permite el
swap genérico:

~~~tondo
(values[left], values[right]) = (values[right], values[left])
~~~

sin habilitar extracción ordinaria de un elemento afín ni dejar un hueco en la
colección. Para fuentes no `Copy`, el compilador debe demostrar la correspondencia
y disjunción de todos los lugares antes de reservar ownership, o conservar esa
comprobación para runtime según la regla siguiente.

Por ello, el intercambio no necesita un temporal escrito por el usuario. Repetir de forma estáticamente demostrable el mismo destino es error:

~~~tondo compile-fail E1405
var value = 0
(value, value) = (1, 2) // error
~~~

Para esta regla, un **lugar lógico** es el par formado por la identidad del
propietario raíz y su ruta de proyección después de evaluar y normalizar campos,
claves, índices y límites. Dos lugares se solapan cuando designan al menos una
misma celda lógica; la representación física y un posible detach por copy-on-write
no alteran esa identidad.

La clasificación cerrada de cada par es:

- Si el solapamiento es inevitable, emite `E1405`, también cuando los valores son
  `Copy`.
- En otro caso, si la operación no exige disjunción, no genera comprobación
  dinámica; una coincidencia eventual sigue el orden de escrituras.
- Si la operación exige disjunción y esta queda demostrada, no genera
  comprobación.
- Si la operación exige disjunción y el resultado depende de datos de runtime,
  genera obligatoriamente una comprobación de solapamiento después de resolver
  todos los lugares y antes de reservar ownership, mover un valor o escribir un
  destino.

Exigen disjunción los pares en los que interviene una fuente no `Copy`, un valor
terminal o una transferencia atómica. La validez del programa no depende así de
la potencia del análisis estático.

En la permutación especial de valores no `Copy`, la evaluación derecha produce
**tickets de movimiento** para las fuentes que también son destinos, en vez de
moverlas inmediatamente. Primero se evalúan todos los demás componentes y se
validan aridad, tipos, bounds, correspondencia y disjunción. Solo entonces se
confirman juntos los movimientos y las escrituras. Si cualquier evaluación falla,
o la comprobación dinámica detecta solapamiento, no se ha transferido ownership ni
se ha escrito ningún destino; el segundo caso produce el pánico de lenguaje
`P0004 overlapping-borrow`.

Cuando todos los componentes son `Copy` o se descartan válidamente, esos pares no
entran en el conjunto de disjunción obligatoria: dos destinos que solo resultan
iguales en runtime conservan la regla simple de escritura de izquierda a derecha
y gana el último. Un solapamiento inevitable ya fue rechazado por `E1405`. En
cuanto interviene una fuente no `Copy`, un valor terminal o una transferencia que
deba ser atómica, todos los lugares correspondientes deben ser distintos mediante
prueba estática o mediante la comprobación dinámica anterior. Así ninguna
escritura puede abandonar silenciosamente un valor anterior.

### 13.9 Acceso y llamadas

~~~tondo
value.field
tuple.0
module.name
function(arguments)
value.method(arguments)
array[index]
array[start:end:step]
generic[Type](arguments)
~~~

El acceso opcional `?.` no existe. La option se consume mediante `match`, `?` o una operación nombrada.

### 13.10 `in`

`in` comprueba pertenencia:

~~~tondo
item in array
key in map
item in valuesSet
number in range
char in string
~~~

La pertenencia observa el elemento y el contenedor durante la operación; no consume una colección afín. Requiere las capacidades de igualdad o clave que correspondan al contenedor.

Las combinaciones cerradas son:

~~~text
T    in Array[T]  cuando T: Equatable
K    in Map[K, V]
K    in Set[K]
T    in Range[T]
Char in String
~~~

Devuelve `Bool`. En maps comprueba claves, no valores; en strings comprueba un
scalar, no un substring. Búsqueda de texto o subsecuencias utiliza una operación
nombrada.

### 13.11 No truthiness

Solo `Bool` puede controlar `if` o `for condition`. Las conversiones se escriben:

~~~tondo
if not values.isEmpty() {}
if optional != none {}
if count != 0 {}
~~~

La comparación directa de una option con `none` requiere que `T` sea equatable. El estilo canónico para extraer el valor sigue siendo `match`.

---

## 14. Patrones y `match`

### 14.1 `match` como expresión

~~~tondo
let area = match shape {
    Shape.Circle(radius) =>
        math.pi * radius * radius

    Shape.Rectangle(width, height) =>
        width * height

    Shape.Point =>
        0.0
}
~~~

Una rama puede usar un bloque:

~~~tondo
match result {
    ok(value) => {
        logSuccess(value)
        consume(value)
    }

    err(error) => {
        logError(error)
        recover(error)
    }
}
~~~

Como forma breve, un arm puede contener directamente `return`, `fail`, `break` o
`continue`. La transferencia conserva exactamente su destino léxico y aporta
`Never` al tipo del arm:

~~~tondo
match result {
    ok(value) => return value
    err(error) => fail error
}
~~~

Bindings, asignaciones, `defer`, loops y varias sentencias continúan requiriendo
un bloque.

`match` evalúa el scrutinee una sola vez y elige un modo estático uniforme:

- **Copia:** si el scrutinee cumple `Copy`, no es un lvalue estable solicitado por
  algún binding `ref` y no necesita otro modo, se materializa una copia lógica
  propiedad del `match`. Un origen lvalue permanece disponible. Los bindings
  ordinarios copian sus componentes; un binding `ref` sobre un scrutinee temporal
  presta desde ese temporal hasta terminar el arm.
- **Observación:** si el scrutinee es un lvalue estable y ningún arm enlaza por
  valor el scrutinee completo ni un componente no `Copy`, el `match` crea un
  préstamo inmutable intrínseco mientras sea necesario. Este modo se elige para un
  valor no `Copy` observado y también cuando cualquier arm pide explícitamente
  `ref` sobre un lvalue `Copy`. Tags, literales y forma pueden inspeccionarse; los
  componentes `Copy` enlazados normalmente se copian y `ref name` crea un
  préstamo compartido limitado al arm. `_` y `..` no transfieren nada. El origen
  vuelve a estar disponible después del `match`.
- **Consumo:** si un scrutinee no `Copy` no es un lvalue estable o cualquier arm
  enlaza **por valor** uno de sus componentes no `Copy`, el valor completo se
  mueve a un temporal propiedad del `match` antes de probar arms. El binding
  origen queda no disponible en todos los caminos, incluso si la rama seleccionada
  solo contiene `_`.

Esta decisión uniforme evita que la disponibilidad posterior dependa de la rama
elegida en runtime, pero permite consultar elegantemente el estado de enums y
records afines sin destruirlos. Durante un match de observación no se puede mover,
reasignar ni prestar con `mut` o `var` el origen por ninguna ruta; sí pueden
invocarse operaciones `self`.

Si el scrutinee se alcanza únicamente mediante un préstamo `ref`, `self`, `mut`
o `var`, o mediante una proyección no propietaria como `Ref[T].value`, el modo
consumo no está disponible porque esa ubicación no pertenece al `match`. Un
patrón que exigiera enlazar **por valor** un componente no `Copy` es error; puede
cambiarse a `ref name`, observar solo fields `Copy`, consumir un propietario
completo o usar una operación de reemplazo cuando proceda.

Para esta regla, un lvalue es estable exactamente cuando ya está disponible, su
propietario vive durante todo el `match`, su raíz es un binding o `self`, y cada
segmento es un campo, tuple slot o índice declarado prestable por un tipo
intrínseco. Una ruta con llamada, propagación `?`, conversión o temporal nunca es
estable. La ubicación se resuelve una vez antes del préstamo. Este predicado se
decide solo con la forma de la ruta y los contratos de sus tipos; cuando es falso
se elige consumo.

Los patrones prueban tags, literales y forma antes de transferir componentes. Solo
la rama finalmente seleccionada conserva sus bindings. Por ello un guard falso no
consume valores necesarios para probar ramas posteriores. Un guard puede leer
bindings de patrón `Copy` o `ref`; cualquier decisión que necesite ownership afín
se realiza dentro del body de la rama consumidora.

Al terminar una rama en modo consumo, todo componente afín enlazado, ignorado
mediante `_` o `..`, o conservado en el temporal debe haber sido transferido,
consumido o ser abandonable. En modo observación esas obligaciones siguen en el
origen. Una obligación terminal nunca desaparece por hacer `match`.

### 14.2 Exhaustividad

Todo `match` debe ser exhaustivo, incluso si su valor se descarta.

El compilador comprueba:

- Todas las variantes de enums.
- `some` y `none`.
- `ok` y `err`.
- Todos los miembros de una unión estructural.
- Ambos valores de `Bool` y cobertura finita por literales cuando sea demostrable.
- Formas de longitud de arrays cuando los patrones cubren explícitamente vacío y no vacío; en otro caso se requiere wildcard.
- Presencia de wildcard para dominios abiertos como enteros.

Agregar una variante a un enum o miembro a una unión puede convertir matches existentes en errores de compilación. Esto es intencional.

### 14.3 Clases de patrón

#### Wildcard

~~~tondo
_
~~~

Coincide con cualquier valor y no crea binding.

#### Binding

~~~tondo
value
~~~

Coincide con cualquier valor y lo vincula.

#### Binding prestado

~~~tondo
ref value
~~~

Coincide con cualquier valor y crea un préstamo compartido limitado al arm o a la
iteración actual. No copia ni mueve el componente, incluso si este cumple `Copy`.
El binding puede utilizar métodos `self`, proyectar datos `Copy`, participar en
otro match de observación o volver a pasarse como `ref`; no puede almacenarse,
devolverse, mutarse ni consumirse.

El nombre debe ser un identificador ordinario. `ref _` es redundante e inválido;
`_` por sí solo ya observa forma sin transferir el componente.

~~~tondo
match connection {
    Connection.Open(ref socket) => socket.status()
    Connection.Closed => Status.Closed
}
~~~

En un patrón record, `ref field` abrevia `field: ref field`:

~~~tondo
match session {
    Session { ref socket, userId } =>
        inspect(ref socket, userId)
}
~~~

#### Literal

~~~tondo
0
"quit"
true
none
()
~~~

Un patrón literal utiliza la igualdad intrínseca de su tipo, sin conversiones.
Para floats sigue IEEE 754: `0.0` y `-0.0` cubren el mismo valor y arms que intenten
distinguirlos se solapan; ningún literal cubre NaN, por lo que un match sobre float
siempre necesita otra rama exhaustiva. Strings comparan su secuencia exacta de
scalars y un string interpolado no es patrón.

#### Tupla

~~~tondo
(left, right)
~~~

#### Record

~~~tondo
Point { x, y }
User { id, name: displayName, .. }
~~~

`..` ignora campos restantes. Sin `..` deben mencionarse todos.

#### Enum

~~~tondo
Shape.Circle(radius)
HttpResult.Success { status, body }
Shape.Point
~~~

#### Option y Result

~~~tondo
some(value)
none
ok(value)
err(error)
~~~

#### Unión por tipo

~~~tondo
Int(number)
String(text)
IoError { path, reason }
~~~

#### Array

~~~tondo
[]
[only]
[head, ..tail]
[first, second, ..rest]
[ref first, ..ref rest]
~~~

`tail` y `rest` tienen tipo `Array[T]`. Cuando `T: Copy`, son vistas inmutables
O(1). Con elementos afines enlazan un valor no `Copy`, por lo que seleccionan el
modo consumo: el scrutinee se mueve al `match` y el resto se transfiere a un array
propietario; la implementación no duplica elementos y puede necesitar O(n). Un
patrón de array afín que solo compruebe forma y use `_`, sin crear `tail`, sí puede
participar en un match de observación.

En `..ref rest`, el resto es en cambio un préstamo de región `ref Array[T]` O(1)
y no exige `T: Copy`; solo es válido mientras viva el arm o la iteración. Un
binding `ref` de un elemento sigue la misma regla.

No existen patrones de map porque su conjunto dinámico de claves hace difícil expresar exhaustividad y costos.

### 14.4 Guards

Una rama puede añadir una condición:

~~~tondo
match value {
    Int(number) if number < 0 => "negative"
    Int(_) => "non-negative"
    String(text) => text
}
~~~

El guard debe ser `Bool` y se evalúa después de que el patrón coincida. Una rama con guard no cuenta por sí sola como cobertura exhaustiva porque el guard puede ser falso.

### 14.5 Orden

Las ramas se prueban de arriba abajo. Una rama completamente cubierta por anteriores es error, no warning.

Tondo 0.1 no tiene patrones alternativos. Se escriben ramas separadas para evitar bindings incompatibles dentro de un único patrón.

### 14.6 Patrones irrefutables

`let`, `var` y el header de `for` solo aceptan patrones que siempre coinciden:

~~~tondo
let (x, y) = pair
let Point { x, y } = point
~~~

No aceptan:

~~~tondo compile-fail E1201
enum Shape {
    Circle(Float)
    Square(Float)
}

let optional: Int? = some(1)
let shape = Shape.Circle(1.0)

let some(value) = optional
let Shape.Circle(radius) = shape
~~~

Estos casos requieren `match`.

Un binding `ref` también es irrefutable, pero solo puede aparecer en el header de
`for`, donde su vida está cerrada por una iteración, o en un arm de `match`. No se
admite en `let` ni `var`: Tondo no crea variables locales que contengan préstamos
de duración abierta.

La asignación múltiple no acepta el lenguaje general de patrones: utiliza exclusivamente la forma tuple anidada de lvalues y `_` definida en 13.8. Esa forma siempre es irrefutable y evita que una escritura dependa de matching o cree bindings nuevos.

Los parámetros de funciones y cierres utilizan nombres, no patrones, porque sus nombres forman parte de las llamadas nombradas. La desestructuración se realiza en la primera sentencia del cuerpo.

---

## 15. Errores recuperables y pánicos

### 15.1 Tipo fallible

`T ! E` es la forma canónica de `Result[T, E]`.

`!E` es la forma canónica de `Result[Unit, E]`. En firmas aparece después de `:`:

~~~tondo
fn save(config: Config): !IoError
~~~

Precedencia de tipos:

1. Aplicación genérica y agrupación.
2. Option sufija `?`.
3. Result `!`.
4. Unión `|`.

Para evitar firmas visualmente ambiguas, una unión de varios errores requiere paréntesis:

~~~tondo
fn load(path: Path): User ! (IoError | DecodeError)
fn save(config: Config): !(IoError | ValidationError)
~~~

Una unión utilizada como tipo de éxito también se agrupa porque `!` tiene mayor precedencia que `|`:

~~~tondo
fn readValue(): (Int | String) ! DecodeError
~~~

En un resultado opaco, la lista de bounds termina antes del canal exterior:
`fn open(): impl Iterator[Row] + Discard ! IoError`. El `+` pertenece a los
bounds y `! IoError` a la función; no se requieren paréntesis adicionales.

No es canónico:

~~~tondo
fn load(path: Path): User ! IoError | DecodeError
~~~

Por precedencia, esa última firma significa `(User ! IoError) | DecodeError`; no declara dos errores del mismo `Result`. El formateador inserta los paréntesis que hacen visible esa interpretación. Para declarar la unión de errores se utiliza siempre `User ! (IoError | DecodeError)`.

### 15.2 Propagación con `?`

Aplicado a `T?`:

- `some(value)?` produce `value`.
- `none?` retorna `none` desde la función actual.

Aplicado a `T ! E`:

- `ok(value)?` produce `value`.
- `err(error)?` retorna ese error desde la función actual.

~~~tondo
fn loadUser(path: Path): User ! (IoError | DecodeError) {
    let data = fs.read(path)?
    decodeUser(data)?
}
~~~

El `?` es válido solo si la función o cierre envolvente puede representar la
salida en su canal superior:

- Para `T?`, el tipo lógico de éxito de la función debe ser directamente `U?`
  después de expandir aliases; puede existir además un canal exterior `! E`.
  Una unión que solo contenga una option como miembro no basta, porque elegiría
  implícitamente un tag de unión.
- Para `T ! E1`, la función debe declarar directamente un canal `! E2` y `E1`
  debe ser asignable a `E2`, incluido el widening cerrado de 15.3. Contener un
  `Result` dentro de una option, unión, array u otro éxito ordinario tampoco crea
  un canal de propagación.

La propagación actúa sobre la función o cierre más cercano, nunca sobre uno
exterior. Cuando una option se propaga dentro de una función cuyo éxito también
es opcional y cuyo resultado completo es fallible, la ausencia sale por el canal
de éxito. Si `lookupEntry` devuelve `Entry? ! CacheError`, se necesitan dos
propagaciones visibles porque primero se abre el `Result` exterior y después la
`Option`:

~~~tondo
fn cachedUser(id: UserId): User? ! CacheError {
    let entry = (lookupEntry(id)?)? // `none` devuelve `ok(none)`
    decodeEntry(entry)?
}
~~~

De forma dual, un error solo puede salir por el `Result` exterior declarado. El operador no reordena automáticamente `Option[Result[T, E]]` y `Result[Option[T], E]`; son tipos distintos.

No puede utilizarse para:

- Ignorar errores.
- Convertir un error arbitrariamente.
- Propagar desde una función infallible.
- Salir de un callback exterior distinto del cierre que contiene el `?`.

### 15.3 Widening de uniones de error

Si una operación produce `T ! E1` y la función devuelve
`T2 ! (E1 | E2)`, `?` inyecta `E1` automáticamente en la unión. Si el error
origen ya es una unión, el widening es válido cuando su conjunto normalizado de
miembros es subconjunto del destino:

~~~text
T ! (A | B)  ->  T ! (A | B | C)
~~~

El tag del miembro concreto se conserva. Esta es una inclusión de conjunto
cerrada, no una conversión de usuario ni una transformación recursiva de tipos
contenedores.

~~~tondo
fn loadConfig(path: Path): Config ! (IoError | DecodeError) {
    let bytes = fs.read(path)?
    decodeConfig(bytes)?
}
~~~

Si el error no es miembro exacto, el compilador rechaza la propagación y muestra:

- Error producido.
- Error esperado.
- Miembros ausentes.
- Ubicación de la firma que debe manejarse o ampliarse.

### 15.4 Errores nominales de API

Para fronteras públicas se recomienda:

~~~tondo
pub enum LoadUserError {
    Io(IoError)
    Decode(DecodeError)
    Invalid(ValidationError)
}
~~~

La conversión desde errores internos es explícita:

~~~tondo
pub fn loadUser(path: Path): User ! LoadUserError {
    let bytes = match fs.read(path) {
        ok(bytes) => bytes
        err(error) => fail LoadUserError.Io(error)
    }

    match decodeUser(bytes) {
        ok(user) => user
        err(error) => fail LoadUserError.Decode(error)
    }
}
~~~

La librería podrá ofrecer `mapError` como abreviatura ordinaria, pero el lenguaje no define conversiones automáticas `From`.

### 15.5 Consumo

~~~tondo
match loadUser(path) {
    ok(user) => show(user)

    err(LoadUserError.Io(error)) =>
        reportIo(error)

    err(LoadUserError.Decode(error)) =>
        reportDecode(error)

    err(LoadUserError.Invalid(error)) =>
        reportValidation(error)
}
~~~

El compilador exige exhaustividad.

### 15.6 `fail` frente a `err`

Dentro de una función fallible se prefiere:

~~~tondo
fail ValidationError.Invalid
~~~

`err(error)` se utiliza cuando `Result` es un valor ordinario:

~~~tondo
let result: Int ! ParseError = err(ParseError.Empty)
~~~

Esto conserva una diferencia visible entre “salir de esta función” y “construir un valor”.

### 15.7 Pánicos

`panic` es un intrinsic del prelude con la firma exacta:

~~~tondo
fn panic(message: String): Never
~~~

Acepta exactamente un `String`, no puede sobrecargarse ni reemplazarse mediante
imports. Su argumento se evalúa por completo antes de comenzar el pánico; la
invocación produce `P0008` y el mensaje pasa al diagnóstico runtime. Interpolación,
formato y construcción de mensajes ocurren antes de la llamada mediante las reglas
ordinarias.

Un pánico representa un invariante roto, por ejemplo:

- Índice directo fuera de rango.
- Paso de slice igual a cero.
- División entera por cero.
- Overflow comprobado.
- Conteo de shift negativo, no representable como `Int` o fuera del ancho del
  operando.
- Longitudes incompatibles en aritmética de arrays.
- Solapamiento detectado por el check dinámico normativo de préstamos o
  transferencia atómica.
- Claves dinámicas repetidas en un literal de map cuyo valor no es `Discard`.
- Una llamada explícita a `panic` o `assert` fallido.

Un pánico:

- No forma parte de `! E`.
- No puede capturarse en Tondo 0.1.
- Desenrolla scopes mediante el orden único de 8.10 y ejecuta en LIFO todos los
  `defer` registrados por el código que abandona.
- Ejecuta exactamente una vez el fallback intrínseco de todo token terminal vivo
  que no tenga guard, incluidos bindings ordinarios y temporales cuya transferencia
  no se confirmó.
- Dentro de concurrencia estructurada, cancela hermanos, espera su cleanup y se propaga como pánico hasta el scope propietario.
- Si alcanza el scope raíz, termina el proceso después del cleanup estructurado.
- Debe producir un diagnóstico con ubicación y una traza cuando el runtime disponga de símbolos.

Todos los pánicos definidos por el lenguaje —incluidos bounds, overflow, división
entera por cero, solapamiento dinámico, `assert` y `panic(...)`— siguen esas
mismas reglas y los códigos `P` de 22.2 en todas las implementaciones conformes.
OOM irrecuperable, stack overflow, corrupción del runtime, señales fatales y
terminación externa quedan fuera del modelo de pánico Tondo y pueden abortar sin
unwind.

Los pánicos no deben utilizarse para:

- Validación de usuario.
- Ausencia esperable.
- I/O.
- Errores de parseo.
- Timeouts.
- Fallos remotos.

Esos casos son errores recuperables.

### 15.8 `assert`

`assert` es un intrinsic no sobrecargable del prelude con la firma exacta:

~~~tondo
fn assert(condition: Bool, messageParts: ...String)
~~~

Produce pánico si la condición es falsa. Sin partes de mensaje, el runtime utiliza la ubicación y una representación de la condición; con una o más, las concatena en orden sin separador. Así `assert(condition)` y `assert(condition, message)` no requieren sobrecarga ni parámetros por defecto.

La condición y las partes siguen la evaluación ordinaria de argumentos de izquierda a derecha; los mensajes no son cierres lazy implícitos.

Las aserciones no se eliminan en builds optimizadas. Si una comprobación puede eliminarse, deberá existir una operación de debug explícita en tooling, no cambiar la semántica de `assert`.

---

## 16. Mutabilidad, préstamos, memoria y concurrencia

### 16.1 Semántica de `let` y `var`

~~~tondo
let immutable = [1, 2, 3]
var mutable = [1, 2, 3]
~~~

Mediante `immutable` no se puede:

- Reasignar.
- Cambiar elementos.
- Cambiar campos.
- Invocar métodos con `mut self`.
- Invocar métodos con `var self`.
- Pasar como argumento `mut` o `var`.

Sí puede observarse mediante métodos `self` o pasarse como `ref`; ninguno concede
escritura ni mueve el valor.

Mediante `mutable` sí puede hacerse.

La inmutabilidad es transitiva a través del binding. No depende de si el almacenamiento interno está compartido.

### 16.2 Parámetros inmutables

Por defecto, un parámetro recibe un valor lógico inmutable:

~~~tondo
fn sum(values: Array[Int]): Int {
    panic("cuerpo de ejemplo omitido")
}
~~~

Pasar un array, map, set, record o enum `Copy` no exige una copia física
inmediata. La implementación puede compartirlo porque la función no puede
modificar el valor del llamador. Pasar un valor afín por un parámetro ordinario
mueve ownership al callee; una observación temporal sin transferencia utiliza
`ref` y un acceso temporal exclusivo utiliza `mut` o `var`.

### 16.3 Parámetros `ref`, `mut` y `var`

~~~tondo
fn inspect(resource: ref Resource): Status {
    resource.status()
}

let status = inspect(ref resource)
~~~

`ref` crea un préstamo compartido y de solo lectura:

- No copia ni mueve el valor y deja disponible el propietario al terminar.
- Dentro del callee, el parámetro puede observarse, proyectar campos `Copy`,
  participar en un `match` de observación y volver a prestarse como `ref`; no
  puede devolverse por valor, modificarse ni transferir contenido no `Copy`.
- Acepta un lvalue `let` o `var`, un parámetro propietario y una reborrow de
  `ref`, `mut` o `var`. También acepta un temporal, que permanece poseído por el
  llamador hasta completar una llamada directa.
- Pueden coexistir préstamos `ref` solapados. Cualquier préstamo `mut` o `var`, o
  movimiento del propietario, espera a que todos ellos terminen.
- En una llamada síncrona dura únicamente hasta regresar el callee. En async
  sigue las reglas de `await` y `spawn` de 11.10 y 11.12.

`ref` no es `Ref[T]`: el primero es permiso temporal comprobado por el compilador,
no tiene identidad ni puede almacenarse; el segundo es un valor copiablemente
almacenable con identidad estable.

~~~tondo
fn scale(values: mut Array[Float], factor: Float) {
    values *= factor
}
~~~

Llamada:

~~~tondo
var numbers = [1.0, 2.0, 3.0]
scale(mut numbers, 10.0)
~~~

`mut` aparece tanto en la firma como en la llamada para que la mutación sea visible en ambos lados.

Un parámetro `mut` es acceso exclusivo de **extensión fija** al lvalue del
llamador. Puede cambiar o reemplazar los valores contenidos cuando conserva la
extensión estructural:

- En `Array`, conserva longitud y región; permite escribir índices, slices de igual longitud y aritmética in-place.
- En `Map`, conserva el conjunto y orden de claves; puede reemplazar valores existentes mediante una operación que afirme presencia.
- En `Set`, observar o reemplazar una representación equivalente no cambia pertenencia; insertar o eliminar requiere `var`.
- Escalares, records, enums y tuples ocupan una sola región lógica, por lo que sus valores o campos pueden reemplazarse normalmente.
- Un tipo opaco declara qué operaciones conservan extensión.

Para un `T` genérico, el compilador no presupone que dos valores tengan la misma
extensión estructural. Un préstamo `mut T` solo admite operaciones cuyo contrato
demuestre preservación —por ejemplo un método de trait declarado `mut self`—; un
reemplazo genérico arbitrario utiliza `var T`.

La asignación simple a la raíz de un préstamo, `target = replacement`, solo se
admite mediante `mut` cuando el tipo exterior demuestra estáticamente una
extensión fija. Esto incluye escalares, records, enums, tuples y newtypes, pero no
una raíz `Array`, `Map`, `Set`, un parámetro genérico ni un resultado opaco. En
esos últimos casos la asignación produce `E1411`; un reemplazo estructural
arbitrario requiere `var`. La regla no elimina el reemplazo de contenido con
extensión conocida: `values[:] = replacement` sustituye atómicamente todos los
elementos de un `Array` después de comprobar que ambas longitudes coinciden, y
las operaciones o métodos con contrato `mut self` siguen disponibles. Tondo no
compara silenciosamente la forma dinámica de dos valores para decidir qué
significa una asignación raíz.

Un parámetro `var` es acceso exclusivo **estructural**. Incluye todas las
capacidades de `mut` y permite además reasignar sin conservar la extensión,
redimensionar una colección o cambiar su estructura:

~~~tondo
fn addValue(values: var Array[Int], value: Int) {
    values.append(value)
}

var numbers = [1, 2]
addValue(var numbers, 3)
~~~

Los dos préstamos exclusivos terminan al regresar la llamada y no transfieren al
callee el ownership de la ubicación. El argumento queda inaccesible por cualquier
ruta solapada mientras el préstamo está activo. `var` solo acepta un lvalue
completo y reemplazable: un binding, campo, tuple slot o elemento que almacene el
valor completo. No acepta slices, regiones parciales ni un lookup de map cuya
presencia no esté garantizada.

El préstamo no adquiere ownership del contenido no `Copy`. Leerlo por valor,
devolverlo o pasarlo a un parámetro consumidor sería un movimiento ordinario y es
error. Sí puede participar en una asignación confirmada que instala a la vez un
reemplazo válido. Esto permite escribir swaps y un `replace` genérico sin dejar un
hueco observable:

~~~tondo
fn replace[T](target: var T, replacement: T): T {
    var previous = replacement
    (previous, target) = (target, previous)
    previous
}
~~~

La ubicación prestada debe quedar disponible en todos los caminos que salen
normalmente de la llamada. La asignación múltiple anterior reserva ambos contenidos
y solo confirma sus movimientos al instalar todos los destinos; un pánico antes
de esa confirmación restaura los propietarios según 8.10. `mut` admite el mismo
patrón cuando ambos valores conservan la extensión exigida.

La distinción forma parte de la firma y del tipo de función; nunca se infiere desde el cuerpo. De este modo una función `mut Array[T]` acepta indistintamente un array completo o un slice y sabe que su longitud no cambiará, mientras una función `var Array[T]` conserva toda la capacidad de redimensionar un propietario completo.

Las reborrows se interpretan respecto de la extensión de la **raíz prestada**:

- `ref` puede salir de `ref`, `mut` o `var`.
- `mut` puede salir de `mut` o `var` cuando conserva la extensión de la región
  exterior.
- `var` sobre esa misma raíz solo puede salir de `var`.
- Un `mut` exterior puede, no obstante, prestar como `var` un sublugar estricto,
  completo y reemplazable cuando cambiar la extensión de ese subvalor conserva
  demostrablemente la extensión de la raíz exterior. Esto incluye un campo de
  record, tuple slot o elemento existente que almacene un valor completo; no
  incluye un slice, una clave potencialmente ausente ni una proyección opaca sin
  ese contrato.

Así un método `mut self` de un record puede ejecutar
`self.items.append(value)`: cambia el `Array` almacenado en el campo, pero el
record continúa teniendo exactamente los mismos campos. Del mismo modo, un
`mut Array[Array[T]]` puede redimensionar un elemento existente sin cambiar la
longitud del array exterior. Para un `mut T` genérico no se presume una
proyección de este tipo; debe venir dada por la forma estática o por un contrato
de trait. Durante la llamada interior el préstamo exterior queda suspendido en la
región solapada y vuelve a estar disponible al terminar.

Los argumentos de préstamo se resuelven y reservan de izquierda a derecha en su
posición de evaluación. Desde esa reserva hasta el retorno, argumentos
posteriores solo pueden crear préstamos compatibles sobre una región solapada;
no pueden moverla ni obtener acceso exclusivo incompatible. El callee comienza
después de resolver todos los argumentos. Si una evaluación falla antes, se
liberan las reservas ya creadas y ningún préstamo escapa de la llamada. La regla
especial para resolver varias regiones de un mismo origen sin detach intermedio
se define en 16.4.

### 16.4 Préstamos de regiones

Una región puede observarse sin crear un slice propietario:

~~~tondo
let status = summarize(ref resources[1:3])
~~~

`ref resources[start:end:step]` produce un préstamo compartido
`ref Array[T]`, incluso cuando `T` no sea `Copy`. Regiones compartidas
solapadas pueden coexistir; mientras viva alguna, el origen no puede moverse,
redimensionarse ni recibir un préstamo exclusivo solapado. No se realiza detach
por copy-on-write porque no existe escritura.

~~~tondo
var numbers = [1.0, 2.0, 3.0, 4.0]
scale(mut numbers[1:3], 10.0)
~~~

La función recibe una vista mutable exclusiva de la región. Los cambios afectan a `numbers`.

Si `numbers` comparte almacenamiento con una copia o slice inmutable anterior, el compilador separa primero el valor completo de `numbers` y crea la vista mutable sobre su nuevo almacenamiento. El snapshot anterior conserva su contenido.

Durante un préstamo de región `mut` no se puede:

- Leer o escribir la región solapada por otra ruta.
- Redimensionar el almacenamiento.
- Crear otro préstamo mutable solapado.
- Conservar la vista más allá de la llamada.

Regiones demostrablemente disjuntas pueden prestarse a la vez:

~~~tondo
process(mut numbers[:2], mut numbers[2:])
~~~

La aceptación de una llamada no depende de la potencia del análisis estático. Para
cada región se calcula el conjunto lógico de índices que produciría el algoritmo
de slicing de 10.4 después de normalizar bounds y paso. No es necesario enumerar
el conjunto: una implementación puede decidir la intersección aritméticamente.

- Una intersección demostrable entre préstamos incompatibles produce `E1403`.
- Una disjunción demostrable no necesita comprobación en runtime.
- Si la intersección depende de valores de runtime, el compilador inserta
  obligatoriamente una comprobación de solapamiento.

En una llamada con varias regiones del mismo origen, bases, índices y límites se
evalúan exactamente una vez y de izquierda a derecha. Después se validan bounds y
solapamientos, y solo si todo tiene éxito se realiza como máximo un detach y se
crean todos los préstamos. Ninguna región apunta al almacenamiento abandonado por
copy-on-write. Si la comprobación dinámica falla, se produce
`P0004 overlapping-borrow` antes del detach y antes de entrar en el callee; por
tanto no hay mutación observable ni préstamo parcial.

Un argumento `var` sobre el origen completo entra en conflicto estáticamente con
cualquier otro préstamo de ese origen, aunque las regiones parezcan disjuntas,
porque puede cambiar su representación.

Cuando una misma llamada mezcla regiones `ref` y `mut`, toda región `mut` debe ser
disjunta de las demás; las regiones exclusivamente `ref` sí pueden solaparse.
Bases y límites se evalúan una sola vez antes de activar préstamos, y cualquier
fallo de bounds o disjunción ocurre antes de que el callee comience. La prueba
estática es únicamente una optimización que permite eliminar el check normativo;
nunca cambia qué programas son válidos.

### 16.5 Vida de préstamos

La vida de un préstamo `ref`, `mut` o `var` se infiere y no se escribe como
lifetime. En una llamada síncrona queda limitada dinámicamente a esa llamada. Un
`ref` de una llamada async puede extenderse hasta completar el `await` o consumir
el `Join` propietario, pero continúa ligado a esa estructura y nunca se convierte
en un valor de referencia. Un binding de patrón `ref` queda limitado al arm o a
la iteración; puede cruzar un `await` dentro de ellos cuando el valor referido cumple
`Send`, y solo puede prestarse a un hijo concurrente con las garantías
estructuradas `Send + Share` de 11.12.

Dentro de esos límites, el análisis termina un préstamo en su último uso posible,
no necesariamente al final textual del bloque. Una rama posterior que todavía
pueda utilizar el binding prolonga la vida; la mera visibilidad de un nombre ya no
usado no lo hace. El préstamo de la colección que sustenta un `for ref` sí dura
todo el bucle porque cada iteración puede solicitar la siguiente ubicación.

Ningún préstamo puede:

- Devolverse.
- Guardarse en un record, enum, array o map.
- Capturarse por un cierre.
- Asignarse a estado global.
- Sobrevivir al valor origen.

Además, un préstamo `mut` o `var` no puede cruzar una suspensión. Estas
restricciones son deliberadamente menores que un sistema general de ownership:
protegen observación temporal, mutación temporal y regiones sin introducir
referencias de primera clase ni anotaciones de vida.

### 16.6 Copy-on-write

Las colecciones y valores grandes que cumplen `Copy` tienen semántica de copia lógica:

~~~tondo
var left = [1, 2, 3]
var right = left

right[0] = 9
~~~

Resultado:

~~~text
left  = [1, 2, 3]
right = [9, 2, 3]
~~~

Una implementación típica comparte el buffer hasta la primera mutación. El programa no puede observar si se copió antes o después.

Una vista inmutable creada por slicing actúa como snapshot lógico. Si el origen se modifica después, copy-on-write conserva el valor observado por la vista.

Solo un préstamo `mut` o `var` establece alias mutable intencional.

Un compuesto que contiene un elemento afín no cumple `Copy` y se mueve como un todo; copy-on-write nunca duplica ownership de recursos afines.

### 16.7 Gestión automática

#### Contrato normativo

Tondo no expone `malloc`, `free`, operaciones raw ni destructores arbitrarios en
código seguro. La acción de unwind cerrada de un tipo terminal no es un destructor
de usuario: no se ejecuta por recolección ni por la salida ordinaria de un binding,
y solo neutraliza ownership en los casos cerrados definidos por 8.10, incluido el
teardown de un propietario estructurado. `Pointer[T]` existe exclusivamente detrás
de `unsafe`, como se define en 16.12.

La implementación debe:

- Mantener vivos valores alcanzables.
- Hacer reclamable la memoria no alcanzable y recuperarla bajo presión de
  asignación continuada.
- Manejar ciclos sin exigir intervención del usuario.
- No ejecutar finalizadores de usuario en momentos no deterministas.

En esta especificación, **gestión automática** incluye tanto un tracing GC como
Automatic Reference Counting (ARC) u otras técnicas que satisfagan esas
garantías. Un contador de referencias puro que conserve para siempre un ciclo
inalcanzable no es una implementación conforme. El usuario tampoco puede quedar
obligado a introducir `WeakRef[T]` para que un programa correcto no filtre ciclos.

La estrategia concreta puede ser tracing GC, ARC con recolección de ciclos o una
combinación. No puede cambiar el resultado ordinario del programa; el instante
físico de reclamación solo puede manifestarse mediante consumo de recursos,
rendimiento y APIs de observación de liveness expresamente documentadas, como una
futura `WeakRef[T]`. Estas APIs no constituyen un mecanismo de cleanup ni pueden
ser necesarias para la corrección funcional.

No existe un plazo observable para reclamar un objeto concreto cuando no hay
presión de memoria ni progreso del runtime. Sin embargo, una implementación no
puede conservar indefinidamente ciclos inalcanzables mientras continúan
asignaciones que obligan a recolectar: antes de declarar OOM por heap debe
intentar una recolección completa de memoria inalcanzable y reintentar la
asignación. Si el live set, fragmentación inevitable o límites del host todavía
impiden satisfacerla, se aplica el aborto de 15.7. Terminar el proceso no exige
recorrer el heap ni simular cleanup de recursos.

#### Diseño de la implementación de referencia (no normativo)

La implementación de referencia de Tondo 0.1 se diseñará alrededor de **ARC con
recolección diferida de ciclos**. Esta elección no forma parte de la conformidad
del lenguaje ni de su ABI y puede sustituirse sin cambiar semántica de código
fuente.

El diseño previsto es:

- Representar inline o en stack los valores que no necesiten escapar.
- Administrar mediante ARC los buffers compartidos de `String`, `Array[T]`,
  `Map[K, V]` y `Set[T]`. El mismo contador permite comprobar unicidad para
  copy-on-write: un buffer único puede mutarse in place; uno compartido se copia
  antes de la mutación.
- Administrar también mediante ARC las identidades `Ref[T]` y, cuando escapen,
  cierres, entornos capturados y frames asíncronos.
- Mantener por separado las referencias fuertes y débiles de una eventual
  `WeakRef[T]`; una referencia débil nunca mantiene vivo su destino.
- Introducir en la IR operaciones internas equivalentes a `retain`, `release`,
  `is_unique` y `trace_edges`. No son operaciones del lenguaje fuente. Los moves
  transfieren ownership sin incrementar el contador y el análisis de último uso
  puede eliminar pares de retención y liberación redundantes.
- Emitir para cada tipo administrado que pueda contener referencias fuertes un
  descriptor o función de trazado. Un decremento que no alcance cero puede
  registrar el objeto como candidato; el recolector de ciclos utiliza esas aristas
  para liberar componentes inalcanzables sin explorar innecesariamente objetos
  hoja.
- Ejecutar las cascadas de liberación de forma acotada o iterativa. Recolectar un
  objeto o un ciclo nunca ejecuta código Tondo de usuario.

El baseline inicial del runtime de referencia concreta ese diseño así:

- Todo bloque administrado es no móvil y comienza con un header privado que
  contiene contador fuerte atómico, contador débil atómico, flags y un descriptor
  de tipo. La estabilidad física no se expone como identidad ni se promete a
  código fuente.
- El descriptor enumera aristas administradas y las operaciones internas para
  destruir almacenamiento. No contiene finalizadores ni callbacks Tondo.
- Cada owner lógico en stack, agregado, buffer o frame posee una referencia
  fuerte. Un move transfiere ese token sin tocar el contador; una copia lógica
  que comparte bloque ejecuta `retain`.
- Mientras el contador fuerte sea distinto de cero existe una referencia débil
  implícita al header. El último `release` fuerte marca el payload como muerto de
  forma linealizable, invalida futuros upgrades, libera sus aristas mediante una
  cola iterativa que no asigna memoria administrada y retira la débil implícita.
  El header desaparece al llegar a cero el contador débil.
- `WeakRef.upgrade`, cuando la librería lo exponga, intenta incrementar
  atómicamente un contador fuerte todavía no nulo. Su éxito o ausencia es
  linealizable respecto al último release o a la decisión del recolector de
  ciclos.
- Un decremento que no llegue a cero registra como candidato únicamente un bloque
  cuyo descriptor contenga aristas fuertes. Bloques hoja no entran en el
  recolector de ciclos.
- Al superar un umbral de candidatos o presión de asignación, el runtime detiene
  en safepoints a los threads Tondo adjuntos, obtiene un snapshot de los
  candidatos y aplica trial deletion: descuenta aristas internas, marca desde los
  nodos con referencias externas y reclama el resto. Reanuda threads solo después
  de dejar coherentes contadores y weak refs. Un thread extranjero que ejecute un
  callback Tondo debe haberse adjuntado y cooperar con ese protocolo.
- Antes de informar un OOM irrecuperable por heap, el runtime ejecuta una pasada
  completa de ciclos, libera sus colas pendientes y reintenta una vez la
  asignación. Si todavía falla, conserva la política de aborto sin unwind de
  15.7.
- `is_unique` solo permite mutación in place cuando existe exactamente un owner
  fuerte del buffer y el análisis de préstamos ya concedió acceso exclusivo. Un
  contador igual a uno nunca sustituye al borrow checking.

Este baseline utiliza contadores atómicos también para bloques confinados a un
thread, priorizando una representación única. El compilador puede eliminar
retenciones, asignar valores en stack o reemplazar internamente el contador por
una variante local cuando demuestre que una identidad no cruza una frontera; no
puede alterar `Send`, `Share`, liveness, COW ni el resultado de `WeakRef.upgrade`.
Otras implementaciones conservan libertad para tracing GC o diseños híbridos.

### 16.8 Recursos externos

Archivos, sockets, locks y otros recursos no dependen del recolector. Se cierran explícitamente y pueden apoyarse en `defer`:

~~~tondo
let file = fs.open(path)?
defer fs.File.release(file)

file.readHeader()?
~~~

El defer reserva el handle afín para su consumo terminal, pero sus métodos de observación continúan disponibles hasta abandonar el scope. No existen finalizadores de usuario como contrato de cleanup.

### 16.9 Valores e identidad

Records, arrays, maps, sets y strings comparan por valor. Copiar uno no crea una relación de identidad entre origen y copia, aunque la implementación comparta almacenamiento mediante copy-on-write.

Tondo no ofrece un operador universal `===`. La identidad aparece únicamente en tipos que la declaran, principalmente `Ref[T]` y handles nominales de librería.

Las tres formas de acceso no se solapan:

| Forma | Almacenable | Identidad | Seguridad |
|---|---:|---:|---|
| `ref value` / parámetro `ref T` | No | No | Préstamo compartido comprobado |
| `Ref[T]` | Sí | Sí | Referencia fuerte administrada |
| `Pointer[T]` | Sí | Dirección raw | Operaciones solo en `unsafe` |

Un préstamo `ref` nunca se convierte implícitamente en `Ref[T]` ni en
`Pointer[T]`; crear identidad o aceptar una dirección son operaciones distintas y
visibles.

### 16.10 `Ref[T]`

`Ref[T]` es una referencia segura, fuerte, no nula y con identidad estable a un
valor `T: Discard` administrado automáticamente:

~~~tondo
let users: Ref[Map[String, User]] = Ref([:])
let sameUsers = users
~~~

La formación de `Ref[T]` exige `T: Discard`. Esta es la restricción mínima que
permite que una referencia fuerte duplicable desaparezca con normalidad: impide
esconder una obligación terminal cuyo cleanup tendría que ejecutarse de forma
implícita al desaparecer la última referencia, pero no exige que el contenido
pueda copiarse. Un valor afín abandonable puede por tanto adquirir identidad; un
archivo, `Join` u otro owner terminal utiliza en cambio un handle explícito
diseñado para su protocolo de cierre. En código genérico, formar `Ref[T]` exige el
constraint `T: Discard`.

Copiar un `Ref[T]` copia la referencia, no `T`. Ambos bindings identifican el
mismo objeto y lo mantienen vivo. El contenido se observa mediante la proyección
intrínseca `value`:

~~~tondo
let count = users.value.length()
~~~

Con precisión, `value` es una proyección compartida y de solo lectura sobre el
objeto identificado, no una extracción de ownership. Puede utilizarse como
receptor `self`, scrutinee de observación, origen de proyecciones `Copy` o
argumento prestado:

~~~tondo
inspect(ref reference.value)
~~~

Materializar `reference.value` como un valor independiente solo es válido cuando
`T: Copy`; en ese caso produce su copia lógica ordinaria. Si `T` no es `Copy`, el
programa continúa pudiendo observarlo sin moverlo, pero no puede devolverlo,
almacenarlo por valor ni extraerlo de la referencia. La proyección tampoco admite
asignación ni préstamos `mut` o `var`.

Propiedades:

- `Ref(value)` crea una identidad nueva incluso si existe otra referencia a un valor igual.
- Construirla copia `value` cuando cumple `Copy` y lo mueve en otro caso.
- `Ref[T]` cumple `Copy`; duplicarlo nunca duplica inmediatamente `T`.
- `Ref[T]` nunca contiene ausencia; se utiliza `Ref[T]?` cuando sea necesaria.
- `==` y `!=` entre referencias comparan identidad, no contenido.
- `Ref[T]` cumple `Key` y su hash de identidad no cambia mientras vive.
- Dos referencias distintas pueden contener valores iguales y seguir siendo claves diferentes.
- La identidad es válida dentro del proceso; no se serializa ni se conserva entre ejecuciones.
- No se expone la dirección física. Un recolector móvil puede cambiarla sin cambiar identidad.
- Para comparar contenido se compara explícitamente `left.value == right.value` cuando `T` sea equatable.

Ejemplo como clave:

~~~tondo
let metadata: Map[Ref[Map[String, User]], Metadata] = [:]
var indexed = metadata
indexed[users] = Metadata { source: "cache" }
~~~

Cuando `T: Copy`, materializar `Ref[T].value` puede evitar una copia física y
utilizar copy-on-write. En ningún caso puede moverse el objeto fuera de la
referencia ni escribirse por alias. `Ref[T]` no introduce mutabilidad escondida
del objeto identificado. El estado compartido modificable se construye
explícitamente con tipos como `Cell[T]`, `Mutex[T]`, `Atomic[T]` o actores, cuyos
contratos pertenecerán a la librería estándar.

Una referencia débil `WeakRef[T]`, también limitada a `T: Discard`, puede existir
en la librería para caches y grafos que no deban mantener vivo el destino. No
cambia las garantías de `Ref[T]`. Consultarla constituye una observación de
liveness: la librería debe documentar que el resultado puede depender del progreso
del recolector y de carreras concurrentes. `WeakRef[T]` no puede utilizarse para
cleanup, sincronización ni para una transición de estado que deba ocurrir en un
instante determinista; esos contratos requieren ownership o coordinación
explícitos.

### 16.11 Handles con identidad

Un tipo opaco de librería puede declarar identidad propia, por ejemplo un proceso hijo, socket o ventana. Debe especificar:

- Si el handle cumple `Copy` y comparte identidad, o si es afín y transfiere ownership al moverse.
- Si cumple `Discard` o conserva una obligación terminal; ambas afirmaciones son
  mutuamente excluyentes.
- Si tiene igualdad o cumple `Key`.
- Qué operación libera o cierra el recurso.
- Qué acción infallible y no suspendible utiliza durante unwind si conserva una
  obligación terminal. `Join` es la única excepción estructurada y no es un handle
  opaco de librería.
- Si puede cruzar tareas o threads.
- Si abandonar el scope exige haber ejecutado una operación terminal.

Un handle compartido copiablemente debe hacer seguro y explícito el estado compartido de cierre. Un handle en propiedad se declara afín; sus operaciones de observación utilizan `self`, las mutaciones de extensión fija utilizan `mut self`, los reemplazos de estado estructural utilizan `var self` y una operación terminal que consume ownership se expresa como función asociada con un parámetro ordinario por valor.

La mera representación interna mediante un puntero no concede identidad observable ni `Key` automáticamente.

### 16.12 `Pointer[T]` y `unsafe`

`Pointer[T]` representa una dirección raw no nula a un valor de tipo `T`. No mantiene vivo el almacenamiento, no tiene lifetime comprobado y no puede leerse, escribirse, desplazarse ni convertirse desde o hacia una dirección fuera de una región `unsafe`.

Una función `unsafe` declara que el llamador debe cumplir precondiciones no verificables por el compilador:

~~~tondo
unsafe fn readHeader(address: Pointer[Byte]): Header ! DecodeError {
    let first = address.read()
    decodeHeader(first)?
}
~~~

El cuerpo de una función `unsafe` es una región unsafe. Su documentación pública debe enumerar todas las obligaciones que hacen válida una llamada.

La llamada requiere una región explícita:

~~~tondo
let header = unsafe {
    readHeader(address)?
}
~~~

Una función puede ser simultáneamente async y unsafe:

~~~tondo
async unsafe fn readHeaderAsync(
    address: Pointer[Byte],
): Header ! DecodeError {
    let first = address.read()
    await decodeHeaderAsync(first)?
}
~~~

El llamador mantiene visibles ambos efectos:

~~~tondo
let header = unsafe {
    await readHeaderAsync(address)?
}
~~~

El bloque `unsafe` autoriza las precondiciones raw y `await` expresa la
suspensión; ninguno sustituye al otro. La combinación no exime las reglas
concurrentes de 11.10 y 16.13.

Una función segura puede contener un bloque `unsafe` privado si valida primero las condiciones necesarias y no permite que un estado inválido escape. `unsafe` habilita únicamente operaciones catalogadas como inseguras; no desactiva comprobación de tipos, inicialización, exhaustividad ni manejo de errores.

#### Frontera de seguridad y comportamiento indefinido

Un programa escrito íntegramente en Tondo seguro, ejecutado sobre un compilador,
runtime, librería estándar y wrappers nativos conformes, no produce
**comportamiento indefinido**. El comportamiento indefinido solo puede originarse
cuando se ejecuta una operación unsafe sin satisfacer todas sus precondiciones, o
cuando un componente nativo viola el contrato de ABI, memoria o runtime que
declaró.

Escribir `unsafe`, construir un bloque unsafe o conservar un `Pointer[T]` opaco no
constituye por sí solo comportamiento indefinido. Cada operación unsafe debe
publicar precondiciones completas y comprobables por quien audita la llamada. Si
una precondición ejecutada se incumple:

- No se produce un pánico ni un `Result`; tampoco existe un diagnóstico `P`
  recuperable.
- Esta especificación deja de imponer comportamiento a esa ejecución. El proceso
  puede abortar, atrapar la infracción, continuar aparentemente o corromper
  estado; ningún programa puede depender de uno de esos resultados.
- El optimizador solo puede asumir las precondiciones documentadas de las
  operaciones unsafe que llegan a ejecutarse. Una precondición oculta o no
  documentada es un defecto de la implementación o de la API, no una obligación
  inventada para el llamador.

Entre las infracciones que producen comportamiento indefinido se incluyen:

- Leer o escribir mediante un puntero con procedencia, alineación, tamaño,
  inicialización, vida o representación de `T` inválidas.
- Acceder fuera del objeto o región que concede la procedencia, aunque la
  dirección numérica coincida con memoria asignada.
- Escribir almacenamiento inmutable o mientras existe un alias incompatible.
- Provocar una data race mediante código unsafe o nativo.
- Invocar código nativo con firma o calling convention incorrectas, o utilizar un
  callback después de terminar la vida concedida.
- Violar obligaciones de roots, retain/release, pinning o attachment de threads
  que exija el runtime.
- Permitir que un pánico de Tondo o una excepción extranjera cruce una frontera
  ABI que no lo admita.

`unsafe` no desactiva las reglas ordinarias de tipos, bounds, overflow,
inicialización, exhaustividad, ownership ni evaluación. Una infracción que el
lenguaje define como check dinámico sigue produciendo su pánico normativo aunque
ocurra dentro de un bloque unsafe. Una API segura implementada con unsafe debe
validar todas las condiciones controlables por sus argumentos y encapsular las
restantes de forma que ningún uso seguro pueda romper sus invariantes.

Reglas:

- `Pointer[T]?` representa un puntero que también puede contener ausencia; Tondo no introduce `null`.
- `Pointer[T]` no cumple `Key`; para identidad estable se utiliza `Ref[T]` o un handle nominal.
- No existe dereference automático.
- Lectura, escritura, offsets, casts y construcción desde direcciones son operaciones nombradas y unsafe.
- No existen operadores aritméticos especiales sobre punteros.
- El programador debe garantizar alineación, tamaño, inicialización, mutabilidad, procedencia y vida suficiente.
- Un puntero solo puede capturarse por un cierre `unsafe` o `async unsafe`; así la
  procedencia, alineación y vida exigidas por cada uso forman parte visible del
  contrato de llamada. El llamador debe mantener esas precondiciones durante toda
  la invocación y, en forma async, a través de todas sus suspensiones. El cierre
  concreto deriva que no es `Send` ni `Share` porque `Pointer[T]` tampoco lo es.
  Una frontera que necesite cruzar una task o thread valida sus precondiciones y
  encapsula primero el puntero en un handle opaco auditado; `unsafe` no convierte
  al puntero raw ni al cierre que lo contiene en `Send` o `Share`.
- `malloc`, `free`, layouts y calling conventions pertenecen a la capa FFI o de sistema, no al prelude seguro.

Una integración que necesite indexar por dirección no convierte
`Pointer[T]` implícitamente en clave. Un módulo de sistema privilegiado puede
exponer un handle nominal `Key` construido dentro de `unsafe`, con un contrato
explícito sobre procedencia, reutilización y vida de la identidad. Así la
capacidad existe sin afirmar que toda dirección raw sea estable.

### 16.13 Capacidades `Send` y `Share`

La seguridad concurrente se expresa mediante dos capacidades intrínsecas cerradas:

- `Send`: un valor puede transferirse o copiarse lógicamente a otra tarea o thread sin invalidar la seguridad de memoria.
- `Share`: un valor puede observarse de forma inmutable desde varias tareas o threads a la vez.

`Unit`, `Never`, booleanos, números, `Byte`, `Char`, `String` y todos los valores
uniformes de tipo `fn(...)` cumplen ambas. Tuples, records, enums, options,
results, uniones, arrays, maps, sets, ranges, newtypes y cierres concretos las
derivan cuando todos sus componentes o capturas relevantes las cumplen. Un cursor
que implemente `Iterator[T]` conserva las
capacidades de su tipo concreto: ni `T: Send` vuelve enviable su estado ni un
estado `Share` permite avanzar sin préstamo exclusivo. `Command` y `Pipeline`
cumplen ambas porque sus planes solo conservan configuración copiable y
compartible. Los tipos opacos restantes declaran estas capacidades como parte
auditada de su contrato. Son bounds genéricos intrínsecos, no traits que un módulo
pueda implementar de forma arbitraria.

Reglas relevantes:

- Un valor que cruza en propiedad hacia otra task o thread debe ser `Send`. Si se duplica una identidad para observarla concurrentemente, su contenido debe ser `Share`.
- Un préstamo `ref` enviado a un hijo estructurado exige `Send + Share`; un
  `await` secuencial exige `Send` y mantiene el propietario bloqueado para
  movimiento o acceso exclusivo.
- Un cierre enviado en propiedad a una task o thread debe ser `Send`; no necesita
  `Share`. Sus capturas afines se mueven con él y las `Copy` se copian
  lógicamente. Invocar concurrentemente el mismo callable mediante préstamos
  compartidos exige además `Share + Call[...]`.
- `Ref[T]` es `Send` y `Share` únicamente cuando `T: Send + Share`; compartir la
  identidad y observar `.value` deben ser seguros en el thread receptor. Si
  además `T: Copy`, materializar esa proyección conserva la misma garantía.
- Un contenedor de mutabilidad interior no sincronizado, como un futuro `Cell[T]`, no es `Share`.
- `Mutex[T]`, atomics, canales y actores pueden declarar `Share` mediante implementación intrínseca auditada.
- `Pointer[T]` no cumple `Send` ni `Share`; una región `unsafe` no cambia
  capacidades estáticas.
- `Join[T, E]` no es `Send` ni `Share`, con independencia de `T` y `E`.
- Un préstamo `mut` o `var` nunca es `Share` y no puede cruzar un punto de
  suspensión o frontera de thread. `ref` no cruza una frontera de thread
  independiente; la única excepción concurrente es el hijo ligado a `scope`
  definido en 11.12.

### 16.14 Tasks, threads y canales

Las tasks creadas por `spawn` pertenecen al modelo async estructurado de la sección 11. Pueden ejecutarse en el thread actual o migrar entre workers; el programa no puede depender de una correspondencia entre task y thread.

Los threads del sistema operativo son recursos explícitos de la librería. Para no
depender de variadic generics, una operación conceptual
`Thread.start[F, A, R](operation: F, argument: A)` exige
`F: Send + CallOnce[fn(A): R]`, `A: Send` y `R: Send`. Varios valores se agrupan
en una tuple y la ausencia de datos usa `()`. El payload y el callable se copian
si cumplen `Copy` y se mueven en otro caso; por ello un cierre puede llevar
ownership afín directamente en su entorno sin ocultar sus capacidades. La llamada
devuelve un handle afín y joinable y hace visible cualquier fallo de creación.
Ningún thread se separa silenciosamente de su handle.

Los canales tipados, mutexes, atomics, actores y pools de trabajo pertenecen a la librería estándar, pero deben respetar `Send` y `Share`. Un `Channel[T]` solo transporta `T: Send`; enviar un `T` afín mueve el valor al canal. Tondo 0.1 no introduce un segundo modelo implícito de memoria compartida ni una keyword `select`; una futura operación de selección debe diseñarse de forma exhaustiva y cancelable.

El trabajo bloqueante o intensivo de CPU no debe ejecutarse directamente en un worker async cuando pueda impedir el progreso de otras tasks. La librería ofrecerá una frontera explícita hacia threads o un pool bloqueante.

#### Garantía de progreso cooperativo

Una task está **runnable** cuando no espera una operación externa, un punto de
sincronización ni otra task, y el runtime la ha encolado o despertado. El runtime
no puede perder un wakeup: si una operación esperada completa, la task propietaria
debe quedar runnable exactamente una vez salvo que su scope ya haya terminado.

Mientras el entorno proporcione tiempo de CPU y ninguna ejecución síncrona
monopolice todos los workers sin alcanzar un punto de suspensión, una task que
permanezca runnable debe volver a ejecutar eventualmente. Bajo las mismas
condiciones, completar una operación esperada o un `Join` hace que su continuador
avance eventualmente. Esta es una garantía de ausencia de starvation permanente,
no un límite temporal ni una promesa de orden round-robin.

El lenguaje no fija:

- Cuántos workers existen ni en qué thread ejecuta cada continuación.
- El orden relativo entre tasks runnable.
- Fairness entre contendientes de un canal, mutex, actor o primitiva futura; cada
  API estándar deberá declarar su contrato.
- Progreso cuando el sistema operativo deja de planificar el proceso o una
  dependencia externa no completa.

Un loop de CPU que no suspende puede monopolizar su worker. Si todas las
capacidades de ejecución están monopolizadas de ese modo, Tondo no promete
progreso async. Las APIs bloqueantes de la librería estándar deben usar una
frontera explícita de blocking o threads y no ocupar silenciosamente un worker
async. La cancelación se observa solo en los puntos definidos por 11.14; una vez
alcanzado uno con cancelación pendiente, debe observarse antes de iniciar nuevo
trabajo hijo.

### 16.15 Modelo de memoria concurrente

Un programa escrito íntegramente en Tondo seguro no contiene data races. El
compilador, el runtime y las APIs seguras de la librería deben impedir dos accesos
simultáneos al mismo almacenamiento cuando al menos uno sea escritura y no exista
sincronización intrínseca. Código nativo alcanzado mediante `unsafe` solo conserva
esta garantía si cumple su contrato declarado. Una data race provocada mediante
código unsafe o nativo es comportamiento indefinido según 16.12.

Copy-on-write conserva semántica de valor entre threads: mutar una copia lógica separa su almacenamiento antes de escribir. La contabilidad interna utilizada para compartir buffers debe ser thread-safe cuando esos valores cumplan `Send` o `Share`.

El orden entre tareas concurrentes no está definido salvo por:

- Secuencia dentro de una misma tarea.
- `await` y finalización de joins.
- Operaciones de canales.
- Locks, atomics y otras primitivas con contrato de sincronización.

La librería estándar deberá definir el orden de memoria concreto de sus atomics. El lenguaje no permite inventar sincronización mediante lecturas o escrituras ordinarias.

---

## 17. Operadores

### 17.1 Principio

Los operadores de Tondo tienen significados cerrados definidos por el lenguaje. Un trait o tipo de usuario no puede redefinirlos.

Esto garantiza que:

- `a + b` no ejecuta código encontrado por una búsqueda abierta.
- El significado puede conocerse por los tipos estáticos.
- Los diagnósticos enumeran exactamente las combinaciones permitidas.
- Dos módulos no pueden dar significados distintos al mismo operador.

Los métodos nombrados cubren operaciones de dominio, conversiones, concatenación, orden custom y algoritmos algebraicos.

### 17.2 Operadores postfix

De mayor precedencia:

~~~text
()      llamada
[]      índice, slice o argumentos genéricos según contexto
.       acceso o método
?       propagación
~~~

Ejemplo:

~~~tondo
let name = repository
    .find(id)?
    .displayName()
~~~

El `?` se aplica al valor completo inmediatamente anterior.

`?.` no es un token de navegación segura. Cuando aparece sin espacio, el lexer
produce `?` y `.`: primero se propaga ausencia o error fuera de la función actual
y solo en el camino presente/exitoso continúa el acceso ordinario. Por claridad,
dos propagaciones consecutivas se parentizan como `(value?)?`; la secuencia
adyacente `??` es error léxico y nunca significa coalescencia.

### 17.3 Operaciones prefijas

~~~text
-value     negación de entero con signo o float
not value  negación booleana
~value     complemento bitwise de entero o Byte
await op   esperar una operación async
spawn call iniciar una llamada async en el scope actual
~~~

No existe `!value` como negación; `!` queda reservado en tipos fallibles.

`ref`, `mut` y `var` delante de un argumento no son operadores prefijos
generales. Solo aparecen en las posiciones de parámetro, tipo de función o
argumento definidas en 11.3 y 23.21; fuera de una llamada, `ref value` o
`mut value` no forman una expresión.

`await` y `spawn` no son sobrecargables. Sus restricciones se definen en la sección 11. `scope { ... }` y `unsafe { ... }` son expresiones de bloque introducidas por keywords, no operadores sobre valores.

`-`, `not` y `~` aceptan recursivamente otra expresión unaria. `await` y `spawn`
aceptan en cambio la llamada o el `Join` postfix cerrado definido en 23.19; no se
encadenan como prefijos desnudos. Cuando el operando necesite otra construcción
se utilizan paréntesis. Esta restricción mantiene inequívoco que el `?` escrito
después de una llamada se aplica al resultado ya esperado.

No existen `++` ni `--`. Se utiliza:

~~~tondo
index += 1
index -= 1
~~~

Dos signos `-` adyacentes no se reinterpretan como decremento ni como doble
negación. Para negar dos veces se escribe `-(-value)`; la secuencia léxica `--`
fuera de strings o comentarios es error.

### 17.4 Operadores aritméticos

~~~text
* / %
+ -
~~~

Se aplican a combinaciones numéricas explícitamente compatibles. Los operandos escalares deben tener el mismo tipo salvo que la tabla intrínseca de tipos defina un resultado exacto sin conversión, lo que 0.1 limita al mismo tipo.

Los enteros con y sin signo admiten `+`, `-`, `*`, `/` y `%`. Los floats admiten `+`, `-`, `*` y `/`; un resto o módulo flotante utiliza una operación nombrada porque existen varias convenciones útiles. `Byte` no participa en aritmética ordinaria: se convierte explícitamente a `UInt8` cuando el programa desea tratarlo como número.

También se elevan sobre arrays como se define en 10.6.

`+` no concatena strings, arrays ni maps.

### 17.5 Shifts

~~~text
<< >>
~~~

El operando izquierdo es un entero o `Byte`. El derecho puede ser cualquier entero
intrínseco salvo `Byte`; su valor matemático debe ser no negativo, representable
como `Int` y menor que el ancho del operando izquierdo. Incumplir cualquiera de
esas condiciones produce `P0010 invalid-shift-count`. Esta validación cerrada del
operador no es una conversión numérica implícita y el tipo del operando derecho
no cambia. El resultado conserva exactamente el tipo del operando izquierdo.

Los shifts operan sobre el patrón de ancho fijo: `<<` descarta bits altos y rellena con ceros; no es una operación aritmética comprobada. `>>` sobre enteros con signo replica el bit de signo; sobre enteros sin signo y `Byte` rellena con ceros.

### 17.6 Bitwise y pipelines

~~~text
& ^ |
~~~

`&` y `^` solo se aplican a enteros del mismo tipo o a dos `Byte`. `|` se aplica a enteros del mismo tipo, a dos `Byte` y, de forma cerrada, a los tipos opacos intrínsecos de plan de proceso `Command` y `Pipeline`:

~~~text
Command  | Command  -> Pipeline
Command  | Pipeline -> Pipeline
Pipeline | Command  -> Pipeline
Pipeline | Pipeline -> Pipeline
~~~

Cada operando representa una lista ordenada no vacía de etapas. `|` concatena las
listas y conecta stdout de la última etapa izquierda con stdin de la primera
etapa derecha; conserva el orden interno de ambas. Esto permite componer y
reutilizar subpipelines sin introducir una segunda operación.

`Command` y `Pipeline` forman parte del contrato del lenguaje para evitar que el
compilador reconozca por nombre tipos ordinarios de un módulo. La librería
estándar proporciona sus constructores, configuración y operaciones terminales.
No se aplican operadores bitwise a `Bool`, sets ni enums. Los tipos de usuario no
pueden declarar nuevas combinaciones de pipeline.

La composición copia lógicamente sus planes inertes. `Command` y `Pipeline` nunca poseen recursos one-shot ni obligaciones terminales; esos valores se transfieren al handle de ejecución mediante operaciones explícitas de la librería.

En contexto de tipo, `|` construye una unión. En contexto de valor, los tipos estáticos distinguen bitwise de composición de procesos.

### 17.7 Ranges

~~~text
..  ..=
~~~

Son no asociativos:

~~~tondo
0..10
0..=10
~~~

`a..b..c` es error y requiere una API nombrada para el paso.

### 17.8 Comparación

~~~text
< <= > >=
== !=
in
~~~

Las comparaciones relacionales se definen para:

- Enteros del mismo tipo.
- `Byte` con `Byte`.
- Floats del mismo tipo.
- `Char`.
- `String` mediante orden lexicográfico por valores escalares Unicode.
- Los operadores relacionales no se aplican directamente a newtypes. El programa
  compara explícitamente su `value` o invoca un método/trait de orden nombrado; ese
  método no sobrecarga el operador.

No se encadenan comparaciones:

~~~tondo compile-fail E0005
let value = 5
0 < value < 10 // error
~~~

Se escribe:

~~~tondo
0 < value and value < 10
~~~

`in` tiene la precedencia de comparación.

### 17.9 Operadores booleanos

~~~text
and
or
~~~

Ambos requieren `Bool` y realizan short-circuit:

- `left and right` no evalúa `right` cuando `left` es `false`.
- `left or right` no evalúa `right` cuando `left` es `true`.

No devuelven uno de sus operandos como en algunos lenguajes dinámicos; siempre devuelven `Bool`.

### 17.10 Asignación compuesta

~~~text
= += -= *= /= %= &= ^= |= <<= >>=
~~~

`target op= value` equivale semánticamente a evaluar la ubicación de `target` una sola vez, calcular `old op value` y asignar el resultado.

Esto importa para índices con efectos:

~~~tondo
values[nextIndex()] += 1
~~~

`nextIndex()` se ejecuta exactamente una vez.

La asignación compuesta no existe para `and` y `or`.

Tampoco se aplica a índices de `Map`: una clave ausente necesita una política explícita mediante `getOr` o `entry`.

### 17.11 Precedencia

De mayor a menor:

| Nivel | Operadores | Asociatividad |
|---:|---|---|
| 1 | llamada, índice, slice, acceso, `?` | izquierda/postfix |
| 2 | `-` unario, `not`, `~`, `await`, `spawn` | prefijo; operando restringido para `await`/`spawn` |
| 3 | `* / %` | izquierda |
| 4 | `+ -` | izquierda |
| 5 | `<< >>` | izquierda |
| 6 | `&` | izquierda |
| 7 | `^` | izquierda |
| 8 | `|` bitwise o pipeline | izquierda |
| 9 | `.. ..=` | no asociativo |
| 10 | `< <= > >= in` | no asociativo |
| 11 | `== !=` | no asociativo |
| 12 | `and` | izquierda con short-circuit |
| 13 | `or` | izquierda con short-circuit |
| 14 | `with` de record | izquierda |

Asignaciones y declaraciones no son expresiones y quedan fuera de la tabla.

El formateador añade únicamente los paréntesis necesarios para conservar el AST
según esta tabla y los casos normativos de 15.1 y 23.9. En una cadena de igual
precedencia conserva la asociatividad declarada; ante un operador no asociativo
parentiza cualquier hijo de la misma familia. No existe una decisión subjetiva de
“ambigüedad visual”.

### 17.12 Operaciones deliberadamente nombradas

No tienen operador:

- Concatenación de strings o arrays.
- Repetición de arrays o strings.
- Merge de maps.
- Unión/intersección/diferencia de sets.
- Potencia.
- Producto escalar o matricial.
- Comparación elemento a elemento.
- Coalescencia de options.
- Conversión de tipos.

Se llaman mediante nombres que expresan intención.

---

## 18. Semántica numérica

### 18.1 Enteros

`Int` es exactamente `Int64` en rango:

~~~text
-9_223_372_036_854_775_808
 9_223_372_036_854_775_807
~~~

Los enteros de ancho fijo utilizan complemento a dos para representación, pero las operaciones normales son comprobadas.

### 18.2 Overflow

Suma, resta, multiplicación, negación, división especial del mínimo por `-1` y conversiones deben comprobar rango.

Un resultado fuera del rango produce pánico en todos los modos de compilación.

No existe diferencia semántica entre debug y release:

~~~tondo
let maximum: Int8 = 127i8
let invalid = maximum + 1i8 // pánico siempre
~~~

Las variantes:

- wrapping,
- saturating,
- overflowing,
- checked,

serán operaciones nombradas de tipos numéricos.

### 18.3 División y resto enteros

- División por cero produce pánico.
- El cociente trunca hacia cero.
- El resto tiene el signo del dividendo o es cero.
- Para el entero firmado mínimo y `-1`, la división produce pánico por overflow, pero el resto es `0` porque sí es representable.
- Se cumple `a == (a / b) * b + (a % b)` cuando la división es válida.

Ejemplos:

~~~text
 7 /  3 =  2
-7 /  3 = -2
 7 %  3 =  1
-7 %  3 = -1
~~~

### 18.4 Enteros sin signo

`UIntN` representa `0` hasta `2^N - 1`. Una resta por debajo de cero produce pánico. La negación unaria no está definida para enteros sin signo ni para `Byte`.

`Byte` tiene la representación de `UInt8`, pero es un tipo intrínseco nominal destinado a unidades binarias. La conversión entre `Byte` y `UInt8` es explícita para impedir mezclar accidentalmente contenido binario con aritmética ordinaria.

### 18.5 Floats

`Float32` y `Float64` siguen IEEE 754. `Float` es `Float64`.

Reglas:

- Redondeo por defecto: nearest, ties to even.
- División flotante por cero produce infinito o NaN según IEEE; no pánico.
- Overflow produce infinito.
- El underflow utiliza gradual underflow: produce el subnormal correctamente redondeado o cero con signo cuando así lo exige IEEE 754. Una implementación no puede convertir por conveniencia un subnormal representable en cero.
- `NaN` no es igual a ningún valor, incluido sí mismo.
- Comparaciones relacionales con NaN son falsas.
- `NaN != NaN` es verdadero.
- El signo de cero se conserva donde IEEE lo haga observable.

Una implementación compatible no puede activar “fast math” que cambie resultados observables sin una opción explícita fuera del modo semántico normal.

No puede contraer automáticamente `a * b + c` a FMA si cambia el redondeo. La FMA se solicita mediante operación nombrada.

### 18.6 Conversión numérica

Ejemplos:

~~~tondo
let exact: Int64 = Int64(valueI32)
let checked: Int8 = Int8(valueI64)?
let floating: Float = Float(valueI64)
let integer: Int = Int(valueFloat)?
~~~

Los constructores numéricos forman una tabla intrínseca cerrada; no son
sobrecarga definida por el usuario. Para cada valor origen:

- Entero o `Byte` a entero o `Byte`: devuelve directamente el destino cuando
  todo el rango del tipo origen cabe en él. En otro caso devuelve
  `Destino ! NumericConversionError` y falla con `OutOfRange` solo para los
  valores que no caben. `Byte` y `UInt8` se convierten entre sí de forma total,
  aunque la conversión siga escrita.
- Entero o `Byte` a `Float32`/`Float64`: es total, siempre finita y redondea a
  nearest, ties to even. Todos los enteros intrínsecos tienen magnitud
  representable finitamente por ambos formatos, aunque no siempre con exactitud.
- `Float32` a `Float64`: es total y conserva exactamente todo valor numérico,
  infinidades, signo de cero y la condición NaN.
- `Float64` a `Float32`: devuelve `Float32 ! NumericConversionError`. Infinidades
  y NaN son representables; un valor finito cuya magnitud redondearía a infinito
  falla con `OutOfRange`. Los demás valores redondean a nearest, ties to even,
  incluido underflow gradual a subnormal o cero con signo.
- Float a entero o `Byte`: exige un valor finito, integral y dentro del rango
  destino. Clasifica el fallo como `NotFinite`, después `NotIntegral` y por último
  `OutOfRange`.

Convertir entre dos grafías del mismo tipo, como `Int`/`Int64` o
`Float`/`Float64`, es una identidad explícita que el linter puede señalar como
redundante. El payload concreto de un NaN no forma parte de la semántica y no se
promete preservarlo al cambiar de formato.

Las construcciones de newtype no participan en esta tabla. `UserId(value)` exige
que `value` sea exactamente del tipo subyacente después de expandir aliases; toda
conversión numérica necesaria se escribe antes de cruzar la frontera nominal.

La librería ofrecerá operaciones explícitas `floor`, `ceil`, `round` y `truncate`.

### 18.7 Promoción

No hay promoción numérica implícita:

~~~tondo compile-fail E1102
let integer: Int = 10
let floating: Float = 2.5

let invalid = integer + floating // error
let valid = Float(integer) + floating
~~~

Lo mismo aplica dentro de arrays. `Array[Int] + Float` es error.

### 18.8 Literales negativos

`-42` se analiza como negación unaria aplicada al literal positivo. El compilador permite representar el mínimo de un entero firmado como caso contextual:

~~~tondo
let minimum: Int8 = -128
~~~

sin exigir que `128` exista primero como `Int8`. La misma excepción contextual
se aplica cuando el sufijo fija el tipo, por ejemplo `-128i8`; no permite ninguna
otra magnitud fuera de rango.

---

## 19. Texto y Unicode

### 19.1 `String`

`String` contiene texto Unicode válido almacenado conceptualmente como UTF-8. Nunca contiene secuencias UTF-8 inválidas.

Es:

- Inmutable.
- Con semántica de valor.
- Iterable por `Char`.
- Indexable por posición de valor escalar Unicode.
- Comparable por secuencia de valores escalares.

No es un `Array[Byte]` ni un `Array[Char]`, aunque pueda convertirse explícitamente a representaciones de librería.

### 19.2 Longitud e indexación

La longitud lógica de un string es el número de valores escalares Unicode, no bytes
ni grapheme clusters. Como cualquier longitud materializada, debe caber en `Int` y
sigue la política de límite y agotamiento de recursos de 10.3.

~~~tondo
let text = "añ🙂"

let first: Char = text[0]
let last: Char = text[-1]
~~~

La indexación puede ser O(n) debido a UTF-8. Tondo no promete O(1).

El índice debe tener tipo `Int` y admite las mismas posiciones negativas que `Array`.

Un índice fuera de rango produce pánico. La consulta segura devuelve `Char?` mediante una operación `get`.

### 19.3 Slicing de strings

Los strings usan la misma sintaxis `start:end:step` por posiciones escalares:

~~~tondo
let head = text[:3]
let reversed = text[::-1]
~~~

El resultado es otro `String`. La implementación puede compartir almacenamiento para slices contiguos, pero el comportamiento es el de un valor inmutable.

Los límites y el paso, cuando aparecen, deben tener tipo `Int`. Se aplica
literalmente el algoritmo matemático de normalización de 10.4, incluidos los
defaults distintos según el signo, el centinela de `end` omitido y el
comportamiento de valores extremos como `Int.min`. Los extremos se recortan y un
paso cero produce pánico igual que en arrays.

Un step distinto de uno puede requerir materializar un string nuevo porque UTF-8 no representa una vista de stride fijo sobre valores escalares.

### 19.4 Grapheme clusters

`Char` no equivale necesariamente a un carácter percibido por una persona:

~~~text
"é" puede ser:

U+00E9
o
U+0065 U+0301
~~~

Operaciones por grapheme, palabras, locale y normalización pertenecen a la librería Unicode. El lenguaje nunca normaliza strings automáticamente.

### 19.5 Igualdad y orden

La igualdad compara secuencias exactas de valores escalares. Dos representaciones canónicamente equivalentes pero no normalizadas pueden ser distintas.

El orden relacional es lexicográfico por valores escalares Unicode y no depende de locale. Orden alfabético humano requiere una operación de collation de librería.

### 19.6 Concatenación

`+` no concatena strings.

Una interpolación requiere que la expresión implemente el trait estático
predeclarado `Display`:

~~~tondo
trait Display {
    fn display(self): String
}
~~~

`Display` es un trait estático predeclarado, no una capacidad estructural cerrada:
los módulos pueden implementarlo explícitamente para sus tipos respetando las
reglas ordinarias de coherencia. “Intrínseco” significa aquí que la interpolación
lo consulta por contrato del lenguaje, no que el compilador invente
implementaciones para tipos de usuario.

La resolución es estática. No crea un trait object ni realiza reflection. Los escalares, strings y colecciones intrínsecas implementan `Display` cuando sus componentes también lo implementan. Records, enums y newtypes de usuario deben implementarlo explícitamente si quieren interpolarse como un valor completo.

La interpolación invoca `Display.display` mediante su receptor `self`: observa la expresión durante la llamada y no mueve un valor afín. Si la expresión es un temporal, ese temporal permanece vivo hasta terminar su conversión.

Formas canónicas:

~~~tondo
let message = "{prefix}{name}{suffix}"
let combined = first.concat(second)
~~~

La interpolación es preferida cuando se mezclan texto y valores. El contrato de formato de valores pertenecerá a la librería estándar.

### 19.7 Binario

Texto y binario permanecen separados:

- `String`: Unicode válido.
- `Byte`: unidad binaria.
- `Array[Byte]`: secuencia numérica de bytes.
- `Bytes`: futuro tipo nominal de librería para blobs inmutables.

Decodificar bytes a string puede fallar y debe devolver `String ! TextDecodeError`. Codificar un string a UTF-8 es total.

---

## 20. Programas ejecutables, scripts y procesos

### 20.1 `main` explícito

Un ejecutable ordinario tiene exactamente una función `main` privada en su módulo raíz.

Programa infallible:

~~~tondo
fn main() {
    console.print("Hola")
}
~~~

Programa fallible:

~~~tondo
fn main(): !AppError {
    let config = loadConfig("app.tondo.toml")?
    run(config)?
}
~~~

Error union inline:

~~~tondo
fn main(): !(ConfigError | NetworkError) {
    let config = loadConfig()?
    start(config)?
}
~~~

Un punto de entrada puede ser asíncrono. El runtime crea y conduce el scope raíz sin exponer un executor en la firma:

~~~tondo
async fn main(): !AppError {
    let server = await Server.start()?
    defer Server.stop(server)

    await server.run()?
}
~~~

### 20.2 Restricciones de `main`

`main`:

- No es `pub`.
- No recibe parámetros.
- No es genérica.
- No es un método.
- Devuelve `Unit`.
- Puede declarar un error `E` mediante `: !E`.
- Su error `E` debe cumplir `Discard`; al llegar a la frontera del runtime debe
  ser un dato diagnosticable, no ownership pendiente de cleanup.
- Puede ser `async`, pero no `unsafe`.
- No puede sobrecargarse.

Un target `hosted` ejecutable sin `main` explícito válido ni script raíz produce
`E1806`. Es un diagnóstico de target sin span y utiliza la ubicación nula definida
en 22.3. Si existe una declaración `main` pero incumple cualquiera de estas reglas,
el diagnóstico es `E1803`; la presencia de varias entradas continúa siendo
`E1802`.

Los argumentos del proceso, entorno, entrada y salida se solicitan explícitamente a la librería estándar. No son parámetros mágicos.

### 20.3 Script raíz y `main` implícito

Una herramienta puede compilar el archivo raíz de un target en modo script, por ejemplo mediante `tondo run script.to`. Solo ese archivo permite sentencias ejecutables de nivel superior. El compilador las envuelve, en orden textual, en una función `main` privada implícita.

~~~tondo script spec.process
#!/usr/bin/env tondo

import std.console
import std.process

enum ScriptError {
    MissingPattern
}

let pattern = process.args().getOr(0, "")

if pattern.isEmpty() {
    fail ScriptError.MissingPattern
}

let pipeline = (
    process.cmd("git", "log", "--oneline") |
    process.cmd("grep", pattern)
)

let output = await pipeline.output()?
console.print(output.stdout.text()?)
~~~

En un script raíz:

- `import`, `const`, `type`, `alias`, `enum`, `trait`, `impl` y funciones nombradas siguen siendo declaraciones de módulo.
- `let`, `var`, control de flujo, expresiones y `defer` de nivel superior son sentencias locales del `main` implícito.
- Una función nombrada de módulo no captura `let` o `var` del script; esos bindings solo son visibles para sentencias posteriores del `main` implícito.
- Las sentencias se ejecutan en orden de fuente.
- Un `await` o un `scope` de nivel superior convierte el `main` implícito en
  `async`. Un `scope` sin hijos es válido aunque normalmente redundante; su
  contrato sigue siendo async y no depende de demostrar si contiene trabajo
  concurrente.
- El compilador infiere localmente la unión cerrada de errores propagados por el cuerpo, porque la firma implícita no es una API importable.
- El error inferido debe cumplir `Discard`, igual que en un `main` explícito; una
  obligación terminal no puede convertirse en el diagnóstico final del runtime.
- `return` y `fail` actúan sobre el `main` implícito.
- No puede coexistir una función `main` explícita con sentencias ejecutables top-level.
- Un archivo con sentencias de script no puede importarse como módulo.
- El mismo script puede interpretarse mediante tooling o compilarse como ejecutable sin cambiar semántica.

El scope raíz creado por el runtime administra la terminación, pero no cuenta como `scope` léxico para `spawn`. Un script que lance tareas concurrentes escribe explícitamente `scope { ... }`; un `spawn` directo en nivel superior continúa siendo error.

El shebang `#!...` solo es válido como primera línea de un script raíz. El lexer lo trata como metadato de ejecución y no como código Tondo.

### 20.4 Módulos sin efectos

Fuera del archivo raíz en modo script no hay sentencias top-level. Importar nunca ejecuta código, tampoco cuando el módulo fue utilizado originalmente por otro target como raíz de aplicación.

Por tanto:

- El orden de inicio permanece visible.
- Los tests pueden cargar módulos sin ejecutarlos.
- No existen inicializadores globales ocultos.
- Un script obtiene ergonomía top-level sin introducir efectos de importación.

### 20.5 Comandos como valores

`Command` y `Pipeline` son valores de plan opacos e intrínsecos definidos por el lenguaje. La librería de procesos construye y opera esos valores. Programa y argumentos se proporcionan por separado:

~~~tondo
let command = process.cmd("git", "show", revision)
~~~

Su forma conceptual aprovecha los variádicos homogéneos:

~~~tondo
fn cmd(program: String, args: ...String): Command
~~~

Construir un comando no lo ejecuta. `Command` y `Pipeline` son siempre planes inertes `Copy + Send + Share`, por lo que su configuración persistente solo puede contener datos copiables, compartibles y reutilizables. Un stream, descriptor u otro recurso one-shot se proporciona al iniciar la ejecución y se mueve al handle resultante; no puede quedar oculto dentro del plan. No hay parsing de shell, interpolación textual, globbing ni expansión de variables implícita. Cada argumento conserva exactamente los caracteres del `String` recibido.

La ejecución mediante shell requiere una API nombrada y explícita:

~~~tondo
let command = process.shell(scriptText)
~~~

Un futuro perfil de análisis de taint de la librería puede advertir cuando datos
no confiables lleguen a una llamada a shell. El núcleo 0.1 no inventa confianza a
partir de nombres ni promete un warning sin un modelo de procedencia. La API
concreta pertenece a la librería estándar; la ausencia de shell implícito sí es
una garantía del lenguaje.

### 20.6 Pipelines

`|` conecta stdout de la última etapa del plan izquierdo con stdin de la primera
etapa del plan derecho y produce un `Pipeline` inerte:

~~~tondo
let pipeline = (
    process.cmd("git", "log", "--oneline") |
    process.cmd("grep", pattern) |
    process.cmd("head", "-n", "10")
)
~~~

Solo existen las cuatro combinaciones cerradas de `Command` y `Pipeline`
definidas en 17.6. Los usuarios no pueden sobrecargar `|`, y el operador no
conecta arrays, cursores u otros valores Tondo.

Un pipeline comienza a ejecutar únicamente mediante una operación terminal explícita, conceptualmente:

- `start()`: inicia y devuelve un handle de proceso o pipeline.
- `status()`: espera y devuelve estados de salida.
- `output()`: espera y captura stdout y stderr.
- `run()`: conecta los streams configurados y espera.
- `check()`: espera y convierte estados no satisfactorios en un error nominal.

Las operaciones que esperan I/O deben ofrecer forma async y se consumen con `await`. Las variantes bloqueantes, si existen, deben nombrarse como tales y no bloquear silenciosamente un worker async.

### 20.7 Streams y datos

El enlace `|` transporta bytes desde stdout a stdin. Stderr permanece separado salvo configuración explícita. Ninguna etapa recibe automáticamente strings, líneas, JSON, records o arrays.

Decodificar texto o datos estructurados es visible y puede fallar:

~~~tondo
let output = await pipeline.output()?
let text = output.stdout.text()?
let values = decodeRecords(text)?
~~~

Esta separación preserva fidelidad binaria, evita elegir codecs implícitos y mantiene compatibles los procesos externos de cualquier plataforma.

### 20.8 Inicio, salida y errores de procesos

No poder crear un proceso, perder un stream o fallar al esperar son errores recuperables de la API de proceso. Un código de salida no cero es inicialmente un valor `ExitStatus`, no un pánico ni necesariamente un error Tondo.

Una operación `check()` puede aplicar una política explícita y devolver `!ProcessExitError`. En un pipeline, el resultado conserva el estado de cada etapa; la librería deberá especificar qué políticas de comprobación ofrece sin copiar silenciosamente la regla particular de un shell.

`Command.start()` crea un proceso del sistema y devuelve un handle; la keyword `spawn` queda reservada exclusivamente para crear una task async dentro de `scope`. La separación evita una excepción léxica para nombres de método y hace visible qué clase de concurrencia comienza.

### 20.9 Vida, cancelación y cleanup

Un proceso iniciado es un recurso externo. Su handle en propiedad es afín y debe permitir esperar, cancelar o transferir ownership mediante movimiento explícito. Abandonar un hijo todavía activo sin una operación terminal o un `defer` que garantice cleanup es error de compilación.

Cuando una operación de proceso pertenece a un `scope` async:

- La cancelación del scope solicita detener la espera y aplica la política de cleanup configurada.
- Cerrar pipes y esperar hijos evita zombies y tareas suspendidas permanentemente.
- Un timeout es un error recuperable o resultado nominal visible, no un pánico.
- La terminación forzada nunca se infiere solo por abandonar un binding; debe formar parte del contrato del handle o de una operación nombrada.

Los detalles portables de señales, grupos de procesos, consolas, quoting por plataforma y herencia de file descriptors pertenecen a la especificación de la librería estándar.

### 20.10 Terminación del programa

Las reglas de esta subsección corresponden al perfil ejecutable `hosted` definido
en 20.11.

- Alcanzar el final o devolver `()` termina con éxito.
- Un éxito corresponde al código de proceso `0`.
- Un error no manejado de `main`, explícito o implícito, corresponde a un código de fallo no cero; el valor canónico inicial es `1`.
- El runtime escribe una representación diagnóstica del error en stderr.
- Un pánico utiliza un código de fallo distinto cuando la plataforma lo permita y termina solo después del unwind estructurado.
- Antes de terminar, el scope async raíz cancela y espera cualquier hijo estructurado pendiente.

La representación diagnóstica por defecto incluye al menos el nombre del tipo y sus variantes, pero no constituye una interfaz de usuario estable. No obtiene por reflection acceso a campos privados ni revela contenido opaco; cualquier payload adicional requiere un contrato de formato explícito definido por la librería. Una aplicación que necesite mensajes o códigos concretos debe manejar el error dentro de `main` y utilizar la API de proceso.

### 20.11 Perfiles de host y capacidades de target

Tondo 0.1 define el contrato de entrada y terminación de 20.1–20.10 para el perfil
ejecutable **`hosted`**. Un target hosted proporciona como mínimo:

- Invocación de un `main` explícito o implícito.
- Código de terminación y un canal diagnóstico equivalente a stderr.
- Runtime suficiente para ownership, pánicos, unwind estructurado y async, aunque
  el executor utilice un único thread.

Un target de librería no necesita punto de entrada. Un entorno freestanding o
embebido que no pueda cumplir ese contrato tampoco puede anunciarse como
`hosted`; su forma de entrada, pánico y terminación requiere otro perfil
especificado por separado. WASI, una VM o un entorno embebido sí pueden
implementar `hosted` si ofrecen semántica equivalente, sin necesidad de imitar
internamente un proceso POSIX.

El manifiesto declara el target, su perfil de host y un conjunto cerrado de
capacidades. Los IDs canónicos iniciales son:

~~~text
process
threads
filesystem
network
console
environment
clock
entropy
dynamic-linking
~~~

No se admiten strings de capacidad definidos por cada paquete ni nombres
desconocidos. El manifiesto fija una versión del registro; una revisión posterior
del contrato de target puede añadir IDs de forma versionada sin introducir
sintaxis `.to`, y un toolchain anterior los rechazará en lugar de ignorarlos.
Las capacidades describen al target, no conceden autoridad en runtime ni
sustituyen permisos del sistema operativo.

La presencia de una capacidad afirma que la correspondiente especificación
estándar puede cumplir su contrato completo en ese target; no basta con exponer
stubs que fallen siempre en runtime. La ausencia de una capacidad elimina los
módulos estándar que dependen de ella. Importarlos se rechaza con `E1008`,
incluyendo en el diagnóstico la capacidad ausente.

Las semánticas nucleares, los tipos de valor y async no dependen de `threads`:
un executor monothread es conforme. `Command` y `Pipeline` siguen siendo tipos
intrínsecos de plan, pero sus constructores y operaciones terminales pertenecen a
`std.process` y solo existen con la capacidad `process`. Por tanto, un script que
requiere procesos se rechaza durante resolución o build para un target sin esa
capacidad; no recibe un fallback tardío ni una emulación shell implícita.

No existe compilación condicional dentro de un archivo `.to`. Un paquete aporta
implementaciones distintas mediante los source sets declarados en 6.8. El target,
perfil, capacidades, features y source sets seleccionados son entradas declaradas
del build y forman parte de la identidad de interfaces y artefactos compilados.
Cambiarlos obliga a resolver y comprobar de nuevo el grafo.

---

## 21. Formato canónico y documentación

### 21.1 Formateador normativo

Toda distribución compatible incluye `tondo fmt`. La edición 0.1 tiene un único
perfil, identificado byte a byte como `tondo-format-0.1`; no lee configuración de
proyecto ni preferencias personales. Este capítulo contiene su algoritmo
completo. Un corpus sirve para comprobarlo, pero ninguna herramienta o fichero
externo puede cambiar su resultado.

La entrada es el árbol sintáctico concreto sin pérdida —tokens, spelling de
literales, comentarios, shebang y trivia— conservando como nodos preliminares las
formas contextuales de 23.27. El formateador no consulta resolución de imports,
nombres o tipos, locale, reloj, filesystem ni target. Los nombres intrínsecos que
normaliza están reservados de forma no calificada por 7.10 y por ello no requieren
resolución semántica.

Para toda fuente sintácticamente válida `s`, `F(s)`:

- Es UTF-8 válido, no genera whitespace horizontal al final de una línea
  estructural y termina en exactamente un `LF`. Los bytes que ya pertenecen al
  contenido de un comentario o literal son parte del átomo y pueden incluirlo.
- Vuelve a parsear con una estructura **equivalente para formato** y conserva el
  valor exacto de literales e interpolaciones.
- Es idempotente: `F(F(s))` es byte a byte igual a `F(s)`.
- Produce los mismos bytes en todas las plataformas.

Dos CST son equivalentes para formato cuando, después de eliminar trivia, emiten
el mismo árbol normalizado salvo por las transformaciones enumeradas en 21.2 y
por el orden de imports dentro de los grupos de 21.4. En ese árbol, los
identificadores ya están en NFC, `Option`/`Result` y sus shorthands tienen una
única representación interna, el resultado `Unit` omitido está reinsertado y los
modificadores tienen orden canónico. Esta definición evita afirmar falsamente que
`Option[T]` y `T?` conservan el mismo CST superficial.

`tondo fmt --check` termina con éxito exactamente cuando la entrada ya es
`F(s)`. Una implementación puede ofrecer otro estilo con otro nombre, pero solo
el resultado de este algoritmo puede llamarse formato canónico 0.1.
Si la fuente no alcanza un CST sintácticamente válido, `tondo fmt` emite el
diagnóstico léxico o sintáctico correspondiente, no escribe salida parcial ni
modifica el archivo y termina con fallo.

### 21.2 Normalización y algoritmo de layout

Antes de construir el layout se aplican, en este orden:

1. Cada `CRLF` estructural se normaliza a `LF`. Un `CR` aislado ya fue rechazado
   por 5.1 y no alcanza el formateador.
2. Cada identificador se emite en NFC.
3. Whitespace entre tokens se descarta; su única información conservada es la
   asociación de comentarios y si había al menos una línea vacía entre dos
   unidades, según 21.4.
4. El spelling de números, chars y strings se conserva. Un literal multilínea es
   un átomo indivisible: solo sus finales físicos se normalizan cuando el lexer
   ya los interpreta como el mismo `LF`; nunca se reindenta ni cambia de
   delimitador.
5. Toda forma normalizada equivalente a `Option[T]`, `Result[T, E]` o
   `Result[Unit, E]` no calificados se emite, respectivamente, como `T?`,
   `T ! E` o `!E`. Esto incluye convertir `Unit ! E` en `!E`, con los
   paréntesis normativos de 9.7, 15.1 y 23.9. Si existe un comentario dentro del
   constructor largo o junto a `Unit`, `!` o sus delimitadores, se conserva la
   grafía que mantiene inequívoca su asociación.
6. `: Unit` se omite como resultado infallible de una declaración, cierre o tipo
   de función, salvo que `:` o `Unit` tenga un comentario asociado.
7. Los modificadores se ordenan `pub async unsafe fn`, omitiendo los ausentes.
8. Si la emisión sin trivia uniría dos negaciones `-` como el token inválido `--`,
   se parentiza el operando interior: `- -value` se emite `-(-value)`. Si uniría
   dos propagaciones postfix como `??`, se parentiza la primera:
   `value? ?` se emite `(value?)?`. Un comentario entre operadores conserva su
   asociación y ya impide la adyacencia, por lo que no se elimina ni obliga a
   introducir estos paréntesis.

El árbol se transforma después en un documento con seis primitivas:

- `text(s)`: emite `s` sin decisión de layout.
- `hardline`: emite siempre `LF` y deja pendiente la indentación actual; los
  espacios se materializan solo antes del siguiente `text` no vacío, nunca sobre
  una línea vacía.
- `softline`: emite un espacio en modo plano o `hardline` en modo partido.
- `softzero`: no emite nada en modo plano o emite `hardline` en modo partido.
- `indent(d)`: incrementa en cuatro espacios la indentación de `d`.
- `group(d)`: intenta representar `d` en modo plano como una unidad.

Concatenación e `ifBreak(partido, plano)` son operaciones internas ordinarias del
documento. Al visitar un `group`, el renderer simula su forma plana desde la
columna actual. La simulación sustituye `softline` por un espacio, `softzero` por
vacío, elige la rama plana de `ifBreak` y aplana también grupos anidados. Si
encuentra `hardline` o supera la columna 100, el grupo se parte; en otro caso se
aplana. El empate en columna 100 se aplana. Los grupos se deciden en recorrido
preorder, de fuera hacia dentro, y un grupo interior todavía puede aplanarse
dentro de otro partido.

Un `LF` conservado dentro de un átomo multilínea se comporta como `hardline` para
la simulación y reinicia la columna, aunque los bytes internos del átomo no se
reconstruyen.

La columna es el número de valores escalares Unicode desde el último `LF`; cada
valor cuenta uno, incluidos combining marks y cualquier tab conservado dentro de
un literal o comentario. El formatter nunca genera tabs. Un átomo indivisible,
literal o comentario puede exceder 100 columnas; no hace que el algoritmo sea
dependiente del ancho visual de una terminal.

Ejemplo:

~~~tondo
pub fn loadUser(
    repository: Repository,
    id: UserId,
): User ! RepositoryError {
    repository
        .find(id)?
        .validate()?
}
~~~

### 21.3 Construcción del documento

Estas reglas determinan dónde aparecen las primitivas anteriores:

- Se utilizan cuatro espacios por nivel y nunca punto y coma.
- Entre keywords, nombres y tokens que podrían fusionarse se emite un espacio.
  No hay espacio antes de `(`, `[` o `.`, ni dentro de delimitadores vacíos.
- Todo operador binario de expresión o tipo tiene un espacio a cada lado. En
  expresiones, una cadena maximal de operadores del mismo nivel que siga la
  asociatividad de 17.11 forma un único `group`. En tipos, cada unión normalizada
  `A | B | C` y cada lista de bounds `A + B + C` forma el grupo plano análogo; un
  resultado agrupa una sola vez su éxito, `!` y error. El separador `!` puede
  utilizar el mismo punto de ruptura porque 5.2 suprime el `NL` posterior. Cada
  grupo conserva el primer operando y, por cada tramo, emite
  `" " + operator + indent(softline + operand)`. En modo partido queda una sola
  indentación de cuatro espacios para todos los operandos continuados, sin
  acumular indentación por cada nodo izquierdo del AST. Un hijo cuya agrupación
  no corresponda a esa cadena conserva sus paréntesis.
- Una asignación o `=>` emite el lado izquierdo, un espacio, el operador y
  `indent(softline + right)`. Por tanto se parte después del operador y evalúa
  visualmente el lado derecho bajo una única indentación.
- Una cadena de accesos o llamadas es un `group`. En modo partido cada segmento
  desde el primer `.` comienza en una nueva línea indentada cuatro espacios; el
  punto es el primer carácter no blanco.
- Una llave de bloque abre en la misma línea que su header. Un bloque vacío se
  emite `{}`. Cualquier bloque no vacío utiliza `hardline`, contenido indentado y
  `hardline` antes de `}`; nunca se aplana.
- `} else {` y `} else if ... {` forman una misma línea lógica.
- Declaraciones de nivel superior se separan por exactamente una línea vacía.
  Dentro de un bloque hay un `hardline` entre unidades; si la fuente tenía una o
  más líneas vacías se conserva exactamente una, salvo al inicio o final.
- El shebang se conserva como primer átomo salvo el final `LF`. Si existe otra
  unidad, se emite exactamente una línea vacía entre ambas; si alcanza
  directamente `EOF`, se emite solo su `LF` final.

El spacing entre tokens queda cerrado por esta tabla; “uno” significa un único
U+0020:

| Forma | Espacio canónico |
|---|---|
| `name(...)`, `name[...]`, `value.field`, postfix `?` | Ninguno junto al delimitador u operador. |
| Coma | Ninguno antes; uno después en layout plano. |
| `:` de tipo, field, map o argumento nombrado | Ninguno antes; uno después. |
| `:` de slice | Ninguno a ninguno de sus lados. |
| `...T` y `...value` | Ninguno después de `...`. |
| `!E` de resultado sin éxito | Ninguno entre `!` y `E`. |
| `T ! E`, `A | B`, bounds `A + B` | Uno a ambos lados del operador. |
| Operadores binarios, asignación y `=>` | Uno a ambos lados. |
| `-`, `~` unarios | Ninguno antes del operando. |
| `not`, `await`, `spawn`, y modos de llamada `ref`/`mut`/`var` | Uno después. |
| Keyword seguida de nombre o expresión | Uno cuando el token siguiente no es un delimitador de llamada. |
| `Type { ... }`, headers y llave de bloque | Uno antes de `{`. |
| Delimitadores `()`, `[]` y `{}` vacíos | Ninguno dentro. |

Fuera de bloques, listas y comentarios, cada producción de 23 se envuelve en un
`group` y solo introduce `softline` después de un operador binario, una
asignación o `=>`, y `softzero` antes del punto de una cadena. No se introduce un
salto directamente después de una keyword: en particular, `return expression`,
`if expression`, `for expression` y `match expression` conservan el primer token
de su expresión en la misma línea; esa expresión puede partirse después en sus
propios puntos seguros. Todo salto generado coincide así con una supresión de
`NL` definida en 5.2. No existen otros puntos de salto implícitos.

Arrays, maps, sets, tuples, listas de parámetros y argumentos, argumentos y
parámetros genéricos utilizan el mismo documento de lista:

~~~text
group(
    open
    + indent(softzero + join(items, "," + softline))
    + ifBreak(",", "")
    + softzero
    + close
)
~~~

La coma final solo aparece cuando el grupo se parte. Una lista vacía emite sus
dos delimitadores sin espacio. Un elemento que contenga comentario de línea o
`hardline` fuerza la forma partida.

Los bodies de record, variante con campos, pattern-record y literal record usan
coma y espacio entre fields en modo plano, pero en modo partido utilizan un field
por `hardline` y ninguna coma. Declaraciones de record, enum, trait e `impl`, y
todo `match`, siempre usan la forma partida. Un literal record puede aplanarse si
cabe y no contiene comentarios.

Un header de función agrupa primero su lista de parámetros. Si se parte, cada
parámetro ocupa una línea con coma final y el resultado comienza después del `)`
de cierre. Los headers de `if`, `for` y `match` mantienen el comienzo de su
expresión después de la keyword; si la expresión se parte por un operador o una
cadena, su propia regla aporta la indentación y la llave de apertura permanece en
la línea de su último token. Cada arm de `match` comienza en línea propia; un body
simple permanece tras `=>` si su grupo cabe, y un bloque sigue las reglas de
bloque.

Formato corto:

~~~tondo
let point = Point { x: 10, y: 20 }
let values = [1, 2, 3]
~~~

Formato partido:

~~~tondo
let user = User {
    id: UserId(42)
    name: "Ada"
    email: none
}

let users = [
    UserId(1): first,
    UserId(2): second,
]
~~~

No existe una búsqueda de “mejor” layout: la construcción anterior y el test
plano de 100 columnas eligen una única disposición.

### 21.4 Comentarios

El contenido entre los delimitadores de un comentario se conserva byte a byte,
salvo la normalización estructural de finales de línea. El formatter no refluye
Markdown, URLs ni código documentado.

La asociación se calcula sobre runs de comentarios sin línea vacía interna:

1. `///` se asocia siempre a la declaración inmediatamente siguiente. Cualquier
   línea vacía intermedia se elimina.
2. Un comentario que comparte línea con un token anterior es trailing de la
   unidad sintáctica que contiene ese token.
3. Otro run sin línea vacía antes del token siguiente es leading de ese token.
4. Un run seguido por al menos una línea vacía queda asociado a la unidad anterior
   como comentario de sección. Si no existe unidad anterior, es header de archivo.

Un comentario leading se emite en su propia línea con la indentación de la unidad.
Un `//` trailing se precede por exactamente dos espacios y fuerza `hardline`. Un
`/* ... */` de una línea puede permanecer inline con exactamente un espacio a
cada lado cuando no toca un delimitador; si contiene `LF`, se emite como unidad
partida. La puntuación de cierre propia de la unidad —en particular su coma de
lista— se emite antes del comentario trailing, como muestra el corpus 21.7.
Entre runs se conserva como máximo una línea vacía.

Los imports forman grupos separados por una línea vacía o por un comentario de
sección. Dentro de cada grupo se ordenan por:

1. Secuencia de valores escalares del module path ya normalizado a NFC.
2. Alias normalizado, con ausencia antes que cualquier alias.
3. Posición original como desempate estable.

Los comentarios leading y trailing asociados a un import se mueven con él. Los
headers y comentarios de sección no se mueven, por lo que nunca cambia qué grupo
describen. Después del último grupo de imports hay exactamente una línea vacía
antes de la primera declaración o statement.

### 21.5 Documentación pública

Toda declaración `pub` debe tener:

- Comentario `///`, o
- Nombre y firma considerados autoexplicativos por una política de lint configurable.

El perfil de lint `docs-strict` exige un comentario `///` para toda declaración
`pub` y utiliza `W1010` cuando falta. Fuera de ese perfil, la política que decide
qué nombres son autoexplicativos es una entrada declarada de tooling y no una
propiedad semántica del programa.

La documentación de una función fallible debe explicar:

- Significado del éxito.
- Variantes de error relevantes.
- Pánicos posibles por precondiciones.
- Mutación observable.
- Complejidad no evidente.

Una función `unsafe` pública debe enumerar las obligaciones de alineación,
lifetime, procedencia, aliasing e inicialización que correspondan. Una función
async pública debe documentar cancelación, recursos retenidos y efectos de larga
duración cuando no sean obvios por sus tipos. Una `async unsafe fn` documenta
además cuáles de sus obligaciones deben seguir siendo ciertas a través de cada
suspensión y hasta completar la llamada.

### 21.6 Ejemplos verificables

El runner normativo se invoca como `tondo doc-test --edition 0.1 <markdown>` y
habilita el perfil de lint `core`, sin perfiles opcionales. El Markdown de
entrada debe ser UTF-8 válido; admite terminadores físicos `LF` y `CRLF`, y
rechaza un `CR` aislado. El scanner divide líneas excluyendo su terminador. Para
cada fence une con `LF` las líneas de contenido situadas entre apertura y cierre
y añade exactamente un `LF` final, incluso al cuerpo vacío. Ese byte string es su
`source`; por tanto los finales `CRLF` del contenedor no cambian el programa ni
sus hashes. El header admite únicamente estas formas:

~~~text
tondo
tondo fragment
tondo fragment fixture-name
tondo script
tondo script fixture-name
tondo compile-fail Edddd
tondo compile-fail Edddd Edddd
tondo pseudocode
~~~

El scanner no depende de una versión externa de Markdown: reconoce una apertura
solo cuando una línea sin indentación empieza exactamente por `~~~tondo` y
termina después del header sin whitespace final. La cierra la siguiente línea
exacta `~~~`, también sin indentación ni otros bytes. `fence_byte` apunta al
primer `~` de la apertura como offset de byte cero-based. Otros fences se ignoran
y una apertura Tondo sin cierre hace fallar el documento.

Cuando una forma que admite fixture lo omite, utiliza `spec.0_1`, definido en el
apéndice C. Las categorías `tondo` y `pseudocode` no admiten fixture. Un header
desconocido o un código que no cumpla `E[0-9]{4}` hace fallar el propio
documento. El código debe existir además en el registro 22.2. Un nombre de
fixture cumple `[a-z][a-z0-9_.]*`. La última línea de ejemplo representa uno o
más códigos distintos separados por un único espacio; repetir uno hace fallar el
documento y no existe un máximo semántico.

Las categorías tienen este procedimiento exacto:

- `tondo`: se prueba, en este orden, `module_program`, `syntax_sequence` y
  `standalone_block`. Dentro de cada elemento de `syntax_sequence` se prueba
  `top_decl`, `function_signature`, `statement`, `type_expr_line` y
  `pattern_line`, también en ese orden. Se elige la primera alternativa que
  consume la unidad lógica completa. El bloque entero debe llegar a `EOF`; se
  registra la secuencia de producciones elegida y se ejecuta el formatter sobre
  sus CST. No se resuelven nombres ni se afirma que el snippet sea un programa
  completo.
- `tondo fragment`: se parsea primero con la superficie de `script_program`. Los
  imports y declaraciones permanecen a nivel de módulo; los statements superiores
  se trasladan, sin cambiar su AST, a una función privada con símbolo higiénico
  inaccesible al source. Esa función infiere `async` y su unión cerrada de errores
  con las mismas reglas que el `main` implícito de 20.3, pero no es un entry
  point y nunca se ejecuta. Después se añade el fixture, se resuelve y se
  typecheckea. Si el bloque declara una función `main`, se comprueba como raíz
  ejecutable; en otro caso, como biblioteca. Mezclar `main` explícito y statements
  superiores conserva el error ordinario `E1802`. Cualquier otro error `E` hace
  fallar el bloque.
- `tondo script`: se compila como archivo raíz completo, sin wrapper ni
  declaraciones sintéticas, contra los módulos del fixture elegido. Cualquier
  error `E` hace fallar el bloque.
- `tondo compile-fail`: utiliza el mismo wrapper y fixture que `fragment`. Debe
  llegar a la fase que produce el error pretendido. El conjunto de códigos `E`
  primarios distintos debe ser exactamente el conjunto escrito en el header; que
  compile, que falte uno o que aparezca otro hace fallar el bloque. Notes,
  `related` y warnings no alteran ese conjunto.
- `tondo pseudocode`: no se entrega al lexer, parser, formatter ni compilador.

Todo bloque salvo `pseudocode` que alcance un CST sintácticamente válido registra
también `F(source)`; ante un error léxico o sintáctico esperado,
`formatted_sha256` vale `null`. No se exige que la presentación didáctica ya
estuviera formateada; los pares de 21.7 sí comparan bytes. Los lints `W`
habilitados por el perfil del runner se ejecutan y registran, pero nunca invalidan
un fence; `compile-fail` acepta únicamente códigos `E`.

El resultado del runner es un array JSON ordenado por byte de apertura del fence.
Cada entrada contiene obligatoriamente `file`, `fence_byte`, `category`,
`edition`, `fixture`, `fixture_sha256`, `production`, `source_sha256`,
`formatted_sha256`, `parse_ok`, `typecheck_ok`, `expected_codes` y
`actual_codes`. `expected_codes` es el array lexicográficamente ordenado de
códigos `E` distintos del header, o `[]`; `actual_codes` contiene, también una
sola vez y en orden lexicográfico, todos los códigos primarios `E` y `W`
producidos para el fence. La comparación de `compile-fail` utiliza exclusivamente
el subconjunto `E` de `actual_codes`.

`file` es el nombre lógico de entrada. En la CLI es el operando `<markdown>`
normalizado a NFC y con cada separador de directorio nativo emitido como `/`; no
se convierte en path absoluto, no resuelve symlinks y conserva componentes `.` o
`..` escritos por el invocador. `edition` es exactamente el string `"0.1"`.
`category` es exactamente `syntax`, `fragment`, `script`, `compile-fail` o
`pseudocode`. `production` es un array ordenado de nombres de producción:
`syntax` registra la alternativa y los `syntax_item` elegidos; `fragment` y
`compile-fail` registran `script_program` y `fragment_wrapper`; `script` registra
`script_program`; si el parse necesario falla, y para `pseudocode`, utiliza
`null`. `source_sha256` cubre siempre los bytes `source` preparados al comienzo
de esta sección. En todas las categorías salvo `pseudocode` son además los bytes
entregados al lexer; en `pseudocode` solo se hashean. `parse_ok` es booleano salvo
para `pseudocode`, donde vale
`null`; `typecheck_ok` es booleano solo en las tres categorías que typecheckean y
vale `null` en las demás.

Un fixture solo
añade declaraciones externas: no puede reemplazar texto, cambiar keywords,
habilitar reglas privadas, relajar tipos u ownership, silenciar un diagnóstico
originado en el bloque ni introducir una implementación de trait que cambie una
resolución ya presente. `production`, `formatted_sha256` o `typecheck_ok` valen
`null` cuando la categoría o una fase anterior hacen que no correspondan.
`fixture` y `fixture_sha256` valen `null` para `tondo` y `pseudocode`; las claves
nunca se omiten. Todos los hashes son SHA-256 del byte string indicado,
codificados como 64 dígitos hexadecimales ASCII en minúscula.

Las producciones auxiliares del runner no amplían la gramática de archivos:

~~~ebnf
function_signature
                = [ visibility ], [ function_modifiers ], "fn",
                  function_head, parameter_list,
                  [ decl_outcome_annotation ], NL ;

type_expr_line  = type_expr, NL ;
pattern_line    = pattern, NL ;

syntax_item     = top_decl
                | function_signature
                | statement
                | type_expr_line
                | pattern_line ;

syntax_sequence = { NL }, syntax_item,
                  { NL | syntax_item }, EOF ;

standalone_block
                = { NL }, block, { NL }, EOF ;
~~~

Solo permiten documentar firmas o formas aisladas sin fingir que una declaración
sin body sea válida dentro de un módulo.

### 21.7 Corpus mínimo de formato 0.1

Los siguientes pares forman parte normativa de `tondo-format-0.1`. Cada bloque
termina en un `LF`, incluso cuando Markdown no lo haga visible.

Entrada:

~~~text
fn add( a:Int,b:Int):Int {a+b}
~~~

Salida:

~~~text
fn add(a: Int, b: Int): Int {
    a + b
}
~~~

Entrada:

~~~text
let values=[
1,
2
]
~~~

Salida:

~~~text
let values = [1, 2]
~~~

Entrada:

~~~text
import zeta
import alpha as a
fn main(){}
~~~

Salida:

~~~text
import alpha as a
import zeta

fn main() {}
~~~

Entrada:

~~~text
let values=[1, // first
2]
~~~

Salida:

~~~text
let values = [
    1,  // first
    2,
]
~~~

Entrada:

~~~text
type Loader=fn():Result[Option[Int],IoError]
~~~

Salida:

~~~text
type Loader = fn(): Int? ! IoError
~~~

Entrada:

~~~text
fn make():impl Iterator[Int]+Discard{build()}
~~~

Salida:

~~~text
fn make(): impl Iterator[Int] + Discard {
    build()
}
~~~

Entrada:

~~~text
let inverse=- -value
let nested=value? ?
~~~

Salida:

~~~text
let inverse = -(-value)
let nested = (value?)?
~~~

La suite oficial de una implementación debe incluir estos pares, el caso vacío
de cada lista delimitada de la gramática y, para cada lista no vacía, tres casos
generados cuya forma plana termina respectivamente en columnas 99, 100 y 101. La
salida esperada se obtiene aplicando literalmente el test de anchura de 21.2:
99 y 100 permanecen planos; 101 se parte. Así el límite no depende de un corpus
binario no publicado.

---

## 22. Diagnósticos y herramientas

### 22.1 Principios de diagnóstico

Un diagnóstico debe responder:

1. Qué ocurrió.
2. Dónde ocurrió o, si no existe un span, qué target lo exige.
3. Qué esperaba el compilador.
4. Qué recibió.
5. Qué contrato relacionado causó la expectativa.
6. Qué correcciones locales son seguras.

Mensajes vagos como “type mismatch” sin tipos concretos no son conformes.

### 22.2 Códigos estables

Este es el registro normativo de la edición 0.1. `E` identifica errores de
compilación, `W` warnings estáticos y `P` clases de pánico definidas por el
lenguaje. Un modo estricto puede elevar la severidad de un `W`, pero conserva su
código. Una implementación puede añadir notas y códigos propios con prefijo
distinto; no puede reutilizar estos códigos ni cambiar su condición primaria.

Cuando una construcción viola varias reglas, se elige el código más específico
de la fase más temprana que permita continuar de forma fiable. Los demás hechos
se adjuntan como `related`, no como diagnósticos primarios en cascada. Recuperarse
de un error para encontrar otros no autoriza a inventar tipos o ownership que
oculten un diagnóstico independiente.

#### Léxico y sintaxis

| Código | Nombre estable | Condición primaria |
|---|---|---|
| `E0001` | `invalid-utf8` | Byte no válido en una fuente que debe ser UTF-8. |
| `E0002` | `invalid-token` | Carácter o secuencia sin token léxico. |
| `E0003` | `malformed-literal` | Literal numérico, string, char o interpolación mal formado. |
| `E0004` | `invalid-syntax` | Los tokens no forman la producción requerida. |
| `E0005` | `invalid-operator-chain` | Cadena de operadores prohibida, incluida una comparación encadenada. |
| `E0006` | `invalid-source-form` | Mezcla o selección inválida de forma módulo, script o snippet. |

#### Nombres y módulos

| Código | Nombre estable | Condición primaria |
|---|---|---|
| `E1001` | `unknown-name` | Ningún símbolo visible coincide con el nombre. |
| `E1002` | `duplicate-name` | Dos declaraciones ocupan el mismo namespace y scope. |
| `E1003` | `shadowing` | Una declaración ocultaría un binding todavía visible. |
| `E1004` | `ambiguous-name` | Más de un símbolo visible es candidato y falta calificación. |
| `E1005` | `reserved-name` | Se declara un nombre contextual o intrínseco reservado. |
| `E1006` | `import-cycle` | El grafo de imports contiene un ciclo. |
| `E1007` | `invalid-import-position` | Un import aparece después de otra declaración o statement. |
| `E1008` | `invalid-module-path` | El path no identifica un módulo disponible del target. |

#### Tipos, declaraciones, genéricos y traits

| Código | Nombre estable | Condición primaria |
|---|---|---|
| `E1101` | `missing-type-context` | Inferencia local sin solución única, incluido un literal vacío. |
| `E1102` | `type-mismatch` | Tipo obtenido incompatible con el tipo exacto esperado. |
| `E1103` | `invalid-conversion` | Conversión no permitida o constructor de conversión incompatible. |
| `E1104` | `invalid-generic-arguments` | Aridad, orden o binder genérico inválido. |
| `E1105` | `unsatisfied-bound` | Un tipo no satisface un trait, capacidad o protocolo requerido. |
| `E1106` | `recursive-alias` | Ciclo en aliases transparentes. |
| `E1107` | `nonproductive-type` | Tipo nominal recursivo sin base finita. |
| `E1108` | `invalid-function-coercion` | Un cierre concreto no puede convertirse al `fn(...)` esperado. |
| `E1109` | `uninitialized-binding` | Binding declarado sin inicializador. |
| `E1110` | `trait-is-not-a-value` | Trait usado como tipo de valor fuera de un constraint. |
| `E1111` | `overlapping-impl` | Dos cabeceras de `impl` se unifican según 12.6. |
| `E1112` | `nonterminating-trait-resolution` | El análisis de cambio de tamaño rechaza un ciclo de bounds. |
| `E1113` | `invalid-iterator-implementation` | Un target tendría más de un tipo de elemento `Iterator`. |
| `E1114` | `invalid-impl-contract` | Un `impl` no coincide exactamente con su trait o viola orphan rules. |
| `E1115` | `invalid-declaration` | Declaración semánticamente mal formada sin código más específico. |
| `E1116` | `duplicate-map-key` | Un literal de map repite una clave constante. |
| `E1117` | `invalid-opaque-result` | Un resultado opaco carece de testigo normal o viola `Discard`, unicidad concreta o bounds publicados. |

#### Patrones y control de flujo

| Código | Nombre estable | Condición primaria |
|---|---|---|
| `E1201` | `refutable-binding-pattern` | Patrón refutable usado donde se exige uno irrefutable. |
| `E1202` | `invalid-pattern` | Forma de patrón incompatible con el scrutinee o el contexto. |
| `E1203` | `unreachable-match-arm` | Un arm está cubierto por arms anteriores. |
| `E1204` | `non-exhaustive-match` | El conjunto de arms no cubre todos los casos. |
| `E1205` | `invalid-control-transfer` | `return`, `break`, `continue` o `fail` no tiene destino válido. |
| `E1206` | `invalid-for-source` | Header, patrón o fuente de `for` no admite el modo solicitado. |

#### Option, Result y propagación

| Código | Nombre estable | Condición primaria |
|---|---|---|
| `E1301` | `incompatible-error-propagation` | `?` no puede representar el canal propagado en la firma envolvente. |
| `E1302` | `invalid-fail-context` | `fail` aparece sin error compatible. |
| `E1303` | `discarded-result` | Expresión no `Unit` se abandona sin uso ni `_ =`. |
| `E1304` | `invalid-result-construction` | `ok`, `err`, `some` o `none` carece de contexto único o payload válido. |

#### Ownership, préstamos y cleanup

| Código | Nombre estable | Condición primaria |
|---|---|---|
| `E1401` | `use-after-move` | Se usa un binding no disponible después de un movimiento. |
| `E1402` | `invalid-borrow-lifetime` | Un préstamo escapa, sobrevive a su origen o cruza una frontera prohibida. |
| `E1403` | `overlapping-mutable-borrow` | Solapamiento estáticamente demostrable entre préstamos incompatibles. |
| `E1404` | `terminal-value-not-consumed` | Una obligación terminal alcanza final, `return`, `fail`, `?`, `break` o `continue` sin consumo o guard. |
| `E1405` | `duplicate-assignment-destination` | Destinos que deben ser distintos se solapan estáticamente. |
| `E1406` | `invalid-affine-transfer` | Movimiento parcial, duplicado o handoff afín no confirmable. |
| `E1407` | `invalid-call-mode` | El callable requiere `CallMut` o `CallOnce` y el lvalue/ownership no lo permite. |
| `E1408` | `terminal-overwrite` | Una escritura abandonaría el valor terminal anterior. |
| `E1409` | `discard-requires-capability` | `_ =` o abandono genérico carece de `Discard`. |
| `E1410` | `invalid-defer` | Registro diferido viola aridad afín, sincronía, resultado o consumo único. |
| `E1411` | `invalid-assignment-target` | El lvalue no concede la escritura, preservación de extensión o presencia que exige la asignación. |

#### Visibilidad y API

| Código | Nombre estable | Condición primaria |
|---|---|---|
| `E1501` | `inaccessible-symbol` | Símbolo o campo existe, pero no es visible desde el módulo actual. |
| `E1502` | `invalid-public-construction` | Construcción pública intenta nombrar o fijar estado privado. |
| `E1503` | `private-type-in-public-api` | Firma pública expone un tipo no exportado. |
| `E1504` | `invalid-method-owner` | El módulo no puede declarar el método inherente para ese owner. |
| `E1505` | `member-name-conflict` | Campo, método o variante viola las reglas de unicidad del owner. |

#### Async y concurrencia estructurada

| Código | Nombre estable | Condición primaria |
|---|---|---|
| `E1601` | `async-call-not-awaited` | Llamada async fuera de `await` o `spawn`. |
| `E1602` | `spawn-outside-scope` | `spawn` no pertenece a un `scope` async válido. |
| `E1603` | `join-escapes` | Un `Join` sale de su región propietaria. |
| `E1604` | `unconsumed-join` | Una salida normal conserva un `Join` pendiente. |
| `E1605` | `non-send-transfer` | Valor no `Send` cruza task/thread o queda vivo a través de `await`. |
| `E1606` | `non-share-borrow` | Préstamo concurrente exige `Share` y el origen no lo cumple. |
| `E1607` | `exclusive-borrow-across-await` | Préstamo `mut`/`var` cruza una suspensión prohibida. |
| `E1608` | `invalid-async-cleanup` | `defer` intenta suspender, crear scope o lanzar trabajo. |
| `E1609` | `invalid-async-signature` | Firma async contiene receptor o parámetro exclusivo prohibido. |
| `E1610` | `invalid-async-context` | `await` o `scope` aparece fuera de una función, cierre o script async. |
| `E1611` | `invalid-async-operand` | `await` o `spawn` recibe una forma que no representa la operación permitida. |

#### Unsafe, programas y constantes

| Código | Nombre estable | Condición primaria |
|---|---|---|
| `E1701` | `unsafe-operation-outside-region` | Operación raw invocada sin región `unsafe`. |
| `E1702` | `invalid-unsafe-contract` | Firma, modificadores o captura raw no hace visible el contrato requerido. |
| `E1801` | `script-imported-as-module` | Un archivo con statements se importa como módulo. |
| `E1802` | `duplicate-main` | El target contiene más de un entry point. |
| `E1803` | `invalid-main` | Existe `main`, pero el target, visibilidad, parámetros, modificadores u outcome no lo permiten. |
| `E1804` | `invalid-top-level-statement` | Un módulo contiene ejecución superior. |
| `E1805` | `invalid-shebang` | Shebang ausente de la primera línea o usado fuera de un script raíz. |
| `E1806` | `missing-main` | Un target ejecutable hosted no contiene `main` válido ni script raíz. |
| `E1901` | `nonconstant-expression` | Un `const` usa una operación no evaluable en compilación. |
| `E1902` | `constant-cycle` | Dependencias entre constantes forman un ciclo. |
| `E1903` | `constant-panic` | La evaluación constante produciría error o pánico. |

#### Warnings

| Código | Nombre estable | Condición primaria |
|---|---|---|
| `W1001` | `unused-import` | Import sin referencias. |
| `W1002` | `unused-binding` | Binding local no leído ni consumido. |
| `W1003` | `unused-parameter` | Parámetro nombrado sin uso; `_` lo desactiva. |
| `W1004` | `naming-convention` | Nombre no sigue la convención de 5.4. |
| `W1005` | `confusable-identifier` | Skeleton Unicode confundible en el mismo scope. |
| `W1006` | `unreachable-code` | Statement o expresión posterior no es alcanzable. |
| `W1007` | `redundant-conversion` | Conversión explícita que conserva exactamente el tipo. |
| `W1008` | `known-nan-comparison` | Comparación con un float constante conocido como NaN. |
| `W1009` | `avoidable-cow-copy` | Detach repetido evitable con un préstamo equivalente local. |
| `W1010` | `incomplete-public-documentation` | Documentación pública omite un contrato exigido por 21.5. |
| `W1011` | `duplicate-set-entry` | Literal de set repite una clave constante. |

#### Pánicos del lenguaje

| Código | Nombre estable | Condición runtime |
|---|---|---|
| `P0001` | `bounds` | Índice o límite directo fuera del rango permitido. |
| `P0002` | `zero-slice-step` | Slice con paso cero. |
| `P0003` | `integer-division-by-zero` | División o resto entero por cero. |
| `P0004` | `overlapping-borrow` | Check dinámico de regiones o transferencia atómica detecta solapamiento. |
| `P0005` | `checked-overflow` | Aritmética entera representable excedida. |
| `P0006` | `array-shape-mismatch` | Operación vectorizada recibe longitudes incompatibles. |
| `P0007` | `assertion-failed` | `assert` recibe `false`. |
| `P0008` | `explicit-panic` | Invocación explícita de `panic`. |
| `P0009` | `duplicate-dynamic-map-key` | Literal de map con valor terminal produce claves dinámicas repetidas. |
| `P0010` | `invalid-shift-count` | Conteo de shift negativo, no representable como `Int` o mayor o igual que el ancho izquierdo. |

Un runtime incluye el código `P` y el nombre estable en su diagnóstico. OOM
irrecuperable y los aborts fuera del modelo de 15.7 no reciben un código `P`.
Nuevas ediciones menores pueden añadir códigos libres, pero nunca renumerar ni
reutilizar uno existente.

### 22.3 Salida estructurada

Además de texto humano, el compilador ofrece JSON:

~~~json
{
  "id": "diag:657cc6f1f65d18bda1f1c6e81b157a903c56abfccf55086e3884e23ac14b9da4",
  "severity": "error",
  "code": "E1102",
  "message": "expected Int, found Int32",
  "source_id": "pkg:example/app",
  "module": "app",
  "file": "src/main.to",
  "range": {
    "start": { "byte": 318, "line": 12, "column": 18 },
    "end": { "byte": 323, "line": 12, "column": 23 }
  },
  "expected": "Int",
  "actual": "Int32",
  "related": [
    {
      "message": "parameter declared here",
      "source_id": "pkg:example/app",
      "module": "app.models",
      "file": "src/user.to",
      "range": {
        "start": { "byte": 86, "line": 4, "column": 22 },
        "end": { "byte": 89, "line": 4, "column": 25 }
      }
    }
  ],
  "fixes": [
    {
      "title": "convert Int32 to Int",
      "applicability": "safe",
      "edits": [
        {
          "source_id": "pkg:example/app",
          "module": "app",
          "file": "src/main.to",
          "range": {
            "start": { "byte": 318, "line": 12, "column": 18 },
            "end": { "byte": 323, "line": 12, "column": 23 }
          },
          "replacement": "Int(value)"
        }
      ]
    }
  ]
}
~~~

Cada diagnóstico primario contiene exactamente las claves `id`, `severity`,
`code`, `message`, `source_id`, `module`, `file`, `range`, `expected`, `actual`,
`related` y `fixes`. `severity` es `error` o `warning`; `expected` y `actual` son
un string canónico de tipo o nombre, definido abajo, o `null`; `related` y
`fixes` son arrays, también cuando están vacíos. Una entrada `related` contiene
exactamente `message`, `source_id`, `module`, `file` y `range`.

`source_id` identifica sin colisiones al propietario lógico del diagnóstico. Para
un archivo de paquete deriva uno a uno de su `PackageId`; dos `PackageId`
distintos producen siempre IDs distintos. Para una raíz suelta deriva de la
identidad lógica declarada por la invocación. Para una condición de target utiliza
la identidad estable de ese target. El formato concreto es opaco al lenguaje,
pero el grafo de build debe proporcionar un string UTF-8 canónico, estable con las
mismas entradas y sin `LF`. Paquetes, raíces sueltas y targets comparten un único
namespace de IDs y nunca pueden colisionar entre categorías. El valor
`pkg:example/app` del ejemplo es ilustrativo.

Un stream de diagnósticos corresponde a exactamente un target, perfil, conjunto
de capacidades, features y grafo resuelto. Una herramienta que compruebe varios
targets emite un stream independiente para cada identidad de build; no mezcla sus
IDs en un único namespace de ejecución.

Una ubicación de fuente, primaria o `related`, contiene `module`, `file` y
`range` no nulos. Una condición que no pertenece a ningún span —por ejemplo
`E1806` cuando falta `main`— utiliza el `source_id` del target y contiene las tres
claves con valor `null`. No se permite ninguna combinación parcial. Una
reparación siempre apunta a fuente y, por tanto, nunca utiliza esos valores nulos.

`id` es `diag:` seguido del SHA-256 hexadecimal minúsculo de la concatenación
UTF-8 `edition LF source_id LF module_or_empty LF file_or_empty LF code LF
start_or_empty LF end_or_empty LF`. En la edición actual, `edition` contiene
exactamente los tres bytes ASCII `0.1`. En una ubicación de fuente, los bytes de
inicio y fin son sus enteros decimales sin signo, sin ceros iniciales salvo `0`;
en una ubicación nula, los cuatro componentes anulables se representan mediante
strings vacíos. `module` y `file` son las cadenas canónicas de 22.6. Los
componentes del hash no pueden contener `LF`.

El compilador fusiona dos diagnósticos con la misma edición, `source_id`,
ubicación primaria y código, uniendo sin duplicados sus `related` y `fixes`;
por tanto el ID es único dentro de una ejecución y permanece estable mientras esa
identidad de fuente o target no cambie. El hash mostrado arriba corresponde
exactamente al ejemplo.

Cada posición contiene siempre `byte`, un offset cero-based sobre los bytes
originales del archivo. En fuente UTF-8 válida contiene además `line` y `column`,
también cero-based; la columna cuenta valores escalares Unicode y los rangos son
semiabiertos `[start, end)`. Ante un byte UTF-8 inválido, `byte` continúa siendo
exacto y `line`/`column` pueden omitirse desde la primera posición que ya no pueda
decodificarse inequívocamente. El formato humano puede mostrar línea y columna
one-based, y un adaptador de editor puede convertirlas a UTF-16 sin cambiar el
protocolo del compilador.

El string canónico de tipo o nombre es una serialización de tooling, no spelling
local de la fuente:

- Expande aliases transparentes y utiliza `Int` y `Float` para sus sinónimos
  intrínsecos. Newtypes, records y enums conservan identidad nominal.
- Un átomo nominal se escribe
  `@<bytes>:<source_id>::<module>::<namespace>::<declaration>`, donde `<bytes>` es
  la longitud decimal UTF-8 de `source_id`; módulo, namespace y path de
  declaración están en NFC. El prefijo de longitud hace inequívoco cualquier
  contenido opaco de `source_id` y el namespace conserva la identidad completa de
  6.7. El namespace se serializa exactamente como `type`, `value` o `module`.
- Los parámetros genéricos se escriben `$0`, `$1`, etc. por posición en el
  ambiente completo de binders, del exterior al interior. Aplicación, tuples,
  funciones, modos de parámetro, option y result utilizan la sintaxis canónica de
  tipos de esta edición de forma recursiva.
- Una unión se aplana, elimina duplicados y ordena sus miembros por bytes UTF-8 de
  esta misma serialización. `Never` se elimina como exige 8.9.
- Un resultado opaco utiliza el átomo nominal de su declaración seguido de
  `#result`; nunca revela el tipo concreto oculto.
- Un tipo anónimo generado por el lenguaje utiliza
  `generated["<kind>","<source_id>","<module>","<file>",<start_byte>]`, con los
  cuatro strings escapados como JSON y argumentos de tipo canónicos cuando
  existan. `<kind>` es `closure`, `unsafe-closure`, `async-closure` o
  `async-unsafe-closure`; la ubicación es el nodo de fuente que lo crea. Los
  cursores intrínsecos sin identidad de fuente se escriben como
  `cursor[own,<collection_type>]` o `cursor[ref,<collection_type>]` según el modo
  de 10.17.
- Keywords, nombres intrínsecos y códigos se escriben sin calificación. Los demás
  nombres de símbolo utilizan el mismo átomo nominal.

Así `Int` permanece legible, mientras dos `User` de paquetes distintos nunca
producen el mismo valor en `expected` o `actual`. El `message` humano puede usar
el spelling local; el tooling compara las claves estructuradas, no analiza ese
mensaje.

### 22.4 Fixes

Una reparación contiene exactamente `title`, `applicability` y `edits`.
`applicability` es `safe` o `requires-decision`. `edits` es un array no vacío;
cada entrada contiene exactamente `source_id`, `module`, `file`, `range` y
`replacement`. Todos sus rangos son de fuente, semiabiertos, no se solapan dentro
del mismo archivo y se aplican atómicamente sobre el snapshot inmutable que
produjo el diagnóstico. Si ese snapshot ya no es el actual, el cliente rechaza la
reparación en vez de intentar reubicarla heurísticamente. `replacement` es UTF-8
y puede contener `LF`, por lo que una reparación puede abarcar varias líneas.

Una reparación `safe` significa que, sobre ese snapshot exacto, el compilador ha
comprobado que aplicar todos sus edits:

- produce fuente sintácticamente válida;
- elimina el diagnóstico objetivo sin introducir ningún diagnóstico `E`;
- no cambia la interfaz pública;
- no descarta un resultado ni inserta `panic`; y
- no elige entre comportamientos de dominio plausibles.

Si no puede demostrar todas esas condiciones, la reparación se marca
`requires-decision`. Una reparación puede tocar varios archivos relacionados
cuando el cambio es indivisible, pero nunca incluye ediciones ajenas a su
propósito declarado.

### 22.5 Consultas semánticas

El tooling debe poder consultar de forma estructurada:

- Tipo de una expresión.
- Símbolo al que resuelve un nombre.
- Referencias de una declaración.
- Miembros de un enum o unión.
- Conjunto cerrado de errores de una llamada.
- Préstamos activos.
- Checks dinámicos normativos de solapamiento o duplicados que permanecen después
  del análisis, y la prueba estática que permitió eliminar cada check ausente.
- Origen, región y fin estructurado de cada parámetro o binding `ref`.
- Modo de ownership de cada `match` —copia, observación o consumo— y el patrón que
  obliga a elegirlo.
- Scope propietario y estado de consumo de cada `Join`.
- Capacidades `Copy`, `Discard`, `Equatable`, `Key`, `Send` y `Share` derivadas.
- Firma y protocolos `Call`, `CallMut` o `CallOnce` derivados para cada cierre,
  junto con las capacidades de su entorno sin exponer valores privados.
- ID nominal y bounds publicados de cada resultado `impl Bound`, sin revelar su
  tipo concreto fuera del módulo propietario.
- Tipo concreto de cursor y elemento único demostrado por `Iterator[T]`.
- Superficie pública opaca de esas capacidades y de igualdad/hash cuando las
  determina un campo privado, sin revelar su representación.
- Constructibilidad externa de cada record público, sin revelar qué campo privado
  la impide.
- Presencia y origen estructural de una obligación terminal.
- Estado disponible, movido, reservado o consumido de cada valor afín.
- Regiones `unsafe` y operación que exige cada una.
- Expansión de azúcar como `T?`, `T ! E` y dot-call.
- AST formateada.

### 22.6 Builds deterministas

Con las mismas fuentes, versión de compilador, edición, target, perfil,
capacidades, features, source sets, flags semánticos y lockfile:

- Resolución de módulos es idéntica.
- Los mismos aliases resuelven a los mismos `PackageId` exactos y hashes de
  contenido.
- Orden de diagnósticos es estable.
- El resultado observable no depende de orden de archivos.
- No se consulta red ni entorno sin una entrada declarada por tooling.

El **module path canónico** une con `.` los identificadores NFC que derivan del
path de módulo. El **file path lógico canónico** es relativo a la raíz del
paquete, utiliza `/`, no contiene componentes vacíos, `.` ni `..`, y normaliza
cada componente Unicode a NFC. El grafo de build rechaza dos entradas físicas que
produzcan el mismo path lógico. Fuera de un paquete, la invocación declara una
raíz lógica y aplica la misma regla. `source_id`, módulo y archivo lógicos, no los
paths físicos dependientes de plataforma, aparecen en diagnósticos, orden e IDs.

Los diagnósticos primarios se ordenan por `source_id`, módulo, archivo, byte
inicial, byte final, severidad —`error` antes de `warning`—, código y mensaje,
todos en orden ascendente por bytes UTF-8 o enteros según corresponda. Los nulos
se ordenan antes que cualquier string o entero. Las entradas `related` permanecen
anidadas y se ordenan por `source_id`, módulo, archivo, byte inicial, byte final y
mensaje.

Dentro de una reparación, los edits se ordenan por `source_id`, módulo, archivo,
byte inicial, byte final y `replacement`. Las reparaciones se ordenan por
`applicability` —`safe` antes de `requires-decision`—, título y, por último, la
secuencia ya ordenada de sus edits. El formato humano y el JSON utilizan esos
mismos órdenes.

La interfaz compilada registra al menos versión de formato, compilador, edición,
target, perfil, capacidades, features, `PackageId`, hash de API y hashes de
dependencias. Un artefacto registra además los source sets, hashes de fuente y
todos los inputs declarados utilizados por generadores. Esos metadatos sirven
para rechazar mezclas incompatibles; no crean la ABI binaria estable que 8.13
excluye.

La reproducibilidad bit a bit del artefacto es un objetivo de implementación,
aunque metadatos de plataforma pueden requerir normalización adicional.

### 22.7 Modo LLM

Una implementación puede ofrecer una vista compacta para agentes, pero no cambia el lenguaje. Debe incluir:

- Diagnósticos JSON.
- Firma completa de símbolos.
- Contexto mínimo de tipos relacionados.
- Patches aplicables.
- IDs estables.
- Sin logs irrelevantes ni colores ANSI.

La optimización para LLMs se basa en información explícita, no en sintaxis secreta ni prompts incrustados.

### 22.8 Cobertura diagnóstica y perfiles

Toda condición `E` del registro 22.2 es obligatoria cuando aparece en un programa;
la lista siguiente destaca comprobaciones que una implementación no puede
degradar a lint. Además, toda distribución ofrece un perfil de warnings `core`
con condiciones cerradas:

- Import no utilizado (`W1001`).
- Binding local no utilizado (`W1002`).
- Parámetro no utilizado que no se haya escrito como descarte `_` (`W1003`).
- Nombre que no sigue la convención canónica (`W1004`).
- Identificadores visualmente confundibles según 5.4 (`W1005`).
- Código estructuralmente inalcanzable tras una expresión `Never` o una
  transferencia de control (`W1006`).
- Conversión explícita redundante (`W1007`).
- Comparación con un float constante evaluado como NaN (`W1008`).
- Literal `Set[...]` con una entrada constante repetida (`W1011`).

Son errores obligatorios, entre otros:

- Rama `match` cubierta por anteriores (`E1203`).
- `match` no exhaustivo (`E1204`).
- Resultado no `Unit` descartado (`E1303`).
- Declaración que produciría shadowing (`E1003`).
- Cabeceras de `impl` que se solapan (`E1111`) o un ciclo de obligaciones que no
  supera la terminación por cambio de tamaño (`E1112`).
- Más de una implementación `Iterator[T]` para el mismo target (`E1113`).
- Literal de map con una clave constante repetida (`E1116`).
- Llamada async sin `await` o `spawn` (`E1601`).
- `Join` que intenta escapar (`E1603`) o alcanza una salida normal sin consumirse
  (`E1604`).
- Uso de un binding después de mover un valor afín (`E1401`).
- Intento de almacenar, capturar, devolver o prolongar un préstamo fuera de su
  región estructurada (`E1402`).
- Préstamos incompatibles con solapamiento estático (`E1403`); un solapamiento
  dependiente de datos se comprueba en runtime como `P0004`.
- Recurso con obligación terminal que abandona su scope sin consumo o `defer`
  (`E1404`).
- Descarte genérico que carece de `Discard` (`E1409`).
- Sobrescritura de un valor terminal (`E1408`).
- Invocación que no satisface el modo `Call`, `CallMut` o `CallOnce` requerido
  (`E1407`).
- Destino de asignación que no concede el permiso o la clase de escritura
  requeridos (`E1411`).
- Operación raw fuera de `unsafe` (`E1701`) o captura raw cuyo contrato no queda
  visible (`E1702`).
- Valor o callee que no cumple `Send` en una transferencia o suspensión
  (`E1605`) y préstamo concurrente que no cumple `Share` (`E1606`).
- Intento de importar un archivo que contiene sentencias de script (`E1801`).

Dos perfiles opcionales completan el registro sin volver heurística la
conformidad del lenguaje:

- `performance` puede emitir `W1009` solo cuando adjunta una prueba semántica de
  que el mismo owner realiza un detach en una arista de retorno de loop y una
  única reborrow local evitaría la copia conservando orden, aliasing, pánicos y
  efectos. No se exige que dos optimizadores descubran el mismo conjunto.
- `docs-strict` emite `W1010` para toda declaración `pub` sin comentario `///`.
  La calidad del contenido contractual exigido por 21.5 sigue siendo revisable,
  no se adivina mediante búsqueda de palabras.

Los defectos de tipos, exhaustividad, préstamos, visibilidad, shadowing, uso de
`unsafe`, vidas estructuradas y descarte de resultados son errores. Los asuntos
de estilo, documentación o rendimiento identificados con `W` son warnings y no
cambian semántica. La selección de perfiles forma parte de los flags declarados
del target. Un modo estricto puede promoverlos, pero conserva el código `W`.

### 22.9 Conformidad ejecutable

La conformidad completa de la edición 0.1 requiere una suite versionada denominada
`tondo-conformance-0.1`. Los ejemplos y doc-tests de esta especificación ayudan a
detectar inconsistencias editoriales, pero no sustituyen esa suite. Este Markdown
define su contrato, no afirma que el artefacto ya exista: una publicación final de
la edición debe distribuir la suite y una release de conformidad publica:

- Un manifiesto con versión de suite, edición y hashes de todos los casos.
- El target, perfil de host y capacidades exigidos por cada grupo.
- Comandos de compilación y ejecución con entradas declaradas.
- Un resultado estructurado JSON por caso, incluidos exit status, diagnóstico
  esperado y observaciones permitidas.

La suite se divide como mínimo en:

1. **Lexing, parsing y formato:** casos válidos, inválidos, recuperación y
   resultados byte a byte de `tondo fmt`.
2. **Compile-pass y compile-fail:** resolución, tipos, traits, exhaustividad,
   ownership, préstamos, capacidades, async, unsafe y códigos diagnósticos.
3. **Consultas semánticas:** schema JSON, IDs, spans, reparaciones y datos
   obligatorios de 22.5.
4. **Runtime:** orden de evaluación, overflow, bounds, slicing, copy-on-write,
   arrays, maps, sets, defer, pánico, unwind, movimiento y obligaciones
   terminales; mediante un adaptador de runtime también comprueba reachability,
   preservación de roots, ciclos inalcanzables y el reintento previo a OOM.
5. **Concurrencia:** litmus tests de memoria y progreso que enumeran resultados
   permitidos y prohibidos, sin exigir un orden de scheduling ni tiempos de
   pared concretos.
6. **Hosted:** entrada, salida, código de terminación, `main` async y cleanup del
   scope raíz. Los casos de procesos solo se aplican cuando existe la capacidad
   `process`.

Un compilador que anuncie **conformidad completa Tondo 0.1** para un target debe
pasar todos los grupos aplicables al perfil y capacidades declarados. Una
implementación solo de frontend, formatter o análisis puede certificar esos
componentes por separado, pero no presentarse como implementación completa del
lenguaje. Una capacidad no soportada permite omitir sus casos operativos, pero
obliga a pasar los casos que comprueban su rechazo temprano según 20.11.

Los tests concurrentes no observan una planificación concreta. Cada caso define
un conjunto de resultados permitidos, barreras de comienzo y una condición de
progreso; un resultado fuera del conjunto o un wakeup perdido es fallo. Los
timeouts son únicamente límites del runner y deben ser suficientemente holgados,
calibrados y repetidos para distinguir una violación del lenguaje de una máquina
saturada.

Las estrategias de memoria distintas de ARC pueden ser conformes si conservan la
semántica observable. Para comprobar el contrato común, cada runtime conecta un
adaptador de test que puede crear nodos administrados con aristas fuertes,
mantener o retirar roots, solicitar presión de asignación y observar si el
payload continúa vivo. Los casos comprueban que un root nunca se reclama, que un
ciclo sin roots se recupera bajo presión y que se intenta una pasada completa
antes de OOM. El adaptador usa el allocator y collector reales y no es accesible
desde fuente Tondo.

La implementación de referencia dispone además de una suite propia que comprueba
contadores atómicos, upgrade linealizable de `WeakRef`, liberación iterativa y
trial deletion. Sus hooks para forzar una pasada de ciclos solo existen en builds
de test del runtime: no forman parte del lenguaje ni pueden cambiar la semántica
del programa probado.

Ningún modo oculto de conformidad puede relajar checks, cambiar overflow,
scheduling observable o sustituir una ruta real por un resultado prefabricado.
Una implementación puede instrumentar ejecución, pero debe ejecutar el mismo
programa y contrato público que distribuiría para ese target.

---

## 23. Gramática de referencia

### 23.1 Alcance

Esta sección ofrece una gramática EBNF de referencia para Tondo 0.1. Las reglas semánticas de las secciones anteriores prevalecen sobre ambigüedades puramente sintácticas.

Convenciones:

~~~text
"token"    token literal
name       referencia a otra producción
[ item ]   cero o una aparición
{ item }   cero o más apariciones
item | alt una alternativa
( item )   agrupación
NL         un token de nueva línea significativa
EOF        final de archivo
~~~

Las comas finales se permiten donde la producción contiene listas separadas por coma.

El lexer colapsa una o más nuevas líneas físicas consecutivas que no hayan sido
suprimidas por 5.2 en un único token `NL`, conservando el trivia para formatter y
documentación. También inserta el `NL` sintético de 5.1 antes de `EOF`; por eso las
producciones no contienen una alternativa especial para terminar una declaración
directamente en `EOF`.

El token `...` forma parte del lenguaje únicamente en parámetros variádicos y argumentos spread. Los ejemplos de esta especificación no lo utilizan como elipsis metatextual.

### 23.2 Tokens léxicos

~~~ebnf
letter          = Unicode_XID_Start | "_" ;
letter_or_digit = Unicode_XID_Continue | "_" ;

identifier      = letter, { letter_or_digit } ;

field_name      = identifier
                | "alias" | "and" | "as" | "async" | "await"
                | "break" | "const" | "continue" | "defer" | "else"
                | "enum" | "err" | "fail" | "false" | "fn"
                | "for" | "if" | "impl" | "import" | "in"
                | "let" | "match" | "mut" | "none" | "not"
                | "ok" | "or" | "priv" | "pub" | "ref"
                | "return" | "scope" | "self" | "some" | "spawn"
                | "trait" | "true" | "type" | "unsafe" | "var"
                | "with" ;

decimal_digit   = "0"…"9" ;
nonzero_decimal_digit
                = "1"…"9" ;
binary_digit    = "0" | "1" ;
octal_digit     = "0"…"7" ;
hex_digit       = decimal_digit | "a"…"f" | "A"…"F" ;

decimal_digits  = decimal_digit, { [ "_" ], decimal_digit } ;
decimal_numeral = "0"
                | nonzero_decimal_digit,
                  { [ "_" ], decimal_digit } ;
binary_digits   = binary_digit, { [ "_" ], binary_digit } ;
octal_digits    = octal_digit, { [ "_" ], octal_digit } ;
hex_digits      = hex_digit, { [ "_" ], hex_digit } ;

integer_suffix  = "i8" | "i16" | "i32" | "i64"
                | "u8" | "u16" | "u32" | "u64" ;

integer_literal = decimal_numeral, [ integer_suffix ]
                | "0b", binary_digits, [ integer_suffix ]
                | "0o", octal_digits, [ integer_suffix ]
                | "0x", hex_digits, [ integer_suffix ] ;

exponent        = ( "e" | "E" ), [ "+" | "-" ], decimal_digits ;
float_suffix    = "f32" | "f64" ;

float_literal   = decimal_numeral, ".", decimal_digits,
                  [ exponent ], [ float_suffix ]
                | decimal_numeral, exponent, [ float_suffix ] ;

bool_literal    = "true" | "false" ;
none_literal    = "none" ;
unit_literal    = "(", ")" ;
~~~

El lexer reconoce primero la secuencia XID maximal y después, si su spelling NFC
coincide exactamente con 5.5, emite el token keyword correspondiente en lugar de
`identifier`. Por tanto `identifier` en las producciones sintácticas excluye
siempre palabras reservadas.

Las alternativas keyword de `field_name` solo son válidas cuando el nombre
introduce o resuelve realmente un field. La alternativa `identifier` continúa cubriendo
también métodos y otros miembros ordinarios después de `.`. Una keyword no puede
aprovechar esta producción para nombrar otro tipo de símbolo.

La gramática interna de chars y strings se rige por 5.9 y 5.10. El lexer produce tokens:

~~~text
CHAR_LITERAL
STRING_LITERAL
RAW_STRING_LITERAL
MULTILINE_STRING_LITERAL
RAW_MULTILINE_STRING_LITERAL
~~~

Los comentarios se eliminan antes del parser, conservando posiciones y asociación de documentación.

Tras reconocer la forma más larga de un literal numérico, un
`Unicode_XID_Continue` o `_` adyacente que no pertenezca a sus dígitos o a un
sufijo válido convierte toda la secuencia en error léxico. La misma comprobación
rechaza un segundo dígito decimal después del `0` único de `decimal_numeral`;
formas como `01` no se dividen en dos literales.

`STRING_LITERAL` y `MULTILINE_STRING_LITERAL` pueden transportar segmentos interpolados con sus expresiones ya delimitadas; no son texto opaco para el análisis semántico. Las variantes `RAW_...` nunca contienen interpolación. Esta estructura interna se rige por 5.10 y no cambia las producciones exteriores de expresiones.

El lexer utiliza maximal munch para operadores válidos. En particular reconoce
`<<=` y `>>=` antes de `<<`/`>>`, `...` antes de `..` y `.`, `..=` antes de
`..`, y cada asignación o comparación de dos caracteres antes de su prefijo de un
carácter. Reconoce comentarios antes de tratar `/` como operador y los prefijos
`r"`/`r"""` antes de separar un identificador `r` de un literal.

Las secuencias adyacentes `--` y `??` fuera de comentarios o strings se rechazan
según 17.2 y 17.3; no se dividen en dos operadores. `?.` sí se divide
deliberadamente en propagación `?` seguida de acceso `.` y nunca es safe
navigation.

En un script raíz el lexer puede reconocer además una línea shebang que comience
por `#!` exclusivamente en el primer byte del archivo; esa línea se descarta antes
del parser.

### 23.3 Separadores

El lexer, no el parser, aplica el algoritmo completo de 5.2 y emite los tokens
`NL`. Dentro de `()` y `[]` nunca los emite mientras uno de ellos sea el
delimitador abierto más interior; dentro de `{}` sí los emite salvo continuación
explícita por los tokens enumerados en 5.2, incluso si ese body está anidado en
otros delimitadores. El parser consume `NL` para separar declaraciones, fields,
variants, arms y sentencias según las producciones siguientes.

### 23.4 Programa

~~~ebnf
program         = module_program | script_program ;

module_program  = { NL | import_decl },
                  { NL | top_decl }, EOF ;

script_program  = { NL | import_decl },
                  { NL | top_decl | script_statement }, EOF ;

script_statement
                = statement ;

top_decl        = const_decl
                | type_decl
                | alias_decl
                | enum_decl
                | trait_decl
                | impl_decl
                | function_decl ;

visibility      = "pub" ;
~~~

La herramienta selecciona `module_program` o `script_program` antes del análisis semántico según el target raíz. Un archivo importado siempre se valida como `module_program`.

### 23.5 Imports

~~~ebnf
import_decl     = "import", module_path, [ "as", identifier ], NL ;
module_path     = identifier, { ".", identifier } ;
~~~

### 23.6 Constantes

~~~ebnf
const_decl      = [ visibility ], "const", identifier,
                  [ ":", type_expr ], "=", expression, NL ;
~~~

Una constante privada sin anotación debe tener tipo completamente inferible desde una expresión constante. Una constante `pub` exige anotación de tipo.

### 23.7 Parámetros genéricos

~~~ebnf
generic_params  = "[", generic_param,
                  { ",", generic_param }, [ "," ], "]" ;

generic_param   = identifier, [ ":", generic_bound ] ;
generic_bound   = type_path, { "+", type_path } ;

generic_args    = "[", type_expr,
                  { ",", type_expr }, [ "," ], "]" ;
~~~

Cada elemento de `generic_bound` debe resolver a un trait, una capacidad
intrínseca cerrada como `Copy`, `Discard`, `Equatable`, `Key`, `Send` o `Share`,
o un protocolo cerrado `Call[S]`, `CallMut[S]` o `CallOnce[S]` cuya `S` sea un
`function_type`.

### 23.8 Paths

~~~ebnf
type_path       = identifier, { ".", identifier },
                  [ generic_args ] ;

value_path      = identifier, { ".", identifier } ;
~~~

El resolver distingue módulos, tipos, variantes y valores por scope y posición.

### 23.9 Expresiones de tipo

~~~ebnf
type_expr       = union_type ;

union_type      = result_type, { "|", result_type } ;

result_type     = "!", error_type_operand
                | optional_type,
                  [ "!", error_type_operand ] ;

error_type_operand
                = optional_type
                | "(", union_type, ")" ;

optional_type   = primary_type, [ "?" ] ;

primary_type    = type_path
                | tuple_type
                | function_type
                | "(", type_expr, ")" ;

tuple_type      = "(", type_expr, ",", type_expr,
                  { ",", type_expr }, [ "," ], ")" ;

function_type   = [ function_modifiers ], "fn", "(",
                  [ function_type_list ], ")",
                  [ outcome_annotation ] ;

function_modifiers
                = "async", [ "unsafe" ]
                | "unsafe" ;

parameter_modifier
                = "ref" | "mut" | "var" ;

receiver_modifier
                = "mut" | "var" ;

function_type_list
                = function_type_item,
                  { ",", function_type_item }, [ "," ] ;

function_type_item
                = [ parameter_modifier ], type_expr
                | "...", type_expr ;

type_list       = type_expr, { ",", type_expr }, [ "," ] ;

outcome_annotation
                = ":", type_expr ;

decl_outcome_annotation
                = ":", ( type_expr | opaque_outcome ) ;

opaque_outcome  = "impl", generic_bound,
                  [ "!", error_type_operand ] ;
~~~

Restricciones contextuales:

- `T?` equivale a `Option[T]`.
- Solo se permite un sufijo `?` sin paréntesis. Una option anidada se escribe `(T?)?`.
- `T ! E` equivale a `Result[T, E]`.
- `!E` equivale a `Result[Unit, E]`.
- Una unión como error debe agruparse: `T ! (E1 | E2)`.
- Una función infallible que devuelve `Unit` omite la anotación completa. Cualquier otro resultado se introduce con `:`.
- `opaque_outcome` se limita semánticamente a las declaraciones enumeradas en
  12.8. Su `generic_bound` debe demostrar `Discard`; el `!` opcional pertenece al
  canal exterior de la función y no al bound.
- En `function_type_list`, `...T` puede aparecer como máximo una vez y únicamente al final.
- `ref T`, `mut T` y `var T` forman parte del tipo de función y no pueden
  combinarse con `...T`.
- El modificador de préstamo aplica al `type_expr` completo que le sigue. El
  formatter parentiza un result o unión superior: `ref (A | B)`,
  `mut (A | B)` y `var (T ! E)`.

### 23.10 Tipos y aliases

~~~ebnf
type_decl       = [ visibility ], "type", identifier,
                  [ generic_params ], "=",
                  ( record_body | type_expr ), declaration_end ;

alias_decl      = [ visibility ], "alias", identifier,
                  [ generic_params ], "=", type_expr, declaration_end ;

record_body     = "{", { NL },
                  [ record_field,
                    { field_separator, record_field },
                    [ field_separator ] ],
                  { NL }, "}" ;

record_field    = [ "priv" ], field_name, ":", type_expr ;

field_separator = ",", { NL }
                | NL, { NL } ;

declaration_end = NL ;
~~~

`type Name = type_expr` crea un newtype. `alias Name = type_expr` crea un alias transparente.

Dentro de `record_body`, `priv` es modificador solo cuando va seguido por otro
`field_name` y `:`. Si va seguido inmediatamente por `:`, es el propio nombre del
campo. Por tanto un campo público llamado `priv` se escribe `priv: Bool` y uno
privado con ese mismo nombre se escribe `priv priv: Bool`.

### 23.11 Enums

~~~ebnf
enum_decl       = [ visibility ], "enum", identifier,
                  [ generic_params ], "{",
                  { NL },
                  enum_variant,
                  { field_separator, enum_variant },
                  [ field_separator ],
                  { NL }, "}", declaration_end ;

enum_variant    = identifier,
                  [ tuple_payload | record_body ] ;

tuple_payload   = "(", type_list, ")" ;
~~~

El `record_body` de una variante no admite el modificador `priv`, debe contener al
menos un campo y hereda la visibilidad del enum. Un field llamado `priv` continúa
siendo válido como `priv: Type` según la excepción contextual. Una variante sin
payload se escribe sin delimitadores; `Variant()` y `Variant {}` no declaran
formas alternativas de unit variant.

### 23.12 Traits e implementaciones

~~~ebnf
trait_decl      = [ visibility ], "trait", identifier,
                  [ generic_params ], "{",
                  { NL | trait_method }, "}", declaration_end ;

trait_method    = [ function_modifiers ], "fn", identifier,
                  [ generic_params ], parameter_list,
                  [ outcome_annotation ],
                  ( NL | block, NL ) ;

impl_decl       = "impl", [ generic_params ],
                  type_path, "for", type_expr, "{",
                  { NL | implementation_method },
                  "}", declaration_end ;

implementation_method
                = [ function_modifiers ], "fn", identifier,
                  [ generic_params ], parameter_list,
                  [ outcome_annotation ], block, NL ;
~~~

Un método de trait puede anteponer `async`, `unsafe` o la combinación canónica
`async unsafe` a `fn`; la implementación debe coincidir exactamente. El primer
parámetro puede ser `self`, `mut self` o `var self`.

En una llamada calificada a una operación de trait sin receptor, el primer argumento genérico después del nombre del método selecciona `Self`; no forma parte de los `generic_params` declarados por el método.

### 23.13 Funciones y métodos

~~~ebnf
function_decl   = [ visibility ], [ function_modifiers ],
                  "fn", function_head, parameter_list,
                  [ decl_outcome_annotation ], block, declaration_end ;

function_head   = identifier, [ generic_params ]
                | method_owner, ".", identifier,
                  [ generic_params ] ;

method_owner    = identifier, [ generic_params ] ;

parameter_list  = "(", [ parameter,
                  { ",", parameter }, [ "," ] ], ")" ;

parameter       = identifier, ":", [ parameter_modifier ], type_expr
                | identifier, ":", "...", type_expr
                | "self"
                | receiver_modifier, "self" ;
~~~

Una firma no puede expresar dos `!` superiores. El tipo completo después de `:` puede ser un éxito ordinario, `T ! E` o el shorthand `!E`.

Una firma `async`, también cuando sea `async unsafe`, no puede contener parámetros
ni receptores `mut` o `var`; cada parámetro `ref T` exige `T: Send`, y un receptor
`self` exige `Self: Send`. Ambos adquieren además `Share` al lanzarse mediante
`spawn`. `unsafe` no relaja esas condiciones. Un parámetro `...T` debe ser el
último, no puede tener `parameter_modifier` y solo puede aparecer una vez. El
identificador `_` sigue las reglas de descarte o préstamo sin binding de 11.3.

En un `method_owner`, los parámetros genéricos son binders del tipo propietario. Los parámetros genéricos después del nombre del método pertenecen solo al método. `self`, `mut self` y `var self` únicamente son válidos dentro de un método inherente, de trait o de implementación.
Puede existir como máximo un receptor y, cuando existe, es obligatoriamente el
primer parámetro. `ref self` no existe porque `self` ya es el receptor compartido.

### 23.14 Bloques y sentencias

~~~ebnf
block           = "{", { NL | statement }, [ tail_expression ],
                  { NL }, "}" ;

statement       = binding_decl, statement_end
                | assignment, statement_end
                | return_stmt, statement_end
                | fail_stmt, statement_end
                | break_stmt, statement_end
                | continue_stmt, statement_end
                | defer_stmt, statement_end
                | for_stmt, statement_end
                | expression_stmt, statement_end ;

statement_end   = NL ;

binding_decl    = ( "let" | "var" ), irrefutable_pattern,
                  [ ":", type_expr ], [ "=", expression ] ;

return_stmt     = "return", [ expression ] ;
fail_stmt       = "fail", expression ;
break_stmt      = "break" ;
continue_stmt   = "continue" ;

defer_stmt      = "defer", ( postfix_expression | block ) ;

expression_stmt = expression ;
tail_expression = expression ;
~~~

La distinción entre `expression_stmt` y `tail_expression` es posicional, no
dependiente de tipos: una expresión seguida únicamente por cero o más `NL` y la
`}` del bloque es siempre el tail. El parser da prioridad a esa interpretación en
la EBNF anterior. Una expresión no `Unit` anterior requiere consumo o descarte
explícito, y un tail incompatible produce error de tipos.

La alternativa sin `= expression` se conserva únicamente para que el parser
pueda producir el diagnóstico específico `E1109`; nunca forma un binding válido
ni introduce un nombre en el scope.

Fuera de la forma `defer { ... }`, el `postfix_expression` debe terminar en un `call_suffix`; diferir un valor, un acceso de campo o un método sin llamarlo es error semántico.

### 23.15 Asignación

~~~ebnf
assignment      = assignment_pattern, assignment_op, expression ;

assignment_op   = "=" | "+=" | "-=" | "*=" | "/=" | "%="
                | "&=" | "^=" | "|=" | "<<=" | ">>=" ;

assignment_pattern
                = lvalue
                | "_"
                | tuple_assignment_pattern ;

tuple_assignment_pattern
                = "(", assignment_pattern, ",",
                  assignment_pattern,
                  { ",", assignment_pattern }, [ "," ], ")" ;

lvalue          = place_root, { place_suffix } ;

place_root      = identifier | "self" ;
place_suffix    = ".", ( field_name | tuple_slot )
                | index_suffix ;
~~~

Una llamada, un `?`, un literal o cualquier otro temporal nunca es un lvalue. La
validez mutable de la raíz y de cada proyección se comprueba semánticamente; que
una forma sea una ruta de lugar no concede por sí solo permiso de escritura.

### 23.16 `for`

~~~ebnf
for_stmt        = "for",
                  ( block
                  | irrefutable_pattern, "in", expression, block
                  | expression, block ) ;
~~~

Resolución:

- `for { ... }`: infinito.
- Si aparece `in` al nivel del header: iteración.
- En otro caso: la expresión debe ser `Bool`.

### 23.17 Expresiones condicionales y match

~~~ebnf
if_expression   = "if", expression, block,
                  [ "else", ( if_expression | block ) ] ;

match_expression
                = "match", expression, "{",
                  { NL | match_arm }, "}" ;

match_arm       = pattern, [ "if", expression ], "=>",
                  ( expression | block | control_transfer ),
                  match_arm_end ;

match_arm_end   = NL | "," ;

control_transfer
                = return_stmt
                | fail_stmt
                | break_stmt
                | continue_stmt ;
~~~

Una rama de una sola expresión permanece tras `=>` cuando su `group` cabe en 100
columnas y no contiene `hardline`; en otro caso se parte según 21.3. Los arms
ocupan siempre líneas distintas. Una línea vacía original entre arms se conserva
como una única línea vacía; el formatter no añade ninguna por criterio subjetivo.

La alternativa con coma existe para código compacto o migrado: según 5.2, la
nueva línea física posterior a `,` queda suprimida y es la propia coma la que
termina el arm. El formatter canónico emite un arm por línea y omite esa coma.

### 23.18 Cierres

~~~ebnf
closure_expression
                = [ function_modifiers ], closure_parameter_list,
                  [ outcome_annotation ], block ;

closure_parameter_list
                = "(", [ closure_parameter,
                  { ",", closure_parameter }, [ "," ] ], ")" ;

closure_parameter
                = identifier, [ ":", [ parameter_modifier ], type_expr ]
                | identifier, ":", "...", type_expr ;
~~~

En cierres, una anotación de parámetro puede omitirse cuando hay tipo esperado.
Un parámetro `ref`, `mut` o `var` siempre conserva su anotación completa. Un
parámetro `...T` debe ser único, final, nombrado y no puede tener
`parameter_modifier`; si el tipo esperado ya contiene esa posición variádica, el
cierre puede escribir solo su nombre. `_` puede conservar un modificador
explícito con las reglas de 11.3, pero nunca ser variádico. El parser reconoce una
`closure_parameter_list` cuando una lista entre paréntesis aparece seguida de una
anotación de resultado opcional y un bloque. `fn` no introduce cierres.

### 23.19 Jerarquía de expresiones

~~~ebnf
expression      = if_expression
                | match_expression
                | closure_expression
                | with_expression ;

with_expression = logical_or_expression,
                  { "with", record_update_body } ;

logical_or_expression
                = logical_and_expression,
                  { "or", logical_and_expression } ;

logical_and_expression
                = equality_expression,
                  { "and", equality_expression } ;

equality_expression
                = comparison_expression,
                  [ ( "==" | "!=" ), comparison_expression ] ;

comparison_expression
                = range_expression,
                  [ ( "<" | "<=" | ">" | ">=" | "in" ),
                    range_expression ] ;

range_expression
                = bitwise_or_expression,
                  [ ( ".." | "..=" ), bitwise_or_expression ] ;

bitwise_or_expression
                = bitwise_xor_expression,
                  { "|", bitwise_xor_expression } ;

bitwise_xor_expression
                = bitwise_and_expression,
                  { "^", bitwise_and_expression } ;

bitwise_and_expression
                = shift_expression,
                  { "&", shift_expression } ;

shift_expression
                = additive_expression,
                  { ( "<<" | ">>" ), additive_expression } ;

additive_expression
                = multiplicative_expression,
                  { ( "+" | "-" ), multiplicative_expression } ;

multiplicative_expression
                = unary_expression,
                  { ( "*" | "/" | "%" ), unary_expression } ;

unary_expression
                = ( "-" | "not" | "~" ), unary_expression
                | postfix_expression ;

postfix_expression
                = await_expression,
                  [ "?", { postfix_suffix } ]
                | spawn_expression
                | primary_expression, { postfix_suffix } ;

await_expression
                = "await", plain_postfix_expression ;

spawn_expression
                = "spawn", plain_postfix_expression ;

plain_postfix_expression
                = primary_expression, { plain_postfix_suffix } ;

postfix_suffix  = call_suffix
                | index_suffix
                | generic_args
                | ".", ( field_name | tuple_slot )
                | "?" ;

plain_postfix_suffix
                = call_suffix
                | index_suffix
                | generic_args
                | ".", ( field_name | tuple_slot ) ;

tuple_slot      = "0"
                | nonzero_decimal_digit, { decimal_digit } ;
~~~

Un tuple slot se comprueba además contra la aridad estática; ceros iniciales como
`.01` no son otra grafía de `.1`.

`plain_postfix_expression` excluye deliberadamente `?` y se toma de forma
maximal: todos los accesos, índices, especializaciones y llamadas contiguos forman
parte del operando de `await` o `spawn`. Por ello `await operation()?` aplica
propagación al resultado esperado y el `?` marca una frontera tras la cual pueden
continuar otros postfix:

~~~tondo
let id = await fetchUser()?.id
~~~

Sin esa frontera, un postfix sobre el valor ya esperado se parentiza:

~~~tondo
let id = (await fetchUser()).id
~~~

Así `await factory().fetch()` espera la llamada async formada por la cadena
completa, mientras `(await factory()).fetch()` espera primero `factory` y llama
después al método del resultado. Un `?` dentro de argumentos o de una expresión
agrupada conserva su significado local. `spawn` no admite postfix exterior
directo; el `Join` se enlaza o se parentiza antes de cualquier operación
posterior.

Las alternativas `index_suffix` y `generic_args` se solapan deliberadamente para formas como `name[Item]`. El parser conserva un nodo de corchetes preliminar; la resolución lo clasifica como argumentos de tipo solo cuando el receptor nombra una declaración genérica y la continuación forma una especialización, acceso calificado o llamada válida. En cualquier otro caso se trata como índice o slice. Una ambigüedad que sobreviva a la resolución de namespaces es error y nunca se decide desde un valor de runtime.

`with` es una keyword reservada y se reconoce sin consultar el tipo de la expresión izquierda. El type checker exige después que esa expresión sea un record.

### 23.20 Primarias

~~~ebnf
primary_expression
                = literal
                | "self"
                | value_path
                | tuple_or_group
                | bracket_literal
                | set_literal
                | record_literal
                | option_result_constructor
                | scope_expression
                | unsafe_expression
                | block ;

scope_expression = "scope", block ;
unsafe_expression = "unsafe", block ;

literal         = integer_literal
                | float_literal
                | bool_literal
                | CHAR_LITERAL
                | STRING_LITERAL
                | RAW_STRING_LITERAL
                | MULTILINE_STRING_LITERAL
                | RAW_MULTILINE_STRING_LITERAL
                | none_literal
                | unit_literal ;

tuple_or_group  = "(", expression,
                  [ ",", expression,
                    { ",", expression }, [ "," ] ], ")" ;
~~~

### 23.21 Llamadas

~~~ebnf
call_suffix     = "(", [ call_argument,
                  { ",", call_argument }, [ "," ] ], ")" ;

call_argument   = [ identifier, ":" ], "...", expression
                | [ identifier, ":" ], [ parameter_modifier ], expression ;
~~~

Restricciones:

- `ref` debe corresponder a un parámetro `ref`. Acepta un lvalue o un temporal
  poseído hasta terminar la llamada; con `spawn`, solo un lvalue estable.
- `mut` solo puede preceder una expresión lvalue pasada a un parámetro `mut`.
- `var` solo puede preceder un lvalue completo y reemplazable pasado a un parámetro `var`; una región o slice no lo es.
- Los argumentos nombrados deben aparecer después de todos los posicionales.
- Un argumento `...array` solo puede corresponder al parámetro variádico, debe ser
  el último y no puede combinarse con `parameter_modifier`.
- Un spread nombrado debe utilizar exactamente el nombre del parámetro variádico y es la única forma de proporcionar un variádico después de argumentos fijos nombrados.
- Una llamada genérica ordinaria se reconoce cuando un path de función recibe `generic_args` inmediatamente antes del `call_suffix`. En una llamada de trait calificada, un grupo anterior puede pertenecer al trait —`Codec[Json].decode[User](bytes)`— y el grupo final comienza por el implementador `Self`.

### 23.22 Índices y slices

~~~ebnf
index_suffix    = "[", ( slice_spec | expression ), "]" ;

slice_spec      = [ expression ], ":",
                  [ expression ],
                  [ ":", expression ] ;
~~~

La presencia de `:` al nivel superior del corchete distingue slice de índice.
Si aparece el segundo `:`, el paso es obligatorio; `values[::]` y
`values[start:end:]` no crean grafías redundantes de un slice con paso `1`.

### 23.23 Literales de array y map

~~~ebnf
bracket_literal = "[",
                  ( "]"
                  | ":", "]"
                  | expression, bracket_literal_tail ) ;

bracket_literal_tail
                = ":", expression,
                  { ",", expression, ":", expression },
                  [ "," ], "]"
                | { ",", expression }, [ "," ], "]" ;
~~~

Resolución:

- `[]`: array vacío.
- `[:]`: map vacío.
- Si el primer elemento contiene `:` de entrada: map.
- En otro caso: array.

### 23.24 Set

~~~ebnf
set_literal     = CONTEXT_SET, "[",
                  [ expression, { ",", expression }, [ "," ] ],
                  "]" ;
~~~

`CONTEXT_SET` es el identificador intrínseco exacto `Set` utilizado en posición
de expresión antes de `[`. No es una keyword y no puede redeclararse como nombre
no calificado porque pertenece al prelude.

### 23.25 Records y variantes

~~~ebnf
record_literal  = type_path, "{",
                  { NL },
                  [ record_initializer,
                    { field_separator, record_initializer },
                    [ field_separator ] ],
                  { NL }, "}" ;

record_initializer
                = field_name,
                  [ ":", expression ] ;

record_update_body
                = "{", { NL },
                  record_update,
                  { field_separator, record_update },
                  [ field_separator ],
                  { NL }, "}" ;

record_update   = field_name, ":", expression ;

option_result_constructor
                = "some", "(", expression, ")"
                | "ok", "(", expression, ")"
                | "err", "(", expression, ")" ;
~~~

Una variante enum sin payload es un `value_path`. Una variante con payload utiliza un path seguido de llamada o record literal.

### 23.26 Patrones

~~~ebnf
pattern         = wildcard_pattern
                | unit_pattern
                | literal_pattern
                | option_result_pattern
                | tuple_pattern
                | array_pattern
                | constructor_pattern
                | record_pattern
                | qualified_value_pattern
                | borrow_binding_pattern
                | binding_pattern ;

irrefutable_pattern
                = pattern ;

wildcard_pattern = "_" ;
unit_pattern    = "(", ")" ;
binding_pattern = identifier ;
borrow_binding_pattern
                = "ref", identifier ;

qualified_value_pattern
                = identifier, ".", identifier,
                  { ".", identifier } ;

literal_pattern = integer_literal
                | "-", integer_literal
                | float_literal
                | "-", float_literal
                | bool_literal
                | CHAR_LITERAL
                | STRING_LITERAL
                | RAW_STRING_LITERAL
                | MULTILINE_STRING_LITERAL
                | RAW_MULTILINE_STRING_LITERAL ;

option_result_pattern
                = "some", "(", pattern, ")"
                | "none"
                | "ok", "(", pattern, ")"
                | "err", "(", pattern, ")" ;

tuple_pattern   = "(", pattern, ",", pattern,
                  { ",", pattern }, [ "," ], ")" ;

array_pattern   = "[",
                  [ pattern,
                    { ",", pattern },
                    [ ",", "..", [ array_rest_binding ] ] ],
                  [ "," ], "]" ;

array_rest_binding
                = binding_pattern
                | borrow_binding_pattern ;

constructor_pattern
                = type_path, "(", pattern,
                  { ",", pattern }, [ "," ], ")" ;

record_pattern  = type_path, "{", { NL },
                  [ record_pattern_item,
                    { field_separator, record_pattern_item },
                    [ field_separator ] ],
                  { NL }, "}" ;

record_pattern_field
                = field_name, [ ":", pattern ]
                | "ref", identifier ;

record_pattern_item
                = record_pattern_field | ".." ;
~~~

`irrefutable_pattern` comparte la gramática general de patrones. Su
irrefutabilidad se comprueba semánticamente: bindings, wildcards, `()`, newtypes y
desestructuración completa de tuples o records son válidos; variantes, literales,
cualquier patrón de array, uniones, options y results no lo son. Un binding
`ref` es irrefutable, pero solo se admite en un arm de `match` o header de `for`;
`let` y `var` no crean préstamos locales de vida abierta.

`unit_pattern` coincide con el único valor de `Unit`. Un `qualified_value_pattern` debe resolver a una variante de enum sin payload; las variantes de usuario siempre permanecen calificadas.

Un `constructor_pattern` puede resolver a una variante con payload, un newtype o el discriminador de un miembro de unión. En el último caso, `type_path` debe coincidir exactamente con uno de los miembros normalizados y los paréntesis contienen exactamente un patrón aplicado al valor completo.

`..` puede aparecer como máximo una vez en un patrón record y debe ser su último item.

En un record pattern, `ref field` es la forma corta de
`field: ref field`. En un array pattern, `..ref rest` presta la región restante;
`..rest` la enlaza por valor y conserva las reglas de consumo de 14.3.
El resto solo aparece después de al menos un patrón fijo: `[..rest]` sería
idéntico al binding `rest`, y `[..]` sería idéntico a `_`, por lo que ambas
grafías redundantes se rechazan.

En un record pattern, `ref:` comienza en cambio un field llamado `ref` y debe ir
seguido por su patrón explícito. El lookahead de `:` distingue esta forma de
`ref field`.

### 23.27 Ambigüedades contextuales

El parser puede construir una AST preliminar antes de resolución para:

- `name[items]`: argumentos genéricos —incluidos tipos compuestos— o indexación.
- `Type { ... }`: record literal o variante record.
- `Type(value)` en patrón: variante, newtype o discriminación de unión.
- `(parameters) { ... }`: cierre o expresión agrupada seguida de un bloque inválido.
- `async (parameters) { ... }`: cierre asíncrono.
- `async unsafe (parameters) { ... }`: cierre asíncrono con contrato unsafe.
- `unsafe (parameters) { ... }`: cierre unsafe; `unsafe { ... }` sigue siendo una
  región y no un cierre.
- `for header`: patrón iterador o condición.

El parser conserva nodos preliminares para esas formas y la resolución estática determina su categoría. Ninguna decisión léxica o de estructura sintáctica requiere conocer el tipo de una expresión; `with` dejó de ser ambiguo al convertirse en keyword. Las decisiones restantes nunca dependen de valores de runtime.

---

## 24. Ejemplos integrados

Los ejemplos de este capítulo marcados `tondo fragment` se comprueban con un contexto mínimo que declara los tipos y funciones de dominio omitidos. Las sentencias locales de nivel superior se envuelven en una función privada sintética; imports, visibilidad y resolución conservan exactamente las reglas normales. Un bloque `tondo script` representa en cambio un archivo raíz completo.

### 24.1 Modelo de dominio y errores nominales

~~~tondo fragment spec.domain
import std.fs
import std.json

pub type UserId = Int

pub type User = {
    id: UserId
    name: String
    email: String?
    priv passwordHash: String
}

pub enum UserLoadError {
    Io(fs.IoError)
    Decode(json.DecodeError)
    InvalidName(String)
}

fn validateName(name: String): String ! UserLoadError {
    if name.isEmpty() {
        fail UserLoadError.InvalidName(name)
    }

    name
}

pub fn loadUser(path: fs.Path): User ! UserLoadError {
    let bytes = match fs.read(path) {
        ok(bytes) => bytes
        err(error) => fail UserLoadError.Io(error)
    }

    let user = match decodeUser(bytes) {
        ok(user) => user
        err(error) => fail UserLoadError.Decode(error)
    }

    _ = validateName(user.name)?
    user
}
~~~

Este ejemplo muestra:

- Newtype nominal.
- Record con campo privado.
- Error público nominal.
- Conversión explícita de errores internos.
- `?` después de convertir al error de API.
- Retorno de éxito sin `ok(...)`.

### 24.2 Unión de errores interna

~~~tondo fragment spec.settings
fn readSettings(path: Path): Settings ! (IoError | DecodeError) {
    let bytes = fs.read(path)?
    decodeSettings(bytes)?
}
~~~

`IoError` y `DecodeError` se inyectan automáticamente porque son miembros exactos de la unión.

### 24.3 Option y búsqueda

~~~tondo fragment spec.user
fn findUser(users: Array[User], id: UserId): User? {
    for user in users {
        if user.id == id {
            return user
        }
    }

    none
}

fn displayUser(users: Array[User], id: UserId): String {
    match findUser(users, id) {
        some(user) => {
            let email = match user.email {
                some(address) => address
                none => "unknown"
            }

            "{user.name} <{email}>"
        }
        none => "unknown user"
    }
}
~~~

### 24.4 Arrays, slices y cálculo

~~~tondo fragment spec.console
enum StatisticsError {
    EmptyInput
}

fn mean(values: Array[Float]): Float ! StatisticsError {
    if values.isEmpty() {
        fail StatisticsError.EmptyInput
    }

    var total = 0.0

    for value in values {
        total += value
    }

    total / Float(values.length())
}

fn center(values: Array[Float]): Array[Float] ! StatisticsError {
    values - mean(values)?
}

fn main(): !StatisticsError {
    let samples = [10.0, 20.0, 30.0, 40.0]
    let interior = samples[1:-1]
    let centered = center(interior)?

    console.print("{centered}")
}
~~~

La resta array-escalar se eleva elemento a elemento y devuelve un array nuevo.

### 24.5 Mutación explícita de una región

~~~tondo fragment spec.core
fn scale(values: mut Array[Float], factor: Float) {
    values *= factor
}

fn transform() {
    var values = [1.0, 2.0, 3.0, 4.0]

    scale(mut values[1:3], 10.0)

    assert(values == [1.0, 20.0, 30.0, 4.0])
}
~~~

El préstamo solo vive durante `scale` y modifica la región original.

### 24.6 Conteo con map

~~~tondo fragment spec.console
fn countWords(words: Array[String]): Map[String, Int] {
    var counts: Map[String, Int] = [:]

    for word in words {
        counts[word] = counts.getOr(word, 0) + 1
    }

    counts
}

fn printCounts(counts: Map[String, Int]) {
    for (word, count) in counts {
        console.print("{word}: {count}")
    }
}
~~~

La iteración conserva el orden de primera aparición de cada palabra.

### 24.7 Set y pertenencia

~~~tondo fragment spec.core
fn canEdit(permissions: Set[String]): Bool {
    "admin" in permissions or "write" in permissions
}

let permissions = Set["read", "write"]
assert(canEdit(permissions))
~~~

### 24.8 Unión heterogénea

~~~tondo fragment spec.core
type SettingName = String

let settings: Map[SettingName, Int | String | Bool] = [
    SettingName("port"): 8080,
    SettingName("host"): "localhost",
    SettingName("debug"): true,
]

fn renderSetting(value: Int | String | Bool): String {
    match value {
        Int(number) => "{number}"
        String(text) => text
        Bool(flag) if flag => "true"
        Bool(_) => "false"
    }
}
~~~

### 24.9 Trait estático

~~~tondo fragment spec.user
impl Display for User {
    fn display(self): String {
        "{self.name} ({self.id.value})"
    }
}

fn renderAll[T: Discard + Display](values: Array[T]): Array[String] {
    var output: Array[String] = []

    for value in values {
        output.append(value.display())
    }

    output
}
~~~

`Display` es un trait estático predeclarado e implementable, resuelto sin vtable.
No existe conversión de `User` a un objeto `Display`.

### 24.10 Bucle único

~~~tondo fragment spec.jobs
fn process(queue: var Deque[Job]): !JobError {
    for not queue.isEmpty() {
        let job = queue.popFront().at()

        if job.cancelled {
            continue
        }

        run(job)?
    }
}
~~~

No se necesita `while`. `for condition` expresa el mismo flujo.

### 24.11 Main completo

~~~tondo fragment spec.application
import std.process

enum AppError {
    Arguments(ArgsError)
    Config(ConfigError)
    Runtime(RuntimeError)
}

fn runApplication(): !AppError {
    let options = match parseArgs(process.args()) {
        ok(options) => options
        err(error) => fail AppError.Arguments(error)
    }

    let config = match loadConfig(options.configPath) {
        ok(config) => config
        err(error) => fail AppError.Config(error)
    }

    match run(config) {
        ok(()) => ()
        err(error) => fail AppError.Runtime(error)
    }
}

fn main(): !AppError {
    runApplication()?
}
~~~

Los nombres de módulos y métodos del ejemplo son ilustrativos; sus contratos definitivos pertenecerán a la librería estándar.

### 24.12 Variádicos, spread y cierres

~~~tondo fragment spec.core
fn decorate(prefix: String, values: ...String): Array[String] {
    let format: fn(String): String = (value) {
        "{prefix}{value}"
    }

    var output: Array[String] = []

    for value in values {
        output.append(format(value))
    }

    output
}

let rest = ["two", "three"]
let lines = decorate("> ", "one", ...rest)
~~~

El cierre infiere `String` desde su tipo esperado. El variádico se observa como `Array[String]`, aunque la implementación pueda pasar una vista temporal.

### 24.13 Asignación múltiple

~~~tondo fragment spec.core
var left = 10
var right = 20

(left, right) = (right, left)

assert(left == 20)
assert(right == 10)
~~~

El lado derecho se completa antes de la primera escritura.

### 24.14 Concurrencia estructurada

~~~tondo fragment spec.async_page
async fn loadPage(userId: UserId): Page ! ApiError {
    scope {
        let userJob = spawn fetchUser(userId)
        let postsJob = spawn fetchPosts(userId)

        let user = await userJob?
        let posts = await postsJob?

        Page { user, posts }
    }
}

async fn main(): !ApiError {
    let page = await loadPage(UserId(42))?
    console.print("{page}")
}
~~~

Los dos fetches pueden progresar en paralelo, pero sus handles no pueden escapar del `scope`.

### 24.15 Identidad con `Ref[T]`

~~~tondo fragment spec.user
let users: Ref[Map[String, User]] = Ref([:])
let sameUsers = users
let otherUsers: Ref[Map[String, User]] = Ref([:])

assert(users == sameUsers)
assert(users != otherUsers)
assert(users.value == otherUsers.value)

var labels: Map[Ref[Map[String, User]], String] = [:]
labels[users] = "primary"
labels[otherUsers] = "secondary"
~~~

El contenido de ambas referencias es igual, pero sus identidades forman dos claves diferentes.

### 24.16 Frontera raw con `Pointer[T]`

~~~tondo fragment spec.core
unsafe fn readFirst(address: Pointer[Byte]): Byte {
    address.read()
}

unsafe fn readAddress(address: Pointer[Byte]): Byte {
    readFirst(address)
}
~~~

`readAddress` continúa siendo `unsafe` porque no puede validar procedencia, validez
ni lifetime a partir del puntero. El llamador escribe
`unsafe { readAddress(address) }` y hace visible el punto exacto donde acepta esas
obligaciones. Una función segura solo podría ocultarlo tras recibir un handle
seguro que mantuviera dichas invariantes.

### 24.17 Script con pipeline

~~~tondo script spec.process
#!/usr/bin/env tondo

import std.console
import std.process

let pipeline = (
    process.cmd("git", "log", "--oneline") |
    process.cmd("head", "-n", "5")
)

let output = await pipeline.check()?
console.print(output.stdout.text()?)
~~~

El `await` convierte el `main` implícito en async. Construir el pipeline no ejecuta nada hasta alcanzar `check()`.

---

## 25. Características deliberadamente ausentes

### 25.1 `null`

Ausencia se representa con `T?`. Esto impide que cualquier referencia o valor pueda ser ausente sin declararlo.

### 25.2 Excepciones

No existen `throw`, `try`, `catch` ni stack unwinding recuperable. Los errores forman parte de la firma y se manejan con `?` o `match`.

### 25.3 Clases y herencia

No existen clases, superclases, métodos virtuales, constructors mágicos ni campos protegidos. Se utilizan:

- Records.
- Enums.
- Composición.
- Traits estáticos.
- Funciones y métodos explícitos.

### 25.4 Sobrecarga de funciones

Un nombre resuelve a una sola función en un scope. Las variaciones usan:

- Nombres diferentes.
- Genéricos.
- Records de opciones.
- Enums para modos.

Esto evita resolución dependiente de conversiones y tipos incompletos.

### 25.5 Sobrecarga de operadores

Los operadores son cerrados. Las cuatro composiciones intrínsecas entre `Command`
y `Pipeline` están enumeradas por el lenguaje. Un tipo de dominio utiliza métodos
como `add`, `merge`, `compose` o `combine`.

### 25.6 Conversiones implícitas

No hay:

- Promoción numérica.
- Stringification automática fuera de interpolación definida.
- Conversión booleano/número.
- Conversión record por similitud.
- Conversión automática de errores nominales.
- Conversión de array covariante.

### 25.7 Truthiness

`Bool` es el único tipo de condición. Cero, string vacío, array vacío, `none` y maps vacíos no se convierten a `false`.

### 25.8 Múltiples bucles

No existen `while`, `do while`, `repeat until` ni C-style `for`. `for` cubre los tres casos necesarios.

### 25.9 Ternario y Elvis

No existen `condition ? a : b` ni `?:`. `if` es expresión. Options y Results se consumen explícitamente.

### 25.10 Safe navigation

No existe un operador `?.`. La secuencia escrita igual son dos tokens: `?` propaga
fuera de la función y `.` continúa solo en el camino exitoso, según 17.2; nunca
produce una option local por navegación. Las cadenas que deban decidir localmente
dónde aparece ausencia utilizan `match` u operaciones nombradas.

### 25.11 Globals mutables

Un módulo solo admite constantes y declaraciones. Los `let` y `var` top-level de un script son locales del `main` implícito, no globals. Estado global controlado se construye mediante tipos de sincronización o inicialización explícita en el punto de entrada.

### 25.12 Inicializadores e imports con efectos

Importar nunca ejecuta código. No hay bloques `init`.

### 25.13 Destructores y finalizadores

El tiempo de recolección no es parte del contrato. Los recursos externos se
cierran explícitamente y pueden utilizar `defer`. No existen destructores de
usuario ejecutados al abandonar normalmente un scope ni finalizadores ligados al
recolector. La acción de unwind intrínseca de un tipo terminal es el fallback
cerrado del registro único de 8.10 para pánico, cancelación, transferencias
incompletas y teardown de propietarios estructurados. Se desarma al registrar un
guard, por lo que nunca se ejecutan ambos, y no reemplaza el cleanup visible
exigido en salidas ordinarias.

### 25.14 Punteros en código seguro y memoria manual implícita

Tondo seguro no expone lectura, escritura, offsets, casts, `malloc` ni `free` raw. `Pointer[T]` solo se opera dentro de `unsafe`; la capa FFI y los módulos de sistema deben encapsularlo tras APIs seguras siempre que sea posible. La identidad cotidiana utiliza `Ref[T]`, no direcciones.

### 25.15 Macros y metaprogramación

No hay preprocesador, macros textuales, AST macros, reflection mutable, `eval` ni generación de declaraciones dentro del lenguaje.

El código repetitivo puede resolverse mediante:

- Genéricos.
- Funciones.
- Herramientas externas deterministas.
- Generación previa declarada por el sistema de build.

El código generado es fuente ordinaria y se valida igual.

La serialización de un record de usuario no obtiene acceso reflectivo implícito a sus campos. Debe existir una implementación explícita de un trait de codec, una función escrita por el módulo propietario o código fuente generado antes de compilar. Los campos privados solo pueden ser leídos o construidos por código con visibilidad válida.

### 25.16 Tipo dinámico universal

No existe `Any`. Datos heterogéneos usan enums o uniones cerradas. Datos externos sin schema usan un enum explícito como `Json`.

### 25.17 Múltiples colecciones equivalentes

El núcleo no contiene:

- `List` como alias de `Array`.
- `Vector`.
- `Slice`.
- `Span`.
- `Dictionary` como alias de `Map`.
- Maps ordenados y desordenados separados.
- `Stack` y `Queue` separados.

`Array`, `Map`, `Set` y posteriormente `Deque` cubren las necesidades comunes con nombres únicos.

### 25.18 Concatenación mediante aritmética

`+` y `*` conservan significado numérico. Concatenar, repetir o mergear se expresa con funciones nombradas.

### 25.19 Semicolons e indentación semántica

No hay punto y coma. Las llaves delimitan bloques; la indentación es canónica pero no define scope.

### 25.20 Parámetros por defecto y packs heterogéneos

Los parámetros por defecto no forman parte de 0.1; un record de opciones conserva evolución de API. Los variádicos existen únicamente como un pack final homogéneo `...T`. No hay parameter packs heterogéneos, variadic generics ni conversión automática desde tuples.

### 25.21 Generators y concurrencia no estructurada

No hay `yield`, generators de sintaxis especial, tasks detached implícitas ni futures creados por llamadas async sin `await` o `spawn`. El protocolo estático `Iterator[T]` cubre consumo síncrono; los streams asíncronos se definirán sobre `async` y la librería sin añadir trabajo huérfano.

### 25.22 Dispatch dinámico de traits

Los traits son constraints estáticos. Un resultado `impl Bound` conserva un único
tipo concreto por declaración y tampoco introduce dispatch dinámico. La
heterogeneidad dinámica debe ser cerrada y visible mediante enum, unión o
callbacks.

### 25.23 API dependiente de capitalización

Mayúsculas y minúsculas son estilo, no control de acceso. Solo `pub` exporta.

### 25.24 Estados “no inicializados”

Toda variable se inicializa en su declaración; no existen declaraciones pendientes de una asignación futura ni valores indeterminados. Cuando un valor afín se mueve, su binding queda estáticamente no disponible. Un `var` puede reponerse después mediante una asignación completa, pero ninguna lectura ni escritura parcial es válida hasta entonces. Este estado pertenece al análisis de flujo y nunca expone memoria sin inicializar en runtime.

### 25.25 Comportamiento distinto en debug y release

Overflow, bounds, assert, orden y errores conservan semántica. Los modos solo cambian optimización, símbolos de depuración y diagnósticos adicionales.

### 25.26 Arrays de longitud en el tipo

Tondo 0.1 no tiene `Array[T, N]`, `[N]T` ni const generics. `Array[T]` mantiene longitud en runtime. Protocolos binarios y FFI que necesiten layout fijo utilizarán tipos buffer específicos de una especificación posterior.

### 25.27 Shadowing

No se puede redeclarar un nombre mientras otro binding con el mismo nombre siga visible. Las transformaciones utilizan nombres distintos y los scopes hermanos pueden reutilizar nombres sin ocultarse entre sí.

### 25.28 Atributos generales y compilación condicional

Tondo 0.1 no tiene decorators, macros de atributos, pragmas semánticos,
`#if` dentro de fuente ni un mecanismo abierto para que una herramienta cambie
la compilación de una declaración. La documentación y el metadato de tooling
pueden acompañar símbolos, pero no modificar resolución, tipos, layout,
ownership, efectos ni generación de código.

La selección por plataforma, perfil y capacidad utiliza source sets declarados
por el build según 6.8 y 20.11. Las integraciones FFI utilizan unidades
privilegiadas o descriptores del toolchain según 8.13; no crean una puerta lateral
de atributos en módulos ordinarios. Incorporar sintaxis de atributos o
compilación condicional en el futuro requiere una nueva edición y un contrato
cerrado para cada efecto admitido.

---

## 26. Frontera con la librería estándar

### 26.1 Responsabilidad del lenguaje

Esta especificación define:

- Tokens, gramática y formato canónico.
- Módulos, imports, identidad nominal de paquetes y resolución cerrada por
  lockfile.
- Ediciones, source sets, targets, perfiles de host y capacidades declaradas.
- Visibilidad.
- Bindings y control de flujo.
- Sistema de tipos.
- Records, enums, uniones, tuples y newtypes.
- `Option` y `Result`.
- `Array`, `Map`, `Set` y `Range` como tipos intrínsecos, y `Iterator[T]` como
  protocolo estático intrínseco implementable.
- Literales e indexación de colecciones.
- Operadores y aritmética vectorizada.
- Funciones, métodos, cierres, genéricos y traits.
- Resultados opacos estáticos `impl Bound` sin dispatch dinámico.
- Parámetros variádicos homogéneos.
- Errores, propagación y pánicos.
- Préstamos compartidos `ref`, mutabilidad `mut`/`var`, valores `Copy` y afines,
  gestión automática, `Ref[T]`, frontera `unsafe` de `Pointer[T]` y contrato
  cerrado de comportamiento indefinido.
- Funciones async, `await`, scopes de tasks, `spawn`, `Join`, cancelación
  estructurada y garantía de progreso cooperativo.
- Capacidades intrínsecas `Copy`, `Discard`, `Equatable`, `Key`, `Send` y
  `Share`, y protocolos cerrados `Call`, `CallMut` y `CallOnce`.
- Contrato mínimo de `Display` utilizado por interpolación; formatos concretos y
  helpers pertenecen a la librería.
- Contrato de `main`, script raíz y `main` implícito.
- Tipos opacos de plan `Command` y `Pipeline`, y su composición cerrada mediante `|`.
- Ausencia de ABI binaria estable para código Tondo 0.1 y requisitos mínimos de
  cualquier futura frontera FFI.
- Requisitos de diagnostics, tooling, builds deterministas y conformidad
  ejecutable.

### 26.2 Responsabilidad de la librería estándar

La futura especificación deberá definir, sin cambiar la semántica anterior:

- Métodos completos de `String`, `Array`, `Map`, `Set` y `Range`, además de
  adaptadores que produzcan cursores nominales o resultados concretos
  `impl Iterator[T] + Discard`.
- Algoritmos de orden superior genéricos sobre `Call`, `CallMut` o `CallOnce`
  según su patrón real de invocación; `fn(...)` se reserva para almacenamiento
  uniforme explícito.
- `Bytes` y buffers binarios.
- `Deque` y priority queues.
- `Decimal`, `BigInt` y `Complex`.
- Formato, parsing y codecs.
- Paths y filesystem.
- Consola, constructores y operaciones de `Command` y `Pipeline`, procesos hijos, exit status y shell explícito.
- Environment.
- Tiempo: `Duration`, `Instant`, `Date`, `Time`, `DateTime` y `TimeZone`.
- `Url`, `Uuid`, IPs y sockets.
- Regex.
- JSON y serialización.
- Aleatoriedad.
- `WeakRef`, mutabilidad interior auditada como `Cell`, y contratos concretos de
  observación de liveness.
- Threads, canales, mutexes, atomics, actores, streams, pools bloqueantes y APIs
  explícitas de cancelación.
- Capacidades `Copy`, `Discard`, `Equatable`, `Key`, `Send` y `Share`, identidad y operaciones terminales de cada tipo opaco de librería.
- Testing.
- Logging.
- Declaraciones FFI, layouts, calling conventions y wrappers seguros sobre `Pointer[T]`.

### 26.3 Prelude

El prelude mínimo, importado implícitamente, contiene solo nombres necesarios para escribir el lenguaje:

~~~text
Bool Int Float Byte Char String Unit Never
Int8 Int16 Int32 Int64
UInt8 UInt16 UInt32 UInt64
Float32 Float64
Option Result
Array Map Set Range Iterator
Ref Pointer Join
Command Pipeline
Copy Discard Equatable Key Send Share
Call CallMut CallOnce
Display
NumericConversionError
~~~

También expone los intrinsics:

~~~text
panic assert
~~~

`some`, `none`, `ok` y `err` son sintaxis del lenguaje, no imports. `Ref(value)` es la construcción intrínseca de una referencia con identidad nueva. Las operaciones de `Pointer[T]` permanecen inaccesibles fuera de `unsafe` aunque el nombre del tipo esté en el prelude.

El prelude no incluye I/O, colecciones especializadas, fechas, red, JSON ni utilidades de aplicación.

### 26.4 Métodos intrínsecos frente a métodos de librería

El lenguaje garantiza las capacidades necesarias para:

- Obtener longitud.
- Indexar y slicing.
- Iterar.
- Copiar lógicamente valores `Copy` y mover una sola vez valores afines.
- Rastrear obligaciones terminales y consumo estructurado.
- Observar temporalmente cualquier valor mediante `ref` sin copiarlo ni moverlo.
- Mutar con extensión fija mediante `mut` y cambiar estructura mediante `var`.
- Consultar/insertar una clave.
- Comprobar pertenencia.
- Aplicar operadores definidos.

Los nombres concretos como `length`, `get`, `getOr`, `append`, `remove`, `concat`, `repeat`, `keys` y `values` se fijarán en la especificación estándar. Los ejemplos de este documento anticipan nombres recomendados, pero no constituyen aún el inventario final de métodos.

### 26.5 Criterio para añadir una característica al lenguaje

Una capacidad solo debe pasar de librería a lenguaje si cumple todos:

1. No puede expresarse con seguridad y ergonomía razonables como librería.
2. Es necesaria para una gran proporción de programas.
3. Tiene una semántica única y pequeña.
4. Puede diagnosticarse localmente.
5. No crea varias formas equivalentes de resolver el mismo problema.
6. Conserva formato y análisis deterministas.
7. Su costo puede explicarse.

Si no cumple estos criterios, permanece en la librería.

### 26.6 Criterio de estabilidad

Tondo evoluciona de manera conservadora:

- Una versión menor puede aclarar reglas, mejorar diagnósticos y añadir APIs de librería compatibles.
- La compatibilidad de Tondo 0.1 es de fuente y semántica para una edición,
  target, perfil y conjunto de capacidades declarados; no promete compatibilidad
  binaria entre versiones de compilador.
- Añadir sintaxis, keywords, coerciones, nuevas inferencias, atributos semánticos
  o compilación condicional requiere una nueva edición cuando pueda cambiar cómo
  se interpreta fuente existente.
- Añadir, retirar o cambiar un nombre del prelude implícito es un cambio de
  compatibilidad de fuente porque esos nombres están reservados en scopes de
  usuario; requiere una nueva edición o versión mayor. APIs ordinarias nuevas
  permanecen en módulos calificados.
- Cambiar significado de código válido requiere una versión mayor.
- Añadir el primer campo privado de un record público requiere una versión mayor
  porque retira su construcción literal externa. En un record que ya no era
  externamente construible, cambiar campos privados requiere una versión mayor
  cuando altera campos públicos, capacidades derivadas, obligación terminal o la
  semántica documentada de igualdad y hashing, según 7.6. Eliminar el último campo
  privado añade una forma pública de construcción y requiere una versión menor
  solo si conserva el resto del contrato; de lo contrario requiere una mayor.
- Cambiar los bounds publicados de un resultado `impl Bound` es un cambio de API;
  cambiar solo su tipo concreto oculto no lo es si conserva esos bounds,
  comportamiento y contrato de versión.
- La identidad de un tipo público incluye su `PackageId`; cambiar versión,
  integridad u origen resuelto puede producir una identidad distinta incluso si
  el texto de la API coincide.
- Toda interfaz compilada registra edición, target, perfil, capacidades,
  features, `PackageId`, hash de API y dependencias. El artefacto añade source
  sets e inputs generados. El consumidor rechaza combinaciones incompatibles en
  vez de enlazarlas por parecido nominal.
- Retirar una capacidad anunciada o sustituirla por un fallback que falla en
  runtime es un cambio de target. Los source sets y capacidades se vuelven a
  resolver explícitamente.
- El formateador y compilador deben poder indicar la edición del lenguaje y la
  versión exacta del formato o suite de conformidad que implementan.
- Los archivos no contienen pragmas locales que cambien semántica silenciosamente.

---

## Apéndice A. Referencia rápida

### Declaraciones

~~~tondo
let value = expression
var value: Type = expression
const DefaultValue: Type = expression

pub type Name = ExistingType
pub type Record = { field: Type }
pub alias Alias = ExistingType

pub enum Choice {
    First
    Second(Type)
}
~~~

### Funciones

~~~tondo
fn action()
fn calculate(): Value
fn save(): !Error
fn load(): Value ! Error
fn log(values: ...String)
fn transform[T: Discard + Display](value: T): String
fn makeCounter(): impl CallMut[fn(): Int] + Discard
async fn fetch(): Value ! Error
unsafe fn read(address: Pointer[Byte]): Byte
~~~

### Tipos compactos

~~~tondo
T?              // Option[T]
T ! E           // Result[T, E]
!E              // Result[Unit, E]
A | B           // unión discriminada estructural
Array[T]
Map[K, V]
Set[K]
Range[T]
Iterator[T]     // solo como bound; no es un tipo de valor
Ref[T]          // requiere T: Discard
Pointer[T]
Join[T, E]
Command
Pipeline
(A, B)
fn(A): B ! E
fn(ref A): B
fn(mut A, B): C
fn(var A, B): C
async fn(A): B ! E
async unsafe fn(Pointer[Byte]): B ! E
Call[fn(A): B]      // solo como bound
CallMut[fn(A): B]   // solo como bound
CallOnce[fn(A): B]  // solo como bound
fn makeIterator[T](): impl Iterator[T] + Discard
~~~

### Control

~~~tondo
if condition {} else {}
match value {
    pattern => ()
}

for condition {}
for item in values {}
for ref item in values {}
for {}

return value
fail error
break
continue
defer cleanup()

scope {
    let job = spawn fetch()
    let value = await job?
    consume(value)
}
~~~

### Errores

~~~tondo
let value = fallible()?

match fallible() {
    ok(_) => ()
    err(_) => ()
}
~~~

### Mutación

~~~tondo
var values = [1, 2, 3]
values[0] = 10
(left, right) = (right, left)

fn change(values: mut Array[Int]) {}
fn appendValue(values: var Array[Int], value: Int) {}
fn inspect(values: ref Array[Int]): Int {
    values.length()
}

change(mut values)
appendValue(var values, 4)
let count = inspect(ref values)
~~~

- `ref T` presta observación compartida sin copiar ni mover y puede aparecer sobre
  un `let`; no tiene identidad ni puede almacenarse.
- `mut T` presta un lvalue exclusivo y conserva su extensión estructural; puede
  reemplazar contenido de igual extensión y recibir un slice.
- `var T` presta un lvalue completo y exclusivo; permite reemplazar sin conservar
  extensión o redimensionar colecciones y no puede recibir una región parcial.

### Ownership y capacidades

~~~tondo
fn duplicate[T: Copy](value: T): (T, T) {
    (value, value)
}

scope {
    let job = spawn fetch()
    let result = await job? // consume el Join
    consume(result)
}
~~~

- Pasar o asignar un valor `Copy` deja disponible el origen.
- Pasar o asignar un valor no `Copy` lo mueve.
- `Discard` permite abandonar un valor aunque no sea `Copy`; su ausencia indica
  una obligación terminal potencial.
- Un `var` movido puede reponerse mediante asignación completa.
- Un valor con obligación terminal debe consumirse explícitamente o reservarse con `defer`.
- `Send` controla transferencia entre tasks o threads; `Share`, observación concurrente de una identidad.
- Cada cierre captura por valor: copia capturas `Copy`, mueve las demás y deriva
  de su entorno `Copy`, `Discard`, obligación terminal, `Send` y `Share`.
- `Call`, `CallMut` y `CallOnce` expresan respectivamente llamada compartida,
  exclusiva reutilizable y consumidora.

### Scripts y procesos

~~~tondo script spec.process
#!/usr/bin/env tondo

import std.console
import std.process

let pipeline = process.cmd("producer") | process.cmd("consumer")
let output = await pipeline.output()?
console.print(output.stdout.text()?)
~~~

Las sentencias top-level existen solo en el archivo raíz de un script y forman un `main` implícito.

---

## Apéndice B. Declaración de diseño

Tondo se considera fiel a esta especificación cuando un programa puede leerse como una colección de contratos cerrados:

- Los datos posibles están enumerados por sus tipos.
- La ausencia se ve como `?`.
- El fallo recuperable se ve como `!`.
- La mutación se ve como `var` o `mut`.
- La duplicación permitida está limitada a valores `Copy`; el resto del ownership se mueve una sola vez.
- El abandono genérico seguro se ve como `Discard`; su ausencia conserva visible
  una posible obligación terminal.
- La identidad compartida se ve como `Ref`.
- La observación prestada se ve como `ref`.
- El acceso raw se ve como `Pointer` y `unsafe`.
- El código seguro no tiene comportamiento indefinido; una operación unsafe
  publica exactamente las precondiciones que podrían introducirlo.
- La memoria ordinaria se administra automáticamente, incluidos los ciclos; el
  cleanup de recursos externos permanece explícito.
- La suspensión se ve como `async` y `await`.
- La concurrencia se ve como `scope` y `spawn`.
- La exportación se ve como `pub`.
- La discriminación se ve como `match`.
- El cleanup se ve como `defer`.
- Las fronteras concurrentes se comprueban con `Send` y `Share`.
- La iteración se ve como `for`.
- Los efectos comienzan en `main` explícito o en el `main` implícito de un script raíz.
- Las diferencias de target se declaran como capacidades y source sets del
  build, nunca como semántica ambiental escondida dentro de la fuente.

El lenguaje no intenta ganar concisión ocultando información. Gana concisión eliminando ceremonias cuya semántica puede deducirse de una única regla.

**Pequeño por diseño, completo en la práctica.**

---

## Apéndice C. Fixtures normativos de documentación

### C.1 Propósito y aislamiento

Los fixtures existen solo para typecheckear los fences de esta especificación
mediante 21.6. No forman parte del prelude, de la futura librería estándar ni de
la superficie disponible para un programa ordinario. Son interfaces privilegiadas
sin cuerpo ejecutable; el doc runner nunca ejecuta un ejemplo.

Cada fixture se compone de:

- Un **universo de módulos**, que solo queda visible cuando el fence contiene el
  import correspondiente.
- Declaraciones **inyectadas** en el mismo módulo sintético que el fragmento.
  Se sitúan en una cola sintética del mismo archivo, por lo que ven su header de
  imports; el orden no afecta firmas de módulo. A efectos de orphan rules, ese
  módulo posee sus tipos nominales.
- Extensiones intrínsecas de prueba sobre tipos del lenguaje.

Una declaración del bloque siempre gana por identidad propia, no por sustitución
del fixture. Una colisión de nombre produce `E1002`; el runner no retira
declaraciones para hacer pasar un ejemplo. Todos los tipos opacos siguientes son
`Copy + Discard + Send + Share` salvo que se indique otra cosa. Una línea
`capabilities:` reemplaza por completo ese conjunto, no se suma a él.
Dentro de la serialización C.6, un path que comienza por `std.` es una identidad
de módulo canónica ya resuelta; no introduce un import en el fence.

### C.2 Base `spec.core` y `spec.0_1`

`spec.0_1` es exactamente un alias de `spec.core`. La base expone estas
interfaces de comprobación:

~~~text
opaque Bytes
opaque Utf8Error
opaque Status
opaque AcquireError

opaque Resource
    capabilities: Send
    terminal: Resource.release(Resource)
    unwind: resource-release

fn Resource.release(resource: Resource)
fn Resource.status(self): Status
fn consume[T: Discard](value: T)

fn String.isEmpty(self): Bool
fn Bytes.text(self): String ! Utf8Error
fn Array[T].length(self): Int
fn Array[T].isEmpty(self): Bool
fn Array[T: Copy].getOr(self, index: Int, fallback: T): T
fn Array[T].append(var self, value: T)
fn Map[K: Key, V: Copy].getOr(self, key: K, fallback: V): V
fn Option[T: Copy].at(self): T
unsafe fn Pointer[T].read(self): T
~~~

Los cuerpos y el comportamiento runtime de estas operaciones no forman parte del
fixture y nunca se ejecutan. Los intrinsics `Float(Int)`, interpolación, `assert`
y las implementaciones `Display` de escalares, strings y arrays de elementos
`Display` son las ya asumidas por los ejemplos de lenguaje.

El universo de módulos contiene:

~~~text
module std.console
    fn print(value: String)

module std.fs
    opaque Path
    opaque IoError
    fn read(path: Path): Bytes ! IoError

module std.json
    opaque DecodeError

module std.process
    fn args(): Array[String]
    fn cmd(program: String, arguments: ...String): Command
~~~

Para `Command` y `Pipeline`, el fixture de procesos añade:

~~~text
opaque ProcessError

type ProcessOutput = {
    stdout: Bytes
}

async fn Pipeline.output(self): ProcessOutput ! ProcessError
async fn Pipeline.check(self): ProcessOutput ! ProcessError
~~~

### C.3 Fixtures especializados

Cada nombre de esta tabla extiende `spec.core` con exactamente las declaraciones
indicadas:

| Fixture | Declaraciones inyectadas |
|---|---|
| `spec.cursor` | `Query`, `Row`, `DatabaseError`, `RowCursor`, `Database.openRows`, `RowCursor.close` y su `impl Iterator[Row]`. |
| `spec.resource` | `fn acquire(): Resource ! AcquireError`. |
| `spec.domain` | `fn decodeUser(bytes: Bytes): User ! std.json.DecodeError`; `User` debe declararse en el propio fence. |
| `spec.settings` | Binding de módulo `fs`; aliases `Path = fs.Path`, `IoError = fs.IoError`; tipos `DecodeError`, `Settings`; `fn decodeSettings(Bytes): Settings ! DecodeError`. |
| `spec.user` | Tipos `UserId` y `User` definidos en C.4. |
| `spec.console` | Binding de módulo `console = std.console`. |
| `spec.jobs` | Tipos `Deque[T]`, `Job`, `JobError` y las operaciones de C.4. |
| `spec.application` | Tipos y operaciones de aplicación de C.4; el import `std.process` permanece en el fence. |
| `spec.async_page` | Binding `console = std.console`; tipos async de C.4. |
| `spec.process` | Interfaces de proceso de C.2; los imports permanecen en el script. |

`Database` es un tipo opaco usado solo como propietario de una operación asociada;
no se construye ningún valor suyo. Su contrato completo de fixture es:

~~~text
opaque Query
opaque Row
opaque DatabaseError
opaque Database

opaque RowCursor
    capabilities: Send
    terminal: RowCursor.close(RowCursor)
    unwind: cursor-close

fn Database.openRows(query: Query): RowCursor ! DatabaseError
fn RowCursor.close(cursor: RowCursor)

impl Iterator[Row] for RowCursor {
    fn next(mut self): Row?
}
~~~

El cursor no cumple `Copy`, `Discard` ni `Share`. Esta declaración comprueba que
`for` conserva el tipo concreto, que el guard de `defer` sigue su movimiento y
que `Iterator[Row]` no borra la obligación terminal.

### C.4 Tipos de dominio de los ejemplos

`spec.user` inyecta:

~~~text
type UserId = Int

type User = {
    id: UserId
    name: String
    email: String?
}
~~~

`spec.jobs` inyecta una `Deque[T: Discard]` opaca con capacidad exacta
`Discard`: no es `Copy`, `Send` ni `Share` dentro del fixture. El bound impide
que el stub oculte una obligación terminal de `T`. Añade además:

~~~text
type Job = {
    cancelled: Bool
}

opaque JobError

fn Deque[T].isEmpty(self): Bool
fn Deque[T].popFront(var self): T?
fn run(job: Job): !JobError
~~~

`spec.application` inyecta:

~~~text
opaque Path
opaque ArgsError
opaque ConfigError
opaque RuntimeError
opaque Config

type Options = {
    configPath: Path
}

fn parseArgs(arguments: Array[String]): Options ! ArgsError
fn loadConfig(path: Path): Config ! ConfigError
fn run(config: Config): !RuntimeError
~~~

`spec.async_page` inyecta:

~~~text
type UserId = Int
opaque User
opaque Posts
opaque ApiError

type Page = {
    user: User
    posts: Posts
}

async fn fetchUser(userId: UserId): User ! ApiError
async fn fetchPosts(userId: UserId): Posts ! ApiError
impl Display for Page
~~~

La última línea es una abreviatura de fixture para una implementación ordinaria
de `Display.display(self): String`; no añade sintaxis al lenguaje.

### C.5 Validación del manifiesto

Un doc runner conforme rechaza un fixture desconocido. Al cargar la edición
verifica:

1. Que cada interfaz anterior sea internamente bien formada.
2. Que no existan dos cabeceras de `impl` solapadas.
3. Que `RowCursor` tenga exactamente un `Iterator[T]`.
4. Que el SHA-256 de la serialización canónica de C.6 coincida con el valor
   publicado.

Al comprobar cada fence valida además que ninguna declaración inyectada colisione
con el bloque. Una línea `require type Name` de C.6 no inyecta un símbolo: exige
que el fence lo declare y permite que otras firmas del fixture lo mencionen. La
ausencia produce `E1001`; no se trata como un manifiesto mal formado.

`fixture_sha256` es siempre el hash de **todo** el manifiesto C.6, no de un
subconjunto por fixture. De ese modo identifica la misma edición de stubs incluso
cuando dos fences seleccionan fixtures distintos.

### C.6 Serialización canónica

El byte string canónico es exactamente el contenido del siguiente fence, desde
la `t` inicial de `tondo-fixture-manifest` hasta el `LF` posterior a `end`. Está
codificado en UTF-8, ya utiliza `LF` y no incluye los delimitadores Markdown:

~~~text
tondo-fixture-manifest 0.1
defaults Copy Discard Send Share
universe module std.console
universe decl std.console fn print(value: String)
universe module std.fs
universe decl std.fs opaque Path
universe decl std.fs opaque IoError
universe decl std.fs fn read(path: Path): Bytes ! IoError
universe module std.json
universe decl std.json opaque DecodeError
universe module std.process
universe decl std.process fn args(): Array[String]
universe decl std.process fn cmd(program: String, arguments: ...String): Command
fixture spec.core
decl spec.core opaque Bytes
decl spec.core opaque Utf8Error
decl spec.core opaque Status
decl spec.core opaque AcquireError
decl spec.core opaque Resource capabilities Send terminal Resource.release(Resource) unwind resource-release
decl spec.core fn Resource.release(resource: Resource)
decl spec.core fn Resource.status(self): Status
decl spec.core fn consume[T: Discard](value: T)
decl spec.core fn String.isEmpty(self): Bool
decl spec.core fn Bytes.text(self): String ! Utf8Error
decl spec.core fn Array[T].length(self): Int
decl spec.core fn Array[T].isEmpty(self): Bool
decl spec.core fn Array[T: Copy].getOr(self, index: Int, fallback: T): T
decl spec.core fn Array[T].append(var self, value: T)
decl spec.core fn Map[K: Key, V: Copy].getOr(self, key: K, fallback: V): V
decl spec.core fn Option[T: Copy].at(self): T
decl spec.core unsafe fn Pointer[T].read(self): T
alias fixture spec.0_1 = spec.core
fixture spec.cursor extends spec.core
decl spec.cursor opaque Query
decl spec.cursor opaque Row
decl spec.cursor opaque DatabaseError
decl spec.cursor opaque Database
decl spec.cursor opaque RowCursor capabilities Send terminal RowCursor.close(RowCursor) unwind cursor-close
decl spec.cursor fn Database.openRows(query: Query): RowCursor ! DatabaseError
decl spec.cursor fn RowCursor.close(cursor: RowCursor)
decl spec.cursor impl Iterator[Row] for RowCursor fn next(mut self): Row?
fixture spec.resource extends spec.core
decl spec.resource fn acquire(): Resource ! AcquireError
fixture spec.domain extends spec.core
require spec.domain type User
decl spec.domain fn decodeUser(bytes: Bytes): User ! std.json.DecodeError
fixture spec.settings extends spec.core
binding spec.settings fs = std.fs
decl spec.settings alias Path = fs.Path
decl spec.settings alias IoError = fs.IoError
decl spec.settings opaque DecodeError
decl spec.settings opaque Settings
decl spec.settings fn decodeSettings(bytes: Bytes): Settings ! DecodeError
fixture spec.user extends spec.core
decl spec.user type UserId = Int
decl spec.user type User = { id: UserId, name: String, email: String? }
fixture spec.console extends spec.core
binding spec.console console = std.console
fixture spec.jobs extends spec.core
decl spec.jobs opaque Deque[T: Discard] capabilities Discard
decl spec.jobs type Job = { cancelled: Bool }
decl spec.jobs opaque JobError
decl spec.jobs fn Deque[T].isEmpty(self): Bool
decl spec.jobs fn Deque[T].popFront(var self): T?
decl spec.jobs fn run(job: Job): !JobError
fixture spec.application extends spec.core
decl spec.application opaque Path
decl spec.application opaque ArgsError
decl spec.application opaque ConfigError
decl spec.application opaque RuntimeError
decl spec.application opaque Config
decl spec.application type Options = { configPath: Path }
decl spec.application fn parseArgs(arguments: Array[String]): Options ! ArgsError
decl spec.application fn loadConfig(path: Path): Config ! ConfigError
decl spec.application fn run(config: Config): !RuntimeError
fixture spec.async_page extends spec.core
binding spec.async_page console = std.console
decl spec.async_page type UserId = Int
decl spec.async_page opaque User
decl spec.async_page opaque Posts
decl spec.async_page opaque ApiError
decl spec.async_page type Page = { user: User, posts: Posts }
decl spec.async_page async fn fetchUser(userId: UserId): User ! ApiError
decl spec.async_page async fn fetchPosts(userId: UserId): Posts ! ApiError
decl spec.async_page impl Display for Page
fixture spec.process extends spec.core
decl spec.process opaque ProcessError
decl spec.process type ProcessOutput = { stdout: Bytes }
decl spec.process async fn Pipeline.output(self): ProcessOutput ! ProcessError
decl spec.process async fn Pipeline.check(self): ProcessOutput ! ProcessError
end
~~~

El SHA-256 esperado de esos bytes es
`1b6ab9f853b7ef4b94b4b9aaff6297e20556f81e8d99c322bed03854453d76c2`. Un cambio de una firma, capability, requirement, binding,
orden o byte exige publicar un hash nuevo y una nueva revisión del Markdown. De
este modo dos runners no pueden usar stubs distintos y afirmar que comprobaron el
mismo ejemplo.
