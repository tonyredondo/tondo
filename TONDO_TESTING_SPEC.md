# Tondo: especificación del lenguaje y toolchain de testing

- **Estado:** diseño normativo aprobado para Tondo 0.2; todavía no implementado.
- **Revisión:** 0.2-draft.6 — 2026-07-29.
- **Edición objetivo:** Tondo 0.2.
- **Especificación base:** [Tondo 0.1](./TONDO_LANGUAGE_SPEC.md).
- **SHA-256 de la base:** `ded4e17ab57836d032e5fb9e5be5dba03fc83ac6ff74cee90ab1bb7f8e5c7084`.
- **Formatos de tooling:** `tondo-test-report-0.2/6`,
  `tondo-test-list-0.2/5` y `tondo-junit-report-0.2/3`.

Esta especificación añade a Tondo las declaraciones `suite` y `test` y define
cómo el toolchain descubre, compila, ejecuta y reporta árboles estáticos de
tests. `suite` es un contenedor léxico con lifecycle compartido; `test` es
siempre una hoja ejecutable. Un núcleo sellado de `std.testing` permite registrar
logs y tags, fallar inmediatamente u omitir de forma explícita el nodo activo
sin exponer un contexto de test como valor. El runner resuelve ownership desde
CODEOWNERS, particiona y ordena ejecuciones de forma reproducible y exporta
reportes JSON o JUnit XML sin alterar el programa probado. Un glob portable
amplía la selección y retries explícitos pueden confirmar fallos intermitentes
en workers nuevos sin presentar un éxito posterior como un `passed` ordinario.
Un dominio temporal opt-in ejecuta suspensión, timers y deadlines contra un
reloj monotónico virtual, avanza únicamente bajo quiescencia demostrable y
permite observar fronteras temporales exactas sin `sleep` wall-clock.
Complementa Tondo 0.1; no modifica retroactivamente esa edición ni la suite
publicada `tondo-conformance-0.1`.

La próxima especificación consolidada de Tondo debe incorporar normativamente
estas reglas sin cambiar sus decisiones, resolver las referencias de sección
contra la nueva numeración y publicar una suite de conformidad distinta. Hasta
entonces, este documento es la fuente normativa para el diseño de testing de la
edición 0.2 y el tracker es solo su plan de implementación.

En este documento, **debe** expresa un requisito de conformidad, **no puede**
expresa una prohibición, **puede** expresa una capacidad permitida y **se
recomienda** expresa orientación no obligatoria.

## Índice

