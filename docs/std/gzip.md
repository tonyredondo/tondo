# std.gzip

data compressed in the gzip format of RFC 1952, whole or as streams of std.io.

```tondo
import std.console
import std.gzip

let packed = gzip.compress("hello ".repeat(100).bytes())
let unpacked = gzip.decompress(packed)?
console.println("{packed.length()} bytes for {unpacked.length()}")
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.gzip` shows the same in a terminal.

- [Functions](#functions): [`compress`](#compress), [`decompress`](#decompress), [`GzipLimits.Default`](#gziplimitsdefault), [`writer`](#writer), [`reader`](#reader)
- [GzipLimits](#gziplimits)
- [GzipError](#gziperror)

### Functions

#### compress

```tondo
pub fn compress(data: Array[Byte], level: Int = 6): Array[Byte]
```

The data compressed at `level`, from 0, which does not compress, to 9,
the smallest and slowest; 6 is the one of the `gzip` tool and of Go.

#### decompress

```tondo
pub fn decompress(
    data: Array[Byte],
    limits: GzipLimits = GzipLimits.Default,
): Array[Byte] ! GzipError
```

The data of one gzip member or several after one another;
`GzipError.InvalidData` when it is not gzip, is cut short or its
checksum does not match, and `GzipError.TooLarge` once it decompresses
to more than `limits.maxOutputBytes`, which `gzip.reader` does not
bound: it gives the data a part at a time.

#### GzipLimits.Default

```tondo
pub const GzipLimits.Default: GzipLimits = GzipLimits { maxOutputBytes: 128 * 1024 * 1024 }
```

Up to 128 MiB of data decompressed.

#### writer

```tondo
pub fn writer(to: io.Writer, level: Int = 6): io.Writer
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
lets go of `from`, which closes with its last copy. Data that is
not gzip fails as `IoError.InvalidData` in the read that reaches it.

### GzipLimits

```tondo
pub type GzipLimits = {
    maxOutputBytes: Int
}
```

How much `decompress` may make of data from outside the program, which a
few kilobytes can make gigabytes of: `maxOutputBytes` of the data
decompressed.

```tondo
import std.console
import std.gzip

let packed = gzip.compress("a".repeat(1000).bytes())
match gzip.decompress(packed, limits: gzip.GzipLimits.Default with { maxOutputBytes: 100 }) {
    ok(data) => console.println("{data.length()} bytes")
    err(error) => console.println("{error}")
}
```

### GzipError

```tondo
pub enum GzipError {
    /// Not gzip, cut short, or with a checksum that does not match.
    InvalidData
    /// More data decompressed than its `limits` allow, the bound it holds:
    /// `GzipLimits.maxOutputBytes`.
    TooLarge(Int)
}
```

Why `decompress` gave no data.

```tondo
import std.console
import std.gzip

match gzip.decompress("not gzip".bytes()) {
    ok(_) => console.println("decompressed")
    err(gzip.GzipError.InvalidData) => console.println("not gzip")
    err(error) => console.println("{error}")
}
```

Implements `reflect.Reflect`, `Display`.
