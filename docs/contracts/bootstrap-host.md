# Bootstrap standard-library host boundary

**Status:** implemented M3 provisional boundary
**Language baseline:** Tondo 0.1-draft.8

This contract exists only to make the first VM backend observable without
freezing the future standard-library ABI. It defines one source-visible module
and one typed host operation:

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

`std.console` is present only when the closed target capability set contains
the registered `console` capability. The built-in `tondo-vm-hosted` CLI target
declares exactly that capability during M3. A request without it removes the
module from the selected bootstrap standard package; importing it produces
`E1008` and names the missing capability. There is no runtime stub that always
fails.

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
state. The host must return `Unit`; any other value is a toolchain host error,
not a Tondo value or panic.

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
host argument snapshots, exact output without an implicit newline, and
verification before the first possible host invocation.
