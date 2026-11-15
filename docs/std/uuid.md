# std.uuid

RFC 9562 identifiers, sixteen bytes that display as `8-4-4-4-12` lowercase hexadecimal.

```tondo
import std.console
import std.uuid

console.println("{uuid.Uuid.v4()} {uuid.Uuid.v7()}")
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.uuid` shows the same in a terminal.

- [Functions](#functions): [`Uuid.Nil`](#uuidnil), [`Uuid.NamespaceDns`](#uuidnamespacedns), [`Uuid.NamespaceUrl`](#uuidnamespaceurl), [`Uuid.NamespaceOid`](#uuidnamespaceoid), [`Uuid.NamespaceX500`](#uuidnamespacex500)
- [Uuid](#uuid): [`Uuid.v4`](#uuidv4), [`Uuid.v5`](#uuidv5), [`Uuid.v7`](#uuidv7), [`Uuid.parse`](#uuidparse), [`Uuid.fromBytes`](#uuidfrombytes), [`Uuid.bytes`](#uuidbytes), [`Uuid.version`](#uuidversion), [`Uuid.compare`](#uuidcompare)
- [UuidError](#uuiderror)

### Functions

#### Uuid.Nil

```tondo
pub const Uuid.Nil: Uuid = Uuid { hex: "00000000000000000000000000000000" }
```

The UUID whose bits are all zero.

#### Uuid.NamespaceDns

```tondo
pub const Uuid.NamespaceDns: Uuid = Uuid { hex: "6ba7b8109dad11d180b400c04fd430c8" }
```

The namespace of domain names, for name-based UUIDs (RFC 9562, `v5`).

#### Uuid.NamespaceUrl

```tondo
pub const Uuid.NamespaceUrl: Uuid = Uuid { hex: "6ba7b8119dad11d180b400c04fd430c8" }
```

The namespace of URLs, for name-based UUIDs (RFC 9562).

#### Uuid.NamespaceOid

```tondo
pub const Uuid.NamespaceOid: Uuid = Uuid { hex: "6ba7b8129dad11d180b400c04fd430c8" }
```

The namespace of ISO object identifiers, for name-based UUIDs (RFC 9562).

#### Uuid.NamespaceX500

```tondo
pub const Uuid.NamespaceX500: Uuid = Uuid { hex: "6ba7b8149dad11d180b400c04fd430c8" }
```

The namespace of X.500 names, for name-based UUIDs (RFC 9562).

### Uuid

```tondo
pub type Uuid = {
    // private fields
}
```

A UUID. Equal, ordered and usable as a key by its sixteen bytes, as
unsigned numbers in network order.

```tondo
import std.console
import std.uuid

let id = uuid.Uuid.v5(uuid.Uuid.NamespaceDns, "example.com")
let again = uuid.Uuid.parse("{id}")?
console.println("{id} {again == id} {uuid.Uuid.Nil}")
```

Implements `Ordered`, `Display`, `serialization.Encode`, `serialization.Decode`, `reflect.Reflect`.

#### Uuid.v4

```tondo
pub fn Uuid.v4(): Uuid
```

A random UUID (version 4): 122 bits from the system's entropy.

#### Uuid.v5

```tondo
pub fn Uuid.v5(namespace: Uuid, name: String): Uuid
```

The UUID of a name in a namespace (version 5, on SHA-1): the same pair
always gives the same UUID.

#### Uuid.v7

```tondo
pub fn Uuid.v7(): Uuid
```

A UUID that sorts by when it was made (version 7): the Unix time in
milliseconds, then 74 bits from the system's entropy.

#### Uuid.parse

```tondo
pub fn Uuid.parse(input: String): Uuid ! UuidError
```

A UUID written `8-4-4-4-12` in hexadecimal of either case, with or
without `urn:uuid:` before it.

#### Uuid.fromBytes

```tondo
pub fn Uuid.fromBytes(bytes: Array[Byte]): Uuid ! UuidError
```

The UUID of sixteen bytes; other lengths are `UuidError.WrongLength`.

#### Uuid.bytes

```tondo
pub fn Uuid.bytes(self): Array[Byte]
```

The sixteen bytes, in network order.

#### Uuid.version

```tondo
pub fn Uuid.version(self): Int
```

The version its bits say: 4, 5 or 7 for the ones this module makes.

#### Uuid.compare

```tondo
pub fn Uuid.compare(self, other: Uuid): Int
```

Negative, zero or positive as `self` goes before, with or after `other`.

### UuidError

```tondo
pub enum UuidError {
    /// A text other than `8-4-4-4-12` hexadecimal, after an optional
    /// `urn:uuid:`.
    InvalidText
    /// Other than sixteen bytes.
    WrongLength
}
```

Why a UUID could not be read: a text that is not one, or other than
sixteen bytes.

```tondo
import std.console
import std.uuid

match uuid.Uuid.fromBytes([1, 2, 3]) {
    ok(id) => console.println("{id}")
    err(uuid.UuidError.WrongLength) => console.println("a UUID has sixteen bytes")
    err(uuid.UuidError.InvalidText) => console.println("not 8-4-4-4-12")
}
```

Implements `Display`, `reflect.Reflect`.
