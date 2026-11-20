# Runbook: porting from Python

A Python program decides at run time what Tondo decides when it compiles:
what type each value has, whether it can be missing, what a call can fail
with. So the first work of a port is to name the types, and most of the rest
follows from them. The code gets no longer, and what was a `TypeError` when
the program ran is an error of `check`. `runbook-port` has the method every
port follows; this is what each thing of Python becomes.

## What each thing becomes

| Python | Tondo |
| --- | --- |
| `x = 5` | `let x = 5`, and `var x = 5` for what changes |
| `def f(a, b=1):` and the indentation | `fn f(a: Int, b: Int = 1): Int { ... }`: types in the signature, braces |
| `@dataclass class Line:` | `type Line = { name: String, quantity: Int }` |
| a class with methods and `self.x = ...` | a record, `fn Line.total(self)`, and `mut self` to change a field |
| a `dict` used as an object | a record; a `Map[String, V]` when the keys are data |
| a value that is one type or another | an `enum` of the cases; `json.Value` for JSON read without a type |
| `None`, `Optional[T]`, `x is None` | `none`, `T?`, `x == none` or a `match` |
| `d.get(k, default)`, `k in d` | `d.get(k) ?? default`, `d.contains(k)` |
| `raise ValueError(...)` | `fail error`, in a function that returns `T ! E` |
| `try:` and `except ValueError as e:` | `match f() { ok(v) => ..., err(e) => ... }`; `?` to pass it on |
| `finally:`, `with open(path) as f:` | `defer` |
| `[f(x) for x in xs if g(x)]` | `xs.filter(g).map(f)`; with more to do in a turn, `let ys = for x in xs { ... }`, whose turn ends with its value and leaves an element out with `continue` |
| `{k(x): x for x in xs}`, `set(xs)` | `xs.keyedBy(k)`, `xs.toSet()` |
| `[1, 2]`, `{"a": 1}`, `{1, 2}`, `(1, "a")` | `[1, 2]`, `["a": 1]`, `Set[1, 2]`, `(1, "a")` |
| `len(x)`, `xs[-1]`, `xs[1:3]`, `x in xs` | `x.length()`, `xs[-1]`, `xs[1:3]`, `xs.contains(x)` |
| `for i, x in enumerate(xs)`, `for k, v in d.items()` | `for (i, x) in xs.enumerate()`, `for (k, v) in d` |
| `range(n)`, `range(a, b)` | `0..n`, `a..b` |
| `sorted(xs, key=f)`, `max(xs, key=f)`, `sum(xs)`, `zip(xs, ys)` | `xs.sortedBy(f)`, `xs.maxBy(f)`, `xs.sum()`, `xs.zip(ys)` |
| `", ".join(xs)`, `s.split()`, `s.strip()`, `s.upper()` | `xs.join(", ")`, `s.words()`, `s.trim()`, `s.toUpper()` |
| `f"{name}: {price:.2f}"` | `"{name}: {price:.2}"` |
| `lambda x: x * 2` | `(x) { x * 2 }` |
| `a if cond else b` | `if cond { a } else { b }` |
| `if xs:`, `if not name:` | `if not xs.isEmpty()`: a condition is a `Bool` |
| `and`, `or`, `not` | the same |
| `int("3")`, `str(n)`, `float(n)` | `Int.parse("3")`, `"{n}"`, `Float(n)` |
| `isinstance(x, T)`, a method any object may have | a `match` on an `enum`; a `trait` for what the types share |
| a generator with `yield` | an iterator: `xs.iter()` and its adapters, or a type with `next` |
| `async def`, `await`, `asyncio.gather` | a plain function, a plain call, `wait()` on an array of tasks, or on a tuple of them when their types differ |
| `threading`, `ThreadPoolExecutor` | `spawn`, an array of tasks: they run on every core |
| `queue.Queue`, `threading.Lock` | `channel[T]()`, `sync.Mutex(value)` |
| `import os`, `from json import loads` | `import std.fs`, `import std.json`: a name keeps its module, `json.decode` |
| `if __name__ == "__main__":` | `fn main()`, or statements at the top of a script |
| `def test_x(): assert ...` | `test x { assert(...) }`, in `x_test.to` |
| `*args`, `**kwargs` | `values: ...Int`; named arguments with defaults |
| a decorator | a function that takes a function; `derive` for code from a type |
| `print(x)`, `input()` | `console.println("{x}")`, `console.readLine()` |

## The story: a till, ported

The original reads sales, a line each, prices them in a pool of threads, and
prints the bill. Its imports are left out here.

