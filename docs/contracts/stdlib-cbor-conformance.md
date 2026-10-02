# `std.cbor` bounded VM/native conformance

`STD-CBOR-CONF-001` compares seven cases through verified hosted bytecode and
a fresh native Rust process. The register is
[`testing/stdlib-cbor-conformance.json`](../../testing/stdlib-cbor-conformance.json).
Its initial `adapter-ready` state describes executable cases awaiting the
clean-source comparison and required functional/quality gates. A verified
closure advances the owner to `STD-CBOR-DOC-001`.

Both probes compile the same
[`shared case implementation`](../../crates/tondo-stdlib/examples/support/cbor_conformance_cases.rs)
and [wire corpus](../../crates/tondo-reliability/tests/fixtures/cbor-wire.json).
There are 61 valid and 30 invalid documents. The existing independent bounded
wire model verifies their authored expectations before VM execution; the
model suite is also required by the capture. No new external codec or dependency
is introduced. This is bounded RFC-vector/model interoperability, with the same
portable kernel behind both execution routes.

The VM uses a bodyless **test-only** callable for each case and returns its
computed string through verified dispatch and result admission. This is
not a source-level Tondo `std.cbor` call. The production compiler API and host
registration are unimplemented. The native process uses the Rust kernel;
it does not execute a native CBOR ABI or Cranelift lowering. Its zero-live
table objects check observes the runtime table counter. The CBOR kernel
allocates no such objects, so this is not a CBOR memory-management proof.

Seven cases separate contracts that can fail independently:

- `typed-dynamic`: a nonempty `Vec[u64]` through common static protocols and
  dynamic values, the full unsigned range, typed duplicate rejection/first/last
  policies, and a type mismatch.
- `wire-model`: every valid ordinary encoding, bit-preserving floats, tags,
  undefined, arbitrary map keys, exact validated raw bytes and borrowed views;
  every invalid document fails before a value or reader is published.
- `deterministic`: 59 exact outputs, preferred widths, NaN normalization,
  signed zero, bytewise key ordering, idempotence, and two normalized-key
  collisions rejected before bytes are returned.
- `streaming`: all 61 valid documents, 270 events in total, identical complete
  events for read fragments of one, two and seven bytes, owned payload copies,
  writer reconstruction, one EOF observation, explicit finish and terminal
  `Closed`. Reader buffering and owned Rust events remain the kernel boundary.
- `errors-path`: exact `InvalidUtf8`, half-open bytes `5..7`, and nested
  array/map/value/tag path through parse, validation, raw, view and reader.
- `limits-lifecycle`: atomic input/depth/output rejection, early reader finish
  failure, invalid writer event rejection, and subsequent `Closed`.
- `route-boundary`: portable scalar kernel; public API, native ABI/AOT and SIMD
  promotion remain absent.

Run `scripts/stdlib-cbor-conformance.sh` for the shared-process comparison and
`scripts/stdlib-cbor-conformance-test.sh` for positive and negative contracts.
The runner requires a clean checkout. `TONDO_STDLIB_CBOR_CONF_ALLOW_DIRTY=1`
permits local development only and retains `dirty: true`; such a report cannot
support promotion. Both sources and Git identity must stay unchanged throughout
capture. Build overrides that change the selected compiler or profile are
rejected. Each Cargo invocation has a three-minute bound.

The report records the actual host target, Rust toolchain, development profile,
incremental flag, source revision/tree, input hashes, exact VM/native
observations, log hashes and successful prerequisite suites. Report identity
contains no physical artifact paths, clock values, process IDs or addresses.
Negative checks reject missing/duplicate cases, observable/error/event drift,
false cleanup, nonzero counters, incorrect source hashes, stale owner state,
and unsupported promotion claims. Kernel, independent-model and VM dispatch
tests must pass before the report is written. The terminal runtime counter is
not RSS, allocator instrumentation or a native codec handle count.
