# Tondo: especificación del lenguaje y toolchain de testing

- **Estado:** diseño normativo aprobado para Tondo 0.2; todavía no implementado.
- **Revisión:** 0.2-draft.1 — 2026-07-28.
- **Edición objetivo:** Tondo 0.2.
- **Especificación base:** [Tondo 0.1](./TONDO_LANGUAGE_SPEC.md).
- **SHA-256 de la base:** `ded4e17ab57836d032e5fb9e5be5dba03fc83ac6ff74cee90ab1bb7f8e5c7084`.
- **Formatos de tooling:** `tondo-test-report-0.2/1` y
  `tondo-test-list-0.2/1`.

Esta especificación añade a Tondo una única declaración de test y define cómo el
toolchain descubre, compila, ejecuta y reporta tests. Complementa Tondo 0.1; no
modifica retroactivamente esa edición ni la suite publicada
`tondo-conformance-0.1`.

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
3. [Declaración `test`](#3-declaración-test)
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

El sistema de testing de Tondo persigue cinco objetivos:

1. Escribir un test ordinario requiere únicamente un nombre y un bloque.
2. El test utiliza exactamente el lenguaje normal: `assert`, `?`, `match`,
   `defer`, `for`, `scope`, `spawn`, `await`, ownership y préstamos conservan su
   significado.
3. Descubrimiento, ejecución y reporte son deterministas y observables.
4. El código y las dependencias de test no cambian el artefacto de producción.
5. El núcleo no introduce clases de test, annotations, macros, reflection ni
   hooks de ciclo de vida.

Forma mínima:

~~~tondo
test addReturnsSum {
    assert(add(20, 22) == 42)
}
~~~

`test` existe porque el registro estático de tests, su aislamiento y su acceso
unitario a declaraciones privadas no pueden expresarse como una función de
librería sin introducir registro en runtime, efectos top-level, reflection,
convenciones mágicas de nombres o boilerplate manual.

La declaración no sustituye a una librería de assertions. El lenguaje define
el test como unidad ejecutable; `assert` proporciona la comprobación mínima y
`std.testing` añadirá comparaciones, diffs y recursos de test como API ordinaria.

## 2. Compatibilidad y límite de edición

### 2.1 Tondo 0.1 permanece inmutable

Tondo 0.1 no contiene una keyword ni una declaración `test`. Su especificación,
diagnósticos, grammar, formatter y suite de conformidad permanecen byte a byte
independientes de esta extensión.

Una implementación no puede anunciar soporte Tondo 0.1 y aceptar `test` como
extensión silenciosa. Debe seleccionar explícitamente la edición 0.2 o una
edición posterior que incorpore este contrato.

### 2.2 `test` es keyword en Tondo 0.2

La edición 0.2 añade `test` a la lista de palabras reservadas. Por tanto:

- `test` no puede utilizarse como identificador no calificado.
- Una función, variable, tipo, módulo o parámetro de usuario no puede llamarse
  `test`.
- La API estándar utiliza el nombre de módulo `std.testing`, no `std.test`.
- Código Tondo 0.1 que utilizara `test` como identificador requiere renombrarlo
  al migrar de edición.

Reservarla globalmente evita una keyword contextual cuya interpretación dependa
del source set o del lugar del parser.

### 2.3 Una sola forma

No existen formas equivalentes como:

~~~text
@test
#[test]
test fn name()
fn testName(test: Test)
testing.register(...)
~~~

La única forma canónica es:

~~~text
test identifier block
~~~

## 3. Declaración `test`

### 3.1 Sintaxis

~~~tondo
test parsesNegativeNumbers {
    assert(parseInt("-12") == ok(-12))
}
~~~

Gramática:

~~~ebnf
top_decl_0_2 = top_decl_0_1 | test_decl ;
test_decl     = "test", identifier, block ;
~~~

`test_decl` es una declaración de nivel superior. No puede aparecer dentro de
una función, cierre, `impl`, trait, bloque, otro test ni script.

El parser de edición 0.2 reconoce `test_decl` dentro de una forma módulo y
conserva el nodo aunque la comprobación posterior determine que la fuente es
`production`. Esa separación permite emitir `E2001` con el range de toda la
declaración. La edición 0.1 sigue utilizando exactamente `top_decl_0_1`.

No admite:

- `pub` ni `priv`.
- Parámetros.
- Parámetros genéricos ni constraints.
- Receptor.
- Anotación de retorno o de error.
- Modificadores `async` o `unsafe`.
- Atributos.
- Nombre string alternativo.

Una descripción humana opcional se escribe como documentación sobre la
declaración. El identificador continúa siendo la identidad estable:

~~~tondo
/// Verifica el redondeo simétrico alrededor de cero.
test roundsHalfAwayFromZero {
    assert(round(1.5) == 2)
    assert(round(-1.5) == -2)
}
~~~

### 3.2 Nombre e identidad

El nombre sigue la convención `camelCase` de las funciones. Incumplirla produce
el warning de naming ordinario. `_` es un descarte, no una identidad de test, y
no puede ocupar esta posición.

La identidad semántica exacta de un test es:

~~~text
PackageId + module path + test identifier
~~~

La identidad visible del runner es:

~~~text
package-name::unit::module.path::testName
package-name::integration::relative.path::testName
~~~

Tooling la interpreta con la forma cerrada:

~~~ebnf
visible_test_id = package_name, "::", test_kind, "::",
                  logical_module_path, "::", identifier ;
test_kind       = "unit" | "integration" ;
~~~

El nombre de paquete mostrado es el nombre local declarado por el manifiesto;
la identidad interna conserva el `PackageId` completo para distinguir versiones
u orígenes diferentes. El segmento de clase evita que un unit test y una raíz
de integración produzcan selectores visibles ambiguos.

Los tests forman un registro de tooling separado de los namespaces de tipos y
valores. Puede coexistir una función y un test con el mismo identificador:

~~~tondo
fn normalize(value: String): String {
    value
}

test normalize {
    assert(normalize("value") == "value")
}
~~~

Dos tests con el mismo identificador dentro del mismo módulo son un error aunque
se encuentren en archivos distintos. Un test no puede referenciarse, importarse,
llamarse ni convertirse a un valor de función.

### 3.3 Formato canónico

El formatter emite:

~~~text
test name {
    body
}
~~~

Hay exactamente un espacio entre `test` y el identificador y otro antes de `{`.
El body utiliza las reglas ordinarias de bloques. Dos declaraciones consecutivas
se separan igual que dos funciones de módulo. El formatter nunca transforma una
función en test ni infiere un test a partir de su nombre.

## 4. Semántica estática

### 4.1 Entrada oculta

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

### 4.2 Async y concurrencia

No se escribe `async test`. La necesidad de async se infiere porque la
declaración no forma parte de una API invocable.

~~~tondo
test loadsConfiguration {
    let config = await loadConfiguration()?
    assert(config.port > 0)
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
Un test escribe `scope` de forma explícita igual que un script.

### 4.3 Ownership, préstamos y `defer`

Un test no relaja ownership. Todo valor afín, obligación terminal, préstamo o
cleanup debe satisfacer las mismas reglas que en una función normal.

`defer` es la única construcción de teardown del lenguaje:

~~~tondo
test writesAndReadsRecord {
    let workspace = createWorkspace()?
    defer Workspace.remove(workspace)

    writeRecord(workspace, "Ada")?
    assert(readRecord(workspace)? == "Ada")
}
~~~

Un pánico, error, `return` o cancelación ejecuta el unwind estructurado antes de
entregar el resultado al runner.

### 4.4 Unsafe

No existe `unsafe test`. Una operación raw requiere una región `unsafe` local:

~~~tondo
test readsAlignedByte {
    let address = makeTestAddress()
    let value = unsafe {
        address.read()
    }

    assert(value == 42u8)
}
~~~

La presencia de un test nunca rebaja las obligaciones de procedencia,
alineación, inicialización, aliasing o lifetime.

### 4.5 Visibilidad

Un test unitario ve las declaraciones privadas del módulo al que acompaña. Esa
es una concesión estática de visibilidad, no reflection ni acceso raw.

Un test de integración pertenece a un consumidor separado y solo ve la API
pública importada. No existe una opción del runner que eleve su visibilidad.

Los campos privados, nombres ocultos y tipos opacos continúan ausentes de
diagnósticos y reportes cuando el test no tiene visibilidad válida.

### 4.6 Ausencia de efectos de importación

Un test solo se ejecuta cuando el runner invoca su entrada. Importar un módulo
que contiene un overlay de test no ejecuta tests ni registra callbacks en
runtime.

El orden textual de los tests no produce inicialización global. Tondo continúa
sin globals mutables ni efectos de importación.

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

Solo una fuente `unit-test` o `integration-test` puede contener `test_decl`.
Encontrarlo en `production` produce `E2001`.

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
privadas auxiliares y tests. No puede:

- Hacer que una fuente de producción inválida compile.
- Reabrir ni volver a resolver bodies de producción.
- Cambiar la interfaz pública, capacidades derivadas o artefacto de producción.
- Exportar una declaración `pub`.
- Ser importado desde un source set de producción.

Una colisión entre una declaración auxiliar y una declaración ya visible utiliza
las reglas ordinarias de nombres. Los tests permanecen en su registro separado.

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

Por ello un test de integración:

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

### 5.6 Suite vacía

Una invocación normal que descubre cero tests o cuyo selector no selecciona
ninguno falla con un diagnóstico de tooling. No se considera éxito silencioso.
`--allow-empty` permite solicitar explícitamente exit status exitoso para
workspaces en los que una partición pueda no contener tests.

## 6. Construcción del target de test

### 6.1 Compilación completa antes de ejecutar

El runner debe resolver, comprobar, bajar y verificar todas las fuentes activas
antes de iniciar el primer test. Si existe cualquier error de compilación:

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
- Registro ordenado de tests y sus source ranges.
- Entradas ejecutables privadas.
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
helpers son declaraciones y los efectos comienzan dentro de cada `test`.

## 7. Modelo de ejecución

### 7.1 Aislamiento por test

Cada test obtiene:

- Un scope raíz nuevo.
- Estado de runtime y heap no observable desde otros tests.
- Captura separada de stdout y stderr del runtime Tondo.
- Presupuesto de recursos independiente.
- Un resultado independiente.

Una implementación puede reutilizar threads, allocators o procesos internos
solo si esa reutilización no permite observar valores, roots, tasks, handles,
buffers, pánicos ni output de otro test.

No existen variables globales mutables Tondo que sobrevivan entre entradas. Los
efectos externos —filesystem, procesos, red, reloj o servicios— no se revierten
mágicamente; un test debe aislarlos mediante paths, recursos y cleanup
explícitos.

### 7.2 Inicio y terminación

El runner invoca la entrada oculta y, si es async, conduce su scope raíz hasta
uno de estos terminales:

- Retorno normal.
- Error recuperable no manejado.
- Pánico.
- Límite de recursos.
- Timeout del runner.
- Fallo de infraestructura.

Antes de registrar un terminal de lenguaje, el runtime ejecuta cleanup y cancela
y espera hijos estructurados según la especificación base. Un test no se marca
como finalizado mientras quede cleanup estructurado pendiente.

### 7.3 Pánico y continuidad de la suite

Un pánico termina el test actual después del unwind, no el proceso completo de
la suite. El runner conserva el código `P`, ubicación y stack trace disponibles
y continúa con tests posteriores cuando el aislamiento sigue siendo válido.

Un abort fuera del modelo de pánico, corrupción del runtime o imposibilidad de
restablecer aislamiento se clasifica como fallo de infraestructura. El runner
puede detener la suite porque ya no puede garantizar resultados fiables.

### 7.4 Errores recuperables

Un valor que alcanza el canal `E` inferido hace fallar el test. El reporte
conserva como mínimo la identidad nominal visible de su tipo y la ubicación del
terminal. Su presentación humana sigue las reglas de la frontera de `main`: no
usa reflection para revelar campos privados ni promete serialización estable del
payload.

Para verificar un error esperado, el test lo consume localmente con `match`; no
deja que alcance al runner.

### 7.5 Inputs de host

El runner no proporciona parámetros mágicos al body. Los argumentos de proceso
del programa Tondo son vacíos.

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

### 7.6 Límites y timeouts

Todo test se ejecuta con límites finitos de instrucciones o trabajo, memoria,
profundidad y output. El toolchain publica sus defaults y registra los valores
efectivos o el hash de su resource profile en el reporte.

Un timeout wall-clock es un límite del runner, no un error Tondo ni un pánico. El
runner debe poder terminar la entrada aislada incluso si el código no llega a un
punto cooperativo de cancelación. No puede dejar un proceso o thread de usuario
ejecutándose después de reportar el test.

Los presupuestos estructurales y de runtime siempre permanecen finitos. El
timeout wall-clock puede desactivarse únicamente mediante `--timeout none`; esa
opción no desactiva ningún otro presupuesto. Un timeout o resource limit nunca
se convierte automáticamente en test ignorado.

Timeout y agotamiento de un presupuesto pueden impedir que el código Tondo
complete sus `defer`; no son terminales del lenguaje con garantía de unwind. El
runner sí debe limpiar su propia frontera de aislamiento y declarar el estado
correspondiente. Un test con efectos externos no puede confiar en teardown de
usuario después de una terminación forzada.

## 8. Selección, orden y paralelismo

### 8.1 Orden canónico

El registro y la presentación de resultados se ordenan lexicográficamente por la
identidad visible completa del test usando bytes UTF-8.

La ejecución por defecto utiliza `--jobs 1` y ese mismo orden. Esto proporciona
feedback reproducible sin depender del número de CPUs de la máquina.

### 8.2 Selección

El runner ofrece exactamente dos selectores básicos:

- `--filter text`: substring bytewise, case-sensitive, sobre la identidad
  visible completa.
- `--exact id`: igualdad bytewise con una identidad visible completa.

Son mutuamente excluyentes y cada uno aparece como máximo una vez. No hay regex,
glob ni interpretación locale-dependent.

`--list` compila y valida la suite, aplica el selector y emite las identidades
sin ejecutar bodies.

### 8.3 Paralelismo explícito

`--jobs N`, con `N > 0`, permite ejecutar hasta `N` tests simultáneamente. El
resultado final y la salida capturada continúan presentándose en orden canónico,
no en orden de finalización.

El runner no garantiza un orden de efectos externos entre tests paralelos. Una
suite que use `--jobs N` es responsable de que sus recursos de host no
colisionen. El aislamiento de runtime sí permanece obligatorio.

### 8.4 Sin dependencias entre tests

No existe sintaxis para ordenar tests, declarar dependencias ni compartir una
fixture mutable entre ellos. Cada test debe poder ejecutarse solo mediante
`--exact` y producir el mismo resultado de lenguaje que dentro de la suite con
los mismos inputs declarados.

### 8.5 Sin retries implícitos

El runner ejecuta cada test seleccionado exactamente una vez. No reintenta
fallos, no decide que un test es flaky y no convierte éxito después de retry en
verde.

Campañas que repitan un test deben solicitarlo fuera del resultado canónico,
registrar cada intento y no sustituir la regresión determinista.

## 9. Resultados, diagnósticos y salida

### 9.1 Estados

Cada test termina exactamente en uno de estos estados:

| Estado | Significado |
|---|---|
| `passed` | La entrada devolvió `Unit` normalmente. |
| `failed-error` | Un error recuperable alcanzó el runner. |
| `failed-panic` | Ocurrió un pánico Tondo después de unwind. |
| `resource-limit` | Se agotó un presupuesto configurado. |
| `timeout` | Venció el límite wall-clock del runner. |
| `infrastructure` | El harness, runtime o aislamiento dejó de ser fiable. |

No existe `ignored`, `expected-failure`, `flaky` ni `passed-after-retry` en el
contrato inicial.

### 9.2 `assert`

`assert(false)` conserva el pánico `P0007`. Dentro de un test, el runner lo
clasifica como `failed-panic`; no introduce un segundo código ni una excepción
recuperable.

La representación fuente de la condición, el mensaje, la ubicación y el stack
trace se conservan según la especificación base. Una librería puede construir
mensajes mejores, pero no alterar el terminal.

### 9.3 Captura de output

Stdout y stderr de cada entrada se capturan por separado como UTF-8. El modo
humano:

- Muestra siempre la identidad y el estado.
- Muestra output de tests fallidos.
- Oculta output de tests que pasan salvo `--show-output`.
- Nunca intercala bytes de dos tests.

El output de procesos hijos solo pertenece a la captura si el programa lo
redirige explícitamente a los streams Tondo. Heredar descriptores del runner no
constituye captura conforme.

El diagnóstico que el runner construye para error, pánico, timeout o
infraestructura pertenece a `failure`; no se añade artificialmente al
`stderr` capturado del programa.

### 9.4 Duración

Una implementación puede mostrar duración como metadato informativo. La duración
wall-clock no forma parte del resultado semántico, del orden ni del reporte
canónico reproducible.

### 9.5 Exit status

`tondo test` utiliza:

| Exit | Condición |
|---|---|
| `0` | Compilación correcta y todos los tests seleccionados `passed`; o selección vacía solicitada con `--allow-empty`. |
| `1` | Error de compilación, suite vacía no permitida o al menos un test no pasó. |
| `2` | Uso inválido de CLI. |
| `3` | Fallo interno del toolchain antes de producir un reporte fiable. |

Un test que llame a APIs de proceso no puede elegir el exit status del runner.
Un estado `infrastructure` que todavía permite un reporte íntegro usa exit `1`;
si el runner no puede garantizar ni serializar ese reporte, usa exit `3`.

## 10. Contrato de `tondo test`

Interfaz mínima:

~~~text
tondo test [--manifest <path>]
           [--filter <text> | --exact <test-id>]
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
- `--timeout` aplica por test, no a la suite completa.
- `--test-format human` es el default interactivo.
- `--test-format json` emite exactamente un reporte
  `tondo-test-report-0.2/1`, o una lista `tondo-test-list-0.2/1` con `--list`.
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
tests en runtime.

### 11.3 Setup y teardown

Setup compartido es una función normal:

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

Teardown utiliza `defer`. No existen `beforeEach`, `afterEach`, constructors de
suite ni orden implícito de fixtures.

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
`dividesIntegers`.

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

## 13. Características deliberadamente ausentes

El contrato inicial no incluye:

- Clases base de test.
- Decorators, annotations o atributos.
- Macros de assertions.
- Descubrimiento por reflection o prefijo de función.
- Registro de tests en runtime.
- Tests anidados o subtests con identidad propia.
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
| `E2001` | `test-outside-test-source` | Una declaración `test` aparece en una fuente `production`, script o forma no clasificada como test. |
| `E2002` | `duplicate-test` | Dos declaraciones producen la misma identidad de test dentro del mismo package/module overlay. |
| `E2003` | `invalid-test-source-declaration` | Una fuente de test intenta exportar API, alterar la unidad de producción sellada o ser consumida desde producción. |

El resto reutiliza diagnósticos existentes:

- Sintaxis de test inválida: `E0004`.
- `test` usado como identificador reservado: `E1005`.
- Colisión de helpers ordinarios: `E1002`.
- Identidad `_` u otra declaración semánticamente mal formada: `E1115`.
- Tipo final distinto de `Unit`: `E1102`.
- Error inferido sin `Discard`: `E1105`.
- Transferencia de control inválida: `E1205`.
- Error o `?` incompatible: `E1301`/`E1302`.
- Ownership, préstamos y cleanup: familia `E14xx`.
- Async y concurrencia: familia `E16xx`.
- Unsafe: familia `E17xx`.

Suite vacía, selector vacío, timeout e infraestructura son diagnósticos del
toolchain, no nuevos errores de compilación `E`.

## 15. Formato machine-readable

### 15.1 Forma canónica

`--test-format json` emite un único objeto JSON UTF-8, sin BOM ni bytes
posteriores salvo `LF`, por stdout. Los valores concretos siguientes son un
ejemplo; la forma y los tipos de los campos son normativos:

~~~json
{
  "format": "tondo-test-report-0.2/1",
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
  "tests": [
    {
      "id": "application::unit::math::addReturnsSum",
      "package": "application",
      "kind": "unit",
      "module": "math",
      "name": "addReturnsSum",
      "status": "passed",
      "failure": null,
      "stdout": "",
      "stderr": ""
    }
  ],
  "summary": {
    "selected": 1,
    "passed": 1,
    "failed_error": 0,
    "failed_panic": 0,
    "resource_limit": 0,
    "timeout": 0,
    "infrastructure": 0,
    "failed": 0
  }
}
~~~

`selection.kind` es `all`, `filter` o `exact`; `value` es `null` para `all` y el
texto exacto en los otros casos. `timeout_ms` es `null` solo para
`--timeout none`. El resource profile contiene todos los presupuestos finitos
del frontend, verifiers y runtime y se distribuye de forma recuperable por el
toolchain; el hash del reporte identifica exactamente sus bytes canónicos.

`tests` contiene únicamente entradas seleccionadas y ejecutadas. Todos sus
campos aparecen incluso cuando están vacíos o son `null`; `kind` es `unit` o
`integration`. No se admiten campos de extensión sin cambiar el identificador
de formato.

### 15.2 Orden y estabilidad

- `capabilities` se ordena por bytes.
- `tests` se ordena por `id`.
- Keys conocidas se serializan en el orden mostrado por el schema del
  toolchain.
- `stdout` y `stderr` contienen texto UTF-8 exacto.
- `summary.failed` cuenta todo estado distinto de `passed`.
- `summary.failed` es la suma exacta de los otros cinco contadores de fallo y
  `summary.selected` es `passed + failed`.
- Duración, timestamps, PID, número de CPU, paths físicos y direcciones no
  aparecen en la forma canónica.
- Paths de source dentro de failures son lógicos.

### 15.3 Failure

Cuando `status` es `passed`, `failure` es `null`. En otro estado contiene
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
      "function": "application::unit::math::addReturnsSum",
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

Campos privados y payloads opacos no se serializan por reflection.

### 15.4 Lista machine-readable

`--list --test-format json` emite `tondo-test-list-0.2/1`. Comparte `edition`,
`target`, `compiled` y `selection`, pero contiene descriptores sin estado,
failure ni output:

~~~json
{
  "format": "tondo-test-list-0.2/1",
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
  "tests": [
    {
      "id": "application::unit::math::addReturnsSum",
      "package": "application",
      "kind": "unit",
      "module": "math",
      "name": "addReturnsSum"
    }
  ]
}
~~~

Los descriptores se ordenan por `id`. `--allow-empty` permite un array vacío;
sin esa opción, una selección vacía conserva exit `1`.

### 15.5 Fallo de compilación

Si la compilación falla:

- `compiled` vale `false`.
- `tests` está vacío porque ningún body se ejecutó.
- En un reporte de ejecución, todos los contadores de `summary` valen `0`.
- Los diagnostics se emiten por stderr mediante el formato solicitado con
  `--diagnostic-format`.
- El reporte no copia diagnostics como strings.

## 16. Conformidad

Una implementación de esta extensión debe publicar una suite distinta a
`tondo-conformance-0.1`. La suite cubre como mínimo:

1. Token, CST lossless, parser y formatter de `test_decl`.
2. Rechazo de cada forma alternativa deliberadamente ausente.
3. Reserva de `test` como identificador.
4. Identidad y duplicados entre varios archivos.
5. Unit overlay con acceso privado.
6. Integration root sin acceso privado.
7. Separación exacta entre grafo de producción y dev-dependencies.
8. Producción inválida que no puede ser reparada por el overlay.
9. Retorno normal, error, pánico, assert, resource limit y timeout.
10. `defer`, terminal obligations y unwind en cada terminal.
11. Async, `scope`, `spawn`, cancelación y pánico de hijos.
12. Filtros, exact match, list, suite vacía y allow-empty.
13. Orden serial y presentación estable bajo `--jobs N`.
14. Captura separada de stdout/stderr.
15. Reporte JSON canónico y rechazo de schema inválido.
16. Targets y capabilities distintos.
17. Ausencia total de tests y dev-dependencies en productos de producción.
18. Ejecución individual mediante `--exact` equivalente a la misma entrada en
    suite bajo inputs idénticos.

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
tondo test --jobs 4
tondo test --test-format json
~~~

### Regla de diseño

> El lenguaje identifica y aísla tests; el código del test sigue siendo Tondo
> ordinario y la ergonomía adicional pertenece a `std.testing`.
