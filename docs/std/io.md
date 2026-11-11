# std.io

the streams shared by the console, files, processes and memory. Streams are shared handles: copies use the same stream, each operation is atomic, and the last copy closes it.

```tondo
import std.console
import std.io

let (writer, reader) = io.pipe()
writer.writeLine("through a pipe")?
writer.close()?
console.println(reader.readText()?)
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.io` shows the same in a terminal.

- [Functions](#functions): [`reader`](#reader), [`discard`](#discard), [`pipe`](#pipe), [`copy`](#copy), [`multiWriter`](#multiwriter)
- [IoError](#ioerror)
- [Writer](#writer): [`Writer.write`](#writerwrite), [`Writer.writeLine`](#writerwriteline), [`Writer.writeBytes`](#writerwritebytes), [`Writer.flush`](#writerflush), [`Writer.close`](#writerclose)
- [Reader](#reader-1): [`Reader.readLine`](#readerreadline), [`Reader.read`](#readerread), [`Reader.readText`](#readerreadtext), [`Reader.readBytes`](#readerreadbytes), [`Reader.close`](#readerclose)
- [Buffer](#buffer): [`Buffer`](#buffer-1), [`Buffer.writer`](#bufferwriter), [`Buffer.reader`](#bufferreader), [`Buffer.text`](#buffertext), [`Buffer.bytes`](#bufferbytes), [`Buffer.length`](#bufferlength), [`Buffer.clear`](#bufferclear)

### Functions

#### reader

```tondo
pub fn reader(text: String): Reader
```

A reader of a fixed text.

#### discard

```tondo
pub fn discard(): Writer
```

A writer that keeps nothing.

#### pipe

```tondo
pub fn pipe(capacity: Int = 65536): (Writer, Reader)
```

A writer and a reader joined: what a task writes to one, another reads
from the other, in order and as soon as it is written. A write waits
while `capacity` bytes are unread; a read waits for bytes, and ends once
the writer is closed. Once the reader is closed, writes fail as `Closed`.

#### copy

```tondo
pub fn copy(from: Reader, to: Writer): Int ! IoError
```

Writes to `to` what `from` holds, to its end, and gives how many bytes
it copied; it closes neither.

#### multiWriter

```tondo
pub fn multiWriter(writers: ...Writer): Writer
```

A writer that writes to every one of `writers`, in their order: a write
fails with the first of their errors. `flush` flushes them all, and
`close` lets go of them, which close with their last copies.

### IoError

```tondo
pub enum IoError {
    Closed
    InvalidData
    Host
}
```

Why a stream failed: it is closed, its data is not UTF-8 where text was
wanted, or the host failed.

```tondo
import std.console
import std.io

let (writer, reader) = io.pipe()
reader.close()?
match writer.writeLine("nobody reads") {
    ok(_) => console.println("written")
    err(io.IoError.Closed) => console.println("the reader is gone")
    err(error) => console.println("{error}")
}
```

### Writer

```tondo
pub type Writer
```

Where data goes.

```tondo
import std.console
import std.io

fn greet(out: io.Writer): !io.IoError {
    out.writeLine("hello")?
}

let buffer = io.Buffer()
greet(buffer.writer())?
console.print(buffer.text()?)
```

#### Writer.write

```tondo
pub fn Writer.write(self, text: String): Unit ! IoError
```

Writes the text.

#### Writer.writeLine

```tondo
pub fn Writer.writeLine(self, text: String): Unit ! IoError
```

Writes the text and one LF.

#### Writer.writeBytes

```tondo
pub fn Writer.writeBytes(self, data: Array[Byte]): Unit ! IoError
```

Writes the bytes.

#### Writer.flush

```tondo
pub fn Writer.flush(self): Unit ! IoError
```

Hands what the stream keeps to where it goes.

#### Writer.close

```tondo
pub fn Writer.close(self): Unit ! IoError
```

Closes the stream now, with the error of its last writes if they fail;
it also closes when its last copy goes, losing that error. A stream that
writes into another, as `gzip.writer(to)`, lets go of `to`, which closes
with its last copy: at once if the writer had the only one, and not while
another is kept, as `console.stdout()`.

### Reader

```tondo
pub type Reader
```

Where data comes from.

```tondo
import std.console
import std.io

let input = io.reader("first\nsecond\n")
for {
    match input.readLine()? {
        some(line) => console.println("> {line}")
        none => break
    }
}
```

#### Reader.readLine

```tondo
pub fn Reader.readLine(self): String? ! IoError
```

The next line without its `\n` or `\r\n`, or `none` at the end.

#### Reader.read

```tondo
pub fn Reader.read(self, max: Int): Array[Byte]? ! IoError
```

Up to `max` bytes, or `none` at the end.

#### Reader.readText

```tondo
pub fn Reader.readText(self): String ! IoError
```

The rest, as UTF-8 text.

#### Reader.readBytes

```tondo
pub fn Reader.readBytes(self): Array[Byte] ! IoError
```

The rest, as bytes.

#### Reader.close

```tondo
pub fn Reader.close(self): Unit ! IoError
```

Closes the stream now; it also closes when its last copy goes. A stream
that reads another, as `gzip.reader(from)`, lets go of `from`, which
closes with its last copy.

### Buffer

```tondo
pub type Buffer
```

Shared memory a writer appends to and a reader reads.

```tondo
import std.console
import std.io

let buffer = io.Buffer()
buffer.writer().write("in memory")?
console.println("{buffer.length()} bytes: {buffer.text()?}")
```

#### Buffer

```tondo
pub fn Buffer(): Buffer
```

An empty buffer in memory.

#### Buffer.writer

```tondo
pub fn Buffer.writer(self): Writer
```

A writer that appends to the buffer.

#### Buffer.reader

```tondo
pub fn Buffer.reader(self): Reader
```

Reads what the buffer holds at that moment.

#### Buffer.text

```tondo
pub fn Buffer.text(self): String ! IoError
```

What the buffer holds, as UTF-8 text.

#### Buffer.bytes

```tondo
pub fn Buffer.bytes(self): Array[Byte]
```

What the buffer holds.

#### Buffer.length

```tondo
pub fn Buffer.length(self): Int
```

The number of bytes the buffer holds.

#### Buffer.clear

```tondo
pub fn Buffer.clear(self)
```

Empties the buffer.
