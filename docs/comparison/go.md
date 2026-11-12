# Tondo for someone who writes Go

Go is the language Tondo is closest to in what it is for: servers, tools
and command lines, compiled fast into one executable, by a toolchain that
has an opinion. Much of a Tondo program reads the way a Go program does.
This page takes twelve small programs and writes each in both, to show
where the two part and why. Every program here was run, and prints what
is shown under it.

- [The short version](#the-short-version)
- [Values and copies](#values-and-copies)
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
- [Where Go is the better choice today](#where-go-is-the-better-choice-today)

## The short version

| | Go | Tondo |
| --- | --- | --- |
| A function fails | returns an `error` beside its result, checked with `if err != nil` | returns `T ! E`, passed on with `?` |
| An error nobody looked at | compiles | does not compile |
| A value that may be missing | `nil`, a zero value, or `value, ok` | `T?`, which has to be opened |
| Assigning a slice or a map | shares its storage | copies it; the storage is shared until one changes |
| A type with several shapes | an interface and a type switch, with a `default` | an `enum` with data and a `match` that covers every case |
| Interfaces | satisfied by having the methods | traits, implemented with `impl Trait for Type` |
| Generics | type parameters with constraints | type parameters with trait bounds, resolved when compiling |
| A goroutine | runs until it ends, whoever started it | a task belongs to its block, which waits for it |
| Cancelling and deadlines | a `context.Context` parameter in every function | `within`, around the block; no parameter |
| A lock | a `sync.Mutex` beside what it guards | a `sync.Mutex[T]` around what it guards |
| A data race | found by `-race`, when a test runs into it | does not compile |
| An integer overflows | wraps | panics, in every build |
| Exported names | start with a capital | are marked `pub` |
| Formatting | `gofmt` | `tondo fmt`, also with no options |
| What ships | one native executable | one native executable |

## Values and copies

The first difference is under the simplest code. In Go a slice is a view of
an array, and assigning it gives a second view of the same storage. Whether
a later `append` moves the copy to storage of its own depends on its
capacity, which the code does not show:

```go
package main

import "fmt"

type Point struct{ X, Y int }

func main() {
	original := []int{1, 2, 3}
	alias := original // the same storage under another name
	alias[0] = 99
	alias = append(alias, 4) // no room left: this one moves to new storage
	fmt.Println(original, alias)

	home := Point{X: 1, Y: 2}
	away := home // a struct is copied
	away.X = 10
	fmt.Println(home.X, away.X)
}
```

```text
[99 2 3] [99 2 3 4]
1 10
```

In Tondo an array is a value, as a struct is in both languages. Assigning it
copies it, and the two are apart from then on:

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
  storage, and the first write to a shared array makes it its own.
- Maps, sets and strings are values in the same way. There is no `copy()`
  to remember, no `slices.Clone`, and no function that changes its caller's
  slice by surprise.
- A function that is meant to change its argument says so, and so does its
  caller: `fn add(items: mut Array[Int])`, called as `add(mut numbers)`.
- `var` is a binding that can change and `let` one that cannot. There is no
  `:=` beside `=`.

## A value that may be missing

Go has three ways to say that something is not there: a `nil` pointer, the
zero value, and a second result. Each one is a convention, and forgetting
it compiles:

```go
package main

import "fmt"

type User struct {
	Name  string
	Email *string
}

func find(users []User, name string) *User {
	for i := range users {
		if users[i].Name == name {
			return &users[i]
		}
	}
	return nil
}

func main() {
	email := "ada@example.com"
	users := []User{{Name: "ada", Email: &email}, {Name: "alan"}}
	for _, name := range []string{"ada", "alan", "grace"} {
		text := "no email"
		// Forgetting either check compiles, and panics when it matters.
		if user := find(users, name); user != nil && user.Email != nil {
			text = *user.Email
		}
		fmt.Printf("%s: %s\n", name, text)
	}
}
```

In Tondo the type says it, `User?` and `String?`, and the value cannot be
used until it is opened:

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

- There is no `nil`. A `User` is always a user, and a `String` is always a
  string, so there is nothing to check and nothing to forget.
- `match`, `unwrapOr`, `map` and `andThen` open an option. `?` passes a
  `none` on from a function that itself returns an option.
- A map lookup gives an option: `ages.get("cy")` is `none`, not a zero that
  looks like an age. An index out of range is a panic, as in Go.
- A record's field can have a default, `port: Int = 8080`. There are no zero
  values: a field with no default has to be given.

## Errors

The shape of Go's error handling is three lines after each call. This
function reads a port from some settings:

```go
package main

import (
	"fmt"
	"strconv"
)

func port(settings map[string]string, key string) (int, error) {
	text, ok := settings[key]
	if !ok {
		return 0, fmt.Errorf("%s is missing", key)
	}
	value, err := strconv.Atoi(text)
	if err != nil {
		return 0, fmt.Errorf("%s: %q is not a number", key, text)
	}
	if value < 1 || value > 65535 {
		return 0, fmt.Errorf("%s: %d is out of range", key, value)
	}
	return value, nil
}

func address(settings map[string]string) (string, error) {
	value, err := port(settings, "port")
	if err != nil {
		return "", err
	}
	return fmt.Sprintf("0.0.0.0:%d", value), nil
}

func main() {
	all := []map[string]string{{"port": "8080"}, {}, {"port": "http"}, {"port": "70000"}}
	for _, settings := range all {
		found, err := address(settings)
		if err != nil {
			fmt.Println("error:", err)
			continue
		}
		fmt.Println(found)
	}
}
```

Tondo keeps Go's idea, that an error is a value the caller handles, and
changes two things. The error has a type, which is part of the function's
signature, and `?` does what the three lines do:

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

- `Int ! ConfigError` says what the function gives and what it fails with.
  The caller can `match` on the cases, and the compiler knows them all.
- A result that is not used does not compile. `_ = save()` discards one on
  purpose, where Go lets `save()` drop its error without a word.
- `fail` returns an error, and `?` passes one on. `mapErr` turns one error
  into another, as `fmt.Errorf` wraps one.
- For programs that only want to say what went wrong, `Failure` is one
  error type that any error fits in, and `.context("reading the file")`
  adds what `fmt.Errorf("reading the file: %w", err)` adds.
- A panic is still a panic: a bug, not a failure. `recover` is a block,
  `recover { ... }`, that gives a `T ! Panic`, used at the edge of a
  request or a job.

## One of several things

Go has no type for "one of these". The usual answer is an interface with a
method nobody calls, one struct for each case, and a type switch:

```go
package main

import (
	"fmt"
	"math"
)

type Shape interface{ isShape() }

type Circle struct{ Radius float64 }
type Rectangle struct{ Width, Height float64 }
type Triangle struct{ A, B, C float64 }

func (Circle) isShape()    {}
func (Rectangle) isShape() {}
func (Triangle) isShape()  {}

func area(shape Shape) float64 {
	switch s := shape.(type) {
	case Circle:
		return math.Pi * s.Radius * s.Radius
	case Rectangle:
		return s.Width * s.Height
	case Triangle:
		p := (s.A + s.B + s.C) / 2
		return math.Sqrt(p * (p - s.A) * (p - s.B) * (p - s.C))
	default:
		// The compiler does not know that the cases above are all there are.
		panic("a shape nobody handled")
	}
}

func main() {
	for _, shape := range []Shape{Circle{1}, Rectangle{2, 3.5}, Triangle{3, 4, 5}} {
		fmt.Printf("%.2f\n", area(shape))
	}
}
```

Tondo has enums whose variants carry data, and `match` has to cover every
one of them:

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

- Adding a fourth shape makes every `match` that forgot it an error that
  names the missing case. In Go the switch still compiles, and the
  `default` is found when the program runs.
- An enum with no data is what a `const` block with `iota` is used for,
  and it prints its name with nothing to generate.
- `math.sqrt` returns a result, since the root of a negative number is not
  a number: the failure is in the type, where Go's `math.Sqrt` gives a
  `NaN` to find later.

## Interfaces, traits and generics

A Go type has an interface by having its methods. Generics came later and
are written with constraints:

```go
package main

import (
	"cmp"
	"fmt"
)

type Describer interface{ Describe() string }

type Book struct {
	Title string
	Pages int
}

type Song struct {
	Title   string
	Seconds int
}

// A type has the interface by having the method: nothing says so.
func (b Book) Describe() string { return fmt.Sprintf("%s, %d pages", b.Title, b.Pages) }
func (s Song) Describe() string {
	return fmt.Sprintf("%s, %d:%02d", s.Title, s.Seconds/60, s.Seconds%60)
}

func announce(item Describer) string { return "Now: " + item.Describe() }

func largest[T cmp.Ordered](items []T) (best T, found bool) {
	if len(items) == 0 {
		return best, false
	}
	best = items[0]
	for _, item := range items {
		if item > best {
			best = item
		}
	}
	return best, true
}

func main() {
	fmt.Println(announce(Book{"Dune", 612}))
	fmt.Println(announce(Song{"Hey Jude", 431}))
	n, _ := largest([]int{3, 9, 4})
	s, _ := largest([]string{"pear", "apple"})
	_, found := largest([]int{})
	fmt.Println(n, s, found)
}
```

```text
Now: Dune, 612 pages
Now: Hey Jude, 7:11
9 pear false
```

A Tondo trait is implemented on purpose, with `impl`. Generic functions take
a bound, and the compiler writes a version for each type they are used
with, so there is no interface value and no dynamic call:

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

- `impl Describe for Book` is where the two meet. A type cannot satisfy a
  trait by accident, and finding who implements a trait is a search for
  `impl Describe`.
- `largest` returns a `T?`. Go's version needs a second result and a zero
  value to return beside `false`.
- Methods are functions named with their type, `fn Book.pages(self)`,
  declared anywhere in the type's module. There are no pointer receivers:
  a method that changes its value takes `mut self`.
- There is no `any`. Where Go would take an `interface{}`, Tondo takes a
  generic, an enum, or a union of the types it accepts, `Int | String`.

## Collections

Counting words, the hundredth time. In Go the map has no order, so the
program keeps one beside it:

```go
package main

import (
	"fmt"
	"sort"
	"strings"
)

func main() {
	text := "to be or not to be that is the question to be"
	words := strings.Fields(text)
	counts := map[string]int{}
	var seen []string // a map has no order of its own: keep the order they came in
	for _, word := range words {
		if counts[word] == 0 {
			seen = append(seen, word)
		}
		counts[word]++
	}
	sort.SliceStable(seen, func(i, j int) bool { return counts[seen[i]] > counts[seen[j]] })
	var top []string
	for _, word := range seen[:3] {
		top = append(top, fmt.Sprintf("%s %d", word, counts[word]))
	}
	fmt.Println(strings.Join(top, ", "))

	long, letters := 0, 0
	for _, word := range words {
		letters += len(word)
		if len(word) > 2 {
			long++
		}
	}
	fmt.Printf("%d long words, %d letters\n", long, letters)
}
```

A Tondo map keeps the order its keys were added in, and arrays have the
methods that Go writes as loops:

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
  There is no `make`, no capacity to think about, and no nil map that
  panics on its first write.
- A map's order is the order of insertion, every run. A test that prints a
  map prints the same thing each time.
- `map`, `filter`, `sortedBy`, `take`, `sum`, `join` and the rest are
  methods. A `for` loop is as good, and often clearer.
- A string is text, measured in characters: `"añil".length()` is 4. Its
  bytes are `text.bytes()`.

## Tasks

Goroutines and tasks are the same kind of thing: cheap, scheduled over
every core, and written as ordinary calls with no `async`. What differs is
who waits for them. In Go the program has to arrange it:

```go
package main

import (
	"fmt"
	"sync"
	"time"
)

// Stands for a request that takes a while.
func fetch(name string, delay int) string {
	time.Sleep(time.Duration(delay) * time.Millisecond)
	return fmt.Sprintf("%s in %d ms", name, delay)
}

func main() {
	started := time.Now()
	jobs := []struct {
		name  string
		delay int
	}{{"users", 300}, {"orders", 500}, {"prices", 100}}
	results := make([]string, len(jobs)) // a place for each result, by index
	var wg sync.WaitGroup
	for i, job := range jobs {
		wg.Add(1)
		go func() {
			defer wg.Done()
			results[i] = fetch(job.name, job.delay)
		}()
	}
	wg.Wait() // forgetting this line compiles, and prints three empty lines
	for _, result := range results {
		fmt.Println(result)
	}
	// One after another they would take 900 ms.
	fmt.Println("all three in under 800 ms:", time.Since(started) < 800*time.Millisecond)
}
```

In Tondo a task belongs to the block that started it, which does not end
before the task does. A `Group` collects their results:

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

- `spawn f()` gives a `Join[T]`, the task and its result. `job.wait()` is
  the result, with no channel and no shared slice to carry it.
- A task nobody waited for is waited for when its block ends. A function
  that returns has left nothing running. The goroutine of Go, which
  outlives whoever started it, is `spawn detached f()`, and it is the
  exception.
- An error or a panic in a task comes out of `wait()`. It cannot vanish
  with the goroutine.
- Channels are `channel[T](capacity)`, with `send`, `receive`, `for value
  in receiver` and `select`. They close when their last sender goes.

## Deadlines

A deadline in Go is a `context.Context`, passed as the first parameter of
every function between where it is set and where it matters:

```go
package main

import (
	"context"
	"fmt"
	"time"
)

// The context is a parameter of every function that may have to stop.
func fetch(ctx context.Context, delay int) (string, error) {
	select {
	case <-time.After(time.Duration(delay) * time.Millisecond):
		return fmt.Sprintf("answered in %d ms", delay), nil
	case <-ctx.Done():
		return "", ctx.Err()
	}
}

func main() {
	for _, delay := range []int{10, 5000} {
		ctx, cancel := context.WithTimeout(context.Background(), 250*time.Millisecond)
		answer, err := fetch(ctx, delay)
		cancel()
		if err != nil {
			fmt.Println("gave up after 250 ms")
			continue
		}
		fmt.Println(answer)
	}
}
```

In Tondo the deadline is around the block. Everything the block calls and
every task it starts is under it, and no function takes it as a parameter:

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

- `within` gives its block's value, or `err(time.TimedOut)`. When the time
  passes, the block's next wait ends, its `defer`s run and its tasks are
  cancelled.
- `time.sleep` in the Tondo version is an ordinary sleep. It did not have
  to be written as a `select` on `ctx.Done()` to be cancellable: every wait
  is.
- The library knows about it: an HTTP request or a gRPC call made inside a
  `within` has that long.
- What `context.WithValue` carries, a request's id or a logger, is a
  `context` declared once and read by name in any function below, with its
  fields typed.

## Shared state

A Go mutex is a field beside the data it guards. Which fields it guards is
a comment, when there is one:

```go
package main

import (
	"fmt"
	"sync"
)

// The lock and what it guards are two fields: nothing ties them together.
type Hits struct {
	mu     sync.Mutex
	counts map[string]int
}

func (h *Hits) work(page string) {
	for range 100 {
		h.mu.Lock()
		h.counts[page]++
		h.mu.Unlock()
	}
}

func main() {
	hits := &Hits{counts: map[string]int{}}
	var wg sync.WaitGroup
	for n := range 50 {
		page := "/about"
		if n%2 == 0 {
			page = "/home"
		}
		wg.Add(1)
		go func() {
			defer wg.Done()
			hits.work(page)
		}()
	}
	wg.Wait()
	fmt.Println(hits.counts["/home"], hits.counts["/about"])
}
```

A Tondo mutex holds the value. The only way to the map is through
`update`, which has the lock while its function runs:

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

- There is no `Lock` without its `Unlock`, and no way to read the map
  without the lock.
- A task cannot be given a plain `var` to change: what tasks share has to
  be something made for sharing, a `sync.Mutex`, an `Atomic`, a channel.
  The data race that `go test -race` finds when a test happens to run into
  it is a compile error here.
- `sync` also has `RwLock`, `Atomic`, `Once`, `Semaphore`, `Barrier` and
  shared collections, and `tondo test` runs a test that starts tasks in the
  orders that could change its result.

## JSON

Go decodes into a struct through reflection, guided by tags in strings:

```go
package main

import (
	"encoding/json"
	"fmt"
)

type Person struct {
	Name      string   `json:"name"`
	Age       int      `json:"age"`
	Languages []string `json:"languages"`
	Email     *string  `json:"email"`
}

func main() {
	var person Person
	text := `{"name": "Ada", "age": 36, "languages": ["en", "fr"]}`
	if err := json.Unmarshal([]byte(text), &person); err != nil {
		fmt.Println(err)
		return
	}
	fmt.Println(person.Name, person.Age, person.Languages, person.Email)
	person.Age = 37
	encoded, _ := json.Marshal(person)
	fmt.Println(string(encoded))

	var other Person
	err := json.Unmarshal([]byte(`{"name": "Alan", "age": "forty"}`), &other)
	fmt.Println(err)
}
```

```text
Ada 36 [en fr] <nil>
{"name":"Ada","age":37,"languages":["en","fr"],"email":null}
json: cannot unmarshal string into Go struct field Person.age of type int
```

In Tondo any record is read from JSON and written to it by its field names.
Reading is strict about types, and says where:

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

- There are no struct tags. A field named differently on the wire carries
  `@name("user_id")`, checked by the compiler like any other code.
- A field that is not optional and has no default must be there. Go fills
  what is missing with zeros, and the program cannot tell.
- The same record reads and writes YAML, TOML, CBOR and MessagePack.
- `@validate.Min(0)` on a field is checked by the decoder, so a handler
  never sees a value that breaks its rules.

## Tests

A table-driven test in Go:

```go
package titles

import "testing"

func TestTitleCase(t *testing.T) {
	cases := []struct{ in, want string }{
		{"the old man and the sea", "The Old Man And The Sea"},
		{"  war   AND peace ", "War And Peace"},
		{"", ""},
	}
	for _, c := range cases {
		if got := TitleCase(c.in); got != c.want {
			t.Errorf("TitleCase(%q) = %q, want %q", c.in, got, c.want)
		}
	}
}
```

In Tondo a test is a `test` block, in a file beside the code, and `assert`
takes any condition. When one fails, it prints both sides:

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

- `tondo test` is `go test`. Tests in `name_test.to` see the module's
  private names, and tests in `tests/` see only what is `pub`.
- A failed `assert(a == b)` shows `a` and `b`, so there is no `got` and
  `want` to format by hand.
- Benchmarks, fuzz tests, snapshots, a virtual clock and coverage are in
  the same command, and so is checking a state machine in every state it
  can reach.

## An HTTP server and its client

Go's standard library made a web server a few lines. Here a route with a
parameter answers JSON, and a client reads it:

```go
package main

import (
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
)

type Greeting struct {
	Message string `json:"message"`
	Length  int    `json:"length"`
}

func main() {
	mux := http.NewServeMux()
	mux.HandleFunc("GET /hello/{name}", func(w http.ResponseWriter, r *http.Request) {
		name := r.PathValue("name")
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(Greeting{Message: "Hello, " + name + "!", Length: len(name)})
	})
	server := httptest.NewServer(mux) // on a free port
	defer server.Close()

	response, err := http.Get(server.URL + "/hello/Ada")
	if err != nil {
		fmt.Println(err)
		return
	}
	defer response.Body.Close()
	var greeting Greeting
	if err := json.NewDecoder(response.Body).Decode(&greeting); err != nil {
		fmt.Println(err)
		return
	}
	fmt.Printf("%s (%d)\n", greeting.Message, greeting.Length)

	missing, err := http.Get(server.URL + "/nothing")
	if err != nil {
		fmt.Println(err)
		return
	}
	defer missing.Body.Close()
	fmt.Println(missing.StatusCode) // a 404 is not an error here: the code has to look
}
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
  handler, `app.get("/users/:id", (id: Int) { find(id) })`, takes its
  parameters from the path and the query and writes what it returns as
  JSON.
- The client fails with an error for a status of 400 or more, where Go
  returns the response and leaves the status to be checked.
- WebSockets, server-sent events, HTTP/2, TLS, gRPC, OpenAPI and metrics
  are in `std.http` and beside it.

## The toolchain, command by command

| Go | Tondo | |
| --- | --- | --- |
| `go run .` | `tondo run` | compiles, keeps the build, runs |
| `go build` | `tondo build` | one executable; `--static`, `--target aarch64-linux` |
| `go test ./...` | `tondo test` | with `--bench`, `--fuzz`, `--coverage` |
| `go test -race` | not needed | a data race does not compile |
| `gofmt`, `go vet` | `tondo fmt`, `tondo check` | `tondo fix` applies what the checker proposes |
| `go doc fmt.Println` | `tondo api std.console` | with an example that compiles |
| `go mod init`, `go get` | `tondo new`, `tondo add` | a `tondo.toml`, and a lock file |
| `dlv debug` | `tondo debug` | also backwards, over a recording |
| `go tool pprof` | `tondo profile` | writes pprof, so the same viewers read it |
| `gopls` | `tondo lsp` | in the same binary, as the MCP server is |
| `cgo` | `extern fn`, `tondo bind` | declarations written from a C header |
| `go generate` | `derive` and generators | part of the build, with nothing generated to commit |

## Habits to leave behind

| In Go | In Tondo |
| --- | --- |
| `x := 1`, then `x = 2` | `var x = 1`; a `let` cannot change |
| `if err != nil { return err }` | `?` |
| `errors.Is(err, ErrNotFound)` | `match` on the error's cases |
| `value, ok := m[key]` | `m.get(key)`, an option |
| `var p *User`, `if p == nil` | `User?`, and `match` |
| `for i := 0; i < n; i++` | `for i in 0..n` |
| `for cond { }`, `for { }` | the same |
| `switch` | `match`, which covers every case |
| `go f()` | `spawn f()`, waited for by its block |
| `wg.Add`, `wg.Done`, `wg.Wait` | a `Group`, or the end of the block |
| `ctx context.Context` as first parameter | `within` and `context` |
| `defer mu.Unlock()` | `mutex.update((value) { ... })` |
| `fmt.Sprintf("%s is %d", name, age)` | `"{name} is {age}"` |
| `"a" + "b"` | `"{a}{b}"`, or `a.concat(b)` |
| `len(x)` | `x.length()` |
| `Exported`, `unexported` | `pub fn name`, `fn name` |
| `x := x` to shadow | another name: there is no shadowing |
| `interface{}`, `any` | a generic, an enum, or `A \| B` |
| `init()` | `main`, and context fields computed when first read |

`tondo guide pitfalls` has these and the error each gives.

## Where Go is the better choice today

- **It is finished, and Tondo is not.** Go has more than a decade of
  compatibility behind it. Tondo is in early development and can still
  change.
- **Its ecosystem.** Whatever the service talks to has a Go client. Tondo's
  standard library is wide, and what is outside it is called through C.
- **Platforms.** Go runs on Windows and on many architectures. Tondo builds
  for Linux and macOS, on x86-64 and arm64.
- **People and tools.** Editors, linters, cloud platforms and colleagues
  already know Go.

What Tondo offers against that is in the programs above: failures that
cannot be forgotten, values that are not shared by accident, cases the
compiler counts, tasks with an owner, and one tool for all of it. Its goal
for speed is Go's own, measured side by side in
[`bench/`](../../bench/README.md).

Next: [Tondo for someone who writes Rust](rust.md).
