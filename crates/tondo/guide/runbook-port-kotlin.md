# Runbook: porting from Kotlin

Kotlin already asks for much of what Tondo does: `val` and `var`, types that
say whether a value may be missing, data classes, `when` over a sealed
family. A port is mostly a change of words. What changes shape is what
Kotlin took from the JVM: exceptions nobody declares, classes that inherit,
and `suspend` functions. `runbook-port` has the method every port follows;
this is what each thing of Kotlin becomes.

## What each thing becomes

| Kotlin | Tondo |
| --- | --- |
| `val x = 5`, `var x = 5` | `let x = 5`, `var x = 5` |
| `Int`, `Long`, `Double`, `Boolean`, `String`, `Char`, `Byte` | `Int`, `Int`, `Float`, `Bool`, `String`, `Char`, `Byte` |
| `fun total(line: Line): Int` | `fn total(line: Line): Int` |
| `fun money(cents: Int) = "..."` | `fn money(cents: Int): String { "..." }`: the result is stated |
| `data class Line(val name: String)` | `type Line = { name: String }`: it prints, compares and copies |
| `Line("tea", 2)`, `line.copy(quantity = 3)` | `Line { name: "tea", quantity: 2 }`, `line with { quantity: 3 }` |
| a method, an extension function of your type | `fn Line.total(self): Int`, declared outside the type |
| an extension function of `String` | a function that takes one |
| `object Config`, a `companion object` | functions and constants of the module |
| `interface Priced`, `class Line : Priced` | `trait Priced`, `impl Priced for Line` |
| a `sealed class` and its subclasses, `when` | an `enum` whose variants hold data, `match` |
| an `open class` and its subclasses | a `trait` for what they share |
| `Int?`, `null`, `x?.y`, `x ?: d`, `x!!` | `Int?`, `none`, `x?.y`, `x ?? d`, `x.expect("why")` |
| `x?.let { use(it) }`, a smart cast after `if (x != null)` | `match x { some(v) => use(v), none => {} }` |
| `throw IllegalArgumentException(...)`, `require(...)` | `fail error`, in a function that returns `T ! E` |
| `try { } catch (e: XException) { }` | `match f() { ok(v) => ..., err(e) => ... }`; `?` to pass it on |
| `runCatching { }`, `Result<T>` | `T ! E` |
| `s.toIntOrNull()`, `s.toInt()` | `Int.parse(s)`, an `Int ! text.TextError` |
| `List<T>`, `MutableList<T>`, `Map<K, V>`, `Set<T>` | `Array[T]`, the same in a `var`, `Map[K, V]`, `Set[T]` |
| `listOf(1, 2)`, `mapOf("a" to 1)`, `setOf(1)` | `[1, 2]`, `["a": 1]`, `Set[1]` |
| `xs.size`, `xs += x`, `xs.isEmpty()`, `xs.first()` | `xs.length()`, `xs.push(x)`, `xs.isEmpty()`, `xs.first()` |
| `map`, `filter`, `fold`, `any`, `all`, `sortedBy`, `maxByOrNull`, `groupBy` | `map`, `filter`, `reduce`, `any`, `all`, `sortedBy`, `maxBy`, `groupBy` |
| `xs.map { it * 2 }`, `{ x -> x * 2 }` | `xs.map((x) { x * 2 })` |
| `for (x in xs)`, `for ((i, x) in xs.withIndex())`, `for (i in 0 until n)` | `for x in xs`, `for (i, x) in xs.enumerate()`, `for i in 0..n` |
| `"$name: ${price}"`, `"%.2f".format(price)` | `"{name}: {price:.2}"` |
| `if (c) a else b` | `if c { a } else { b }` |
| `suspend fun`, `launch`, `async` and `await`, `coroutineScope` | a plain function; `spawn`, a `Group`, the block that waits for its tasks |
| `Flow<T>`, `Channel<T>` | an iterator, `channel[T]()` |
| `Mutex().withLock { }`, `AtomicInteger`, `synchronized` | `sync.Mutex(value)`, `sync.Atomic(0)` |
| `use { }`, `finally` | `defer` |
| `<T : Comparable<T>>` | `[T: Ordered]` |
| `@Serializable`, `@SerialName("user_id")` | nothing, and `@name("user_id")` on the field |
| `package shop.orders`, `import shop.orders.Line` | the directory `src/orders`, `import shop.orders` |
| `@Test fun x() { assertEquals(a, b) }` | `test x { assert(b == a) }`, in `orders_test.to` |
| `println(x)`, `readLine()` | `console.println("{x}")`, `console.readLine()` |

## The story: a till, ported

The original reads sales, a line each, prices them as futures, and prints the
bill. Its `import` is left out here.

