# Calling C

`extern fn` declares a C function; calls go inside `unsafe`. `std.ffi` names
C's types.

```tondo
import std.console
import std.ffi

extern fn abs(value: ffi.CInt): ffi.CInt

fn main() {
    let result = unsafe { abs(-42) }
    console.println("{result}")
}
```

- `extern type Point = { x: ffi.CInt, y: ffi.CInt }` is a C struct by value.
- A Tondo function can be a C callback; C can call it from any thread.
- `[native] libraries = ["sqlite3"]` in `tondo.toml`, or `--link`, links a
  library. `tondo bind header.h` writes the `extern` declarations of a
  header for you.
- `Pointer[T]` and raw memory exist only inside `unsafe`.
