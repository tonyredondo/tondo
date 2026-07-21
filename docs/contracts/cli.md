# Bootstrap CLI contract

**Status:** accepted for implementation version 0.0.x

## Commands

~~~text
tondo fmt [--check] [--diagnostic-format <human|json>] <source.to>
tondo check [--diagnostic-format <human|json>] <source.to>
tondo run [--diagnostic-format <human|json>] <source.to>
tondo --help
tondo --version
~~~

The bootstrap accepts exactly one source file. Multiple-file package builds are
added only after the manifest and package graph contract exists.

- `fmt` and `check` classify the loose root as a module.
- `run` classifies the loose root as a script.
- `fmt` writes canonical source to stdout and never edits the input file.
- `fmt --check` writes no source and exits with code 1 when the input differs
  from its canonical form; canonical input exits with code 0.
- The source must use the `.to` extension.
- Diagnostic format defaults to `human`.
- `--diagnostic-format=json` and the two-argument spelling are equivalent.
- Unknown flags and additional source paths are usage errors.
- `--check` on the `check` or `run` command is a usage error.

## Logical identity for a loose source

The CLI reads the physical path, but the bootstrap driver receives:

~~~text
source_id = root:cli
module    = main
file      = <UTF-8 basename of the physical path>
target    = tondo-vm-hosted
profile   = hosted
edition   = 0.1
package   = synthetic loose root selected before compilation
~~~

This rule is intentionally limited to one loose file. A package invocation will
derive identity from the resolved package graph instead.

## Streams

- Help, version information, formatter output, and program stdout use stdout.
- Human and JSON diagnostics use stderr.
- Usage errors and internal toolchain errors use stderr.
- JSON diagnostics are JSON Lines and never include ANSI escapes.

Keeping diagnostics on stderr prevents `tondo run` from mixing a program's
stdout with machine-readable compiler output.

## Exit codes

| Code | Meaning |
|---|---|
| `0` | Requested operation completed successfully |
| `1` | Tondo diagnostics rejected the operation, or `fmt --check` found a formatting difference |
| `2` | Invalid invocation, unreadable input, or unsupported CLI shape |
| `3` | Internal toolchain failure |
| `101` | An executed Tondo program ended in a language panic |

For a launched synchronous program, returning `Unit` or `ok(Unit)` exits 0. A
fallible `main` is admitted only when its error type satisfies `Discard`; an
unhandled admitted error emits `R0001` and exits 1. A language panic emits its
normative `P` diagnostic and exits 101.

## Bootstrap honesty

Every implemented phase runs before the bootstrap marker. A lexical or later
language error therefore returns only its normative diagnostics with exit code
1 and no partial formatter output. `fmt` is complete for its one-file bootstrap
surface and succeeds after syntax validation. `check` succeeds with exit code 0
when expression checking reports a complete semantic snapshot; warnings are
rendered without changing that status. `run` lowers a valid synchronous
explicit `main` through verified HIR, MIR, and bytecode and executes it in the
VM. Async entry points and implicit script bodies remain explicit later
milestones and return `T0001`. The CLI never returns success for an
unimplemented operation.
