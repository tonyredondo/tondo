# std.embed

files of the package, read when the program compiles and kept in the executable, which does not read them from the disk when it runs.

```tondo
import std.console
import std.embed

const Source: String = embed.text("main.to")

console.println("this program is {Source.lines().length()} lines long")
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.embed` shows the same in a terminal.

- [Functions](#functions): [`text`](#text), [`bytes`](#bytes), [`files`](#files)

### Functions

#### text

```tondo
pub fn text(path: String): String
```

The text of a UTF-8 file of the package, read when the program compiles,
at a path relative to the file that writes it: `embed.text("page.html")`.

#### bytes

```tondo
pub fn bytes(path: String): Array[Byte]
```

The bytes of a file of the package, read when the program compiles, at a
path relative to the file that writes it: `embed.bytes("logo.png")`.

#### files

```tondo
pub fn files(path: String): fs.FileSystem
```

The files under a directory of the package, read when the program
compiles, at a path relative to the file that writes it, but those whose
name starts with `.`: a tree of `std.fs` that only reads, which
`App.files` serves and `template.Set.addFiles` loads:
`const Site: fs.FileSystem = embed.files("static")`.
