# Tondo Toolchain 0.1

**Estado:** borrador normativo en desarrollo; Tondo todavía no se ha publicado

**Especificación de lenguaje:** [Tondo 0.1](./TONDO_LANGUAGE_SPEC.md)

**Especificación de testing:** [Testing Tondo 0.1](./TONDO_TESTING_SPEC.md)

**Formato para LLMs:** [Tondo LLM Form](./TONDO_LLM_FORM_SPEC.md)

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

[native]
libraries = ["sqlite3", "static:z", "vendor/libpng.a", "vendor/libfoo.so"]
~~~

- `[package]` admite `name`, un identificador, y `edition`, cuyo único valor
  es `"0.1"`, el que se usa por defecto.
- `[dependencies]` asocia a cada alias su fuente (sección 4).
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
- Toda otra tabla o clave es error, igual que una clave fuera de tabla.

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
tondo fmt [--check] <file.to>...
tondo update [<project>] [<dependency>...]
tondo bind <header.h>... [--library <library>]... [--project <dir>] [--output <file.to>] [-I <dir>] [-D <name[=value]>]
tondo test [<project>]
~~~

- `run` compila, guarda el ejecutable en la caché y lo ejecuta; `build` escribe
  el ejecutable; `check` solo informa diagnósticos, en JSON con `--json` (22.3).
- `--link` enlaza una librería de C más, por nombre o archivo como en
  `[native] libraries`; un archivo suelto, que no tiene `tondo.toml`, la usa
  para sus `extern fn`.
- `fmt` aplica el formato canónico (21) a archivos sueltos; `--check` falla si
  alguno no lo cumple.
- `bind` escribe un módulo con las declaraciones de C de unos headers
  (sección 7.1) y añade cada `--library` a `[native] libraries`.
- `test` sigue la [especificación de testing](./TONDO_TESTING_SPEC.md).
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
| `union`, `struct` con bit fields | `extern type` de sus bytes, con su alineación |
| `typedef` | `alias` |
| `enum` | `alias` de `ffi.CInt` y una constante por enumerador |
| macro de un número o un texto | `const` |

Los nombres de C se conservan; uno que es palabra reservada o nombre del
prelude de Tondo recibe `_` al final. Un puntero es opcional, porque C puede
pasar `NULL`; `const char*` es `ffi.CString?`. Un tipo de otro header se
resuelve a lo que nombra, y un struct que solo se alcanza por puntero es
opaco. Lo que no tiene forma en Tondo, como un `va_list`, una unión por valor
o una macro que es un puntero, se lista al final del módulo en un comentario.
El módulo se regenera con el mismo comando que muestra su primera línea.

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
- fuentes condicionales por plataforma ni generación de código en el build;
- paquetes binarios, interfaces compiladas ni enlace dinámico;
- una ABI nativa general ni declaraciones `extern`.

Añadir cualquiera de ellos no puede volver no reproducible un build fijado por
`tondo.lock`.
