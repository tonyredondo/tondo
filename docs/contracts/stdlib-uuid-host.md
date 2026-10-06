# Hosted UUID provider boundary

`STD-UUID-HOST-001` registers the four public `std.uuid` nominal types and all
fourteen operations with the compiler and production hosted VM. The route is
`hosted-scalar`, targeting `tondo-vm-hosted`. The parent contract remains
[`stdlib-uuid.md`](stdlib-uuid.md); the machine register is
[`stdlib-uuid-host.json`](../../testing/stdlib-uuid-host.json).

`ready-production-hosted` records focused implementation evidence while quality
is `pending-80-percent-per-scope`. `verified-production-hosted` additionally
requires the full functional gate and source-bound quality at every locked
global/risk floor. Publication and exact-SHA CI closure belong to the tracker.
Public compiler/VM registration is implemented; conformant public promotion,
native runtime ABI and native AOT remain separate claims.

## Public values and capability checks

The private `Uuid` carrier has two `UInt64` fields in high/low network order.
It preserves all 128 bits, including `max`, and supports `Copy`, `Discard`,
`Equatable`, `Key`, `Send` and `Share`. Private fields cannot be read or used to
construct a UUID from Tondo code. The VM uses its normal managed record
representation; this is not a native inline layout or an ABI commitment.

Core operations and v5 require no provider capabilities. Importing `std.uuid`
does not read a provider. A v4 reference requires `entropy`; a v7 reference
requires both `civil-clock` and `entropy`. Direct calls, stored function values
and calls in `defer` are checked statically with `E1008`. Installing test inputs
never grants a capability. Every UUID operation is synchronous and is not
`selectable`; `await` on an ordinary generator call is rejected.

`UuidError` has public `kind: UuidErrorKind` and `offset: Int?` fields. Both the
closed kind enum and the complete error implement intrinsic `Display`.
`Display.display(error)` and interpolation emit the kind name, followed by
` at byte N` when an offset exists. `testing.assertOk` uses this same display
path on a failed UUID result. The VM accepts this intrinsic only for the exact
standard identities and validated enum/record/optional-integer field schemas.

## Production providers

Entropy uses the approved, exactly pinned `getrandom = "=0.4.3"` dependency in
the compiler host adapter. Each generation invokes `getrandom::fill` once on a
fresh bounded stack buffer: sixteen bytes for v4 and ten for v7. Tondo performs
no retry, alternative RNG selection or collision lookup. OS/backend internals
are owned by `getrandom`; no application filesystem capability is exposed.
`UNSUPPORTED` becomes `EntropyUnavailable`; other returned failures become
`EntropyFailure`. A failed fill cannot publish even a partially initialized
UUID. No errno or platform message is included in the nominal error.

v7 reads `SystemTime::now` and checks `duration_since(UNIX_EPOCH)` before
truncating the nonnegative duration to milliseconds. A fractional instant
before the epoch is rejected, even when truncation could otherwise yield zero.
The inclusive bound is `0..=281474976710655`. A range failure precedes any
entropy read. The adapter consults no timezone or locale. `SystemTime::now`
does not return a fallible OS result; `ClockUnavailable`/`ClockFailure` are
exercised at the sealed test boundary, not claimed as observed OS failures.
Clock regressions are preserved; no state manufactures strict monotonicity.

## Sealed testing and lifecycle

The Rust runtime control surface `EnvelopeHandle.with_uuid_providers` accepts
one sealed `UuidTestProviders` fixture during `Setup`. There is no public Tondo
provider setter, production injection function or provider handle. The fixture
contains at most 256 clock rows and 256 entropy rows, each entropy row at most
sixteen bytes. Wrong lengths refuse with `ProviderMisconfigured` before copy.
Only clock failures may occupy clock rows and entropy failures entropy rows.

Envelope clones share once-only consumption under the existing envelope mutex.
Exhaustion returns the nominal unavailable kind and never falls back to the OS.
Each read admits one work unit before consuming a row. Installation atomically
admits work and logical memory; a failed install does not replace prior state.
The conservative storage charge is 64 bytes plus 32 per row and the supplied
entropy bytes. It includes fixture setup and is re-admitted on phase changes.
Closing discards snapshots and refuses later access. A UUID never retains a
fixture, mutex, provider state or host handle.

## Result admission and failure atomicity

Before providers run, the host prepares a maximum 182-byte detached reply and
checks the complete typed VM storage for both success and the largest nominal
error, including an offset. An active phase account additionally reserves the
transport and joint import storage. Ordinary synchronous execution preflights
the VM byte/object limits without creating a test account; a paused worker
exports its own stable heap capacity to the servicing thread. No worker heap
allocation may interleave before its response returns. Heap refusals leave
both provider streams unconsumed. Charged replies shrink to their actual
logical size. Direct host tests and production VM sweeps exercise refusal and
success transitions; the ordinary/paused import regression checks receiving
capacity and commit ordering. Phase tests verify reuse and charge release.
Internal synchronous reference-host calls without a scheduled VM recipient
retain their transport-only route. Executable UUID calls have a root task and
retain result preflight. The existing malformed virtual-time callback
regression verifies invocation and clock restoration across this internal path.

Provider buffers check target byte limits before access. `toBytes` reserves
ownership for a fresh sixteen-byte buffer before allocation/publication;
`toString` checks the target bound and makes a fallible 36-byte reservation.
v5 retains the pure kernel's 16 MiB default name limit, further bounded by the
host target. Infallible public materializers report VM resource refusal when
their result cannot be admitted; fallible generators return nominal provider
errors after admission. None publishes a partial UUID or retained provider.

These counters describe logical transport, fixture and VM storage. They do not
measure RSS, OS allocator calls or native code size. The pure kernel's fixed
width state does not mean that hosted transport or managed records allocate
nothing. No throughput or provider-cost claim precedes `STD-UUID-PERF-001`.

## Evidence and limits

The 23 focused compiler/VM tests cover public core calls and keys, direct/alias/defer
capability rejection, private storage, synchronous effects, RFC v4/v7 vectors
through real `Operation::Test`, real OS v4/v7 shape/round trips, every error kind
and a lexical offset, framework error formatting, finite fixtures, shared
consumption/closure, work and memory admission, clock boundaries/regressions,
provider failure/recovery and all carrier bits. The schema test rejects thirteen
invalid intrinsic display descriptions. Actual OS success is evidence for the
observed host; it is not portable-target or statistical uniqueness proof.

[`stdlib-uuid-host-check.sh`](../../scripts/stdlib-uuid-host-check.sh) checks the
register, source/test anchors, dependency pin and parent progression.
[`stdlib-uuid-host-test.sh`](../../scripts/stdlib-uuid-host-test.sh) verifies both
local progression states, invalid records and the compiler/VM tests. Independent
model/fuzz, performance, conformance and executable usage remain TEST, PERF,
CONF and DOC. Native ABI, AOT and SIMD are not measured or promoted here.
