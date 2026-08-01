# Isolated repeat iterations

`tondo-compiler::test_repeat` implements `--repeat N`.  `N` defaults to one,
must be finite and positive, and each iteration executes the complete selected
program set.  The same immutable compiled `LeafProgram` values are passed to
the runner; there is no second compilation or implicit retry policy.

Iterations are strictly sequential.  A call to `RuntimeRunner::run` returns
and revokes its workers, resources, envelopes, virtual domains, output
buffers, and budgets before the next call starts.  `--jobs` still controls
parallelism inside one iteration, but two iterations never overlap.  Every
attempt records `iteration: 1..N`, `round: 0`, and `unit: null`, plus the fresh
worker identity and complete envelope report.

The immutable `RepeatContext` copies selection, `execution_plan`, shard,
target, inputs identity, seed, order, capabilities, limits, and artifact/
snapshot store identities into the report.  Its canonical bytes are stable
regardless of capability input order.  Virtual time starts at zero in every
iteration because it belongs to the fresh envelope.

Repeat rejects retry, `allow-flaky`, list mode, and snapshot update.  A leaf is
`all_passed` only when every iteration passed; one non-passed iteration keeps
the process exit unsuccessful even when later iterations pass.  `N = 1` is the
ordinary single-attempt policy and does not create a special flaky mode.

