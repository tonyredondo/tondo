# Runbook: porting from C#

Three habits of C# have no place in Tondo, and a port is mostly taking them
out: exceptions, which become errors in the signature; `async` and `await`,
which become plain calls; and classes that inherit, which become records,
traits and enums. `runbook-port` has the method every port follows; this is
what each thing of C# becomes.

## What each thing becomes

| C# | Tondo |
| --- | --- |
| a `class` or `record` with properties | `type Line = { name: String }`, a value: assigning copies |
| a method of the class | `fn Line.total(self): Int`, declared outside the type |
| a method that changes the object | `fn Line.add(mut self, n: Int)` |
| a property `{ get; set; }` | a field; a method when it computes |
| `public`, `internal`, `private` | `pub`, a module under a directory `internal`, nothing |
| `static` methods, a static class | functions of the module |
| an extension method of `string` | a function that takes one |
| `interface IShape` | `trait Shape`, and `impl Shape for Circle` |
| a base class with `virtual` methods | a `trait` for what they share; an `enum` when the cases are known |
| `string?`, `null`, `x?.Y`, `x ?? y`, `x!` | `String?`, `none`, `x?.y`, `x ?? y`, `x.expect("why")` |
| `throw new XException(...)` | `fail error`, in a function that returns `T ! E` |
| `try { } catch (XException e) { }` | `match f() { ok(v) => ..., err(e) => ... }`; `?` to pass it on |
| `catch (Exception)` around a request | `recover { ... }`, which stops a panic |
| `int.TryParse(s, out var n)` | `Int.parse(s)`, an `Int ! text.TextError` |
| `async Task<T> F()`, `await F()` | `fn f(): T`, `f()`: any function can wait |
| `Task.Run(...)`, `Task.WhenAll(...)` | `spawn f()`, `wait()` on an array of tasks, or on a tuple of them when their types differ |
| a `CancellationToken` passed down | `job.cancel()`, which reaches what the task started |
| `lock (gate) { }`, `Interlocked` | `sync.Mutex(value)`, `sync.Atomic(0)` |
| `Channel<T>` | `channel[T]()` |
| `List<T>`, `Dictionary<K, V>`, `HashSet<T>` | `Array[T]`, `Map[K, V]`, `Set[T]` |
| `d.TryGetValue(k, out var v)` | `d.get(k)`, a `V?` |
| `Where`, `Select`, `SelectMany`, `Aggregate` | `filter`, `map`, `flatMap`, `reduce` |
| `Sum`, `MaxBy`, `OrderBy`, `GroupBy`, `Distinct`, `Any` | `sum`, `maxBy`, `sortedBy`, `groupBy`, `unique`, `any` |
| `foreach (var x in xs)` | `for x in xs` |
| a `switch` expression, patterns | `match`, which must cover every case |
| `enum`, and a class for each case of a union | `enum`, whose variants hold data |
| `using var file = ...`, `IDisposable` | `defer file.close()` |
| `$"{name}: {price:F2}"` | `"{name}: {price:.2}"` |
| `where T : IComparable<T>` | `[T: Ordered]` |
| `[JsonPropertyName("user_id")]` | `@name("user_id")` on the field |
| `namespace Shop.Orders` | the directory `src/orders`, the module `shop.orders` |
| `[Fact] public void X()` | `test x { assert(...) }`, in `orders_test.to` |
| what a container injects | a parameter; a `context` for what is ambient, as the request |

## The story: a till, ported

The original reads sales, a line each, prices them as tasks, and prints the
bill. Its `using` lines are left out here.