1. [Propósito y principios](#1-propósito-y-principios)
2. [Compatibilidad y límite de edición](#2-compatibilidad-y-límite-de-edición)
3. [Declaraciones `suite` y `test`](#3-declaraciones-suite-y-test)
4. [Semántica estática](#4-semántica-estática)
5. [Source sets y descubrimiento](#5-source-sets-y-descubrimiento)
6. [Construcción del target de test](#6-construcción-del-target-de-test)
7. [Modelo de ejecución](#7-modelo-de-ejecución)
8. [Selección, orden y paralelismo](#8-selección-orden-y-paralelismo)
9. [Resultados, diagnósticos y salida](#9-resultados-diagnósticos-y-salida)
10. [Contrato de `tondo test`](#10-contrato-de-tondo-test)
11. [Frontera con `assert` y `std.testing`](#11-frontera-con-assert-y-stdtesting)
12. [Patrones de uso](#12-patrones-de-uso)
13. [Características deliberadamente ausentes](#13-características-deliberadamente-ausentes)
14. [Diagnósticos nuevos](#14-diagnósticos-nuevos)
15. [Formato machine-readable](#15-formato-machine-readable)
16. [Conformidad](#16-conformidad)
17. [Referencia rápida](#17-referencia-rápida)

---

## 1. Propósito y principios

El sistema de testing de Tondo persigue once objetivos:

1. Escribir un test ordinario requiere únicamente un nombre y un bloque.
2. Agrupar tests y compartir un recurso costoso requiere únicamente una `suite`
   léxica; no clases, annotations ni registro runtime.
3. El test utiliza exactamente el lenguaje normal: `assert`, `?`, `match`,
   `defer`, `for`, `scope`, `spawn`, `await`, ownership y préstamos conservan su
   significado.
4. Logs, tags, fallo inmediato y skip explícito funcionan también desde helpers
   y concurrencia estructurada sin obligar a recibir ni propagar un
   `TestContext`.
5. Ownership de código, partición y orden de ejecución son inputs explícitos,
   auditables y reproducibles.
6. Un mismo resultado alimenta salida humana, JSON canónico y JUnit XML sin
   volver a ejecutar tests.
7. Reintentar un fallo es siempre explícito, conserva cada intento y utiliza una
   frontera de aislamiento nueva.
8. El código temporal puede ejecutarse con el API de producción sobre un reloj
   virtual determinista, sin convertir timeouts del runner en tiempo simulado.
9. Discovery, plan, orden y presentación canónica son deterministas y
   observables; todo campo operacional no reproducible se identifica como tal.
10. El código y las dependencias de test no cambian el artefacto de producción.
11. El núcleo no introduce clases de test, annotations, macros, reflection ni
   hooks de ciclo de vida.

Forma mínima:

~~~tondo
test addReturnsSum {
    assert(add(20, 22) == 42)
}
~~~

Forma jerárquica:

~~~tondo
suite arithmetic {
    let offset = 20

    test addsValues {
        assert(offset + 22 == 42)
    }

    test subtractsValues {
        assert(22 - offset == 2)
    }
}
~~~

`test` existe porque el registro estático de entradas, su aislamiento y su
acceso unitario a declaraciones privadas no pueden expresarse como una función
de librería sin introducir registro en runtime, efectos top-level, reflection,
convenciones mágicas de nombres o boilerplate manual. `suite` existe porque el
lifecycle de un grupo seleccionado, la identidad jerárquica y el teardown
posterior a todos sus descendientes tampoco pueden expresarse mediante un bloque
ordinario sin exponer el runner dentro del programa.

La declaración no sustituye a una librería de assertions. El lenguaje define
el test como unidad ejecutable; `assert` proporciona la comprobación mínima y
`std.testing` fija un núcleo sellado de control, metadata y tiempo virtual y
añadirá comparaciones, diffs y recursos de test como API ordinaria.

## 2. Compatibilidad y límite de edición

### 2.1 Tondo 0.1 permanece inmutable

Tondo 0.1 no contiene las keywords ni las declaraciones `test` y `suite`. Su
especificación, diagnósticos, grammar, formatter y suite de conformidad
permanecen byte a byte independientes de esta extensión.

Una implementación no puede anunciar soporte Tondo 0.1 y aceptar `test` o
`suite` como extensión silenciosa. Debe seleccionar explícitamente la edición
0.2 o una edición posterior que incorpore este contrato.

### 2.2 `test` y `suite` son keywords en Tondo 0.2

La edición 0.2 añade `test` y `suite` a la lista de palabras reservadas. Por
tanto:

- Ninguna de las dos puede utilizarse como identificador no calificado.
- Una función, variable, tipo, módulo o parámetro de usuario no puede llamarse
  `test` ni `suite`.
- La API estándar utiliza el nombre de módulo `std.testing`, no `std.test`.
- Código Tondo 0.1 que utilizara cualquiera de ambos nombres como identificador
  requiere renombrarlo al migrar de edición.

Reservarlas globalmente evita keywords contextuales cuya interpretación dependa
del source set o del lugar del parser.

`log`, `tags`, `failNow`, `skip`, `withVirtualTime`, `VirtualTime`, `settle` y
`advance` no son keywords ni nombres predeclarados. Son declaraciones del
módulo test-only `std.testing` y se resuelven mediante un `import` ordinario.

### 2.3 Una sola forma por concepto

No existen formas equivalentes como:

~~~text
@test
#[test]
test fn name()
test name(context)
fn testName(test: Test)
testing.register(...)
describe name { ... }
beforeAll(...)
afterAll(...)
~~~

Las dos formas canónicas son:

~~~text
test identifier block
suite identifier suite-block
~~~

### 2.4 Dependencia del contrato temporal de producción

El tiempo virtual no define una segunda API temporal exclusiva de testing. Su
implementación exige que la especificación estándar haya fijado antes un
sustrato mínimo compartido:

- `Duration` como valor portable con quantum canónico de nanosegundo, signo y
  overflow explícitos.
- `Instant` monotónico, sin relación implícita con calendario o zona horaria.
- Suspensión, timers y deadlines async ligados al executor.
- Identidad opaca de proveedor y rechazo de operaciones entre instantes de
  dominios distintos.
- Los puntos de cancelación, overflow, resolución y capabilities de esas
  operaciones.

`withVirtualTime` intercepta exactamente ese sustrato. El programa probado
continúa llamando a las mismas APIs que usaría en producción y no recibe un
`Clock` de test como parámetro. Calendario civil, timezone data y sincronización
con reloj de pared no son requisito de esta extensión y pueden permanecer en una
versión posterior de la librería.

El gate de testing no puede anunciar tiempo virtual conforme hasta que exista
esa especificación mínima y el adaptador de la VM la ejecute tanto con proveedor
monotónico real como virtual. Sus módulos y bytes deben entrar versionados en el
plan cerrado. El tracker puede secuenciar ese slice antes del resto de la
librería estándar, pero no sustituirlo por nombres, duraciones o bridges
privados del runner.

## 3. Declaraciones `suite` y `test`

### 3.1 Sintaxis

~~~tondo
test parsesNegativeNumbers {
    assert(parseInt("-12") == ok(-12))
}
~~~

Gramática:

~~~ebnf
top_decl_0_2  = top_decl_0_1 | test_decl | suite_decl ;
test_decl      = "test", identifier, block ;
suite_decl     = "suite", identifier, suite_block ;
suite_block    = "{", { NL | statement },
                 { suite_member, { NL } }, "}" ;
suite_member   = test_decl | suite_decl ;
~~~

`test_decl` puede aparecer en nivel superior o como miembro directo de una
`suite`. `suite_decl` puede aparecer en nivel superior o como miembro directo de
otra `suite`. Ninguna puede aparecer dentro de una función, cierre, `impl`,
trait, bloque ordinario, body de `test`, sentencia, `if`, `for` ni script.

El prefijo de un `suite_block` contiene cero o más sentencias ordinarias de
setup. Después del primer `suite_member` solo pueden aparecer otros miembros y
saltos de línea; no existe una expresión final ni un epílogo implícito. La
gramática conserva una suite vacía para recovery y tooling, pero la comprobación
emite `E2004`: cada suite válida contiene al menos un miembro directo. Puesto que
la regla se aplica recursivamente, toda suite válida contiene al menos un `test`
descendiente.

El parser de edición 0.2 reconoce ambos nodos dentro de una forma módulo y los
conserva aunque la comprobación posterior determine que la fuente es
`production`. Esa separación permite emitir `E2001` con el range completo. La
edición 0.1 sigue utilizando exactamente `top_decl_0_1`.

Ninguna declaración admite:

- `pub` ni `priv`.
- Parámetros.
- Parámetros genéricos ni constraints.
- Receptor.
- Anotación de retorno o de error.
- Modificadores `async` o `unsafe`.
- Atributos.
- Nombre string alternativo.

Una descripción humana opcional se escribe como documentación. El identificador
continúa siendo la identidad estable:

~~~tondo
/// Verifica el redondeo simétrico alrededor de cero.
test roundsHalfAwayFromZero {
    assert(round(1.5) == 2)
    assert(round(-1.5) == -2)
}
~~~

### 3.2 Árbol estático

`suite` es siempre un contenedor y `test` es siempre una hoja. Un test no puede
contener tests ni suites. Todas las sentencias del prefijo de una suite
pertenecen a su setup; un `assert` allí comprueba ese setup y su fallo pertenece
a la suite, no a uno de sus descendientes.

Los miembros directos de un mismo nivel forman un namespace de tooling único.
Dos miembros `suite`/`test` con el mismo identificador son `E2002`, incluso si
proceden de archivos distintos. Las suites no se reabren, fusionan ni extienden
por repetir su nombre.

El árbol completo se construye durante compilación. Nombres calculados, registro
runtime, suites dentro de control de flujo y generación de subtests desde un
`for` están prohibidos. Por tanto, `--list` nunca necesita ejecutar setup para
descubrir un nodo.

### 3.3 Nombre e identidad

Los nombres de suites y tests siguen la convención `camelCase` de las funciones.
Incumplirla produce el warning de naming ordinario. `_` es un descarte, no una
identidad de tooling, y no puede ocupar ninguna de estas posiciones.

La identidad semántica exacta de un nodo incluye:

~~~text
PackageId + source class + module path + ordered node path + node kind
~~~

Las identidades visibles del runner son:

~~~text
package-name::unit::module.path::testName
package-name::unit::module.path::suiteName::testName
package-name::integration::relative.path::testName
package-name::integration::relative.path::suiteName::nestedSuite::testName
~~~

Tooling la interpreta con la forma cerrada:

~~~ebnf
visible_node_id = package_name, "::", source_kind, "::",
                  logical_module_path, "::",
                  identifier, { "::", identifier } ;
source_kind     = "unit" | "integration" ;
~~~

El nombre de paquete mostrado es el nombre local declarado por el manifiesto;
la identidad interna conserva el `PackageId` completo para distinguir versiones
u orígenes diferentes. El segmento de clase evita que un unit test y una raíz
de integración produzcan selectores visibles ambiguos. Cada descriptor indica
además si el ID corresponde a una `suite` o un `test`; dos nodos de distinto kind
no pueden compartir el mismo ID.

Suites y tests forman un registro de tooling separado de los namespaces de tipos
y valores. Puede coexistir una función y un nodo con el mismo identificador:

~~~tondo
fn normalize(value: String): String {
    value
}

test normalize {
    assert(normalize("value") == "value")
}
~~~

Un nodo no puede referenciarse, importarse, llamarse ni convertirse a un valor
de función. La única relación entre suite y descendiente es la arista estática
registrada por el toolchain.

### 3.4 Formato canónico

El formatter emite:

~~~text
test name {
    body
}

suite name {
    setup

    test child {
        body
    }
}
~~~

Hay exactamente un espacio después de `test` o `suite` y otro antes de `{`. El
body de test utiliza las reglas ordinarias de bloques. En una suite, el formatter
separa mediante una línea vacía el setup de su primer miembro y dos miembros
consecutivos igual que dos declaraciones de módulo. Nunca transforma una función
en test, un test en suite ni infiere nodos a partir de nombres.

## 4. Semántica estática

### 4.1 Entradas ocultas

Cada test se comprueba como una entrada privada no direccionable equivalente a:

~~~text
async? fn <test-entry>(): Unit ! E
~~~

El símbolo sintético no puede escribirse ni observarse desde fuente. El body:

- Tiene resultado normal `Unit`.
- Infiere localmente una unión cerrada `E` de errores, como el `main` implícito
  de un script.
- Es infallible cuando no propaga ni produce errores.
- Se vuelve async cuando contiene una operación que exige suspensión, con las
  mismas reglas que el `main` implícito.
- Puede usar `return`, que sale únicamente del test actual.
- Puede usar `fail` y `?`; ambos alimentan el canal inferido `E`.

El error inferido debe cumplir `Discard`. Un error con obligación terminal no
puede escapar hasta el runner; debe transformarse o consumirse dentro del test.

Una expresión final distinta de `Unit`, un `return` con valor incompatible o
cualquier error de tipos utiliza los diagnósticos ordinarios. No existe un tipo
especial `TestResult`.

El prefijo de setup de cada suite se comprueba como otra entrada privada
equivalente a `async? fn <suite-setup>(): Unit ! E`, con unión cerrada inferida
`E: Discard`. Su entorno local permanece vivo mientras el runner ejecuta los
descendientes seleccionados y abandona después el scope léxico de la suite. Ese
entorno y sus entradas de ejecución no son valores Tondo, no tienen ABI pública
y no pueden observarse desde fuente.

El setup admite `fail` y `?`, que hacen fallar la fase, pero no `return`: una
suite no puede terminar normalmente ocultando descendientes registrados. La
transferencia de control que intente abandonar el setup utiliza `E1205`. Una
suite tampoco produce un `SuiteResult` visible; el runner conserva su lifecycle
en el reporte de tooling.

### 4.2 Async y concurrencia

No se escribe `async test` ni `async suite`. La necesidad de async se infiere
porque ninguna declaración forma parte de una API invocable.

~~~tondo
test loadsConfiguration {
    let config = await loadConfiguration()?
    assert(config.port > 0)
}
~~~

El setup de suite puede suspender y propagar errores:

~~~tondo
suite remoteApi {
    let service = await TestService.start()?
    let endpoint: String = service.endpoint()
    defer TestService.stop(service)

    test reportsHealth {
        assert((await readHealth(endpoint)?).ready)
    }
}
~~~

`scope`, `spawn`, `Join`, `Send`, `Share`, préstamos a través de suspensión,
cancelación y cleanup conservan exactamente la semántica general:

~~~tondo
test downloadsBothDocuments {
    scope {
        let first = spawn download("/first")
        let second = spawn download("/second")

        assert((await first?).length() > 0)
        assert((await second?).length() > 0)
    }
}
~~~

El scope raíz creado por el runner no cuenta como `scope` léxico para `spawn`.
Una suite o test escribe `scope` de forma explícita igual que un script.

### 4.3 Ownership, préstamos y `defer`

Una suite o test no relaja ownership. Todo valor afín, obligación terminal,
préstamo o cleanup debe satisfacer las mismas reglas que en una función normal.

`defer` es la única construcción de teardown del lenguaje:

~~~tondo
test writesAndReadsRecord {
    let workspace = createWorkspace()?
    defer Workspace.remove(workspace)

    writeRecord(workspace, "Ada")?
    assert(readRecord(workspace)? == "Ada")
}
~~~

Dentro de una suite, un `defer` registrado por el setup pertenece al scope
léxico de esa suite. Se ejecuta después de que terminen todos sus descendientes
seleccionados, en LIFO y con las reglas ordinarias. Si el setup falla antes de
alcanzar los miembros, se ejecutan los defers que ya hubiera registrado. No
existe una variante async de `defer` ni una excepción de testing a sus
restricciones de resultado y sincronía.

Un descendiente puede leer un binding de setup de sus suites ancestras solo
cuando:

- Fue declarado con `let`.
- Su tipo cumple `Copy + Send + Share`.
- La referencia no mueve, presta con `mut`/`var` ni consume el binding ancestral.

El runner construye un snapshot lógico de esas capturas para cada hijo antes de
programarlo. Un binding `var`, un préstamo `ref`/`mut`/`var`, un valor afín o un
valor con obligación terminal no puede cruzar esa frontera y produce `E2005`.
Constantes, funciones y declaraciones de módulo se resuelven por nombre y no son
capturas.

Así la suite conserva ownership del recurso costoso y expone a sus tests solo
datos compartibles:

~~~tondo
suite userApi {
    let service = TestService.start()?
    let endpoint: String = service.endpoint()
    defer TestService.stop(service)

    test createsUser {
        let client = ApiClient.connect(endpoint)?
        defer ApiClient.close(client)

        assert(client.createUser("Ada")?.name == "Ada")
    }
}
~~~

Compartir identidad deliberadamente requiere un tipo que satisfaga el mismo
contrato, por ejemplo un `Ref[T]` cuyo tipo completo cumpla `Send + Share`.
`suite` nunca convierte estado mutable ordinario en estado concurrente seguro.

Un pánico, error, `return` o cancelación estructurada de lenguaje de un test
ejecuta su unwind antes de entregar el resultado al runner. Los terminales de
suite siguen el lifecycle definido en la sección 7.

### 4.4 Unsafe

No existe `unsafe test` ni `unsafe suite`. Una operación raw requiere una región
`unsafe` local:

~~~tondo
test readsAlignedByte {
    let address = makeTestAddress()
    let value = unsafe {
        address.read()
    }

    assert(value == 42u8)
}
~~~

La presencia de una suite o test nunca rebaja las obligaciones de procedencia,
alineación, inicialización, aliasing o lifetime.

### 4.5 Visibilidad

Una suite o test unitario ve las declaraciones privadas del módulo al que
acompaña. Esa es una concesión estática de visibilidad, no reflection ni acceso
raw.

Una suite o test de integración pertenece a un consumidor separado y solo ve la
API pública importada. No existe una opción del runner que eleve su visibilidad.

Los campos privados, nombres ocultos y tipos opacos continúan ausentes de
diagnósticos y reportes cuando el test no tiene visibilidad válida.

### 4.6 Ausencia de efectos de importación

Un setup de suite o body de test solo se ejecuta cuando el runner invoca su
entrada. Importar un módulo que contiene un overlay de test no ejecuta nodos ni
registra callbacks en runtime.

El orden textual de suites y tests no produce inicialización global. Tondo
continúa sin globals mutables ni efectos de importación.

## 5. Source sets y descubrimiento

### 5.1 Clases de fuente

El plan cerrado de un build de test asigna a cada fuente exactamente una clase:

~~~text
production
unit-test
integration-test
~~~

La clase, path lógico, edición, target, capacidades y contenido forman parte de
la identidad del build de test. La clase no se deduce dentro del compilador a
partir de un path físico; el frontend recibe el plan ya resuelto.

Solo una fuente `unit-test` o `integration-test` puede contener `test_decl` o
`suite_decl`. Encontrar cualquiera en `production` produce `E2001`.

`std.testing` es un módulo test-only. Solo forma parte del grafo cerrado de un
artefacto de test; importarlo desde una fuente o dependencia activa de
producción produce `E2003`. Esta restricción cubre todo su núcleo sellado y los
helpers portables que se añadan posteriormente: ningún producto publicable
adquiere una dependencia sobre el runner.

### 5.2 Descubrimiento convencional de `tondo test`

Cuando el manifiesto no declara source sets de test explícitos, el comando
oficial aplica estas convenciones ASCII, case-sensitive:

- Un archivo con sufijo exacto `_test.to` situado dentro de un source root de
  producción es `unit-test`.
- Un archivo `.to` situado bajo el directorio de proyecto `tests/` es
  `integration-test`.
- Dentro de `tests/`, la regla de integración tiene precedencia aunque el
  archivo termine en `_test.to`.
- Ningún otro archivo se descubre implícitamente como test.

El toolchain convierte los paths físicos descubiertos en paths lógicos antes de
compilar, ordena esos paths por bytes UTF-8 y registra el conjunto exacto en el
artefacto de test.

La invocación descubre únicamente tests del paquete raíz seleccionado. No
ejecuta tests incluidos en dependencias normales o de desarrollo. Un workspace
con varios paquetes materializa una invocación cerrada por paquete y conserva
sus reportes separados o los agrega mediante un formato de workspace distinto.

Un manifiesto puede sustituir esas convenciones mediante source sets cerrados,
pero debe clasificar cada entrada como unit o integration. No puede cambiar la
semántica de ambas clases ni inventar una tercera con mayor visibilidad.

### 5.3 Unit test companions

Un archivo unitario acompaña al módulo derivado de sus fuentes hermanas:

~~~text
src/math.to
src/math_test.to
~~~

Ambos contribuyen al module path de producción. La compilación ocurre en dos
fases:

1. El módulo de producción se resuelve y comprueba sin fuentes de test.
2. Sobre esa unidad semántica sellada se aplica el overlay unitario.

El overlay puede leer declaraciones privadas y añadir imports, declaraciones
privadas auxiliares, suites y tests. No puede:

- Hacer que una fuente de producción inválida compile.
- Reabrir ni volver a resolver bodies de producción.
- Cambiar la interfaz pública, capacidades derivadas o artefacto de producción.
- Exportar una declaración `pub`.
- Ser importado desde un source set de producción.

Una colisión entre una declaración auxiliar y una declaración ya visible utiliza
las reglas ordinarias de nombres. Suites y tests permanecen en su registro
separado.

### 5.4 Integration test roots

Cada fuente descubierta bajo `tests/` actúa como raíz de un consumidor de test.
El toolchain le asigna un PackageId sintético de integración distinto al paquete
probado. El paquete probado y las dev-dependencies solo están disponibles bajo
los nombres declarados en el plan; todo import sigue siendo explícito.

La identidad visible conserva el nombre del paquete probado y utiliza el path
lógico relativo a `tests/`, sin extensión. Por ejemplo,
`tests/http/client.to` produce IDs bajo
`application::integration::http.client::...`; el PackageId sintético permanece
en la identidad semántica interna y evita conceder visibilidad privada.

Por ello una raíz de integración:

- No comparte scope de módulo con las fuentes de producción.
- No accede a declaraciones privadas.
- Puede contener funciones, tipos, constantes y helpers privados propios.
- No puede publicar una interfaz ni convertirse en dependencia de producción.

Dos raíces físicas no comparten declaraciones implícitamente. Helpers
compartidos se proporcionan mediante una dependencia de test o source set
explícito, con identidad y hash fijados.

### 5.5 Dev-dependencies y generación

Las dependencias de test forman un subgrafo cerrado del lockfile. Pueden ser
usadas por unit e integration tests, pero no quedan disponibles al compilar el
target de producción.

Una fuente generada puede ser test solo si su salida declarada especifica la
clase correspondiente. Generadores de test conservan las mismas restricciones
de red, reloj, entorno y aleatoriedad declarada que cualquier otro generador.

El build de producción debe producir la misma interfaz y artefacto
independientemente de que existan, cambien o desaparezcan tests y
dev-dependencies.

### 5.6 Invocación o selección vacía

Una invocación normal que descubre cero tests hoja o cuyo selector no selecciona
ninguno falla con un diagnóstico de tooling. No se considera éxito silencioso.
Una declaración `suite` vacía ya fue rechazada estáticamente y no altera este
conteo. `--allow-empty` permite solicitar explícitamente exit status exitoso
cuando discovery o el selector básico puedan no producir tests.

Una selección no vacía puede producir un shard vacío. Ese resultado es válido y
termina con éxito sin exigir `--allow-empty`: el conjunto anterior al shard sí
existía y la partición simplemente no asignó ninguna hoja a ese índice.

### 5.7 Ownership mediante CODEOWNERS

El plan de test puede asociar owners estáticos a cada suite y test a partir de
un archivo CODEOWNERS. `--codeowners auto`, que es el default, busca en este
orden desde la raíz lógica del repositorio y usa únicamente el primer archivo
existente:

~~~text
.github/CODEOWNERS
CODEOWNERS
docs/CODEOWNERS
~~~

`--codeowners <path>` selecciona un archivo explícito y
`--codeowners none` desactiva la resolución. Un path explícito es lógico,
relativo a la raíz, no contiene `.`/`..`, no escapa mediante symlink y debe
resolver a un archivo regular legible; incumplirlo es error de tooling. En modo
`auto`, un candidato existente pero inválido o ilegible falla y no provoca
fallback al siguiente. La raíz del repositorio y el path elegido se
materializan antes de compilar; su forma física no aparece en identidades ni
reportes.

El archivo debe ser UTF-8 sin BOM. Se divide por `LF`, se elimina un `CR` final
y se separa cada línea por espacios o tabs ASCII. Blank lines y líneas cuyo
primer carácter no blank es `#` se ignoran. Una línea activa contiene
exactamente un pattern no vacío seguido de uno o más owners no vacíos. No
existe comentario inline.

Tondo implementa el subset portable fijado aquí, no extensiones particulares de
un proveedor. Cada pattern se compara de forma case-sensitive por scalars
Unicode contra el path lógico relativo a la raíz, con `/` como único separador:

- Un `/` inicial no forma parte del match y obliga a comenzar en el primer
  segmento.
- Sin ese anclaje, un pattern que no contiene `/` puede coincidir con cualquier
  segmento completo.
- Un pattern sin `/` que coincide con un segmento no final cubre su subárbol;
  si está anclado, solo puede hacerlo desde el primer segmento.
- Cualquier otro pattern se compara con el path completo desde la raíz.
- Un `/` final equivale a añadir `**` y cubre el subárbol.
- `*` consume cero o más scalars salvo `/`; `?` consume exactamente uno salvo
  `/`; `**` consume cero o más scalars y puede cruzar `/`.
- El resto de scalars es literal. Segmentos vacíos interiores, `.` y `..` son
  inválidos.

El subset no admite negación `!`, rangos `[]`, backslash ni escape de un `#`
inicial. La última línea coincidente gana y aporta todos sus owners conservando
el orden textual y duplicados.

Un owner es el token UTF-8 opaco no vacío delimitado anteriormente, normalmente
`@usuario`, `@organización/equipo` o un email. El runner nunca valida la
identidad contra red, membresía, visibilidad ni permisos del proveedor. Un
archivo o línea activa inválidos producen un diagnóstico de tooling: Tondo no
ignora ownership roto de forma silenciosa.

Cada nodo se resuelve contra el archivo lógico que contiene su declaración. Por
ello todos los nodos declarados en un mismo archivo reciben el mismo resultado
CODEOWNERS. Una fuente generada utiliza su origin path declarado; sin un origen
lógico dentro del repositorio obtiene `owners: []`. Ausencia de archivo o de
match también produce el array vacío y no falla por defecto.

El plan registra modo, source path lógico, bytes y SHA-256 del CODEOWNERS
efectivo. Cambiarlo altera el artefacto y reportes de test, pero nunca la
interfaz ni el producto de producción.

## 6. Construcción del target de test

### 6.1 Compilación completa antes de ejecutar

El runner debe resolver, comprobar, bajar y verificar todas las fuentes activas
antes de iniciar la primera entrada de usuario. Si existe cualquier error de
compilación:

- No se ejecuta ningún test.
- Se emiten los diagnostics ordinarios.
- El comando termina como fallo.

Un filtro limita ejecución, no compilación. Tests no seleccionados continúan
typecheckeándose para evitar que código roto quede oculto por una invocación
parcial.

### 6.2 Artefacto de test

El artefacto contiene:

- Identidad del package graph y lockfile.
- Edición, target, perfil y capacidades.
- Source sets y paths lógicos activos.
- Árbol ordenado de suites/tests, parent IDs y source ranges.
- Configuración y resultado estático de ownership por nodo.
- Entradas privadas de setup y de tests hoja.
- Operaciones verificadas de control de testing y la asociación de cada entrada
  con su envelope privado de ejecución.
- Operaciones verificadas de dominio temporal y su catálogo cerrado de puntos
  de suspensión duraderos.
- Layout comprobado de los snapshots `Copy + Send + Share` que cruzan cada
  arista del árbol.
- Hashes necesarios para reproducir el build.

No contiene un registro mutable de funciones ni descubre tests mediante
reflection en runtime.

El artefacto de test no es una biblioteca publicable ni una dependencia válida.
Su ABI de entrada es interna al toolchain y no forma parte de la ABI pública de
Tondo.

### 6.3 Relación con `main`

Un target de test no busca ni ejecuta `main`. Si el módulo probado contiene un
`main` válido, se comprueba como código de producción pero no actúa como entry
point del test.

Una fuente de test no puede contener sentencias top-level ni un script raíz. Los
helpers son declaraciones y los efectos comienzan dentro del setup de una suite
seleccionada o del body de un test hoja. Un helper puede invocar
`std.testing`, pero la operación solo se observa al alcanzarlo desde una de esas
entradas.

## 7. Modelo de ejecución

### 7.1 Árbol de ejecución y aislamiento

Después de seleccionar tests hoja, el runner construye el bosque mínimo que los
contiene: cada test seleccionado y todas sus suites ancestras. Ningún otro nodo
se ejecuta.

Cada test hoja obtiene:

- Un scope raíz nuevo.
- Estado de runtime, roots, tasks y handles no observable desde otra hoja salvo
  los snapshots de suite permitidos por 4.3.
- Un envelope privado de control, un buffer ordenado de logs y un mapa de tags.
- Un registro inicialmente vacío de dominios temporales virtuales pertenecientes
  a ese intento.
- Captura separada de stdout y stderr del runtime Tondo.
- Presupuesto de recursos independiente.
- Un resultado independiente.

Cada suite ejecutada obtiene un entorno de lifecycle separado que conserva
sus bindings de setup y su pila de cleanup hasta que terminan los descendientes.
Los hijos solo observan sus snapshots `Copy + Send + Share`; no reciben acceso
general al heap, stack, préstamos ni propietarios de la suite.
Cada participación de suite comienza además con su propio registro vacío de
dominios virtuales; un dominio abierto y cerrado durante setup o teardown nunca
se hereda por los envelopes de sus descendientes.

Una implementación puede reutilizar threads, allocators o procesos internos
solo si esa reutilización no expone otros valores, roots, tasks, handles,
buffers, pánicos u output. No existen globals mutables Tondo que sobrevivan
entre entradas. Esta libertad no aplica a una unidad de retry, cuya frontera de
proceso nueva es obligatoria según 8.7. Los efectos externos —filesystem,
procesos, red, reloj o servicios— no se revierten mágicamente y deben aislarse
mediante nombres, recursos y cleanup explícitos.

### 7.2 Contexto estructurado del runner

Cada entrada ejecutada queda asociada internamente a un envelope conceptual:

~~~text
TestExecution {
    node_id
    log_sink
    tag_sink
    stdout_sink
    stderr_sink
    virtual_time_domains
    cancellation
    limits
}
~~~

Esta forma explica el contrato; no declara un record Tondo. El envelope:

- No es un valor, parámetro, binding, tipo ni capability visible desde fuente.
- No puede obtenerse, almacenarse, moverse, compararse, imprimirse ni
  falsificarse.
- Pertenece a la raíz de ejecución que el runner ya conoce al invocar el ID de
  suite o test.
- Acompaña a frames y tasks, no al thread del host. Llamadas, closures y tasks
  creadas mediante concurrencia estructurada heredan el mismo enlace aunque
  migren entre workers.
- Deja de existir únicamente después de completar hijos y cleanup. Ninguna task
  puede registrar eventos una vez finalizado el nodo.

Cada test hoja recibe un envelope distinto. El setup y teardown de una suite
comparten el envelope de esa suite; sus descendientes reciben otros envelopes y
no pueden escribir logs ni tags en el de la suite ancestral. No existe herencia
implícita de tags entre suite y descendientes.

Las operaciones selladas de `std.testing` emiten un evento o terminal hacia el
envelope activo. No consultan una API `currentTest()`: el runner atribuye el
evento al nodo cuya entrada está conduciendo. Por ello funcionan desde helpers
y tasks estructuradas sin un parámetro `TestContext`, pero no introducen un
global mutable ni un thread-local observable.

`withVirtualTime` añade temporalmente un dominio al mismo envelope. Las tasks
creadas dentro de su closure heredan ese dominio por la raíz estructurada, no
por el thread del host. El controlador `VirtualTime` es un préstamo explícito
para dirigir el reloj; no da acceso a identidad, estado, sinks ni políticas del
test.

HIR, MIR y bytecode representan estas operaciones mediante un catálogo cerrado.
Sus verifiers solo las admiten dentro de un artefacto de test y ningún backend
puede implementarlas como una escritura a un contexto global compartido.
`std.testing` no exige la capability `console`; sus logs y tags utilizan canales
del runner, no stdout ni stderr. El dominio virtual tampoco concede una
capability de host ausente: sustituye únicamente el proveedor monotónico de las
APIs temporales que ya fueron admitidas para el target.

### 7.3 Lifecycle de suite

En cada participación de una suite en la ronda inicial o en una unidad de retry,
la suite se ejecuta de esta forma:

1. Si no contiene ningún test seleccionado, no se entra en ella.
2. El runner ejecuta su setup exactamente una vez para esa participación.
3. Tras éxito, materializa los snapshots permitidos y ejecuta sus miembros
   seleccionados. Una suite hija repite el mismo protocolo.
4. El fallo de una hoja se registra y no impide ejecutar sus hermanos.
5. Cuando han terminado todos los descendientes seleccionados, el runner
   abandona el scope de la suite y ejecuta su cleanup ordinario. Por estructura,
   suites internas terminan antes que sus ancestras.

No existe lifecycle por orden textual entre hojas. Todos los miembros de una
suite deben poder ejecutarse en cualquier orden y un test individual seleccionado
mediante `--exact` ejecuta primero las mismas suites ancestras.

Si el setup termina por error, pánico, resource limit, timeout o infraestructura:

- La suite conserva ese estado y fase `setup`.
- Se ejecuta el unwind de lenguaje que realmente sea observable, incluidos los
  `defer` ya registrados cuando el terminal lo permita.
- Ningún descendiente suyo comienza.
- Cada test hoja seleccionado bajo ella queda `blocked-setup` e identifica la
  suite que lo bloqueó.
- Suites y tests hermanos fuera de ese subárbol continúan cuando el aislamiento
  sigue siendo fiable.

Si `testing.skip(reason)` alcanza el runner como terminal primario del setup:

- El runner completa hijos estructurados y cleanup ya registrado.
- Si ese cleanup termina correctamente, la suite queda `skipped` con fase
  `setup` y conserva la razón una sola vez.
- Ningún descendiente comienza; sus suites y hojas seleccionadas quedan
  `blocked-skip` y señalan mediante `blocked_by` la suite que conserva la razón.
- Suites y tests hermanos fuera del subárbol continúan.

Un pánico, resource limit, timeout o fallo de infraestructura durante ese
cleanup prevalece sobre el skip. La suite conserva el fallo con fase `setup` y
sus descendientes pasan a `blocked-setup`; un skip nunca oculta cleanup
incompleto.

Si un `defer` produce pánico durante el unwind de un setup fallido, se aplica la
prioridad de terminales de Tondo 0.1 y la fase continúa siendo `setup`: la suite
nunca llegó a admitir descendientes. `teardown` designa exclusivamente el
cleanup iniciado después de que un setup correcto y todos sus descendientes
seleccionados hayan terminado.

Un fallo durante ese teardown conserva fase `teardown`. Los resultados ya
producidos por los descendientes no cambian ni se reetiquetan; la suite añade su
propio fallo y hace fallar la invocación. El estado `passed` de una suite
significa únicamente que su setup y teardown terminaron correctamente, no que
todos sus descendientes pasaron.

Este lifecycle equivale a setup/teardown una vez por contenedor y participación,
no una vez global por invocación. No implica `beforeEach` ni `afterEach`: cada
hoja construye sus fixtures propias mediante helpers y `defer`.

### 7.4 Inicio y terminación de entradas

El runner conduce cada body de test y setup de suite —síncronos o async según la
inferencia ordinaria— y cada teardown síncrono hasta uno de estos terminales:

- Retorno normal.
- Error recuperable no manejado.
- Pánico.
- Skip solicitado desde body de test o setup de suite.
- Límite de recursos.
- Timeout del runner.
- Fallo de infraestructura.

Antes de registrar un terminal de lenguaje, el runtime ejecuta cleanup y cancela
y espera hijos estructurados según la especificación base. Una entrada no se
marca como finalizada mientras quede cleanup estructurado pendiente. El tiempo
durante el que una suite solo espera a sus descendientes no ejecuta código de
usuario y no constituye una cuarta fase.

Un skip de test solo se confirma después de completar ese protocolo. Si un
pánico o terminal forzado impide el cleanup, prevalece el fallo.

Si `testing.skip` se ejecuta en una task hija, el envelope marca la entrada
completa como pendiente de skip, solicita cancelación a sus hermanas y lo
propaga a la propietaria en su siguiente punto de cancelación o al abandonar el
`scope`, igual que un pánico de hijo según 11.14 de la especificación base. No se
limita a terminar silenciosamente el hijo. Antes de confirmar el resultado se
espera el cleanup estructurado de todos ellos.

Si varias tasks hijas solicitan skip antes de completar ese teardown, la creada
primero por orden de evaluación aporta la razón. Un skip ya producido por la
propietaria conserva prioridad sobre skips derivados de sus hijos. Cualquier
pánico del propietario o de un hijo prevalece sobre todos los skips.

`testing.skip` no puede utilizarse para abandonar cleanup. Si se invoca durante
un `defer`, unwind o teardown de suite, produce `P2001` y el nodo falla como
`failed-panic`; un pánico primario anterior conserva la precedencia ordinaria.
`testing.log`, `testing.tags` y `testing.failNow` sí conservan su significado
durante cleanup: las dos primeras registran contexto y metadata en el nodo
activo y la última produce el pánico `P0007`.

### 7.5 Pánico y continuidad

Un pánico termina el test actual después del unwind, no el proceso completo del
runner. El runner conserva el código `P`, ubicación y stack trace disponibles y
continúa con tests posteriores cuando el aislamiento sigue siendo válido.

Un pánico en setup o teardown pertenece a la suite y sigue 7.3. Un abort fuera
del modelo de pánico, corrupción del runtime o imposibilidad de restablecer
aislamiento se clasifica como fallo de infraestructura. El runner puede detener
el bosque restante porque ya no puede garantizar resultados fiables. En ese
caso termina con exit `3` y no emite un reporte canónico incompleto; todo reporte
`tondo-test-report-0.2/6` válido clasifica cada hoja seleccionada.

### 7.6 Errores recuperables

Un valor que alcanza el canal `E` inferido hace fallar el test o fase de suite
actual. El reporte conserva como mínimo la identidad nominal visible de su tipo
y la ubicación del terminal. Su presentación humana sigue las reglas de la
frontera de `main`: no usa reflection para revelar campos privados ni promete
serialización estable del payload.

Para verificar un error esperado, el test lo consume localmente con `match`; no
deja que alcance al runner.

### 7.7 Inputs de host

El runner no proporciona parámetros mágicos a suites ni tests. Los argumentos
de proceso del programa Tondo son vacíos.

Entorno, cwd, filesystem, red, reloj, entropía y procesos solo existen mediante
sus APIs y capabilities normales. El plan de test registra los inputs de host
declarados. La invocación oficial:

- Usa como cwd inicial el directorio de proyecto asociado al manifiesto cuando
  el target ofrece cwd; el path físico no entra en hashes ni reportes.
- Proporciona un environment vacío salvo entradas explícitas materializadas en
  el plan de test por el contrato del toolchain.
- No concede una capability ausente en el target de producción únicamente por
  tratarse de un test.

Una implementación puede ofrecer un target de test con capacidades adicionales,
pero ese target es distinto y debe quedar visible en el reporte.

Dentro de 7.9, las APIs monotónicas admitidas conservan sus firmas y capability,
pero resuelven el proveedor virtual del dominio. Ese proveedor y su cero no son
inputs de host ni permiten acceder a una API temporal que el target rechazaba.

### 7.8 Límites y timeouts

Cada test hoja y cada fase activa de setup o teardown se ejecuta con límites
finitos de instrucciones o trabajo, memoria, profundidad y output. Los bytes de
keys/values de `testing.tags`, logs, stdout y stderr consumen el presupuesto de
output del mismo nodo; metadata y logs no ofrecen canales ilimitados
alternativos. Dominios virtuales, timers, cola ready y sus descriptores consumen
los presupuestos de trabajo/memoria/metadata del mismo intento; un loop no puede
crear reportes ilimitados abriendo dominios secuenciales. El toolchain publica
sus defaults y registra los valores efectivos o el hash de su resource profile
en el reporte.

`--timeout` aplica de forma independiente a un body de test, a un setup de suite
y a un teardown de suite. El reloj de una suite se pausa mientras solo espera
descendientes; por tanto, una suite grande no consume el timeout por sumar la
duración de sus tests.

Un timeout wall-clock es un límite del runner, no un error Tondo ni un pánico. El
runner debe poder terminar la entrada aislada incluso si el código no llega a un
punto cooperativo de cancelación. No puede dejar un proceso o thread de usuario
ejecutándose después de reportar el terminal. Se mide con un reloj monotónico
real exterior al envelope y nunca se sustituye ni avanza mediante
`withVirtualTime`.

Los presupuestos estructurales y de runtime siempre permanecen finitos. El
timeout wall-clock puede desactivarse únicamente mediante `--timeout none`; esa
opción no desactiva ningún otro presupuesto. Un timeout o resource limit nunca
se convierte automáticamente en test ignorado.

Timeout y agotamiento de presupuesto pueden impedir que el código Tondo complete
sus `defer`; no son terminales del lenguaje con garantía de unwind. El runner sí
debe limpiar su propia frontera de aislamiento y declarar el estado
correspondiente. Una suite o test con efectos externos no puede confiar en
teardown de usuario después de una terminación forzada.

### 7.9 Dominio de tiempo virtual determinista

`testing.withVirtualTime` ejecuta una closure async exactamente una vez dentro
de un dominio temporal nuevo. El dominio pertenece al intento y a la fase que
alcanzó la llamada; no sustituye el reloj de otras hojas, otras fases, suites
descendientes ni código ejecutado antes o después de la closure.

Forma canónica:

~~~tondo
import std.testing
import std.time

test requestTimesOut {
    await testing.withVirtualTime(async (clock) {
        scope {
            let timeout: time.Duration = requestTimeout()
            let result = spawn requestWithTimeout(timeout)

            await clock.advance(timeout)

            assert(await result == RequestOutcome.TimedOut)
        }
    })?
}
~~~

`withVirtualTime` no sustituye a `scope`: todo `spawn` conserva su propietario
léxico y ninguna task puede sobrevivir a la closure. El cierre recibe
`ref VirtualTime`; el controlador no puede moverse, almacenarse ni escapar de
esa región. La closure devuelve `Unit`, puede ser fallible y propaga su unión de
error al body o fase exterior sin envolverla. Pánico, skip, cancelación y cleanup
conservan sus reglas ordinarias y siempre desmontan el dominio antes de terminar
el intento.

Dentro del dominio:

- El instante monotónico inicial es un cero virtual opaco. Solo sus diferencias
  son observables mediante la API temporal de producción.
- Cada `Instant` conserva internamente la identidad opaca de su proveedor y
  dominio. Un `Duration` puede cruzar dominios; un `Instant`, timer o deadline no
  puede mezclarse con otro dominio. La API estándar debe rechazar esa mezcla de
  forma determinista en lugar de comparar contadores de relojes distintos.
- Suspensión, timers y deadlines monotónicos admitidos por el target consultan
  el proveedor virtual. No se reescribe el código ni se inyecta un `Clock`.
- El calendario civil y las APIs que consulten explícitamente reloj de pared no
  se virtualizan en esta edición.
- La cola ready es determinista: tareas listas se ordenan por su secuencia de
  creación y un nuevo wake se añade después de las ya listas. Timers con el
  mismo deadline se despiertan por secuencia de creación.
- Una task terminada deja de participar. Una task está **durablemente
  bloqueada** solo cuando el runtime puede demostrar que despertará únicamente
  por un timer virtual, por terminación o cancelación de otra task del dominio,
  o por una operación de sincronización cuyo catálogo estándar la declare
  durable y cuyo ownership demuestre que todos sus productores, endpoints y
  posibilidades de wake permanecen confinados al dominio. Si el tipo o análisis
  no puede demostrarlo, la espera no es durable.
- Esperas sobre filesystem, red, procesos, reloj de pared, syscalls, callbacks
  o recursos que puedan recibir eventos externos nunca son duraderas. No se
  aceleran ni se simulan; doubles y transportes in-memory controlados son la
  forma portable de probar esos casos.

Cuando la task raíz de la closure está esperando en `settle` o suspendida de
forma durable y todas las demás tasks vivas están terminadas o duraderamente
bloqueadas, el scheduler aplica exactamente una de estas acciones:

1. Si existe una llamada activa a `settle`, la completa sin mover el reloj.
2. En otro caso, si existe al menos un timer pendiente, avanza al deadline
   menor, despierta todos los timers de ese instante en su orden normativo y
   continúa ejecutando.
3. Si no existe un evento interno capaz de progresar, produce `P2003`
   `test-virtual-time-deadlock`.

Por ello un `await` ordinario puede atravesar horas de backoff virtual sin
consumirlas en tiempo real. El reloj nunca avanza automáticamente mientras haya
una task runnable o una espera externa no durable.

`await clock.settle()` suspende al caller y conduce las demás tasks hasta que
cada una termina o queda duraderamente bloqueada en el instante actual. Retorna
antes de cualquier salto automático a un timer futuro. Esto permite comprobar
un estado intermedio o la ausencia de un efecto sin elegir una espera
wall-clock.

`await clock.advance(duration)` exige una duración no negativa y fija un target
exacto `now + duration`. Visita en orden cada deadline no posterior al target,
ejecuta las tasks despertadas
hasta quiescencia en ese instante y finalmente retorna con `now == target`;
durante esa llamada no salta más allá del target. Una duración cero drena el
trabajo debido en el instante actual. Una duración negativa o un overflow del
rango virtual produce `P2005` `test-virtual-time-range` sin wraparound ni
retroceso.

Un envelope puede tener como máximo un dominio activo. Anidar
`withVirtualTime` o abrirlo simultáneamente desde dos tasks hermanas del mismo
intento produce `P2004` `overlapping-test-virtual-time` antes de ejecutar la
segunda closure. Tests distintos pueden mantener dominios paralelos y un mismo
intento puede crear varios dominios secuenciales; cada uno vuelve a su cero
virtual y obtiene un índice creciente.

La garantía de determinismo cubre únicamente eventos gobernados por el dominio.
Si el código realiza I/O externo, su terminación y orden continúan siendo
observaciones del host. Una espera externa puede completar normalmente, pero
impide declarar quiescencia mientras siga pendiente y queda protegida solo por
el timeout wall-clock y los límites reales del runner.

El timeout de 7.8, la duración JUnit, límites de CPU/instrucciones, memoria y
output siempre utilizan recursos reales. Un loop runnable, un timer periódico
sin condición terminal o una espera externa atascada no puede esconderse detrás
del reloj virtual.

## 8. Selección, orden y paralelismo

### 8.1 Orden canónico

El registro, los descriptores y los resultados de suites y tests se ordenan
lexicográficamente por identidad visible completa usando bytes UTF-8. Los
descendientes de una suite forman así un rango contiguo.

La ejecución por defecto utiliza `--jobs 1 --order canonical` y recorre el árbol
en ese orden, entrando en una suite antes de su primer descendiente seleccionado
y abandonándola después del último. Esto proporciona feedback reproducible sin
depender del número de CPUs de la máquina.

### 8.2 Selección

El runner ofrece exactamente tres selectores:

- `--filter text`: substring bytewise, case-sensitive, sobre la identidad
  visible completa de cada test hoja.
- `--glob pattern`: glob portable con match completo sobre la identidad visible
  de cada test o suite.
- `--exact id`: igualdad bytewise con la identidad visible de un test o suite.

Son mutuamente excluyentes y cada uno aparece como máximo una vez. No existe un
selector regex en esta edición.

El glob trata `::` como separador de componentes y admite únicamente:

- `*`: cero o más Unicode scalars dentro de un componente, nunca `:`.
- `?`: exactamente un Unicode scalar dentro de un componente, nunca `:`.
- `**`: cero o más componentes completos, solo cuando constituye por sí mismo
  un componente.

Los demás scalars son literales. El match es case-sensitive, independiente de
locale, sin normalización Unicode automática y sobre el ID completo; nunca es
un match de prefijo implícito. El runner no realiza expansión de shell,
filesystem ni environment. No existen character classes, braces, escapes,
alternativas ni operadores regex.

Un pattern glob es inválido si está vacío, contiene un componente vacío, un `:`
aislado, `**` dentro de otro componente, dos componentes `**` adyacentes o dos
`*` consecutivos dentro de un componente. Esta última regla reserva `**` para
su única forma estructural y mantiene una representación canónica. El matching
debe ser determinista y acotado; una implementación conforme puede utilizar
programación dinámica con complejidad
`O(pattern_scalars * id_scalars)` y no puede introducir backtracking
exponencial.

Un exact o glob match de test selecciona esa hoja. Un exact o glob match de
suite selecciona todos sus tests descendientes. Un mismo pattern puede hacer
match con una suite y con hojas ya incluidas por ella; la unión se deduplica por
ID antes del shard. Puesto que el ID de suite es prefijo de sus descendientes,
un filtro que contiene su path los selecciona naturalmente, pero `--filter`
nunca devuelve una suite sin hojas.

Toda selección incorpora las suites ancestras necesarias. `--list` compila y
valida el árbol, aplica el selector y emite los tests seleccionados junto con
esas suites sin ejecutar ningún setup ni body.

Un selector sintácticamente válido sin matches sigue la regla de selección
vacía de 5.6. Un glob inválido termina con exit `2` antes de materializar inputs
o compilar. Como un shell puede interpretar `*` y `?` antes de invocar al
programa, la forma portable en una shell es citar el argumento, por ejemplo
`--glob 'application::integration::**::creates*'`.

`testing.tags` se ejecuta dentro de una entrada y, por tanto, no participa en
discovery ni selección. No existe `--tag` en esta edición. Añadir filtrado por
tags requerirá metadata declarativa disponible antes de ejecutar código; el
runner nunca ejecuta setup o bodies para decidir qué debe ejecutar.

### 8.3 Sharding estable

`--shard index/count` particiona las hojas obtenidas por 8.2, con índices
humanos desde `1` y `1 <= index <= count`. Ambos componentes son enteros
decimales positivos sin signo ni padding y la opción aparece como máximo una
vez.

Cada ID visible se asigna exactamente a:

~~~text
1 + uint256_be(
    SHA-256("tondo-test-shard-v1\0" || UTF8(test_id))
) mod count
~~~

El string de dominio contiene los bytes ASCII mostrados y un byte `00`; el
digest completo se interpreta como entero big-endian sin signo. El algoritmo
estable se identifica como `sha256-mod-v1`.

Vector normativo: para
`application::unit::math::arithmetic::addReturnsSum`, el digest es
`ee5252232b68a78e79fc22b6e8d761a22e2989369358efc402802d22989f2517`;
con `count = 8` la hoja pertenece al shard `8/8`.

La partición ocurre después de `--filter`/`--glob`/`--exact` y antes del orden
de ejecución. Para una selección e igual `count`:

- Cada hoja pertenece a un único shard.
- Dos shards distintos son disjuntos.
- La unión de `1/count` a `count/count` reconstruye exactamente la selección.
- Cambiar el orden de archivos, `--jobs` o la seed no cambia la asignación.

El runner compila el target completo en cada shard. Cada invocación construye el
bosque mínimo de su propia partición; una suite con hojas en varios shards
ejecuta setup y teardown independientemente en cada proceso. No existe fixture,
envelope ni estado compartido entre shards.

`--list` aplica el shard y permite inspeccionar su plan sin ejecutar código. Una
selección previa vacía conserva la regla de 5.6; un shard vacío derivado de una
selección previa no vacía es un resultado válido.

### 8.4 Orden de ejecución y randomización reproducible

`--order canonical|random` elige la prioridad de dispatch y aparece como máximo
una vez. `canonical` es el default. `--seed hex` solo es válido con
`--order random`; acepta de uno a dieciséis dígitos ASCII hexadecimales y se
normaliza en reportes como dieciséis dígitos lowercase. No admite prefijo
`0x`, signo, separadores ni whitespace.

Si `random` no recibe seed, el runner obtiene un `U64` de la entropía del host
durante la materialización del plan y antes de compilar, y lo registra. La salida
humana lo muestra siempre; la salida JSON y JUnit lo conserva en sus campos
normativos. No existe una seed ambiental ni oculta. Esa seed generada se
convierte en un input efectivo de la invocación: para reproducirla debe
reutilizarse explícitamente el valor mostrado.

El modo random ordena los miembros directos seleccionados —tests y suites— de
cada padre por el digest:

~~~text
SHA-256(
    "tondo-test-order-v1\0" ||
    seed_u64_be ||
    00 ||
    UTF8(parent_id) ||
    00 ||
    UTF8(child_id)
)
~~~

La raíz utiliza `parent_id` vacío. Los digests se comparan completos en orden
bytewise ascendente y los empates se resuelven por ID bytewise. El algoritmo se
identifica como `sha256-tree-v1`; no depende del PRNG, hash map ni scheduler del
host.

Con seed normalizada `0000000000005eed` y parent
`application::unit::math::arithmetic`, los hijos:

~~~text
application::unit::math::arithmetic::subtractReturnsDifference
  -> 00c637b1e275874ed716704cd93b6b8928b23d378fd457c41a38060684094a68
application::unit::math::arithmetic::addReturnsSum
  -> 9bd8be59d09c20c60e7549466be069ce52f98d8cff6d608878d194029e9650cf
~~~

aparecen en ese orden.

Una suite permanece estructuralmente atómica en la representación del plan:
ejecuta setup, recorre su subárbol ordenado y ejecuta teardown. Los descendientes
permanecen contiguos en el plan; esto no añade exclusión mutua entre entradas
que `--jobs N` permite ejecutar concurrentemente. El modo canonical usa
`id-byte-order-v1`. Ambos modos publican `execution_plan`, un array de IDs hoja
en prioridad de dispatch; los arrays de descriptores y resultados continúan
ordenados por ID.

Con `--jobs 1`, repetir selección, shard y seed reproduce exactamente el orden
observable. Con `--jobs N`, la seed reproduce la prioridad de dispatch, no el
interleaving wall-clock de entradas concurrentes.

### 8.5 Paralelismo explícito

`--jobs N`, con `N > 0`, permite ejecutar simultáneamente hasta `N` entradas de
usuario entre setup, test y teardown. Una suite termina su setup antes de
programar hijos y no comienza teardown hasta que todos sus descendientes
seleccionados han terminado.

El resultado final, los logs y la salida capturada continúan presentándose por
nodo en orden canónico, no en orden de finalización. Eventos de dos envelopes
distintos nunca se intercalan dentro del buffer del otro.

El runner no garantiza un orden de efectos externos entre tests paralelos. Una
suite que comparta un servicio bajo `--jobs N` es responsable de que ese servicio
admita concurrencia o de solicitar `--jobs 1`. Los snapshots estáticos impiden
data races de lenguaje; el aislamiento de runtime y el máximo global permanecen
obligatorios.

### 8.6 Sin dependencias entre tests

No existe sintaxis para ordenar tests, declarar dependencias ni compartir una
fixture mutable ordinaria entre ellos. Una suite puede compartir snapshots
inmutables y efectos externos explícitos, pero ningún hijo puede preparar estado
para otro.

Cada test debe poder ejecutarse solo mediante `--exact`; el runner entra en las
mismas suites ancestras y debe producir el mismo resultado de lenguaje que
dentro del árbol completo con los mismos inputs declarados. Sharding y
randomización existen para descubrir dependencias accidentales, no para
legitimarlas.

### 8.7 Retries explícitos, acotados y aislados

Sin `--retry`, cada nodo participa únicamente en la ronda inicial. La opción
`--retry N`, con `N >= 0`, autoriza como máximo `N` rondas adicionales. El
default es `0`; el runner nunca infiere retries desde historial, tags, nombre,
owners o estado de CI.

Solo son elegibles los estados `failed-error`, `failed-panic` y `timeout`.
`resource-limit` e `infrastructure` no se reintentan porque indican que el mismo
perfil no puede garantizar una nueva ejecución fiable. Tampoco se reintentan
fallos de compilación, `skipped`, `blocked-skip` ni un bloqueo como causa
independiente. Un nodo bloqueado por una suite fallida puede volver a ejecutarse
como parte de la unidad que reintenta esa causa.

El estado agregado conserva fallos previos según 9.1, pero la planificación no
puede usarlo para reintentar indirectamente un terminal excluido. Si la
participación más reciente de un candidato terminó en `skipped`,
`blocked-skip`, `resource-limit` o `infrastructure`, no genera otra unidad. Si
terminó `blocked-setup`, la suite causal genera la unidad cuando su fallo es
elegible; si esa suite ya quedó agregada como `flaky-pass`, el nodo todavía
fallido puede generar su propia unidad; y si la causa conserva un terminal
excluido, no se reintenta el subárbol. Así un skip o fallo de infraestructura
nunca se convierte accidentalmente en retry.

La ronda `0` ejecuta el plan original completo. Una ronda posterior solo
comienza después de que todas las entradas y cleanup de la ronda anterior hayan
terminado. Al cerrarla, el runner calcula el estado agregado de cada nodo según
9.1 y construye las unidades de la ronda siguiente:

- Un test hoja con fallo elegible forma una unidad equivalente a
  `--exact <test-id>`: vuelve a ejecutar la hoja y todo el lifecycle de sus
  suites ancestras.
- Una suite cuyo propio setup o teardown conserva un fallo elegible forma una
  unidad de suite: vuelve a ejecutar sus suites ancestras y el subárbol de hojas
  que pertenecía a la selección original de ese shard.
- Una suite exterior elegible absorbe cualquier unidad elegible de suite o test
  contenida en su subárbol. Fuera de ese caso, dos hojas fallidas son unidades
  independientes aunque compartan ancestros; así cada confirmación conserva su
  propia frontera limpia.

Las unidades se ordenan por la primera hoja que contienen dentro del
`execution_plan` original y el ID resuelve cualquier empate. Conservan target,
artefacto compilado, inputs declarados, capabilities, shard, seed, orden,
timeouts y resource profile de la invocación original. Un retry nunca mueve una
hoja a otro shard. Cada ronda vuelve a calcular candidatos desde el estado
agregado y la participación más reciente disponibles al terminarla, aplicando
las exclusiones anteriores. Se detiene cuando no queda una unidad elegible o se
han consumido las `N` rondas.

Cada unidad de retry se ejecuta en un proceso worker nuevo. Solo puede
reutilizarse el artefacto compilado inmutable. El worker comienza con VM, heap,
GC roots, executor, tasks, handles, envelopes, tags, logs, stdout, stderr,
presupuestos y recursos temporales nuevos; no restaura snapshots de suite ni
reutiliza objetos de un intento anterior. Antes de completar el intento, el
runner revoca y espera los procesos y recursos de host que Tondo le haya
entregado de forma rastreable. Un worker que no puede cerrarse limpiamente
produce `infrastructure`, no otra oportunidad silenciosa.

El registro de dominios temporales también comienza vacío. Cada
`withVirtualTime` del nuevo intento vuelve al mismo cero virtual y no hereda
timers, secuencias de task, contadores ni tiempo avanzado. Por tanto un retry
puede confirmar el mismo comportamiento determinista, pero no hacer pasar una
prueba porque su reloj continuó desde el intento anterior.

El límite `--jobs N` es global a la invocación: cuenta conjuntamente las
entradas activas de la ronda inicial y de todos los workers de retry. Lanzar
workers nuevos no permite superar ese máximo. Esta edición no introduce
delay, backoff ni jitter entre rondas.

La frontera nueva evita fugas del runtime Tondo, pero no puede deshacer efectos
externos no controlados en bases de datos, filesystem, red o servicios. Una
fixture de integración que habilite retries sigue siendo responsable de usar
nombres aislados, operaciones idempotentes y cleanup verificable.

Todos los intentos permanecen en los reportes. Un éxito posterior se agrega
como `flaky-pass`, nunca como `passed`. Por defecto `flaky-pass` conserva exit
`1`; `--allow-flaky` permite exit `0` sin cambiar el estado ni borrar el
historial. No existen annotations de retry por test, labels estáticos de flaky
ni una base histórica oculta en este contrato.

### 8.8 Sin fail-fast global

`testing.failNow` termina únicamente el nodo activo; no es una política del
runner. Tras registrar el resultado y completar cleanup, continúan hermanos y
raíces posteriores cuando el aislamiento lo permite.

La edición 0.2 no define `--fail-fast`. Detener el runner dejaría hojas
seleccionadas sin ejecutar y exigiría otro estado y otra política de scheduling,
especialmente bajo `--jobs N`. Una edición posterior puede añadirlo únicamente
si reporta honestamente esos nodos y conserva una frontera determinista.

## 9. Resultados, diagnósticos y salida

### 9.1 Estados de test y suite

Cuando la compilación termina correctamente y el runner puede producir un
reporte fiable, cada oportunidad de ejecución de un test o suite produce
exactamente uno de estos estados de intento:

| Estado | Significado |
|---|---|
| `passed` | La entrada devolvió `Unit` normalmente. |
| `skipped` | La entrada comenzó y solicitó skip explícito; cleanup terminó correctamente. |
| `failed-error` | Un error recuperable alcanzó el runner. |
| `failed-panic` | Ocurrió un pánico Tondo después de unwind. |
| `resource-limit` | Se agotó un presupuesto configurado. |
| `timeout` | Venció el límite wall-clock del runner. |
| `infrastructure` | El harness, runtime o aislamiento dejó de ser fiable. |
| `blocked-setup` | No se invocó porque falló una suite ancestral seleccionada. |
| `blocked-skip` | No se invocó porque una suite ancestral solicitó skip. |

Cada intento se indexa desde `1` dentro de su nodo y registra la ronda que lo
produjo: `0` para la ejecución inicial y `1..N` para retries. En una ronda de
retry también registra la unidad que lo produjo. Un nodo puede participar
varias veces en una ronda porque unidades hoja independientes recorren los
mismos ancestros. `blocked-setup` y `blocked-skip` identifican
mediante `blocked_by` tanto el ID como el índice de intento de la suite que
conserva la causa o razón; no duplican su payload. Ninguno significa ignored ni
éxito; `blocked-skip` es neutral únicamente bajo la política default de skips.

Para un intento de suite ejecutado, `phase` vale `setup` o `teardown` en un
fallo, `setup` en un skip propio y `null` al pasar o quedar bloqueado. Su
`passed` solo describe el lifecycle propio y no agrega resultados de
descendientes.

Las combinaciones de intento válidas son cerradas:

- `passed`, `blocked-setup` y `blocked-skip` siempre usan `phase: null`.
- `skipped` usa `phase: setup` en una suite y no tiene phase en un test.
- Cualquiera de los cinco estados de fallo ejecutado puede usar
  `phase: setup`.
- `phase: teardown` admite `failed-panic`, `resource-limit`, `timeout` o
  `infrastructure`. No admite `failed-error`, porque `defer` es infallible desde
  el sistema de tipos, ni `skipped`, porque cleanup no puede omitir un resultado
  ya producido.

Un intento `skipped` conserva una razón explícita y una ubicación en `skip`; su
`failure` es `null`. `blocked-skip` conserva ambos campos en `null` y señala el
intento originario.

Después de cada ronda, el runner deriva un único estado agregado y un
`decisive_attempt` por nodo:

- Si el último intento es `passed` y todos los intentos son `passed`, el
  agregado es `passed` y el decisivo es el último.
- Si el último intento es `passed` y existe algún intento anterior distinto de
  `passed`, el agregado es `flaky-pass` y el decisivo es el último.
- Si el último intento no es `passed`, el decisivo es el intento ejecutado con
  fallo más reciente, si existe; en otro caso es el último intento. El agregado
  copia su estado.

Los cinco estados de fallo ejecutado son `failed-error`, `failed-panic`,
`resource-limit`, `timeout` e `infrastructure`. Elegir el fallo ejecutado más
reciente impide que un bloqueo o skip posterior oculte un fallo previo todavía
no resuelto. `flaky-pass` solo existe como agregado; nunca es el estado de un
intento. Un nodo reejecutado únicamente porque pertenecía a una unidad de suite
y cuyos intentos pasaron todos sigue agregado como `passed`.

No existe skip estático, `ignored`, `expected-failure` ni
`passed-after-retry`. La única representación de intermitencia confirmada es
`flaky-pass` con todos sus intentos preservados.

### 9.2 `assert` y fallo inmediato

`assert(false)` conserva el pánico `P0007`. Dentro de un test, el runner lo
clasifica como `failed-panic`; dentro de setup o teardown hace fallar esa fase de
suite. No introduce un segundo código ni una excepción recuperable.

`testing.failNow(message)` es una assertion fallida incondicional con resultado
estático `Never`. Produce el mismo `P0007`, ejecuta el mismo unwind y no detiene
hermanos ni cambia la política del runner. Existe para expresar intención y
control de flujo sin escribir `assert(false, message)`.

La representación fuente de la condición, el mensaje, la ubicación y el stack
trace se conservan según la especificación base. Una librería puede construir
mensajes mejores, pero no alterar el terminal.

### 9.3 Logs y captura de output

Los logs, stdout y stderr de cada entrada de suite o test se capturan por
separado. Cada llamada a `testing.log(message)` añade el `String` exacto como un
elemento nuevo; no añade prefijo, nivel, timestamp ni salto de línea implícito.
Los streams continúan siendo UTF-8. El modo humano:

- Muestra siempre la identidad y el estado.
- Muestra owners y los tags por intento no vacíos de nodos fallidos, skipped o
  `flaky-pass`; para un nodo bloqueado muestra la metadata del intento causal,
  no la duplica.
- Muestra siempre la razón y logs de nodos `skipped`.
- Muestra todos los intentos, con logs y output separados, de un nodo
  `flaky-pass` o cuyo agregado termina en fallo.
- Muestra por intento el número de dominios y su tiempo virtual final cuando
  `virtual_time` no está vacío; nunca lo rotula como duración real.
- Oculta tags, logs y output de entradas que pasan salvo `--show-output`.
- Nunca intercala bytes de dos entradas.

La lista humana muestra cada ID con sus owners estáticos no vacíos. No muestra
tags porque `--list` no ejecuta código.

El output de procesos hijos solo pertenece a la captura si el programa lo
redirige explícitamente a los streams Tondo. Heredar descriptores del runner no
constituye captura conforme.

El diagnóstico que el runner construye para error, pánico, timeout o
infraestructura pertenece a `failure`; no se añade artificialmente al
`stderr` capturado del programa.

En una suite, cada stream concatena los bytes producidos por su setup y por su
teardown posterior en ese orden, sin incluir output de descendientes. Si la
suite queda `blocked-setup` o `blocked-skip`, ambos streams y sus logs están
vacíos. Los logs de una suite ejecutada siguen el mismo orden setup-teardown y
no incluyen logs de descendientes.

Cada intento mantiene buffers, tags y payloads propios. La presentación nunca
concatena dos intentos como si fueran una sola ejecución; incluso con
`--show-output` conserva sus índices y rondas.

### 9.4 Tags de ejecución

Cada entrada comienza con un `Map[String, String]` vacío y privado.
`testing.tags(values)` fusiona sus pares en el mapa del envelope activo. La
llamada completa es atómica:

- Una key nueva conserva el `String` exacto recibido.
- Repetir la misma key con el mismo valor es idempotente.
- Si una o más keys ya tienen otro valor, no se fusiona ningún par y la llamada
  produce `P2002` para la key conflictiva menor por bytes UTF-8. El valor
  anterior se conserva; nunca existe last-write-wins.
- Si el crecimiento de keys/values UTF-8 no cabe en el presupuesto de output,
  no se fusiona ningún par y el nodo termina como `resource-limit`.
- Un par idempotente no vuelve a cargar bytes al presupuesto; un mapa vacío es
  un no-op.
- Llamadas desde tasks distintas comparten las mismas reglas. Keys distintas no
  entran en conflicto y el reporte las ordena por bytes UTF-8.
- La operación conserva su significado durante cleanup mientras el envelope
  siga activo.

La operación valida conflictos, calcula el delta de presupuesto y publica el
nuevo mapa en un único punto de linearización dentro del envelope. Como el
lenguaje no fija el orden de avance de tasks concurrentes, código que necesite
determinar qué writer de una misma key se observa debe sincronizarlos; sin
sincronización el resultado sigue siendo un fallo `P2002`, pero la llamada que
lo descubre puede variar.

Los tags de una suite pertenecen únicamente a esa suite. No se heredan, copian
ni mezclan en sus descendientes. Un nodo bloqueado nunca tuvo envelope y
conserva `tags: {}`. Un nodo fallido o skipped conserva los tags registrados
antes de completar su cleanup.

El contenido registrado no concede capabilities ni se interpreta para cambiar
identidad, estado, owners, selección, shard, orden o exit status. La operación
sí puede terminar el nodo con `P2002` o agotar su presupuesto. Los tags son
metadata no confiable escrita por el propio test; una integración no puede
interpretar una key como autoridad de seguridad.

### 9.5 Duración

Una implementación puede mostrar duración como metadato informativo. La duración
wall-clock no forma parte del resultado semántico, del orden ni del reporte
JSON canónico reproducible. El exportador JUnit sí incluye duración observada
por intento porque ese formato es un artefacto operacional no canónico.

El tiempo virtual no es duración operacional: `virtual_time.elapsed_ns` describe
el estado determinista de cada dominio y sí pertenece al intento canónico. Un
test puede avanzar horas virtuales y consumir milisegundos reales; ninguna de
las dos magnitudes sustituye a la otra.

### 9.6 Exit status

`tondo test` utiliza:

| Exit | Condición |
|---|---|
| `0` | Compilación correcta, ningún agregado falló, quedó `blocked-setup` ni es `flaky-pass`, y todo skip se permite por la política default; o selección vacía solicitada con `--allow-empty`. `--allow-flaky` elimina únicamente la condición `flaky-pass`. |
| `1` | Error al materializar inputs del plan, error de compilación, selección vacía no permitida, algún agregado falló/quedó `blocked-setup`, existe `flaky-pass` sin `--allow-flaky`, o `--deny-skips` encontró `skipped`/`blocked-skip`. |
| `2` | Uso inválido de CLI. |
| `3` | Fallo interno del toolchain, pérdida de fiabilidad o imposibilidad de serializar/publicar un output solicitado. |

Un test que llame a APIs de proceso no puede elegir el exit status del runner.
Un estado `infrastructure` que todavía permite un reporte íntegro usa exit `1`;
si el runner no puede garantizar ni serializar ese reporte, usa exit `3`.
`--deny-skips` solo modifica el exit status: no falsifica un skip como failure ni
altera los estados o contadores canónicos. De igual modo, `--allow-flaky` solo
modifica el exit status y la proyección JUnit de policy; nunca cambia
`flaky-pass`, `decisive_attempt` ni los intentos.

## 10. Contrato de `tondo test`

Interfaz mínima:

~~~text
tondo test [--manifest <path>]
           [--filter <text> | --glob <pattern> | --exact <node-id>]
           [--codeowners <auto|none|path>]
           [--shard <index/count>]
           [--order <canonical|random>]
           [--seed <hex-u64>]
           [--list]
           [--jobs <positive-int>]
           [--timeout <duration|none>]
           [--retry <non-negative-int>]
           [--diagnostic-format <human|json>]
           [--test-format <human|json>]
           [--report <json|junit>=<path>]...
           [--show-output]
           [--deny-skips]
           [--allow-flaky]
           [--allow-empty]
~~~

Reglas:

- Sin `--manifest`, el toolchain descubre el proyecto mediante su contrato
  ordinario y materializa un plan cerrado antes de compilar.
- `--filter`, `--glob` y `--exact` seleccionan ejecución, no compilación, y
  siguen 8.2.
- `--codeowners auto` es el default y sigue 5.7.
- `--shard` sigue 8.3 y `--order`/`--seed` siguen 8.4.
- `--list` no ejecuta bodies y no admite `--show-output`, `--deny-skips` ni un
  reporte `junit`; tampoco admite `--retry` ni `--allow-flaky`. Sí admite los
  tres selectores, sharding, orden y reporte `json`.
- `--jobs` vale `1` por defecto.
- `--timeout` aplica por test hoja y por fase activa de suite, no al tiempo total
  del subárbol.
- `--retry` aparece como máximo una vez, acepta únicamente un entero decimal
  canónico no negativo dentro del límite estructural publicado y sigue 8.7:
  `0` o un dígito `1..9` seguido de dígitos, sin signo, padding, separadores ni
  whitespace. El default efectivo es `0`; los retries reutilizan la compilación
  y nunca recompilan entre intentos.
- `--deny-skips` conserva resultados y reporte, pero usa exit `1` si cualquier
  suite/test queda `skipped` o `blocked-skip`.
- `--allow-flaky` conserva el estado e historial y solo permite exit `0` y una
  proyección JUnit no roja cuando los demás resultados lo permiten.
- `--test-format human` es el default interactivo.
- `--test-format json` emite exactamente un reporte
  `tondo-test-report-0.2/6`, o una lista `tondo-test-list-0.2/5` con `--list`.
- `--report` es repetible y escribe el resultado de la misma ejecución sin
  volver a compilar ni ejecutar. Se divide por el primer `=`; format y path
  vacíos son inválidos.
- `--report json=<path>` escribe exactamente los mismos bytes que el JSON
  correspondiente de `--test-format json`.
- `--report junit=<path>` escribe `tondo-junit-report-0.2/3` según 15.5.
- Dos reportes no pueden resolver al mismo output ni sobrescribir un input,
  source, manifest, lockfile o producto declarado. Cada archivo se publica
  atómicamente después de completar su serialización.
- Los diagnostics de compilación conservan `--diagnostic-format` y su schema
  propio; no se insertan como strings ambiguos dentro de resultados de test.
- Opciones desconocidas, repetidas cuando no son repetibles o combinaciones
  incompatibles terminan con exit `2` sin compilar.

Un argumento `--codeowners` sintácticamente inválido es uso de CLI y termina con
exit `2`; un archivo seleccionado que falta, no puede leerse o no cumple 5.7 es
un input de proyecto inválido y termina con exit `1` antes de compilar.
Un glob mal formado, un entero de retry inválido o una combinación prohibida
con `--list` termina con exit `2` antes de compilar.

Si un output solicitado no puede serializarse o publicarse después de ejecutar,
el comando termina con exit `3` y no presenta el conjunto de reportes como
completo. Un JSON ya publicado debe continuar siendo byte a byte válido; el
toolchain no deja archivos parciales ni finge atomicidad entre paths distintos.

El schema concreto del manifiesto, los defaults de límites y la representación
de inputs de environment pertenecen a la especificación del toolchain, pero no
pueden contradecir las observaciones fijadas aquí.

## 11. Frontera con `assert` y `std.testing`

### 11.1 Núcleo suficiente

Un programa puede escribir tests útiles usando solo lenguaje y prelude:

~~~tondo
test addsValues {
    let value = 20 + 22
    assert(value == 42, "expected 42")
}
~~~

`assert` sigue disponible fuera de tests y nunca se elimina de builds
optimizados.

### 11.2 Núcleo sellado y responsabilidad de `std.testing`

El control, la metadata y el dominio temporal mínimos del runner forman parte de
esta especificación. Sus firmas exactas son:

~~~tondo
import std.time

pub fn log(message: String)
pub fn tags(values: Map[String, String])
pub fn failNow(message: String): Never
pub fn skip(reason: String): Never

pub async fn withVirtualTime[
    E,
    F: Send + CallOnce[async fn(ref VirtualTime): Unit ! E],
](body: F): ! E

pub async fn VirtualTime.settle(ref self)
pub async fn VirtualTime.advance(ref self, duration: time.Duration)
~~~

Se utilizan mediante resolución de módulo ordinaria:

~~~tondo
import std.testing

fn connectService(): Client ! ConnectError {
    testing.log("connecting to integration service")
    Client.connect()
}

test importsUsers {
    testing.tags([
        "component": "users",
        "kind": "integration",
    ])

    if not serviceAvailable() {
        testing.skip("requires the integration service")
    }

    let client = connectService()?
    let imported = client.importUsers()?

    if imported != 42 {
        testing.failNow("expected 42 users, found {imported}")
    }
}
~~~

Las cuatro operaciones de control y metadata son monomórficas. `log`, `failNow`
y `skip` reciben un solo `String`; interpolación y formatting ocurren antes de
la llamada. `tags` recibe exactamente un `Map[String, String]`. No existen
niveles de log, attachments, timestamps, un tipo dinámico de metadata ni
sobrecargas variádicas en 0.2.

`withVirtualTime` es genérica únicamente sobre la unión de error y el tipo
concreto del cierre. Exige `CallOnce` porque ejecuta el body una vez y `Send`
porque su frame async puede migrar. Un body infallible infiere `E = Never`; uno
fallible conserva exactamente `E`. La llamada usa el `?` ordinario para
propagar ese canal; con `E = Never` no existe rama `err` ejecutable. No existe
type erasure, allocation de callback ni wrapper `Task`.

`VirtualTime` es un tipo opaco test-only con `Send`, sin `Copy`, `Share`,
`Equatable`, `Key` ni `Display`. El usuario nunca lo construye ni posee:
`withVirtualTime` presta un único controlador al cierre y lo revoca al
terminarlo. `settle` y `advance` son async porque conducir otras tasks es un
punto de suspensión visible; no esconden bloqueo detrás de una llamada
síncrona.

Estas operaciones son intrínsecas y selladas dentro de un artefacto de test:

- `log` devuelve `Unit` y añade un elemento al buffer del nodo activo.
- `tags` devuelve `Unit` y fusiona metadata conforme a 9.4.
- `failNow` produce `P0007` y tiene resultado `Never`.
- `skip` exige una razón, produce el terminal cooperativo de skip y tiene
  resultado `Never`.
- `withVirtualTime` crea y desmonta el dominio de 7.9 y propaga retorno, error,
  pánico, skip y cancelación de la closure sin reinterpretarlos.
- `VirtualTime.settle` observa quiescencia sin mover el reloj y
  `VirtualTime.advance` conduce el dominio hasta un target exacto.
- Llamadas desde helpers, closures o tasks estructuradas conservan el envelope
  de la entrada que las alcanzó.
- Ninguna expone ID, nombre, path, estado, runner ni referencia a otro nodo.
- No existe `TestContext`, `currentTest()`, registro runtime ni callback de
  lifecycle.

El resto de `std.testing` es librería ordinaria y su especificación estándar
debe considerar como mínimo:

- Igualdad con impresión de actual y esperado.
- Comparación de texto con diff.
- Comparación aproximada de floats con tolerancia explícita.
- Comprobaciones sobre `Option` y `Result` sin ocultar su consumo.
- Workspace o directorio temporal con cleanup terminal explícito.
- Captura o inspección portable de output cuando el target lo permita.
- Utilidades de snapshot y datos generados, si pueden fijar formato, seed,
  actualización y seguridad de forma reproducible.

Sus funciones pueden terminar mediante `assert`/pánico o devolver un error
documentado. No reciben acceso reflectivo a valores privados ni pueden registrar
suites o tests en runtime. La especificación estándar puede añadir helpers, pero
no cambiar estas firmas, sus terminales, merge de tags, dominio temporal ni el
envelope fijados aquí.

### 11.3 Setup y teardown

Setup reutilizable por cada test es una función normal y se invoca
explícitamente:

~~~tondo
fn configuredStore(): Store ! StoreError {
    Store.open(testConfiguration())?
}

test readsExistingValue {
    let store = configuredStore()?
    defer Store.close(store)

    assert(store.read("name")? == "Ada")
}
~~~

Teardown por test utiliza `defer`. No existen `beforeEach`, `afterEach` ni orden
implícito de fixtures.

Cuando una inicialización costosa debe ejecutarse una vez para varias hojas se
utiliza una suite:

~~~tondo
suite persistentStore {
    let service = StoreService.start()?
    let endpoint: String = service.endpoint()
    defer StoreService.stop(service)

    test readsWrittenValue {
        let store = Store.connect(endpoint)?
        defer Store.close(store)

        store.write("persistentStore.readsWrittenValue", "Ada")?
        assert(store.read("persistentStore.readsWrittenValue")? == "Ada")
    }

    test writesNewValue {
        let store = Store.connect(endpoint)?
        defer Store.close(store)

        store.write("persistentStore.writesNewValue", "Grace")?
        assert(store.read("persistentStore.writesNewValue")? == "Grace")
    }
}
~~~

El setup de `persistentStore` y su `defer` forman el equivalente explícito de
`beforeAll`/`afterAll`, pero su orden es léxico, existe un solo scope propietario
y no hay callbacks heredados. Cada test continúa construyendo y cerrando su
estado particular. Una suite no ejecutada por el selector tampoco ejecuta setup.

### 11.4 Tests parametrizados

Los tests tabulares utilizan colecciones, tuplas y el único `for`:

~~~tondo
test dividesIntegers {
    let cases = [
        (6, 2, 3),
        (20, 5, 4),
        (-10, 2, -5),
    ]

    for (left, right, expected) in cases {
        assert(left / right == expected)
    }
}
~~~

No existe otra sintaxis de parameterized tests. Una librería puede mejorar el
mensaje de cada fila, pero el registro contiene una sola entrada
`dividesIntegers`. Un `for` no registra subtests dinámicos y sus valores no
pueden cambiar la identidad estática del árbol.

### 11.5 Fakes y mocks

Un fake se construye con records, enums, funciones y traits estáticos. El
lenguaje no genera mocks por reflection, no intercepta métodos y no altera
visibilidad.

Un generador externo puede producir fuente de test ordinaria si declara inputs y
outputs en el build. Esa fuente se comprueba igual y no recibe privilegios
adicionales.

## 12. Patrones de uso

### 12.1 Error esperado

~~~tondo
test rejectsEmptyName {
    match parseName("") {
        ok(_) => panic("expected EmptyName, found success")
        err(NameError.EmptyName) => ()
        err(_) => panic("expected EmptyName, found another error")
    }
}
~~~

El error esperado se consume dentro del test. Permitir que escape significaría
fallo.

### 12.2 Privacidad unitaria

Archivo `src/tokenizer.to`:

~~~tondo
fn scanDigits(source: String): Int {
    source.length()
}
~~~

Archivo `src/tokenizer_test.to`:

~~~tondo
test scansAllDigits {
    assert(scanDigits("1234") == 4)
}
~~~

El companion comparte el módulo y puede llamar a `scanDigits`.

### 12.3 Frontera de integración

Archivo `tests/public_api.to`:

~~~tondo
import application.api

test createsUserThroughPublicApi {
    let user = api.createUser("Ada")?
    assert(user.name == "Ada")
}
~~~

El test solo ve símbolos `pub` de `application.api`.

### 12.4 Cleanup tras pánico

~~~tondo
test releasesResourceOnFailure {
    let resource = Resource.acquire()?
    defer Resource.release(resource)

    assert(false, "intentional failure")
}
~~~

El runner registra `failed-panic` solo después de ejecutar el `defer`.

### 12.5 Servicio de integración compartido

Archivo `tests/users.to`:

~~~tondo
import application.client

suite userApi {
    let service = TestApplication.start()?
    let endpoint: String = service.endpoint()
    defer TestApplication.stop(service)

    test createsUser {
        let api = client.connect(endpoint)?
        defer client.close(api)

        let user = api.createUser("create-user@example.test")?
        assert(user.email == "create-user@example.test")
    }

    suite validation {
        test rejectsEmptyEmail {
            let api = client.connect(endpoint)?
            defer client.close(api)

            match api.createUser("") {
                err(ApiError.EmptyEmail) => ()
                ok(_) => panic("expected EmptyEmail")
                err(_) => panic("expected EmptyEmail, found another error")
            }
        }
    }
}
~~~

Seleccionar `...::userApi::validation::rejectsEmptyEmail` ejecuta únicamente los
setups de `userApi` y `validation`, esa hoja y los teardowns correspondientes.
No ejecuta `createsUser`.

### 12.6 Skip de una suite con razón observable

~~~tondo
import std.testing

suite externalSearch {
    if not searchServiceAvailable() {
        testing.log("search service probe did not succeed")
        testing.skip("requires the external search service")
    }

    let endpoint: String = searchServiceEndpoint()

    test findsExactTitle {
        assert(search(endpoint, "Tondo")?.length() == 1)
    }
}
~~~

Si el servicio no está disponible, `externalSearch` conserva ambos mensajes,
queda `skipped` y `findsExactTitle` queda `blocked-skip`. El modo normal no lo
convierte en fallo; `tondo test --deny-skips` termina con exit `1` conservando
los mismos estados.

### 12.7 Metadata operacional

~~~tondo
import std.testing

fn identifyPaymentsScenario() {
    testing.tags([
        "component": "payments",
        "kind": "integration",
        "priority": "critical",
    ])
}

test capturesPayment {
    identifyPaymentsScenario()
    testing.log("capturing authorized payment")

    assert(capturePayment()?.status == PaymentStatus.Captured)
}
~~~

Los tres tags pertenecen únicamente a `capturesPayment`. El helper no conoce su
ID ni recibe contexto; el envelope atribuye la llamada. CODEOWNERS se resuelve
por separado desde el path del archivo y no puede ser reemplazado por un tag
`owner`.

### 12.8 Backoff sin espera real

~~~tondo
import std.testing

test retriesAfterBackoff {
    let delay = retryDelay()
    let probe = RetryProbe.new()

    await testing.withVirtualTime(async (clock) {
        scope {
            let result = spawn fetchWithRetry(probe, delay)

            await clock.settle()
            assert(probe.attemptCount() == 1)

            await clock.advance(delay)
            assert(await result == expectedResponse())
            assert(probe.attemptCount() == 2)
        }
    })?
}
~~~

`settle` demuestra que el primer intento terminó y el segundo permanece detrás
del timer sin elegir una pausa real aproximada. `advance` cruza exactamente el
deadline y el `await` final usa la misma implementación de backoff que
producción. `RetryProbe` es un double ordinario y seguro para concurrencia; no
forma parte del runner.

## 13. Características deliberadamente ausentes

El contrato inicial no incluye:

- Clases base de test.
- Decorators, annotations o atributos.
- Macros de assertions.
- Descubrimiento por reflection o prefijo de función.
- Registro de tests en runtime.
- Tests o suites con nombres calculados o creados dentro de control de flujo.
- Subtests dinámicos registrados desde un `for` o callback.
- Hooks `beforeAll`, `afterAll`, `beforeEach` o `afterEach`.
- Orden declarado en fuente o dependencias entre tests.
- Estado mutable compartido de suite.
- Tags declarativos o filtrado por tags runtime.
- Selectores regex, character classes o dialectos glob dependientes del host.
- Ignorados, disabled tests o expected failures estáticos dentro de fuente.
- Skip sin razón explícita.
- Retries implícitos, históricos, dirigidos por tags o configurados mediante
  annotations por test.
- Labels estáticos de flaky y delay, backoff o jitter entre retries.
- Tiempo virtual implícito para todos los tests, un flag global que cambie su
  semántica o una keyword/modificador temporal de `test`.
- Virtualización automática de calendario civil, filesystem, red, procesos o
  callbacks externos.
- Fail-fast global que abandone hojas seleccionadas.
- Un modo que elimine `assert`.
- Una keyword separada para benchmarks.
- Una keyword separada para property tests.
- Captura recuperable de pánicos dentro del mismo runtime.

Estas ausencias no impiden que `std.testing` o tooling posterior añadan
utilidades explícitas. Cualquier operación que espere un pánico debe conservar
el principio de que los pánicos no son excepciones recuperables; por ejemplo,
puede ejecutar una closure en una frontera de runtime aislada y reportar el
resultado sin exponer un `catch`.

Benchmarks, coverage, fuzzing, mutation testing y property testing son modos de
tooling construidos sobre fuentes y artefactos explícitos. No cambian la
semántica de `test`.

## 14. Diagnósticos nuevos

La edición 0.2 añade estos códigos al registro normativo:

| Código | Nombre estable | Condición primaria |
|---|---|---|
| `E2001` | `test-node-outside-test-source` | Una declaración `suite` o `test` aparece en una fuente `production`, script o forma no clasificada como test. |
| `E2002` | `duplicate-test-node` | Dos miembros suite/test producen la misma identidad o nombre de hermano dentro del mismo árbol. |
| `E2003` | `invalid-test-source-declaration` | Una fuente de test intenta exportar API, alterar la unidad de producción sellada o ser consumida desde producción; o producción intenta importar `std.testing`. |
| `E2004` | `empty-test-suite` | Una suite no contiene ningún miembro directo y, por tanto, ningún test descendiente. |
| `E2005` | `invalid-suite-capture` | Un descendiente intenta capturar `var`, préstamo, valor afín/terminal o un tipo que no cumple `Copy + Send + Share`. |

El runtime de test añade cinco pánicos:

| Código | Nombre estable | Condición primaria |
|---|---|---|
| `P2001` | `test-skip-during-cleanup` | `testing.skip` intenta omitir un nodo mientras ejecuta `defer`, unwind o teardown. |
| `P2002` | `test-tag-conflict` | `testing.tags` intenta asociar una key ya registrada a un valor distinto dentro del mismo envelope. |
| `P2003` | `test-virtual-time-deadlock` | La raíz del dominio espera, todas sus tasks están terminadas o bloqueadas de forma durable y no existe timer ni evento interno capaz de progresar. |
| `P2004` | `overlapping-test-virtual-time` | `withVirtualTime` intenta crear un segundo dominio mientras el mismo envelope ya mantiene otro activo, por nesting o concurrencia hermana. |
| `P2005` | `test-virtual-time-range` | Un avance recibe una duración negativa o un avance explícito/automático excedería el rango representable del reloj virtual. |

`testing.failNow` reutiliza `P0007`; no añade otro código.

El resto reutiliza diagnósticos existentes:

- Sintaxis de suite/test inválida: `E0004`.
- `test` o `suite` usado como identificador reservado: `E1005`.
- Colisión de helpers ordinarios: `E1002`.
- Identidad `_` u otra declaración semánticamente mal formada: `E1115`.
- Tipo final distinto de `Unit`: `E1102`.
- Error inferido sin `Discard`: `E1105`.
- Transferencia de control inválida, incluido `return` desde setup: `E1205`.
- Error o `?` incompatible: `E1301`/`E1302`.
- Ownership, préstamos y cleanup: familia `E14xx`.
- Async y concurrencia: familia `E16xx`.
- Unsafe: familia `E17xx`.

Selector vacío, glob inválido, CODEOWNERS inválido, opciones de
retry/shard/order/report, timeout e infraestructura son diagnósticos del
toolchain, no nuevos errores de compilación `E`.

## 15. Formato machine-readable

### 15.1 Forma canónica

`--test-format json` y `--report json=<path>` emiten un único objeto JSON UTF-8,
sin BOM ni bytes posteriores salvo `LF`. Los valores concretos siguientes son
un ejemplo; la forma y los tipos de los campos son normativos:

~~~json
{
  "format": "tondo-test-report-0.2/6",
  "edition": "0.2",
  "target": {
    "name": "tondo-vm-hosted",
    "profile": "hosted",
    "capabilities": ["console", "process"]
  },
  "compiled": true,
  "selection": {
    "kind": "all",
    "value": null
  },
  "ownership": {
    "mode": "auto",
    "source": ".github/CODEOWNERS",
    "sha256": "1111111111111111111111111111111111111111111111111111111111111111"
  },
  "shard": null,
  "order": {
    "mode": "canonical",
    "seed": null,
    "algorithm": "id-byte-order-v1"
  },
  "execution_plan": [
    "application::unit::math::arithmetic::addReturnsSum"
  ],
  "retry": {
    "max_additional_rounds": 0,
    "isolation": "fresh-worker-v1",
    "rounds": []
  },
  "policy": {
    "deny_skips": false,
    "allow_flaky": false
  },
  "limits": {
    "jobs": 1,
    "timeout_ms": 30000,
    "resource_profile_sha256": "2abaca4911e68fa9bfbf88195b6c8bca41fd731d5a45f9b51a2bca9234e0c83f"
  },
  "suites": [
    {
      "id": "application::unit::math::arithmetic",
      "parent": null,
      "package": "application",
      "kind": "unit",
      "module": "math",
      "path": ["arithmetic"],
      "name": "arithmetic",
      "source": {
        "file": "src/math_test.to",
        "start": 0,
        "end": 126
      },
      "owners": ["@tondo/math"],
      "status": "passed",
      "decisive_attempt": 1,
      "attempts": [
        {
          "index": 1,
          "round": 0,
          "unit": null,
          "status": "passed",
          "phase": null,
          "blocked_by": null,
          "failure": null,
          "skip": null,
          "tags": {},
          "virtual_time": [],
          "logs": [],
          "stdout": "",
          "stderr": ""
        }
      ]
    }
  ],
  "tests": [
    {
      "id": "application::unit::math::arithmetic::addReturnsSum",
      "parent": "application::unit::math::arithmetic",
      "package": "application",
      "kind": "unit",
      "module": "math",
      "path": ["arithmetic", "addReturnsSum"],
      "name": "addReturnsSum",
      "source": {
        "file": "src/math_test.to",
        "start": 32,
        "end": 124
      },
      "owners": ["@tondo/math"],
      "status": "passed",
      "decisive_attempt": 1,
      "attempts": [
        {
          "index": 1,
          "round": 0,
          "unit": null,
          "status": "passed",
          "blocked_by": null,
          "failure": null,
          "skip": null,
          "tags": {
            "component": "math",
            "kind": "unit"
          },
          "virtual_time": [],
          "logs": [],
          "stdout": "",
          "stderr": ""
        }
      ]
    }
  ],
  "summary": {
    "selected": 1,
    "executed": 1,
    "passed": 1,
    "flaky_passed": 0,
    "skipped": 0,
    "blocked_setup": 0,
    "blocked_skip": 0,
    "failed_error": 0,
    "failed_panic": 0,
    "resource_limit": 0,
    "timeout": 0,
    "infrastructure": 0,
    "retried": 0,
    "test_attempts": 1,
    "suite_selected": 1,
    "suite_passed": 1,
    "suite_flaky_passed": 0,
    "suite_skipped": 0,
    "suite_blocked_setup": 0,
    "suite_blocked_skip": 0,
    "suite_failed": 0,
    "suite_retried": 0,
    "suite_attempts": 1,
    "failed": 0
  }
}
~~~

`selection.kind` es `all`, `filter`, `glob` o `exact`; `value` es `null` para
`all` y conserva el argumento exacto en los otros casos.
`policy.deny_skips` y `policy.allow_flaky` contienen los valores efectivos de
sus flags. `timeout_ms` es `null` solo para `--timeout none`. El resource profile
contiene todos los presupuestos finitos del frontend, verifiers y runtime y se
distribuye de forma recuperable por el toolchain; el hash del reporte identifica
exactamente sus bytes canónicos.

`ownership.mode` es `auto`, `explicit` o `none`. `source` y `sha256` son strings
cuando se utilizó un CODEOWNERS y `null` en otro caso. `source` siempre es
lógico y relativo a la raíz del repositorio; `sha256` contiene exactamente
sesenta y cuatro dígitos hexadecimales lowercase sobre los bytes originales.

`shard` es `null` sin partición. Con `--shard 2/8` contiene exactamente:

~~~json
{
  "index": 2,
  "count": 8,
  "algorithm": "sha256-mod-v1"
}
~~~

`order` refleja la configuración efectiva. En modo random, `seed` contiene
dieciséis dígitos hexadecimales lowercase y `algorithm` es
`sha256-tree-v1`. `execution_plan` contiene una vez cada ID de test posterior al
shard, en prioridad de dispatch; no contiene suites.

`retry.max_additional_rounds` contiene el valor efectivo de `--retry` e
`isolation` vale `fresh-worker-v1`. `rounds` describe únicamente las rondas de
retry que llegaron a ejecutarse; la ronda inicial ya está representada por
`execution_plan`. Una ronda no vacía tiene esta forma:

~~~json
{
  "round": 1,
  "units": [
    {
      "kind": "test",
      "id": "application::unit::math::arithmetic::addReturnsSum",
      "execution_plan": [
        "application::unit::math::arithmetic::addReturnsSum"
      ]
    }
  ]
}
~~~

`round` comienza en `1` y es contiguo. `kind` es `test` o `suite`; `id`
identifica la raíz causal definida por 8.7. El plan de una unidad contiene
únicamente hojas de la selección y shard originales, en su orden relativo
dentro del plan original. Las unidades siguen el orden normativo de 8.7 y no se
solapan por descendencia después de absorber causas bajo una suite exterior.
`rounds` queda vacío si no se autorizó retry o no apareció ningún candidato
elegible.

`suites` contiene el bosque mínimo necesario para los tests del shard,
incluidos nodos `blocked-setup` y `blocked-skip`. `tests` contiene todas sus
hojas, se hayan ejecutado o bloqueado. Cada descriptor aparece una sola vez,
aunque tenga varios intentos. Todos los campos normativos aparecen incluso
cuando están vacíos o son `null`; `kind` es `unit` o `integration`.

`source` identifica la declaración mediante path lógico y rango bytewise.
`owners` conserva el orden de la línea CODEOWNERS ganadora. Un descriptor nunca
mezcla owners estáticos con los `tags` runtime de sus intentos.

`parent` contiene el ID de la suite inmediata o `null` para un nodo top-level.
`path` contiene únicamente los identificadores de suite y test posteriores al
module path. En una suite termina en su propio nombre; en un test termina en el
nombre del test.

`status` es el agregado y `decisive_attempt` es un índice válido dentro de
`attempts`, ambos derivados exactamente como en 9.1. `attempts` nunca está vacío
y sus objetos se ordenan por `index`. Los índices empiezan en `1`, son contiguos
y no se reinician entre rondas. `round` vale `0` o el número de una ronda
presente en `retry.rounds`. `unit` es `null` en ronda `0`; en otra ronda es el
índice one-based de la unidad dentro de `retry.rounds[].units`. Un nodo produce
como máximo un intento por pareja `(round, unit)`, aunque varias unidades
pueden producir intentos del mismo nodo en una misma ronda.

Cada intento de suite incluye `phase`; cada intento de test no lo incluye. La
pareja `status`/`phase` de suite pertenece al conjunto cerrado de 9.1.
`blocked_by` es `null` salvo en un intento `blocked-setup` o `blocked-skip`, en
cuyo caso contiene exactamente:

~~~json
{
  "id": "application::unit::math::arithmetic",
  "attempt": 2
}
~~~

El ID señala una suite del mismo reporte y `attempt` un índice existente de esa
suite que contiene la causa. `tags`, `virtual_time`, `logs`, `stdout`, `stderr`,
`failure` y `skip` pertenecen exclusivamente a su intento; nunca se fusionan
entre intentos. No se admiten campos de extensión sin cambiar el identificador
de formato.

`virtual_time` contiene los dominios creados durante ese intento y está vacío si
no alcanzó `withVirtualTime`. Cada dominio aparece incluso si su closure terminó
por error, pánico, skip, timeout o límite, con esta forma exacta:

~~~json
{
  "index": 1,
  "elapsed_ns": "5000000000",
  "automatic_advances": 0,
  "explicit_advances": 1,
  "settles": 0
}
~~~

`index` empieza en `1` y es contiguo por orden de creación. `elapsed_ns` es el
instante final relativo a cero expresado como entero decimal no negativo sin
padding dentro de un string; así no pierde precisión en consumidores JSON.
`automatic_advances` cuenta saltos al próximo deadline realizados por
quiescencia, `explicit_advances` llamadas a `advance` completadas y `settles`
llamadas a `settle` completadas. Visitar varios deadlines dentro de una llamada
explícita no aumenta `automatic_advances`. Un dominio interior rechazado por
`P2004` nunca fue creado y no añade descriptor.

### 15.2 Orden y estabilidad

- `capabilities` se ordena por bytes.
- `suites` se ordena por `id`.
- `tests` se ordena por `id`.
- `execution_plan` sigue 8.4 y no altera el orden de los arrays anteriores.
- `retry.rounds` se ordena por `round`; sus `units` siguen 8.7 y cada
  `execution_plan` de unidad conserva el orden relativo del plan inicial.
- `attempts` se ordena por `index`; no se reordena por status ni por tiempo de
  finalización.
- `virtual_time` se ordena por `index`; sus contadores y `elapsed_ns` son
  deterministas para la misma ejecución interna.
- `owners` conserva el orden textual de su única línea ganadora.
- Las keys de cada mapa `tags` se ordenan por bytes UTF-8.
- Keys conocidas se serializan en el orden mostrado por el schema del
  toolchain.
- Cada `logs` conserva el orden observado dentro de su único envelope.
- Cada `stdout` y `stderr` contiene texto UTF-8 exacto.
- `summary.selected = execution_plan.length = tests.length`.
- `summary.selected` es la suma exacta de `passed`, `flaky_passed`, `skipped`,
  `blocked_setup`, `blocked_skip` y los cinco contadores de fallo agregado de
  test.
- `summary.executed` cuenta identidades de test con al menos un intento distinto
  de `blocked-setup` y `blocked-skip`; puede solaparse con un bloqueo agregado
  posterior y no es una segunda partición de `selected`.
- `summary.retried` cuenta tests con más de un intento y
  `summary.test_attempts` es la suma de las longitudes de todos sus arrays
  `attempts`, incluidos intentos bloqueados.
- `summary.suite_selected` es la suma exacta de `suite_passed`,
  `suite_flaky_passed`, `suite_skipped`, `suite_blocked_setup`,
  `suite_blocked_skip` y `suite_failed`.
- `summary.suite_retried` cuenta suites con más de un intento y
  `summary.suite_attempts` suma las longitudes de sus arrays `attempts`.
- `summary.suite_failed` cuenta suites en cualquiera de los cinco estados de
  fallo ejecutado; las suites bloqueadas no vuelven a contar la causa.
- `summary.failed` es la suma de los cinco contadores de fallo de test y
  `suite_failed`. `flaky_passed`, skips y hojas bloqueadas no incrementan ese
  contador; las policies pueden producir exit `1` con `failed: 0`.
- `passed` y `suite_passed` cuentan solo agregados limpios; nunca incluyen
  `flaky-pass`.
- `retry.rounds.length <= retry.max_additional_rounds`; todo intento con
  `round > 0` referencia una unidad reportada mediante `unit`; toda
  participación descrita produce los intentos correspondientes y no existe
  ejecución oculta.
- Duración wall-clock, timestamps reales, PID, número de CPU, paths físicos y
  direcciones no aparecen en la forma JSON canónica. `virtual_time.elapsed_ns`
  sí aparece porque es una observación semántica determinista del dominio.
- Paths de source dentro de descriptors y failures son lógicos.

### 15.3 Failure y skip

Cuando `attempt.status` es `passed`, `skipped`, `blocked-setup` o
`blocked-skip`, su `failure` es `null`. En un estado de fallo ejecutado contiene
exactamente:

~~~json
{
  "kind": "panic",
  "code": "P0007",
  "error_type": null,
  "message": "assertion failed: value == 42",
  "source": {
    "file": "src/math_test.to",
    "start": 42,
    "end": 61
  },
  "stack": [
    {
      "function": "application::unit::math::arithmetic::addReturnsSum",
      "file": "src/math_test.to",
      "start": 20,
      "end": 63
    }
  ]
}
~~~

- `kind` es `error`, `panic`, `resource-limit`, `timeout` o
  `infrastructure`.
- `code` es el `Pdddd` o código versionado de tooling disponible; en otro caso
  es `null`.
- `error_type` contiene la identidad nominal visible de un error recuperable y
  es `null` para los demás kinds.
- `message` es presentación humana y nunca actúa como identidad.
- `source` es una ubicación lógica o `null`.
- `stack` es un array posiblemente vacío de frames Tondo, sin paths físicos.

Un intento `blocked-setup` explica su causa únicamente mediante `blocked_by`;
el intento de suite señalado contiene el único objeto `failure` normativo. Un
fallo de teardown de suite no reemplaza failures de tests ya ejecutados ni
payloads de intentos anteriores.

`skip` solo es no nulo en el intento de test o suite que ejecutó
`testing.skip`. Contiene exactamente:

~~~json
{
  "reason": "requires the external search service",
  "source": {
    "file": "tests/search.to",
    "start": 118,
    "end": 171
  }
}
~~~

`reason` conserva el `String` exacto y `source` es la ubicación lógica de la
llamada. Un intento `blocked-skip` tiene `skip: null` y explica su razón
únicamente mediante `blocked_by`; el intento señalado contiene el único objeto
`skip` normativo. Dentro de un intento, `failure` y `skip` nunca son no nulos a
la vez.

Campos privados y payloads opacos no se serializan por reflection.

### 15.4 Lista machine-readable

`--list --test-format json` emite `tondo-test-list-0.2/5`. Comparte `edition`,
`target`, `compiled`, `selection`, `ownership`, `shard`, `order` y
`execution_plan`, pero contiene descriptores sin estado, phase, failure, skip,
tags, bloqueo, logs ni output:

~~~json
{
  "format": "tondo-test-list-0.2/5",
  "edition": "0.2",
  "target": {
    "name": "tondo-vm-hosted",
    "profile": "hosted",
    "capabilities": ["console", "process"]
  },
  "compiled": true,
  "selection": {
    "kind": "all",
    "value": null
  },
  "ownership": {
    "mode": "auto",
    "source": ".github/CODEOWNERS",
    "sha256": "1111111111111111111111111111111111111111111111111111111111111111"
  },
  "shard": null,
  "order": {
    "mode": "canonical",
    "seed": null,
    "algorithm": "id-byte-order-v1"
  },
  "execution_plan": [
    "application::unit::math::arithmetic::addReturnsSum"
  ],
  "suites": [
    {
      "id": "application::unit::math::arithmetic",
      "parent": null,
      "package": "application",
      "kind": "unit",
      "module": "math",
      "path": ["arithmetic"],
      "name": "arithmetic",
      "source": {
        "file": "src/math_test.to",
        "start": 0,
        "end": 126
      },
      "owners": ["@tondo/math"]
    }
  ],
  "tests": [
    {
      "id": "application::unit::math::arithmetic::addReturnsSum",
      "parent": "application::unit::math::arithmetic",
      "package": "application",
      "kind": "unit",
      "module": "math",
      "path": ["arithmetic", "addReturnsSum"],
      "name": "addReturnsSum",
      "source": {
        "file": "src/math_test.to",
        "start": 32,
        "end": 124
      },
      "owners": ["@tondo/math"]
    }
  ]
}
~~~

Ambos arrays se ordenan por `id`. `suites` contiene las ancestras necesarias y
las suites descendientes del shard; `tests` contiene sus hojas.
`execution_plan` usa los mismos IDs exactamente una vez. `--allow-empty` permite
una selección previa vacía; un shard vacío válido produce ambos arrays y el plan
vacíos sin necesitar esa opción. `selection.kind` admite `glob` con el mismo
contrato de 15.1; la lista no incluye retry ni policy de flaky porque `--retry`
y `--allow-flaky` son inválidos con `--list`.

### 15.5 Perfil JUnit XML

`--report junit=<path>` genera `tondo-junit-report-0.2/3` como XML 1.0 UTF-8. Es
un artefacto operacional para CI, no la fuente normativa ni reproducible del
resultado: incluye duración wall-clock. El reporte JSON `/6` continúa siendo la
forma canónica y sin pérdida.

El archivo comienza con `<?xml version="1.0" encoding="UTF-8"?>`, no lleva BOM,
usa `LF`, termina en un único `LF` y no contiene DTD, entities externas ni
processing instructions adicionales.

La raíz es `testsuites`. El exportador emite un `testsuite` plano por suite
Tondo y uno por módulo que contenga tests top-level; la jerarquía se conserva
mediante IDs y una property `tondo.parent`, no mediante nesting XML dependiente
del consumidor. Cada hoja produce exactamente un `testcase` en su contenedor
inmediato. El nombre de un `testsuite` es el ID completo de la suite Tondo o el
prefijo visible `package::kind::module` del contenedor top-level. Un testcase de
hoja usa su nombre local como `name`, el ID del contenedor como `classname` y su
ID completo en `tondo.id`.

La proyección de resultados es:

| Estado Tondo | JUnit |
|---|---|
| `passed` | `testcase` sin outcome hijo |
| `flaky-pass` | hijo `failure` con type `tondo.flaky-pass`; sin outcome hijo bajo `--allow-flaky` |
| `skipped`, `blocked-skip` | hijo `skipped` |
| `failed-error`, `failed-panic` | hijo `failure` |
| `resource-limit`, `timeout`, `infrastructure` | hijo `error` |
| `blocked-setup` | hijo `skipped` con estado Tondo explícito |

Un fallo agregado de setup o teardown de suite produce además un único testcase
sintético basado en su intento decisivo, con name `@setup` o `@teardown`,
classname igual al ID de suite y outcome `failure` o `error` según la tabla. Así
el reporte JUnit queda rojo sin duplicar la causa en cada descendiente
bloqueado. Usa
`tondo.synthetic: "suite-lifecycle"` para distinguirlo de una hoja y el ID
`<suite-id>::@setup` o `<suite-id>::@teardown`.

Una suite agregada como `flaky-pass` produce siempre un testcase sintético
`@flaky`, ID `<suite-id>::@flaky` y
`tondo.synthetic: "flaky-policy"`. Por defecto contiene un hijo `failure` de
type `tondo.flaky-pass`; con `--allow-flaky` se emite sin outcome hijo. Así la
policy no cambia su identidad ni los conteos `tests`, solo `failures`. El
`testsuite` conserva además el estado y todos los intentos en sus properties. Un
test hoja, independientemente de sus intentos, continúa produciendo un único
testcase agregado. Los fallos de intentos previos se preservan en
`tondo.attempts` y nunca crean testcases rojos adicionales.

Las `properties` con prefijo `tondo.` son la extensión versionada de este
perfil. El primer `testsuite` en orden de ID actúa como portador de metadata de
la ejecución y contiene:

~~~text
tondo.format
tondo.json_format
tondo.edition
tondo.target
tondo.compiled
tondo.selection
tondo.ownership
tondo.shard
tondo.order
tondo.seed
tondo.execution_plan
tondo.retry
tondo.policy
tondo.limits
tondo.summary
~~~

Cada `testsuite` que representa una suite Tondo y cada `testcase` contiene las
properties de nodo aplicables:

~~~text
tondo.id
tondo.parent
tondo.package
tondo.kind
tondo.module
tondo.path
tondo.name
tondo.status
tondo.decisive_attempt
tondo.attempts
tondo.virtual_time
tondo.source
tondo.owners
tondo.synthetic
~~~

Arrays, objetos y `null` se codifican como JSON compacto canónico dentro de
`value`; booleanos y números usan su token JSON y un string escalar usa su
contenido después del escaping XML ordinario. Las properties aparecen en el
orden listado, sin nombres duplicados; una property no aplicable se omite y una
aplicable cuyo valor es nulo se conserva como `null`. Los valores de ejecución
son los mismos del JSON `/6`;
`tondo.format` vale `tondo-junit-report-0.2/3` y `tondo.json_format` conserva
`tondo-test-report-0.2/6`. Las properties forman la representación completa de
los campos normativos; los elementos JUnit convencionales proyectan además el
subconjunto que los consumidores suelen mostrar.

`tondo.virtual_time` contiene el array `virtual_time` del intento decisivo y es
`[]` cuando ese intento no creó dominios. Los dominios de intentos anteriores
permanecen además dentro de `tondo.attempts`; nunca se suman como si compartieran
un reloj. La property no cambia el atributo JUnit `time`.

En un hijo `failure` o `error` de un fallo agregado, el atributo `type` usa
`failure.code` del intento decisivo si existe y `failure.kind` en otro caso;
`message` usa su `failure.message` y el body contiene ese objeto como JSON
compacto. En `flaky-pass`, `type` es siempre `tondo.flaky-pass`, `message` es
`passed after retry` y el body contiene el array completo `attempts` como JSON
compacto. Un hijo `skipped` usa la razón del intento decisivo como `message`
para un skip propio y `blocked by <id>` para un bloqueo. Las properties
conservan en todos los casos los intentos y referencias exactos.

Todo scalar no representable por XML 1.0 que aparezca en un atributo o elemento
JUnit convencional se muestra mediante el escape ASCII visible `\u{HEX}`; su
property estructurada conserva el valor exacto como JSON. `system-out` y
`system-err` proyectan únicamente los streams del intento decisivo. Todos los
streams, tags, logs, failures y skips, incluidos los de intentos anteriores,
permanecen en `tondo.attempts` y en el reporte JSON canónico.

Los atributos `tests`, `failures`, `errors`, `skipped` y `time` se calculan sobre
los testcases agregados realmente emitidos, incluidos sintéticos, nunca sobre el
número de intentos. El orden de testsuites sigue el ID, no completion order.
Dentro de cada testsuite se ordena `@setup`, después `@flaky`, después hojas por
ID y por último `@teardown`; los elementos ausentes no ocupan posición. Shard,
seed, algoritmo y retry se incluyen aunque no haya hojas por `--allow-empty` o
por un shard vacío. En ese caso se emite un único `testsuite` de cero casos
llamado `@tondo-plan`, con `tondo.synthetic: "empty-plan"`, únicamente como
portador de la metadata de ejecución.

`time` usa segundos no negativos en decimal ASCII, sin exponente y con hasta
nueve dígitos fraccionarios obtenidos de un reloj monotónico. En `testcase`
representa la suma de tiempo activo de todos los intentos de esa hoja o nodo
sintético; un intento bloqueado aporta cero. En `testsuite` es la suma de sus
testcases y en `testsuites` la suma de sus suites hijas. Nunca representa el
intervalo wall-clock del contenedor, para no contar paralelismo de forma
dependiente del scheduler.

El perfil no promete que cada consumidor muestre properties o jerarquía de la
misma forma. Para preservar todos los campos y bytes, una invocación CI debe
solicitar ambos formatos en la misma ejecución:

~~~text
tondo test \
    --report junit=artifacts/tests.xml \
    --report json=artifacts/tests.json
~~~

### 15.6 Fallo de compilación

Si la compilación falla:

- `compiled` vale `false`.
- `suites`, `tests` y `execution_plan` están vacíos porque ningún setup ni body
  se ejecutó.
- `retry.rounds` está vacío; un fallo de compilación no consume intentos.
- En un reporte de ejecución, todos los contadores de `summary` valen `0`.
- Los diagnostics se emiten por stderr mediante el formato solicitado con
  `--diagnostic-format`.
- El reporte JSON no copia diagnostics como strings.
- No se produce JUnit, porque no existe una ejecución que proyectar.

## 16. Conformidad

Una implementación de esta extensión debe publicar una suite distinta a
`tondo-conformance-0.1`. La suite cubre como mínimo:

1. Tokens, CST lossless, parser y formatter de `suite_decl` y `test_decl`.
2. Reserva de `suite` y `test` únicamente en edición 0.2.
3. Rechazo de cada forma alternativa, hook y registro dinámico ausente.
4. Árbol estático, suites vacías, nesting y colisiones de hermanos entre
   archivos.
5. Identidad de suite/test, parent IDs y orden independiente del orden de
   archivos.
6. Capturas válidas `let: Copy + Send + Share` y rechazo de `var`, préstamos,
   valores afines y obligaciones terminales.
7. Las firmas exactas y test-only del núcleo `std.testing`, incluido
   `VirtualTime`, rechazo desde producción y ausencia de `TestContext`,
   `currentTest()` o identidad observable.
8. Envelope no falsificable, propagación por helpers/closures/tasks
   estructuradas, aislamiento paralelo y rechazo verifier de operaciones
   forjadas.
9. `testing.log` en body/setup/teardown y helpers, asociación exacta, orden,
   límite de output y separación de stdout/stderr.
10. `testing.failNow: Never`, `P0007`, unwind y continuidad de hermanos.
11. Skip de hoja/setup, razón única, `blocked-skip`, cleanup y precedencia,
    `P2001`, policy default y `--deny-skips`.
12. `testing.tags`, merge idempotente, `P2002` por conflicto, atribución desde
    helpers/tasks, uso durante cleanup, presupuesto de output y prohibición de
    usar tags runtime para discovery, selección, sharding u orden.
13. Setup síncrono, async, infallible y fallible inferido.
14. Ejecución de setup exactamente una vez y solo para subárboles seleccionados.
15. Nesting exterior-interior, teardown interior-exterior y LIFO de `defer`.
16. Fallo de setup, `blocked-setup`, causa única y continuidad de hermanos.
17. Fallo de teardown sin reescribir resultados de descendientes.
18. Retorno normal, error, pánico, assert, resource limit y timeout de tests.
19. `defer`, terminal obligations y unwind en terminales de suite/test.
20. Async, `scope`, `spawn`, cancelación y pánico de hijos.
21. Unit overlay con acceso privado que no repara producción inválida.
22. Integration root sin acceso privado.
23. Separación exacta entre grafo de producción y dev-dependencies.
24. Resolución CODEOWNERS automática, explícita y desactivada; precedencia de
    archivos, parsing estricto, última regla aplicable, paths lógicos, source y
    hash reportados, owners opacos y ausencia de red o efecto en producción.
25. Filtros, exact match de suite/test, list, selección vacía, allow-empty y
    combinaciones CLI de deny-skips.
26. Sharding `sha256-mod-v1` posterior a selección, validación de índices,
    cobertura y disjunción entre shards, compilación completa, lifecycle
    independiente y shard válido vacío.
27. Orden canónico `id-byte-order-v1`, orden aleatorio `sha256-tree-v1`, seed
    explícita/generada, suites estructuralmente atómicas, `execution_plan` y
    alcance exacto de replay con uno o varios jobs.
28. Orden serial, lifecycle jerárquico, límite conjunto de `--jobs N` y
    presentación estable aunque cambie completion order.
29. Captura separada de logs/stdout/stderr para suites y tests.
30. Parsing y combinaciones de CLI, formatos stdout, reportes repetibles,
    colisiones de paths, publicación atómica por archivo y exit `3`.
31. Reportes `tondo-test-report-0.2/6` y `tondo-test-list-0.2/5`, ownership,
    shard, order, tags, intentos, retry, `execution_plan`, invariantes de
    summary, skips, bloqueos y rechazo de schema inválido.
32. Perfil `tondo-junit-report-0.2/3`, mapeo de estados, lifecycle sintético,
    properties, streams, duración operacional, conteos y equivalencia con la
    misma ejecución JSON.
33. Targets y capabilities distintos.
34. Ausencia total de suites, tests, `std.testing`, metadata y
    dev-dependencies en productos de producción.
35. Ejecución individual mediante `--exact` equivalente a la misma hoja dentro
    del árbol completo bajo inputs idénticos.
36. Vectores de glob portable con `*`, `?` y `**`; match completo de
    suite/test, unión deduplicada, Unicode scalar, patrones inválidos, selección
    vacía y ausencia de regex o expansión del host.
37. Elegibilidad, absorción y orden de unidades de retry, rondas acotadas,
    lifecycle ancestral, subárbol de suite, conservación de shard/seed/plan y
    máximo global de jobs.
38. Worker nuevo por unidad de retry, reutilización exclusiva del artefacto
    inmutable, heap/roots/executor/envelopes/buffers/presupuestos nuevos y
    revocación de procesos y recursos rastreados antes de terminar.
39. Historial de intentos, causalidad `blocked_by`, intento decisivo,
    `flaky-pass`, summaries JSON y matriz de exit status con y sin
    `--allow-flaky`; skips o resource/infrastructure failures no se reintentan.
40. Proyección JUnit de retry: un testcase agregado por hoja, lifecycle y flaky
    sintéticos únicos, `tondo.retry`, `tondo.decisive_attempt`,
    `tondo.attempts`, streams decisivos, conteos por identidad y policy
    `--allow-flaky`.
41. Contrato temporal mínimo de producción compartido, body
    `CallOnce[async fn(ref VirtualTime)]`, propagación exacta de `E`, préstamo
    no escapable, identidad/mismatch entre proveedores, ausencia de `Clock`
    inyectado y rechazo desde producción.
42. Herencia del dominio por tasks estructuradas, cola ready y empates de timers
    deterministas, avance automático solo bajo quiescencia durable y catálogo
    que excluye esperas externas.
43. `settle` sin avance, `advance` exacto/cero/múltiples deadlines, salto
    automático, varios dominios secuenciales, rechazo de solapamiento `P2004`,
    deadlock `P2003` y rango/overflow `P2005`.
44. Separación entre tiempo monotónico virtual y calendario/I/O real, timeout
    wall-clock, resource limits, cancelación, pánico, skip, errores y cleanup
    durante creación, ejecución y desmontaje del dominio.
45. `virtual_time` por intento en JSON `/6`, orden y contadores canónicos,
    reinicio exacto entre retries, `tondo.virtual_time` en JUnit `/3`, duración
    JUnit real y equivalencia VM/backend para el mismo corpus temporal.

La VM de referencia y cada backend nativo deben ejecutar las mismas fuentes y
producir el mismo estado, código de pánico, output y reporte canónico. Duración
no participa en la comparación.

## 17. Referencia rápida

### Declaración

~~~tondo
test behaviorName {
    assert(condition)
}
~~~

### Suite

~~~tondo
suite behaviorGroup {
    let resource = Resource.acquire()?
    let configuration: String = resource.configuration()
    defer Resource.release(resource)

    test firstBehavior {
        assert(run(configuration))
    }

    suite nestedGroup {
        test secondBehavior {
            assert(runAgain(configuration))
        }
    }
}
~~~

### Fallible y async

~~~tondo
test loadsValue {
    let value = await loadValue()?
    assert(value == expected)
}
~~~

### Log, tags, fallo inmediato y skip

~~~tondo
import std.testing

test externalBehavior {
    testing.tags([
        "component": "payments",
        "kind": "integration",
    ])
    testing.log("starting external behavior")

    if not prerequisiteAvailable() {
        testing.skip("requires the external prerequisite")
    }

    if not behaviorIsCorrect() {
        testing.failNow("behavior was not correct")
    }
}
~~~

### Tabla

~~~tondo
test handlesCases {
    for (input, expected) in cases {
        assert(transform(input) == expected)
    }
}
~~~

### Tiempo virtual

~~~tondo
import std.testing

test expiresAtDeadline {
    let timeout = operationTimeout()

    await testing.withVirtualTime(async (clock) {
        scope {
            let result = spawn operation(timeout)
            await clock.advance(timeout)
            assert(await result == OperationOutcome.TimedOut)
        }
    })?
}
~~~

### Comandos

~~~text
tondo test
tondo test --list
tondo test --filter parser
tondo test --glob 'application::integration::**::creates*'
tondo test --exact application::unit::parser::rejectsInvalidToken
tondo test --exact application::integration::users::userApi
tondo test --retry 2
tondo test --retry 2 --allow-flaky
tondo test --jobs 4
tondo test --shard 2/8
tondo test --order random --seed 5eed
tondo test --codeowners auto
tondo test --deny-skips
tondo test --test-format json
tondo test \
    --report junit=artifacts/tests.xml \
    --report json=artifacts/tests.json
~~~

### Regla de diseño

> `suite` aporta jerarquía y lifecycle léxico; `test` identifica una hoja
> aislada. `std.testing` controla y anota el nodo mediante un envelope
> estructurado que nunca se expone como valor y puede abrir un dominio temporal
> prestado, determinista y opt-in sobre las APIs de producción. Ownership,
> sharding, orden y reportes son políticas reproducibles del runner; glob
> selecciona sin depender del host y cada retry explícito obtiene una frontera
> y un reloj nuevos sin ocultar flakiness. El resto del código continúa siendo
> Tondo ordinario.
