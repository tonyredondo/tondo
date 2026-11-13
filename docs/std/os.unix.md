# std.os.unix

what Linux and macOS have alike. Descriptors and mappings that close as their values go, and the calls on them, each made again when a signal interrupts it; and the constants, types and functions of C that both systems have with one shape, with C's names, for the build's target. `tondo api std.os.unix.<name>` shows one of C's declarations, and `tondo api --search <words>` finds them.

```tondo
import std.console
import std.os.unix

fn main() {
    match unix.pipe() {
        ok((reading, writing)) => {
            _ = unix.write(ref writing, "ping".bytes())
            var into: Array[Byte] = [0, 0, 0, 0, 0, 0, 0, 0]
            let count = unix.read(ref reading, mut into) ?? 0
            console.println("{count} bytes: {into.take(count)}")
        }
        err(error) => console.println("pipe: {error}")
    }
}
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.os.unix` shows the same in a terminal.

- [Functions](#functions): [`open`](#open), [`read`](#read), [`write`](#write), [`seek`](#seek), [`pipe`](#pipe), [`duplicate`](#duplicate), [`control`](#control), [`deviceControl`](#devicecontrol), [`isTerminal`](#isterminal), [`terminalMode`](#terminalmode), [`setTerminalMode`](#setterminalmode), [`poll`](#poll), [`signal`](#signal)
- [Fd](#fd): [`Fd.adopt`](#fdadopt), [`Fd.number`](#fdnumber), [`Fd.standardInput`](#fdstandardinput), [`Fd.standardOutput`](#fdstandardoutput), [`Fd.standardError`](#fdstandarderror), [`Fd.close`](#fdclose)
- [Mapping](#mapping): [`Mapping.of`](#mappingof), [`Mapping.anonymous`](#mappinganonymous), [`Mapping.pointer`](#mappingpointer), [`Mapping.length`](#mappinglength), [`Mapping.close`](#mappingclose)

### Functions

#### open

```tondo
pub fn open(path: String, flags: Int32, mode: Int32 = 0o644): Fd ! os.Errno
```

The file at `path`, opened with `flags`, `unix.O_RDONLY` or others
combined with `|`, and created with `mode` when `unix.O_CREAT` is one of
them.

#### read

```tondo
pub fn read(from: ref Fd, into: mut Array[Byte]): Int ! os.Errno
```

Reads at most `into.length()` bytes into `into`, from its start, and
gives how many: 0 at the end of what there is to read.

#### write

```tondo
pub fn write(to: ref Fd, bytes: Array[Byte]): Int ! os.Errno
```

Writes `bytes` and gives how many it wrote, which may be fewer.

#### seek

```tondo
pub fn seek(on: ref Fd, offset: Int, from: Int32): Int ! os.Errno
```

Moves where the next read or write is, `offset` bytes from where `from`
says, `unix.SEEK_SET`, `unix.SEEK_CUR` or `unix.SEEK_END`, and gives it.

#### pipe

```tondo
pub fn pipe(): (Fd, Fd) ! os.Errno
```

A pipe: what is written to the second is read from the first.

#### duplicate

```tondo
pub fn duplicate(of: ref Fd): Fd ! os.Errno
```

Another descriptor of what `of` is a descriptor of.

#### control

```tondo
pub fn control(on: ref Fd, command: Int32, argument: Int = 0): Int32 ! os.Errno
```

C's `fcntl`: `command`, `unix.F_GETFL` or another, with `argument`.

#### deviceControl

```tondo
pub unsafe fn deviceControl(
    on: ref Fd,
    request: UInt,
    argument: Pointer[ffi.Void],
): Int32 ! os.Errno
```

C's `ioctl`: `request`, with `argument`, which it reads or writes as the
request says.

#### isTerminal

```tondo
pub fn isTerminal(on: ref Fd): Bool
```

Whether it is a terminal.

#### terminalMode

```tondo
pub fn terminalMode(of: ref Fd): termios ! os.Errno
```

The mode of the terminal it is.

#### setTerminalMode

```tondo
pub fn setTerminalMode(of: ref Fd, mode: termios, when: Int32): !os.Errno
```

Sets the mode of the terminal it is, `when` `unix.TCSANOW` or another
says.

#### poll

```tondo
pub fn poll(descriptors: mut Array[pollfd], timeout: time.Duration? = none): Int ! os.Errno
```

Waits until one of `descriptors` is ready as its `events` ask, or for
`timeout` in all, and gives how many are: each one's `revents` says
how.

#### signal

```tondo
pub fn signal(pid: Int32, number: Int32): !os.Errno
```

Sends the signal `number`, `unix.SIGTERM` or another, to the process
`pid`.

### Fd

```tondo
pub type Fd = {
    // private fields
}
```

An open descriptor of the system. It is closed when its value goes, but
for the three standard ones, which are never closed; `close` closes it
before, with the error the system gives.

```tondo
import std.console
import std.os.unix

let output = unix.Fd.standardOutput()
console.println("{output.number()}")
```

#### Fd.adopt

```tondo
pub unsafe fn Fd.adopt(number: Int32): Fd
```

The descriptor `number`, which the program got elsewhere: the `Fd` will
close it, so nothing else may.

#### Fd.number

```tondo
pub fn Fd.number(self): Int32
```

Its number, or -1 once it is closed.

#### Fd.standardInput

```tondo
pub fn Fd.standardInput(): Fd
```

The standard input, 0, which is never closed.

#### Fd.standardOutput

```tondo
pub fn Fd.standardOutput(): Fd
```

The standard output, 1, which is never closed.

#### Fd.standardError

```tondo
pub fn Fd.standardError(): Fd
```

The standard error, 2, which is never closed.

#### Fd.close

```tondo
pub fn Fd.close(self): !os.Errno
```

Closes it now, with the error the system gives; it also closes when its
value goes. A standard one is let go without being closed.

### Mapping

```tondo
pub type Mapping = {
    // private fields
}
```

Memory of the system mapped into the program. It is unmapped when its
value goes; `close` unmaps it before, with the error the system gives.

```tondo
import std.console
import std.os.unix

match unsafe { unix.Mapping.anonymous(4096, unix.PROT_READ | unix.PROT_WRITE, unix.MAP_PRIVATE) } {
    ok(mapping) => console.println("{mapping.length()} bytes")
    err(error) => console.println("mmap: {error}")
}
```

#### Mapping.of

```tondo
pub unsafe fn Mapping.of(
    file: ref Fd,
    length: Int,
    protection: Int32,
    flags: Int32,
    offset: Int = 0,
): Mapping ! os.Errno
```

`length` bytes of `file`, from `offset`, mapped with `protection`,
`unix.PROT_READ` and others, and `flags`, `unix.MAP_SHARED` or
`unix.MAP_PRIVATE` with others.

#### Mapping.anonymous

```tondo
pub unsafe fn Mapping.anonymous(length: Int, protection: Int32, flags: Int32): Mapping ! os.Errno
```

`length` bytes of memory of no file, mapped with `protection` and
`flags`, to which it adds `unix.MAP_ANON`.

#### Mapping.pointer

```tondo
pub fn Mapping.pointer(self): Pointer[Byte]
```

Where its memory starts.

#### Mapping.length

```tondo
pub fn Mapping.length(self): Int
```

How many bytes it maps.

#### Mapping.close

```tondo
pub fn Mapping.close(self): !os.Errno
```

Unmaps it now, with the error the system gives; it is also unmapped when
its value goes.
