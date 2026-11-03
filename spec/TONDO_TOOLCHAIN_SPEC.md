# Tondo Toolchain 0.1

**Estado:** borrador normativo en desarrollo; Tondo todavía no se ha publicado

**Especificación de lenguaje:** [Tondo 0.1](./TONDO_LANGUAGE_SPEC.md)

**Especificación de testing:** [Testing Tondo 0.1](./TONDO_TESTING_SPEC.md)

Esta especificación define la frontera de proyecto del toolchain: layout
convencional, `tondo.toml`, dependencias, `tondo.lock`, caché y comandos. No
modifica la sintaxis ni la semántica de un archivo `.to`.

Las palabras **debe**, **no debe**, **puede** y **error** son normativas.

## 1. Objetivos

1. **Convención antes que configuración.** Un programa no necesita manifiesto;
   `tondo.toml` solo nombra el paquete y sus dependencias.
2. **Builds reproducibles.** `tondo.lock` fija cada dependencia descargada por
   commit o por SHA-256. Con el lockfile y la caché, compilar no usa la red.
3. **Análisis cerrado.** Una vez resueltas las dependencias, el compilador
   recibe un conjunto fijo de archivos y no consulta red, reloj, entorno ni
   directorio actual.
4. **Una sola forma.** Cada dependencia se declara con una tabla en línea; nada
   repite fuentes, módulos ni versiones.

## 2. Proyectos

Un proyecto es un directorio. Los comandos lo reciben como argumento o con
`--project <dir>`; sin ninguno usan el directorio actual. La forma recomendada:

~~~text
app/
  src/
    main.to
    models/user.to
  tests/
    user_test.to
  libs/units/              # una dependencia local, si la hay
  tondo.toml               # opcional
  tondo.lock               # generado cuando hay dependencias descargadas
~~~

Reglas de descubrimiento, deterministas:

1. Si existe `src/`, el paquete son sus archivos `.to`, recursivamente,
   excepto `*_test.to` y los de otro sistema que el del build: uno que
   termina en `_linux.to`, `_macos.to` o `_unix.to` es del paquete solo al
   compilar para Linux, para macOS o para cualquiera de los dos (6.8 del
   lenguaje).
2. Sin `src/`, el directorio debe contener `tondo.toml` o `main.to`, y el
   paquete son los `.to` de ese nivel, con las mismas excepciones.
3. Se ignoran symlinks, entradas ocultas y los directorios `target/` y
   `vendor/`.
4. Todos los archivos de un directorio forman un módulo. Su path es el nombre
   del paquete seguido de los directorios bajo la raíz de fuentes:
   `src/models/user.to` pertenece a `app.models`. Un directorio cuyo nombre no
   es un identificador no puede ser un módulo.
5. El archivo raíz es `main.to` de la raíz de fuentes o, sin él, el primero en
   orden de bytes UTF-8. Contiene `main` o el script del programa (6.6 y 20 de
   la especificación de lenguaje).
6. El nombre del paquete es `[package].name` o, sin él, el nombre del
   directorio, que entonces debe ser un identificador.

Un archivo `.to` suelto también es un programa: se compila sin manifiesto ni
dependencias y solo puede importar `std`.

## 3. `tondo.toml`

~~~toml
[package]
name = "app"
version = "1.2.0"
edition = "0.1"

[dependencies]
units = { path = "libs/units" }
http = { git = "https://github.com/acme/http", tag = "v1.2.0" }
zlib = { url = "https://example.com/zlib-1.0.tar.gz" }

[dev-dependencies]
fixtures = { path = "libs/fixtures" }

[native]
libraries = ["sqlite3", "static:z", "vendor/libpng.a", "vendor/libfoo.so"]

[derive]
Show = "derive/show.to"

[generators.protos]
program = "gen/protos.to"
inputs = ["proto/user.proto"]
outputs = ["proto.user"]
~~~

- `[package]` admite `name`, un identificador; `version`, un texto sin
  espacios, como `"1.2.0"`, que el programa da con `env.build()` (14.3.6 de
  la librería estándar) y que es `"0.0.0"` sin él; `edition`, cuyo único
  valor es `"0.1"`, el que se usa por defecto; y `systems`, los sistemas en
  que corre el paquete, `["linux"]`, `["macos"]` o los dos, que son los que
  admite sin ella (6.8 del lenguaje): compilarlo para otro es un error que
  los nombra, y `check` lo comprueba solo para ellos. `tondo new` escribe
  `version = "0.1.0"`.
- `[dependencies]` asocia a cada alias su fuente (sección 4).
- `[dev-dependencies]` declara del mismo modo las dependencias que solo usan
  las pruebas del proyecto (5.5 de la especificación de testing): los
  archivos `_test.to` y los de `tests/` las importan, el programa no, y
  importarlas desde él produce `E1008`. Un alias no puede estar en las dos
  tablas. Las dev-dependencies de una dependencia no se descargan, y un build
  del programa no carga los paquetes a los que solo llegan las pruebas.
- `[native]` admite `libraries`: las librerías de C que el ejecutable enlaza
  además de la del sistema, para las funciones que declaran sus `extern fn`
  (16.16 del lenguaje). El programa enlaza las de todos sus paquetes:
  - un nombre, `"sqlite3"`, se enlaza de forma dinámica desde las rutas de
    librerías del sistema (`-lsqlite3`);
  - `"static:z"` enlaza de forma estática `libz.a` de esas rutas;
  - un archivo, relativo al `tondo.toml`: uno `.a` se enlaza de forma
    estática, y uno `.so` o `.dylib` de forma dinámica, con su directorio en
    la ruta de búsqueda del ejecutable para que lo encuentre al arrancar.

  Una librería estática no arrastra las suyas: el proyecto lista también las
  que necesita.
- `[derive]` asocia a cada trait que declara el paquete el archivo de su
  provider, relativo al `tondo.toml` (27.3 del lenguaje). Solo el paquete que
  declara un trait puede dar su provider.
- `[generators.<nombre>]` declara un generador (27.5 del lenguaje): `program`,
  su archivo; `inputs`, los archivos que recibe como valores; `outputs`, los
  módulos que produce dentro del paquete (`"proto.user"`); y opcionalmente
  `roots`, las declaraciones públicas que puede consultar
  (`"models.User"`, el tipo `User` del módulo `app.models`), y `limits`, una tabla en línea con `steps`, `memory`
  (bytes) y `output` (bytes de fuente), cada uno un número positivo en un
  string: `limits = { steps = "1000000" }`. Los archivos son relativos al
  `tondo.toml`.
- Toda otra tabla o clave es error, igual que una clave fuera de tabla. El
  manifiesto de 0.1 no declara target, perfil de host ni capacidades: solo
  existe el perfil `hosted`, con todas ellas (20.11 del lenguaje).

El archivo usa el subconjunto de TOML de tablas, strings, enteros, tablas en
línea de strings y arrays en una línea.

## 4. Dependencias

### 4.1 Fuentes

Cada dependencia es una tabla en línea con exactamente una fuente:

| Fuente | Forma | Qué fija `tondo.lock` |
|---|---|---|
| Directorio local | `{ path = "libs/units" }` | Nada: se lee el directorio tal como está. |
| Repositorio git | `{ git = "<url>", tag = "v1.2.0" }`, o `branch = "main"`, o `rev = "<commit>"`; sin ninguno, la rama por defecto | El commit. |
| Archivo HTTP | `{ url = "<url>" }`, con `sha256 = "<hex>"` opcional | El SHA-256 del archivo. |

- `path` es relativo al directorio del `tondo.toml` que lo declara.
- `git` acepta cualquier URL que entienda `git`, incluidas las de GitHub y
  `file://`. `tag`, `branch` y `rev` se excluyen entre sí.
- `url` acepta `.tar`, `.tar.gz`, `.tgz`, `.tar.bz2`, `.tar.xz` y `.zip`. Si el
  archivo contiene un único directorio, ese directorio es el paquete; así
  funcionan los archivos de versión de GitHub
  (`https://github.com/acme/http/archive/refs/tags/v1.2.0.tar.gz`). Con
  `sha256`, el contenido descargado debe coincidir.

Una dependencia es un paquete con las reglas de la sección 2: su propio
`tondo.toml`, sus módulos y sus dependencias. Su `main` o su script no se
ejecutan; sus sentencias de nivel superior producen `E1804`.

### 4.2 Alias e imports

La clave de cada dependencia es su alias local: un identificador distinto de
`std` y del nombre del propio paquete. El primer segmento de un import nombra el
propio paquete, un alias o `std`:

~~~tondo
import app.models     // un módulo de este paquete
import http           // el módulo raíz de la dependencia `http`
import http.client    // el módulo `client` de esa dependencia
~~~

Otro primer segmento produce `E1008`. Solo las declaraciones `pub` de otro
paquete son visibles (7.5). Las dependencias de una dependencia no son
visibles: un paquete importa únicamente lo que declara.

### 4.3 Identidad y grafo

Cada paquete se identifica por su origen resuelto: el directorio local, el
commit o el SHA-256. Dos dependencias que resuelven al mismo origen son el mismo
paquete; dos orígenes distintos son paquetes distintos aunque compartan nombre,
y sus tipos son distintos (6.7). Las dependencias no pueden formar un ciclo.

## 5. `tondo.lock`

~~~toml
# Generated by tondo: the exact version of each fetched dependency.
# Commit it; `tondo update` changes it.
version = 1

[[package]]
source = "git+https://github.com/acme/http?tag=v1.2.0"
commit = "9fceb02d0ae598e95dc970b74767f19372d61af8"

[[package]]
source = "https://example.com/zlib-1.0.tar.gz"
sha256 = "5b833887a4dc1fa6504a356e56aa12cfd4cce098f03e153f8c09c3961d1e9380"
~~~

- Solo existe en la raíz del proyecto y registra todas las dependencias
  descargadas del grafo, también las transitivas. Las dependencias por `path`
  no se registran.
- `source` identifica la declaración: la URL de git con su referencia, o la URL
  del archivo. Las entradas se ordenan por `source`.
- Al compilar, una fuente presente en el lockfile usa exactamente su commit o su
  SHA-256, aunque el tag o la rama remotos hayan cambiado. Una fuente nueva o
  modificada en `tondo.toml` se resuelve y se añade, y las entradas que ya no se
  alcanzan se eliminan. El toolchain reescribe el archivo solo cuando cambia.
- `tondo update` resuelve de nuevo todas las dependencias; `tondo update http`
  solo la indicada.
- Si una descarga no coincide con el SHA-256 fijado o declarado, el build
  falla; nunca acepta otro contenido en silencio.

## 6. Caché y descarga

Las dependencias descargadas se guardan en la caché del usuario
(`$XDG_CACHE_HOME/tondo`, o `~/.cache/tondo`): los repositorios en `git/` y el
contenido de cada commit o archivo en `deps/`, nombrado por su commit o SHA-256.
El contenido de la caché no se modifica después de crearse, y cada entrada
aparece con un rename atómico.

La caché guarda también lo que un build puede volver a hacer: el runtime del
compilador, y los ejecutables de `run`, de las pruebas y de los providers y
generadores, en `run/`, `test/` y `meta/`, por el compilador y los archivos
que el build lee: sus fuentes y manifiestos, providers, generadores y sus
inputs, y los archivos que embebe (14.27 de la librería estándar). Usar su
runtime lo marca; al escribirlo, un compilador borra los de otras versiones
que ningún build ha usado en un día, así que dos versiones a la vez, como la
del editor y la de la terminal, no se quitan el runtime la una a la otra. Una
entrada de esos directorios que nadie usa en 30 días se borra, y de un mismo
programa, o proyecto en `test/`, compilado del mismo modo (debug, live,
estático o para otro target), un build nuevo deja solo el anterior más
reciente: editar un programa no llena la caché con cada versión suya.

Para descargar, el toolchain usa `git`, `curl`, `tar` y `unzip` del sistema.
Si falta una herramienta, el error lo dice. Con `tondo.lock` y la caché
completos no se ejecuta ninguna.

## 7. Comandos

