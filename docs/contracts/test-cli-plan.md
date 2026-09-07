# `tondo test` CLI plan contract

**Status:** implemented as the parse-only CLI boundary; execution consumes an
optional TOML `tondo.test.toml` sidecar or materializes the opinionated defaults
in memory before worker creation.

`tondo_cli::test_cli::parse` converts one UTF-8 argument vector beginning with
`test` into a typed `TestCliPlan`. It performs no discovery, source I/O,
compilation, worker creation or test execution.

## Normalized options

The plan closes an optional conventional project and test-plan path, one
selector (`all`, `filter`, `glob` or `exact`), CODEOWNERS
mode/path, optional shard, canonical/random order with an optional `u64` seed,
list mode, jobs, timeout, retry/repeat, artifact root, diagnostic/test formats,
repeatable JSON/JUnit report outputs and the `show-output`, `deny-skips`,
`allow-flaky`, `allow-empty` and snapshot-update policies. Decimal integers are
canonical and bounded; random seeds are parsed from at most sixteen hex digits;
durations normalize `ms`, `s`, `m` and `h` to checked milliseconds.

When `--project` is omitted, execution discovers the current directory by
convention. `--project` selects another conventional root. `--test-plan` is
optional; execution reads the adjacent `tondo.test.toml` when present and
otherwise materializes the canonical default plan in memory.

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
this boundary. Without `--test-plan`, an adjacent `tondo.test.toml` is used if
present; otherwise the compiler materializes the defaults from the closed
project graph. TOML is normalized to the same closed wire shape before
validation. A supplied `--test-plan` is
validated against the project/lockfile hashes. Discovery, selection, scheduling,
process-isolated VM execution and report publication then consume one effective
plan. Invocation-local flags overlay only policy and selection fields; they do
not rewrite the sidecar. Timeout/resource ceilings, target capabilities and
snapshot/artifact formats remain closed. An explicit `--artifacts` path may
relocate bounded output, and `--retry`/`--repeat` can enable bounded campaigns
without editing TOML. An explicit `--timeout none` is accepted by the parser
for compatibility but rejected because every effective plan has a positive
wall-clock limit.

The closed wire field `policy.fail_fast` must be `false`. A supplied `true`
value is rejected with a usage error before selection or listing, including
`--allow-empty`. Tondo 0.1 has no global fail-fast policy; `testing.failNow`
terminates only the active node, as specified in Testing Spec section 8.9.

Unit tests cover defaults, both option spellings, complete option composition,
selectors/numbers/paths/globs/report collisions, explicit zero/one retry and
repeat rules, list/update incompatibilities, and unknown or positional input.
An integration test confirms the exit boundary for valid and invalid `test`
invocations.

## Closed worker inputs

The coordinator validates production and every independent test consumer before
selection, listing, or `--allow-empty`. Integration roots have distinct internal
package identities and retain their relative logical paths in visible test IDs.
It compiles all discovered test bodies through verified MIR and bytecode before
selection can omit any of them. `driver::compile` does not enter the VM, execute
suite setup or consume the runtime instruction budget.
The compiler consumes explicit source classes in discovery and lowering;
ordinary compilation classifies its input as production. Static tree validation
reports `E2001` for production test nodes and shares the compiler diagnostic
budget. Production imports of `std.testing` and public test declarations report
`E2003`. A project containing only conventional test sources binds an explicit
empty production set. It does not introduce a synthetic production source.

An empty selector requires `--allow-empty`. A shard receiving zero leaves from a
nonempty selection succeeds and emits its ordinary empty list or report. It
does not create workers or publish snapshot mutations.

The invocation captures exact project/lock documents, source/interface bytes,
the effective test plan and snapshot expectations. After selection and ordering,
the coordinator compiles each participation and takes its verified bytecode and
exact entry. All initial attempts, retries and repeats of that participation
reuse the same immutable encoded program in fresh processes.

The private `tondo-test-worker-input/2` protocol carries bytecode, the entry,
selection, source revision, target, limits and snapshot expectations through a
pipe. SHA-256 binds the whole payload. Workers receive no compilable source
graph and perform no parsing, resolution, type checking or lowering. They check
the payload identity, source revision, target and selection; the VM verifies
bytecode again before execution. This same-executable transport defines no
persistent artifact or cross-version bytecode ABI.

The transport allows at most 512 MiB and 1,024 JSON container levels. An iterative
depth check precedes deserialization on the explicit 8 MiB worker stack, allowing
recursive constants beyond the JSON parser's default depth while retaining a
closed limit. The encoder enforces both limits before an attempt starts. Input
writing, output draining and the worker deadline run concurrently so pipe
capacity cannot disable the timeout.

The worker receives the exact hashed payload length through `--input-bytes`.
It reads only that frame, leaving stdin open as the supervisor lease. A `C`
byte or lease closure requests cancellation. Ordinary hosted child commands
receive closed stdin unless connected to a declared pipeline, so they cannot
consume the private control stream. Worker-origin interruption uses a private
stderr notification and a final cleanup acknowledgement; neither is a public
complete report. See `test-interrupt.md` for grace, rollback and exit status.

Reports and lists receive the public digest computed by `TestInputPlan` from
the captured source/interface hashes, project and test-plan documents, snapshot
stores, and selected CODEOWNERS input. Reports bind their resource profile to
the declared test limits and effective job count and timeout. These fields are
computed from the invocation, rather than the metadata type's default values.
VM instruction and logical heap limits are taken from the closed test plan.

The executable regression removes the project directory after capture, then
runs its test and verifies a snapshot through the VM host. A changed expectation
fails; mutations of each transported input category fail identity validation.
Malformed bytecode is rejected even with a freshly computed payload hash.

Input and response JSON are serialized through a writer with a 512 MiB byte
budget; serialization stops before an over-budget write is admitted. The
response budget includes its final newline. Coordinator pipe readers retain
at most one 512 MiB response frame and 1 MiB of worker diagnostics, with one
extra byte used to detect overflow. These internal transport ceilings are
separate from per-node output and artifact budgets. An oversized or malformed
frame is an infrastructure failure and cannot publish partial test results.

The coordinator checks production once from the pinned bytes. It retains the
successful semantic snapshot, including private declarations, inferred types,
constants, bodies and its package graph. Consumer requests append test files
after the unchanged production source prefix. Resolution and HIR checking
consume only the added syntax; complete HIR, MIR and bytecode verification
still apply to the combined executable. Workers receive the compiled bytecode.
Unit overlays cannot add implementations or members to production types.

Explicit sidecars are reconciled against conventional discovery by class,
physical/logical paths, module, input and package before selection or worker
creation. General declared runtime providers and dev-dependency integration
remain open under the reopened testing tracker tasks. The hosted OS
interruption route and remaining containment/target evidence are described in
`test-interrupt.md`. This
integration alone does not close the testing gate.
