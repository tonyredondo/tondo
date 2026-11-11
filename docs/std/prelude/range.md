# Range and Iterator

`0..10` is a `Range[Int]`, and an `Iterator[T]` is what `for` goes through: its adapters are lazy.

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api Range`, `tondo api Iterator` show the same in a terminal.

- [Range](#range): [`Range.start`](#rangestart), [`Range.end`](#rangeend), [`Range.isInclusive`](#rangeisinclusive), [`Range.isEmpty`](#rangeisempty), [`Range.contains`](#rangecontains)
- [Iterator](#iterator)
- [For other types](#for-other-types)

### Range

```tondo
pub type Range[T] = {
    // private fields
}
```

`start..end`, without `end`, or `start..=end`, with it: the values from
`start` on, one after another; none when `start` is past the end.

```tondo
import std.console

let hours = 9..17
let week = 1..=7
console.println("{hours.start()}..{hours.end()} has 12: {hours.contains(12)}, inclusive: {week.isInclusive()}")
console.println("{(0..10).filter((n) { n % 3 == 0 }).collect()} {(5..5).isEmpty()}")
```

Implements `Iterator[T]`, `Display`.

#### Range.start

```tondo
pub fn Range[T].start(self): T
```

The first value it yields, unless it is empty.

#### Range.end

```tondo
pub fn Range[T].end(self): T
```

The value where it stops: the last it yields with `..=`, the one after
it with `..`.

#### Range.isInclusive

```tondo
pub fn Range[T].isInclusive(self): Bool
```

Whether it was written `..=`, and yields its `end`.

#### Range.isEmpty

```tondo
pub fn Range[T: Ordered].isEmpty(self): Bool
```

Whether it yields no value.

#### Range.contains

```tondo
pub fn Range[T: Ordered].contains(self, value: T): Bool
```

Whether `value` is one the range still yields.

### Iterator

```tondo
pub trait Iterator[T] {
    fn map[U](self, transform: fn(T): U): MapIterator[Self, T, U]

    fn filter(self, keep: fn(T): Bool): FilterIterator[Self, T]

    /// The first `count` elements.
    fn take(self, count: Int): TakeIterator[Self, T]

    /// The elements after the first `count`.
    fn skip(self, count: Int): SkipIterator[Self, T]

    /// The elements until the first that fails the test.
    fn takeWhile(self, predicate: fn(T): Bool): TakeWhileIterator[Self, T]

    /// The elements from the first that fails the test.
    fn skipWhile(self, predicate: fn(T): Bool): SkipWhileIterator[Self, T]

    /// Each element with its position.
    fn enumerate(self): EnumerateIterator[Self, T]

    /// Pairs of elements from both, as long as both have one.
    fn zip[U, J: Iterator[U]](self, other: J): ZipIterator[Self, J, T, U]

    /// The elements, then those of `other`.
    fn chain[J: Iterator[T]](self, other: J): ChainIterator[Self, J, T]

    /// The elements of the array each element gives, in order.
    fn flatMap[U](self, transform: fn(T): Array[U]): FlatMapIterator[Self, T, U]

    fn collect(self): Array[T]

    fn count(self): Int

    fn any(self, predicate: fn(T): Bool): Bool

    fn all(self, predicate: fn(T): Bool): Bool

    fn find(self, predicate: fn(T): Bool): T?

    fn reduce[A](self, initial: A, combine: fn(A, T): A): A

    fn forEach(self, action: fn(T))
}
```

What every iterator can do besides `next`. The adapters are lazy: they
take elements from their source only as they are asked for theirs. Each
method works on a copy of the iterator, so its type is `Copy`.

```tondo
import std.console

/// The numbers of Fibonacci, as many as asked for: an iterator of its own,
/// which every adapter takes.
pub type Fibonacci = { a: Int, b: Int }

impl Iterator[Int] for Fibonacci {
    fn next(mut self): Int? {
        let current = self.a
        self.a = self.b
        self.b = current + self.b
        some(current)
    }
}

let evens = Fibonacci { a: 0, b: 1 }.filter((n) { n % 2 == 0 }).take(5).collect()
console.println("{evens}")
```

### For other types

```tondo
impl[I: Iterator[T], T, U] Iterator[U] for MapIterator[I, T, U]
impl[I: Iterator[T], T] Iterator[T] for FilterIterator[I, T]
impl[I: Iterator[T], T] Iterator[T] for TakeIterator[I, T]
impl[I: Iterator[T], T] Iterator[T] for SkipIterator[I, T]
impl[I: Iterator[T], T] Iterator[T] for TakeWhileIterator[I, T]
impl[I: Iterator[T], T] Iterator[T] for SkipWhileIterator[I, T]
impl[I: Iterator[T], T] Iterator[(Int, T)] for EnumerateIterator[I, T]
impl[I: Iterator[T], J: Iterator[U], T, U] Iterator[(T, U)] for ZipIterator[I, J, T, U]
impl[I: Iterator[T], J: Iterator[T], T] Iterator[T] for ChainIterator[I, J, T]
impl[I: Iterator[T], T, U] Iterator[U] for FlatMapIterator[I, T, U]
impl[T: Discrete + Ordered] Iterator[T] for Range[T]
impl Iterator[Char] for CharIterator
impl[T] Iterator[T] for ArrayIterator[T]
impl[K: Key, V] Iterator[K] for MapKeys[K, V]
impl[K: Key, V] Iterator[V] for MapValues[K, V]
impl[K: Key, V] Iterator[(K, V)] for MapEntries[K, V]
impl[T: Key] Iterator[T] for SetValues[T]
```
