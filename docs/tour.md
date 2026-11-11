# A tour of Tondo

This is the language by example, for someone who already programs. Every
example is whole, with nothing left out: save one as a `.to` file and
`tondo run` it. The tests compile them all, so they stay true.

`tondo guide` is the reference this tour points to: each topic it names, as
`tondo guide types`, goes further than the section here.

- [A program, and a script](#a-program-and-a-script)
- [Bindings, functions and the one loop](#bindings-functions-and-the-one-loop)
- [Records, enums and match](#records-enums-and-match)
- [Missing values and errors](#missing-values-and-errors)
- [Collections are values](#collections-are-values)
- [Text and numbers](#text-and-numbers)
- [Generics and traits](#generics-and-traits)
- [Lending a value, and identity](#lending-a-value-and-identity)
- [Tasks](#tasks)
- [Contexts and deadlines](#contexts-and-deadlines)
- [Types that write their own code](#types-that-write-their-own-code)
- [Projects and tests](#projects-and-tests)
- [Processes, cleanup and C](#processes-cleanup-and-c)
- [What is not there](#what-is-not-there)

## A program, and a script

```tondo
import std.console

fn main() {
    console.println("Hello, Tondo")
}
```

`tondo run hello.to` compiles it to a native executable, keeps it in a cache,
and runs it. There is no interpreter: what `run` runs is what `build` ships.

A file needs no `main`. Its statements run from the top, so a script is a
program with less around it, and `#!/usr/bin/env tondo` makes it executable:

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

## Bindings, functions and the one loop

```tondo
import std.console

fn describe(n: Int): String {
    if n % 15 == 0 { "fizzbuzz" } else if n % 5 == 0 { "buzz" } else if n % 3 == 0 { "fizz" } else { "{n}" }
}

/// An argument can have a default, and be given by name.
fn greet(name: String, greeting: String = "Hello"): String {
    "{greeting}, {name}!"
}

fn main() {
    let limit = 15                  // `let` never changes
    var words = 0                   // `var` does
    for n in 1..=limit {            // also `for condition { }` and `for { }`
        if describe(n) != "{n}" {
            words += 1
        }
    }
    console.println("{words} of {limit}")
    console.println(greet("Ada"))
    console.println(greet("Ada", greeting: "Hi"))
}
```

```text
7 of 15
Hello, Ada!
Hi, Ada!
```

- A newline ends a statement, and there are no semicolons.
- The last expression of a block is its value, so `if` gives one and
  `return` is only for leaving early.
- `for` is the only loop. A condition is a `Bool`: there is no truthiness.
- Signatures state their types; inside a function they are inferred.
- A binding never hides another one that is visible: a new value takes a new
  name.

## Records, enums and match

```tondo
import std.console

pub type User = {
    name: String
    age: Int
    admin: Bool = false
}

pub enum Payment {
    Cash
    Card(String)
    Transfer { iban: String, reference: String }
}

/// A method is a function declared outside its type.
fn User.describe(self): String {
    "{self.name} ({self.age})"
}

fn label(payment: Payment): String {
    match payment {
        Payment.Cash => "cash"
        Payment.Card(number) => "card {number}"
        Payment.Transfer { iban, reference } => "transfer {iban} {reference}"
    }
}

fn main() {
    let ada = User { name: "Ada", age: 36 }
    let older = ada with { age: 37 }          // a copy with one change
    console.println("{ada.describe()} {older.describe()} {older.admin}")
    console.println(label(Payment.Card("4242")))
}
```

```text
Ada (36) Ada (37) false
card 4242
```

A `match` covers every case, and the compiler names the one that is missing.
Without the last arm of `label`:

```text
error[E1204]: `match` is not exhaustive: `Payment.Transfer { .. }` is not covered
  --> pay.to:10:5
```

There are no classes and nothing is inherited: a program is records, enums,
functions and traits. `pub` exports a name, and `priv` hides a field from
other modules. `type UserId = Int` makes a new type over another, built with
`UserId(7)`, that the compiler never mixes with an `Int`; `A | B` is a value
of either type, told apart with `match`. `tondo guide types` has them all.

## Missing values and errors

What may be missing is a `T?`, with `some(value)` or `none`. A function that
can fail returns `T ! E`, and its signature is the whole contract: nothing
is thrown.

```tondo
import std.console

pub enum ConfigError {
    Missing(String)
    Invalid(String)
}

/// `Display` gives the error its message.
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

fn address(host: String, text: String): String ! ConfigError {
    let number = port(text)?            // an error goes on to the caller
    "{host}:{number}"
}

fn first(names: Array[String]): String? {
    names.get(0)
}

fn main() {
    console.println("{address("localhost", "8080")}")
    console.println("{address("localhost", "http")}")
    console.println(first([]).unwrapOr("nobody"))
}
```

```text
ok(localhost:8080)
err(`port` is not valid)
nobody
```

- `?` gives the value or returns the error. A result that nobody uses is a
  compile error, so a failure cannot be dropped by forgetting it.
- A function that fails in several ways says `!(ConfigError | NetworkError)`,
  and `?` widens into it with no conversion to write.
- `.context("reading {path}")` turns any error into a `Failure` that says
  what the program was doing and keeps the cause, for code that only needs
  to report it.
- `main` may fail: the program prints the error and exits with 1.

A bug is a panic, not an error: an index out of range, an overflow, a failed
`assert`. It stops the program with a code, the values that broke the rule
and the stack:

```text
panic[P0005]: checked-overflow: 9223372036854775807 + 1 does not fit in Int (-9223372036854775808 to 9223372036854775807)
 --> over.to:5:23
stack:
  main  over.to:5:23
```

A server should not die of one request's bug, so `recover { ... }` stops a
panic at the edge of a request or a job and gives it as a value:

```tondo
import std.console

fn main() {
    let values = [3, 4]
    let third = recover { values[2] }.unwrapOr(0)
    console.println("{third}")
}
```

`tondo guide errors` goes through all of it.

## Collections are values

`Array[T]`, `Map[K, V]`, `Set[T]` and `Deque[T]` are values, as numbers are.
Assigning one copies it, and the copy costs nothing until one of the two
changes, because they share their storage until then. A function changes its
caller's collection only through a parameter that says so.

```tondo
import std.console

fn main() {
    let prices = [10.0, 20.0, 30.0, 40.0]
    console.println("{prices[-1]} {prices[1:3]}")        // from the end, and a slice
    console.println("{prices * 1.21}")                    // arithmetic on every element
    console.println("{prices.filter((p) { p > 15.0 }).map((p) { p / 2.0 }).sum()}")

    var ages: Map[String, Int] = [:]
    ages["ada"] = 36
    ages["bo"] = 20
    let before = ages                                     // a copy
    ages["cy"] = 41
    for (name, age) in ages {                             // in the order they were added
        console.println("{name} is {age}")
    }
    console.println("{before.length()} {ages.get("dee")} {ages.getOr("dee", 0)} {"bo" in ages}")

    let evens = (0..10).filter((n) { n % 2 == 0 }).map((n) { n * n }).collect()
    console.println("{evens}")
}
```

```text
40.0 [20.0, 30.0]
[12.1, 24.2, 36.3, 48.4]
45.0
ada is 36
bo is 20
cy is 41
2 none 0 true
[0, 4, 16, 36, 64]
```

A key that is not there is a `none`, never a zero that looks like a value.
Ranges, `text.chars()` and `array.iter()` are lazy iterators with the same
adapters. `tondo guide collections` and `tondo api Array` list the methods.

## Text and numbers

```tondo
import std.console
import std.math

fn main() {
    let id = 42
    let name = "ada"
    let ratio = 0.98765
    console.println("[{id:08}] [{name:<8}] [{name:>8}] [{ratio:.2}] [{255:#x}]")

    let price: math.Decimal = 19.99
    console.println("{price * 3} {0.1 + 0.2}")

    let big = Int.max
    console.println("{big.checkedAdd(1)} {big.wrappingAdd(1)} {big.saturatingAdd(1)}")
}
```

```text
[00000042] [ada     ] [     ada] [0.99] [0xff]
59.97 0.30000000000000004
none -9223372036854775808 9223372036854775807
```

- A string interpolates any expression, with a format the compiler checks
  against the value's type. `+` is arithmetic and does not join strings.
- `Int` is 64 bits and its overflow panics, in every build: an optimized one
  does not change what the program means. Arithmetic that wraps, checks or
  saturates has a name.
- Every width is there, `Int8` to `Int128` and `UInt8` to `UInt128`, with
  the same operations, and conversions that can lose a value return a
  result.
- `math.Decimal` is an exact decimal, for money, and `math.BigInt` an integer
  of any size.

## Generics and traits

```tondo
import std.console

trait Shape {
    fn area(self): Float

    /// A method with a body is a default.
    fn describe(self): String {
        "a shape of area {self.area()}"
    }
}

type Square = { side: Float }

impl Shape for Square {
    fn area(self): Float {
        self.side * self.side
    }
}

/// `Display` makes a type usable in `"{value}"`.
impl Display for Square {
    fn display(self): String {
        "square {self.side}"
    }
}

fn largest[T: Ordered](items: Array[T]): T? {
    var best: T? = none
    for item in items {
        best = match best {
            some(current) if current >= item => some(current)
            _ => some(item)
        }
    }
    best
}

fn total[S: Shape](shapes: Array[S]): Float {
    var sum = 0.0
    for shape in shapes {
        sum += shape.area()
    }
    sum
}

fn main() {
    let square = Square { side: 2.0 }
    console.println(square.describe())
    console.println("{square} {total([square, square])} {largest([3, 9, 4])}")
}
```

```text
a shape of area 4.0
square 2.0 8.0 some(9)
```

A type implements a trait where it says so, with `impl`. Generic code is
compiled for each type it is used with, so a trait costs nothing when the
program runs. What a type can do by its parts, the compiler works out:
whether it can be copied, compared, used as a key, or sent to another task.

Operators are closed: a type of yours takes `+` and `-` by implementing
`Additive`, and `*` and `/` with `Number`, with their meaning in arithmetic,
and nothing else can be given to them.

## Lending a value, and identity

A parameter receives a value. To let a function read a large one without
copying it, or change the caller's, the call lends it, and both the signature
and the call say so:

```tondo
import std.console

fn total(values: ref Array[Float]): Float {
    values.sum()
}

fn scale(values: mut Array[Float], factor: Float) {
    values *= factor
}

fn main() {
    var numbers = [1.0, 2.0, 3.0]
    scale(mut numbers, 10.0)
    console.println("{numbers} {total(ref numbers)}")
}
```

A loan lasts for the call and cannot be kept, so there are no lifetimes to
write: `ref` reads, `mut` changes what is there, and `var` may also replace
or resize it.

Two equal records are the same value. When a program needs one thing that
several others point at, as a node of a graph, `Ref(value)` gives it an
identity. Values that refer to each other in a cycle are freed like any
other, with nothing to break by hand.

```tondo
import std.console

type Module = { name: String, imports: Array[Ref[Module]] }

fn main() {
    let util = Ref(Module { name: "util", imports: [] })
    let http = Ref(Module { name: "http", imports: [util] })
    let json = Ref(Module { name: "json", imports: [util] })
    // `util` is one module with two importers, not two equal copies.
    console.println("{http.value.imports[0] == json.value.imports[0]}")
}
```

## Tasks

Any function can wait, for the network, a file, a lock or a timer, and it is
written like any other: no `async`, no `await`, and no second version of
each library. `spawn` starts a task. Waiting stops the task and not the
thread, and tasks run on all cores.

```tondo
import std.console
import std.time

fn fetch(id: Int): Int {
    time.sleep(time.milliseconds(10))
    id * 10
}

fn main() {
    let job = spawn fetch(1)
    console.println("{job.wait()}")

    var group = Group[Int]()
    for id in 0..5 {
        group.add(spawn fetch(id))
    }
    console.println("{group.wait()}")           // results in the order they were added
}
```

A task belongs to the block that spawned it: one nobody waited for is waited
for when the block ends, so none is left running by accident. A task that
should outlive its block says `spawn detached`. With a `Group[T ! E]`, the
first error cancels the others, and a cancelled task stops at its next wait
and runs its `defer`s.

Tasks talk through channels, and `select` waits for whichever comes first:

```tondo
import std.console
import std.time

fn produce(out: Sender[Int]) {
    for n in 0..3 {
        out.send(n)
    }
}

fn slow(): Int {
    time.sleep(time.milliseconds(50))
    1
}

fn main() {
    let (sender, receiver) = channel[Int](8)
    spawn produce(sender)
    for value in receiver {                     // until closed and empty
        console.println("{value}")
    }

    let (requests, inbox) = channel[String](1)
    let job = spawn slow()
    let event = select {
        let message = inbox.receive() => "message {message}"
        let n = job.wait() => "done {n}"
        time.sleep(time.milliseconds(5)) => "timeout"
    }
    console.println(event)
    requests.close()
}
```

What tasks share goes in `std.sync`: `Mutex`, `RwLock`, `Atomic`,
`Semaphore`, `Once`, and collections built for many writers. The compiler
checks what crosses to another task, so safe code has no data races, and a
lock is let go when its closure ends, however it ends.

```tondo
import std.console
import std.sync

pub type Stats = { hits: Int }

fn record(stats: sync.Mutex[Stats], counter: sync.Atomic[Int]) {
    stats.update((s) { s.hits += 1 })
    _ = counter.add(1)
}

fn main() {
    let stats = sync.Mutex(Stats { hits: 0 })
    let counter = sync.Atomic(0)
    var group = Group[Unit]()
    for _ in 0..4 {
        group.add(spawn record(stats, counter))
    }
    group.wait()
    console.println("{stats.get().hits} {counter.load()}")
}
```

`std.executor` adds pools that bound how much runs at once and actors with a
mailbox. `tondo guide tasks` has the rest.

## Contexts and deadlines

Some values belong to the work in progress rather than to a function: the
request, the user, the trace. A `context` declares them, any function reads
them by name, and nothing passes them down as a parameter.

```tondo
import std.console

context Request = {
    id: String = ""
    user: String? = none
}

fn greet(): String {
    let user = Request.user.unwrapOr("guest")
    "hello {user} ({Request.id})"
}

fn handle(id: String, user: String) {
    with Request { id, user: some(user) }
    console.println(greet())
}

fn main() {
    handle("r1", "ada")
    console.println(greet())
}
```

```text
hello ada (r1)
hello guest ()
```

`with` changes a context until its block ends, however it ends, and a change
never reaches the caller. A task takes the contexts of the place that spawned
it. There are no mutable globals: what a whole program shares, as a compiled
pattern or a pool, is a context field whose first value is computed once.

`within` gives a block a deadline. When it passes, the block's next wait
ends, its tasks are cancelled and its `defer`s run:

```tondo
import std.console
import std.time

fn fetch(): String {
    time.sleep(time.seconds(10))
    "page"
}

fn main() {
    match within time.milliseconds(100) { fetch() } {
        ok(page) => console.println(page)
        err(timedOut) => console.println("{timedOut}")
    }
}
```

## Types that write their own code

A type is enough for the compiler to read and write it in every format of
the library, to check what arrives against its rules, and to describe it
when the program runs. Nothing is derived or registered by hand.

```tondo
import std.console
import std.json
import std.toml

pub type Server = {
    host: String
    port: Int = 8080
    @name("tag_list")
    tags: Array[String] = []
}

fn main(): !(json.JsonError | toml.TomlError) {
    let server = json.decode[Server]("""
        {{ "host": "localhost" }}
        """)?
    console.println("{server.host}:{server.port}")
    console.print(toml.encode(server with { tags: ["web"] })?)
}
```

```text
localhost:8080
host = "localhost"
port = 8080
tag_list = ["web"]
```

The same type is a route's body in `std.http`, a row in `std.sql`, a command
line in `std.cli`, and a message of Protocol Buffers when its fields carry
numbers. A program declares its own annotations, as `@Check(min: 3)`, and
reads them with their types through `std.reflect`; `derive` runs a Tondo
program that writes an `impl` when the program compiles, and `tondo expand`
shows what it wrote. `tondo guide meta` explains each.

## Projects and tests

A project is a directory whose folders are modules. `tondo new shop` makes
one, and a `tondo.toml` names it and its dependencies, which come from a
path, a git repository or an archive and are pinned in `tondo.lock`.

```text
shop/
  tondo.toml
  src/
    main.to
    cart.to            the module shop.cart
    cart_test.to       its unit tests: they see what the module keeps private
  tests/
    api.to             an integration test: it sees only what is public
```

Tests are part of the language. `tondo test` runs them in parallel:

```tondo test
fn total(prices: Array[Int]): Int {
    prices.sum()
}

test addsPrices {
    assert(total([2, 3]) == 5)
}

test anEmptyCartIsFree {
    assert(total([]) == 0, "an empty cart costs {total([])}")
}
```

A failed `assert` shows both sides of its comparison. A test that calls
`testing.bench` is a benchmark, one that calls `testing.fuzz` is tried with
inputs the runner searches for, and one that starts tasks is run again in
the orders of them that differ. [The toolchain](toolchain.md) shows what
`tondo test` does with each.

## Processes, cleanup and C

A command is a value, and `|` joins commands into a pipeline with no shell
in between, so there is nothing to quote and nothing to inject:

```tondo
import std.console
import std.process

fn main(): !process.ProcessError {
    let commits = process.command("git", "log", "--oneline") | process.command("head", "-n", "3")
    console.print(commits.output()?.text()?)
}
```

`defer` runs when its block ends, whichever way it ends. Ctrl-C and `SIGTERM`
cancel the program rather than kill it, so every task runs its `defer`s and
a server finishes the requests it has.

```tondo
import std.console

fn work() {
    defer console.println("cleanup")
    console.println("working")
}

fn main() {
    work()
}
```

C is called through `extern fn`, inside `unsafe`, which is the only place
where raw pointers exist. `tondo bind header.h` writes the declarations of a
header, and `tondo build --library` goes the other way: a library, with its
header, that C calls.

```tondo
import std.console
import std.ffi

extern fn abs(value: ffi.CInt): ffi.CInt

fn main() {
    let result = unsafe { abs(-42) }
    console.println("{result}")
}
```

## What is not there

Tondo leaves out what would give a second way to do something, or hide what
a signature should say.

| Not in Tondo | Instead |
| --- | --- |
| `null` | `T?`, which the compiler makes you look at |
| Exceptions | `T ! E` in the signature, `?` and `match` |
| `async` and `await` | Any function waits; `spawn` and `wait()` |
| Classes and inheritance | Records, enums, traits and functions |
| Overloaded functions and operators | One function per name; `Additive` and `Number` for arithmetic |
| Conversions that lose a value, and truthiness | `Int(x)?`, which can fail, and a `Bool` in every condition |
| Mutable globals | Contexts |
| Macros | `derive` and generators, which are Tondo programs, and annotations |
| Lifetimes | Loans that last one call: `ref`, `mut`, `var` |
| Semicolons, `while`, `loop`, a ternary operator | A newline, `for`, and `if` as an expression |
| Shadowing | A new name |
| A build that behaves differently when optimized | The same overflow checks, order of evaluation and results in every build |

`tondo guide pitfalls` lists the habits other languages leave, with the
error each gives and what to write instead.

## Where to go next

- `tondo guide`, a topic at a time, in the terminal or in
  [`crates/tondo/guide`](../crates/tondo/guide/overview.md).
- [The toolchain](toolchain.md): what `tondo` does besides compiling.
- [Tondo next to Go, Rust and C#](comparison.md), for someone who comes from
  one of them.
- The [specifications](../spec), which define all of it.
