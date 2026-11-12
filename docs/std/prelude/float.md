# Float and Float32

`Float` is a 64-bit IEEE 754 number and `Float32` a 32-bit one.

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api Float`, `tondo api Float32` show the same in a terminal.

- [Float](#float): [`Float.parse`](#floatparse), [`Float.compare`](#floatcompare), [`Float.min`](#floatmin), [`Float.max`](#floatmax), [`Float.clamp`](#floatclamp), [`Float.pow`](#floatpow), [`Float.abs`](#floatabs), [`Float.floor`](#floatfloor), [`Float.ceil`](#floatceil), [`Float.round`](#floatround), [`Float.roundTiesAway`](#floatroundtiesaway), [`Float.truncate`](#floattruncate), [`Float.sqrt`](#floatsqrt), [`Float.checkedSqrt`](#floatcheckedsqrt), [`Float.isNaN`](#floatisnan), [`Float.isInfinite`](#floatisinfinite), [`Float.isFinite`](#floatisfinite), [`Float.toFixed`](#floattofixed), [`Float.bits`](#floatbits), [`Float.fromBits`](#floatfrombits)
- [Float32](#float32): [`Float32.parse`](#float32parse), [`Float32.compare`](#float32compare), [`Float32.min`](#float32min), [`Float32.max`](#float32max), [`Float32.clamp`](#float32clamp), [`Float32.pow`](#float32pow), [`Float32.abs`](#float32abs), [`Float32.floor`](#float32floor), [`Float32.ceil`](#float32ceil), [`Float32.round`](#float32round), [`Float32.roundTiesAway`](#float32roundtiesaway), [`Float32.truncate`](#float32truncate), [`Float32.sqrt`](#float32sqrt), [`Float32.checkedSqrt`](#float32checkedsqrt), [`Float32.isNaN`](#float32isnan), [`Float32.isInfinite`](#float32isinfinite), [`Float32.isFinite`](#float32isfinite), [`Float32.toFixed`](#float32tofixed), [`Float32.bits`](#float32bits), [`Float32.fromBits`](#float32frombits)

### Float

Implements `Ordered`.

#### Float.parse

```tondo
pub fn Float.parse(text: String): Float ! text.TextError
```

The float the text writes, such as `1.5`, `-2e3`, `inf` or `NaN`.

#### Float.compare

```tondo
pub fn Float.compare(self, other: Float): Int
```

Negative, zero or positive as the value goes before, with or after `other`,
with `NaN` after every other value and equal to itself.

#### Float.min

```tondo
pub fn Float.min(self, other: Float): Float
```

The smaller of the two; for a float, the value itself when either is NaN.

#### Float.max

```tondo
pub fn Float.max(self, other: Float): Float
```

The larger of the two; for a float, the value itself when either is NaN.

#### Float.clamp

```tondo
pub fn Float.clamp(self, low: Float, high: Float): Float
```

The value inside `low` to `high`.

#### Float.pow

```tondo
pub fn Float.pow(self, exponent: Float): Float
```

The value raised to `exponent`.

#### Float.abs

```tondo
pub fn Float.abs(self): Float
```

The value without its sign.

#### Float.floor

```tondo
pub fn Float.floor(self): Float
```

The greatest integer not above the value: `-1.0` for `-0.5`.

#### Float.ceil

```tondo
pub fn Float.ceil(self): Float
```

The least integer not below the value: `1.0` for `0.5`.

#### Float.round

```tondo
pub fn Float.round(self): Float
```

The nearest integer; a value halfway between goes to the even one.

#### Float.roundTiesAway

```tondo
pub fn Float.roundTiesAway(self): Float
```

The nearest integer; a value halfway between goes away from zero.

#### Float.truncate

```tondo
pub fn Float.truncate(self): Float
```

The integer part, toward zero: `-1.0` for `-1.7`.

#### Float.sqrt

```tondo
pub fn Float.sqrt(self): Float
```

The square root, by IEEE 754: `NaN` for a negative value.

#### Float.checkedSqrt

```tondo
pub fn Float.checkedSqrt(self): Float ! math.MathError
```

The square root; an error for a negative value.

#### Float.isNaN

```tondo
pub fn Float.isNaN(self): Bool
```

Whether the value is not a number, as `0.0 / 0.0`.

#### Float.isInfinite

```tondo
pub fn Float.isInfinite(self): Bool
```

Whether the value is infinite, of either sign.

#### Float.isFinite

```tondo
pub fn Float.isFinite(self): Bool
```

Whether the value is neither infinite nor NaN.

#### Float.toFixed

```tondo
pub fn Float.toFixed(self, decimals: Int): String
```

The value with exactly `decimals` digits after the point: `2.50`.

#### Float.bits

```tondo
pub fn Float.bits(self): UInt
```

The value's IEEE 754 bits, as they are: `UInt` for a `Float`, `UInt32` for a
`Float32`.

#### Float.fromBits

```tondo
pub fn Float.fromBits(bits: UInt): Float
```

The float of these IEEE 754 bits, as they are.

### Float32

#### Float32.parse

```tondo
pub fn Float32.parse(text: String): Float32 ! text.TextError
```

The float the text writes, such as `1.5`, `-2e3`, `inf` or `NaN`.

#### Float32.compare

```tondo
pub fn Float32.compare(self, other: Float32): Int
```

Negative, zero or positive as the value goes before, with or after `other`,
with `NaN` after every other value and equal to itself.

#### Float32.min

```tondo
pub fn Float32.min(self, other: Float32): Float32
```

The smaller of the two; for a float, the value itself when either is NaN.

#### Float32.max

```tondo
pub fn Float32.max(self, other: Float32): Float32
```

The larger of the two; for a float, the value itself when either is NaN.

#### Float32.clamp

```tondo
pub fn Float32.clamp(self, low: Float32, high: Float32): Float32
```

The value inside `low` to `high`.

#### Float32.pow

```tondo
pub fn Float32.pow(self, exponent: Float32): Float32
```

The value raised to `exponent`.

#### Float32.abs

```tondo
pub fn Float32.abs(self): Float32
```

The value without its sign.

#### Float32.floor

```tondo
pub fn Float32.floor(self): Float32
```

The greatest integer not above the value: `-1.0` for `-0.5`.

#### Float32.ceil

```tondo
pub fn Float32.ceil(self): Float32
```

The least integer not below the value: `1.0` for `0.5`.

#### Float32.round

```tondo
pub fn Float32.round(self): Float32
```

The nearest integer; a value halfway between goes to the even one.

#### Float32.roundTiesAway

```tondo
pub fn Float32.roundTiesAway(self): Float32
```

The nearest integer; a value halfway between goes away from zero.

#### Float32.truncate

```tondo
pub fn Float32.truncate(self): Float32
```

The integer part, toward zero: `-1.0` for `-1.7`.

#### Float32.sqrt

```tondo
pub fn Float32.sqrt(self): Float32
```

The square root, by IEEE 754: `NaN` for a negative value.

#### Float32.checkedSqrt

```tondo
pub fn Float32.checkedSqrt(self): Float32 ! math.MathError
```

The square root; an error for a negative value.

#### Float32.isNaN

```tondo
pub fn Float32.isNaN(self): Bool
```

Whether the value is not a number, as `0.0 / 0.0`.

#### Float32.isInfinite

```tondo
pub fn Float32.isInfinite(self): Bool
```

Whether the value is infinite, of either sign.

#### Float32.isFinite

```tondo
pub fn Float32.isFinite(self): Bool
```

Whether the value is neither infinite nor NaN.

#### Float32.toFixed

```tondo
pub fn Float32.toFixed(self, decimals: Int): String
```

The value with exactly `decimals` digits after the point: `2.50`.

#### Float32.bits

```tondo
pub fn Float32.bits(self): UInt32
```

The value's IEEE 754 bits, as they are: `UInt` for a `Float`, `UInt32` for a
`Float32`.

#### Float32.fromBits

```tondo
pub fn Float32.fromBits(bits: UInt32): Float32
```

The float of these IEEE 754 bits, as they are.
