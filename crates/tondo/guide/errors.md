# Errors

Recoverable errors are values in the signature; programming errors panic.

```tondo
import std.console

pub enum ConfigError {
    Missing(String)
    Invalid(String)
}

/// `Display` gives the error its message, also when `main` fails with it.
impl Display for ConfigError {
    fn display(self): String {
        match self {
            ConfigError.Missing(key) => "missing `{key}`"
            ConfigError.Invalid(key) => "`{key}` is not valid"
        }
    }
}

fn port(text: String): Int ! ConfigError {
    if text.isEmpty() {
        fail ConfigError.Missing("port")
    }
    match Int.parse(text) {
        ok(value) => value
        err(_) => fail ConfigError.Invalid("port")
    }
}

/// `?` returns the error to the caller; the success value is unwrapped.
fn address(host: String, text: String): String ! ConfigError {
    let number = port(text)?
    "{host}:{number}"
}

fn main() {
    console.println(address("localhost", "8080")?)
    match port("") {
        ok(value) => console.println("{value}")
        err(error) => console.println("error: {error}")
    }
}
```

- `T ! E`: a function that returns `T` or fails with `E`. Its body's value is
  the success; `fail error` ends it with the error. In a function that
  declares an enum as its error, `fail Missing("port")` names its variant
  alone.
- `?` on a `T ! E` gives the `T` or returns the error; on a `T?` in a function
  that returns an option, it gives the value or returns `none`.
- Different error types: declare a union, `fn run(): !(ConfigError |
  NetworkError)`, and `?` widens into it.
- `value ?? fallback` gives what an option or a result holds, or else the
  fallback, which runs only then and may leave: `ages[name] ?? 0`,
  `users.get(id) ?? fail ApiError.NotFound(id)`, `?? return`, `?? continue`.
  After an option, an optional fallback keeps it, so `flag ?? env ?? 8080`
  chains. It binds least of all: `(name ?? "").trim()`.
- Convert with `result.mapErr((e) { ... })`, and turn an option into a result
  with `option.okOr(error)`.
- `main` fails with what its `?` and `fail` propagate, with no error to
  declare, as a script's top level does. The program then prints `error: `
  and the error's `Display` (or its variant), and exits with status 1;
  `process.exit(code)` ends it with another status. Any other function
  declares what it propagates, which `E1301` writes for it.

## What the program was doing: `Failure`

Typed errors are for callers that tell them apart. Inside a program, it is
often enough to say what failed and keep the cause: `.context("...")` turns
any error, or a missing value, into the prelude's `Failure`.

```tondo
import std.console
import std.fs
import std.json

type Config = { port: Int }

fn load(path: String): Config ! Failure {
    let text = fs.readText(path).context("reading {path}")?
    json.decode[Config](text).context("decoding {path}")?
}

fn main() {
    match load("/missing/app.json").context("starting") {
        ok(config) => console.println("{config.port}")
        err(failure) => {
            console.println("{failure}")      // starting: reading /missing/app.json: not found
            console.println("{failure.find[fs.FsError]() == fs.FsError.NotFound}")
        }
    }
}
```

`failure.find[E]()` gives the nearest cause of type `E` through the chain,
as Go's `errors.As`; `Failure { message: "..." }` is one with no cause.

## Optional values

```tondo
import std.console

fn find(names: Array[String], wanted: String): Int? {
    for (index, name) in names.enumerate() {
        if name == wanted {
            return index
        }
    }
    none
}

fn main() {
    let names = ["ada", "bo"]
    console.println("{find(names, "bo")} {find(names, "cy") ?? -1}")
    match find(names, "ada") {
        some(index) => console.println("at {index}")
        none => console.println("absent")
    }
}
```

`?` leaves the function, but on an option `?.` goes on only when there is a
value, and gives `none` when there is not: a chain of options needs no
function of its own. `(value?).field` leaves the function instead, and on a
result `?.` propagates the error as `?` does:

```tondo
type Address = { city: String? }
type User = { address: Address? }

fn cityOf(user: User): String {
    user.address?.city ?? "nowhere"
}

fn main() {}
```

## Panics

Overflow, an index out of range, division by zero, `assert(false)` and
`panic("message")` panic: the program stops with a code like `P0005` and the
place. Use them for bugs, not for input: check input and return an error.
`array.get(i)` gives an option where `array[i]` would panic.

A panic says what failed, with the values, and how the program got there:

```text
panic[P0001]: bounds: index 5 is out of bounds for an array of length 3: valid indices are 0 to 2, and -3 to -1 from the end
 --> src/main.to:6:5
stack:
  Cell.display       src/main.to:6:5
  depth              src/main.to:22:5  (3 times)
  closure in worker  src/main.to:29:17
  worker             src/main.to:29:5
in a task spawned at:
  main  src/main.to:38:15
```

Read the stack from the top: the first line is where it failed, each next one
the call that led there; `in a task spawned at:` continues in the task that
started this one. `assert(a == b)` shows both sides (`left:` and `right:`). A
deadlock lists where every task waits, and a stack overflow or a crash in C
prints `fatal:` with the same trace.

A panic can carry the program's own code, dotted words such as
`cache.corrupt`, and data, plain values by name, which its report shows:

```tondo
fn main() {
    let key = "users"
    panic("the cache index is corrupt", code: "cache.corrupt", data: ["key": key, "entries": 12])
}
```

With `TONDO_PANIC_FORMAT=json`, the program writes how it failed, a panic,
`main`'s error or a crash, as one line of JSON on standard error, for tools;
the MCP tool `run` gives it as data.

`recover { ... }` stops a panic of its block, of what it calls and of the
tasks it starts: it is a `T ! Panic`, `ok` with the block's value or `err`
with the panic's `code`, `message`, `data`, `location` and `stack`. On the
way, `defer`s run, what was held is released, `std.sync` locks are let go
and the block's other tasks are cancelled; `p.resume()` lets the panic go
on. Recover at the edge of a request or a job, not around each call: a
panic is still a bug, and `std.http` already answers `500` for a handler
that panics. A deadlock, a crash, `testing.skip` and `testing.failNow` are
never stopped.

```tondo
import std.console

fn main() {
    let values = [3, 4]
    let third = recover { values[2] } ?? 0
    console.println("{third}")
    match recover { panic("broken", code: "cache.corrupt") } {
        err(p) if p.code == "cache.corrupt" => console.println("rebuild: {p.message}")
        err(p) => p.resume()
    }
}
```
