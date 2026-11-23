# std.zstd

data compressed in the Zstandard format of RFC 8878, whole or as streams of std.io, as `zstd -1` does.

```tondo
import std.console
import std.zstd

let packed = zstd.compress("hello ".repeat(100).bytes())
let unpacked = zstd.decompress(packed)?
console.println("{packed.length()} bytes for {unpacked.length()}")
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.zstd` shows the same in a terminal.

- [Functions](#functions): [`compress`](#compress), [`decompress`](#decompress), [`ZstdLimits.Default`](#zstdlimitsdefault), [`writer`](#writer), [`reader`](#reader)
- [ZstdLimits](#zstdlimits)
- [ZstdError](#zstderror)

### Functions

#### compress

```tondo
pub fn compress(data: Array[Byte]): Array[Byte]
```

The data compressed as `zstd -1`, the fastest of its levels.

#### decompress

```tondo
pub fn decompress(
    data: Array[Byte],
    limits: ZstdLimits = ZstdLimits.Default,
): Array[Byte] ! ZstdError
```

The data of one Zstandard frame or several after one another;
`ZstdError.InvalidData` when it is not Zstandard, is cut short or its
checksum does not match, and `ZstdError.TooLarge` once it decompresses
to more than `limits.maxOutputBytes`, which `zstd.reader` does not
bound: it gives the data a part at a time.

#### ZstdLimits.Default

```tondo
pub const ZstdLimits.Default: ZstdLimits = ZstdLimits { maxOutputBytes: 128 * 1024 * 1024 }
```

Up to 128 MiB of data decompressed.

#### writer

```tondo
pub fn writer(to: io.Writer): io.Writer
```

A stream that compresses what it is given into `to`: `flush()` writes
what is compressed so far, and `close()` writes the end of the format and
lets go of `to`, which closes with its last copy. So a writer over
`file.writer()` closes the file when it closes, and one over
`console.stdout()` leaves the console open.

#### reader

```tondo
pub fn reader(from: io.Reader): io.Reader
```

A stream of what `from` holds, decompressed as it is read; closing it
lets go of `from`, which closes with its last copy. Data that is not
Zstandard fails as `IoError.InvalidData` in the read that reaches it.

### ZstdLimits

```tondo
pub type ZstdLimits = {
    maxOutputBytes: Int
}
```

How much `decompress` may make of data from outside the program, which a
few kilobytes can make gigabytes of: `maxOutputBytes` of the data
decompressed.

```tondo
import std.console
import std.zstd

let packed = zstd.compress("a".repeat(1000).bytes())
match zstd.decompress(packed, limits: zstd.ZstdLimits.Default with { maxOutputBytes: 100 }) {
    ok(data) => console.println("{data.length()} bytes")
    err(error) => console.println("{error}")
}
```

### ZstdError

```tondo
pub enum ZstdError {
    /// Not Zstandard, cut short, or with a checksum that does not match.
    InvalidData
    /// More data decompressed than its `limits` allow, the bound it holds:
    /// `ZstdLimits.maxOutputBytes`.
    TooLarge(Int)
}
```

Why `decompress` gave no data.

```tondo
import std.console
import std.zstd

match zstd.decompress("not Zstandard".bytes()) {
    ok(_) => console.println("decompressed")
    err(zstd.ZstdError.InvalidData) => console.println("not Zstandard")
    err(error) => console.println("{error}")
}
```

Implements `reflect.Reflect`, `Display`.
