# std.cpu.arm64

functions that are one instruction of an arm64 processor each, for the files of arm64 (`name_arm64.to`) or a package limited to it. Each says the feature of `cpu.Feature` it needs. A function that calls one needs that feature too, unless the build's level has it, and so does any function that calls that one outside an `if cpu.has(...)` that tests the feature: the compiler works it out, and compiles each function for what it needs.

```tondo
import std.console
import std.cpu
import std.cpu.arm64

fn checksum(crc: UInt32, value: UInt): UInt32 {
    arm64.crc32cInt(crc, value)
}

fn main() {
    let crc = if cpu.has(cpu.Feature.Crc32) { checksum(0xFFFFFFFF, 0) } else { 1943489909 }
    console.println("{crc}")
}
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.cpu.arm64` shows the same in a terminal.

- [Functions](#functions): [`crc32cByte`](#crc32cbyte), [`crc32cInt`](#crc32cint), [`aesEncrypt`](#aesencrypt), [`aesDecrypt`](#aesdecrypt), [`aesMixColumns`](#aesmixcolumns), [`aesInverseMixColumns`](#aesinversemixcolumns), [`polynomialMultiply`](#polynomialmultiply), [`pause`](#pause)

### Functions

#### crc32cByte

```tondo
pub fn crc32cByte(crc: UInt32, value: Byte): UInt32
```

`crc32cb`: `crc` updated with `value` by the CRC-32C of the Castagnoli
polynomial, without inverting it before or after. Needs `Crc32`.

#### crc32cInt

```tondo
pub fn crc32cInt(crc: UInt32, value: UInt): UInt32
```

`crc32cx`: `crc` updated with the eight bytes of `value`, the lowest
first. Needs `Crc32`.

#### aesEncrypt

```tondo
pub fn aesEncrypt(state: simd.Vector128[Byte], key: simd.Vector128[Byte]): simd.Vector128[Byte]
```

`aese`: AES's first steps of a round of encryption: `key` added, then
the bytes substituted and the rows shifted. Needs `Aes`.

#### aesDecrypt

```tondo
pub fn aesDecrypt(state: simd.Vector128[Byte], key: simd.Vector128[Byte]): simd.Vector128[Byte]
```

`aesd`: AES's first steps of a round of decryption: `key` added, then
the inverse substitution and shift. Needs `Aes`.

#### aesMixColumns

```tondo
pub fn aesMixColumns(state: simd.Vector128[Byte]): simd.Vector128[Byte]
```

`aesmc`: AES's mix of the columns of `state`. Needs `Aes`.

#### aesInverseMixColumns

```tondo
pub fn aesInverseMixColumns(state: simd.Vector128[Byte]): simd.Vector128[Byte]
```

`aesimc`: AES's inverse mix of the columns of `state`. Needs `Aes`.

#### polynomialMultiply

```tondo
pub fn polynomialMultiply(a: UInt, b: UInt): simd.Vector128[UInt]
```

`pmull` of 64 bits: the product without carries of `a` and `b`, as 128
bits, the low half first. Needs `Pmull`.

#### pause

```tondo
pub fn pause()
```

`isb`: tells the processor that the code waits in a loop for another
thread, as Rust's `spin_loop` does on arm64. Needs nothing.
