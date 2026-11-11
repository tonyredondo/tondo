# Text

A `String` is text: a sequence of Unicode characters. It is a value, it
never changes in place, and it is always valid. This chapter covers writing
strings, putting values into them, taking them apart, and the characters and
bytes they are made of.

- [String literals](#string-literals)
- [Interpolation](#interpolation)
- [Formats](#formats)
- [Raw and multi-line strings](#raw-and-multi-line-strings)
- [Length, indexes and slices](#length-indexes-and-slices)
- [The methods of a string](#the-methods-of-a-string)
- [Building text](#building-text)
- [Characters](#characters)
- [Bytes](#bytes)
- [Text and numbers](#text-and-numbers)
- [Unicode, for real](#unicode-for-real)
- [Text for people](#text-for-people)

## String literals

```tondo
import std.console

fn main() {
    let plain = "a line of text"
    let quoted = "she said \"hello\""
    let escapes = "tab:\there, a new line:\nand a snowman: \u{2603}"
    let braces = "a literal {{brace}}"
    console.println(plain)
    console.println(quoted)
    console.println(escapes)
    console.println(braces)
}
```

```text output
a line of text
she said "hello"
tab:	here, a new line:
and a snowman: ☃
a literal {brace}
```

The escapes are `\n`, `\r`, `\t`, `\\`, `\"`, `\0` and `\u{HEX}`; any other
is an error. A `{` starts an interpolation, so a literal brace is doubled.

## Interpolation

`{expression}` inside a string puts a value in it. Any expression goes
there, and any value whose type has `Display`:

```tondo
import std.console

type Point = { x: Int, y: Int }

impl Display for Point {
    fn display(self): String {
        "({self.x}, {self.y})"
    }
}

fn main() {
    let name = "Ada"
    let items = ["pen", "ink"]
    let point = Point { x: 3, y: 4 }
    console.println("Hello, {name}!")
    console.println("{items.length()} items: {items.join(", ")}")
    console.println("{point} is {if point.x > 0 { "right" } else { "left" }} of the origin")
    console.println("{1 + 2} {[1, 2]} {some(3)} {("a", 1)} {'x'} {true} {2.5}")
}
```

```text output
Hello, Ada!
2 items: pen, ink
(3, 4) is right of the origin
3 [1, 2] some(3) (a, 1) x true 2.5
```

Numbers, booleans, characters and strings have `Display`. So do options,
results, tuples and collections of things that have it, and an enum whose
variants hold no data, which shows its variant's name. A record or an enum
with data has it when the program writes it: `impl Display for T`.

`+` does not join strings. Interpolation does, and reads better:
`"{first} {last}"`. To join two strings held in variables without a
literal, `first.concat(last)`.

## Formats

After a `:`, an interpolation says how to write the value. The compiler
checks the format against the value's type.

```tondo
import std.console

fn main() {
    let name = "Ada"
    let price = 3.14159
    let count = 42
    console.println("[{name:<8}] [{name:>8}] [{name:^8}] [{name:*^8}]")
    console.println("[{price:.2}] [{price:8.1}] [{price:+.3}] [{1500.0:e}]")
    console.println("[{count:6}] [{count:06}] [{count:<6}] [{count:+}]")
    console.println("[{255:x}] [{255:#x}] [{255:b}] [{255:#o}] [{255:#010b}]")
}
```

```text output
[Ada     ] [     Ada] [  Ada   ] [**Ada***]
[3.14] [     3.1] [+3.142] [1.5e3]
[    42] [000042] [42    ] [+42]
[ff] [0xff] [11111111] [0o377] [0b11111111]
```

The shape is `[[fill]align][+][#][0][width][.precision][type]`:

| Part | Means |
| --- | --- |
| `<` `>` `^` | align left, right or centre, in `width`; a character before it fills |
| `+` | write the sign of a positive number too |
| `0` | fill a number with zeros up to `width` |
| `width` | the least number of characters |
| `.precision` | decimals of a float |
| `x` `o` `b` | an integer in hexadecimal, octal or binary; `#` adds `0x`, `0o` or `0b` |
| `e` | a float in scientific notation |

There is no `d` or `f`: a number is written as a number by default, so two
decimals are `{price:.2}`. A format chosen while the program runs goes
through `format.apply(value, "08.2")`, which gives a result, since the text
may not be a format.

## Raw and multi-line strings

`r"..."` is a string with no escapes and no interpolation, for regular
expressions and paths. `"""` begins a string of several lines:

```tondo
import std.console

fn main() {
    let pattern = r"\d{3}-\d{4}"
    let name = "Ada"
    let message = """
        Dear {name},

          your order has shipped.
        """
    let query = r"""
        SELECT "name" FROM users WHERE note LIKE '{%}'
        """
    console.println(pattern)
    console.println(message)
    console.println(query)
}
```

```text output
\d{3}-\d{4}
Dear Ada,

  your order has shipped.
SELECT "name" FROM users WHERE note LIKE '{%}'
```

In a multi-line string, the indentation of the closing `"""` is taken off
every line, so the text can be indented with the code around it. The line
break after the opening `"""` and the one before the closing `"""` are not
part of the text. A multi-line string interpolates like any other, and
`r"""` makes one that does not.

## Length, indexes and slices

Positions in a string count characters, not bytes:

```tondo
import std.console

fn main() {
    let word = "año"
    console.println("{word.length()} {word.byteLength()} {word.isEmpty()}")
    console.println("{word[0]} {word[1]} {word[-1]} {word.get(9)}")
    console.println("{word[1:]} {word[:2]} {word.slice(0, 2)} {"hello"[::-1]}")
    for letter in word.chars() {
        console.println("{letter} {letter.code()}")
    }
}
```

```text output
3 4 false
a ñ o none
ño añ ok(añ) olleh
a 97
ñ 241
o 111
```

- `text[i]` is the character at `i` and panics when there is none;
  `text.get(i)` gives an option.
- A slice with `:` gives a string and brings its ends in, as an array's
  does. `slice(start, end)` gives a result instead, for positions that come
  from outside.
- `for c in text.chars()` is the way to walk a string. Indexing an ASCII
  string is direct, and in another the string remembers where the last index
  was, so a loop over the indexes is still linear.

A `Char` is one Unicode scalar. What a reader sees as one character may be
several, as a flag or a letter with a combining accent: Tondo counts scalars,
and [Unicode, for real](#unicode-for-real) says how to deal with the rest.

## The methods of a string

`tondo api String` lists them. Every one gives a new string, since a string
never changes.

| To | Use |
| --- | --- |
| ask | `contains`, `startsWith`, `endsWith`, `find`, `findLast`, `count`, `isEmpty` |
| cut | `split`, `splitOnce`, `lines`, `words`, `chars`, slices |
| trim | `trim`, `trimStart`, `trimEnd`, `removePrefix`, `removeSuffix` |
| change | `replace`, `toUpper`, `toLower`, `repeat`, `padStart`, `padEnd`, `concat` |
| compare | `==`, `<`, `compare`, `equalsIgnoringCase` |

```tondo
import std.console

fn main() {
    let line = "  name = Ada Lovelace  "
    let (key, value) = line.trim().splitOnce(" = ").unwrapOr(("", ""))
    console.println("[{key}] [{value}] {value.words()} {value.toUpper()}")

    console.println("{"a,b,,c".split(",")} {"a=b=c".split("=", limit: 2)} {"one\ntwo".lines()}")
    console.println("{"--x--".trim("-")} {"report.pdf".removeSuffix(".pdf")} {"7".padStart(3, "0")} {"ab".repeat(3)}")
    console.println("{"banana".count("an")} {"banana".find("an")} {"banana".findLast("an")} {"banana".replace("a", "o")}")
    console.println("{"Straße".equalsIgnoringCase("STRASSE")} {"apple" < "banana"} {"b".compare("a")}")
}
```

```text output
[name] [Ada Lovelace] [Ada, Lovelace] ADA LOVELACE
[a, b, , c] [a, b=c] [one, two]
x report 007 ababab
2 some(1) some(3) bonono
true true 1
```

Strings compare by their characters, in the order Unicode gives them. That
is the right order for a sorted index and the wrong one for a list people
read, which wants a language's rules: see [Text for people](#text-for-people).
For patterns, `std.regex` ([its page](../std/regex.md)).

## Building text

A string that is made in many steps is built with `format.Builder`, which
collects pieces and gives the text at the end:

```tondo
import std.console
import std.format

fn table(rows: Array[(String, Int)]): String {
    var out = format.Builder()
    out.addLine("name       score")
    for (name, score) in rows {
        out.addLine("{name:<10} {score:>5}")
    }
    out.add("{rows.length()} rows")
    out.text()
}

fn main() {
    console.println(table([("ada", 95), ("bo", 7)]))
}
```

```text output
name       score
ada           95
bo             7
2 rows
```

For a list of things, `parts.join(", ")` is usually all it takes. Text that
goes straight to a file, a socket or the console is written there with an
`io.Writer`, without building it first.

## Characters

A `Char` is written between single quotes. It knows what Unicode says of it:

```tondo
import std.console

fn main() {
    let letter = 'ñ'
    console.println("{letter.isLetter()} {letter.isUpper()} {letter.toUpper()} {letter.code()}")
    console.println("{'7'.isDigit()} {' '.isWhitespace()} {'!'.isPunctuation()} {'€'.isSymbol()}")
    console.println("{Char.from(955)} {Char.from(0x1F642)} {'a' < 'b'} {('a'..='e').collect()}")

    let text = "Hello, World 2024!"
    let letters = text.chars().filter((c) { c.isLetter() }).count()
    let initials = text.words().map((word) { word[0] }).filter((c) { c.isUpper() })
    console.println("{letters} letters, initials {initials}")
}
```

```text output
true false Ñ 241
true true true true
some(λ) some(🙂) true [a, b, c, d, e]
10 letters, initials [H, W]
```

`Char.from(code)` gives an option, since not every number is a character.
Characters compare by their code, go in ranges, and are keys of maps.

## Bytes

A `Byte` is a byte of data: what a file, a socket or a hash is made of. It
is not a small number, and has no arithmetic.

```tondo
import std.console
import std.encoding

fn main(): !encoding.EncodingError {
    let text = "héllo"
    let bytes = text.bytes()
    console.println("{bytes.length()} {bytes[0]} {encoding.hexEncode(bytes)}")
    console.println("{String.fromBytes(bytes)} {String.fromBytes([0xff, 0xfe]).isErr()}")

    let magic: Array[Byte] = [0x89, 0x50, 0x4E, 0x47]
    let encoded = encoding.base64Encode(magic)
    console.println("{encoded} {encoding.base64Decode(encoded)? == magic}")
    console.println("{Int(magic[0]) + 1} {Byte(200).isOk()} {Byte(300).isOk()} {magic[1] | 0x20}")
}
```

```text output
6 104 68c3a96c6c6f
ok(héllo) true
iVBORw== true
138 true false 112
```

- `text.bytes()` gives a string's UTF-8 bytes, and `String.fromBytes(bytes)`
  checks them: bytes that are not valid UTF-8 are an error, never a broken
  string.
- A literal from 0 to 255 is a `Byte` where one is expected. Any other
  integer becomes one with `Byte(n)`, which may fail, and a byte becomes a
  number with `Int(b)`.
- Bytes have the bit operators, `&`, `|`, `^`, `~`, `<<` and `>>`, and
  compare. `std.encoding` writes them as Base64 and hexadecimal, and
  `std.hash` and `std.crypto` work on them.

## Text and numbers

Nothing converts between a number and its text by itself:

```tondo
import std.console

fn main() {
    console.println("{Int.parse("42")} {Int.parse("4x")} {Int.parse("ff", radix: 16)} {Int.parse("1_000")}")
    console.println("{Float.parse("2.5")} {Float.parse("1e3")} {UInt8.parse("300").isErr()}")
    let n = 255
    console.println("{n} {n.toString(radix: 2)} {n.toString(radix: 16)} {3.14159.toFixed(2)}")
    let port = Int.parse("80a").unwrapOr(8080)
    console.println("{port}")
}
```

```text output
ok(42) err(invalid data) ok(255) ok(1000)
ok(2.5) ok(1000.0) true
255 11111111 ff 3.14
8080
```

Parsing gives a result, because text from outside may not be a number.
Writing a number is interpolation, with a format when one is needed.

## Unicode, for real

Two strings that look the same may hold different scalars: `é` can be one
scalar, or `e` followed by a combining accent. `std.text` deals with what
`String` leaves alone.

```tondo
import std.console
import std.text

fn main() {
    let composed = "caf\u{e9}"              // é as one scalar
    let combining = "cafe\u{301}"           // e, then a combining accent
    console.println("{composed == combining} {composed.length()} {combining.length()}")
    let normal = text.normalize(combining)
    console.println("{composed == normal} {text.isNormalized(combining)}")

    // Taking the accents off: decompose, then drop the marks.
    let decomposed = text.normalize("Crème brûlée", text.Normalization.Nfd)
    let plain = decomposed.chars().filter((c) { not c.isMark() }).collect().join("")
    console.println(plain)
    console.println("{text.category('λ')} {text.script('λ')} {text.script('字')}")
}
```

```text output
false 4 5
true false
Creme brulee
LowercaseLetter Greek Han
```

Normalize text that comes from outside before comparing it or using it as a
key. Tondo never does it behind the program's back: a string is exactly the
scalars it was given.

## Text for people

Sorting, numbers and plurals depend on the language of whoever reads them:

```tondo
import std.console
import std.format
import std.text

fn main() {
    let names = ["Zoe", "ñandú", "nube", "Ana", "ana"]
    let spanish = text.collator("es")
    console.println("{names.sorted()}")
    console.println("{names.sortedWith((a, b) { spanish.compare(a, b) })}")

    let files = ["file10", "file2", "file1"]
    let natural = text.collator("en", numeric: true)
    console.println("{files.sortedWith((a, b) { natural.compare(a, b) })}")

    console.println("{format.number(1234567.891, "en")} | {format.number(1234567.891, "de")} | {format.number(1234567.891, "fr")}")
    console.println("{format.currency(9.5, "EUR", "es")} | {format.percent(0.256, decimals: 1)}")
}
```

```text output
[Ana, Zoe, ana, nube, ñandú]
[ana, Ana, nube, ñandú, Zoe]
[file1, file2, file10]
1,234,567.891 | 1.234.567,891 | 1 234 567,891
9,50 € | 25.6%
```

A program carries the data of only the languages it uses.
`format.plural(count, locale)` says which form of a message a count takes,
`std.time` writes dates the same way, and [`std.text`](../std/text.md) and
[`std.format`](../std/format.md) have the rest.

Next: [Numbers](10-numbers.md).
