# Tondo STD-0.1A hosted owner contract

Estado: contrato de owner cerrado para `std.console`, `std.path`, `std.fs` y
`std.process`. Los cuatro módulos usan los protocolos de `std.io`; importar un
módulo nunca concede por sí solo la capability del host.

## `std.console`

```tondo
pub async fn stdin(): std.io.Reader ! ConsoleError
pub async fn stdout(): std.io.Writer ! ConsoleError
pub async fn stderr(): std.io.Writer ! ConsoleError
pub async fn readLine(var input: std.io.Reader): String? ! ConsoleError
pub async fn print(value: String): Unit ! ConsoleError
pub async fn println(value: String): Unit ! ConsoleError
pub async fn flush(var output: std.io.Writer): Unit ! ConsoleError
pub enum ConsoleError { Unavailable, Closed, Cancelled, Io(std.io.IoError) }
```

`print` y `println` escriben mediante `std.io.Writer`; no asumen terminal,
locale ni newline de plataforma (`println` usa LF). El orden de varias writes
solo es el orden de invocación dentro del mismo writer. La capability `console`
es necesaria para adquirir los tres handles y para las comodidades de output.

## `std.path`

```tondo
pub type Path
pub enum PathKind { Relative, Absolute }
pub fn Path.fromString(value: String): Path ! PathError
pub fn Path.fromBytes(value: Bytes): Path ! PathError
pub fn Path.join(self, component: String): Path ! PathError
pub fn Path.parent(self): Path?
pub fn Path.fileName(self): String?
pub fn Path.extension(self): String?
pub fn Path.kind(self): PathKind
pub fn Path.isEmpty(self): Bool
pub fn Path.toString(self): String ! PathError
pub fn Path.toBytes(self): Bytes
pub enum PathError { InvalidEncoding, EmptyComponent, Nul, ResourceLimit, Unsupported }
```

Las operaciones de esta sección son léxicas y no consultan el filesystem.
`Path` conserva los bytes nativos cuando el target los admite; `toString` solo
falla si esos bytes no son UTF-8. No normaliza symlinks, no resuelve `..` y no
promete igualdad entre raíces de targets distintos.

## `std.fs`

```tondo
pub type File
pub type Directory
pub type Metadata
pub enum OpenMode { Read, Write, ReadWrite, Append, Create, CreateNew }
pub enum FsError { NotFound, PermissionDenied, AlreadyExists, InvalidPath, NotDirectory, IsDirectory, Closed, ResourceLimit, Cancelled, Io }
pub async fn open(path: Path, mode: OpenMode): File ! FsError
pub async fn openDirectory(path: Path): Directory ! FsError
pub async fn readAll(path: Path): Bytes ! FsError
pub async fn writeAll(path: Path, data: Bytes): Unit ! FsError
pub async fn createDirectory(path: Path, parents: Bool): Unit ! FsError
pub async fn remove(path: Path): Unit ! FsError
pub async fn metadata(path: Path): Metadata ! FsError
pub async fn list(path: Path): Array[Path] ! FsError
pub async fn rename(from: Path, to: Path): Unit ! FsError
pub async fn atomicWrite(path: Path, data: Bytes): Unit ! FsError
pub async fn File.read(var self, max: Int): Option[Bytes] ! FsError
pub async fn File.write(var self, data: Bytes): Int ! FsError
pub async fn File.flush(var self): Unit ! FsError
pub async fn Directory.list(var self): Array[Path] ! FsError
```

Las operaciones requieren `filesystem`. `File` es un handle afín que ofrece la
misma semántica de lectura/escritura que `Reader`/`Writer` mediante sus
métodos `read`/`write`/`flush`; `std.io.readAll` y `std.io.writeAll` siguen
recibiendo los handles `Reader`/`Writer` explícitos. `Directory` es un handle
afín de iteración. Ambos cierran sus recursos en cleanup normal y durante
unwind. `Read` devuelve `none` al alcanzar EOF, `Write` acepta short writes y
devuelve los bytes escritos. `Write` y `ReadWrite` abren un archivo existente;
`Create` trunca o crea y `CreateNew` exige que no exista. `Append` escribe
siempre al final.

El verificador de ownership impide que un programa seguro conserve un handle
después de su cleanup; el host también rechaza tokens stale o forjados como
una violación de la invariante de runtime. `FsError.Closed` queda reservado
para un cierre observable del recurso que pueda ocurrir sin invalidar esa
invariante.

`atomicWrite` escribe en un temporal dentro del mismo directorio, hace flush y
rename; no promete durabilidad de hardware salvo una capability posterior. La
iteración devuelve paths en orden lexicográfico de bytes para determinismo. Los
errores no incluyen rutas físicas adicionales ni fragmentos de contenido.

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
pub async fn Command.run(self): ExitStatus ! ProcessError
pub async fn Command.output(self): ProcessOutput ! ProcessError
pub async fn Command.check(self): ProcessOutput ! (ProcessError | ProcessExitError)
pub async fn Command.start(self): ProcessHandle ! ProcessError
pub async fn ProcessHandle.wait(var self): ExitStatus ! ProcessError
pub fn ProcessHandle.cancel(var self): Unit
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
huérfanos. Las rutas async tienen puntos de cancelación definidos antes de
publicar output. `ProcessExitError` conserva el output capturado sin depender
de códigos o mensajes concretos del sistema operativo.
