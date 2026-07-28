# Tondo: especificación del lenguaje y toolchain de testing

- **Estado:** diseño normativo aprobado para Tondo 0.2; todavía no implementado.
- **Revisión:** 0.2-draft.2 — 2026-07-28.
- **Edición objetivo:** Tondo 0.2.
- **Especificación base:** [Tondo 0.1](./TONDO_LANGUAGE_SPEC.md).
- **SHA-256 de la base:** `ded4e17ab57836d032e5fb9e5be5dba03fc83ac6ff74cee90ab1bb7f8e5c7084`.
- **Formatos de tooling:** `tondo-test-report-0.2/2` y
  `tondo-test-list-0.2/2`.

Esta especificación añade a Tondo las declaraciones `suite` y `test` y define
cómo el toolchain descubre, compila, ejecuta y reporta árboles estáticos de
tests. `suite` es un contenedor léxico con lifecycle compartido; `test` es
siempre una hoja ejecutable. Complementa Tondo 0.1; no modifica
retroactivamente esa edición ni la suite publicada `tondo-conformance-0.1`.

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

El sistema de testing de Tondo persigue seis objetivos:

1. Escribir un test ordinario requiere únicamente un nombre y un bloque.
2. Agrupar tests y compartir un recurso costoso requiere únicamente una `suite`
   léxica; no clases, annotations ni registro runtime.
3. El test utiliza exactamente el lenguaje normal: `assert`, `?`, `match`,
   `defer`, `for`, `scope`, `spawn`, `await`, ownership y préstamos conservan su
   significado.
4. Descubrimiento, ejecución y reporte son deterministas y observables.
5. El código y las dependencias de test no cambian el artefacto de producción.
6. El núcleo no introduce clases de test, annotations, macros, reflection ni
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
`std.testing` añadirá comparaciones, diffs y recursos de test como API ordinaria.

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

### 2.3 Una sola forma por concepto

No existen formas equivalentes como:

~~~text
@test
#[test]
test fn name()
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
para workspaces en los que una partición pueda no contener tests.

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
- Entradas privadas de setup y de tests hoja.
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
seleccionada o del body de un test hoja.

## 7. Modelo de ejecución

### 7.1 Árbol de ejecución y aislamiento

Después de seleccionar tests hoja, el runner construye el bosque mínimo que los
contiene: cada test seleccionado y todas sus suites ancestras. Ningún otro nodo
se ejecuta.

Cada test hoja obtiene:

- Un scope raíz nuevo.
- Estado de runtime, roots, tasks y handles no observable desde otra hoja salvo
  los snapshots de suite permitidos por 4.3.
- Captura separada de stdout y stderr del runtime Tondo.
- Presupuesto de recursos independiente.
- Un resultado independiente.

Cada suite ejecutada obtiene un entorno de lifecycle separado que conserva
sus bindings de setup y su pila de cleanup hasta que terminan los descendientes.
Los hijos solo observan sus snapshots `Copy + Send + Share`; no reciben acceso
general al heap, stack, préstamos ni propietarios de la suite.

Una implementación puede reutilizar threads, allocators o procesos internos
solo si esa reutilización no expone otros valores, roots, tasks, handles,
buffers, pánicos u output. No existen globals mutables Tondo que sobrevivan
entre entradas. Los efectos externos —filesystem, procesos, red, reloj o
servicios— no se revierten mágicamente y deben aislarse mediante nombres,
recursos y cleanup explícitos.

### 7.2 Lifecycle de suite

Una suite se ejecuta de esta forma:

1. Si no contiene ningún test seleccionado, no se entra en ella.
2. El runner ejecuta su setup exactamente una vez.
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

Este lifecycle equivale a setup/teardown una vez por contenedor. No implica
`beforeEach` ni `afterEach`: cada hoja construye sus fixtures propias mediante
helpers y `defer`.

### 7.3 Inicio y terminación de entradas

El runner conduce cada body de test y setup de suite —síncronos o async según la
inferencia ordinaria— y cada teardown síncrono hasta uno de estos terminales:

