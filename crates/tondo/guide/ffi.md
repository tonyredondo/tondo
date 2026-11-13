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
- `extern fn chdir(path: ffi.CString): ffi.CInt ! os.Errno` fails with the
  system's error number when it gives -1, or null for a pointer: the call
  gives `err(os.Errno(2))`, shown `os error 2: No such file or directory`.

## Loading a library as the program runs

`ffi.Library` loads a shared library, a plugin, while the program runs, and
gives its functions as `ffi.Function[F]`, called as the `extern fn` `F`:

```tondo
import std.console
import std.ffi

fn main(): !ffi.LibraryError {
    let program = ffi.Library.program()
    let length = unsafe { program.function[extern fn(ffi.CString): ffi.CSize]("strlen")? }
    console.println("{unsafe { length("tondo") }}")
}
```

- `Library.open("plugins/libkit.so")` loads a file, from the current
  directory unless the path is absolute, and never searches;
  `Library.named("z")` searches as the system does for `libz.so`
  (`ffi.libraryFileName`).
- A function keeps its library loaded. A library of `open` stays loaded until
  the program ends; one of `unsafe { Library.openUnloadable(...) }` is
  unloaded when nothing holds it, and `unsafe { library.close() }` unloads
  any now.
- `function[F]` takes an `extern fn` type, and `has(name)` asks whether the
  library publishes a name. A `--static` or `--target` build cannot load
  libraries, and says so.

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
    UInt64(words.length()) ?? 0u64
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
