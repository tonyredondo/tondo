# std.meta

what a `derive` provider receives and how it writes the implementation it gives back. A provider runs while the program compiles, in the `meta` profile, and sees only its request.

```tondo
import std.meta

/// A `derive` provider: writes `impl Describe` for a record, its fields'
/// names.
pub fn provide(request: meta.DeriveRequest): meta.DeriveResponse ! meta.Error {
    let target = request.target()
    let names = target.fields().map((field) { field.name }).join(", ")
    var out = request.sourceBuilder()
    out.add(meta.code(r"""
        impl $trait for $type {
            fn describe(self): String { "$names" }
        }
        """, ["trait": request.traitName(), "type": target.typeName(), "names": names]))?
    out.finishDerive()?
}
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.meta` shows the same in a terminal.

- [Functions](#functions): [`literal`](#literal), [`code`](#code), [`serveGenerate`](#servegenerate), [`serveDerive`](#servederive)
- [Error](#error)
- [TypeRef](#typeref): [`TypeRef.identity`](#typerefidentity)
- [AnnotationView](#annotationview): [`AnnotationView.name`](#annotationviewname), [`AnnotationView.to`](#annotationviewto)
- [FieldView](#fieldview)
- [VariantView](#variantview)
- [ParameterView](#parameterview)
- [GenericView](#genericview)
- [DeclarationKind](#declarationkind)
- [Declaration](#declaration): [`Declaration.name`](#declarationname), [`Declaration.module`](#declarationmodule), [`Declaration.kind`](#declarationkind-1), [`Declaration.typeName`](#declarationtypename), [`Declaration.generics`](#declarationgenerics), [`Declaration.fields`](#declarationfields), [`Declaration.variants`](#declarationvariants), [`Declaration.underlying`](#declarationunderlying), [`Declaration.parameters`](#declarationparameters), [`Declaration.result`](#declarationresult), [`Declaration.annotations`](#declarationannotations)
- [DeriveRequest](#deriverequest): [`DeriveRequest.target`](#deriverequesttarget), [`DeriveRequest.traitName`](#deriverequesttraitname), [`DeriveRequest.sourceBuilder`](#deriverequestsourcebuilder)
- [Input](#input): [`Input.text`](#inputtext)
- [GenerateRequest](#generaterequest): [`GenerateRequest.inputs`](#generaterequestinputs), [`GenerateRequest.input`](#generaterequestinput), [`GenerateRequest.outputs`](#generaterequestoutputs), [`GenerateRequest.declarations`](#generaterequestdeclarations), [`GenerateRequest.sourceBuilder`](#generaterequestsourcebuilder)
- [SourceBuilder](#sourcebuilder): [`SourceBuilder.write`](#sourcebuilderwrite), [`SourceBuilder.finish`](#sourcebuilderfinish), [`SourceBuilder.renderType`](#sourcebuilderrendertype), [`SourceBuilder.add`](#sourcebuilderadd), [`SourceBuilder.error`](#sourcebuildererror), [`SourceBuilder.finishDerive`](#sourcebuilderfinishderive)
- [GenerateResponse](#generateresponse)
- [DeriveResponse](#deriveresponse)

### Functions

#### literal

```tondo
pub fn literal(value: String): String
```

The text as a Tondo string literal, which the code a generator writes
reads back as the same text: `"` and `\` escaped, `{` and `}` doubled,
and lines, tabs and other control characters as `\n`, `\t` and
`\u{...}`.

```tondo
import std.console
import std.meta

// `pub const Greeting: String = "Say \"hi\" to {{name}}"`
console.println("pub const Greeting: String = {meta.literal("Say \"hi\" to {{name}}")}")
```

#### code

```tondo
pub fn code(template: String, values: Map[String, String]): String
```

The template with each `$name` replaced by its value and `$$` by `$`;
with a raw template, the template is literally the code it writes. A
`$name` without a value panics.

#### serveGenerate

```tondo
pub fn serveGenerate(generator: fn(GenerateRequest): GenerateResponse ! Error)
```

Runs a generator on the request the toolchain wrote and writes its
response; the toolchain calls it from the program it builds around the
generator.

#### serveDerive

```tondo
pub fn serveDerive(provider: fn(DeriveRequest): DeriveResponse ! Error)
```

Runs a provider on the request the toolchain wrote and writes its
response; the toolchain calls it from the program it builds around the
provider.

### Error

```tondo
pub enum Error {
    UnknownInput(String)
    UnknownOutput(String)
    DuplicateOutput(String)
    MissingOutput(String)
    OutputLimit
    Provider(String)
    TypeRendering(String)
}
```

Why a provider cannot give its response.

```tondo
import std.meta

/// A generator whose input may be missing from the manifest.
pub fn generate(request: meta.GenerateRequest): meta.GenerateResponse ! meta.Error {
    let colors = match request.input("colors.txt") {
        ok(input) => input.text()?
        err(meta.Error.UnknownInput(name)) => fail meta.Error.Provider("add `{name}` to the generator's `inputs` in tondo.toml")
        err(error) => fail error
    }
    var out = request.sourceBuilder()
    out.write("palette", "pub const Count: Int = {colors.lines().length()}")?
    out.finish()?
}
```

Implements `Display`, `reflect.Reflect`.

### TypeRef

```tondo
pub type TypeRef = {
    // private fields
}
```

A type as the program writes it where the declaration is.

```tondo
import std.meta

/// Writes `impl Schema` with each field's type as the program writes it.
pub fn provide(request: meta.DeriveRequest): meta.DeriveResponse ! meta.Error {
    let target = request.target()
    var out = request.sourceBuilder()
    var lines: Array[String] = []
    for field in target.fields() {
        let written = out.renderType(field.typeRef)?
        lines.push("\"{field.name}: {written}\"")
    }
    out.add(meta.code(r"""
        impl $trait for $type {
            fn schema(self): Array[String] { [$lines] }
        }
        """, ["trait": request.traitName(), "type": target.typeName(), "lines": lines.join(", ")]))?
    out.finishDerive()?
}
```

Implements `Display`.

#### TypeRef.identity

```tondo
pub fn TypeRef.identity(self): String
```

What tells two types apart: their text where the declaration is.

### AnnotationView

```tondo
pub type AnnotationView = {
    // private fields
}
```

An annotation the program writes on a declaration, a variant, a field or
a parameter (language 27.13), with every field's value.

```tondo
import std.meta

/// The annotation `@db.Column(name: "...")` as this provider reads it.
type Column = { name: String }

/// Writes `impl Columns`: each field's column, its own name without one.
pub fn provide(request: meta.DeriveRequest): meta.DeriveResponse ! meta.Error {
    var columns: Array[String] = []
    for field in request.target().fields() {
        var column = field.name
        for annotation in field.annotations {
            if annotation.name().endsWith("Column") {
                let given: Column = annotation.to()?
                column = given.name
            }
        }
        columns.push("\"{column}\"")
    }
    var out = request.sourceBuilder()
    out.add(meta.code(r"""
        impl $trait for $type {
            fn columns(self): Array[String] { [$columns] }
        }
        """, ["trait": request.traitName(), "type": request.target().typeName(), "columns": columns.join(", ")]))?
    out.finishDerive()?
}
```

#### AnnotationView.name

```tondo
pub fn AnnotationView.name(self): String
```

Its type as the module of the declaration writes it: `binding.Check`.

#### AnnotationView.to

```tondo
pub fn AnnotationView.to[T: serialization.Decode](self): T ! Error
```

Its value as `T`, a record of the provider with the fields it reads:
with `type Check = { required: Bool, min: Int? }`,
`let check: Check = annotation.to()?`.

### FieldView

```tondo
pub type FieldView = {
    name: String
    typeRef: TypeRef
    private: Bool
    annotations: Array[AnnotationView]
}
```

A field of the declaration, private ones included.

```tondo
import std.meta

/// Writes `impl Describe`: the public fields with their values.
pub fn provide(request: meta.DeriveRequest): meta.DeriveResponse ! meta.Error {
    let shown = request.target().fields().filter((field) { not field.private }).map((field) { "{field.name}={{self.{field.name}}}" })
    var out = request.sourceBuilder()
    out.add(meta.code(r"""
        impl $trait for $type {
            fn describe(self): String { "$shown" }
        }
        """, ["trait": request.traitName(), "type": request.target().typeName(), "shown": shown.join(" ")]))?
    out.finishDerive()?
}
```

### VariantView

```tondo
pub type VariantView = {
    name: String
    fields: Array[FieldView]
    elements: Array[TypeRef]
    annotations: Array[AnnotationView]
}
```

A variant: its name and the fields or elements of its payload.

```tondo
import std.meta

/// Writes `impl Named` for an enum: the name of the variant a value is.
pub fn provide(request: meta.DeriveRequest): meta.DeriveResponse ! meta.Error {
    let target = request.target()
    var arms: Array[String] = []
    for variant in target.variants() {
        // A payload is matched without its values: `{ .. }` or a `_` each.
        let pattern = if not variant.fields.isEmpty() {
            " {{ .. }}"
        } else if variant.elements.isEmpty() {
            ""
        } else {
            "({variant.elements.map((_) { "_" }).join(", ")})"
        }
        arms.push("{target.name()}.{variant.name}{pattern} => \"{variant.name}\"")
    }
    var out = request.sourceBuilder()
    out.add(meta.code(r"""
        impl $trait for $type {
            fn variantName(self): String {
                match self {
                    $arms
                }
            }
        }
        """, ["trait": request.traitName(), "type": target.typeName(), "arms": arms.join("\n            ")]))?
    out.finishDerive()?
}
```

### ParameterView

```tondo
pub type ParameterView = {
    name: String
    typeRef: TypeRef
    annotations: Array[AnnotationView]
}
```

A parameter of a function.

```tondo
import std.meta

/// A generator that lists the functions annotated `@routes.Get(...)`, with
/// the parameters each takes from the path.
pub fn generate(request: meta.GenerateRequest): meta.GenerateResponse ! meta.Error {
    var routes: Array[String] = []
    for declaration in request.declarations() {
        if declaration.kind() != meta.DeclarationKind.Function {
            continue
        }
        let parameters = declaration.parameters().map((parameter) { "{parameter.name}: {parameter.typeRef.identity()}" })
        routes.push("\"{declaration.name()}({parameters.join(", ")})\"")
    }
    var out = request.sourceBuilder()
    out.write("routes", "pub const Routes: Array[String] = [{routes.join(", ")}]")?
    out.finish()?
}
```

### GenericView

```tondo
pub type GenericView = {
    name: String
    bounds: Array[String]
}
```

A type parameter and the bounds written on it.

```tondo
import std.meta

/// Writes `impl Show` for a generic type, asking `Show` of each parameter:
/// `impl[T: Show] Show for Box[T]`.
pub fn provide(request: meta.DeriveRequest): meta.DeriveResponse ! meta.Error {
    let target = request.target()
    let binders = target.generics().map((generic) { "{generic.name}: {request.traitName()}" })
    let header = if binders.isEmpty() { "impl" } else { "impl[{binders.join(", ")}]" }
    var out = request.sourceBuilder()
    out.add(meta.code(r"""
        $header $trait for $type {
            fn show(self): String { "$name" }
        }
        """, ["header": header, "trait": request.traitName(), "type": target.typeName(), "name": target.name()]))?
    out.finishDerive()?
}
```

### DeclarationKind

```tondo
pub enum DeclarationKind {
    Record
    Enum
    Newtype
    Function
}
```

What a declaration is: a record, an enum, a newtype or a function.

```tondo
import std.meta

/// A provider for records only, which says so of anything else.
pub fn provide(request: meta.DeriveRequest): meta.DeriveResponse ! meta.Error {
    let target = request.target()
    var out = request.sourceBuilder()
    match target.kind() {
        meta.DeclarationKind.Record => out.add(meta.code(r"""
            impl $trait for $type {
                fn fieldCount(self): Int { $count }
            }
            """, ["trait": request.traitName(), "type": target.typeName(), "count": "{target.fields().length()}"]))?
        meta.DeclarationKind.Enum => out.error("`{target.name()}` is an enum: derive it for a record")
        meta.DeclarationKind.Newtype => out.error("`{target.name()}` is a newtype: derive it for a record")
        meta.DeclarationKind.Function => out.error("`{target.name()}` is a function")
    }
    out.finishDerive()?
}
```

### Declaration

```tondo
pub type Declaration = {
    // private fields
}
```

The record, enum or newtype a `derive` names, or a declaration a
generator sees.

```tondo
import std.meta

/// A generator that writes a constant naming each record it may read, with
/// its module and fields.
pub fn generate(request: meta.GenerateRequest): meta.GenerateResponse ! meta.Error {
    var names: Array[String] = []
    for declaration in request.declarations() {
        if declaration.kind() == meta.DeclarationKind.Record {
            let fields = declaration.fields().map((field) { field.name }).join(" ")
            names.push("\"{declaration.module()}.{declaration.typeName()}: {fields}\"")
        }
    }
    var out = request.sourceBuilder()
    out.write(request.outputs()[0], "pub const Records: Array[String] = [{names.join(", ")}]")?
    out.finish()?
}
```

#### Declaration.name

```tondo
pub fn Declaration.name(self): String
```

The declaration's name, without its type parameters.

#### Declaration.module

```tondo
pub fn Declaration.module(self): String
```

The module that declares it, as `app.shapes`.

#### Declaration.kind

```tondo
pub fn Declaration.kind(self): DeclarationKind
```

What the declaration is.

#### Declaration.typeName

```tondo
pub fn Declaration.typeName(self): String
```

The type with its parameters, as an `impl` header names it: `Page[T]`.

#### Declaration.generics

```tondo
pub fn Declaration.generics(self): Array[GenericView]
```

The declaration's type parameters, with their bounds.

#### Declaration.fields

```tondo
pub fn Declaration.fields(self): Array[FieldView]
```

A record's fields, in declaration order; none for another kind.

#### Declaration.variants

```tondo
pub fn Declaration.variants(self): Array[VariantView]
```

An enum's variants, in declaration order; none for another kind.

#### Declaration.underlying

```tondo
pub fn Declaration.underlying(self): TypeRef?
```

The type a newtype wraps.

#### Declaration.parameters

```tondo
pub fn Declaration.parameters(self): Array[ParameterView]
```

A function's parameters; none for a type.

#### Declaration.result

```tondo
pub fn Declaration.result(self): TypeRef?
```

A function's result, with its `Result` or `Option`; `none` for a type.

#### Declaration.annotations

```tondo
pub fn Declaration.annotations(self): Array[AnnotationView]
```

The program's annotations of the declaration, in the order they are
written.

### DeriveRequest

```tondo
pub type DeriveRequest = {
    // private fields
}
```

What `derive` asks of a provider: an `impl` of the trait, as the program
writes its name, for the declaration.

```tondo
import std.meta

/// A `derive` provider, which the package that declares the trait names in
/// its tondo.toml, as `[derive] Identify = "derive/identify.to"`: writes
/// `impl Identify` with the type's module and name.
pub fn provide(request: meta.DeriveRequest): meta.DeriveResponse ! meta.Error {
    let target = request.target()
    var out = request.sourceBuilder()
    out.add(meta.code(r"""
        impl $trait for $type {
            fn identify(self): String { "$module.$name" }
        }
        """, ["trait": request.traitName(), "type": target.typeName(), "module": target.module(), "name": target.name()]))?
    out.finishDerive()?
}
```

#### DeriveRequest.target

```tondo
pub fn DeriveRequest.target(self): Declaration
```

The record, enum or newtype the `derive` names.

#### DeriveRequest.traitName

```tondo
pub fn DeriveRequest.traitName(self): String
```

The trait as the `derive` writes it, such as `show.Show`, which the
`impl` header names.

#### DeriveRequest.sourceBuilder

```tondo
pub fn DeriveRequest.sourceBuilder(self): SourceBuilder
```

A builder of the code the provider writes for the `derive`.

### Input

```tondo
pub type Input = {
    name: String
    bytes: Array[Byte]
    sha256: String
}
```

A file a generator is given: its name in the manifest, its bytes and
their SHA-256.

```tondo
import std.meta

/// The size and hash of each input file, which change when the file does.
pub fn generate(request: meta.GenerateRequest): meta.GenerateResponse ! meta.Error {
    var lines: Array[String] = []
    for (index, input) in request.inputs().enumerate() {
        lines.push("// {input.name}")
        lines.push("pub const Size{index}: Int = {input.bytes.length()}")
        lines.push("pub const Hash{index}: String = \"{input.sha256}\"")
    }
    var out = request.sourceBuilder()
    out.write("files", lines.join("\n"))?
    out.finish()?
}
```

#### Input.text

```tondo
pub fn Input.text(self): String ! Error
```

The bytes as UTF-8 text.

### GenerateRequest

```tondo
pub type GenerateRequest = {
    // private fields
}
```

What a generator is asked for: the modules it declares, from its inputs
and the public declarations it may look at.

```tondo
import std.meta

/// The generator of tondo.toml's `[generators.messages]`, with
/// `program = "gen/messages.to"`, `inputs = ["messages.txt"]` and
/// `outputs = ["messages"]`: an enum with a variant for the key of each
/// `Key = text` line.
pub fn generate(request: meta.GenerateRequest): meta.GenerateResponse ! meta.Error {
    var keys: Array[String] = []
    for line in request.input("messages.txt")?.text()?.lines() {
        match line.find(" = ") {
            some(at) => keys.push((line.slice(0, at) ?? "").trim())
            none => {}
        }
    }
    var out = request.sourceBuilder()
    out.write(request.outputs()[0], "pub enum Message {{\n    {keys.join("\n    ")}\n}}")?
    out.finish()?
}
```

#### GenerateRequest.inputs

```tondo
pub fn GenerateRequest.inputs(self): Array[Input]
```

The files the generator reads, as the project names them.

#### GenerateRequest.input

```tondo
pub fn GenerateRequest.input(self, name: String): Input ! Error
```

The input called `name`; another name is `Error.UnknownInput`.

#### GenerateRequest.outputs

```tondo
pub fn GenerateRequest.outputs(self): Array[String]
```

The modules it writes, as `proto.user`.

#### GenerateRequest.declarations

```tondo
pub fn GenerateRequest.declarations(self): Array[Declaration]
```

The declarations of the program the generator may read.

#### GenerateRequest.sourceBuilder

```tondo
pub fn GenerateRequest.sourceBuilder(self): SourceBuilder
```

A builder of the modules the generator writes.

### SourceBuilder

```tondo
pub type SourceBuilder = {
    // private fields
}
```

The source a provider or generator gives back, and what went wrong.

```tondo
import std.meta

/// Writes `impl Validate` for records whose fields are all `String`, and
/// says which field stops it otherwise.
pub fn provide(request: meta.DeriveRequest): meta.DeriveResponse ! meta.Error {
    let target = request.target()
    var out = request.sourceBuilder()
    var checks: Array[String] = []
    for field in target.fields() {
        if out.renderType(field.typeRef)? != "String" {
            out.error("the field `{field.name}` is not a `String`")
            continue
        }
        checks.push("not self.{field.name}.isEmpty()")
    }
    out.add(meta.code(r"""
        impl $trait for $type {
            fn isComplete(self): Bool { $checks }
        }
        """, ["trait": request.traitName(), "type": target.typeName(), "checks": checks.join(" and ")]))?
    out.finishDerive()?
}
```

#### SourceBuilder.write

```tondo
pub fn SourceBuilder.write(mut self, output: String, source: String): !Error
```

A module a generator declares, once.

#### SourceBuilder.finish

```tondo
pub fn SourceBuilder.finish(self): GenerateResponse ! Error
```

Every module the generator declares.

#### SourceBuilder.renderType

```tondo
pub fn SourceBuilder.renderType(mut self, ty: TypeRef): String ! Error
```

The type as the declaration's module writes it.

#### SourceBuilder.add

```tondo
pub fn SourceBuilder.add(mut self, source: String): !Error
```

The implementation, once.

#### SourceBuilder.error

```tondo
pub fn SourceBuilder.error(mut self, message: String)
```

A problem with the declaration, which the compiler reports at the
`derive`; the response is not published.

#### SourceBuilder.finishDerive

```tondo
pub fn SourceBuilder.finishDerive(self): DeriveResponse ! Error
```

What the provider gives back: the code it wrote and the problems it
found; with neither, `Error.MissingOutput`.

### GenerateResponse

```tondo
pub type GenerateResponse = {
    // private fields
}
```

What a generator gives back.

```tondo
import std.meta

/// The smallest generator: one module, with one constant.
pub fn generate(request: meta.GenerateRequest): meta.GenerateResponse ! meta.Error {
    var out = request.sourceBuilder()
    out.write("built", "pub const Generated: Bool = true")?
    // Every output the manifest names must be written, once.
    out.finish()?
}
```

### DeriveResponse

```tondo
pub type DeriveResponse = {
    // private fields
}
```

What a provider gives back.

```tondo
import std.meta

/// The smallest provider: an `impl` whose method names the type.
pub fn provide(request: meta.DeriveRequest): meta.DeriveResponse ! meta.Error {
    var out = request.sourceBuilder()
    out.add("impl {request.traitName()} for {request.target().typeName()} {{\n    fn typeName(self): String {{ \"{request.target().name()}\" }}\n}}")?
    out.finishDerive()?
}
```
