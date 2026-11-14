# std.json

JSON read into the program's types and written from them. Any record or
enum is read and written by its fields, with code the compiler writes for
it: there is nothing to derive, no reflection as the program runs, and no
value of the wrong type in a field. JSON of no known shape is a
[`Value`](#value), and a document too large to hold is read and written a
piece at a time.

- [Types as the schema](#types-as-the-schema)
- [JSON of no known shape](#json-of-no-known-shape)
- [What to reach for](#what-to-reach-for)

## Types as the schema

```tondo
import std.console
import std.json

enum Status {
    @name("in_progress")
    InProgress
    Done
}

type Task = {
    title: String
    status: Status = Status.InProgress
    @name("due_on")
    due: String?
    tags: Array[String] = []
}

fn main() {
    let text = r"""{"title": "Write the manual", "due_on": "2026-12-01", "tags": ["docs"]}"""
    match json.decode[Task](text) {
        ok(task) => {
            console.println("{task.title}, {task.status == Status.InProgress}, {task.due}, {task.tags}")
            console.println(json.encode(task with { status: Status.Done }))
        }
        err(error) => console.println("{error}")
    }
    for wrong in [r"""{"status": "Done"}""", r"""{"title": "x", "status": "Paused"}""", r"""{"title": 7}"""] {
        match json.decode[Task](wrong) {
            ok(task) => console.println(task.title)
            err(error) => console.println("{error}")
        }
    }
}
```

```text output
Write the manual, true, some(2026-12-01), [docs]
{"title":"Write the manual","status":"Done","due_on":"2026-12-01","tags":["docs"]}
missing field `title` at $ (line 1, column 19)
unknown variant `Paused` of `Status` at $.status (line 1, column 34)
expected a string, found a number at $.title (line 1, column 12)
```

- A field with a default is optional in the JSON, and so is a `T?` field. A
  field with neither must be there.
- `@name("due_on")` gives a field or a variant its name on the wire,
  `@ignore` leaves a field out, and `@omitNone` writes nothing for a `none`
  where `null` would be written.
- A value of the wrong type is an error that says where: the path, the
  line and the column.
- `@validate` rules on a field are checked by `decode`
  ([`std.validate`](validate.md)).
- The same types read and write [YAML](yaml.md), [TOML](toml.md),
  [CBOR](cbor.md) and [MessagePack](messagepack.md).

## JSON of no known shape

```tondo
import std.console
import std.json

fn main(): !json.JsonError {
    let document = json.parse(r"""{"name": "tondo", "items": [{"id": 7, "tags": ["a", "b"]}], "ratio": 0.5}""")?
    let id = document.find("items[0].id")?.asInt()
    let tags = document.find("items[0].tags")?.asArray() ?? []
    console.println("{id} {tags.length()} {document.find("missing.path").isNone()}")
    console.println(json.encode(document.field("items").item(0), pretty: true))
}
```

```text output
some(7) 2 true
{
  "id": 7,
  "tags": [
    "a",
    "b"
  ]
}
```

## What to reach for

| To | Use |
| --- | --- |
| Read text into a type, or write a value | [`decode`](#decode), [`encode`](#encode) |
| Read from a stream, or write to one | [`decodeFrom`](#decodefrom), [`encodeTo`](#encodeto) |
| Read several values, one after another | [`decodeAll`](#decodeall), [`parseAll`](#parseall) |
| Hold JSON whose shape is not known | [`parse`](#parse), [`Value.find`](#valuefind), [`Value.field`](#valuefield), [`Value.item`](#valueitem) |
| Turn a `Value` into a type, or back | [`fromValue`](#fromvalue), [`toValue`](#tovalue) |
| Read a huge document a piece at a time | [`reader`](#reader), [`Reader.next`](#readernext) |
| Write one a piece at a time | [`writer`](#writer) |
| Keep a piece of JSON as it came | [`raw`](#raw) |

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.json` shows the same in a terminal.

- [Functions](#functions): [`number`](#number), [`reader`](#reader), [`readerFrom`](#readerfrom), [`writer`](#writer), [`writerTo`](#writerto), [`encode`](#encode), [`decode`](#decode), [`parse`](#parse), [`parseAll`](#parseall), [`decodeAll`](#decodeall), [`values`](#values), [`toValue`](#tovalue), [`fromValue`](#fromvalue), [`decodeFrom`](#decodefrom), [`encodeTo`](#encodeto), [`raw`](#raw)
- [Value](#value): [`Value.isNull`](#valueisnull), [`Value.get`](#valueget), [`Value.at`](#valueat), [`Value.find`](#valuefind), [`Value.field`](#valuefield), [`Value.item`](#valueitem), [`Value.asText`](#valueastext), [`Value.asInt`](#valueasint), [`Value.asFloat`](#valueasfloat), [`Value.asBool`](#valueasbool), [`Value.asArray`](#valueasarray), [`Value.asObject`](#valueasobject)
- [Number](#number-1): [`Number.text`](#numbertext), [`Number.toInt`](#numbertoint), [`Number.toFloat`](#numbertofloat)
- [ErrorKind](#errorkind)
- [JsonError](#jsonerror)
- [Event](#event)
- [UnknownFields](#unknownfields)
- [Reader](#reader-1): [`Reader.next`](#readernext), [`Reader.value`](#readervalue), [`Reader.nextKey`](#readernextkey), [`Reader.skip`](#readerskip), [`Reader.unknownField`](#readerunknownfield), [`Reader.beginObject`](#readerbeginobject), [`Reader.beginArray`](#readerbeginarray), [`Reader.endArray`](#readerendarray), [`Reader.required`](#readerrequired), [`Reader.mismatch`](#readermismatch), [`Reader.failure`](#readerfailure)
- [Writer](#writer-1): [`Writer.null`](#writernull), [`Writer.bool`](#writerbool), [`Writer.int`](#writerint), [`Writer.float`](#writerfloat), [`Writer.number`](#writernumber), [`Writer.string`](#writerstring), [`Writer.key`](#writerkey), [`Writer.beginArray`](#writerbeginarray), [`Writer.endArray`](#writerendarray), [`Writer.beginObject`](#writerbeginobject), [`Writer.endObject`](#writerendobject), [`Writer.finish`](#writerfinish), [`Writer.flush`](#writerflush)
- [Values](#values-1)
- [Raw](#raw-1): [`Raw.text`](#rawtext)

### Functions

#### number

```tondo
pub fn number(text: String): Number ! JsonError
```

A number from its JSON text.

#### reader

```tondo
pub fn reader(
    text: String,
    unknownFields: UnknownFields = UnknownFields.Ignore,
    allowDuplicateKeys: Bool = false,
): Reader
```

A reader of a JSON document, event by event: members a type does not
have are ignored or rejected as `unknownFields` says, and a key repeated
in an object is an error unless `allowDuplicateKeys`.

#### readerFrom

```tondo
pub fn readerFrom(
    stream: io.Reader,
    unknownFields: UnknownFields = UnknownFields.Ignore,
    allowDuplicateKeys: Bool = false,
): Reader
```

A JSON document read event by event from a stream, as it arrives: only
the part being read is in memory.

#### writer

```tondo
pub fn writer(pretty: Bool = false, canonical: Bool = false, htmlSafe: Bool = false): Writer
```

`pretty` indents the output; `canonical` writes RFC 8785 (JCS): sorted
keys, no whitespace and numbers as doubles. `htmlSafe` escapes `<`, `>`,
`&`, U+2028 and U+2029 in texts, so the output can go inside a page's
`<script>`; canonical output has one way to write each text, and leaves
it out.

#### writerTo

```tondo
pub fn writerTo(
    stream: io.Writer,
    pretty: Bool = false,
    canonical: Bool = false,
    htmlSafe: Bool = false,
): Writer
```

A JSON document written value by value to a stream, which receives the
output as it grows.

#### encode

```tondo
pub fn encode[T: serialization.Encode](
    value: T,
    pretty: Bool = false,
    canonical: Bool = false,
    htmlSafe: Bool = false,
): String
```

The JSON text of a value, as `writer` writes it.

#### decode

```tondo
pub fn decode[T: serialization.Decode](
    text: String,
    unknownFields: UnknownFields = UnknownFields.Ignore,
    allowDuplicateKeys: Bool = false,
): T ! JsonError
```

A value of `T` from JSON text, which must hold exactly one value.

#### parse

```tondo
pub fn parse(text: String): Value ! JsonError
```

A document of unknown shape.

#### parseAll

```tondo
pub fn parseAll(text: String): Array[Value] ! JsonError
```

The values of JSON text that holds several, one after another with or
without space between them, as `go list -json` writes them.

#### decodeAll

```tondo
pub fn decodeAll[T: serialization.Decode](
    text: String,
    unknownFields: UnknownFields = UnknownFields.Ignore,
    allowDuplicateKeys: Bool = false,
): Array[T] ! JsonError
```

Values of `T` from JSON text that holds several, one after another.

#### values

```tondo
pub fn values[T: serialization.Decode](
    stream: io.Reader,
    unknownFields: UnknownFields = UnknownFields.Ignore,
    allowDuplicateKeys: Bool = false,
): Values[T]
```

The values of `T` in a stream that holds several, one after another,
each read as it is asked for.

#### toValue

```tondo
pub fn toValue[T: serialization.Encode](value: T): Value
```

A value as a `Value`.

#### fromValue

```tondo
pub fn fromValue[T: serialization.Decode](value: Value): T ! JsonError
```

A `Value` as a value of `T`.

#### decodeFrom

```tondo
pub fn decodeFrom[T: serialization.Decode](
    stream: io.Reader,
    unknownFields: UnknownFields = UnknownFields.Ignore,
    allowDuplicateKeys: Bool = false,
): T ! JsonError
```

A value from a stream, which must hold exactly one, read as it arrives.

#### encodeTo

```tondo
pub fn encodeTo[T: serialization.Encode](
    value: T,
    stream: io.Writer,
    pretty: Bool = false,
    canonical: Bool = false,
    htmlSafe: Bool = false,
): !JsonError
```

Writes the JSON text of a value to a stream as it is produced.

#### raw

```tondo
pub fn raw(text: String): Raw ! JsonError
```

Raw JSON from its text, which must be one valid value.

### Value

```tondo
pub enum Value {
    Null
    Bool(Bool)
    Number(Number)
    Text(String)
    Array(Array[Value])
    Object(Map[String, Value])
}
```

A JSON document of unknown shape.

```tondo
import std.console
import std.json

let doc = json.parse(r"""{"user": {"name": "Ada", "tags": ["a", "b"]}}""")?
console.println("{doc.find("user.name")} {doc.find("user.tags[1]")} {doc.field("user").field("age")}")
```

Implements `Display`, `reflect.Reflect`, `serialization.Encode`, `serialization.Decode`.

#### Value.isNull

```tondo
pub fn Value.isNull(self): Bool
```

Whether the value is `null`.

#### Value.get

```tondo
pub fn Value.get(self, key: String): Value?
```

The member `key` of an object.

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

#### Value.field

```tondo
pub fn Value.field(self, key: String): Value
```

The member `key` of an object, or `Null`: `document["key"]`.

#### Value.item

```tondo
pub fn Value.item(self, index: Int): Value
```

The element `index` of an array, or `Null`: `document[index]`.

#### Value.asText

```tondo
pub fn Value.asText(self): String?
```

The string, or `none` when the value is not one.

#### Value.asInt

```tondo
pub fn Value.asInt(self): Int?
```

The number as an `Int`, or `none` when the value is not a number that
is an integer and fits.

#### Value.asFloat

```tondo
pub fn Value.asFloat(self): Float?
```

The number as the nearest `Float`, or `none` when the value is not a
number.

#### Value.asBool

```tondo
pub fn Value.asBool(self): Bool?
```

The boolean, or `none` when the value is not one.

#### Value.asArray

```tondo
pub fn Value.asArray(self): Array[Value]?
```

The elements, or `none` when the value is not an array.

#### Value.asObject

```tondo
pub fn Value.asObject(self): Map[String, Value]?
```

The members, or `none` when the value is not an object.

### Number

```tondo
pub type Number = {
    // private fields
}
```

A JSON number as written, so that no precision is lost before the
program chooses a type for it.

```tondo
import std.console
import std.json

// A number as JSON wrote it, beyond what Int or Float hold.
let big = json.number("123456789012345678901234567890")?
console.println("{big.text()} {big.toInt()} {big.toFloat()}")
```

Implements `Display`, `serialization.Encode`, `serialization.Decode`.

#### Number.text

```tondo
pub fn Number.text(self): String
```

The number's JSON text, as it was read.

#### Number.toInt

```tondo
pub fn Number.toInt(self): Int?
```

The number as an `Int`, when it is an integer that fits.

#### Number.toFloat

```tondo
pub fn Number.toFloat(self): Float
```

The nearest `Float`.

### ErrorKind

```tondo
pub alias ErrorKind = serialization.ErrorKind
```

The kinds of failure, which every format shares.

```tondo
import std.console
import std.json

type User = { name: String }

match json.decode[User]("{{\"age\": 3}}") {
    ok(user) => console.println(user.name)
    err(error) => {
        let kind: json.ErrorKind = error.kind
        console.println("{kind} at {error.path}")
    }
}
```

### JsonError

```tondo
pub alias JsonError = serialization.Error
```

A failure to read or write JSON, with where it happened: the error of
every format.

```tondo
import std.console
import std.json

match json.parse("[1, 2,") {
    ok(value) => console.println("{value}")
    err(error) => {
        let failure: json.JsonError = error
        console.println("line {failure.line}, column {failure.column}: {failure.message}")
    }
}
```

### Event

```tondo
pub enum Event {
    Null
    Bool(Bool)
    Number(Number)
    Text(String)
    BeginArray
    EndArray
    BeginObject
    EndObject
    Key(String)
}
```

What a `Reader` finds next.

```tondo
import std.console
import std.json

// A document read as it streams, a token at a time.
let reader = json.reader("[1, \"two\"]")
for {
    match reader.next()? {
        some(json.Event.Text(text)) => console.println("text {text}")
        some(json.Event.Number(number)) => console.println("number {number.text()}")
        some(_) => {}
        none => break
    }
}
```

### UnknownFields

```tondo
pub alias UnknownFields = serialization.UnknownFields
```

What reading does with a member that the type has no field for.

```tondo
import std.console
import std.json

type Point = { x: Int }

let text = "{{\"x\": 1, \"z\": 3}}"
console.println("{json.decode[Point](text).isOk()}")
console.println("{json.decode[Point](text, unknownFields: json.UnknownFields.Reject).isOk()}")
```

### Reader

```tondo
pub type Reader = {
    // private fields
}
```

A JSON document read event by event.

```tondo
import std.console
import std.json

let reader = json.reader("{{\"name\": \"Ada\", \"age\": 36}}")
let first = reader.value()?
reader.beginObject(first, "a person")?
for {
    match reader.nextKey()? {
        some(key) => {
            console.println(key)
            reader.skip(reader.value()?)?
        }
        none => break
    }
}
```

Implements `serialization.Source`.

#### Reader.next

```tondo
pub fn Reader.next(self): Event? ! JsonError
```

The next event, or `none` after the whole document.

#### Reader.value

```tondo
pub fn Reader.value(self): Event ! JsonError
```

The next event, which must exist.

#### Reader.nextKey

```tondo
pub fn Reader.nextKey(self): String? ! JsonError
```

The next key of an object, or `none` at its end.

#### Reader.skip

```tondo
pub fn Reader.skip(self, first: Event): !JsonError
```

Skips the rest of a value whose first event was `first`.

#### Reader.unknownField

```tondo
pub fn Reader.unknownField(self, key: String, first: Event): !JsonError
```

A member the type does not have: skipped, or an error when unknown
fields are rejected.

#### Reader.beginObject

```tondo
pub fn Reader.beginObject(self, first: Event, name: String): !JsonError
```

Checks that an object starts, for the type `name`.

#### Reader.beginArray

```tondo
pub fn Reader.beginArray(self, first: Event, name: String): !JsonError
```

Checks that an array starts, for `name`.

#### Reader.endArray

```tondo
pub fn Reader.endArray(self): !JsonError
```

Checks that an array ends.

#### Reader.required

```tondo
pub fn Reader.required[T: ~Copy](self, value: T?, field: String): T ! JsonError
```

A field that must have been present.

#### Reader.mismatch

```tondo
pub fn Reader.mismatch(self, first: Event, expected: String): JsonError
```

An error for a value of the wrong kind.

#### Reader.failure

```tondo
pub fn Reader.failure(self, kind: ErrorKind, message: String): JsonError
```

An error at the current position.

### Writer

```tondo
pub type Writer = {
    // private fields
}
```

A JSON document written value by value.

```tondo
import std.console
import std.json

let out = json.writer(pretty: true)
out.beginObject()
out.key("name")
out.string("Ada")
out.key("tags")
out.beginArray()
out.string("a")
out.endArray()
out.endObject()
console.println(out.finish())
```

Implements `serialization.Sink`.

#### Writer.null

```tondo
pub fn Writer.null(self)
```

Writes `null`.

#### Writer.bool

```tondo
pub fn Writer.bool(self, value: Bool)
```

Writes `true` or `false`.

#### Writer.int

```tondo
pub fn Writer.int(self, value: Int)
```

Writes the integer.

#### Writer.float

```tondo
pub fn Writer.float(self, value: Float)
```

A float; `null` when it is not finite.

#### Writer.number

```tondo
pub fn Writer.number(self, value: Number)
```

Writes the number as its JSON text.

#### Writer.string

```tondo
pub fn Writer.string(self, value: String)
```

Writes the text as a JSON string, escaped.

#### Writer.key

```tondo
pub fn Writer.key(self, name: String)
```

Writes the key of the next member of the object being written.

#### Writer.beginArray

```tondo
pub fn Writer.beginArray(self)
```

Starts an array; `endArray` ends it.

#### Writer.endArray

```tondo
pub fn Writer.endArray(self)
```

Ends the array being written.

#### Writer.beginObject

```tondo
pub fn Writer.beginObject(self)
```

Starts an object, whose members are each a `key` and a value;
`endObject` ends it.

#### Writer.endObject

```tondo
pub fn Writer.endObject(self)
```

Ends the object being written.

#### Writer.finish

```tondo
pub fn Writer.finish(self): String
```

The document written so far; empty for a writer to a stream, which
`flush` ends instead.

#### Writer.flush

```tondo
pub fn Writer.flush(self): !JsonError
```

Ends the document of a writer to a stream: delivers what is pending and
reports a failure to write.

### Values

```tondo
pub type Values[T] = {
    // private fields
}
```

The values of `T` in a stream that holds several, one after another,
each read as it is asked for and as it arrives: `for each in
json.values[Package](output) { let package = each? }`. After an error
there are no more.

```tondo
import std.console
import std.io
import std.json

type Event = { kind: String }

// One value after another, as a log of JSON lines.
for event in json.values[Event](io.reader("{{\"kind\": \"a\"}}\n{{\"kind\": \"b\"}}\n")) {
    console.println(event?.kind)
}
```

Implements `Iterator[T ! JsonError]`.

### Raw

```tondo
pub type Raw = {
    // private fields
}
```

JSON text kept as it is, validated: a member whose content another part
of the program reads, or one that must round-trip byte for byte.

```tondo
import std.console
import std.json

// Text kept as it was written, inside a value written as JSON.
type Envelope = { kind: String, payload: json.Raw }

let envelope = json.decode[Envelope]("{{\"kind\": \"user\", \"payload\": {{\"name\": \"Ada\"}}}}")?
console.println(envelope.payload.text())
```

Implements `Display`, `serialization.Encode`, `serialization.Decode`.

#### Raw.text

```tondo
pub fn Raw.text(self): String
```

The JSON text, as it was given or read.
