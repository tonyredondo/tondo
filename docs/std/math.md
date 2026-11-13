# std.math

scalar math with IEEE-754 semantics and no fast-math, and integers of any size and exact decimals written in Tondo. An operation outside its domain gives `NaN` or an infinity, as `/` does, and its `checked` twin an error.

```tondo
import std.console
import std.math

let side = math.sqrt(3.0 * 3.0 + 4.0 * 4.0)
console.println("{side} {math.round(2.5)} {math.pow(2.0, 10.0)}")
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.math` shows the same in a terminal.

- [Functions](#functions): [`Pi`](#pi), [`E`](#e), [`sqrt`](#sqrt), [`checkedSqrt`](#checkedsqrt), [`pow`](#pow), [`exp`](#exp), [`ln`](#ln), [`checkedLn`](#checkedln), [`log10`](#log10), [`checkedLog10`](#checkedlog10), [`log2`](#log2), [`checkedLog2`](#checkedlog2), [`sin`](#sin), [`cos`](#cos), [`tan`](#tan), [`asin`](#asin), [`checkedAsin`](#checkedasin), [`acos`](#acos), [`checkedAcos`](#checkedacos), [`atan`](#atan), [`atan2`](#atan2), [`hypot`](#hypot), [`floor`](#floor), [`ceil`](#ceil), [`round`](#round), [`roundTiesAway`](#roundtiesaway), [`truncate`](#truncate), [`fma`](#fma), [`dot`](#dot), [`norm`](#norm), [`gcd`](#gcd), [`lcm`](#lcm)
- [MathError](#matherror)
- [BigInt](#bigint): [`BigInt.parse`](#bigintparse), [`BigInt.toInt`](#biginttoint), [`BigInt.toString`](#biginttostring), [`BigInt.abs`](#bigintabs), [`BigInt.remainder`](#bigintremainder), [`BigInt.pow`](#bigintpow), [`BigInt.modPow`](#bigintmodpow), [`BigInt.gcd`](#bigintgcd)
- [Decimal](#decimal): [`Decimal.parse`](#decimalparse), [`Decimal.round`](#decimalround), [`Decimal.divide`](#decimaldivide), [`Decimal.scale`](#decimalscale), [`Decimal.toFloat`](#decimaltofloat)
- [Rounding](#rounding)
- [Tensor](#tensor): [`Tensor`](#tensor-1), [`Tensor.fromRows`](#tensorfromrows), [`Tensor.zeros`](#tensorzeros), [`Tensor.filled`](#tensorfilled), [`Tensor.shape`](#tensorshape), [`Tensor.rank`](#tensorrank), [`Tensor.length`](#tensorlength), [`Tensor.values`](#tensorvalues), [`Tensor.at`](#tensorat), [`Tensor.set`](#tensorset), [`Tensor.slice`](#tensorslice), [`Tensor.reshape`](#tensorreshape), [`Tensor.transposed`](#tensortransposed), [`Tensor.map`](#tensormap), [`Tensor.sum`](#tensorsum), [`Tensor.sumAlong`](#tensorsumalong), [`Tensor.min`](#tensormin), [`Tensor.max`](#tensormax), [`Tensor.indexOfMax`](#tensorindexofmax), [`Tensor.dot`](#tensordot), [`Tensor.matmul`](#tensormatmul)

### Functions

#### Pi

```tondo
pub const Pi: Float = 3.141592653589793
```

π, the ratio of a circle's circumference to its diameter, as the
nearest `Float`.

#### E

```tondo
pub const E: Float = 2.718281828459045
```

e, the base of the natural logarithm, as the nearest `Float`.

#### sqrt

```tondo
pub fn sqrt(value: Float): Float
```

The square root, by IEEE 754: `NaN` for a negative value. `checkedSqrt`
gives an error instead.

#### checkedSqrt

```tondo
pub fn checkedSqrt(value: Float): Float ! MathError
```

The square root; a negative value is `MathError.Domain`.

#### pow

```tondo
pub fn pow(base: Float, exponent: Float): Float
```

`base` raised to `exponent`.

#### exp

```tondo
pub fn exp(value: Float): Float
```

e raised to the value.

#### ln

```tondo
pub fn ln(value: Float): Float
```

The natural logarithm, by IEEE 754: `-inf` for zero and `NaN` for a
negative value. `checkedLn` gives an error instead.

#### checkedLn

```tondo
pub fn checkedLn(value: Float): Float ! MathError
```

The natural logarithm of a value above zero; another is
`MathError.Domain`.

#### log10

```tondo
pub fn log10(value: Float): Float
```

The base-10 logarithm, by IEEE 754: `-inf` for zero and `NaN` for a
negative value. `checkedLog10` gives an error instead.

#### checkedLog10

```tondo
pub fn checkedLog10(value: Float): Float ! MathError
```

The base-10 logarithm of a value above zero; another is
`MathError.Domain`.

#### log2

```tondo
pub fn log2(value: Float): Float
```

The base-2 logarithm, by IEEE 754: `-inf` for zero and `NaN` for a
negative value. `checkedLog2` gives an error instead.

#### checkedLog2

```tondo
pub fn checkedLog2(value: Float): Float ! MathError
```

The base-2 logarithm of a value above zero; another is
`MathError.Domain`.

#### sin

```tondo
pub fn sin(value: Float): Float
```

The sine of an angle in radians.

#### cos

```tondo
pub fn cos(value: Float): Float
```

The cosine of an angle in radians.

#### tan

```tondo
pub fn tan(value: Float): Float
```

The tangent of an angle in radians.

#### asin

```tondo
pub fn asin(value: Float): Float
```

The arcsine, by IEEE 754: `NaN` for a value outside -1 to 1.
`checkedAsin` gives an error instead.

#### checkedAsin

```tondo
pub fn checkedAsin(value: Float): Float ! MathError
```

The arcsine of a value from -1 to 1; another is `MathError.Domain`.

#### acos

```tondo
pub fn acos(value: Float): Float
```

The arccosine, by IEEE 754: `NaN` for a value outside -1 to 1.
`checkedAcos` gives an error instead.

#### checkedAcos

```tondo
pub fn checkedAcos(value: Float): Float ! MathError
```

The arccosine of a value from -1 to 1; another is `MathError.Domain`.

#### atan

```tondo
pub fn atan(value: Float): Float
```

The angle in radians, from `-π/2` to `π/2`, whose tangent is the value.

#### atan2

```tondo
pub fn atan2(y: Float, x: Float): Float
```

The angle of the point `(x, y)`.

#### hypot

```tondo
pub fn hypot(x: Float, y: Float): Float
```

The length of the hypotenuse, without overflowing on the way.

#### floor

```tondo
pub fn floor(value: Float): Float
```

The largest integer not above the value: `-1.0` for `-0.5`.

#### ceil

```tondo
pub fn ceil(value: Float): Float
```

The smallest integer not below the value: `1.0` for `0.5`.

#### round

```tondo
pub fn round(value: Float): Float
```

The nearest integer; a value halfway between goes to the even one.

#### roundTiesAway

```tondo
pub fn roundTiesAway(value: Float): Float
```

The nearest integer; a value halfway between goes away from zero.

#### truncate

```tondo
pub fn truncate(value: Float): Float
```

The integer part of the value, toward zero: `-1.0` for `-1.7`.

#### fma

```tondo
pub fn fma(a: Float, b: Float, c: Float): Float
```

`a * b + c`, rounded once.

#### dot

```tondo
pub fn dot[T: Number](a: Array[T], b: Array[T]): T
```

The scalar product: the sum of the products of the elements at each
place. Arrays of different lengths panic (`P0006`). Of `Float` and
`Float32` it adds the products in the order of `sum` (language 18.5),
each product and each sum rounded on its own; of any other `T` it
multiplies and adds from left to right, as its operators do.

```tondo
import std.console
import std.math

console.println("{math.dot([1.0, 2.0, 3.0], [4.0, 5.0, 6.0])} {math.dot([2, 3], [4, 5])}")
```

#### norm

```tondo
pub fn norm(values: Array[Float]): Float
```

The length of a vector: the square root of the sum of its squares,
`math.dot(values, values)`.

```tondo
import std.console
import std.math

console.println("{math.norm([3.0, 4.0])}")
```

#### gcd

```tondo
pub fn gcd[T: Integer](a: T, b: T): T
```

The greatest common divisor, never negative.

#### lcm

```tondo
pub fn lcm[T: Integer](a: T, b: T): T
```

The least common multiple, never negative.

### MathError

```tondo
pub enum MathError {
    /// The value is outside the operation's domain, such as the square root
    /// of a negative number.
    Domain
    /// A text that is not a number of the type.
    NotANumber(String)
}
```

Why an operation has no result.

```tondo
import std.console
import std.math

match math.checkedSqrt(-1.0) {
    ok(root) => console.println("{root}")
    err(math.MathError.Domain) => console.println("no real square root")
    err(error) => console.println("{error}")
}
```

Implements `Display`, `reflect.Reflect`.

### BigInt

```tondo
pub type BigInt = {
    // private fields
}
```

An integer of any size, which never overflows: `+`, `-`, `*` and `/`
(truncated toward zero) as for `Int`, and literals of any size where
one is expected, `let n: math.BigInt = 340282366920938463463374607431768211456`.

```tondo
import std.console
import std.math

let big = math.BigInt.parse("123456789012345678901234567890")?
console.println("{big.pow(2)} {big.toString(16)}")
```

Implements `Display`, `Ordered`, `Additive`, `Number`, `serialization.Encode`, `serialization.Decode`, `reflect.Reflect`.

#### BigInt.parse

```tondo
pub fn BigInt.parse(text: String, radix: Int = 10): BigInt ! MathError
```

The integer a text writes in `radix`, from 2 to 36, with an optional
`-` or `+`: `BigInt.parse("ff", radix: 16)`.

#### BigInt.toInt

```tondo
pub fn BigInt.toInt(self): Int?
```

The value as an `Int`, or `none` when it does not fit.

#### BigInt.toString

```tondo
pub fn BigInt.toString(self, radix: Int = 10): String
```

The digits in `radix`, from 2 to 36, lowercase, with `-` when negative.

#### BigInt.abs

```tondo
pub fn BigInt.abs(self): BigInt
```

The value without its sign.

#### BigInt.remainder

```tondo
pub fn BigInt.remainder(self, divisor: BigInt): BigInt
```

What is left of dividing by `divisor`, with the sign of `self`, as `%`
of an `Int`.

#### BigInt.pow

```tondo
pub fn BigInt.pow(self, exponent: Int): BigInt
```

The value raised to `exponent`, which is not negative.

#### BigInt.modPow

```tondo
pub fn BigInt.modPow(self, exponent: BigInt, modulus: BigInt): BigInt
```

The value raised to `exponent` modulo `modulus`, as RSA does, without the
size of the power: `exponent` is not negative and `modulus` positive.

#### BigInt.gcd

```tondo
pub fn BigInt.gcd(self, other: BigInt): BigInt
```

The greatest common divisor, never negative.

### Decimal

```tondo
pub type Decimal = {
    // private fields
}
```

A decimal number, exact, of any size: money, measures, what a `Float`
rounds. `+`, `-` and `*` are exact; `/` gives 34 significant digits,
rounded half to even, and `divide` as many decimals as asked. A literal
where one is expected is exact: `let price: math.Decimal = 19.99`.

```tondo
import std.console
import std.math

let price = math.Decimal.parse("19.99")?
let share = price.divide(math.Decimal.parse("3")?, 2)
console.println("{share} {share.scale()}")
```

Implements `Display`, `Ordered`, `Additive`, `Number`, `serialization.Encode`, `serialization.Decode`, `reflect.Reflect`.

#### Decimal.parse

```tondo
pub fn Decimal.parse(text: String): Decimal ! MathError
```

The decimal a text writes: `-12.50`, `1e-3`, `+7`.

#### Decimal.round

```tondo
pub fn Decimal.round(self, decimals: Int, mode: Rounding = Rounding.HalfEven): Decimal
```

The value rounded to `decimals` after the point.

#### Decimal.divide

```tondo
pub fn Decimal.divide(self, by: Decimal, decimals: Int, mode: Rounding = Rounding.HalfEven): Decimal
```

`self / by` rounded to `decimals` after the point.

#### Decimal.scale

```tondo
pub fn Decimal.scale(self): Int
```

How many decimals the value has after the point.

#### Decimal.toFloat

```tondo
pub fn Decimal.toFloat(self): Float
```

The nearest `Float`.

### Rounding

```tondo
pub enum Rounding {
    HalfEven
    HalfUp
    Down
    Up
    Floor
    Ceiling
}
```

How a value is rounded to fewer decimals: half to even, as banks do;
half away from zero; toward zero (`Down`) or away from it (`Up`); or
toward minus or plus infinity (`Floor`, `Ceiling`).

```tondo
import std.console
import std.math

let amount = math.Decimal.parse("2.345")?
console.println("{amount.round(2, mode: math.Rounding.HalfUp)} {amount.round(2, mode: math.Rounding.Down)}")
```

### Tensor

```tondo
pub type Tensor[T: Number] = {
    // private fields
}
```

Numbers in several dimensions: a shape, the size of each, and the values
in one block in row-major order, the last index changing fastest. Its
rank is the length of its shape; rank 0 is a single value. `+`, `-`, `*`
and `/` work element by element between two tensors, broadcasting a size
of 1, and between a tensor and a number.

```tondo
import std.console
import std.math

let weights = math.Tensor.fromRows([[0.5, 0.25], [0.125, 1.0]])
let input = math.Tensor([1.0, 2.0])
console.println("{weights.matmul(input) + 1.0}")
```

Implements `serialization.Encode`, `serialization.Decode`, `Display`.

#### Tensor

```tondo
pub fn Tensor[T: Number](values: Array[T], shape: Array[Int]? = none): Tensor[T]
```

A tensor of `values`, of `shape`, whose sizes' product must be their
length, or else of rank 1; what does not fit panics (`P0006`).

#### Tensor.fromRows

```tondo
pub fn Tensor[T: Number].fromRows(rows: Array[Array[T]]): Tensor[T]
```

A tensor of rank 2 whose rows are `rows`, all of one length (`P0006`).

#### Tensor.zeros

```tondo
pub fn Tensor[T: Number].zeros(shape: Array[Int]): Tensor[T]
```

A tensor of `shape` whose values are zero.

#### Tensor.filled

```tondo
pub fn Tensor[T: Number].filled(shape: Array[Int], value: T): Tensor[T]
```

A tensor of `shape` whose values are all `value`.

#### Tensor.shape

```tondo
pub fn Tensor[T: Number].shape(self): Array[Int]
```

The size of each axis.

#### Tensor.rank

```tondo
pub fn Tensor[T: Number].rank(self): Int
```

How many axes it has: the length of its shape.

#### Tensor.length

```tondo
pub fn Tensor[T: Number].length(self): Int
```

How many values it has, of all its axes.

#### Tensor.values

```tondo
pub fn Tensor[T: Number].values(self): Array[T]
```

Its values, in row-major order.

#### Tensor.at

```tondo
pub fn Tensor[T: Number].at(self, indexes: ...Int): T
```

The value at `indexes`, one for each axis; a negative one counts from
the end of its axis. Another count, or one out of its size, panics
(`P0001`).

#### Tensor.set

```tondo
pub fn Tensor[T: Number].set(mut self, value: T, indexes: ...Int)
```

Changes the value at `indexes`, as `at` reads it.

#### Tensor.slice

```tondo
pub fn Tensor[T: Number].slice(self, axis: Int, index: Int): Tensor[T]
```

The tensor of one rank less that `index` of `axis` holds.

#### Tensor.reshape

```tondo
pub fn Tensor[T: Number].reshape(self, shape: Array[Int]): Tensor[T]
```

The same values, in the same order, in `shape`, whose sizes' product
must be the same (`P0006`).

#### Tensor.transposed

```tondo
pub fn Tensor[T: Number].transposed(self, axes: Array[Int]? = none): Tensor[T]
```

The tensor whose axis `i` is axis `axes[i]` of this one, a permutation
of its axes (`P0006`); without `axes`, its axes in reverse order, which
swaps the rows and columns of a matrix.

#### Tensor.map

```tondo
pub fn Tensor[T: Number].map(self, transform: fn(T): T): Tensor[T]
```

Each value through `transform`, in the same shape.

#### Tensor.sum

```tondo
pub fn Tensor[T: Number].sum(self): T
```

The sum of its values, as `sum` of `values()` gives it.

#### Tensor.sumAlong

```tondo
pub fn Tensor[T: Number].sumAlong(self, axis: Int): Tensor[T]
```

The tensor of one rank less whose each value is the `sum` of the values
along `axis`, in the order of its index.

#### Tensor.min

```tondo
pub fn Tensor[T: Number + Ordered].min(self): T?
```

The smallest value, the first of equal ones, or `none` when it has none.

#### Tensor.max

```tondo
pub fn Tensor[T: Number + Ordered].max(self): T?
```

The greatest value, the first of equal ones, or `none` when it has none.

#### Tensor.indexOfMax

```tondo
pub fn Tensor[T: Number + Ordered].indexOfMax(self): Array[Int]?
```

The indexes of the first of the greatest values, or `none` when it has
none.

#### Tensor.dot

```tondo
pub fn Tensor[T: Number].dot(self, other: Tensor[T]): T
```

The `math.dot` of two tensors of rank 1 and one length (`P0006`).

#### Tensor.matmul

```tondo
pub fn Tensor[T: Number].matmul(self, other: Tensor[T]): Tensor[T]
```

The product of matrices: an `n × k` by a `k × m` gives an `n × m`, each
value the `math.dot` of a row of this one and a column of `other`. A
tensor of rank 1 is a row on the left and a column on the right, and the
result has one rank less; other ranks or sizes panic (`P0006`).
