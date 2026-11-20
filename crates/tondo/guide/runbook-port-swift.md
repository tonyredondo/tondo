# Runbook: porting from Swift

Swift and Tondo share more than any other pair here: `let` and `var`,
values that copy, optionals with `?.` and `??`, enums with data, protocols,
arithmetic that stops on overflow. A port is mostly a change of words. What
changes shape is what Swift marks at every call, `try` and `await`, and the
classes that are shared by reference. `runbook-port` has the method every
port follows; this is what each thing of Swift becomes.

## What each thing becomes

| Swift | Tondo |
| --- | --- |
| `let x = 5`, `var x = 5` | the same |
| `Int`, `Double`, `Bool`, `String`, `Character`, `UInt8` | `Int`, `Float`, `Bool`, `String`, `Char`, `Byte` |
| `func total(of line: Line) -> Int` | `fn total(line: Line): Int`; a call may name it: `total(line: x)` |
| `struct Line { let name: String }` | `type Line = { name: String }`, a value too |
| `Line(name: "tea", cents: 250)`, an `init` | `Line { name: "tea", cents: 250 }`; a function when it computes |
| a method, a computed property, `mutating func` | `fn Line.total(self): Int`; `mut self` to change a field |
| `extension Line { }` | nothing: methods are always declared outside the type |
| `protocol Priced`, `struct Line: Priced` | `trait Priced`, `impl Priced for Line` |
| `enum Kind { case food, drink(Int) }`, `switch` | `enum Kind { Food, Drink(Int) }`, `match` |
| `Int?`, `nil`, `x?.y`, `x ?? d`, `x!` | `Int?`, `none`, `x?.y`, `x ?? d`, `x.expect("why")` |
| `if let v = x { }` | `match x { some(v) => ..., none => ... }` |
| `guard let v = x else { return }` | `let v = x ?? return` |
| `func f() throws -> T`, `throw`, `try f()` | `fn f(): T ! E`, `fail`, `f()?` |
| `do { } catch { }` | `match f() { ok(v) => ..., err(e) => ... }` |
| `try? f()`, `try! f()` | `f().ok()`, `f().expect("why")` |
| `Result<T, E>` | `T ! E` |
| `[Int]`, `[String: Int]`, `Set<Int>` | `Array[Int]`, `Map[String, Int]`, `Set[Int]` |
| `[1, 2]`, `["a": 1]` | the same |
| `xs.count`, `xs.append(x)`, `xs.isEmpty`, `xs.first` | `xs.length()`, `xs.push(x)`, `xs.isEmpty()`, `xs.first()` |
| `map`, `filter`, `reduce`, `contains`, `sorted(by:)`, `max(by:)` | `map`, `filter`, `reduce`, `contains`, `sortedBy`, `maxBy` |
| `{ $0 * 2 }`, `{ x in x * 2 }` | `(x) { x * 2 }` |
| `for x in xs`, `for (i, x) in xs.enumerated()`, `for i in 0..<n` | `for x in xs`, `for (i, x) in xs.enumerate()`, `for i in 0..n` |
| `"\(name): \(price)"`, `String(format: "%.2f", price)` | `"{name}: {price:.2}"` |
| `async`, `await`, `Task { }`, `withTaskGroup` | a plain function and a plain call; `spawn`, `wait()` on an array of tasks |
| an `actor` | `sync.Mutex(value)`, or a task that owns the value behind a channel |
| `defer` | `defer` |
| `guard cond else { return }` | `if not cond { return }` |
| `<T: Comparable>`, `where` | `[T: Ordered]` |
| `Codable`, `CodingKeys` | nothing, and `@name("user_id")` on the field |
| `func testX() { XCTAssertEqual(a, b) }` | `test x { assert(a == b) }`, in `orders_test.to` |
| `print(x)`, `readLine()` | `console.println("{x}")`, `console.readLine()` |

## The story: a till, ported

The original reads sales, a line each, prices them in a task group, and
prints the bill. Its `import` is left out here.

```swift
struct Line {
    let name: String
    let quantity: Int
    let cents: Int

    var total: Int { quantity * cents }
}

struct ParseError: Error, CustomStringConvertible {
    let description: String
}

func number(_ part: Substring) throws -> Int {
    guard let value = Int(part) else {
        throw ParseError(description: "`\(part)` is no number")
    }
    return value
}

/// A line as it is written, tea,2,250; throws when it is not one.
func parse(_ text: String) throws -> Line {
    let parts = text.split(separator: ",", omittingEmptySubsequences: false)
    guard parts.count == 3 else {
        throw ParseError(description: "it has \(parts.count) parts, not 3")
    }
    return Line(name: String(parts[0]), quantity: try number(parts[1]), cents: try number(parts[2]))
}

func money(_ cents: Int) -> String {
    String(format: "%d.%02d", cents / 100, cents % 100)
}

var lines: [Line] = []
while let text = readLine() {
    do {
        lines.append(try parse(text))
    } catch {
        print("skipped `\(text)`: \(error)")
    }
}
let bought = lines
let totals = await withTaskGroup(of: (Int, Int).self) { group in
    for (index, line) in bought.enumerated() {
        group.addTask { (index, line.total) }
    }
    var totals = [Int](repeating: 0, count: bought.count)
    for await (index, total) in group {
        totals[index] = total
    }
    return totals
}
for (line, total) in zip(bought, totals) {
    print("\(line.name): \(money(total))")
}
print("total \(money(totals.reduce(0, +)))")
if let dearest = bought.max(by: { $0.total < $1.total }) {
    print("dearest: \(dearest.name)")
}
```

