# Tondo for someone who writes Swift

Swift and Tondo agree on the things that are hardest to add to a language
later. Arrays, dictionaries and strings are values, copied by sharing until
one changes. A value that may be missing is an optional that has to be
opened. Enums carry data and a `switch` covers every case. Memory is
counted, not traced. Tasks belong to the scope that started them. Tondo's
specification names Swift for two of those. Where they part is in what
Swift keeps beside its values: classes, `async`, and a standard library
that leaves servers to packages. This page writes the same small programs
in both. Every one was run, and prints what is shown.

- [The short version](#the-short-version)
- [Values, and where they stop](#values-and-where-they-stop)
- [A value that may be missing](#a-value-that-may-be-missing)
- [Errors](#errors)
- [Enums and switch](#enums-and-switch)
- [Protocols, traits and generics](#protocols-traits-and-generics)
- [Collections](#collections)
- [Tasks](#tasks)
- [Deadlines](#deadlines)
- [Shared state](#shared-state)
- [JSON](#json)
- [Tests](#tests)
- [An HTTP server and its client](#an-http-server-and-its-client)
- [The toolchain, command by command](#the-toolchain-command-by-command)
- [Habits to leave behind](#habits-to-leave-behind)
- [Where Swift is the better choice today](#where-swift-is-the-better-choice-today)

## The short version

| | Swift | Tondo |
| --- | --- | --- |
| Arrays, dictionaries, strings, structs | values, copied on write | values, copied on write |
| Classes | references, shared; what the frameworks are made of | none: something shared is a `Ref[T]` |
| A value that may be missing | `T?` | `T?` |
| A function fails | `throws`, or `throws(E)` since Swift 6 | returns `T ! E`, passed on with `?` |
| One of several shapes | an `enum` with associated values | an `enum` with data |
| Protocols | conformance by `extension`; `some` and `any` | traits by `impl`; no `any`: every call is resolved when compiling |
| Memory | reference counts; cycles are the programmer's, with `weak` | reference counts, and a collector that finds cycles |
| A function that waits | `async`, and so are its callers | written like any other |
| Tasks | structured, with `async let` and task groups | structured: a task belongs to its block |
| Shared state | actors; data races rejected in Swift 6 | `sync.Mutex[T]` and channels; data races rejected |
| A deadline | written by hand over a task group | `within`, around a block |
| An integer overflows | traps | panics |
| HTTP server, SQL, command lines | packages | the standard library |
| Where it runs best | Apple's platforms | Linux and macOS servers |

## Values, and where they stop

This first program is the same in both, and prints the same:

```swift
struct Point {
    var x: Int
    var y: Int
}

let original = [1, 2, 3]
var copy = original // an array is a value: the two are apart from here on
copy[0] = 99
copy.append(4)
print(original, copy)

let home = Point(x: 1, y: 2)
var away = home // a struct is a value too; a class would be shared
away.x = 10
print(home.x, away.x)
```

```text
[1, 2, 3] [99, 2, 3, 4]
1 10
```

```tondo
import std.console

type Point = { x: Int, y: Int }

fn main() {
    let original = [1, 2, 3]
    var copy = original                 // a copy: the two are apart from here on
    copy[0] = 99
    copy.push(4)
    console.println("{original} {copy}")

    let home = Point { x: 1, y: 2 }
    var away = home
    away.x = 10
    console.println("{home.x} {away.x}")
}
```

```text output
[1, 2, 3] [99, 2, 3, 4]
1 10
```

- `let` and `var` mean what they mean in Swift, down through the whole
  value: a `let` array cannot be appended to.
- A function is let into a caller's value with `mut`, written at both
  ends as `inout` and `&` are: `fn scale(values: mut Array[Float])`,
  `scale(mut prices)`.
- Swift has a second kind of type, the class, which is shared, and the
  frameworks an app is built on are classes. Tondo has only the first
  kind. Something that several parts of a program must see as the same
  thing is a `Ref[T]`, written where it is meant, and it does not change:
  state that changes and is shared goes in a `sync.Mutex`.
- A record is `type Point = { x: Int, y: Int }`, made as `Point { x: 1, y:
  2 }`. It has `==` and can be a dictionary's key with nothing to conform
  to, and `point with { x: 10 }` gives a changed copy in one expression.

## A value that may be missing

Optionals are optionals:

```swift
struct User {
    let name: String
    let email: String?
}

func find(_ users: [User], _ name: String) -> User? {
    users.first { $0.name == name }
}

let users = [User(name: "ada", email: "ada@example.com"), User(name: "alan", email: nil)]
for name in ["ada", "alan", "grace"] {
    // A user that may not be there, with an email that may not be there.
    let email = find(users, name)?.email ?? "no email"
    print("\(name): \(email)")
}
```

```tondo
import std.console

type User = { name: String, email: String? }

fn find(users: Array[User], name: String): User? {
    users.find((user) { user.name == name })
}

fn main() {
    let users = [
        User { name: "ada", email: "ada@example.com" },
        User { name: "alan", email: none },
    ]
    for name in ["ada", "alan", "grace"] {
        // A user that may not be there, with an email that may not be there.
        let email = find(users, name)?.email ?? "no email"
        console.println("{name}: {email}")
    }
}
```

```text output
ada: ada@example.com
alan: no email
grace: no email
```

- `?.` and `??` are `?.` and `??`. `match` is `if let` with its `else`,
  and `?? return` does what a `guard let ... else { return }` does.
- There is no `!`. `expect("why it must be there")` panics with the reason.
- `none` and `some(x)` are what `nil` and `.some(x)` are. A plain value
  becomes an optional where one is expected, as in Swift.
- `?` after an optional passes a `none` on from a function that returns an
  optional.

## Errors

Since Swift 6 a function can say what it throws, and then the two
languages are close:

```swift
enum ConfigError: Error, CustomStringConvertible {
    case missing(String)
    case notANumber(String, String)
    case outOfRange(String, Int)

    var description: String {
        switch self {
        case .missing(let key): "\(key) is missing"
        case .notANumber(let key, let text): "\(key): \"\(text)\" is not a number"
        case .outOfRange(let key, let value): "\(key): \(value) is out of range"
        }
    }
}

// `throws(ConfigError)` says what it fails with. A plain `throws` says only that it may.
func port(_ settings: [String: String], _ key: String) throws(ConfigError) -> Int {
    guard let text = settings[key] else { throw .missing(key) }
    guard let value = Int(text) else { throw .notANumber(key, text) }
    guard (1...65535).contains(value) else { throw .outOfRange(key, value) }
    return value
}

func address(_ settings: [String: String]) throws(ConfigError) -> String {
    "0.0.0.0:\(try port(settings, "port"))"
}

for settings in [["port": "8080"], [:], ["port": "http"], ["port": "70000"]] {
    do {
        print(try address(settings))
    } catch {
        print("error: \(error)")
    }
}
```

In Tondo the error is always typed, and it is part of the result. There is
no `try` to mark a call and no `do` and `catch` to receive it:

```tondo
import std.console

enum ConfigError {
    Missing(String)
    NotANumber(String, String)
    OutOfRange(String, Int)
}

impl Display for ConfigError {
    fn display(self): String {
        match self {
            Missing(key) => "{key} is missing"
            NotANumber(key, text) => "{key}: \"{text}\" is not a number"
            OutOfRange(key, value) => "{key}: {value} is out of range"
        }
    }
}

fn port(settings: Map[String, String], key: String): Int ! ConfigError {
    let text = settings.get(key) ?? fail Missing(key)
    let value = Int.parse(text).mapErr((_) { ConfigError.NotANumber(key, text) })?
    if value < 1 or value > 65535 {
        fail OutOfRange(key, value)
    }
    value
}

/// The address to listen on: a failure of `port` passes through `?`.
fn address(settings: Map[String, String]): String ! ConfigError {
    "0.0.0.0:{port(settings, "port")?}"
}

fn main() {
    for settings in [["port": "8080"], [:], ["port": "http"], ["port": "70000"]] {
        match address(settings) {
            ok(found) => console.println(found)
            err(error) => console.println("error: {error}")
        }
    }
}
```

```text output
0.0.0.0:8080
error: port is missing
error: port: "http" is not a number
error: port: 70000 is out of range
```

- `Int ! ConfigError` is `throws(ConfigError) -> Int`. `?` after a call is
  `try` before it, and `fail` is `throw`.
- A failure is handled with `match` on the result, which is an ordinary
  value: it can be stored, put in an array, or sent to another task.
- A function that can fail in two ways says `T ! (ReadError | ParseError)`,
  a union the compiler makes. Swift's typed throws take one type, so mixed
  failures go back to an untyped `throws`.
- A result that is not used does not compile.

## Enums and switch

Nothing to translate but the punctuation:

```swift
import Foundation

enum Shape {
    case circle(Double)
    case rectangle(Double, Double)
    case triangle(Double, Double, Double)
}

func area(_ shape: Shape) -> Double {
    switch shape {
    case .circle(let radius):
        return Double.pi * radius * radius
    case .rectangle(let width, let height):
        return width * height
    case .triangle(let a, let b, let c):
        let s = (a + b + c) / 2
        return (s * (s - a) * (s - b) * (s - c)).squareRoot()
    }
}

for shape in [Shape.circle(1), .rectangle(2, 3.5), .triangle(3, 4, 5)] {
    print(String(format: "%.2f", area(shape)))
}
```

```tondo
import std.console
import std.math

enum Shape {
    Circle(Float)
    Rectangle(Float, Float)
    Triangle(Float, Float, Float)
}

fn area(shape: Shape): Float {
    match shape {
        Circle(radius) => math.Pi * radius * radius
        Rectangle(width, height) => width * height
        Triangle(a, b, c) => {
            let s = (a + b + c) / 2.0
            math.sqrt(s * (s - a) * (s - b) * (s - c))
        }
    }
}

fn main() {
    for shape in [Shape.Circle(1.0), Shape.Rectangle(2.0, 3.5), Shape.Triangle(3.0, 4.0, 5.0)] {
        console.println("{area(shape):.2}")
    }
}
```

```text output
3.14
7.00
6.00
```

- A variant is `Shape.Circle(1.0)`, and a pattern names its parts without
  `let`.
- `match` is an expression, and so is `if`. A function's last expression is
  its value.
- A recursive enum needs no `indirect`.
- `math.sqrt` of a negative number is `NaN`, as `squareRoot()` is.
  `math.checkedSqrt` fails instead, for code that would rather have the
  failure in the type.

## Protocols, traits and generics

```swift
protocol Describe {
    func describe() -> String
}

struct Book {
    let title: String
    let pages: Int
}

struct Song {
    let title: String
    let seconds: Int
}

extension Book: Describe {
    func describe() -> String {
        "\(title), \(pages) pages"
    }
}

extension Song: Describe {
    func describe() -> String {
        let rest = seconds % 60
        return "\(title), \(seconds / 60):\(rest < 10 ? "0" : "")\(rest)"
    }
}

/// Any type that can describe itself; resolved when compiling.
func announce(_ item: some Describe) -> String {
    "Now: \(item.describe())"
}

/// The largest of some values of any type that has an order.
func largest<T: Comparable>(_ items: [T]) -> T? {
    guard var best = items.first else { return nil }
    for item in items where item > best {
        best = item
    }
    return best
}

print(announce(Book(title: "Dune", pages: 612)))
print(announce(Song(title: "Hey Jude", seconds: 431)))
print(largest([3, 9, 4]) as Any, largest(["pear", "apple"]) as Any, largest([Int]()) as Any)
```

```text
Now: Dune, 612 pages
Now: Hey Jude, 7:11
Optional(9) Optional("pear") nil
```

A trait is a protocol, and an `impl` is an extension that conforms:

```tondo
import std.console

trait Describe {
    fn describe(self): String
}

type Book = { title: String, pages: Int }
type Song = { title: String, seconds: Int }

impl Describe for Book {
    fn describe(self): String {
        "{self.title}, {self.pages} pages"
    }
}

impl Describe for Song {
    fn describe(self): String {
        "{self.title}, {self.seconds / 60}:{self.seconds % 60:02}"
    }
}

/// Any type that can describe itself; resolved when compiling.
fn announce[T: Describe](item: T): String {
    "Now: {item.describe()}"
}

/// The largest of some values of any type that has an order.
fn largest[T: Ordered](items: Array[T]): T? {
    var best = items.first()?
    for item in items {
        if item > best {
            best = item
        }
    }
    best
}

fn main() {
    console.println(announce(Book { title: "Dune", pages: 612 }))
    console.println(announce(Song { title: "Hey Jude", seconds: 431 }))
    console.println("{largest([3, 9, 4])} {largest(["pear", "apple"])} {largest[Int]([])}")
}
```

```text output
Now: Dune, 612 pages
Now: Hey Jude, 7:11
some(9) some(pear) none
```

- `fn announce[T: Describe](item: T)` is `func announce(_ item: some
  Describe)`. Generics are compiled for each type they are used with.
- There is no `any Describe`. Several types in one array are an enum, or a
  union, `Book | Song`; behaviour chosen as the program runs is a function
  value.
- Methods are functions named with their type, `fn Book.summary(self)`,
  written outside it. There are no computed properties: a value that is
  computed is a method.
- Arguments have no labels by default. A call can name any of them,
  `resize(image, width: 800)`, and the names are the parameters' own.
- There is no overloading: one name, one function.

## Collections

Swift's `Dictionary` has no order, so the program keeps one:

```swift
let text = "to be or not to be that is the question to be"
let words = text.split(separator: " ").map(String.init)

var counts: [String: Int] = [:]
var seen: [String] = [] // a Dictionary has no order: keep the order they came in
for word in words {
    if counts[word] == nil {
        seen.append(word)
    }
    counts[word, default: 0] += 1
}
// The three most frequent; those that tie, in the order they came.
let ranked = seen.enumerated().sorted { a, b in
    (counts[a.element]!, -a.offset) > (counts[b.element]!, -b.offset)
}
print(ranked.prefix(3).map { "\($0.element) \(counts[$0.element]!)" }.joined(separator: ", "))

let lengths = words.map(\.count)
print("\(lengths.filter { $0 > 2 }.count) long words, \(lengths.reduce(0, +)) letters")
```

A Tondo map keeps the order its keys were added in:

```tondo
import std.console

fn main() {
    let text = "to be or not to be that is the question to be"
    var counts: Map[String, Int] = [:]
    for word in text.split(" ") {
        counts[word, default: 0] += 1
    }
    // The three most frequent; those that tie stay in the order they came.
    let top = counts.entries().collect().sortedBy((entry) { -entry.1 }).take(3)
    console.println(top.map((entry) { "{entry.0} {entry.1}" }).join(", "))

    let lengths = text.split(" ").map((word) { word.length() })
    console.println("{lengths.filter((n) { n > 2 }).length()} long words, {lengths.sum()} letters")
}
```

```text output
to 3, be 3, or 1
4 long words, 34 letters
```

- `Array[T]`, `Map[K, V]`, `Set[T]` and `Deque[T]` are the collections,
  with `map`, `filter`, `sortedBy`, `take`, `sum` and `join` as methods.
- A closure is `(word) { word.length() }`. There is no `$0`, and no
  trailing closure outside the parentheses.
- `"{name} is {age}"` interpolates, with formats after a colon:
  `{price:.2}`.
- A string is indexed with integers: `text[0]`, `text[1:4]`. Positions
  count Unicode scalars, not grapheme clusters; normalization and the rest
  of Unicode are in `std.text`.

## Tasks

Swift's structured concurrency: child tasks that their scope waits for.

```swift
// Stands for a request that takes a while. It waits, so it is `async`,
// and so is everything that calls it.
func fetch(_ name: String, _ delay: Int) async -> String {
    try? await Task.sleep(for: .milliseconds(delay))
    return "\(name) in \(delay) ms"
}

let clock = ContinuousClock()
let started = clock.now
// Three child tasks, which this scope does not leave before they end.
async let users = fetch("users", 300)
async let orders = fetch("orders", 500)
async let prices = fetch("prices", 100)
for result in await [users, orders, prices] {
    print(result)
}
// One after another they would take 900 ms.
print("all three in under 800 ms: \(clock.now - started < .milliseconds(800))")
```

Tondo's is the same structure, without `async`. `fetch` is an ordinary
function, and any function can wait:

```tondo
import std.console
import std.time

/// Stands for a request that takes a while.
fn fetch(name: String, delay: Int): String {
    time.sleep(time.milliseconds(delay))
    "{name} in {delay} ms"
}

fn main() {
    let started = time.now()
    var group = Group[String]()
    group.add(spawn fetch("users", 300))
    group.add(spawn fetch("orders", 500))
    group.add(spawn fetch("prices", 100))
    for result in group.wait() {            // in the order they were added
        console.println(result)
    }
    // One after another they would take 900 ms.
    console.println("all three in under 800 ms: {started.elapsed().milliseconds() < 800}")
}
```

```text output
users in 300 ms
orders in 500 ms
prices in 100 ms
all three in under 800 ms: true
```

- No `async`, no `await`, and no `Task { }` to cross from code that is not
  `async`. A library is written once.
- `spawn f()` is `async let`: a child task that its block waits for. It
  gives a `Join[T]`, which can be kept, passed on or put in a `Group`.
- Tasks run on every core. One that computes for long does not hold up the
  others, and nothing has to be moved off a main actor.
- `spawn detached f()` is `Task.detached`.

## Deadlines

Swift's library has no deadline. It is written with a task group that
races the work against a sleep:

```swift
struct TimedOut: Error {}

// The library has no deadline: this races the work against a sleep,
// and cancels whichever is left.
func within<T: Sendable>(
    _ limit: Duration, _ work: @escaping @Sendable () async throws -> T
) async throws -> T {
    try await withThrowingTaskGroup(of: T.self) { group in
        group.addTask { try await work() }
        group.addTask {
            try await Task.sleep(for: limit)
            throw TimedOut()
        }
        defer { group.cancelAll() }
        return try await group.next()!
    }
}

func fetch(_ delay: Int) async throws -> String {
    try await Task.sleep(for: .milliseconds(delay))
    return "answered in \(delay) ms"
}

for delay in [10, 5000] {
    do {
        print(try await within(.milliseconds(250)) { try await fetch(delay) })
    } catch {
        print("gave up after 250 ms")
    }
}
```

Tondo has it in the language, around a block:

```tondo
import std.console
import std.time

fn fetch(delay: Int): String {
    time.sleep(time.milliseconds(delay))
    "answered in {delay} ms"
}

fn main() {
    for delay in [10, 5000] {
        // The deadline holds for all the block calls, and for the tasks it starts.
        match within time.milliseconds(250) { fetch(delay) } {
            ok(answer) => console.println(answer)
            err(_) => console.println("gave up after 250 ms")
        }
    }
}
```

```text output
answered in 10 ms
gave up after 250 ms
```

- `within` holds for everything the block calls and every task it starts.
  When the time passes they are cancelled, their `defer`s run, and
  `within` gives `err(time.TimedOut)`.
- Every wait can be cancelled. Nothing has to check `Task.isCancelled`.
- What a task-local value carries is a `context`: declared once, typed,
  read by name in any function below, inherited by tasks.

## Shared state

Swift 6 rejects data races when compiling, as Tondo does. Its tool for
shared state is the actor:

```swift
// An actor keeps its state to itself: the only way in is its methods,
// one caller at a time. The compiler rejects any other access.
actor Hits {
    private(set) var counts: [String: Int] = [:]

    func add(_ page: String) {
        counts[page, default: 0] += 1
    }
}

let hits = Hits()
await withTaskGroup(of: Void.self) { group in
    for n in 0..<50 {
        group.addTask {
            for _ in 0..<100 {
                await hits.add(n % 2 == 0 ? "/home" : "/about")
            }
        }
    }
}
let counts = await hits.counts
print(counts["/home"]!, counts["/about"]!)
```

Tondo's is a mutex that holds the value. `update` has it to itself while
its function runs, and the call is not `async`:

```tondo
import std.console
import std.sync

fn work(hits: sync.Mutex[Map[String, Int]], page: String) {
    for _ in 0..100 {
        hits.update((counts) { counts[page, default: 0] += 1 })
    }
}

fn main() {
    let empty: Map[String, Int] = [:]
    let hits = sync.Mutex(empty)
    var group = Group[Unit]()
    for n in 0..50 {
        group.add(spawn work(hits, if n % 2 == 0 { "/home" } else { "/about" }))
    }
    group.wait()
    let counts = hits.get()
    console.println("{counts.get("/home") ?? 0} {counts.get("/about") ?? 0}")
}
```

```text output
2500 2500
```

- What can be given to a task is checked when compiling, as `Sendable` is.
- An actor's methods can interleave at each `await` inside them. `update`
  runs its function from start to end with the value locked, so the check
  and the change are one step.
- `std.sync` has `Atomic`, `RwLock`, `Semaphore` and shared collections,
  and `std.executor` has actors with a bounded mailbox.

## JSON

`Codable`, from Foundation:

```swift
import Foundation

struct Person: Codable {
    var name: String
    var age: Int
    var languages: [String]?
    var email: String?
}

let text = #"{"name": "Ada", "age": 36, "languages": ["en", "fr"]}"#
do {
    var person = try JSONDecoder().decode(Person.self, from: Data(text.utf8))
    print("\(person.name), \(person.age), \(person.languages ?? []), \(person.email ?? "nil")")
    person.age = 37
    let encoder = JSONEncoder()
    encoder.outputFormatting = .sortedKeys
    print(String(decoding: try encoder.encode(person), as: UTF8.self))
} catch {
    print(error)
}

// The wrong type in a field is an error that says where.
do {
    _ = try JSONDecoder().decode(Person.self, from: Data(#"{"name": "Alan", "age": "forty"}"#.utf8))
} catch DecodingError.typeMismatch(_, let context) {
    print("\(context.codingPath.map(\.stringValue).joined(separator: ".")): \(context.debugDescription)")
} catch {
    print(error)
}
```

```text
Ada, 36, ["en", "fr"], nil
{"age":37,"languages":["en","fr"],"name":"Ada"}
age: Expected to decode Int but found a string instead.
```

In Tondo any record is read and written, with nothing to conform to:

```tondo
import std.console
import std.json

type Person = {
    name: String
    age: Int
    languages: Array[String] = []
    email: String?
}

fn main() {
    let text = r"""{"name": "Ada", "age": 36, "languages": ["en", "fr"]}"""
    match json.decode[Person](text) {
        ok(person) => {
            console.println("{person.name}, {person.age}, {person.languages}, {person.email}")
            console.println(json.encode(person with { age: 37 }))
        }
        err(error) => console.println("{error}")
    }
    // The wrong type in a field is an error that says where.
    match json.decode[Person](r"""{"name": "Alan", "age": "forty"}""") {
        ok(person) => console.println(person.name)
        err(error) => console.println("{error}")
    }
}
```

```text output
Ada, 36, [en, fr], none
{"name":"Ada","age":37,"languages":["en","fr"],"email":null}
expected an integer, found a string at $.age (line 1, column 32)
```

- A field with a default is optional on the wire. Synthesized `Codable`
  has no defaults for missing keys, which is why `languages` is an
  optional in the Swift version.
- `@name("user_id")` on a field is a `CodingKeys` entry.
- The same record reads and writes YAML, TOML, CBOR and MessagePack, and
  `@validate.Min(0)` on a field is checked by the decoder.

## Tests

With Swift Testing, in a package:

```swift
import Testing

@testable import Titles

@Test func capitalizesEachWord() {
    #expect(titleCase("the old man and the sea") == "The Old Man And The Sea")
}

@Test func keepsOneSpaceBetweenWords() {
    #expect(titleCase("  war   AND peace ") == "War And Peace")
    #expect(titleCase("") == "")
}
```

In Tondo a test is a `test` block, in a file beside the code it tests:

```tondo test
/// The words of a title, each with a capital.
fn titleCase(text: String): String {
    text.split(" ").filter((word) { not word.isEmpty() }).map((word) { "{word[0:1].toUpper()}{word[1:].toLower()}" }).join(" ")
}

test capitalizesEachWord {
    assert(titleCase("the old man and the sea") == "The Old Man And The Sea")
}

test keepsOneSpaceBetweenWords {
    assert(titleCase("  war   AND peace ") == "War And Peace")
    assert(titleCase("") == "")
}
```

- `assert(a == b)` prints both sides when it fails, as `#expect` does.
- `tondo test` runs them, with benchmarks, fuzz tests, snapshots and
  coverage in the same command.
- A test that starts tasks is run in the orders that could change its
  result.

## An HTTP server and its client

Swift's standard library and Foundation have a client, `URLSession`, and no
server: a service is written with a package, as Vapor or Hummingbird over
SwiftNIO. In Tondo both are in the standard library:

```tondo
import std.console
import std.http

type Greeting = { message: String, length: Int }

fn main(): !http.HttpError {
    var app = http.App()
    app.get("/hello/:name", (request) {
        let name = request.param("name")
        http.Reply.json(Greeting { message: "Hello, {name}!", length: name.length() })
    })
    let server = app.start()?                   // on a free port
    defer server.stop()

    let greeting: Greeting = http.get("{server.url()}/hello/Ada")?.json()?
    console.println("{greeting.message} ({greeting.length})")
    match http.get("{server.url()}/nothing") {
        err(Status(response)) => console.println("{response.status()}")
        _ => console.println("unexpected")
    }
}
```

```text output
Hello, Ada! (3)
404
```

- A route has its parameters, and a typed handler takes them as arguments:
  `app.get("/users/:id", (id: Int) { find(id) })`.
- WebSockets, gRPC from `.proto` files, SQL, templates and a command-line
  parser are in the standard library too.

## The toolchain, command by command

| Swift | Tondo | |
| --- | --- | --- |
| `swift run` | `tondo run` | compiles, keeps the build, runs; a project or one file |
| `swift build -c release` | `tondo build` | one executable; `--static`, `--target aarch64-linux` |
| `Package.swift` | `tondo.toml` | data, not a program |
| `swift test` | `tondo test` | with `--bench`, `--fuzz`, `--coverage` |
| `swift-format`, SwiftLint | `tondo fmt`, `tondo check`, `tondo fix` | one layout, no configuration |
| `swift package add-dependency` | `tondo add` | locked in `tondo.lock` |
| `lldb` | `tondo debug` | also backwards, over a recording |
| Instruments | `tondo profile` | writes pprof |
| SourceKit-LSP | `tondo lsp` | in the same binary |
| a module map, a bridging header | `extern fn`, `tondo bind` | declarations written from a C header |
| macros | `derive` and generators | part of the build |

## Habits to leave behind

| In Swift | In Tondo |
| --- | --- |
| `func f(_ a: Int) -> String` | `fn f(a: Int): String` |
| `f(a, label: b)` required | `f(a, b)`, or `f(a, name: b)` when it helps |
| `nil`, `x ?? y`, `x?.y`, `x!` | `none`, `x ?? y`, `x?.y`, `x.expect("...")` |
| `if let x = y { }`, `guard let` | `match y { some(x) => ..., none => ... }` |
| `throws`, `try`, `do`, `catch` | `T ! E`, `?`, `match` |
| `switch`, `case .circle(let r):` | `match`, `Shape.Circle(r) =>` |
| `struct`, `class`, `protocol`, `extension` | `type`, `trait`, `impl` |
| `Point(x: 1, y: 2)` | `Point { x: 1, y: 2 }` |
| `inout`, `&value` | `mut`, `mut value` |
| `[T]`, `[K: V]`, `Set<T>` | `Array[T]`, `Map[K, V]`, `Set[T]` |
| `{ $0 * 2 }` | `(x) { x * 2 }` |
| `"\(name) is \(age)"` | `"{name} is {age}"` |
| `a + b` for strings and arrays | `"{a}{b}"`, `a.concat(b)` |
| `items.count`, `text.count` | `items.length()`, `text.length()` |
| `for x in xs`, `while c` | `for x in xs`, `for c` |
| `0..<n`, `0...n` | `0..n`, `0..=n` |
| `&&`, `\|\|`, `!` | `and`, `or`, `not` |
| `Int(x)` | `Int(x)`, which fails if it does not fit |
| `async`, `await`, `Task { }` | nothing: every function can wait; `spawn` |
| `async let`, `withTaskGroup` | `spawn`, a `Group` |
| `actor` | `sync.Mutex[T]`, or `executor.actor` |
| `weak`, `unowned` | `WeakRef[T]`, seldom: cycles are collected |
| `private`, `internal`, `public` | nothing, or `pub` |
| `import Foundation` | `import std.json`, `std.time`, `std.fs`: a module for each |

`tondo guide pitfalls` has these and the error each gives.

## Where Swift is the better choice today

- **Apple's platforms.** An app for a phone, a watch or a desktop is
  Swift's, with SwiftUI and every framework beside it. Tondo has no user
  interface.
- **Its tools and its age.** Xcode, Instruments, and a language that has
  shipped to millions of devices. Tondo is in early development and can
  still change.
- **Predictable memory with no collector at all**, where cycles are known
  and broken by hand.
- **Embedded and Windows**, which Swift reaches and Tondo does not.

What Tondo offers against that is Swift's values and optionals on a server:
functions of one colour, a deadline in the language, cycles that do not
leak, and a standard library with the server, the database and the command
line already in it.

Back to [Tondo next to other languages](../comparison.md).