~~~text
tondo run [<project>|<file.to>] [--link <library>]... [--static] [--debug | --live | --watch] [--target <target>] [--diagnostic-format human|json] [-- <argument>...]
tondo build [<project>|<file.to>] [--output <path>] [--link <library>]... [--static] [--strip] [--debug | --live] [--target <target>] [--diagnostic-format human|json] [--json]
tondo build --library [<project>|<file.to>] [--output <dir>] [--debug] [--json]
tondo check [<project>|<file.to>] [--library] [--json]
tondo expand [<project>|<file.to>] [--json]
tondo fmt [--check | --compact] [--json] <file.to>...
tondo api [<module>|<module>.<declaration>|<file.to>] [--private] [--list] [--project <dir>] [--json]
tondo api --search <palabras> [--project <dir>] [--json]
tondo graph [<project>|<dir>|<file.to>] [--packages | --files] [--module <module> [--reverse]] [--json | --dot | --mermaid]
tondo guide [<topic>] [--list] [--skill] [--json]
tondo update [<project>] [<dependency>...] [--json]
tondo new <name> [--json]
tondo init [<dir>] [--name <name>] [--json]
tondo add <alias> --path <dir> | --git <url> [--tag <tag> | --branch <branch> | --rev <commit>] | --url <url> [--sha256 <hash>] [--dev] [--project <dir>] [--json]
tondo remove <alias> [--project <dir>] [--json]
tondo fix [<project>|<file.to>] [--library] [--json]
tondo explain [<code>] [--json]
tondo clean [--all] [--json]
tondo bind [<header.h>...] [--package <name>] [--library <library>]... [--project <dir>] [--output <file.to>] [-I <dir>] [-D <name[=value]>] [--json]
tondo test [<project>] [--static] [--debug] [--library] [--target <target>] [--watch] [--coverage] [--bench [--bench-time <seconds>]] [--fuzz [--fuzz-time <seconds>]] [--test-format human|json|events] [--json] [--diagnostic-format human|json]
tondo doc-test [--edition 0.1] <markdown>
tondo target pack [<dir>] [--json]
tondo lsp
tondo mcp
tondo setup [--vsix <file>] [--json]
tondo ps [--json]
tondo counters <pid|name> [--interval <seconds>] [--count <n>] [--json]
tondo trace <pid|name> [--duration <seconds>] [--output <path>] [--json]
tondo trace [--duration <seconds>] [--output <path>] [--json] -- <program> [<argument>...]
tondo profile <pid|name> [--cpu | --memory | --tasks] [--duration <seconds>] [--output <path>] [--json]
tondo profile [--cpu | --memory | --tasks] [--duration <seconds>] [--output <path>] [--json] -- <program> [<argument>...]
tondo profile show <profile> [--top <n>] [--lines] [--sample <type>] [--html <path>] [--json]
tondo dump <pid|name> [--output <path>] [--json]
tondo dump read <file> [tasks | task <id> | heap | objects | object <address>] [--json]
tondo debug [--json] [--live] <file.to|project|executable|pid|name> [-- <argument>...]
tondo debug [--json] --test <id> [<project>]
tondo dap
tondo reload <pid|name> [--json]
tondo completions bash|zsh|fish
tondo help [<command>]
tondo version [--json]
~~~

- `run` compila, guarda el ejecutable en la caché y lo ejecuta; `build` escribe
  el ejecutable; `check` solo informa diagnósticos, en JSON con `--json`
  (sección 7.12 y 22.3 del lenguaje).
  `check` no pide `main`: un paquete de solo declaraciones, como una librería
  de la que otros dependen, se comprueba sin `E1806`, que dan `run` y `build`.
- `--link` enlaza una librería de C más, por nombre o archivo como en
  `[native] libraries`; un archivo suelto, que no tiene `tondo.toml`, la usa
  para sus `extern fn`.
- El toolchain enlaza el ejecutable él mismo, sin compilador de C ni paquetes
  de desarrollo: en Linux con las librerías compartidas que el sistema tiene
  para ejecutar programas, por el nombre con que se cargan (`libc.so.6`,
  `libsqlite3.so.0`), y en macOS con `libSystem`, los frameworks y las
  `.dylib` instaladas. El ejecutable es independiente de posición. Con la
  variable `TONDO_CC`, enlaza el compilador de C que nombra.
- `--static` da en Linux un ejecutable estático, con musl, que no carga
  librerías compartidas: las de `[native] libraries` y `--link` son
  estáticas, `"static:z"` o un archivo `.a`. En macOS es error.
- `--strip` deja fuera del ejecutable la tabla de símbolos. Las trazas de
  Tondo no la usan; los depuradores y profilers externos, sí.
- `--debug` compila un build de depuración para `tondo debug` (sección 7.6),
  y `--live` uno vivo, cuyo código cambia mientras corre; `--watch` lo
  cambia cuando cambian sus archivos (sección 7.10).
- `--library` compila, comprueba o prueba una librería para C, sin `main`
  (sección 7.7).
- `--target` compila para otra máquina, y `target pack` escribe lo que otra
  máquina necesita para compilar para esta (sección 7.8).
- `expand` muestra, con su formato canónico, el `impl` que dio el provider de
  cada `derive` (27.3 del lenguaje), y dónde está el `derive`.
- `fmt` aplica el formato canónico (21) a archivos sueltos; `--check` falla si
  alguno no lo cumple. `--compact` escribe en stdout, sin tocar el archivo, su
  forma compacta: los mismos tokens, comentarios incluidos, sin sangría, con
  espacios solo donde dos tokens se unirían y una línea por línea lógica, sin
  la que sigue a `{`. Es Tondo válido, para dar código a un modelo con unos
  15 % menos de tokens; `fmt` lo devuelve a la forma canónica.
- `api` muestra las declaraciones de un módulo con su documentación y sin
  cuerpos (sección 7.2).
- `graph` muestra de qué está hecho un programa: sus paquetes, sus módulos
  con sus archivos y lo que importa cada uno (sección 7.11).
- `explain` dice qué significa el código de un error, un warning o un pánico
  (sección 7.13); `new`, `init`, `add`, `remove`, `fix` y `clean` hacen un
  proyecto, cambian sus dependencias, aplican las reparaciones seguras y
  vacían la caché (sección 7.14).
- `guide` enseña Tondo a personas y a modelos (sección 7.3).
- `bind` escribe un módulo con las declaraciones de C de unos headers, de un
  paquete del sistema o de una librería (sección 7.1) y, en un proyecto, añade
  cada librería a `[native] libraries`.
- `test` sigue la [especificación de testing](./TONDO_TESTING_SPEC.md).
- `lsp` sirve a un editor (sección 7.4), y `mcp`, a un agente; `setup`
  prepara los agentes y los editores (sección 7.9).
- `ps`, `counters`, `trace` y `dump` observan los programas en ejecución
  (sección 7.5), y `profile` dice dónde gastan su tiempo y su memoria
  (sección 7.15); `debug` depura uno, y `dap` hace lo mismo para un editor
  (sección 7.6).
- `help` y `<comando> --help` muestran el uso de un comando, o de todos;
  `completions` escribe el script que completa los comandos y sus opciones en
  `bash`, `zsh` o `fish`; `version` dice la versión, el target y el backend.
- Con `--json`, un comando escribe su resultado como un objeto JSON, y
  `--diagnostic-format json` da los diagnósticos de lo que compila como JSON
  (sección 7.12).
- Estado de salida: `0` éxito; `1` diagnósticos de error o fallo de formato; `2`
  uso incorrecto, que muestra el uso del comando; `3` error del toolchain
  (sección 9). `run` devuelve el estado del programa, y `test`, `4` si se
  interrumpe (7.10 de la especificación de testing).

### 7.1 `tondo bind`

`tondo bind` expande los headers con el preprocesador de C del sistema
(`cc -E`, con los `-I` y `-D` dados) y escribe, en formato canónico, un módulo
del proyecto: `src/<nombre>/<nombre>.to` por el primer header, u `--output`.
El módulo declara, como `pub`, lo que declaran los headers y los de su
directorio que incluyen directamente:

| C | Tondo |
|---|---|
| función | `extern fn`, con `@symbol` si su símbolo difiere |
| `struct` | `extern type`, opaco si no tiene cuerpo |
| `union` | `@union extern type` |
| `struct` con bit fields | `extern type` con cada grupo de bit fields como sus bytes, y un método para leer y otro para escribir cada uno |
| `typedef` | `alias` |
| `enum` | `alias` de `ffi.CInt` y una constante por enumerador |
| macro de un número o un texto | `const` |
| macro de un puntero | `const` de valor `none` si es `NULL`; si no, una función que lo construye |
| macro con parámetros que llama a una función | una función `unsafe` que la llama, con los tipos de sus parámetros |
| `va_list` como parámetro | `Pointer[ffi.VaList]` |

Los nombres de C se conservan; uno que es palabra reservada o nombre del
prelude de Tondo recibe `_` al final. Un puntero es opcional, porque C puede
pasar `NULL`; `const char*` es `ffi.CString?`. Un tipo de otro header se
resuelve a lo que nombra, y un struct que solo se alcanza por puntero es
opaco. Lo que no tiene forma en Tondo, como `long double` o una macro que no es
un valor ni una llamada, se lista al final del módulo en un comentario.
El módulo se regenera con el mismo comando que muestra su primera línea.

Sin headers, `bind` los busca:

- `--package <name>` pregunta a `pkg-config` por los headers del paquete
  (`<name>.h` o `<name>/<name>.h` en su `includedir`), sus flags de C y sus
  librerías, que pasan a `[native] libraries`. Si `pkg-config` no lo conoce,
  usa `lib<name>` de las rutas de librerías del sistema como un `--library`.
- `--library <archivo>` lee la información de depuración (DWARF) de la
  librería, o de su `.dSYM` en macOS: las funciones que exporta, con los tipos
  que alcanzan, y las constantes de sus `enum`. No tiene macros, así que no hay
  otras constantes.
- Si nada de eso da declaraciones, es un error que pide el header.

### 7.2 `tondo api`

`tondo api` muestra lo que se puede llamar de un módulo sin leer su
implementación: sus declaraciones públicas, con los comentarios de
documentación y sin los cuerpos de funciones ni tests.

~~~text
$ tondo api std.csv
pub fn rows(input: io.Reader, separator: Char = ','): Rows
...
$ tondo api app.models
$ tondo api std.http.Client
$ tondo api --search base64
$ tondo api --list
~~~

- El argumento es un módulo de la stdlib (`std.http`, `std.fs`), `prelude`
  (los métodos de los tipos intrínsecos), un módulo del proyecto o de sus
  dependencias (`app.models`, `shapes`) o un archivo `.to`. Sin argumento,
  muestra todos los módulos del paquete, cada uno tras `// module nombre`.
  `--list` nombra los módulos disponibles.
- Un módulo de la stdlib empieza por lo que es y un ejemplo de su uso, un
  programa completo que un test compila, como cada ejemplo de los `///` de
  sus declaraciones; con `--json`, `summary` y `example`. Cada tipo público
  de la stdlib (record, enum, trait, alias o contexto) lleva en sus `///` un
  ejemplo así, y un test falla si a uno le falta.
- Con una declaración de un módulo, `std.http.Client`, `std.json.encode` o
  `String`, del preludio, muestra solo ella y lo que le pertenece: los
  métodos y los `impl` de un tipo, o un método por su nombre, `Client.get`.
  Un módulo de la stdlib se nombra también sin `std.`, como lo escriben el
  compilador y sus errores (`http.Client`), salvo que el proyecto tenga un
  módulo de ese nombre.
- `--search` busca las palabras en los nombres y la documentación de las
  declaraciones de la stdlib, el proyecto y sus dependencias, primero las que
  se llaman así, y da cada una con su módulo, su declaración y la primera
  frase de su documentación, y al final cómo ver una con su ejemplo; con
  `--json`, `matches`.
- El error de un nombre que un módulo no declara propone el parecido que
  declara, con su reparación, o nombra el `tondo api` del módulo; el de un
  método que no existe en un tipo del preludio o de la stdlib, sin uno
  parecido, nombra el `tondo api` del tipo.
- Un record con campos privados muestra los públicos y `// private fields`;
  un trait, las firmas de sus métodos; un `impl`, su cabecera. `--private`
  muestra también lo privado, para quien trabaja dentro del paquete.
- Un import con otro nombre se escribe con el del módulo: `io.Reader`, no
  `__io.Reader`.
