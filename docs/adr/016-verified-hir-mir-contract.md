# ADR-016: Verify typed HIR and make effects explicit in MIR

**Status:** accepted

## Context

Typed HIR has enough source structure to report semantic errors, while
bytecode needs an explicit control-flow and storage model. Ownership, loans,
cleanup, panic unwinding, and suspension arrive at different milestones, but
placing any of them only in backend metadata would make their ordering and
correctness impossible to verify independently.

Partial semantic snapshots are also useful to tooling. Treating every snapshot
as backend input would allow recovery types, unresolved associations, or broken
arena references to cross a phase boundary intended for accepted programs.

## Decision

Only a complete typed HIR that passes `verify_typed_hir` may enter MIR
lowering. Recovery HIR remains a tooling artifact and is never executable.

MIR is a typed control-flow graph and owns the following facts:

- logical moves and copies are distinct operand forms;
- loans are explicit operations over a resolved place and carry their kind and
  inferred region identity;
- every operation capable of leaving its normal path has an explicit normal
  successor and cleanup/unwind successor where applicable;
- cleanup code lives in ordinary, marked MIR blocks and is not synthesized by
  bytecode generation;
- suspension is a terminator with explicit resume, cancellation, and unwind
  successors; and
- source spans remain attached to locals, statements, and terminators.

M3 creates the CFG and cleanup-capable edge shape even while cleanup lists are
empty. M5 classifies ownership, inserts loans and real cleanup actions. M7 adds
suspension terminators and transforms live locals into frame state. Each of
those transformations produces MIR that must pass the same structural
verifier plus the invariants introduced by that phase.

## Consequences

The backend never reinterprets AST/HIR control flow, guesses a move, invents a
loan lifetime, or derives cleanup from types. MIR is somewhat more verbose,
but bytecode verification, the VM, and a future native backend consume the same
explicit semantics. Adding ownership or async later extends verified MIR
invariants instead of replacing its CFG.
