# Runbook: porting from TypeScript or JavaScript

TypeScript's types are checked and then erased, so a value may not be what
its type says when the program runs. Tondo's types are what runs. A port
keeps the shapes the types describe and drops what JavaScript needs around
them: `undefined` and `null`, the exception nobody declared, the `Promise`
and its `await`. JavaScript without types is ported as Python is: name the
types first (`runbook-port-python`). `runbook-port` has the method every port
follows; this is what each thing becomes.

## What each thing becomes

| TypeScript | Tondo |
| --- | --- |
| `const x = 5`, `let x = 5` | `let x = 5`, `var x = 5` |
| `function f(a: number): string`, `const f = (a) => ...` | `fn f(a: Int): String`; a closure is `(a) { ... }` |
| `number` | `Int` or `Float`: say which; `Int(x)?` and `Float(n)` convert |
| `string`, `boolean`, `T[]`, `Map<K, V>`, `Set<T>` | `String`, `Bool`, `Array[T]`, `Map[K, V]`, `Set[T]` |
| `interface Line { name: string }`, an object literal | `type Line = { name: String }`, `Line { name: "tea" }` |
| a `class` with methods | a record, and `fn Line.total(self)` outside it |
| `Record<string, V>`, an object used as a map | `Map[String, V]`, written `["a": 1]` |
| `type Shape = Circle \| Square`, with a `kind` field | `enum Shape { Circle(Float), Square(Float) }` and `match` |
| `T \| undefined`, `T \| null`, `x?: T` | `T?`, with `none` |
| `x?.y`, `x ?? y`, `x!` | `x?.y`, `x ?? y`, `x.expect("why")` |
| `===`, `!==` | `==`, `!=`: there is one equality, and it compares values |
| `throw new Error(...)`, `try { } catch (e) { }` | `fail error` in a `T ! E`; `match`, `?`, `??` |
| `async function f(): Promise<T>`, `await f()` | `fn f(): T`, `f()`: any function can wait |
| `Promise.all(xs.map(f))` | a `Group`, with `spawn f(x)` for each |
| a callback, an event emitter | a closure; a `channel[T]()` for a stream of events |
| `xs.map`, `filter`, `reduce`, `some`, `every`, `find`, `includes` | `map`, `filter`, `reduce`, `any`, `all`, `find`, `contains` |
| `xs.length`, `xs.push(x)`, `xs.forEach(f)` | `xs.length()`, `xs.push(x)` on a `var`, `for x in xs` |
| `for (const x of xs)`, `for (const [k, v] of map)` | `for x in xs`, `for (k, v) in map` |
| `` `${name}: ${price.toFixed(2)}` `` | `"{name}: {price:.2}"` |
| `cond ? a : b` | `if cond { a } else { b }` |
| `if (xs.length)`, `if (name)` | `if not xs.isEmpty()`: a condition is a `Bool` |
| `Number(s)`, `parseInt(s)`, `String(n)` | `Int.parse(s)`, which can fail; `"{n}"` |
| `JSON.parse(text) as Line`, `JSON.stringify(x)` | `json.decode[Line](text)`, which checks it; `json.encode(x)` |
| `import { f } from "./orders"`, `export` | `import app.orders`, then `orders.f`; `pub` |
| `describe`, `it`, `expect(x).toBe(y)` | `test name { assert(x == y) }`, in `orders_test.to` |
| `console.log(x)` | `console.println("{x}")` |
| `fetch`, `express`, `process.argv` | `std.http`, `env.args()`: `tondo guide stdlib` |

## The story: a till, ported

The original reads sales, a line each, prices them as promises, and prints
the bill. Its import is left out here.

```typescript
interface Line {
  name: string;
  quantity: number;
  cents: number;
}

const total = (line: Line): number => line.quantity * line.cents;

function number(part: string): number {
  if (!/^-?\d+$/.test(part)) {
    throw new Error(`\`${part}\` is no number`);
  }
  return Number(part);
}

/** A line as it is written, tea,2,250; throws when it is not one. */
function parse(text: string): Line {
  const parts = text.split(",");
  if (parts.length !== 3) {
    throw new Error(`it has ${parts.length} parts, not 3`);
  }
  return { name: parts[0], quantity: number(parts[1]), cents: number(parts[2]) };
}

const money = (cents: number): string =>
  `${Math.floor(cents / 100)}.${String(cents % 100).padStart(2, "0")}`;

async function price(line: Line): Promise<number> {
  return total(line);
}

