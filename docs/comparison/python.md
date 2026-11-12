# Tondo for someone who writes Python

Python is where many programs start, because nothing stands between an idea
and running it. Tondo keeps as much of that as a compiled language can: a
script is one file with no ceremony, strings interpolate, lists slice, maps
keep their order, and the standard library has the batteries. What changes
is when mistakes are found. A Tondo program is checked whole before its
first line runs, and compiled to native code. This page writes twelve small
programs in both. Every one was run, and prints what is shown.

- [The short version](#the-short-version)
- [What will feel familiar](#what-will-feel-familiar)
- [Values and references](#values-and-references)
- [A value that may be missing](#a-value-that-may-be-missing)
- [Errors](#errors)
- [One of several things](#one-of-several-things)
- [Protocols, traits and generics](#protocols-traits-and-generics)
- [Collections](#collections)
- [Tasks](#tasks)
- [Deadlines](#deadlines)
- [Shared state, and every core](#shared-state-and-every-core)
- [JSON](#json)
- [Tests](#tests)
- [An HTTP server and its client](#an-http-server-and-its-client)
- [The toolchain, command by command](#the-toolchain-command-by-command)
- [Habits to leave behind](#habits-to-leave-behind)
- [Where Python is the better choice today](#where-python-is-the-better-choice-today)

## The short version

| | Python | Tondo |
| --- | --- | --- |
| Types | checked as the program runs; hints are for other tools | checked before it runs, and inferred inside a function |
| A wrong name or a wrong type | fails when that line is reached | does not compile |
| A function fails | raises an exception, which no signature shows | returns `T ! E`, passed on with `?` |
| A value that may be missing | `None`, in any variable | `T?`, which has to be opened; there is no `None` |
| Assigning a list or a dict | shares it | copies it; the storage is shared until one changes |
| Data | classes, and dataclasses to write less of them | records, enums and tuples: all values |
| Blocks | indentation | braces, laid out by `tondo fmt` |
| A function that waits | `async def`, and so are its callers | written like any other |
| Threads | one runs Python code at a time, in the usual build | tasks run on every core |
| An integer | grows without limit | 64 bits, and a panic on overflow; `BigInt` when asked for |
| Running | an interpreter, with the packages installed beside it | one native executable |
| Speed | an interpreter's | compiled code's |

## What will feel familiar

Before the differences, the things a Python programmer does not have to
give up:

```tondo
import std.console

/// The words of a text that are long enough, with their lengths.
fn long(text: String, least: Int = 4): Map[String, Int] {
    var found: Map[String, Int] = [:]
    for word in text.split(" ") {
        if word.length() >= least and not (word in found) {
            found[word] = word.length()
        }
    }
    found
}

fn main() {
    let numbers = [3, 1, 4, 1, 5, 9, 2, 6]
    console.println("{numbers[-1]} {numbers[1:4]} {numbers[::2]} {5 in numbers}")

    let (smallest, largest) = (numbers.min() ?? 0, numbers.max() ?? 0)
    console.println("from {smallest} to {largest}, {numbers.sum() / numbers.length()} on average")

    let words = long("the quick brown fox jumps over the lazy dog", least: 5)
    for (word, size) in words {
        console.println("{word:>6} {"#".repeat(size)}")
    }
    console.println("{words.keys().collect().sorted()} {"quick" in words}")
}
```

```text output
6 [1, 4, 1] [3, 4, 5, 2] true
from 1 to 9, 3 on average
 quick #####
 brown #####
 jumps #####
[brown, jumps, quick] true
```

- Negative indexes, slices with a step, `in`, tuples that unpack, arguments
  by name and with defaults.
- Interpolation in any string, with a format after the colon much as in
  an f-string: `{word:>6}`, `{price:.2}`.
- A map keeps the order its keys were added in, as a `dict` does.
- A script needs no `main`: a `.to` file with statements at the top runs
  from the top, as `tondo script.to`, or as a program of its own with
  `#!/usr/bin/env tondo` in its first line.

What is not there: `min` of an empty list is `none`, not an exception, so
it has to be opened; and `/` between two integers gives an integer.

## Values and references

A Python name is a label on an object. Two names can be the same list, and
a function can change the list its caller passed:

```python
from dataclasses import dataclass


@dataclass
class Point:
    x: int
    y: int


original = [1, 2, 3]
alias = original  # the same list under another name
alias[0] = 99
alias.append(4)
print(original, alias)

home = Point(1, 2)
away = home  # the same object, too
away.x = 10
print(home.x, away.x)
```

```text
[99, 2, 3, 4] [99, 2, 3, 4]
10 10
```

In Tondo a list is a value, as a number is in Python. Assigning it gives
the other name its own:

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

- No more `copy()`, `deepcopy()` or `list(items)` to be safe, and no
  mutable default argument shared between calls.
- The copy costs nothing until one of the two changes: they share their
  storage, and the first write to a shared list makes it its own.
- A function that changes its argument says so, `fn add(items: mut
  Array[Int])`, and so does the call, `add(mut numbers)`.
- `let` is a name that cannot be assigned again and `var` one that can.
  A name's type does not change, and a name cannot be declared twice in a
  function: there is no shadowing.

## A value that may be missing

`None` can be in any Python variable. Type hints say where it is expected,
and a type checker, if one is run, warns:

```python
from dataclasses import dataclass


@dataclass
class User:
    name: str
    email: str | None = None


def find(users: list[User], name: str) -> User | None:
    return next((user for user in users if user.name == name), None)


users = [User("ada", "ada@example.com"), User("alan")]
for name in ["ada", "alan", "grace"]:
    user = find(users, name)
    # The hints say that None is possible. Nothing stops `user.email` from
    # running without the check, and failing for "grace".
    email = user.email if user and user.email else "no email"
    print(f"{name}: {email}")
```

In Tondo `None` does not exist. A `User` is always a user, and a value
that may be missing is a `User?`, a different type that has to be opened:

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
        let email = find(users, name).andThen((user) { user.email }) ?? "no email"
        console.println("{name}: {email}")
    }
}
```

```text output
ada: ada@example.com
alan: no email
grace: no email
```

- `'NoneType' object has no attribute 'email'` is not an error a Tondo
  program can have. Using an option without opening it does not compile.
- `unwrapOr`, `map` and `andThen` open an option in an expression, and
  `match` when both cases need code.
- There is no truthiness. `if items` is `if not items.isEmpty()`, and
  `if user` is a `match`, so an empty string and a missing one are never
  confused.
- `ages.get("cy")` gives an option, as `dict.get` gives `None`;
  `ages["cy"]` on a key that is not there gives one too, where Python
  raises `KeyError`.

## Errors

A Python function that fails raises. What it can raise is in its
documentation, if anywhere:

```python
class ConfigError(Exception):
    pass


def port(settings: dict[str, str], key: str) -> int:
    if key not in settings:
        raise ConfigError(f"{key} is missing")
    text = settings[key]
    try:
        value = int(text)
    except ValueError:
        raise ConfigError(f'{key}: "{text}" is not a number') from None
    if not 1 <= value <= 65535:
        raise ConfigError(f"{key}: {value} is out of range")
    return value


# Nothing in this signature says that it can fail, or with what.
def address(settings: dict[str, str]) -> str:
    return f"0.0.0.0:{port(settings, 'port')}"


for settings in [{"port": "8080"}, {}, {"port": "http"}, {"port": "70000"}]:
    try:
        print(address(settings))
    except ConfigError as error:
        print(f"error: {error}")
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

- `Int ! ConfigError` is the whole contract. `?` after a call passes its
  failure on to the caller, which is what an uncaught exception does, but
  written and checked.
- There is no `try` and `except`. `match` on a result handles each case,
  and the compiler knows them all.
- `Int.parse(text)` returns a result. Nothing in the standard library
  raises for input that is merely wrong.
- A bug is a panic, as an index out of range: it stops the task with a
  message and a stack, and is not caught in ordinary code.
- `defer` is `finally`, and a file closes when the block that opened it
  ends, as `with` does.

## One of several things

Dataclasses and `match` came to Python for this. Nothing checks that the
cases are all there:

```python
import math
from dataclasses import dataclass


@dataclass
class Circle:
    radius: float


@dataclass
class Rectangle:
    width: float
    height: float


@dataclass
class Triangle:
    a: float
    b: float
    c: float


Shape = Circle | Rectangle | Triangle


def area(shape: Shape) -> float:
    match shape:
        case Circle(radius):
            return math.pi * radius * radius
        case Rectangle(width, height):
            return width * height
        case Triangle(a, b, c):
            s = (a + b + c) / 2
            return math.sqrt(s * (s - a) * (s - b) * (s - c))
    # A shape with no case falls out of the match, and the function
    # returns None where a float was promised.


for shape in [Circle(1), Rectangle(2, 3.5), Triangle(3, 4, 5)]:
    print(f"{area(shape):.2f}")
```

A Tondo `enum` lists its variants, each with its data, and `match` has to
cover them all:

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
            math.sqrt(s * (s - a) * (s - b) * (s - c)) ?? 0.0
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

- Forgetting a case is a compile error that names it. In Python the
  function returns `None`, and the mistake shows wherever that `None` is
  first used.
- A record is `type Point = { x: Float, y: Float }`: what a frozen
  dataclass is, with `==`, defaults, and `point with { x: 0.0 }` for a
  changed copy.
- There are no classes, no inheritance and no `self` to forget. A method
  is a function named with its type, `fn Point.length(self): Float`.

## Protocols, traits and generics

Python is duck-typed: whatever has the method will do. A `Protocol` writes
the expectation down for a type checker:

```python
from dataclasses import dataclass
from typing import Protocol


class Describe(Protocol):
    def describe(self) -> str: ...


@dataclass
class Book:
    title: str
    pages: int

    def describe(self) -> str:
        return f"{self.title}, {self.pages} pages"


@dataclass
class Song:
    title: str
    seconds: int

    def describe(self) -> str:
        return f"{self.title}, {self.seconds // 60}:{self.seconds % 60:02}"


# Any object with a `describe` will do; whether it has one is found at the call.
def announce(item: Describe) -> str:
    return f"Now: {item.describe()}"


def largest[T](items: list[T]) -> T | None:
    return max(items, default=None)


print(announce(Book("Dune", 612)))
print(announce(Song("Hey Jude", 431)))
print(largest([3, 9, 4]), largest(["pear", "apple"]), largest([]))
```

```text
Now: Dune, 612 pages
Now: Hey Jude, 7:11
9 pear None
```

A Tondo trait is the same idea, checked by the compiler: a type has it
because an `impl` says so, and a generic function says what it needs of
its type:

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

- `largest([3, "pear"])` is a `TypeError` when Python gets to it. In Tondo
  an array has one type of element, and one of several is said out loud:
  `Array[Int | String]`.
- A generic function is compiled for each type it is used with. There is
  no lookup of a method by name as the program runs.
- `largest` returns a `T?`: the empty case is in the type, where Python's
  `max` raises unless given a `default`.

## Collections

`Counter` is hard to beat for this:

```python
from collections import Counter

text = "to be or not to be that is the question to be"
words = text.split()

# The three most frequent; those that tie stay in the order they came.
top = Counter(words).most_common(3)
print(", ".join(f"{word} {count}" for word, count in top))

lengths = [len(word) for word in words]
print(f"{sum(1 for n in lengths if n > 2)} long words, {sum(lengths)} letters")
```

Tondo has no `Counter`, and a map does the counting. The rest is methods
where Python has comprehensions:

```tondo
import std.console

fn main() {
    let text = "to be or not to be that is the question to be"
    var counts: Map[String, Int] = [:]
    for word in text.split(" ") {
        counts[word] = (counts.get(word) ?? 0) + 1
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

- `[len(w) for w in words if len(w) > 2]` is
  `words.filter((w) { w.length() > 2 }).map((w) { w.length() })`. A closure
  is `(w) { ... }`, with as many lines as it needs: there is no `lambda`
  limited to one expression.
- `Array[T]`, `Map[K, V]`, `Set[T]` and `Deque[T]` are `list`, `dict`,
  `set` and `deque`. Each holds one type of element.
- `len(x)` is `x.length()`, and a string's length is its characters.
- Arithmetic on arrays goes element by element, as NumPy's does:
  `prices * 1.21`.

## Tasks

`asyncio` runs many waits on one thread. A function that waits is `async`,
and so is everything that calls it:

```python
import asyncio
import time


# Stands for a request that takes a while. It waits, so it is `async`,
# and so is everything that calls it.
async def fetch(name: str, delay: int) -> str:
    await asyncio.sleep(delay / 1000)
    return f"{name} in {delay} ms"


async def main() -> None:
    started = time.monotonic()
    async with asyncio.TaskGroup() as group:  # does not end before its tasks
        jobs = [("users", 300), ("orders", 500), ("prices", 100)]
        tasks = [group.create_task(fetch(name, delay)) for name, delay in jobs]
    for task in tasks:
        print(task.result())
    # One after another they would take 900 ms.
    print("all three in under 800 ms:", time.monotonic() - started < 0.8)


asyncio.run(main())
```

Tondo has one kind of function. Any of them can wait, and the runtime runs
something else meanwhile. Doing several things at once is `spawn`:

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

- `fetch` is an ordinary function. There is no `async def`, no `await`, no
  event loop to start, and no library that comes in two versions, one for
  each kind of caller.
- A task belongs to the block that started it, which waits for it, as a
  `TaskGroup` does. That is how every `spawn` works, not a context manager
  to remember.
- A blocking call does not freeze the others: the scheduler has every core,
  and when a task computes for long it starts another worker for the rest.

## Deadlines

```python
import asyncio


async def fetch(delay: int) -> str:
    await asyncio.sleep(delay / 1000)
    return f"answered in {delay} ms"


async def main() -> None:
    for delay in [10, 5000]:
        try:
            async with asyncio.timeout(0.25):
                print(await fetch(delay))
        except TimeoutError:
            print("gave up after 250 ms")


asyncio.run(main())
```

`within` is `asyncio.timeout`, for any code:

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

- The deadline holds for everything the block calls and every task it
  starts, whether or not they were written with it in mind.
- When the time passes, `within` gives `err(time.TimedOut)`: a value to
  match on, in the type of the expression.

## Shared state, and every core

Python threads share everything, and a lock is a thing to remember. In the
usual build of Python only one thread runs Python code at a time, so
threads help with waiting and not with computing:

```python
import threading

# The lock and what it guards are two names: nothing ties them together.
hits: dict[str, int] = {}
gate = threading.Lock()


def work(page: str) -> None:
    for _ in range(100):
        with gate:
            hits[page] = hits.get(page, 0) + 1


threads = [
    threading.Thread(target=work, args=("/home" if n % 2 == 0 else "/about",))
    for n in range(50)
]
for thread in threads:
    thread.start()
for thread in threads:
    thread.join()
print(hits["/home"], hits["/about"])
```

Tondo's tasks run on every core. What they share has to be made for
sharing, and a mutex holds the value it guards:

```tondo
import std.console
import std.sync

fn work(hits: sync.Mutex[Map[String, Int]], page: String) {
    for _ in 0..100 {
        hits.update((counts) { counts[page] = (counts.get(page) ?? 0) + 1 })
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
    console.println("{counts.get("/home") ?? 0} {counts.get("/about") ?? 0}")
}
```

```text output
2500 2500
```

- A task cannot be given a plain variable to change: that does not compile.
  The lock is not something to remember, since the map is inside it.
- Work that needs the processor uses all of them, with no
  `multiprocessing`, no pickling of arguments and no second interpreter.
- `tondo test` runs a test that starts tasks in the orders that could
  change its result.

## JSON

`json.loads` gives dicts and lists. Turning them into a dataclass is the
program's job, and nothing checks the types:

```python
import json
from dataclasses import asdict, dataclass, field, replace


@dataclass
class Person:
    name: str
    age: int
    languages: list[str] = field(default_factory=list)
    email: str | None = None


text = '{"name": "Ada", "age": 36, "languages": ["en", "fr"]}'
person = Person(**json.loads(text))
print(f"{person.name}, {person.age}, {person.languages}, {person.email}")
print(json.dumps(asdict(replace(person, age=37))))

# The hint says `int`. Nothing checks it: the text goes in as the age.
other = Person(**json.loads('{"name": "Alan", "age": "forty"}'))
print(other.name, repr(other.age))
```

```text
Ada, 36, ['en', 'fr'], None
{"name": "Ada", "age": 37, "languages": ["en", "fr"], "email": null}
Alan 'forty'
```

The age of Alan is the text `'forty'`, in a field declared `int`, and the
program goes on. In Tondo the type is the schema:

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

- `json.decode[Person](text)` is what `pydantic` is installed for: the
  fields, their types, what is optional, and an error that says where.
- `@validate.Min(0)` or `@validate.Length(max: 80)` on a field is checked
  by the decoder.
- JSON of no known shape is a `json.Value`, read with
  `value.find("items[0].name")`.

## Tests

```python
import unittest


def title_case(text: str) -> str:
    """The words of a title, each with a capital."""
    return " ".join(word[:1].upper() + word[1:].lower() for word in text.split())


class TitleCaseTest(unittest.TestCase):
    def test_capitalizes_each_word(self):
        self.assertEqual(title_case("the old man and the sea"), "The Old Man And The Sea")

    def test_keeps_one_space_between_words(self):
        self.assertEqual(title_case("  war   AND peace "), "War And Peace")
        self.assertEqual(title_case(""), "")


if __name__ == "__main__":
    unittest.main()
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

- `assert(a == b)` prints both sides when it fails, as `pytest` rewrites
  `assert` to do.
- `tondo test` runs them, with benchmarks, fuzz tests, snapshots and
  coverage in the same command and nothing to install.
- Tests have less to do: a test that checks what happens when a function
  is given a string for a number has nothing to check.

## An HTTP server and its client

With the standard library alone, Python's server is a class with a method
for each verb. Most programs install Flask or FastAPI, and `requests`:

```python
import json
import threading
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, HTTPServer


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if not self.path.startswith("/hello/"):
            self.send_error(404)
            return
        name = self.path.removeprefix("/hello/")
        body = json.dumps({"message": f"Hello, {name}!", "length": len(name)}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass  # the default writes a line for each request


server = HTTPServer(("127.0.0.1", 0), Handler)  # a free port
threading.Thread(target=server.serve_forever, daemon=True).start()
url = f"http://127.0.0.1:{server.server_port}"

with urllib.request.urlopen(f"{url}/hello/Ada") as response:
    greeting = json.load(response)  # a dict: its keys are checked when they are read
print(f"{greeting['message']} ({greeting['length']})")
try:
    urllib.request.urlopen(f"{url}/nothing")
except urllib.error.HTTPError as error:
    print(error.code)
server.shutdown()
```

Tondo's standard library has the server and the client that those packages
are:

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

- A route's parameters and its reply are typed. `greeting.message` is a
  field the compiler knows, where `greeting['message']` is a key looked up
  as the program runs.
- A typed handler, `app.get("/users/:id", (id: Int) { find(id) })`, takes
  its parameters from the path and the query and writes what it returns as
  JSON, as FastAPI does from type hints.
- The server answers each request in a task, on every core, with no worker
  processes to configure.

## The toolchain, command by command

| Python | Tondo | |
| --- | --- | --- |
| `python script.py` | `tondo script.to` | compiles, keeps the build, runs |
| `python -m venv`, `pip install`, `uv` | `tondo add` | dependencies in `tondo.toml`, locked in `tondo.lock` |
| `mypy`, `pyright` | `tondo check` | not optional: it is the compiler |
| `ruff`, `black` | `tondo fmt`, `tondo fix` | one layout, with no options |
| `pytest`, `coverage`, `hypothesis` | `tondo test` | `--coverage`, `--fuzz`, `--bench` |
| `pdb` | `tondo debug` | also backwards, over a recording |
| `cProfile`, `py-spy` | `tondo profile` | of a run, or of a program that is running |
| `pyinstaller`, a Docker image with Python | `tondo build` | one file that needs nothing installed |
| `help(str)`, docs.python.org | `tondo api String`, `tondo guide` | in the terminal, with examples that compile |
| `ctypes`, `cffi` | `extern fn`, `tondo bind` | declarations written from a C header |

There is no REPL. `tondo run --watch` runs a program again as its files
are saved, and a running program can be changed without stopping it.

## Habits to leave behind

| In Python | In Tondo |
| --- | --- |
| `x = 1`, later `x = "one"` | `var x = 1`; its type stays |
| `def f(a, b=2):` | `fn f(a: Int, b: Int = 2): Int { }` |
| `return x` at the end | `x`: the last expression is the value |
| `None`, `if x is None` | `none`, `match`, `x.isNone()` |
| `try`, `except`, `raise` | `T ! E`, `?`, `match`, `fail` |
| `class Point:` with `__init__` | `type Point = { x: Float, y: Float }` |
| `self.x = x` | `Point { x, y }` |
| `if items:`, `if not name:` | `if not items.isEmpty()`, `if name.isEmpty()` |
| `f"{name} is {age}"` | `"{name} is {age}"` |
| `"a" + "b"`, `"-" * 10` | `"{a}{b}"`, `"-".repeat(10)` |
| `len(x)`, `str(x)`, `int(s)` | `x.length()`, `"{x}"`, `Int.parse(s)` |
| `[f(x) for x in xs if c(x)]` | `xs.filter(c).map(f)` |
| `lambda x: x * 2` | `(x) { x * 2 }` |
| `for i in range(n):`, `while c:` | `for i in 0..n { }`, `for c { }` |
| `for i, x in enumerate(xs):` | `for (i, x) in xs.enumerate() { }` |
| `elif`, `and`, `or`, `not` | `else if`, `and`, `or`, `not` |
| `True`, `False` | `true`, `false` |
| `import os.path as p` | `import std.path as p` |
| `async def`, `await` | nothing: every function can wait |
| `with open(path) as f:` | `fs.readText(path)?`; a file closes when its block ends |
| `if __name__ == "__main__":` | `fn main()`, or a script with none |
| indentation | braces |

`tondo guide pitfalls` has these and the error each gives.

## Where Python is the better choice today

- **Data, science and machine learning.** NumPy, pandas, PyTorch and the
  notebooks around them are Python's, and nothing here replaces them.
- **Exploring.** A REPL and a notebook, where each line answers at once.
- **Its ecosystem and its age.** PyPI has a package for everything, and
  everyone can read Python. Tondo is in early development and can still
  change.
- **Glue of a few lines.** A script that will never grow gains little from
  being checked.
- **Platforms.** Python runs everywhere. Tondo builds for Linux and macOS.

What Tondo offers against that is for the script that did grow: mistakes
found before it runs instead of in production, data that says what it is,
failures that cannot be ignored, every core without a second process, and
one file to ship.

Next: [Tondo for someone who writes TypeScript](typescript.md).
