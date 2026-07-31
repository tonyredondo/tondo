# Hierarchical test-suite lifecycle

**Status:** implemented for `UTEST-SUITE-001`

`tondo_compiler::test_suite` is the coordinator-side lifecycle machine for the
testing draft. It does not execute a second compiler or share leaf workers:
each call to `SuiteRunner::run` creates a fresh participation context and a
new run ID. The same plan can therefore be used for retries without reusing
setup state, guards, snapshots, or resources.

## Participation and ordering

Only a selected leaf participates. A suite participates when at least one
selected descendant exists; unselected subtrees do not run setup, cleanup,
teardown, or bodies. Children are validated and traversed in canonical UTF-8
byte order. Setup runs once on entry, descendants run outside-in, cleanup runs
in LIFO order, and teardown runs inside-out after all selected descendants.

The setup context is scoped to the participating tree. Descendants can observe
values captured by setup, while a new runner invocation receives a distinct
context and run ID. Context state is cleared at the lifecycle boundary.

## Outcomes and blocking

Setup failures block only their subtree and do not prevent sibling roots from
running. A skipped setup produces `skipped` for the suite and `blocked-skip` for
descendants; every other setup failure produces `blocked-setup`. Cleanup is
always attempted after a participating setup, including a failed or skipped
setup. A cleanup failure takes precedence over the setup outcome. A teardown
failure changes only the suite result; already emitted descendant results are
not rewritten.

Actions are panic-contained and map to the existing `AttemptStatus` vocabulary.
The API models an already-lowered async action, so a future executor can drive
`defer await` to completion inside teardown without adding an `afterAll` hook or
sharing worker state.

