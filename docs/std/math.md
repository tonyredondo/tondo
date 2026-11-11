# std.math

scalar math with IEEE-754 semantics and no fast-math, and integers of any size and exact decimals written in Tondo. An operation outside its domain is an error, never a quiet `NaN`.

```tondo
import std.console
import std.math

let side = math.sqrt(3.0 * 3.0 + 4.0 * 4.0)?
console.println("{side} {math.round(2.5)} {math.pow(2.0, 10.0)}")
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.math` shows the same in a terminal.

- [Functions](#functions): [`sqrt`](#sqrt), [`pow`](#pow), [`exp`](#exp), [`ln`](#ln), [`log10`](#log10), [`log2`](#log2), [`sin`](#sin), [`cos`](#cos), [`tan`](#tan), [`asin`](#asin), [`acos`](#acos), [`atan`](#atan), [`atan2`](#atan2), [`hypot`](#hypot), [`floor`](#floor), [`ceil`](#ceil), [`round`](#round), [`roundTiesAway`](#roundtiesaway), [`truncate`](#truncate), [`fma`](#fma), [`gcd`](#gcd), [`lcm`](#lcm)
- [MathError](#matherror)
- [BigInt](#bigint): [`BigInt.parse`](#bigintparse), [`BigInt.toInt`](#biginttoint), [`BigInt.toString`](#biginttostring), [`BigInt.abs`](#bigintabs), [`BigInt.remainder`](#bigintremainder), [`BigInt.pow`](#bigintpow), [`BigInt.modPow`](#bigintmodpow), [`BigInt.gcd`](#bigintgcd)
- [Decimal](#decimal): [`Decimal.parse`](#decimalparse), [`Decimal.round`](#decimalround), [`Decimal.divide`](#decimaldivide), [`Decimal.scale`](#decimalscale), [`Decimal.toFloat`](#decimaltofloat)
- [Rounding](#rounding)

### Functions

#### sqrt

```tondo
pub fn sqrt(value: Float): Float ! MathError
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
pub fn ln(value: Float): Float ! MathError
```

The natural logarithm of a value above zero.

#### log10

```tondo
pub fn log10(value: Float): Float ! MathError
```

The base-10 logarithm of a value above zero; another is
`MathError.Domain`.

#### log2

```tondo
pub fn log2(value: Float): Float ! MathError
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
pub fn asin(value: Float): Float ! MathError
```

The arcsine of a value from -1 to 1.

#### acos

```tondo
pub fn acos(value: Float): Float ! MathError
```

The arccosine of a value from -1 to 1.

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

match math.sqrt(-1.0) {
    ok(root) => console.println("{root}")
    err(math.MathError.Domain) => console.println("no real square root")
    err(error) => console.println("{error}")
}
```

Implements `Display`.

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
