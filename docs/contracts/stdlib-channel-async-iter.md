# `std.channel` AsyncIterator adaptation

`STD-CHANNEL-ASYNC-ITER-001` closes the compiler-owned adaptation from
`std.channel.Receiver[T]` to the already specified `AsyncIterator[T]` protocol.
The machine-readable contract is
[`testing/stdlib-channel-async-iter.json`](../../testing/stdlib-channel-async-iter.json).
The parent channel contract remains
[`testing/stdlib-channel.json`](../../testing/stdlib-channel.json); channel
model/fuzz, performance, conformance and executable documentation are separate
tracker leaves, with `STD-CHANNEL-DOC-001` now closed.

This leaf verifies the hosted VM path over the existing scheduler-owned channel
state. It does not add a stream type, a `for await` spelling, a channel-specific
materializer, a public poller, or generic native AOT lowering.

## Source surface

When the payload satisfies `Discard`, a receiver selects the sealed prelude
protocol:

```tondo
fn next(mut self): T? suspends
fn close(iterator: Self) suspends

for item in receiver {
    use(item)
}
```

The ordinary `for` form is the only spelling. If the source also has a
synchronous iterator, the synchronous protocol keeps precedence. Suspension is
inferred from the body; callers do not write `for await`. `AsyncIterator.collect`
continues to be the generic extension from `std.async`, not a channel method.
The extension may materialize a bounded `Array[T]`; direct channel iteration
never creates an array intermediate.

The checker recognizes the nominal bootstrap identity
`toolchain:std:0.1-bootstrap::channel::type::Receiver[T]` and proves
`T: Discard` before accepting the protocol. A payload such as
`Receiver[Join[Int, Never]]` is rejected with `E1105`. Affine payloads therefore
use the explicit API:

```tondo
let item = receiver.receive()
let pending = receiver.close()
```

That path returns pending values instead of silently discarding them.

## Iteration and ownership semantics

Each `next` returns at most one committed value. A bounded channel keeps its
existing backpressure and FIFO waiter order; a receiver reaches `none` only
after the last sender is closed and the buffer is drained. The compiler reserves
an allocation-free consuming close in a private cleanup scope before the first
poll. Normal exhaustion, `break`, `return`, propagation, panic and cancellation
close that endpoint without draining unrelated user defers. Pending values are
discarded only after the static `Discard` proof.

The `collect(limit:)` lowering uses the same private `next` and consuming close
witnesses. Before capacity allocation or the first poll it reserves that close;
the direct sealed route also marks the endpoint's discardable iterator view.
The reserved close applies even at `limit: 0` and while a `next` is parked.
Direct and spawned generic collection use the same MIR loop and cleanup rules.
The extension consumes its source
and closes it on every outcome, including zero, exhaustion and a reached limit.
Access to another receiver requires an explicit `fork` before collecting.
Closing the collected endpoint preserves the buffer for that distinct receiver;
its consuming public close returns any remaining values.

The marker is endpoint-local and private. Calling public `receive` does not set
it, so a manually consumed affine channel still fails cleanup until the caller
uses `Receiver.close` and receives the pending values.

## Hosted runtime evidence

The compiler-owned host names are not source-level APIs:

- `std.channel.Receiver.__asyncIteratorNext[T]` has the `mut self` protocol
  shape but reuses the existing `receive` scheduler waiter and channel FIFO.
- `std.channel.Receiver.__asyncIteratorAdopt[T]` is synchronous and marks the
  endpoint before generic `collect` enters its control flow.
- `std.channel.Receiver.__asyncIteratorClose[T]` consumes the endpoint and
  retires it without allocating a drain array, after the payload Discard proof.

The hosted VM stores the marker in the endpoint table. Cancellation unregisters
the waiter before commit; cleanup then closes the adopted receiver and discards
only `T: Discard` values. The existing private native channel bridge is
unchanged, and this leaf deliberately records `native_aot_lowering` as
`not-claimed`.

The fixture
[`tests/runtime/m11-std-channel-async-iter-001.to`](../../tests/runtime/m11-std-channel-async-iter-001.to)
covers buffered drain, early `break`, generic `collect(limit: 2)`, explicit
receiver forks and their post-limit close, and `collect(limit: 0)`. The negative
fixture
[`tests/compile-fail/m11-std-channel-async-iter-discard.to`](../../tests/compile-fail/m11-std-channel-async-iter-discard.to)
pins the affine rejection to `E1105`. Host tests cover waiter reuse, adoption,
manual receive isolation, cancellation and terminal cleanup.

Contract and negative-case runners are
`scripts/stdlib-channel-async-iter-check.sh` and
`scripts/stdlib-channel-async-iter-test.sh`; the implementation runner is
`scripts/stdlib-channel-async-iter.sh`. Their report is written to
`target/reliability/evidence/stdlib-channel-async-iter.json` and includes the
source revision and fixture hashes.
