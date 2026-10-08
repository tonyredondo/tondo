# Channel endpoint ownership

`STD-CHANNEL-OWNERSHIP-001` repairs a compiler ownership gap required by the
generic logger. The implementation is in progress; quality and publication
evidence must pass before promotion. The register is
[stdlib-channel-ownership.json](../../testing/stdlib-channel-ownership.json).

The bootstrap `Sender[T]` and `Receiver[T]` declarations use a nominal Unit
placeholder. Deriving capabilities from that placeholder incorrectly granted
Copy, equality and keys, and removed the receiver's terminal obligation.
An enclosing record then copied its old state into defer instead of reserving
the live affine owner. The repair binds the existing endpoint contract to the
exact standard package, module, type namespace and one generic parameter.
User declarations named Sender or Receiver retain ordinary structural rules.

Neither endpoint is Copy, Equatable or Key. Sender is Discard; Receiver is
terminal and cannot be abandoned on a normal exit. Both endpoints are Send and
Share when their payload is Send. These properties propagate through records,
generics, options, results, arrays, enums and closure captures. HIR and the
independent bytecode verifier derive the rules rather than trusting annotations.

An aggregate can move an unguarded sender and a guarded receiver in one
assignment. Its consecutive cleanup transitions may include no-op disarms for
unguarded operands. Each guarded operand still needs its exact transfer before
another instruction or a terminator; a disarm for another owner cannot satisfy
that obligation. MIR and bytecode mutation tests omit, misdirect and replace
the receiver transition and require rejection.

Ordinary execution collects discarded endpoints even without a test memory
budget. Fork preserves the original endpoint and creates another owner; only
the last sender closes input. Ordinary receiver discard refuses pending values.
Terminal unwind retires a receiver without materializing a drain array, then
root reconciliation preserves reachable values. A blocking worker forwards
that terminal operation to its provider. Explicit close still returns pending
values according to the parent channel contract.

The public projects exercise latest-state affine defer and a generic sink slot
shared by two structured tasks. The slot returns its owner through deferred
cleanup and observes both writes. It is a channel composition regression,
not an implementation or promotion of `std.log`.

Every explicit AsyncIterator implementation supplies the consuming
`close(iterator: Self) suspends` method. The compiler reserves that close in a
private scope, retaining the final cursor state even when the cursor is Copy.
Source defers keep their existing contextual Copy snapshot behavior. Direct
and spawned collect share the same ordinary MIR body and guarded cleanup;
the hosted provider's sealed receiver witness implements the same protocol.

The sealed receiver iterator reserves its consuming close in a private scope.
It closes on exhaustion, break, return, propagation and unwind without draining
outer user defers. `collect` consumes and closes on every outcome, including
negative limits and zero; retained access uses an explicit receiver fork.
The old Copy-dependent fixtures now destructure complete endpoint pairs and
preserve their existing diagnostics and stdout sidecars.

Focused tests cover forbidden capabilities, payload transfer bounds, owned
shapes, reuse, abandonment, explicit fork/close, provider collection with and
without test accounting, recoverable errors, panic and structured cancellation.
`scripts/stdlib-channel-ownership-check.sh` executes the focused Rust cases and
both ordinary hosted projects; its contract tests reject policy drift.

This block qualifies public hosted VM ownership on Linux x86_64. It adds no
dependency or language syntax and makes no native Tondo AOT, new native ABI,
performance, RSS, OS allocation or release claim. Previous bounded channel
transport, model and private native ABI evidence keep their own scopes.
