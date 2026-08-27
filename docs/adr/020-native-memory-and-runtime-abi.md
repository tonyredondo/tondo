# ADR-020: Native memory policy and private runtime ABI

**Status:** Accepted for the unpublished 0.1 development line  
**Supersedes:** the open DEC-013/DEC-014 boundary  
**Contracts:** [`native-memory.md`](../contracts/native-memory.md),
[`native-abi.md`](../contracts/native-abi.md)

## Context

The bootstrap VM already defines the observable ownership, cleanup, async and
diagnostic semantics. A native backend must be able to use a faster memory
strategy without turning those semantics into a public object layout or a
second set of APIs. The backend-evaluation adapter now has a real verified
scalar call boundary, so the memory and compiler/runtime contract can be fixed
before managed lowering begins.

## Decision

The native runtime uses hybrid ARC with cycle collection. Non-shared values use
non-atomic counts; values crossing `Send`/`Share` use atomic counts. Trial
deletion runs at quiescence and under pressure, weak edges are runtime-owned,
and all stack/task/thread/async-frame/host-handle roots are published before a
possible suspension. Affine resources are released by MIR cleanup and are
never finalized by the collector. Cancellation reaches a terminal task only
after cleanup drains. COW is an unobservable uniqueness optimization.

The private ABI is `tondo-native-runtime-abi/1`. It has verified direct calls,
runtime result records, explicit retain/release and resource-terminal edges,
normal-unwind/abort, frame/task/waker registration, diagnostic identities and
opaque capability-indexed host handles. The compiler/runtime are the only
consumers. No FFI ABI, user layout, allocator, pointer or mangled name is
stable by this decision.

`ARC-001` and `ARC-002` now provide the complete memory side of this decision in
the private native runtime: checked local/shared counts, payload-edge
transfer, root and scope cleanup, select registration ownership, worker
terminal pins, trial-deletion cycle reclamation, and linearizable weak
upgrades. Native diagnostic parity remains the separate `DIAG-NATIVE-001`
frontier.

## Consequences

- The VM remains the value/error/ordering/ownership oracle for native tests.
- Scalar direct calls can be measured now without implying managed-value
  support; unknown targets and unsupported protocols trap fail-closed.
- Cleanup, ownership and async lowering must consume these contracts and prove
  parity before N1; a typed contract or a report alone cannot close those
  leaves.
- A future public FFI or alternate memory strategy requires a new ADR and a
  versioned contract rather than mutating this boundary.

The canonical machine records and negative checks are the source of truth for
the closed decision; this ADR explains the rationale and scope.
