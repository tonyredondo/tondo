# Standard Library performance contract

**Status:** accepted design contract for `STD-0.1A`; no module is published by
this document alone.

`STD-PERF-001` defines how Tondo evaluates performance without turning a
machine-specific benchmark number into language semantics. The canonical
machine-readable record is [`testing/stdlib-performance.json`](../../testing/stdlib-performance.json).
The record fixes the protocol, dimensions, default regression budgets, workload
classes, owner groups and promotion gates. Concrete module contracts add their
operation IDs, input identities and absolute baselines before implementation.

## One protocol

Every measurement has one logical identity:

~~~text
module + operation + workload + target + backend + profile + toolchain + revision
~~~

The report must record the CPU model and features, memory, OS/kernel, target,
backend, profile, compiler/toolchain, flags and source revision. Physical paths,
process IDs, timestamps and ambient environment values are not identity fields.

The harness uses a monotonic clock, three warmups, nine measurements in each of
three independent processes, and reports median, p95 and p99. This produces at
least 27 samples for a logical measurement. Outliers are reported, never
silently deleted. A benchmark that cannot reproduce its environment or samples
fails the performance gate rather than being normalized until it passes.

The first capture is a reviewed baseline. Later captures compare the same
operation and workload identity. A workload may be replaced only by an explicit
contract revision with a new identity; changing its input to improve a result is
not a valid optimization.

## Dimensions and default budgets

Each hot path reports the applicable dimensions:

| Dimension | Direction | Default maximum regression |
| --- | --- | ---: |
| Throughput | higher is better | 5% |
| Tail latency (p95/p99) | lower is better | 10% |
| Allocations per operation | lower is better | 0 allocations |
| Allocated bytes per operation | lower is better | 10% |
| Peak memory | lower is better | 10% |
| Startup / first call | lower is better | 10% |
| Code size | lower is better | 5% |
| Compile time | lower is better | 10% |

The JSON record expresses these values as basis points. The allocation-count
budget is exact: a change that adds an allocation is a regression unless the
owner contract explicitly changes the operation's allocation contract and
records the reason. An owner may publish a stricter budget, never a looser one
without a reviewed trade-off and a new baseline.

Budgets are evaluated against the reviewed baseline with the same target,
backend, profile, toolchain and workload. A change that improves one dimension
and regresses another must show both observations; a single faster throughput
number cannot hide memory, startup or compile-time cost.

## Workload classes

Every applicable owner supplies bounded, content-addressed workloads for:

- `empty`: no-work and smallest valid input;
- `small`: ordinary fast path where setup does not dominate;
- `representative`: production-shaped input documented by the owner;
- `large`: input that exposes allocation, cache and backpressure behavior;
- `fragmented_stream`: one-byte and boundary-splitting delivery for readers and
  decoders; and
- `adversarial`: every published size, depth, overflow, timeout and malformed
  input boundary.

The workload is part of the baseline identity. Inputs are bounded before they
reach a parser, decoder, generator or host operation. Hostile inputs must prove
that resource limits remain finite; they are not optional stress tests.

## Scalar oracle and optimized kernels

Every critical bytes, text, parsing, hashing or codec operation starts with a
portable scalar implementation. It is a simple executable oracle, not merely a
reference description. Optimized routes compare exact observations with it:

- values and encoded bytes;
- error kind, path and precedence;
- ordering and overflow;
- ownership and allocation contract; and
- streaming boundaries and final state.

SIMD, word-at-a-time, lookup tables, specialization, automatic vectorization
and target multiversioning are permitted. Kernel selection is outside the public
API and happens at most once per operation domain. Every target has a portable
scalar fallback. A native instruction set, layout assumption or unsafe binding
cannot become a required capability merely because it is fast on one machine.

An optimized route is not promoted when it only matches successful examples. It
must match rejection, truncation, overflow, empty input, fragmented input and
adversarial cases. A mismatch is a correctness failure even when the optimized
route is faster.

## Allocation and streaming rules

Owners must distinguish:

- logical values from physical allocations;
- borrowed spans from materialized results;
- collector convenience APIs from streaming APIs; and
- reusable capacity from newly allocated capacity.

A materializing convenience function may collect the same semantic machine used
by a streaming writer, but it cannot introduce a second parser or encoder with
different errors. Typed decoding must not materialize a dynamic DOM first.
When a result can be written to `std.io.Writer`, the owner documents that route
and its allocation behavior. COW, pooling and reuse are implementation choices
only when the public ownership and allocation contract remains unchanged.

## Gate sequence

Each owner passes four gates:

1. **Design:** operation IDs, workload identities, scalar oracle and applicable
   dimensions are reviewed before implementation.
2. **Capture:** a clean workspace and pinned toolchain produce repeated samples
   with the complete environment record.
3. **Compare:** exact oracle equivalence and every applicable regression budget
   pass for every declared target/backend.
4. **Promote:** the baseline and report are reproducible, reviewed and free of
   unexplained regressions.

`STD-PERF-CONF-001` coordinates these owner reports through
[`testing/stdlib-performance-conformance.json`](../../testing/stdlib-performance-conformance.json)
and `scripts/stdlib-performance-conformance.sh`. The coordinator has one row
for every stdlib owner: a captured row names its operation, workload, scalar
oracle and measured dimensions; an owner without a reviewed hot-path identity is
explicitly deferred with a reason. It never accepts an omitted owner, a report
that claims dimensions it did not measure, a missing environment field, or a
sample set shorter than the protocol. The current probe captures throughput and
tail latency for five portable kernels; allocations, memory, startup, code size
and compile-time remain explicit owner-promotion work until their workloads and
baselines are reviewed. The coordinator does not replace the owner gates, accept
a missing report, or average incompatible targets into a green number. VM and
native backends are compared by semantic observation first; their performance
baselines remain separate.

## What is not a guarantee

The contract does not promise one universal operations-per-second number across
CPUs, operating systems or backends. It does guarantee that a published number
has a stable workload identity, a recorded environment, a scalar oracle, a
repeatable measurement protocol and an explicit budget. Language semantics are
defined by values, errors, ordering, ownership and effects; performance data
cannot redefine them.
