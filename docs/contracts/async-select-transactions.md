# Hosted selection transactions

`ASYNC-SELECT-ATOMIC-001` repairs the common hosted protocol used by source
`selectable` adapters, network readiness, channels, timers, one-shot waiters,
group completions, actors and Join waits. Its promotion remains pending until
the current source passes the complete repository gate and publication CI.

Every selection receives a private registration identity. Preparation retains
affine argument and callable owners. Source adapters receive private aggregate
snapshots; shared loans retain their original reservation. Registration must
finish before a different operation can pair with a rendezvous arm. A send and
receive from the same selection cannot pair with each other.

Readiness does not dequeue a channel value, remove a group completion, consume
a waiter, deliver a datagram or advance a source continuation. A successful
rendezvous jointly admits both typed results and claims both selection owners.
Each owner must observe that claim before considering another alternative.
Winner commitment transfers the original affine places once. Losing calls and
`else` release reservations without consuming those places or results.

Source adapters pause at their first selectable operation or Join await.
A nested pure selectable return is also a checkpoint: its caller continues
only after selection. Before that checkpoint, `E1614` requires reversible
preparation. The checker proves local value construction, scalar calculations,
local replacements, known value-only host setup and inspected pure helpers.
It keeps a path uncommitted when a conditional, short-circuit expression or
loop can bypass the checkpoint. Borrowed writes, unproved dynamic calls,
ordinary suspension and externally visible host effects cannot establish this
proof. A pure defer is inspected without granting a commit checkpoint.
Structured panic remains observable and precedes ready value arms.

The typed-HIR verifier repeats the source proof. The bytecode verifier checks
the reachable uncommitted control-flow graph independently, including helper
and formatting dispatch, borrowed parameters and closure environments. A
function flag alone cannot establish the contract of an executable artifact.

Cancellation before commitment preserves caller owners. Cancellation after a
peer has committed a rendezvous executes the transferred owners' terminal
fallbacks. The host retires pending reactor futures before publishing cancelled
results. Timer ownership is transferred only after an admitted completion.
Waiter and group result allocation failure preserves their original completion
for retry; group result construction precedes child removal.

The executable regressions are listed in
[`testing/async-select-transactions.json`](../../testing/async-select-transactions.json).
They include mixed network/channel selections, source forwarders, aggregate
and closure rollback, paired selectors, self-pair refusal, affine sends,
timers, waiters, groups, Join adapters, actor adapters, pure continuations,
forced-winner cancellation, rejected bytecode and result-memory refusal.
The independent finite rendezvous model runs 4,096 deterministic seeds.

The joint selection/network quality campaign passes 2,726 Rust tests and
196 layer observations. It covers 300,345 of 328,050 lines (91.554641%),
87.760902% of functions and 89.941218% of regions, preserves every global and
risk-scope 80% floor, and catches all six critical mutants without survivors,
timeouts or unviable cases. The current ratchet binds both reports to source
tree `0e3204de1b7fd4c9a127dc5f4f8c9b4651b2c064be14895f610565f3442bbaa7`.
The complete functional gate and exact-source publication CI still precede
tracker closure.

This contract promotes hosted VM execution only. It does not establish native
ABI, Cranelift AOT, performance, physical-thread equivalence or a release.
Historical selection campaigns retain their original bounded scope.
