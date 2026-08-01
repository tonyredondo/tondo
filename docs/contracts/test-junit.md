# JUnit XML projection

**Status:** implemented for `UTEST-JUNIT-001`

`tondo_compiler::test_junit` is the operational CI projection of the canonical
`TestReport`. JSON remains the lossless, reproducible representation; this
module adds only the XML 1.0 envelope and optional monotonic durations that CI
consumers expect. It never reruns tests or reconstructs aggregate state.

## Format and topology

The exporter emits `tondo-junit-report-0.1/4` as UTF-8 XML with no BOM, DTD,
external entity, or extra processing instruction. The declaration is followed
by LF-delimited output with one final LF. The root is `testsuites`. Tondo suites
are flat `testsuite` elements, and top-level tests are grouped by
`package::source-class::module`; parent IDs are retained in `tondo.parent`.
Each leaf is one aggregate `testcase`, never one testcase per attempt.

An empty execution plan emits the `@tondo-plan` suite with zero cases and
`tondo.synthetic=empty-plan`, while still carrying the execution metadata.
Suite setup/teardown failures produce one synthetic `@setup`/`@teardown` case,
and a `flaky-pass` suite produces one synthetic `@flaky` case. Synthetic cases
are ordered before leaves (`@setup`, `@flaky`, leaves by ID, `@teardown`).

## Outcomes and preserved evidence

Passed leaves have no outcome child. Skips and setup blocks use `<skipped>`;
test failures use `<failure>`; resource, timeout, and infrastructure failures
use `<error>`. A flaky pass is a `tondo.flaky-pass` failure unless the report
policy explicitly allows flaky results. A repeat with any non-passing attempt
is never silently green: when its aggregate has no red outcome, the exporter
uses `tondo.repeat-instability` with the normative message
`repeat observed a non-passing attempt`.

Every emitted node retains ordered `tondo.*` properties for identity, parent,
source class, status, decisive attempt, all attempts, owners, source,
artifacts, snapshots, virtual-time observations, and the execution metadata
(selection, inputs, ownership, shard/order/seed, retry/repeat, stores, policy,
limits, plan, and summary). Artifact bytes and complete snapshot values are
never embedded. `system-out` and `system-err` contain only the decisive
attempt's streams; previous attempts remain in `tondo.attempts`.

The JUnit `tests`, `failures`, `errors`, `skipped`, and `time` attributes count
the cases actually emitted, including synthetic cases. Durations are supplied
as optional per-node, per-attempt nanoseconds and are summed with checked
arithmetic; the serialized value is decimal seconds with at most nine
fractional digits. Unknown nodes, overlong timing vectors, and overflow are
rejected.

XML scalar escaping covers attributes and text, including the XML 1.0 control
range. Unsupported scalars are rendered as visible `\u{HEX}` text while
structured JSON properties retain the original value. Output construction is
deterministic and uses no XML dependency or external parser state.
