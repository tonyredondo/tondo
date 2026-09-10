# Bootstrap standard-library host boundary

**Status:** hosted implementation; full owner promotion remains tracked separately
**Language baseline:** Tondo 0.1

This contract makes hosted effects observable without freezing a native ABI.
It defines the console bridge here and delegates the process surface to
[`process-host.md`](./process-host.md):

~~~tondo
import std.console

fn main(): !console.ConsoleError {
    console.print("Hello, world")?
}
~~~

The exact signature is `std.console.print(value: String): Unit ! ConsoleError`.
Callers handle, propagate or explicitly discard the result. `ConsoleError` is
an ordinary nominal enum, including `Io(std.io.IoError)`.
`print` appends the UTF-8 bytes of its argument and adds no separator or
newline.

## Target admission

`std.console`, `std.process`, `std.time`, and `std.env` are present only when the
closed target capability set contains `console`, `process`, `clock`, and
`environment`, respectively. The built-in `tondo-vm-hosted` CLI target declares
all four. A request without
the relevant capability removes that module from the selected bootstrap
standard package; importing it produces `E1008` and names the missing
capability. There is no runtime stub that always fails. The monotonic time
boundary is documented in [`stdlib-time.md`](./stdlib-time.md); the environment
snapshot boundary is documented in [`stdlib-env.md`](./stdlib-env.md).

The module belongs to package `toolchain:std:0.1-bootstrap`. Its registered
declarations and selected standard sources expose only the documented console
surface. The compiler does not treat arbitrary unresolved names as callable
host functions. Ordinary source implements the static I/O trait adapters for
the concrete `Input` and `Output` types.

## Compiler and VM representation

Public console calls lower through registered typed host callables. The
compiler and bytecode verifier preserve the argument and fallible result
types. The internal legacy `BytecodeBootstrapHostFunction::ConsolePrint`
operation is not emitted for the public `print` declaration and does not
define its signature. This is not a general-purpose FFI.

Only verified bytecode can invoke the host. The VM passes detached
`RuntimeValue` snapshots, never heap handles, frame references, or mutable VM
state. Retaining or mutating such a snapshot does not retain or mutate its
former VM object. A returned compound snapshot is rematerialized while
completed children remain operation-local roots. `print` returns a typed
successful `Unit` or nominal `ConsoleError` result. Its complete VM result is
admitted before output bytes are emitted.
Process plans and opaque results use typed run-local host identities; process
waits run independently and enter the VM again only through the verified suspendible
completion path. Any shape mismatch is a toolchain host error, not a Tondo
value or panic.

M9 adds six separate raw Pointer host-operation identities. They are not part
of `std.console` or `std.process`, cannot be selected by an arbitrary source
name, and preserve their concrete receiver, arguments, result, and unsafe
effect through every verifier. Their dynamic implementation requires a pinned
privileged target adapter; the bootstrap target exposes no allocator, stable
layout, or safe native-address source. See [`unsafe.md`](./unsafe.md) and
`../../TONDO_TOOLCHAIN_SPEC.md`.

The compiler driver's bootstrap host buffers bytes in evaluation order and
places them in `CompilationOutput.stdout`. The CLI writes that buffer to process
stdout and keeps all compiler/runtime diagnostics on stderr. Output produced
before a language panic remains program output; an internal VM/toolchain
failure does not masquerade as a successful partial run.

## Promotion boundary

The complete public signatures, input cursor rules, output routing and flushing
contract are defined in [`stdlib-hosted.md`](./stdlib-hosted.md) and the
standard-library specification. This hosted implementation does not establish
a native ABI or native AOT support. Whole-owner fuzzing and public conformance
remain separate tracker obligations.

Required regression coverage includes accepted and rejected call shapes,
capability-present and capability-absent imports, HIR-to-bytecode preservation,
host argument snapshots that do not become VM roots, exact output without an
implicit newline, suspension progress, cleanup, and verification before the first
possible host invocation.
