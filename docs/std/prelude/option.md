# Option

`T?` is `Option[T]`: `some(value)` or `none`.

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api Option` shows the same in a terminal.

- [Option](#option): [`Option.isSome`](#optionissome), [`Option.isNone`](#optionisnone), [`Option.map`](#optionmap), [`Option.andThen`](#optionandthen), [`Option.unwrapOr`](#optionunwrapor), [`Option.orElse`](#optionorelse), [`Option.filter`](#optionfilter), [`Option.okOr`](#optionokor), [`Option.expect`](#optionexpect), [`Option.context`](#optioncontext)

### Option

#### Option.isSome

```tondo
pub fn Option[T].isSome(self): Bool
```

Whether there is a value.

#### Option.isNone

```tondo
pub fn Option[T].isNone(self): Bool
```

Whether there is no value.

#### Option.map

```tondo
pub fn Option[T].map[U](self, transform: fn(T): U): U?
```

The function's result for the value, or `none` when there is none.

#### Option.andThen

```tondo
pub fn Option[T].andThen[U](self, next: fn(T): U?): U?
```

The option the function gives for the value, or `none`.

#### Option.unwrapOr

```tondo
pub fn Option[T].unwrapOr(self, fallback: T): T
```

The value, or `fallback` when there is none.

#### Option.orElse

```tondo
pub fn Option[T].orElse(self, other: T?): T?
```

This option if it has a value, or `other`.

#### Option.filter

```tondo
pub fn Option[T].filter(self, keep: fn(T): Bool): T?
```

The value when it passes the test, or `none`.

#### Option.okOr

```tondo
pub fn Option[T].okOr[E](self, error: E): T ! E
```

The value, or the error when there is none.

#### Option.expect

```tondo
pub fn Option[T].expect(self, message: String): T
```

The value; a `none` panics with the message.

#### Option.context

```tondo
pub fn Option[T].context(self, message: String): T ! Failure
```

The value, or a `Failure` that says what is missing:
`users.get(id).context("no user {id}")?`.
