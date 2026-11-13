# Tondo for someone who writes Kotlin

Of the languages on these pages, Kotlin is the one whose code looks most
like Tondo's. `if` and `when` are expressions, strings interpolate, a type
with a `?` may be missing, arguments have names and defaults, and
coroutines belong to a scope that waits for them. Tondo's specification
names Kotlin as where several of those came from. What it left behind is
the JVM underneath, and the things Kotlin keeps for its sake: classes and
inheritance, exceptions, `null` arriving from Java, and `suspend`. This
page writes twelve small programs in both. Every one was run, and prints
what is shown.

- [The short version](#the-short-version)
- [Values and references](#values-and-references)
- [A value that may be missing](#a-value-that-may-be-missing)
- [Errors](#errors)
- [One of several things](#one-of-several-things)
- [Interfaces, traits and generics](#interfaces-traits-and-generics)
- [Collections](#collections)
- [Tasks](#tasks)
- [Deadlines](#deadlines)
- [Shared state](#shared-state)
- [JSON](#json)
- [Tests](#tests)
- [An HTTP server and its client](#an-http-server-and-its-client)
- [The toolchain, command by command](#the-toolchain-command-by-command)
- [Habits to leave behind](#habits-to-leave-behind)
- [Where Kotlin is the better choice today](#where-kotlin-is-the-better-choice-today)

## The short version

| | Kotlin | Tondo |
| --- | --- | --- |
| A value that may be missing | `T?`, checked; `null` can still come in from Java | `T?`, checked; there is no `null` anywhere |
| A function fails | throws; no signature says so | returns `T ! E`, passed on with `?` |
| `val` | fixes the name; a `MutableList` behind it still changes | `let` fixes the value, all of it |
| Assigning a list or an object | shares it | copies it; the storage is shared until one changes |
| Data | classes, data classes, objects | records, enums and tuples: all values |
| One of several shapes | a sealed interface and `when` | an `enum` with data and `match` |
| Reuse | inheritance, interfaces, delegation | traits and composition; there is no inheritance |
| A function that waits | `suspend`, and so are its callers | written like any other |
| Coroutines and tasks | belong to a scope, from a library | belong to their block, in the language |
| A data race | possible | does not compile |
| An integer overflows | wraps | panics, in every build |
| Coroutines, JSON | libraries, one with a compiler plugin | the language and the standard library |
| What ships | a jar for a JVM, or Kotlin/Native | one native executable |

## Values and references

`val` fixes the name. The list it names is an object, shared by every name
that was given it. A data class is not changed in place: `copy` makes the
changed one:

```kotlin
data class Point(val x: Int, val y: Int)

fun main() {
    val original = mutableListOf(1, 2, 3)
    val alias = original // the same list under another name: `val` fixes the name, not the list
    alias[0] = 99
    alias.add(4)
    println("$original $alias")

    val home = Point(1, 2)
    val away = home.copy(x = 10) // a data class is not changed: a changed copy is asked for
    println("${home.x} ${away.x}")
}
```

```text
[99, 2, 3, 4] [99, 2, 3, 4]
1 10
```

In Tondo a list is a value. Assigning it gives the other name its own, and
a `let` cannot change at all:

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

- There is no `List` beside `MutableList`. A read-only view in Kotlin can
  still change under its reader, through the name that kept the mutable
  type. In Tondo what a function was given is its own.
- The copy costs nothing until one of the two changes: they share their
  storage until then.
- `point with { x: 10 }` is `point.copy(x = 10)`, for any record.
- `let` and `var` are `val` and `var`. `==` compares values, as in Kotlin,
  and there is no `===`, since two values have no identity to compare.

## A value that may be missing

Here the two languages are the same idea with different spelling. Kotlin:

```kotlin
data class User(val name: String, val email: String?)

fun find(users: List<User>, name: String): User? = users.find { it.name == name }

fun main() {
    val users = listOf(User("ada", "ada@example.com"), User("alan", null))
    for (name in listOf("ada", "alan", "grace")) {
        // A user that may not be there, with an email that may not be there.
        val email = find(users, name)?.email ?: "no email"
        println("$name: $email")
    }
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

- `?.` is `?.`, and `?:` is `??`, which takes a `return` or a `fail` on its
  right as `?: return` does. `match` opens an option when both cases need
  code, and `?` after an expression passes a `none` on from a function that
  returns an option.
- There is no `!!`. `expect("why it must be there")` panics with the reason
  when the value is missing.
- Kotlin's guarantee stops at the edge of Kotlin: a value from Java has a
  platform type, and may be `null` whatever the code assumes. Tondo has no
  such edge. What comes from C is a `Pointer[T]?`, and has to be opened.
- An option of an option is two layers, `Int??`, where Kotlin's `Int??` is
  `Int?`. A map of optional values can tell a missing key from a key whose
  value is missing.

## Errors

Kotlin dropped Java's checked exceptions, so nothing says what a function
throws:

```kotlin
class ConfigException(message: String) : Exception(message)

fun port(settings: Map<String, String>, key: String): Int {
    val text = settings[key] ?: throw ConfigException("$key is missing")
    val value = text.toIntOrNull() ?: throw ConfigException("$key: \"$text\" is not a number")
    if (value !in 1..65535) {
        throw ConfigException("$key: $value is out of range")
    }
    return value
}

// Nothing in this signature says that it can fail, or with what.
fun address(settings: Map<String, String>): String = "0.0.0.0:${port(settings, "port")}"

fun main() {
    val all = listOf(mapOf("port" to "8080"), mapOf(), mapOf("port" to "http"), mapOf("port" to "70000"))
    for (settings in all) {
        try {
            println(address(settings))
        } catch (error: ConfigException) {
            println("error: ${error.message}")
        }
    }
}
```

Tondo went the other way: a failure a caller should handle is always in
the signature, as a value:

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

- `Int ! ConfigError` is what Kotlin's `Result<Int>` would be if it were
  typed by its error and used by the standard library. `Int.parse` returns
  one, as `toIntOrNull` returns a nullable, but with the reason.
- `?` after a call passes its failure on. It is an early return, like `?:
  return`, for errors.
- The error is an `enum`, and a `match` on it must cover its cases, as a
  `when` over a sealed type must.
- A panic is the other kind of failure: a bug. It is not caught in
  ordinary code. `recover { ... }` at the edge of a request turns one into
  a value.
- `defer` is `finally`, and `use { }` is a `defer` or a value that closes
  when its block ends.

## One of several things

A sealed interface and `when`, which needs no `else` and fails to compile
if a case is missing:

```kotlin
import java.util.Locale
import kotlin.math.PI
import kotlin.math.sqrt

sealed interface Shape

data class Circle(val radius: Double) : Shape

data class Rectangle(val width: Double, val height: Double) : Shape

data class Triangle(val a: Double, val b: Double, val c: Double) : Shape

// The interface is sealed, so `when` knows these are all the cases.
fun area(shape: Shape): Double = when (shape) {
    is Circle -> PI * shape.radius * shape.radius
    is Rectangle -> shape.width * shape.height
    is Triangle -> {
        val s = (shape.a + shape.b + shape.c) / 2
        sqrt(s * (s - shape.a) * (s - shape.b) * (s - shape.c))
    }
}

fun main() {
    for (shape in listOf(Circle(1.0), Rectangle(2.0, 3.5), Triangle(3.0, 4.0, 5.0))) {
        println(String.format(Locale.ROOT, "%.2f", area(shape)))
    }
}
```

A Tondo `enum` is the sealed interface and its data classes in one
declaration, and `match` takes a variant apart where `when` smart-casts:

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

- A variant carries its data in order, `Shape.Rectangle(width, height)`, or
  by name, `Rectangle { width: Float, height: Float }`.
- `match` is an expression, like `when`, and so is `if`. A function's last
  expression is its value, so there is no `return` and no `=` form.
- A record is `type Point = { x: Float, y: Float }`: a data class with no
  `class` behind it. There is nothing to inherit from and no `open`.
- `math.sqrt` of a negative number is `NaN`, as `sqrt` is.
  `math.checkedSqrt` fails instead, for code that would rather have the
  failure in the type.

## Interfaces, traits and generics

```kotlin
interface Describe {
    fun describe(): String
}

data class Book(val title: String, val pages: Int) : Describe {
    override fun describe() = "$title, $pages pages"
}

data class Song(val title: String, val seconds: Int) : Describe {
    override fun describe() = "$title, ${seconds / 60}:${(seconds % 60).toString().padStart(2, '0')}"
}

// The call goes through the interface, found as the program runs.
fun announce(item: Describe) = "Now: ${item.describe()}"

fun <T : Comparable<T>> largest(items: List<T>): T? {
    var best = items.firstOrNull() ?: return null
    for (item in items) {
        if (item > best) {
            best = item
        }
    }
    return best
}

fun main() {
    println(announce(Book("Dune", 612)))
    println(announce(Song("Hey Jude", 431)))
    println("${largest(listOf(3, 9, 4))} ${largest(listOf("pear", "apple"))} ${largest(listOf<Int>())}")
}
```

```text
Now: Dune, 612 pages
Now: Hey Jude, 7:11
9 pear null
```

A Tondo trait is implemented with an `impl` outside the type. Generic
functions are compiled for each type, as an `inline` function with a
`reified` parameter is:

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

- `impl Describe for Book` can be written by whoever owns the trait, for a
  type they did not write. It is what an extension function does, except
  that the type then really has the trait, and can be passed where one is
  asked for.
- Type arguments are never erased, and an `Array[Int]` holds integers, not
  boxes.
- Methods are functions named with their type, `fn Book.summary(self)`,
  written outside it. There are no companion objects, no `object`, and no
  properties with getters: a field is a field.
- There is no overloading. Defaults and named arguments do that job, as
  they often do in Kotlin.

## Collections

Kotlin's collection functions are the model for what a standard library
should have:

```kotlin
fun main() {
    val text = "to be or not to be that is the question to be"
    val words = text.split(" ")

    // The counts keep the order the words came in, and the sort is stable.
    val top = words.groupingBy { it }.eachCount().entries.sortedByDescending { it.value }.take(3)
    println(top.joinToString(", ") { "${it.key} ${it.value}" })

    val lengths = words.map { it.length }
    println("${lengths.count { it > 2 }} long words, ${lengths.sum()} letters")
}
```

Tondo's read much the same, with a closure written `(x) { ... }` where
Kotlin writes `{ it ... }`:

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
  each one type that is a value. A map keeps the order its keys were added
  in, as `LinkedHashMap` does.
- There is no `it`. A closure names its parameters.
- `"{name} is {age}"` interpolates any expression, with formats after a
  colon, `{price:.2}`, where Kotlin's `"$name"` needs `String.format`.
- `for i in 0..n` leaves `n` out, as `until` does; `0..=n` takes it in.

## Tasks

Structured concurrency is Kotlin's gift to the rest: a scope does not end
before the coroutines it started.

```kotlin
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import kotlin.system.measureTimeMillis

// Stands for a request that takes a while. It waits, so it is `suspend`,
// and so is everything that calls it.
suspend fun fetch(name: String, delay: Long): String {
    delay(delay)
    return "$name in $delay ms"
}

fun main() = runBlocking {
    val took = measureTimeMillis {
        // A scope does not end before the coroutines it started.
        val results = coroutineScope {
            listOf("users" to 300L, "orders" to 500L, "prices" to 100L)
                .map { (name, delay) -> async { fetch(name, delay) } }
                .awaitAll()
        }
        results.forEach(::println)
    }
    // One after another they would take 900 ms.
    println("all three in under 800 ms: ${took < 800}")
}
```

Tondo built that into the language, and without the colour. `fetch` is an
ordinary function, and any function can wait:

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

- There is no `suspend`, no `runBlocking` to cross from one world to the
  other, no dispatcher to choose and no library to add. Tasks run on every
  core, and a call that blocks does not starve the others.
- `spawn f()` is `async { f() }`: it gives a `Join[T]`, and `wait()` is
  `await()`. The block that spawned it is its scope.
- A `Group` is `awaitAll`, and gives its results in the order the tasks
  were added. In a group of tasks that can fail, the first error cancels
  the ones still running.
- `spawn detached f()` is `GlobalScope.launch`, and as rare.
- Channels and `select` are in the language. A `Flow` is an iterator, or a
  channel when it crosses tasks.

## Deadlines

```kotlin
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeoutOrNull

suspend fun fetch(delay: Long): String {
    delay(delay)
    return "answered in $delay ms"
}

fun main() = runBlocking {
    for (delay in listOf(10L, 5000L)) {
        // The limit holds for all the block calls, and for the coroutines it starts.
        val answer = withTimeoutOrNull(250) { fetch(delay) }
        println(answer ?: "gave up after 250 ms")
    }
}
```

`within` is `withTimeout`, for any code:

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

- Every wait can be cancelled, including the ones in code that was never
  written for it. In Kotlin cancelling is cooperative: a coroutine that
  blocks a thread, or loops without suspending, goes on.
- `within` gives `err(time.TimedOut)`, a value in the type, where
  `withTimeout` throws a `CancellationException` that code has to be
  careful not to swallow.
- What a `CoroutineContext` element or a `ThreadLocal` carries is a
  `context`: declared once, typed, read by name, inherited by tasks.

## Shared state

```kotlin
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

// The lock and what it guards are two values: nothing ties them together.
val hits = mutableMapOf<String, Int>()
val gate = Mutex()

suspend fun work(page: String) {
    repeat(100) {
        gate.withLock { hits[page] = (hits[page] ?: 0) + 1 }
    }
}

fun main() = runBlocking {
    coroutineScope {
        repeat(50) { n ->
            launch(Dispatchers.Default) { work(if (n % 2 == 0) "/home" else "/about") } // on every core
        }
    }
    println("${hits["/home"]} ${hits["/about"]}")
}
```

A Tondo mutex holds the value it guards:

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

- The map cannot be reached without the lock, and a task cannot be given a
  plain `var` to change: that does not compile. In Kotlin the top-level
  `hits` above is open to any coroutine on any thread.
- `std.sync` has `Atomic`, `RwLock`, `Semaphore` and shared collections.

## JSON

With `kotlinx.serialization`: a library, and a compiler plugin for the
annotation:

```kotlin
import kotlinx.serialization.SerializationException
import kotlinx.serialization.Serializable
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json

@Serializable
data class Person(
    val name: String,
    val age: Int,
    val languages: List<String> = emptyList(),
    val email: String? = null,
)

fun main() {
    val text = """{"name": "Ada", "age": 36, "languages": ["en", "fr"]}"""
    val person = Json.decodeFromString<Person>(text)
    println("${person.name}, ${person.age}, ${person.languages}, ${person.email}")
    println(Json { encodeDefaults = true }.encodeToString(person.copy(age = 37)))

    // The wrong type in a field is an error that says where.
    try {
        Json.decodeFromString<Person>("""{"name": "Alan", "age": "forty"}""")
    } catch (error: SerializationException) {
        println(error.message?.lines()?.first())
    }
}
```

```text
Ada, 36, [en, fr], null
{"name":"Ada","age":37,"languages":["en","fr"],"email":null}
Unexpected JSON token at offset 24: Unexpected symbol 'f' in numeric literal at path: $.age
```

In Tondo it is the standard library, and there is no annotation: a record
whose fields can be read is read.

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

- Defaults and optional fields mean the same in both. `@name("user_id")` is
  `@SerialName`.
- `@validate.Min(0)` on a field is checked by the decoder.
- The same record reads and writes YAML, TOML, CBOR and MessagePack, and
  is what `std.sql` fills from a row and `std.cli` from a command line.

## Tests

```kotlin
import kotlin.test.Test
import kotlin.test.assertEquals

/** The words of a title, each with a capital. */
fun titleCase(text: String): String =
    text.split(" ").filter { it.isNotEmpty() }.joinToString(" ") { word ->
        word.take(1).uppercase() + word.drop(1).lowercase()
    }

class TitlesTest {
    @Test
    fun capitalizesEachWord() {
        assertEquals("The Old Man And The Sea", titleCase("the old man and the sea"))
    }

    @Test
    fun keepsOneSpaceBetweenWords() {
        assertEquals("War And Peace", titleCase("  war   AND peace "))
        assertEquals("", titleCase(""))
    }
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

- `assert(a == b)` prints both sides when it fails, so there is no
  `assertEquals(expected, actual)`.
- `tondo test` runs them, with benchmarks, fuzz tests, snapshots and
  coverage in the same command and no build file to configure.
- A test that starts tasks is run in the orders that could change its
  result, and under `testing.withVirtualTime` a `sleep` of an hour takes
  no time, as `runTest` does for coroutines.

## An HTTP server and its client

With the JDK's own classes. A Kotlin service would use Ktor or Spring:

```kotlin
import com.sun.net.httpserver.HttpServer
import kotlinx.serialization.Serializable
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import java.net.InetSocketAddress
import java.net.URI
import java.net.http.HttpClient
import java.net.http.HttpRequest
import java.net.http.HttpResponse

@Serializable
data class Greeting(val message: String, val length: Int)

fun main() {
    // The JDK's server and client. Most Kotlin services use Ktor or Spring.
    val server = HttpServer.create(InetSocketAddress("127.0.0.1", 0), 0) // a free port
    server.createContext("/hello/") { exchange ->
        val name = exchange.requestURI.path.removePrefix("/hello/")
        val body = Json.encodeToString(Greeting("Hello, $name!", name.length)).toByteArray()
        exchange.responseHeaders.add("Content-Type", "application/json")
        exchange.sendResponseHeaders(200, body.size.toLong())
        exchange.responseBody.use { it.write(body) }
    }
    server.start()
    val url = "http://127.0.0.1:${server.address.port}"

    val client = HttpClient.newHttpClient()
    fun get(path: String) =
        client.send(HttpRequest.newBuilder(URI.create(url + path)).build(), HttpResponse.BodyHandlers.ofString())

    val greeting = Json.decodeFromString<Greeting>(get("/hello/Ada").body())
    println("${greeting.message} (${greeting.length})")
    println(get("/nothing").statusCode()) // a 404 is not an exception here: the code has to look
    server.stop(0)
}
```

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
- The client fails with an error for a status of 400 or more.
- WebSockets, gRPC from `.proto` files, SQL, templates and a command-line
  parser are in the standard library, with one way to use each.

## The toolchain, command by command

| Kotlin | Tondo | |
| --- | --- | --- |
| `kotlinc`, `gradle run` | `tondo run` | compiles in moments, keeps the build, runs |
| `build.gradle.kts` | `tondo.toml` | a few lines; no build to program |
| `gradle build`, a fat jar, GraalVM | `tondo build` | one native executable; `--static`, `--target aarch64-linux` |
| `gradle test` | `tondo test` | with `--bench`, `--fuzz`, `--coverage` |
| ktlint, detekt | `tondo fmt`, `tondo check`, `tondo fix` | one layout, no configuration |
| Maven Central | `tondo add` | dependencies by git or URL, locked in `tondo.lock` |
| the IDE's debugger | `tondo debug` | in a terminal or an editor, and backwards over a recording |
| the IDE | `tondo lsp` | any editor; the language server is in the same binary |
| JFR, a profiler | `tondo profile` | writes pprof |
| JNI, cinterop | `extern fn`, `tondo bind` | declarations written from a C header |
| KSP, compiler plugins | `derive` and generators | part of the build |

## Habits to leave behind

| In Kotlin | In Tondo |
| --- | --- |
| `val x = 1`, `var y = 2` | `let x = 1`, `var y = 2` |
| `fun f(a: Int): String` | `fn f(a: Int): String` |
| `fun f() = expr` | `fn f(): T { expr }` |
| `null`, `x ?: y`, `x?.y`, `x!!` | `none`, `x ?? y`, `x?.y`, `x.expect("...")` |
| `throw`, `try`, `catch` | `fail`, `T ! E`, `?`, `match` |
| `when (x) { is A -> ... }` | `match x { A(...) => ... }` |
| `data class`, `sealed interface` | `type`, `enum` |
| `class`, `open`, `override`, `object` | `type`, `trait`, `impl`; functions of a module |
| `Point(1, 2)` | `Point { x: 1, y: 2 }` |
| `point.copy(x = 3)` | `point with { x: 3 }` |
| `{ it * 2 }`, `{ a, b -> a + b }` | `(x) { x * 2 }`, `(a, b) { a + b }` |
| `"$name is ${user.age}"` | `"{name} is {user.age}"` |
| `listOf(1, 2)`, `mapOf("a" to 1)` | `[1, 2]`, `["a": 1]` |
| `List<T>`, `Map<K, V>` | `Array[T]`, `Map[K, V]` |
| `x.size`, `s.length` | `x.length()`, `s.length()` |
| `for (x in xs)`, `while (c)` | `for x in xs`, `for c` |
| `&&`, `\|\|`, `!` | `and`, `or`, `not` |
| `x as Int`, `x.toInt()` | `Int(x)`, which fails if it does not fit |
| `suspend fun`, `runBlocking` | nothing: every function can wait |
| `async { }`, `.await()`, `launch { }` | `spawn f()`, `.wait()` |
| `withTimeout(1000) { }` | `within time.seconds(1) { }` |
| `mutex.withLock { }` | `mutex.update((value) { ... })` |
| `private`, `internal`, `public` | nothing, or `pub` |
| `package` and `import` | a directory is a module; `import app.orders` |

`tondo guide pitfalls` has these and the error each gives.

## Where Kotlin is the better choice today

- **Android**, and Compose for user interfaces.
- **The JVM's ecosystem.** Every Java library is a Kotlin library, and
  Kotlin fits beside Java in a codebase that already exists.
- **Multiplatform.** One language for a phone, a browser and a server.
- **Its tools and its age.** An IDE that understands every line, and a
  language that is finished. Tondo is in early development and can still
  change.
- **Platforms.** The JVM runs everywhere. Tondo builds for Linux and macOS.

What Tondo offers against that is Kotlin's feel without the machine
underneath: no `null` coming in from elsewhere, failures in signatures,
values that are not shared, functions of one colour, and a native
executable that starts at once.

Next: [Tondo for someone who writes Swift](swift.md).