```kotlin
data class Line(val name: String, val quantity: Int, val cents: Int) {
    fun total() = quantity * cents
}

fun number(part: String): Int =
    part.toIntOrNull() ?: throw IllegalArgumentException("`$part` is no number")

/** A line as it is written, tea,2,250; throws when it is not one. */
fun parse(text: String): Line {
    val parts = text.split(",")
    require(parts.size == 3) { "it has ${parts.size} parts, not 3" }
    return Line(parts[0], number(parts[1]), number(parts[2]))
}

fun money(cents: Int) = "%d.%02d".format(cents / 100, cents % 100)

fun main() {
    val lines = mutableListOf<Line>()
    for (text in generateSequence(::readlnOrNull)) {
        try {
            lines += parse(text)
        } catch (why: IllegalArgumentException) {
            println("skipped `$text`: ${why.message}")
        }
    }
    val totals = lines.map { line -> CompletableFuture.supplyAsync { line.total() } }.map { it.join() }
    for ((line, total) in lines.zip(totals)) {
        println("${line.name}: ${money(total)}")
    }
    println("total ${money(totals.sum())}")
    lines.maxByOrNull { it.total() }?.let { println("dearest: ${it.name}") }
}
```

What it prints for four lines, one with a letter where a number goes and one
too short, is what the port must print:

```text
$ printf 'tea,2,250\ncake,x,400\nwater,1,100\nbread\n' | java -jar orders.jar
skipped `cake,x,400`: `x` is no number
skipped `bread`: it has 1 parts, not 3
tea: 5.00
water: 1.00
total 6.00
dearest: tea
```

## The first try, with Kotlin's words

Kotlin's forms are close, and a port tends to keep `fun`, the `try` and the
`!!`. `check` knows each and says Tondo's.

```tondo error E0004 file=orders.to
import std.console

type Line = { name: String, quantity: Int, cents: Int }

fun total(line: Line): Int {
    line.quantity * line.cents
}

fn number(part: String): Int {
    try {
        return Int.parse(part)
    } catch (why: NumberFormatException) {
        return 0
    }
}

fn main() {
    let dearest: Line? = none
    console.println("{total(dearest!!)} {number("2")}")
}
```

```mcp
check {"path": "orders.to"}
=> a function is `fn total(value: Type): Result { ... }`
=> there is no `try`: a function that can fail returns `T ! E`
=> `value.expect("why")` panics with why
```

With the forms right, three names are left.

```tondo error E1001 file=orders.to
import std.console

type Line = { name: String, quantity: Int, cents: Int }

fn dearest(lines: Array[Line]): Line? {
    if lines.size == 0 {
        return null
    }
    lines[0]
}

fn main() {
    println(dearest([]))
}
```

```mcp
check {"path": "orders.to"}
=> has no field `size`; use `length()`
=> unknown name `null`; Tondo writes `none`
=> unknown name `println`; `console.println(text)` writes a line
```

What Kotlin's library does, ask for by what it does.

```mcp
api {"search": "parse integer"}
=> Int.parse
```

One habit compiles, which makes it the one to look for: a string carried
over as it is. Kotlin's `$name` is text here, and `check` warns where
`name` is a binding there.

```tondo file=greeting.to
import std.console

fn main() {
    let name = "Ada"
    console.println("hi $name")
    console.println("hi ${name}")
}
```

```mcp
check {"path": "greeting.to"}
=> `$name` is written as it is: Tondo interpolates `{name}`
=> `${name}` writes a `$` and then the `String`: Tondo interpolates `{name}`
run {"path": "greeting.to"}
=> hi $name
=> hi $Ada
```

Take the `$` out of both. Before a number it says nothing: `"${price}"` is
a dollar and the price, as a price is written.

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

- **The exception, `require` and the `catch`.** `parse` says in its result
  that it can fail, which no Kotlin signature does, and `match` takes both
  cases.
- **The futures and `join()`.** `spawn` starts each pricing as a task, and
  the `Group` waits for them and gives their results in order.
- **`?.let`.** `maxBy` gives a `Line?`, and `match` takes the case with none.
- **`mutableListOf` against `listOf`.** An array changes when its binding is
  a `var`.

## What does not carry over

- **Exceptions.** Kotlin has no checked ones, so nothing says what a call can
  throw. In Tondo an error a caller should handle is in the signature, and a
  bug panics: `recover { ... }` stops a panic at the edge of a request or a
  job.
- **Inheritance.** `open`, `abstract` and `override` go. A contract is a
  trait, a closed family an `enum`, shared code a function.
- **Objects shared and changed.** A record is a value: a function that gets
  one cannot change the caller's, unless its parameter says `var`. What tasks
  share goes in a `sync.Mutex(value)`.
- **`suspend` and its colour.** Any function can wait, so there is no
  `suspend` to spread through the callers and no dispatcher to choose. A task
  that is not waited for is waited at the end of its block, which is what a
  `coroutineScope` gave.
- **Platform types.** A value that came from Java and may be `null` without
  saying so has no like here: only a `T?` can be `none`.
- **Arithmetic that wraps.** An `Int` has 64 bits and panics when a result
  does not fit, where Kotlin's wraps in silence. Code that counts on it, as a
  hash, says `wrappingAdd` and `wrappingMul`.
- **A text with `$name` in it.** In Tondo it is `{name}`, and a `$` is a
  dollar. `check` warns (`W1022`), as above, of `$name` where `name` is a
  binding and of `${name}` where the value is not a number; a dollar before
  a number is a price, and stays.
- **The JVM and its libraries.** A program is one native executable. The
  standard library has HTTP, JSON, SQL, templates and a command line:
  `tondo guide stdlib`.

The original's tests become the port's first (`runbook-port`).
