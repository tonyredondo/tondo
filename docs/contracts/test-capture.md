# Static suite-capture contract

**Status:** implemented for `UTEST-CAPTURE-001`

`tondo_compiler::test_capture` is the semantic boundary between the static
`suite`/`test` tree and future test-body checking/lowering. It consumes the
resolved suite bindings and uses supplied by the checker; it never executes a
setup, keeps a mutable environment, or consults the worker.

## Snapshot rule

A binding can cross from a suite setup to a descendant only when all of these
conditions hold:

- the binding belongs to an ancestor `suite` node;
- it is an immutable `let`, not `var`, `ref` or `mut`;
- the type facts are `Copy + Send + Share` and are all satisfied; and
- the type has no present or potential terminal obligation.

The descendant use must be an ordinary value observation. Shared/exclusive
loans, replaceable loans and moves are rejected. The source binding remains
owned by its suite; the plan contains one immutable snapshot slot per
`(target node, source binding)` pair.

Nested suites do not create an escape hatch. A use is accepted only when the
declaring suite appears on the target's parent chain. Constants, functions and
module declarations are resolved by name and do not enter this capture plan.

## Determinism and diagnostics

Uses are ordered by target identity, local identity, source span and access.
Capture slots are assigned in that order independently of input vector order.
`E2005` points at the descendant use and relates the suite binding declaration;
missing capabilities, terminal ownership, loans, moves and non-ancestor uses
all use that diagnostic. Existing tree warnings are preserved on a successful
plan and before capture errors on a rejected plan.

`CaptureTypeFacts::from_hir` is the only adapter from HIR capability and
terminal summaries. Missing facts are rejected instead of being treated as
safe. The resulting `TestCapturePlan` is host-free input for the later checker
and lowering stages.
