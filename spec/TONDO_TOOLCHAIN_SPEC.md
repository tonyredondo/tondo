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
   excepto `*_test.to`.
2. Sin `src/`, el directorio debe contener `tondo.toml` o `main.to`, y el
   paquete son los `.to` de ese nivel, excepto `*_test.to`.
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

- `[package]` admite `name`, un identificador, y `edition`, cuyo único valor
  es `"0.1"`, el que se usa por defecto.
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

Para descargar, el toolchain usa `git`, `curl`, `tar` y `unzip` del sistema, como
usa `cc` para enlazar. Si falta una herramienta, el error lo dice. Con
`tondo.lock` y la caché completos no se ejecuta ninguna.

## 7. Comandos

~~~text
tondo run [<project>|<file.to>] [--link <library>]... [-- <argument>...]
tondo build [<project>|<file.to>] [--output <path>] [--link <library>]...
tondo check [<project>|<file.to>] [--json]
tondo expand [<project>|<file.to>]
tondo fmt [--check | --compact] <file.to>...
tondo api [<module>|<file.to>] [--private] [--list] [--project <dir>]
tondo guide [<topic>] [--list] [--skill]
tondo update [<project>] [<dependency>...]
tondo bind [<header.h>...] [--package <name>] [--library <library>]... [--project <dir>] [--output <file.to>] [-I <dir>] [-D <name[=value]>]
tondo test [<project>]
tondo lsp
~~~

- `run` compila, guarda el ejecutable en la caché y lo ejecuta; `build` escribe
  el ejecutable; `check` solo informa diagnósticos, en JSON con `--json` (22.3).
- `--link` enlaza una librería de C más, por nombre o archivo como en
  `[native] libraries`; un archivo suelto, que no tiene `tondo.toml`, la usa
  para sus `extern fn`.
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
- `guide` enseña Tondo a personas y a modelos (sección 7.3).
- `bind` escribe un módulo con las declaraciones de C de unos headers, de un
  paquete del sistema o de una librería (sección 7.1) y, en un proyecto, añade
  cada librería a `[native] libraries`.
- `test` sigue la [especificación de testing](./TONDO_TESTING_SPEC.md).
- `lsp` sirve a un editor (sección 7.4).
- Estado de salida: `0` éxito; `1` diagnósticos de error o fallo de formato; `2`
  uso incorrecto; `3` error del toolchain (sección 9). `run` devuelve el estado
  del programa.

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
$ tondo api --list
~~~

- El argumento es un módulo de la stdlib (`std.http`, `std.fs`), `prelude`
  (los métodos de los tipos intrínsecos), un módulo del proyecto o de sus
  dependencias (`app.models`, `shapes`) o un archivo `.to`. Sin argumento,
  muestra todos los módulos del paquete, cada uno tras `// module nombre`.
  `--list` nombra los módulos disponibles.
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
  `meta`, `ffi`, `pitfalls`) y `--list` los nombra.
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
  un campo, la declaración de un tipo, y la documentación `///`; de lo que
  implementa el runtime, como `text.length()` o `console.println`, lo que
  muestra `tondo api`.
- Las apariciones en el archivo de lo que está bajo el cursor, y los símbolos
  del proyecto cuyo nombre contiene lo que se busca.
- La firma de la llamada que se escribe, con su documentación y el argumento
  en que está el cursor.
- El tipo de cada variable declarada sin escribirlo, como pista en línea.
- Definición y referencias de variables, parámetros (también en argumentos con
  nombre), funciones, métodos, tipos, campos, variantes, constantes, traits y
  módulos; las de la librería estándar se abren desde una copia en la caché.
- Renombrar una declaración del programa cambia cada referencia; un campo
  escrito con el nombre de una variable, `User { name }`, pasa a
  `User { fullName: name }`.
- Autocompletado: tras `valor.`, los campos y métodos de su tipo, también los
  que implementa el runtime con su documentación de `tondo api`; tras
  `modulo.`, sus declaraciones públicas; y si no, las variables y parámetros
  de la función hasta el cursor, las declaraciones del módulo, los módulos que
  usa y las palabras clave.
- Resaltado semántico de cada nombre según lo que declara (módulo, tipo, enum,
  trait, parámetro, variable, campo, variante, función, método), de las
  palabras clave, los textos, los números y los comentarios.
- Los símbolos de un archivo y su formato canónico (21 del lenguaje).
- Las reparaciones de los diagnósticos (22.4 del lenguaje), cada una como
  acción `quickfix`, preferida si es `safe`, y una acción `source.fixAll` que
  aplica las seguras del archivo que no se tocan entre sí.

Las posiciones cuentan bytes UTF-8 cuando el editor lo admite y unidades
UTF-16 si no.

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
- paquetes binarios, interfaces compiladas ni enlace dinámico;
- una ABI nativa general ni declaraciones `extern`.

Añadir cualquiera de ellos no puede volver no reproducible un build fijado por
`tondo.lock`.
