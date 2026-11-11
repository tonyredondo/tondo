# std.env

the program's arguments and environment.

```tondo
import std.console
import std.env

let home = env.get("HOME").unwrapOr("/")
console.println("{env.args().length()} arguments; home is {home}")
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.env` shows the same in a terminal.

- [Functions](#functions): [`args`](#args), [`get`](#get), [`vars`](#vars), [`set`](#set), [`setAll`](#setall), [`remove`](#remove), [`currentDirectory`](#currentdirectory), [`setCurrentDirectory`](#setcurrentdirectory), [`syncNative`](#syncnative), [`hostname`](#hostname), [`processId`](#processid), [`executable`](#executable), [`invokedAs`](#invokedas), [`homeDirectory`](#homedirectory), [`configDirectory`](#configdirectory), [`cacheDirectory`](#cachedirectory), [`processors`](#processors), [`user`](#user), [`system`](#system), [`build`](#build)
- [EnvError](#enverror)
- [Build](#build-1)
- [Dependency](#dependency)
- [User](#user-1)
- [System](#system-1)

### Functions

#### args

```tondo
pub fn args(): Array[String]
```

The arguments without the program's name.

#### get

```tondo
pub fn get(name: String): String?
```

The value of the environment variable, or `none` when it is not set.

#### vars

```tondo
pub fn vars(): Map[String, String]
```

The whole environment, sorted by name.

#### set

```tondo
pub fn set(name: String, value: String)
```

Changes the environment of the program and the processes it starts later.

#### setAll

```tondo
pub fn setAll(values: Map[String, String])
```

Sets each variable of the map, as `set` does.

#### remove

```tondo
pub fn remove(name: String)
```

Removes the variable from the environment of the program and of the
processes it starts later.

#### currentDirectory

```tondo
pub fn currentDirectory(): String
```

The program's working directory, which relative paths start from.

#### setCurrentDirectory

```tondo
pub fn setCurrentDirectory(path: String): !EnvError
```

Changes the program's working directory, for all its tasks.

#### syncNative

```tondo
pub fn syncNative()
```

Copies the program's environment to the system's, before C code reads it.

#### hostname

```tondo
pub fn hostname(): String
```

The machine's name, as the system gives it.

#### processId

```tondo
pub fn processId(): Int
```

The identifier of the program's process.

#### executable

```tondo
pub fn executable(): String
```

The absolute path of the program's executable, its links resolved.

#### invokedAs

```tondo
pub fn invokedAs(): String
```

The name the program was run by, its `argv[0]` as given: `go` for a
link named `go`.

#### homeDirectory

```tondo
pub fn homeDirectory(): String?
```

The user's home directory, `$HOME`; `none` when it is not set.

#### configDirectory

```tondo
pub fn configDirectory(): String?
```

Where programs keep the user's settings: `$XDG_CONFIG_HOME` or
`~/.config` on Linux, `~/Library/Application Support` on macOS.

#### cacheDirectory

```tondo
pub fn cacheDirectory(): String?
```

Where programs keep for the user what they can make again:
`$XDG_CACHE_HOME` or `~/.cache` on Linux, `~/Library/Caches` on macOS.

#### processors

```tondo
pub fn processors(physical: Bool = false): Int
```

How many processors the program may run on at once, as its affinity
and its cgroup's quota allow: at least 1. With `physical`, the cores of
the machine, each once however many threads it runs.

#### user

```tondo
pub fn user(): User
```

The user the program runs as.

#### system

```tondo
pub fn system(): System
```

The system the program runs on.

#### build

```tondo
pub fn build(): Build
```

What the program was built from, read when it compiled.

### EnvError

```tondo
pub enum EnvError {
    NotFound
    NotADirectory
    PermissionDenied
    Host
}
```

Why the working directory could not change: the path does not exist, is
not a directory or is not allowed, or the host failed.

```tondo
import std.console
import std.env

match env.setCurrentDirectory("/nowhere") {
    ok(_) => console.println("moved")
    err(env.EnvError.NotFound) => console.println("no such directory")
    err(error) => console.println("{error}")
}
```

### Build

```tondo
pub type Build = {
    /// The package's name, or the file's without `.to`.
    name: String
    /// The `version` of `tondo.toml`'s `[package]`, or `0.0.0`.
    version: String
    /// The commit of the git repository the package was in.
    commit: String?
    /// Whether tracked files had changes that were not committed.
    modified: Bool
    /// The target, as tondo names it: `x86_64-linux-gnu`, `aarch64-macos`.
    target: String
    /// The compiler that built it: `tondo 0.1.0`.
    compiler: String
    /// The packages it uses, without those only its tests use.
    dependencies: Array[Dependency]
}
```

What a program was built from, which `env.build()` gives.

```tondo
import std.console
import std.env

let build = env.build()
console.println("{build.name} {build.version} ({build.commit})")
```

### Dependency

```tondo
pub type Dependency = {
    name: String
    version: String
}
```

A package a program uses: its name and the `version` of its
`tondo.toml`, or `0.0.0`.

```tondo
import std.console
import std.env

for dependency in env.build().dependencies {
    console.println("{dependency.name} {dependency.version}")
}
```

### User

```tondo
pub type User = {
    id: Int
    group: Int
    name: String
    home: String
}
```

The user the program runs as: its id, its group's, its name and its
home, as the system's accounts say; a name or a home they do not have
is empty.

```tondo
import std.console
import std.env

let user = env.user()
console.println("{user.name} ({user.id}) at {user.home}")
```

### System

```tondo
pub type System = {
    name: String
    release: String
    machine: String
}
```

The system the program runs on, as `uname` says: its name, `Linux` or
`Darwin`, its release and the machine, as `x86_64` or `arm64`.

```tondo
import std.console
import std.env

let system = env.system()
console.println("{system.name} {system.release} on {system.machine}")
```
