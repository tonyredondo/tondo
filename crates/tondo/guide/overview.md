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
  ends. Nothing passes it as a parameter. What the whole program shares, as
  a compiled pattern or a cache, is a field whose initial value is computed
  once, when first read: `context Patterns = { words: Regex =
  Regex.compile(r"\w+")? }`. There are no global variables.
- Strings interpolate with `{expr}`, formatted as `{price:.2}`, `{id:08}`
  or `{name:<12}`; `{{` is a literal brace. `+` does not join strings.
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
tondo api <module>.<Type>          one type, its methods and an example: String,
                                   Option, std.http.Client (or http.Client)
tondo api --list                   the modules there are
tondo api --search <words>         declarations by their names and docs
tondo graph [<path>]               a program's modules, files and imports; with a
                                   directory or file, its project and module
tondo add <alias> --path <dir>     a dependency (or --git <url> --tag <t>, --url <url>)
tondo expand                       what each `derive` wrote
tondo guide <topic>                one topic of this guide
```

`tondo guide cli` goes through every command; `tondo <command> --help`
explains one, and with `--json` a command writes its result as one JSON
object. When something goes wrong, `tondo guide runbooks` says what to do step
by step: a crash, a dump, a hang, a change to a running program.

Every error has a code and says how to fix it, often with the exact code to
write; `tondo check --json` gives the same as data with spans, and many
errors' repairs as edits (`fixes`), `safe` ones ready to apply.

## Working as an agent

1. Look before you call: Tondo's library is its own, and guessing a name
   from another language is the commonest mistake. `tondo api --search
   <words>` finds what does a job; `tondo api std.http.Client`, or
   `String`, `Array`, `Option`, shows a type with its methods and an
   example that compiles, which every type of the standard library has;
   `tondo api <module>` shows a whole module, `--list` the modules. An
   error about a name that does not exist says which `tondo api` to run.
2. Write the code; layout does not matter.
3. `tondo check --json`, fix what it reports, repeat; then `tondo test`.
4. `tondo fmt` the files you wrote. To show a file to a model, `tondo fmt
   --compact` uses about 15% fewer tokens.

`tondo mcp` gives an agent all of this as MCP tools, and more: applying
the fixes, navigating, running with input, testing, observing and debugging
programs; `tondo guide mcp` goes through every tool.

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
models       std.model: every state of a design or of the code, and what must hold
stdlib       the standard modules and what each is for
http         HTTP client and server, typed handlers, OpenAPI, WebSockets, GraphQL, gRPC
templates    text and HTML templates checked against their data
databases    SQLite, PostgreSQL, MySQL: queries, rows, transactions, migrations
meta         derive, reflection, generators, annotations
ffi          calling C: extern fn, unsafe
cli          every tondo command: write, run, test, observe, debug
mcp          every tool of tondo mcp, for agents
pitfalls     mistakes models make in Tondo, with the error each gives
```