```csharp
var lines = new List<Line>();
string? text;
while ((text = Console.ReadLine()) != null)
{
    try
    {
        lines.Add(Line.Parse(text));
    }
    catch (FormatException why)
    {
        Console.WriteLine($"skipped `{text}`: {why.Message}");
    }
}
var totals = await Task.WhenAll(lines.Select(line => Task.Run(() => line.Total())));
foreach (var (line, total) in lines.Zip(totals))
{
    Console.WriteLine($"{line.Name}: {Money(total)}");
}
Console.WriteLine($"total {Money(totals.Sum())}");
var dearest = lines.MaxBy(line => line.Total());
if (dearest != null)
{
    Console.WriteLine($"dearest: {dearest.Name}");
}

static string Money(int cents) => $"{cents / 100}.{cents % 100:D2}";

record Line(string Name, int Quantity, int Cents)
{
    public int Total() => Quantity * Cents;

    /// A line as it is written, tea,2,250; throws when it is not one.
    public static Line Parse(string text)
    {
        var parts = text.Split(',');
        if (parts.Length != 3)
        {
            throw new FormatException($"it has {parts.Length} parts, not 3");
        }
        return new Line(parts[0], Number(parts[1]), Number(parts[2]));
    }

    static int Number(string part) =>
        int.TryParse(part, out var number) ? number : throw new FormatException($"`{part}` is no number");
}
```

What it prints for four lines, one with a letter where a number goes and one
too short, is what the port must print:

```text
$ printf 'tea,2,250\ncake,x,400\nwater,1,100\nbread\n' | dotnet run
skipped `cake,x,400`: `x` is no number
skipped `bread`: it has 1 parts, not 3
tea: 5.00
water: 1.00
total 6.00
dearest: tea
```

## The first try, with C#'s habits

Written as C# thinks, the port throws, catches and awaits. `check` answers
each with what Tondo does instead.

```tondo error E0004 file=orders.to
import std.console

type Line = { name: String, quantity: Int, cents: Int }

fn number(part: String): Int {
    try {
        return Int.parse(part)
    } catch (FormatException e) {
        throw new FormatException("`{part}` is no number")
    }
}

async fn total(line: Line): Int {
    line.quantity * line.cents
}

fn main() {
    let line = Line { name: "tea", quantity: number("2"), cents: 250 }
    let price = await total(line)
    console.println("{price}")
}
```

```mcp
check {"path": "orders.to"}
=> there is no `try`: a function that can fail returns `T ! E`
=> there is no `async`: any function can wait
=> there is no `await`: call the function
```

A function that can fail says so in its result, and its caller cannot forget
it: that is what the `try` was for. What C#'s library does, ask for by what
it does, not by its name there.

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

- **The exception and its `catch`.** `parse` gives a `Line` or fails with the
  reason, and `match` takes both cases where the `try` stood. Nothing can
  throw past a caller that did not expect it.
- **`async`, `await`, `Task`.** `main` is a plain function. `spawn` starts
  each pricing as a task, and `wait()` on the array of them gives their
  results in its order, as `WhenAll` did.
- **The `null` check.** `maxBy` gives a `Line?`, which has to be matched
  before its name is read.
- **The static class.** `money` and `parse` are functions of the module.

## What does not carry over

- **Inheritance.** There is none. What classes share by contract is a trait;
  what they share by code is a function both call, or a record one holds in
  a field. A closed family of cases, as the nodes of a tree, is an `enum`,
  and `match` then says when a case is missing.
- **Exceptions for what is expected.** An error a caller should handle is in
  the signature. A panic is for a bug, and `recover { ... }` stops one at the
  edge of a request or a job, as the `catch (Exception)` of a server does.
- **`null` by default.** Only a `T?` can be without a value, and it is
  matched, chained with `?.` or given a fallback with `??` before it is used.
- **Objects that are shared and changed.** A record is a value: a function
  that gets one cannot change the caller's, unless its parameter says `var`.
  What tasks share goes in a `sync.Mutex(value)`.
- **Reflection and attributes at run time.** `std.reflect` reads a type's
  fields and annotations, and `derive` writes code from them when the program
  is built: `tondo guide meta`.
- **The container, LINQ to SQL, the host.** Pass what a function needs.
  `std.sql` runs queries written as SQL, whose values are parameters;
  `std.http` serves with typed handlers; `std.cli` makes a command line of a
  record. `tondo guide stdlib` names the rest.

The original's tests become the port's first (`runbook-port`), and what
BenchmarkDotNet measured a test that calls `testing.bench`:
`runbook-testing`.
