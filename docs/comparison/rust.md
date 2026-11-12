# Tondo for someone who writes Rust

Tondo took more ideas from Rust than from any other language: options and
results instead of null and exceptions, enums with data, a `match` that
covers every case, traits with explicit `impl`s, and mutation that has to
be asked for. It left out what Rust pays for those with: lifetimes, a
borrow checker to satisfy, and two kinds of function. This page writes
twelve small programs in both. Every one was run, and prints what is shown.

- [The short version](#the-short-version)
- [Values, moves and copies](#values-moves-and-copies)
- [A value that may be missing](#a-value-that-may-be-missing)
- [Errors](#errors)
- [Enums and match](#enums-and-match)
- [Traits and generics](#traits-and-generics)
- [Collections](#collections)
- [Tasks](#tasks)
- [Deadlines, and async](#deadlines-and-async)
- [Shared state](#shared-state)
- [JSON](#json)
- [Tests](#tests)
- [An HTTP server and its client](#an-http-server-and-its-client)
- [What replaces the borrow checker](#what-replaces-the-borrow-checker)
- [The toolchain, command by command](#the-toolchain-command-by-command)
- [Habits to leave behind](#habits-to-leave-behind)
- [Where Rust is the better choice today](#where-rust-is-the-better-choice-today)

## The short version

| | Rust | Tondo |
| --- | --- | --- |
| A function fails | `Result<T, E>`, passed on with `?` | `T ! E`, passed on with `?` |
| A value that may be missing | `Option<T>` | `T?` |
| Assigning a `Vec` or a `String` | moves it; `.clone()` copies | copies it; the storage is shared until one changes |
| Letting a function into a value | `&T` and `&mut T`, with lifetimes when they are returned | `ref` and `mut`, for the length of the call |
| A reference kept in a struct | a lifetime parameter | not possible: a value, or a `Ref[T]` |
| Memory | ownership, checked when compiling | reference counts and a collector of cycles |
| A function that waits | `async fn`, and so are its callers | written like any other |
| Tasks | threads in `std`; async tasks with a runtime from a crate | tasks in the language, which their block waits for |
| Data races | rejected when compiling | rejected when compiling |
| Polymorphism | generics, or `dyn Trait` | generics; there are no trait objects |
| Macros | `macro_rules!` and procedural macros | none; `derive` and generators for the same jobs |
| An integer overflows | panics in debug builds, wraps in release | panics, in every build |
| JSON, HTTP, async, CLI | crates | the standard library |
| What ships | one native executable | one native executable |

## Values, moves and copies

In Rust, assigning a `Vec` moves it: the old name is gone, and a copy is
something to ask for. A small struct is copied only if it derives `Copy`:

```rust
#[derive(Clone, Copy)]
struct Point {
    x: i32,
    y: i32,
}

fn main() {
    let original = vec![1, 2, 3];
    let mut copy = original.clone(); // without `.clone()`, `original` is moved and gone
    copy[0] = 99;
    copy.push(4);
    println!("{:?} {:?}", original, copy);

    let home = Point { x: 1, y: 2 };
    let mut away = home; // a copy, because `Point` derives `Copy`
    away.x = 10;
    println!("{},{} {},{}", home.x, home.y, away.x, away.y);
}
```

```text
[1, 2, 3] [99, 2, 3, 4]
1,2 10,2
```

In Tondo every value is copied when it is assigned, a collection as much as
a number, and no name is ever left empty:

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

- The copy is not the cost a `.clone()` is. The two arrays share their
  storage until one of them is written to, and the write copies only if the
  storage is still shared. Passing a large array to a function costs a
  pointer and a count.
- There is no "use of moved value" for ordinary data, and so no `.clone()`
  sprinkled until the compiler is quiet.
- A few values do move, because copying them would make no sense: the
  sending end of a channel, a `Join`, an open file. They are the types
  marked `~Copy`, and using one after passing it on is an error, as in
  Rust.
- `let` and `var` are Rust's `let` and `let mut`.

## A value that may be missing

The two languages agree here. Rust:

```rust
struct User {
    name: String,
    email: Option<String>,
}

fn find<'a>(users: &'a [User], name: &str) -> Option<&'a User> {
    users.iter().find(|user| user.name == name)
}

fn main() {
    let users = vec![
        User { name: "ada".to_string(), email: Some("ada@example.com".to_string()) },
        User { name: "alan".to_string(), email: None },
    ];
    for name in ["ada", "alan", "grace"] {
        // A user that may not be there, with an email that may not be there.
        let email = find(&users, name).and_then(|user| user.email.as_deref()).unwrap_or("no email");
        println!("{name}: {email}");
    }
}
```

Tondo, with the same shape and less to write around it:

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

- `T?` is `Option<T>`, and `some` and `none` are lowercase. `map`,
  `andThen`, `unwrapOr`, `filter` and `?` are there under those names.
- `find` returns a `User?`, a user, where Rust returns an
  `Option<&'a User>`, a borrow tied to the slice by a lifetime. Since a
  Tondo value is cheap to copy, the function hands one over and there is no
  lifetime to write.
- No `as_deref`, `as_ref` or `to_string`: there is one string type, and no
  difference between owning a value and looking at it.
- There is no `if let` and no `let else`. `match` opens an option, and an
  arm can `return`, `break` or `continue`.

## Errors

Rust's errors are the model for Tondo's, and the functions line up almost
token for token:

```rust
use std::collections::HashMap;
use std::fmt;

enum ConfigError {
    Missing(String),
    NotANumber(String, String),
    OutOfRange(String, i64),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ConfigError::Missing(key) => write!(f, "{key} is missing"),
            ConfigError::NotANumber(key, text) => write!(f, "{key}: \"{text}\" is not a number"),
            ConfigError::OutOfRange(key, value) => write!(f, "{key}: {value} is out of range"),
        }
    }
}

fn port(settings: &HashMap<&str, &str>, key: &str) -> Result<i64, ConfigError> {
    let text = settings.get(key).ok_or_else(|| ConfigError::Missing(key.to_string()))?;
    let value: i64 = text
        .parse()
        .map_err(|_| ConfigError::NotANumber(key.to_string(), text.to_string()))?;
    if !(1..=65535).contains(&value) {
        return Err(ConfigError::OutOfRange(key.to_string(), value));
    }
    Ok(value)
}

/// The address to listen on: a failure of `port` passes through `?`.
fn address(settings: &HashMap<&str, &str>) -> Result<String, ConfigError> {
    Ok(format!("0.0.0.0:{}", port(settings, "port")?))
}

fn main() {
    let all = [
        HashMap::from([("port", "8080")]),
        HashMap::new(),
        HashMap::from([("port", "http")]),
        HashMap::from([("port", "70000")]),
    ];
    for settings in &all {
        match address(settings) {
            Ok(found) => println!("{found}"),
            Err(error) => println!("error: {error}"),
        }
    }
}
```

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

- `Int ! ConfigError` is `Result<i64, ConfigError>`, and `!ConfigError` is
  `Result<(), ConfigError>`.
- A function's last expression is its value, and it is not wrapped: `value`,
  not `Ok(value)`. `fail error` is `return Err(error)`.
- A function that can fail in two ways says `T ! (ReadError | ParseError)`.
  The union is a type the compiler makes, so `?` passes either on with no
  `From` to implement and no error enum to write for each function, and the
  caller can still `match` on each.
- `Failure` is what `anyhow::Error` or `Box<dyn Error>` is used for: any
  error fits, `.context("reading the file")` adds to it, and it keeps the
  chain of causes, which `failure.find[E]()` searches.
- A result that is not used does not compile, as `#[must_use]` warns in
  Rust.

## Enums and match

Nothing to translate but the punctuation:

```rust
use std::f64::consts::PI;

enum Shape {
    Circle(f64),
    Rectangle(f64, f64),
    Triangle(f64, f64, f64),
}

fn area(shape: &Shape) -> f64 {
    match shape {
        Shape::Circle(radius) => PI * radius * radius,
        Shape::Rectangle(width, height) => width * height,
        Shape::Triangle(a, b, c) => {
            let s = (a + b + c) / 2.0;
            (s * (s - a) * (s - b) * (s - c)).sqrt()
        }
    }
}

fn main() {
    for shape in [Shape::Circle(1.0), Shape::Rectangle(2.0, 3.5), Shape::Triangle(3.0, 4.0, 5.0)] {
        println!("{:.2}", area(&shape));
    }
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

- A variant is `Shape.Circle`, with a dot. There is no `&` on the parameter
  and no `ref` in the patterns: `match` reads the value and its parts.
- Patterns have literals, ranges, tuples, records, arrays with a rest, and
  guards. There is no `|` between patterns: an arm for each, or a guard.
- A record is `type Point = { x: Float, y: Float }`, with defaults for its
  fields and `point with { x: 0.0 }` for a changed copy. A newtype is
  `type Meters = Float`.
- `math.sqrt` returns a result here, since the root of a negative number is
  not a number.

## Traits and generics

The same model: a trait, an `impl` for each type, and generic functions
that the compiler writes once for each type they are used with.

```rust
trait Describe {
    fn describe(&self) -> String;
}

struct Book {
    title: String,
    pages: u32,
}

struct Song {
    title: String,
    seconds: u32,
}

impl Describe for Book {
    fn describe(&self) -> String {
        format!("{}, {} pages", self.title, self.pages)
    }
}

impl Describe for Song {
    fn describe(&self) -> String {
        format!("{}, {}:{:02}", self.title, self.seconds / 60, self.seconds % 60)
    }
}

/// Any type that can describe itself; resolved when compiling.
fn announce<T: Describe>(item: &T) -> String {
    format!("Now: {}", item.describe())
}

/// The largest of some values of any type that has an order.
fn largest<T: PartialOrd + Copy>(items: &[T]) -> Option<T> {
    let mut best = *items.first()?;
    for &item in items {
        if item > best {
            best = item;
        }
    }
    Some(best)
}

fn main() {
    println!("{}", announce(&Book { title: "Dune".to_string(), pages: 612 }));
    println!("{}", announce(&Song { title: "Hey Jude".to_string(), seconds: 431 }));
    println!("{:?} {:?} {:?}", largest(&[3, 9, 4]), largest(&["pear", "apple"]), largest::<i32>(&[]));
}
```

```text
Now: Dune, 612 pages
Now: Hey Jude, 7:11
Some(9) Some("pear") None
```

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

- Type parameters go in square brackets, `fn largest[T: Ordered]`, and so
  do type arguments: `Array[Int]`, `largest[Int]([])`. There is no
  turbofish.
- `self` is lent for reading and `mut self` for changing. There is no
  `&self` beside `self`, because there is no difference between having a
  value and borrowing it for a call.
- Comparing with `==`, being a map's key, copying, and reading from JSON
  and writing to it come with any type whose parts have them. There is
  nothing to derive. `Display` and `Ordered` are written as an `impl`.
- There is no `dyn Trait`. Several types in one collection are an enum, or
  a union `Book | Song`; behaviour chosen as the program runs is a function
  value. Every call is resolved when compiling.
- A function can return `impl Describe`, as in Rust.

## Collections

Rust's `HashMap` has no order, so counting words and keeping the first of
those that tie means keeping the order beside it:

```rust
use std::collections::HashMap;

fn main() {
    let text = "to be or not to be that is the question to be";
    let mut counts: HashMap<&str, usize> = HashMap::new();
    let mut seen: Vec<&str> = Vec::new(); // a HashMap has no order: keep the order they came in
    for word in text.split(' ') {
        let count = counts.entry(word).or_insert(0);
        if *count == 0 {
            seen.push(word);
        }
        *count += 1;
    }
    // The three most frequent; a stable sort keeps those that tie in order.
    seen.sort_by_key(|word| std::cmp::Reverse(counts[word]));
    let top: Vec<String> = seen.iter().take(3).map(|word| format!("{word} {}", counts[word])).collect();
    println!("{}", top.join(", "));

    let lengths: Vec<usize> = text.split(' ').map(|word| word.len()).collect();
    let long = lengths.iter().filter(|&&n| n > 2).count();
    println!("{long} long words, {} letters", lengths.iter().sum::<usize>());
}
```

A Tondo map keeps the order its keys were added in:

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

- `Array[T]`, `Map[K, V]`, `Set[T]` and `Deque[T]` are the collections, and
  the adapters are methods on them: `words.map(...)`, with no `.iter()`
  before and usually no `.collect()` after.
- `counts[word] = ...` adds or replaces. There is no entry API, because
  there is no borrow to keep alive across the lookup and the write.
- A closure is `(word) { word.length() }`. It captures by value, so there
  is no `move` and no closure that cannot be returned.
- Indexing with a number out of range panics, and `get` gives an option, in
  both languages. A Tondo index can be negative, from the end, and a slice
  is `values[1:3]`.

## Tasks

Rust's standard library has threads, and since 1.63 a scope that waits for
the ones it started:

```rust
use std::thread;
use std::time::{Duration, Instant};

/// Stands for a request that takes a while.
fn fetch(name: &str, delay: u64) -> String {
    thread::sleep(Duration::from_millis(delay));
    format!("{name} in {delay} ms")
}

fn main() {
    let started = Instant::now();
    let jobs = [("users", 300), ("orders", 500), ("prices", 100)];
    // A scope does not end before the threads it started: they may borrow from it.
    let results: Vec<String> = thread::scope(|scope| {
        let handles: Vec<_> = jobs.iter().map(|&(name, delay)| scope.spawn(move || fetch(name, delay))).collect();
        handles.into_iter().map(|handle| handle.join().unwrap()).collect()
    });
    for result in results {
        println!("{result}");
    }
    // One after another they would take 900 ms.
    println!("all three in under 800 ms: {}", started.elapsed() < Duration::from_millis(800));
}
```

That scope is how every Tondo task works. A task belongs to the block that
started it, which waits for it, and tasks are not threads: one that waits
costs a little memory and no thread, and a hundred thousand of them share
the cores.

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

- `spawn f()` gives a `Join[T]`, and `wait()` its result. An error or a
  panic in the task comes out there, with no `unwrap` on a join handle.
- A value given to a task is copied to it, or moved if it is `~Copy`. There
  is no `'static` bound to satisfy and no `Arc` to wrap things in first.
- A task that should outlive its block is `spawn detached f()`.
- Channels are `channel[T](capacity)`, with `select` over receives, sends,
  joins and timeouts.

## Deadlines, and async

For work that waits on the network, Rust code is written `async`, with a
runtime from a crate. A function that waits is of another kind than one
that does not, and so is everything that calls it:

```rust
use std::time::Duration;
use tokio::time::{sleep, timeout};

// A function that waits is `async`, and so is every function that calls it.
async fn fetch(delay: u64) -> String {
    sleep(Duration::from_millis(delay)).await;
    format!("answered in {delay} ms")
}

#[tokio::main]
async fn main() {
    for delay in [10, 5000] {
        // Dropping the future is what cancels it.
        match timeout(Duration::from_millis(250), fetch(delay)).await {
            Ok(answer) => println!("{answer}"),
            Err(_) => println!("gave up after 250 ms"),
        }
    }
}
```

Tondo has one kind of function. Any of them can wait, the scheduler runs
something else meanwhile, and a deadline goes around a block:

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

- There is no `async`, no `.await`, no `Future`, no `Pin` and no runtime to
  choose. A library is written once, and there is no blocking call to keep
  out of an executor.
- `within` cancels the way dropping a future does, at the next wait. The
  block's `defer`s run, and the tasks it started are cancelled with it.
- `defer` is how cleanup is written, where Rust uses `Drop`. A file or a
  socket also closes when its owner ends.

## Shared state

Rust shares with `Arc` and changes with `Mutex`:

```rust
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;

fn work(hits: &Mutex<HashMap<&str, u32>>, page: &'static str) {
    for _ in 0..100 {
        *hits.lock().unwrap().entry(page).or_insert(0) += 1;
    }
}

fn main() {
    // `Arc` to share it among threads, `Mutex` to change it from them.
    let hits = Arc::new(Mutex::new(HashMap::new()));
    let handles: Vec<_> = (0..50)
        .map(|n| {
            let hits = Arc::clone(&hits);
            thread::spawn(move || work(&hits, if n % 2 == 0 { "/home" } else { "/about" }))
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    let counts = hits.lock().unwrap();
    println!("{} {}", counts["/home"], counts["/about"]);
}
```

A Tondo `sync.Mutex[T]` is both: a handle that its copies share, holding
the value. The way in is `update`, which has the lock while its function
runs:

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

- There is no guard to hold too long or to carry across a wait, no
  `lock().unwrap()`, and no poisoning: a panic inside `update` releases the
  lock and goes on up.
- What can be given to a task is checked when compiling, as `Send` and
  `Sync` are. A plain `var` cannot be changed from two tasks.
- `updateWhen(ready, change)` waits for a condition and then changes the
  value, which is what a `Condvar` beside a `Mutex` is for.
- `tondo test` runs a test that starts tasks in the orders that could
  change its result, as `loom` does for code written for it.

## JSON

In Rust this is `serde`, two crates and a derive:

```rust
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Person {
    name: String,
    age: u32,
    #[serde(default)]
    languages: Vec<String>,
    email: Option<String>,
}

fn main() {
    let text = r#"{"name": "Ada", "age": 36, "languages": ["en", "fr"]}"#;
    match serde_json::from_str::<Person>(text) {
        Ok(mut person) => {
            println!("{}, {}, {:?}, {:?}", person.name, person.age, person.languages, person.email);
            person.age = 37;
            println!("{}", serde_json::to_string(&person).unwrap());
        }
        Err(error) => println!("{error}"),
    }
    // The wrong type in a field is an error that says where.
    match serde_json::from_str::<Person>(r#"{"name": "Alan", "age": "forty"}"#) {
        Ok(person) => println!("{}", person.name),
        Err(error) => println!("{error}"),
    }
}
```

```text
Ada, 36, ["en", "fr"], None
{"name":"Ada","age":37,"languages":["en","fr"],"email":null}
invalid type: string "forty", expected u32 at line 1 column 31
```

In Tondo it is the standard library, and there is nothing to derive: a
record whose fields can be read is read.

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

- A field with a default is optional on the wire, and a `T?` field too.
  `@name("user_id")` renames one, as `#[serde(rename)]` does.
- The same record reads and writes YAML, TOML, CBOR and MessagePack, and
  `@validate.Min(0)` on a field is checked by the decoder.
- Reflection is in the language, for code that needs it:
  `reflect.typeInfo[T]()` gives a type's fields and annotations, which is
  how `std.json`, `std.cli` and `std.sql` read types without a macro.

## Tests

```rust
/// The words of a title, each with a capital.
fn title_case(text: &str) -> String {
    let words: Vec<String> = text
        .split_whitespace()
        .map(|word| {
            let mut letters = word.chars();
            match letters.next() {
                Some(first) => first.to_uppercase().collect::<String>() + &letters.as_str().to_lowercase(),
                None => String::new(),
            }
        })
        .collect();
    words.join(" ")
}

fn main() {
    println!("{}", title_case("the old man and the sea"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capitalizes_each_word() {
        assert_eq!(title_case("the old man and the sea"), "The Old Man And The Sea");
    }

    #[test]
    fn keeps_one_space_between_words() {
        assert_eq!(title_case("  war   AND peace "), "War And Peace");
        assert_eq!(title_case(""), "");
    }
}
```

In Tondo a test is a `test` block, in a file beside the code:

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

- `assert(a == b)` prints both sides when it fails, so there is one
  `assert` and no `assert_eq!`.
- `tondo test` also runs benchmarks, fuzz tests with a corpus, snapshots
  and coverage, for which Rust has `criterion`, `cargo-fuzz`, `insta` and
  `cargo-llvm-cov`.
- A string index in Tondo counts characters: `word[0:1]` is the first
  letter, whatever bytes it takes.

## An HTTP server and its client

In Rust: `axum` for the server, `reqwest` for the client, `tokio` under
both and `serde` for the JSON. With what they depend on, 167 crates are
compiled for this program:

```rust
use axum::{extract::Path, routing::get, Json, Router};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Greeting {
    message: String,
    length: usize,
}

async fn hello(Path(name): Path<String>) -> Json<Greeting> {
    Json(Greeting { message: format!("Hello, {name}!"), length: name.len() })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let app = Router::new().route("/hello/{name}", get(hello));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?; // a free port
    let address = listener.local_addr()?;
    tokio::spawn(async move { axum::serve(listener, app).await });

    let greeting: Greeting = reqwest::get(format!("http://{address}/hello/Ada")).await?.json().await?;
    println!("{} ({})", greeting.message, greeting.length);
    let missing = reqwest::get(format!("http://{address}/nothing")).await?;
    println!("{}", missing.status().as_u16()); // a 404 is not an error here: the code has to look
    Ok(())
}
```

In Tondo, two modules of the standard library:

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

- A handler returns its reply, or fails with an `http.Problem`. A typed
  handler, `app.get("/users/:id", (id: Int) { find(id) })`, is the
  counterpart of axum's extractors.
- The client fails with an error for a status of 400 or more.
- WebSockets, HTTP/2, TLS, gRPC from `.proto` files, SQL, templates and a
  command-line parser are in the standard library too, with one version and
  one way to use each.

## What replaces the borrow checker

Rust proves that memory is used correctly from who owns each value and how
long each borrow lasts. Tondo gets to the same safety from another side,
and it helps to see the whole of it at once.

**Values instead of owners.** Every value is its own, and assigning copies.
Two names cannot be the same array, so there is no aliasing to control.
Sharing storage until a write makes this cost what passing a reference
costs.

**Loans that last one call.** A function is let into a caller's value with
`ref`, to read it without the count, or `mut`, to change it:

```tondo
import std.console

fn total(values: ref Array[Float]): Float {
    values.sum()
}

fn scale(values: mut Array[Float], factor: Float) {
    for mut value in values {
        value *= factor
    }
}

fn main() {
    var prices = [1.0, 2.5, 4.0]
    scale(mut prices, 2.0)                      // the call says it changes them
    console.println("{prices} {total(ref prices)}")
}
```

```text output
[2.0, 5.0, 8.0] 15.0
```

A loan cannot be stored, returned or given to a task, so it never outlives
the call. That one rule is why there are no lifetimes. The checks that
remain are Rust's: a value cannot be lent to be changed twice at once, and
two parts of an array can be, when they do not overlap.

**Identity when it is asked for.** Something that several parts of a
program must see as the same thing, a node of a graph or a cache, is a
`Ref[T]`, and `WeakRef[T]` is the one that does not keep it alive. They are
`Rc` and `Weak`, used far less often, since ordinary data needs neither.

**Counted memory.** Storage is freed when the last holder lets go, at once,
and a collector finds the cycles that counting cannot. There is no tracing
collector over the whole heap, and no finalizers.

**What this costs.** A count to keep where Rust has none, though the
compiler leaves out the counts it can prove away and keeps values that do
not outlive their function off the heap. A program cannot place its own
allocations or run without the runtime. Where that matters, Rust is the
tool.

**What it buys.** No lifetime in any signature, no fight over a
self-referential struct, no `Rc<RefCell<T>>`, no difference between an
owned string and a borrowed one, and no change that ripples through every
caller because a borrow got longer.

## The toolchain, command by command

| Rust | Tondo | |
| --- | --- | --- |
| `cargo run` | `tondo run` | compiles, keeps the build, runs; also a single file |
| `cargo build --release` | `tondo build` | one executable; `--static`, `--target aarch64-linux` |
| `cargo check`, `cargo clippy` | `tondo check` | with `tondo fix` for what it proposes |
| `cargo test` | `tondo test` | with `--bench`, `--fuzz`, `--coverage` |
| `cargo fmt` | `tondo fmt` | with no options |
| `cargo doc`, docs.rs | `tondo api <module>` | with examples that compile |
| `cargo new`, `cargo add` | `tondo new`, `tondo add` | `tondo.toml` and `tondo.lock` |
| `rust-analyzer` | `tondo lsp` | in the same binary |
| `gdb`, `lldb` | `tondo debug` | steps backwards over a recording |
| `perf`, `cargo flamegraph` | `tondo profile` | writes pprof |
| `bindgen` | `tondo bind` | `extern` declarations from a C header |
| `cargo expand` | `tondo expand` | what a `derive` or a generator wrote |
| `cargo asm` | `tondo asm` | the machine code of a function, under its lines |

Tondo compiles with Cranelift, the code generator that Rust offers as a
faster backend for debug builds. Builds are quick, and the code is
optimized less than LLVM would.

## Habits to leave behind

| In Rust | In Tondo |
| --- | --- |
| `let mut x = 0` | `var x = 0` |
| `fn f(x: i32) -> i32` | `fn f(x: Int): Int` |
| `Vec<T>`, `HashMap<K, V>`, `&[T]` | `Array[T]`, `Map[K, V]`, `Array[T]` |
| `String`, `&str`, `.to_string()` | `String` |
| `Some(x)`, `None`, `Ok(x)`, `Err(e)` | `some(x)`, `none`, `ok(x)`, `err(e)` |
| `Ok(value)` at the end | `value` |
| `return Err(e)` | `fail e` |
| `if let Some(x) = y` | `match y` |
| `&x`, `&mut x` at a call | `ref x`, `mut x`, or nothing |
| `x.clone()` | `x` |
| `\|x\| x * 2` | `(x) { x * 2 }` |
| `Shape::Circle` | `Shape.Circle` |
| `impl Point { fn area(&self) }` | `fn Point.area(self)` |
| `x as i64` | `Int(x)`, which fails if it does not fit |
| `while`, `loop` | `for cond { }`, `for { }` |
| `&&`, `\|\|`, `!` | `and`, `or`, `not` |
| `format!("{} is {}", name, age)` | `"{name} is {age}"` |
| `println!` | `console.println` |
| `async fn`, `.await` | nothing: every function can wait |
| `Arc<Mutex<T>>` | `sync.Mutex[T]` |
| `#[derive(Clone, PartialEq, Eq, Hash)]` | nothing: they come with the type |
| `let x = x.trim();` to shadow | another name: there is no shadowing |
| `;` at the end of a line | nothing |

`tondo guide pitfalls` has these and the error each gives.

## Where Rust is the better choice today

- **Without a runtime.** A kernel, a driver, firmware, a library that must
  add nothing to its host. A Tondo program carries a scheduler and managed
  memory.
- **When every allocation counts.** Rust lets a program decide where each
  byte lives. Tondo decides, and tells what it decided.
- **Its ecosystem and its age.** crates.io has a library for almost
  anything, and Rust has been stable for over ten years. Tondo is in early
  development and can still change.
- **The fastest code.** LLVM optimizes more than Cranelift. Tondo's goal is
  Go's speed, not Rust's.
- **Platforms.** Rust runs nearly everywhere, Windows and WebAssembly
  included. Tondo builds for Linux and macOS, on x86-64 and arm64.

What Tondo offers against that is most of what makes Rust code right, the
types, the errors, the checked concurrency, for the programs where the
borrow checker was a cost and not the point: services, tools and command
lines.

Next: [Tondo for someone who writes C#](csharp.md).
