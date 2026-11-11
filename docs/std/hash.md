# std.hash

digests, HMAC, both whole or in parts, and hashes that are not for security.

```tondo
import std.console
import std.hash

console.println("{hash.sha256("hello".bytes())}")
console.println("{hash.hmacSha256("key".bytes(), "message".bytes())}")
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.hash` shows the same in a terminal.

- [Functions](#functions): [`md5`](#md5), [`sha1`](#sha1), [`sha256`](#sha256), [`sha384`](#sha384), [`sha512`](#sha512), [`hmacSha256`](#hmacsha256), [`hmacSha512`](#hmacsha512), [`hasher`](#hasher), [`hmac`](#hmac), [`fnv32`](#fnv32), [`fnv32a`](#fnv32a), [`fnv64`](#fnv64), [`fnv64a`](#fnv64a), [`xxhash64`](#xxhash64), [`murmur3`](#murmur3)
- [Digest](#digest): [`Digest.bytes`](#digestbytes), [`Digest.hex`](#digesthex)
- [Algorithm](#algorithm)
- [Hasher](#hasher-1): [`Hasher.update`](#hasherupdate), [`Hasher.finish`](#hasherfinish)

### Functions

#### md5

```tondo
pub fn md5(data: Array[Byte]): Digest
```

MD5, for formats and protocols that name it, such as `Content-MD5`. It is
broken for security: use SHA-256 there.

#### sha1

```tondo
pub fn sha1(data: Array[Byte]): Digest
```

SHA-1, for formats that name it, such as UUIDv5 and Git. It is broken
for security: use SHA-256 there.

#### sha256

```tondo
pub fn sha256(data: Array[Byte]): Digest
```

The SHA-256 digest of the data.

#### sha384

```tondo
pub fn sha384(data: Array[Byte]): Digest
```

The SHA-384 digest of the data.

#### sha512

```tondo
pub fn sha512(data: Array[Byte]): Digest
```

The SHA-512 digest of the data.

#### hmacSha256

```tondo
pub fn hmacSha256(key: Array[Byte], data: Array[Byte]): Digest
```

The HMAC of the data with the key (RFC 2104), on SHA-256.

#### hmacSha512

```tondo
pub fn hmacSha512(key: Array[Byte], data: Array[Byte]): Digest
```

The HMAC of the data with the key (RFC 2104), on SHA-512.

#### hasher

```tondo
pub fn hasher(algorithm: Algorithm): Hasher
```

A hasher of the digest of `algorithm`.

#### hmac

```tondo
pub fn hmac(algorithm: Algorithm, key: Array[Byte]): Hasher
```

A hasher of the HMAC with the key (RFC 2104), on `algorithm`.

#### fnv32

```tondo
pub fn fnv32(data: Array[Byte]): UInt32
```

FNV-1 of 32 bits.

#### fnv32a

```tondo
pub fn fnv32a(data: Array[Byte]): UInt32
```

FNV-1a of 32 bits.

#### fnv64

```tondo
pub fn fnv64(data: Array[Byte]): UInt
```

FNV-1 of 64 bits.

#### fnv64a

```tondo
pub fn fnv64a(data: Array[Byte]): UInt
```

FNV-1a of 64 bits.

#### xxhash64

```tondo
pub fn xxhash64(data: Array[Byte], seed: UInt = 0): UInt
```

XXH64, the 64-bit xxHash, with a seed.

#### murmur3

```tondo
pub fn murmur3(data: Array[Byte], seed: UInt32 = 0): UInt128
```

MurmurHash3 x64 of 128 bits, with a seed: its first half in the high 64
bits.

### Digest

```tondo
pub type Digest = {
    // private fields
}
```

The digest of some data. It displays as lowercase hexadecimal.

```tondo
import std.console
import std.hash

let digest = hash.sha256("hello".bytes())
console.println("{digest.hex()} {digest.bytes().length()} bytes")
```

Implements `Display`.

#### Digest.bytes

```tondo
pub fn Digest.bytes(self): Array[Byte]
```

The digest's bytes.

#### Digest.hex

```tondo
pub fn Digest.hex(self): String
```

The digest as lowercase hexadecimal, two digits per byte.

### Algorithm

```tondo
pub enum Algorithm {
    Md5
    Sha1
    Sha256
    Sha384
    Sha512
}
```

The digests a `Hasher` computes.

```tondo
import std.console
import std.hash

let signer = hash.hmac(hash.Algorithm.Sha512, "key".bytes())
signer.update("message".bytes())
console.println("{signer.finish()}")
```

### Hasher

```tondo
pub type Hasher = {
    // private fields
}
```

A digest or an HMAC computed in parts, without the data together: each
part to `update`, then `finish`. Its copies share the computation.

```tondo
import std.console
import std.hash

// A digest of data that comes in parts.
let hasher = hash.hasher(hash.Algorithm.Sha256)
for part in ["hel", "lo"] {
    hasher.update(part.bytes())
}
console.println("{hasher.finish() == hash.sha256("hello".bytes())}")
```

#### Hasher.update

```tondo
pub fn Hasher.update(self, data: Array[Byte])
```

Gives the hasher the next part of the data.

#### Hasher.finish

```tondo
pub fn Hasher.finish(self): Digest
```

The digest of the parts so far: the same as the function that takes
them together gives. The hasher can take more after.
