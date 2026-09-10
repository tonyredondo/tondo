# Tondo STD-0.1A hosted owner contract

This document specifies the hosted owners. Individual implementation and
conformance boundaries remain in the tracker; a contract alone does not close
an owner. Importing a module never grants its host capability.

## `std.console`

```tondo
// Input and Output are concrete types with private representation.

pub fn stdin(): Input ! ConsoleError
pub fn stdout(): Output ! ConsoleError
pub fn stderr(): Output ! ConsoleError
pub fn readLine(input: var Input): String? ! ConsoleError suspends
pub fn print(value: String): Unit ! ConsoleError
pub fn println(value: String): Unit ! ConsoleError
pub fn flush(): Unit ! ConsoleError suspends
pub enum ConsoleError { Unavailable, Closed, Cancelled, Io(std.io.IoError) }
```

`Input` and `Output` are concrete public types with private representation.
`Input` implements the static `std.io.Reader` trait and `Output` implements
`std.io.Writer`; neither trait is a public handle alias. `readLine` is specific
to `Input`, preserving the cursor on invalid UTF-8. An `Output` argument is
rejected statically. Generic readers have no implicit rewind capability.

The hosted protocol adapters and generic helper tests are executable.
`ConsoleError` is an ordinary nominal enum, including the nested `IoError`
payload. `print`, `println` and `flush` return `Unit ! ConsoleError`; callers
handle, propagate or explicitly discard that result. The public CLI regression
`console_public_errors_and_results_execute` checks every error variant and the
three output results. Full owner promotion still requires current integrated
evidence, whole-owner fuzzing and public conformance.

Console output and successful `readLine` admit the complete typed VM result
before emitting bytes or advancing the input cursor. EOF and invalid UTF-8 do
not consume input. Cancellation returns `ConsoleError.Cancelled`, remains
idempotent before polling, and releases the retired request and response.

`print` y `println` escriben mediante `std.io.Writer`; no asumen terminal,
locale ni newline de plataforma (`println` usa LF). El orden de varias writes
solo es el orden de invocación dentro del mismo writer. La capability `console`
es necesaria para adquirir los tres handles y para las comodidades de output.

La frontera es estática: importar `std.console` no concede `console`, y un
target sin esa capability rechaza el programa con `E1008` antes de generar
bytecode. `stdin`, `stdout` y `stderr` son tokens distintos que reutilizan los
protocolos `std.io.Reader`/`std.io.Writer`; no existe un terminal implícito ni
un locale ambiental. Los lectores pueden entregar partial I/O y
`readLine` conserva su offset hasta aceptar una línea UTF-8 completa. EOF
devuelve `none`; una línea con UTF-8 inválido o un reader que no sea stdin
devuelve `ConsoleError` sin avanzar el cursor ni publicar un valor parcial.

`print` anexa exactamente los bytes UTF-8 de `String` al buffer ordenado del
runtime y `println` añade un único byte LF. Ambas operaciones son acotadas y no
suspendibles; ninguna hace flush implícito. `flush` es la única frontera de
entrega suspendible y terminal dentro del writer correspondiente. Los errores del
host se convierten a `ConsoleError` nominal y no exponen rutas, mensajes o
detalles dependientes del sistema operativo. Los límites de bytes, chunks,
progreso y cancelación pertenecen al protocolo `std.io`; el adaptador de
console no crea buffers duplicados ni otra API síncrona/suspendible.

## `std.path`

```tondo
pub type Path
pub fn Path.fromString(value: String): Path ! PathError
pub fn Path.fromBytes(value: Bytes): Path ! PathError
pub fn Path.join(self, component: String): Path ! PathError
pub fn Path.parent(self): Path?
pub fn Path.fileName(self): String?
pub fn Path.extension(self): String?
pub fn Path.kind(self): Bool
pub fn Path.isEmpty(self): Bool
pub fn Path.toString(self): String ! PathError
pub fn Path.toBytes(self): Bytes
pub enum PathError { InvalidEncoding, EmptyComponent, Nul, ResourceLimit, Unsupported }
```

Las operaciones de esta sección son léxicas y no consultan el filesystem.
`Path` conserva los bytes nativos cuando el target los admite; `toString` solo
falla si esos bytes no son UTF-8. No normaliza symlinks, no resuelve `..` y no
promete igualdad entre raíces de targets distintos.

`kind` devuelve `true` si el snapshot es absoluto y `false` si es relativo; no
introduce un enum paralelo para una propiedad binaria.

La representación conserva exactamente los bytes entregados a `fromBytes` y
los devuelve mediante `toBytes`; no aplica NFC/NFD, case-folding, expansión de
separadores ni otra normalización Unicode. `join` acepta un único componente,
rechaza separadores, NUL y entradas que exceden el límite de 32 KiB, y deja el
path original intacto cuando falla. `parent`, `fileName` y `extension` son
consultas puras sobre los separadores `/`; no consultan existencia, permisos,
symlinks ni el directorio de trabajo. El corpus de bytes nativos y UTF-8 se
ejecuta sin capability `filesystem` y es determinista en todos los targets.

