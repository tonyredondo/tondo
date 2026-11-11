# Array

`Array[T]` is a sequence that grows, and a value: assigning one copies it.

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api Array` shows the same in a terminal.

- [Array](#array): [`Array.first`](#arrayfirst), [`Array.last`](#arraylast), [`Array.map`](#arraymap), [`Array.filter`](#arrayfilter), [`Array.flatMap`](#arrayflatmap), [`Array.reduce`](#arrayreduce), [`Array.contains`](#arraycontains), [`Array.any`](#arrayany), [`Array.all`](#arrayall), [`Array.find`](#arrayfind), [`Array.findIndex`](#arrayfindindex), [`Array.count`](#arraycount), [`Array.sum`](#arraysum), [`Array.min`](#arraymin), [`Array.max`](#arraymax), [`Array.sorted`](#arraysorted), [`Array.sortedBy`](#arraysortedby), [`Array.sortedWith`](#arraysortedwith), [`Array.reversed`](#arrayreversed), [`Array.replaced`](#arrayreplaced), [`Array.unique`](#arrayunique), [`Array.take`](#arraytake), [`Array.drop`](#arraydrop), [`Array.zip`](#arrayzip), [`Array.enumerate`](#arrayenumerate), [`Array.chunks`](#arraychunks), [`Array.windows`](#arraywindows), [`Array.groupBy`](#arraygroupby), [`Array.partition`](#arraypartition), [`Array.join`](#arrayjoin), [`Array.sort`](#arraysort), [`Array.sortBy`](#arraysortby), [`Array.reverse`](#arrayreverse), [`Array.swap`](#arrayswap), [`Array.retain`](#arrayretain), [`Array.extend`](#arrayextend), [`Array.iter`](#arrayiter), [`Array.length`](#arraylength), [`Array.isEmpty`](#arrayisempty), [`Array.get`](#arrayget), [`Array.slice`](#arrayslice), [`Array.concat`](#arrayconcat), [`Array.repeat`](#arrayrepeat), [`Array.push`](#arraypush), [`Array.pop`](#arraypop), [`Array.insert`](#arrayinsert), [`Array.removeAt`](#arrayremoveat), [`Array.clear`](#arrayclear)

### Array

#### Array.first

```tondo
pub fn Array[T].first(self): T?
```

The first element, or `none` when the array is empty.

#### Array.last

```tondo
pub fn Array[T].last(self): T?
```

The last element, or `none` when the array is empty.

#### Array.map

```tondo
pub fn Array[T].map[U](self, transform: fn(T): U): Array[U]
```

The function's result for each element, in order.

#### Array.filter

```tondo
pub fn Array[T].filter(self, keep: fn(T): Bool): Array[T]
```

The elements that pass the test, in order.

#### Array.flatMap

```tondo
pub fn Array[T].flatMap[U](self, transform: fn(T): Array[U]): Array[U]
```

The elements of the arrays the function gives for each element, in
order.

#### Array.reduce

```tondo
pub fn Array[T].reduce[A](self, initial: A, combine: fn(A, T): A): A
```

The elements combined from `initial`, in order.

#### Array.contains

```tondo
pub fn Array[T: Equatable].contains(self, value: T): Bool
```

Whether an element equals `value`, as `value in array`.

#### Array.any

```tondo
pub fn Array[T].any(self, predicate: fn(T): Bool): Bool
```

Whether some element passes the test; `false` for an empty array.

#### Array.all

```tondo
pub fn Array[T].all(self, predicate: fn(T): Bool): Bool
```

Whether every element passes the test; `true` for an empty array.

#### Array.find

```tondo
pub fn Array[T].find(self, predicate: fn(T): Bool): T?
```

The first element that passes the test.

#### Array.findIndex

```tondo
pub fn Array[T].findIndex(self, predicate: fn(T): Bool): Int?
```

The index of the first element that passes the test.

#### Array.count

```tondo
pub fn Array[T].count(self, predicate: fn(T): Bool): Int
```

How many elements pass the test.

#### Array.sum

```tondo
pub fn Array[T: Additive].sum(self): T
```

The sum of the elements; zero for an empty array.

#### Array.min

```tondo
pub fn Array[T: Ordered].min(self): T?
```

The smallest element, the first of equal ones, or `none` when the array
is empty.

#### Array.max

```tondo
pub fn Array[T: Ordered].max(self): T?
```

The largest element, the first of equal ones, or `none` when the array
is empty.

#### Array.sorted

```tondo
pub fn Array[T: Ordered].sorted(self): Array[T]
```

The elements in order, equal ones as they were.

#### Array.sortedBy

```tondo
pub fn Array[T].sortedBy[K: Ordered](self, key: fn(T): K): Array[T]
```

The elements in the order of their keys, equal ones as they were.

#### Array.sortedWith

```tondo
pub fn Array[T].sortedWith(self, compare: fn(T, T): Int): Array[T]
```

The elements in the order `compare` gives, equal ones as they were.

#### Array.reversed

```tondo
pub fn Array[T].reversed(self): Array[T]
```

The elements in the opposite order.

#### Array.replaced

```tondo
pub fn Array[T].replaced(self, index: Int, item: T): Array[T]
```

A copy with the element at `index` replaced by `item`; the array itself
stays as it was. An index out of range panics, as `items[index] = item`
does.

```tondo
import std.console