- Retorno normal.
- Error recuperable no manejado.
- Pánico.
- Límite de recursos.
- Timeout del runner.
- Fallo de infraestructura.

Antes de registrar un terminal de lenguaje, el runtime ejecuta cleanup y cancela
y espera hijos estructurados según la especificación base. Una entrada no se
marca como finalizada mientras quede cleanup estructurado pendiente. El tiempo
durante el que una suite solo espera a sus descendientes no ejecuta código de
usuario y no constituye una cuarta fase.

### 7.4 Pánico y continuidad

Un pánico termina el test actual después del unwind, no el proceso completo del
runner. El runner conserva el código `P`, ubicación y stack trace disponibles y
continúa con tests posteriores cuando el aislamiento sigue siendo válido.

Un pánico en setup o teardown pertenece a la suite y sigue 7.2. Un abort fuera
del modelo de pánico, corrupción del runtime o imposibilidad de restablecer
aislamiento se clasifica como fallo de infraestructura. El runner puede detener
el bosque restante porque ya no puede garantizar resultados fiables. En ese
caso termina con exit `3` y no emite un reporte canónico incompleto; todo reporte
`tondo-test-report-0.2/2` válido clasifica cada hoja seleccionada.

### 7.5 Errores recuperables

Un valor que alcanza el canal `E` inferido hace fallar el test o fase de suite
actual. El reporte conserva como mínimo la identidad nominal visible de su tipo
y la ubicación del terminal. Su presentación humana sigue las reglas de la
frontera de `main`: no usa reflection para revelar campos privados ni promete
serialización estable del payload.

Para verificar un error esperado, el test lo consume localmente con `match`; no
deja que alcance al runner.

### 7.6 Inputs de host

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

### 7.7 Límites y timeouts

Cada test hoja y cada fase activa de setup o teardown se ejecuta con límites
finitos de instrucciones o trabajo, memoria, profundidad y output. El toolchain
publica sus defaults y registra los valores efectivos o el hash de su resource
profile en el reporte.

`--timeout` aplica de forma independiente a un body de test, a un setup de suite
y a un teardown de suite. El reloj de una suite se pausa mientras solo espera
descendientes; por tanto, una suite grande no consume el timeout por sumar la
duración de sus tests.

Un timeout wall-clock es un límite del runner, no un error Tondo ni un pánico. El
runner debe poder terminar la entrada aislada incluso si el código no llega a un
punto cooperativo de cancelación. No puede dejar un proceso o thread de usuario
ejecutándose después de reportar el terminal.

Los presupuestos estructurales y de runtime siempre permanecen finitos. El
timeout wall-clock puede desactivarse únicamente mediante `--timeout none`; esa
opción no desactiva ningún otro presupuesto. Un timeout o resource limit nunca
se convierte automáticamente en test ignorado.

Timeout y agotamiento de presupuesto pueden impedir que el código Tondo complete
sus `defer`; no son terminales del lenguaje con garantía de unwind. El runner sí
debe limpiar su propia frontera de aislamiento y declarar el estado
correspondiente. Una suite o test con efectos externos no puede confiar en
teardown de usuario después de una terminación forzada.

## 8. Selección, orden y paralelismo

### 8.1 Orden canónico

El registro, los descriptores y los resultados de suites y tests se ordenan
lexicográficamente por identidad visible completa usando bytes UTF-8. Los
descendientes de una suite forman así un rango contiguo.

La ejecución por defecto utiliza `--jobs 1` y recorre los tests hoja en ese
orden, entrando en una suite justo antes de su primer descendiente seleccionado
y abandonándola después del último. Esto proporciona feedback reproducible sin
depender del número de CPUs de la máquina.

### 8.2 Selección

El runner ofrece exactamente dos selectores básicos:

- `--filter text`: substring bytewise, case-sensitive, sobre la identidad
  visible completa de cada test hoja.
- `--exact id`: igualdad bytewise con la identidad visible de un test o suite.

