# Metaprogramming

## Automatic implementations

The compiler implements `json.Encode` and `json.Decode` for every record,
enum and newtype whose parts have them, when the program uses `std.json`
(or `std.http`), and `reflect.Reflect` when it uses `std.reflect`. Nothing
to write.

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
