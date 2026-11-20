# Runbook: porting from Rust

Rust's types carry over almost as they are: records, enums with data, traits,
`Option` and `Result`, `match`. What does not is what Rust asks its author to
decide about memory: references, lifetimes, `clone`, `Rc`, `Box`. A port
deletes them and keeps the rest. `runbook-port` has the method every port
follows; this is what each thing of Rust becomes.

## What each thing becomes

| Rust | Tondo |
| --- | --- |
| `let x = 5;`, `let mut x = 0;` | `let x = 5`, `var x = 0`, and no `;` |
| `struct Line { name: String }` | `type Line = { name: String }` |
| `impl Line { fn total(&self) -> i64 }` | `fn Line.total(self): Int`, outside the type |
| `&mut self` | `mut self` |
| `&T`, `&mut T`, `'a` | the value; `var` on a parameter the function changes; no lifetimes |
| `.clone()`, `Rc<T>`, `Arc<T>` | nothing: a value is shared until one side changes it |
| `Box<T>` in a recursive type | the type itself: `enum Tree { Leaf(Int), Node(Tree, Tree) }` |
| `Vec<T>`, `HashMap<K, V>`, `HashSet<T>`, `VecDeque<T>` | `Array[T]`, `Map[K, V]`, `Set[T]`, `Deque[T]` |
| `Option<T>`, `Some(x)`, `None` | `T?`, `some(x)`, `none` |
| `Result<T, E>`, `Ok(x)`, `Err(e)` | `T ! E`, the value itself, `fail e` |
| `?`, `.map_err(f)`, `.ok_or(e)` | `?`, `.mapErr(f)`, `.okOr(e)` |
| `anyhow::Result`, `.context("...")` | `T ! Failure`, `.context("...")` |
| `.unwrap()`, `.expect("...")`, `.unwrap_or(d)` | `.expect("...")`, `?? d` |
| `if let Some(x) = v { }`, `let Some(x) = v else { return }` | `match v { ... }`, `let x = v ?? return` |
| `while let Some(x) = rx.recv()` | `for x in receiver` |
| `enum Shape { Circle(f64) }`, `Shape::Circle(r)` | `enum Shape { Circle(Float) }`, `Shape.Circle(r)` |
| `trait Shape`, `impl Shape for Circle` | the same |
| `fn largest<T: Ord>(xs: &[T])`, `where` clauses | `fn largest[T: Ordered](xs: Array[T])` |
| `Box<dyn Shape>`, `&dyn Shape` | an `enum` of the implementations, or a closure |
| `\|x\| x * 2`, `move \|\| ...` | `(x) { x * 2 }` |
| `xs.iter().map(f).filter(g).collect::<Vec<_>>()` | `xs.map(f).filter(g)`; from `xs.iter()` for a lazy chain |
| `for (i, x) in xs.iter().enumerate()` | `for (i, x) in xs.enumerate()` |
| `i64`, `u8`, `usize`, `f64`, `&str` | `Int`, `Byte`, `Int`, `Float`, `String` |
| `x as f64`, `"3".parse::<i64>()` | `Float(x)`, `Int.parse("3")` |
| `format!("{}: {:02}", a, b)`, `println!` | `"{a}: {b:02}"`, `console.println(...)` |
| `thread::spawn`, `thread::scope`, `tokio::spawn`, `.await` | `spawn f()`, `wait()` on an array of tasks; there is no `async` |
| `mpsc::channel()`, `select!` | `channel[T]()`, `select` |
| `Mutex<T>`, `RwLock<T>`, `AtomicUsize` | `sync.Mutex(value)`, `sync.RwLock(value)`, `sync.Atomic(0)` |
| `impl Drop` | `defer` where the value is made |
| `panic!`, `catch_unwind` | `panic`, `recover { ... }` |
| `#[derive(Debug, Clone, PartialEq)]` | nothing: every value prints, copies and compares |
| `#[derive(Serialize)]`, `#[serde(rename = "id")]` | nothing, and `@name("id")` on the field |
| `mod orders;`, `use crate::orders::Line` | the directory `orders`, `import app.orders` |
| `pub(crate)` | a module under a directory `internal` |
| `#[test] fn x()`, `mod tests` | `test x { ... }` in `orders_test.to` |
| `#[cfg(target_os = "linux")]` | a file `name_linux.to` |
| `unsafe`, `extern "C"` | `unsafe`, `extern fn`: `tondo guide ffi` |

