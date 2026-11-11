# std.validate

the rules of a type, written on its fields, which no value read from outside breaks: the code the compiler writes for `Decode` checks them, and `check` checks a value the program makes. The rules themselves are in `std/rules.to`.

```tondo
import std.console
import std.json
import std.validate

pub type Signup = {
    @validate.Length(min: 3, max: 20)
    name: String
    @validate.Email
    email: String
}

match json.decode[Signup](r"""{"name": "al", "email": "x"}""") {
    ok(signup) => console.println("welcome {signup.name}")
    err(error) => console.println("{error.violations}")
}
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.validate` shows the same in a terminal.

- [Functions](#functions): [`check`](#check), [`checkAs`](#checkas)
- [Length](#length)
- [Min](#min)
- [Max](#max)
- [Matches](#matches)
- [Email](#email)
- [Violation](#violation)
- [Rules](#rules)
- [Invalid](#invalid)

### Functions

#### check

```tondo
pub fn check[T: reflect.Reflect](value: T): !Invalid
```

Whether a value the program made keeps the rules of its type, and those
of the types of its fields, as one read from outside does.

#### checkAs

```tondo
pub fn checkAs(value: reflect.Value, info: reflect.TypeInfo): !Invalid
```

Whether a value of reflection keeps the rules of the type that `info`
describes, for code that reads values by reflection before it makes
them, as `std.mcp` does with the arguments of a tool.

### Length

```tondo
pub annotation Length = {
    min: Int = 0
    max: Int? = none
}
```

How many characters a `String` has, or elements an `Array`, a `Map` or a
`Set`: from `min` to `max`.

Implements `reflect.Reflect`.

### Min

```tondo
pub annotation Min = {
    value: Float
}
```

The smallest a number may be, in the type of its field:
`@validate.Min(18)` of an `Int`, `@validate.Min(0.5)` of a `Float`.

Implements `reflect.Reflect`.

### Max

```tondo
pub annotation Max = {
    value: Float
}
```

The largest a number may be, in the type of its field.

Implements `reflect.Reflect`.

### Matches

```tondo
pub annotation Matches = {
    pattern: String
}
```

A pattern of `std.regex` that all of the text matches.

Implements `reflect.Reflect`.

### Email

```tondo
pub annotation Email = {}
```

An email address, as a form of HTML accepts one.

Implements `reflect.Reflect`.

### Violation

```tondo
pub alias Violation = serialization.Violation
```

A rule a value breaks: the path of its field and what the rule asks.

```tondo
import std.console
import std.validate

pub type Account = {
    @validate.Min(18)
    age: Int
}

match validate.check(Account { age: 12 }) {
    ok(_) => console.println("valid")
    err(invalid) => {
        for violation in invalid.violations {
            console.println("{violation.path} {violation.message}")
        }
    }
}
```

### Rules

```tondo
pub trait Rules {
    fn violations(self): Array[Violation]
}
```

Rules of a type of its own, which its fields' annotations cannot say:
between its fields, on each element of a list, or any other. The code
the compiler writes for `Decode` checks them after the fields' rules,
and so does `check`. A path names a field from the value: `"end"`.

```tondo
import std.console
import std.validate

pub type Booking = { start: Int, end: Int }

impl validate.Rules for Booking {
    fn violations(self): Array[validate.Violation] {
        if self.end < self.start {
            return [validate.Violation { path: "end", message: "must not be before start" }]
        }
        []
    }
}

match validate.check(Booking { start: 5, end: 1 }) {
    ok(_) => console.println("valid")
    err(invalid) => console.println("{invalid}")
}
```

### Invalid

```tondo
pub type Invalid = {
    violations: Array[Violation]
}
```

The rules a value breaks.

```tondo
import std.console
import std.validate

pub type Signup = {
    @validate.Length(min: 3)
    name: String
    @validate.Email
    email: String
}

match validate.check(Signup { name: "al", email: "nowhere" }) {
    ok(_) => console.println("valid")
    err(invalid) => console.println("{invalid.violations.length()} rules broken: {invalid}")
}
```

Implements `Display`.
