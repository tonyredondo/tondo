# Bootstrap CLI contract

**Status:** active contract for the current unpublished Tondo 0.1 draft

## Commands

~~~text
tondo fmt [--check] [--diagnostic-format <human|json>] <source.to>
tondo check [--diagnostic-format <human|json>] [--project <dir>]
tondo run [--diagnostic-format <human|json>] [--project <dir>] [-- <argument>...]
tondo test [--project <dir>] [--test-plan <tondo.test.toml>] [options]
tondo check [--diagnostic-format <human|json>] --manifest <tondo.json>
tondo run [--diagnostic-format <human|json>] --manifest <tondo.json> [-- <argument>...]
tondo --help
tondo --version
~~~

The bootstrap accepts one loose source file, one conventional project directory,
or one closed legacy project manifest. The forms cannot be combined. Without
`--project`, the current directory is used for `check`, `run` and `test`.

- `fmt` and `check` classify the loose root as a module.
- A conventional project uses `src/`, optional `tests/`, optional `tondo.toml`
  and an optional generated `tondo.lock.toml`; it never requires `tondo.json`.
- `run` classifies the loose root as a script.
- `fmt` writes canonical source to stdout and never edits the input file.
- `fmt --check` writes no source and exits with code 1 when the input differs
  from its canonical form; canonical input exits with code 0.
- The source must use the `.to` extension.
- Diagnostic format defaults to `human`.
- `--diagnostic-format=json` and the two-argument spelling are equivalent.
- Unknown flags and additional source paths are usage errors.
- `--check` on the `check` or `run` command is a usage error.
- Only `run` accepts program arguments, and only after the exact `--`
  separator. `fmt` and `check` reject the separator.
- Program arguments must be valid UTF-8 and reach `std.process.args()` in
  their original order. Flags after `--` are program data, not CLI options.

Build and project options are:

~~~text
--lockfile <path>
--emit-interface <path>
--emit-artifact <path>
~~~

Without `--lockfile`, legacy manifests use `tondo.lock.json` beside the
manifest. Conventional projects use `tondo.lock.toml` automatically and do
not accept a manual lockfile override. `--lockfile` requires `--manifest`.
`--emit-interface` and `--emit-artifact`
are valid for `check` or `run`, in loose-source or project mode, and write
canonical products only after successful compilation. The two outputs must
differ and cannot overwrite the source, manifest, lockfile, active project
source, dependency interface, generator input, or privileged unit named by the
invocation. `fmt` accepts neither a project nor build products.

The CLI either discovers the conventional project and creates an equivalent
closed internal plan, or parses the legacy manifest and lockfile. It then asks
the pure project plan for its exact required inputs, reads only those relative
to the project root/manifest directory, and passes their bytes back to the
compiler. Discovery does not resolve versions or use network access; after the
plan boundary the compiler performs no directory scan or implicit generator
execution.

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

This rule is intentionally limited to one loose file. A project invocation
derives identity from the exact `PackageId`, module, logical path, target, and
source-set selection recorded by its closed manifest and lockfile.

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
rendered without changing that status. `run` lowers explicit sync or async
`main` and implicit script entry bodies through verified HIR, MIR, and bytecode
and executes them in the VM. Root scripts may use a shebang, top-level
statements, `await`, structured scopes, and the capability-gated process API.
The CLI never returns success for an unimplemented operation.
