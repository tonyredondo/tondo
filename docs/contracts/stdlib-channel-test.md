# std.channel testing contract

This leaf closes STD-CHANNEL-TEST-001 for the draft/0.1 channel contract.
The machine-readable record is
[testing/stdlib-channel-test.json](../../testing/stdlib-channel-test.json).
It is a reliability boundary, not a public-library release and not evidence
that generic channel AOT lowering exists.

## Scope and target

The reference model in
[crates/tondo-reliability/src/channel_model.rs](../../crates/tondo-reliability/src/channel_model.rs)
is deliberately independent of the hosted scheduler and the private native
bridge. It represents a channel as a bounded state machine with explicit
sender and receiver endpoint sets, FIFO waiter queues, a payload ownership
ledger, and one wakeup record per completed waiter. The model is bounded so
that a failed or cancelled transition can be exhaustively inspected without
turning the reliability lane into an unbounded resource consumer.

The target is
reference-model-and-hosted-native-regression-boundary. It covers the
compiler-hosted VM and the existing private native ABI through their existing
regression tests. It does not promote std.channel symbols, change the runtime
ABI, or claim Cranelift/AOT execution.

The fixed limits are:

- bounded capacity: 0 through 64;
- explicit unbounded queue: at most 64 committed values in the model;
- live sender and receiver handles: at most 128;
- fuzz input: at most 4096 bytes and 512 transitions.

## Model laws

The model and its integration suite cover rendezvous (bounded(0)), positive
capacity backpressure, explicit unbounded resource limits, endpoint limits and
fork identity, multiple producers and consumers, FIFO registration, and
last-sender/last-receiver state transitions. It also checks simultaneous
readiness, rotating select ties, prepare/rollback non-mutation, else
non-mutation, one-arm commit, cancellation before commit, close wakeups,
terminal FIFO drain, stale handles, and structured cleanup.

Every payload is an affine token. The ledger records whether it belongs to the
caller, the committed queue, a pending send, or a completed send/receive
result. A failed, cancelled, or losing operation must leave the token
recoverable; a receive commit must remove exactly one queue value. The
invariant checker rejects duplicated tokens, lost ownership, stale waiters,
more than one wakeup per waiter, and queue growth beyond the declared limit.

Receiver.close cancels waiters owned by the closed receiver. Closing the last
receiver wakes pending senders with their values intact and returns committed
values in FIFO order. Closing a sender completes its own pending sends with
their values intact; the last sender only terminates receive after the queue
has drained. cleanup cancels pending operations, consumes endpoints, polls
completed results, and proves that no model container retains a payload.

## Tests and fuzzing

[crates/tondo-reliability/tests/channel_models.rs](../../crates/tondo-reliability/tests/channel_models.rs)
contains focused positive, negative, edge, close/cancel and select cases plus
4096 deterministic seed replays. Each seed is replayed twice and compared
byte-for-byte at the final snapshot. The same leaf reruns the existing
compiler-host and native-runtime channel regressions; those tests remain
evidence for hosted/native boundaries only.

[fuzz/fuzz_targets/stdlib_channel.rs](../../fuzz/fuzz_targets/stdlib_channel.rs)
includes the model with a panic-catching libFuzzer entry point. It replays each
input twice, checks the summary and cleanup invariants, and caps input length
and transitions. The checked-in corpus seed is
[fuzz/corpus/stdlib_channel/seed](../../fuzz/corpus/stdlib_channel/seed).
The observed smoke command is:

~~~text
TONDO_CHANNEL_FUZZ_RUNS=128 scripts/stdlib-channel-fuzz.sh
~~~

It completed 128 runs with seed 4104 and no panic, ownership violation or
cleanup residue. The fuzz runner is intentionally separate from the native AOT
lane; it proves bounded model robustness, not native code generation.

The contract checker is
[scripts/stdlib-channel-test-check.sh](../../scripts/stdlib-channel-test-check.sh)
and its negative/regression runner is
[scripts/stdlib-channel-test-test.sh](../../scripts/stdlib-channel-test-test.sh).
The parent channel contract and the async-iterator leaf link this record so a
stale testing frontier fails the repository gate.

## Promotion boundary

STD-CHANNEL-TEST-001 is complete for the model, regression suite and fuzz
smoke. STD-CHANNEL-PERF-001 is the next block. Conformance and documentation
remain separate leaves. No test result here changes the parent contract's
native_aot_lowering: not-claimed or public_api_promoted: false decisions.