- Los módulos de la stdlib que implementa el compilador tienen sus
  declaraciones escritas en `std/api` del toolchain.
- Escribe solo en stdout, no compila y no modifica nada; con un módulo que no
  existe es un error del toolchain (estado 3).

### 7.3 `tondo guide`

`tondo guide` es una guía de Tondo que viaja con el toolchain, para quien lo
aprende y para un modelo que lo escribe: el lenguaje, la stdlib y los
comandos, un tema cada vez para leer solo lo necesario.

- Sin argumento muestra la visión general: un programa, las reglas que más
  difieren de otros lenguajes, los comandos, cómo trabajar como agente
  (`tondo api`, `tondo check --json`, `tondo fmt --compact`) y la lista de
  temas. `tondo guide <tema>` muestra uno (`syntax`, `types`, `errors`,
  `collections`, `text`, `tasks`, `projects`, `testing`, `stdlib`, `http`,
  `templates`, `databases`, `meta`, `ffi`, `cli`, `mcp`, `pitfalls`) y
  `--list` los nombra.
- `cli` recorre cada comando de `tondo` y `mcp` cada herramienta de `tondo
  mcp`: un test comprueba que no falta ninguno.
- `pitfalls` reúne los errores que traen otros lenguajes, cada uno con el
  diagnóstico que produce y su arreglo.
- `--skill` da la visión general como skill de un agente, un `SKILL.md` con
  su cabecera: `tondo guide --skill > .claude/skills/tondo/SKILL.md`.
- Cada ejemplo de la guía compila con el toolchain que la lleva, y cada
  ejemplo de un error falla con el código que nombra: un test lo comprueba.

### 7.4 `tondo lsp`

`tondo lsp` es un servidor del Language Server Protocol 3.17 por la entrada y
la salida estándar, que el editor arranca. Analiza el proyecto de cada archivo
abierto como su build de tests, para incluir los archivos de tests, o el
archivo solo si no está en un proyecto, con el texto que el editor tiene aunque
no esté guardado. Tras un cambio, cuando la escritura se detiene, analiza de
nuevo y publica los diagnósticos de cada archivo, los mismos de `check`.

- Hover: el tipo de una variable o un parámetro, la cabecera de una función o
  un campo, la declaración de un tipo, y la documentación `///`; de la lectura
  de un contexto (`Trace.span`, 11.16 del lenguaje), además, los `with` de la
  función que le dan valor antes de ese punto, o que vale lo que hereda; de una
  anotación del programa (`@binding.Check(...)`, 27.13 del lenguaje), su
  declaración y su documentación; de lo que implementa el runtime, como
  `text.length()` o `console.println`, lo que muestra `tondo api`; de un
  módulo de la stdlib o su import, lo que es y su ejemplo.
- Sobre `fn`, `return`, `fail` o un `?`, cada lugar por el que sale la
  función: sus `return`, `fail` y `?`, no los de sus closures.
- Las apariciones en el archivo de lo que está bajo el cursor, y los símbolos
  del proyecto y de la stdlib cuyo nombre es lo que se busca, empieza por
  ello, lo contiene o tiene sus letras en orden (`hRq` para
  `handleRequest`), en ese orden, los de la stdlib con su módulo, en su
  copia de la caché.
- La firma de la llamada que se escribe, con su documentación y el argumento
  en que está el cursor.
- El tipo de cada variable y cada parámetro de un closure declarados sin
  escribirlo, el parámetro al que va cada argumento que no lo nombra ni es
  ya su nombre, y el tipo de cada línea de una cadena de llamadas escrita en
  varias (la que sigue empieza por `.`), como pistas en línea.
- Mientras el depurador está parado (`textDocument/inlineValue`), las
  variables y parámetros que usan las líneas de la función donde paró hasta
  esa línea, para que el editor muestre su valor junto a ellas.
- Definición y referencias de variables, parámetros (también en argumentos con
  nombre), funciones, métodos, tipos, campos, variantes, constantes, traits,
  anotaciones (también sus campos en los argumentos de un uso) y módulos; las
  de la librería estándar se abren desde una copia en la caché. La de la ruta
  de `embed.text` o `embed.bytes` es el archivo que lee.
- La definición del tipo de un binding, un campo o un parámetro; las
  implementaciones de un trait o de un tipo (sus `impl`) y de un método de un
  trait; quién llama a una función y a quién llama (call hierarchy); y los
  traits que implementa un tipo o los tipos que implementan un trait (type
  hierarchy).
- Renombrar una declaración del programa cambia cada referencia; un campo
  escrito con el nombre de una variable, `User { name }`, pasa a
  `User { fullName: name }`.
- Autocompletado: tras `valor.`, los campos y métodos que ese valor puede
  usar, también los que implementa el runtime: no las funciones de su tipo
  sin `self`, ni los campos `priv` o las funciones no públicas de otro
  módulo; tras `modulo.`, que el archivo importa, sus declaraciones públicas,
  las de un módulo estándar como las muestra `tondo api`; tras `Tipo.` o
  `modulo.Tipo.`, sus funciones sin `self`, como `Builder.new`, y las
  variantes de un enum; y si no, las variables y parámetros de la función
  hasta el cursor, las declaraciones del módulo, los módulos que usa, los que
  no importa todavía, con el `import` que los trae, y las palabras clave,
  las que abren una construcción como plantilla con sus partes por rellenar
  (`match`, `for`, `fn`...). Donde empieza un argumento, los nombres de los
  parámetros de la llamada que no se dieron todavía, `nombre: `; en las
  llaves de un record escrito por el nombre de su tipo, `User { ... }`, sus
  campos que faltan. Lo que declaran los módulos que el archivo no importa
  y empieza por lo escrito, de la stdlib y del proyecto, se escribe con su
  módulo (`uuid.Uuid` para `Uui`) y añade su `import`. Tras `valor.`, además, lo que lo convierte en una
  construcción alrededor: `match`, `let`, `some` y `ok`, `if` y `not` si es
  un `Bool`, y `for` si es una colección o un rango. Tras `let x: T = ` o
  `var x: T = `, o en un argumento de un parámetro de tipo `T`, lo que da un
  `T` va primero y preseleccionado, salvo la variable que se declara.
  Tras `@`, las anotaciones visibles, y dentro de sus paréntesis los campos
  que faltan, como los parámetros de una llamada. En la ruta de `std.embed`,
  lo que hay en el directorio escrito hasta el cursor.
  En un `import`, el siguiente segmento de los módulos que puede nombrar:
  `std`, el paquete y sus dependencias, y tras `std.` los módulos estándar,
  cada uno con lo que ofrece según el comentario que lo abre.
  Cada elemento lleva su declaración y su documentación; el de una función,
  sus parámetros y su resultado junto al nombre, y al elegirlo escribe sus
  paréntesis, salvo si los hay ya, y abre la ayuda de sus parámetros.
- Resaltado semántico de cada nombre según lo que declara (módulo, tipo, enum,
  trait, parámetro, variable, campo, variante, función, método), de las
  palabras clave, los textos, los números y los comentarios; del archivo
  entero o de las líneas de un rango. Una variable `var` o un parámetro
  `mut` o `var` llevan el modificador `mutable`, que la extensión de VS Code
  subraya.
- Enlaces (`textDocument/documentLink`): cada `import` al primer archivo de
  su módulo, y cada ruta de `std.embed` al archivo que lee.
- Los colores escritos como `Color.hex("#rrggbb")` (o `#rrggbbaa`) y
  `Color.rgb(r, g, b)` con literales, que el editor muestra y deja elegir;
  el elegido se escribe como estaba: el texto hexadecimal o los números.
- Hover de una palabra clave: qué hace, un uso y el tema de la guía. El de
  un `?` o un `fail` dice qué da y qué propaga ahí: el valor, y el error o
  `none` que devuelve de la función o el closure, con el error de la
  función si es otro; en `main` o un script, que el error termina el
  programa.
- Los símbolos de un archivo y su formato canónico (21 del lenguaje), entero o
  en las líneas de un rango; los bloques, corchetes de varias líneas, imports
  y comentarios que se pliegan; y la selección que crece del token a los
  corchetes que lo rodean, su línea y el archivo.
- Renombrar la carpeta de un módulo del proyecto
  (`workspace/willRenameFiles`) cambia los imports de él y de los módulos que
  contiene, y los nombres con que se usa.
- Al empezar, analiza los proyectos de las carpetas del editor, para que sus
  problemas se vean antes de abrir sus archivos, y le pide que vigile sus
  `.to`, `tondo.toml` y `tondo.lock`: lo que cambia en el disco, como tras un
  checkout, se analiza de nuevo. Lo que nada usa o nada alcanza (W1001,
  W1002, W1003, W1006, W1007) lleva la etiqueta `Unnecessary`, que el editor
  atenúa.
- El código de cada diagnóstico enlaza a `tondo-explain:<código>`, lo que
  `tondo explain` dice de él (sección 7.13), y sus `related` (22.3 del
  lenguaje) van como `relatedInformation`: la primera declaración de lo que
  se declara dos veces, campos y variantes incluidos; el binding que otro
  ocultaría; el método del trait que un `impl` no cumple o repite; el
  `impl` con que otro se solapa; el primer `derive` de uno repetido.
- Las reparaciones de los diagnósticos (22.4 del lenguaje), cada una como
  acción `quickfix`, preferida si es `safe`, y una acción `source.fixAll` que
  aplica las seguras del archivo que no se tocan entre sí. La acción
  `source.organizeImports` añade, en orden, los imports de los módulos que el
  archivo nombra sin importar y quita los que no usa; la extensión de VS
  Code la aplica al guardar. Entre las reparaciones están escribir el método
  que le falta a un `impl` (`E1114`), con la firma del trait para ese tipo y
  un `panic` que dice que falta, y los campos que le faltan a un literal
  (`E1102`) cuando cada uno tiene un valor del que parte uno nuevo: `0`,
  `0.0`, `false`, `""`, `none`, `[]` o `[:]`. Una variable o un parámetro
  sin usar (`W1002`, `W1003`) se renombra a `_nombre`, como decisión: a veces
  es un error que no conviene ocultar. Un `match` que no cubre todos
  los casos (`E1204`) ofrece, además del brazo del primero que falta, uno
  para cada caso que falta, hasta 32; y un nombre privado de otro módulo
  (`E1501`), declararlo `pub` en el archivo que lo declara, como decisión
  porque cambia lo que el módulo ofrece. Un nombre desconocido que declara
  un módulo estándar (`Uuid`, `encode`) dice cuál y se escribe con él,
  añadiendo su `import` si falta: segura si solo uno lo declara, y una
  decisión por módulo si son varios, primero los que el archivo importa.
- Refactors de una selección, cuando el editor los pide y no para la
  bombilla (`triggerKind` 2): extraer una expresión a una variable, con un
  `let` antes de su sentencia, salvo si ahí podría evaluarse cuando antes no
  (tras un `and` o un `or`, dentro de un bloque o un closure de la línea, en
  un brazo); y extraer líneas enteras de un bloque a una función, con las
  variables de fuera que leen como parámetros y las que se leen después como
  su resultado, que propaga con `?` el error de la función que las contenía.
  Una variable `var` de fuera, o un parámetro `mut` o `var`, pasa prestado
  con `mut` (`total: mut Int`, llamado con `mut total`); si las líneas usan
  `self`, la función nueva es un método del mismo tipo con el mismo
  receptor, llamado con `self.`. No se ofrece si las líneas salen del
  bloque (`return`, `break`, `continue`, `defer`). Sobre
  un `let` leído una sola vez, en la sentencia siguiente y no tras lo que
  pueda saltarlo o retrasarlo, escribir su valor allí y quitar el `let`.
  Cada refactor se ofrece solo si el programa, con él aplicado, no tiene un
  error que no tuviera.
- Sobre el nombre de una función de un proyecto, un test de ella en los
  tests del módulo (`archivo_test.to`, que crea si no existe, con
  `import std.testing`): la llama con el valor del que parte cada parámetro
  sin valor por defecto y termina en `testing.failNow`, que dice qué falta
  escribir. No se ofrece para una función genérica ni para un parámetro de
  un tipo sin tal valor.