Son mutuamente excluyentes y cada uno aparece como máximo una vez. No hay regex,
glob ni interpretación locale-dependent.

Un exact match de test selecciona esa hoja. Un exact match de suite selecciona
todos sus tests descendientes. Puesto que el ID de suite es prefijo de sus
descendientes, un filtro que contiene su path los selecciona naturalmente, pero
`--filter` nunca devuelve una suite sin hojas.

Toda selección incorpora las suites ancestras necesarias. `--list` compila y
valida el árbol, aplica el selector y emite los tests seleccionados junto con
esas suites sin ejecutar ningún setup ni body.

### 8.3 Paralelismo explícito

`--jobs N`, con `N > 0`, permite ejecutar simultáneamente hasta `N` entradas de
usuario entre setup, test y teardown. Una suite termina su setup antes de
programar hijos y no comienza teardown hasta que todos sus descendientes
seleccionados han terminado.

El resultado final y la salida capturada continúan presentándose en orden
canónico, no en orden de finalización.

El runner no garantiza un orden de efectos externos entre tests paralelos. Una
suite que comparta un servicio bajo `--jobs N` es responsable de que ese servicio
admita concurrencia o de solicitar `--jobs 1`. Los snapshots estáticos impiden
data races de lenguaje; el aislamiento de runtime y el máximo global permanecen
obligatorios.

### 8.4 Sin dependencias entre tests

No existe sintaxis para ordenar tests, declarar dependencias ni compartir una
fixture mutable ordinaria entre ellos. Una suite puede compartir snapshots
inmutables y efectos externos explícitos, pero ningún hijo puede preparar estado
para otro.

Cada test debe poder ejecutarse solo mediante `--exact`; el runner entra en las
mismas suites ancestras y debe producir el mismo resultado de lenguaje que
dentro del árbol completo con los mismos inputs declarados.

### 8.5 Sin retries implícitos

El runner ejecuta cada test seleccionado exactamente una vez. No reintenta
fallos, no decide que un test es flaky y no convierte éxito después de retry en
verde.

Campañas que repitan un test deben solicitarlo fuera del resultado canónico,
registrar cada intento y no sustituir la regresión determinista.

## 9. Resultados, diagnósticos y salida

### 9.1 Estados de test y suite

Cuando la compilación termina correctamente y el runner puede producir un
reporte fiable, cada test seleccionado termina exactamente en uno de estos
estados:

| Estado | Significado |
|---|---|
| `passed` | La entrada devolvió `Unit` normalmente. |
| `failed-error` | Un error recuperable alcanzó el runner. |
| `failed-panic` | Ocurrió un pánico Tondo después de unwind. |
| `resource-limit` | Se agotó un presupuesto configurado. |
| `timeout` | Venció el límite wall-clock del runner. |
| `infrastructure` | El harness, runtime o aislamiento dejó de ser fiable. |
| `blocked-setup` | No se invocó porque falló una suite ancestral seleccionada. |

`blocked-setup` no contiene un `failure` duplicado; identifica mediante
`blocked_by` la suite que conserva la causa. No significa ignored, skipped ni
éxito.

Cada suite necesaria conserva uno de los seis estados ejecutados anteriores o
`blocked-setup` cuando una suite ancestral impidió incluso su setup. Para una
suite ejecutada, `phase` vale `setup` o `teardown` en un fallo y `null` al pasar.
El estado `passed` solo describe su lifecycle propio y no agrega resultados de
descendientes.

Las combinaciones válidas son cerradas:

- `passed` y `blocked-setup` siempre usan `phase: null`.
- Cualquiera de los cinco estados de fallo ejecutado puede usar
  `phase: setup`.
- `phase: teardown` admite `failed-panic`, `resource-limit`, `timeout` o
  `infrastructure`. No admite `failed-error`, porque `defer` es infallible desde
  el sistema de tipos y no puede propagar un error recuperable.

No existe `ignored`, `expected-failure`, `flaky` ni `passed-after-retry` en este
contrato.

