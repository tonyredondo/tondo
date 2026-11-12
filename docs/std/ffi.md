# std.ffi

what calling C needs. C functions are declared with `extern fn` and C structs with `extern type`; this module names C's types and gives memory and strings that C can use.

```tondo
import std.console
import std.ffi

extern fn abs(value: ffi.CInt): ffi.CInt

console.println("{unsafe { abs(-42) }}")
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.ffi` shows the same in a terminal.

- [Functions](#functions): [`sizeOf`](#sizeof), [`alignOf`](#alignof), [`allocate`](#allocate), [`free`](#free), [`copy`](#copy), [`toCString`](#tocstring), [`fromCString`](#fromcstring), [`fromCChars`](#fromcchars), [`toCChars`](#tocchars), [`context`](#context), [`fromContext`](#fromcontext), [`freeContext`](#freecontext), [`bytes`](#bytes)
- [CChar](#cchar)
- [CShort](#cshort)
- [CUShort](#cushort)
- [CInt](#cint)
- [CUInt](#cuint)
- [CLong](#clong)
- [CULong](#culong)
- [CLongLong](#clonglong)
- [CULongLong](#culonglong)
- [CSize](#csize)
- [CSSize](#cssize)
- [CFloat](#cfloat)
- [CDouble](#cdouble)
- [CBool](#cbool)
- [CString](#cstring)
- [Void](#void)
- [VaList](#valist)

### Functions

#### sizeOf

```tondo
pub fn sizeOf[T](): Int
```

The size in bytes of a value of `T` in memory.

#### alignOf

```tondo
pub fn alignOf[T](): Int
```

The alignment in bytes of a value of `T` in memory.

#### allocate

```tondo
pub fn allocate[T: ~Copy](count: Int = 1): Pointer[T]
```

Zeroed memory for `count` values of `T`, from C's allocator: C may free
it, and `free` gives it back.

#### free

```tondo
pub unsafe fn free(pointer: Pointer[Void]?)
```

Gives back memory from `allocate`, `toCString` or C's allocator; `none`
does nothing, as in C.

#### copy

```tondo
pub unsafe fn copy[T](from: Pointer[T], to: Pointer[T], count: Int = 1)
```

Copies `count` values of `T` from one address to another; the regions
may overlap.

#### toCString

```tondo
pub fn toCString(text: String): CString
```

A copy of the text that ends with a zero byte, for C to keep; `free`
gives it back. Text with a zero byte inside panics (`P0016`).

#### fromCString

```tondo
pub unsafe fn fromCString(text: CString): String
```

The text of a C string, up to its zero byte; bytes that are not UTF-8
become U+FFFD.

#### fromCChars

```tondo
pub fn fromCChars(chars: Array[CChar]): String
```

The text of a C array of characters, such as a struct's
`@count(16) name: CChar`, up to its first zero byte; bytes that are not
UTF-8 become U+FFFD.

#### toCChars

```tondo
pub fn toCChars(text: String, count: Int): Array[CChar]
```

The text as a C array of `count` characters: its bytes, then zero bytes.
Text that does not fit with a zero byte after it panics.

#### context

```tondo
pub fn context[T: ~Copy](value: T): Pointer[Void]
```

A value for C to hand back to a callback, as C passes data through a
`void*`: a copy of it in memory from `allocate`. `fromContext` reads it
and `freeContext` gives it back.

#### fromContext

```tondo
pub unsafe fn fromContext[T](data: Pointer[Void]): T
```

The value of a context made by `context`, which must hold a `T`.

#### freeContext

```tondo
pub unsafe fn freeContext[T: ~Copy](data: Pointer[Void])
```

Gives back a context made by `context`, with the value it holds.

#### bytes

```tondo
pub unsafe fn bytes(from: Pointer[Byte], count: Int): Array[Byte]
```

A copy of `count` bytes at an address.

### CChar

```tondo
pub alias CChar = Byte
```

C's `char`, a byte.

```tondo
import std.console
import std.ffi

extern fn snprintf(buffer: Pointer[ffi.CChar], size: ffi.CSize, format: ffi.CString, ...): ffi.CInt

// A buffer of C characters that C writes into.
let buffer = ffi.allocate[ffi.CChar](32)
unsafe {
    _ = snprintf(buffer, 32u64, "%s has %d items", "cart", 3i32)
    console.println(ffi.fromCString(buffer))
    ffi.free(buffer)
}
```

### CShort

```tondo
pub alias CShort = Int16
```

C's `short`.

```tondo
import std.console
import std.ffi

extern type PollFd = {
    fd: ffi.CInt
    events: ffi.CShort
    revents: ffi.CShort
}

extern fn poll(fds: Pointer[PollFd], count: ffi.CULong, timeout: ffi.CInt): ffi.CInt

// Whether standard input has something to read, without waiting.
let readable: ffi.CShort = 1
let watched = ffi.allocate[PollFd]()
unsafe {
    watched.write(PollFd { fd: 0, events: readable, revents: 0 })
    let ready = poll(watched, 1u64, 0)
    console.println("ready: {ready}, events: {watched.read().revents}")
    ffi.free(watched)
}
```

### CUShort

```tondo
pub alias CUShort = UInt16
```

C's `unsigned short`.

```tondo
import std.console
import std.ffi

extern fn htons(value: ffi.CUShort): ffi.CUShort

// A port in the byte order of the network, as a C socket address holds it.
let port: ffi.CUShort = 8080
console.println("{port} is {unsafe { htons(port) }} in network order on this machine")
```

### CInt

```tondo
pub alias CInt = Int32
```

C's `int`.

```tondo
import std.console
import std.ffi

@symbol("atoi")
extern fn parseInt(text: ffi.CString): ffi.CInt

extern fn getpid(): ffi.CInt

let count: ffi.CInt = unsafe { parseInt("42") }
// A narrower result widens where an `Int` is expected.
let total: Int = count + 1
console.println("{total} in process {unsafe { getpid() } > 0}")
```

### CUInt

```tondo
pub alias CUInt = UInt32
```

C's `unsigned int`.

```tondo
import std.console
import std.ffi

extern fn htonl(value: ffi.CUInt): ffi.CUInt

let address: ffi.CUInt = 0x7f000001u32
console.println("127.0.0.1 is {address} here and {unsafe { htonl(address) }} on the wire")
```

### CLong

```tondo
pub alias CLong = Int
```

C's `long`, of 64 bits on the supported targets.

```tondo
import std.console
import std.ffi

extern fn strtol(text: ffi.CString, end: Pointer[ffi.CString]?, base: ffi.CInt): ffi.CLong

console.println("{unsafe { strtol("ff", none, 16) }} {unsafe { strtol("-777", none, 8) }}")
```

### CULong

```tondo
pub alias CULong = UInt64
```

C's `unsigned long`, of 64 bits on the supported targets.

```tondo
import std.console
import std.ffi

extern fn strtoul(text: ffi.CString, end: Pointer[ffi.CString]?, base: ffi.CInt): ffi.CULong

let mask: ffi.CULong = unsafe { strtoul("11110000", none, 2) }
console.println("{mask}")
```

### CLongLong

```tondo
pub alias CLongLong = Int
```

C's `long long`.

```tondo
import std.console
import std.ffi

extern fn atoll(text: ffi.CString): ffi.CLongLong

extern fn llabs(value: ffi.CLongLong): ffi.CLongLong

let bytes = unsafe { atoll("-9000000000") }
console.println("{unsafe { llabs(bytes) }}")
```

### CULongLong

```tondo
pub alias CULongLong = UInt64
```

C's `unsigned long long`.

```tondo
import std.console
import std.ffi

extern fn strtoull(text: ffi.CString, end: Pointer[ffi.CString]?, base: ffi.CInt): ffi.CULongLong

let largest: ffi.CULongLong = unsafe { strtoull("18446744073709551615", none, 10) }
console.println("{largest}")
```

### CSize

```tondo
pub alias CSize = UInt64
```

C's `size_t`.

```tondo
import std.console
import std.ffi

extern fn strlen(text: ffi.CString): ffi.CSize

// The bytes of UTF-8, which is what C counts.
let size: ffi.CSize = unsafe { strlen("año") }
console.println("{size} bytes")
```

### CSSize

```tondo
pub alias CSSize = Int
```

C's `ssize_t`.

```tondo
import std.ffi

extern fn write(fd: ffi.CInt, data: ffi.CString, count: ffi.CSize): ffi.CSSize

// The bytes written, or -1 when it fails.
let text = "written by C\n"
let written: ffi.CSSize = unsafe { write(1, text, UInt64(text.length()) ?? 0u64) }
if written < 0 {
    panic("write failed")
}
```

### CFloat

```tondo
pub alias CFloat = Float32
```

C's `float`.

```tondo
import std.console
import std.ffi

extern fn strtof(text: ffi.CString, end: Pointer[ffi.CString]?): ffi.CFloat

let ratio: ffi.CFloat = unsafe { strtof("0.75", none) }
console.println("{ratio * 2.0f32}")
```

### CDouble

```tondo
pub alias CDouble = Float
```

C's `double`.

```tondo
import std.console
import std.ffi

extern fn atof(text: ffi.CString): ffi.CDouble

extern fn frexp(value: ffi.CDouble, exponent: Pointer[ffi.CInt]): ffi.CDouble

let exponent = ffi.allocate[ffi.CInt]()
unsafe {
    let fraction = frexp(atof("48.0"), exponent)
    console.println("48 = {fraction} * 2^{exponent.read()}")
    ffi.free(exponent)
}
```

### CBool

```tondo
pub alias CBool = Bool
```

C's `bool`.

```tondo
import std.console
import std.ffi

/// The options a C library takes, with C's `bool`.
extern type Options = {
    verbose: ffi.CBool
    level: ffi.CInt
}

/// A function C can call back, as a library's filter.
extern fn accept(options: Options, value: ffi.CInt): ffi.CBool {
    options.verbose or value <= options.level
}

let options = Options { verbose: false, level: 3 }
console.println("{unsafe { accept(options, 2) }} {ffi.sizeOf[Options]()}")
```

### CString

```tondo
pub alias CString = memory.CString
```

A C string: the address of bytes that end with a zero byte. A `String`
passed to a C function that takes one becomes such a copy for the call.

```tondo
import std.console
import std.ffi

extern fn getenv(name: ffi.CString): ffi.CString?

extern fn strdup(text: ffi.CString): ffi.CString

unsafe {
    // A C string C may return as null is an option.
    match getenv("HOME") {
        some(home) => console.println("home is {ffi.fromCString(home)}")
        none => console.println("no home")
    }
    // A copy C made with its allocator, given back with `free`.
    let copy = strdup("hello")
    console.println(ffi.fromCString(copy))
    ffi.free(copy)
}
let kept = ffi.toCString("for C to keep")
unsafe { ffi.free(kept) }
```

### Void

```tondo
pub alias Void = memory.Void
```

What C's `void*` points to: any `Pointer[T]` converts to
`Pointer[ffi.Void]`.

```tondo
import std.console
import std.ffi

extern fn memset(target: Pointer[ffi.Void], value: ffi.CInt, count: ffi.CSize): Pointer[ffi.Void]

let numbers = ffi.allocate[Int32](4)
unsafe {
    // Any pointer goes where C takes a `void*`.
    _ = memset(numbers, 0xff, 16u64)
    console.println("{numbers.read()}")
    ffi.free(numbers)
}
```

### VaList

```tondo
pub extern type VaList
```

A `va_list` of C, which only C makes: a function C calls may receive a
`Pointer[VaList]` and pass it along, as to `vsnprintf`.

```tondo
import std.console
import std.ffi

extern fn vsnprintf(buffer: Pointer[ffi.CChar], size: ffi.CSize, format: ffi.CString, args: Pointer[ffi.VaList]): ffi.CInt

/// A library's log callback, which receives the arguments of its `printf`
/// format as a `va_list` and lets C format them.
extern fn logged(level: ffi.CInt, format: ffi.CString, args: Pointer[ffi.VaList]) {
    let buffer = ffi.allocate[ffi.CChar](512)
    unsafe {
        _ = vsnprintf(buffer, 512u64, format, args)
        console.println("[{level}] {ffi.fromCString(buffer)}")
        ffi.free(buffer)
    }
}

extern fn library_set_log(callback: extern fn(ffi.CInt, ffi.CString, Pointer[ffi.VaList]))

unsafe { library_set_log(logged) }
```
