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

- [Functions](#functions): [`compress`](#compress), [`decompress`](#decompress), [`writer`](#writer), [`reader`](#reader)

### Functions

#### compress

```tondo
pub fn compress(data: Array[Byte]): Array[Byte]
```

The data compressed as `zstd -1`, the fastest of its levels.

#### decompress

```tondo
pub fn decompress(data: Array[Byte]): Array[Byte] ! io.IoError
```

The data of one Zstandard frame or several after one another;
`IoError.InvalidData` when it is not Zstandard, is cut short or its
checksum does not match.

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