### 9.2 `assert`

`assert(false)` conserva el pánico `P0007`. Dentro de un test, el runner lo
clasifica como `failed-panic`; dentro de setup o teardown hace fallar esa fase de
suite. No introduce un segundo código ni una excepción recuperable.

La representación fuente de la condición, el mensaje, la ubicación y el stack
trace se conservan según la especificación base. Una librería puede construir
mensajes mejores, pero no alterar el terminal.

### 9.3 Captura de output

Stdout y stderr de cada entrada de suite o test se capturan por separado como
UTF-8. El modo humano:

- Muestra siempre la identidad y el estado.
- Muestra output de tests o suites fallidos.
- Oculta output de entradas que pasan salvo `--show-output`.
- Nunca intercala bytes de dos entradas.

El output de procesos hijos solo pertenece a la captura si el programa lo
redirige explícitamente a los streams Tondo. Heredar descriptores del runner no
constituye captura conforme.

El diagnóstico que el runner construye para error, pánico, timeout o
infraestructura pertenece a `failure`; no se añade artificialmente al
`stderr` capturado del programa.

En una suite, cada stream concatena los bytes producidos por su setup y por su
teardown posterior en ese orden, sin incluir output de descendientes. Si la
suite queda `blocked-setup`, ambos streams están vacíos.

### 9.4 Duración

Una implementación puede mostrar duración como metadato informativo. La duración
wall-clock no forma parte del resultado semántico, del orden ni del reporte
canónico reproducible.

### 9.5 Exit status

`tondo test` utiliza:

| Exit | Condición |
|---|---|
| `0` | Compilación correcta, todas las suites necesarias y tests seleccionados `passed`; o selección vacía solicitada con `--allow-empty`. |
| `1` | Error de compilación, selección vacía no permitida o al menos una suite/test no pasó. |
| `2` | Uso inválido de CLI. |
| `3` | Fallo interno del toolchain antes de producir un reporte fiable. |

Un test que llame a APIs de proceso no puede elegir el exit status del runner.
Un estado `infrastructure` que todavía permite un reporte íntegro usa exit `1`;
si el runner no puede garantizar ni serializar ese reporte, usa exit `3`.

## 10. Contrato de `tondo test`

Interfaz mínima:

~~~text
tondo test [--manifest <path>]
           [--filter <text> | --exact <node-id>]
           [--list]
           [--jobs <positive-int>]
           [--timeout <duration|none>]
           [--diagnostic-format <human|json>]
           [--test-format <human|json>]
           [--show-output]
           [--allow-empty]
~~~

Reglas:

- Sin `--manifest`, el toolchain descubre el proyecto mediante su contrato
  ordinario y materializa un plan cerrado antes de compilar.
- `--filter` y `--exact` seleccionan ejecución, no compilación.
- `--list` no ejecuta bodies y no admite `--show-output`.
- `--jobs` vale `1` por defecto.
- `--timeout` aplica por test hoja y por fase activa de suite, no al tiempo total
  del subárbol.
- `--test-format human` es el default interactivo.
- `--test-format json` emite exactamente un reporte
  `tondo-test-report-0.2/2`, o una lista `tondo-test-list-0.2/2` con `--list`.
- Los diagnostics de compilación conservan `--diagnostic-format` y su schema
  propio; no se insertan como strings ambiguos dentro de resultados de test.
- Opciones desconocidas, repetidas cuando no son repetibles o combinaciones
  incompatibles terminan con exit `2` sin compilar.

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

### 11.2 Responsabilidad de `std.testing`

La especificación de la librería estándar debe definir `std.testing` como un
módulo ordinario, no como registro ni framework alternativo. Como mínimo debe
considerar:

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
suites o tests en runtime.

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
- Orden o dependencias entre tests.
- Estado mutable compartido de suite.
- Ignorados o expected failures dentro de fuente.
- Retries automáticos.
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
| `E2003` | `invalid-test-source-declaration` | Una fuente de test intenta exportar API, alterar la unidad de producción sellada o ser consumida desde producción. |
| `E2004` | `empty-test-suite` | Una suite no contiene ningún miembro directo y, por tanto, ningún test descendiente. |
| `E2005` | `invalid-suite-capture` | Un descendiente intenta capturar `var`, préstamo, valor afín/terminal o un tipo que no cumple `Copy + Send + Share`. |

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

