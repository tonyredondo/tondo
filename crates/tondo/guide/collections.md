# Collections

`Array[T]`, `Map[K, V]`, `Set[T]` and `Deque[T]` are values: assigning one
copies it (the storage is shared until a copy changes). A function changes
the caller's collection only through a `var` parameter or `var self`.

```tondo
import std.console

fn main() {
    var numbers = [3, 1, 2]
    numbers.push(4)
    numbers.sort()
    console.println("{numbers} {numbers.length()} {numbers[0]} {numbers.get(9)}")

    var ages: Map[String, Int] = [:]
    ages["ada"] = 36
    ages["bo"] = 20
    console.println("{ages.get("ada")} {ages.getOr("cy", 0)} {"bo" in ages}")
    for (name, age) in ages {                   // in insertion order
        console.println("{name} is {age}")
    }

    var seen = Set[1, 2]
    seen.insert(3)
    console.println("{seen.contains(2)} {seen}")

    var queue = Deque.new[String]()
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
    let byLetter = items.groupBy((item) { item.name.slice(0, 1).unwrapOr("") })
    console.println("{names} {cheap.length()} {total} {sorted[0].name} {byLetter.length()}")
    console.println("{names.join(", ")} {[3, 1, 2].sorted()} {[1, 2, 3].sum()} {[1, 2].zip(["a", "b"])}")
}
```

## Ranges and iterators

`0..10` and `0..=10` are `Range[Int]` values; `'a'..='z'` a `Range[Char]`.
A range, `text.chars()`, `array.iter()` and `map.keys()` are iterators: their
adapters are lazy and `collect()` makes an array.

```tondo
import std.console

fn main() {
    let evens = (0..10).filter((n) { n % 2 == 0 }).map((n) { n * n }).collect()
    let letters = "hello".chars().filter((c) { c != 'l' }).collect()
    let indexed = ["a", "b"].iter().enumerate().collect()
    console.println("{evens} {letters} {indexed} {5 in 0..10}")
}
```

A type becomes an iterator by implementing `Iterator[T]` with
`fn next(mut self): T?`; `for x in value` then works, and so do the adapters.
