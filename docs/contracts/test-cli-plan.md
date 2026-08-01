# `tondo test` CLI plan contract

**Status:** implemented as the parse-only CLI boundary; execution consumes the
canonical `tondo.test.json` sidecar before worker creation.

`tondo_cli::test_cli::parse` converts one UTF-8 argument vector beginning with
`test` into a typed `TestCliPlan`. It performs no discovery, source I/O,
compilation, worker creation or test execution.

## Normalized options

The plan closes one selector (`all`, `filter`, `glob` or `exact`), CODEOWNERS
mode/path, optional shard, canonical/random order with an optional `u64` seed,
list mode, jobs, timeout, retry/repeat, artifact root, diagnostic/test formats,
repeatable JSON/JUnit report outputs and the `show-output`, `deny-skips`,
`allow-flaky`, `allow-empty` and snapshot-update policies. Decimal integers are
canonical and bounded; random seeds are parsed from at most sixteen hex digits;
durations normalize `ms`, `s`, `m` and `h` to checked milliseconds.

Explicit retry/repeat presence is retained even for values `0` and `1`, because
the specification gives those spellings distinct compatibility rules for
`--list`, `--update-snapshots` and `--allow-flaky`. Report paths are checked for
duplicates after lexical path normalization. CODEOWNERS and artifact paths are
validated as relative logical paths where the spec requires it.

## Rejections and execution boundary

Unknown or repeated singleton options, missing values, invalid globs, shard
ranges, non-canonical numbers, seed/order mismatches, selector collisions,
report collisions, positional arguments, and incompatible list/retry/repeat/
snapshot modes produce a usage error (exit `2`) before any compilation. The
parser remains side-effect-free; the CLI consumes the closed plan only after
this boundary, validates the adjacent `tondo-test-plan-draft` sidecar against
the manifest/lockfile hashes, and delegates discovery, selection, scheduling,
process-isolated VM execution and report publication to their compiler/runtime
modules. The sidecar is required, canonical, and its timeout/resource and
snapshot-store inputs are not replaceable by environment variables. Its
artifact-store path is the default output root and its format/byte limit remain
closed; an explicit `--artifacts` path may relocate that bounded output for one
invocation. An explicit `--timeout none` is accepted by the parser for
compatibility but rejected at this execution boundary because the sidecar
always carries a positive wall-clock limit.

Unit tests cover defaults, both option spellings, complete option composition,
selectors/numbers/paths/globs/report collisions, explicit zero/one retry and
repeat rules, list/update incompatibilities, and unknown or positional input.
An integration test confirms the exit boundary for valid and invalid `test`
invocations.
