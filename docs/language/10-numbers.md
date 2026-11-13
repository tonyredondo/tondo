# Numbers

Numbers in Tondo do what arithmetic says or stop the program: an integer
that overflows is a panic, in every build, never a wrong value that goes on.
This chapter covers the integers and floats, what each operation does at its
edges, the operations that have names, exact decimals and integers of any
size, and how a type of the program gets arithmetic.

- [Integers](#integers)
- [Overflow](#overflow)
- [Division and remainder](#division-and-remainder)
- [Wrapping, checking and saturating](#wrapping-checking-and-saturating)
- [Bits and bytes](#bits-and-bytes)
- [Floats](#floats)
- [Mixing numbers](#mixing-numbers)
- [Mathematics](#mathematics)
- [Vectors](#vectors)
- [Tensors](#tensors)
- [Exact decimals and big integers](#exact-decimals-and-big-integers)
- [Arithmetic for your types](#arithmetic-for-your-types)
- [Code for any number](#code-for-any-number)
- [Random numbers](#random-numbers)

## Integers

`Int` is the integer to use: 64 bits, signed, the same on every machine.
`UInt` is its unsigned twin. The others exist for when the width is part of
the job, as in a binary format or a call to C:

| Signed | Unsigned | Bits | From | To |
| --- | --- | ---: | --- | --- |
| `Int8` | `UInt8` | 8 | -128 | 127, or 255 unsigned |
| `Int16` | `UInt16` | 16 | -32 768 | 32 767, or 65 535 |
| `Int32` | `UInt32` | 32 | about -2.1 billion | 2.1 billion, or 4.2 |
| `Int`, `Int64` | `UInt`, `UInt64` | 64 | about -9.2 × 10¹⁸ | 9.2 × 10¹⁸, or 1.8 × 10¹⁹ |
| `Int128` | `UInt128` | 128 | about -1.7 × 10³⁸ | 1.7 × 10³⁸, or 3.4 × 10³⁸ |

Every width has the same operations, and 128 bits are as complete as 8.

```tondo
import std.console

fn main() {
    let big: Int128 = 170_141_183_460_469_231_731_687_303_715_884_105_727
    let id = 18_446_744_073_709_551_615u64
    console.println("{Int.max} {Int.min} {UInt8.max} {Int16.min} {Int.bits}")
    console.println("{big} {id} {2.pow(62)} {7.max(3)} {(-7).abs()} {15.clamp(0, 10)}")
}
```

```text output
9223372036854775807 -9223372036854775808 255 -32768 64
170141183460469231731687303715884105727 18446744073709551615 4611686018427387904 7 7 10
```

## Overflow

An operation whose result does not fit its type panics. It does so in a
release build exactly as in a debug one:

```tondo
import std.console

fn main() {
    let small: Int8 = 127
    let result = recover { small + 1 }
    match result {
        ok(value) => console.println("{value}")
        err(crash) => console.println("{crash.code}: {crash.message}")
    }
    let unsigned: UInt = 0
    console.println("{recover { unsigned - 1 }.isErr()} {recover { Int.min / -1 }.isErr()}")
}
```

```text output
P0005: 127 + 1 does not fit in Int8 (-128 to 127)
true true
```

This is a decision about what a program means. Wrapping silently turns a
count into a negative number three functions before anyone notices;
stopping at the addition points at the line. The checks cost little, and the
compiler drops the ones it can prove are not needed, as in a loop whose
bounds it knows: `tondo optimizations` says which checks a function keeps,
and where.

When overflow is expected, it is asked for by name, below.

## Division and remainder

```tondo
import std.console

fn main() {
    console.println("{7 / 3} {-7 / 3} {7 % 3} {-7 % 3} {7 / -3} {7 % -3}")
    console.println("{recover { 1 / 0 }.isErr()} {1.checkedDiv(0)} {17.checkedRem(5)}")
}
```

```text output
2 -2 1 -1 -2 1
true none some(2)
```

Integer division truncates toward zero, and the remainder has the sign of
the dividend, so `a == (a / b) * b + a % b` always holds. Dividing an integer
by zero panics.

## Wrapping, checking and saturating

Each arithmetic operation has four named forms, for when the edge is part of
the algorithm:

| Form | At the edge | Use it for |
| --- | --- | --- |
| `a.wrappingAdd(b)` | wraps around | hashes, checksums, counters that roll over |
| `a.checkedAdd(b)` | gives `none` | input that may be too big |
| `a.saturatingAdd(b)` | stays at the limit | volumes, scores, anything clamped |
| `a.overflowingAdd(b)` | gives the wrapped value and whether it overflowed | arithmetic in several words |

```tondo
import std.console

/// The FNV-1a hash of a text: multiplication that is meant to wrap.
fn fnv1a(text: String): UInt {
    var hash: UInt = 0xcbf29ce484222325
    for byte in text.bytes() {
        hash = (hash ^ UInt(byte)).wrappingMul(0x100000001b3)
    }
    hash
}

fn main() {
    let top = Int.max
    console.println("{top.wrappingAdd(1)} {top.checkedAdd(1)} {top.saturatingAdd(1)} {top.overflowingAdd(1)}")
    console.println("{200u8.wrappingAdd(100)} {200u8.checkedAdd(100)} {200u8.saturatingAdd(100)} {0u8.saturatingSub(1)}")
    console.println("{fnv1a("hello"):x}")
}
```

```text output
-9223372036854775808 none 9223372036854775807 (-9223372036854775808, true)
44 none 255 0
a430d84680aabd0b
```

The same four exist for `Sub`, `Mul` and `Pow`, with `checkedDiv`,
`checkedRem`, `checkedNeg` and `wrappingNeg`. Converting follows the same
idea: `UInt8.wrapping(300)` keeps the low bits and `UInt8.saturating(300)`
the nearest value, where `UInt8(300)` gives an error
([Values](02-values.md)).

## Bits and bytes

```tondo
import std.console

fn main() {
    let flags = 0b1011_0000
    console.println("{flags.countOnes()} {flags.leadingZeros()} {flags.trailingZeros()}")
    console.println("{flags & 0b1111_0000:b} {flags | 1:b} {flags ^ 0xff:b} {1 << 10} {flags >> 4}")
    console.println("{0x12u8.rotateLeft(4):x} {0x1234u16.reverseBytes():x} {1u8.reverseBits():b}")

    // A number as bytes and back, in the order a format asks for.
    let id: UInt32 = 0xCAFEBABE
    let bytes = id.toBytes()
    let little = id.toBytes(ByteOrder.Little)
    console.println("{bytes} {little} {UInt32.fromBytes(bytes):x}")
}
```

```text output
3 56 4
10110000 10110001 1001111 1024 11
21 3412 10000000
[202, 254, 186, 190] [190, 186, 254, 202] cafebabe
```

The bit operators are `&`, `|`, `^`, `~`, `<<` and `>>`, on integers of the
same type. A shift by a negative count, or by the width of the type or more,
panics. `>>` on a signed integer keeps the sign.

`toBytes` and `fromBytes` are how a number is written to a file or a packet
and read back: `UInt.fromBytes(bytes, at: 8)` reads one from the middle of
an array.

## Floats

`Float` is an IEEE 754 number of 64 bits, and `Float32` one of 32. They
follow the standard, which means a float never panics:

```tondo
import std.console

fn main() {
    console.println("{0.1 + 0.2} {0.1 + 0.2 == 0.3} {1.0 / 3.0}")
    let zero = 0.0
    let infinite = 1.0 / zero
    let undefined = zero / zero
    console.println("{infinite} {-infinite} {undefined} {undefined == undefined} {undefined.isNaN()}")
    console.println("{2.5.round()} {3.5.round()} {2.5.roundTiesAway()} {2.7.floor()} {2.2.ceil()} {(-2.7).truncate()}")
    console.println("{2.0.pow(10.0)} {(-3.5).abs()} {3.14159.toFixed(2)} {7.5.clamp(0.0, 5.0)} {1.5.min(2.5)}")
    console.println("{2.0.sqrt()} {(-1.0).sqrt()} {(-1.0).checkedSqrt().isErr()}")
}
```

```text output
0.30000000000000004 false 0.3333333333333333
inf -inf NaN false true
2.0 4.0 3.0 2.0 3.0 -2.0
1024.0 3.5 3.14 5.0 1.5
1.4142135623730951 NaN true
```

- Floats are approximate. `0.1 + 0.2` is not `0.3`: compare with a
  tolerance, `(a - b).abs() < 0.000001`, or use a `Decimal` when the number
  is money.
- Dividing by zero gives infinity or NaN, and NaN is equal to nothing, not
  even itself: test it with `isNaN()`. The square root of a negative number
  is NaN in the same way.
- `round()` rounds halves to the even neighbour, as IEEE does, and
  `roundTiesAway()` away from zero, as school does.
- A float is not a key of a map and has no `%`.
- The compiler never reorders float arithmetic or merges `a * b + c` into
  one instruction, because the result would change. `math.fma(a, b, c)` asks
  for it.
- A method binds tighter than a minus sign: `-2.5.abs()` is `-(2.5.abs())`,
  which is `-2.5`. Write `(-2.5).abs()`.

## Mixing numbers

The two operands of an operator have the same type. A narrower integer
widens to a wider one of its kind by itself, since nothing can be lost, and
nothing else is converted:

```tondo
import std.console

fn main() {
    let count: Int32 = 3
    let total: Int = 10
    let average = Float(total) / Float(count)
    console.println("{count + total} {average:.3} {Int(average.floor())}")
}
```

```text output
13 3.333 ok(3)
```

`total / 2.5` does not compile: an integer and a float do not mix, and the
error says to write `Float(total)`. This is deliberate. Whether a division
is whole or fractional is a decision, and it should be visible.

## Mathematics

`std.math` has the functions, as `tondo api std.math` lists:

```tondo
import std.console
import std.math

fn main() {
    console.println("{math.hypot(3.0, 4.0)} {math.pow(2.0, 0.5):.4} {math.exp(1.0):.4}")
    console.println("{math.sin(0.0)} {math.cos(0.0)} {math.atan2(1.0, 1.0):.4}")
    console.println("{math.sqrt(2.0)} {math.ln(-1.0)} {math.ln(0.0)} {math.log10(1000.0)} {math.checkedLn(-1.0).isErr()}")
    console.println("{math.gcd(12, 18)} {math.lcm(4, 6)} {math.dot([10.0, 20.0], [3.0, 1.0])}")
}
```

```text output
5.0 1.4142 2.7183
0.0 1.0 0.7854
1.4142135623730951 NaN -inf 3.0 true
6 12 50.0
```

A function that is not defined for every input, as `sqrt`, `ln` and `asin`,
follows the rest of the floats: outside its domain it gives NaN, or an
infinity where that is the limit, as `ln(0.0)`. Each has a twin that fails
with `math.MathError` instead, for code that would rather not find a NaN
later: `checkedSqrt`, `checkedLn`, `checkedLog10`, `checkedLog2`,
`checkedAsin` and `checkedAcos`.

`math.dot` is the sum of the products of two arrays of numbers. For floats
it adds as an array's `sum` does, in an order that is the same on every
machine ([Collections](08-collections.md#arithmetic-on-arrays)).

## Vectors

`std.simd` computes several numbers at once, with the processor's vector
instructions. A `simd.Vector128[Int32]` holds four `Int32`, its lanes, and
an operator works on each lane, with a number on either side standing for
every lane:

```tondo
import std.console
import std.simd

fn main() {
    let a = simd.Vector128[Int32].of(1, 2, 3, 4)
    let b = simd.Vector128[Int32].all(10)
    console.println("{a + b} {a * 2} {a.sum()} {a.maxLane()}")
    let big = a.greater(simd.Vector128[Int32].all(2))
    console.println("{big} {big.count()} {big.choose(a, -a)}")
}
```

```text output
<11, 12, 13, 14> <2, 4, 6, 8> 10 4
<false, false, true, true> 2 <-1, -2, 3, 4>
```

- The types are `Vector128[T]`, `Vector256[T]` and `Vector512[T]`, of that
  many bits, and `Vector[T]`, for code that does not choose a width, which
  is of 128 bits today. `T` is an integer of any size, `Byte`, `Float32` or
  `Float`, and a vector has as many lanes as fit: `Vector128[Float]` has
  two.
- Every type works on every processor and gives the same results. A width
  the processor has no instructions for is computed by its halves, and
  `accelerated()` says which case it is where the code runs. The 128-bit
  types are accelerated on both architectures, and the 256-bit ones on an
  x64 processor with AVX2, in a build for any level
  ([Performance](../toolchain/performance.md#many-numbers-at-once-vectors)).
- An integer lane that overflows wraps, where a number alone would panic:
  no operation on vectors panics for what a lane holds. Float lanes follow
  IEEE 754, each on its own.
- Between vectors, `<` and the like would give one `Bool`, so comparing is
  a method, `less`, `lessOrEqual`, `greater`, `greaterOrEqual` and `equal`,
  that gives a mask: a `Bool` for each lane. A mask has `any`, `all` and
  `count`, and `choose(a, b)` takes each lane from `a` where it is true and
  from `b` where it is not.
- `load(ref values, at: i)` takes its lanes from an array, and `store(mut
  into, at: i)` writes them back; `sum`, `minLane` and `maxLane` make one
  number of the lanes.

A loop over an array takes a vector at a time, and what is left one by one:

```tondo
import std.console
import std.simd

/// The sum of each value doubled.
fn doubled(values: Array[Float32]): Float32 {
    let lanes = simd.Vector[Float32].lanes()
    var total = simd.Vector[Float32].zero()
    var at = 0
    for at + lanes <= values.length() {
        total = total + simd.Vector[Float32].load(ref values, at: at) * 2.0
        at += lanes
    }
    var rest = total.sum()
    for value in values[at:] {
        rest += value * 2.0
    }
    rest
}

fn main() {
    let values: Array[Float32] = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0]
    console.println("{doubled(values)}")
}
```

```text output
90.0
```

Most code needs none of this: the arithmetic of arrays, their `sum` and
`math.dot` already compute with vectors
([Collections](08-collections.md#arithmetic-on-arrays)). `std.simd` is for
the loop those do not express, and [`std.simd`](../std/simd.md) lists every
operation.

## Tensors

`math.Tensor` holds numbers in several dimensions: a shape, and the values
in one block. A matrix is a tensor of two dimensions:

```tondo
import std.console
import std.math

fn main() {
    let weights = math.Tensor.fromRows([[0.5, 0.25], [0.125, 1.0]])
    let input = math.Tensor([1.0, 2.0])
    console.println("{weights.shape()} {weights.at(1, 0)} {weights.matmul(input)}")

    let table = math.Tensor.fromRows([[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]])
    let scale = math.Tensor([10.0, 100.0, 1000.0])
    console.println("{table * scale}")
    console.println("{table * 2.0 + 1.0} {table.sumAlong(0)} {table.sumAlong(1)}")
    console.println("{table.max()} {table.indexOfMax()} {table.transposed()}")
    console.println("{table.mean()} {table.meanAlong(0)} {table.meanAlong(1)}")
}
```

```text output
[2, 2] 0.125 [1.0, 2.125]
[[10.0, 200.0, 3000.0], [40.0, 500.0, 6000.0]]
[[3.0, 5.0, 7.0], [9.0, 11.0, 13.0]] [5.0, 7.0, 9.0] [6.0, 15.0]
some(6.0) some([1, 2]) [[1.0, 4.0], [2.0, 5.0], [3.0, 6.0]]
3.5 [2.5, 3.5, 4.5] [2.0, 5.0]
```

- `math.Tensor(values, shape)` makes one of an array and a shape, a vector
  when no shape is given; `fromRows` of arrays in arrays; `zeros` and
  `filled` of a shape alone. A tensor shows as arrays in arrays.
- `+`, `-`, `*` and `/` go element by element, between two tensors and with
  a number on either side. A dimension of size 1, or one the smaller tensor
  lacks, is repeated along the other's, as the row `scale` is for each row
  of `table`. Shapes that do not fit are a panic (`P0006`), as two arrays
  of different lengths are.
- `matmul` is the product of matrices, and `dot` that of two vectors;
  `transposed`, `reshape` and `slice(axis, index)` give the same values in
  another shape or a part of them; `sum` adds everything and
  `sumAlong(axis)` along one dimension, and `mean` and `meanAlong(axis)` are
  their means, for a tensor of floats: on one of integers they do not
  compile, and the error says to divide the `sum()` as floats.
- `at(1, 0)` reads an element and `set(value, 1, 0)` writes one; an index
  out of range is a panic (`P0001`). `min`, `max` and `indexOfMax` give
  options, since a tensor may have no elements.

## Exact decimals and big integers

A `Float` cannot hold `0.1`. Money, and anything else counted in decimals,
is a `math.Decimal`: exact, of any size, with the rounding said out loud.
`math.BigInt` is an integer with no limit.

```tondo
import std.console
import std.math

fn main() {
    let price: math.Decimal = 19.99
    let quantity: math.Decimal = 3
    let subtotal = price * quantity
    let tax = (subtotal * 0.21).round(2)
    console.println("{subtotal} {tax} {subtotal + tax} {subtotal < 60}")
    console.println("{price.divide(3, 4)} {price.round(1)} {price.round(1, math.Rounding.Down)} [{price:>10.3}]")

    let factorial: math.BigInt = 30
    var result: math.BigInt = 1
    var n: math.BigInt = 1
    for n <= factorial {
        result = result * n
        n = n + 1
    }
    console.println("{result} {math.BigInt.parse("ff", radix: 16)} {result.toInt()}")
}
```

```text output
59.97 12.59 72.56 true
6.6633 20 19.9 [    19.990]
265252859812191058636308480000000 ok(255) none
```

A literal where a `Decimal` is expected is exact: `19.99` never passes
through a float on its way. Division of decimals may not end, so `divide`
takes the number of decimals to keep and the way to round, half to even
unless told otherwise. Both types are `Number`s, so they have the operators,
`sum()`, and every format of the library reads and writes them.

## Arithmetic for your types

A type that adds implements `Additive`, and gets `+`, `-`, `+=` and
`values.sum()`. One that also multiplies and divides implements `Number`,
and gets `*`, `/` and integer literals. No other operator is open, so
arithmetic always means arithmetic.

```tondo
import std.console

type Vector = { x: Float, y: Float }

impl Additive for Vector {
    fn zero(): Vector {
        Vector { x: 0.0, y: 0.0 }
    }

    fn plus(self, other: Vector): Vector {
        Vector { x: self.x + other.x, y: self.y + other.y }
    }

    fn minus(self, other: Vector): Vector {
        Vector { x: self.x - other.x, y: self.y - other.y }
    }
}

impl Display for Vector {
    fn display(self): String {
        "({self.x}, {self.y})"
    }
}

fn main() {
    let a = Vector { x: 1.0, y: 2.0 }
    let b = Vector { x: 0.5, y: -1.0 }
    var position = a + b
    position += a
    console.println("{a + b} {a - b} {position} {[a, b, a].sum()}")
}
```

```text output
(1.5, 1.0) (0.5, 3.0) (2.5, 3.0) (2.5, 3.0)
```

`Additive` is `zero`, `plus` and `minus`. `Number` requires it and adds
`one`, `fromInt`, `times` and `dividedBy`. Ordering is a separate trait,
`Ordered`, whose one method, `compare`, gives `<`, `<=`, `>` and `>=`
([Generics and traits](11-generics.md)).

## Code for any number

A function can be written once for every number, with a bound that says how
much it needs:

```tondo
import std.console
import std.math

fn mean[T: Fractional](values: Array[T]): T {
    values.sum() / Number.fromInt[T](values.length())
}

/// Any integer in hexadecimal, with all its digits.
fn hex[T: Integer](value: T): String {
    value.toString(radix: 16).padStart(T.bits / 4, fill: "0")
}

fn main() {
    let prices: Array[math.Decimal] = [19.99, 5.01, 10]
    console.println("{mean([1.5, 2.5])} {mean(prices)}")
    console.println("{hex(255u8)} {hex(255u16)} {hex(255)}")
}
```

```text output
2.0 11.66666666666666666666666666666667
ff 00ff 00000000000000ff
```

| Bound | Is | Gives |
| --- | --- | --- |
| `T: Additive` | anything that adds | `+`, `-`, `sum()` |
| `T: Number` | anything that also multiplies | `*`, `/`, integer literals |
| `T: Integer` | every integer of the language | all an integer has: `%`, bits, `T.max`, `T.bits`, `T.parse` |
| `T: Fractional` | a number whose division keeps the fraction: every float, `math.Decimal` | a `/` that does not cut: `1.0 / 2.0` is `0.5` |

`mean` asks for a `Fractional` and not for any `Number`, since the division
of integers cuts: with `T: Number` the mean of `[1, 2]` would be 1. With the
bound, `mean([1, 2])` does not compile, and the error says what the function
asks.

There is no unary minus for a generic `T`: write `Number.fromInt[T](0) - x`.

## Random numbers

`std.random` gives generators. One made from a seed gives the same numbers
every time, which is what a test or a simulation wants; one made from the
system's entropy does not:

```tondo
import std.console
import std.random

fn main() {
    var seeded = random.Random.seeded(42)
    let dice = [seeded.int(1..=6), seeded.int(1..=6), seeded.int(1..=6)]
    var again = random.Random.seeded(42)
    console.println("{dice.length()} {dice[0] == again.int(1..=6)} {dice.all((n) { n >= 1 and n <= 6 })}")
    console.println("{seeded.shuffled([1, 2, 3, 4]).length()} {seeded.choice(["a", "b"]).isSome()} {seeded.float() < 1.0}")

    var fresh = random.Random()             // from the system's entropy
    console.println("{fresh.int(0..10) < 10} {random.entropy(16).length()}")
}
```

```text output
3 true true
4 true true
true 16
```

For secrets, keys and tokens, use `std.crypto`, not a generator
([its page](../std/crypto.md)).

Next: [Generics and traits](11-generics.md).
