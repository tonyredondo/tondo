# Runbook: porting from Java

Java's records, interfaces, generics and streams have their like in Tondo.
What goes is the class as the unit of everything: inheritance, `null`,
exceptions, and the objects made only to hold one method. A port keeps the
data and the functions, and lets the rest go. `runbook-port` has the method
every port follows; this is what each thing of Java becomes.

## What each thing becomes

| Java | Tondo |
| --- | --- |
| `String s = "x";`, `final int n = 5;`, `var x = 5;` | `let s = "x"`, `let n = 5`; `var` for what changes |
| `int`, `long`, `double`, `boolean`, `char`, `String` | `Int`, `Int`, `Float`, `Bool`, `Char`, `String` |
| a `record`, or a class of fields with getters | `type Line = { name: String }`, read as `line.name` |
| a method of the class | `fn Line.total(self): Int`, outside the type |
| a method that changes a field | `fn Line.add(mut self, n: Int)` |
| `static` methods, a class of utilities | functions of the module |
| `public`, package-private, `private` | `pub`, a module under a directory `internal`, nothing |
| `interface Shape`, `class Circle implements Shape` | `trait Shape`, `impl Shape for Circle` |
| `class B extends A`, `abstract`, `@Override` | a `trait` for what they share; an `enum` when the cases are known |
| a sealed interface and its records | `enum Shape { Circle(Float), Square(Float) }` |
| `null`, `Optional<T>`, `opt.orElse(d)` | `T?`, `none`, `opt ?? d` |
| `a.equals(b)`, `hashCode` | `a == b`: values compare by what they hold |
| `throw new XException(...)`, `throws` | `fail error`, and `T ! E` as the result |
| `try { } catch (XException e) { }` | `match f() { ok(v) => ..., err(e) => ... }`; `?` to pass it on |
| `finally`, try-with-resources | `defer` |
| `Integer.parseInt(s)` | `Int.parse(s)`, an `Int ! text.TextError` |
| `List<T>`, `Map<K, V>`, `Set<T>`, `Deque<T>` | `Array[T]`, `Map[K, V]`, `Set[T]`, `Deque[T]` |
| `new ArrayList<>()`, `list.add(x)`, `list.size()`, `list.get(i)` | `[]`, `list.push(x)`, `list.length()`, `list[i]` |
| `map.get(k)`, `map.getOrDefault(k, d)`, `map.merge(k, 1, Integer::sum)` | `map.get(k)`, `map.get(k) ?? d`, `map[k, default: 0] += 1` |
| `for (String x : xs)`, `for (int i = 0; i < n; i++)` | `for x in xs`, `for i in 0..n` |
| `stream().filter(g).map(f).toList()` | `xs.filter(g).map(f)`; from `xs.iter()` for a lazy chain |
| `max(Comparator.comparingInt(f))`, `sorted(...)`, `Collectors.groupingBy(f)` | `maxBy(f)`, `sortedBy(f)`, `groupBy(f)` |
| `x -> x * 2`, `Line::total` | `x => x * 2`, and `(x) { ... }` with statements; `Line.total` |
| a `switch` expression, `instanceof` with a pattern | `match`, which must cover every case |
| `<T extends Comparable<T>>` | `[T: Ordered]` |
| `Thread`, an `ExecutorService`, `CompletableFuture` | `spawn f()`, `wait()` on an array of tasks |
| `synchronized`, `AtomicInteger`, `BlockingQueue` | `sync.Mutex(value)`, `sync.Atomic(0)`, `channel[T]()` |
| `String.format("%s: %02d", a, b)`, `"a" + b` | `"{a}: {b:02}"`: `+` does not join strings |
| `package shop.orders;`, `import shop.orders.Line;` | the directory `src/orders`, `import shop.orders` |
| `@Test void x()`, `assertEquals(a, b)` | `test x { assert(b == a) }`, in `orders_test.to` |
| `@JsonProperty("user_id")` | `@name("user_id")` on the field |
| what a container injects | a parameter; a `context` for what is ambient, as the request |
| `System.out.println(x)` | `console.println("{x}")` |

## The story: a till, ported

The original reads sales, a line each, prices them as futures, and prints the
bill. Its imports and the class around it are left out here.