## `std.fs`

```tondo
pub type File
pub type Directory
pub enum FileKind { File, Directory, Symlink, Other }
pub type Metadata = { kind: FileKind, size: Int, readOnly: Bool }
pub enum OpenMode { Read, Write, ReadWrite, Append, Create, CreateNew }
pub enum FsError { NotFound, PermissionDenied, AlreadyExists, InvalidPath, NotDirectory, IsDirectory, Closed, ResourceLimit, Cancelled, Io }
pub fn open(path: Path, mode: OpenMode): File ! FsError suspends
pub fn openDirectory(path: Path): Directory ! FsError suspends
pub fn readAll(path: Path): Bytes ! FsError suspends
pub fn writeAll(path: Path, data: Bytes): Unit ! FsError suspends
pub fn createDirectory(path: Path, parents: Bool): Unit ! FsError suspends
pub fn remove(path: Path): Unit ! FsError suspends
pub fn metadata(path: Path): Metadata ! FsError suspends
pub fn list(path: Path): Array[Path] ! FsError suspends
pub fn rename(from: Path, to: Path): Unit ! FsError suspends
pub fn atomicWrite(path: Path, data: Bytes): Unit ! FsError suspends
pub fn File.read(var self, max: Int): Option[Bytes] ! FsError suspends
pub fn File.write(var self, data: Bytes): Int ! FsError suspends
pub fn File.flush(var self): Unit ! FsError suspends
pub fn Directory.list(var self): Array[Path] ! FsError suspends
```

Operations require `filesystem`. `OpenMode` and `FsError` are public nominal
enums: use `OpenMode.Read`, without a constructor call, and match their variants
normally. Neither enum allocates a host handle. OS errors retain their structured
category; their message text never determines the public error variant.

`File` is affine and implements the ordinary `io.Reader` and `io.Writer` traits.
The implementations belong to the trait owner `std.io` and are selected only
with the available, explicitly imported compiler-provided `std.fs` module.
An explicitly supplied source module retains its own declarations and receives
no hosted File adapter. Direct File methods
retain `Option[Bytes] ! FsError`; generic readers translate `some`/`none` to
`ReadResult.Data`/`Eof`. The adapters preserve `Closed`, `Cancelled` and
`ResourceLimit`, map other filesystem errors to `IoError.Host`, and reject
nonpositive read requests as `IoError.InvalidData` before calling the file.
`io.readAll` and `io.writeAll` accept a mutable File through generic dispatch,
including instantiated function values. An aggregate read limit returns no
partial buffer, but cannot rewind chunks consumed by earlier successful reads.

`Directory` es un handle
afín de iteración. Ambos cierran sus recursos en cleanup normal y durante
unwind. `Read` devuelve `none` al alcanzar EOF, `Write` acepta short writes y
devuelve los bytes escritos. `Write` y `ReadWrite` abren un archivo existente;
`Create` trunca o crea y `CreateNew` exige que no exista. `Append` escribe
siempre al final.

La capability `filesystem` se comprueba estáticamente: importar `std.fs` no la
concede y un target sin ella rechaza el módulo con `E1008` antes del lowering.
Los límites de bytes y de entradas se validan antes de materializar el resultado;
un exceso devuelve `FsError.ResourceLimit` sin publicar una escritura, una lista
o un contenido parcial. El cleanup se ejecuta también durante unwind y
cancelación. Los handles son tokens no forjables: usar un token stale devuelve
un error tipado y no reabre ni recicla el recurso. `list` conserva el orden
lexicográfico de bytes nativos, y sus errores no exponen rutas físicas ni
fragmentos de contenido.

El verificador de ownership impide que un programa seguro conserve un handle
después de su cleanup; el host también rechaza tokens stale o forjados como
una violación de la invariante de runtime. `FsError.Closed` queda reservado
para un cierre observable del recurso que pueda ocurrir sin invalidar esa
invariante.

`atomicWrite` creates a temporary file in the same directory, flushes it and
renames it over the destination. It does not promise hardware durability.
Exclusive creation establishes ownership of that temporary: a name collision
preserves the existing file and returns an error. Later write, flush or rename
failure closes the created handle and attempts to remove only that operation's
temporary. Directory iteration orders paths by native bytes. Errors expose no
additional physical paths or content fragments.

