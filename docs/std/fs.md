# std.fs

Files and directories: reading and writing them whole or a piece at a
time, listing and walking trees, patterns, metadata, links, locks and
watching for changes. Every function that can fail returns a result with an
[`FsError`](#fserror) that says which way, so a missing file is a case to
match and not an exception to remember.

A tree of files is also a value. A [`FileSystem`](#filesystem) is a
directory of the disk, files in memory, the files kept inside the
executable or the content of an archive, all used through the same
functions, which lets code that works on files be tested without touching
the disk.

- [Reading and writing](#reading-and-writing)
- [A file system as a value](#a-file-system-as-a-value)
- [What to reach for](#what-to-reach-for)

## Reading and writing

```tondo
import std.console
import std.fs

fn main(): !fs.FsError {
    let directory = fs.createTemporaryDirectory(pattern: "notes-")?
    defer { _ = fs.remove(directory, recursive: true) }

    fs.writeText("{directory}/today.md", "# Today\n\n- write\n- test\n")?
    fs.appendText("{directory}/today.md", "- ship\n")?
    fs.createDirectory("{directory}/archive")?
    fs.copy("{directory}/today.md", "{directory}/archive/yesterday.md")?

    let text = fs.readText("{directory}/today.md")?
    console.println("{text.lines().length()} lines, {fs.metadata("{directory}/today.md")?.size} bytes")
    console.println("{fs.list(directory)?.sorted()}")
    console.println("{fs.glob("{directory}/**/*.md")?.length()} Markdown files")

    match fs.readText("{directory}/tomorrow.md") {
        ok(_) => console.println("there is a tomorrow")
        err(fs.FsError.NotFound) => console.println("no tomorrow yet")
        err(error) => fail error
    }
}
```

```text output
5 lines, 31 bytes
[archive, today.md]
2 Markdown files
no tomorrow yet
```

- `readText` and `writeText` take a whole file. [`open`](#open) gives a
  [`File`](#file) to read a line at a time or to write as the data comes,
  and the file closes when its block ends.
- `writeText` writes through a temporary file and a rename, so a reader
  sees the old content or the new, never half of it.
- Paths are strings. [`std.path`](path.md) joins and splits them.

## A file system as a value

The same functions exist on a [`FileSystem`](#filesystem), with paths
relative to its top. A function that takes one works on the disk in the
program and in memory in its tests:

```tondo
import std.console
import std.fs

/// The titles of the Markdown files of a tree: the first line of each.
fn titles(files: fs.FileSystem): Array[String] ! fs.FsError {
    var found: Array[String] = []
    for file in files.glob("**/*.md")? {
        found.push(files.readText(file)?.lines().first().unwrapOr("").removePrefix("# "))
    }
    found
}

fn main(): !fs.FsError {
    let files = fs.memory()
    files.createDirectory("guide")?
    files.writeText("index.md", "# Welcome\n")?
    files.writeText("guide/install.md", "# Installing\n\nOne command.\n")?
    console.println("{titles(files)?}")
    console.println("{titles(files.sub("guide"))?}")
}
```

```text output
[Installing, Welcome]
[Installing]
```

`fs.disk("site")` is a directory of the disk, `embed.files("public")` the
files the compiler put inside the executable ([`std.embed`](embed.md)), and
`files.sub("guide")` a view of one directory that cannot name anything
outside it. The [`sitegen`](../../examples/sitegen) example is built on
this.

## What to reach for

| To | Use |
| --- | --- |
| Read or write a whole file | [`readText`](#readtext), [`readBytes`](#readbytes), [`writeText`](#writetext), [`writeBytes`](#writebytes), [`appendText`](#appendtext) |
| Read or write a piece at a time | [`open`](#open), [`File.readLine`](#filereadline), [`File.write`](#filewrite) |
| Know whether something is there, and what it is | [`exists`](#exists), [`metadata`](#metadata) |
| List a directory, or a whole tree | [`list`](#list), [`walk`](#walk), [`glob`](#glob) |
| Make, move, copy and remove | [`createDirectory`](#createdirectory), [`rename`](#rename), [`copy`](#copy), [`remove`](#remove) |
| Work somewhere that goes away | [`createTemporaryDirectory`](#createtemporarydirectory), [`createTemporaryFile`](#createtemporaryfile) |
| Run something when files change | [`watch`](#watch) |
| Keep two programs off the same file | [`File.lock`](#filelock), [`File.tryLock`](#filetrylock) |
| Pass a tree of files around | [`disk`](#disk), [`memory`](#memory), [`overlay`](#overlay), [`FileSystem.sub`](#filesystemsub) |

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.fs` shows the same in a terminal.

- [Functions](#functions): [`readText`](#readtext), [`readBytes`](#readbytes), [`writeText`](#writetext), [`writeBytes`](#writebytes), [`appendText`](#appendtext), [`exists`](#exists), [`list`](#list), [`createDirectory`](#createdirectory), [`rename`](#rename), [`copy`](#copy), [`remove`](#remove), [`open`](#open), [`metadata`](#metadata), [`walk`](#walk), [`glob`](#glob), [`createTemporaryDirectory`](#createtemporarydirectory), [`createTemporaryFile`](#createtemporaryfile), [`setPermissions`](#setpermissions), [`createSymlink`](#createsymlink), [`createLink`](#createlink), [`setModified`](#setmodified), [`space`](#space), [`readSymlink`](#readsymlink), [`realPath`](#realpath), [`watch`](#watch), [`disk`](#disk), [`memory`](#memory), [`overlay`](#overlay)
- [FsError](#fserror)
- [FileKind](#filekind)
- [OpenMode](#openmode)
- [File](#file): [`File.readLine`](#filereadline), [`File.read`](#fileread), [`File.readText`](#filereadtext), [`File.write`](#filewrite), [`File.writeBytes`](#filewritebytes), [`File.flush`](#fileflush), [`File.close`](#fileclose), [`File.lock`](#filelock), [`File.tryLock`](#filetrylock), [`File.writer`](#filewriter), [`File.reader`](#filereader)
- [Metadata](#metadata-1)
- [Space](#space-1)
- [Entry](#entry)
- [ChangeKind](#changekind)
- [Change](#change)
- [FileSystem](#filesystem): [`FileSystem.sub`](#filesystemsub), [`FileSystem.directory`](#filesystemdirectory), [`FileSystem.tag`](#filesystemtag), [`FileSystem.readBytes`](#filesystemreadbytes), [`FileSystem.readText`](#filesystemreadtext), [`FileSystem.writeBytes`](#filesystemwritebytes), [`FileSystem.writeText`](#filesystemwritetext), [`FileSystem.exists`](#filesystemexists), [`FileSystem.list`](#filesystemlist), [`FileSystem.metadata`](#filesystemmetadata), [`FileSystem.walk`](#filesystemwalk), [`FileSystem.glob`](#filesystemglob), [`FileSystem.createDirectory`](#filesystemcreatedirectory), [`FileSystem.remove`](#filesystemremove)

### Functions

#### readText

```tondo
pub fn readText(path: String): String ! FsError
```

The content of the file as text; one that is not UTF-8 is
`FsError.InvalidData`.

#### readBytes

```tondo
pub fn readBytes(path: String): Array[Byte] ! FsError
```

The content of the file as bytes.

#### writeText

```tondo
pub fn writeText(path: String, text: String): Unit ! FsError
```

Writes through a temporary file and a rename: readers see the old or the
new content, never half of it. It keeps the permissions of the file it
replaces, and writes the file a link names.

#### writeBytes

```tondo
pub fn writeBytes(path: String, data: Array[Byte]): Unit ! FsError
```

Writes through a temporary file and a rename, as `writeText`.

#### appendText

```tondo
pub fn appendText(path: String, text: String): Unit ! FsError
```

Adds the text at the end of the file, which it creates if missing.

#### exists

```tondo
pub fn exists(path: String): Bool
```

Whether the path names an entry; a link to nothing does not count.

#### list

```tondo
pub fn list(path: String): Array[String] ! FsError
```

The names of the entries, sorted by bytes.

#### createDirectory

```tondo
pub fn createDirectory(path: String): Unit ! FsError
```

Creates the missing parents too; an existing directory is fine.

#### rename

```tondo
pub fn rename(from: String, to: String): Unit ! FsError
```

Moves the entry to `to`, replacing a file there.

#### copy

```tondo
pub fn copy(from: String, to: String): Unit ! FsError
```

Copies the file's content to `to`, replacing a file there.

#### remove

```tondo
pub fn remove(path: String, recursive: Bool = false): Unit ! FsError
```

A file, a link or an empty directory; with `recursive`, a directory and
its content.

#### open

```tondo
pub fn open(path: String, mode: OpenMode): File ! FsError
```

The file opened as `mode` says.

#### metadata

```tondo
pub fn metadata(path: String): Metadata ! FsError
```

What the entry at the path is, without following a link.

#### walk

```tondo
pub fn walk(
    root: String,
    enter: (fn(Entry): Bool)? = none,
    skipUnreadable: Bool = false,
): Array[Entry] ! FsError
```

The tree under `root`, without `root`: each directory before what it
holds and siblings sorted by bytes, without following links. `enter`
says whether to go into each directory, which is listed either way:
`(entry) { not entry.path.endsWith("/node_modules") }`. With
`skipUnreadable`, a directory below `root` that cannot be read is left
out rather than failing the walk.

```tondo
import std.console
import std.fs

// Every Markdown file under `docs`, but those of hidden directories.
let entries = fs.walk("docs", enter: some((entry: fs.Entry) { not entry.path.contains("/.") }))?
for entry in entries {
    if entry.path.endsWith(".md") {
        console.println(entry.path)
    }
}
```

#### glob

```tondo
pub fn glob(pattern: String, skipUnreadable: Bool = false): Array[String] ! FsError
```

The paths that match a pattern of `path.matches`, sorted by bytes:
`content/**/*.md`. With `skipUnreadable`, a directory that cannot be
read is left out rather than failing.

#### createTemporaryDirectory

```tondo
pub fn createTemporaryDirectory(directory: String? = none, pattern: String = ""): String ! FsError
```

A new empty directory in `directory`, or the system's temporary one,
which only its user can read; the program removes it. Its name is
`pattern` with its last `*` made random, `"build-*"`, or with random
characters after it if it has none.

#### createTemporaryFile

```tondo
pub fn createTemporaryFile(directory: String? = none, pattern: String = ""): String ! FsError
```

A new empty file in `directory`, or the system's temporary one, which
only its user can read; the program removes it. Its name is `pattern`
with its last `*` made random, `"report-*.json"`, or with random
characters after it if it has none.

#### setPermissions

```tondo
pub fn setPermissions(path: String, permissions: Int): !FsError
```

Sets the Unix permissions of the entry: `0o755` to run a script.

#### createSymlink

```tondo
pub fn createSymlink(target: String, link: String): !FsError
```

Makes `link` a symbolic link to `target`, which is kept as written and
may not exist; a relative one is read from the link's directory.

#### createLink

```tondo
pub fn createLink(target: String, link: String): !FsError
```

Makes `link` another name of the file `target`, a hard link: the same
file, which lasts while either name does.

#### setModified

```tondo
pub fn setModified(path: String, time: time.DateTime): !FsError
```

Sets when the content of the entry last changed, as `make` and caches
read it.

#### space

```tondo
pub fn space(path: String): Space ! FsError
```

The space of the file system that holds `path`.

#### readSymlink

```tondo
pub fn readSymlink(path: String): String ! FsError
```

The target of a symbolic link, as it was written.

#### realPath

```tondo
pub fn realPath(path: String): String ! FsError
```

The absolute, clean path of the same entry, with every link resolved.

#### watch

```tondo
pub fn watch(
    paths: Array[String],
    settle: time.Duration? = none,
): Receiver[Array[Change]] ! FsError
```

Watches the paths, with all that their directories hold, and sends their
changes in batches: once `settle` (100 ms without it) passes with no
other change, each path that changed once, sorted. Watching ends when no
copy of the receiver is left.

#### disk

```tondo
pub fn disk(root: String = "."): FileSystem
```

The directory `root` of the disk as a tree.

#### memory

```tondo
pub fn memory(): FileSystem
```

An empty tree in memory, whose files' directories exist with them.

#### overlay

```tondo
pub fn overlay(layers: ...FileSystem): FileSystem
```

Several trees as one, as a site's own files over its theme's: a path is
read from the first layer that has it, written to the first, and a
directory lists what all of them hold.

### FsError

```tondo
pub enum FsError {
    NotFound
    PermissionDenied
    AlreadyExists
    NotADirectory
    IsADirectory
    InvalidData
    InvalidName
    /// A failure the others do not name, with the system's number of it,
    /// `errno`: it shows with the system's text of it, `os error 28: No
    /// space left on device`, and `Host(0)` as `host`.
    Host(Int)
}
```

It names no path and no content.

```tondo
import std.console
import std.fs

match fs.readText("missing.txt") {
    ok(text) => console.println(text)
    err(fs.FsError.NotFound) => console.println("there is no such file")
    err(error) => console.println("{error}")
}
```

### FileKind

```tondo
pub enum FileKind {
    File
    Directory
    Symlink
    Other
}
```

What an entry is: a file, a directory, a symbolic link or something else.

```tondo
import std.console
import std.fs

for entry in fs.walk(".")? {
    if entry.kind == fs.FileKind.Directory {
        console.println("{entry.path}/")
    }
}
```

### OpenMode

```tondo
pub enum OpenMode {
    Read
    Write
    Append
    CreateNew
    ReadWrite
}
```

`Read` an existing file; `Write` creates or empties it; `Append` creates it
if missing and writes at the end; `CreateNew` fails if it exists;
`ReadWrite` reads and writes an existing one.

```tondo
import std.fs

// `Append` adds at the end, creating the file when it is missing.
var log = fs.open("app.log", fs.OpenMode.Append)?
log.write("started\n")?
log.close()?
```

### File

```tondo
pub type File
```

An open file, with one owner; it closes when abandoned.

```tondo
import std.console
import std.fs

var file = fs.open("notes.txt", fs.OpenMode.Read)?
for {
    match file.readLine()? {
        some(line) => console.println(line)
        none => break
    }
}
file.close()?
```

#### File.readLine

```tondo
pub fn File.readLine(var self): String? ! FsError
```

The next line without its `\n` or `\r\n`, or `none` at the end.

#### File.read

```tondo
pub fn File.read(var self, max: Int): Array[Byte]? ! FsError
```

Up to `max` bytes, or `none` at the end of the file.

#### File.readText

```tondo
pub fn File.readText(var self): String ! FsError
```

The rest of the file as text; one that is not UTF-8 is
`FsError.InvalidData`.

#### File.write

```tondo
pub fn File.write(var self, text: String): Unit ! FsError
```

Writes the text at the file's position.

#### File.writeBytes

```tondo
pub fn File.writeBytes(var self, data: Array[Byte]): Unit ! FsError
```

Writes the bytes at the file's position.

#### File.flush

```tondo
pub fn File.flush(var self): Unit ! FsError
```

Hands the writes kept for the file to the system, without closing it.

#### File.close

```tondo
pub fn File.close(self): Unit ! FsError
```

Closes the file now, with the error of its last writes if they fail;
abandoned, it closes by itself and loses that error.

#### File.lock

```tondo
pub fn File.lock(var self, shared: Bool = false): Unit ! FsError
```

Takes the file's lock, exclusive or `shared`, waiting for whoever has
it, without holding a thread; closing the file lets it go. Unix's
advisory lock (`flock`): it orders the programs that ask for it.

#### File.tryLock

```tondo
pub fn File.tryLock(var self, shared: Bool = false): Bool ! FsError
```

Takes the file's lock if it is free, without waiting: whether it did.

#### File.writer

```tondo
pub fn File.writer(self): io.Writer
```

The file as a stream of `std.io`; it closes with the stream's last copy.

#### File.reader

```tondo
pub fn File.reader(self): io.Reader
```

The file as a stream of `std.io`; it closes with the stream's last copy.

### Metadata

```tondo
pub type Metadata = {
    kind: FileKind
    size: Int
    readOnly: Bool
    modified: time.DateTime
    permissions: Int
    inode: Int
    links: Int
    owner: Int
    group: Int
}
```

An entry itself, without following links: its kind, size in bytes,
read-only flag, last change of its content and Unix permissions, as
`0o644`; its number in its file system, its hard links, and the ids of
its owner and its group. A tree that is not the disk's has no numbers:
0, and one link.

```tondo
import std.console
import std.fs

let about = fs.metadata("notes.txt")?
console.println("{about.size} bytes, changed {about.modified}, a file: {about.kind == fs.FileKind.File}")
```

### Space

```tondo
pub type Space = {
    total: Int
    free: Int
    available: Int
}
```

The space of a file system: its bytes, those free, and those free that
the program may use, which leaves out what the system keeps for itself.

```tondo
import std.console
import std.fs

let space = fs.space(".")?
console.println("{space.available / 1024 / 1024} MiB free of {space.total / 1024 / 1024}")
```

### Entry

```tondo
pub type Entry = {
    path: String
    kind: FileKind
}
```

An entry under the root of `walk`: its path, from the root, and kind.

```tondo
import std.console
import std.fs

for entry in fs.walk("src")? {
    if entry.kind == fs.FileKind.File and entry.path.endsWith(".to") {
        console.println(entry.path)
    }
}
```

### ChangeKind

```tondo
pub enum ChangeKind {
    Created
    Modified
    Removed
}
```

What happened to a path while it was watched.

```tondo
import std.console
import std.fs

for batch in fs.watch(["src"])? {
    for change in batch {
        if change.kind == fs.ChangeKind.Removed {
            console.println("gone: {change.path}")
        }
    }
}
```

### Change

```tondo
pub type Change = {
    path: String
    kind: ChangeKind
}
```

A path that changed while it was watched, and what happened to it.

```tondo
import std.console
import std.fs
import std.time

let changes = fs.watch(["content"], settle: some(time.milliseconds(200)))?
for change in changes.receive().unwrapOr([]) {
    let what = match change.kind {
        fs.ChangeKind.Created => "new"
        fs.ChangeKind.Modified => "changed"
        fs.ChangeKind.Removed => "gone"
    }
    console.println("{what} {change.path}")
}
```

### FileSystem

```tondo
pub type FileSystem = {
    // private fields
}
```

A tree of files, with the operations of `std.fs` by the same names on
paths relative to its top, written with `/`: a directory of the disk
(`fs.disk`), files in memory (`fs.memory`), the files kept in the
executable (`embed.files`), an archive's (`archive.Archive.files`), the
part under a directory of another (`sub`), or several in layers
(`fs.overlay`). Its copies share it: what one writes the others read.

```tondo
import std.console
import std.fs

// Files in memory over those on disk: what a test writes leaves the disk as it was.
let files = fs.overlay(fs.memory(), fs.disk("."))
files.writeText("draft.txt", "only in memory")?
console.println("{files.readText("draft.txt")?} {files.list()?.length()}")
```

#### FileSystem.sub

```tondo
pub fn FileSystem.sub(self, directory: String): FileSystem
```

The tree under `directory` of this one.

#### FileSystem.directory

```tondo
pub fn FileSystem.directory(self): String?
```

The directory of the disk this tree is, when it is one, for what hands
paths to the system, as another program.

#### FileSystem.tag

```tondo
pub fn FileSystem.tag(self, path: String): String?
```

The digest of a file kept in the executable, which `std.http` sends as
its `ETag`; `none` for another tree's.

#### FileSystem.readBytes

```tondo
pub fn FileSystem.readBytes(self, path: String): Array[Byte] ! FsError
```

The contents of the file at `path`.

#### FileSystem.readText

```tondo
pub fn FileSystem.readText(self, path: String): String ! FsError
```

The text of the UTF-8 file at `path`.

#### FileSystem.writeBytes

```tondo
pub fn FileSystem.writeBytes(self, path: String, data: Array[Byte]): !FsError
```

Writes the file at `path`, replacing what it held.

#### FileSystem.writeText

```tondo
pub fn FileSystem.writeText(self, path: String, text: String): !FsError
```

Writes the text to the file at `path`.

#### FileSystem.exists

```tondo
pub fn FileSystem.exists(self, path: String): Bool
```

Whether there is a file or a directory at `path`.

#### FileSystem.list

```tondo
pub fn FileSystem.list(self, path: String = ""): Array[String] ! FsError
```

The names of what is directly in the directory at `path`, sorted.

#### FileSystem.metadata

```tondo
pub fn FileSystem.metadata(self, path: String): Metadata ! FsError
```

What the entry at `path` is.

#### FileSystem.walk

```tondo
pub fn FileSystem.walk(self, root: String = ""): Array[Entry] ! FsError
```

The tree under `root`, without `root`, its paths from the top: each
directory before what it holds, and siblings sorted.

#### FileSystem.glob

```tondo
pub fn FileSystem.glob(self, pattern: String): Array[String] ! FsError
```

The paths that match a pattern of `path.matches`, as `**/*.md`, sorted.

#### FileSystem.createDirectory

```tondo
pub fn FileSystem.createDirectory(self, path: String): !FsError
```

Makes the directory at `path` and those it is in.

#### FileSystem.remove

```tondo
pub fn FileSystem.remove(self, path: String, recursive: Bool = false): !FsError
```

Removes the file at `path`, or the directory, with what it holds when
`recursive`.
