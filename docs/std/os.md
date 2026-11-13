# std.os

What is of each system. This module, for every system, holds the error number of the system, which a C function declared `T ! os.Errno` fails with (language 16.16).

```tondo
import std.console
import std.ffi
import std.os

extern fn chdir(path: ffi.CString): ffi.CInt ! os.Errno

match unsafe { chdir("/no/such/directory") } {
    ok(_) => console.println("changed")
    err(error) => console.println("{error.value == 2}")
}
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.os` shows the same in a terminal.

- [Errno](#errno)

### Errno

```tondo
pub type Errno = Int32
```

An error number of the system, as C's `errno` holds it: its `value` is
the number. It shows as the system writes it, `os error 2: No such file
or directory`.

```tondo
import std.console
import std.os

console.println("{os.Errno(2)}")
```

Implements `Display`, `reflect.Reflect`.