`STD-FS-IMPL-001` and `STD-A-FS-EVIDENCE-001` remain open. Public enum matching,
File protocol calls, typed errors and File result admission have focused tests;
fourteen discovered function signatures did not establish those behaviors.
`fs.open` also reserves its result and path scratch before the OS call, so a
memory rejection neither creates nor truncates a file. Focused tests retain
exact File/error transport charges and release path scratch on both outcomes.
Path mutations also admit the typed result and copied path/payload storage
before effects or write-quota consumption; atomic writes include their temporary
name and path. `metadata` admits the complete nominal result and path conversion
storage before its OS query, and creates no Metadata host handle. Directory
construction and bounded `readAll`/listing materialization still need complete
admission and integrated owner evidence. Whole-owner fuzzing, target performance and
global public conformance remain separate, unpromoted boundaries.

`Metadata` is an ordinary record with public `kind`, `size` and `readOnly` fields.
It follows normal value semantics and is immutable under `let`. Each call
observes the entry itself with `symlink_metadata`, so a dangling link still has
kind `FileKind.Symlink`. `Other` covers entries such as sockets and devices.
`size` is the host-reported byte length, whose meaning for nonregular entries is
platform-dependent; values larger than `Int` return `FsError.ResourceLimit`
without a partial record. `readOnly` records the host read-only attribute, which
does not establish the caller's effective write permission. Existing snapshots
remain unchanged when the file changes or disappears. Field access, copying,
record construction and exhaustive `FileKind` matching are ordinary language
operations. This public contract establishes the hosted route; retired internal
Metadata ABI scaffolds do not establish native AOT support.

## `std.process`

```tondo
pub type Command
pub type Pipeline
pub type ProcessHandle
pub type ExitStatus
pub type ProcessOutput
pub enum ProcessError { Unavailable, PermissionDenied, InvalidArgument, Spawn, Io, Cancelled, ResourceLimit }
pub enum ProcessExitError { NonZero(ProcessOutput), Signalled(ProcessOutput) }
pub fn command(program: String, arguments: ...String): Command ! ProcessError
pub fn shell(command: String): Command ! ProcessError
pub fn pipe(left: Command, right: Command): Pipeline ! ProcessError
pub fn Command.mergeStderr(self): Command
pub fn Pipeline.mergeStderr(self): Pipeline
pub fn Command.run(self): ExitStatus ! ProcessError suspends
pub fn Command.output(self): ProcessOutput ! ProcessError suspends
pub fn Command.check(self): ProcessOutput ! (ProcessError | ProcessExitError) suspends
pub fn Command.start(self): ProcessHandle ! ProcessError suspends
pub fn ProcessHandle.wait(var self): ExitStatus ! ProcessError suspends
pub fn ProcessHandle.cancel(var self): Unit suspends
pub fn ProcessOutput.stdout(self): Bytes
pub fn ProcessOutput.stderr(self): Bytes
pub fn ProcessOutput.combined(self): Bytes
pub fn ProcessOutput.statuses(self): Array[ExitStatus]
pub fn ExitStatus.code(self): Int?
pub fn ExitStatus.success(self): Bool
```

La capability `process` es necesaria para construir o ejecutar planes. `shell`
es siempre explícito; `command` conserva argv exacto sin re-tokenizar. Pipes
usan backpressure bounded y cierre coordinado. `pipe` conecta únicamente
`stdout` del lado izquierdo con `stdin` del lado derecho, como `|` en un shell.
`mergeStderr` es una redirección tipada: conecta ambos `stdout` y `stderr` del
plan con el siguiente `stdin`, como `|&` (`2>&1 |`) en Bash, sin invocar un
shell ni re-tokenizar ningún argumento. El `stderr` de las etapas que no se
redirigen conserva su canal separado.

`ProcessOutput.stdout()` y `ProcessOutput.stderr()` siempre son bytes separados;
no se presupone UTF-8. `ProcessOutput.combined()` devuelve la secuencia de bytes
observada en el límite de captura, intercalando los chunks de ambos canales en
el orden en que el host los recibe, que es la semántica útil de la salida de un
terminal. No se puede inferir una ordenación por líneas ni se reordena por
contenido. La salida combinada no convierte ni elimina bytes.

`start` devuelve un handle afín;
`wait`, `cancel` o el cleanup del owner son terminales y no dejan procesos
huérfanos. Las rutas suspendible tienen puntos de cancelación definidos antes de
publicar output. `ProcessExitError` conserva el output capturado sin depender
de códigos o mensajes concretos del sistema operativo.

La evidencia ejecutable de este owner es `STD-A-PROC-EVIDENCE-001`: enlaza las
diecisiete firmas públicas con el contrato de capability, el modelo de planes
inertes y handles terminales, HIR/lowering, bytecode/VM y el adaptador
`process_host`. Las pruebas M8 cubren argv literal, shell explícito, las cuatro
formas de pipe, backpressure con salida superior a la ventana del kernel,
stdout/stderr separados, `combined`, redirección `mergeStderr`, estados de
salida, errores de spawn, cancelación, panic/unwind y reaping. `STD-A-FUZZ-001`
promueve el fuzz owner-aware; capturas de coste por target y `STD-CONF-001`
permanecen explícitos como promoción posterior.
