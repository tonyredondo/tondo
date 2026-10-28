# Metaprogramming

## Automatic implementations

The compiler implements `serialization.Encode` and `serialization.Decode`
for every record, enum and newtype whose parts have them, when the program
uses a format (`std.json`, `std.http`) or `std.serialization`, and
`reflect.Reflect` when it uses `std.reflect`. Nothing to write: the same
type reads and writes every format.

## derive

A package that declares a trait can name a provider in its `tondo.toml`,
a Tondo program that writes the `impl` at compile time:

```text
// in a program
import show
pub type Point = { x: Int, y: Int }
derive show.Show for Point

// in the package `show`: tondo.toml
[derive]
Show = "derive/show.to"
```

`tondo expand` prints what each `derive` wrote. Providers and generators
run in a sandbox: no files, network, clock or randomness.

## Generators

`[generators.name]` in `tondo.toml` runs a Tondo program before the build,
with input files and the public declarations of chosen types, and adds the
modules it writes:

```text
[generators.colors]
program = "gen/colors.to"
inputs = ["colors.txt"]
outputs = ["palette"]          # the module app.palette
roots = ["models.User"]
```

## Reflection

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
    let value = reflect.valueOf(user)            // a value of any type
    for field in value.fields() {
        console.println("{field.name} = {field.value}")
    }
    let back: User = value.to()?
    console.println(back.name)
}
```

`tondo api std.reflect` shows paths, method calls and type descriptions.

## Annotations

A module declares an annotation as a record whose fields have defaults,
and writes it with the capital of its type on a type, field, variant,
function, method or parameter. Readers get it with its type:

```tondo
import std.console
import std.reflect

/// What a value must be.
pub annotation Check = { required: Bool = false, min: Int? = none }

pub type Signup = {
    @Check(required: true, min: 3)
    name: String
    email: String
}

fn main() {
    for field in reflect.typeInfo[Signup]().fields() {
        match field.annotation[Check]() {
            some(check) => console.println("{field.name()}: {check.required} {check.min}")
            none => console.println("{field.name()}: unchecked")
        }
    }
}
```

From another module it is `@binding.Check(min: 3)`. Its arguments are
constants, by position or by name; a field left out takes its default, and
an option takes its value without `some`. An annotation goes once on each
thing: what repeats is an array inside it. `@name` and `@ignore` stay
lowercase: they are the language's.

A `derive` provider or a generator reads them too, before the program is
checked: `field.annotations` in `std.meta`, and `annotation.to()` decodes
one into a record the provider declares with the fields it reads. A
generator's root can be a public function, with its `parameters()`,
`result()` and `annotations()`: that is how a library writes a router.