## The story: a till, ported

The original reads sales, a line each, prices them in scoped threads, and
prints the bill. Its `use` lines are left out here.

```rust
struct Line {
    name: String,
    quantity: i64,
    cents: i64,
}

impl Line {
    fn total(&self) -> i64 {
        self.quantity * self.cents
    }
}

/// A line as it is written, `tea,2,250`, or why it is not one.
fn parse(text: &str) -> Result<Line, String> {
    let parts: Vec<&str> = text.split(',').collect();
    if parts.len() != 3 {
        return Err(format!("it has {} parts, not 3", parts.len()));
    }
    let number = |part: &str| part.parse::<i64>().map_err(|_| format!("`{part}` is no number"));
    Ok(Line { name: parts[0].to_string(), quantity: number(parts[1])?, cents: number(parts[2])? })
}

fn money(cents: i64) -> String {
    format!("{}.{:02}", cents / 100, cents % 100)
}

fn main() {
    let mut lines = Vec::new();
    for text in io::stdin().lock().lines().map_while(Result::ok) {
        match parse(&text) {
            Ok(line) => lines.push(line),
            Err(why) => println!("skipped `{text}`: {why}"),
        }
    }
    let totals: Vec<i64> = thread::scope(|scope| {
        let jobs: Vec<_> = lines.iter().map(|line| scope.spawn(move || line.total())).collect();
        jobs.into_iter().map(|job| job.join().unwrap()).collect()
    });
    for (line, total) in lines.iter().zip(&totals) {
        println!("{}: {}", line.name, money(*total));
    }
    println!("total {}", money(totals.iter().sum()));
    if let Some(dearest) = lines.iter().max_by_key(|line| line.total()) {
        println!("dearest: {}", dearest.name);
    }
}
```

What it prints for four lines, one with a letter where a number goes and one
too short, is what the port must print:

```text
$ printf 'tea,2,250\ncake,x,400\nwater,1,100\nbread\n' | cargo run -q
skipped `cake,x,400`: `x` is no number
skipped `bread`: it has 1 parts, not 3
tea: 5.00
water: 1.00
total 6.00
dearest: tea
```

## The first try, with Rust's forms

Rust's forms are close enough that a port tends to keep them. `check` knows
each and says Tondo's.

```tondo error E0004 file=orders.to
import std.console

struct Line {
    name: String,
    quantity: Int,
    cents: Int,
}

impl Line {
    fn total(self): Int {
        self.quantity * self.cents
    }
}

fn money(cents: Int) -> String {
    "{cents / 100}.{cents % 100:02}"
}

fn main() {
    let totals = [250, 100].map(|cents| money(cents))
    console.println("{totals}")
}
```

```mcp
check {"path": "orders.to"}
=> a record is `type Line = { field: Type }`
=> a type's own methods are declared apart, as `fn Type.method(self): Result { ... }`
=> a result type follows `:`
=> a closure is `(x) { x * 2 }`
```

With the forms right, the names of Rust's library are the next round: each
has its answer too.

```tondo error E1001 file=orders.to
import std.console

type Line = { name: String, quantity: Int, cents: Int }

fn parse(text: String): Line ! String {
    let parts = text.split(",")
    let quantity = Int.parse(parts[1]).unwrap()
    let name = parts[0].to_string()
    Ok(Line { name: name, quantity: quantity, cents: parts.len() })
}

fn main() {
    console.println("{parse("tea,2,250").is_ok()}")
}
```

