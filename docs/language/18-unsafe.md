# Unsafe and C

Everything so far has been safe: no code in the earlier chapters can read
memory it does not own, use something after it is freed, or race with
another task. `unsafe` is the door out, for the two things that need it:
calling C, and working with raw memory. This chapter covers both, and
building a Tondo library that C calls.

- [What unsafe means](#what-unsafe-means)
- [Calling a C function](#calling-a-c-function)
- [C's types](#cs-types)
- [Strings](#strings)
- [Structs](#structs)
- [Pointers and raw memory](#pointers-and-raw-memory)
- [Callbacks](#callbacks)
- [Interfaces of C](#interfaces-of-c)
- [The system's calls](#the-systems-calls)
- [Linking a library](#linking-a-library)
- [A library loaded as the program runs](#a-library-loaded-as-the-program-runs)
- [Plugins with a contract](#plugins-with-a-contract)
- [Bindings written for you](#bindings-written-for-you)
- [A library for C](#a-library-for-c)
- [C and tasks](#c-and-tasks)
- [Keeping unsafe small](#keeping-unsafe-small)

## What unsafe means

Safe code cannot go wrong in certain ways because the compiler checks every
step. Inside `unsafe { ... }` a few operations are allowed that it cannot
check:

- calling an `extern fn`, a function of C;
- calling a function declared `unsafe fn`;
- reading and writing through a `Pointer[T]`.

Nothing else changes. The types are still checked, an integer still panics
on overflow, and the rules of the earlier chapters still hold for
everything that is not one of those three. `unsafe` does not turn checks
off: it marks the places where the programmer, not the compiler, vouches
that memory is used correctly.

A program that never writes `unsafe` and has no dependency that does has
none of the bugs that come from misused memory. One that does has them only
where the word appears, which is where to look.

## Calling a C function

`extern fn` declares a function of C by its name and its types. Calling it
is unsafe:

```tondo
import std.console
import std.ffi

extern fn abs(value: ffi.CInt): ffi.CInt

extern fn strlen(text: ffi.CString): ffi.CSize

fn main() {
    let magnitude = unsafe { abs(-42) }
    let length = unsafe { strlen("hello") }
    console.println("{magnitude} {length}")
}
```

```text output
42 5
```

The functions of the system's C library are there to call with nothing to
link. A Tondo string goes where a `ffi.CString` is expected, for the length
of the call.

An `unsafe { ... }` block is an expression, as any block is. It should hold
the call and as little else as it can.

Many of C's functions fail by giving -1, or a null pointer, and leaving the
reason in `errno`. One declared with `! os.Errno` fails as a Tondo function
does, with the system's error number, which the call reads before anything
else can change it:

```tondo
import std.console
import std.ffi
import std.os

extern fn chdir(path: ffi.CString): ffi.CInt ! os.Errno

fn main() {
    match unsafe { chdir("/no/such/place") } {
        ok(_) => console.println("moved")
        err(error) => console.println("{error} ({error.value})")
    }
}
```

```text output
os error 2: No such file or directory (2)
```

The result is a signed integer or a pointer. A function that says it failed
in another way is declared without `! os.Errno` and checked by hand.

## C's types

`std.ffi` names C's types, so that a declaration reads like the header it
comes from:

| C | Tondo |
| --- | --- |
| `char` | `ffi.CChar`, a `Byte` |
| `short`, `unsigned short` | `ffi.CShort`, `ffi.CUShort` |
| `int`, `unsigned int` | `ffi.CInt`, `ffi.CUInt` |
| `long`, `unsigned long` | `ffi.CLong`, `ffi.CULong` |
| `long long` | `ffi.CLongLong` |
| `size_t`, `ssize_t` | `ffi.CSize`, `ffi.CSSize` |
| `float`, `double` | `ffi.CFloat`, `ffi.CDouble` |
| `bool` | `ffi.CBool` |
| `const char *` | `ffi.CString` |
| `void *` | `Pointer[ffi.Void]` |
| `T *` | `Pointer[T]`, or `Pointer[T]?` when it may be null |

These are aliases of Tondo's own numbers, `ffi.CInt` is `Int32`, so
arithmetic and conversion work as they always do. A narrower result widens
where an `Int` is expected, and an `Int` going where C wants an `int` is
converted on purpose, `Int32(n)?`, since it may not fit.

A null pointer is not a hidden possibility of every pointer. A function that
may return null says so with `?`, and the result is an option:

```tondo
import std.console
import std.ffi

extern fn getenv(name: ffi.CString): ffi.CString?

fn main() {
    let home = unsafe { getenv("HOME") }
    let missing = unsafe { getenv("TONDO_SURELY_UNSET_VARIABLE") }
    console.println("{home.isSome()} {missing.isNone()}")
}
```

```text output
true true
```

A C function with a variable number of arguments is declared with `...`,
as `extern fn printf(format: ffi.CString, ...): ffi.CInt`, and
`@symbol("name")` before a declaration gives it a Tondo name different from
C's.

## Strings

A C string is bytes ending in a zero, with nobody saying who owns them. A
Tondo `String` is a value that knows its length. Crossing between them is
explicit:

```tondo
import std.console
import std.ffi

extern fn toupper(character: ffi.CInt): ffi.CInt

fn main() {
    unsafe {
        // To C: a copy that C may keep, which is ours to free.
        let copy = ffi.toCString("abc")
        // From C: a Tondo string, copied out of C's memory.
        let back = ffi.fromCString(copy)
        ffi.free(copy)
        console.println("{back} {Char.from(Int(toupper(97)))}")
    }
}
```

```text output
abc some(A)
```

- A `String` passed straight to a `ffi.CString` parameter is lent for the
  call. If C keeps the pointer after returning, give it `ffi.toCString(text)`
  instead, and free it when C is done.
- `ffi.fromCString(pointer)` copies, so the result is safe to keep whatever
  C does with its own memory afterwards.
- A Tondo string may contain a zero byte, which C would read as the end;
  passing one to C panics rather than cut the text short.

Many functions of C take a buffer and its size, and write there what fits.
`ffi.writeCString` writes a text that way, as `snprintf` does: as much as
fits with its zero, never half a character, and it gives the bytes of the
whole text, so that a result not under the capacity says it was cut.

```tondo
import std.console
import std.ffi

fn main() {
    let buffer = ffi.allocate[ffi.CChar](16)
    unsafe {
        defer ffi.free(buffer)
        for capacity in [16, 11, 1] {
            let whole = ffi.writeCString("crème brûlée", to: buffer, capacity: capacity)
            console.println("{capacity}: {whole} `{ffi.fromCString(buffer)}`")
        }
    }
}
```

```text output
16: 15 `crème brûlée`
11: 15 `crème br`
1: 15 ``
```

Eleven bytes have room for ten and the zero, and the tenth would be half of
`û`, so nine are written. A capacity of zero writes nothing and still gives
the length, which is how a caller asks how much room to bring.

## Structs

`extern type` declares a struct laid out as C lays it out. It is passed by
value or through a pointer, as the C function takes it:

```tondo
import std.console
import std.ffi

extern type Division = {
    quotient: ffi.CInt
    remainder: ffi.CInt
}

extern fn div(numerator: ffi.CInt, denominator: ffi.CInt): Division

fn main() {
    let result = unsafe { div(17, 5) }
    console.println("{result.quotient} remainder {result.remainder}")
}
```

```text output
3 remainder 2
```

An ordinary Tondo record has no promised layout, which lets the compiler
arrange it as it likes. An `extern type` gives that up to match C: its
fields are in order, with C's alignment. Unions, bit fields and arrays of a
fixed size have forms too, which [`tondo bind`](#bindings-written-for-you)
writes.

## Pointers and raw memory

`Pointer[T]` is an address. It can be made, copied and passed in safe code,
since holding an address harms nothing; reading and writing through it is
unsafe.

```tondo
import std.console
import std.ffi

fn main() {
    unsafe {
        let numbers = ffi.allocate[Int](4)         // room for four, not yet anything
        for index in 0..4 {
            numbers.offset(index).write(index * index)
        }
        var total = 0
        for index in 0..4 {
            total += numbers.offset(index).read()
        }
        console.println("{total} {ffi.sizeOf[Int]()} {ffi.alignOf[Int]()}")
        ffi.free(numbers)
    }
}
```

```text output
14 8 8
```

| Operation | Does |
| --- | --- |
| `ffi.allocate[T](count)` | room for `count` values, which the program frees |
| `ffi.free(pointer)` | gives it back |
| `pointer.read()`, `pointer.write(value)` | through the address |
| `pointer.offset(n)` | the address `n` values on |
| `pointer.cast[U]()` | the same address, as another type |
| `ffi.copy(from, to, count)` | copies values |
| `ffi.bytes(pointer, count)` | a Tondo array of what is there |
| `ffi.sizeOf[T]()`, `ffi.alignOf[T]()` | what C's `sizeof` and `alignof` give |

Everything a pointer can get wrong is the programmer's to get right: that
the memory is still allocated, that it holds a value of that type, that the
offset is inside it, that it is freed once. That is what `unsafe` says. A
pointer is neither sent to another task nor shared between tasks by the
compiler's leave, for the same reason.

Raw memory is for talking to C and for the rare data structure the language
cannot express. It is not a faster way to do what an `Array` does: an array
of numbers is the same memory, with its bounds proved away where the
compiler can.

## Callbacks

A Tondo function can be given to C to call. Its type is `extern fn(...)`,
and a function declared `unsafe fn` with C's types fits:

```tondo
import std.console
import std.ffi

extern fn qsort(
    base: Pointer[ffi.Void],
    count: ffi.CSize,
    size: ffi.CSize,
    compare: extern fn(Pointer[ffi.Void], Pointer[ffi.Void]): ffi.CInt,
)

unsafe fn descending(left: Pointer[ffi.Void], right: Pointer[ffi.Void]): ffi.CInt {
    let (a, b) = (left.cast[Int]().read(), right.cast[Int]().read())
    if a > b { -1 } else if a < b { 1 } else { 0 }
}

fn main() {
    let input = [5, 2, 8, 1, 9, 3]
    unsafe {
        let values = ffi.allocate[Int](input.length())
        for (index, value) in input.enumerate() {
            values.offset(index).write(value)
        }
        qsort(values.cast[ffi.Void](), 6u64, 8u64, descending)
        var sorted: Array[Int] = []
        for index in 0..input.length() {
            sorted.push(values.offset(index).read())
        }
        ffi.free(values)
        console.println("{sorted}")
    }
}
```

```text output
[9, 8, 5, 3, 2, 1]
```

C may call the function from any thread, and inside it the function is
ordinary Tondo: it may wait, spawn tasks and use channels. Called from a
thread that C made, it runs on that thread, as a task of its own, and
returns to C when it ends; if it waits, the thread waits with it. State goes to a
callback the way C libraries expect, through a `void *`: `ffi.context(value)`
makes one of any Tondo value, and `ffi.fromContext[T](pointer)` reads it
back in the callback.

## Interfaces of C

An object of COM, or of .NET's profiling API, starts with a pointer to a
table of functions, each of which takes the object first; a header of C++
writes one as a class of pure virtual methods. In Tondo such an interface
is a trait with `@ffi.Interface`. Its methods are the entries of the table,
in the order they are declared, after those of the interface it requires.

```tondo
import std.console
import std.ffi
import std.sync

/// A counter, as a library's header declares it.
@ffi.Interface(id: "6A1B2C3D-0000-4000-8000-00000000C0DE")
pub trait Counter: ffi.Unknown {
    fn add(self, by: Int32): !ffi.HResult
    fn total(self): Int ! ffi.HResult
}

/// A Tondo type that is one.
type Tally = { sum: sync.Atomic[Int] }

impl Counter for Tally {
    fn add(self, by: Int32): !ffi.HResult {
        if by < 0 {
            fail ffi.HResult.InvalidArgument
        }
        _ = self.sum.add(Int(by))
    }

    fn total(self): Int ! ffi.HResult {
        self.sum.load()
    }
}

/// What a library does with an object: all it has of it is a pointer.
fn count(object: Pointer[ffi.Void]): Int ! ffi.HResult {
    let counter = unsafe { CounterObject.of(object) }
    counter.add(40)?
    counter.add(2)?
    counter.total()
}

fn main() {
    let object = ffi.object(Tally { sum: sync.Atomic(0) })
    match count(object) {
        ok(total) => console.println("counted {total}")
        err(code) => console.println("failed: {code}")
    }
    let counter = unsafe { CounterObject.of(object) }
    match counter.add(-1) {
        ok(_) => console.println("added")
        err(code) => console.println("refused: {code}")
    }
    console.println("{CounterObject.Id}")
    console.println("held {counter.addRef()}, then {counter.release()} and {counter.release()}")
}
```

```text output
counted 42
refused: 0x80070057: The parameter is incorrect
6a1b2c3d-0000-4000-8000-00000000c0de
held 2, then 1 and 0
```

The program has both sides in one process, so that it runs without a
library: `ffi.object` gives C an object made of a Tondo value, and `count`
does what the library would, calling it through its table.

**An object C made.** The trait's module gets a type with the trait's name
and `Object`, here `CounterObject`, which holds the pointer and implements
the trait by calling the entries of the table. `CounterObject.of(pointer)`
makes one, and is `unsafe` because nothing checks that the object is of that
interface. `pointer()` gives the pointer back, `CounterObject.Id` is the
interface's identity, an `ffi.Guid`, and `unknown()` is the same object as
the interface this one requires.

**What a method gives.** C's function returns an `HRESULT` in the first
three, and how the method is declared says what the call makes of it:

| Declared | The call gives |
| --- | --- |
| `: ffi.HResult` | the number as it is, so that `S_FALSE`, `HResult.False`, is told from `S_OK` |
| `: !ffi.HResult` | nothing, or the failure when the number is negative |
| `: T ! ffi.HResult` | the `T`, which C writes through a last parameter that the declaration leaves out, or the failure |
| a type that crosses to C, or none | what C returns |

`ffi.HResult` has the common results as its constants, `HResult.Ok`,
`HResult.NoInterface`, `HResult.InvalidArgument`
([Constants of a type](03-functions.md#constants-of-a-type)), and a library
declares its own as constants of its module, `pub const BufferTooSmall:
ffi.HResult = ffi.HResult(-2147024774)`.

**`ffi.Unknown`** is `IUnknown`, which every interface of COM extends:
`queryInterface`, `addRef` and `release`. An interface that requires it
states its identity, the GUID its header gives, in `id`. The text is read
when the program compiles, and one that is no GUID is an error there
(`E1708`).

**A Tondo value for C.** A type implements an interface with an ordinary
`impl`, of the methods that have no body in the trait. `ffi.object(value)`
gives C an object of it, with the table the compiler writes. For an
interface that requires `ffi.Unknown` the compiler writes `queryInterface`,
`addRef` and `release` as well, and the `impl` has none of the three: the
object starts held once, `queryInterface` answers for each interface of the
type that has an identity and with `HResult.NoInterface` for any other, and
the value is released when the count reaches zero.

A trait that cannot be a table is an error at the trait (`E1707`): one with
type parameters, one that requires more than one trait, or a method whose
parameters or result do not cross to C. The calls follow C's convention on
Linux and macOS.

Text in the APIs of Windows and .NET is UTF-16. `ffi.toWide(text)` makes a
copy of a text as an `ffi.WideString` for C, which `ffi.free` gives back,
and `unsafe { ffi.fromWide(pointer) }` reads one, up to its zero or for a
count of units. A method that answers with a name in the caller's buffer, as
`GetName(capacity, needed, name)` does in those APIs, writes it with
`ffi.writeWide(text, to: name, capacity: n)`: what fits with its zero, never
half of a pair, and the units of the whole text as its result
([Strings](#strings) has its twin for bytes).

The example [`profiler`](../../examples/profiler) is a host and a profiler
that talk through two such interfaces, in the shape of .NET's profiling
API, with objects going both ways.

## The system's calls

What the standard library does not wrap is still within reach, in three
modules.

`std.os.unix` has what Linux and macOS share, with the calls on descriptors
made safe: a `unix.Fd` closes when its value goes, a call that a signal
interrupts is made again, and each fails with `os.Errno`.

```tondo
import std.console
import std.fs
import std.os
import std.os.unix

/// The first bytes of a file, read as the system gives them.
fn start(path: String, length: Int): String ! os.Errno {
    let file = unix.open(path, unix.O_RDONLY)?
    let zero: Byte = 0
    var into = [zero].repeat(length)
    let count = unix.read(ref file, mut into)?
    String.fromBytes(into.take(count)) ?? ""
}

fn main(): !fs.FsError {
    fs.writeText("note.txt", "remember the milk\n")?
    console.println("{start("note.txt", 8)}")
    console.println("{start("missing.txt", 8)}")
    fs.remove("note.txt")?
}
```

```text output
ok(remember)
err(os error 2: No such file or directory)
```

| | |
| --- | --- |
| `unix.open`, `read`, `write`, `seek`, `duplicate`, `pipe` | files and pipes by their descriptors |
| `unix.poll` | waits until one of several descriptors is ready |
| `unix.isTerminal`, `terminalMode`, `setTerminalMode` | a terminal, and how it gives its keys |
| `unix.Mapping.of`, `Mapping.anonymous` | memory mapped from a file or from nothing, unmapped when its value goes |
| `unix.O_RDONLY`, `unix.termios`, `unix.SIGTERM` | C's constants and types, by C's names |

`std.os.linux` and `std.os.macos` declare what is of each system alone, with
C's names and for the architecture of the build: `linux.epoll_create1(0)`,
`linux.O_DIRECT`, thousands of them, which `tondo api
std.os.linux.epoll_create1` shows one at a time. They are imported from a
file of their system
([Modules](15-modules.md#code-for-one-system-or-architecture)), and a call
that has no function is made by its number:

```text
// src/process/process_linux.to
import std.os
import std.os.linux

fn pid(): Int ! os.Errno {
    unsafe { linux.syscall(linux.SYS_getpid) }
}
```

From a file of every system the import is an error, which says where it
goes:

```text
error[E1008]: on macOS: `std.os.linux` is of Linux only: import it from a file of Linux, as `name_linux.to`, or limit the package with `systems = ["linux"]` in tondo.toml
```

## Linking a library

The system's C library needs nothing. Any other is named in `tondo.toml`,
or on the command line:

```toml
[package]
name = "archive"

[native]
libraries = ["z"]                   # -lz
```

```text
tondo run --link z app.to           the same, for one file
```

`"static:z"`, or the path of a `.a` file, links the library into the
executable. A build with `--static` needs every native library to be
static.

`tondo` links executables itself and needs no C compiler for that. It needs
the library to be there, as any program that uses it does.

## A library loaded as the program runs

A library named in `[native]` is part of the program from its start. One
that is only known when the program runs, as a plugin is, is loaded with
`ffi.Library`, which gives its functions by their names:

```tondo
import std.console
import std.ffi

fn main(): !ffi.LibraryError {
    let program = ffi.Library.program()
    let length = unsafe { program.function[extern fn(ffi.CString): ffi.CSize]("strlen")? }
    console.println("{unsafe { length("tondo") }}")
    console.println("{program.has("strlen")} {program.has("no_such_function")}")
}
```

```text output
5
true false
```

| | |
| --- | --- |
| `ffi.Library.open("plugins/libkit.so")` | loads that file, from the current directory unless the path is absolute, and never searches |
| `ffi.Library.named("z")` | searches as the system does, for `libz.so` on Linux and `libz.dylib` on macOS, the name `ffi.libraryFileName("z")` gives |
| `ffi.Library.program()` | the program itself with the libraries it started with, C's among them |

- `function[F](name)` takes the type of an `extern fn` and gives an
  `ffi.Function[F]`, which is called as that function is, inside `unsafe`.
  Asking for it is `unsafe` too, since nothing checks that the function has
  that type. `pointer[T](name)` gives the address of a value the library
  publishes, and `has(name)` says whether it publishes a name.
- A `Function` keeps its library loaded, so it can be kept and called for as
  long as the program likes. Its `raw()` is the bare `extern fn`, to give to
  C as a callback, and keeps nothing.
- A C library stays loaded until the program ends. `unsafe { library.close() }`
  unloads it now, and what is asked of it afterwards is
  `LibraryError.Closed`: the program promises that nothing it took from the
  library is used again and that none of its code is running. One opened
  with `unsafe { ffi.Library.openUnloadable(path) }` is unloaded when the
  last `Library` and `Function` of it go.
- A library that Tondo built is known for what it is, and needs no
  `openUnloadable`: when its last holder goes, or at `close()`, its runtime
  is ended first, its tasks cancelled and its threads finished, and then it
  is unloaded. Whether the memory is given back is the system's decision:
  Linux with glibc unloads the library, and macOS and Linux with musl keep
  it, so there it ends, stays, and starts again when it is next loaded or
  called.
- A static executable loads no library, so a build with `--static` or
  `--target` of a program that reaches `ffi.Library` is refused, and says
  to build it without.

What fails is an `ffi.LibraryError`, which says why, in the system's words
when they are the system's:

```text
the library did not load: libkit.so: cannot open shared object file: No such file or directory
the library publishes no `kit_start`
```

The [`textkit`](../../examples/textkit) example is loaded this way by a
Tondo program.

## Plugins with a contract

`ffi.Library` gives C's functions. A plugin written in Tondo gives more with
`std.plugin`, and with no `unsafe`: it implements a trait, and the program
calls it as that trait, with Tondo's types on both sides.

```tondo
import std.console
import std.plugin

/// What greets someone.
@plugin.Contract
pub trait Greeter {
    fn greet(self, name: String): String ! plugin.Error
}

type Polite = {}

impl Greeter for Polite {
    fn greet(self, name: String): String ! plugin.Error {
        "good morning, {name}"
    }
}

/// A greeting from whatever greets: this program's own type, or a plugin.
fn greeting[G: Greeter](greeter: G): String {
    greeter.greet("Ada") ?? "nobody greets"
}

fn main() {
    console.println(greeting(Polite {}))
    match GreeterPlugin.load("plugins/libpolite.so") {
        ok(loaded) => console.println(greeting(loaded))
        err(_) => console.println("no plugin is installed")
    }
}
```

```text output
good morning, Ada
no plugin is installed
```

- `@plugin.Contract` before a trait makes it a contract. Each of its methods
  takes `self` and gives `T ! plugin.Error`, or `T ! (E | plugin.Error)`
  with an error of its own, since a plugin can be gone when it is called;
  its types are those that are written and read, as records, enums, arrays
  and maps.
- The contract's module gets a type the compiler writes, `GreeterPlugin`,
  which implements `Greeter` by calling a plugin. `GreeterPlugin.load(path)`
  loads a library built with `tondo build --library`, whose function marked
  `@plugin.Export`, as `fn greeter(): impl Greeter { Polite {} }`, gives
  what it offers. `GreeterPlugin.start(process.command("polite"))` starts a
  program, whose `main` serves with `serveGreeter(Polite {})`.
- What crosses is MessagePack, written and read for the contract's types by
  the compiler's own code. Loading compares a fingerprint of the contract,
  its methods and every type they reach, and a plugin built against another
  is `plugin.Error.Contract`. A panic in the plugin comes back as
  `Panicked`, and a program that ended as `Closed`.
- A `GreeterPlugin` is shared by tasks. When its last copy goes, the program
  it started ends, and the library it loaded is ended and unloaded.

The [`tabulate`](../../examples/tabulate) example is a program with a
plugin of each kind, each a package of its own.

## Bindings written for you

Declaring a library's functions by hand is tedious and easy to get wrong.
`tondo bind` reads a C header and writes the module:

```text
tondo bind /usr/include/zlib.h --library z --output src/zlib/zlib.to
tondo bind --package libpng --output src/png/png.to       what pkg-config knows of it
```

It writes the `extern fn` declarations, the `extern type` of each struct and
union, the constants of the header's macros and enums, and the comments, so
that `tondo api app.zlib` then shows the library as it shows any module.
`tondo bind` reads headers with the system's C compiler; the program built
from its output does not need one.

`--fails <file>` names, one on each line, the functions that fail with -1 or
a null pointer and `errno`: they are declared `T ! os.Errno`. `--included`
binds what the header includes as well, for a library that keeps its
constants in other headers, as the system's do.

A header's [interfaces of C](#interfaces-of-c) come as traits. `tondo bind`
takes for one a struct whose only field is `lpVtbl`, a pointer to a struct
of function pointers that each take the object first, which is how MIDL
writes an interface for C; a header of MIDL is read as C with no flag. The
methods keep the header's names and its results, and the `id` is read from
the header's own text, `MIDL_INTERFACE("...")` or `DEFINE_GUID(IID_Name,
...)`:

```text
$ tondo bind counter.h --output src/counter/counter.to
```

```tondo
// Generated by `tondo bind counter.h`; run it again to update this module.

import std.ffi

pub alias ULONG = UInt32

pub alias WCHAR = UInt16

pub alias REFIID = Pointer[ffi.Guid]?

@ffi.Interface(id: "2a0b2c6e-5f1d-4c3a-9b1e-7d410c559321")
pub trait ICounter: ffi.Unknown {
    fn Add(self, amount: Int32): ffi.HResult {
        ffi.HResult.NotImplemented
    }

    fn GetTotal(self, total: Pointer[ffi.CInt]?): ffi.HResult {
        ffi.HResult.NotImplemented
    }
}

@ffi.Interface(id: "6c1e9f02-3b7a-4d5e-8f21-0ab3c4d5e6f7")
pub trait INamedCounter: ICounter {
    fn GetName(self, capacity: ULONG, needed: Pointer[ULONG]?, name: Pointer[WCHAR]?): ffi.HResult {
        ffi.HResult.NotImplemented
    }
}

pub extern fn counter_make(made: Pointer[Pointer[ffi.Void]?]?): ffi.HResult
```

Each method that gives an `HRESULT` comes with a body that answers
`HResult.NotImplemented`, which is COM's own answer for a method an object
does not have. So a Tondo type implements only the methods it cares for, of
an interface that may have eighty: with an `impl counter.ICounter for Tally`
that has `Add` and nothing else, `GetTotal` called through the object's
table answers `0x80004001: Not implemented`. A method with another result
has no body, and every type that implements the interface writes it.

An interface whose id is in a file that was not given, as the `_i.c` that
MIDL writes beside a header, takes it from `--id ICounter=<GUID>`, which can
be repeated. One left without an id is not written, and the module ends by
saying so and what to give:

```text
// Not bound:
// - ICounter: no IID found for it: give it with `--id ICounter=<GUID>`, or bind the header or file that defines IID_ICounter
// - INamedCounter: it extends ICounter, which is not bound
```

The usual shape of a binding is two layers: the module `tondo bind` wrote,
which is C's API as it is, and a small module over it that offers safe
functions, with Tondo's strings, errors as values, and the `unsafe` inside.

## A library for C

It works the other way too. A project with no `main`, built with
`--library`, becomes a library that C, or anything that calls C, can use:

```tondo library
import std.ffi

/// The words of `text`, separated by spaces.
pub extern fn words_count(text: ffi.CString): ffi.CSize {
    let words = unsafe { ffi.fromCString(text) }.split(" ")
    UInt64(words.length()) ?? 0u64
}

/// The sum of `count` numbers.
pub extern fn numbers_sum(numbers: Pointer[ffi.CLong], count: ffi.CSize): ffi.CLong {
    var total = 0
    for index in 0..(Int(count) ?? 0) {
        total += unsafe { numbers.offset(index).read() }
    }
    total
}
```

```text
tondo build --library               libwords.so (.dylib on macOS), libwords.a and words.h
```

A `pub extern fn` with a body is a function C calls by that name. The build
writes a shared library, a static one, and a header with the declarations
and their documentation, and says how to link each:

```c
#include "words.h"

int main(void) {
    return (int) words_count("to be or not to be");
}
```

The library carries Tondo's runtime, which starts at the first call. Inside
an exported function everything of the language is there: tasks, channels,
the standard library. `tondo check --library` and `tondo test --library`
check and test such a project.

The header also declares `void words_stop(void)`, which ends that runtime:
its tasks are cancelled and run their `defer`s, the processes and threads it
started end, and the output it had still to write is written. A call in
progress on another thread finishes first, and a later call starts the
runtime again. A C program calls it before it unloads the library, which
`dlclose` alone does not do.

Two limits are worth knowing. A task of the library in a wait that
cancelling does not end, as reading a terminal or a pipe that nobody writes
to, keeps `words_stop` waiting, and so the unloading: end that wait first.
And each copy of a library that is loaded and unloaded leaves the memory its
allocator took, about 4 MB, so a program that loads a plugin again on every
change of it grows by that much each time.

## C and tasks

- A call into C runs on the task's thread and holds it for as long as it
  takes. When a call takes long, the runtime starts another worker for the
  other tasks, so nothing needs to be marked as blocking.
- C may call back into Tondo from threads of its own. Each such call is a
  task for its length, and may wait.
- A crash inside C, as a fault or an abort, is reported like a panic, with
  the Tondo stack that led to the call. It cannot be recovered from.
- C's memory is C's. What `malloc` gave is freed with `free`, what the
  library allocated with the library's own function, and none of it is seen
  by Tondo's counting.

## Keeping unsafe small

- Put the `unsafe` in one module, behind functions that are safe to call.
  The rest of the program should be unable to misuse it.
- Make each block as short as the operation that needs it, and say next to
  it why it is right: who owns the memory, how long it lives.
- Copy out of C's memory at the boundary. A `String` or an `Array` built
  from it is safe from then on.
- Free in a `defer`, right after the allocation: `defer { unsafe {
  ffi.free(buffer) } }`.
- Prefer a library of the standard one when there is one. SQLite, TLS,
  compression, images and regular expressions are already there, with no
  `unsafe` in the program that uses them.

Next: [The design, and what is not there](19-design.md).
