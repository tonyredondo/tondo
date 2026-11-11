# std.messagepack

MessagePack as values of `Value`, and any type read from and written to MessagePack through the traits of `std.serialization`, on top of the runtime's decoder and encoder.

```tondo
import std.console
import std.messagepack

type Point = { x: Int, y: Int }

let bytes = messagepack.encode(Point { x: 1, y: 2 })
let point = messagepack.decode[Point](bytes)?
console.println("{bytes.length()} bytes: {point.x}, {point.y}")
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.messagepack` shows the same in a terminal.

- [Functions](#functions): [`decode`](#decode), [`encode`](#encode), [`parse`](#parse), [`writer`](#writer), [`writerTo`](#writerto)
- [Value](#value): [`Value.isNil`](#valueisnil), [`Value.get`](#valueget), [`Value.at`](#valueat), [`Value.find`](#valuefind), [`Value.asText`](#valueastext), [`Value.asInt`](#valueasint), [`Value.asFloat`](#valueasfloat), [`Value.asBool`](#valueasbool), [`Value.asBytes`](#valueasbytes), [`Value.asTimestamp`](#valueastimestamp), [`Value.asArray`](#valueasarray), [`Value.asMap`](#valueasmap)
- [MessagePackError](#messagepackerror)
- [UnknownFields](#unknownfields)
- [Writer](#writer-1): [`Writer.nil`](#writernil), [`Writer.bool`](#writerbool), [`Writer.int`](#writerint), [`Writer.float`](#writerfloat), [`Writer.string`](#writerstring), [`Writer.bytes`](#writerbytes), [`Writer.beginArray`](#writerbeginarray), [`Writer.beginMap`](#writerbeginmap), [`Writer.value`](#writervalue), [`Writer.finish`](#writerfinish), [`Writer.flush`](#writerflush)

### Functions

#### decode

```tondo
pub fn decode[T: serialization.Decode](
    input: Array[Byte],
    unknownFields: UnknownFields = UnknownFields.Ignore,
): T ! MessagePackError
```

A value of `T` from MessagePack bytes, which must hold exactly one.

#### encode

```tondo
pub fn encode[T: serialization.Encode](value: T): Array[Byte]
```

The MessagePack bytes of a value, every integer and length in its
shortest form. An `Encode` written by hand that declares another count
of items than it writes panics.

#### parse

```tondo
pub fn parse(input: Array[Byte]): Value ! MessagePackError
```

A value of unknown shape.

#### writer

```tondo
pub fn writer(): Writer
```

A writer whose bytes `finish` gives.

#### writerTo

```tondo
pub fn writerTo(stream: io.Writer): Writer
```

A writer that passes its bytes to the stream as they grow; `flush` passes
the rest.

### Value

```tondo
pub enum Value {
    Nil
    Bool(Bool)
    Int(Int)
    /// An integer above `Int.max`, which MessagePack holds up to `UInt.max`.
    UInt(UInt)
    Float(Float)
    Text(String)
    Bytes(Array[Byte])
    /// The timestamp extension, as RFC 3339 text in UTC.
    Timestamp(String)
    Array(Array[Value])
    Map(Map[String, Value])
}
```

A MessagePack value of unknown shape. Map keys are text, or integers as
their digits.

```tondo
import std.console
import std.messagepack

type Point = { x: Int, y: Int }

let value = messagepack.parse(messagepack.encode(Point { x: 1, y: 2 }))?
let x = value.get("x").andThen((x) { x.asInt() })
console.println("x is {x.unwrapOr(0)}, z is set: {value.get("z").isSome()}")
```

Implements `Display`, `serialization.Encode`, `serialization.Decode`.

#### Value.isNil

```tondo
pub fn Value.isNil(self): Bool
```

Whether the value is nil.

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

#### Value.asTimestamp

```tondo
pub fn Value.asTimestamp(self): String?
```

The timestamp as RFC 3339 text, or `none` when the value is not one.

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

### MessagePackError

```tondo
pub alias MessagePackError = serialization.Error
```

A failure to read MessagePack, with where it happened: the error of
every format.

```tondo
import std.console
import std.messagepack

type Point = { x: Int, y: Int }

match messagepack.decode[Point]([0x92, 0x01]) {
    ok(point) => console.println("{point.x}, {point.y}")
    err(error) => console.println("{error.path}: {error.message}")
}
```

### UnknownFields

```tondo
pub alias UnknownFields = serialization.UnknownFields
```

What reading does with a member that the type has no field for.

```tondo
import std.console
import std.messagepack

type Point = { x: Int, y: Int }
type Point3 = { x: Int, y: Int, z: Int }

let bytes = messagepack.encode(Point3 { x: 1, y: 2, z: 3 })
match messagepack.decode[Point](bytes, unknownFields: messagepack.UnknownFields.Reject) {
    ok(point) => console.println("{point.x}, {point.y}")
    err(error) => console.println("rejected: {error.message}")
}
```

### Writer

```tondo
pub type Writer = {
    // private fields
}
```

MessagePack written event by event.
MessagePack written in parts: single values, the header of an array or
a map with how many items follow, and any value with `Encode`. It does not
count the items of a header it writes, so an array can come in pieces: a
payload that gathers its items with `value` and only learns how many at
the end writes their header in another writer and puts it first.

```tondo
import std.console
import std.messagepack

let out = messagepack.writer()
out.beginMap(2)
out.string("name")
out.string("Ada")
out.string("tags")
out.beginArray(1)
out.string("math")
let bytes = out.finish()
console.println("{bytes.length()} bytes: {messagepack.parse(bytes)?.get("name").andThen((name) { name.asText() })}")
```

Implements `serialization.Sink`.

#### Writer.nil

```tondo
pub fn Writer.nil(self)
```

Nil.

#### Writer.bool

```tondo
pub fn Writer.bool(self, value: Bool)
```

A boolean.

#### Writer.int

```tondo
pub fn Writer.int(self, value: Int)
```

An integer, in its shortest form.

#### Writer.float

```tondo
pub fn Writer.float(self, value: Float)
```

A float of 64 bits.

#### Writer.string

```tondo
pub fn Writer.string(self, value: String)
```

Text, as a `str`.

#### Writer.bytes

```tondo
pub fn Writer.bytes(self, value: Array[Byte])
```

Bytes, as a `bin`.

#### Writer.beginArray

```tondo
pub fn Writer.beginArray(self, count: Int)
```

The header of an array of `count` items, which follow.

#### Writer.beginMap

```tondo
pub fn Writer.beginMap(self, count: Int)
```

The header of a map of `count` members, which follow, each a key and a
value.

#### Writer.value

```tondo
pub fn Writer.value[T: serialization.Encode](self, value: T)
```

Any value with `Encode`, as `encode` writes it.

#### Writer.finish

```tondo
pub fn Writer.finish(self): Array[Byte]
```

The bytes written; for a writer to a stream, those not passed to it yet.
An `Encode` written by hand that declares another count of items than it
writes panics, as in `encode`.

#### Writer.flush

```tondo
pub fn Writer.flush(self): !MessagePackError
```

Passes what is pending to the stream of `writerTo`, and reports a failure
to write it.
