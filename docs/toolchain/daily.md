# From a new project to an executable

The commands of every day: making a project, checking it as it is written,
running it, and building what ships. Everything on this page was typed and
is shown with what it printed.

- [A new project](#a-new-project)
- [Check: the compiler as a reviewer](#check-the-compiler-as-a-reviewer)
- [Fix and format](#fix-and-format)
- [Explain, and look things up](#explain-and-look-things-up)
- [Run](#run)
- [Build](#build)
- [Dependencies](#dependencies)
- [What a project is made of](#what-a-project-is-made-of)
- [The cache](#the-cache)

## A new project

```text
$ tondo new words
made the project `words` in words: `tondo run words` runs it, `tondo test words` tests it

$ find words -type f
words/.gitignore
words/src/main.to
words/src/main_test.to
words/tondo.toml
```

Four files, and none of them is configuration to maintain. `tondo.toml` is
three lines:

```toml
[package]
name = "words"
version = "0.1.0"
```

`src/main.to` is a program with one function to test, and `main_test.to`
tests it:

```text
$ cd words
$ tondo run
Hello, world!

$ tondo test
test words::unit::words::greetsByName ... ok
ok. 1 passed; 0 failed; 0 skipped; 0 blocked; 0.01s
```

`tondo init` does the same in a directory that already exists. A program
does not need a project at all: `tondo run file.to` runs one file, and a
directory with a `main.to` in it is a project too
([Modules, packages and projects](../language/15-modules.md)).

## Check: the compiler as a reviewer

`tondo check` reports what is wrong without building anything, in the time
it takes to save a file. Here is a first draft of the program, with two
mistakes that are easy to make coming from another language:

```text
fn main() {
    let path = env.args().get(0)
    let text = fs.readText(path)
    for (word, times) in count(text) {
        console.println("{word} {times}")
    }
}
```

```text
$ tondo check
error[E1102]: expected `String`, found `String?`, which may be `none`: give a fallback with `value ?? fallback`, handle it with `match`, or pass it on with `?` in a function that returns an option
  --> src/main.to:16:28
   |
16 |     let text = fs.readText(path)
   |                            ^^^^
error[E1102]: expected `String`, found `String ! fs.FsError`: add `?` to pass the failure on, give a fallback with `value ?? fallback`, or handle it with `match`
  --> src/main.to:17:32
   |
17 |     for (word, times) in count(text) {
   |                                ^^^^
```

Each diagnostic has a code, the place, what was expected and what was
found, and then what to write. That last part is deliberate: an error that
only says what is wrong leaves the reader, or the model, to guess the fix.

- `tondo check --json` gives the same as data, with each diagnostic's
  range, a stable `id`, and its `fixes` as edits. Editors and agents read
  that ([Editors and agents](editors-and-agents.md)).
- `tondo check --unreached` lists the code that neither `main` nor any test
  reaches. On the finished program it prints `main and the tests reach all
  the code`.
- `tondo check --docs` lists what is public and has no documentation, and
  `--performance` a copy that a loop makes on every turn.
- A project with files for one system or one architecture, as
  `watch_macos.to` or `checksum_arm64.to`, is checked for every target it
  is for, on any machine: an error that another target would have shows as
  `on macOS: ...` or `on arm64: ...`
  ([Modules](../language/15-modules.md#code-for-one-system-or-architecture)).
- Warnings are few and each means something: an unused import, a result
  that is dropped, a `spawn` in a loop that waits for each task.

## Fix and format

Some mistakes have one right repair, and `tondo fix` makes it. This draft
has habits from three languages in one line, and an import nobody uses:

```text
import std.json

pub fn count(text: String): Map[String, Int] {
    let mut counts: Map[String, Int] = [:];
```

```text
$ tondo check
error[E0004]: `let mut` declares nothing in Tondo: a variable is `var name = value`
 --> src/main.to:8:9
  |
8 |     let mut counts: Map[String, Int] = [:];
  |         ^^^
error[E0002]: `;` is not a Tondo token: a new line ends a statement
 --> src/main.to:8:43
  |
8 |     let mut counts: Map[String, Int] = [:];
  |                                           ^

$ tondo fix
fixes applied: 1
error[E0004]: `let mut` declares nothing in Tondo: a variable is `var name = value`
 --> src/main.to:8:9
  |
8 |     let mut counts: Map[String, Int] = [:]
  |         ^^^
```

The semicolon is gone. `let mut` is left for the author, since the message
already says what to write and a tool should not guess at names. With that
changed by hand, the next round finds the import:

```text
$ tondo check
warning[W1001]: `json` is imported but never used: remove the import
 --> src/main.to:4:1
  |
4 | import std.json
  | ^^^^^^^^^^^^^^^

$ tondo fix
fixes applied: 1
```

`tondo fmt` writes a file in the one layout there is. It has no options, so
there is nothing to configure and nothing to argue about:

```text
$ cat ugly.to
import std.console

fn main() {
  let   total=1+2
      console.println( "{total}" )
}

$ tondo fmt ugly.to
formatted ugly.to

$ cat ugly.to
import std.console

fn main() {
    let total = 1 + 2
    console.println("{total}")
}
```

`tondo fmt --check` fails when a file is not formatted, for a CI job, and
`tondo fmt --compact` prints a file in fewer tokens, for a model's context.

## Explain, and look things up

Every code has an explanation, and with it what the guide says of the
mistake, with an example where the guide has one:

```text
$ tondo explain E0002
E0002 invalid-token (error)

A character or sequence that is no token of the language.

From `tondo guide pitfalls`:

No semicolons; a newline ends a statement.

    fn main() {
        let x = 1;
    }

The diagnostic itself says what to write; `tondo check --json` gives its repairs as data, and `tondo fix` applies the safe ones.
```

A code with more than one cause, as `E1303` for a result that is dropped
and for a task whose failure nobody would read, shows each with its
example.

The standard library's documentation is in the binary. `tondo api` gives a
module, a type or one function, and `--search` finds what does a job:

```text
$ tondo api std.fs.readText
/// The content of the file as text; one that is not UTF-8 is
/// `FsError.InvalidData`.
pub fn readText(path: String): String ! FsError

$ tondo api --search sha256
std.hash.sha256
    pub fn sha256(data: Array[Byte]): Digest
    The SHA-256 digest of the data.
std.hash.hmacSha256
    pub fn hmacSha256(key: Array[Byte], data: Array[Byte]): Digest
    The HMAC of the data with the key (RFC 2104), on SHA-256.
std.crypto.verifyPassword
    pub fn verifyPassword(password: String, hash: String): Bool
    Whether `password` is the one `hash` was made from.
std.hash.Digest
    pub type Digest =
    The digest of some data.
std.hash.Hasher
    pub type Hasher =
    A digest or an HMAC computed in parts, without the data together: each part to `update`, then `finish`.
std.meta.Input
    pub type Input =
    A file a generator is given: its name in the manifest, its bytes and their SHA-256.
`tondo api <name>` shows one, with its methods and an example, as `tondo api std.hash.sha256`
```

`tondo api` works on a project's own modules too, and `tondo api --diff
<commit>` says how a package's public API changed since a commit.
`tondo guide` is the language on a few pages, a topic at a time, and
[the standard library](../std/README.md) has all of `tondo api` as pages.

## Run

```text
$ tondo run
to 2
be 2
or 1
not 1

$ tondo run -- other.txt
error: not found
$ echo $?
1
```

`tondo run` compiles, keeps the build, and runs. A second run of the same
code starts at once, since the build is found by the content of the files
it was made from, and says the code's warnings again: a warning stays until
the code changes. What follows `--` goes to the program.

A `main` that returns `!E` and fails ends the program with the error and a
status of 1, as above. A panic prints its message, its code and the stack.

| | |
| --- | --- |
| `tondo run file.to`, `tondo file.to` | one file, with or without a `main` |
| `tondo run --watch` | runs again, or reloads in place, as the files are saved |
| `tondo run --live` | a build whose code `tondo reload` can change as it runs |
| `tondo run --debug` | a build for the debugger |
| `tondo run --record run.rec` | keeps the run, to debug it again and backwards |
| `tondo run --static` | linked statically |

[Debugging](debugging.md) and [A running program](observing.md) are about
the last four.

## Build

```text
$ tondo build
$ ls -l words
-rwxr-xr-x  2941048  words

$ tondo build --output words-static --static --strip
$ file words words-static
words:        ELF 64-bit LSB pie executable, x86-64, dynamically linked, not stripped
words-static: ELF 64-bit LSB pie executable, x86-64, static-pie linked, stripped
```

One file, under three megabytes, with the scheduler, the memory manager
and the part of the standard library the program uses inside it. It needs
nothing installed where it runs. The static one needs not even a C
library, which makes it the content of a container image from `scratch`.

| | |
| --- | --- |
| `--output <path>` | where to write it; the package's name by default |
| `--static` | no shared libraries at all |
| `--strip` | without its symbol table |
| `--debug` | with what the debugger needs |
| `--target aarch64-linux` | for another architecture |
| `--cpu x86-64-v3` | for a level of processor above the baseline |
| `--no-logpoints` | without the place each line keeps for a [logpoint](observing.md#what-a-line-holds-logpoint) |
| `--no-lifetimes` | without [regions](performance.md#what-a-region-takes), when a release build gives [another result](debugging.md#a-release-build-that-gives-another-result) |
| `--link <library>` | a C library to link, for `extern fn` |
| `--library` | a library for C instead: `lib<name>.so`, `lib<name>.a` and a header |

Building for another architecture needs that architecture's runtime. A
`tondo` that does not carry it says so, and how to get it:

```text
$ tondo build --target aarch64-linux
tondo: building for aarch64-linux needs its runtime, which this tondo does not carry: write it with `tondo target pack <dir>` of a tondo for aarch64-linux, and set TONDO_TARGETS to <dir>
```

A build is for a level of processor, and for the same one wherever it is
made: `baseline`, which is x86-64-v2 or ARMv8.0. `--cpu x86-64-v3`, or `cpu`
in the `[build]` of `tondo.toml`, lets the code use more, and the program
then checks its processor when it starts
([Performance](performance.md#the-processor-a-build-is-for)).

Tondo links its own executables, so a build needs no C compiler and no
linker on the machine. The [`textkit`](../../examples/textkit) example
builds a library that a C program links.

The same files, built again by the same `tondo` with the same options, are
the same bytes: two executables that differ were not made alike. Two things
count as what is built. A debug build holds the directory of its sources,
where the debugger finds them, and on macOS the signature of an executable
holds its file's name. A [live build](observing.md#changing-it-while-it-runs)
is never the same twice, since each carries the identity of when and where
it was made, by which `tondo reload` finds it.

## Dependencies

A dependency is another package: a directory beside this one, a git
repository at a tag, or an archive at a URL with its hash.

```text
$ tondo add textutil --path ../textutil
added `textutil` to [dependencies]: `import textutil` uses it

$ cat tondo.toml
[package]
name = "words"
version = "0.1.0"

[dependencies]
textutil = { path = "../textutil" }
```

Its modules are imported by the name it was given:

```text
import textutil.clean

    for each in text.words() {
        let word = clean.word(each)
```

| | |
| --- | --- |
| `tondo add <name> --path <dir>` | a package on this machine |
| `tondo add <name> --git <url> --tag v1.2.0` | a repository, at a tag, a branch or a commit |
| `tondo add <name> --url <url> --sha256 <hash>` | an archive |
| `tondo add ... --dev` | for the tests alone |
| `tondo remove <name>` | takes one out |
| `tondo update` | fetches them again, and writes `tondo.lock` again |

`tondo.lock` records what each git or URL dependency resolved to, the
commit of a tag and the hash of an archive, so that a build from the same
lock is a build of the same code. There is no registry and no version
solver: a dependency is named by where it is.

### In a script

A script is one file, with no `tondo.toml` to write its dependencies in. It
names them itself, in comments at its top, one a line. The same commands
write them, given the script where they take a project:

```text
$ tondo add greet --git file:///tmp/tondo-script/greet --tag v1.0.0 --project hello.to
fetching greet: git file:///tmp/tondo-script/greet, tag v1.0.0
added `greet` to the script's header: `import greet` uses it

$ cat hello.to
#!/usr/bin/env tondo

// tondo: dependency greet = { git = "file:///tmp/tondo-script/greet", tag = "v1.0.0" }

import greet
import std.console
import std.env

for name in env.args() {
    console.println(greet.hello(name))
}

$ tondo hello.to ada bo
hello, ada
hello, bo
```

- A line is `// tondo: dependency <name> = <source>`, with the source as
  `tondo.toml` writes it: a `git` with a `tag`, a `branch` or a `rev`, a
  `url` with its `sha256`, or a `path`, which is counted from the script's
  directory.
- The lines go before the script's code, after its `#!` line if it has one.
  One further down is not read, and the compiler says so (`E0009`); so is
  one in a file of a project, where the error gives the `tondo add` that
  writes it in `tondo.toml`. `tondo fmt` leaves them where and as they are.
- What they resolved to is kept beside the script, in a file of its name
  and `.lock`: `hello.to.lock`. It is found by where the script is, not by
  where it is run from, so a script on the `PATH` uses its own. Keep the
  two together, in a repository or wherever the script is installed.
- A package is fetched the first time, with a line that says so, as above;
  with the lock and the cache in place a run fetches nothing and says
  nothing.
- `tondo remove greet --project hello.to` takes a line out, and
  `tondo update hello.to` fetches again and writes the lock again.

A script installed where its user cannot write, without its lock, still
runs. It resolves its dependencies each time and says why:

```text
warning: cannot write hello.to.lock (Permission denied): the dependencies were resolved for this run only; install the script with its lock beside it to fix them
```

A script has dependencies and nothing else of a project: none for tests
alone, no C libraries, no settings of the build. One that needs those is a
project, which `tondo new <name>` makes, with the script as its
`src/main.to` and its dependencies in its `tondo.toml`; the commands say so
when asked for one of them on a script.

## What a project is made of

```text
$ tondo graph
words  src/main.to
  -> std.console
  -> std.env
  -> std.fs
  -> textutil.clean
textutil.clean  textutil/src/clean/clean.to
std.console
std.env
std.fs
```

`tondo graph --packages` gives the packages alone, `--files` every file,
`--module <name> --reverse` who imports a module, and `--mermaid` or
`--dot` a diagram:

```text
$ tondo graph --mermaid
flowchart LR
    subgraph p0["words"]
        n0["words"]
    end
    subgraph p1["textutil"]
        n1["textutil.clean"]
    end
    subgraph p2["std"]
        n2["std.console"]
        n3["std.env"]
        n4["std.fs"]
    end
```

`tondo expand` shows what each `derive` and generator of a program wrote,
as source ([Metaprogramming](../language/17-meta.md)).

## The cache

Builds are kept in the user's cache directory, by the content of what they
were made from. Nothing is written into the project but what `build` is
asked for. `tondo clean` empties the cache, and with `--all` the
dependencies that were fetched too.

The cache keeps itself small. A build that nobody used in 30 days goes.
Of a program that is built again and again, only its last build and the
one before stay. And once a day, while the builds together hold more than
10 GB, the one that nobody used for the longest goes, until they fit.
`TONDO_CACHE_LIMIT`, in bytes or with a unit, sets another limit:

```text
TONDO_CACHE_LIMIT=40G tondo test
```

The dependencies that were fetched and the runtime do not count towards
it: a build makes the rest again, and those cost a download or a long
build. `tondo clean --list` says what the cache holds, part by part, and
empties nothing:

```text
$ tondo clean --list
the cache in /home/ada/.cache/tondo holds 88.84 MiB
  86.04 MiB  runtime         the runtime, built once for each toolchain
   2.80 MiB  run             builds of programs, toward the limit
the builds hold 2.80 MiB of a limit of 10.00 GiB (TONDO_CACHE_LIMIT): once a day the least used past it go
```

Next: [Testing](testing.md).
