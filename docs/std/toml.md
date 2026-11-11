# std.toml

TOML documents as values of `Value`, and any type read from and written to TOML through the traits of `std.serialization`, on top of the runtime's parser and writer (TOML 1.1).

```tondo
import std.console
import std.toml

type Server = { host: String, port: Int }

let server = toml.decode[Server]("host = \"localhost\"\nport = 8080\n")?
console.println("{server.host}:{server.port}")
console.print(toml.encode(server with { port: 9090 })?)
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.toml` shows the same in a terminal.

- [Functions](#functions): [`decode`](#decode), [`encode`](#encode), [`parse`](#parse)
- [Value](#value): [`Value.get`](#valueget), [`Value.at`](#valueat), [`Value.find`](#valuefind), [`Value.asText`](#valueastext), [`Value.asInt`](#valueasint), [`Value.asFloat`](#valueasfloat), [`Value.asBool`](#valueasbool), [`Value.asDateTime`](#valueasdatetime), [`Value.asArray`](#valueasarray), [`Value.asTable`](#valueastable)
- [TomlError](#tomlerror)
- [UnknownFields](#unknownfields)

### Functions

#### decode

```tondo
pub fn decode[T: serialization.Decode](
    text: String,
    unknownFields: UnknownFields = UnknownFields.Ignore,
): T ! TomlError
```

A value of `T` from a TOML document.

#### encode

```tondo
pub fn encode[T: serialization.Encode](value: T): String ! TomlError
```

The TOML document of a value, which must be a record or a map: a
document is a table. A `none` field is left out; one inside an array
cannot be written.

#### parse

```tondo
pub fn parse(text: String): Value ! TomlError
```

A document of unknown shape.

### Value

```tondo
pub enum Value {
    Text(String)
    Int(Int)
    Float(Float)
    Bool(Bool)
    /// A date, a time or both, as written: `1979-05-27T07:32:00Z`.
    DateTime(String)
    Array(Array[Value])
    Table(Map[String, Value])
}
```

A TOML value of unknown shape; a document is a `Table`.

```tondo
import std.console
import std.toml

let config = toml.parse("[server]\nhost = \"localhost\"\nports = [8080, 8081]\n")?
let host = config.find("server.host").andThen((value) { value.asText() })
let port = config.find("server.ports").andThen((ports) { ports.at(1) }).andThen((value) { value.asInt() })
console.println("{host.unwrapOr("?")}:{port.unwrapOr(0)}")
```

Implements `Display`, `reflect.Reflect`, `serialization.Encode`, `serialization.Decode`.

#### Value.get

```tondo
pub fn Value.get(self, key: String): Value?
```

The member `key` of a table.

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

#### Value.asTable

```tondo
pub fn Value.asTable(self): Map[String, Value]?
```

The keys and values, or `none` when the value is not a table.

### TomlError

```tondo
pub alias TomlError = serialization.Error
```

A failure to read or write TOML, with where it happened: the error of
every format.

```tondo
import std.console
import std.toml

type Server = { port: Int }

match toml.decode[Server]("port = \"eighty\"\n") {
    ok(server) => console.println("port {server.port}")
    err(error) => console.println("line {error.line}, {error.path}: {error.message}")
}
```

### UnknownFields

```tondo
pub alias UnknownFields = serialization.UnknownFields
```

What reading does with a member that the type has no field for.

```tondo
import std.console
import std.toml

type Server = { port: Int }

let text = "port = 8080\nprot = 8081\n"
match toml.decode[Server](text, unknownFields: toml.UnknownFields.Reject) {
    ok(server) => console.println("port {server.port}")
    err(error) => console.println("rejected: {error.message}")
}
```