```python
@dataclass
class Line:
    name: str
    quantity: int
    cents: int

    def total(self):
        return self.quantity * self.cents


def number(part):
    try:
        return int(part)
    except ValueError:
        raise ValueError(f"`{part}` is no number") from None


def parse(text):
    """A line as it is written, tea,2,250; raises ValueError when it is not one."""
    parts = text.split(",")
    if len(parts) != 3:
        raise ValueError(f"it has {len(parts)} parts, not 3")
    return Line(parts[0], number(parts[1]), number(parts[2]))


def money(cents):
    return f"{cents // 100}.{cents % 100:02}"


lines = []
for text in sys.stdin.read().splitlines():
    try:
        lines.append(parse(text))
    except ValueError as why:
        print(f"skipped `{text}`: {why}")
with ThreadPoolExecutor() as pool:
    totals = list(pool.map(Line.total, lines))
for line, total in zip(lines, totals):
    print(f"{line.name}: {money(total)}")
print(f"total {money(sum(totals))}")
dearest = max(lines, key=Line.total, default=None)
if dearest is not None:
    print(f"dearest: {dearest.name}")
```

What it prints for four lines, one with a letter where a number goes and one
too short, is what the port must print:

```text
$ printf 'tea,2,250\ncake,x,400\nwater,1,100\nbread\n' | python3 orders.py
skipped `cake,x,400`: `x` is no number
skipped `bread`: it has 1 parts, not 3
tea: 5.00
water: 1.00
total 6.00
dearest: tea
```

## The first try, with Python's words

The colon and the indentation go first: a function has its types and its
braces.

```tondo error E0004 file=orders.to
def parse(text):
    parts = text.split(",")
    return parts
```

```mcp
check {"path": "orders.to"}
=> a function is `fn parse(value: Type): Result { ... }`
```

With the form right, Python's built-ins are the next round. `check` knows
them and says what Tondo writes.

```tondo error E1001 file=orders.to
import std.console

type Line = { name: String, quantity: Int, cents: Int }

fn parse(text: String): Line? {
    let parts = text.split(",")
    if len(parts) != 3 {
        return None
    }
    Line { name: parts[0].upper(), quantity: 2, cents: 250 }
}

fn main() {
    for index in range(2) {
        print(parse("tea,2,250"))
    }
}
```

```mcp
check {"path": "orders.to"}
=> unknown name `len`; use `value.length()`
=> unknown name `None`; Tondo writes `none`
=> has no method `upper`: use `toUpper()`
=> unknown name `range`; a range is `0..10`
=> unknown name `print`; `console.println(text)` writes a line
```

What turns a text into a number is not a conversion here, since it can fail:
ask for what does the job.

```mcp
api {"search": "parse integer"}
=> Int.parse
```

## The port

The types come first: a `Line` is a record, and `parse` gives one or fails
with the reason, which its signature says.

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
    let lines = for {
        let text = console.readLine() ?? break
        match parse(text) {
            ok(line) => line
            err(why) => {
                console.println("skipped `{text}`: {why}")
                continue
            }
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

- **The exception and its two `try`.** `parse` says in its signature that it
  can fail, and `match` takes both cases; `??` says what to do when
  `Int.parse` fails.
- **The pool.** `spawn` starts each pricing as a task, on every core, and
  `wait()` on the array of them gives their results in order.
- **`default=None` and `is not None`.** `maxBy` gives a `Line?`, and `match`
  takes the case with none.

## What does not carry over

- **A value of whatever type.** Each binding has one. Where the original
  passes a `dict` around, write the record it stands for; where a value is
  one thing or another, an `enum`. This is the part of the port that takes
  thought, and the part that finds the original's bugs.
- **Truthiness.** `if xs:` is `if not xs.isEmpty()`, and `if x:` on a number
  is `if x != 0`.
- **Lists and dicts shared by reference.** In Python `b = a` and then
  `b.append(1)` changes `a` too. In Tondo a collection is a value, and `a`
  stays as it was: a function changes its caller's list only through a `var`
  parameter. Code that counts on the sharing must say it.
- **Integers of any size, and division.** An `Int` has 64 bits and panics
  when a result does not fit: `math.BigInt` holds the others. `/` on two
  `Int` gives an `Int`, cut towards zero, and `%` takes the sign of what is
  divided: `-7 / 2` is `-3` and `-7 % 2` is `-1`, where Python's `//` and `%`
  give `-4` and `1`.
- **`getattr`, `eval`, patching a class at run time.** `std.reflect` reads the
  fields of a type that is known; code from a type is `derive`.
- **A package for each thing.** The standard library has HTTP, JSON, YAML,
  CSV, SQL, templates, regular expressions, dates and a command line:
  `tondo guide stdlib`. A C library is called as it is: `tondo guide ffi`.

The original's tests become the port's first (`runbook-port`), which matters
more here than from any other language: they are the only place where the
types Python never wrote down can be read.
