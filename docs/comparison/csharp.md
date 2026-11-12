# Tondo for someone who writes C#

C# and Tondo want the same things from a language: static types that
stay out of the way, a large standard library, and tools that come in the
box. They get there differently. C# is objects that are shared, exceptions,
`null` held back by annotations, and `async` through every layer. Tondo is
values that are copied, failures in the signature, no `null`, and functions
that wait without saying so. This page writes twelve small programs in
both. Every one was run, and prints what is shown.

- [The short version](#the-short-version)
- [Values and references](#values-and-references)
- [A value that may be missing](#a-value-that-may-be-missing)
- [Errors](#errors)
- [One of several things](#one-of-several-things)
- [Interfaces, traits and generics](#interfaces-traits-and-generics)
- [Collections and LINQ](#collections-and-linq)
- [Tasks](#tasks)
- [Deadlines and cancelling](#deadlines-and-cancelling)
- [Shared state](#shared-state)
- [JSON](#json)
- [Tests](#tests)
- [An HTTP server and its client](#an-http-server-and-its-client)
- [The toolchain, command by command](#the-toolchain-command-by-command)
- [Habits to leave behind](#habits-to-leave-behind)
- [Where C# is the better choice today](#where-c-is-the-better-choice-today)

## The short version

| | C# | Tondo |
| --- | --- | --- |
| A function fails | throws an exception, which no signature shows | returns `T ! E`, passed on with `?` |
| A value that may be missing | `null`, with warnings from nullable annotations | `T?`, which has to be opened; there is no `null` |
| Assigning a list or an object | shares it | copies it; the storage is shared until one changes |
| Data | classes, structs, records | records, enums and tuples: all values |
| A type with several shapes | a class hierarchy and a `switch` with a discard arm | an `enum` with data and a `match` that covers every case |
| Reuse | inheritance and interfaces | traits and composition; there is no inheritance |
| Generics | kept at run time, constrained with `where` | resolved when compiling, bounded by traits |
| A function that waits | `async`, returns a `Task`, and so do its callers | written like any other |
| Cancelling | a `CancellationToken` parameter in every function | `within`, around the block; no parameter |
| A lock | an object beside what it guards | a `sync.Mutex[T]` around what it guards |
| A data race | possible | does not compile |
| Cleanup | `using` and `IDisposable`, finalizers | `defer`, and resources that close when their owner ends |
| Memory | a tracing garbage collector | reference counts and a collector of cycles |
| An integer overflows | wraps, unless `checked` | panics, in every build |
| What ships | an assembly for the runtime, or Native AOT with its limits | one native executable |

## Values and references

In C# what an assignment does depends on how the type was declared. A
`List<T>`, like any class, is shared: two names, one list. A struct is
copied:

```csharp
var original = new List<int> { 1, 2, 3 };
var alias = original; // the same list under another name
alias[0] = 99;
alias.Add(4);
Console.WriteLine($"[{string.Join(", ", original)}] [{string.Join(", ", alias)}]");

var home = new Point(1, 2);
var away = home; // a struct is copied; a class would be shared, as the list is
away.X = 10;
Console.WriteLine($"{home.X} {away.X}");

record struct Point(int X, int Y);
```

```text
[99, 2, 3, 4] [99, 2, 3, 4]
1 10
```

Tondo has no reference types. An array is a value as much as a number is,
and assigning it gives the other name its own:

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

- The copy costs nothing until one of the two changes: they share their
  storage, and the first write to a shared array makes it its own. Passing
  a large collection to a function costs a pointer and a count.
- A function cannot change a collection it was given unless its parameter
  says `mut` and the call says `mut` too. There are no defensive copies,
  no `IReadOnlyList<T>` to promise what nothing enforces, and no
  `ImmutableArray<T>` to reach for.
- `let` is a binding that cannot change and `var` one that can, so `var`
  means something else than in C#. Types are inferred for both.
- Something that several parts of a program must see as the same thing is
  a `Ref[T]`, and it is the exception, written where it is meant.

## A value that may be missing

C# has `null` in every reference type. Nullable annotations make the
compiler warn about it, and `?.` and `??` make it short to step past:

```csharp
var users = new List<User> { new("ada", "ada@example.com"), new("alan", null) };
foreach (var name in new[] { "ada", "alan", "grace" })
{
    // `?.` and `??` step past what is not there. Without them this compiles,
    // with a warning, and throws a NullReferenceException for "grace".
    var email = Find(users, name)?.Email ?? "no email";
    Console.WriteLine($"{name}: {email}");
}

static User? Find(List<User> users, string name) => users.FirstOrDefault(user => user.Name == name);

record User(string Name, string? Email);
```

In Tondo there is no `null` at all. A `User` is always a user. A value
that may be missing is a `User?`, a different type that has to be opened
before it is used:

```tondo
import std.console

type User = { name: String, email: String? }

fn find(users: Array[User], name: String): User? {
    users.find((user) { user.name == name })
}

fn main() {
    let users = [
        User { name: "ada", email: some("ada@example.com") },
        User { name: "alan", email: none },
    ]
    for name in ["ada", "alan", "grace"] {
        // A user that may not be there, with an email that may not be there.
        let email = find(users, name).andThen((user) { user.email }).unwrapOr("no email")
        console.println("{name}: {email}")
    }
}
```

```text output
ada: ada@example.com
alan: no email
grace: no email
```

- A forgotten check is a compile error, where C# gives a warning that can
  be silenced with `!`, and code compiled without the annotations gives
  nothing.
- `andThen`, `map` and `unwrapOr` are `?.` and `??` as methods. `match`
  opens an option when both cases need code, and `?` passes a `none` on
  from a function that itself returns an option.
- A map lookup gives an option, `ages.get("cy")`, where a `Dictionary`
  throws from its indexer or needs `TryGetValue` with an `out`.
- A field with no default has to be given. There is no `default(T)`, and
  no object half built by a constructor that forgot a field.

## Errors

A C# method that fails throws. What it throws is in its documentation,
when it is anywhere:

```csharp
var all = new List<Dictionary<string, string>>
{
    new() { ["port"] = "8080" }, new(), new() { ["port"] = "http" }, new() { ["port"] = "70000" },
};
foreach (var settings in all)
{
    try
    {
        Console.WriteLine(Address(settings));
    }
    catch (ConfigException error)
    {
        Console.WriteLine($"error: {error.Message}");
    }
}

static int Port(Dictionary<string, string> settings, string key)
{
    if (!settings.TryGetValue(key, out var text))
        throw new ConfigException($"{key} is missing");
    if (!int.TryParse(text, out var value))
        throw new ConfigException($"{key}: \"{text}\" is not a number");
    if (value < 1 || value > 65535)
        throw new ConfigException($"{key}: {value} is out of range");
    return value;
}

// Nothing in this signature says that it can fail, or with what.
static string Address(Dictionary<string, string> settings) => $"0.0.0.0:{Port(settings, "port")}";

class ConfigException(string message) : Exception(message);
```

A Tondo function that fails says so in its type, and the caller has to do
something with the failure:

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
            ConfigError.Missing(key) => "{key} is missing"
            ConfigError.NotANumber(key, text) => "{key}: \"{text}\" is not a number"
            ConfigError.OutOfRange(key, value) => "{key}: {value} is out of range"
        }
    }
}

fn port(settings: Map[String, String], key: String): Int ! ConfigError {
    let text = match settings.get(key) {
        some(found) => found
        none => fail ConfigError.Missing(key)
    }
    let value = Int.parse(text).mapErr((_) { ConfigError.NotANumber(key, text) })?
    if value < 1 or value > 65535 {
        fail ConfigError.OutOfRange(key, value)
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

- `Int ! ConfigError` is the whole contract: what comes back, and what can
  go wrong. `address` fails with the same error because `?` passed it on,
  and its signature says so too.
- There is no `try` and `catch`. `match` on a result handles each case,
  and the compiler knows the cases: adding a variant to `ConfigError`
  makes every `match` that forgot it an error.
- A result that is not used does not compile. An exception that nobody
  expected travels up until something catches `Exception`, or the process
  ends.
- A panic is the other kind of failure: a bug, as an index out of range.
  It is not part of a signature, and `recover { ... }` at the edge of a
  request turns one into a value, as a top-level `catch` does.
- `Failure` is for code that only reports: any error fits in it, and
  `.context("reading the settings")` adds what an inner exception adds.

## One of several things

C#'s way to say "one of these" is a base type and a class for each case.
Pattern matching reads them well, but the hierarchy is open, so the
`switch` needs an arm for a case that cannot happen:

```csharp
using System.Globalization;

foreach (var shape in new Shape[] { new Circle(1), new Rectangle(2, 3.5), new Triangle(3, 4, 5) })
{
    Console.WriteLine(Area(shape).ToString("F2", CultureInfo.InvariantCulture));
}

static double Area(Shape shape) => shape switch
{
    Circle c => Math.PI * c.Radius * c.Radius,
    Rectangle r => r.Width * r.Height,
    Triangle t => Heron(t.A, t.B, t.C),
    // Anyone can add a Shape: the compiler cannot know these are all of them.
    _ => throw new ArgumentException("a shape nobody handled"),
};

static double Heron(double a, double b, double c)
{
    var s = (a + b + c) / 2;
    return Math.Sqrt(s * (s - a) * (s - b) * (s - c));
}

abstract record Shape;
record Circle(double Radius) : Shape;
record Rectangle(double Width, double Height) : Shape;
record Triangle(double A, double B, double C) : Shape;
```

A Tondo enum is closed, its variants carry data, and `match` has to cover
them all:

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
        Shape.Circle(radius) => math.Pi * radius * radius
        Shape.Rectangle(width, height) => width * height
        Shape.Triangle(a, b, c) => {
            let s = (a + b + c) / 2.0
            math.sqrt(s * (s - a) * (s - b) * (s - c)).unwrapOr(0.0)
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

- Adding a fourth shape breaks every `match` that does not handle it, with
  an error that names the missing case. The C# `switch` keeps compiling,
  and the discard arm throws when the program runs.
- A record is `type Point = { x: Float, y: Float }`: equality by value,
  `point with { x: 0.0 }` for a changed copy, defaults for its fields. It
  is what a C# `record` is, and it is the only kind of data there is.
- There are no classes and no inheritance. Behaviour is functions and
  traits, and what would be a base class with virtual methods is an enum,
  or a trait with several `impl`s.
- `math.sqrt` returns a result, since the root of a negative number is not
  a number: the failure is in the type, where `Math.Sqrt` gives a `NaN` to
  find later.

## Interfaces, traits and generics

An interface, two types that implement it, and a generic method with a
constraint:

```csharp
Console.WriteLine(Announce(new Book("Dune", 612)));
Console.WriteLine(Announce(new Song("Hey Jude", 431)));
Console.WriteLine($"{Largest([3, 9, 4])} {Largest(["pear", "apple"])} {Largest(new int[0], -1)}");

// The call goes through the interface, found as the program runs.
static string Announce(IDescribe item) => $"Now: {item.Describe()}";

// "The largest, or nothing" has no one type for both ints and strings,
// so the caller gives what to return when there is none.
static T? Largest<T>(IReadOnlyList<T> items, T? none = default) where T : IComparable<T>
{
    if (items.Count == 0) return none;
    var best = items[0];
    foreach (var item in items)
    {
        if (item.CompareTo(best) > 0) best = item;
    }
    return best;
}

interface IDescribe
{
    string Describe();
}

record Book(string Title, int Pages) : IDescribe
{
    public string Describe() => $"{Title}, {Pages} pages";
}

record Song(string Title, int Seconds) : IDescribe
{
    public string Describe() => $"{Title}, {Seconds / 60}:{Seconds % 60:00}";
}
```

```text
Now: Dune, 612 pages
Now: Hey Jude, 7:11
9 pear -1
```

In Tondo an interface is a trait, implemented with `impl` outside the type.
A generic function is compiled once for each type it is used with:

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

- `largest` returns a `T?` for any `T`. C# has no one way to say "a `T` or
  nothing" for both value and reference types, which is why the C# version
  takes what to return for an empty list.
- `announce[T: Describe]` is resolved when compiling: there is no interface
  dispatch and no boxing of a struct. There are no trait objects either;
  several types in one collection are an enum or a union, `Book | Song`.
- A trait of your own can be implemented for a type of another module, or
  of the standard library, which is the job of an extension method. The
  type then has the trait, where an extension method cannot give a type an
  interface.
- Methods are functions named with their type, `fn Book.summary(self)`.
  There are no properties: a field is a field, and a computed value is a
  method.
- There is no overloading. A function has one name and one signature, with
  default and named arguments where C# would overload.

## Collections and LINQ

LINQ makes this short in C#:

```csharp
var text = "to be or not to be that is the question to be";
var words = text.Split(' ');

// Groups come in the order their keys first appear, and the sort is stable.
var top = words.GroupBy(word => word)
    .OrderByDescending(group => group.Count())
    .Take(3)
    .Select(group => $"{group.Key} {group.Count()}");
Console.WriteLine(string.Join(", ", top));

var lengths = words.Select(word => word.Length).ToList();
Console.WriteLine($"{lengths.Count(n => n > 2)} long words, {lengths.Sum()} letters");
```

Tondo's collections have the same operations as methods, and a map that
keeps the order its keys were added in:

```tondo
import std.console

fn main() {
    let text = "to be or not to be that is the question to be"
    var counts: Map[String, Int] = [:]
    for word in text.split(" ") {
        counts[word] = counts.get(word).unwrapOr(0) + 1
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

- `Array[T]`, `Map[K, V]`, `Set[T]` and `Deque[T]` are the collections.
  There is no `List` beside `Array`, no `IEnumerable` beside both, and no
  query that runs again each time it is enumerated: `map` on an array gives
  an array.
- A closure is `(word) { word.length() }`. The last expression is its
  value.
- A string is text measured in characters, `"añil".length()` is 4, and it
  is UTF-8 underneath. Interpolation is `"{name} is {age}"` in any string,
  with formats as `{price:.2}`.
- A `for` goes over a range, a collection or an iterator: `for i in 0..n`,
  `for word in words`, `for (key, value) in map`.

## Tasks

`async` and `await` are how C# waits without holding a thread:

```csharp
using System.Diagnostics;

var clock = Stopwatch.StartNew();
// Each call starts at once and gives a Task; WhenAll waits for the three.
var results = await Task.WhenAll(Fetch("users", 300), Fetch("orders", 500), Fetch("prices", 100));
foreach (var result in results)
{
    Console.WriteLine(result);
}
// One after another they would take 900 ms.
Console.WriteLine($"all three in under 800 ms: {clock.ElapsedMilliseconds < 800}");

// Stands for a request that takes a while. It waits, so it is `async`,
// returns a Task, and so does everything that calls it.
static async Task<string> Fetch(string name, int delay)
{
    await Task.Delay(delay);
    return $"{name} in {delay} ms";
}
```

Tondo has one kind of function. Any of them can wait, and the runtime runs
something else on that thread meanwhile. Doing several things at once is
`spawn`:

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

- `fetch` is an ordinary function that returns a `String`. There is no
  `async`, no `await`, no `Task<T>` in a signature and no
  `ConfigureAwait`. A library is written once, not once for each kind of
  caller, and there is no `.Result` that deadlocks.
- A task belongs to the block that started it, which waits for it before
  it ends. The C# task that nobody awaited, with its exception lost, is not
  something a Tondo program can write by accident: `spawn detached f()` is
  how to ask for it.
- `spawn f()` gives a `Join[T]`, and `wait()` its result or the task's
  error. A `Group` waits for many.
- Tasks are cheap, as C#'s are: a server has one for each connection.
  Channels, `select` and the tools of `std.sync` are there for when they
  have to talk.

## Deadlines and cancelling

Cancelling in C# is a `CancellationToken`, a parameter of every method
between where it is made and where it matters:

```csharp
foreach (var delay in new[] { 10, 5000 })
{
    using var deadline = new CancellationTokenSource(TimeSpan.FromMilliseconds(250));
    try
    {
        Console.WriteLine(await Fetch(delay, deadline.Token));
    }
    catch (OperationCanceledException)
    {
        Console.WriteLine("gave up after 250 ms");
    }
}

// The token is a parameter of every function that may have to stop,
// and each passes it on to what it calls.
static async Task<string> Fetch(int delay, CancellationToken token)
{
    await Task.Delay(delay, token);
    return $"answered in {delay} ms";
}
```

In Tondo the deadline goes around the block. Everything the block calls,
and every task it starts, is under it:

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

- No function takes a token, and none can forget to pass it on. Every wait
  is cancellable, `time.sleep` as much as a read from a socket.
- `within` gives its block's value, or `err(time.TimedOut)`: a value to
  match on, not an `OperationCanceledException` to catch.
- When the time passes, the block's `defer`s run and its tasks are
  cancelled with it.
- What an `AsyncLocal<T>` or a scoped service carries, a request's id or a
  logger, is a `context` declared once, with typed fields that any function
  below reads by name.

## Shared state

A C# lock is an object beside the data. Which data it guards is known to
whoever wrote it:

```csharp
// The lock and what it guards are two variables: nothing ties them together.
var hits = new Dictionary<string, int>();
var gate = new Lock();

await Task.WhenAll(Enumerable.Range(0, 50).Select(n => Task.Run(() =>
{
    var page = n % 2 == 0 ? "/home" : "/about";
    for (var i = 0; i < 100; i++)
    {
        lock (gate)
        {
            hits[page] = hits.GetValueOrDefault(page) + 1;
        }
    }
})));
Console.WriteLine($"{hits["/home"]} {hits["/about"]}");
```

A Tondo mutex holds the value, and `update` is the only way in:

```tondo
import std.console
import std.sync

fn work(hits: sync.Mutex[Map[String, Int]], page: String) {
    for _ in 0..100 {
        hits.update((counts) { counts[page] = counts.get(page).unwrapOr(0) + 1 })
    }
}

fn main() {
    let empty: Map[String, Int] = [:]
    let hits = sync.Mutex(empty)
    var group = Group[Unit]()
    for n in 0..50 {
        group.add(spawn work(hits, if n % 2 == 0 { "/home" } else { "/about" }))
    }
    _ = group.wait()
    let counts = hits.get()
    console.println("{counts.get("/home").unwrapOr(0)} {counts.get("/about").unwrapOr(0)}")
}
```

```text output
2500 2500
```

- The map cannot be reached without the lock, since the lock is where it
  lives.
- A task cannot be given a plain variable to change. What tasks share has
  to be made for sharing: a `sync.Mutex`, an `Atomic`, a channel, a shared
  collection. The race that C# allows does not compile.
- There is no `await` inside a `lock` to worry about, because there is no
  `await`.

## JSON

`System.Text.Json` reads a record by reflection, with options for the
names:

```csharp
using System.Text.Json;

var web = new JsonSerializerOptions(JsonSerializerDefaults.Web);
var text = """{"name": "Ada", "age": 36, "languages": ["en", "fr"]}""";
var person = JsonSerializer.Deserialize<Person>(text, web)!;
Console.WriteLine($"{person.Name}, {person.Age}, [{string.Join(", ", person.Languages ?? [])}], {person.Email ?? "null"}");
Console.WriteLine(JsonSerializer.Serialize(person with { Age = 37 }, web));

try
{
    JsonSerializer.Deserialize<Person>("""{"name": "Alan", "age": "forty"}""", web);
}
catch (JsonException error)
{
    Console.WriteLine(error.Message);
}

record Person(string Name, int Age, List<string>? Languages, string? Email);
```

```text
Ada, 36, [en, fr], null
{"name":"Ada","age":37,"languages":["en","fr"],"email":null}
The JSON value could not be converted to Person. Path: $.age | LineNumber: 0 | BytePositionInLine: 31.
```

In Tondo any record is read from JSON and written to it by its field names,
with code the compiler writes. Reading is strict, and says where:

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

- The C# program above, built as Native AOT, stops with `Reflection-based
  serialization has been disabled for this application`: an ahead-of-time
  build needs a source generator and a `JsonSerializerContext` listing each
  type. A Tondo program is always compiled ahead of time, and there is
  nothing to add.
- A field that is not optional and has no default must be in the JSON. C#
  leaves a missing one `null` or zero unless it is marked `required`.
- `@name("user_id")` on a field names it on the wire, and
  `@validate.Min(0)` is checked by the decoder, as data annotations are by
  a framework above the serializer.
- The same record reads and writes YAML, TOML, CBOR and MessagePack.

## Tests

With xUnit, a project of its own and a package:

```csharp
using Xunit;

public class TitlesTests
{
    [Fact]
    public void CapitalizesEachWord()
    {
        Assert.Equal("The Old Man And The Sea", Titles.TitleCase("the old man and the sea"));
    }

    [Theory]
    [InlineData("  war   AND peace ", "War And Peace")]
    [InlineData("", "")]
    public void KeepsOneSpaceBetweenWords(string text, string expected)
    {
        Assert.Equal(expected, Titles.TitleCase(text));
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

- There is no test project, no framework to choose and no attribute.
  `tondo test` runs them, and tests in `name_test.to` see the module's
  private names.
- `assert(a == b)` prints both sides when it fails, so there is one
  `assert` and no `Assert.Equal(expected, actual)` to get the wrong way
  round.
- The same command runs benchmarks, fuzz tests, snapshots and coverage,
  and it runs a test that starts tasks in the orders that could change its
  result.

## An HTTP server and its client

ASP.NET Core's minimal API and `HttpClient`:

```csharp
#:sdk Microsoft.NET.Sdk.Web

using System.Net.Http.Json;

var builder = WebApplication.CreateBuilder(args);
builder.WebHost.UseUrls("http://127.0.0.1:0"); // a free port
builder.Logging.ClearProviders();
var app = builder.Build();
app.MapGet("/hello/{name}", (string name) => new Greeting($"Hello, {name}!", name.Length));
await app.StartAsync();

var address = app.Urls.First();
using var client = new HttpClient();
var greeting = await client.GetFromJsonAsync<Greeting>($"{address}/hello/Ada");
Console.WriteLine($"{greeting!.Message} ({greeting.Length})");
var missing = await client.GetAsync($"{address}/nothing");
Console.WriteLine((int)missing.StatusCode); // a 404 is not an exception here: the code has to look
await app.StopAsync();

record Greeting(string Message, int Length);
```

The same in Tondo, with two modules of the standard library:

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
        err(http.HttpError.Status(response)) => console.println("{response.status()}")
        _ => console.println("unexpected")
    }
}
```

```text output
Hello, Ada! (3)
404
```

- A handler returns its reply or fails with an `http.Problem`, which is the
  RFC 9457 answer that `ProblemDetails` is. A typed handler,
  `app.get("/users/:id", (id: Int) { find(id) })`, takes its parameters
  from the path and the query, as a minimal API's delegate does.
- There is no container of services to configure. What a handler needs is
  a value it captures or a `context` it reads.
- The client fails with an error for a status of 400 or more, so there is
  no `EnsureSuccessStatusCode` to remember.
- WebSockets, server-sent events, gRPC from `.proto` files, OpenAPI, SQL
  and templates are in the standard library.

## The toolchain, command by command

| C# | Tondo | |
| --- | --- | --- |
| `dotnet run` | `tondo run` | compiles, keeps the build, runs; a project or one file |
| `dotnet build`, `dotnet publish` | `tondo build` | one native executable; `--static`, `--target aarch64-linux` |
| `dotnet test` | `tondo test` | with `--bench`, `--fuzz`, `--coverage` |
| `dotnet format` | `tondo fmt` | with no options and no `.editorconfig` |
| analyzers | `tondo check` | with `tondo fix` for what it proposes |
| `dotnet new`, `dotnet add package` | `tondo new`, `tondo add` | `tondo.toml` and `tondo.lock` |
| the debugger of the IDE | `tondo debug` | in a terminal or an editor, and backwards over a recording |
| `dotnet-trace`, `dotnet-counters`, `dotnet-dump` | `tondo profile`, `tondo counters`, `tondo dump` | on a program started with `TONDO_DIAGNOSTICS=1`; nothing to install |
| BenchmarkDotNet | `tondo test --bench` | with baselines and a limit on regressions |
| P/Invoke | `extern fn`, `tondo bind` | declarations written from a C header |
| source generators | `derive` and generators | part of the build, with what they wrote shown by `tondo expand` |
| the language server of the IDE | `tondo lsp` | in the same binary, for any editor |

## Habits to leave behind

| In C# | In Tondo |
| --- | --- |
| `var x = 1;` then `x = 2;` | `var x = 1`; a `let` cannot change |
| `string? name = null` | `let name: String? = none` |
| `user?.Email ?? "none"` | `user.andThen((u) { u.email }).unwrapOr("none")` |
| `throw new InvalidOperationException(...)` | `fail error`, in a function that returns `T ! E` |
| `try { } catch (E e) { }` | `match` on the result |
| `try { } finally { }`, `using` | `defer` |
| `class`, `: Base`, `virtual`, `override` | `type`, `enum`, `trait`, `impl` |
| `public`, `internal`, `private` | `pub`, or nothing |
| `new Point(1, 2)` | `Point { x: 1, y: 2 }` |
| a property with `get` and `set` | a field, or a method |
| `async Task<T>`, `await` | nothing: every function can wait |
| `Task.WhenAll(a, b)` | `spawn`, and a `Group` |
| `CancellationToken token` | `within` |
| `lock (gate) { }` | `mutex.update((value) { ... })` |
| `List<T>`, `Dictionary<K, V>`, `HashSet<T>` | `Array[T]`, `Map[K, V]`, `Set[T]` |
| `x => x * 2` | `(x) { x * 2 }` |
| `items.Where(...).Select(...)` | `items.filter(...).map(...)` |
| `$"{name} is {age}"` | `"{name} is {age}"` |
| `a + b` for strings | `"{a}{b}"`, or `a.concat(b)` |
| `foreach (var x in xs)`, `while (c)` | `for x in xs`, `for c` |
| `switch` | `match`, which covers every case |
| `&&`, `\|\|`, `!` | `and`, `or`, `not` |
| `(int)x`, `x as Foo` | `Int(x)`, which fails if it does not fit; `match` |
| `[Attribute]` | `@annotation`, which is only data |
| `namespace` and `using` | a directory is a module; `import app.orders` |
| `;` and `{ }` on their own lines | no semicolons; one layout, written by `tondo fmt` |

`tondo guide pitfalls` has these and the error each gives.

## Where C# is the better choice today

- **Desktop, mobile and games.** WPF, MAUI, Avalonia and Unity are C#.
  Tondo is for servers, tools and command lines.
- **Windows.** .NET is at home there. Tondo builds for Linux and macOS.
- **Its ecosystem and its age.** NuGet has a library for anything a
  business needs, and the language has more than twenty years of tooling,
  people and answers behind it. Tondo is in early development and can still change.
- **A program that changes as it runs.** Loading plugins, emitting code and
  reflecting over anything need a runtime with a JIT. A Tondo program is
  compiled whole.
- **Object models that are really hierarchies.** A UI toolkit is one.

What Tondo offers against that is in the programs above: failures that a
signature shows, values in place of `null`, data that is not shared by
accident, cases the compiler counts, concurrency without two kinds of
function, and a native executable that starts at once with no runtime to
install.

Next: [Tondo for someone who writes Python](python.md).
