# std.path

lexical operations on paths, which are strings; they never touch the filesystem.

```tondo
import std.console
import std.path

let file = path.join("docs", "guide.md")
console.println("{file} {path.extension(file)} {path.parent(file)} {path.clean("a/./b/../c")}")
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.path` shows the same in a terminal.

- [Functions](#functions): [`join`](#join), [`parent`](#parent), [`fileName`](#filename), [`extension`](#extension), [`isAbsolute`](#isabsolute), [`clean`](#clean), [`relative`](#relative), [`matches`](#matches)

### Functions

#### join

```tondo
pub fn join(base: String, part: String): String
```

Adds a component, or gives the second path when it is absolute.

#### parent

```tondo
pub fn parent(path: String): String?
```

The path without its last component, or `none` when it has one alone:
`some("docs")` for `docs/guide.md`.

#### fileName

```tondo
pub fn fileName(path: String): String?
```

The last component, or `none` when there is none: `some("guide.md")` for
`docs/guide.md`.

#### extension

```tondo
pub fn extension(path: String): String?
```

Without the dot.

#### isAbsolute

```tondo
pub fn isAbsolute(path: String): Bool
```

Whether the path starts at the root, as `/tmp` does.

#### clean

```tondo
pub fn clean(path: String): String
```

The shortest path that names the same, lexically, as Go's `path.Clean`:
`docs/guide.md` for `docs//./api/../guide.md`, and `.` for an empty path.

#### relative

```tondo
pub fn relative(from: String, to: String): String?
```

The path from `from` to `to`, with `..` where it goes up: `../c/d` from
`a/b` to `a/c/d`; `none` when one is absolute and the other is not, or
`from` goes up past what is known of `to`.

#### matches

```tondo
pub fn matches(pattern: String, path: String): Bool
```

Whether the whole path matches a glob pattern: `*` and `?` within a
component, `[a-z]` and `[!a-z]`, `{md,markdown}`, `**` for any number of
components and `\` before a character to take it as itself. Every text
is a pattern: a `[` or `{` without its closing is itself.