- Code lenses *Run*, *Debug*, *Debug Live* y *Profile* sobre el `main` de un
  programa, o sobre la primera sentencia de un script, en el archivo que lo
  contiene, con las órdenes `tondo.run`, `tondo.debug`, `tondo.debugLive` y
  `tondo.profile` y la URI del archivo como argumento, que el editor
  implementa. Sobre cada función, método, tipo y trait de un archivo, salvo
  `main`, cuántas referencias tiene y, en un trait, cuántas
  implementaciones, con la orden `tondo.showReferences` y como argumentos la
  URI, la posición y las ubicaciones, que el editor muestra como sus
  referencias.
- Si el editor no lo pide, *Run test* o *Run suite* sobre cada test y cada
  suite, con la orden `tondo.runTest`, que el servidor ejecuta
  (`workspace/executeCommand`): corre `tondo test` con ese test, o los de la
  suite, aparte del servidor, y dice cómo fue con `window/showMessage`, el
  pánico o el error que lo hizo fallar, con lo que escribió en
  `window/logMessage`.
- Al escribir `}` (`textDocument/onTypeFormatting`), el bloque que cierra
  toma el formato canónico, si el archivo se lee; la extensión de VS Code
  lo activa con `editor.formatOnType`.
- Si el editor lo pide con `"tests": true` en `initializationOptions`, la
  notificación `tondo/tests` le da las suites y los tests de un proyecto cada
  vez que cambian al analizarlo, aunque no estén guardados:
  `{"root": <uri>, "tests": [{"id", "parent", "name", "suite", "bench", "fuzz", "location"}]}`,
  con los ids de la lista de `tondo test --list` (15.4 de testing), el
  `parent` como id o `null`, `bench` si el test es un benchmark (10.4 de
  testing) y `fuzz` si hace fuzzing (10.6 de testing). Los tests de los
  archivos de otro sistema llevan además `system`, su nombre (`"macOS"`):
  esta máquina no los ejecuta, y el editor los muestra sin ejecutarlos.
- Un proyecto con archivos de un sistema (6.8 del lenguaje) se analiza para
  cada sistema en que corre: los diagnósticos de los otros van con los de
  esta máquina, los de solo uno con su nombre delante (`on macOS:`), como en
  `check`. Un archivo de otro sistema se analiza como lo compila ese sistema,
  con su hover, su autocompletado y su navegación. La definición de una
  declaración de un archivo de sistema lleva a la de cada sistema, y su
  hover dice en qué archivo está la de cada uno.

Las posiciones cuentan bytes UTF-8 cuando el editor lo admite y unidades
UTF-16 si no.

### 7.5 Programas en ejecución

Un programa de Tondo que arranca con `TONDO_DIAGNOSTICS=1` atiende a las
herramientas que lo observan en un socket Unix, `$TMPDIR/tondo-<uid>/<pid>`,
dentro de un directorio que solo su usuario puede abrir. Lo sirve un hilo
propio del runtime, así que responde aunque sus workers estén ocupados o
bloqueados. Sin esa variable el programa no abre el socket ni cuenta nada para
las herramientas, así que no paga por ellas; `tondo trace -- <program>` y
`TONDO_DUMP` la dan por puesta. Un programa que termina borra su socket, y
`tondo ps` borra los de los que terminaron sin hacerlo.

Las órdenes que miran un programa en ejecución, `ps`, `counters`, `trace`,
`profile`, `dump`, `reload` y `debug`, llegan con `--via <command>` a uno de
otra máquina o de un contenedor: ejecutan `<command> tondo relay <name>`,
como `ssh host`, y hablan con el programa por la entrada y la salida de ese
`tondo relay` (sección 7.6); `ps` ejecuta allí `tondo ps`.

- `ps` lista los programas del usuario en ejecución: su proceso, cuánto llevan
  en marcha y su línea de órdenes; con `--json`, como `programs` de
  `tondo-ps-0.1/1` (sección 7.12).
- `counters` muestra cada intervalo, un segundo si no se da otro, los
  contadores de un programa, nombrado por su proceso o por el nombre de su
  archivo cuando un solo programa lo tiene: cada nivel con su valor y cada
  total con su ritmo por segundo desde la vez anterior. `--count` para tras
  esas veces. Con `--json` escribe un objeto por línea, `{"pid", "uptime_ms",
  "counters": [{"name", "unit", "value", "rate"}]}`, con `rate` solo en los
  totales y `unit` `bytes`, `us` (microsegundos) o vacía.

Los contadores del runtime, por grupos:

| Grupo | Contadores |
|---|---|
| `memory` | `held`, los bytes asignados ahora; `allocated`, bytes; `allocations`; `objects`, los strings y boxes vivos |
| `collector` | `collections`; `freed`, objetos de ciclos liberados; `paused`, el tiempo que los workers esperaron al colector; `longest`, la pausa más larga |
| `tasks` | `live`; `ready`, las que esperan un worker; `spawned` |
| `workers` | `total`; `idle`; `in_c`, los que están dentro de una llamada a C; `replaced`, los que tomaron el puesto de uno que tardaba en C; `c_calls` |
| `timers` | `pending` |
| `net` | `connections`; `read` y `written`, bytes de `std.net` |
| `http` | `client.requests`; `server.requests`; `server.active`, peticiones que se están atendiendo |
| `grpc` | `client.calls`, `client.active` y `client.failed`, las llamadas de los canales; `server.calls`, `server.active` y `server.failed`, las que sirven sus servicios, gRPC-Web y Connect incluidos; falla la que termina con un status que no es `Ok`, o la que su cliente deja antes |
| `sockets` | `tcp`, `listening`, `udp` y `unix` abiertos |
| `descriptors` | `files`, `pipes` y `total` abiertos |
| `processes` | `running` y `started`, los procesos que lanzó el programa |
| `os` | `cpu`, el tiempo de procesador; `resident`, la memoria en RAM; `threads` |

Los del programa (14.25 de la librería estándar) forman el grupo `program`.
Los sockets del propio canal no cuentan, ni la memoria de su hilo, así que
observar un programa no cambia lo que muestra.

`trace` graba lo que hace un programa en ejecución, nombrado como en
`counters`, o uno que arranca tras `--`: un ejecutable, o un archivo `.to` o
un proyecto, que compila como `run`. Un programa que arranca espera a la
herramienta antes de `main`, así que la traza lo cubre entero. Graba hasta
que el programa termina, pasa `--duration` o llega Ctrl-C, que termina
también el programa que arrancó, y escribe dos archivos, `<output>.json` y
`<output>.folded`, con `<programa>-<pid>` como `<output>` si no se da. Si el
programa que arrancó termina por una señal o con un estado distinto de 0, sin
que la traza lo terminara, `trace` lo dice:

- una línea de tiempo en Trace Event Format, que abre Perfetto: una pista por
  hilo, con un tramo por cada vez que una tarea corrió, nombrado por la
  función que la tarea ejecuta y con el lugar donde esperó al parar; los
  `spawn`, con su lugar; las llamadas a C de un milisegundo o más; los
  pánicos; y las pausas del colector, en una pista propia;
- un perfil de CPU en *folded stacks*, una línea por pila con cuántas muestras
  la tuvieron: el runtime mira dónde está el hilo que corre con cada
  milisegundo de procesador, tan a menudo como lo permite el reloj del
  sistema.

Mientras nada traza el programa, grabar no cuesta más que comprobarlo.

`dump` escribe un volcado de un programa en ejecución, nombrado como en
`counters`, en `<output>` o en `<programa>-<pid>.dump.json`: un JSON con sus
contadores, cada tarea con su estado (`waits`, `ready`, `runs` o `in C`), la
pila de donde está, dónde se lanzó y dónde tomó los locks que tiene, sus
contextos con los campos que no tienen su valor inicial, y su memoria viva.
Para ver dónde están las tareas que corren, los workers se paran un momento,
y el que no llega a un punto donde pararse da su pila por una señal. Con `TONDO_DUMP=<dir>`, un programa que acaba en un pánico, en una
señal fatal o en un deadlock deja su volcado allí antes de terminar. `dump`
nombra cada marco, así que su volcado se lee en otra máquina; uno al fallar
se nombra al leerlo, con el ejecutable del programa.

Con `TONDO_PANIC_FORMAT=json`, lo que termina un programa por un fallo se
escribe en la salida de error como una línea JSON, en lugar del informe de
texto (15.7 del lenguaje), para las herramientas y los agregadores de logs.
Su `format` es `tondo-panic-0.1/1`, y sus campos, en orden alfabético:

- `kind`: `panic`; `fault`, una señal fatal o un stack overflow; o `error`,
  el error con que termina `main` (20.10 del lenguaje).
- `code`, el código del pánico, `P0001` o uno propio, y `name`, el nombre
  estable de uno del lenguaje (`bounds`), del fallo (`stack-overflow`,
  `SIGSEGV`) o `null`; `error_type`, el tipo de un `error`, o `null`.
- `message`, el mensaje o el `Display` del error, y `data`, los datos del
  pánico (15.7 y 22.2 del lenguaje), cada uno con su tipo de JSON (un
  `Float` que no es finito, como su texto), o `null`.
- `source`, la ubicación (`file`, `line` y `column`), y `stack`, los frames
  como en el informe de un test que falla (15.3 de testing): `function`,
  `source`, `task`, que es `0` en la tarea del fallo y crece en cada tarea que
  la lanzó, y `times`.
- `recovered`: `true` en un pánico que el programa recuperó y que la
  biblioteca informa, como el de un handler de `std.http`.
- `context`, los campos `@log` del contexto de la tarea (11.16 del lenguaje)
  como textos, o `null` sin ninguno.

~~~json
{"code":"cache.corrupt","data":{"entries":12,"key":"users","ratio":0.75},"error_type":null,"format":"tondo-panic-0.1/1","kind":"panic","message":"the cache index is corrupt","name":null,"recovered":false,"source":{"column":9,"file":"src/cache.to","line":41},"stack":[{"function":"Cache.load","source":{"column":9,"file":"src/cache.to","line":41},"task":0,"times":1}]}
~~~

Sin la variable, o con otro valor, el informe es el de texto. El volcado de
`TONDO_DUMP` describe su pánico con el mismo objeto.

`dump read` muestra un volcado: sin más, un resumen, con por qué se tomó, la
memoria, las tareas por estado y dónde esperan, y dónde se asignó la memoria
viva; `tasks`, cada tarea con los marcos del programa; `task <id>`, una con
todos sus marcos; `heap`, la memoria viva por sitio y tipo; y, en el
volcado de un build de depuración, `objects`, cada objeto vivo por tipo, y
`object <address>` uno: su tipo, su tamaño, dónde se hizo, su texto si es un
string, los objetos que apuntan a él y la cadena más corta que lo mantiene
vivo desde la pila de una tarea. Con `--json`, la misma vista como JSON, en
`tondo-dump-read-0.1/1` (sección 7.12).

La memoria viva sale de muestras que el runtime toma siempre, como Go: una
asignación por cada 512 KiB de media, a intervalos al azar, con la pila que
la hizo, mientras vive. Una muestra de `n` bytes cuenta por
`n / (1 − e^(−n / 512 KiB))`, así que sus cifras son estimaciones; el sitio
es el código del programa que asignó, pasando por la librería estándar, y el
tipo, el del valor que hacía la sentencia o la colección que cambiaba, que el
compilador anota junto a cada llamada sin coste al ejecutar.

### 7.6 Depurador

`--debug`, en `run`, `build` y `test`, compila un build de depuración: sin
optimizar, con una sonda donde empieza cada línea y lo que el depurador
necesita para leer los bindings de cada función. Un build sin `--debug` no
tiene nada de eso, ni su coste.

`tondo debug` depura un build de depuración: compila con `--debug` un archivo
`.to` o un proyecto, o arranca un ejecutable, y el programa espera antes de
`main` a que se le diga que siga; o se adjunta a uno en ejecución, nombrado
como en `counters`, que arrancó con `TONDO_DIAGNOSTICS=1`. Con `--via
<command>` se adjunta a uno de otra máquina o de un contenedor: ejecuta
`<command> tondo relay <name>`, como `ssh host` o `docker exec -i caja`, y
`tondo relay` une su entrada y su salida a la conexión del programa allí,
sin abrir puertos; el acceso es el del comando. `--test <id>` depura
un test del proyecto, solo, con sus snapshots y las entradas de su fuzz store
(10.6 de testing): dice cómo terminó en lugar de los frames del runner; con
`--schedule <seed>`, sus tareas corren en el orden de esa seed (8.10 de
testing), la que nombra un fallo de `tondo test --schedules`, y el editor
la da como `"schedule"` en `launch`. Lee una orden por
línea, de la terminal o de su entrada, y con `--json` escribe cada respuesta
y cada evento como una línea de JSON, para una herramienta o un modelo:

