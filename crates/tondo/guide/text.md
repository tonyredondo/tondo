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
  `{price:.2}`, not `{price:.2f}`. The format is literal.
- `text[i]` is the character at `i` and panics out of range; `text.get(i)`
  gives a `Char?`, and `slice(start, end)` a `String ! TextError`.
- `String.fromBytes(bytes)` checks UTF-8; `text.bytes()` gives them.
- `std.format.Builder` builds long text piece by piece; `std.regex` matches
  patterns; `std.encoding` turns bytes into Base64, hexadecimal or URL text.
