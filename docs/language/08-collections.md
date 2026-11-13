# Collections

Four collections are part of the language: `Array[T]`, `Map[K, V]`,
`Set[T]` and `Deque[T]`. All four are values: assigning one copies it, a
function that receives one cannot change the caller's, and two are equal
when their contents are. This chapter covers them, with ranges and
iterators.

- [Arrays](#arrays)
- [Indexes and slices](#indexes-and-slices)
- [Changing an array](#changing-an-array)
- [Arithmetic on arrays](#arithmetic-on-arrays)
- [The methods of an array](#the-methods-of-an-array)
- [Maps](#maps)
- [Sets](#sets)
- [Deques](#deques)
- [Ranges](#ranges)
- [Iterators](#iterators)
- [Your own iterator](#your-own-iterator)
- [Collections are values](#collections-are-values)
- [What each operation costs](#what-each-operation-costs)

## Arrays

An array is an ordered sequence of values of one type, which grows.

```tondo
import std.console

fn main() {
    let primes = [2, 3, 5, 7]                   // Array[Int]
    let names: Array[String] = []               // an empty one says its type
    var squares = Array[Int]()                  // or is built by calling the type
    for n in 1..=4 {
        squares.push(n * n)
    }
    console.println("{primes} {names} {squares}")
    console.println("{primes.length()} {primes.isEmpty()} {primes.first()} {primes.last()} {3 in primes}")
}
```

```text output
[2, 3, 5, 7] [] [1, 4, 9, 16]
4 false some(2) some(7) true
```

## Indexes and slices

```tondo
import std.console

fn main() {
    let values = [10, 20, 30, 40, 50]
    console.println("{values[0]} {values[-1]} {values[-2]}")
    console.println("{values[1:4]} {values[:2]} {values[3:]}")
    console.println("{values[::2]} {values[::-1]} {values[-100:100]}")
    console.println("{values.get(9)} {values.get(-1)}")
}
```

```text output
10 50 40
[20, 30, 40] [10, 20] [40, 50]
[10, 30, 50] [50, 40, 30, 20, 10] [10, 20, 30, 40, 50]
none some(50)
```

- An index counts from 0, and a negative one from the end: `-1` is the last
  element.
- `values[i]` panics when there is no such element, since that is a mistake
  in the program. `values.get(i)` gives an option, for an index that comes
  from outside.
- A slice is `values[start:end:step]`, from `start` up to but not including
  `end`. Any of the three may be left out, a negative step goes backwards,
  and ends that fall outside are brought in rather than panicking.
- A slice is an array, and taking one copies nothing: it shares the
  original's storage, as any copy does until one of them changes.

## Changing an array

An array changes through a `var`:

```tondo
import std.console

fn main() {
    var values = [1, 2, 3, 4, 5]
    values[0] = 10                      // one element
    values[1:3] = [20, 30]              // a slice, by as many elements
    values[3:] += 100                   // arithmetic in place on a slice
    console.println("{values}")

    values.push(6)
    values.insert(0, 0)
    let last = values.pop()
    let removed = values.removeAt(1)
    values.extend([7, 8])
    console.println("{values} {last} {removed}")

    values.sort()
    values.reverse()
    values.retain((n) { n % 2 == 0 })
    console.println("{values}")
}
```

```text output
[10, 20, 30, 104, 105]
[0, 20, 30, 104, 105, 7, 8] some(6) some(10)
[104, 30, 20, 8, 0]
```

Assigning to a slice never changes the array's length: the right side has as
many elements as the slice. Growing and shrinking have names, `push`, `pop`,
`insert`, `removeAt`, `extend`, `retain`, `clear`.

Many methods come in two forms: one that changes the array in place and one
that gives a new array and leaves the original alone.

| In place, on a `var` | A new array |
| --- | --- |
| `values.sort()` | `values.sorted()` |
| `values.sortBy((v) { ... })` | `values.sortedBy((v) { ... })` |
| `values.reverse()` | `values.reversed()` |
| `values[i] = x` | `values.replaced(i, x)` |
| `values.retain((v) { ... })` | `values.filter((v) { ... })` |
| `values.extend(other)` | `values.concat(other)` |

## Arithmetic on arrays

The arithmetic operators work on arrays of numbers, element by element, and
between an array and a number:

```tondo
import std.console

fn main() {
    let prices = [10.0, 20.0, 30.0]
    let quantities = [3.0, 1.0, 2.0]
    let totals = prices * quantities
    console.println("{totals} {totals.sum()}")
    console.println("{prices * 1.21} {prices - 5.0} {100.0 / prices}")

    var scores = [70, 85, 90]
    scores += 5                         // in place
    scores[::2] *= 2
    console.println("{scores}")
}
```

```text output
[30.0, 20.0, 60.0] 110.0
[12.1, 24.2, 36.3] [5.0, 15.0, 25.0] [10.0, 5.0, 3.3333333333333335]
[150, 90, 190]
```

Two arrays must have the same length; when they do not, that is a panic
(`P0006`), checked before anything is computed. The elements keep the rules
of their type: integers still panic on overflow, and nothing is converted.

`+` never joins two arrays and `*` never repeats one. Those are
`a.concat(b)` and `a.repeat(3)`.

## The methods of an array

`tondo api Array` lists them all, with what each gives. By what they are
for:

| To | Use |
| --- | --- |
| transform | `map`, `flatMap`, `enumerate`, `zip` |
| select | `filter`, `take`, `drop`, `unique`, `partition` |
| ask | `contains`, `any`, `all`, `find`, `findIndex`, `count` |
| reduce | `reduce`, `sum`, `min`, `max`, `join` |
| order | `sorted`, `sortedBy`, `sortedWith`, `reversed` |
| group | `groupBy`, `chunks`, `windows` |

```tondo
import std.console

pub type Item = { name: String, price: Int, tags: Array[String] }

fn main() {
    let items = [
        Item { name: "pen", price: 3, tags: ["office"] },
        Item { name: "ink", price: 5, tags: ["office", "refill"] },
        Item { name: "tea", price: 4, tags: ["kitchen"] },
    ]
    let names = items.map((item) { item.name })
    let cheap = items.filter((item) { item.price < 5 }).map((item) { item.name })
    let total = items.reduce(0, (sum, item) { sum + item.price })
    let dearest = items.sortedBy((item) { -item.price })[0].name
    let tags = items.flatMap((item) { item.tags }).unique()
    let byTag = items.groupBy((item) { item.tags[0] }).mapValues((group) { group.length() })
    console.println("{names.join(", ")} | {cheap} | {total} | {dearest}")
    console.println("{tags} {byTag}")
    console.println("{items.any((item) { item.price > 4 })} {items.find((item) { item.name == "tea" }).isSome()}")
    console.println("{[1, 2, 3, 4, 5].chunks(2)} {[1, 2, 3, 4].windows(2)} {[1, 2, 3, 4].partition((n) { n > 2 })}")
}
```

```text output
pen, ink, tea | [pen, tea] | 12 | ink
[office, refill, kitchen] [office: 2, kitchen: 1]
true true
[[1, 2], [3, 4], [5]] [[1, 2], [2, 3], [3, 4]] ([3, 4], [1, 2])
```

These methods take closures compiled in place, so a chain of them is not
slower than the loop that would replace it, and `tondo optimizations` says
what a release build made of each call.

## Maps

A map goes from keys to values, and remembers the order its keys were added
in.

```tondo
import std.console

fn main() {
    var ages = ["ana": 32, "leo": 28]           // Map[String, Int]
    var empty: Map[String, Int] = [:]
    empty["first"] = 1

    ages["zoe"] = 41                            // adds
    ages["ana"] = 33                            // replaces, and keeps its place
    console.println("{ages} {ages.length()}")

    console.println("{ages["leo"]} {ages["nobody"]} {ages["nobody"] ?? 0} {"zoe" in ages}")
    let removed = ages.remove("leo")
    console.println("{removed} {ages}")

    for (name, age) in ages {
        console.println("{name} is {age}")
    }
    console.println("{ages.keys().collect()} {ages.values().collect()} {empty}")
}
```

```text output
[ana: 33, leo: 28, zoe: 41] 3
some(28) none 0 true
some(28) [ana: 33, zoe: 41]
ana is 33
zoe is 41
[ana, zoe] [33, 41] [first: 1]
```

- Reading `map[key]` gives a `V?`, because a key that is not there is
  normal. A missing key is `none`, never a zero that looks like a value.
  `map[key] ?? fallback` gives the value or the fallback.
- Writing `map[key] = value` adds or replaces.
- `map[key, default: value]` is the entry to change in place: when the key
  is missing it is put there first with that value, which is evaluated only
  then. So `counts[word, default: 0] += 1` counts, and
  `groups[key, default: []].push(item)` groups. A plain `map[key] += 1` is
  an error, since the key may not be there and nothing says what then.
- `remove(key)` takes an entry out and gives its value. Assigning `none`
  does not remove anything.
- Order is the order of adding. Replacing a value keeps the key's place;
  removing a key and adding it again puts it at the end. Two maps with the
  same entries are equal whatever their order.
- A key is any type that can be compared and hashed, which the compiler
  works out: numbers, strings, characters, booleans, tuples, records and
  enums made of those. A `Float` is not a key.

```tondo
import std.console

type Point = { x: Int, y: Int }

fn main() {
    // Counting words, and grouping them by their length.
    var counts: Map[String, Int] = [:]
    var byLength: Map[Int, Array[String]] = [:]
    for word in "to be or not to be".words() {
        counts[word, default: 0] += 1
        if counts[word] == 1 {
            byLength[word.length(), default: []].push(word)
        }
    }
    console.println("{counts} {byLength}")

    // A record as a key, and a map built from others.
    var visited: Map[Point, String] = [:]
    visited[Point { x: 0, y: 0 }] = "start"
    let frequent = counts.filter((_, count) { count > 1 })
    let doubled = counts.mapValues((count) { count * 2 })
    let merged = frequent.merge(["and": 1])
    console.println("{visited.length()} {frequent} {doubled["to"] ?? 0} {merged}")
}
```

```text output
[to: 2, be: 2, or: 1, not: 1] [2: [to, be, or], 3: [not]]
1 [to: 2, be: 2] 4 [to: 2, be: 2, and: 1]
```

## Sets

A set holds each value once, in the order they were added.

```tondo
import std.console

fn main() {
    var seen = Set["read", "write"]
    console.println("{seen.insert("admin")} {seen.insert("read")} {seen}")
    console.println("{"write" in seen} {seen.remove("write")} {seen.length()}")

    let editors = Set["ana", "leo"]
    let admins = Set["leo", "zoe"]
    console.println("{editors.union(admins)} {editors.intersection(admins)} {editors.difference(admins)}")
    console.println("{Set["leo"].isSubset(admins)} {editors == Set["leo", "ana"]}")

    var empty = Set[Int]()
    empty.insert(1)
    console.println("{[3, 1, 3, 2, 1].unique()} {empty}")
}
```

```text output
true false Set[read, write, admin]
true true 2
Set[ana, leo, zoe] Set[leo] Set[ana]
true true
[3, 1, 2] Set[1]
```

`insert` and `remove` say whether the set changed. Union, intersection and
difference have names: `|` and `&` are not for sets.

## Deques

A deque is a queue with two ends: adding and taking at either one is cheap.

```tondo
import std.console

fn main() {
    var queue = Deque[String]()
    queue.push("second")
    queue.push("third")
    queue.pushFront("first")
    console.println("{queue} {queue.length()}")
    console.println("{queue.popFront()} {queue.pop()} {queue}")
}
```

```text output
[first, second, third] 3
some(first) some(third) [second]
```

Use an `Array` as a stack, with `push` and `pop`, and a `Deque` as a queue,
with `push` and `popFront`. To hand work from one task to another, a channel
is the queue to use ([Tasks](13-tasks.md)).

## Ranges

`0..10` is the integers from 0 up to but not including 10, and `0..=10`
includes the 10. A range is a value, and takes no space for its elements:

```tondo
import std.console

fn main() {
    let digits = 0..10
    console.println("{digits.start()} {digits.end()} {5 in digits} {10 in digits} {(5..5).isEmpty()}")
    console.println("{(1..=5).collect()} {('a'..='e').collect()}")
    var total = 0
    for n in 1..=100 {
        total += n
    }
    console.println("{total}")
}
```

```text output
0 10 true false true
[1, 2, 3, 4, 5] [a, b, c, d, e]
5050
```

A range is of integers or of characters, and goes up by one. One whose start
is past its end is empty. For another step or direction, take the array and
slice it, or use the adapters below: `(0..10).filter((n) { n % 3 == 0 })`.

## Iterators

A range, the characters of a string, the keys of a map and `array.iter()`
are iterators: they give their values one at a time, and what is chained
onto them does no work until something asks.

```tondo
import std.console

fn main() {
    let squares = (1..=1_000_000)
        .map((n) { n * n })
        .filter((n) { n % 3 == 1 })
        .take(4)
        .collect()
    console.println("{squares}")

    let letters = "Hello, World".chars().filter((c) { c.isUpper() }).collect()
    let numbered = ["a", "b"].iter().enumerate().collect()
    let pairs = (0..3).zip("abc".chars()).collect()
    console.println("{letters} {numbered} {pairs}")

    let total = (1..=10).reduce(0, (sum, n) { sum + n })
    let firstBig = (1..=100).find((n) { n * n > 50 })
    console.println("{total} {firstBig} {(1..=10).count()} {(0..5).chain(10..12).collect()}")
}
```

```text output
[1, 4, 16, 25]
[H, W] [(0, a), (1, b)] [(0, a), (1, b), (2, c)]
55 some(8) 10 [0, 1, 2, 3, 4, 10, 11]
```

Only four squares were computed above, not a million.

| Adapters, which give another iterator | Ends, which run it |
| --- | --- |
| `map`, `filter`, `flatMap` | `collect()`: an array |
| `take`, `skip`, `takeWhile`, `skipWhile` | `count()`, `reduce`, `find` |
| `enumerate`, `zip`, `chain` | `any`, `all`, `forEach` |

An array's own `map` and `filter` give arrays at once, which is what is
wanted most of the time. Reach for `iter()` when the chain is long and its
intermediate arrays would be large, or when it should stop early.

## Your own iterator

A type becomes an iterator by implementing `Iterator[T]`, which is one
method: `next`, giving the next value or `none`. `for` and all the adapters
then work on it.

```tondo
import std.console

type Countdown = { remaining: Int }

impl Iterator[Int] for Countdown {
    fn next(mut self): Int? {
        if self.remaining == 0 {
            return none
        }
        self.remaining -= 1
        self.remaining + 1
    }
}

type Fibonacci = { current: Int, following: Int }

impl Iterator[Int] for Fibonacci {
    fn next(mut self): Int? {
        let value = self.current
        (self.current, self.following) = (self.following, self.current + self.following)
        value
    }
}

fn main() {
    let countdown = Countdown { remaining: 3 }
    for n in countdown {
        console.println("{n}")
    }
    let fibonacci = Fibonacci { current: 0, following: 1 }
    console.println("{fibonacci.takeWhile((n) { n < 60 }).filter((n) { n % 2 == 0 }).collect()}")
}
```

```text output
3
2
1
[0, 2, 8, 34]
```

An iterator can go on for ever, as `Fibonacci` does, since nothing is
computed until it is asked for.

## Collections are values

```tondo
import std.console

fn withTax(prices: Array[Float]): Array[Float] {
    prices * 1.21
}

fn addBonus(scores: var Array[Int], bonus: Int) {
    scores.push(bonus)
}

fn main() {
    let original = [1, 2, 3]
    var copy = original
    copy.push(4)                        // `original` is untouched
    console.println("{original} {copy}")

    let prices = [10.0, 20.0]
    let taxed = withTax(prices)         // the function got its own
    console.println("{prices} {taxed}")

    var scores = [70, 80]
    addBonus(var scores, 5)             // unless the call lends it, and says so
    console.println("{scores}")
}
```

```text output
[1, 2, 3] [1, 2, 3, 4]
[10.0, 20.0] [12.1, 24.2]
[70, 80, 5]
```

A copy is not paid for when it is made. The two share their storage, and the
one that is changed first takes its own copy of it then: copying a million
elements and never changing them costs one reference count. This is why it
is fine to pass collections around, keep an old version, or return one from
a function, without thinking about who else can see it. Nobody else can.

For data that several tasks do change together, `std.sync` has collections
made for that ([Tasks](13-tasks.md)).

## What each operation costs

| Operation | Cost |
| --- | --- |
| `array[i]`, `array.length()`, `push`, `pop` | constant |
| a slice, a copy | constant: the storage is shared |
| the first change after a copy | the size of the collection, once |
| `insert`, `removeAt` | the elements after the index |
| `array.contains(x)`, `x in array` | the length |
| `map[key]`, `map[key] = v`, `key in map`, `set.insert`, `x in set` | constant on average |
| `map.remove(key)` | constant on average, and the order is kept |
| `deque.push`, `pushFront`, `pop`, `popFront` | constant |
| `sorted`, `sort` | n log n |

Next: [Text](09-text.md).
