# Tondo next to other languages

Tondo was designed with other languages on the table. Its
[specification](../spec/TONDO_LANGUAGE_SPEC.md) names what it took from each
and what it left. This page goes through the three it is closest to, Go,
Rust and C#, for someone who writes one: what will look familiar, what is
different, and why. Each of them, and five more, has a page of its own that
writes the same twelve programs in that language and in Tondo.

- [A page for each language](#a-page-for-each-language)
- [At a glance](#at-a-glance)
- [If you write Go](#if-you-write-go)
- [If you write Rust](#if-you-write-rust)
- [If you write C#](#if-you-write-c)
- [If you write Python, TypeScript, Java, Kotlin or Swift](#if-you-write-python-typescript-java-kotlin-or-swift)
- [What the toolchain adds](#what-the-toolchain-adds)
- [Where another is the better choice today](#where-another-is-the-better-choice-today)

## A page for each language

Each page takes the same twelve small programs, from copying a list to an
HTTP server, and writes them twice. Every program was run, in both
languages, and the page shows what it prints. Then come the two toolchains
command by command, the habits to leave behind, and where the other
language is the better choice.

| If you write | The page | What it is mostly about |
| --- | --- | --- |
| Go | [Tondo for someone who writes Go](comparison/go.md) | errors that cannot be forgotten, `nil`, slices that share, goroutines with an owner, `context.Context` |
| Rust | [Tondo for someone who writes Rust](comparison/rust.md) | what stands where the borrow checker does, `async`, and what the standard library holds |
| C# | [Tondo for someone who writes C#](comparison/csharp.md) | exceptions, `null`, shared objects, classes, `async` and `Task` |
| Python | [Tondo for someone who writes Python](comparison/python.md) | what stays familiar, and what a compiler finds before the program runs |
| TypeScript | [Tondo for someone who writes TypeScript](comparison/typescript.md) | types that are still there when the program runs |
| Java | [Tondo for someone who writes Java](comparison/java.md) | `null`, checked and unchecked exceptions, boxes, and the virtual machine |
| Kotlin | [Tondo for someone who writes Kotlin](comparison/kotlin.md) | the language Tondo reads most like, without the JVM under it |
| Swift | [Tondo for someone who writes Swift](comparison/swift.md) | the same values and optionals, without classes or `async` |

The twelve programs are: values and copies, a value that may be missing,
errors, a type with several shapes, traits and generics, collections,
tasks, deadlines, shared state, JSON, tests, and an HTTP server with its
client.

## At a glance

| | Tondo | Go | Rust | C# |
| --- | --- | --- | --- | --- |
| A function fails | `T ! E` in the signature, passed on with `?` | an `error` result, checked with `if err != nil` | `Result<T, E>`, passed on with `?` | an exception, which no signature shows |
| A missing value | `T?`; there is no `null` | `nil` | `Option<T>` | `null`, with warnings from nullable annotations |
| A function that waits | is written like any other | is written like any other | is `async`, and so are its callers | is `async`, and so are its callers |
| A task nobody waits for | is waited for when its block ends | keeps running | depends on the runtime | keeps running |
| Memory | automatic: reference counts and a collector of cycles | automatic: a tracing collector | ownership, borrows and lifetimes | automatic: a tracing collector |
| Assigning an array or a map | copies it; storage is shared until a change | shares it | moves it | shares it |
| Data races in safe code | rejected when compiling | found by the race detector, at run time | rejected when compiling | possible |
| Polymorphism | traits, resolved when compiling; enums | interfaces, satisfied implicitly | traits, static or `dyn`; enums | classes, interfaces, inheritance |
| An integer overflows | a panic, in every build | it wraps | a panic in debug builds, it wraps in release | it wraps, unless `checked` |
| Formatting | `tondo fmt`, with no options | `gofmt`, with no options | `rustfmt`, configurable | `dotnet format`, configurable |
| What ships | a native executable | a native executable | a native executable | an assembly for a runtime, or Native AOT |

And the five others, by the same rows:

| | Tondo | Python | TypeScript | Java | Kotlin | Swift |
| --- | --- | --- | --- | --- | --- | --- |
| A function fails | `T ! E` in the signature | an exception | an exception of any type | an exception, checked or not | an exception | `throws`, typed since Swift 6 |
| A missing value | `T?`; no `null` | `None`, anywhere | `undefined` and `null`, checked when `strict` | `null`, anywhere | `T?`, and `null` from Java | `T?` |
| A function that waits | like any other | `async` | `async` | like any other, on a virtual thread | `suspend` | `async` |
| A task nobody waits for | waited for by its block | keeps running | keeps running | keeps running | waited for by its scope | waited for by its scope |
| Assigning a list or a map | copies it | shares it | shares it | shares it | shares it | copies it |
| Data races in safe code | rejected when compiling | possible | one thread | possible | possible | rejected when compiling, in Swift 6 |
| Types when the program runs | there | checked then | erased | generics erased | generics erased | there |
| What ships | a native executable | sources and an interpreter | sources and a runtime | a jar and a JVM | a jar and a JVM | a native executable |

## If you write Go

Tondo is closest to Go in what it is for: servers, tools and command lines,
built fast into one executable, with a toolchain that has an opinion.

**You will recognize**

- A small core: one loop, explicit imports, composition instead of
  inheritance, and one way to format.
- Tasks that are cheap, run on all cores and wait without marking anything,
  with typed channels and `select`.
- `defer`, and `tondo run`, which works like `go run`.
- A static executable, and a build for another architecture from a flag.
- A standard library with HTTP, JSON, SQL, templates and crypto in it.

**What is different**

- **Errors.** A function that fails returns `T ! E`, and `?` passes the
  failure on. An error cannot be ignored by not looking at it: an unused
  result does not compile.
- **No `nil`.** A value that may be missing is a `T?`, and the compiler makes
  the code look. A map has no zero value that looks like an entry:
  `ages.get("cy")` is `none`.
- **Values.** Assigning an array or a map copies it, where a Go slice or map
  shares its storage with the copy. Mutation goes through `var`, or through
  a parameter that says so.
- **Tasks have an owner.** A `spawn` belongs to its block, which waits for
  it. `go f()` is `spawn detached f()`, and it is the exception.
- **No `context.Context` parameter.** A `context` is declared once and read
  by name in any function below; `within` gives a block a deadline.
- **Types.** Enums carry data and `match` covers every case. Traits are
  implemented explicitly, with `impl Shape for Square`. `pub` exports a
  name, and capitals are only style.
- **Numbers.** An overflow panics instead of wrapping.

The same function in both:

```go
func load(path string) (Config, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return Config{}, fmt.Errorf("reading %s: %w", path, err)
	}
	var config Config
	if err := json.Unmarshal(data, &config); err != nil {
		return Config{}, fmt.Errorf("decoding %s: %w", path, err)
	}
	return config, nil
}
```

```tondo
import std.console
import std.fs
import std.json

pub type Config = { name: String, port: Int = 8080 }

fn load(path: String): Config ! Failure {
    let text = fs.readText(path).context("reading {path}")?
    json.decode[Config](text).context("decoding {path}")?
}

fn main() {
    match load("app.json") {
        ok(config) => console.println("{config.name} listens on {config.port}")
        err(failure) => console.println("{failure}")
    }
}
```

And waiting for several tasks, which in Go takes a `sync.WaitGroup` and a
place to put the results:

```go
func sizes(urls []string) []int {
	out := make([]int, len(urls))
	var wg sync.WaitGroup
	for i, url := range urls {
		wg.Add(1)
		go func() {
			defer wg.Done()
			out[i] = size(url)
		}()
	}
	wg.Wait()
	return out
}
```

```tondo
import std.console

fn size(url: String): Int {
    url.length()
}

fn sizes(urls: Array[String]): Array[Int] {
    var group = Group[Int]()
    for url in urls {
        group.add(spawn size(url))
    }
    group.wait()
}

fn main() {
    console.println("{sizes(["https://example.com", "https://example.org"])}")
}
```

Tondo's goal for speed is Go's: every row of [`bench/`](../bench/README.md)
at or under Go's time and allocations, measured side by side. The standard
library was sized against what porting Gin, Hugo and dd-trace-go needs, and
`tondo guide templates` has a section for someone who knows Go's templates.

[Tondo for someone who writes Go](comparison/go.md) has twelve programs in
both languages, and the commands of one toolchain beside the other's.

## If you write Rust

A Rust programmer already knows most of Tondo's types. What changes is
memory: there is nothing to own, to borrow for long, or to annotate.

**You will recognize**

- `Option` and `Result`, written `T?` and `T ! E`, with `?`.
- Enums that carry data, and a `match` that covers every case.
- Traits with explicit `impl`s, generics compiled for each type, and `impl
  Trait` results.
- No `null`, mutation that is visible, and panics kept apart from errors.
- `unsafe` as the one place for raw pointers and C.

**What is different**

- **No lifetimes.** A loan is a way of passing a parameter, `ref`, `mut` or
  `var`, that lasts for the call and cannot be stored, so there is nothing
  to name. What lives longer is a value, or a `Ref[T]` when it needs an
  identity.
- **Memory is automatic.** Reference counts free a value when its last user
  goes, and a collector finds the cycles. Strings and collections copy by
  sharing their storage until one changes.
- **No `async`.** Any function waits, and there is no runtime to choose: the
  scheduler, the timers and the network are part of every program.
- **No macros in everyday code.** The compiler writes the encoders and
  decoders of every type, `derive` runs a Tondo program, and what is printed
  is an interpolated string.
- **Errors join without `From`.** A function that fails in two ways says
  `!(A | B)`, and `?` widens into it. `.context("...")` will be familiar from
  `anyhow`.
- **Traits are static only.** There is no `dyn`: values of several types go
  in an enum or a union, which a `match` tells apart.
- **Operators are closed.** A type takes `+ - * /` through two traits,
  `Additive` and `Number`, and no other operator.
- **Spelling.** `var x` for `let mut x`, `fn f(): Int` for `-> Int`,
  `Array[T]` for `Vec<T>`, `(x) { x * 2 }` for `|x| x * 2`, and `some`,
  `none`, `ok`, `err` in lower case.

A function that returns one of its arguments needs a lifetime in Rust, to
say that the result lives as long as both:

```rust
fn longest<'a>(a: &'a str, b: &'a str) -> &'a str {
    if a.len() >= b.len() { a } else { b }
}
```

In Tondo a string is a value, and returning it copies nothing but a
reference to its storage:

```tondo
import std.console

fn longest(a: String, b: String): String {
    if a.length() >= b.length() { a } else { b }
}

fn main() {
    console.println(longest("go", "rust"))
}
```

A function that fails in two ways needs a type for both and a conversion
from each:

```rust
#[derive(Debug)]
enum PortError {
    Io(std::io::Error),
    Parse(std::num::ParseIntError),
}

impl From<std::io::Error> for PortError {
    fn from(error: std::io::Error) -> Self {
        PortError::Io(error)
    }
}

impl From<std::num::ParseIntError> for PortError {
    fn from(error: std::num::ParseIntError) -> Self {
        PortError::Parse(error)
    }
}

fn port(path: &str) -> Result<u16, PortError> {
    let text = std::fs::read_to_string(path)?;
    Ok(text.trim().parse()?)
}
```

```tondo
import std.console
import std.fs
import std.text

fn port(path: String): Int ! (fs.FsError | text.TextError) {
    let content = fs.readText(path)?
    Int.parse(content.trim())?
}

fn main() {
    console.println("{port("/etc/port")}")
}
```

[Tondo for someone who writes Rust](comparison/rust.md) has twelve programs
in both languages, and a section on what stands where the borrow checker
does, with its cost.

## If you write C#

Tondo shares much of what makes C# comfortable: a library that comes
complete, records, interpolated strings, and tools that look inside a running
program. It leaves out the runtime, the classes, the exceptions and `null`.

**You will recognize**

- Records with `with`: `ada with { age: 37 }` is the same expression.
- Interpolated strings with formats, `"{price:.2}"`, and `T?` for what may
  be missing.
- `ref` parameters. Tondo has three ways of lending a value for a call: `ref`
  to read it, as `in`, and `mut` and `var` to change it, as `ref`.
- Methods on collections that read like LINQ: `filter`, `map`, `groupBy`,
  `sortedBy`.
- Route handlers whose parameters are the route's, as in minimal APIs:
  `app.get("/users/:id", (id: Int) { find(id) })`.
- Values that follow the work across tasks. A `context` is what
  `AsyncLocal<T>` is used for, declared as a type and read by name.
- Shared collections built as `ConcurrentDictionary` is: `sync.Map` reads
  without locks and writes by stripes.
- The tools: counters, traces and dumps of a running program, as
  `dotnet-counters`, `dotnet-trace` and `dotnet-dump` give, and Edit and
  Continue in a live build. `std.cli` styles the terminal with Spectre's
  markup, `[bold red]text[/]`.

**What is different**

- **No runtime to install.** A program is a native executable, compiled
  ahead of time.
- **No exceptions.** What can fail is in the signature, `T ! E`, and nothing
  travels up the stack unseen. A bug is a panic, which a server stops at the
  edge of a request with `recover`.
- **No `null`.** `T?` is an option the compiler makes the code open, rather
  than an annotation that warns.
- **No `async`, `await` or `Task<T>`.** A function that waits returns its
  value, and `spawn` gives a `Join` to wait for.
- **No classes.** Records, enums and traits, with no inheritance and no
  overloads: one function for each name.
- **Value semantics everywhere.** A list assigned is a copy, where a
  `List<T>` is shared.
- **Reference counts instead of a tracing collector**, with a collector only
  for cycles.
- **Generated code instead of reflection.** Serializers are written when the
  program compiles; `std.reflect` is there for the cases that need names at
  run time.

Fetching several pages at once, in C#:

```csharp
static async Task<int> SizeAsync(HttpClient http, string url)
{
    var text = await http.GetStringAsync(url);
    return text.Length;
}

static async Task<int[]> SizesAsync(HttpClient http, string[] urls)
{
    try
    {
        return await Task.WhenAll(urls.Select(url => SizeAsync(http, url)));
    }
    catch (HttpRequestException)
    {
        return [];
    }
}
```

```tondo
import std.console
import std.http

fn size(url: String): Int ! http.HttpError {
    http.get(url)?.text()?.length()
}

fn sizes(urls: Array[String]): Array[Int] {
    var pages = Group[Int ! http.HttpError]()
    for url in urls {
        pages.add(spawn size(url))
    }
    pages.wait() ?? []
}

fn main() {
    console.println("{sizes(["https://example.com", "https://example.org"])}")
}
```

Nothing in `size` or `sizes` is marked as asynchronous, and the signature of
`size` says what the `catch` had to know from the documentation.

A chain of values that may be missing:

```csharp
static string CityOf(User? user) => user?.Address?.City ?? "nowhere";
```

```tondo
import std.console

type Address = { city: String? }
type User = { name: String, address: Address? }

fn cityOf(user: User?): String {
    user?.address?.city ?? "nowhere"
}

fn main() {
    let ada = User { name: "Ada", address: Address { city: "London" } }
    console.println("{cityOf(ada)} {cityOf(none)}")
}
```

[Tondo for someone who writes C#](comparison/csharp.md) has twelve programs
in both languages, from a class hierarchy turned into an enum to a minimal
API.

## If you write Python, TypeScript, Java, Kotlin or Swift

Tondo was not designed against these five as it was against the three
above, but each has something in it, and someone who writes one will find
part of Tondo already known.

- **Python.** Scripts of one file, slices, negative indexes, interpolated
  strings, maps in insertion order and a library with the batteries in it
  are all there. What is new is that the whole program is checked before
  its first line runs, that a list assigned is a copy, and that it runs on
  every core. [The page](comparison/python.md) starts with what stays.
- **TypeScript.** Unions, inference and types that say "this or nothing"
  will feel like home. The difference is that Tondo's types are still
  there when the program runs: what `JSON.parse` returns is checked, not
  cast. [The page](comparison/typescript.md) follows that through.
- **Java.** Records, sealed types and pattern matching became Tondo's
  records, enums and `match`, and virtual threads are how every Tondo
  function already waits. What is gone is `null`, the exceptions no
  signature shows, and the virtual machine. [The page](comparison/java.md)
  says where modern Java does as well.
- **Kotlin.** `if` and `match` as expressions, `T?`, named arguments and
  structured concurrency came to Tondo from Kotlin, and the two read
  alike. Tondo has no `suspend`, no exceptions and no `null` arriving from
  another language. [The page](comparison/kotlin.md) has them side by
  side.
- **Swift.** Values copied on write, optionals, enums with data, counted
  memory and tasks that belong to their scope are Swift's too, and the
  first program of [the page](comparison/swift.md) is the same in both,
  line for line. Tondo has no classes and no `async`, and it collects the
  cycles that Swift leaves to `weak`.

## What the toolchain adds

Each of the three has good tools, and Go's and .NET's come close to complete:
Go has its race detector, fuzzing, benchmarks and `pprof`, and .NET has hot
reload and its diagnostic tools. Tondo puts its own of each in the one
binary that compiles, and goes further in a few places:

- **Every order of a test's tasks.** `tondo test` runs a concurrent test in
  the orders of its tasks that differ, and gives the one that fails to run
  again and to debug, where a race detector reports what one run happened to
  do.
- **A model checker in the standard library.** `std.model` checks a design,
  or the production function itself, in every state it can reach.
- **A recorded run to debug backwards.** `tondo debug --replay` steps back
  and answers `why` a value is what it is.
- **A dump that becomes a test.** `tondo dump test` writes the test that
  fails as the crash did, with its values.
- **Live builds of native code.** A running program takes in the code that
  was saved and keeps its state.
- **Tools for coding agents.** Every command gives JSON, every diagnostic
  says what to write, and `tondo mcp` serves all of it as tools.

[The toolchain](toolchain.md) shows each one.

## Where another is the better choice today

Tondo is not released, its language can still change, and it runs on Linux
and macOS only. Until that changes, a program that has to last should not
depend on it. Beyond that:

- **Go** has years of production behind it, a promise of compatibility, and a
  package for nearly everything.
- **Rust** is the choice where a program cannot afford a runtime or a
  reference count: kernels, firmware, and code where every allocation is
  placed by hand.
- **C#** has Windows, desktop and mobile interfaces, and a runtime that
  optimizes a program from how it actually runs, which in Tondo is still
  research.
- **Python** has data, science and machine learning, a REPL and notebooks.
- **TypeScript** has the browser, and one language on both sides of a web
  application.
- **Java** and **Kotlin** have the JVM's thirty years of libraries, and
  Android.
- **Swift** has Apple's platforms and everything with a user interface on
  them.
