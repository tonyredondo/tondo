# std.cpu.x64

functions that are one instruction of an x64 processor each, for the files of x64 (`name_x64.to`) or a package limited to it. Each says the feature of `cpu.Feature` it needs. A function that calls one needs that feature too, unless the build's level has it, and so does any function that calls that one outside an `if cpu.has(...)` that tests the feature: the compiler works it out, and compiles each function for what it needs.

```tondo
import std.console
import std.cpu
import std.cpu.x64

fn spread(value: UInt, mask: UInt): UInt {
    x64.depositBits(value, mask)
}

fn main() {
    let spreadOut = if cpu.has(cpu.Feature.Bmi2) { spread(0b101, 0xF0) } else { 0b1010000 }
    console.println("{spreadOut}")
}
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.cpu.x64` shows the same in a terminal.

- [Functions](#functions): [`crc32cByte`](#crc32cbyte), [`crc32cInt`](#crc32cint), [`depositBits`](#depositbits), [`extractBits`](#extractbits), [`aesEncrypt`](#aesencrypt), [`aesEncryptLast`](#aesencryptlast), [`aesDecrypt`](#aesdecrypt), [`aesDecryptLast`](#aesdecryptlast), [`carrylessMultiply`](#carrylessmultiply), [`pause`](#pause)

### Functions

#### crc32cByte

```tondo
pub fn crc32cByte(crc: UInt32, value: Byte): UInt32
```

`crc32` on a byte: `crc` updated with `value` by the CRC-32C of the
Castagnoli polynomial, without inverting it before or after. Needs
`Sse42`, which the baseline has.

#### crc32cInt

```tondo
pub fn crc32cInt(crc: UInt32, value: UInt): UInt32
```

`crc32` on 64 bits: `crc` updated with the eight bytes of `value`, the
lowest first. Needs `Sse42`, which the baseline has.

#### depositBits

```tondo
pub fn depositBits(value: UInt, mask: UInt): UInt
```

`pdep`: the lowest bits of `value`, one after another, put where `mask`
has a bit set. Needs `Bmi2`.

#### extractBits

```tondo
pub fn extractBits(value: UInt, mask: UInt): UInt
```

`pext`: the bits of `value` where `mask` has a bit set, gathered at the
lowest. Needs `Bmi2`.

#### aesEncrypt

```tondo
pub fn aesEncrypt(state: simd.Vector128[Byte], key: simd.Vector128[Byte]): simd.Vector128[Byte]
```

`aesenc`: a round of AES encryption of `state` with the round key `key`.
Needs `Aes`.

#### aesEncryptLast

```tondo
pub fn aesEncryptLast(state: simd.Vector128[Byte], key: simd.Vector128[Byte]): simd.Vector128[Byte]
```

`aesenclast`: the last round of AES encryption, which mixes no columns.
Needs `Aes`.

#### aesDecrypt

```tondo
pub fn aesDecrypt(state: simd.Vector128[Byte], key: simd.Vector128[Byte]): simd.Vector128[Byte]
```

`aesdec`: a round of AES decryption of `state` with the round key `key`,
of the equivalent inverse cipher. Needs `Aes`.

#### aesDecryptLast

```tondo
pub fn aesDecryptLast(state: simd.Vector128[Byte], key: simd.Vector128[Byte]): simd.Vector128[Byte]
```

`aesdeclast`: the last round of AES decryption. Needs `Aes`.

#### carrylessMultiply

```tondo
pub fn carrylessMultiply(
    a: simd.Vector128[UInt],
    b: simd.Vector128[UInt],
    halves: Int,
): simd.Vector128[UInt]
```

`pclmulqdq`: the product without carries of a half of `a` and a half of
`b`, as 128 bits. `halves` is a constant that says which: 0 the low halves
of both, 1 the high half of `a` and the low half of `b`, 16 the low half
of `a` and the high half of `b`, 17 the high halves of both. Needs
`Pclmulqdq`.

#### pause

```tondo
pub fn pause()
```

`pause`: tells the processor that the code waits in a loop for another
thread, which saves power and lets that thread run. Needs nothing.
