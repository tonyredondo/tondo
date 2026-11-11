# std.cbor

CBOR (RFC 8949) as values of `Value`, and any type read from and written to CBOR through the traits of `std.serialization`, on top of the runtime's decoder and encoder.

```tondo
import std.cbor
import std.console

type Point = { x: Int, y: Int }

let bytes = cbor.encode(Point { x: 1, y: 2 })
let point = cbor.decode[Point](bytes)?
console.println("{bytes.length()} bytes: {point.x}, {point.y}")
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.cbor` shows the same in a terminal.

- [Functions](#functions): [`decode`](#decode), [`encode`](#encode), [`parse`](#parse)
- [Value](#value): [`Value.isNull`](#valueisnull), [`Value.get`](#valueget), [`Value.at`](#valueat), [`Value.find`](#valuefind), [`Value.asText`](#valueastext), [`Value.asInt`](#valueasint), [`Value.asFloat`](#valueasfloat), [`Value.asBool`](#valueasbool), [`Value.asBytes`](#valueasbytes), [`Value.asDateTime`](#valueasdatetime), [`Value.asArray`](#valueasarray), [`Value.asMap`](#valueasmap)
- [CborError](#cborerror)
- [UnknownFields](#unknownfields)

### Functions

#### decode

```tondo
pub fn decode[T: serialization.Decode](
    input: Array[Byte],
    unknownFields: UnknownFields = UnknownFields.Ignore,
): T ! CborError
```

A value of `T` from CBOR bytes, which must hold exactly one data item.

#### encode

```tondo
pub fn encode[T: serialization.Encode](value: T, deterministic: Bool = false): Array[Byte]
```

The CBOR bytes of a value in the preferred serialization: the shortest
heads and floats, and definite lengths. `deterministic` orders the keys
of every map by their encoding (RFC 8949, 4.2). An `Encode` written by
hand that declares another count of items than it writes panics.

#### parse

```tondo
pub fn parse(input: Array[Byte]): Value ! CborError
```

A value of unknown shape.

### Value

```tondo
pub enum Value {
    Null
    Bool(Bool)
    Int(Int)
    Float(Float)
    Text(String)
    Bytes(Array[Byte])
    /// A date and time (tag 0 or 1), as RFC 3339 text in UTC.
    DateTime(String)
    Array(Array[Value])
    Map(Map[String, Value])
}
```

A CBOR data item of unknown shape. Map keys are text, or integers as
their digits; `undefined` is `Null`.

```tondo
import std.cbor
import std.console

type Point = { x: Int, y: Int }

let value = cbor.parse(cbor.encode(Point { x: 1, y: 2 }))?
console.println("{value.get("x").andThen((x) { x.asInt() })} {value.find("y")}")
```

Implements `Display`, `serialization.Encode`, `serialization.Decode`.

#### Value.isNull

```tondo
pub fn Value.isNull(self): Bool
```

Whether the value is null.

#### Value.get

```tondo
pub fn Value.get(self, key: String): Value?
```

The member `key` of a map.

#### Value.at

```tondo
pub fn Value.at(self, index: Int): Value?
```

The element `index` of an array.

#### Value.find

```tondo
pub fn Value.find(self, path: String): Value?
```

The value at a path of keys and indices, as `server.port`,
`items[0].name` or `jobs[".setup"].script`, with a key that holds `.`,
`[`, `]` or `"` in quotes; `""` is the value itself.

#### Value.asText

```tondo
pub fn Value.asText(self): String?
```

The string, or `none` when the value is not one.

#### Value.asInt

```tondo
pub fn Value.asInt(self): Int?
```

The integer, or `none` when the value is not one.

#### Value.asFloat

```tondo
pub fn Value.asFloat(self): Float?
```

A float, or an integer as a float.

#### Value.asBool

```tondo
pub fn Value.asBool(self): Bool?
```

The boolean, or `none` when the value is not one.

#### Value.asBytes

```tondo
pub fn Value.asBytes(self): Array[Byte]?
```

The bytes, or `none` when the value is not a byte string.

#### Value.asDateTime

```tondo
pub fn Value.asDateTime(self): String?
```

The date and time as RFC 3339 text, or `none` when the value is not
one.

#### Value.asArray

```tondo
pub fn Value.asArray(self): Array[Value]?
```

The elements, or `none` when the value is not an array.

#### Value.asMap

```tondo
pub fn Value.asMap(self): Map[String, Value]?
```

The entries, or `none` when the value is not a map.

### CborError

```tondo
pub alias CborError = serialization.Error
```

A failure to read CBOR, with where it happened: the error of every
format.

```tondo
import std.cbor
import std.console

match cbor.decode[Int]([0x61, 0x61]) {
    ok(number) => console.println("{number}")
    err(error) => {
        let failure: cbor.CborError = error
        console.println("{failure.kind}: {failure.message}")
    }
}
```

### UnknownFields

```tondo
pub alias UnknownFields = serialization.UnknownFields
```

What reading does with a member that the type has no field for.

```tondo
import std.cbor
import std.console

type Point = { x: Int }

// A key the type has no field for is ignored, or with `Reject` fails.
let bytes = cbor.encode(["x": 1, "z": 3])
console.println("{cbor.decode[Point](bytes).isOk()}")
console.println("{cbor.decode[Point](bytes, unknownFields: cbor.UnknownFields.Reject).isOk()}")
```