const lines: Line[] = [];
for await (const text of createInterface({ input: process.stdin })) {
  try {
    lines.push(parse(text));
  } catch (why) {
    console.log(`skipped \`${text}\`: ${(why as Error).message}`);
  }
}
const totals = await Promise.all(lines.map(price));
lines.forEach((line, index) => console.log(`${line.name}: ${money(totals[index])}`));
console.log(`total ${money(totals.reduce((sum, each) => sum + each, 0))}`);
let dearest: Line | undefined;
for (const line of lines) {
  if (dearest === undefined || total(line) > total(dearest)) {
    dearest = line;
  }
}
if (dearest !== undefined) {
  console.log(`dearest: ${dearest.name}`);
}
```

What it prints for four lines, one with a letter where a number goes and one
too short, is what the port must print:

```text
$ printf 'tea,2,250\ncake,x,400\nwater,1,100\nbread\n' | node orders.ts
skipped `cake,x,400`: `x` is no number
skipped `bread`: it has 1 parts, not 3
tea: 5.00
water: 1.00
total 6.00
dearest: tea
```

## The first try, with TypeScript's forms

Written as TypeScript writes, the port has `function`, `async`, `===`, a
template and an arrow. `check` answers each with Tondo's.

```tondo error E0004 file=orders.to
import std.console

type Line = { name: String, quantity: Int, cents: Int }

function total(line: Line): Int {
    line.quantity * line.cents
}

async fn price(line: Line): Int {
    total(line)
}

fn main() {
    let line = Line { name: "tea", quantity: 2, cents: 250 }
    let cents = await price(line)
    if cents === 500 {
        console.println(`total ${cents}`)
    }
    let doubled = [cents].map((each) => each * 2)
    console.println("{doubled}")
}
```

```mcp
check {"path": "orders.to"}
=> a function is `fn total(value: Type): Result { ... }`
=> there is no `async`: any function can wait
=> there is no `await`: call the function
=> Tondo has no `===`
=> strings use double quotes, and `{name}` inside one interpolates
=> a closure is `(x) { x * 2 }`
```

With the forms right, the names are the next round: the types of
TypeScript, `undefined`, and the methods of its arrays.

```tondo error E1001 file=orders.to
import std.console

type Line = { name: String, quantity: number, cents: number }

fn dearest(lines: Array[Line]): Line? {
    var best: Line? = undefined
    lines.forEach((line) {
        best = line
    })
    best
}

fn main() {
    console.println("{dearest([])}")
}
```

```mcp
check {"path": "orders.to"}
=> unknown type `number`: Tondo writes `Int`
=> unknown name `undefined`; Tondo writes `none`
=> has no method `forEach`: use `for value in array { ... }`
```

A `number` that counts is an `Int`, and one that measures a `Float`: the
port decides, where the original never had to. What the library does, ask
for by what it does.

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
    var pricing = Group[Int]()
    for line in lines {
        pricing.add(spawn line.total())
    }
    let totals = pricing.wait()
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

- **The exception and its `catch (why)`,** whose type nobody knew. `parse`
  gives a `Line` or fails with a `String`, and `match` takes both cases.
- **`async`, `await` and `Promise.all`.** `spawn` starts each pricing as a
  task, on every core and not on one thread, and the `Group` gives their
  results in order.
- **`undefined` and the loop that kept the best.** `maxBy` gives a `Line?`,
  which is matched before its name is read.
- **The regular expression before `Number`.** `Int.parse` fails for what is
  no whole number, where `Number("x")` gives `NaN` and goes on.

## What does not carry over

- **A value that is whatever it is at run time.** `any`, `as`, and JSON
  trusted to have a shape: `json.decode[Line](text)` checks the text against
  the type and fails with what is wrong. An object with keys nobody listed is
  a `Map`.
- **`null` and `undefined`.** One `none`, and only in a `T?`.
- **`${name}` left in a string.** A template whose backticks became double
  quotes keeps its dollars: `"hi ${name}"` prints `hi $Ada`, a dollar and
  the value, as a price is written, and nothing warns. Take the `$` out
  when the backticks go. `"hi $name"`, with no braces, does warn (`W1022`).
- **Truthiness and `==` that converts.** A condition is a `Bool`; `0`, `""`
  and an empty array are not false.
- **Objects shared and changed.** An array or a record given to a function
  is the caller's still, unchanged: Tondo's are values. Code that counts on
  the callee changing them takes a `var` parameter.
- **One thread and its loop.** Tasks run at once on every core, so what two
  of them change goes in a `sync.Mutex(value)`; `test` tries their orders by
  itself (`runbook-flaky`).
- **`number` for everything.** An `Int` has 64 bits and panics when a result
  does not fit; a `Float` is what JavaScript's number was. `7 / 2` on `Int` is
  `3`.
- **The packages.** HTTP, JSON, SQL, templates, a command line and more are
  in the standard library: `tondo guide stdlib`.

The original's tests become the port's first (`runbook-port`).
