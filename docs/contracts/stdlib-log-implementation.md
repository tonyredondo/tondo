# Structured logging core implementation

`STD-LOG-IMPL-001` implements 19 public operations and the three-method
`LogSink` protocol from the locked [logging contract](stdlib-log.md). The
authoritative boundary is [stdlib-log-implementation.json](../../testing/stdlib-log-implementation.json).
ConsoleSink and FileSink constructors and their privileged providers remain
owned by `STD-LOG-HOST-001`. Importing the core requires no capability and
performs no provider read. Timestamps are caller-supplied optional UTC values.

The public API is ordinary Tondo source selected by the compiler as the sealed
standard module. Only two private pure helpers enter the Rust scalar kernel:
Fields.__withField validates an immutable replacement; LogEvent.__create
validates the complete event. External calls and function references to these
helpers are rejected. Public getters, option constructors, filtering and
generic sink dispatch execute ordinary Tondo functions.

Fields have unique nonempty UTF-8 keys without controls or NUL. Insertion
validates before replacing the builder and preserves earlier logical copies.
Events snapshot their fields and reject nonfinite floats and invalid targets.
Validation counts eight logical bytes for every value node, including empty
scalars, plus key and payload bytes; nested fields share the event limit.
The Rust formatter uses iterative frames and emits either one escaped Text
line in insertion order or canonical JsonLines with UTF-8-sorted field keys.
Bytes use canonical Base64; redaction is an explicit value. Formatting is
complete before a provider writes. No locale, ambient clock or secret lookup
participates.

The hosted construction envelope reserves 512 bytes plus four times the
retained argument bytes and every referenced Bytes payload length. It includes
validation copies. Typed VM reply admission precedes publication; transport
shrinks to the exact reply. A refusal publishes no replacement and releases
temporary charges. These are logical bounds, not RSS or OS allocator counts.
Arrays are checked against the remaining node budget before reserving their
element buffer.

Logger owns one logical sink value in a bounded channel slot. A guarded lease
serializes writes and flushes and restores ownership on cancellation and panic.
Custom sinks retain their structural capabilities; Logger is always affine.
The protocol uses the existing plain trait syntax, with `LogSink + Share`
bounds on Logger and its constructor. Suspendible methods imply Send. Close
consumes the logger and the sink, drains the slot and reports the first close
error after completing all owned cleanup. Filtered events return Filtered
without acquiring the slot or invoking the sink. Sink backpressure remains
observable through Accepted, Dropped or Backpressure.

A write or flush Io error marks the sink terminal, closes it once and wakes
waiting callers with Closed. The originating Io remains visible; a physical
prefix is allowed after successful admission. A close error is retained for
the consuming logger close. Other recoverable errors return the lease to its
owner. No retry, rotation, background thread or global logger is introduced.

Implementation testing exposed two shared prerequisites. A root importing a
dependency through two acyclic paths previously corrupted resolver DFS finish
order and could panic while diagnosing a nonexistent cycle. Iterator frames
now preserve postorder; an independent transitive-closure oracle checks all
65,536 four-node directed graphs and real-cycle diagnostics remain covered.
A fatal ordinary VM heap error also left a terminal channel endpoint live.
Whole-engine failure now retires owned provider work and values without
importing another response or executing user callbacks. This does not turn
fatal OOM into a catchable language panic or guarantee user defers after OOM.

The ordinary project exercises every core operation, immutable snapshots,
explicit timestamps, custom value and affine sinks, concurrency, all three
backpressure policies, retained caller events, terminal Io and close errors.
Focused hosted tests cover malformed private ABI payloads, depth and node
limits, joint admission, cancellation and fatal resource retirement. Compiler
tests enforce private fields/helpers, ownership and generic bounds.

Source implementation and focused verification are in progress. Consolidated
quality, the complete repository gate and exact-source publication CI remain
required before promotion. Independent logging model/fuzz, performance,
common conformance and user documentation remain separate owner blocks.
Native Tondo logging ABI and AOT execution are not implemented or promoted.

Run `scripts/stdlib-log-implementation-check.sh` through
`scripts/test-process-scope.sh` inside an explicit delegated OS scope. Its
fatal-retirement regression starts only its own bounded process fixtures and
verifies that the worker and child are reaped. `--contract-only` performs no
process execution; the separate implementation-test script checks refusals
of surface, lifecycle, capability, limit and source-provenance drift.