Selector vacío, timeout e infraestructura son diagnósticos del toolchain, no
nuevos errores de compilación `E`.

## 15. Formato machine-readable

### 15.1 Forma canónica

`--test-format json` emite un único objeto JSON UTF-8, sin BOM ni bytes
posteriores salvo `LF`, por stdout. Los valores concretos siguientes son un
ejemplo; la forma y los tipos de los campos son normativos:

~~~json
{
  "format": "tondo-test-report-0.2/2",
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
      "status": "passed",
      "phase": null,
      "blocked_by": null,
      "failure": null,
      "stdout": "",
      "stderr": ""
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
      "status": "passed",
      "blocked_by": null,
      "failure": null,
      "stdout": "",
      "stderr": ""
    }
  ],
  "summary": {
    "selected": 1,
    "executed": 1,
    "passed": 1,
    "blocked_setup": 0,
    "failed_error": 0,
    "failed_panic": 0,
    "resource_limit": 0,
    "timeout": 0,
    "infrastructure": 0,
    "suite_selected": 1,
    "suite_passed": 1,
    "suite_blocked_setup": 0,
    "suite_failed": 0,
    "failed": 0
  }
}
~~~

`selection.kind` es `all`, `filter` o `exact`; `value` es `null` para `all` y el
texto exacto en los otros casos. `timeout_ms` es `null` solo para
`--timeout none`. El resource profile contiene todos los presupuestos finitos
del frontend, verifiers y runtime y se distribuye de forma recuperable por el
toolchain; el hash del reporte identifica exactamente sus bytes canónicos.

`suites` contiene el bosque mínimo necesario para los tests seleccionados,
incluidos nodos `blocked-setup`. `tests` contiene todas las hojas seleccionadas,
se hayan ejecutado o bloqueado. Todos los campos aparecen incluso cuando están
vacíos o son `null`; `kind` es `unit` o `integration`.

`parent` contiene el ID de la suite inmediata o `null` para un nodo top-level.
`path` contiene únicamente los identificadores de suite y test posteriores al
module path. En una suite termina en su propio nombre; en un test termina en el
nombre del test.

Para una suite:

- `phase` es `setup` o `teardown` cuando su propio lifecycle falla.
- `phase` es `null` cuando pasa o queda bloqueada.
- `blocked_by` es el ID de la primera suite fallida que impidió entrar en ella,
  y es `null` en otro caso.
- La pareja `status`/`phase` debe pertenecer al conjunto cerrado definido en
  9.1.

Para un test, `blocked_by` sigue la misma regla y solo es no nulo con estado
`blocked-setup`. No se admiten campos de extensión sin cambiar el identificador
de formato.

### 15.2 Orden y estabilidad

- `capabilities` se ordena por bytes.
- `suites` se ordena por `id`.
- `tests` se ordena por `id`.
- Keys conocidas se serializan en el orden mostrado por el schema del
  toolchain.
- `stdout` y `stderr` contienen texto UTF-8 exacto.
- `summary.selected = summary.executed + summary.blocked_setup`.
- `summary.executed` es la suma exacta de `passed` y los cinco contadores de
  fallo de test ejecutado.
- `summary.suite_selected` es la suma exacta de `suite_passed`,
  `suite_blocked_setup` y `suite_failed`.
- `summary.suite_failed` cuenta suites en cualquiera de los cinco estados de
  fallo ejecutado; las suites bloqueadas no vuelven a contar la causa.
- `summary.failed` es la suma de los cinco contadores de fallo de test y
  `suite_failed`. Las hojas bloqueadas tampoco duplican el fallo de setup.
