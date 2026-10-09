# Hosted logging performance boundary

`STD-LOG-PERF-001` measures the existing scalar logging implementation on
`x86_64-unknown-linux-gnu`, using the hosted bytecode VM in Cargo's `test`
profile. The register is [stdlib-log-performance.json](../../testing/stdlib-log-performance.json).
This initial protocol is measurement-ready; promotion requires an actual
clean-source capture, current-source quality, the full repository gate and
exact-publication CI. No benchmark result is invented by the report tests.

The campaign provides a baseline for complete small logging programs. Its
latencies include VM verification and initialization, event creation, logger
setup, delivery, consuming close and Tondo assertions. They are not individual
`emit` latency, production optimized latency, a speedup comparison, or native
AOT performance. Compilation, independent oracle construction, file preload,
and Rust post-execution byte/lifecycle checks are outside the clock. Selected
retained fixture storage is still reported. The unmeasured compiler, bytecode,
provider and allocator storage is explicitly excluded.

## Workloads and observables

Eighteen routes are the smallest table that separates the two filtering paths,
both encoders and structured input, larger input, each full-queue policy,
explicit flush, multiple producers, both file modes, three distinct writer
outcomes, and rejection before delivery. Combining those outcomes would hide
which path a baseline measures. Ordinary calls use the public `Logger` and
built-in sinks. Three writer routes install the existing test-only sealed
`BufferedSink[ProbeWriter]` source and measure that internal sink directly;
their cost is not presented as public logger cost.

| Workload | Timed activity | Operations | Sink/format |
| --- | --- | ---: | --- |
| enabled-query | False `enabled` checks | 128 | Console/Text |
| filtered-emit | Exact `Filtered` receipts, no delivery | 128 | Console/Text |
| text-small | Accepted plain records | 32 | Console/Text |
| json-small | Accepted plain records | 32 | Console/JsonLines |
| text-structured | Nested fields, bytes, redaction and Unicode | 16 | Console/Text |
| json-structured | Same values; explicit supplied UTC token | 16 | Stderr/JsonLines |
| json-large | Authored 4,096-byte ASCII message | 8 | Console/JsonLines |
| block-capacity-one | Drain on a full queue | 32 | Console/JsonLines |
| reject-full | One acceptance and seven Backpressure errors | 8 | Console/Text |
| drop-full | One acceptance and seven Dropped receipts | 8 | Console/Text |
| flush-each | Admit and flush each whole record | 8 | Console/Text |
| concurrent-producers | Four tasks, eight records per producer | 32 | Console/Text |
| file-append | Preserve authored prefix and append records | 8 | File/JsonLines |
| file-truncate | Replace authored prefix with records | 8 | File/JsonLines |
| writer-short | Two-byte writes of complete records | 8 | Sealed buffer/Text |
| writer-interrupt | Cancelled after a two-byte prefix, resume suffix | 8 | Sealed buffer/Text |
| writer-io | Two admitted records; Io prefix and one Closed refusal | 3 | Sealed buffer/Text |
| reject-event-limit | Eight ResourceLimit refusals, no output | 8 | Console/Text |

Capacity is eight except the three full-queue routes (one) and terminal Io
(two). Default event limits apply except the explicit eight-byte event limit
route. The larger message does not expand the finite reference domain: its
reference constructor must return `OutsideDomain`, and an authored ASCII JSON
byte law verifies the target output. It is not a Tondo ResourceLimit result.

Before timing, the independent std-only value model constructs the exact
small/nested records; the independent finite queue model determines receipts,
queue high water, short-write offsets, interrupted resumption and terminal Io.
Each observed output is checked byte for byte after timing. Concurrent output
must contain each whole authored record exactly once and preserve each
producer's order, then is replayed in that legal observed order. No global
interleaving is prescribed. Accepted, filtered, dropped and rejected counts
are asserted by the Tondo program and must conserve attempted events.

The values oracle shares Rust's primitive finite-float rendering, so it does
not independently establish numeric formatting. The supplied UTC token is
authored; it does not establish a calendar or clock-provider oracle. The
existing model, hosted tests and fuzz boundary remain separately qualified.

## Protocol, identity and retained evidence

