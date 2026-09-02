# `std.channel` performance contract

`STD-CHANNEL-PERF-001` closes the target-qualified performance measurement
boundary for the channel runtime in the Tondo 0.1 draft. The machine-readable
contract is [`testing/stdlib-channel-performance.json`](../../testing/stdlib-channel-performance.json).
This document records a reproducible hosted baseline; it does not promote the
public API or choose a native algorithm.

## Target and protocol

The private probe is
`process_host::tests::channel_performance_probe` in
[`crates/tondo-compiler/src/process_host.rs`](../../crates/tondo-compiler/src/process_host.rs).
It exercises the scheduler-owned hosted VM (`tondo-vm-hosted`,
`bytecode-vm`) with three warmups and nine measured repetitions in each of
three independent processes. Every workload therefore has 27 monotonic-clock
samples. The 16-operation batch, deterministic seed and identity fields are
fixed in the JSON contract. Outliers stay in `samples_ns`; they are reported,
not deleted to improve a percentile.

Fixture setup is outside timed latency but remains in allocation and logical
memory counters. Report identity includes the suite, workload, probe hash,
target, backend, profile, toolchain, flags and git revision. PID, path,
timestamp, ambient environment and instantaneous CPU frequency are forbidden
identity inputs.

## Workloads and metrics

The nine workloads make topology and queue behavior explicit:

| Workload | Topology | Capacity | Behavior |
| --- | --- | ---: | --- |
| `rendezvous-1-1` | 1:1 | 0 | direct handoff and FIFO waiter wakeup |
| `buffered-1-1` | 1:1 | 1 | single-slot buffered exchange |
| `rendezvous-n-1` | n:1 | 0 | eight producers and one receiver |
| `buffered-n-1` | n:1 | 8 | producer fan-in with finite storage |
| `rendezvous-n-m` | n:m | 0 | eight producers and four receivers |
| `buffered-n-m` | n:m | 8 | buffered producer/consumer fan-in |
| `unbounded-n-m` | n:m | explicit unbounded | burst and queue growth |
| `backpressure-buffered` | 1:1 | 1 | blocked send retains its affine payload |
| `close-wakeup-senders` | n:1 | 0 | closing the receiver wakes all blocked senders |

Each sample reports latency, derived throughput, tail latency (median/P95/P99),
logical host-value allocations, logical memory, queue peak, backpressure,
wakeups and live handles. Logical memory includes channel state, waiter
capacity and queue capacity; it excludes allocator headers, fragmentation and
RSS. Allocation counts are host-value identities, not OS allocation counts.

The probe checks FIFO values at the commit boundary, exact wakeups for pending
waiters, intact payloads on backpressure and close, and no pending waiter,
scheduler job or endpoint before returning. `live_handles` must be zero after
the terminal cleanup.

## Strategy boundary

The selected strategy is
`scheduler-owned-single-worker-channel-baseline`. The hosted scheduler is the
real execution carrier for this report, but it is not native concurrent
throughput. Native runtime ABI contention and native AOT lowering are not
measured here; `native AOT` remains `not-claimed`. Algorithmic fast paths are
deferred to a native-targeted performance campaign with comparable concurrent
evidence. No semantic result, ownership rule, ordering rule or public API
changes with that future campaign.

The independent oracle is the bounded channel model and its integration tests
in [`crates/tondo-reliability/src/channel_model.rs`](../../crates/tondo-reliability/src/channel_model.rs)
and [`crates/tondo-reliability/tests/channel_models.rs`](../../crates/tondo-reliability/tests/channel_models.rs).
This oracle is run before the hosted probe and supplies the model/test
boundary; the performance report never combines targets or backends.

## Reproduction

```bash
scripts/stdlib-channel-performance-check.sh
scripts/stdlib-channel-performance-test.sh
TONDO_STDLIB_CHANNEL_PERF_ALLOW_DIRTY=1 \
  scripts/stdlib-channel-performance.sh
```

The runner writes
`target/reliability/evidence/stdlib-channel-performance.json` and rejects a
dirty workspace by default. CI runs the same runner from a clean checkout.
The channel conformance and documentation leaves remain separate promotion
gates, and this report does not claim native AOT or a completed Cranelift
lowering.
