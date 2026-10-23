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
}
```

- Interpolation `{expr}` takes any value with `Display`; `{{` and `}}` write
  braces. `+` does not join strings: interpolate, or `a.concat(b)`.
- `text[i]` is the character at `i` and panics out of range; `text.get(i)`
  gives a `Char?`, and `slice(start, end)` a `String ! TextError`.
- `String.fromBytes(bytes)` checks UTF-8; `text.bytes()` gives them.
- `std.format.Builder` builds long text piece by piece; `std.regex` matches
  patterns; `std.encoding` turns bytes into Base64, hexadecimal or URL text.