The runner verifies the oracle and fixtures before capture. Three independent
processes each execute three warmups and nine measured repetitions per route.
All 27 samples per route remain in the report (486 total), including outliers.
Timing uses monotonic `std::time::Instant`. Median, P95 and P99 use nearest
rank; with 27 samples P99 selects the maximum. There is no outlier removal,
interpolation, best-of selection or numerical performance SLO in this baseline.

Each sample runs with two million VM steps and an 8 MiB managed heap limit.
Latency must be positive and no greater than 30 seconds. Each already-built
independent probe has a 120-second process limit; reports and sample input
are bounded to 32 MiB. The normal runner requires a clean working tree and
verifies unchanged source, toolchain, build flags, Git revision and tree status
after capture. `TONDO_STDLIB_LOG_PERF_ALLOW_DIRTY=1` records `development` and
cannot support clean-source promotion.

Each workload identity includes the suite/workload, exact probe SHA-256,
quality provenance source-tree SHA-256, target, backend, profile, Rust/Cargo
versions, recorded flags and Git revision. The canonical identity has its own
SHA-256. Host CPU/OS descriptions are observational fields outside that identity.
Physical file paths, PIDs, timestamps, CPU frequency and ambient environment
are excluded. Owned unique directories isolate file fixtures; only their
authored file and then-empty directory are retired. No user file is involved.

Unrecorded compiler, profile, target or wrapper overrides are refused before
execution. Exact Rust probe bytes must exist in the recorded clean-source
commit. Duplicate JSON keys, non-finite numbers, booleans masquerading as
counters, missing/duplicated coordinates, unknown routes, stale hashes,
edited summaries and false native claims are rejected. Summaries are
recomputed from every retained sample; fixtures cannot be replaced by an
edited average or a fabricated report.

## What the resource counters mean

`VmStatistics` supplies observed steps, logical managed-object allocations,
collections, peak live managed objects/bytes and collection buffer copy,
element, share and detach counters. These are not Rust or OS allocator calls
and do not include the full host heap. Host handle creations are the actual
registry ID increment during each complete workload. Terminal actual host
values, channels, jobs and waiters must be zero; queued host/provider work and
buffer/async memory registries must also be empty after consuming close.

`model_peak_queued_records/bytes` describe the independent queue replay,
not a sampled host memory peak. The whole logical record remains retained
until fully delivered, even after a physical short-write prefix. Selected
fixture bytes count retained source, sealed source, authored record vectors,
expected output and file prefix. Selected fixture allocations count their
String/Vec identities, including empty logical identities; they are not
allocator calls or a full process allocation count. Compiler/bytecode storage,
temporary oracle objects, stack, capacity slack, Arc/map nodes, provider
internals and OS/native memory remain unmeasured. Ordinary logging has no
shared provider admission budget, so `budget_bytes` is null. Native live handles
are also null. Null is never converted into a claim of zero native memory.

Throughput divides the stable operation, accepted-event or delivered-record
count by the median whole-workload latency. Queue acceptance is reported
separately from completed delivery: the terminal-Io route admits two records
but delivers none and leaves exactly two physical bytes. Output bytes, receipt
counts, modeled queue high water and actual VM/handle counters retain their
per-sample distributions. No summed modeled/observed value is called RSS.

## Validation and promotion

Run the focused contract/tests and campaign with the repository toolchain:

```sh
bash scripts/stdlib-log-performance-check.sh
bash scripts/stdlib-log-performance-test.sh
bash scripts/stdlib-log-performance.sh
```

Use a task-scoped `CARGO_TARGET_DIR` on the physical artifact disk. The campaign
writes `reliability/evidence/stdlib-log-performance.json` there. Contract bytes
and probe binding are regenerated through
`python3 -B scripts/stdlib_log_performance.py write-contract --contract testing/stdlib-log-performance.json`.
The verifier accepts only the exact ready/verified stages and corresponding
parent quality/next-owner states. Negative tests include predecessor phase
fixtures, invalid provenance, retained-sample corruption and lifecycle/receipt
refusals. The repository full gate executes the maintained campaign.

Promotion additionally requires current-source consolidated coverage above
80% in every global/risk dimension, all six unchanged critical mutants,
generated inventory/matrix/conformance provenance, the full functional gate
and exact-publication CI. Native ABI, native AOT, SIMD, multiversion dispatch,
code size, optimized production latency and full-owner promotion remain
unmeasured or unclaimed. The next owner is `STD-LOG-CONF-001`.
