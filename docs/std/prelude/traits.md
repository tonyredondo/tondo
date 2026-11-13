# The prelude's traits

`Ordered` gives `<` and its kin, `Additive` gives `+` and `-`, `Number` adds `*` and `/`, and `Integer` is every integer of every width.

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api Ordered`, `tondo api Additive`, `tondo api Number`, `tondo api Integer`, `tondo api Fractional` show the same in a terminal.

- [Ordered](#ordered)
- [Additive](#additive)
- [Number](#number)
- [Integer](#integer)
- [Fractional](#fractional)

### Ordered

```tondo
pub trait Ordered {
    fn compare(self, other: Self): Int
}
```

A type whose values are in order: `compare` is negative, zero or
positive as `self` goes before, with or after `other`.

```tondo
import std.console

/// A version, in order by its parts.
pub type Version = { major: Int, minor: Int }

impl Ordered for Version {
    fn compare(self, other: Version): Int {
        if self.major != other.major { self.major.compare(other.major) } else { self.minor.compare(other.minor) }
    }
}

let versions = [Version { major: 1, minor: 10 }, Version { major: 1, minor: 2 }, Version { major: 0, minor: 9 }]
let newest = versions.sortedBy((version) { version }).last().expect("versions")
console.println("{newest.major}.{newest.minor} {Version { major: 1, minor: 2 } < Version { major: 1, minor: 10 }}")
```

### Additive

```tondo
pub trait Additive {
    fn zero(): Self

    fn plus(self, other: Self): Self

    fn minus(self, other: Self): Self
}
```

A type whose values add and subtract, from a zero: `a + b`, `a - b`
and `values.sum()` (language 18.10). Every number is one, and so is a
duration; a program's type, as an amount of money, may be one too.

```tondo
import std.console

/// An amount of money, in cents, which adds as numbers do.
pub type Money = { cents: Int }

impl Additive for Money {
    fn zero(): Money {
        Money { cents: 0 }
    }

    fn plus(self, other: Money): Money {
        Money { cents: self.cents + other.cents }
    }

    fn minus(self, other: Money): Money {
        Money { cents: self.cents - other.cents }
    }
}

let prices = [Money { cents: 250 }, Money { cents: 199 }]
let total = prices.sum() - Money { cents: 49 }
console.println("{total.cents / 100}.{"{total.cents % 100}".padStart(2, "0")}")
```

### Number

```tondo
pub trait Number: Additive {
    fn one(): Self

    fn fromInt(value: Int): Self

    fn times(self, other: Self): Self

    fn dividedBy(self, other: Self): Self
}
```

A type whose values also multiply and divide, and that integer literals
write: `a * b`, `a / b` and `x * 2`. Every number is one; a program's
type, as a decimal or a fraction, may be one too.

```tondo
import std.console

/// A fraction, which integer literals write and arithmetic keeps exact.
pub type Fraction = { top: Int, bottom: Int }

impl Additive for Fraction {
    fn zero(): Fraction {
        Fraction { top: 0, bottom: 1 }
    }

    fn plus(self, other: Fraction): Fraction {
        Fraction { top: self.top * other.bottom + other.top * self.bottom, bottom: self.bottom * other.bottom }
    }

    fn minus(self, other: Fraction): Fraction {
        Fraction { top: self.top * other.bottom - other.top * self.bottom, bottom: self.bottom * other.bottom }
    }
}

impl Number for Fraction {
    fn one(): Fraction {
        Fraction { top: 1, bottom: 1 }
    }

    fn fromInt(value: Int): Fraction {
        Fraction { top: value, bottom: 1 }
    }

    fn times(self, other: Fraction): Fraction {
        Fraction { top: self.top * other.top, bottom: self.bottom * other.bottom }
    }

    fn dividedBy(self, other: Fraction): Fraction {
        Fraction { top: self.top * other.bottom, bottom: self.bottom * other.top }
    }
}

let third = Fraction { top: 1, bottom: 3 }
// The literals become fractions through `fromInt`.
let result = third * 2 + 1
console.println("{result.top}/{result.bottom}")
```

### Integer

```tondo
pub trait Integer: Number + Ordered + Display {

}
```

The language's integers, of every width: on a `T: Integer`, generic code
has everything a concrete integer has. A program does not implement it.

```tondo
import std.console

/// Works for an integer of any width, with what every integer has.
fn digits[T: Integer](value: T): Int {
    var rest = value / 10
    var count = 1
    for rest != 0 {
        rest = rest / 10
        count += 1
    }
    count
}

console.println("{digits(12345)} {digits(255u8)} {digits(-7i16)}")
```

### Fractional

```tondo
pub trait Fractional: Number {

}
```

A number whose division keeps the fraction: `1.0 / 2.0` is `0.5`, where
an `Integer` gives 0. `Float`, `Float32` and `math.Decimal` are, and so
may a program's number be, as an exact fraction. What divides to give a
part of a whole, as a mean, asks for it.

```tondo
import std.console

/// The mean of the values, NaN for a float with none.
fn mean[T: Fractional](values: Array[T]): T {
    values.sum().dividedBy(Number.fromInt[T](values.length()))
}

console.println("{mean([1.0, 2.0, 4.0])} {mean([0.5f32, 1.5f32])}")
```