- `break <file>:<line> [if <condición>] [hits <count>] [log <message>]`
  para donde empieza esa línea, o la siguiente con código del archivo;
  `break fn <nombre> ...`, donde empieza cada función con ese nombre, entero
  (`Stack.push`) o el suyo (`push`), y una genérica en cada uno de sus tipos.
  Con `task <id>` tras el lugar, el breakpoint es solo de esa tarea.
  `break on spawn` para cada tarea que empieza en su primera línea, con
  dónde se lanzó; `break on send` y `break on receive`, una tarea que va a
  enviar o recibir por un canal, con lo que tiene; y `break on wait <ms>`,
  una que esperó más de esos milisegundos un lock, un canal u otra tarea,
  al acabar la espera. `delete <id>` los quita como a los demás.
  La condición es una expresión que da un `Bool`; si no lo da, o falla, no se
  cumple. `hits` cuenta las veces que se cumple y para en la
  `n`-ésima, desde ella (`>= n`), tras ella (`> n`) o cada `n` (`% n`). Con
  `log`, no para: escribe el mensaje, cada `{expresión}` cambiada por su
  valor, y sigue. Con `capture` al final, tampoco para: da la pila y los
  bindings de ese punto como `captured`, el estado de un servidor en marcha
  sin detenerlo; `hits 1` lo toma solo la primera vez. La condición y el
  mensaje leen lo mismo que `locals`:
  también lo de las funciones en que está escrito el código de esa línea; un
  nombre que no es un binding de ellas, o una expresión mal escrita, es un
  error al poner el breakpoint. En un build live, `tondo` compila la
  condición con un parche, como `call`, y la tarea la llama en esa línea en
  lugar de leerla, así que puede llamar al código del programa
  (`if isLate(order)`) y la respuesta dice `compiled`. La que el depurador no
  sabe leer y no compila es un error al poner el breakpoint, con el del
  compilador; la que sabe leer y no compila, la lee. Un `reload` que cambia
  el código de esa línea la compila de nuevo para él. `delete <id>` lo quita.
- `retainers <path>` dice qué mantiene vivo el objeto del heap que tiene un
  binding, o una parte suya: las pilas de las tareas que apuntan a él y las
  cadenas de objetos más cortas desde una pila hasta él, cada objeto con su
  tamaño y dónde se hizo. `memory` da los objetos del programa por dónde se
  hicieron, los lugares con más bytes primero, y cuánto creció cada uno
  desde el último `memory`: entre dos paradas, lo que se acumula.
- `freeze <task>` deja una tarea retenida, o la retiene en su siguiente
  línea si corre, mientras las demás siguen; `thaw <task>` la deja seguir.
- Las expresiones de las condiciones, de los mensajes, de `print` y de
  `watch` leen bindings y sus partes, con índices y claves que también son
  expresiones (`items[i + 1].name`), literales (números, textos, caracteres,
  `true`, `false`, `none` y variantes como `Role.Admin`), aritmética,
  comparaciones, `and`, `or` y `not`, con la precedencia del lenguaje, y los
  métodos `length()`, `isEmpty()`, `isSome()`, `isNone()`, `isOk()`,
  `isErr()`, `contains()`, `startsWith()` y `endsWith()`. Leen los valores
  como el depurador los muestra y nunca ejecutan código del programa, así que
  no cambian nada. Un `Int` y un `Float` se operan como `Float`; un valor que
  no es un número, un `Bool`, un texto ni un carácter es igual a otro si se
  muestran igual, y operar dos de esos tipos distintos es un error.
- `watch <expresión> [if <condición>] [hits <count>]` vigila su valor en el
  marco que se mira: la tarea para en la línea siguiente a la que lo cambió,
  en ese marco o en una llamada suya, con la expresión, sus dos valores y
  esa línea como motivo. Acaba cuando el marco vuelve; `delete <id>` la
  quita antes. Con `if`, para solo en los cambios tras los que la condición
  se cumple, y `hits` cuenta esos cambios como en un breakpoint. La de un
  binding que tiene una celda de `std.sync` (`Mutex`, `RwLock`, `Atomic`), o
  una parte de su valor, la vigila desde todas las tareas y más allá de su
  marco: para la tarea que la cambió, en su siguiente línea, y lee la celda
  mientras nadie la tiene para cambiarla.
- Una tarea que entra en pánico para donde el código del programa lo produjo,
  antes de deshacer su pila, con el texto del pánico como motivo. Con
  `catch error`, también para donde empieza un error: un `fail` del código
  del programa, o un `?` que propaga uno de una función de la librería
  estándar; un `?` que propaga el de una función del programa no vuelve a
  parar. `catch` toma `panic`, `error`, los dos o `none`; por defecto,
  `panic`. Tras `panic`, códigos o nombres de pánicos (`P0001`, `bounds`), y
  tras `error`, tipos de errores, con su módulo o sin él (`ConfigError`,
  `json.DecodeError`), lo limitan a esos; de una unión cuenta el miembro que
  es el error. Un deadlock (`P0013`) para el programa antes de terminarlo,
  donde espera la tarea principal, para ver dónde espera cada una; al
  seguir, termina con su pánico. Cuando locks o tareas que esperan a otras
  forman un ciclo, el motivo lo nombra, y también el informe del pánico sin
  depurador: qué tarea espera qué lock que otra tiene, y dónde lo tomó.
- `continue` sigue hasta un breakpoint; `until <línea>` o `until
  <file>:<line>`, hasta esa línea, o la parada que llegue antes; `step`, hasta la siguiente línea,
  entrando en las llamadas; `next`, hasta la siguiente línea de la misma
  función o de una que la llamó; `finish`, hasta una línea de una que la
  llamó. `step into <nombre>` entra en la llamada de ese nombre que hace la
  línea (`plus`, `items.map`), pasando por encima de las demás, o sigue a la
  siguiente línea si no la hace. `pause`, o Ctrl-C, para el programa. Cada sentencia empieza una
  línea donde se para, también una que no calcula nada, como
  `let ready = true`. Los pasos pasan por encima del código que no está en
  los archivos del programa, el de la librería estándar y el derivado, salvo
  uno que empieza en él; `steps all` los hace entrar y `steps program`
  vuelve a lo de antes. Un paso que sale de una función dice lo que devolvió
  (`returned`), que `locals` muestra también en el marco donde paró.
- `record [<lines>]` guarda desde entonces cada línea del programa que
  ejecuta cada tarea, con su pila y los bindings que su código puede usar
  como estaban al empezarla, mostrados y recortados, en un historial de las
  últimas `lines` (10000); `record off` lo deja. Con el programa parado,
  `back [<n>]` va `n` líneas atrás en el historial de la tarea parada,
  `reverse` a la última anterior con un breakpoint, y `forward [<n>]`, o un
  paso, hacia delante; pasada la última, al presente. `back until
  <condición>` y `forward until <condición>` van a la línea más cercana en
  que la condición se cumplía, leída con los bindings de entonces. Mientras muestra el
  pasado, `locals`, `print`, `children` y `stack` lo leen, de su marco más
  interior, y `continue` sigue desde el presente. `why <expresión>` va a la
  línea del historial que dio a la expresión el valor que tiene: la última
  que empezó con otro valor, y dice cuál; y de cada binding que esa línea
  lee, según su texto, el valor que tenía y la línea de donde le vino
  (`causes`), para seguir preguntando por ellos. `timeline <expresión>` da
  cada valor que tomó la expresión en la tarea parada, con la línea que se
  lo dio, y `ran <archivo>:<línea> [<expresión>]` cada vez que una tarea
  empezó esa línea, con el valor de la expresión entonces; las 200 más
  recientes, y cuántas hubo.
- Cuando una tarea para, las demás paran en su siguiente línea, y todas
  siguen juntas. `tasks` dice dónde está cada una, también las que esperan,
  en un canal, un lock o un `sleep`, o están listas para seguir: `task` y
  `stack` muestran sus marcos y sus bindings como los de una parada. De
  una que espera, `for` dice qué: un lock de `std.sync` y la tarea que lo
  tiene, con dónde lo tomó; un valor de un canal o sitio en él, con lo que
  tiene; o el fin de otra tarea.
- `locals` muestra los bindings que tienen un valor en la función donde paró
  la tarea, lo que un closure capturó entre ellos, y después los de las
  funciones en que está escrita cuyos marcos siguen en su pila, cada uno con
  la función de la que es (`from`): las de un closure, la que lo hizo; las de
  un test, sus suites. Así se ve todo lo que el código de ese bloque puede
  usar. Un tipo que implementa `debug.View` (14.42 de la librería estándar)
  se muestra como su vista, y su valor tal cual es `x.raw()`. Un `Ref` y una celda de `std.sync` muestran lo que tienen como un
  record con `value`, que es lo que lee el programa: `Ref { value: 3 }`.
  `print <path>` muestra uno, o una parte suya por su ruta: `user.name`,
  `items[2]`, `scores["x"]`, `pair.0`. Los textos van entre comillas, los
  registros con sus campos, los enums con su variante y las colecciones con
  sus elementos; `children <path> [<start> <count>]` da las partes de un
  valor, cada una con su ruta, todas o las `count` desde la `start`-ésima, y
  cuántas son. Un valor se lee entero, sea cual sea su tamaño; se muestra
  recortado. `print <expresión>` da también el valor de una expresión, con
  su tipo, y la ruta de lo que nombra si es un binding o una parte suya. Un
  binding cuyo valor se movió o se liberó no se muestra.
- `snapshot` da en una respuesta por qué y dónde paró la tarea, cada marco
  de su pila con sus bindings, dónde se lanzó, las demás tareas y las
  últimas líneas de su historial: lo que una herramienta o un modelo
  pediría en varias órdenes. `locals` marca `changed` en un binding cuyo
  valor cambió desde la última vez que mostró ese marco.
- `stack [<task>]` da las funciones de una tarea parada, de dentro afuera,
  y después, en `spawned`, dónde se lanzó la tarea y dónde se lanzó la que
  la lanzó;
  `frame <n>` elige la `n`-ésima para `locals` y `print`, y `task <id>` otra
  tarea parada. Las funciones de fuera muestran sus bindings como estaban al
  empezar la línea que llamó.
- `call <expresión>`, en un build live, da el valor, con su `Display`, de
  una expresión de Tondo que puede llamar al código del programa: un parche
  añade una función que lee los bindings que nombra del marco que se mira y
  la tarea parada la llama en su pila, sin parar en sus líneas. Un error de
  la expresión es el del compilador; una llamada que espera a otra tarea
  parada sigue esperando, y la respuesta lo dice a los 10 segundos. Los
  watches y los logpoints nunca la usan; las condiciones, como dice `break`.
- `goto <línea>`, en un build live, hace que el marco donde paró la tarea
  siga en la sentencia de esa línea de su función, sin ejecutar las de en
  medio, cuando lo que esa sentencia usa ya tiene valor: si no, un error
  dice qué binding no lo tiene. Se muestra ahí al momento y va ahí al
  seguir.
- `set <destino> [=] <expresión>` cambia un binding de la función donde paró
  la tarea, o una parte suya (`user.age`, `items[i]`, `pair.0`), al valor de
  la expresión: un número, `Bool`, `Char`, `Byte`, `String` o la variante de
  un enum sin datos (`Role.Admin`), dentro de records, tuplas y arrays. Un
  array que otro valor comparte se copia antes, como hace el programa, si
  sus elementos no tienen nada contado; si no, el cambio es un error. Un
  texto sin comillas que no es una expresión es el texto mismo.
- `context` muestra los contextos de la tarea parada (11.16 del lenguaje), con
  sus campos como `locals`, y para cada campo que no tiene su valor inicial,
  el `with` que se lo dio: el archivo, la línea y la función, o que lo heredó
  de la tarea que la lanzó.
- `quit` termina el programa que arrancó; al acabar la entrada, el programa
  sigue sin el depurador.

