# profiler: two programs that talk through interfaces of C

A host that loads modules and a profiler that listens to it, in the shape
of .NET's profiling API: the host calls the profiler through one interface
as things happen, and the profiler asks the host through another. Each
side has of the other only a pointer to an object, whose first word points
to a table of functions, as an object of COM is. Both sides are Tondo here,
so it runs with no C compiler; a host written in C++ would be given the
same object. About 175 lines, and 65 of tests.

```text
profiler/
  tondo.toml
  src/
    main.to                       a host with three modules and a profiler that records
    api/api.to                    the two interfaces, as a header would declare them
    host/host.to                  the host: answers `Info`, and drives a `Callback`
    host/host_test.to             the two together, and what the host refuses
    recorder/recorder.to          a profiler: hears `Callback`, and asks `Info`
    recorder/recorder_test.to     a failure that crosses and comes back
```

## Run it

```sh
tondo run examples/profiler
tondo test examples/profiler
```

```text
$ tondo run examples/profiler
the host ran
0 still hold the profiler
initialized
loaded kernel
loaded café.dll
loaded app
shut down
```

## What to look at

**An interface is a trait.** In [`api.to`](src/api/api.to), `Callback` and
`Info` are traits with `@ffi.Interface(id: "...")` that require
`ffi.Unknown`, which is `IUnknown`. Their methods are the entries of the
table in the order they are written, after `queryInterface`, `addRef` and
`release`. The `id` is the identity a header gives the interface, and it is
checked as a GUID when the program compiles.

**A Tondo type is an object.** [`recorder.to`](src/recorder/recorder.to)
implements `Callback` for `Recorder` with an ordinary `impl`, and
[`main.to`](src/main.to) calls `ffi.object(heard)`: the compiler writes the
table, and with it `queryInterface`, `addRef` and `release`, which the
`impl` does not have. What `main` gives the host is a `Pointer[ffi.Void]`.

**An object is called through its interface.** The host, in
[`host.to`](src/host/host.to), asks the pointer for `Callback` by its
identity, `api.CallbackObject.Id`, and makes a `CallbackObject` of the
answer. `CallbackObject` is a type the compiler gives the trait's module:
it implements the trait by calling the table. Making one from a pointer is
`unsafe`, the one thing here that nothing can check.

**It goes both ways.** The host is an object too: it implements `Info`,
gives the profiler `ffi.object(self)` in `initialize`, and the profiler asks
it for each module's name. An object that is not a profiler is refused
before anything is called: `anObjectThatIsNoProfilerIsRefused` gives the
host another host, and gets `HResult.NoInterface`.

**A failure is an `HRESULT` on the way, and a failure at each end.** The
methods are declared `: !ffi.HResult` and `: UInt32 ! ffi.HResult`, so
their callers use `?` as with any function that fails. `moduleName` fails
with `HResult.InvalidArgument` for a module the host does not have. In
[`recorder_test.to`](src/recorder/recorder_test.to) a failure goes through
the table as a negative number and is a failure again in the caller.

**Who holds what.** An object starts held once, by who made it.
`queryInterface` holds it once more for who asked, and `release` lets go.
The host releases in a `defer`, so it lets go on the paths that fail too,
and the profiler releases the host's object in `shutdown`. Each test ends
with `api.released(object) == 0`: the last hold was its own.

**Text is UTF-16 at this boundary.** `moduleName` writes the name the way
.NET's API does: in a buffer the caller owns, what fits of it with its
zero, never past the capacity, and it gives the units of the whole name.
That is `ffi.writeWide`, in one call. The profiler brings room for sixteen
units, and when the answer is not under that it asks again with the room
it was told, as a caller of such an API does:
`aLongNameIsAskedForAgainWithItsRoom`. `ffi.fromWide` reads the name;
`café.dll` is there so that a character of two bytes in UTF-8 is one unit
here.

## Going further

- Give `Callback` a second version, `Callback2: Callback`, with a method
  more, and have the host ask for it first and fall back to the first.
- Count the calls in the recorder with a `sync.Atomic[Int]`, and have a
  task of the host load modules while another asks for their names.
- Write the host's side in C: a function that takes the object and calls
  its table, linked with the program
  ([Linking a library](../../docs/language/18-unsafe.md#linking-a-library)).
