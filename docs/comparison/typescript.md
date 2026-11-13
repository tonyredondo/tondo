# Tondo for someone who writes TypeScript

TypeScript showed how much a type system can find in a program before it
runs, and how pleasant types can be when they are inferred and can say "one
of these". Tondo has that kind of types, with one difference that changes
everything else: they are real. They are there when the program runs, they
describe every value in it, and there is no `any` to step outside them.
This page writes twelve small programs in both. Every one was run, and
prints what is shown.

- [The short version](#the-short-version)
- [Values and references](#values-and-references)
- [A value that may be missing](#a-value-that-may-be-missing)
- [Errors](#errors)
- [One of several things](#one-of-several-things)
- [Interfaces, traits and generics](#interfaces-traits-and-generics)
- [Collections](#collections)
- [Tasks](#tasks)
- [Deadlines and cancelling](#deadlines-and-cancelling)
- [Shared state, and every core](#shared-state-and-every-core)
- [JSON, and the edge of the program](#json-and-the-edge-of-the-program)
- [Tests](#tests)
- [An HTTP server and its client](#an-http-server-and-its-client)
- [The toolchain, command by command](#the-toolchain-command-by-command)
- [Habits to leave behind](#habits-to-leave-behind)
- [Where TypeScript is the better choice today](#where-typescript-is-the-better-choice-today)

## The short version

| | TypeScript | Tondo |
| --- | --- | --- |
| Types | checked, then erased: the program that runs is JavaScript | checked, and compiled into the program |
| A way around them | `any`, `as`, `!`, `// @ts-ignore` | none |
| A value that may be missing | `undefined` and `null`, caught when `strict` is on | `T?`, which has to be opened |
| A function fails | throws anything; what is caught is `unknown` | returns `T ! E`, passed on with `?` |
| Assigning an array or an object | shares it; `const` fixes only the name | copies it; a `let` cannot change at all |
| One of several shapes | a union with a tag, narrowed by `switch` | an `enum` with data and `match` |
| Interfaces | structural: any value of the right shape | traits, implemented with `impl` |
| Numbers | `number`, a 64-bit float, and `bigint` | `Int`, `Float` and the sized integers, each its own type |
| A function that waits | `async`, returns a `Promise`, and so do its callers | written like any other |
| Running at once | one thread; workers that exchange messages | tasks on every core |
| What reads JSON | `JSON.parse`, and a cast or a library to check it | `json.decode[T]`, which checks |
| What ships | sources or a bundle, for a runtime | one native executable |

## Values and references

`const` in TypeScript fixes the name. The array it names can still change,
through that name or any other that was given it:

```typescript
const original = [1, 2, 3];
const alias = original; // the same array under another name: `const` fixes the name, not the array
alias[0] = 99;
alias.push(4);
console.log(original, alias);

const home = { x: 1, y: 2 };
const away = home; // the same object, too
away.x = 10;
console.log(home.x, away.x);
```

```text
[ 99, 2, 3, 4 ] [ 99, 2, 3, 4 ]
10 10
```

In Tondo an array is a value. Assigning it gives the other name its own,
and a `let` is constant all the way down:

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

- There is no spreading to copy, `[...items]` or `{ ...user }`, and no
  `structuredClone`: assigning is the copy. It costs nothing until one of
  the two changes, since they share their storage until then.
- A function cannot change what it was given unless its parameter says
  `mut` and the call says `mut` too. `readonly` and `Readonly<T>` promise
  that for one level; here it holds for the whole value.
- `let` is TypeScript's `const`, made true, and `var` is its `let`.
- `user with { name: "Ada" }` is `{ ...user, name: "Ada" }`, checked: a
  field that does not exist is an error.

## A value that may be missing

TypeScript has two ways not to have a value, and with `strict` on it does
well at making code look:

```typescript
type User = { name: string; email?: string };

function find(users: User[], name: string): User | undefined {
  return users.find((user) => user.name === name);
}

const users: User[] = [{ name: "ada", email: "ada@example.com" }, { name: "alan" }];
for (const name of ["ada", "alan", "grace"]) {
  // With strict checks on, leaving out `?.` here does not compile.
  const email = find(users, name)?.email ?? "no email";
  console.log(`${name}: ${email}`);
}
```

Tondo has one, and it is a type of its own. There is no `undefined` for a
field that was never set and no `null` that someone assigned:

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

- `?.` and `??` are `?.` and `??`, on an option where TypeScript has them
  on `undefined`. `match` opens an option when both cases need code.
- An index is the place TypeScript trusts too much. `items[10]` has type
  `T` and is `undefined`; a key of a `Record<string, string>` is typed
  `string` and may not be there. In Tondo `map.get(key)` is an option, and
  an array index out of range stops the program instead of going on with
  nothing.
- There is no `!` to say "trust me". `value ?? fallback` gives a value,
  and `expect("why it must be there")` panics with the reason if it is not.
- No truthiness: `if (text)` is `if not text.isEmpty()`, and `0` and `""`
  are never mistaken for missing.

## Errors

Anything can be thrown in JavaScript, by anything, and TypeScript has no
way to write that down:

```typescript
class ConfigError extends Error {}

function port(settings: Record<string, string>, key: string): number {
  const text = settings[key]; // typed `string`; `undefined` when the key is not there
  if (text === undefined) {
    throw new ConfigError(`${key} is missing`);
  }
  const value = Number(text);
  if (!Number.isInteger(value)) {
    throw new ConfigError(`${key}: "${text}" is not a number`);
  }
  if (value < 1 || value > 65535) {
    throw new ConfigError(`${key}: ${value} is out of range`);
  }
  return value;
}

// Nothing in this signature says that it can fail, or with what.
function address(settings: Record<string, string>): string {
  return `0.0.0.0:${port(settings, "port")}`;
}

const all: Record<string, string>[] = [{ port: "8080" }, {}, { port: "http" }, { port: "70000" }];
for (const settings of all) {
  try {
    console.log(address(settings));
  } catch (error) {
    // What was caught is `unknown`: anything can be thrown.
    console.log(`error: ${error instanceof ConfigError ? error.message : error}`);
  }
}
```

A Tondo function that fails says so in its type, with what:

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

- `Int ! ConfigError` is the whole contract. The error is a value of a
  known type, so there is no `instanceof` to guess with and no `unknown` in
  a `catch`.
- `?` passes a failure on. It is what an exception does by itself, written
  and checked.
- `Number("http")` is `NaN`, a number, and the check for it is the
  program's to remember. `Int.parse("http")` is an error that has to be
  handled before there is any number.
- A result that is not used does not compile, so a failure cannot be
  dropped the way a rejected promise is when nobody awaits it.

## One of several things

This is TypeScript at its best: a union of shapes with a tag, narrowed
case by case, and an error when a case is missing:

```typescript
type Shape =
  | { kind: "circle"; radius: number }
  | { kind: "rectangle"; width: number; height: number }
  | { kind: "triangle"; a: number; b: number; c: number };

function area(shape: Shape): number {
  switch (shape.kind) {
    case "circle":
      return Math.PI * shape.radius ** 2;
    case "rectangle":
      return shape.width * shape.height;
    case "triangle": {
      const s = (shape.a + shape.b + shape.c) / 2;
      return Math.sqrt(s * (s - shape.a) * (s - shape.b) * (s - shape.c));
    }
  }
}

const shapes: Shape[] = [
  { kind: "circle", radius: 1 },
  { kind: "rectangle", width: 2, height: 3.5 },
  { kind: "triangle", a: 3, b: 4, c: 5 },
];
for (const shape of shapes) {
  console.log(area(shape).toFixed(2));
}
```

Tondo's `enum` is that union with less to write, and the tag is real:

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

- A variant is made by name, `Shape.Circle(1.0)`, and `match` takes it
  apart. There is no `kind` field to keep in step, since the variant is the
  tag.
- `match` must cover every variant, as the `switch` above must once the
  function's return type is checked. Adding a shape breaks both.
- TypeScript's unions of anything are in Tondo too: `Int | String` is a
  type, and a `match` tells its members apart by their types.
- A record is `type Point = { x: Float, y: Float }`. Two record types with
  the same fields are different types: a `Meters` is not a `Feet`.

## Interfaces, traits and generics

A TypeScript interface describes a shape, and any value with that shape
fits:

```typescript
interface Describe {
  describe(): string;
}

class Book {
  constructor(readonly title: string, readonly pages: number) {}

  describe(): string {
    return `${this.title}, ${this.pages} pages`;
  }
}

class Song {
  constructor(readonly title: string, readonly seconds: number) {}

  describe(): string {
    return `${this.title}, ${Math.floor(this.seconds / 60)}:${String(this.seconds % 60).padStart(2, "0")}`;
  }
}

// Anything with the right shape is a Describe: no class says so.
function announce(item: Describe): string {
  return `Now: ${item.describe()}`;
}

function largest<T extends number | string>(items: T[]): T | undefined {
  let best = items[0];
  for (const item of items) {
    if (item > best) {
      best = item;
    }
  }
  return best;
}

console.log(announce(new Book("Dune", 612)));
console.log(announce(new Song("Hey Jude", 431)));
console.log(largest([3, 9, 4]), largest(["pear", "apple"]), largest([]));
```

```text
Now: Dune, 612 pages
Now: Hey Jude, 7:11
9 pear undefined
```

A Tondo trait is implemented on purpose, and a generic function is compiled
for each type it is used with:

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

- `impl Describe for Book` is where the type and the trait meet. A type
  does not fit by accident, and "who implements this" is a search for
  `impl Describe`.
- `largest` takes any type with an order, `T: Ordered`, where the
  TypeScript one lists the types it knows how to compare. Its result is a
  `T?`, so the empty array is in the type.
- Generics are not erased: `Array[Int]` and `Array[String]` are different
  code, and a program can ask a type for its fields when it compiles.
- There are no classes. Data is records and enums; behaviour is functions,
  methods and traits.

## Collections

```typescript
const text = "to be or not to be that is the question to be";
const words = text.split(" ");

const counts = new Map<string, number>(); // keeps the order its keys were added in
for (const word of words) {
  counts.set(word, (counts.get(word) ?? 0) + 1);
}
// The three most frequent; the sort is stable, so those that tie stay in order.
const top = [...counts].sort((a, b) => b[1] - a[1]).slice(0, 3);
console.log(top.map(([word, count]) => `${word} ${count}`).join(", "));

const lengths = words.map((word) => word.length);
const letters = lengths.reduce((sum, n) => sum + n, 0);
console.log(`${lengths.filter((n) => n > 2).length} long words, ${letters} letters`);
```

Tondo's `Map` keeps its order too, and reads like an object: `counts[word]`
sets, and `get` gives an option.

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

- `Array[T]`, `Map[K, V]` and `Set[T]` are `T[]`, `Map<K, V>` and
  `Set<T>`. There is no object used as a dictionary beside the `Map`.
- `map`, `filter`, `find`, `some` as `any`, `every` as `all`, `sortedBy`,
  `sum` and `join` are methods. A closure is `(word) { word.length() }`.
- Sorting gives a new array, `sorted()`, or changes a `var` in place,
  `sort()`, and numbers sort as numbers.
- `Int` and `Float` are different types. `7 / 2` is `3`, `7.0 / 2.0` is
  `3.5`, and mixing them is `Float(count) * rate`.

## Tasks

A promise for each piece of work, and `await` to wait for them:

```typescript
// Stands for a request that takes a while. It waits, so it is `async`,
// gives a Promise, and so does everything that calls it.
async function fetchOne(name: string, delay: number): Promise<string> {
  await new Promise((resolve) => setTimeout(resolve, delay));
  return `${name} in ${delay} ms`;
}

const started = performance.now();
// Each call starts at once; Promise.all waits for the three.
const results = await Promise.all([fetchOne("users", 300), fetchOne("orders", 500), fetchOne("prices", 100)]);
for (const result of results) {
  console.log(result);
}
// One after another they would take 900 ms.
console.log("all three in under 800 ms:", performance.now() - started < 800);
```

Tondo has one kind of function. Any of them can wait, and doing several
things at once is `spawn`:

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

- No `async`, no `await`, no `Promise<T>` in a signature. A function that
  starts waiting one day does not change the signature of every function
  above it.
- A forgotten `await` cannot happen. `spawn f()` gives a `Join[T]`, which
  its block waits for when it ends, and an error in the task comes out of
  `wait()` instead of becoming an unhandled rejection.
- A `Group` is `Promise.all`, with its results in the order the tasks were
  added.

## Deadlines and cancelling

A promise cannot be cancelled. What can is the work behind it, if each
function takes an `AbortSignal` and listens to it:

```typescript
// A wait that can be cancelled has to be written to listen for it.
function sleep(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(resolve, ms);
    signal.addEventListener("abort", () => {
      clearTimeout(timer);
      reject(signal.reason);
    });
  });
}

// The signal is a parameter of every function that may have to stop.
async function fetchOne(delay: number, signal: AbortSignal): Promise<string> {
  await sleep(delay, signal);
  return `answered in ${delay} ms`;
}

for (const delay of [10, 5000]) {
  try {
    console.log(await fetchOne(delay, AbortSignal.timeout(250)));
  } catch {
    console.log("gave up after 250 ms");
  }
}
```

In Tondo every wait can be cancelled, and a deadline goes around a block:

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

- `time.sleep` did not have to be rewritten to listen for a signal, and
  `fetch` takes no extra parameter.
- When the time passes, the block's `defer`s run, the tasks it started are
  cancelled, and `within` gives `err(time.TimedOut)`.
- An HTTP request or a database query made inside a `within` has that
  long.

## Shared state, and every core

JavaScript runs on one thread, which is why this needs no lock, and why
only one core does the work:

```typescript
const hits = new Map<string, number>();

// One thread runs all of this, a piece at a time: nothing happens at once,
// so there is nothing to lock, and only one core does the work.
async function work(page: string): Promise<void> {
  for (let i = 0; i < 100; i++) {
    hits.set(page, (hits.get(page) ?? 0) + 1);
    await Promise.resolve(); // lets the others have a turn
  }
}

await Promise.all(Array.from({ length: 50 }, (_, n) => work(n % 2 === 0 ? "/home" : "/about")));
console.log(hits.get("/home"), hits.get("/about"));
```

Tondo's tasks run on every core. What they share has to be made for
sharing, and the compiler holds them to it:

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

- A task cannot be given a plain variable to change: that does not compile.
  A `sync.Mutex` holds the value it guards, and `update` is the way in.
- A computation that takes a second holds up one task, not the whole
  program. There are no workers to start and no messages to copy between
  them.
- Atomics, channels and shared collections are in `std.sync`.

## JSON, and the edge of the program

This is where erased types hurt. `JSON.parse` returns `any`, and `as
Person` is the programmer telling the compiler what is there:

```typescript
type Person = {
  name: string;
  age: number;
  languages?: string[];
  email?: string;
};

const text = '{"name": "Ada", "age": 36, "languages": ["en", "fr"]}';
const person = JSON.parse(text) as Person; // a promise to the compiler, not a check
console.log(`${person.name}, ${person.age}, [${person.languages}], ${person.email}`);
console.log(JSON.stringify({ ...person, age: 37 }));

// The type says `number`. The types are gone when this runs: the text goes in.
const other = JSON.parse('{"name": "Alan", "age": "forty"}') as Person;
console.log(other.name, typeof other.age, other.age + 1);
```

```text
Ada, 36, [en,fr], undefined
{"name":"Ada","age":37,"languages":["en","fr"]}
Alan string forty1
```

Alan's age is a string, in a field typed `number`, and `age + 1` is
`"forty1"`. Nothing was wrong as far as the compiler could tell. In Tondo
the type does the checking:

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

- `json.decode[Person](text)` is what `zod` or `io-ts` is added for: the
  schema is the type, written once.
- Every edge of a program works this way. A request's body, a row of a
  query, a line of a configuration file and the arguments of a command are
  read into types, and what does not fit is an error that says where.
- JSON of no known shape is a `json.Value`, read with
  `value.find("items[0].name")`: the one place where the program says it
  does not know what it has.

## Tests

```typescript
import assert from "node:assert/strict";

/** The words of a title, each with a capital. */
export function titleCase(text: string): string {
  return text
    .split(" ")
    .filter((word) => word !== "")
    .map((word) => word[0].toUpperCase() + word.slice(1).toLowerCase())
    .join(" ");
}

Deno.test("capitalizes each word", () => {
  assert.equal(titleCase("the old man and the sea"), "The Old Man And The Sea");
});

Deno.test("keeps one space between words", () => {
  assert.equal(titleCase("  war   AND peace "), "War And Peace");
  assert.equal(titleCase(""), "");
});
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

- `tondo test` runs them, with nothing to configure. `assert(a == b)`
  compares by value, records and arrays included, and prints both sides
  when it fails.
- Benchmarks, fuzz tests, snapshots and coverage are in the same command.

## An HTTP server and its client

With Deno's server and `fetch`:

```typescript
type Greeting = { message: string; length: number };

const route = new URLPattern({ pathname: "/hello/:name" });
const server = Deno.serve({ port: 0, onListen() {} }, (request) => { // a free port
  const name = route.exec(request.url)?.pathname.groups.name;
  if (name === undefined) {
    return new Response("not found", { status: 404 });
  }
  return Response.json({ message: `Hello, ${name}!`, length: name.length });
});
const url = `http://localhost:${server.addr.port}`;

const greeting = await (await fetch(`${url}/hello/Ada`)).json() as Greeting; // unchecked, as before
console.log(`${greeting.message} (${greeting.length})`);
const missing = await fetch(`${url}/nothing`);
console.log(missing.status); // a 404 is not an error here: the code has to look
await missing.body?.cancel();
await server.shutdown();
```

The same in Tondo:

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

- The reply is read into a `Greeting` and checked. In the TypeScript above
  it is cast, so a server that changes its answer is found by a user.
- Routes are declared with their parameters, `"/hello/:name"`, and a typed
  handler takes them as arguments: `app.get("/users/:id", (id: Int) {
  find(id) })`.
- The client fails with an error for a status of 400 or more, where
  `fetch` resolves and leaves `response.ok` to be checked.
- WebSockets, server-sent events, SQL, templates and gRPC are in the
  standard library.

## The toolchain, command by command

| TypeScript | Tondo | |
| --- | --- | --- |
| `node`, `deno run`, `tsx` | `tondo run` | compiles, keeps the build, runs |
| `tsc --noEmit` | `tondo check` | it is the compiler: nothing runs unchecked |
| `tsconfig.json`, with `strict` | nothing | there is one language, with no flags |
| `prettier`, `eslint` | `tondo fmt`, `tondo check`, `tondo fix` | one layout, no configuration |
| `npm install`, `node_modules` | `tondo add` | `tondo.toml` and `tondo.lock`; the standard library covers most of it |
| `jest`, `vitest` | `tondo test` | with `--bench`, `--fuzz`, `--coverage` |
| a bundler, a Docker image with Node | `tondo build` | one file that needs nothing installed |
| the debugger of the browser or the editor | `tondo debug` | also backwards, over a recording |
| `tsserver` | `tondo lsp` | in the same binary |
| `zod`, `express`, `pg`, `commander`, `ws` | `std.json`, `std.http`, `std.sql`, `std.cli` | in the standard library |

## Habits to leave behind

| In TypeScript | In Tondo |
| --- | --- |
| `const x = 1`, `let y = 2` | `let x = 1`, `var y = 2` |
| `function f(a: number): string` | `fn f(a: Int): String` |
| `(x) => x * 2` | `(x) { x * 2 }` |
| `x?: string`, `string \| undefined` | `x: String?` |
| `a?.b ?? c` | `a?.b ?? c` |
| `x!`, `x as Foo`, `any` | nothing: say what the value is |
| `throw new Error(...)`, `try`, `catch` | `fail`, `T ! E`, `?`, `match` |
| `type A = { kind: "a" } \| { kind: "b" }` | `enum`, with `match` |
| `interface`, `class`, `extends` | `trait`, `type`, `impl` |
| `{ ...user, name }` | `user with { name }` |
| `` `${name} is ${age}` `` | `"{name} is {age}"` |
| `===`, `!==` | `==`, `!=`: there is only one equality, by value |
| `&&`, `\|\|`, `!` | `and`, `or`, `not` |
| `items.length`, `text.length` | `items.length()`, `text.length()` |
| `for (const x of xs)`, `while (c)` | `for x in xs`, `for c` |
| `switch` | `match` |
| `async`, `await`, `Promise<T>` | nothing: every function can wait |
| `Promise.all([...])` | `spawn`, and a `Group` |
| `export`, `import { a } from "./b"` | `pub`, `import app.b` |
| `number` | `Int` or `Float` |
| `;` | nothing |

`tondo guide pitfalls` has these and the error each gives.

## Where TypeScript is the better choice today

- **The browser.** Anything with a user interface on the web is
  JavaScript's, and TypeScript is the way to write it. Tondo does not run
  there.
- **One language on both sides.** Sharing types and code between a page and
  its server is worth a great deal.
- **Its ecosystem.** npm has a package for everything, and every service
  has a JavaScript client. Tondo is in early development and can still
  change.
- **Types over what already exists.** TypeScript can describe a JavaScript
  library as it is, loosely where it has to. Tondo's types have no loose
  mode.

What Tondo offers against that is for the server and the tool: types that
are still there when the program runs, input checked where it enters,
failures in signatures, every core, and one executable in place of a
runtime, a lock file and a directory of packages.

Next: [Tondo for someone who writes Java](java.md).
