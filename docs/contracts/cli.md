# CLI contract

**Status:** active contract for the current unpublished Tondo 0.1 draft

## Commands

~~~text
tondo fmt [--check] [--diagnostic-format <human|json>] <source.to>
tondo check [--diagnostic-format <human|json>] [--project <dir>]
tondo run [--diagnostic-format <human|json>] [--project <dir>] [-- <argument>...]
tondo test [--project <dir>] [--test-plan <tondo.test.toml>] [options]
tondo --help
tondo --version
~~~

The CLI accepts one loose source file or one conventional project directory.
The forms cannot be combined. Without
`--project`, the current directory is used for `check`, `run` and `test`.

- `fmt` and `check` classify the loose root as a module.
- A conventional project uses `src/`, optional `tests/`, optional `tondo.toml`
  and an optional generated `tondo.lock.toml`; it never uses a JSON project
  configuration.
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
- Program arguments must be valid UTF-8 and reach
  `std.env.snapshot().arguments()` in their original order. Flags after `--`
  are program data, not CLI options.

Build and project options are:

~~~text
--emit-interface <path>
--emit-artifact <path>
~~~

`--emit-interface` and `--emit-artifact`
are valid for `check` or `run`, in loose-source or project mode, and write
canonical products only after successful compilation. The two outputs must
differ and cannot overwrite the source, active project source, dependency
interface, generator input, or privileged unit named by the invocation. `fmt`
accepts neither a project nor build products.

The CLI discovers the conventional project and creates the closed internal
plan. It then asks the pure project plan for its exact required inputs, reads
only those relative to the project root, and passes their bytes back to the
compiler. Discovery does not resolve versions or use network access; after the
plan boundary the compiler performs no directory scan or implicit generator
execution. JSON diagnostics and reports are output formats, not project input
formats.

## Logical identity for a loose source

The CLI reads the physical path, but the driver receives:

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

## Dynamic diagnostic profiles (contract-locked 0.1 tooling)

The compiler diagnostic stream above remains unchanged. Runtime instrumentation
is an explicit, separate profile selected for one invocation:

~~~text
tondo run  --diagnostics <race|leaks|crash|all>[,...] ...
tondo test --diagnostics <race|leaks|crash|all>[,...] ...
tondo dump analyze <file.tdump> [--format human|json]
~~~

The exact option spelling and report schema are owned by
[`diagnostic-tooling.md`](./diagnostic-tooling.md),
[`testing/diagnostic-tooling.json`](../../testing/diagnostic-tooling.json), and
RFC-019. Profiles are
not project configuration, do not add source keywords, and do not introduce a
second stdlib API. `tondo test` associates each report or `.tdump` with the
existing attempt/artifact identity; retries use fresh processes. An unsupported
profile is an explicit toolchain result, never a silent skip. The D0 contract is
locked, while runtime instrumentation and CI promotion remain pending until
the later `DIAG-*` blocks close.

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
rendered without changing that status. `run` lowers sync or inferred suspendible
`main` and implicit script entry bodies through verified HIR, MIR, and bytecode
and executes them in the VM. Root scripts may use a shebang, top-level
statements, `await`, structured scopes, and the capability-gated process API.
The CLI never returns success for an unimplemented operation.