`tondo dap` habla el Debug Adapter Protocol por su entrada y su salida, para
un editor: `launch` compila y arranca un programa como `tondo debug`, o lo
arranca si es un ejecutable, y `attach` se adjunta a uno por su proceso o su
nombre, también con `"via"` como `--via`. Con `"test"`, el id de un test del proyecto que `tondo test --list`
da, `launch` compila los tests del proyecto como un build de depuración y
ejecuta ese test solo; la consola dice cómo terminó. Con `"dump"`, un
volcado (sección 7.5), `launch` lo abre sin programa: sus tareas como hilos,
sus pilas, el estado de cada una y la memoria por dónde se creó. Los
breakpoints, con su condición, sus hits y sus logpoints, los de función
(`setFunctionBreakpoints`), los de datos, que son watches de una variable o
una parte suya (`dataBreakpointInfo` y `setDataBreakpoints`), los pasos, la
pausa, las tareas (`threads`), también las que esperan, la pila de cada una
y, tras una etiqueta, dónde se lanzó, la de un test con sus
suites, con los bindings de cada función, desplegables por partes
(`variables`), los elementos de un array o un set por páginas
(`indexedVariables`), en cada marco un ámbito por cada función en que está
escrita, como la suite de un test, y un ámbito `Context` con los contextos de
la tarea, `evaluate` de una expresión, y en la consola, en un build live, de una que
llama al código del programa, como `call`, `stepInTargets` con las llamadas de la línea,
`gotoTargets` y `goto` en un build live, las órdenes que solo miran
(`memory`, `retainers`, `snapshot`, `tasks`, `freeze`, `thaw`, `record`,
`context`) escritas en la consola,
`completions` en la consola (los bindings, los
campos de un valor tras su punto y los métodos de las expresiones),
`setVariable` y `setExpression`, de una parte también, y las excepciones de un
pánico y de un error (los filtros `panic` y `error`, cuya condición nombra
códigos o tipos, con `exceptionInfo`), y los eventos como filtros (`spawn`,
`send`, `receive` y `wait`, cuya condición son los milisegundos) son
las órdenes de `tondo debug`; lo que devolvió una función al salir de ella
va en el ámbito de los bindings, y `"justMyCode": false` en `launch` hace
que los pasos entren en el código de la librería estándar; con `"record"`
(`true` o cuántas líneas) guarda el historial, por el que van `stepBack` y
`reverseContinue`; la salida del programa y los logpoints van a la
consola del editor. La petición `table`, propia de Tondo, da una página de
las partes de un valor, como un array o un map de records, por su
`variablesReference`: una fila por parte, con su nombre, y una columna por
campo, o una sola, `value`, si las partes no tienen campos, con cuántas hay
y la parada de la que son; la página siguiente de otra parada es un error.
La extensión de VS Code la muestra con *Show as Table* en el menú de una
variable, en una tabla que se ordena por columnas y se filtra. Un
breakpoint en un archivo de otro sistema (6.8 del
lenguaje) queda sin verificar, con el motivo: el programa corre en el de
esta máquina, que no compila ese archivo.

`tondo run --record <dir>` ejecuta un build de depuración del programa y lo
graba en el directorio `<dir>`: sus tareas corren en un solo thread, en el
orden de una seed (8.10 de testing), y el runtime escribe lo que dio cada
llamada a la librería de C que hizo ese thread (el reloj, los bytes
aleatorios, los archivos, la entrada estándar, el sistema) y en qué turno
del planificador llegó cada señal. El directorio guarda además el
ejecutable, sus argumentos, su entorno y el directorio en que corrió.
`tondo debug --replay <dir>` lo ejecuta otra vez bajo el depurador: cada
llamada da lo que dio entonces sin hacerse, salvo escribir en la salida
estándar y de error, que se ve, y cada señal llega en el mismo turno. La
ejecución es la misma cada vez, con sus horas y sus datos, y no toca el
mundo: un fallo que pasó una vez se depura con todas las órdenes de esta
sección. El editor lo hace con `"replay"` en `launch`, y el servidor MCP con
`record` en `run` y `replay` en `debug_start`. Solo en Linux. Mientras se
graba, una lectura de la entrada estándar o de un archivo detiene las demás
tareas. Lo que corre en threads propios no se graba: la red y HTTP
(`std.net`, `std.http` y los módulos que los usan), los procesos
(`std.process`), `fs.watch` y las funciones de C que llaman al programa desde
sus threads. La reproducción para donde el programa empieza uno de esos
threads, o donde hace una llamada distinta de la grabada, y dice por qué. El
directorio tiene el entorno del programa y lo que leyó, y se guarda como esos
datos.

En una reproducción, `back [<n>]`, `reverse` y `why <expresión>` vuelven al
programa entero como estaba, no a un historial: otra reproducción corre
hasta donde está el programa, y guarda dónde empiezan las líneas de la tarea
parada, dónde habrían parado los breakpoints o el historial, y una tercera
corre hasta la línea que eso da, `n` líneas atrás, la última parada de un
breakpoint o la línea que dio a la expresión su valor, y toma el relevo, con
los breakpoints y los ajustes de la sesión. Cada parada de la reproducción
guarda un checkpoint, una copia del proceso hecha con `fork` donde paró, y
las reproducciones que vuelven atrás empiezan en el más cercano anterior, o
en otro anterior si ahí no encuentran lo que buscan, en lugar del principio;
un breakpoint que cuenta sus hits hace empezar desde el principio, donde
empezó a contarlos. En el editor, `stepBack` y `reverseContinue` de una
reproducción son `back` y `reverse`. Allí `timeline` y `ran` miran la
ejecución entera hasta donde está el programa, con otra reproducción desde el
principio que guarda el historial, y dan el probe de cada línea: `seek
<probe>` lleva el programa a ese punto, antes o después del de ahora.

### 7.7 Librerías para C

`tondo build --library` compila un proyecto, o un archivo, como una librería
que un programa de C enlaza o carga: escribe en el directorio de `--output`, o
en el actual, la librería compartida `lib<nombre>.so` (`lib<nombre>.dylib` en
macOS), la estática `lib<nombre>.a` y su header `<nombre>.h`, con el nombre
del paquete o del archivo. La librería no tiene `main` ni script; publica las `pub extern fn`
con cuerpo de su paquete (16.16 del lenguaje), y sin ninguna es `E1807`.
`check --library` y `test --library` la comprueban y la prueban sin pedir
`main`.

El header declara, con su documentación `///`, las funciones publicadas y los
`extern type` que alcanzan sus firmas: los alias de `std.ffi` con su nombre de
C (`int`, `size_t`, `char*` para `ffi.CString`), los números de Tondo con los
de `stdint.h`, `Bool` como `bool`, un `Pointer[T]` como `T*`, un
`extern fn(...)` como puntero a función y un `extern type` opaco como
`typedef struct Nombre Nombre`. El build imprime cómo enlazar cada una, líneas
que también lleva el header: la compartida con su directorio en la ruta de
búsqueda del programa, y la estática con las librerías del sistema que el
runtime usa y las de `[native] libraries`.

El runtime arranca con la primera llamada, sin función de inicio, y cada
llamada corre en el thread que llama (16.16). Las tareas que lanza siguen tras
volver la llamada. Lo que la librería escribe en la salida estándar se entrega
al terminar cada llamada, y al terminar el proceso. Un pánico termina el
proceso, como en un programa; un fallo de memoria, una instrucción ilegal o un
aborto fuera de sus tareas y de su código va al handler que el proceso tenía.
Cada librería contiene el runtime. La compartida solo publica sus funciones,
así que un proceso carga varias de Tondo, cada una con el suyo, y no se
descarga, porque sus threads siguen ejecutando su código; de la estática, un
programa enlaza una sola. `--static` y `--link` construyen ejecutables: con
`--library` son error.

### 7.8 Targets

`--target <target>`, en `build`, `run` y `test`, compila para Linux en
x86-64, `x86_64-linux`, o en arm64, `aarch64-linux`, desde cualquier sistema
donde corre `tondo`. El ejecutable es estático, con musl, como con `--static`:
no carga librerías del sistema, y las de `[native] libraries` y `--link` son
estáticas.

El runtime del target de su propia máquina viene con `tondo`; el de otro, en
un pack: el directorio `<dir>/<target>` que escribe `tondo target pack [<dir>]`
de un `tondo` para ese target, con el runtime, la librería de C, el unwinder
y el archivo de inicio de musl, y la identidad del runtime. `TONDO_TARGETS`
nombra los directorios donde están los packs, separados como en `PATH`. Sin
pack es error, y uno de otra versión de `tondo` también: cada uno dice cómo
escribirlo.

`run` y `test` ejecutan un programa de otra arquitectura bajo la emulación de
usuario de qemu: `qemu-<arch>` o `qemu-<arch>-static` de `PATH`, o el que el
kernel tiene registrado en `binfmt_misc`. Sin ninguno es error, que pide
instalar qemu-user; en un sistema que no es Linux, también: el programa
corre en Linux.

### 7.9 `tondo mcp`

`tondo mcp` es un servidor del Model Context Protocol por la entrada y la
salida estándar, un mensaje JSON-RPC por línea, que un agente arranca. Habla
las dos eras del protocolo: la de 2026-07-28, sin handshake, en la que cada
petición dice su versión en `_meta` y `server/discover` da las versiones, las
capacidades y la identidad del servidor; y la de `initialize`, de 2025-11-25 y
anteriores. Una versión que no conoce es el error `-32022`, con las que sí.

Sus herramientas hacen lo que los comandos de `tondo` y el servidor de
lenguaje, cada una con un texto para el modelo y su resultado como datos
(`structuredContent`):

- Código: `check` da los diagnósticos, cada uno con el id de `check --json`
  (22.3) y sus reparaciones; `fix` aplica las seguras, o las que se eligen por
  id, a los archivos del programa, y comprueba de nuevo. `format`, `compact`,
  `expand`, `api` (también de una declaración, y `search`), `graph`, `guide`
  y `explain`.
- Navegación: `definition`, `references`, `implementations`, `calls`,
  `hover`, `complete`, `symbols`, y `rename`, que reescribe los archivos, con
  lo mismo que el servidor de lenguaje da de cada declaración, las anotaciones
  incluidas.
- Proyectos: `new_project`, `add_dependency` y `remove_dependency`.
- Builds: `build`; `run`, con su entrada y un tiempo tras el que el programa
  termina, 60 s si no se dice, y el fallo con que termine como `panic`, el
  objeto de `tondo-panic-0.1/1` (7.5), en sus datos; `test`, con lo que imprime y su informe JSON, y su cobertura si se pide,
  y sus benchmarks con `bench`, comparados con `baseline` o `against`, y
  con `fuzz` la búsqueda de `--fuzz` durante tantos segundos (10.6 de testing);
  `doc_test`, `bind`, `update` y `target_pack`.
- Programas en ejecución: `ps`, `counters`, `trace`, `dump` y `dump_read`.
- Perfiles (sección 7.15): `profile` perfila un programa hasta que termina o
  pasa `duration`, uno en ejecución, un test o lo que miden los benchmarks,
  y da por perfil sus funciones y las líneas del programa que más toman, cada
  línea con su código, y lo que tomó cada label.
- Depuración: `debug_start` abre una sesión de `tondo debug`, viva con
  `live`, y da su id; `debug` le pasa una orden, `reload` incluida, y
  devuelve las respuestas, los eventos y lo que el programa imprimió. Si el
  programa sigue corriendo pasado `wait`, `pause` lo para y `wait` espera a
  que pare. `debug_stop` cierra la sesión.
- Programas vivos (sección 7.10): `run_start` arranca uno, con `watch` si se
  pide, y da su sesión; `run_output` da lo que imprimió desde la última vez,
  y su fallo como `panic` si terminó con uno; `run_input` le escribe en la
  entrada, `run_stop` lo termina y `reload`
  cambia su código, o el de un build vivo en ejecución nombrado como en
  `counters`.

Una herramienta que hace lo que un comando con `--json` da el JSON del
comando (sección 7.12) como sus datos. Las que ejecutan un programa lo
arrancan con `TONDO_PANIC_FORMAT=json` (7.5), así que un fallo llega como
datos y no como texto que haya que interpretar.

