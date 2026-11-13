# std.os.linux

the calls of Linux, their constants and their types, as the C headers of the standard library's 14.45 declare them, with C's names, for the build's architecture; and `syscall`, the kernel's call by its number. Calling them is `unsafe`, and those that fail with the system's error number give `os.Errno`. `tondo api std.os.linux.<name>` shows one of C's declarations, and `tondo api --search <words>` finds them.

```tondo
import std.console
import std.os.linux

fn main() {
    match unsafe { linux.syscall(linux.SYS_getpid) } {
        ok(process) => console.println("process {process}")
        err(error) => console.println("getpid: {error}")
    }
}
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.os.linux` shows the same in a terminal.

- [Functions](#functions): [`syscall`](#syscall)

### Functions

#### syscall

```tondo
pub unsafe fn syscall(number: Int, arguments: ...Int): Int ! os.Errno
```

The kernel's call of `number`, one of `linux.SYS_...`, with up to six
arguments; more is a panic. It fails with the kernel's error number.
