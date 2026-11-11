# std.reflect

reading, writing and calling by name on values whose type a tool does not know beforehand. The compiler implements `Reflect` for every record, enum and newtype whose parts do, with code that reaches each field and method directly; a `Value` keeps a value with those operations of its type.

```tondo
import std.console
import std.reflect

pub type User = { name: String, age: Int }

let ada = User { name: "Ada", age: 36 }
for field in reflect.typeInfo[User]().fields() {
    console.println("{field.name()}")
}
console.println(reflect.get[String, User](ada, "name")?)
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.reflect` shows the same in a terminal.

- [Functions](#functions): [`valueOf`](#valueof), [`get`](#get), [`set`](#set), [`call`](#call), [`callMut`](#callmut), [`convert`](#convert), [`annotationsOf`](#annotationsof), [`typeInfo`](#typeinfo), [`typeOf`](#typeof)
- [ReflectErrorKind](#reflecterrorkind)
- [ReflectError](#reflecterror): [`ReflectError.noField`](#reflecterrornofield), [`ReflectError.noMethod`](#reflecterrornomethod), [`ReflectError.wrongType`](#reflecterrorwrongtype), [`ReflectError.arguments`](#reflecterrorarguments), [`ReflectError.privateFields`](#reflecterrorprivatefields), [`ReflectError.needsMut`](#reflecterrorneedsmut)
- [Field](#field)
- [Value](#value): [`Value.brokenRules`](#valuebrokenrules), [`Value.get`](#valueget), [`Value.set`](#valueset), [`Value.call`](#valuecall), [`Value.callMut`](#valuecallmut), [`Value.to`](#valueto), [`Value.typeName`](#valuetypename), [`Value.typeInfo`](#valuetypeinfo), [`Value.fields`](#valuefields), [`Value.elements`](#valueelements), [`Value.entries`](#valueentries), [`Value.variant`](#valuevariant), [`Value.optional`](#valueoptional)
- [Reflect](#reflect)
- [TypeId](#typeid)
- [PrimitiveKind](#primitivekind)
- [AppliedKind](#appliedkind)
- [ReferenceKind](#referencekind)
- [TypeKind](#typekind)
- [TypeCapability](#typecapability)
- [VariantPayloadKind](#variantpayloadkind)
- [ParameterMode](#parametermode)
- [TypeDescription](#typedescription): [`TypeDescription.of`](#typedescriptionof)
- [TypeInfo](#typeinfo-1): [`TypeInfo.id`](#typeinfoid), [`TypeInfo.qualifiedName`](#typeinfoqualifiedname), [`TypeInfo.kind`](#typeinfokind), [`TypeInfo.genericArguments`](#typeinfogenericarguments), [`TypeInfo.capabilities`](#typeinfocapabilities), [`TypeInfo.fields`](#typeinfofields), [`TypeInfo.variants`](#typeinfovariants), [`TypeInfo.methods`](#typeinfomethods), [`TypeInfo.tupleElements`](#typeinfotupleelements), [`TypeInfo.underlying`](#typeinfounderlying), [`TypeInfo.annotation`](#typeinfoannotation), [`TypeInfo.annotations`](#typeinfoannotations), [`TypeInfo.docs`](#typeinfodocs)
- [FieldInfo](#fieldinfo): [`FieldInfo.of`](#fieldinfoof), [`FieldInfo.annotated`](#fieldinfoannotated), [`FieldInfo.externallyNamed`](#fieldinfoexternallynamed), [`FieldInfo.externalName`](#fieldinfoexternalname), [`FieldInfo.defaulted`](#fieldinfodefaulted), [`FieldInfo.default`](#fieldinfodefault), [`FieldInfo.annotation`](#fieldinfoannotation), [`FieldInfo.annotations`](#fieldinfoannotations), [`FieldInfo.name`](#fieldinfoname), [`FieldInfo.ordinal`](#fieldinfoordinal), [`FieldInfo.docs`](#fieldinfodocs), [`FieldInfo.typeInfo`](#fieldinfotypeinfo)
- [VariantInfo](#variantinfo): [`VariantInfo.of`](#variantinfoof), [`VariantInfo.externallyNamed`](#variantinfoexternallynamed), [`VariantInfo.externalName`](#variantinfoexternalname), [`VariantInfo.annotated`](#variantinfoannotated), [`VariantInfo.documented`](#variantinfodocumented), [`VariantInfo.docs`](#variantinfodocs), [`VariantInfo.annotation`](#variantinfoannotation), [`VariantInfo.annotations`](#variantinfoannotations), [`VariantInfo.name`](#variantinfoname), [`VariantInfo.ordinal`](#variantinfoordinal), [`VariantInfo.payloadKind`](#variantinfopayloadkind), [`VariantInfo.tupleElements`](#variantinfotupleelements), [`VariantInfo.fields`](#variantinfofields)
- [MethodInfo](#methodinfo): [`MethodInfo.of`](#methodinfoof), [`MethodInfo.annotated`](#methodinfoannotated), [`MethodInfo.documented`](#methodinfodocumented), [`MethodInfo.docs`](#methodinfodocs), [`MethodInfo.annotation`](#methodinfoannotation), [`MethodInfo.annotations`](#methodinfoannotations), [`MethodInfo.name`](#methodinfoname), [`MethodInfo.receiver`](#methodinforeceiver), [`MethodInfo.function`](#methodinfofunction)
- [ParameterInfo](#parameterinfo): [`ParameterInfo.of`](#parameterinfoof), [`ParameterInfo.annotated`](#parameterinfoannotated), [`ParameterInfo.name`](#parameterinfoname), [`ParameterInfo.annotation`](#parameterinfoannotation), [`ParameterInfo.annotations`](#parameterinfoannotations), [`ParameterInfo.position`](#parameterinfoposition), [`ParameterInfo.typeInfo`](#parameterinfotypeinfo), [`ParameterInfo.mode`](#parameterinfomode)
- [FunctionInfo](#functioninfo): [`FunctionInfo.of`](#functioninfoof), [`FunctionInfo.parameters`](#functioninfoparameters), [`FunctionInfo.outcome`](#functioninfooutcome), [`FunctionInfo.variadic`](#functioninfovariadic), [`FunctionInfo.isUnsafe`](#functioninfoisunsafe)
- [For other types](#for-other-types)

### Functions

#### valueOf

```tondo
pub fn valueOf[T: Reflect](value: T): Value
```

A value of any type that implements `Reflect`, with its operations.

#### get

```tondo
pub fn get[T: Reflect, S: Reflect](value: S, path: String): T ! ReflectError
```

The value at a path of fields, indexes and map keys separated by `.`.

#### set

```tondo
pub fn set[S: Reflect](value: mut S, path: String, new: Value): !ReflectError
```

Changes the value at a path, to a value of its type or one that
converts to it.

#### call

```tondo
pub fn call[R: Reflect, S: Reflect](target: S, method: String, args: ...Value): R ! ReflectError
```

Calls a `pub` method with `self` by name.

#### callMut

```tondo
pub fn callMut[R: Reflect, S: Reflect](
    target: mut S,
    method: String,
    args: ...Value,
): R ! ReflectError
```

Calls a `pub` method by name, which may change the value.

#### convert

```tondo
pub fn convert[T: Reflect, S: Reflect](value: S): T
```

A value of another type of the same shape; the compiler checks the
shapes when it compiles the call.

#### annotationsOf

```tondo
pub fn annotationsOf[T](place: Int, index: Int, part: Int, method: String): Array[Value]
```

The program's annotations of a thing, which the code the compiler writes
for a type asks for; its values come from the annotations themselves
(language 27.13).

#### typeInfo

```tondo
pub fn typeInfo[T: Reflect](): TypeInfo
```

The description of the type `T`.

#### typeOf

```tondo
pub fn typeOf[T: Reflect](value: T): TypeInfo
```

The description of the type of a value.

### ReflectErrorKind

```tondo
pub enum ReflectErrorKind {
    NoField
    NoMethod
    NoValue
    WrongType
    WrongArguments
    OutOfRange
}
```

What went wrong.

```tondo
import std.console
import std.reflect

pub type User = { name: String, age: Int }

let user = User { name: "Ada", age: 36 }
for path in ["name", "email", "age.years"] {
    match reflect.valueOf(user).get(path) {
        ok(value) => console.println("{path} = {value}")
        err(error) if error.kind == reflect.ReflectErrorKind.NoField => console.println("{path}: no such field")
        err(error) => console.println("{path}: {error.message}")
    }
}
```

Implements `Display`.

### ReflectError

```tondo
pub type ReflectError = {
    kind: ReflectErrorKind
    path: String
    message: String
}
```

The path or method that failed, and how to fix the call.

```tondo
import std.console
import std.reflect

pub type User = { name: String, age: Int }

// A path the program reads at run time, as from a template or a query.
fn read(user: User, path: String): String ! reflect.ReflectError {
    "{reflect.valueOf(user).get(path)?}"
}

match read(User { name: "Ada", age: 36 }, "agee") {
    ok(text) => console.println(text)
    err(error) => console.println("{error.kind} at `{error.path}`: {error.message}")
}
```

Implements `Display`.

#### ReflectError.noField

```tondo
pub fn ReflectError.noField(typeName: String, name: String, fields: Array[String]): ReflectError
```

A field, element or key a value does not have.

#### ReflectError.noMethod

```tondo
pub fn ReflectError.noMethod(typeName: String, name: String, methods: Array[String]): ReflectError
```

A method a value does not have.

#### ReflectError.wrongType

```tondo
pub fn ReflectError.wrongType(expected: String, value: Value): ReflectError
```

A value of a type that does not convert to the one wanted.

#### ReflectError.arguments

```tondo
pub fn ReflectError.arguments(
    typeName: String,
    method: String,
    expected: Int,
    found: Int,
    optional: Int = 0,
    variadic: Bool = false,
): ReflectError
```

A call with a number of arguments the method does not take: `expected`,
and up to `optional` more that have defaults, or any more with
`variadic`.

#### ReflectError.privateFields

```tondo
pub fn ReflectError.privateFields(typeName: String, value: Value): ReflectError
```

A conversion to a type with private fields, which only it can build.

#### ReflectError.needsMut

```tondo
pub fn ReflectError.needsMut(typeName: String, method: String): ReflectError
```

The method `Value.callMut` needs, called on a value with `call`.

### Field

```tondo
pub type Field = {
    name: String
    value: Value
}
```

A field of a value: its name and its value.

```tondo
import std.console
import std.reflect

pub type Config = { host: String, port: Int, debug: Bool }

// Each field of any record, by name, as a value of any type.
for field in reflect.valueOf(Config { host: "localhost", port: 8080, debug: false }).fields() {
    console.println("{field.name} = {field.value} ({field.value.typeName()})")
}
```

### Value

```tondo
pub type Value = {
    // private fields
}
```

Any value, with the operations of its type.

```tondo
import std.console
import std.reflect

pub type Address = { city: String }
pub type User = { name: String, address: Address }

pub fn User.greet(self, greeting: String): String {
    "{greeting}, {self.name}"
}

var value = reflect.valueOf(User { name: "Ada", address: Address { city: "London" } })
value.set("address.city", reflect.valueOf("Paris"))?
let greeting = value.call("greet", reflect.valueOf("Hello"))?
let user: User = value.to()?
console.println("{value.get("address.city")?} {greeting} {user.address.city}")
```

Implements `Display`, `Reflect`.

#### Value.brokenRules

```tondo
pub fn Value.brokenRules(self): Array[(String, String)]
```

The rules of its type's own, `validate.Rules`, that the value breaks:
each path, from the value, and what the rule asks.

#### Value.get

```tondo
pub fn Value.get(self, path: String): Value ! ReflectError
```

The value at a path of fields, indexes and map keys separated by `.`.

#### Value.set

```tondo
pub fn Value.set(mut self, path: String, new: Value): !ReflectError
```

Changes the value at a path, to a value of its type or one that
converts to it.

#### Value.call

```tondo
pub fn Value.call(self, method: String, args: ...Value): Value ! ReflectError
```

Calls a `pub` method with `self` by name.

#### Value.callMut

```tondo
pub fn Value.callMut(mut self, method: String, args: ...Value): Value ! ReflectError
```

Calls a `pub` method by name, which may change the value.

#### Value.to

```tondo
pub fn Value.to[T: Reflect](self): T ! ReflectError
```

The value as `T`: itself, or converted from a value of the same shape.

#### Value.typeName

```tondo
pub fn Value.typeName(self): String
```

The name of the type of the value.

#### Value.typeInfo

```tondo
pub fn Value.typeInfo(self): TypeInfo
```

The description of the type of the value.

#### Value.fields

```tondo
pub fn Value.fields(self): Array[Field]
```

The visible fields of a record, or of the active variant's record
payload.

#### Value.elements

```tondo
pub fn Value.elements(self): Array[Value]
```

The elements of an array, deque or set, or the active variant's payload.

#### Value.entries

```tondo
pub fn Value.entries(self): Array[(Value, Value)]
```

The entries of a map.

#### Value.variant

```tondo
pub fn Value.variant(self): String?
```

The name of the active variant of an enum, `Option` or `Result`.

#### Value.optional

```tondo
pub fn Value.optional(self, name: String): Value? ! ReflectError
```

A field that may be missing, as an optional field converts from: its
value, or `none` when the value has no field of that name.

### Reflect

```tondo
pub trait Reflect {
    /// The value of a field, element or key.
    fn reflectGet(self, name: String): Value ! ReflectError

    /// Changes a field, element or key.
    fn reflectSet(mut self, name: String, value: Value): !ReflectError

    /// The visible fields of a record or of the active variant's record
    /// payload; `none` for any other value.
    fn reflectFields(self): Array[Field]?

    /// The elements of a sequence or of the active variant's payload.
    fn reflectElements(self): Array[Value]?

    /// The entries of a map.
    fn reflectEntries(self): Array[(Value, Value)]?

    /// The name of the active variant of an enum.
    fn reflectVariant(self): String?

    /// Calls a `pub` method with `self`.
    fn reflectCall(self, method: String, args: Array[Value]): Value ! ReflectError

    /// Calls a `pub` method, which may change the value.
    fn reflectCallMut(mut self, method: String, args: Array[Value]): Value ! ReflectError

    /// The value as `Value` displays it; strings are quoted when `nested`.
    fn reflectShow(self, nested: Bool): String

    /// A value of this type from a value of another type of the same shape.
    fn reflectFrom(value: Value): Self ! ReflectError

    /// What the type is made of.
    fn reflectDescription(): TypeDescription

    /// The rules of its own the value breaks, as `validate.Rules` gives
    /// them: each path and message.
    fn reflectRules(self): Array[(String, String)]
}
```

The operations reflection has on a type. The compiler implements it for
every record, enum and newtype whose parts implement it; a program can
also implement it.

```tondo
import std.console
import std.reflect

/// A secret that reflection shows masked: templates, logs and the
/// debugger see `***` rather than its text.
pub type Secret = { priv text: String }

impl reflect.Reflect for Secret {
    fn reflectShow(self, _: Bool): String {
        "***"
    }
}

pub type Login = { user: String, password: Secret }

console.println("{reflect.valueOf(Login { user: "ada", password: Secret { text: "hunter2" } })}")
```

### TypeId

```tondo
pub type TypeId = {
    // private fields
}
```

The identity of a type in this executable: equal for the same type, and
meaningless outside it.

```tondo
import std.console
import std.reflect

pub type Celsius = { degrees: Float }
pub type Fahrenheit = { degrees: Float }

fn isCelsius(value: reflect.Value): Bool {
    value.typeInfo().id() == reflect.typeInfo[Celsius]().id()
}

console.println("{isCelsius(reflect.valueOf(Celsius { degrees: 21.0 }))} {isCelsius(reflect.valueOf(Fahrenheit { degrees: 70.0 }))}")
```

### PrimitiveKind

```tondo
pub enum PrimitiveKind {
    Bool
    Int
    Int8
    Int16
    Int32
    Int128
    UInt
    UInt8
    UInt16
    UInt32
    UInt128
    Float
    Float32
    Byte
    Char
    String
    Unit
    Never
}
```

Which primitive type a type is.

```tondo
import std.console
import std.reflect

pub type Row = { id: Int, name: String, score: Float, active: Bool }

// A column type of SQL for each field, from its primitive kind.
for field in reflect.typeInfo[Row]().fields() {
    let column = match field.typeInfo().kind() {
        reflect.TypeKind.Primitive(reflect.PrimitiveKind.Int) => "BIGINT"
        reflect.TypeKind.Primitive(reflect.PrimitiveKind.Float) => "DOUBLE"
        reflect.TypeKind.Primitive(reflect.PrimitiveKind.Bool) => "BOOLEAN"
        reflect.TypeKind.Primitive(reflect.PrimitiveKind.String) => "TEXT"
        _ => "BLOB"
    }
    console.println("{field.name()} {column}")
}
```

Implements `Display`.

### AppliedKind

```tondo
pub enum AppliedKind {
    Array
    Map
    Set
    Range
    Option
    Result
    Other
}
```

Which generic type a type applies: `Array[T]`, `Map[K, V]`, `T?`.

```tondo
import std.console
import std.reflect

pub type Order = { items: Array[String], prices: Map[String, Float], note: String? }

for field in reflect.typeInfo[Order]().fields() {
    let info = field.typeInfo()
    let shape = match info.kind() {
        reflect.TypeKind.Applied(reflect.AppliedKind.Array) => "a list of {info.genericArguments()[0].qualifiedName()}"
        reflect.TypeKind.Applied(reflect.AppliedKind.Map) => "a map"
        reflect.TypeKind.Applied(reflect.AppliedKind.Option) => "optional"
        _ => "something else"
    }
    console.println("{field.name()}: {shape}")
}
```

Implements `Display`.

### ReferenceKind

```tondo
pub enum ReferenceKind {
    Ref
    Pointer
}
```

Which reference a type is: a `Ref[T]` or a `Pointer[T]`.

```tondo
import std.console
import std.reflect

/// How a tool that walks types names each kind, references included.
fn kindName(kind: reflect.TypeKind): String {
    match kind {
        reflect.TypeKind.Reference(reflect.ReferenceKind.Ref) => "a shared reference, `Ref[T]`"
        reflect.TypeKind.Reference(reflect.ReferenceKind.Pointer) => "a raw address, `Pointer[T]`"
        reflect.TypeKind.Primitive(primitive) => "the primitive {primitive}"
        other => "{other}"
    }
}

console.println(kindName(reflect.typeInfo[Int]().kind()))
console.println(kindName(reflect.TypeKind.Reference(reflect.ReferenceKind.Ref)))
```

Implements `Display`.

### TypeKind

```tondo
pub enum TypeKind {
    Primitive(PrimitiveKind)
    Record
    Enum
    Newtype
    Tuple
    Union
    Function
    Applied(AppliedKind)
    Reference(ReferenceKind)
    Opaque
}
```

What kind of type a type is: a primitive, a record, an enum, a newtype,
a tuple, a union, a function, a generic type applied, a reference, or
one whose inside is hidden.

```tondo
import std.console
import std.reflect

pub type Point = { x: Int, y: Int }
pub enum Shape {
    Dot
    Circle(Float)
}
pub type Meters = Float

fn describe(info: reflect.TypeInfo): String {
    match info.kind() {
        reflect.TypeKind.Record => "a record of {info.fields().length()} fields"
        reflect.TypeKind.Enum => "an enum of {info.variants().length()} variants"
        reflect.TypeKind.Newtype => "a newtype of {info.underlying().map((inner) { inner.qualifiedName() }).unwrapOr("?")}"
        reflect.TypeKind.Primitive(_) => "a primitive"
        _ => "another kind"
    }
}

for info in [reflect.typeInfo[Point](), reflect.typeInfo[Shape](), reflect.typeInfo[Meters](), reflect.typeInfo[Int]()] {
    console.println("{info.qualifiedName()}: {describe(info)}")
}
```

Implements `Display`.

### TypeCapability

```tondo
pub enum TypeCapability {
    Copy
    Equatable
    Key
    Send
    Share
}
```

A capability a type has: `Copy`, `Equatable`, `Key`, `Send` or `Share`.

```tondo
import std.console
import std.reflect

pub type Point = { x: Int, y: Int }

let capabilities = reflect.typeInfo[Point]().capabilities()
console.println("{capabilities}")
console.println("a map key: {capabilities.contains(reflect.TypeCapability.Key)}, to another task: {capabilities.contains(reflect.TypeCapability.Send)}")
```

Implements `Display`.

### VariantPayloadKind

```tondo
pub enum VariantPayloadKind {
    Unit
    Tuple
    Record
}
```

What a variant holds: nothing, a tuple, or fields.

```tondo
import std.console
import std.reflect

pub enum Event {
    Started
    Moved(Int, Int)
    Renamed { from: String, to: String }
}

for variant in reflect.typeInfo[Event]().variants() {
    let holds = match variant.payloadKind() {
        reflect.VariantPayloadKind.Unit => "nothing"
        reflect.VariantPayloadKind.Tuple => "{variant.tupleElements().length()} values"
        reflect.VariantPayloadKind.Record => "fields {variant.fields().map((field) { field.name() })}"
    }
    console.println("{variant.name()} holds {holds}")
}
```

Implements `Display`.

### ParameterMode

```tondo
pub enum ParameterMode {
    Value
    Ref
    Mut
    Var
}
```

How a parameter receives its argument: by value, or lent as `ref`,
`mut` or `var`.

```tondo
import std.console
import std.reflect

pub type Counter = { count: Int }

pub fn Counter.value(self): Int {
    self.count
}

pub fn Counter.add(mut self, amount: Int) {
    self.count += amount
}

// Which methods change the value, as `Value.callMut` must call them.
for method in reflect.typeInfo[Counter]().methods() {
    let changes = method.receiver() == reflect.ParameterMode.Mut
    console.println("{method.name()} changes the counter: {changes}")
}
```

Implements `Display`.

### TypeDescription

```tondo
pub type TypeDescription = {
    kind: TypeKind
    generics: Array[TypeInfo]
    fields: Array[FieldInfo]
    variants: Array[VariantInfo]
    methods: Array[MethodInfo]
    elements: Array[TypeInfo]
    underlying: TypeInfo?
    /// The program's annotations of the type (language 27.13).
    annotations: Array[Value]
    /// Its `///` documentation.
    docs: String?
}
```

What a type is made of, as the code the compiler writes for it says.

```tondo
import std.console
import std.reflect

/// A handle whose reflection shows one field of its own making: what
/// `reflectDescription` says the type is made of.
pub type Handle = { priv id: Int }

impl reflect.Reflect for Handle {
    fn reflectShow(self, _: Bool): String {
        "Handle #{self.id}"
    }

    fn reflectDescription(): reflect.TypeDescription {
        reflect.TypeDescription.of(reflect.TypeKind.Record) with {
            fields: [reflect.FieldInfo.of("id", 0, some("Its number."), reflect.typeInfo[Int]())]
        }
    }
}

let info = reflect.typeInfo[Handle]()
console.println("{info.fields().map((field) { field.name() })} {info.fields()[0].docs()}")
```

#### TypeDescription.of

```tondo
pub fn TypeDescription.of(kind: TypeKind): TypeDescription
```

A description of a kind with nothing in it yet.

### TypeInfo

```tondo
pub type TypeInfo = {
    // private fields
}
```

A type: its identity, name and capabilities, and what it is made of,
worked out when asked for.

```tondo
import std.console
import std.reflect

/// A person of the system.
pub type User = { name: String, age: Int }

pub fn User.greet(self, greeting: String): String {
    "{greeting}, {self.name}"
}

let info = reflect.typeInfo[User]()
console.println("{info.qualifiedName()}: {info.docs().unwrapOr("")}")
console.println("fields {info.fields().map((field) { field.name() })}, methods {info.methods().map((method) { method.name() })}")
console.println("{reflect.typeOf(User { name: "Ada", age: 36 }).id() == info.id()}")
```

Implements `Display`.

#### TypeInfo.id

```tondo
pub fn TypeInfo.id(self): TypeId
```

The type's identity, the same for the same type everywhere in the
program.

#### TypeInfo.qualifiedName

```tondo
pub fn TypeInfo.qualifiedName(self): String
```

The type's name with its module: `app.models.User`.

#### TypeInfo.kind

```tondo
pub fn TypeInfo.kind(self): TypeKind
```

What kind of type it is.

#### TypeInfo.genericArguments

```tondo
pub fn TypeInfo.genericArguments(self): Array[TypeInfo]
```

The type arguments of a generic type, in order: `[Int]` for
`Array[Int]`.

#### TypeInfo.capabilities

```tondo
pub fn TypeInfo.capabilities(self): Set[TypeCapability]
```

The capabilities the compiler demonstrates of the type.

#### TypeInfo.fields

```tondo
pub fn TypeInfo.fields(self): Array[FieldInfo]
```

A record's fields, in declaration order; none for another type.

#### TypeInfo.variants

```tondo
pub fn TypeInfo.variants(self): Array[VariantInfo]
```

An enum's variants, in declaration order; none for another type.

#### TypeInfo.methods

```tondo
pub fn TypeInfo.methods(self): Array[MethodInfo]
```

The type's methods.

#### TypeInfo.tupleElements

```tondo
pub fn TypeInfo.tupleElements(self): Array[TypeInfo]
```

A tuple's element types, in order; none for another type.

#### TypeInfo.underlying

```tondo
pub fn TypeInfo.underlying(self): TypeInfo?
```

The type a newtype wraps.

#### TypeInfo.annotation

```tondo
pub fn TypeInfo.annotation[A: Reflect](self): A?
```

The type's annotation of type `A` (language 27.13), if it has it.

#### TypeInfo.annotations

```tondo
pub fn TypeInfo.annotations(self): Array[Value]
```

The type's annotations, in the order they are written.

#### TypeInfo.docs

```tondo
pub fn TypeInfo.docs(self): String?
```

The type's `///` documentation.

### FieldInfo

```tondo
pub type FieldInfo = {
    // private fields
}
```

A visible field: its name, type, position among all the fields and
documentation.

```tondo
import std.console
import std.reflect

pub annotation Column = { name: String = "", indexed: Bool = false }

pub type User = {
    /// The name people see.
    @Column(name: "display_name", indexed: true)
    name: String
    @name("years")
    age: Int = 18
}

for field in reflect.typeInfo[User]().fields() {
    let column = field.annotation[Column]().map((column) { column.name }).unwrapOr(field.externalName())
    console.println("{field.ordinal()} {field.name()} -> {column}, default {field.default()}, docs {field.docs()}")
}
```

#### FieldInfo.of

```tondo
pub fn FieldInfo.of(name: String, ordinal: Int, docs: String?, info: TypeInfo): FieldInfo
```

A field's description, as the code the compiler writes for a type
makes it.

#### FieldInfo.annotated

```tondo
pub fn FieldInfo.annotated(self, annotations: Array[Value]): FieldInfo
```

The description with the field's annotations, as the code the compiler
writes for a type gives them.

#### FieldInfo.externallyNamed

```tondo
pub fn FieldInfo.externallyNamed(self, name: String): FieldInfo
```

The description with the field's `@name` (language 9.2), as the code
the compiler writes for a type gives it.

#### FieldInfo.externalName

```tondo
pub fn FieldInfo.externalName(self): String
```

The field's name outside the program, which formats write and read: its
`@name("text")`, or its own name.

#### FieldInfo.defaulted

```tondo
pub fn FieldInfo.defaulted(self, value: Value): FieldInfo
```

The description with the field's default (language 9.2), as the code
the compiler writes for a type gives it.

#### FieldInfo.default

```tondo
pub fn FieldInfo.default(self): Value?
```

The field's default (language 9.2), the value a literal or a decoding
that leaves it out gives it, if it has one.

#### FieldInfo.annotation

```tondo
pub fn FieldInfo.annotation[A: Reflect](self): A?
```

The field's annotation of type `A` (language 27.13), if it has it.

#### FieldInfo.annotations

```tondo
pub fn FieldInfo.annotations(self): Array[Value]
```

The field's annotations, in the order they are written.

#### FieldInfo.name

```tondo
pub fn FieldInfo.name(self): String
```

The field's name.

#### FieldInfo.ordinal

```tondo
pub fn FieldInfo.ordinal(self): Int
```

The field's position among the record's fields, from zero.

#### FieldInfo.docs

```tondo
pub fn FieldInfo.docs(self): String?
```

The field's `///` documentation, if it has some.

#### FieldInfo.typeInfo

```tondo
pub fn FieldInfo.typeInfo(self): TypeInfo
```

The field's type.

### VariantInfo

```tondo
pub type VariantInfo = {
    // private fields
}
```

A variant: its name, position and payload.

```tondo
import std.console
import std.reflect

pub enum Level {
    /// Nothing to do.
    Low
    /// Look at it today.
    @name("urgent")
    High
}

for variant in reflect.typeInfo[Level]().variants() {
    console.println("{variant.ordinal()} {variant.name()} as {variant.externalName()}: {variant.docs().unwrapOr("")}")
}
```

#### VariantInfo.of

```tondo
pub fn VariantInfo.of(
    name: String,
    ordinal: Int,
    payloadKind: VariantPayloadKind,
    tupleElements: Array[TypeInfo],
    fields: Array[FieldInfo],
): VariantInfo
```

A variant's description, as the code the compiler writes for a type
makes it.

#### VariantInfo.externallyNamed

```tondo
pub fn VariantInfo.externallyNamed(self, name: String): VariantInfo
```

The description with the variant's `@name` (language 9.4), as the code
the compiler writes for a type gives it.

#### VariantInfo.externalName

```tondo
pub fn VariantInfo.externalName(self): String
```

The variant's name outside the program, which formats write and read:
its `@name("text")`, or its own name.

#### VariantInfo.annotated

```tondo
pub fn VariantInfo.annotated(self, annotations: Array[Value]): VariantInfo
```

The description with the variant's annotations, as the code the
compiler writes for a type gives them.

#### VariantInfo.documented

```tondo
pub fn VariantInfo.documented(self, docs: String?): VariantInfo
```

The description with the variant's documentation, as the code the
compiler writes makes it.

#### VariantInfo.docs

```tondo
pub fn VariantInfo.docs(self): String?
```

The variant's `///` documentation.

#### VariantInfo.annotation

```tondo
pub fn VariantInfo.annotation[A: Reflect](self): A?
```

The variant's annotation of type `A` (language 27.13), if it has it.

#### VariantInfo.annotations

```tondo
pub fn VariantInfo.annotations(self): Array[Value]
```

The variant's annotations, in the order they are written.

#### VariantInfo.name

```tondo
pub fn VariantInfo.name(self): String
```

The variant's name.

#### VariantInfo.ordinal

```tondo
pub fn VariantInfo.ordinal(self): Int
```

The variant's position among the enum's variants, from zero.

#### VariantInfo.payloadKind

```tondo
pub fn VariantInfo.payloadKind(self): VariantPayloadKind
```

What the variant holds: nothing, a tuple, or fields.

#### VariantInfo.tupleElements

```tondo
pub fn VariantInfo.tupleElements(self): Array[TypeInfo]
```

The types of a tuple payload's elements; none for another payload.

#### VariantInfo.fields

```tondo
pub fn VariantInfo.fields(self): Array[FieldInfo]
```

The fields of a record payload; none for another payload.

### MethodInfo

```tondo
pub type MethodInfo = {
    // private fields
}
```

A `pub` method that reflection can call.

```tondo
import std.console
import std.reflect

pub type Account = { balance: Int }

/// What the account holds.
pub fn Account.total(self): Int {
    self.balance
}

/// Puts money in.
pub fn Account.deposit(mut self, amount: Int) {
    self.balance += amount
}

for method in reflect.typeInfo[Account]().methods() {
    let function = method.function()
    console.println("{method.name()}: {function.parameters().length()} parameters, gives {function.outcome().qualifiedName()}; {method.docs().unwrapOr("")}")
}
```

#### MethodInfo.of

```tondo
pub fn MethodInfo.of(name: String, receiver: ParameterMode, function: FunctionInfo): MethodInfo
```

A method's description, as the code the compiler writes for a type
makes it.

#### MethodInfo.annotated

```tondo
pub fn MethodInfo.annotated(self, annotations: Array[Value]): MethodInfo
```

The description with the method's annotations, as the code the
compiler writes for a type gives them.

#### MethodInfo.documented

```tondo
pub fn MethodInfo.documented(self, docs: String?): MethodInfo
```

The description with the method's documentation, as the code the
compiler writes makes it.

#### MethodInfo.docs

```tondo
pub fn MethodInfo.docs(self): String?
```

The method's `///` documentation.

#### MethodInfo.annotation

```tondo
pub fn MethodInfo.annotation[A: Reflect](self): A?
```

The method's annotation of type `A` (language 27.13), if it has it.

#### MethodInfo.annotations

```tondo
pub fn MethodInfo.annotations(self): Array[Value]
```

The method's annotations, in the order they are written.

#### MethodInfo.name

```tondo
pub fn MethodInfo.name(self): String
```

The method's name.

#### MethodInfo.receiver

```tondo
pub fn MethodInfo.receiver(self): ParameterMode
```

How the method takes `self`.

#### MethodInfo.function

```tondo
pub fn MethodInfo.function(self): FunctionInfo
```

The method's signature.

### ParameterInfo

```tondo
pub type ParameterInfo = {
    // private fields
}
```

A parameter: its position, type and how it is passed.

```tondo
import std.console
import std.reflect

pub annotation Bounds = { min: Int = 0, max: Int = 100 }

pub type Volume = { level: Int }

pub fn Volume.set(mut self, @Bounds(min: 0, max: 11) level: Int, fade: Bool) {
    self.level = if fade { level / 2 } else { level }
}

let method = reflect.typeInfo[Volume]().methods()[0]
for parameter in method.function().parameters() {
    let range = parameter.annotation[Bounds]().map((bounds) { " in {bounds.min}..={bounds.max}" }).unwrapOr("")
    console.println("{parameter.position()} {parameter.name()}: {parameter.typeInfo().qualifiedName()}{range}")
}
```

#### ParameterInfo.of

```tondo
pub fn ParameterInfo.of(
    index: Int,
    info: TypeInfo,
    mode: ParameterMode,
    name: String = "",
): ParameterInfo
```

A parameter's description, as the code the compiler writes for a type
makes it.

#### ParameterInfo.annotated

```tondo
pub fn ParameterInfo.annotated(self, annotations: Array[Value]): ParameterInfo
```

The description with the parameter's annotations, as the code the
compiler writes for a type gives them.

#### ParameterInfo.name

```tondo
pub fn ParameterInfo.name(self): String
```

The parameter's name.

#### ParameterInfo.annotation

```tondo
pub fn ParameterInfo.annotation[A: Reflect](self): A?
```

The parameter's annotation of type `A` (language 27.13), if it has it.

#### ParameterInfo.annotations

```tondo
pub fn ParameterInfo.annotations(self): Array[Value]
```

The parameter's annotations, in the order they are written.

#### ParameterInfo.position

```tondo
pub fn ParameterInfo.position(self): Int
```

The parameter's position, from zero.

#### ParameterInfo.typeInfo

```tondo
pub fn ParameterInfo.typeInfo(self): TypeInfo
```

The parameter's type.

#### ParameterInfo.mode

```tondo
pub fn ParameterInfo.mode(self): ParameterMode
```

How the parameter receives its argument.

### FunctionInfo

```tondo
pub type FunctionInfo = {
    // private fields
}
```

A signature: the parameters after the receiver and the whole outcome.

```tondo
import std.console
import std.reflect

pub type Logger = { prefix: String }

pub fn Logger.log(self, message: String, values: ...Int): String {
    "{self.prefix} {message} {values}"
}

let function = reflect.typeInfo[Logger]().methods()[0].function()
console.println("variadic {function.variadic()}, unsafe {function.isUnsafe()}, gives {function.outcome().qualifiedName()}")
let logged = reflect.valueOf(Logger { prefix: ">" }).call("log", reflect.valueOf("sizes"), reflect.valueOf(1), reflect.valueOf(2))?
console.println("{logged}")
```

#### FunctionInfo.of

```tondo
pub fn FunctionInfo.of(
    parameters: Array[ParameterInfo],
    outcome: TypeInfo,
    variadic: Bool,
    isUnsafe: Bool,
): FunctionInfo
```

A function's description, as the code the compiler writes for a type
makes it.

#### FunctionInfo.parameters

```tondo
pub fn FunctionInfo.parameters(self): Array[ParameterInfo]
```

The function's parameters, in order.

#### FunctionInfo.outcome

```tondo
pub fn FunctionInfo.outcome(self): TypeInfo
```

The type the function returns.

#### FunctionInfo.variadic

```tondo
pub fn FunctionInfo.variadic(self): Bool
```

Whether the function's last parameter takes any number of arguments.

#### FunctionInfo.isUnsafe

```tondo
pub fn FunctionInfo.isUnsafe(self): Bool
```

Whether the function is `unsafe`.

### For other types

```tondo
impl Reflect for Int
impl Reflect for Int8
impl Reflect for Int16
impl Reflect for Int32
impl Reflect for UInt8
impl Reflect for UInt16
impl Reflect for UInt32
impl Reflect for UInt
impl Reflect for Int128
impl Reflect for UInt128
impl Reflect for Float
impl Reflect for Float32
impl Reflect for Bool
impl Reflect for Char
impl Reflect for Byte
impl Reflect for Unit
impl Reflect for String
impl[T: Reflect] Reflect for Array[T]
impl[T: Reflect] Reflect for T?
impl[T: Reflect] Reflect for Ref[T]
impl[K: Reflect + Key, V: Reflect] Reflect for Map[K, V]
impl[T: Reflect, E: Reflect] Reflect for T ! E
impl[T: Reflect] Reflect for Deque[T]
impl[T: Reflect + Key] Reflect for Set[T]
```
