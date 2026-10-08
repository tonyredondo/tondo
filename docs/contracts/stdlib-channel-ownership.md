# Channel endpoint ownership

`STD-CHANNEL-OWNERSHIP-001` repairs a compiler ownership gap required by the
generic logger. Its public hosted ownership boundary is verified on Linux
x86_64 by the source-bound quality and exact-checkout evidence below. The register is
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
Source defers keep their existing contextual Copy snapshot behavior. Consuming
iteration transfers an existing exact caller guard to the protocol close;
it does not execute both cleanups. Ordinary defers cannot replace another
explicit guard. Direct and spawned collect share the same MIR loop lowering
and guarded cleanup;
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

## Verified evidence

The final source checkpoint is `0227fb4e`; its immutable quality tree is
`16ae4881714f75dbfedf0052dbf4f51e62ca7a91849cee4123755bf0621b0145`
(1,415 source inputs). The fresh workspace capture passes 2,846 Rust tests in
77 suites and records 196 executable layer observations. It measures:

| Dimension | Covered / total | Percent |
|---|---|---|
| Lines | 304,766 / 332,726 | 91.596689% |
| Functions | 20,032 / 22,798 | 87.867357% |
| Regions | 447,922 / 497,732 | 89.992606% |

Every global and risk-scope dimension passes its 80% floor. All six selected
critical mutants are caught, with a passing unmutated baseline and no missed,
timed-out or unviable mutants. Coverage and mutation provenance are identical
before and after their respective captures. Supported ratchet generation and
verification pass all five draft case layers.

All 360 canonical functional steps pass as a composed local proof: eight on
`25fb726d`, 340 on `3afaa136`, eight on `fd01b42d` and four on `02eae742`.
Documentation fence offsets, the expanded async conformance inventory and the
select suite pin were corrected after their checkers refused stale metadata.
The shell inventory correction required a fresh quality capture; the final
manifest pin preserves the measured source. The final steps pass all 206
draft cases and 96 exact select observations. S1A remains pending with 115
open prerequisites, and its refusal is verified. This is a composed proof,
not an uninterrupted local run.

Publication CI verifies the actual checkout
`02eae742622312e53e4d1691c04e64d6c1c97f94` in
[run 37826292533](https://github.com/tonyredondo/tondo/actions/runs/37826292533),
strict Linux x86_64 job `113479901934`. It executes all 360 steps and 2,846
Rust tests in 77 suites without error annotations. Paginated checks, statuses,
runs, open main PRs and the remote main ref remain stable for 94 seconds.
Portable and independent fuzz jobs are expected normal-push skips; their
platform coverage is not established by this run.

This block qualifies public hosted VM ownership on Linux x86_64. It adds no
dependency or language syntax and makes no native Tondo AOT, new native ABI,
performance, RSS, OS allocation or release claim. Previous bounded channel
transport, model and private native ABI evidence keep their own scopes.
