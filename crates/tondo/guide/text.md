# Text

`String` is UTF-8 and immutable; positions are character indexes. `Char` is
one Unicode scalar, written `'a'`.

```tondo
import std.console

fn main() {
    let name = "Ada"
    let line = "Hello, {name}! {1 + 2} {{literal braces}}"
    let raw = r"no {interpolation} \n here"
    let poem = """
        Lines keep
        their breaks.
        """
    console.println(line)
    console.println(raw)
    console.print(poem)
    console.println("{name.length()} {name.toUpper()} {name.get(0)} {name.slice(1, 3)}")
    console.println("{"a,b,c".split(",")} {"  x ".trim()} {"abc".contains("b")} {"abc".replace("b", "B")}")
    console.println("{"42".startsWith("4")} {Int.parse("42")} {Float.parse("2.5")} {"x".repeat(3)}")
    console.println("{"k=a=b".splitOnce("=")} {"a=b=c".split("=", limit: 2)} {"--x--".trim("-")}")
    console.println("{"Straße".equalsIgnoringCase("STRASSE")}")
    let joined = ["a", "b"].join("-")
    let padded = "7".padStart(3, "0")
    console.println("{joined} {padded} {"héllo".chars().count()} {"hé".bytes()}")
    let price = 3.5
    console.println("{name:<6}|{price:>8.2}|{42:08}|{255:#x}|{1500.0:e}|{price:+.1}")
}
```

- Interpolation `{expr}` takes any value with `Display`; `{{` and `}}` write
  braces. `+` does not join strings: interpolate, or `a.concat(b)`.
- A format after `:` writes it as Python's and Rust's do:
  `[[fill]align][+][#][0][width][.precision][type]`. `<`, `>` and `^` align,
  `.2` is two decimals of a float, `08` fills a number with zeros, `x`, `o`
  and `b` write an integer in hexadecimal, octal or binary (`#x` with
  `0x`), and `e` a float in scientific notation. There is no `f` or `d`:
  `{price:.2}`, not `{price:.2f}`. A format chosen while the program runs
  goes through `format.apply(value, spec)`, which gives `String ! FormatError`.
- `text[i]` is the character at `i` and panics out of range; `text.get(i)`
  gives a `Char?`, and `slice(start, end)` a `String ! TextError`.
- `String.fromBytes(bytes)` checks UTF-8; `text.bytes()` gives them.
- `std.format.Builder` builds long text piece by piece; `std.regex` matches
  patterns; `std.encoding` turns bytes into Base64, hexadecimal or URL text.
- Numbers for people go through `std.format`, which knows how each language
  writes them: `format.number(1234.5, "de")` is `1.234,5`,
  `format.currency(9.5, "EUR", "es")` is `9,50 €`, `format.percent(0.25)` is
  `25%`, and `format.plural(count, locale)` picks the message for a count.
