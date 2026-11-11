# String

Text: a sequence of Unicode characters, a value like any other.

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api String` shows the same in a terminal.

- [String](#string): [`String.compare`](#stringcompare), [`String.split`](#stringsplit), [`String.splitOnce`](#stringsplitonce), [`String.equalsIgnoringCase`](#stringequalsignoringcase), [`String.lines`](#stringlines), [`String.words`](#stringwords), [`String.repeat`](#stringrepeat), [`String.padStart`](#stringpadstart), [`String.padEnd`](#stringpadend), [`String.trim`](#stringtrim), [`String.trimStart`](#stringtrimstart), [`String.trimEnd`](#stringtrimend), [`String.removePrefix`](#stringremoveprefix), [`String.removeSuffix`](#stringremovesuffix), [`String.findLast`](#stringfindlast), [`String.count`](#stringcount), [`String.toLower`](#stringtolower), [`String.toUpper`](#stringtoupper), [`String.chars`](#stringchars), [`String.length`](#stringlength), [`String.byteLength`](#stringbytelength), [`String.isEmpty`](#stringisempty), [`String.get`](#stringget), [`String.slice`](#stringslice), [`String.concat`](#stringconcat), [`String.contains`](#stringcontains), [`String.startsWith`](#stringstartswith), [`String.endsWith`](#stringendswith), [`String.find`](#stringfind), [`String.replace`](#stringreplace), [`String.toLowerAscii`](#stringtolowerascii), [`String.toUpperAscii`](#stringtoupperascii), [`String.bytes`](#stringbytes), [`String.fromBytes`](#stringfrombytes)

### String

Implements `Ordered`.

#### String.compare

```tondo
pub fn String.compare(self, other: String): Int
```

Negative, zero or positive as `self` goes before, with or after `other`,
by Unicode scalar values.

#### String.split

```tondo
pub fn String.split(self, separator: String, limit: Int? = none): Array[String]
```

The pieces between each `separator`; the characters when it is empty.
With a `limit`, at most that many, the last with the rest of the text:
`"a=b=c".split("=", limit: 2)` is `["a", "b=c"]`.

#### String.splitOnce

```tondo
pub fn String.splitOnce(self, separator: String): (String, String)?
```

The text before the first `separator` and the text after it, or `none`
when it has none: `"key=a=b".splitOnce("=")` is `some(("key", "a=b"))`.

#### String.equalsIgnoringCase

```tondo
pub fn String.equalsIgnoringCase(self, other: String): Bool
```

Whether the texts are the same but for case, by Unicode's rules:
`"Straße".equalsIgnoringCase("STRASSE")` is `true`.

#### String.lines

```tondo
pub fn String.lines(self): Array[String]
```

The lines, without their `\n` or `\r\n`.

#### String.words

```tondo
pub fn String.words(self): Array[String]
```

The pieces between runs of whitespace.

#### String.repeat

```tondo
pub fn String.repeat(self, count: Int): String
```

The text written `count` times in a row; empty when `count` is not
positive.

#### String.padStart

```tondo
pub fn String.padStart(self, width: Int, fill: String = " "): String
```

The text with `fill` before it, repeated and cut, up to `width` scalars.

#### String.padEnd

```tondo
pub fn String.padEnd(self, width: Int, fill: String = " "): String
```

The text with `fill` after it, repeated and cut, up to `width` scalars.

#### String.trim

```tondo
pub fn String.trim(self, characters: String? = none): String
```

The text without the whitespace at its start and end, or without the
characters of `characters` there: `"--a-b--".trim("-")` is `"a-b"`.

#### String.trimStart

```tondo
pub fn String.trimStart(self, characters: String? = none): String
```

The text without the whitespace at its start, or without the
characters of `characters` there.

#### String.trimEnd

```tondo
pub fn String.trimEnd(self, characters: String? = none): String
```

The text without the whitespace at its end, or without the characters
of `characters` there.

#### String.removePrefix

```tondo
pub fn String.removePrefix(self, prefix: String): String
```

The text without `prefix` at its start, if it has it.

#### String.removeSuffix

```tondo
pub fn String.removeSuffix(self, suffix: String): String
```

The text without `suffix` at its end, if it has it.

#### String.findLast

```tondo
pub fn String.findLast(self, needle: String): Int?
```

The scalar index of the last `needle`.

#### String.count

```tondo
pub fn String.count(self, needle: String): Int
```

How many times `needle` appears, without overlapping.

#### String.toLower

```tondo
pub fn String.toLower(self): String
```

The text in lowercase, by Unicode's rules.

#### String.toUpper

```tondo
pub fn String.toUpper(self): String
```

The text in uppercase, by Unicode's rules.

#### String.chars

```tondo
pub fn String.chars(self): CharIterator
```

The characters, one after another, without copying the text.

#### String.length

```tondo
pub fn String.length(self): Int
```

The number of characters.

#### String.byteLength

```tondo
pub fn String.byteLength(self): Int
```

The number of UTF-8 bytes, which `length` counts as characters.

#### String.isEmpty

```tondo
pub fn String.isEmpty(self): Bool
```

Whether the text has no characters.

#### String.get

```tondo
pub fn String.get(self, index: Int): Char?
```

The character at a scalar index.

#### String.slice

```tondo
pub fn String.slice(self, start: Int, end: Int): String ! text.TextError
```

The characters from `start` up to `end`, by scalar index.

#### String.concat

```tondo
pub fn String.concat(self, other: String): String
```

The text followed by `other`, as `"{a}{b}"` writes it.

#### String.contains

```tondo
pub fn String.contains(self, needle: String): Bool
```

Whether `needle` appears in the text; an empty one always does.

#### String.startsWith

```tondo
pub fn String.startsWith(self, prefix: String): Bool
```

Whether the text begins with `prefix`.

#### String.endsWith

```tondo
pub fn String.endsWith(self, suffix: String): Bool
```

Whether the text ends with `suffix`.

#### String.find

```tondo
pub fn String.find(self, needle: String): Int?
```

The scalar index of the first `needle`.

#### String.replace

```tondo
pub fn String.replace(self, from: String, to: String): String
```

The text with every `from` replaced by `to`.

#### String.toLowerAscii

```tondo
pub fn String.toLowerAscii(self): String
```

The text with its ASCII letters in lowercase; other characters stay.

#### String.toUpperAscii

```tondo
pub fn String.toUpperAscii(self): String
```

The text with its ASCII letters in uppercase; other characters stay.

#### String.bytes

```tondo
pub fn String.bytes(self): Array[Byte]
```

The UTF-8 bytes.

#### String.fromBytes

```tondo
pub fn String.fromBytes(bytes: Array[Byte]): String ! text.TextError
```

UTF-8 bytes as text; others are `TextError.InvalidData`.
