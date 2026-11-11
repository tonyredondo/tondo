# Set and Deque

`Set[T]` holds each value once, in the order they were added. `Deque[T]` is a queue with two ends.

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api Set`, `tondo api Deque` show the same in a terminal.

- [Set](#set): [`Set.union`](#setunion), [`Set.intersection`](#setintersection), [`Set.difference`](#setdifference), [`Set.isSubset`](#setissubset), [`Set.length`](#setlength), [`Set.contains`](#setcontains), [`Set.insert`](#setinsert), [`Set.remove`](#setremove), [`Set.values`](#setvalues)
- [Deque](#deque): [`Deque.contains`](#dequecontains), [`Deque.length`](#dequelength), [`Deque.get`](#dequeget), [`Deque.push`](#dequepush), [`Deque.pushFront`](#dequepushfront), [`Deque.pop`](#dequepop), [`Deque.popFront`](#dequepopfront)

### Set

#### Set.union

```tondo
pub fn Set[T: Key].union(self, other: Set[T]): Set[T]
```

The values in either set.

#### Set.intersection

```tondo
pub fn Set[T: Key].intersection(self, other: Set[T]): Set[T]
```

The values in both sets.

#### Set.difference

```tondo
pub fn Set[T: Key].difference(self, other: Set[T]): Set[T]
```

The values of this set that are not in `other`.

#### Set.isSubset

```tondo
pub fn Set[T: Key].isSubset(self, other: Set[T]): Bool
```

Whether every element is also in `other`.

#### Set.length

```tondo
pub fn Set.length(self): Int
```

The number of values.

#### Set.contains

```tondo
pub fn Set.contains(self, value: T): Bool
```

Whether the set has the value.

#### Set.insert

```tondo
pub fn Set.insert(var self, value: T): Bool
```

Adds the value: `true` when it was new, `false` when the set had it.

#### Set.remove

```tondo
pub fn Set.remove(var self, value: T): Bool
```

Removes the value: `true` when the set had it.

#### Set.values

```tondo
pub fn Set.values(self): SetValues[T]
```

The values, as an iterator in insertion order: `collect()` makes an array of them.

### Deque

#### Deque.contains

```tondo
pub fn Deque[T: Equatable].contains(self, value: T): Bool
```

Whether an element equals `value`.

#### Deque.length

```tondo
pub fn Deque.length(self): Int
```

The number of elements.

#### Deque.get

```tondo
pub fn Deque.get(self, index: Int): T?
```

The element at `index` from the front, counting from the back when it is
negative, or `none` outside the deque.

#### Deque.push

```tondo
pub fn Deque.push(var self, value: T)
```

Adds the value at the back.

#### Deque.pushFront

```tondo
pub fn Deque.pushFront(var self, value: T)
```

Adds the value at the front.

#### Deque.pop

```tondo
pub fn Deque.pop(var self): T?
```

Removes the element at the back and gives it, or `none` when the deque
is empty.

#### Deque.popFront

```tondo
pub fn Deque.popFront(var self): T?
```

Removes the element at the front and gives it, or `none` when the deque
is empty.
