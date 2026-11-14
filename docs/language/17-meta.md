# Metaprogramming

Tondo has no macros. Code that writes code is either the compiler doing a
job every program needs, or a Tondo program that runs while another one
compiles and whose output can be read. This chapter covers what the compiler
writes by itself, annotations, reflection, `derive` and generators.

- [What the compiler writes for you](#what-the-compiler-writes-for-you)
- [Annotations](#annotations)
- [Reflection](#reflection)
- [Types described at run time](#types-described-at-run-time)
- [derive](#derive)
- [Writing a provider](#writing-a-provider)
- [Generators](#generators)
- [What a provider may do](#what-a-provider-may-do)
- [Which to use](#which-to-use)

## What the compiler writes for you

Much of what other languages need macros, attributes or code generation for
is simply done:

| Every type gets | When |
| --- | --- |
| `==`, hashing, copying | its parts have them |
| being sent to a task, or shared | its parts can be |
| `serialization.Encode` and `Decode`, for every format | the program reads or writes it |
| `reflect.Reflect` | the program uses `std.reflect` |
| `Display` | it is an enum whose variants hold no data |

```tondo
import std.console
import std.json
import std.toml
import std.yaml

pub enum Role {
    Admin
    Editor
    Viewer
}

pub type User = {
    name: String
    role: Role = Role.Viewer
    tags: Array[String] = []
}

fn main(): !(json.JsonError | toml.TomlError) {
    let ada = User { name: "Ada", role: Role.Admin, tags: ["core"] }
    let text = json.encode(ada)
    console.println(text)
    console.println("{json.decode[User](text)? == ada}")
    console.print(toml.encode(ada)?)
    console.print(yaml.encode(ada))
}
```

```text output
{"name":"Ada","role":"Admin","tags":["core"]}
true
name = "Ada"
role = "Admin"
tags = ["core"]
name: Ada
role: Admin
tags:
  - core
```

Nothing was derived, registered or annotated. One encoder and one decoder
are written for `User` when the program compiles, and every format uses
them: JSON, YAML, TOML, XML, CBOR, MessagePack, the rows of a database, the
body of a request, the arguments of a command line. They are ordinary
compiled code, not a walk over the type as the program runs.

## Annotations

An annotation attaches data to a declaration. It is declared as a record
whose fields have defaults, and written with `@` before a type, a field, a
variant, a function, a method or a parameter:

```tondo
import std.console
import std.reflect

/// What a field of a form must hold.
pub annotation Check = {
    required: Bool = false
    min: Int? = none
    label: String = ""
}

pub type Signup = {
    @Check(required: true, min: 3, label: "User name")
    name: String
    @Check(required: true)
    email: String
    newsletter: Bool
}

fn main() {
    for field in reflect.typeInfo[Signup]().fields() {
        match field.annotation[Check]() {
            some(check) => console.println("{field.name()}: required {check.required}, min {check.min}, label [{check.label}]")
            none => console.println("{field.name()}: unchecked")
        }
    }
}
```

```text output
name: required true, min some(3), label [User name]
email: required true, min none, label []
newsletter: unchecked
```

- An annotation's arguments are constants, by position or by name. A field
  left out takes its default.
- It is read with its type: `field.annotation[Check]()` gives a `Check?`.
  Nothing is a string to parse.
- From another module it is written with the module's name:
  `@forms.Check(min: 3)`.
- An annotation does nothing by itself. It is data for whoever reads it:
  the program through reflection, a `derive` provider, a generator, or a
  library.

The standard library declares the annotations its modules read, as
`@validate.Min(18)`, `@cli.Option(short: "d")`, `@xml.Attribute` and
`@protobuf.Field(1)`. A few lower-case ones are the language's own: `@name`,
`@ignore`, `@omitNone`, `@secret`, `@deprecated`, and on a context's fields
`@log` and `@label` ([Records](05-records.md)).

## Reflection

`std.reflect` reaches a value's fields and methods by their names, when the
names are only known as the program runs:

```tondo
import std.console
import std.reflect

pub type Address = { city: String }
pub type User = { name: String, age: Int, address: Address }

pub fn User.greet(self, greeting: String): String {
    "{greeting}, {self.name}"
}

fn main(): !reflect.ReflectError {
    var user = User { name: "Ada", age: 36, address: Address { city: "London" } }

    let city: String = reflect.get(user, "address.city")?
    reflect.set(mut user, "age", 37)?
    let greeting: String = reflect.call(user, "greet", "Hello")?
    console.println("{city} {user.age} {greeting}")

    // A value of any type, to walk.
    let value = reflect.valueOf(user)
    for field in value.fields() {
        console.println("{field.name} = {field.value}")
    }
    let back: User = value.to()?
    // A path that is only known as the program runs.
    let path = ["address", "street"].join(".")
    let street: String ! reflect.ReflectError = reflect.get(user, path)
    console.println("{back == user} {street.isErr()}")
}
```

```text output
London 37 Hello, Ada
name = Ada
age = 37
address = Address { city: "London" }
true true
```

Each operation gives a result: a path that does not exist or a type that
does not fit is an error value, not a crash. When the path is written as a
literal, the compiler checks it too, and a wrong one does not compile:

```text
error[E2110]: `reflect.get` cannot reach `missing`: `User` has no field `missing`; its fields are `name`, `age` and `address`
```

Reflection sees what is public.

It is the tool for a program that handles types it was not written for: a
template engine, a GraphQL server, an inspector, a form built from a record.
`std.template`, `std.graphql` and `std.mcp` are built on it. Ordinary code,
which knows its types, does not need it and is faster without.

## Types described at run time

`reflect.typeInfo[T]()` describes a type: its name, its fields with their
types, annotations and documentation, its variants, its methods.

```tondo
import std.console
import std.reflect

/// Someone who can sign in.
pub type User = {
    /// The name shown to others.
    name: String
    age: Int
    email: String?
}

pub enum Status {
    Active
    Suspended(String)
}

fn main() {
    let info = reflect.typeInfo[User]()
    console.println("{info.qualifiedName()}: {info.docs() ?? ""}")
    for field in info.fields() {
        console.println("  {field.name()}: {field.typeInfo().qualifiedName()} {field.docs() ?? ""}")
    }
    let status = reflect.typeInfo[Status]()
    console.println("{status.kind()} with {status.variants().length()} variants")
}
```

```text output
User: Someone who can sign in.
  name: String The name shown to others.
  age: Int 
  email: String? 
Enum with 2 variants
```

The documentation comments are there too, which is how `std.cli` makes a
command's help of a record, and `std.http` an OpenAPI document of a
handler.

## derive

`derive Trait for Type` asks for an `impl` to be written when the program
compiles. The package that declares the trait names the program that writes
it, the trait's provider:

```text
// src/main.to
import describe
import std.console

pub type Point = { x: Int, y: Int }

derive describe.Describe for Point

fn main() {
    console.println(Point { x: 3, y: 4 }.describe())
}
```

```text
Point: x = 3, y = 4
```

`tondo expand` prints what each `derive` wrote, as source:

```text
// derive describe.Describe for Point (src/main.to:7:1)
impl describe.Describe for Point {
    fn describe(self): String {
        "Point: x = {self.x}, y = {self.y}"
    }
}
```

That is the whole of it: what a `derive` produces is Tondo code that could
have been written by hand, checked like any other, and shown on request.
Nothing is rewritten, and a `derive` cannot change the type it is for or any
other code. Several traits go in one line, `derive a.Show + b.Hash for
Point`, and a `derive` sits in the module of its type.

## Writing a provider

The package of the trait declares it and names its provider in
`tondo.toml`:

```toml
[package]
name = "describe"

[derive]
Describe = "derive/describe.to"
```

```text
// libs/describe/src/describe.to
/// A type that can say what it holds, field by field.
pub trait Describe {
    fn describe(self): String
}
```

The provider is a Tondo program with one function. It receives the
declaration it is asked about and gives back source:

```text
// libs/describe/derive/describe.to
import std.meta

/// Writes `impl Describe` for a record: its name and its fields.
pub fn provide(request: meta.DeriveRequest): meta.DeriveResponse ! meta.Error {
    let target = request.target()
    var out = request.sourceBuilder()
    if target.kind() != meta.DeclarationKind.Record {
        out.error("`Describe` derives only for records, and `{target.name()}` is not one")
        return out.finishDerive()?
    }
    var parts: Array[String] = []
    for field in target.fields() {
        parts.push(meta.code(r"$name = {self.$name}", ["name": field.name]))
    }
    out.add(meta.code(r"""
        impl $trait for $type {
            fn describe(self): String {
                "$name: $fields"
            }
        }
        """, [
        "trait": request.traitName(),
        "type": target.typeName(),
        "name": target.name(),
        "fields": parts.join(", "),
    ]))?
    out.finishDerive()
}
```

- `request.target()` is the declaration: its name, kind, generic
  parameters, fields or variants, each with its type and its annotations.
- `meta.code(template, values)` fills a piece of source, putting each
  `$name` in. The template is a raw string, so its braces are the generated
  code's.
- `out.error(message)` refuses, with a message the user sees at the
  `derive` line.
- A provider reads annotations to take instructions: `field.annotations`,
  and `annotation.to()` to decode one into a record the provider declares.

## Generators

A generator writes whole modules before the build, from files of the package
and from its declarations. It is for code that comes from data: constants
from a table, a client from a schema, a router from the handlers there are.

```toml
[package]
name = "app"

[generators.colors]
program = "gen/colors.to"
inputs = ["colors.txt"]
outputs = ["palette"]
```

```text
// gen/colors.to: a constant for each line of colors.txt, as `red #ff0000`
import std.meta

pub fn generate(request: meta.GenerateRequest): meta.GenerateResponse ! meta.Error {
    let input = request.input("colors.txt")?
    var lines: Array[String] = []
    for line in (input.text() ?? "").lines() {
        let (name, hex) = line.splitOnce(" ") ?? ("", "")
        let constant = "{name[:1].toUpper()}{name[1:]}"
        lines.push(meta.code(r"pub const $name: String = $hex", ["name": constant, "hex": "\"{hex}\""]))
    }
    var out = request.sourceBuilder()
    out.write("palette", lines.join("\n"))?
    out.finish()
}
```

The program then imports `app.palette` like any module of its own, and
`tondo expand` shows what was written:

```text
// generator colors wrote palette (`[generators.colors]` of the package `app`)
pub const Red: String = "#ff0000"

pub const Green: String = "#00ff00"
```

`inputs` are the files the generator may read, `outputs` the modules it
writes, and `roots = ["models.User"]` the declarations of the package it is
shown, which may be types or public functions with their parameters and
annotations. Changing an input runs the generator again; nothing else does.

## What a provider may do

A provider or a generator runs in a sandbox. It gets its request and gives
its response, and that is all: it cannot read files other than its declared
inputs, reach the network, read the clock or the environment, or use
randomness that is not seeded. Importing `std.fs` in one is a compile error:

```text
error[E2108]: `std.fs` is not available to a provider or generator, which runs while the program compiles and works only on its request
```

It also has a budget of steps, so one that loops ends with an error instead
of hanging the build.

The point is that a build gives the same result every time, on every
machine, and that reading a dependency's `derive` cannot be a way to run
something on the machine that compiles it.

## Which to use

| To | Use |
| --- | --- |
| read and write a type in any format | nothing: it is there |
| compare, copy, hash, send a type | nothing: it is there |
| attach data to a declaration | an `annotation` |
| act on names known only as the program runs | `std.reflect` |
| write the same `impl` for many types | `derive`, with a provider |
| write modules from data or from the package's declarations | a generator |
| check that what is read obeys rules | `std.validate`, whose annotations the decoder checks |

Reach for the lowest row that does the job. Code that is written out can be
read, searched and debugged; code that is generated can at least be printed,
with `tondo expand`.

Next: [Unsafe and C](18-unsafe.md).
