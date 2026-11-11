# std.archive

zip, tar, tar.gz and tar.zst behind one API, read and written in Tondo; the runtime deflates, inflates and checks CRC-32.

```tondo
import std.archive
import std.console

let site = archive.open("site.zip")?
for entry in site.entries() {
    console.println("{entry.name} {entry.size}")
}
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.archive` shows the same in a terminal.

- [Functions](#functions): [`open`](#open), [`read`](#read), [`create`](#create), [`writer`](#writer)
- [Format](#format)
- [EntryKind](#entrykind)
- [Entry](#entry)
- [ArchiveError](#archiveerror)
- [Archive](#archive): [`Archive.format`](#archiveformat), [`Archive.entries`](#archiveentries), [`Archive.readBytes`](#archivereadbytes), [`Archive.readText`](#archivereadtext), [`Archive.extract`](#archiveextract), [`Archive.files`](#archivefiles)
- [Writer](#writer-1): [`Writer.add`](#writeradd), [`Writer.addFile`](#writeraddfile), [`Writer.addDirectory`](#writeradddirectory), [`Writer.finish`](#writerfinish)

### Functions

#### open

```tondo
pub fn open(path: String): Archive ! ArchiveError
```

The archive in the file at `path`, of the format its contents say.

#### read

```tondo
pub fn read(bytes: Array[Byte]): Archive ! ArchiveError
```

The archive these bytes are, of the format their first bytes say: zip,
tar, or tar in gzip or Zstandard.

#### create

```tondo
pub fn create(path: String, from: fs.FileSystem, format: Format? = none): !ArchiveError
```

Writes the files of a tree to an archive at `path`, of the format its
name says unless `format` says another: `archive.create("site.zip",
fs.disk("public"))`.

#### writer

```tondo
pub fn writer(out: io.Writer, format: Format): Writer
```

A writer of an archive of `format` to `out`.

### Format

```tondo
pub enum Format {
    Zip
    Tar
    TarGzip
    TarZstd
}
```

A kind of archive.

```tondo
import std.archive
import std.fs

// A `.tar.gz` of a directory; without a format, the path's extension says.
archive.create("site.tar.gz", fs.disk("site"), format: some(archive.Format.TarGzip))?
```

Implements `Display`.

### EntryKind

```tondo
pub enum EntryKind {
    File
    Directory
    Symlink
}
```

What an entry of an archive is.

```tondo
import std.archive
import std.console

for entry in archive.open("site.zip")?.entries() {
    match entry.kind {
        archive.EntryKind.Directory => console.println("{entry.name}/")
        archive.EntryKind.Symlink => console.println("{entry.name} -> {entry.target}")
        archive.EntryKind.File => console.println("{entry.name} {entry.size} bytes")
    }
}
```

### Entry

```tondo
pub type Entry = {
    name: String
    kind: EntryKind
    size: Int
    modified: time.DateTime
    permissions: Int
    target: String?
}
```

An entry of an archive: its path, with `/`, what it is, its size, when
it changed, its Unix permissions, and the target of a link.

```tondo
import std.archive
import std.console

let largest = archive.open("site.zip")?.entries().sortedBy((entry) { -entry.size }).first()
console.println("{largest.map((entry) { entry.name })}")
```

### ArchiveError

```tondo
pub type ArchiveError = {
    message: String
}
```

Why an archive could not be read, written or extracted.

```tondo
import std.archive
import std.console

match archive.open("missing.zip") {
    ok(found) => console.println("{found.entries().length()} entries")
    err(error) => console.println("cannot open it: {error.message}")
}
```

Implements `Display`.

### Archive

```tondo
pub type Archive = {
    // private fields
}
```

An archive read whole, its entries and their contents.

```tondo
import std.archive
import std.console

let site = archive.open("site.zip")?
console.println(site.readText("index.html")?)
site.extract("out")?
```

#### Archive.format

```tondo
pub fn Archive.format(self): Format
```

Its format.

#### Archive.entries

```tondo
pub fn Archive.entries(self): Array[Entry]
```

Its entries, in their order.

#### Archive.readBytes

```tondo
pub fn Archive.readBytes(self, name: String): Array[Byte] ! ArchiveError
```

The contents of the file `name`.

#### Archive.readText

```tondo
pub fn Archive.readText(self, name: String): String ! ArchiveError
```

The text of the UTF-8 file `name`.

#### Archive.extract

```tondo
pub fn Archive.extract(self, into: String): !ArchiveError
```

Writes the entries under the directory `into`, with their permissions.
An entry whose path, or a link whose target, would leave `into` stops
it, as one that a crafted archive holds.

#### Archive.files

```tondo
pub fn Archive.files(self): fs.FileSystem
```

The files of the archive as a tree of `std.fs`, in memory.

### Writer

```tondo
pub type Writer = {
    // private fields
}
```

Writes an archive to a stream entry by entry; `finish` ends it.

```tondo
import std.archive
import std.fs

let file = fs.open("notes.zip", fs.OpenMode.Write)?
let zip = archive.writer(file.writer(), archive.Format.Zip)
zip.add("hello.txt", "hello".bytes())?
zip.addDirectory("empty")?
zip.finish()?
```

#### Writer.add

```tondo
pub fn Writer.add(self, name: String, data: Array[Byte], permissions: Int = 0o644): !ArchiveError
```

Adds a file of these contents.

#### Writer.addFile

```tondo
pub fn Writer.addFile(self, name: String, path: String): !ArchiveError
```

Adds the file of the disk at `path` as `name`.

#### Writer.addDirectory

```tondo
pub fn Writer.addDirectory(self, name: String): !ArchiveError
```

Adds a directory.

#### Writer.finish

```tondo
pub fn Writer.finish(self): !ArchiveError
```

Ends the archive: a zip's central directory, a tar's two empty blocks,
and the end of the compression.