```java
record Line(String name, int quantity, int cents) {
    int total() {
        return quantity * cents;
    }
}

static int number(String part) {
    try {
        return Integer.parseInt(part);
    } catch (NumberFormatException e) {
        throw new IllegalArgumentException("`" + part + "` is no number");
    }
}

/** A line as it is written, tea,2,250; throws when it is not one. */
static Line parse(String text) {
    String[] parts = text.split(",");
    if (parts.length != 3) {
        throw new IllegalArgumentException("it has " + parts.length + " parts, not 3");
    }
    return new Line(parts[0], number(parts[1]), number(parts[2]));
}

static String money(int cents) {
    return String.format("%d.%02d", cents / 100, cents % 100);
}

public static void main(String[] args) throws IOException {
    List<Line> lines = new ArrayList<>();
    BufferedReader input = new BufferedReader(new InputStreamReader(System.in));
    String text;
    while ((text = input.readLine()) != null) {
        try {
            lines.add(parse(text));
        } catch (IllegalArgumentException why) {
            System.out.println("skipped `" + text + "`: " + why.getMessage());
        }
    }
    List<CompletableFuture<Integer>> pricing =
        lines.stream().map(line -> CompletableFuture.supplyAsync(line::total)).toList();
    List<Integer> totals = pricing.stream().map(CompletableFuture::join).toList();
    for (int i = 0; i < lines.size(); i++) {
        System.out.println(lines.get(i).name() + ": " + money(totals.get(i)));
    }
    System.out.println("total " + money(totals.stream().mapToInt(Integer::intValue).sum()));
    Optional<Line> dearest = lines.stream().max(Comparator.comparingInt(Line::total));
    if (dearest.isPresent()) {
        System.out.println("dearest: " + dearest.get().name());
    }
}
```

What it prints for four lines, one with a letter where a number goes and one
too short, is what the port must print:

```text
$ printf 'tea,2,250\ncake,x,400\nwater,1,100\nbread\n' | java Orders
skipped `cake,x,400`: `x` is no number
skipped `bread`: it has 1 parts, not 3
tea: 5.00
water: 1.00
total 6.00
dearest: tea
```

## The first try, with Java's habits

Written as Java thinks, the port has a class, a `try` and a `throw`. `check`
answers each with what Tondo does instead.

```tondo error E0004 file=orders.to
import std.console

class Line {
    name: String
}

fn number(part: String): Int {
    try {
        return Int.parse(part)
    } catch (NumberFormatException e) {
        return 0
    }
}

fn checked(quantity: Int) {
    if quantity < 0 {
        throw new IllegalArgumentException("negative")
    }
}

fn main() {
    console.println("{number("2")}")
}
```

```mcp
check {"path": "orders.to"}
=> a record is `type Line = { field: Type }`
=> there is no `try`: a function that can fail returns `T ! E`
=> a function that returns `T ! E` fails with `fail error`
```

With the forms right, Java's types and its `null` are the next round.

```tondo error E1001 file=orders.to
import std.console

type Line = { name: String, quantity: long, cents: long }

fn isTea(line: Line?): boolean {
    if line == null {
        return false
    }
    line.name.equals("tea")
}

fn main() {
    console.println("{isTea(none)}")
}
```

```mcp
check {"path": "orders.to"}
=> unknown type `long`: Tondo writes `Int`
=> unknown type `boolean`: Tondo writes `Bool`
=> unknown name `null`; Tondo writes `none`
=> it may be `none`: reach the `Line` inside with `match`
```

The last one is what `null` never said: a `Line?` is not a `Line`, and the
check before it does not make it one. `match`, `?.` or `??` take the value
out. What Java's library does, ask for by what it does.

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
    let totals = lines.map(line => spawn line.total()).wait()
    for (index, line) in lines.enumerate() {
        console.println("{line.name}: {money(totals[index])}")
    }
    console.println("total {money(totals.sum())}")
    match lines.maxBy(line => line.total()) {
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

- **Both `try` and the exception that crossed them.** `parse` gives a `Line`
  or fails with the reason, in its signature, and `match` takes both cases.
- **The futures, their pool and `join`.** `spawn` starts each pricing as a
  task, and `wait()` on the array of them gives their results in order.
- **`Optional`, `isPresent()` and `get()`.** `maxBy` gives a `Line?`, which
  cannot be read without saying what happens when there is none.
- **The class around it all.** Functions belong to a module.

## What does not carry over

- **Inheritance.** There is none. A contract is a trait; code that is shared
  is a function, or a record held in a field. A family of cases that is
  closed is an `enum`, and `match` says when one is missing.
- **`null`.** Only a `T?` can be without a value, and it is taken apart
  before it is used: there is no `NullPointerException` to find later.
- **Exceptions for what is expected, and `RuntimeException` for the rest.**
  An error a caller should handle is in the signature; a bug panics, and
  `recover { ... }` stops a panic at the edge of a request or a job.
- **Objects shared and changed.** A record or a list given to a method is the
  caller's still, unchanged, unless the parameter says `var`. What tasks
  share goes in a `sync.Mutex(value)`.
- **Arithmetic that wraps.** An `Int` has 64 bits and panics when a result
  does not fit, where Java's `int` wraps in silence. Code that counts on it,
  as a hash, says `wrappingAdd` and `wrappingMul`.
- **Reflection, annotations read at run time, a container.** `std.reflect`
  reads a type's fields and annotations, `derive` writes code from them when
  the program is built (`tondo guide meta`), and what a function needs is
  passed to it.
- **The JVM.** A program is one native executable that starts at once:
  nothing to warm up, and nothing to install where it runs.

The original's tests become the port's first (`runbook-port`), and its JMH
benchmarks tests that call `testing.bench`: `runbook-testing`.
