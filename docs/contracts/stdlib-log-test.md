# Logging model, regression and bounded fuzz contract

`STD-LOG-TEST-001` covers an independent finite reference, authored exact
kernel observations and compiled public hosted sink execution. The register is
[stdlib-log-test.json](../../testing/stdlib-log-test.json). The model and testing
boundary stays pending until focused checks, the maintained fuzz smoke, current
source quality and exact-SHA publication CI are verified.

## Reference domain and independence

The reference imports Rust standard collections only. It does not import the
production kernel, compiler, VM, provider, filesystem, clock or serializer.
Its recursive values, field validation, escaping, Base64 encoding and queue
transitions are separate from the production implementation. Finite floating
point rendering uses the same Rust primitive as production; independent numeric
formatting is outside this proof. Timestamps are caller-supplied known canonical
UTC tokens. Calendar validation and clock readings belong to `std.time`.

The finite domain admits 128 value nodes, 256 bytes per scalar, height 20 and
256 KiB of formatted output. The queue admits at most eight records of 2,048
bytes each and at most 512 replay steps. `OutsideDomain` refuses an observation
the reference cannot decide; it is never matched to Tondo's `ResourceLimit`.
Domain admission and production limits are separate checks.

Fields retain insertion order, reject duplicate or control-containing keys
without mutation, and publish immutable event snapshots. Text preserves field
order; JsonLines orders keys by UTF-8 bytes. Both formats preserve Unicode,
escape controls, encode bytes as canonical Base64, retain explicit redaction
tokens, reject nonfinite floats and finish with one physical LF. Limits apply
to validation and to the complete encoded record before sink admission.

## Queue and writer ownership

`write` admits a whole formatted record. It does not deliver until `flush`,
consuming `close`, or a full `Block` queue drains its oldest record. A full
`Reject` or `Drop` queue preserves earlier admissions. Invalid formatting or
limits fail before a full-queue policy can produce a receipt. No model worker,
implicit batching or retries exist.

A scripted writer supplies exact accepted counts, short writes, cancellation,
resource refusal and I/O failures. Valid counts commit only their prefix;
invalid counts deliberately commit no bytes, matching the sealed host fixture.
The queue retains the full logical record and its offset until delivery
finishes. Cancellation and resource refusal resume the unwritten suffix. I/O
may leave a physical prefix and makes the sink terminal. Consuming close retires
the queue and writer exactly once even when draining or flushing fails.

Every generated schedule checks output as a prefix of accepted records, finite
queue occupancy and storage. A successful close delivers every accepted byte.
Terminal reference owners and bytes are zero; writer close count is one. These
are logical model quantities, not RSS, OS allocator calls or production counters.

## Executable comparisons

Fourteen reference unit tests cover construction, immutable snapshots,
validation priority, all queue policies, short writes, cancellation, terminal
I/O, consuming-close failures and domain refusals. Generated queue schedules
use 4,096 deterministic seeds and 96 operations, replayed twice.

Nine integration tests comprise two kernel comparisons, one authored corpus
and six hosted tests. The shared kernel adapter compares two formats and six
limit profiles across 4,096 deterministic seeds; zero dimensions in all six
`LogLimits` constructor parameters, nested nonfinite atomic field failures and
default depth 15/16/17 boundaries are checked separately. Thirty-four authored
corpus rows retain success bytes or
nominal errors and compare both reference and kernel. Expectations are authored
independently of their outputs; no automatic blessing is used.

Public programs exercise nested values, immutable event snapshots, both output
streams, explicit UTC timestamps, static capability denial and affine ownership.
Owned file fixtures check append/create/truncate delivery and failure when parent
directories are missing. Four concurrent producers emit eight known records
each; tests require whole records and each producer's original order.
Twelve console profiles cover both formats, all three queue policies and
capacities one/two, including filtering with no queue admission.

Seven scripted writer profiles and actual `Group.cancel` compare exact prefix
observations with the independent queue. The writer fixture is injected as
trusted `GeneratedStandard` source so tests can access private ordinary sinks;
it is not a shipped public helper. Production host lifecycle tests retain their
own resource accounting and budgets. Process-bearing workspace tests require
the supported delegated cgroup-v2 scope; an unisolated run is not accepted.

## Fuzz protocol and verification

`stdlib_log` includes the same reference source and kernel adapter directly.
The minimal graph contains the stdlib and libFuzzer dependencies, without the
compiler, VM, conformance runner or reliability CLI. The normal compiler-enabled
graph also has to build. Inputs are limited to 4,096 bytes and 512 steps.
Replay checks determinism, byte conservation and terminal retirement; the shared
adapter checks exact formats/errors and atomic nonfinite refusals. The fuzz
oracle does not execute console, files or arbitrary Tondo programs.

The maintained smoke uses `nightly-2026-07-28`, 128 runs, seed 4,113, maximum
input 4,096 bytes, per-input timeout 10 seconds and RSS limit 4,096 MiB. Address
sanitizers remain enabled. Sandbox sanitizer refusal is a setup failure, never
success. The output corpus is task-owned temporary storage; crash artifacts
remain available for diagnosis.

Run `scripts/stdlib-log-test-check.sh` for the source/metadata boundary,
`scripts/stdlib-log-test-test.sh` for invalid inputs, progression and focused
execution, and `scripts/stdlib-log-fuzz.sh` for the actual bounded smoke. The
full gate includes all three. Closure requires current-source coverage at every
80% global/risk floor, all six unchanged critical mutants caught, supported
inventory/matrix/ratchet generation, inspected signed publication and exact-SHA
CI. This boundary does not promote native ABI, native AOT, performance, common
conformance or complete public owner readiness. The next owner block is
`STD-LOG-PERF-001`.
