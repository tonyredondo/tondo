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

## Libraries for C

`tondo build --library` compiles a project without `main` into a shared
library, `lib<name>.so` (`.dylib` on macOS), a static one, `lib<name>.a`,
and their header, `<name>.h`. C calls its `pub extern fn`s, which have a
body, by their names:

```tondo library
import std.ffi

/// The words of `text`, separated by spaces.
pub extern fn words_count(text: ffi.CString): ffi.CSize {
    let words = unsafe { ffi.fromCString(text) }.split(" ")
    UInt64(words.length()).unwrapOr(0u64)
}
```

- Their parameters and results are C's types, as for an `extern fn`; a
  Tondo value goes to C as `ffi.context(value)`.
- The build prints how to link each: `cc app.c -L. -lwords ...` or
  `cc app.c libwords.a ...`. A program may also load the shared one with
  `dlopen`.
- Each call runs as a task on the thread that calls; it may wait, and the
  tasks it spawns go on after it returns.
- `tondo check --library` and `tondo test --library` check and test it.