let tasks = ["idle", "idle"]
console.println("{tasks.replaced(1, "runs")} {tasks}")
```

#### Array.unique

```tondo
pub fn Array[T: Key].unique(self): Array[T]
```

The elements without repeats, each where it first appears.

#### Array.take

```tondo
pub fn Array[T].take(self, count: Int): Array[T]
```

The first `count` elements.

#### Array.drop

```tondo
pub fn Array[T].drop(self, count: Int): Array[T]
```

The elements after the first `count`.

#### Array.zip

```tondo
pub fn Array[T].zip[U](self, other: Array[U]): Array[(T, U)]
```

Pairs of elements at the same index, as many as the shorter has.

#### Array.enumerate

```tondo
pub fn Array[T].enumerate(self): Array[(Int, T)]
```

Each element with its index.

#### Array.chunks

```tondo
pub fn Array[T].chunks(self, size: Int): Array[Array[T]]
```

The elements in groups of `size`; the last may be smaller.

#### Array.windows

```tondo
pub fn Array[T].windows(self, size: Int): Array[Array[T]]
```

Every run of `size` consecutive elements.

#### Array.groupBy

```tondo
pub fn Array[T].groupBy[K: Key](self, key: fn(T): K): Map[K, Array[T]]
```

The elements by key, in the order the keys first appear.

#### Array.partition

```tondo
pub fn Array[T].partition(self, predicate: fn(T): Bool): (Array[T], Array[T])
```

The elements that pass the test, and those that do not.

#### Array.join

```tondo
pub fn Array[T: Display].join(self, separator: String): String
```

The elements as text, with `separator` between them.

#### Array.sort

```tondo
pub fn Array[T: Ordered].sort(mut self)
```

Puts the elements in order, equal ones as they were.

#### Array.sortBy

```tondo
pub fn Array[T].sortBy[K: Ordered](mut self, key: fn(T): K)
```

Puts the elements in the order of their keys.

#### Array.reverse

```tondo
pub fn Array[T].reverse(mut self)
```

Puts the elements in the opposite order.

#### Array.swap

```tondo
pub fn Array[T].swap(mut self, i: Int, j: Int)
```

Exchanges the elements at `i` and `j`; an index out of the array
panics.

#### Array.retain

```tondo
pub fn Array[T].retain(var self, keep: fn(T): Bool)
```

Keeps only the elements that pass the test.

#### Array.extend

```tondo
pub fn Array[T].extend(var self, other: Array[T])
```

Adds the elements of `other` at the end.

#### Array.iter

```tondo
pub fn Array[T].iter(self): ArrayIterator[T]
```

The elements, one after another: the lazy adapters of `Iterator` make
no array until `collect()`.

#### Array.length

```tondo
pub fn Array.length(self): Int
```

The number of elements.

#### Array.isEmpty

```tondo
pub fn Array.isEmpty(self): Bool
```

Whether the array has no elements.

#### Array.get

```tondo
pub fn Array.get(self, index: Int): T?
```

The element at `index`, counting from the end when it is negative, or
`none` outside the array.

#### Array.slice

```tondo
pub fn Array.slice(self, start: Int, end: Int): Array[T] ! collections.CollectionError
```

The elements from `start` up to `end`.

#### Array.concat

```tondo
pub fn Array.concat(self, other: Array[T]): Array[T]
```

The elements of this array, then those of `other`.

#### Array.repeat

```tondo
pub fn Array.repeat(self, count: Int): Array[T]
```

The elements written `count` times in a row; a negative `count` panics.

#### Array.push

```tondo
pub fn Array.push(var self, value: T)
```

Adds the value at the end.

#### Array.pop

```tondo
pub fn Array.pop(var self): T?
```

Removes the last element and gives it, or `none` when the array is
empty.

#### Array.insert

```tondo
pub fn Array.insert(var self, index: Int, value: T)
```

Puts the value at `index`, counting from the end when it is negative,
and the elements from there one further; `index` may be the length, and
beyond it panics.

#### Array.removeAt

```tondo
pub fn Array.removeAt(var self, index: Int): T?
```

Removes the element at `index`, counting from the end when it is
negative, and gives it, or `none` outside the array.

#### Array.clear

```tondo
pub fn Array.clear(var self)
```

Removes every element.
