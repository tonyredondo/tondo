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

The process exit behavior of a successfully launched Tondo program is defined by
the language and hosted runtime; it will be incorporated when `run` can execute.

## Bootstrap honesty

Every implemented phase runs before the bootstrap marker. A lexical or later
language error therefore returns only its normative diagnostics with exit code
1 and no partial formatter output. `fmt` is complete for its one-file bootstrap
surface and succeeds after syntax validation. `check` succeeds with exit code 0
when expression checking reports a complete M2 semantic snapshot; warnings are
rendered without changing that status. A deliberately deferred semantic
surface, and every `run` request until M3 is complete, returns `T0001` with exit
code 1. The CLI never returns success for an unimplemented operation.
