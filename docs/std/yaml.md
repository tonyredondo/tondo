# std.yaml

YAML documents in a safe subset of YAML 1.2 as values of `Value`, and any type read from and written to YAML through the traits of `std.serialization`, on top of the runtime's parser and emitter.

```tondo
import std.console
import std.yaml

var doc = yaml.load("# The service.\nport: 8080   # where it listens\n")?
doc.set("port", 9090)?
console.print(doc.text())
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.yaml` shows the same in a terminal.

- [Functions](#functions): [`decode`](#decode), [`decodeAll`](#decodeall), [`encode`](#encode), [`encodeAll`](#encodeall), [`parse`](#parse), [`parseAll`](#parseall), [`load`](#load)
- [Value](#value): [`Value.isNull`](#valueisnull), [`Value.get`](#valueget), [`Value.at`](#valueat), [`Value.find`](#valuefind), [`Value.asText`](#valueastext), [`Value.asInt`](#valueasint), [`Value.asFloat`](#valueasfloat), [`Value.asBool`](#valueasbool), [`Value.asArray`](#valueasarray), [`Value.asMap`](#valueasmap)
- [YamlError](#yamlerror)
- [UnknownFields](#unknownfields)
- [Document](#document): [`Document.value`](#documentvalue), [`Document.find`](#documentfind), [`Document.text`](#documenttext), [`Document.writeTo`](#documentwriteto), [`Document.set`](#documentset), [`Document.insert`](#documentinsert), [`Document.remove`](#documentremove)
- [Node](#node)
- [NodeKind](#nodekind)
- [NodeStyle](#nodestyle)

### Functions

#### decode

```tondo
pub fn decode[T: serialization.Decode](
    text: String,
    unknownFields: UnknownFields = UnknownFields.Ignore,
): T ! YamlError
```

A value of `T` from YAML text, which must hold one document.

#### decodeAll

```tondo
pub fn decodeAll[T: serialization.Decode](
    text: String,
    unknownFields: UnknownFields = UnknownFields.Ignore,
): Array[T] ! YamlError
```

A value of `T` from each document of YAML text, in order.

#### encode

```tondo
pub fn encode[T: serialization.Encode](value: T): String
```

The YAML document of a value.

#### encodeAll

```tondo
pub fn encodeAll[T: serialization.Encode](values: Array[T]): String
```

The YAML documents of values, each after `---`.

#### parse

```tondo
pub fn parse(text: String): Value ! YamlError
```

A document of unknown shape.

#### parseAll

```tondo
pub fn parseAll(text: String): Array[Value] ! YamlError
```

Every document of the text.

#### load

```tondo
pub fn load(text: String): Document ! YamlError
```

The first document of YAML text, which `set` changes; the documents
after it stay as they are.

### Value

```tondo
pub enum Value {
    Null
    Bool(Bool)
    Int(Int)
    Float(Float)
    Text(String)
    Array(Array[Value])
    Map(Map[String, Value])
    /// A value with a tag of its own, as GitLab's `!reference [.setup,
    /// script]`: the tag, and the value, whose scalars are text. The other
    /// methods read the value, and writing keeps the tag.
    Tagged(String, Value)
}
```

A YAML value of unknown shape.

```tondo
import std.console
import std.yaml

let config = yaml.parse("server:\n  host: localhost\n  ports: [8080, 8081]\n")?
let host = config.find("server.host").andThen((value) { value.asText() })
let ports = config.find("server.ports").andThen((value) { value.asArray() }) ?? []
console.println("{host ?? "?"} with {ports.length()} ports")
```

Implements `Display`, `reflect.Reflect`, `serialization.Encode`, `serialization.Decode`.

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

### YamlError

```tondo
pub alias YamlError = serialization.Error
```

A failure to read YAML, with where it happened: the error of every
format.

```tondo
import std.console
import std.yaml

type Server = { port: Int }

match yaml.decode[Server]("port: eighty\n") {
    ok(server) => console.println("port {server.port}")
    err(error) => console.println("{error.line}:{error.column} {error.path}: {error.message}")
}
```

### UnknownFields

```tondo
pub alias UnknownFields = serialization.UnknownFields
```

What reading does with a member that the type has no field for.

```tondo
import std.console
import std.yaml

type Server = { port: Int }

match yaml.decode[Server]("port: 8080\nprot: 8081\n", unknownFields: yaml.UnknownFields.Reject) {
    ok(server) => console.println("port {server.port}")
    err(error) => console.println("rejected: {error.message}")
}
```

### Document

```tondo
pub type Document = {
    // private fields
}
```

A YAML document that keeps its text: its comments, styles and order,
and where each value is. `set` changes a value and leaves the rest of
the text as it was, byte for byte; `writeTo` writes it as it is, with no
copy. A config of a project is what it is for: the whole document is in
memory, as its text and its outline.

```tondo
import std.console
import std.yaml

var doc = yaml.load("# The service.\nname: api\nhosts:\n  - a.example.com   # main\n")?
doc.set("name", "gateway")?
doc.insert("hosts", 1, "b.example.com")?
doc.remove("hosts[0]")?
console.print(doc.text())
```

#### Document.value

```tondo
pub fn Document.value(self): Value
```

The value of the whole document.

#### Document.find

```tondo
pub fn Document.find(self, path: String): Node?
```

The node at a path of keys and indices, as `jobs.build.steps[0]` or
`jobs[".setup"].script`, the root at `""`; `none` when there is none. A
key that a merge key `<<` brings is in `value()`, and is not a node of
the text.

#### Document.text

```tondo
pub fn Document.text(self): String
```

The text of the document, with its changes.

#### Document.writeTo

```tondo
pub fn Document.writeTo(self, out: io.Writer): !io.IoError
```

Writes the text of the document, with its changes, to a stream.

#### Document.set

```tondo
pub fn Document.set[T: serialization.Encode](mut self, path: String, value: T): !YamlError
```

Sets the value at a path, written as `encode` writes it: in place of
the old one, as a new key at the end of its map, or as a new item at
the end of its list, with the keys a path names that are missing; the
rest of the text stays as it was. A map or a list written in flow
style, `{...}` or `[...]`, that gets a key or an item is written again
in block style.

#### Document.insert

```tondo
pub fn Document.insert[T: serialization.Encode](
    mut self,
    path: String,
    index: Int,
    value: T,
): !YamlError
```

Inserts a value in the sequence at a path before its item `index`, or
after its last when `index` is its length, written as `encode` writes
it; the rest of the text stays as it was. A sequence in flow style is
written again.

#### Document.remove

```tondo
pub fn Document.remove(mut self, path: String): !YamlError
```

Removes the value at a path from its map or sequence, with its lines
and the comment lines right above them; the rest of the text stays as
it was. A map or a sequence in flow style is written again, and one
left empty is written `{}` or `[]`.

### Node

```tondo
pub type Node = {
    value: Value
    line: Int
    column: Int
    comment: String?
    kind: NodeKind
    style: NodeStyle
    tag: String?
    anchor: String?
}
```

A value of a document and where it is: its line and column, counted
from 1, and the comment on its line, or else on the lines right above
it, without `#`; what it is and how it is written, with its tag and the
name of its anchor as the text has them.

```tondo
import std.console
import std.yaml

let doc = yaml.load("port: 8080   # where it listens\n")?
let port = doc.find("port").expect("port is set")
console.println("line {port.line}, column {port.column}: {port.comment ?? ""}")
```

### NodeKind

```tondo
pub enum NodeKind {
    Scalar
    Sequence
    Map
}
```

What a node is; an alias is what its anchor's value is.

```tondo
import std.console
import std.yaml

let doc = yaml.load("name: api\nhosts: [a, b]\nlimits: {{cpu: 2}}\n")?
for path in ["name", "hosts", "limits"] {
    let node = doc.find(path).expect("{path} is set")
    let kind = match node.kind {
        yaml.NodeKind.Scalar => "a scalar"
        yaml.NodeKind.Sequence => "a sequence"
        yaml.NodeKind.Map => "a map"
    }
    console.println("{path} is {kind}")
}
```

Implements `Display`.

### NodeStyle

```tondo
pub enum NodeStyle {
    Plain
    SingleQuoted
    DoubleQuoted
    Literal
    Folded
    Block
    Flow
    Alias
}
```

How a node is written: a scalar plain, in `'...'`, in `"..."`, as a
literal block `|` or a folded one `>`; a sequence or a map in block
style, or in flow style, `[...]` and `{...}`; or as an alias, `*name`.

```tondo
import std.console
import std.yaml

let doc = yaml.load("plain: tea\nquoted: \"cake\"\nhosts: [a, b]\n")?
for path in ["plain", "quoted", "hosts"] {
    let style = doc.find(path).expect("{path} is set").style
    let flow = style == yaml.NodeStyle.Flow
    let quoted = style == yaml.NodeStyle.DoubleQuoted
    console.println("{path}: flow {flow}, double-quoted {quoted}")
}
```

Implements `Display`.
