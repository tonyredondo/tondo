# Collections

`Array[T]`, `Map[K, V]`, `Set[T]` and `Deque[T]` are values: assigning one
copies it (the storage is shared until a copy changes). A function changes
the caller's collection only through a `var` parameter or `var self`.
An empty one takes its type from where it goes, as `[]`, `[:]` and `Set[]`, or
says it: `Array[Int]()`, `Map[String, Int]()`.

```tondo
import std.console

fn main() {
    var numbers = [3, 1, 2]
    numbers.push(4)
    numbers.sort()
    console.println("{numbers} {numbers.length()} {numbers[0]} {numbers.get(9)} {numbers.contains(2)}")

    var ages: Map[String, Int] = [:]
    ages["ada"] = 36
    ages["bo"] = 20
    console.println("{ages.get("ada")} {ages["cy"] ?? 0} {"bo" in ages}")
    for (name, age) in ages {                   // in insertion order
        console.println("{name} is {age}")
    }

    // An entry to change, put there first when it is missing.
    var counts: Map[String, Int] = [:]
    var byLetter: Map[String, Array[String]] = [:]
    for word in ["ox", "ant", "owl"] {
        counts[word, default: 0] += 1
        byLetter[word.slice(0, 1) ?? "", default: []].push(word)
    }
    console.println("{counts} {byLetter}")

    var seen = Set[1, 2]
    seen.insert(3)
    console.println("{seen.contains(2)} {seen} {seen.symmetricDifference(Set[3, 4])}")
    seen.subtract(Set[1])                       // in place: also extend, intersect, retain
    console.println("{seen} {seen.isDisjoint(Set[5])} {[2, 2, 7].toSet()}")

    var queue = Deque[String]()
    queue.push("b")
    queue.pushFront("a")
    console.println("{queue.popFront()} {queue}")
}
```

## Methods of the prelude

`tondo api prelude` lists them. The everyday ones:

```tondo
import std.console

pub type Item = { name: String, price: Int }

fn main() {
    let items = [Item { name: "pen", price: 3 }, Item { name: "ink", price: 5 }]
    let names = items.map((item) { item.name })
    let cheap = items.filter((item) { item.price < 4 })
    let total = items.reduce(0, (sum, item) { sum + item.price })
    let sorted = items.sortedBy((item) { item.price })
    let byLetter = items.groupBy((item) { item.name.slice(0, 1) ?? "" })
    console.println("{names} {cheap.length()} {total} {sorted[0].name} {byLetter.length()}")
    console.println("{names.join(", ")} {[3, 1, 2].sorted()} {[1, 2, 3].sum()} {[1, 2].zip(["a", "b"])}")
}
```

By a key of each element, as LINQ and the like ask it: `minBy`, `maxBy`,
`uniqueBy`, `countBy`, `sumBy`, `averageBy`, `keyedBy`, `toMap`, and
`joinBy`, which pairs two arrays by their keys as a database joins tables;
then `takeWhile`, `dropWhile`, `scan`, `findLast`, `indexOf`, `toSet` and
`average`. There is no `flatten`: `flatMap((inner) { inner })` is it.

```tondo
import std.console

pub type Sale = { product: String, amount: Float }

fn main() {
    let sales = [Sale { product: "pen", amount: 3.0 }, Sale { product: "ink", amount: 5.0 }, Sale { product: "pen", amount: 4.5 }]
    console.println("{sales.countBy((sale) { sale.product })} {sales.sumBy((sale) { sale.amount })}")   // [pen: 2, ink: 1] 12.5
    let prices = [Sale { product: "pen", amount: 1.5 }]
    for (sale, price) in sales.joinBy(prices, (sale) { sale.product }, (price) { price.product }) {
        console.println("{sale.amount / price.amount} pens")                                        // 2.0 pens, then 3.0 pens
    }
}
```

`sum` of floats adds in partial sums, each element into the one of its place,
an order every machine gives alike and vectors compute fast: it can differ
in the last bits from adding left to right. `math.dot(a, b)` adds products the
same way, and `a + b`, `a * 2.0` and the like go element by element.

For numbers in several dimensions, `math.Tensor` keeps a shape and its values
in one block: `math.Tensor.fromRows([[1.0, 2.0], [3.0, 4.0]])`,
`math.Tensor(values, [2, 3])`, `math.Tensor[Float].zeros([4])`. `+ - * /` go
element by element, a size of 1 repeated along the other's (a row with a
matrix), and a number on either side; `matmul`, `transposed`, `sumAlong`,
`at(i, j)` and `slice` do the rest. It shows as nested arrays.

## Ranges and iterators

`0..10` and `0..=10` are `Range[Int]` values; `'a'..='z'` a `Range[Char]`.
A range, `text.chars()`, `array.iter()` and `map.keys()` are iterators: their
adapters are lazy and `collect()` makes an array. A chain ends as an array
does, with no array between: `sum`, `min`, `max`, `average`, `toSet`, `count`
and the `By` methods give what the array's would.

```tondo
import std.console

fn main() {
    let evens = (0..10).filter((n) { n % 2 == 0 }).map((n) { n * n }).collect()
    let letters = "hello".chars().filter((c) { c != 'l' }).collect()
    let indexed = ["a", "b"].iter().enumerate().collect()
    let total = (0..10).filter((n) { n % 2 == 0 }).map((n) { n * n }).sum()
    console.println("{evens} {letters} {indexed} {5 in 0..10} {total}")
}
```

A type becomes an iterator by implementing `Iterator[T]` with
`fn next(mut self): T?`; `for x in value` then works, and so do the adapters.

An operation that can fail by its arguments, as `array.slice(2, 9)` past
the end or `sync.Array.set` outside it, fails with a
`collections.CollectionError` (`import std.collections`): `InvalidIndex`,
`InvalidStep`, `InvalidCapacity` or `ResourceLimit`.