Las líneas y las columnas cuentan desde 1, las columnas en caracteres, como
en los diagnósticos, y los paths son relativos al directorio del servidor. Lo
que impide a una herramienta hacer su trabajo, como un build que no compila,
es un resultado con `isError`; un programa que termina con error, tests que
fallan o un formato que no se cumple son su resultado. Sus recursos son la
guía, `tondo://guide/<tema>`, la skill, `tondo://skill`, y la API de cada
módulo estándar, `tondo://api/<módulo>`.

`tondo setup` prepara los agentes y los editores de la máquina. Comprueba
que `tondo` compila y ejecuta un programa y que su servidor MCP responde;
escribe la skill, la de `tondo guide --skill`, donde la lee cada agente que
encuentra, `~/.claude/skills/tondo` para Claude Code y
`~/.agents/skills/tondo` para Codex y los del estándar Agent Skills; y
registra `tondo mcp`, con la ruta de este `tondo`, en los dos para todos los
proyectos, con sus propios comandos, `claude` y `codex`. Un agente se
encuentra por su comando en `PATH` o por su directorio de configuración en
el home. Instala además la extensión de Tondo para VS Code en cada editor de
su familia que encuentra, VS Code, VS Code Insiders, Cursor, VSCodium y
Windsurf, por su comando en `PATH` o, en macOS, dentro de su aplicación, con
`<comando> --install-extension <vsix> --force`: el `.vsix` de `--vsix`, o
el del último build de `main` que pasó CI, descargado con `gh`; sin él, dice
cómo conseguirlo, y lo demás se hace igual. Ejecutarlo de nuevo lo actualiza
todo. Dice si el directorio de `tondo` no está en `PATH` y, sin ningún
agente, cómo dar la skill y el servidor a otro.

### 7.10 Builds vivos

`--live`, en `run`, `build` y `debug`, compila un build vivo: uno de
depuración (sección 7.6) cuyo código cambia mientras el programa corre.
Cada llamada a una función del programa pasa por un slot de una tabla, y la
imagen guarda tablas de lo que un cambio puede necesitar de ella: las
funciones del runtime, que enlaza todas, las de C que llama y la identidad de
cada tipo. Un build vivo atiende siempre a las herramientas, como con
`TONDO_DIAGNOSTICS=1`, salvo con `TONDO_DIAGNOSTICS=0`. Los demás builds no
tienen nada de eso, ni su coste.

`tondo reload <pid|name>` cambia el código de un build vivo en ejecución por
el que dicen sus archivos ahora: compila el programa de nuevo, con las
opciones con que se compiló y desde el directorio donde se compiló, y el
runtime pone el código nuevo en memoria y apunta a él los slots de las
funciones que cambian. Las llamadas que empiezan después usan el código
nuevo, también las de las clausuras que ya existían, cuyo cuerpo se llama por
su slot. Cambia en caliente:

- el cuerpo de funciones, métodos, instancias genéricas, implementaciones,
  clausuras y constantes;
- las funciones, métodos, tipos e instancias nuevas;
- los campos, las variantes o la representación de un tipo, cuyos valores
  vivos se convierten (ver más abajo).

La identidad de una función, de una clausura o de la identidad de un tipo
incluye la forma de los tipos que su firma alcanza, y la de una clausura
además su lugar entre las de su función, y cuántas tiene: lo que cambia de
forma es otro, y el código viejo que queda, como una clausura que capturó
valores de la forma vieja, sigue con su versión y nunca mezcla las dos.

Un frame que ya estaba en una función cambiada, también de una clausura o de
otra tarea, pasa al código nuevo en la siguiente sentencia que empieza
(on-stack replacement): a la misma sentencia del código nuevo, la de la misma
línea una vez emparejadas las líneas de la función que no cambian, o la que
ocupa el lugar de una línea cambiada, con los valores que tiene. Cada binding
se reconoce por su nombre, y los valores que hace el compilador, como el
iterador de un `for`, por la sentencia que los hace. El frame no pasa, y
sigue con el código viejo hasta otra sentencia, donde el código nuevo
necesita un binding que el viejo no tiene ahí o que tiene otro tipo, donde un
binding presta un valor, como dentro de un `lock`, donde el viejo tiene un
valor que el nuevo no soltaría, o donde los `defer` registrados no son los
mismos; `reload` dice en qué líneas y por qué. Una función cuya firma alcanza
un tipo que cambió de forma es otra identidad, y un frame suyo pasa a la
nueva, del mismo nombre y firma, con sus parámetros convertidos como los
demás valores. Lo que vuelve al código viejo vuelve convertido a la forma
vieja: el resultado, para el llamador viejo, y un valor prestado con `mut` o
`var`, que la llamada nueva cambia en una copia convertida que al volver
sustituye al del llamador; siempre que esa vuelta no pierda nada —cada campo
viejo viene de uno nuevo, cada variante nueva existe en la vieja y cada
número cabe—. Si la pierde, o cuando sus parámetros o su resultado cambian
de otro modo, el frame termina con el código viejo. Las funciones que no cambian siguen con su código, y un cambio
posterior que las cambie mueve sus frames, también los que quedaron en
versiones anteriores.

Una clausura cuya función tiene ahora otras clausuras, o cuyas capturas
cambiaron de forma, es otra identidad. La que es ahora es, de las de su
función con la misma firma, la de texto más parecido, si se parece en lo
principal y ninguna otra tanto: un frame suyo pasa a ella con sus capturas
convertidas, que si las cambia vuelven convertidas a la clausura al
terminar, como un valor prestado, salvo si las entrega; y si solo cambió su
número, los valores de esa clausura
hechos antes ejecutan también su código nuevo.

Los valores de un tipo que cambió de forma que tiene un frame se convierten
al pasar el frame al código nuevo: como los valores no tienen identidad
(4.1 del lenguaje), la conversión es una copia de la forma nueva, y hasta
entonces el frame sigue con el código viejo y sus valores viejos. Un record
conserva sus campos por nombre; uno de un tipo que también cambió se
convierte, uno de tipo `T` que pasa a `T?` queda `some`, un número que pasa a
otro tipo numérico se convierte como `T(x)` (8.6 del lenguaje), uno nuevo o
de otro tipo empieza con su valor por defecto (9.2 del lenguaje), o si no lo
tiene, con el valor vacío de su tipo —0, 0.0, `false`, `""`, una colección
vacía, `none`, o un record de valores vacíos— y uno que ya no está se
suelta; cuando faltan tantos campos como hay nuevos y, en su orden, sus
valores se convierten, son campos renombrados, y conservan sus valores.
Un enum conserva sus variantes por nombre, con su payload convertido igual,
y sus variantes renombradas del mismo modo; y un tipo que falta es otro de
nombre nuevo, del mismo tipo y con la mayoría de sus campos o variantes, si
es el único así. Las colecciones, opciones, resultados y tuplas se
convierten elemento a elemento; un map o un set inserta de nuevo sus
elementos. Las clausuras y los valores `fn(...)` no se convierten: siguen con
su código viejo y sus valores. Un frame no pasa, y lo dice `reload`, cuando
un campo nuevo o de otro tipo no tiene valor por defecto ni valor vacío, como
un enum, y no pasa, y lo dice la salida de errores del programa, cuando tiene
un valor de una variante que ya no está o un número que no cabe en su tipo
nuevo, hasta una sentencia donde sus valores se convierten.

Lo que guarda un valor con identidad de un tipo que cambió de forma —un
`Ref`, un canal o un valor de `std.sync` con estado, como un `Mutex`—, que
comparten varias tareas, se convierte igual, una sola vez y en su sitio,
mientras ninguna tarea ejecuta código: el runtime detiene cada tarea al
empezar una sentencia. Una tarea que espera —recibir de un canal, enviar a
uno lleno, tomar un lock, esperar una tarea, dormir, o leer de la consola o
de un stream de `std.io`— sin que su espera haya hecho nada todavía deja de
esperar y empieza de nuevo esa sentencia, con el código nuevo, si nada se
movió antes en ella; lo que hizo antes de esperar, como evaluar sus
argumentos, se repite. Las demás esperas, como `updateWhen`, `Barrier.wait`,
leer un archivo o esperar un proceso, siguen. En un build vivo un `Ref`
guarda su valor aparte, donde cabe uno de otro tamaño.

El código nuevo no se pone cuando una tarea sigue en una sentencia que,
antes de la siguiente, puede usar con el código viejo lo que guarda uno de
esos valores —pasarlo a una función del programa, que ejecuta la nueva, no
cuenta—, como la que espera en una llamada de C o en una espera que sigue;
cuando una tarea tiene tomado su lock; o cuando lo que guarda no se
convierte. Tampoco cuando un frame puede guardar valores de un tipo que
cambió de forma en una tarea o en una colección de `std.sync`, que no se
convierten.

No se aplica, y el programa sigue como estaba, cuando los archivos no
compilan, con sus diagnósticos, cuando los valores con identidad no pueden
convertirse como se dijo, diciendo cuál y dónde, o cuando
el cambio llama a una función de C que el programa no enlaza: el mensaje
pide arrancarlo de nuevo. `reload` dice qué funciones cambiaron y cuáles
añadió, qué pasa con los valores que convierte, y dónde quedan frames en el
código viejo, y termina con estado 1 si no se aplicó.

Bajo `tondo debug --live`, la orden `reload` hace lo mismo, también con el
programa parado: el frame donde paró sigue en el código nuevo al continuar, y
los breakpoints de las líneas cambiadas paran en el código nuevo. `restart`,
con el programa parado, hace que el frame donde paró
vuelva ahí como con `return`, ejecutando sus `defer` y soltando sus
bindings, y que su función se llame otra vez con los mismos argumentos, con
el código que tiene ahora: el programa para en la primera línea de esa
llamada. Lo que el frame ya hizo no se deshace. Solo reinicia el frame donde
paró la tarea, y no el de una clausura. `tondo dap` lanza un build vivo con
`"live": true`, atiende la petición propia `reload` y `restartFrame` del
frame donde paró la tarea. La respuesta de `reload` tiene los campos de
`tondo reload --json` (sección 7.12): si no se aplica, sin ser un error,
`message` y `restart`, que dice si el cambio pide arrancar el programa de
nuevo; el editor muestra así qué fue de cada cambio.

`tondo run --watch` arranca un build vivo y, cuando cambian sus archivos,
también los que embebe, lo cambia como `reload`; si el cambio pide arrancarlo de nuevo, o el programa
ya terminó, lo vuelve a ejecutar, y si los archivos no compilan muestra los
diagnósticos y el programa sigue con su código. Atiende un cambio cuando los
archivos dejan de cambiar, así que lee entero uno que se guarda en varias
escrituras.

Un build vivo no libera el código de los parches, y un frame que pasa al
código nuevo deja debajo el suyo viejo, que solo devuelve lo que el nuevo
devuelve: cada reload ocupa algo más de memoria, y cada frame que pasa, uno
más de pila.

La salida estándar de un build de depuración, y por tanto de uno vivo, se
entrega en cada línea, como a una terminal, para la herramienta que la lee.

### 7.11 `tondo graph`

`tondo graph` muestra de qué está hecho un programa: sus paquetes y sus
dependencias, los módulos de cada paquete con sus archivos, los módulos que
importa cada uno, también los de `std`, y los archivos que embebe. No
compila: lee los imports y las llamadas a `std.embed`.

~~~text
$ tondo graph
app  src/main.to
  -> std.console
  -> app.models
  -> shapes
app.models  src/models/role.to src/models/user.to
  -> std.embed
  -> std.json
  embeds src/models/roles.csv
shapes  shapes/src/shapes.to
std.console
std.embed
std.json
~~~

- El argumento es un proyecto, un directorio o un archivo. El proyecto de un
  directorio o archivo es el directorio más cercano hacia arriba con
  `tondo.toml` o `src/main.to`, o el que lo contiene con `main.to`; el de un
  directorio o archivo de sus fuentes, además, se reduce a su módulo, como con
  `--module`. Un archivo fuera de un proyecto es un programa solo (sección 2).
- Un módulo se nombra por su path entero, con el nombre del paquete: un
  import de una dependencia por su alias nombra el paquete. Los archivos de
  un paquete se muestran relativos a su directorio, tras el nombre del paquete
  si es una dependencia. Los tests no forman parte del grafo.
