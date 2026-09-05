# `std.encoding` VM/native conformance

This contract closes `STD-ENCODING-CONF-001`. The machine-readable authority is
[`testing/stdlib-encoding-conformance.json`](../../testing/stdlib-encoding-conformance.json).
The std.encoding conformance lane is target-qualified and keeps hosted and
native observations comparable without making an AOT claim.
It replays one six-case observable corpus on the hosted VM and on the private
native encoding ABI. Every probe is a fresh process and every case must return
with zero live native handles.

The same case IDs are checked in both processes before any normalized
observation is accepted.

The shared corpus covers Base64 standard and URL-safe unpadded
interoperability, strict hexadecimal case policies, one-byte streaming and
quantum completion, byte-exact non-canonical errors, cumulative output limits,
terminal `Closed` transitions, and the explicit scalar/SIMD boundary. The VM
fixture emits one ordered line per case. The native probe emits the same case
IDs with normalized output bytes, error kinds and offsets; it never compares a
Rust layout or a host address.

The private native lane uses opaque `u64` capabilities. Its materialized and
incremental operations call the same scalar `std.encoding` kernel as the
hosted implementation, so the comparison proves interoperability and
chunk-boundary invariance without maintaining a second wire model. The native
ABI accepts only the closed codec/policy/operation set and a finite target
budget; invalid options and stale handles fail closed.

Errors are observable, not swallowed. Non-zero Base64 pad bits and a
case-forbidden hexadecimal digit preserve their exact kind and input offset.
An output-limit error publishes no partial bytes and leaves the stream
terminal; a later operation reports `EncodingError.Closed`. Empty input under
zero limits remains a successful empty transformation.

There is currently no optimized SIMD route. The `route-boundary` case records
`simd: not-measured-no-optimized-route`; it is an explicit non-claim, not a
synthetic equivalence result. Likewise the native run is target-qualified ABI
evidence only: `native_aot_lowering: not-claimed`, no Cranelift lowering, no
public FFI layout, and no native fast-path promotion are implied.

Run the lane with:

    scripts/stdlib-encoding-conformance.sh

Contract mutations are exercised by:

    scripts/stdlib-encoding-conformance-test.sh

The runner writes
`target/reliability/evidence/stdlib-encoding-conformance.json` with fixture and
probe hashes, exact VM lines, normalized native observations, cleanup checks
and the hosted/native/AOT boundary. Reports contain no physical paths,
addresses, process IDs or timestamps.
