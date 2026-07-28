# Bootstrap standard-library host boundary

**Status:** implemented and conformant for Tondo 0.1
**Language baseline:** Tondo 0.1

This contract makes hosted effects observable without freezing a native ABI.
It defines the console bridge here and delegates the process surface to
[`process-host.md`](./process-host.md):

~~~tondo
import std.console

fn main() {
    console.print("Hello, world")
}
~~~

The exact bootstrap signature is `std.console.print(value: String): Unit`.
There are no named, borrowed, mutable, variadic, generic, or fallible forms.
`print` appends the UTF-8 bytes of its argument and adds no separator or
newline.

## Target admission

`std.console` and `std.process` are present only when the closed target
capability set contains `console` and `process`, respectively. The built-in
`tondo-vm-hosted` CLI target declares both. A request without either capability
removes that module from the selected bootstrap standard package; importing it
produces `E1008` and names the missing capability. There is no runtime stub
that always fails.

The module is source-less and belongs to package
`toolchain:std:0.1-bootstrap`. Resolution may expose only the exact `print`
value identity above. The bootstrap does not treat arbitrary unresolved names
inside a source-less module as callable host functions.

## Compiler and VM representation

The call becomes a dedicated typed HIR node, then a dedicated MIR operation,
then `BytecodeBootstrapHostFunction::ConsolePrint`. Every verifier independently
checks one `String` argument and a `Unit` result. It does not lower through a
stringly typed general-purpose FFI or through a callable with a missing body.

Only verified bytecode can invoke the host. The VM passes detached
`RuntimeValue` snapshots, never heap handles, frame references, or mutable VM
state. Retaining or mutating such a snapshot does not retain or mutate its
former VM object. A returned compound snapshot is rematerialized while
completed children remain operation-local roots. `print` must return `Unit`.
Process plans and opaque results use typed run-local host identities; process
waits run independently and enter the VM again only through the verified async
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

## Provisional status

This boundary does not specify buffering, flushing, terminal detection,
encoding APIs, formatting, stderr, input, or the eventual implementation layout
of `std.console`. Those belong to the standard-library and toolchain
specifications. A later implementation may replace this dedicated opcode with
ordinary linked standard-library code if it preserves source behavior, target
capability admission, stream routing, evaluation order, and diagnostics.

Required regression coverage includes accepted and rejected call shapes,
capability-present and capability-absent imports, HIR-to-bytecode preservation,
host argument snapshots that do not become VM roots, exact output without an
implicit newline, async progress, cleanup, and verification before the first
possible host invocation.
