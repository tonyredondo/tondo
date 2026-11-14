# textkit: a library for C, written in Tondo

A small library of text functions that a C program links and calls: the
slug of a title, the distance between two words, counts, and the words of a
text by how often they appear. It is written in Tondo, with its Unicode and
its collections, and what C sees is four functions and a header that the
build writes. About 140 lines of Tondo, and 35 of C that use them.

```text
textkit/
  tondo.toml
  src/
    textkit.to                    what C sees: the `pub extern fn`s
    textkit_test.to               those functions, called the way C calls them
    text/text.to                  the work, in ordinary Tondo
    text/text_test.to             its tests, with two properties to fuzz
  demo/
    main.c                        a C program that uses the library
    build.sh                      builds the library, then the program, and runs it
```

## Run it

```sh
cd examples/textkit
tondo test --library
sh demo/build.sh                  # needs a C compiler
```

```text
$ sh demo/build.sh
wrote demo/out/libtextkit.so, demo/out/libtextkit.a and demo/out/textkit.h
slug of "Crème Brûlée, 2 ways!": creme-brulee-2-ways (19 bytes)
in 8 bytes it is cut: creme-b
from "kitten" to "sitting": 3 changes
85 characters, 18 words, 2 lines; said more than once:
  to         3
  be         2
  the        2
that is 3 words
```

`tondo build --library` writes a shared library, a static one, and the
header, with the documentation of each function as its comment:

```c
typedef struct TextkitStats TextkitStats;

/** What a text holds. An `extern type` is laid out as C lays a struct out,
 * so it crosses to C by value.
 */
struct TextkitStats {
    long characters;
    long words;
    long lines;
};

/** How many characters must be added, removed or changed to make `right`
 * of `left`.
 */
long textkit_distance(const char *left, const char *right);

/** The characters, words and lines of `source`. */
TextkitStats textkit_stats(const char *source);
```

## What to look at

**A function for C is a function with a body.** In
[`textkit.to`](src/textkit.to), `pub extern fn textkit_distance(...)` is a
function that C calls by that name. Its parameters are C's types, from
`std.ffi`, and its `///` documentation becomes the comment of the header. A
project with such functions and no `main` is a library.

**Unsafe is a thin layer.** Each exported function does three things: it
turns C's pointers into Tondo's values, calls the module `text`, and turns
the result back. Only the first and the last are in `unsafe { ... }`:
reading a C string, and writing through a pointer. The work itself, in
[`text.to`](src/text/text.to), is ordinary Tondo that never sees a pointer,
and all of the language is there for it.

**C's conventions, kept.** `textkit_slug` writes into a buffer the caller
owns, never past its capacity, always ends it with a zero, and gives the
length of the whole result so the caller can tell that it was cut, as
`snprintf` does. `textkit_stats` gives a struct by value: `TextkitStats`
is an `extern type`, laid out as C lays it out, and the header declares it.
`textkit_words` takes a function and a `void *` to hand back to it, the way
C libraries pass state to a callback.

**What Tondo brings.** The slug of `Crème Brûlée` is `creme-brulee` because
`text.normalize` decomposes each letter and `isMark` drops the accents, in
any script. The distance counts characters, whatever bytes they take. Both
would be a library of their own in C.

**Properties, searched.** [`text_test.to`](src/text/text_test.to) states
two: the slug of a slug is itself, and the distance is the same both ways
and never more than the longer text. `tondo test --library` tries them on
the seeds, and `tondo test --library --fuzz` searches for an input that
breaks them: about a million in fifteen seconds here, and none found.

**The boundary is tested from Tondo.**
[`textkit_test.to`](src/textkit_test.to) calls the exported functions as C
would, with memory from `ffi.allocate` and strings from `ffi.toCString`,
and checks what they wrote: the ending zero, the cut, that nothing is
written when there is no room. The callback there is an `unsafe fn` that
reads its state back with `ffi.fromContext`.

**The runtime comes along.** The library carries Tondo's runtime, which
starts at the first call. An exported function may use tasks, channels and
the standard library, and C may call it from any thread. The header also
declares `textkit_stop()`, which ends that runtime, for a program that
unloads the library or wants its tasks and threads gone before it goes on.

## Going further

- In another Tondo project, `tondo bind <path>/textkit.h --library textkit`
  writes a module of `extern` declarations to call the library from there,
  as it does for any C header.
- The static library, `libtextkit.a`, links into a C program with nothing
  to install beside it; the build prints the line that does it.
- Anything that calls C can use it: Python's `ctypes`, Go's `cgo`, Rust's
  `extern "C"`, the FFI of Node or Ruby.

A Tondo program loads the shared library as it runs, with `ffi.Library`, as
it would a plugin. From this directory, after `tondo build --library
--output demo/out`:

```tondo
import std.console
import std.ffi

fn main(): !ffi.LibraryError {
    let textkit = ffi.Library.open("demo/out/{ffi.libraryFileName("textkit")}")?
    let distance =
        unsafe {
            textkit.function[extern fn(ffi.CString, ffi.CString): ffi.CLong]("textkit_distance")?
        }
    console.println("{unsafe { distance("kitten", "sitting") }}")
}
```

```text
3
```
