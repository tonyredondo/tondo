# Char and Byte

`Char` is one Unicode scalar and `Byte` one byte, which has no arithmetic.

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api Char`, `tondo api Byte` show the same in a terminal.

- [Char](#char): [`Char.compare`](#charcompare), [`Char.isDigit`](#charisdigit), [`Char.isLetter`](#charisletter), [`Char.isWhitespace`](#chariswhitespace), [`Char.isUpper`](#charisupper), [`Char.isLower`](#charislower), [`Char.isAlphanumeric`](#charisalphanumeric), [`Char.isPunctuation`](#charispunctuation), [`Char.isSymbol`](#charissymbol), [`Char.isMark`](#charismark), [`Char.toUpper`](#chartoupper), [`Char.toLower`](#chartolower), [`Char.code`](#charcode), [`Char.from`](#charfrom)
- [Byte](#byte): [`Byte.parse`](#byteparse), [`Byte.toString`](#bytetostring), [`Byte.compare`](#bytecompare), [`Byte.min`](#bytemin), [`Byte.max`](#bytemax), [`Byte.countOnes`](#bytecountones), [`Byte.leadingZeros`](#byteleadingzeros), [`Byte.trailingZeros`](#bytetrailingzeros), [`Byte.rotateLeft`](#byterotateleft), [`Byte.rotateRight`](#byterotateright), [`Byte.reverseBits`](#bytereversebits)

### Char

Implements `Ordered`, `Discrete`.

#### Char.compare

```tondo
pub fn Char.compare(self, other: Char): Int
```

Negative, zero or positive as `self` goes before, with or after `other`.

#### Char.isDigit

```tondo
pub fn Char.isDigit(self): Bool
```

Whether it is one of `0` to `9`.

#### Char.isLetter

```tondo
pub fn Char.isLetter(self): Bool
```

Whether the scalar is a letter of any script, as Unicode's `Alphabetic`.

#### Char.isWhitespace

```tondo
pub fn Char.isWhitespace(self): Bool
```

Whether the scalar is whitespace, as Unicode's `White_Space`.

#### Char.isUpper

```tondo
pub fn Char.isUpper(self): Bool
```

Whether the scalar is uppercase, as Unicode's `Uppercase`.

#### Char.isLower

```tondo
pub fn Char.isLower(self): Bool
```

Whether the scalar is lowercase, as Unicode's `Lowercase`.

#### Char.isAlphanumeric

```tondo
pub fn Char.isAlphanumeric(self): Bool
```

Whether the scalar is a letter or a number of any script.

#### Char.isPunctuation

```tondo
pub fn Char.isPunctuation(self): Bool
```

Whether the scalar is punctuation, of any of its general categories:
`.`, `-`, `(`, `«`, `¿`.

#### Char.isSymbol

```tondo
pub fn Char.isSymbol(self): Bool
```

Whether the scalar is a symbol: of mathematics, `+`, of a currency, `€`,
a modifier, `^`, or another, `©`.

#### Char.isMark

```tondo
pub fn Char.isMark(self): Bool
```

Whether the scalar is a mark that goes on the one before, as the accent
that `text.normalize` with `Nfd` takes out of `é`.

#### Char.toUpper

```tondo
pub fn Char.toUpper(self): Char
```

The uppercase scalar, when it is a single one.

#### Char.toLower

```tondo
pub fn Char.toLower(self): Char
```

The lowercase scalar, when it is a single one.

#### Char.code

```tondo
pub fn Char.code(self): Int
```

The Unicode scalar value.

#### Char.from

```tondo
pub fn Char.from(code: Int): Char?
```

The scalar with this Unicode value, if it is one.

### Byte

Implements `Ordered`.

#### Byte.parse

```tondo
pub fn Byte.parse(text: String, radix: Int = 10): Byte ! text.TextError
```

The integer the text writes in `radix` (2 to 36), with an optional sign and
`_` between digits; `InvalidData` when it is not a value of the type.

#### Byte.toString

```tondo
pub fn Byte.toString(self, radix: Int = 10): String
```

The value in `radix` (2 to 36), with lowercase digits and `-` before a
negative one.

#### Byte.compare

```tondo
pub fn Byte.compare(self, other: Byte): Int
```

Negative, zero or positive as the value goes before, with or after `other`.

#### Byte.min

```tondo
pub fn Byte.min(self, other: Byte): Byte
```

The smaller of the two; for a float, the value itself when either is NaN.

#### Byte.max

```tondo
pub fn Byte.max(self, other: Byte): Byte
```

The larger of the two; for a float, the value itself when either is NaN.

#### Byte.countOnes

```tondo
pub fn Byte.countOnes(self): Int
```

The bits at 1.

#### Byte.leadingZeros

```tondo
pub fn Byte.leadingZeros(self): Int
```

The bits at 0 above the highest 1: all of them for 0.

#### Byte.trailingZeros

```tondo
pub fn Byte.trailingZeros(self): Int
```

The bits at 0 below the lowest 1: all of them for 0.

#### Byte.rotateLeft

```tondo
pub fn Byte.rotateLeft(self, count: Int): Byte
```

The bits moved `count` places toward the highest, those that leave coming
back at the lowest; `count` is taken modulo the width.

#### Byte.rotateRight

```tondo
pub fn Byte.rotateRight(self, count: Int): Byte
```

The bits moved `count` places toward the lowest, those that leave coming
back at the highest; `count` is taken modulo the width.

#### Byte.reverseBits

```tondo
pub fn Byte.reverseBits(self): Byte
```

The bits in the opposite order.
