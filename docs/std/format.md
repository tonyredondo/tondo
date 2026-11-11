# std.format

text built piece by piece, each with its `Display`; values written with a format the program chooses while it runs; and numbers, percents, amounts and plural forms as each language writes them, from the CLDR data that tondo carries.

```tondo
import std.console
import std.format

console.println(format.number(1234567.891, locale: "de", decimals: 2))
console.println(format.currency(19.99, "EUR", locale: "fr"))
console.println(format.percent(0.25))
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.format` shows the same in a terminal.

- [Functions](#functions): [`apply`](#apply), [`number`](#number), [`percent`](#percent), [`currency`](#currency), [`plural`](#plural)
- [Builder](#builder): [`Builder`](#builder-1), [`Builder.add`](#builderadd), [`Builder.addLine`](#builderaddline), [`Builder.isEmpty`](#builderisempty), [`Builder.text`](#buildertext)
- [FormatError](#formaterror)
- [Plural](#plural-1)

### Functions

#### apply

```tondo
pub fn apply[T: Display](value: T, spec: String): String ! FormatError
```

The value written with a format the program chooses while it runs, the
one an interpolation takes after its colon: `format.apply(price,
">10.2")` is `"{price:>10.2}"`. As there, the value's type decides which
options the format may have.

#### number

```tondo
pub fn number[T: Number + Display](value: T, locale: String = "en", decimals: Int? = none): String
```

The number as the language of `locale` writes it, a BCP 47 tag such as
`"es"` or `"de-CH"`: with its digits, its separators of groups and of
decimals and its minus sign, as `1,234.5` in English and `1.234,5` in
German. With `decimals` it is rounded to that many, half to even;
without, it keeps all of its own.

#### percent

```tondo
pub fn percent[T: Number + Display](value: T, locale: String = "en", decimals: Int = 0): String
```

The fraction as a percent in the language of `locale`, rounded to
`decimals`: `format.percent(0.256)` is `26%`, and `26 %` in Spanish.

#### currency

```tondo
pub fn currency[T: Number + Display](amount: T, code: String, locale: String = "en"): String
```

The amount of the currency `code`, as `"EUR"`, in the language of
`locale`, with its symbol there and the decimals the currency has:
`format.currency(1234.5, "EUR", "es")` is `1234,50 €`.

#### plural

```tondo
pub fn plural(count: Int, locale: String = "en"): Plural
```

The plural form a count takes in the language of `locale`: in English
`1` takes `One` and `2` `Other`; in Russian `2` takes `Few` and `5`
`Many`.

### Builder

```tondo
pub type Builder = {
    // private fields
}
```

Text built piece by piece: `add` appends a value's `Display`, in place
when nothing else holds the text.

```tondo
import std.console
import std.format

var text = format.Builder()
for n in 1..=3 {
    text.addLine("line {n}")
}
console.print(text.text())
```

Implements `Display`.

#### Builder

```tondo
pub fn Builder(): Builder
```

An empty builder.

#### Builder.add

```tondo
pub fn Builder.add[T: Display](mut self, value: T)
```

Adds the value as it displays.

#### Builder.addLine

```tondo
pub fn Builder.addLine[T: Display](mut self, value: T)
```

Adds the value and a `\n`.

#### Builder.isEmpty

```tondo
pub fn Builder.isEmpty(self): Bool
```

Whether nothing was added.

#### Builder.text

```tondo
pub fn Builder.text(self): String
```

Everything added, as one string.

### FormatError

```tondo
pub type FormatError = {
    message: String
}
```

Why `apply` could not write a value: its format does not read, or does
not go with the value's type.

```tondo
import std.console
import std.format

match format.apply(3.14159, ".2") {
    ok(text) => console.println(text)
    err(error) => console.println(error.message)
}
```

Implements `Display`.

### Plural

```tondo
pub enum Plural {
    Zero
    One
    Two
    Few
    Many
    Other
}
```

A plural form of CLDR: the form of a word that a count takes in a
language.

```tondo
import std.console
import std.format

fn files(count: Int): String {
    match format.plural(count) {
        format.Plural.One => "1 file"
        _ => "{count} files"
    }
}

console.println("{files(1)}, {files(3)}")
```

Implements `Display`.
