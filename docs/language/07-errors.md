# Errors

Tondo keeps two things apart that many languages mix. What can go wrong in a
correct program, as a file that is not there, is an error: a value, in the
signature, that the caller has to deal with. What cannot happen unless the
program is wrong, as an index past the end, is a panic. This chapter covers
both, and the value that may be missing.

- [A value that may be missing: T?](#a-value-that-may-be-missing-t)
- [A function that can fail: T ! E](#a-function-that-can-fail-t--e)
- [Passing a failure on with ?](#passing-a-failure-on-with-)
- [Several ways to fail](#several-ways-to-fail)
- [Working with options and results](#working-with-options-and-results)
- [A fallback and a chain: ?? and ?.](#a-fallback-and-a-chain--and-)
- [Saying what the program was doing: Failure](#saying-what-the-program-was-doing-failure)
- [Designing a module's errors](#designing-a-modules-errors)
- [Panics](#panics)
- [assert](#assert)
- [recover](#recover)
- [Error or panic](#error-or-panic)

## A value that may be missing: T?

There is no `null`. A value that may not be there has the type `T?`, which
is `some(value)` or `none`, and the code cannot use it as a `T` without
looking:

```tondo
import std.console

type User = { name: String, email: String? }

fn find(users: Array[User], name: String): User? {
    for user in users {
        if user.name == name {
            return user
        }
    }
    none
}

fn main() {
    let users = [
        User { name: "ada", email: "ada@example.com" },
        User { name: "bo", email: none },
    ]
    match find(users, "ada") {
        some(user) => console.println("found {user.name}")
        none => console.println("nobody")
    }
    let email = find(users, "bo")?.email ?? "no email"
    console.println(email)
    console.println("{find(users, "cy").isNone()} {users.get(5).isSome()} {users.first()?.name}")
}
```

```text output
found ada
no email
true false some(ada)
```

Where a `T?` is expected, a plain value is taken as `some` of it: the
literal above gives `email: "ada@example.com"`, not `some(...)`. `T?` is
`Option[T]`, and the library uses it wherever an answer may not exist:
`array.get(i)`, `map.get(key)`, `text.find(needle)`, `env.get(name)`.

## A function that can fail: T ! E

A function that can fail says so after `!`, with the type of what it fails
with. Its body gives the success as usual, and `fail` leaves with an error:

```tondo
import std.console

pub enum AgeError {
    NotANumber(String)
    Negative(Int)
    TooOld(Int)
}

impl Display for AgeError {
    fn display(self): String {
        match self {
            NotANumber(text) => "`{text}` is not a number"
            Negative(age) => "{age} is negative"
            TooOld(age) => "nobody is {age}"
        }
    }
}

fn parseAge(text: String): Int ! AgeError {
    let age = Int.parse(text.trim()) ?? fail NotANumber(text)
    if age < 0 {
        fail Negative(age)
    }
    if age > 150 {
        fail TooOld(age)
    }
    age
}

fn main() {
    for text in ["36", "x", "-4", "999"] {
        match parseAge(text) {
            ok(age) => console.println("{age} years")
            err(error) => console.println("error: {error}")
        }
    }
}
```

```text output
36 years
error: `x` is not a number
error: -4 is negative
error: nobody is 999
```

- `T ! E` is `Result[T, E]`: `ok(value)` or `err(error)`. `!E` alone is a
  function that gives nothing and can fail.
- Any type can be an error. An enum is the usual one, since its variants are
  the ways to fail and `match` makes the caller see each.
- `Display` gives the error its message, and is what `main` prints when it
  fails with it.
- `fail error` is to errors what `return value` is to success. `err(error)`
  builds a result as a value, to store it or return it from a `match`.
- After `fail`, and in a pattern, a variant is written alone: the signature
  says which enum the error is, so `fail Negative(age)` is
  `fail AgeError.Negative(age)`. Where no error is declared, as in `main`,
  the variant takes its enum's name.

The caller cannot forget the failure. A result that is not used is a compile
error, and so is using a `T ! E` where a `T` goes:

```text
error[E1303]: a value of type `String ! fs.FsError` is not used; pass a failure on with `?`, handle it with `match`, or discard it with `_ = ...`
error[E1102]: expected `Int`, found `Int ! text.TextError`: add `?` to pass the failure on, give a fallback with `value ?? fallback`, or handle it with `match`
```

## Passing a failure on with ?

Most functions do not handle the errors of what they call: they pass them
on. `?` after a result gives its value, or returns its error from the
function:

```tondo
import std.console
import std.fs

fn lineCount(path: String): Int ! fs.FsError {
    let text = fs.readText(path)?
    text.lines().length()
}

fn main(): !fs.FsError {
    let notes = fs.createTemporaryFile()?
    fs.writeText(notes, "one\ntwo\n")?
    for path in [notes, "/missing/file"] {
        match lineCount(path) {
            ok(count) => console.println("{count} lines")
            err(error) => console.println("cannot read it: {error}")
        }
    }
    fs.remove(notes)?
}
```

```text output
2 lines
cannot read it: not found
```

On an option, `?` gives the value or returns `none`, in a function that
itself returns an option:

```tondo
import std.console

type Address = { city: String? }
type User = { name: String, address: Address? }

fn cityOf(user: User): String? {
    let address = user.address?
    let city = address.city?
    city.toUpper()
}

fn main() {
    let ada = User { name: "ada", address: Address { city: "london" } }
    let bo = User { name: "bo", address: none }
    console.println("{cityOf(ada)} {cityOf(bo)}")
}
```

```text output
some(LONDON) none
```

`?` leaves the nearest function or closure, and only through the channel
that function declares: a `?` on a result needs a function that can fail
with that error. When it cannot, the error says what to declare:

```text
error[E1301]: `load` cannot fail, but this propagates `ParseError | fs.FsError`: declare its result as `Int ! (ParseError | fs.FsError)`
```

`tondo fix` writes that signature.

## Several ways to fail

A function that calls two things that fail differently fails with either.
Its error is a union, and `?` puts each error in without conversion:

```tondo
import std.console
import std.fs

enum ParseError {
    Empty
    NotANumber(String)
}

fn parse(text: String): Int ! ParseError {
    if text.isEmpty() {
        fail Empty
    }
    Int.parse(text).mapErr((_) { ParseError.NotANumber(text) })
}

fn load(path: String): Int ! (fs.FsError | ParseError) {
    let text = fs.readText(path)?           // an FsError
    parse(text.trim())?                     // a ParseError
}

fn main() {
    match load("/missing/number.txt") {
        ok(value) => console.println("{value}")
        err(fs.FsError(error)) => console.println("cannot read it: {error}")
        err(ParseError(error)) => match error {
            Empty => console.println("the file is empty")
            NotANumber(text) => console.println("not a number: {text}")
        }
    }
}
```

```text output
cannot read it: not found
```

The union needs parentheses, `Int ! (A | B)`. A union of errors widens into
a larger one the same way, so a function three calls up can fail with
`A | B | C` and still write only `?`. There is nothing like a conversion
trait to implement, and no error is ever turned into another behind the
code's back: to change an error, call `mapErr`.

## Working with options and results

`match` handles every case. For the common ones there are two operators,
`??` and `?.`, which the next section is about, and methods, which
`tondo api Option` and `tondo api Result` list:

| To | Option | Result |
| --- | --- | --- |
| ask | `isSome()`, `isNone()` | `isOk()`, `isErr()` |
| give a fallback | `value ?? fallback` | `value ?? fallback` |
| change the value | `map((x) { ... })` | `map((x) { ... })` |
| change the error | | `mapErr((e) { ... })` |
| go on with something that may be missing or fail | `andThen((x) { ... })` | `andThen((x) { ... })` |
| take another when it is missing | `value ?? other` | |
| go on along a chain while there is a value | `value?.field` | |
| keep it only if | `filter((x) { ... })` | |
| turn into the other | `okOr(error)` | `ok()`, `err()` |
| insist, and panic otherwise | `expect("why it is there")` | `expect("why it worked")` |
| say what was going on | `context("doing this")` | `context("doing this")` |

```tondo
import std.console

fn parse(text: String): Int ! String {
    Int.parse(text).mapErr((_) { "`{text}` is not a number" })
}

fn main() {
    let fallback = parse("x") ?? 0
    let doubled = parse("21").map((n) { n * 2 })
    let chained = parse("4").andThen((n) { parse("{n}0") })
    let maybe = parse("").ok()
    let required = some(5).okOr("missing")
    console.println("{fallback} {doubled} {chained} {maybe} {required}")
    let missing: Int? = none
    console.println("{some(3).filter((n) { n > 5 })} {missing ?? some(1)} {parse("7").expect("a literal")}")
}
```

```text output
0 ok(42) ok(40) none ok(5)
none some(1) 7
```

`expect` is for what the program knows cannot fail, and says why: when it
does fail, that is a bug, and it panics with the reason. There is no bare
`unwrap`.

## A fallback and a chain: ?? and ?.

`a ?? b` is the value that `a` holds, or `b` when it holds none. `a?.b` goes
on along a chain of values that may be missing, and is `none` as soon as one
is:

```tondo
import std.console

type Address = { city: String? }
type User = { name: String, address: Address? }

/// `?.` goes on along the chain only while there is a value.
fn cityOf(user: User): String {
    user.address?.city ?? "nowhere"
}

/// The right side of `??` may leave the function instead of giving a value.
fn cityIn(users: Map[String, User], name: String): String ! String {
    let user = users[name] ?? fail "nobody is called {name}"
    cityOf(user)
}

fn expensive(): Int {
    console.println("computing the fallback")
    0
}

fn main() {
    let ada = User { name: "ada", address: Address { city: "London" } }
    let bo = User { name: "bo", address: none }
    let users = ["ada": ada, "bo": bo]
    console.println("{cityOf(ada)} {cityOf(bo)}")
    console.println("{cityIn(users, "ada")} {cityIn(users, "cy")}")

    // The right side runs only when it is needed.
    let known: Int? = 7
    let missing: Int? = none
    console.println("{known ?? expensive()} {missing ?? expensive()}")

    // A result's error is dropped for the fallback, and fallbacks chain.
    let port = Int.parse("eighty") ?? missing ?? 8080
    console.println("{port}")

    var total = 0
    for text in ["1", "two", "3"] {
        let number = Int.parse(text) ?? continue
        total += number
    }
    console.println("{total}")
}
```

```text output
London nowhere
ok(London) err(nobody is called cy)
computing the fallback
7 0
8080
4
```

- **`??` takes an option or a result on its left.** With a result, the error
  is dropped for the fallback, so use it where the error does not matter.
- **Its right side is evaluated only when it is needed**, as `expensive`
  shows: it ran once, for the value that was missing.
- **Its right side may leave**: `?? return`, `?? continue`, `?? break` and
  `?? fail error`. That is how a function takes a value or gives up, in one
  line.
- **Fallbacks chain** when the one in the middle may be missing too:
  `first ?? second ?? last`.
- **`??` binds last of all the operators**, so `count ?? 0 + 1` is
  `count ?? (0 + 1)`. Write `(count ?? 0) + 1` for the other.
- **`?.` on an option stays in the function.** `user.address?.city` is
  `none` when there is no address, and otherwise what `city` is. A `?` that
  ends an expression still leaves the function, as before: in a function
  that gives an option, `user.address?.city?` passes `none` on.
- **On a result, `?.` passes the error on**, as `?` does:
  `client.get(url)?.text()?`. To leave the function when an option is
  missing and go on otherwise, write it with parentheses: `(user.address?).city`.

## Saying what the program was doing: Failure

An error type is for callers that tell the cases apart. Much of a program
only needs to report what went wrong, and what it was doing at the time.
`.context("...")` turns any error, or a missing value, into the prelude's
`Failure`, which keeps the cause:

```tondo
import std.console
import std.fs
import std.json

type Config = { port: Int }

fn load(path: String): Config ! Failure {
    let text = fs.readText(path).context("reading {path}")?
    json.decode[Config](text).context("decoding {path}")?
}

fn start(): Config ! Failure {
    load("/missing/app.json").context("starting the server")
}

fn main() {
    match start() {
        ok(config) => console.println("listening on {config.port}")
        err(failure) => {
            console.println("{failure}")
            let missing = failure.find[fs.FsError]() == fs.FsError.NotFound
            console.println("because the file is missing: {missing}")
        }
    }
}
```

```text output
starting the server: reading /missing/app.json: not found
because the file is missing: true
```

Each `context` adds what was going on at its level, so the message reads
from the outside in. `failure.find[E]()` gives the nearest cause of that
type through the chain, for the caller that does want to tell.
`Failure { message: "..." }` is one with no cause.

A function that returns `T ! Failure` accepts any error through `context`,
which makes it the easy signature for the inside of an application. A
library's public functions are better off with their own error types, below.

## Designing a module's errors

- **Inside a module**, a union is enough: `!(fs.FsError | ParseError)`
  costs nothing to write and loses nothing.
- **At a module's edge**, declare an enum. It names what can go wrong in the
  module's own terms, it can carry what a caller needs, and it does not
  expose which libraries are used inside.
- **In an application's own code**, `Failure` with `context` is usually
  right: someone will read the message, and nobody will match on it.

```tondo
import std.console
import std.fs
import std.json

pub type Settings = { name: String, port: Int = 8080 }

/// Why settings could not be loaded.
pub enum SettingsError {
    /// The file is not there: the path that was tried.
    Missing(String)
    /// The file could not be read.
    Unreadable(fs.FsError)
    /// The file is not valid: where, and why.
    Invalid(String)
}

impl Display for SettingsError {
    fn display(self): String {
        match self {
            Missing(path) => "there is no {path}"
            Unreadable(error) => "cannot read the settings: {error}"
            Invalid(why) => "the settings are not valid: {why}"
        }
    }
}

pub fn loadSettings(path: String): Settings ! SettingsError {
    let text = match fs.readText(path) {
        ok(text) => text
        err(NotFound) => fail Missing(path)
        err(error) => fail Unreadable(error)
    }
    json.decode[Settings](text).mapErr((error) { SettingsError.Invalid("{error}") })
}

fn main() {
    match loadSettings("/missing/settings.json") {
        ok(settings) => console.println(settings.name)
        err(error) => console.println("{error}")
    }
}
```

```text output
there is no /missing/settings.json
```

## Panics

A panic means the program is wrong. It stops the program, after its
`defer`s have run, with a report of what failed, the values involved and the
path the program took, through the task that spawned the one that failed:

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

What panics:

| Code | When |
| --- | --- |
| `P0001` | an index or a slice out of range |
| `P0003` | an integer divided by zero |
| `P0005` | an integer overflowed |
| `P0006` | arithmetic on arrays of different lengths |
| `P0007` | an `assert` that does not hold |
| `P0008` | `panic("message")` |
| `P0010` | a shift by a negative count, or by the width or more |
| `P0012` | a send on a closed channel |
| `P0013` | a deadlock: every task waits for another |
| `P0014` | a task takes a lock it already holds |

`tondo explain P0005` says what any of them means, with an example. A panic
is not for input. What comes from outside is checked and answered with an
error, and for each thing that panics there is a way to ask first:
`array.get(i)` gives an option where `array[i]` would panic, and
`a.checkedAdd(b)` where `a + b` would.

A program can panic with its own code and data, which the report shows and a
`recover` can read:

```tondo
fn main() {
    let key = "users"
    panic("the cache index is corrupt", code: "cache.corrupt", data: ["key": key, "entries": 12])
}
```

A panic ends the program with status 101. A crash below the language, as a
stack overflow or a fault inside C, prints `fatal:` with the same kind of
trace. With `TONDO_PANIC_FORMAT=json` in its environment, a program writes
how it failed as one line of JSON, for tools.

## assert

`assert(condition)` panics when the condition is false, and shows both sides
of a comparison:

```tondo
fn mean(values: Array[Float]): Float {
    assert(not values.isEmpty(), "the mean of no values")
    values.sum() / Float(values.length())
}

fn main() {
    assert(mean([1.0, 2.0, 3.0]) == 2.0)
}
```

```text
panic[P0007]: assertion-failed: balance.get() == 15
  left:  5
  right: 15
```

An `assert` is always checked, in every build: an optimized program does not
behave differently from the one that was tested. In tests it is how a test
fails ([Testing](16-testing.md)).

## recover

A server should answer the next request after a bug in one. `recover { ...
}` stops a panic of its block, of what the block calls, and of the tasks it
starts, and gives it as a value: a `T ! Panic`.

```tondo
import std.console

/// One job of many: a bug in it ends the job, not the worker.
fn run(job: Int): Int {
    let sizes = [10, 20]
    sizes[job]
}

fn main() {
    for job in 0..3 {
        match recover { run(job) } {
            ok(size) => console.println("job {job}: {size}")
            err(crash) => console.println("job {job} failed with {crash.code}")
        }
    }

    match recover { panic("broken", code: "cache.corrupt") } {
        err(crash) if crash.code == "cache.corrupt" => console.println("rebuilding: {crash.message}")
        err(crash) => crash.resume()
    }
}
```

```text output
job 0: 10
job 1: 20
job 2 failed with P0001
rebuilding: broken
```

- A `Panic` has the panic's `code`, `message`, `data`, `location` and
  `stack`. `crash.resume()` lets it go on, for a `recover` that handles only
  some.
- On the way out the `defer`s run, locks of `std.sync` are let go, and the
  block's other tasks are cancelled. A lock a panic went through is free
  afterwards, never poisoned.
- Recover at the edge of a unit of work: a request, a job, a message. A
  panic is still a bug to fix, and `recover` around each call would only hide
  it. `std.http` already answers 500 for a handler that panics and goes on.
- A deadlock and a crash below the language are not stopped.

## Error or panic

| | Error | Panic |
| --- | --- | --- |
| Means | something went wrong that can go wrong | the program is wrong |
| Is | a value, in the signature | a stop, with a report |
| Examples | a missing file, bad input, a timeout | an index out of range, an overflow, a broken invariant |
| The caller | must handle it or pass it on | does nothing, and should not |
| Handled with | `?`, `match`, `??`, `context` | a fix; `recover` at the edge of a request |

When writing a function, ask who is at fault if it cannot do its job. If it
is whoever calls it with input they cannot know to be good, return an error.
If only a mistake in the program can cause it, panic, with an `assert` that
says what was assumed.

Next: [Collections](08-collections.md).
