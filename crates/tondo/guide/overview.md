# Tondo

Tondo is a statically typed, compiled language with value semantics, errors
in signatures (`T ! E`), tasks without colored functions, and one canonical
form. Files end in `.to`. This guide is for people and for models; each
topic is short and every example compiles.

## A program

```tondo
import std.console

pub type User = { name: String, age: Int }

fn greet(user: User): String {
    "Hello, {user.name}!"
}

fn main() {
    let ada = User { name: "Ada", age: 36 }
    console.println(greet(ada))
}
```

`tondo run hello.to` compiles and runs it.

## A script

A file needs no `main`: its statements run in order, from the top.

```tondo
#!/usr/bin/env tondo
import std.console
import std.env

fn shout(text: String): String {
    "{text.toUpper()}!"
}

for argument in env.args() {
    console.println(shout(argument))
}
```

`tondo shout.to a b` runs it with those arguments, and so does `./shout.to
a b` after `chmod +x`. Functions and types may be declared anywhere in it,
but functions do not see the script's bindings: pass them as arguments.
`?` and `fail` at the top level end the script with that error, as a
failing `main` does. A project's `src/main.to` may be a script too; a file
with statements cannot be imported.

## Rules that differ from other languages

- No semicolons: a newline ends a statement. A line continues after `,` `.`
  `=` an operator, or inside `()` and `[]`.
- `let` is immutable, `var` is mutable; a binding never shadows another
  visible one (`E1003`): pick a new name.
- The last expression of a block is its value; `return` is for early exits.
- Errors are values in the signature: `fn load(): Config ! LoadError`; `?`
  propagates and `fail Error.Variant` fails. `T?` is an optional value with
  `some(x)` and `none`; results are `ok(x)` and `err(e)`. A bug panics;
  `recover { ... }` stops a panic, as a `T ! Panic`, at the edge of a
  request or a job.
- What belongs to the work that runs, as the request or the trace, goes in
  a `context Request = { id: String = "" }`: any function reads
  `Request.id`, and `with Request { id: x }` changes it until the block
  ends. Nothing passes it as a parameter.
- Strings interpolate with `{expr}`; `{{` is a literal brace. `+` does not
  join strings.
- Methods are declared outside the type: `fn User.greet(self): String`.
  `mut self` changes fields, `var self` can replace the value.
- A trait method on a concrete value is called through the trait:
  `Display.display(user)`; inside generic code, `value.display()`.
- `match`, `test`, `derive`, `with`, `as` and `select` are keywords, and
  `recover` before `{` and `context` before a declaration's name. There is no `if let`, no `|` in patterns and no
  `null`.
- Collections are values: assigning copies (cheaply). `push` needs `var`.
- `tondo fmt` decides the layout: write compactly and let it format.

## Commands

```text
tondo new <name>                   a project: tondo.toml, src/main.to, a test
tondo run [<file.to>|<project>]    compile and run
tondo check [...] [--json]         diagnostics only; JSON for tools
tondo fix [...]                    apply the compiler's safe repairs
tondo explain E1102                what a code of an error or a panic means
tondo test [<project>]             run the tests
tondo fmt <file.to>                canonical layout; --compact prints the fewest tokens
tondo api <module>                 a module's declarations and docs, no bodies:
                                   std.http, std.fs, prelude, app.models, a dependency
tondo api --list                   the modules there are
tondo graph [<path>]               a program's modules, files and imports; with a
                                   directory or file, its project and module
tondo add <alias> --path <dir>     a dependency (or --git <url> --tag <t>, --url <url>)
tondo expand                       what each `derive` wrote
tondo guide <topic>                one topic of this guide
```

`tondo <command> --help` explains a command; with `--json`, a command
writes its result as one JSON object.

Every error has a code and says how to fix it, often with the exact code to
write; `tondo check --json` gives the same as data with spans, and many
errors' repairs as edits (`fixes`), `safe` ones ready to apply.

## Working as an agent

1. `tondo api --list`, then `tondo api <module>` to learn what to call;
   `tondo api prelude` for the methods of `String`, `Array`, `Map`, `Option`.
2. Write the code; layout does not matter.
3. `tondo check --json`, fix what it reports, repeat; then `tondo test`.
4. `tondo fmt` the files you wrote. To show a file to a model, `tondo fmt
   --compact` uses about 15% fewer tokens.

`tondo mcp` gives an agent all of this as MCP tools, and more: applying
the fixes, navigating, running with input, testing, observing and debugging
programs.

## Setting up

`tondo setup` readies the machine, and again updates it: it checks that
`tondo` runs a program, gives Claude Code and Codex this skill and `tondo
mcp`, and installs the Tondo extension in VS Code, Cursor, VSCodium and
Windsurf, which then reload their windows. Each line it prints says what it
did or what is missing and how to get it; `--json` says the same as data.
Without `tondo`, `sh dist/install.sh` in a clone of Tondo's repository
installs it and runs `tondo setup`. A new session of an agent loads the
skill and the server.

## Topics

```text
syntax       declarations, control flow, match, closures, loops
types        records, enums, newtypes, unions, generics, traits
errors       T ! E, ?, fail, Option, panics, recover, main's errors
collections  Array, Map, Set, Deque, ranges, iterators
text         strings, characters, bytes, formatting
tasks        spawn, Group, channels, select, cancellation, std.sync, contexts
projects     tondo.toml, modules, imports, dependencies, native libraries
testing      test, suite, std.testing, snapshots, tondo test
stdlib       the standard modules and what each is for
http         HTTP client and server, typed handlers, OpenAPI, WebSockets
meta         derive, reflection, generators
ffi          calling C: extern fn, unsafe
pitfalls     mistakes models make in Tondo, with the error each gives
```
