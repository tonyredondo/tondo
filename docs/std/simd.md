# std.simd

vectors of numbers, each lane computed at once by the processor's vector instructions. Vectors of 128, 256 and 512 bits, and `Vector[T]`, the widest that is accelerated where the code runs, give the same results everywhere: a width the processor does not do is computed on halves of 128 bits.

```tondo
import std.console
import std.simd

fn sumOfSquares(values: Array[Float32]): Float32 {
    let lanes = simd.Vector[Float32].lanes()
    var total = simd.Vector[Float32].zero()
    var at = 0
    for at + lanes <= values.length() {
        let chunk = simd.Vector[Float32].load(ref values, at: at)
        total = total + chunk * chunk
        at += lanes
    }
    var rest = total.sum()
    for value in values[at:] {
        rest += value * value
    }
    rest
}

fn main() {
    console.println("{sumOfSquares([1.0, 2.0, 3.0, 4.0, 5.0])}")
}
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.simd` shows the same in a terminal.

- [Vector128](#vector128): [`Vector128.zero`](#vector128zero), [`Vector128.all`](#vector128all), [`Vector128.of`](#vector128of), [`Vector128.load`](#vector128load), [`Vector128.lanes`](#vector128lanes), [`Vector128.accelerated`](#vector128accelerated), [`Vector128.lane`](#vector128lane), [`Vector128.withLane`](#vector128withlane), [`Vector128.toArray`](#vector128toarray), [`Vector128.store`](#vector128store), [`Vector128.min`](#vector128min), [`Vector128.max`](#vector128max), [`Vector128.abs`](#vector128abs), [`Vector128.saturatingAdd`](#vector128saturatingadd), [`Vector128.saturatingSub`](#vector128saturatingsub), [`Vector128.sqrt`](#vector128sqrt), [`Vector128.floor`](#vector128floor), [`Vector128.ceil`](#vector128ceil), [`Vector128.round`](#vector128round), [`Vector128.truncate`](#vector128truncate), [`Vector128.fma`](#vector128fma), [`Vector128.equal`](#vector128equal), [`Vector128.notEqual`](#vector128notequal), [`Vector128.less`](#vector128less), [`Vector128.lessOrEqual`](#vector128lessorequal), [`Vector128.greater`](#vector128greater), [`Vector128.greaterOrEqual`](#vector128greaterorequal), [`Vector128.sum`](#vector128sum), [`Vector128.minLane`](#vector128minlane), [`Vector128.maxLane`](#vector128maxlane), [`Vector128.reversed`](#vector128reversed), [`Vector128.swizzle`](#vector128swizzle), [`Vector128.to`](#vector128to), [`Vector128.cast`](#vector128cast)
- [Vector256](#vector256): [`Vector256.joined`](#vector256joined), [`Vector256.lower`](#vector256lower), [`Vector256.upper`](#vector256upper)
- [Vector512](#vector512)
- [Vector](#vector)
- [Mask128](#mask128): [`Mask128.choose`](#mask128choose), [`Mask128.any`](#mask128any), [`Mask128.all`](#mask128all), [`Mask128.count`](#mask128count), [`Mask128.bits`](#mask128bits), [`Mask128.lane`](#mask128lane)
- [Mask256](#mask256)
- [Mask512](#mask512)
- [Mask](#mask)

### Vector128

```tondo
pub type Vector128[T]
```

A vector of 128 bits: as many lanes of `T` as fit, `Int8`, `Int16`,
`Int32`, `Int`, `UInt8`, `UInt16`, `UInt32`, `UInt`, `Byte`, `Float32` or
`Float`. It is a value, shown as its lanes: `<1.0, 2.0, 3.0, 4.0>`.
`+`, `-` and `*` work lane by lane, with a `T` on either side for every
lane, `/` on floats, and `&`, `|`, `^`, `~`, `<<` and `>>` on integers and
bytes. An integer lane that overflows wraps.

```tondo
import std.console
import std.simd

let a = simd.Vector128[Float32].of(1.0, 2.0, 3.0, 4.0)
console.println("{a * 2.0 + 1.0} {a.sum()} {simd.Vector128[Float32].lanes()}")
```

#### Vector128.zero

```tondo
pub fn Vector128.zero(): Vector128[T]
```

A vector of zeros.

#### Vector128.all

```tondo
pub fn Vector128.all(value: T): Vector128[T]
```

A vector with `value` in every lane.

#### Vector128.of

```tondo
pub fn Vector128.of(values: ...T): Vector128[T]
```

A vector of these values, one for each lane; another count is an error
where the values are written, and a panic for an array spread.

#### Vector128.load

```tondo
pub fn Vector128.load(values: ref Array[T], at: Int = 0): Vector128[T]
```

The lanes from `values[at]` on; past the end of the array, a panic of an
index out of range (`P0001`).

#### Vector128.lanes

```tondo
pub fn Vector128.lanes(): Int
```

How many lanes it has, known when compiling.

#### Vector128.accelerated

```tondo
pub fn Vector128.accelerated(): Bool
```

Whether its operations are done with instructions of its width where the
code runs, known when compiling.

#### Vector128.lane

```tondo
pub fn Vector128.lane(self, index: Int): T
```

The lane `index`; one that is not a lane panics (`P0001`).

#### Vector128.withLane

```tondo
pub fn Vector128.withLane(self, index: Int, value: T): Vector128[T]
```

A copy with `value` in lane `index`.

#### Vector128.toArray

```tondo
pub fn Vector128.toArray(self): Array[T]
```

Its lanes, in order.

#### Vector128.store

```tondo
pub fn Vector128.store(self, into: mut Array[T], at: Int = 0)
```

Writes its lanes into `into` from `at`; past the end, a panic (`P0001`).

#### Vector128.min

```tondo
pub fn Vector128.min(self, other: Vector128[T]): Vector128[T]
```

The lesser of each pair of lanes; of floats, NaN when either is, and
`-0.0` below `+0.0`.

#### Vector128.max

```tondo
pub fn Vector128.max(self, other: Vector128[T]): Vector128[T]
```

The greater of each pair of lanes; of floats, NaN when either is.

#### Vector128.abs

```tondo
pub fn Vector128.abs(self): Vector128[T]
```

Each lane's magnitude; the least integer wraps to itself.

#### Vector128.saturatingAdd

```tondo
pub fn Vector128.saturatingAdd(self, other: Vector128[T]): Vector128[T]
```

The sums of integer lanes, held at the least or the greatest value.

#### Vector128.saturatingSub

```tondo
pub fn Vector128.saturatingSub(self, other: Vector128[T]): Vector128[T]
```

The differences of integer lanes, held at the least or the greatest
value.

#### Vector128.sqrt

```tondo
pub fn Vector128.sqrt(self): Vector128[T]
```

Each float lane's square root; NaN for a negative one.

#### Vector128.floor

```tondo
pub fn Vector128.floor(self): Vector128[T]
```

Each float lane rounded down.

#### Vector128.ceil

```tondo
pub fn Vector128.ceil(self): Vector128[T]
```

Each float lane rounded up.

#### Vector128.round

```tondo
pub fn Vector128.round(self): Vector128[T]
```

Each float lane to the nearest whole number, ties to even.

#### Vector128.truncate

```tondo
pub fn Vector128.truncate(self): Vector128[T]
```

Each float lane toward zero.

#### Vector128.fma

```tondo
pub fn Vector128.fma(self, factor: Vector128[T], addend: Vector128[T]): Vector128[T]
```

`self * factor + addend` of float lanes, rounded once.

#### Vector128.equal

```tondo
pub fn Vector128.equal(self, other: Vector128[T]): Mask128[T]
```

Where the lanes are equal.

#### Vector128.notEqual

```tondo
pub fn Vector128.notEqual(self, other: Vector128[T]): Mask128[T]
```

Where the lanes differ; a NaN lane differs.

#### Vector128.less

```tondo
pub fn Vector128.less(self, other: Vector128[T]): Mask128[T]
```

Where `self`'s lane is less; a NaN lane is not.

#### Vector128.lessOrEqual

```tondo
pub fn Vector128.lessOrEqual(self, other: Vector128[T]): Mask128[T]
```

Where `self`'s lane is less or equal.

#### Vector128.greater

```tondo
pub fn Vector128.greater(self, other: Vector128[T]): Mask128[T]
```

Where `self`'s lane is greater.

#### Vector128.greaterOrEqual

```tondo
pub fn Vector128.greaterOrEqual(self, other: Vector128[T]): Mask128[T]
```

Where `self`'s lane is greater or equal.

#### Vector128.sum

```tondo
pub fn Vector128.sum(self): T
```

The sum of its lanes, by halves: lane `j` with lane `j + lanes / 2`,
until one is left. Integers wrap.

#### Vector128.minLane

```tondo
pub fn Vector128.minLane(self): T
```

The least of its lanes.

#### Vector128.maxLane

```tondo
pub fn Vector128.maxLane(self): T
```

The greatest of its lanes.

#### Vector128.reversed

```tondo
pub fn Vector128.reversed(self): Vector128[T]
```

Its lanes in the opposite order.

#### Vector128.swizzle

```tondo
pub fn Vector128.swizzle(self, indexes: Vector128[T]): Vector128[T]
```

Of a vector of bytes, `Byte`, `UInt8` or `Int8`: in each lane, the lane
of `self` that `indexes` names there, and zero for an index of 16 or more.

#### Vector128.to

```tondo
pub fn Vector128.to[U](self): Vector128[U]
```

Each lane's value as a `U` as wide: between `Int32`, `UInt32` and
`Float32`, and between `Int`, `UInt` and `Float`. A float becomes the
nearest integer toward zero, the greatest or the least when it does not
fit, and zero for NaN; an integer that does not fit, the greatest or the
least.

#### Vector128.cast

```tondo
pub fn Vector128.cast[U](self): Vector128[U]
```

The same bits, as lanes of `U`.

### Vector256

```tondo
pub type Vector256[T]
```

A vector of 256 bits, with the functions and methods of `Vector128` at its
width, and `joined`, `lower` and `upper`. On an x64 with AVX2 it is done
with the processor's instructions of 256 bits: in a build for that level,
as `--cpu x86-64-v3`, and in a release build for a lower one, whose
functions that use it get a version for it. Elsewhere it is computed on
halves of 128 bits.

```tondo
import std.console
import std.simd

let a = simd.Vector256[Int32].all(3)
console.println("{a.lower() + a.upper()} {simd.Vector256[Int32].accelerated()}")
```

#### Vector256.joined

```tondo
pub fn Vector256.joined(lower: Vector128[T], upper: Vector128[T]): Vector256[T]
```

The vector of 256 bits whose lanes are `lower`'s, then `upper`'s.

#### Vector256.lower

```tondo
pub fn Vector256.lower(self): Vector128[T]
```

Its lanes from 0 to half of them.

#### Vector256.upper

```tondo
pub fn Vector256.upper(self): Vector128[T]
```

Its lanes from half of them on.

### Vector512

```tondo
pub type Vector512[T]
```

A vector of 512 bits, with the functions and methods of `Vector128` at its
width, and `joined`, `lower` and `upper`.

```tondo
import std.console
import std.simd

console.println("{simd.Vector512[Byte].lanes()}")
```

### Vector

```tondo
pub type Vector[T]
```

The vector meant to be the widest that is accelerated where the code
runs, with the functions and methods of `Vector128` but `of`: for now of
128 bits at every level, in the versions of a function for a higher level
too. Its result can differ by its width, as `sum` groups the lanes by it,
and `all` or `load` make one, as the values written for one width would
not fill another.

```tondo
import std.console
import std.simd

console.println("{simd.Vector[Float32].lanes() >= 4}")
```

### Mask128

```tondo
pub type Mask128[T]
```

A `Bool` for each lane of a `Vector128[T]`, as a comparison gives it;
shown as `<true, false, ...>`. Masks have `&`, `|`, `^` and `~`.

```tondo
import std.console
import std.simd

let a = simd.Vector128[Int32].of(1, 5, 2, 8)
let big = a.greater(simd.Vector128[Int32].all(3))
console.println("{big} {big.count()} {big.choose(a, -a)}")
```

#### Mask128.choose

```tondo
pub fn Mask128.choose(self, whenTrue: Vector128[T], whenFalse: Vector128[T]): Vector128[T]
```

In each lane, `whenTrue`'s where the mask is `true`, and `whenFalse`'s
where it is not.

#### Mask128.any

```tondo
pub fn Mask128.any(self): Bool
```

Whether any lane is `true`.

#### Mask128.all

```tondo
pub fn Mask128.all(self): Bool
```

Whether every lane is `true`.

#### Mask128.count

```tondo
pub fn Mask128.count(self): Int
```

How many lanes are `true`.

#### Mask128.bits

```tondo
pub fn Mask128.bits(self): UInt
```

Bit `i` set where lane `i` is `true`.

#### Mask128.lane

```tondo
pub fn Mask128.lane(self, index: Int): Bool
```

The lane `index`.

### Mask256

```tondo
pub type Mask256[T]
```

The mask of a `Vector256[T]`.

```tondo
import std.console
import std.simd

let a = simd.Vector256[Int32].all(1)
console.println("{a.equal(a).all()}")
```

### Mask512

```tondo
pub type Mask512[T]
```

The mask of a `Vector512[T]`.

```tondo
import std.console
import std.simd

let a = simd.Vector512[Int32].all(1)
console.println("{a.less(a).any()}")
```

### Mask

```tondo
pub type Mask[T]
```

The mask of a `Vector[T]`.

```tondo
import std.console
import std.simd

let a = simd.Vector[Float].all(1.0)
console.println("{a.lessOrEqual(a).bits() > 0}")
```
