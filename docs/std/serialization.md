# std.serialization

the events every format reads and writes, and the traits through which a type is written to and read from any of them. The compiler implements `Encode` and `Decode` for every record, enum and newtype whose parts implement them; each format gives a `Source` and a `Sink`.

```tondo
import std.console
import std.json
import std.serialization

match json.decode[Int]("\"text\"") {
    ok(number) => console.println("{number}")
    err(error) => {
        let failure: serialization.Error = error
        console.println("{failure.path}: {failure.message}")
    }
}
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.serialization` shows the same in a terminal.

- [Event](#event)
- [ErrorKind](#errorkind)
- [Note](#note)
- [UnknownFields](#unknownfields)
- [Error](#error)
- [Violation](#violation)
- [Source](#source)
- [Record](#record)
- [Field](#field)
- [Sink](#sink)
- [Encode](#encode)
- [Decode](#decode)
- [Bytes](#bytes)
- [For other types](#for-other-types)

### Event

```tondo
pub enum Event {
    Null
    Bool(Bool)
    Int(Int)
    Float(Float)
    /// A number as a text format writes it, which keeps its precision
    /// until a type takes it.
    Number(String)
    Text(String)
    /// A scalar written as text with no type, as XML writes them: each
    /// scalar type reads it from its text, a number or a `Bool` without
    /// the whitespace around it.
    Untyped(String)
    /// A date, a time or both, as RFC 3339 writes them, in a format that
    /// has them: `1979-05-27T07:32:00Z`, `1979-05-27`, `07:32:00`.
    DateTime(String)
    Bytes(Array[Byte])
    BeginList
    EndList
    BeginMap
    /// The key of the next member of a map.
    Key(String)
    EndMap
}
```

A value as every format reads it: scalars, and the bounds of lists and
maps with the keys of their members.

```tondo
import std.console
import std.json
import std.serialization

/// A temperature, written as a number or as a text such as `"21C"`.
pub type Celsius = { degrees: Float }

impl serialization.Decode for Celsius {
    fn decode[S: serialization.Source](input: S, first: serialization.Event): Celsius ! serialization.Error {
        match first {
            serialization.Event.Text(text) if text.endsWith("C") => match Float.parse(text.removeSuffix("C")) {
                ok(value) => Celsius { degrees: value }
                err(_) => fail input.mismatch(first, "a temperature, as `21C`")
            }
            // Any number, as `Float` reads them: `Int`, `Float`, or the
            // `Number` a text format gives to keep its precision.
            _ => Celsius { degrees: serialization.Decode.decode[Float](input, first)? }
        }
    }
}

let readings = json.decode[Array[Celsius]](r"""[21, 19.5, "22C"]""")?
console.println("{readings.map((reading) { reading.degrees })}")
```

### ErrorKind

```tondo
pub enum ErrorKind {
    /// The input is not in the format.
    Syntax
    /// A value of another kind than the type needs.
    Type
    /// A field the type needs is absent.
    Missing
    /// A field the type does not have, when unknown fields are rejected.
    Unknown
    /// A number that does not fit the type.
    Range
    /// The stream it was read from or written to failed.
    Io
    /// A value the format cannot hold, as a `none` in a TOML array.
    Unsupported
    /// A value that reads but breaks the rules of its type, as
    /// `@validate.Min(18)` says them (14.35): `violations` says which.
    Invalid
}
```

What kind of error reading or writing a value met.

```tondo
import std.console
import std.json
import std.serialization

type Server = { host: String, port: Int }

for text in [r"""{"host": "a"}""", r"""{"host": "a", "port": "x"}""", r"""{"host": """] {
    match json.decode[Server](text) {
        ok(_) => console.println("read")
        err(error) => {
            let failure: serialization.Error = error
            let advice = match failure.kind {
                serialization.ErrorKind.Missing => "add the field"
                serialization.ErrorKind.Type => "fix the value's type"
                serialization.ErrorKind.Syntax => "fix the JSON"
                _ => "see the message"
            }
            console.println("{failure.kind}: {advice}")
        }
    }
}
```

Implements `Display`.

### Note

```tondo
pub type Note = {
    format: String
    name: String
    /// The annotation's argument as text; empty without one.
    value: String
}
```

What an annotation of a format says of a type or a field, which the
code the compiler writes for `Encode` and `Decode` gives the format:
`@xml.Name("item")` is `Note { format: "xml", name: "Name", value:
"item" }`. A format reads its own notes and no other.

```tondo
import std.console
import std.serialization
import std.xml

/// A sink that prints the notes a type's annotations give each format.
pub type Notes = { lines: Array[String] }

impl serialization.Sink for Notes {
    fn null(self) {}
    fn bool(self, _: Bool) {}
    fn int(self, _: Int) {}
    fn float(self, _: Float) {}
    fn number(self, _: String) {}
    fn text(self, _: String) {}
    fn bytes(self, _: Array[Byte]) {}
    fn beginList(self, _: Int) {}
    fn endList(self) {}
    fn beginMap(self, _: Int) {}
    fn key(self, _: String) {}
    fn endMap(self) {}

    fn field(self, field: serialization.Field) {
        for note in field.notes {
            console.println("{note.format}.{note.name}({note.value})")
        }
    }
}

pub type Item = {
    @xml.Attribute
    id: Int
    @xml.Name("label")
    title: String
}

Item { id: 1, title: "Tea" }.encode(Notes { lines: [] })
```

### UnknownFields

```tondo
pub enum UnknownFields {
    Ignore
    Reject
}
```

What reading does with a member of a map that the type has no field
for.

```tondo
import std.console
import std.json
import std.serialization

type Server = { port: Int }

let strict = serialization.UnknownFields.Reject
match json.decode[Server](r"""{"port": 80, "prot": 81}""", unknownFields: strict) {
    ok(server) => console.println("port {server.port}")
    err(error) => console.println("{error.message}")
}
```

### Error

```tondo
pub type Error = {
    kind: ErrorKind
    message: String
    /// Where in the document, as `$.users[2].age`.
    path: String
    line: Int
    column: Int
    /// The rules a value of kind `Invalid` breaks, each of its fields.
    violations: Array[Violation]
}
```

A failure to read or write a format, and where.

```tondo
import std.console
import std.yaml

type User = { name: String, age: Int }

match yaml.decode[Array[User]]("- name: Ada\n  age: 36\n- name: Bo\n  age: old\n") {
    ok(users) => console.println("{users.length()} users")
    err(error) => console.println("{error.kind} at {error.path}, line {error.line}: {error.message}")
}
```

Implements `Display`, `reflect.Reflect`.

### Violation

```tondo
pub type Violation = {
    path: String
    message: String
}
```

A rule of its type that a value breaks: the path of the field, from the
value, and what the rule asks, as `age` and `must be at least 18`.

```tondo
import std.console
import std.json
import std.validate

pub type Account = {
    @validate.Min(18)
    age: Int
}

match json.decode[Account](r"""{"age": 12}""") {
    ok(account) => console.println("age {account.age}")
    err(error) => {
        for violation in error.violations {
            console.println("{violation.path}: {violation.message}")
        }
    }
}
```

Implements `Display`.

### Source

```tondo
pub trait Source {
    /// The next event, or `none` after the whole document.
    fn next(self): Event? ! Error

    /// Skips the rest of a value whose first event was `first`.
    fn skip(self, first: Event): !Error

    /// An error at the current position, with its path, line and column.
    fn failure(self, kind: ErrorKind, message: String): Error

    /// Whether a field the type does not have is an error.
    fn rejectsUnknown(self): Bool

    /// The tag of its own the value just read has, in a format that has
    /// them, as YAML's `!reference`; `none` without one.
    fn tag(self): String?

    /// The value whose first event was `first` as the JSON text it was
    /// read from, for `json.Raw`; `none` from another format.
    fn rawJson(self, first: Event): String?

    /// What the format calls a map and a list, for messages.
    fn mapName(self): String

    fn listName(self): String

    /// Before the fields of a record are read, when some have notes: the
    /// notes of each, in the order of the names `nextField` takes.
    fn fields(self, notes: Array[Array[Note]])

    /// The next event, which must exist.
    fn value(self): Event ! Error

    /// The next key of a map, or `none` at its end.
    fn nextKey(self): String? ! Error

    /// The next member of a map, read up to its value: the index of its
    /// key in `names`, or `none` at the map's end. A member whose key is
    /// none of them is skipped whole, or fails when unknown fields are
    /// rejected, and gives `names.length()`. A format that reads keys in
    /// place compares them there, with no string made for each.
    fn nextField(self, names: Array[String]): Int? ! Error

    /// A member the type does not have: skipped, or an error when unknown
    /// fields are rejected.
    fn unknownField(self, key: String, first: Event): !Error

    /// Checks that a map starts, for the type `name`.
    fn beginMap(self, first: Event, name: String): !Error

    /// Checks that a list starts, for `name`.
    fn beginList(self, first: Event, name: String): !Error

    /// Checks that a list ends.
    fn endList(self): !Error

    /// A field that must have been present.
    fn required[T: ~Copy](self, value: T?, field: String): T ! Error

    /// A list field the input left out: missing, but in a format that writes
    /// an empty list as nothing, as XML, empty.
    fn missingList[T](self, field: String): Array[T] ! Error

    /// The error of a value that breaks the rules of its type.
    fn invalid(self, violations: Array[Violation]): Error

    /// An error for a value of the wrong kind.
    fn mismatch(self, first: Event, expected: String): Error
}
```

A document read event by event: the reader of a format.

```tondo
import std.console
import std.json
import std.serialization

/// Every number in a document, wherever it is, read from its events.
pub type Numbers = { values: Array[Float] }

impl serialization.Decode for Numbers {
    fn decode[S: serialization.Source](input: S, first: serialization.Event): Numbers ! serialization.Error {
        var values: Array[Float] = []
        var depth = 0
        var event = first
        for {
            match event {
                serialization.Event.Int(_) => values.push(serialization.Decode.decode[Float](input, event)?)
                serialization.Event.Float(value) => values.push(value)
                serialization.Event.Number(_) => values.push(serialization.Decode.decode[Float](input, event)?)
                serialization.Event.BeginList => depth += 1
                serialization.Event.BeginMap => depth += 1
                serialization.Event.EndList => depth -= 1
                serialization.Event.EndMap => depth -= 1
                _ => {}
            }
            if depth == 0 {
                break
            }
            event = input.value()?
        }
        Numbers { values }
    }
}

let found = json.decode[Numbers](r"""{"a": 1, "b": [2.5, {"c": 3}], "d": "x"}""")?
console.println("{found.values}")
```

### Record

```tondo
pub type Record = {
    name: String
    count: Int
    notes: Array[Note]
}
```

A record about to be written: the name of its type, or of its variant,
how many members it has, and the notes of the type. `Sink.beginRecord`
takes it.

```tondo
import std.console
import std.serialization

let record = serialization.Record { name: "Item", count: 2, notes: [] }
console.println("{record.name} has {record.count} members")
```

### Field

```tondo
pub type Field = {
    name: String
    notes: Array[Note]
}
```

A field about to be written: its key and its notes. `Sink.field` takes it.

```tondo
import std.console
import std.serialization

let field = serialization.Field { name: "title", notes: [] }
console.println("{field.name} has {field.notes.length()} notes")
```

### Sink

```tondo
pub trait Sink {
    fn null(self)

    fn bool(self, value: Bool)

    fn int(self, value: Int)

    fn float(self, value: Float)

    /// A number as a text format writes it.
    fn number(self, digits: String)

    fn text(self, value: String)

    fn bytes(self, value: Array[Byte])

    /// A date, a time or both, as RFC 3339 writes them; as text in a
    /// format without them.
    fn dateTime(self, text: String)

    /// The map of a record follows, for a format that names its values, as
    /// XML its elements.
    fn beginRecord(self, record: Record)

    /// The key of a field with notes.
    fn field(self, field: Field)

    /// Before a value: a tag of its own, as YAML's `!reference`, for a
    /// format that has them.
    fn tag(self, name: String)

    /// A list of `count` values follows, then its end.
    fn beginList(self, count: Int)

    fn endList(self)

    /// A map of `count` members follows, each a key and a value, then its
    /// end.
    fn beginMap(self, count: Int)

    fn key(self, name: String)

    fn endMap(self)

    /// Whether a map leaves out its members that are `none`, as in a
    /// format without null.
    fn omitsNone(self): Bool

    /// Writes JSON text as it is, for `json.Raw`; false when the format
    /// does not take it, and the value is written event by event instead.
    fn rawJson(self, text: String): Bool
}
```

A document written value by value: the writer of a format. A value it
cannot hold is an error that the format reports when it finishes.

```tondo
import std.console
import std.json
import std.serialization

/// A point written as `[x, y]` rather than as a map.
pub type Point = { x: Int, y: Int }

impl serialization.Encode for Point {
    fn encode[S: serialization.Sink](self, out: S) {
        out.beginList(2)
        out.int(self.x)
        out.int(self.y)
        out.endList()
    }
}

console.println(json.encode([Point { x: 1, y: 2 }, Point { x: 3, y: 4 }]))
```

### Encode

```tondo
pub trait Encode {
    fn encode[S: Sink](self, out: S)
}
```

A type that writes itself to any format. The compiler implements it for
every record, enum and newtype whose parts implement it.

```tondo
import std.console
import std.json
import std.serialization
import std.yaml

/// A level written as its name in every format.
pub enum Level {
    Low
    High
}

impl serialization.Encode for Level {
    fn encode[S: serialization.Sink](self, out: S) {
        out.text(match self {
            Level.Low => "low"
            Level.High => "high"
        })
    }
}

pub type Alarm = { name: String, level: Level }

let alarm = Alarm { name: "disk", level: Level.High }
console.println(json.encode(alarm))
console.print(yaml.encode(alarm))
```

### Decode

```tondo
pub trait Decode {
    fn decode[S: Source](input: S, first: Event): Self ! Error
}
```

A type that reads itself from any format, given the first event of its
value. The compiler implements it as `Encode`.

```tondo
import std.console
import std.json
import std.serialization

/// A version written as `"1.2"`, read into its two numbers.
pub type Version = { major: Int, minor: Int }

impl serialization.Decode for Version {
    fn decode[S: serialization.Source](input: S, first: serialization.Event): Version ! serialization.Error {
        let text = serialization.Decode.decode[String](input, first)?
        let parts = text.split(".")
        match (Int.parse(parts[0]), Int.parse(parts.get(1) ?? "")) {
            (ok(major), ok(minor)) => Version { major, minor }
            _ => fail input.mismatch(first, "a version, as `1.2`")
        }
    }
}

pub type Package = { name: String, version: Version }

let package = json.decode[Package](r"""{"name": "tondo", "version": "0.1"}""")?
console.println("{package.name} {package.version.major}.{package.version.minor}")
```

### Bytes

```tondo
pub type Bytes = Array[Byte]
```

Bytes as every format holds them: natively in a binary format, as a
base64 string in a text one.

```tondo
import std.console
import std.json
import std.messagepack
import std.serialization

pub type Blob = { name: String, data: serialization.Bytes }

let blob = Blob { name: "key", data: serialization.Bytes([1, 2, 255]) }
console.println(json.encode(blob))
console.println("{messagepack.encode(blob).length()} bytes in MessagePack")
```

Implements `Encode`, `Decode`.

### For other types

```tondo
impl Encode for Int
impl Decode for Int
impl Encode for Int8
impl Decode for Int8
impl Encode for Int16
impl Decode for Int16
impl Encode for Int32
impl Decode for Int32
impl Encode for Int128
impl Decode for Int128
impl Encode for UInt
impl Decode for UInt
impl Encode for UInt8
impl Decode for UInt8
impl Encode for UInt16
impl Decode for UInt16
impl Encode for UInt32
impl Decode for UInt32
impl Encode for UInt128
impl Decode for UInt128
impl Encode for Byte
impl Decode for Byte
impl Encode for Float
impl Decode for Float
impl Encode for Float32
impl Decode for Float32
impl Encode for Bool
impl Decode for Bool
impl Encode for String
impl Decode for String
impl[T: Encode] Encode for Array[T]
impl[T: Decode] Decode for Array[T]
impl[T: Encode] Encode for T?
impl[T: Decode] Decode for T?
impl[V: Encode] Encode for Map[String, V]
impl[V: Decode] Decode for Map[String, V]
impl[A: Encode, B: Encode] Encode for (A, B)
impl[A: Decode, B: Decode] Decode for (A, B)
impl[A: Encode, B: Encode, C: Encode] Encode for (A, B, C)
impl[A: Decode, B: Decode, C: Decode] Decode for (A, B, C)
impl[A: Encode, B: Encode, C: Encode, D: Encode] Encode for (A, B, C, D)
impl[A: Decode, B: Decode, C: Decode, D: Decode] Decode for (A, B, C, D)
impl[A: Encode, B: Encode, C: Encode, D: Encode, E: Encode] Encode for (A, B, C, D, E)
impl[A: Decode, B: Decode, C: Decode, D: Decode, E: Decode] Decode for (A, B, C, D, E)
impl[A: Encode, B: Encode, C: Encode, D: Encode, E: Encode, F: Encode] Encode for (A, B, C, D, E, F)
impl[A: Decode, B: Decode, C: Decode, D: Decode, E: Decode, F: Decode] Decode for (A, B, C, D, E, F)
impl[A: Encode, B: Encode, C: Encode, D: Encode, E: Encode, F: Encode, G: Encode] Encode for (
    A,
    B,
    C,
    D,
    E,
    F,
    G,
)
impl[A: Decode, B: Decode, C: Decode, D: Decode, E: Decode, F: Decode, G: Decode] Decode for (
    A,
    B,
    C,
    D,
    E,
    F,
    G,
)
impl[A: Encode, B: Encode, C: Encode, D: Encode, E: Encode, F: Encode, G: Encode, H: Encode] Encode for (
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
)
impl[A: Decode, B: Decode, C: Decode, D: Decode, E: Decode, F: Decode, G: Decode, H: Decode] Decode for (
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
)
```
