# Int and the integers of every width

`Int` is 64 bits and its overflow panics. `UInt` is its unsigned twin, and `Int8` to `Int128` and `UInt8` to `UInt128` have the same operations, shown here for `Int` and `UInt`.

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api Int`, `tondo api UInt` show the same in a terminal.

- [Int](#int): [`Int.parse`](#intparse), [`Int.toString`](#inttostring), [`Int.compare`](#intcompare), [`Int.min`](#intmin), [`Int.max`](#intmax), [`Int.clamp`](#intclamp), [`Int.pow`](#intpow), [`Int.abs`](#intabs), [`Int.wrappingAdd`](#intwrappingadd), [`Int.wrappingSub`](#intwrappingsub), [`Int.wrappingMul`](#intwrappingmul), [`Int.wrappingNeg`](#intwrappingneg), [`Int.wrappingPow`](#intwrappingpow), [`Int.checkedAdd`](#intcheckedadd), [`Int.checkedSub`](#intcheckedsub), [`Int.checkedMul`](#intcheckedmul), [`Int.checkedDiv`](#intcheckeddiv), [`Int.checkedRem`](#intcheckedrem), [`Int.checkedNeg`](#intcheckedneg), [`Int.checkedPow`](#intcheckedpow), [`Int.saturatingAdd`](#intsaturatingadd), [`Int.saturatingSub`](#intsaturatingsub), [`Int.saturatingMul`](#intsaturatingmul), [`Int.saturatingPow`](#intsaturatingpow), [`Int.overflowingAdd`](#intoverflowingadd), [`Int.overflowingSub`](#intoverflowingsub), [`Int.overflowingMul`](#intoverflowingmul), [`Int.countOnes`](#intcountones), [`Int.leadingZeros`](#intleadingzeros), [`Int.trailingZeros`](#inttrailingzeros), [`Int.rotateLeft`](#introtateleft), [`Int.rotateRight`](#introtateright), [`Int.reverseBytes`](#intreversebytes), [`Int.reverseBits`](#intreversebits), [`Int.toBytes`](#inttobytes), [`Int.fromBytes`](#intfrombytes), [`Int.wrapping`](#intwrapping), [`Int.saturating`](#intsaturating)
- [UInt](#uint): [`UInt.parse`](#uintparse), [`UInt.toString`](#uinttostring), [`UInt.compare`](#uintcompare), [`UInt.min`](#uintmin), [`UInt.max`](#uintmax), [`UInt.clamp`](#uintclamp), [`UInt.pow`](#uintpow), [`UInt.wrappingAdd`](#uintwrappingadd), [`UInt.wrappingSub`](#uintwrappingsub), [`UInt.wrappingMul`](#uintwrappingmul), [`UInt.wrappingNeg`](#uintwrappingneg), [`UInt.wrappingPow`](#uintwrappingpow), [`UInt.checkedAdd`](#uintcheckedadd), [`UInt.checkedSub`](#uintcheckedsub), [`UInt.checkedMul`](#uintcheckedmul), [`UInt.checkedDiv`](#uintcheckeddiv), [`UInt.checkedRem`](#uintcheckedrem), [`UInt.checkedNeg`](#uintcheckedneg), [`UInt.checkedPow`](#uintcheckedpow), [`UInt.saturatingAdd`](#uintsaturatingadd), [`UInt.saturatingSub`](#uintsaturatingsub), [`UInt.saturatingMul`](#uintsaturatingmul), [`UInt.saturatingPow`](#uintsaturatingpow), [`UInt.overflowingAdd`](#uintoverflowingadd), [`UInt.overflowingSub`](#uintoverflowingsub), [`UInt.overflowingMul`](#uintoverflowingmul), [`UInt.countOnes`](#uintcountones), [`UInt.leadingZeros`](#uintleadingzeros), [`UInt.trailingZeros`](#uinttrailingzeros), [`UInt.rotateLeft`](#uintrotateleft), [`UInt.rotateRight`](#uintrotateright), [`UInt.reverseBytes`](#uintreversebytes), [`UInt.reverseBits`](#uintreversebits), [`UInt.isPowerOfTwo`](#uintispoweroftwo), [`UInt.nextPowerOfTwo`](#uintnextpoweroftwo), [`UInt.toBytes`](#uinttobytes), [`UInt.fromBytes`](#uintfrombytes), [`UInt.wrapping`](#uintwrapping), [`UInt.saturating`](#uintsaturating)

### Int

Implements `Ordered`, `Discrete`.

#### Int.parse

```tondo
pub fn Int.parse(text: String, radix: Int = 10): Int ! text.TextError
```

The integer the text writes in `radix` (2 to 36), with an optional sign and
`_` between digits; `InvalidData` when it is not a value of the type.

#### Int.toString

```tondo
pub fn Int.toString(self, radix: Int = 10): String
```

The value in `radix` (2 to 36), with lowercase digits and `-` before a
negative one.

#### Int.compare

```tondo
pub fn Int.compare(self, other: Int): Int
```

Negative, zero or positive as the value goes before, with or after `other`.

#### Int.min

```tondo
pub fn Int.min(self, other: Int): Int
```

The smaller of the two; for a float, the value itself when either is NaN.

#### Int.max

```tondo
pub fn Int.max(self, other: Int): Int
```

The larger of the two; for a float, the value itself when either is NaN.

#### Int.clamp

```tondo
pub fn Int.clamp(self, low: Int, high: Int): Int
```

The value inside `low` to `high`.

#### Int.pow

```tondo
pub fn Int.pow(self, exponent: Int): Int
```

The value raised to a power of zero or more; overflow panics as any
arithmetic, and so does a negative exponent.

#### Int.abs

```tondo
pub fn Int.abs(self): Int
```

The value without its sign; the least value has none in its type, and
panics.

#### Int.wrappingAdd

```tondo
pub fn Int.wrappingAdd(self, other: Int): Int
```

`self + other`, keeping the low bits of a result that does not fit.

#### Int.wrappingSub

```tondo
pub fn Int.wrappingSub(self, other: Int): Int
```

`self - other`, keeping the low bits of a result that does not fit.

#### Int.wrappingMul

```tondo
pub fn Int.wrappingMul(self, other: Int): Int
```

`self * other`, keeping the low bits of a result that does not fit.

#### Int.wrappingNeg

```tondo
pub fn Int.wrappingNeg(self): Int
```

`-self`, keeping the low bits: the two's complement of an unsigned value.

#### Int.wrappingPow

```tondo
pub fn Int.wrappingPow(self, exponent: Int): Int
```

The value raised to `exponent`, keeping the low bits of a result that does
not fit.

#### Int.checkedAdd

```tondo
pub fn Int.checkedAdd(self, other: Int): Int?
```

`self + other`, or `none` when it does not fit.

#### Int.checkedSub

```tondo
pub fn Int.checkedSub(self, other: Int): Int?
```

`self - other`, or `none` when it does not fit.

#### Int.checkedMul

```tondo
pub fn Int.checkedMul(self, other: Int): Int?
```

`self * other`, or `none` when it does not fit.

#### Int.checkedDiv

```tondo
pub fn Int.checkedDiv(self, other: Int): Int?
```

`self / other`, or `none` when `other` is zero or the quotient does not fit.

#### Int.checkedRem

```tondo
pub fn Int.checkedRem(self, other: Int): Int?
```

`self % other`, or `none` when `other` is zero.

#### Int.checkedNeg

```tondo
pub fn Int.checkedNeg(self): Int?
```

`-self`, or `none` when it does not fit.

#### Int.checkedPow

```tondo
pub fn Int.checkedPow(self, exponent: Int): Int?
```

The value raised to `exponent`, or `none` when it does not fit.

#### Int.saturatingAdd

```tondo
pub fn Int.saturatingAdd(self, other: Int): Int
```

`self + other`, or the least or greatest value when it does not fit.

#### Int.saturatingSub

```tondo
pub fn Int.saturatingSub(self, other: Int): Int
```

`self - other`, or the least or greatest value when it does not fit.

#### Int.saturatingMul

```tondo
pub fn Int.saturatingMul(self, other: Int): Int
```

`self * other`, or the least or greatest value when it does not fit.

#### Int.saturatingPow

```tondo
pub fn Int.saturatingPow(self, exponent: Int): Int
```

The value raised to `exponent`, or the least or greatest value when it does
not fit.

#### Int.overflowingAdd

```tondo
pub fn Int.overflowingAdd(self, other: Int): (Int, Bool)
```

`wrappingAdd`'s result, and whether `self + other` did not fit.

#### Int.overflowingSub

```tondo
pub fn Int.overflowingSub(self, other: Int): (Int, Bool)
```

`wrappingSub`'s result, and whether `self - other` did not fit.

#### Int.overflowingMul

```tondo
pub fn Int.overflowingMul(self, other: Int): (Int, Bool)
```

`wrappingMul`'s result, and whether `self * other` did not fit.

#### Int.countOnes

```tondo
pub fn Int.countOnes(self): Int
```

The bits at 1.

#### Int.leadingZeros

```tondo
pub fn Int.leadingZeros(self): Int
```

The bits at 0 above the highest 1: all of them for 0.

#### Int.trailingZeros

```tondo
pub fn Int.trailingZeros(self): Int
```

The bits at 0 below the lowest 1: all of them for 0.

#### Int.rotateLeft

```tondo
pub fn Int.rotateLeft(self, count: Int): Int
```

The bits moved `count` places toward the highest, those that leave coming
back at the lowest; `count` is taken modulo the width.

#### Int.rotateRight

```tondo
pub fn Int.rotateRight(self, count: Int): Int
```

The bits moved `count` places toward the lowest, those that leave coming
back at the highest; `count` is taken modulo the width.

#### Int.reverseBytes

```tondo
pub fn Int.reverseBytes(self): Int
```

The bytes in the opposite order, which turns big-endian into little-endian
and back.

#### Int.reverseBits

```tondo
pub fn Int.reverseBits(self): Int
```

The bits in the opposite order.

#### Int.toBytes

```tondo
pub fn Int.toBytes(self, order: ByteOrder = ByteOrder.Big): Array[Byte]
```

The value's bytes, as many as its bits over 8, the highest first unless
`order` is `Little`.

#### Int.fromBytes

```tondo
pub fn Int.fromBytes(bytes: Array[Byte], at: Int = 0, order: ByteOrder = ByteOrder.Big): Int
```

The value the bytes from `at` write, as many as its bits over 8, the highest
first unless `order` is `Little`; panics when they are not there.

#### Int.wrapping

```tondo
pub fn Int.wrapping[N](value: N): Int
```

The low bits of any integer or `Byte`, in two's complement:
`UInt8.wrapping(300)` is 44 and `UInt.wrapping(-1)` is `UInt.max`.

#### Int.saturating

```tondo
pub fn Int.saturating[N](value: N): Int
```

The nearest value to any number or `Byte`: `UInt8.saturating(-5)` is 0; a
float is truncated toward zero, and NaN gives 0.

### UInt

Implements `Ordered`, `Discrete`.

#### UInt.parse

```tondo
pub fn UInt.parse(text: String, radix: Int = 10): UInt ! text.TextError
```

The integer the text writes in `radix` (2 to 36), with an optional sign and
`_` between digits; `InvalidData` when it is not a value of the type.

#### UInt.toString

```tondo
pub fn UInt.toString(self, radix: Int = 10): String
```

The value in `radix` (2 to 36), with lowercase digits and `-` before a
negative one.

#### UInt.compare

```tondo
pub fn UInt.compare(self, other: UInt): Int
```

Negative, zero or positive as the value goes before, with or after `other`.

#### UInt.min

```tondo
pub fn UInt.min(self, other: UInt): UInt
```

The smaller of the two; for a float, the value itself when either is NaN.

#### UInt.max

```tondo
pub fn UInt.max(self, other: UInt): UInt
```

The larger of the two; for a float, the value itself when either is NaN.

#### UInt.clamp

```tondo
pub fn UInt.clamp(self, low: UInt, high: UInt): UInt
```

The value inside `low` to `high`.

#### UInt.pow

```tondo
pub fn UInt.pow(self, exponent: Int): UInt
```

The value raised to a power of zero or more; overflow panics as any
arithmetic, and so does a negative exponent.

#### UInt.wrappingAdd

```tondo
pub fn UInt.wrappingAdd(self, other: UInt): UInt
```

`self + other`, keeping the low bits of a result that does not fit.

#### UInt.wrappingSub

```tondo
pub fn UInt.wrappingSub(self, other: UInt): UInt
```

`self - other`, keeping the low bits of a result that does not fit.

#### UInt.wrappingMul

```tondo
pub fn UInt.wrappingMul(self, other: UInt): UInt
```

`self * other`, keeping the low bits of a result that does not fit.

#### UInt.wrappingNeg

```tondo
pub fn UInt.wrappingNeg(self): UInt
```

`-self`, keeping the low bits: the two's complement of an unsigned value.

#### UInt.wrappingPow

```tondo
pub fn UInt.wrappingPow(self, exponent: Int): UInt
```

The value raised to `exponent`, keeping the low bits of a result that does
not fit.

#### UInt.checkedAdd

```tondo
pub fn UInt.checkedAdd(self, other: UInt): UInt?
```

`self + other`, or `none` when it does not fit.

#### UInt.checkedSub

```tondo
pub fn UInt.checkedSub(self, other: UInt): UInt?
```

`self - other`, or `none` when it does not fit.

#### UInt.checkedMul

```tondo
pub fn UInt.checkedMul(self, other: UInt): UInt?
```

`self * other`, or `none` when it does not fit.

#### UInt.checkedDiv

```tondo
pub fn UInt.checkedDiv(self, other: UInt): UInt?
```

`self / other`, or `none` when `other` is zero or the quotient does not fit.

#### UInt.checkedRem

```tondo
pub fn UInt.checkedRem(self, other: UInt): UInt?
```

`self % other`, or `none` when `other` is zero.

#### UInt.checkedNeg

```tondo
pub fn UInt.checkedNeg(self): UInt?
```

`-self`, or `none` when it does not fit.

#### UInt.checkedPow

```tondo
pub fn UInt.checkedPow(self, exponent: Int): UInt?
```

The value raised to `exponent`, or `none` when it does not fit.

#### UInt.saturatingAdd

```tondo
pub fn UInt.saturatingAdd(self, other: UInt): UInt
```

`self + other`, or the least or greatest value when it does not fit.

#### UInt.saturatingSub

```tondo
pub fn UInt.saturatingSub(self, other: UInt): UInt
```

`self - other`, or the least or greatest value when it does not fit.

#### UInt.saturatingMul

```tondo
pub fn UInt.saturatingMul(self, other: UInt): UInt
```

`self * other`, or the least or greatest value when it does not fit.

#### UInt.saturatingPow

```tondo
pub fn UInt.saturatingPow(self, exponent: Int): UInt
```

The value raised to `exponent`, or the least or greatest value when it does
not fit.

#### UInt.overflowingAdd

```tondo
pub fn UInt.overflowingAdd(self, other: UInt): (UInt, Bool)
```

`wrappingAdd`'s result, and whether `self + other` did not fit.

#### UInt.overflowingSub

```tondo
pub fn UInt.overflowingSub(self, other: UInt): (UInt, Bool)
```

`wrappingSub`'s result, and whether `self - other` did not fit.

#### UInt.overflowingMul

```tondo
pub fn UInt.overflowingMul(self, other: UInt): (UInt, Bool)
```

`wrappingMul`'s result, and whether `self * other` did not fit.

#### UInt.countOnes

```tondo
pub fn UInt.countOnes(self): Int
```

The bits at 1.

#### UInt.leadingZeros

```tondo
pub fn UInt.leadingZeros(self): Int
```

The bits at 0 above the highest 1: all of them for 0.

#### UInt.trailingZeros

```tondo
pub fn UInt.trailingZeros(self): Int
```

The bits at 0 below the lowest 1: all of them for 0.

#### UInt.rotateLeft

```tondo
pub fn UInt.rotateLeft(self, count: Int): UInt
```

The bits moved `count` places toward the highest, those that leave coming
back at the lowest; `count` is taken modulo the width.

#### UInt.rotateRight

```tondo
pub fn UInt.rotateRight(self, count: Int): UInt
```

The bits moved `count` places toward the lowest, those that leave coming
back at the highest; `count` is taken modulo the width.

#### UInt.reverseBytes

```tondo
pub fn UInt.reverseBytes(self): UInt
```

The bytes in the opposite order, which turns big-endian into little-endian
and back.

#### UInt.reverseBits

```tondo
pub fn UInt.reverseBits(self): UInt
```

The bits in the opposite order.

#### UInt.isPowerOfTwo

```tondo
pub fn UInt.isPowerOfTwo(self): Bool
```

Whether exactly one bit is 1.

#### UInt.nextPowerOfTwo

```tondo
pub fn UInt.nextPowerOfTwo(self): UInt
```

The least power of two not below the value, 1 for 0; panics when it does not
fit.

#### UInt.toBytes

```tondo
pub fn UInt.toBytes(self, order: ByteOrder = ByteOrder.Big): Array[Byte]
```

The value's bytes, as many as its bits over 8, the highest first unless
`order` is `Little`.

#### UInt.fromBytes

```tondo
pub fn UInt.fromBytes(bytes: Array[Byte], at: Int = 0, order: ByteOrder = ByteOrder.Big): UInt
```

The value the bytes from `at` write, as many as its bits over 8, the highest
first unless `order` is `Little`; panics when they are not there.

#### UInt.wrapping

```tondo
pub fn UInt.wrapping[N](value: N): UInt
```

The low bits of any integer or `Byte`, in two's complement:
`UInt8.wrapping(300)` is 44 and `UInt.wrapping(-1)` is `UInt.max`.

#### UInt.saturating

```tondo
pub fn UInt.saturating[N](value: N): UInt
```

The nearest value to any number or `Byte`: `UInt8.saturating(-5)` is 0; a
float is truncated toward zero, and NaN gives 0.
