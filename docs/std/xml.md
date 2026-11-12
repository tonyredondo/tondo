# std.xml

XML 1.0 written from the program's types and read into them through std.serialization, as Go's encoding/xml does, and read as a tree when its shape is unknown. No DTD is read and no entity but the five predefined and the numeric ones, so that a document can neither ask for files nor grow without end.

```tondo
import std.console
import std.xml

@xml.Name("item")
pub type Item = {
    @xml.Attribute
    id: Int
    @xml.Text
    title: String
}

let text = xml.encode(Item { id: 1, title: "Tea & cake" })
let item = xml.decode[Item](text)?
console.println("{text} {item.title} {xml.parse(text)?.attribute("id")}")
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.xml` shows the same in a terminal.

- [Functions](#functions): [`parse`](#parse), [`decode`](#decode), [`encode`](#encode)
- [Name](#name)
- [Attribute](#attribute)
- [Text](#text)
- [XmlError](#xmlerror)
- [Element](#element): [`Element.child`](#elementchild), [`Element.children`](#elementchildren), [`Element.attribute`](#elementattribute), [`Element.text`](#elementtext)
- [Node](#node)

### Functions

#### parse

```tondo
pub fn parse(text: String): Element ! XmlError
```

The tree of an XML document of unknown shape: its root element.

#### decode

```tondo
pub fn decode[T: serialization.Decode](text: String): T ! XmlError
```

A value of `T` from XML: the root element, whatever its name, read as
`T`. Elements and attributes that `T` does not have are skipped.

#### encode

```tondo
pub fn encode[T: serialization.Encode](value: T, pretty: Bool = false): String
```

The XML of a value: an element named as its type, or as its
`@xml.Name`, in UTF-8 and without the declaration `<?xml ...?>`. With
`pretty`, each element in another goes on a line of its own, two spaces
deeper.

### Name

```tondo
pub annotation Name = {
    value: String
}
```

The name of the element of a type or a field, instead of its own:
`@xml.Name("item")`.

Implements `reflect.Reflect`.

### Attribute

```tondo
pub annotation Attribute = {}
```

Makes a field an attribute of the element of its record.

Implements `reflect.Reflect`.

### Text

```tondo
pub annotation Text = {}
```

Makes a field the text of the element of its record.

Implements `reflect.Reflect`.

### XmlError

```tondo
pub type XmlError = {
    message: String
    line: Int
    column: Int
}
```

Why a text is not XML, or not of the type it is read as: what is wrong,
and where.

```tondo
import std.console
import std.xml

match xml.parse("<note>\n  <to>Ada</from>\n</note>") {
    ok(note) => console.println(note.name)
    err(error) => console.println("{error.line}:{error.column}: {error.message}")
}
```

Implements `Display`, `reflect.Reflect`.

### Element

```tondo
pub type Element = {
    name: String
    attributes: Map[String, String]
    nodes: Array[Node]
}
```

An element: its name, with its prefix, as `atom:link`, its attributes
in their order, and its nodes.

```tondo
import std.console
import std.xml

let feed = xml.parse(r"""<feed><entry id="1"><title>Tea</title></entry><entry id="2"><title>Cake</title></entry></feed>""")?
for entry in feed.children("entry") {
    let title = entry.child("title").map((title) { title.text() }).unwrapOr("")
    console.println("{entry.attribute("id").unwrapOr("?")}: {title}")
}
```

Implements `Display`.

#### Element.child

```tondo
pub fn Element.child(self, name: String): Element?
```

The first element among its nodes named `name`.

#### Element.children

```tondo
pub fn Element.children(self, name: String): Array[Element]
```

The elements among its nodes named `name`, in their order.

#### Element.attribute

```tondo
pub fn Element.attribute(self, name: String): String?
```

The value of its attribute `name`.

#### Element.text

```tondo
pub fn Element.text(self): String
```

The text of its nodes, joined: `"a b"` for `<p>a <i>c</i>b</p>`.

### Node

```tondo
pub enum Node {
    Element(Element)
    Text(String)
}
```

What an element holds: elements and text, without comments or
processing instructions.

```tondo
import std.console
import std.xml

let paragraph = xml.parse("<p>Tea and <b>cake</b>.</p>")?
for node in paragraph.nodes {
    match node {
        xml.Node.Element(element) => console.println("element {element.name}")
        xml.Node.Text(text) => console.println("text \"{text}\"")
    }
}
```