- `--packages` muestra solo los paquetes y sus dependencias, con su alias si
  difiere y `(dev)` las de `[dev-dependencies]`; `--files`, cada archivo y los
  archivos de los módulos que importa.
- `--module <módulo>` muestra solo lo que el módulo alcanza por sus imports,
  directa o indirectamente, y con `--reverse`, lo que lo alcanza a él.
- `--dot` lo escribe para Graphviz y `--mermaid` como flowchart de Mermaid,
  con un grupo por paquete; `--json` como `tondo-graph-0.1/1` (sección 7.12).
- Un módulo que no existe en el grafo, o un directorio fuera de un proyecto,
  es un error del toolchain (estado 3).

### 7.12 Salida para herramientas

Con `--json`, un comando escribe en stdout un solo objeto JSON en una línea,
también cuando falla por diagnósticos. Tiene `format`,
`tondo-<comando>-0.1/1`, y `ok`, si el comando hizo lo que se pedía, y los
campos de su resultado:

| Comando | `format` | Campos |
|---|---|---|
| `check` | `tondo-check-0.1/1` | `diagnostics`; `ok` si compila |
| `build` | `tondo-build-0.1/1` | `output`, o `null`, y `diagnostics`; con `--library`, `files`, `linking` y `diagnostics` |
| `expand` | `tondo-expand-0.1/1` | `expansions`, cada una con `derive`, `location` y `source`, y `diagnostics` |
| `fmt` | `tondo-fmt-0.1/1` | `files`, cada uno con `file` y `changed` (o lo cambiaría, con `--check`), `compact` o `diagnostics` |
| `api` | `tondo-api-0.1/1` | `module` o `file` y `declarations`, cada una con `kind`, `name`, `declaration` y `doc`; sin argumento, `modules` con ellos; con `--list`, `modules`, sus nombres |
| `graph` | `tondo-graph-0.1/1` | `root`, `project`, `focus` (el módulo del path, o `null`), `packages` con `name`, `dir`, `dependencies` y `dev_dependencies` (`alias` y `package`) y `tests_only`, y salvo con `--packages`, `modules` con `name`, `package`, `files`, `imports` (`module`, `file` y `line`) y `embeds` (`path`, `file` y `line`) |
| `guide` | `tondo-guide-0.1/1` | `topic` y `text`; `topics` con `--list`; `skill` con `--skill` |
| `update` | `tondo-update-0.1/1` | `lock` y `packages`, con `name`, `dir` y `tests_only` |
| `bind` | `tondo-bind-0.1/1` | `file`, `module`, `libraries` y `added`, las que añadió a `tondo.toml` |
| `target pack` | `tondo-target-pack-0.1/1` | `pack` y `targets` |
| `setup` | `tondo-setup-0.1/1` | `tondo`, `version`, `agents` (`name`, `skill` y `mcp`), `editors` (`name` y `extension`), `on_path` y `messages`, lo que dice sin `--json` |
| `ps` | `tondo-ps-0.1/1` | `programs` |
| `trace` | `tondo-trace-0.1/1` | `timeline`, `profile`, `events`, `samples`, `lost` y `ended` |
| `profile` | `tondo-profile-0.1/1` | `profile`, la ruta; `kind`; `samples`; `duration_ms`; `lost` y `ended` |
| `profile show` | `tondo-profile-show-0.1/1` | `profile`, `type`, `unit`, `types`, `total`, `duration_ms`, `functions` (`function`, `file`, `flat`, `cum`), `lines` (`file`, `line`, `function`, `flat`, `cum`, `source`), `labels` (`label`, `value`, `total`) y `html` |
| `dump` | `tondo-dump-0.1/1` | `file` y `summary` |
| `dump read` | `tondo-dump-read-0.1/1` | la vista por su nombre: `summary`, `tasks`, `task`, `heap`, `objects` u `object` |
| `reload` | `tondo-reload-0.1/1` | `changed`, `added`, `moves`, `stays` y `converted`; si no puede, `ok` falso, `message` y `restart` |
| `version` | `tondo-version-0.1/1` | `version`, `backend`, `target` y `features`, lo que ofrece a las herramientas además de sus comandos: `json`, `graph`, `test-events`, `test-exact-many`, `test-watch`, `test-coverage`, `test-bench`, `test-fuzz`, `profile`, `lsp-tests`, `lsp-lenses`, `explain`, `new`, `add` y `fix` |
| `new`, `init` | `tondo-new-0.1/1`, `tondo-init-0.1/1` | `project`, `name` y `files`, los que escribió |
| `add`, `remove` | `tondo-add-0.1/1`, `tondo-remove-0.1/1` | `alias`, y con `add` la tabla, `table` |
| `fix` | `tondo-fix-0.1/1` | `applied` y `skipped`, cada reparación con su `id` y su `title`, `compiles` y los `diagnostics` de después; `ok` si compila |
| `explain` | `tondo-explain-0.1/1` | `code`, `name`, `kind`, `summary` y `examples` (`topic`, `text` y `code`); sin código, `codes` |
| `clean` | `tondo-clean-0.1/1` | `freed`, en bytes, y `all` |

Cada diagnóstico es el objeto de 22.3 del lenguaje. Las claves de los
objetos van en orden alfabético; los paths de archivos del programa son los
que se dieron o relativos a su proyecto, y los de `graph`, absolutos.

- Un error que impide el comando, con `--json` o con un `--test-format` que
  no es `human`, es `tondo-error-0.1/1`: `ok` falso, `message`, `status` (`2`
  o `3`) y `usage`, si es de uso; el estado de salida es el mismo.
- `--diagnostic-format json` en `run`, `build` y `test` escribe los
  diagnósticos de la compilación en stderr como un array JSON, en lugar del
  texto, sin cambiar lo demás que escribe el comando.
- `test` da sus formatos con `--test-format` (10 y 15 de la especificación de
  testing): `--json` es `--test-format json`, y `events` da una línea por
  resultado mientras corre. `doc-test` escribe sus registros (21.6 del
  lenguaje), y `counters` y `debug` con `--json`, un objeto por línea mientras
  corren.
- Un programa con `TONDO_PANIC_FORMAT=json` escribe su fallo en stderr como
  `tondo-panic-0.1/1` (7.5).
- Un campo nuevo puede aparecer en la misma versión de un formato; quitar o
  cambiar uno cambia la versión.

### 7.13 `tondo explain`

`tondo explain <código>` dice qué significa un código de diagnóstico o de
pánico: su nombre estable y su condición, del registro de 22.2 del lenguaje y
de 14 de la especificación de testing, y los ejemplos de la guía que lo dan,
cada uno con el texto que dice qué escribir. Sin código, lista todos. Con un
código que no existe es un error del toolchain (estado 3).

### 7.14 Proyectos desde la línea de órdenes

- `tondo new <name>` hace un proyecto en un directorio nuevo, o vacío, de ese
  nombre: `tondo.toml` con `[package] name`, `src/main.to` con un programa,
  `src/main_test.to` con un test, y `.gitignore` con `target/`. `tondo init
  [<dir>]` hace lo mismo en un directorio que existe, el actual si no se da,
  sin tocar los archivos que ya tiene; su nombre es el del directorio, con `_`
  por `-`, o `--name`. El nombre debe ser un identificador, y un directorio
  que ya es un proyecto es un error.
- `tondo add <alias>` añade una dependencia a `[dependencies]`, o con `--dev`
  a `[dev-dependencies]`: `--path`, `--git` con `--tag`, `--branch` o `--rev`,
  o `--url` con `--sha256`, como las escribe la sección 4.1. Después la
  resuelve y la descarga como un build; si no puede, `tondo.toml` queda como
  estaba. Un alias que el proyecto ya tiene es un error. `tondo remove
  <alias>` la quita y vuelve a escribir `tondo.lock`.
- `tondo fix` aplica las reparaciones `safe` de los diagnósticos (22.4 del
  lenguaje) a los archivos del programa, las que no se tocan entre sí, y
  comprueba de nuevo; da `1` si sigue sin compilar. Entre ellas están los
  imports que faltan, en orden, y quitar los que no se usan.
- `tondo clean` vacía la caché de builds (sección 6); con `--all`, también
  los repositorios y archivos descargados, que el siguiente build descarga
  de nuevo.

### 7.15 Perfiles

`tondo profile` dice dónde gasta un programa su tiempo de procesador, qué
memoria asigna y retiene, o dónde están sus tareas, nombrado como en
`counters` (7.5) o uno que arranca tras `--` y compila como `run`:

- `--cpu`, por defecto: las muestras de cada milisegundo de procesador de
  7.5, hasta que el programa termina, pasa `--duration` o llega Ctrl-C. El
  programa que arranca no cuenta su memoria, que cambiaría lo que mide.
- `--memory`: las muestras de memoria de 7.5, la viva y todo lo que asignó
  cada pila desde que empezó a muestrear, al terminar `--duration` o
  Ctrl-C, o al acabar el programa. Cada muestra cuenta como las
  asignaciones y bytes que representa, `1 / (1 − e^(−n / 512 KiB))` de ellas.
- `--tasks`: dónde está cada tarea, como en `dump`, al terminar `--duration`.

Escribe el perfil en el formato de pprof (`profile.proto`), comprimido con
gzip, en `<output>` o en `<programa>-<pid>.<cpu|memory|tasks>.pprof`, que
leen `go tool pprof`, speedscope, Pyroscope y Datadog. Cada marco tiene su
función, su archivo y su línea, también el más interno de una muestra: el
compilador guarda, junto a la tabla de llamadas de las trazas, dónde empieza
el código de cada línea de cada función, sin coste al ejecutar. Los tipos de
muestra son `samples` y `cpu` (nanosegundos); `alloc_objects`,
`alloc_space`, `inuse_objects` e `inuse_space`, este el primero que se
muestra, o `alloc_space` en el perfil de un test o de un benchmark, que no
tiene memoria viva; y `tasks`. Las muestras de CPU llevan la etiqueta `thread`, y las de
tareas, `state`; las de CPU llevan también los labels de la tarea que
corría (`profile.withLabels`, 14.26 de la librería estándar). En un perfil de
memoria, el asignador y su muestreo no cuentan como marcos: el más interno es
lo que pidió la memoria.

`tondo profile show <profile>` lee un perfil de pprof, de Tondo o no: el
tipo de muestra que el perfil muestra primero, u otro con `--sample`, en
total y por función, `--top` de ellas (20 por defecto), cada una con lo
suyo (`flat`, donde la muestra está) y con lo que llama (`cum`). Con
`--lines`, por línea: lo suyo de una línea es lo que gasta la línea más
interna del código del programa de cada pila, con lo que llama del runtime
y de la librería estándar, así que señala el programa; con su texto cuando
el archivo está. `--html <path>` escribe además un flame graph en un archivo
HTML sin scripts: cada función, una caja tan ancha como su parte sobre la de
quien la llamó, cuyo título dice cuánto, y la tabla de funciones. Con
labels, dice además cuánto tomó cada valor de cada uno, salvo `thread`. Con
`--json`, `tondo-profile-show-0.1/1` (7.12).

## 8. Determinismo

Con las mismas fuentes, `tondo.toml`, `tondo.lock` y versión de compilador:

- la resolución de módulos y paquetes es idéntica;
- los diagnósticos tienen el mismo texto y el orden de 22.6;
- el orden físico de lectura de archivos no cambia ningún resultado.

En los diagnósticos, el path lógico de un archivo del proyecto es relativo a su
raíz, y el de un archivo de una dependencia empieza por el nombre de su paquete.

## 9. Errores del toolchain

Un `tondo.toml` o `tondo.lock` inválido, una dependencia que no se puede
descargar o verificar, un ciclo de paquetes o un proyecto sin fuentes son
errores del toolchain, no diagnósticos de fuente: se informan con el archivo y
la línea cuando existen y terminan con estado `3`, antes de lexear ninguna
fuente.

## 10. Límites de 0.1

El toolchain 0.1 no define:

- un registro de paquetes ni resolución de versiones semánticas;
- workspaces de varios paquetes;
- fuentes condicionales por plataforma;
- paquetes binarios, interfaces compiladas ni enlace dinámico entre paquetes.

Añadir cualquiera de ellos no puede volver no reproducible un build fijado por
`tondo.lock`.
