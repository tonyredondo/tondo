# `std.cbor` scalar-kernel performance

`STD-CBOR-PERF-001` defines the target-qualified Rust-kernel baseline in
[`testing/stdlib-cbor-performance.json`](../../testing/stdlib-cbor-performance.json).
The route is `scalar-fixed-target`, backend `rust-stdlib-kernel`, target
`x86_64-unknown-linux-gnu`, and the unoptimized Rust `test` profile. This is
not a hosted VM measurement or a native Tondo benchmark. Public compiler API,
production VM bridge, native ABI/AOT, SIMD, multiversion dispatch and code-size
claims remain absent. `native_live_handles` is unmeasured and must be `null`.

The initial register is `measurement-ready`. It becomes
`verified-stdlib-kernel-baseline` only after the clean-source capture and the
owner's required functional and quality checks pass. A ready register or a
development report cannot establish that closure.

## Protocol and fixtures

The probe is `cbor_performance_probe` in
[`crates/tondo-stdlib/tests/cbor_performance.rs`](../../crates/tondo-stdlib/tests/cbor_performance.rs).
The runner first executes the independent bounded CBOR model suite, then the
probe's exact fixture, terminal-lifecycle and batch tests. Fixtures are authored
from the independent wire model; expected normal/deterministic bytes and values
never come solely from the production codec. Indefinite events and full errors,
including byte spans and paths, are independently specified before timing.

Each of three independent processes executes three warmup batches and nine
measured batches per workload. A batch contains sixteen operations. `Instant`
provides monotonic timing. Every measured result retains its stable process
ordinal and repetition ordinal; all 27 samples survive, including outliers.
Median, P95 and P99 use nearest rank. With 27 samples P99 is the maximum;
this bounded distribution is not a production tail-latency guarantee.
Exact warmup and post-sample comparisons are outside latency. The loop's
status assertion, `black_box` and result destruction stay inside latency.

The fifteen workloads cover the actual routes with five bounded documents:

| Family | Workloads | Boundary |
| --- | ---: | --- |
| Materialized parse | 4 | Arbitrary keys; three/twelve nested row maps; Unicode; indefinite text/bytes, tag and float |
| Borrowed encoded-byte view | 1 | Validation of the twelve-row input and borrowed input ownership |
| Ordinary encode | 1 | The same twelve-row value and ordinary float widths |
| Deterministic encode | 2 | Nested maps; heterogeneous keys, shortest floats, signed zero, NaN, tags, integer extremes and undefined |
| Reader/writer events | 2 | Four indefinite chunks, fourteen exact events, buffered I/O and terminal finish |
| Bounded rejection | 5 | Depth, scalar and event limits; malformed simple value; normalized float-key collision |

The largest document has 1,395 bytes and 109 nodes. These fixtures remain
within the independent model's 4,096-byte, 128-node, depth-eight and 96-byte
per-scalar domain. They do not measure the full kernel limits or large
production documents. The deterministic heterogeneous-key fixture combines
key sorting and float normalization without adding a redundant route.

`parse_view` validates by materializing and dropping a value. It is not a
zero-allocation parser. `from_reader` buffers input, scans owned events, then
`next` clones event payloads. The writer buffers encoded bytes and copies them
to its `Vec` sink at `finish`. These costs belong to the measured draft routes.

## Resource model and provenance

Latency is measured; resource counters are a declared logical model.
`bytes_copied` counts selected payload transport per batch: decoded payload
for parse/view, final emitted bytes for encode, input buffering plus event
construction/delivery for Reader, and event clones plus both output buffers
for Writer. Rejections emit no successful payload. This is not a count of CPU
copy instructions or every temporary copy.

`allocations` counts owned fixture and operation payload/result identities:
nonempty value containers/scalars, tag boxes, event vectors/payloads, output
buffers and retained error paths. Deterministic encode counts one encoded
segment per node; the collision case creates four leaf segments before refusal.
These logical allocations are not OS allocator calls or reallocations.

`logical_memory_bytes` adds retained fixture storage to one operation's modeled
payload/result storage. On the pinned layout a value/event cell is 32 bytes and
a path component 16 bytes. Parse and view account for the materialized value;
Reader accounts for the larger buffering/delivery phase; Writer accounts for
its encoded buffer, cloned event and sink. Fixture setup is outside timing, but
its retained identities and storage remain in the counters. Oracle temporaries,
private worklists, capacity slack, sorting metadata and other workloads are
excluded. This is modeled logical memory, not RSS, a physical allocator peak or
a bound on all parser temporaries. Rejection counters include selected error
paths and collision payload buffers, not an exhaustive transient heap census.

Structural counters describe the independently authored full input, including
well-formed documents refused by smaller kernel limits. A malformed document
has zero materialized nodes/depth; no partial value is published. `event_count`
counts the owned records used by the event routes. `terminal_open_streams: 0`
is the finish/drop invariant checked against exact `Closed` states; it is not
native handle instrumentation. Each rejected batch contains sixteen errors.

Byte throughput uses emitted bytes for successful encoders/writers, otherwise
the fixture's wire size, divided by median batch latency. Rejection throughput
is a wire-size equivalent, not I/O bandwidth. Operations per second is also
reported. The report binds workload, probe hash, source-tree hash, Git revision,
target/backend/profile, Rust/Cargo versions and effective flag sources. CPU and
OS are descriptive metadata. Ambient environment, CPU frequency, path, PID and
timestamp never enter report identity. Compiler/profile overrides outside that
recorded configuration, including an ambient Cargo build target or target flag
source, are refused. Sources and revision must remain unchanged
through capture; a clean-source revision must contain the exact probe.

## Reproduction

```bash
scripts/stdlib-cbor-performance-check.sh
scripts/stdlib-cbor-performance-test.sh
CARGO_TARGET_DIR=target-fast scripts/stdlib-cbor-performance.sh
```

The report is retained in
`$CARGO_TARGET_DIR/reliability/evidence/stdlib-cbor-performance.json`.
The explicit dirty-tree override produces only a `development` report.
Focused checks cover missing/duplicated samples, source and probe drift,
percentile/throughput corruption, resource-model changes, terminal streams,
partial success, unsupported promotion and deterministic identities.
Conformance and public/native integration remain separate owner gates.
