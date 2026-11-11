# std.encoding

binary data as text and back, in Base64, hexadecimal and the percent-encoding of URLs, and text in HTML.

```tondo
import std.console
import std.encoding

let text = encoding.base64Encode("hello".bytes())
let back = String.fromBytes(encoding.base64Decode(text)?)?
console.println("{text} {back} {encoding.hexEncode([255, 1])} {encoding.urlEncode("a b&c")}")
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.encoding` shows the same in a terminal.

- [Functions](#functions): [`base64Encode`](#base64encode), [`base64Decode`](#base64decode), [`hexEncode`](#hexencode), [`hexDecode`](#hexdecode), [`urlEncode`](#urlencode), [`urlDecode`](#urldecode), [`htmlEscape`](#htmlescape), [`htmlUnescape`](#htmlunescape)
- [Base64Alphabet](#base64alphabet)
- [EncodingError](#encodingerror)

### Functions

#### base64Encode

```tondo
pub fn base64Encode(bytes: Array[Byte], alphabet: Base64Alphabet = Base64Alphabet.Standard): String
```

The Base64 text of the bytes.

#### base64Decode

```tondo
pub fn base64Decode(
    input: String,
    alphabet: Base64Alphabet = Base64Alphabet.Standard,
): Array[Byte] ! EncodingError
```

The bytes a Base64 text writes, with or without its padding.

#### hexEncode

```tondo
pub fn hexEncode(bytes: Array[Byte]): String
```

The lowercase hexadecimal text of the bytes, two digits each.

#### hexDecode

```tondo
pub fn hexDecode(input: String): Array[Byte] ! EncodingError
```

The bytes a hexadecimal text writes, in either case.

#### urlEncode

```tondo
pub fn urlEncode(input: String): String
```

The text with each byte of its UTF-8 written as `%XX`, except letters,
digits, `-`, `.`, `_` and `~` (RFC 3986): a path segment or a query
value that keeps its meaning in a URL.

#### urlDecode

```tondo
pub fn urlDecode(input: String): String ! EncodingError
```

The text a percent-encoded text writes; `+` stays `+`.

#### htmlEscape

```tondo
pub fn htmlEscape(input: String): String
```

The text with `&`, `<`, `>`, `"` and `'` written as references, safe in
HTML's content and in a quoted attribute.

#### htmlUnescape

```tondo
pub fn htmlUnescape(input: String): String
```

The text with HTML5's named references, as `&eacute;`, and numeric ones,
as `&#233;`, read; what is not one stays as it is.

### Base64Alphabet

```tondo
pub enum Base64Alphabet {
    Standard
    Url
}
```

The alphabets of Base64 (RFC 4648): `Standard`, with `+`, `/` and `=`
padding, and `Url`, with `-` and `_` and without padding.

```tondo
import std.console
import std.encoding

let token = encoding.base64Encode([251, 255], alphabet: encoding.Base64Alphabet.Url)
console.println("{token} {encoding.base64Decode(token, alphabet: encoding.Base64Alphabet.Url)?}")
```

### EncodingError

```tondo
pub enum EncodingError {
    /// A character the encoding does not use, at this byte offset.
    InvalidCharacter(Int)
    /// The text ends in the middle of a value.
    Truncated
    /// What the text encodes is not UTF-8.
    NotUtf8
}
```

Why a text does not decode.

```tondo
import std.console
import std.encoding

match encoding.hexDecode("0g") {
    ok(bytes) => console.println("{bytes}")
    err(encoding.EncodingError.InvalidCharacter(at)) => console.println("not hexadecimal at byte {at}")
    err(error) => console.println("{error}")
}
```

Implements `Display`.
