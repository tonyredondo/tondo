# std.protobuf

Protocol Buffers, the binary format of gRPC. A record whose fields carry numbers is a message and an enum whose variants carry numbers is a protobuf enum: the compiler writes their `Message` and `Enum` implementations from those annotations, field by field, on the runtime's writer and reader of the wire format.

```tondo
import std.console
import std.protobuf

// A `.proto` file under `src` is a module of such records; one can be
// written by hand too.
pub type Point = {
    @protobuf.Field(1)
    x: Int32 = 0
    @protobuf.Field(2)
    y: Int32 = 0
}

let bytes = protobuf.encode(Point { x: 3, y: 4 })
let point = protobuf.decode[Point](bytes)?
console.println("{bytes.length()} bytes: {point.x}, {point.y} {protobuf.toJson(point)?}")
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.protobuf` shows the same in a terminal.

- [Functions](#functions): [`encode`](#encode), [`decode`](#decode), [`encodeTo`](#encodeto), [`decodeFrom`](#decodefrom), [`entries`](#entries), [`emptyReader`](#emptyreader), [`epoch`](#epoch), [`zeroDuration`](#zeroduration), [`nullValue`](#nullvalue), [`pack`](#pack), [`toJson`](#tojson), [`fromJson`](#fromjson)
- [Field](#field)
- [Encoding](#encoding)
- [Value](#value)
- [Name](#name)
- [Unknown](#unknown)
- [FileDescriptor](#filedescriptor)
- [UnknownFields](#unknownfields): [`UnknownFields.isEmpty`](#unknownfieldsisempty)
- [Message](#message)
- [Enum](#enum)
- [ProtobufError](#protobuferror)
- [Writer](#writer): [`Writer.deterministic`](#writerdeterministic), [`Writer.int32`](#writerint32), [`Writer.int64`](#writerint64), [`Writer.uint32`](#writeruint32), [`Writer.uint64`](#writeruint64), [`Writer.sint32`](#writersint32), [`Writer.sint64`](#writersint64), [`Writer.fixed32`](#writerfixed32), [`Writer.sfixed32`](#writersfixed32), [`Writer.fixed64`](#writerfixed64), [`Writer.sfixed64`](#writersfixed64), [`Writer.bool`](#writerbool), [`Writer.float`](#writerfloat), [`Writer.double`](#writerdouble), [`Writer.string`](#writerstring), [`Writer.bytes`](#writerbytes), [`Writer.message`](#writermessage), [`Writer.group`](#writergroup), [`Writer.enumValue`](#writerenumvalue), [`Writer.begin`](#writerbegin), [`Writer.end`](#writerend), [`Writer.timestamp`](#writertimestamp), [`Writer.duration`](#writerduration), [`Writer.jsonValue`](#writerjsonvalue), [`Writer.jsonStruct`](#writerjsonstruct), [`Writer.jsonList`](#writerjsonlist), [`Writer.unknown`](#writerunknown)
- [Reader](#reader): [`Reader.next`](#readernext), [`Reader.int32`](#readerint32), [`Reader.int64`](#readerint64), [`Reader.uint32`](#readeruint32), [`Reader.uint64`](#readeruint64), [`Reader.sint32`](#readersint32), [`Reader.sint64`](#readersint64), [`Reader.fixed32`](#readerfixed32), [`Reader.sfixed32`](#readersfixed32), [`Reader.fixed64`](#readerfixed64), [`Reader.sfixed64`](#readersfixed64), [`Reader.bool`](#readerbool), [`Reader.float`](#readerfloat), [`Reader.double`](#readerdouble), [`Reader.string`](#readerstring), [`Reader.bytes`](#readerbytes), [`Reader.message`](#readermessage), [`Reader.group`](#readergroup), [`Reader.enumValue`](#readerenumvalue), [`Reader.enter`](#readerenter), [`Reader.isPacked`](#readerispacked), [`Reader.enterPacked`](#readerenterpacked), [`Reader.more`](#readermore), [`Reader.skip`](#readerskip), [`Reader.keep`](#readerkeep), [`Reader.missing`](#readermissing), [`Reader.timestamp`](#readertimestamp), [`Reader.duration`](#readerduration), [`Reader.jsonValue`](#readerjsonvalue), [`Reader.jsonStruct`](#readerjsonstruct), [`Reader.jsonList`](#readerjsonlist)
- [Empty](#empty)
- [Any](#any): [`Any.unpack`](#anyunpack)
- [FieldMask](#fieldmask)
- [JsonWriter](#jsonwriter): [`JsonWriter.begin`](#jsonwriterbegin), [`JsonWriter.end`](#jsonwriterend), [`JsonWriter.field`](#jsonwriterfield), [`JsonWriter.key`](#jsonwriterkey), [`JsonWriter.beginArray`](#jsonwriterbeginarray), [`JsonWriter.endArray`](#jsonwriterendarray), [`JsonWriter.int32`](#jsonwriterint32), [`JsonWriter.uint32`](#jsonwriteruint32), [`JsonWriter.int64`](#jsonwriterint64), [`JsonWriter.uint64`](#jsonwriteruint64), [`JsonWriter.float`](#jsonwriterfloat), [`JsonWriter.double`](#jsonwriterdouble), [`JsonWriter.bool`](#jsonwriterbool), [`JsonWriter.string`](#jsonwriterstring), [`JsonWriter.bytes`](#jsonwriterbytes), [`JsonWriter.enumValue`](#jsonwriterenumvalue), [`JsonWriter.message`](#jsonwritermessage), [`JsonWriter.timestamp`](#jsonwritertimestamp), [`JsonWriter.duration`](#jsonwriterduration), [`JsonWriter.jsonValue`](#jsonwriterjsonvalue)
- [JsonReader](#jsonreader): [`JsonReader.text`](#jsonreadertext), [`JsonReader.missing`](#jsonreadermissing), [`JsonReader.boolKey`](#jsonreaderboolkey), [`JsonReader.fields`](#jsonreaderfields), [`JsonReader.unknown`](#jsonreaderunknown), [`JsonReader.array`](#jsonreaderarray), [`JsonReader.entries`](#jsonreaderentries), [`JsonReader.int32`](#jsonreaderint32), [`JsonReader.uint32`](#jsonreaderuint32), [`JsonReader.int64`](#jsonreaderint64), [`JsonReader.uint64`](#jsonreaderuint64), [`JsonReader.double`](#jsonreaderdouble), [`JsonReader.float`](#jsonreaderfloat), [`JsonReader.bool`](#jsonreaderbool), [`JsonReader.string`](#jsonreaderstring), [`JsonReader.bytes`](#jsonreaderbytes), [`JsonReader.enumValue`](#jsonreaderenumvalue), [`JsonReader.message`](#jsonreadermessage), [`JsonReader.timestamp`](#jsonreadertimestamp), [`JsonReader.duration`](#jsonreaderduration)
- [JsonValue](#jsonvalue)

### Functions

#### encode

```tondo
pub fn encode[T: Message](value: T, deterministic: Bool = false): Array[Byte]
```

The bytes of a message: its fields in the order of their numbers, map
entries in their order, or with `deterministic` sorted by key, so that
equal messages give equal bytes.

#### decode

```tondo
pub fn decode[T: Message](bytes: Array[Byte]): T ! ProtobufError
```

The message the bytes hold.

#### encodeTo

```tondo
pub fn encodeTo[T: Message](value: T, out: io.Writer, deterministic: Bool = false): !io.IoError
```

Writes the bytes of a message to a stream.

#### decodeFrom

```tondo
pub fn decodeFrom[T: Message](input: io.Reader): T ! ProtobufError
```

The message a stream holds, read to its end.

#### entries

```tondo
pub fn entries[K: Key + Ordered, V](map: Map[K, V], sorted: Bool): Array[(K, V)]
```

The entries of a map field, sorted by key when the writer is
deterministic.

#### emptyReader

```tondo
pub fn emptyReader(): Reader
```

A reader of no fields, for a message every field of which has its zero.

#### epoch

```tondo
pub fn epoch(): time.DateTime
```

The zero of a `Timestamp`, for a map entry without its value.

#### zeroDuration

```tondo
pub fn zeroDuration(): time.Duration
```

The zero of a `Duration`, for a map entry without its value.

#### nullValue

```tondo
pub fn nullValue(): json.Value
```

The zero of a `Value`, for a map entry without its value.

#### pack

```tondo
pub fn pack[T: Message](message: T): Any
```

A message in an `Any`, named `type.googleapis.com/` and its full name.

#### toJson

```tondo
pub fn toJson[T: Message](message: T, protoNames: Bool = false): String ! ProtobufError
```

The message as proto3's mapping to JSON writes it: fields by their
names in lowerCamel, or with `protoNames` as the `.proto` writes them;
fields with their default values left out; 64-bit integers as text,
bytes in Base64 and enum values by name.

#### fromJson

```tondo
pub fn fromJson[T: Message](text: String, ignoreUnknown: Bool = false): T ! ProtobufError
```

The message a JSON text gives, as proto3's mapping reads it: fields by
either name, numbers as numbers or as text, enum values by name or by
number. A field the message does not have fails, unless
`ignoreUnknown`.

### Field

```tondo
pub annotation Field = {
    number: Int
    encoding: Encoding = Encoding.Default
    packed: Bool = true
    /// Its name in the `.proto`, when it is not the field's, as `user_id`,
    /// which JSON reads and, with `protoNames`, writes.
    name: String = ""
}
```

The number of a field of a message, or of a variant of a `oneof`:
`@protobuf.Field(1)`. `encoding` chooses among the ways protobuf writes
the field's type, and `packed: false` writes a repeated number with a
tag each, as proto2 does.

Implements `reflect.Reflect`.

### Encoding

```tondo
pub enum Encoding {
    /// As its type says: an integer as a varint, int32 to uint64.
    Default
    /// A signed integer as a zigzag varint: sint32 and sint64.
    ZigZag
    /// An integer in 4 or 8 bytes, by its width: fixed32, fixed64, sfixed32
    /// and sfixed64.
    Fixed
    /// A message as a group of proto2.
    Group
    /// An optional value as a wrapper of `google.protobuf`, as `Int32Value`.
    Wrapper
    /// A `json.Value` as a `google.protobuf.Struct`, an object, or a
    /// `ListValue`, an array, rather than as a `Value`.
    Struct
    List
}
```

How a field is written, where protobuf has several ways for its type.

```tondo
import std.console
import std.protobuf

pub type Reading = {
    // A sensor's id, written in 4 bytes: fixed32.
    @protobuf.Field(1, encoding: protobuf.Encoding.Fixed)
    sensor: UInt32 = 0
    // Often negative: a zigzag varint, sint32.
    @protobuf.Field(2, encoding: protobuf.Encoding.ZigZag)
    delta: Int32 = 0
    // An optional number that keeps zero apart from absent: Int32Value.
    @protobuf.Field(3, encoding: protobuf.Encoding.Wrapper)
    limit: Int32? = none
}

let bytes = protobuf.encode(Reading { sensor: 7u32, delta: -2, limit: some(0) })
console.println("{bytes.length()} bytes: {protobuf.decode[Reading](bytes)?.limit}")
```

### Value

```tondo
pub annotation Value = {
    number: Int
}
```

The number of a variant of a protobuf enum: `@protobuf.Value(1)`.

Implements `reflect.Reflect`.

### Name

```tondo
pub annotation Name = {
    name: String
}
```

The full name of a message, which `Any` and gRPC use:
`@protobuf.Name("users.v1.User")`; without it, the type's own name.

Implements `reflect.Reflect`.

### Unknown

```tondo
pub annotation Unknown = {}
```

Keeps the fields a message does not know in a field of type
`UnknownFields`, which writing puts back.

Implements `reflect.Reflect`.

### FileDescriptor

```tondo
pub type FileDescriptor = {
    name: String
    symbols: Array[String] = []
    bytes: Array[Byte] = []
    imports: Array[FileDescriptor] = []
}
```

A `.proto` file as reflection gives it to tools: its path, the full
names it declares, its `FileDescriptorProto` and the files it imports.
The module of a `.proto` file gives its own as `fileDescriptor()`.

```tondo
import std.console
import std.grpc

// The `.proto` files a service comes from, which reflection gives tools.
for file in grpc.Health().service().files {
    console.println("{file.name}: {file.symbols.length()} names, {file.imports.length()} imports")
}
```

### UnknownFields

```tondo
pub type UnknownFields = {
    // private fields
}
```

The fields of a message that its type does not know, as they were read.

```tondo
import std.console
import std.protobuf

pub type UserV2 = {
    @protobuf.Field(1)
    name: String = ""
    @protobuf.Field(2)
    email: String = ""
}

/// An older reader that keeps what it does not know, and writes it back.
pub type UserV1 = {
    @protobuf.Field(1)
    name: String = ""
    @ignore
    @protobuf.Unknown
    unknown: protobuf.UnknownFields = protobuf.UnknownFields {}
}

let old = protobuf.decode[UserV1](protobuf.encode(UserV2 { name: "Ada", email: "ada@example.com" }))?
let again = protobuf.decode[UserV2](protobuf.encode(old with { name: "Ada L." }))?
console.println("{old.unknown.isEmpty()} {again.name} {again.email}")
```

Implements `serialization.Encode`, `serialization.Decode`, `reflect.Reflect`.

#### UnknownFields.isEmpty

```tondo
pub fn UnknownFields.isEmpty(self): Bool
```

Whether there are none.

### Message

```tondo
pub trait Message {
    fn writeTo(self, out: Writer)

    fn readFrom(input: Reader): Self ! ProtobufError

    /// Its full name, as `users.v1.User`.
    fn protobufName(): String

    /// Writes the message as proto3's mapping to JSON gives it, which the
    /// compiler writes for each message it implements.
    fn writeJson(self, out: JsonWriter): !ProtobufError

    /// The message a JSON value gives, as proto3's mapping reads it.
    fn readJson(input: JsonReader, value: json.Value): Self ! ProtobufError
}
```

A message: the compiler implements it for a record whose fields carry
`@protobuf.Field`.

```tondo
import std.console
import std.protobuf

// The compiler implements `Message` for records with `@protobuf.Field`; a
// type can also write it by hand.
pub type Point = { x: Int, y: Int }

impl protobuf.Message for Point {
    fn writeTo(self, out: protobuf.Writer) {
        if self.x != 0 {
            out.sint64(1, self.x)
        }
        if self.y != 0 {
            out.sint64(2, self.y)
        }
    }

    fn readFrom(input: protobuf.Reader): Point ! protobuf.ProtobufError {
        var point = Point { x: 0, y: 0 }
        for {
            match input.next()? {
                0 => break
                1 => point.x = input.sint64("x")?
                2 => point.y = input.sint64("y")?
                _ => input.skip()?
            }
        }
        point
    }

    fn protobufName(): String {
        "geo.Point"
    }
}

let bytes = protobuf.encode(Point { x: -3, y: 4 })
let point = protobuf.decode[Point](bytes)?
console.println("{bytes} {point.x},{point.y} {protobuf.pack(point).typeUrl}")
```

### Enum

```tondo
pub trait Enum {
    fn number(self): Int32

    fn fromNumber(number: Int32): Self?

    /// The value's name in the `.proto`, as JSON writes it; its number
    /// without one.
    fn protobufValueName(self): String

    /// The value of a name in the `.proto`.
    fn fromProtobufValueName(name: String): Self?
}
```

A protobuf enum: the compiler implements it for an enum whose variants
carry `@protobuf.Value`. One with an `Unrecognized(Int32)` variant is
open, and keeps a number it does not know there.

```tondo
import std.console
import std.protobuf

pub enum Status {
    @protobuf.Value(0)
    Unspecified
    @protobuf.Value(1)
    Active
    @protobuf.Value(2)
    Suspended
    // Open: a number this program does not know is kept here.
    Unrecognized(Int32)
}

pub type Account = {
    @protobuf.Field(1)
    status: Status = Status.Unspecified
}

let account = protobuf.decode[Account](protobuf.encode(Account { status: Status.Suspended }))?
console.println("{protobuf.Enum.number(account.status)} {protobuf.toJson(account)?}")
match protobuf.Enum.fromNumber[Status](7) {
    some(Status.Unrecognized(number)) => console.println("unknown {number}")
    _ => console.println("known")
}
```

### ProtobufError

```tondo
pub alias ProtobufError = serialization.Error
```

A failure to read protobuf, with the path of the field and the byte it
starts at, as `column`: the error of every format.

```tondo
import std.console
import std.protobuf

pub type User = {
    @protobuf.Field(1)
    name: String = ""
    @protobuf.Field(2)
    id: Int = 0
}

// Field 1 says 10 bytes of text follow, but only 2 do.
match protobuf.decode[User]([0x0a, 0x0a, 0x41, 0x64]) {
    ok(user) => console.println(user.name)
    err(error) => console.println("{error.kind} at {error.path}: {error.message}")
}
```

### Writer

```tondo
pub type Writer = {
    // private fields
}
```

Writes the fields of messages, each after the tag of its number; the
number 0 writes a value with no tag, as an element of a packed field.

```tondo
import std.console
import std.protobuf

pub type Tag = { name: String }

/// A message written by hand: a string, a repeated string and a nested
/// message, each with its field number.
pub type Post = { title: String, labels: Array[String], tag: Tag }

impl protobuf.Message for Tag {
    fn writeTo(self, out: protobuf.Writer) {
        out.string(1, self.name)
    }

    fn readFrom(input: protobuf.Reader): Tag ! protobuf.ProtobufError {
        var tag = Tag { name: "" }
        for {
            match input.next()? {
                0 => break
                1 => tag.name = input.string("name")?
                _ => input.skip()?
            }
        }
        tag
    }

    fn protobufName(): String {
        "blog.Tag"
    }
}

impl protobuf.Message for Post {
    fn writeTo(self, out: protobuf.Writer) {
        out.string(1, self.title)
        for label in self.labels {
            out.string(2, label)
        }
        out.message(3, self.tag)
    }

    fn readFrom(input: protobuf.Reader): Post ! protobuf.ProtobufError {
        var post = Post { title: "", labels: [], tag: Tag { name: "" } }
        for {
            match input.next()? {
                0 => break
                1 => post.title = input.string("title")?
                2 => post.labels.push(input.string("labels")?)
                3 => post.tag = input.message[Tag]("tag")?
                _ => input.skip()?
            }
        }
        post
    }

    fn protobufName(): String {
        "blog.Post"
    }
}

let post = protobuf.decode[Post](protobuf.encode(Post { title: "Tea", labels: ["a", "b"], tag: Tag { name: "food" } }))?
console.println("{post.title} {post.labels} {post.tag.name}")
```

#### Writer.deterministic

```tondo
pub fn Writer.deterministic(self): Bool
```

Whether map entries go sorted by key.

#### Writer.int32

```tondo
pub fn Writer.int32(self, number: Int, value: Int32)
```

An int32, as a varint; a negative one in ten bytes, as protobuf writes it.

#### Writer.int64

```tondo
pub fn Writer.int64(self, number: Int, value: Int)
```

An int64, as a varint.

#### Writer.uint32

```tondo
pub fn Writer.uint32(self, number: Int, value: UInt32)
```

A uint32, as a varint.

#### Writer.uint64

```tondo
pub fn Writer.uint64(self, number: Int, value: UInt)
```

A uint64, as a varint.

#### Writer.sint32

```tondo
pub fn Writer.sint32(self, number: Int, value: Int32)
```

A sint32, as a zigzag varint.

#### Writer.sint64

```tondo
pub fn Writer.sint64(self, number: Int, value: Int)
```

A sint64, as a zigzag varint.

#### Writer.fixed32

```tondo
pub fn Writer.fixed32(self, number: Int, value: UInt32)
```

A fixed32, in 4 bytes.

#### Writer.sfixed32

```tondo
pub fn Writer.sfixed32(self, number: Int, value: Int32)
```

An sfixed32, in 4 bytes.

#### Writer.fixed64

```tondo
pub fn Writer.fixed64(self, number: Int, value: UInt)
```

A fixed64, in 8 bytes.

#### Writer.sfixed64

```tondo
pub fn Writer.sfixed64(self, number: Int, value: Int)
```

An sfixed64, in 8 bytes.

#### Writer.bool

```tondo
pub fn Writer.bool(self, number: Int, value: Bool)
```

A bool, as a varint.

#### Writer.float

```tondo
pub fn Writer.float(self, number: Int, value: Float32)
```

A float, in 4 bytes.

#### Writer.double

```tondo
pub fn Writer.double(self, number: Int, value: Float)
```

A double, in 8 bytes.

#### Writer.string

```tondo
pub fn Writer.string(self, number: Int, value: String)
```

A string, as its UTF-8 bytes after their length.

#### Writer.bytes

```tondo
pub fn Writer.bytes(self, number: Int, value: Array[Byte])
```

Bytes, after their length.

#### Writer.message

```tondo
pub fn Writer.message[T: Message](self, number: Int, value: T)
```

A nested message, after its length.

#### Writer.group

```tondo
pub fn Writer.group[T: Message](self, number: Int, value: T)
```

A message as a group of proto2.

#### Writer.enumValue

```tondo
pub fn Writer.enumValue[T: Enum](self, number: Int, value: T)
```

The number of a protobuf enum's variant, as a varint.

#### Writer.begin

```tondo
pub fn Writer.begin(self, number: Int)
```

Opens a nested value of field `number`: a map entry, a packed field or a
wrapper, which `end` closes.

#### Writer.end

```tondo
pub fn Writer.end(self)
```

Closes what `begin` opened, writing its length before it.

#### Writer.timestamp

```tondo
pub fn Writer.timestamp(self, number: Int, value: time.DateTime)
```

A `google.protobuf.Timestamp`: seconds since 1970 and nanoseconds.

#### Writer.duration

```tondo
pub fn Writer.duration(self, number: Int, value: time.Duration)
```

A `google.protobuf.Duration`: seconds and nanoseconds of the same sign.

#### Writer.jsonValue

```tondo
pub fn Writer.jsonValue(self, number: Int, value: json.Value)
```

A `google.protobuf.Value`.

#### Writer.jsonStruct

```tondo
pub fn Writer.jsonStruct(self, number: Int, value: json.Value)
```

A `google.protobuf.Struct`, from a JSON object; another value writes an
empty one.

#### Writer.jsonList

```tondo
pub fn Writer.jsonList(self, number: Int, value: json.Value)
```

A `google.protobuf.ListValue`, from a JSON array.

#### Writer.unknown

```tondo
pub fn Writer.unknown(self, fields: UnknownFields)
```

The fields a message kept without knowing them.

### Reader

```tondo
pub type Reader = {
    // private fields
}
```

Reads the fields of messages: `next` gives the number of each, and a
method of its type its value; `name` says the field in an error.

```tondo
import std.console
import std.protobuf

/// Numbers that come packed, as proto3 writes repeated numbers, or one
/// field each, as proto2 did: a reader takes both.
pub type Samples = { values: Array[Int] }

impl protobuf.Message for Samples {
    fn writeTo(self, out: protobuf.Writer) {
        for value in self.values {
            out.int64(1, value)
        }
    }

    fn readFrom(input: protobuf.Reader): Samples ! protobuf.ProtobufError {
        var values: Array[Int] = []
        for {
            match input.next()? {
                0 => break
                1 if input.isPacked() => {
                    input.enterPacked("values")?
                    for input.more() {
                        values.push(input.int64("values")?)
                    }
                }
                1 => values.push(input.int64("values")?)
                _ => input.skip()?
            }
        }
        Samples { values }
    }

    fn protobufName(): String {
        "stats.Samples"
    }
}

console.println("{protobuf.decode[Samples](protobuf.encode(Samples { values: [3, 270] }))?.values}")
// The same numbers packed: field 1, length 3, then the varints.
console.println("{protobuf.decode[Samples]([0x0a, 0x03, 0x03, 0x8e, 0x02])?.values}")
```

#### Reader.next

```tondo
pub fn Reader.next(self): Int ! ProtobufError
```

The number of the next field, or 0 at the end of the message.

#### Reader.int32

```tondo
pub fn Reader.int32(self, name: String): Int32 ! ProtobufError
```

The int32 of the field read.

#### Reader.int64

```tondo
pub fn Reader.int64(self, name: String): Int ! ProtobufError
```

The int64 of the field read.

#### Reader.uint32

```tondo
pub fn Reader.uint32(self, name: String): UInt32 ! ProtobufError
```

The uint32 of the field read.

#### Reader.uint64

```tondo
pub fn Reader.uint64(self, name: String): UInt ! ProtobufError
```

The uint64 of the field read.

#### Reader.sint32

```tondo
pub fn Reader.sint32(self, name: String): Int32 ! ProtobufError
```

The sint32 of the field read.

#### Reader.sint64

```tondo
pub fn Reader.sint64(self, name: String): Int ! ProtobufError
```

The sint64 of the field read.

#### Reader.fixed32

```tondo
pub fn Reader.fixed32(self, name: String): UInt32 ! ProtobufError
```

The fixed32 of the field read.

#### Reader.sfixed32

```tondo
pub fn Reader.sfixed32(self, name: String): Int32 ! ProtobufError
```

The sfixed32 of the field read.

#### Reader.fixed64

```tondo
pub fn Reader.fixed64(self, name: String): UInt ! ProtobufError
```

The fixed64 of the field read.

#### Reader.sfixed64

```tondo
pub fn Reader.sfixed64(self, name: String): Int ! ProtobufError
```

The sfixed64 of the field read.

#### Reader.bool

```tondo
pub fn Reader.bool(self, name: String): Bool ! ProtobufError
```

The bool of the field read.

#### Reader.float

```tondo
pub fn Reader.float(self, name: String): Float32 ! ProtobufError
```

The float of the field read.

#### Reader.double

```tondo
pub fn Reader.double(self, name: String): Float ! ProtobufError
```

The double of the field read.

#### Reader.string

```tondo
pub fn Reader.string(self, name: String): String ! ProtobufError
```

The string of the field read, which must be UTF-8.

#### Reader.bytes

```tondo
pub fn Reader.bytes(self, name: String): Array[Byte] ! ProtobufError
```

The bytes of the field read.

#### Reader.message

```tondo
pub fn Reader.message[T: Message](self, name: String): T ! ProtobufError
```

The nested message of the field read.

#### Reader.group

```tondo
pub fn Reader.group[T: Message](self, name: String): T ! ProtobufError
```

A message written as a group of proto2.

#### Reader.enumValue

```tondo
pub fn Reader.enumValue[T: Enum](self, name: String): T? ! ProtobufError
```

The variant of an enum, or `none` for a number a closed enum does not
have, which a message keeps among its unknown fields.

#### Reader.enter

```tondo
pub fn Reader.enter(self, name: String): !ProtobufError
```

Enters the field read: a nested message, a map entry or a wrapper,
whose fields `next` reads until it gives 0.

#### Reader.isPacked

```tondo
pub fn Reader.isPacked(self): Bool
```

Whether the field read is packed: then `enterPacked` and `more` read its
elements; otherwise it holds one.

#### Reader.enterPacked

```tondo
pub fn Reader.enterPacked(self, name: String): !ProtobufError
```

Enters a packed field, whose elements `more` and the readers of its type read.

#### Reader.more

```tondo
pub fn Reader.more(self): Bool
```

Whether a packed field has more elements.

#### Reader.skip

```tondo
pub fn Reader.skip(self): !ProtobufError
```

Skips the value of the field read.

#### Reader.keep

```tondo
pub fn Reader.keep(self, fields: UnknownFields): UnknownFields ! ProtobufError
```

Keeps the field read, which the message does not know, among `fields`.

#### Reader.missing

```tondo
pub fn Reader.missing(self, name: String): ProtobufError
```

A missing field that the message requires.

#### Reader.timestamp

```tondo
pub fn Reader.timestamp(self, name: String): time.DateTime ! ProtobufError
```

The `google.protobuf.Timestamp` of the field read.

#### Reader.duration

```tondo
pub fn Reader.duration(self, name: String): time.Duration ! ProtobufError
```

The `google.protobuf.Duration` of the field read.

#### Reader.jsonValue

```tondo
pub fn Reader.jsonValue(self, name: String): json.Value ! ProtobufError
```

The `google.protobuf.Value` of the field read.

#### Reader.jsonStruct

```tondo
pub fn Reader.jsonStruct(self, name: String): json.Value ! ProtobufError
```

The `google.protobuf.Struct` of the field read, as a JSON object.

#### Reader.jsonList

```tondo
pub fn Reader.jsonList(self, name: String): json.Value ! ProtobufError
```

The `google.protobuf.ListValue` of the field read, as a JSON array.

### Empty

```tondo
pub type Empty = {}
```

`google.protobuf.Empty`, the message with no fields.

```tondo
import std.console
import std.protobuf

// The message of a call that takes or gives nothing.
let bytes = protobuf.encode(protobuf.Empty {})
console.println("{bytes.length()} bytes, JSON {protobuf.toJson(protobuf.Empty {})?}")
```

Implements `Message`, `serialization.Encode`, `serialization.Decode`, `reflect.Reflect`.

### Any

```tondo
pub type Any = {
    typeUrl: String = ""
    value: Array[Byte] = []
}
```

`google.protobuf.Any`: a message of any type, as its bytes and the URL
of its type's name.

```tondo
import std.console
import std.protobuf

@protobuf.Name("errors.v1.RetryInfo")
pub type RetryInfo = {
    @protobuf.Field(1)
    seconds: Int = 0
}

// A message of any type, with the name that says which.
let detail = protobuf.pack(RetryInfo { seconds: 30 })
console.println(detail.typeUrl)
let retry = detail.unpack[RetryInfo]()?
console.println("retry in {retry.seconds}s")
```

Implements `Message`, `serialization.Encode`, `serialization.Decode`, `reflect.Reflect`.

#### Any.unpack

```tondo
pub fn Any.unpack[T: Message](self): T ! ProtobufError
```

The message an `Any` holds, which must be a `T`.

### FieldMask

```tondo
pub type FieldMask = {
    paths: Array[String] = []
}
```

`google.protobuf.FieldMask`: the paths of the fields an update sets,
as `user.display_name`.

```tondo
import std.console
import std.protobuf

/// An update that says which fields it changes, as APIs of Google do.
pub type UpdateUser = {
    @protobuf.Field(1)
    name: String = ""
    @protobuf.Field(2)
    email: String = ""
    @protobuf.Field(3)
    mask: protobuf.FieldMask = protobuf.FieldMask {}
}

let update = UpdateUser { email: "ada@example.com", mask: protobuf.FieldMask { paths: ["email"] } }
console.println(protobuf.toJson(update)?)
let read = protobuf.fromJson[UpdateUser](r"""{"email": "x@example.com", "mask": "email,name"}""")?
console.println("{read.mask.paths}")
```

Implements `Message`, `serialization.Encode`, `serialization.Decode`, `reflect.Reflect`.

### JsonWriter

```tondo
pub type JsonWriter = {
    // private fields
}
```

Where a message writes itself as JSON.

```tondo
import std.console
import std.protobuf

/// A message written by hand, whose JSON is a `"x,y"` text rather than an
/// object.
pub type Point = { x: Int, y: Int }

impl protobuf.Message for Point {
    fn writeTo(self, out: protobuf.Writer) {
        out.sint64(1, self.x)
        out.sint64(2, self.y)
    }

    fn readFrom(input: protobuf.Reader): Point ! protobuf.ProtobufError {
        var point = Point { x: 0, y: 0 }
        for {
            match input.next()? {
                0 => break
                1 => point.x = input.sint64("x")?
                2 => point.y = input.sint64("y")?
                _ => input.skip()?
            }
        }
        point
    }

    fn protobufName(): String {
        "geo.Point"
    }

    fn writeJson(self, out: protobuf.JsonWriter): !protobuf.ProtobufError {
        out.string("{self.x},{self.y}")
    }
}

console.println(protobuf.toJson(Point { x: 3, y: -4 })?)
```

#### JsonWriter.begin

```tondo
pub fn JsonWriter.begin(self)
```

Starts an object, a message's or a map's.

#### JsonWriter.end

```tondo
pub fn JsonWriter.end(self)
```

Ends the object.

#### JsonWriter.field

```tondo
pub fn JsonWriter.field(self, jsonName: String, protoName: String)
```

A field's name: its name in JSON, or its name in the `.proto` when the
writer keeps those.

#### JsonWriter.key

```tondo
pub fn JsonWriter.key(self, key: String)
```

A key of a map, or of an object as it is.

#### JsonWriter.beginArray

```tondo
pub fn JsonWriter.beginArray(self)
```

Starts an array, a repeated field's.

#### JsonWriter.endArray

```tondo
pub fn JsonWriter.endArray(self)
```

Ends the array.

#### JsonWriter.int32

```tondo
pub fn JsonWriter.int32(self, value: Int32)
```

An `int32`, `sint32` or `sfixed32`, as a number.

#### JsonWriter.uint32

```tondo
pub fn JsonWriter.uint32(self, value: UInt32)
```

A `uint32` or `fixed32`, as a number.

#### JsonWriter.int64

```tondo
pub fn JsonWriter.int64(self, value: Int)
```

An `int64`, `sint64` or `sfixed64`, as text, which JavaScript reads
without losing digits.

#### JsonWriter.uint64

```tondo
pub fn JsonWriter.uint64(self, value: UInt)
```

A `uint64` or `fixed64`, as text.

#### JsonWriter.float

```tondo
pub fn JsonWriter.float(self, value: Float32)
```

A `float`: a number, or `"NaN"`, `"Infinity"` or `"-Infinity"`.

#### JsonWriter.double

```tondo
pub fn JsonWriter.double(self, value: Float)
```

A `double`: a number, or `"NaN"`, `"Infinity"` or `"-Infinity"`.

#### JsonWriter.bool

```tondo
pub fn JsonWriter.bool(self, value: Bool)
```

A `bool`.

#### JsonWriter.string

```tondo
pub fn JsonWriter.string(self, value: String)
```

A `string`.

#### JsonWriter.bytes

```tondo
pub fn JsonWriter.bytes(self, value: Array[Byte])
```

`bytes`, in Base64 with its padding.

#### JsonWriter.enumValue

```tondo
pub fn JsonWriter.enumValue[T: Enum](self, value: T)
```

An enum's value, by its name in the `.proto`.

#### JsonWriter.message

```tondo
pub fn JsonWriter.message[T: Message](self, value: T): !ProtobufError
```

A message, as its own `writeJson` writes it.

#### JsonWriter.timestamp

```tondo
pub fn JsonWriter.timestamp(self, value: time.DateTime)
```

A `google.protobuf.Timestamp`, in RFC 3339 in UTC, with 0, 3, 6 or 9
digits of a second: `2026-11-01T10:30:00.500Z`.

#### JsonWriter.duration

```tondo
pub fn JsonWriter.duration(self, value: time.Duration)
```

A `google.protobuf.Duration`, in seconds with 0, 3, 6 or 9 digits:
`1.500s`, `-3s`.

#### JsonWriter.jsonValue

```tondo
pub fn JsonWriter.jsonValue(self, value: json.Value)
```

A `google.protobuf.Value`, `Struct` or `ListValue`: the JSON it holds.

### JsonReader

```tondo
pub type JsonReader = {
    // private fields
}
```

What reads a message's JSON.

```tondo
import std.console
import std.json
import std.protobuf

/// A message written by hand that reads its JSON with either name of a
/// field, as proto3's mapping does.
pub type Point = { x: Int, y: Int }

impl protobuf.Message for Point {
    fn writeTo(self, out: protobuf.Writer) {
        out.sint64(1, self.x)
        out.sint64(2, self.y)
    }

    fn readFrom(input: protobuf.Reader): Point ! protobuf.ProtobufError {
        var point = Point { x: 0, y: 0 }
        for {
            match input.next()? {
                0 => break
                1 => point.x = input.sint64("x")?
                2 => point.y = input.sint64("y")?
                _ => input.skip()?
            }
        }
        point
    }

    fn protobufName(): String {
        "geo.Point"
    }

    fn readJson(input: protobuf.JsonReader, value: json.Value): Point ! protobuf.ProtobufError {
        var point = Point { x: 0, y: 0 }
        for (key, field) in input.fields(value, "$")? {
            match key {
                "x" => point.x = input.int64(field, "$.x")?
                "y" => point.y = input.int64(field, "$.y")?
                _ => input.unknown(key, "$")?
            }
        }
        point
    }
}

let point = protobuf.fromJson[Point](r"""{"x": "12", "y": -5}""")?
console.println("{point.x},{point.y}")
```

#### JsonReader.text

```tondo
pub fn JsonReader.text(self, key: String): json.Value
```

A key of a map's object as a value, to read as the map's key.

#### JsonReader.missing

```tondo
pub fn JsonReader.missing(self, name: String): ProtobufError
```

A missing field that the message requires.

#### JsonReader.boolKey

```tondo
pub fn JsonReader.boolKey(self, key: String, path: String): Bool ! ProtobufError
```

A `bool` key of a map: `"true"` or `"false"`.

#### JsonReader.fields

```tondo
pub fn JsonReader.fields(
    self,
    value: json.Value,
    path: String,
): Array[(String, json.Value)] ! ProtobufError
```

The fields of a message's object, without those that are `null`, which
leave their fields as they were.

#### JsonReader.unknown

```tondo
pub fn JsonReader.unknown(self, key: String, path: String): !ProtobufError
```

A field the message does not have: a failure, or nothing when the
reader ignores them.

#### JsonReader.array

```tondo
pub fn JsonReader.array(
    self,
    value: json.Value,
    path: String,
): Array[json.Value] ! ProtobufError
```

The items of a repeated field's array.

#### JsonReader.entries

```tondo
pub fn JsonReader.entries(
    self,
    value: json.Value,
    path: String,
): Array[(String, json.Value)] ! ProtobufError
```

The entries of a map's object.

#### JsonReader.int32

```tondo
pub fn JsonReader.int32(self, value: json.Value, path: String): Int32 ! ProtobufError
```

An `int32`, `sint32` or `sfixed32`.

#### JsonReader.uint32

```tondo
pub fn JsonReader.uint32(self, value: json.Value, path: String): UInt32 ! ProtobufError
```

A `uint32` or `fixed32`.

#### JsonReader.int64

```tondo
pub fn JsonReader.int64(self, value: json.Value, path: String): Int ! ProtobufError
```

An `int64`, `sint64` or `sfixed64`.

#### JsonReader.uint64

```tondo
pub fn JsonReader.uint64(self, value: json.Value, path: String): UInt ! ProtobufError
```

A `uint64` or `fixed64`.

#### JsonReader.double

```tondo
pub fn JsonReader.double(self, value: json.Value, path: String): Float ! ProtobufError
```

A `double`: a number, its text, or `"NaN"`, `"Infinity"` or
`"-Infinity"`.

#### JsonReader.float

```tondo
pub fn JsonReader.float(self, value: json.Value, path: String): Float32 ! ProtobufError
```

A `float`, as a `double` is read.

#### JsonReader.bool

```tondo
pub fn JsonReader.bool(self, value: json.Value, path: String): Bool ! ProtobufError
```

A `bool`.

#### JsonReader.string

```tondo
pub fn JsonReader.string(self, value: json.Value, path: String): String ! ProtobufError
```

A `string`.

#### JsonReader.bytes

```tondo
pub fn JsonReader.bytes(self, value: json.Value, path: String): Array[Byte] ! ProtobufError
```

`bytes`, in Base64 of either alphabet, with or without padding.

#### JsonReader.enumValue

```tondo
pub fn JsonReader.enumValue[T: Enum](self, value: json.Value, path: String): T? ! ProtobufError
```

An enum's value, by name or by number; `none` for a name the enum does
not have when the reader ignores unknown fields.

#### JsonReader.message

```tondo
pub fn JsonReader.message[T: Message](self, value: json.Value, path: String): T ! ProtobufError
```

A message, as its own `readJson` reads it.

#### JsonReader.timestamp

```tondo
pub fn JsonReader.timestamp(
    self,
    value: json.Value,
    path: String,
): time.DateTime ! ProtobufError
```

A `google.protobuf.Timestamp`, in RFC 3339.

#### JsonReader.duration

```tondo
pub fn JsonReader.duration(self, value: json.Value, path: String): time.Duration ! ProtobufError
```

A `google.protobuf.Duration`, in seconds: `1.5s`, `-3s`.

### JsonValue

```tondo
pub alias JsonValue = json.Value
```

A JSON value, which a message's `readJson` reads.

```tondo
import std.console
import std.json
import std.protobuf

/// A field of any JSON, `google.protobuf.Value` in a `.proto`.
pub type Event = {
    @protobuf.Field(1)
    name: String = ""
    @protobuf.Field(2)
    payload: protobuf.JsonValue = json.Value.Null
}

let event = Event { name: "signup", payload: json.parse(r"""{"plan": "pro", "seats": 3}""")? }
let read = protobuf.decode[Event](protobuf.encode(event))?
console.println("{read.payload}")
```