- Duración, timestamps, PID, número de CPU, paths físicos y direcciones no
  aparecen en la forma canónica.
- Paths de source dentro de failures son lógicos.

### 15.3 Failure

Cuando `status` es `passed` o `blocked-setup`, `failure` es `null`. En un estado
de fallo ejecutado contiene exactamente:

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

Un test o suite `blocked-setup` explica su causa únicamente mediante
`blocked_by`; el nodo señalado contiene el único objeto `failure` normativo. Un
fallo de teardown de suite no reemplaza failures de tests ya ejecutados.

Campos privados y payloads opacos no se serializan por reflection.

### 15.4 Lista machine-readable

`--list --test-format json` emite `tondo-test-list-0.2/2`. Comparte `edition`,
`target`, `compiled` y `selection`, pero contiene descriptores separados sin
estado, phase, failure, bloqueo ni output:

~~~json
{
  "format": "tondo-test-list-0.2/2",
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
  "suites": [
    {
      "id": "application::unit::math::arithmetic",
      "parent": null,
      "package": "application",
      "kind": "unit",
      "module": "math",
      "path": ["arithmetic"],
      "name": "arithmetic"
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
      "name": "addReturnsSum"
    }
  ]
}
~~~

Ambos arrays se ordenan por `id`. `suites` contiene las ancestras necesarias y
las suites descendientes seleccionadas; `tests` contiene las hojas
seleccionadas. `--allow-empty` permite ambos arrays vacíos; sin esa opción, una
selección vacía conserva exit `1`.

### 15.5 Fallo de compilación

Si la compilación falla:

- `compiled` vale `false`.
- `suites` y `tests` están vacíos porque ningún setup ni body se ejecutó.
- En un reporte de ejecución, todos los contadores de `summary` valen `0`.
- Los diagnostics se emiten por stderr mediante el formato solicitado con
  `--diagnostic-format`.
- El reporte no copia diagnostics como strings.

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
7. Setup síncrono, async, infallible y fallible inferido.
8. Ejecución de setup exactamente una vez y solo para subárboles seleccionados.
9. Nesting exterior-interior, teardown interior-exterior y LIFO de `defer`.
10. Fallo de setup, `blocked-setup`, causa única y continuidad de hermanos.
11. Fallo de teardown sin reescribir resultados de descendientes.
12. Retorno normal, error, pánico, assert, resource limit y timeout de tests.
13. `defer`, terminal obligations y unwind en terminales de suite/test.
14. Async, `scope`, `spawn`, cancelación y pánico de hijos.
15. Unit overlay con acceso privado que no repara producción inválida.
16. Integration root sin acceso privado.
17. Separación exacta entre grafo de producción y dev-dependencies.
18. Filtros, exact match de suite/test, list, selección vacía y allow-empty.
19. Orden serial, lifecycle jerárquico y presentación estable bajo `--jobs N`.
20. Captura separada de stdout/stderr para suites y tests.
21. Reportes `tondo-test-report-0.2/2` y `tondo-test-list-0.2/2`, invariantes de
    summary, bloqueos y rechazo de schema inválido.
22. Targets y capabilities distintos.
23. Ausencia total de suites, tests y dev-dependencies en productos de
    producción.
24. Ejecución individual mediante `--exact` equivalente a la misma hoja dentro
    del árbol completo bajo inputs idénticos.

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

### Tabla

~~~tondo
test handlesCases {
    for (input, expected) in cases {
        assert(transform(input) == expected)
    }
}
~~~

### Comandos

~~~text
tondo test
tondo test --list
tondo test --filter parser
tondo test --exact application::unit::parser::rejectsInvalidToken
tondo test --exact application::integration::users::userApi
tondo test --jobs 4
tondo test --test-format json
~~~

### Regla de diseño

> `suite` aporta jerarquía y lifecycle léxico; `test` identifica una hoja
> aislada. El código de ambos sigue siendo Tondo ordinario y la ergonomía
> adicional pertenece a `std.testing`.
