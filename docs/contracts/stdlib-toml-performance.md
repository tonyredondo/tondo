# `std.toml` scalar-kernel performance

`STD-TOML-PERF-001` fixes a reproducible baseline for the Rust TOML 1.1.0
kernel. The machine-readable contract is
[`testing/stdlib-toml-performance.json`](../../testing/stdlib-toml-performance.json).
The admitted target is `x86_64-unknown-linux-gnu` with the `test` profile and
`rust-stdlib-kernel` backend. This is not a hosted VM measurement: no public
Tondo TOML compiler ABI or VM bridge exists yet. The report therefore cannot
promote the public API, native runtime ABI, native AOT, SIMD or multiversion
dispatch. The selected route is `scalar-fixed-target` for every sample.
The native AOT status remains `not-claimed`.

## Protocol and oracle

The probe is `toml_performance_probe` in
[`crates/tondo-stdlib/tests/toml_performance.rs`](../../crates/tondo-stdlib/tests/toml_performance.rs).
Before any timed sample, the runner executes the independent bounded TOML
model suite in `tondo-reliability` and verifies exact kernel values, canonical
bytes, events, full errors and lifecycle on every fixture. Each of three
independent processes performs three warmup batches and nine measured batches
per workload. A batch has 16 operations measured with a monotonic clock.
The 27 samples remain in the report, including outliers; median, P95 and P99
use nearest-rank selection for tail latency. Fixture creation and exact post-sample checks are
outside the timed interval. The loop's status check, `black_box` and result
drop remain inside it.

The report identity binds suite, workload, probe hash, target, backend,
profile, Rust/Cargo toolchain, flags and Git revision. CPU model and OS are
descriptive host metadata, not identity inputs. Ambient environment, current
CPU frequency, path, PID and timestamp are excluded. A dirty tree is refused
for a promoted capture; the explicit dirty override is only for development.
Reports from different targets or backends cannot be combined.

## Workloads and counters

The thirteen workloads are the smallest set here that covers each currently
executable kernel route plus representative structure and bounded rejection:

| Family | Workloads | Distinct boundary |
| --- | ---: | --- |
| Materialized parse | 4 | Small scalars, nested/inline tables, 32 array-of-table rows, Unicode, numeric and all four local/offset temporal forms |
| Borrowed view | 1 | `parse_view` validation and borrowed input lifetime |
| Normal/canonical encode | 2 | Insertion order and sorted canonical bytes |
| Reader/writer events | 2 | Buffered event construction, delivery, reconstruction and terminal finish |
| Adversarial rejection | 4 | Depth, node and scalar limits, then malformed duplicate tables |

`parse_view` currently calls `parse` and discards its materialized value before
returning a borrowed view. Its latency and logical allocation count must not be
interpreted as a zero-allocation parser. The reader parses and buffers events;
the writer collects events before `finish`. These workloads measure those
actual draft implementations, not incremental native streaming.

Each workload reports the input/output byte count, fixed operation count,
logical payload bytes copied, logical allocations, modeled logical memory,
depth, nodes, tables, array-of-table rows, event count, rejection count,
selected dispatch and terminal live handles. Throughput divides the relevant
input bytes (or emitted bytes for encode/writer) times 16 by median batch
nanoseconds. Rejected operations have zero output bytes and exactly sixteen
errors with no partial value per sample.

The `bytes_copied` counter is a **logical payload transport** model: decoded
key/text bytes for parse/view, emitted bytes for encode, event payload bytes
for reader and event plus output bytes for writer. It is not a count of CPU
copy instructions or allocator moves. `allocations` counts logical input,
value, event and output identities created by fixture setup and the batch;
it is not a count of allocator calls. `logical_memory_bytes` models retained
input, value, event and output payload plus one operation's payload. It is not
RSS, a physical allocator peak or a bound on parser temporaries. These models
are stable comparison counters, not native runtime memory measurements.

## Reproduction and boundary

```bash
scripts/stdlib-toml-performance-check.sh
scripts/stdlib-toml-performance-test.sh
CARGO_TARGET_DIR=target-fast scripts/stdlib-toml-performance.sh
```

The runner retains
`$CARGO_TARGET_DIR/reliability/evidence/stdlib-toml-performance.json`.
The focused tests reject stale probe hashes, altered protocol/workloads,
missing samples, changed percentiles, live handles, altered throughput,
provenance drift and unsupported hosted/native promotion claims. The
independent TOML model and the kernel's existing regression/fuzz evidence
remain separate from this performance report. `std.toml` is a data codec;
this campaign never parses or changes `tondo.toml` project manifests.
