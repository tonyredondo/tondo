# Hierarchical test-suite lifecycle

**Status:** implemented for `UTEST-SUITE-001`

The public path is a single compiler-owned participation, not a coordinator
simulation. `test_backend::lower_participation` emits the selected suite tree
as generated Tondo source, that source traverses the normal HIR, MIR, bytecode
and verifier pipeline, and the VM invokes every setup, leaf and cleanup in one
fresh worker. Internal node-boundary operations create separate sealed
envelopes without exposing a context value to Tondo code.

`tondo_compiler::test_suite` remains the executable lifecycle model used to
check ordering and outcome rules independently. The CLI path must agree with
that model, but it does not execute it in place of compiled Tondo code.

## Participation and ordering

Only a selected leaf participates. A suite participates when at least one
selected descendant exists; unselected subtrees do not run setup, cleanup,
teardown, or bodies. Children are validated and traversed in canonical UTF-8
byte order. Setup runs once on entry, descendants run outside-in, cleanup runs
in LIFO order, and teardown runs inside-out after all selected descendants.

The setup lexical scope encloses its selected descendants, so closures capture
the permitted setup values through ordinary language semantics. A leaf panic
is contained only after its language cleanup has completed; the VM then resumes
the suite closure and can execute siblings. Nested suites use the same boundary
recursively. A new repeat iteration or retry participation launches another
worker and recompiles only from the immutable artifact, so no heap, guard,
envelope, task, snapshot update, or captured value is reused.

## Outcomes and blocking

Setup failures block only their subtree and do not prevent sibling roots from
running. A skipped setup produces `skipped` for the suite and `blocked-skip` for
descendants; every other setup failure produces `blocked-setup`. Cleanup is
always attempted after a participating setup, including a failed or skipped
setup. A cleanup failure takes precedence over the setup outcome. A teardown
failure changes only the suite result; already emitted descendant results are
not rewritten.

Actions map to the existing `AttemptStatus` vocabulary. The closest failing or
skipped ancestor is recorded in `blocked_by`, including nested suites. A suite
retry is one participation containing all selected descendants in their stable
order; it never retries only the originally failing leaf while reusing setup.

The compiler-generated names and operations are accepted only for
`GeneratedTesting` sources. User test identifiers beginning with `__tondo` are
rejected, and production artifacts cannot forge the internal lifecycle ABI.
`defer` is driven by the ordinary VM unwind path; there is no `afterAll`
hook or host-side substitute for language cleanup.

## Executable evidence

- Compiler tests prove generated source traverses the common pipeline, setup
  bindings are visible to descendants, leaf panic does not suppress siblings,
  and the internal namespace cannot be forged.
- CLI acceptance tests prove setup and teardown phase attribution,
  `blocked-setup`, `skipped`/`blocked-skip`, closest-ancestor causality,
  sibling continuation, one setup per participation, LIFO cleanup and a fresh
  whole-suite retry.
- The testing acceptance project proves suite logs remain on the suite attempt
  while each leaf retains only its own envelope events.
