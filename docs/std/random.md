# std.random

pseudo-random values from a generator, seeded for a sequence that repeats or from the system's entropy. The generator is not for secrets: keys and tokens take `random.entropy`.

```tondo
import std.console
import std.random

var dice = random.Random.seeded(42)
console.println("{dice.int(1..=6)} {dice.choice(["red", "green", "blue"])}")
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.random` shows the same in a terminal.

- [Functions](#functions): [`entropy`](#entropy)
- [Random](#random): [`Random.seeded`](#randomseeded), [`Random`](#random-1), [`Random.int`](#randomint), [`Random.float`](#randomfloat), [`Random.bool`](#randombool), [`Random.bytes`](#randombytes), [`Random.shuffled`](#randomshuffled), [`Random.choice`](#randomchoice)

### Functions

#### entropy

```tondo
pub fn entropy(count: Int): Array[Byte]
```

`count` bytes from the system's entropy, for keys, tokens and seeds.

### Random

```tondo
pub type Random = {
    // private fields
}
```

A generator of pseudo-random values: SplitMix64 from a seed. The same
seed gives the same values on every machine.

```tondo
import std.console
import std.random

// The same seed gives the same values; `Random()` a different seed.
var generator = random.Random.seeded(7)
console.println("{generator.int(1..=100)} {generator.float()} {generator.shuffled([1, 2, 3])}")
```

#### Random.seeded

```tondo
pub fn Random.seeded(seed: Int): Random
```

A generator whose values the seed fixes.

#### Random

```tondo
pub fn Random(): Random
```

A generator seeded from the system's entropy: other values each run.

#### Random.int

```tondo
pub fn Random.int[T: Integer](mut self, range: Range[T]): T
```

An integer of the range, each as likely, of any width:
`random.int(1..=6)`, `random.int(1u64..=UInt64.max)`. An empty range
panics.

#### Random.float

```tondo
pub fn Random.float(mut self): Float
```

A float in `[0, 1)`, each of its 2^53 values as likely.

#### Random.bool

```tondo
pub fn Random.bool(mut self): Bool
```

`true` or `false`, each as likely.

#### Random.bytes

```tondo
pub fn Random.bytes(mut self, count: Int): Array[Byte]
```

`count` random bytes.

#### Random.shuffled

```tondo
pub fn Random.shuffled[T](mut self, items: Array[T]): Array[T]
```

The items in an order where each is as likely (Fisher-Yates).

#### Random.choice

```tondo
pub fn Random.choice[T](mut self, items: Array[T]): T?
```

One of the items, each as likely, or `none` when there are none.