```mcp
check {"path": "orders.to"}
=> has no method `unwrap`: use `expect(message)`
=> has no method `to_string`: interpolate it
=> unknown name `Ok`; Tondo writes `ok(...)`
=> has no method `len`: use `length()`
=> has no method `is_ok`: use `isOk()`
```

A function that returns `T ! E` gives its value as its last line, with no
`Ok` around it, and fails with `fail`. `ok(...)` and `err(...)` are what a
`match` on a result takes apart.

## The port

```tondo file=orders.to
import std.console

type Line = { name: String, quantity: Int, cents: Int }

fn Line.total(self): Int {
    self.quantity * self.cents
}

/// A line as it is written, `tea,2,250`, or why it is not one.
fn parse(text: String): Line ! String {
    let parts = text.split(",")
    if parts.length() != 3 {
        fail "it has {parts.length()} parts, not 3"
    }
    let quantity = Int.parse(parts[1]) ?? fail "`{parts[1]}` is no number"
    let cents = Int.parse(parts[2]) ?? fail "`{parts[2]}` is no number"
    Line { name: parts[0], quantity, cents }
}

fn money(cents: Int): String {
    "{cents / 100}.{cents % 100:02}"
}

fn main() {
    var lines: Array[Line] = []
    for {
        let text = console.readLine() ?? break
        match parse(text) {
            ok(line) => lines.push(line)
            err(why) => console.println("skipped `{text}`: {why}")
        }
    }
    let totals = lines.map((line) { spawn line.total() }).wait()
    for (index, line) in lines.enumerate() {
        console.println("{line.name}: {money(totals[index])}")
    }
    console.println("total {money(totals.sum())}")
    match lines.maxBy((line) { line.total() }) {
        some(dearest) => console.println("dearest: {dearest.name}")
        none => {}
    }
}
```

The same input, and the same six lines:

```mcp
run {"path": "orders.to", "stdin": "tea,2,250\ncake,x,400\nwater,1,100\nbread\n"}
=> skipped `cake,x,400`: `x` is no number
=> skipped `bread`: it has 1 parts, not 3
=> tea: 5.00
=> water: 1.00
=> total 6.00
=> dearest: tea
```

What went, part by part of the original:

- **Every `&`, `.to_string()`, `.iter()` and `*total`.** A function takes the
  value and gives one back; who owns it is not the author's to say.
- **`thread::scope`, the handles and their `join().unwrap()`.** `spawn`
  starts a task, which is far cheaper than a thread, and `wait()` on the
  array of them gives their results in order.
- **The closure with `map_err`.** `??` takes what to do when `Int.parse`
  fails, which may be to fail with the function's own error.
- **`collect::<Vec<_>>()`.** An array's `map` gives an array.

## What does not carry over

- **Ownership as something to write.** There is no borrow to fight and none
  to rely on: a value given to a function is still the caller's, unchanged,
  unless the parameter says `var`. Code that used a borrow to forbid a use
  needs another way to say it, most often a type.
- **Graphs of `Rc<RefCell<T>>`.** Keep the nodes in an array or a map and
  refer to them by index or key; a `sync` cell holds what tasks share.
- **`dyn Trait`.** A closed set of implementations is an `enum`, which
  `match` checks; one behaviour that varies is a closure in a field.
- **Macros.** A function does most of what `macro_rules!` did; code written
  from a type is `derive`, and from a file a generator: `tondo guide meta`.
- **Wrapping arithmetic in release builds.** An `Int` that overflows panics
  in every build. Where the original counts on wrapping, as a hash does, say
  so: `wrappingAdd`, `wrappingMul`; `checkedAdd` gives an option.
- **Control of each allocation.** The compiler decides what stays off the
  heap, and says what it decided: `runbook-allocations`.

The original's tests become the port's first (`runbook-port`), its `criterion`
benchmarks tests that call `testing.bench`, and `cargo fuzz` targets tests
with `testing.fuzz`: `runbook-testing`.
