# Modules, packages and projects

A program that outgrows one file becomes a project: a directory whose
folders are modules. This chapter covers how code is divided, what one
module sees of another, dependencies, code for one operating system, files
kept inside the executable, and what gets built.

- [One file](#one-file)
- [A project](#a-project)
- [Modules](#modules)
- [Imports](#imports)
- [What a module shows: pub](#what-a-module-shows-pub)
- [Modules for the package alone: internal](#modules-for-the-package-alone-internal)
- [tondo.toml](#tondotoml)
- [Dependencies](#dependencies)
- [Libraries](#libraries)
- [Code for one system or architecture](#code-for-one-system-or-architecture)
- [Files inside the executable](#files-inside-the-executable)
- [Other kinds of source](#other-kinds-of-source)
- [What gets built](#what-gets-built)
- [Looking at a project](#looking-at-a-project)

## One file

A single file is a whole program. `tondo run tool.to` compiles and runs it,
`tondo build tool.to --output tool` leaves the executable, and with
`#!/usr/bin/env tondo` at its top it runs by its name
([Programs](01-programs.md)). The standard library is always there:
`import std.http` needs nothing installed. Most of this manual's examples
are single files.

## A project

`tondo new shop` makes one:

```text
shop/
  tondo.toml             the package: its name, its dependencies
  src/
    main.to              the module shop: where the program starts
    main_test.to         the tests of that module
  .gitignore
```

and a project that has grown looks like this:

```text
shop/
  tondo.toml
  tondo.lock             written by tondo: what each dependency resolved to
  src/
    main.to              module shop
    catalog/
      product.to         module shop.catalog
      search.to          module shop.catalog too: one module, two files
      search_test.to     its unit tests
    cart/
      cart.to            module shop.cart
    internal/
      pricing/
        pricing.to       module shop.internal.pricing: this package's alone
  tests/
    cart.to              an integration test: it sees only what is public
  libs/
    money/               a package of its own, used as a dependency
      tondo.toml
      src/money.to
```

The commands take a project's directory, or work on the one they are run
in: `tondo run`, `tondo test`, `tondo check`, `tondo build`.

## Modules

A module is a directory. Every `.to` file in it is part of the same module:
the files see each other's declarations, private ones included, with nothing
to import. How a module is split into files is a matter of taste that nobody
outside it can observe.

```text
// src/catalog/product.to
import money

/// Something the shop sells.
pub type Product = {
    name: String
    price: money.Cents
    priv cost: money.Cents
}

pub fn Product(name: String, price: Int): Product {
    Product { name, price: money.Cents(price), cost: money.Cents(price / 2) }
}
```

```text
// src/catalog/search.to
/// The products whose name has the text.
pub fn search(products: Array[Product], text: String): Array[Product] {
    products.filter((product) { product.name.contains(text) })
}

fn margin(product: Product): Int {
    product.price.value - product.cost.value        // the private field: same module
}
```

A module's name is its path: the package's name, then the directories under
`src`. `src/main.to`, or `main.to` at the project's root, is the module with
the package's own name.

A module declares things: types, functions, constants, contexts, traits and
their implementations. It runs nothing. Only the root file of a program may
have statements, and importing a module never executes code, so there is no
order of initialization to get wrong and nothing happens before `main`.

## Imports

```text
// src/cart/cart.to
import money
import shop.catalog
import shop.internal.pricing as tax

pub type Cart = { items: Array[catalog.Product] }

pub fn Cart.total(self): money.Cents {
    var sum = 0
    for item in self.items {
        sum += tax.withTax(item.price).value
    }
    money.Cents(sum)
}
```

- An import names a module by its full path, which starts with its package:
  `shop.catalog`, `std.http`, `money`.
- What it brings is used through the module's last name:
  `catalog.Product`, `http.App`. `as` gives it another, for a clash or a
  long name.
- There is no way to import a single name, or everything, into the file.
  A name with a dot before it came from an import, and one without was
  declared in this module or is the language's. Reading a file never
  requires knowing what another one exports.
- Imports go at the top of the file. One that is not used is a warning, and
  `tondo fmt` puts them in order.
- Modules cannot import each other in a circle:

```text
error[E1006]: modules cannot import each other in a cycle: `shop.cart` imports `shop.catalog`, which imports `shop.cart`
```

The prelude needs no import: `Int`, `String`, `Array`, `Option`, `Result`,
`Display`, `spawn` and the rest of what this manual has used without one.

## What a module shows: pub

Everything is private to its module unless it says `pub`.

| Declaration | Without `pub` | With `pub` |
| --- | --- | --- |
| a function, a constant, a context | its module only | any module that imports this one |
| a type | its module only | usable outside; a record's fields are public too |
| a field marked `priv` | | hidden even in a `pub` type |
| a method | its module only | callable outside |
| an enum's variants | | as public as the enum, all of them |

Capitals have nothing to do with it. A type's methods are declared in the
type's own module, and nowhere else: another module that wants to add
behaviour to a type writes a function that takes it, or implements a trait
of its own for it.

What a module exports is a promise. `pub` signatures state their types in
full, so that the promise can be read without the bodies: that is what
`tondo api shop.catalog` prints, and `tondo api --diff v1.2` says what a
change added to the promise, changed in it or took away.

## Modules for the package alone: internal

A module under a directory named `internal` can be imported only by its own
package. `shop.internal.pricing` is public to all of `shop` and invisible to
anything that depends on `shop`.

It is the place for what several modules of a package share and no user of
the package should rely on. Nothing marks it but where it lives, so moving a
module in or out of `internal` is how its audience is changed.

## tondo.toml

```toml
[package]
name = "shop"
version = "0.3.0"

[dependencies]
money = { path = "libs/money" }
http = { git = "https://github.com/acme/http", tag = "v1.2.0" }
zlib = { url = "https://example.com/zlib-1.0.tar.gz", sha256 = "..." }

[dev-dependencies]
fixtures = { path = "../fixtures" }

[native]
libraries = ["sqlite3"]
```

| Section | Says |
| --- | --- |
| `[package]` | the package's `name`, its `version`, and `systems` or `architectures` when it is for some only |
| `[dependencies]` | the packages it uses, each under the name it is imported by |
| `[dev-dependencies]` | those only its tests use |
| `[native]` | the C libraries its `extern fn`s link ([Unsafe and C](18-unsafe.md)) |
| `[derive]`, `[generators.x]` | programs that write code when it compiles ([Metaprogramming](17-meta.md)) |
| `[source]` | how to link to a line of its sources, on a host of its own |

A project needs only its name. A program reads its own name and version back
with `env.build()`, which also has the commit it was built from and its
dependencies: enough for a `--version`.

## Dependencies

A dependency is another package, taken from a directory, a git repository
or an archive:

```text
tondo add money --path libs/money
tondo add http --git https://github.com/acme/http --tag v1.2.0
tondo add zlib --url https://example.com/zlib-1.0.tar.gz --sha256 <hash>
tondo add fixtures --path ../fixtures --dev
tondo remove zlib
tondo update                        fetch again, and write tondo.lock again
```

- The name on the left is the name in the code: `import money`,
  `import http.client`. Two packages that call themselves the same can both
  be used, under two names.
- `tondo.lock` records exactly what each dependency resolved to, the commit
  of a tag and the hash of an archive. Commit it: a build from the same lock
  uses the same sources.
- Dependencies are fetched once into `~/.cache/tondo`, with the system's
  `git`, `curl` and `tar`.
- There is no central registry. A dependency is somewhere it can be fetched
  from, and the lock says what was fetched.

## Libraries

A package with no `main` and no statements is a library. `tondo check` and
`tondo test` work on it as on any project; only `run` and `build` need
something to run.

```text
// libs/money/src/money.to
/// An amount of money in cents.
pub type Cents = Int

/// The amount as text: `12.50`.
pub fn format(amount: Cents): String {
    "{amount.value / 100}.{amount.value % 100:02}"
}
```

A good library is small in what it exports, documents each `pub` name with
`///`, keeps its helpers in `internal`, and has its errors as its own types
([Errors](07-errors.md)). `tondo check --docs` lists what is public and
undocumented, and `tondo check --unreached` what nothing reaches.

## Code for one system or architecture

Code that differs between systems, or between processors, goes in files
whose names say which:

```text
src/watch/
  watch.to             what every target shares
  watch_linux.to       fn changes(path: String): ...   with inotify
  watch_macos.to       fn changes(path: String): ...   with FSEvents
  signals_unix.to      for both Linux and macOS
  watch_linux_test.to  tests of the Linux file

src/checksum/
  checksum.to          pub fn of(bytes: Array[Byte]): Int   calls add(bytes)
  checksum_x64.to      fn add(bytes: Array[Byte]): Int      for x86-64
  checksum_arm64.to    fn add(bytes: Array[Byte]): Int      for AArch64
```

A name ends in a system, `linux`, `macos` or `unix` for both; in an
architecture, `x64` or `arm64`; or in a system and then an architecture, as
`poll_linux_arm64.to`. The system goes first, and `tondo` refuses
`poll_arm64_linux.to` with the name to give it.

A build takes the files of its target, which is a system and an
architecture. The common code calls what each of them declares, and the
declarations must have the same signature in every target. There is no
`#if` and no flag inside a file: a file is for some targets or for all.

`tondo check` and the language server check every target, on any machine.
An error that only some would have names them, as `on macOS:`, `on arm64:`
or `on Linux arm64:`, so a change is not found broken later by someone
else. Here `checksum_arm64.to` has not been written yet:

```text
error[E1001]: on arm64: unknown name `add`; `add` is in `src/checksum/checksum_x64.to`, for x64: write it for arm64 too, in a file `*_arm64.to` or a common one, as `fn add(bytes: Array[Byte]): Int`
```

A package that is for some targets only says so in `tondo.toml`, with
`systems = ["linux"]` or `architectures = ["x64"]` in `[package]`, and is
checked and built for those alone. A program that needs to know where it
runs asks `env.build().target` or `env.system()`.

## Files inside the executable

`std.embed` reads files of the package while the program compiles and keeps
them in the executable:

```text
// src/main.to, beside src/pages/ and src/schema.sql
import std.embed
import std.fs
import std.http

const Schema: String = embed.text("schema.sql")
const Logo: Array[Byte] = embed.bytes("pages/logo.png")
const Pages: fs.FileSystem = embed.files("pages")

fn main(): !http.HttpError {
    var app = http.App()
    app.files("/", Pages)               // served from memory, with ETags and ranges
    app.listen(port: 8080)?
}
```

The path is a literal, relative to the file that names it. The files are
inputs of the build, so changing one rebuilds, and the executable needs
nothing beside it when it is deployed. A tree embedded with `embed.files` is
an `fs.FileSystem`, the same type a directory on disk or an archive has, so
code that reads one reads any.

## Other kinds of source

A `.proto` file among the sources is a module. `src/api/user.proto` is
`import shop.api.user`: its messages are records, its enums are enums, and
its services are gRPC's, with a trait for a server to implement and a client
to call it ([`std.grpc`](../std/grpc.md)).

Generators add modules too: a Tondo program named in `tondo.toml` runs
before the build and writes source from the package's own files and types
([Metaprogramming](17-meta.md)).

## What gets built

```text
tondo build --output shop           a native executable
tondo build --static --strip        static, on Linux, and without its symbols
tondo build --target aarch64-linux  for another architecture
tondo build --library               a library for C, with its header
tondo build --debug                 unoptimized, with a probe at each line, for the debugger
```

An executable is one file. It holds the program, the parts of the standard
library it uses and the runtime, and by default links only the C library of
the system. With `--static`, on Linux, it links nothing: it runs in an empty
container, and on any distribution.

A release build and a debug build mean the same thing: the same overflow
checks, the same order of evaluation, the same results. They differ in speed
and in how much the debugger can do.

`tondo run` builds too, into a cache keyed by the files it read, so running
the same code again starts at once. `tondo clean` empties it.

## Looking at a project

```text
tondo graph                         the modules, their files and what each imports
tondo graph --module shop.cart      what one module reaches; --reverse, what reaches it
tondo graph --mermaid               the same as a diagram
tondo api --list                    every module: the project's, its dependencies', std
tondo api shop.catalog              what a module exports, with its documentation
```

```text
shop  src/main.to
  -> money
  -> shop.cart
  -> shop.catalog
  -> std.console
shop.cart  src/cart/cart.to
  -> money
  -> shop.catalog
  -> shop.internal.pricing
shop.catalog  src/catalog/product.to src/catalog/search.to
  -> money
shop.internal.pricing  src/internal/pricing/pricing.to
  -> money
money  libs/money/src/money.to
```

Next: [Testing](16-testing.md).