What it prints for four lines, one with a letter where a number goes and one
too short, is what the port must print:

```text
$ printf 'tea,2,250\ncake,x,400\nwater,1,100\nbread\n' | ./orders
skipped `cake,x,400`: `x` is no number
skipped `bread`: it has 1 parts, not 3
tea: 5.00
water: 1.00
total 6.00
dearest: tea
```

## The first try, with Swift's words

Swift's forms are close, and a port tends to keep them: `struct`, `protocol`
and `func`, a function that `throws`, `guard let`, `if let`, and `\(value)` in
a text. `check` knows each and says Tondo's.

```tondo error E0004 file=orders.to
import std.console

struct Line {
    let name: String
    let quantity: Int
    let cents: Int
}

protocol Priced {
    func total() -> Int
}

func money(cents: Int) -> String {
    "{cents / 100}.{cents % 100:02}"
}

fn number(part: String) throws -> Int {
    Int.parse(part)
}

fn quantity(part: Int?): Int {
    guard let value = part else { return 0 }
    value
}

fn main() {
    let parts = "tea,2,250".split(",")
    console.println("it has \(parts.length()) parts")
    let dearest: Line? = none
    if let line = dearest {
        console.println(line.name)
    }
    console.println(money(cents: 500))
}
```

```mcp
check {"path": "orders.to"}
=> a record is `type Line = { field: Type }`
=> an interface is a trait: `trait Priced { fn method(self): Type }`
=> a function is `fn money(value: Type): Result { ... }`
=> a function that can fail returns `T ! E`, as `fn f(): Int ! ParseError`
=> `let v = x ?? return` gives up where `x` has no value, with no `guard`
=> a string interpolates with braces, `"total {x}"`
=> there is no `if let`: match the value
```

With the forms right, three names are left.

```tondo error E1001 file=orders.to
import std.console

type Line = { name: String, quantity: Int, cents: Int }

fn dearest(lines: Array[Line]): Line? {
    if lines.count == 0 {
        return nil
    }
    lines[0]
}

fn main() {
    print(dearest([]))
}
```

```mcp
check {"path": "orders.to"}
=> has no field `count`; use `length()`
=> unknown name `nil`; Tondo writes `none`
=> unknown name `print`; `console.println(text)` writes a line
```

What Foundation does, ask for by what it does.

```mcp
api {"search": "parse integer"}
=> Int.parse
```

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

- **`throws`, every `try`, and `do` with its `catch`.** `parse` says in its
  result that it can fail and with what, which Swift's `throws` does not, and
  `match` takes both cases.
- **The task group, the pairs of index and total, and the array filled by
  index.** `wait()` on an array of tasks gives their results in the
  array's order.
- **`await`.** Nothing marks a call that waits: any function can.
- **The `Substring` made a `String`.** A part of a text is a `String`.
- **The copy named `bought`,** which the task group needed to capture
  something that does not change. A task is given the value it needs.

## What does not carry over

- **Classes shared by reference.** A record is a value, as a `struct` is. An
  object that several parts of the program change is a `sync.Mutex(value)`
  when tasks share it, or is passed as a `var` parameter when one function
  changes it for its caller. There are no reference cycles to break, so no
  `weak` and no `unowned`.
- **Inheritance.** A contract is a trait, as a protocol is; a closed family
  of cases is an `enum`.
- **Errors without a type.** `throws` says that a function can fail, and
  `T ! E` also says with what: a caller's `match` covers every case of an
  enum of errors, or `E1204` names the one missing.
- **`await` and actor isolation.** Tasks run at once on every core. What two
  of them change goes in a `sync` cell, and `test` tries their orders by
  itself: `runbook-flaky`.
- **Argument labels as part of a name.** `total(of:)` and `total(for:)` are
  two functions in Swift and would be one here: name them apart.
- **The frameworks.** There is no UIKit or SwiftUI. The standard library has
  HTTP, JSON, SQL, templates and a command line: `tondo guide stdlib`.

The original's tests become the port's first (`runbook-port`).
