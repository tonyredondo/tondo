# STD-0.1A implementation and S1A evidence

Status: `implemented-draft`. This document closes the technical Wave 5/S1A
gate for the current Tondo draft; it does not publish `STD-0.1.0` and does not
replace the publication checklist in `TONDO_STANDARD_LIBRARY_SPEC.md`.

## Documentation registry and boundary vocabulary

`testing/stdlib-documentation.json` is the executable documentation index for
the current unpublished draft. It deliberately separates three surfaces for
every owner:

- `kernel` is the portable or intrinsic implementation boundary;
- `bridge` is the compiler/VM/host adapter, or `not-applicable` when no adapter
  exists; and
- `public_api` is derived only from the public-signature audit. `complete` means
  every indexed signature is verified, `partial` preserves audit gaps, and
  `not-applicable` is reserved for intrinsic, metadata-only, or build-only
  owners without public signature rows. The current audit is complete for all
  indexed runtime signatures; `std.meta`, `std.reflect` and `std.serialization`
  use explicit build-only/not-applicable boundaries.

Each owner links at least one verifiable example. Runtime fixtures must retain
their `.exit` and `.stdout`/`.codes` sidecars; codec examples additionally link
the independent external harness; compiler/meta examples link their build
tests. `std.meta` and `std.reflect` explicitly have no runtime example because
they are build-only and metadata-only respectively. This registry documents
the draft and never turns a partial API, implementation, performance or
conformance cell into a release claim.

## Owner-aware fuzz closure

`STD-A-FUZZ-001` closes the S1A FUZZ dimension for all 22 owners without
duplicating cargo-fuzz binaries. The promoted target
[`stdlib_owners`](../../fuzz/fuzz_targets/stdlib_owners.rs) selects one owner
route from the first byte and bounds the payload, compiler source and host
interaction. [`testing/stdlib-fuzz.json`](../../testing/stdlib-fuzz.json) is
the normative registry for every route, corpus, seed, limit, oracle and
regression policy; minimized failures remain replayable in the owner corpus.
The smoke and nightly campaign scripts execute this target, and
`scripts/stdlib-fuzz-check.sh` rejects missing routes, empty corpora, selector
mismatches or unpromoted owner evidence. FUZZ is verified for every owner;
PERF and CONF remain independent promotion dimensions.

## One owner, one implementation boundary

`testing/stdlib-implementation.json` is the machine-readable owner closure.
La trazabilidad por firma se comprueba además con
[`stdlib-public-api-audit.md`](./stdlib-public-api-audit.md), cuya matriz viva
puede permanecer abierta mientras los leaves de implementación sigan
pendientes.
La coordinación del grupo Core ya promovido se comprueba además con
[`stdlib-implementation-coordination.md`](./stdlib-implementation-coordination.md)
y su registro machine-readable; ese gate no convierte los gaps globales de
codec o build-only en un waiver.
La coordinación Hosted se comprueba con
[`stdlib-hosted-implementation-coordination.md`](./stdlib-hosted-implementation-coordination.md)
y [`testing/stdlib-hosted-implementation-coordination.json`](../../testing/stdlib-hosted-implementation-coordination.json);
verifica `std.console`, `std.path`, `std.fs` y `std.process`, sus capabilities,
bridges y las 48 firmas públicas; la auditoría global de codecs y owners
build-only ya está verificada sin promover todavía las celdas S1A.
Every owner has one canonical implementation boundary, source-controlled tests
and a proof description. Portable kernels live in `tondo-stdlib`; compiler and
VM bridges are limited to intrinsic lowering or capability-gated host effects.
There is no second public package, no ambient lookup and no general FFI ABI.

The aggregate owner graph and capability/API rules live in the single
machine-readable integration contract
[`testing/stdlib-spec.json`](../../testing/stdlib-spec.json). The strict gate
validates its topological order and links it to this document and the canonical
standard-library specification; it does not infer promotion from a missing
owner observation. The public codec audit and `STD-A-CONF-001` execution are
closed for the current draft; the native target
descriptor, native artifact and link plan are now closed as pure contracts and
the next implementation block is `STD-A-DIST-001`.

The A0 `std.meta` owner has a dedicated contract at
[`testing/stdlib-meta.json`](../../testing/stdlib-meta.json) and a separate
cell record at [`testing/stdlib-owner-evidence.json`](../../testing/stdlib-owner-evidence.json).
Its build-only `HOST` cell is explicitly `not-applicable`; compile-time and
generated-source-size costs are measured by the target-qualified PERF-001
compile plane.
The A0 `std.reflect` owner has the same explicit boundary in
[`testing/stdlib-reflect.json`](../../testing/stdlib-reflect.json): its
metadata-only implementation has no runtime host adapter, and its roots,
privacy, artifact-local identity and no-value-reflection evidence are kept in
the same per-owner cell record. Link-work and descriptor-size costs remain in
the target-qualified PERF-001 compile plane rather than being inferred from
unit-test timing.
The intrinsic `std.bytes` owner is closed by
[`testing/stdlib-bytes.json`](../../testing/stdlib-bytes.json) and its
`STD-A-BYTES-EVIDENCE-001` cell record. Its evidence covers identity,
immutable snapshots, strict UTF-8, builder atomicity, limits/ranges and the
scalar-oracle hot paths; `HOST` is explicitly `not-applicable` and
`STD-A-FUZZ-001` promotes the owner-aware route. Dedicated performance
promotion remains pending.
The intrinsic `std.core` owner is closed for hosted evidence by the group
contract [`testing/stdlib-core.json`](../../testing/stdlib-core.json) and its
`STD-A-CORE-EVIDENCE-001` cell record. The nine `Option`/`Result` signatures
are traced through static protocol checks, generic specialization, composition,
bytecode aggregates and VM execution. `HOST` is explicitly `not-applicable`
because the owner is compiler/VM-owned; admission fuzz covers the generated
Option/Result and protocol shapes; `STD-A-FUZZ-001` promotes the operation
route while the intrinsic cost is measured by the PERF-001 compiler/VM plane.
The intrinsic `std.text` owner is closed by the shared group contract
[`testing/stdlib-core.json`](../../testing/stdlib-core.json) and its
`STD-A-TEXT-EVIDENCE-001` cell record. All fifteen `String` signatures are
traced through static protocol checking, generic specialization, compiler/VM
value construction and the Unicode runtime fixtures: scalar indexing and
slicing, iteration, search/replace, ASCII transforms and atomic invalid UTF-8
rejection are covered. `HOST` is explicitly `not-applicable`; bounded UTF-8
corpora are linked, and `STD-A-FUZZ-001` promotes the operation route while the
owner cost remains in the PERF-001 compiler/VM plane.
The intrinsic `std.collections` owner is closed by the shared group contract
[`testing/stdlib-core.json`](../../testing/stdlib-core.json) and its
`STD-A-COLL-EVIDENCE-001` cell record. All eighteen `Array`, `Map` and `Set`
signatures are traced through HIR/MIR, bootstrap intrinsics, bytecode and VM
execution. The runtime fixture covers value semantics with internal COW,
atomic capacity errors, insertion order, key membership, replacement/removal
and lazy map/set iteration; lowering properties compare eager and COW copies
and the admission corpus exercises the intrinsic collection shapes. `HOST` is
explicitly `not-applicable`; `STD-A-FUZZ-001` promotes the operation route,
while memory/hash cost remains in the PERF-001 compiler/VM plane; global
conformance is promoted by `STD-A-CONF-001`.
The intrinsic `std.iter` owner is closed by the shared group contract
[`testing/stdlib-core.json`](../../testing/stdlib-core.json) and its
`STD-A-ITER-EVIDENCE-001` cell record. All four `Iterator` signatures are
traced through static HIR protocol checking, MIR/bytecode lowering and VM
execution. The runtime fixture covers lazy single-consumption composition,
synchronous callbacks and closures, qualified/generic dispatch, negative
`take`, bounded `collect`, borrowed cursors, user-iterator dispatch and
exhaustion guards; the VM heap tests trace sources and callbacks and reject
malformed iterator state. `HOST` is explicitly `not-applicable`;
`STD-A-FUZZ-001` promotes the operation route, while owner
retention/allocation/materialization cost remains in the PERF-001 compiler/VM
plane; global conformance is promoted by `STD-A-CONF-001`.
The intrinsic `std.math` owner is closed by the shared group contract
[`testing/stdlib-core.json`](../../testing/stdlib-core.json) and its
`STD-A-MATH-EVIDENCE-001` cell record. All nine scalar signatures are traced
through HIR static dispatch, the `process_host` bridge, the nominal
`MathError` boundary and the public runtime fixture. Kernel tests and the
`m6-num-004-ieee.to` corpus cover rounding ties, signed zero, infinities, NaN,
subnormals, overflow and the `sqrt` domain/non-finite distinction; compiler
properties cover Float32 rounding and compile-time NaN diagnostics. The
portable scalar implementation is the 0.1 scalar oracle: there is no separate
SIMD or fast-math path, and any future vectorized backend must prove bitwise
equivalence before promotion. `HOST` is explicitly `not-applicable`;
`STD-A-FUZZ-001` promotes the operation route; `std.math.fma` has a promoted
baseline for all six workloads and eight dimensions; global conformance is
promoted by `STD-A-CONF-001`.
The intrinsic `std.format` owner is closed by the shared group contract
[`testing/stdlib-core.json`](../../testing/stdlib-core.json) and its
`STD-A-FMT-EVIDENCE-001` cell record. All five signatures are traced through
static `Display` dispatch, bounded builders, HIR/MIR/bytecode verification,
the compiler/VM bridge and `m11-std-format-001.to`. Exact-limit properties,
separator boundaries, empty inputs, invalid receivers and failing `Display`
implementations prove that rejected appends do not mutate observable output.
`HOST` is explicitly `not-applicable`; `STD-A-FUZZ-001` promotes the operation
route and `std.format.join` has a promoted baseline for all six workloads and
eight dimensions, while the public API and documentation links are verified.
The portable `std.io` owner is closed by the shared group contract
[`testing/stdlib-core.json`](../../testing/stdlib-core.json) and its
`STD-A-IO-EVIDENCE-001` cell record. Reader/Writer, `IoLimits`, `readAll` and
`writeAll` are traced through HIR/lowering, bytecode/VM and
`m11-std-io-001.to`; deterministic chunk partitions, EOF, partial progress,
invalid chunks, zero-progress writers, post-write flush failures and
cancellation prove bounded atomic outcomes. `HOST` is explicitly
`not-applicable` because console, filesystem and process own the hosted
adapters; `STD-A-FUZZ-001` promotes the operation route; `std.io.read_write_all`
has a promoted baseline for all six workloads and eight dimensions; global
conformance is promoted by `STD-A-CONF-001`.
The capability-gated `std.time` owner is closed for hosted evidence by
[`testing/stdlib-time.json`](../../testing/stdlib-time.json) and its
`STD-A-TIME-EVIDENCE-001` cell record. It separates the duration/instant/timer
model, real and virtual monotonic providers, checked limits, timer lifecycle,
and explicit `clock` capability/conformance. `HOST` is verified at the single
`process_host` boundary; `STD-A-FUZZ-001` promotes the owner-aware route while
provider-scoped performance remains pending.

The capability-gated `std.env` owner is closed for hosted evidence by
[`testing/stdlib-env.json`](../../testing/stdlib-env.json) and its
`STD-A-ENV-EVIDENCE-001` cell record. It separates the runtime capability and
availability boundary, sealed snapshots, ordered arguments, raw/text values,
strict names, missing-entry options, independent copies and atomic limits.
`HOST` is verified at `process_host`; ambient-environment isolation is tested
explicitly, while `STD-A-FUZZ-001` covers the owner-aware route and
capability-scoped performance promotion remains pending.

The pure `std.path` owner is closed for hosted evidence by its shared contract
[`testing/stdlib-hosted.json`](../../testing/stdlib-hosted.json) and the
`STD-A-PATH-EVIDENCE-001` cell record. Its model keeps native bytes and UTF-8
as an exact lexical snapshot, makes NFC/NFD and `.`/`..` preservation explicit,
and rejects NUL, separators in components and the 32 KiB resource boundary
atomically. The bounded deterministic corpus and compiler/VM host fixture
exercise every public operation without touching the filesystem; `HOST` is
explicitly `not-applicable`. `STD-A-FUZZ-001` promotes the owner-aware route;
`std.path.lexical` has a promoted baseline for all six workloads and eight
dimensions; global conformance is promoted by `STD-A-CONF-001`.

The capability-gated `std.console` owner is closed for hosted evidence by the
same shared contract and the `STD-A-CONSOLE-EVIDENCE-001` cell record. Its
seven public signatures reuse the single `std.io` Reader/Writer model while
keeping stdin, stdout and stderr as distinct tokens. The static capability
check rejects a target without `console`; the host fixture covers partial
reads, EOF, stable LF output, explicit flush, invalid UTF-8, wrong-stream
handles and typed/redacted failures without publishing partial state. `HOST`
is verified at the compiler/VM boundary. `STD-A-FUZZ-001` promotes the
operation route; target hot-path cost is measured by the PERF-001 hosted plane
and global conformance is promoted by `STD-A-CONF-001`.

The capability-gated `std.fs` owner is closed for hosted evidence by the same
shared contract and the `STD-A-FS-EVIDENCE-001` cell record. Its fourteen public
signatures are traced through the static `filesystem` capability gate, the
affine `File`/`Directory` model, HIR/lowering, bytecode/VM and the process-host
adapter. The runtime fixture covers native-byte paths, ordered directory
listing, typed errors, short reads/writes, bounded materialization,
`atomicWrite`, stale-token rejection, cancellation and cleanup on unwind;
`HOST` is verified. `STD-A-FUZZ-001` promotes the owner-aware route; target
hot-path cost is measured by the PERF-001 hosted plane and global conformance
is promoted by `STD-A-CONF-001`.

The capability-gated `std.process` owner is closed for hosted evidence by the
`STD-A-PROC-EVIDENCE-001` cell record. Its seventeen public signatures are
traced through the explicit `process` capability, inert `Command`/`Pipeline`
plans, terminal `ProcessHandle`, HIR/lowering, bytecode/VM and the
`process_host` adapter. M8 fixtures and host tests cover exact argv, explicit
shell use, all four pipe shapes, bounded backpressure, separate and combined
output, typed stderr redirection, exit data versus recoverable errors,
cancellation, panic/unwind cleanup and child reaping; `HOST` is verified.
`STD-A-FUZZ-001` promotes the owner-aware route; target hot-path cost is
measured by the PERF-001 hosted plane and global conformance is promoted by
`STD-A-CONF-001`.

The portable `std.serialization` owner is closed for evidence by the
`STD-A-SER-EVIDENCE-001` cell record. Its common `Encoder`/`Decoder` and
`Encode`/`Decode` protocols, explicit event frames, dynamic value views, raw
bytes, bounded chunking and publish-after-validation rule are traced through
the stdlib kernel. The hermetic build-only providers preserve codec identity,
field order, attributes, source maps and diagnostics for records, enums,
newtypes and generics; tests cover limits, duplicate fields, lengths and
atomic failures. `HOST` is not applicable; `STD-A-FUZZ-001` promotes the
event-protocol route; `std.serialization.events` has a promoted baseline for
all six workloads and eight dimensions, while global conformance is promoted
by `STD-A-CONF-001`.

The portable `std.json` owner is closed for evidence by the
`STD-A-JSON-EVIDENCE-001` cell record. Its typed, dynamic and streaming routes
share the explicit-frame parser and bounded writer; tests cover exact decimal
numbers, Unicode, duplicate-field policies, JCS/RFC 8785 canonicalization,
limits, terminal errors, one-byte fragmentation and bidirectional
`serde_json` interoperability. `HOST` is not applicable because the compiler
bridge has no ambient capability or target-specific codec semantics.
`STD-A-FUZZ-001` promotes the operation route; `std.json.parse_encode` has a
promoted baseline for all six workloads and eight dimensions; global
conformance is promoted by `STD-A-CONF-001`.

The portable `std.messagepack` owner is closed for evidence by the
`STD-A-MSGPACK-EVIDENCE-001` cell record. Its typed, dynamic and streaming
routes cover every wire family, non-minimal forms, arbitrary map keys,
float-bit policy, binary versus UTF-8, extension/timestamp preservation,
deterministic ordering, finite limits and terminal errors. Tests prove
one-byte fragmentation equivalence and bidirectional interoperability with
`rmpv`; `HOST` is not applicable because the compiler bridge has no ambient
capability or target-specific wire semantics. `STD-A-FUZZ-001` promotes the
operation route; `std.messagepack.decode_encode` has a promoted baseline for
all six workloads and eight dimensions, while global conformance is promoted
by `STD-A-CONF-001`.

The portable/build-only `std.protobuf` owner is closed for evidence by the
`STD-A-PROTOBUF-EVIDENCE-001` cell record. Its TOML schema-first boundary and
proto3 wire routes cover closed imports, generated identities, presence,
repeated/packed fields, maps, oneof, open enums, unknown fields/groups,
deterministic encoding, bounded schema/message limits and safe versus unsafe
evolution. Tests prove schema-bound streaming, one-byte fragmentation,
atomic terminal failures and bidirectional `prost` interoperability. `HOST`
is not applicable because wire execution is portable and generation is
hermetic build-only; `STD-A-FUZZ-001` promotes the schema/operation route,
`std.protobuf.decode_message` has a promoted baseline for all six workloads and
eight dimensions, while global conformance is promoted by `STD-A-CONF-001`.

The test-only `std.testing` owner is closed for evidence by
`STD-A-TESTING-EVIDENCE-001`. Its typed assertions, bounded text diffs and
float tolerances, affine Option/Result consumption, isolated temporary roots,
deterministic generators, compiler-sealed shrinking and sealed runner bridge
are traced through stdlib, compiler and VM paths. Acceptance projects dogfood
control terminals, retries/repeats, selection/sharding, JSON/JUnit reports and
the production-import rejection; `HOST` is verified at the worker bridge.
`STD-A-FUZZ-001` now provides the dedicated `std.testing` route;
`STD-A-PERF-001` promotes its eight performance dimensions, and
`STD-A-CONF-001` promotes the independent public execution record rather than
inferring conformance from unit-test execution.

`STD-TEST-001` closes the cross-owner test coordination in
`testing/stdlib-test-coordination.json`: all 214 public signatures and 171
owner requirements have a model law, executable test commands and an explicit
fuzz campaign or bounded-corpus reason. The generated registry is checked
against the public API, normative matrix and owner evidence; performance and
conformance remain visible and are not promoted by this coordination step.

`STD-CONF-001` closes the conformance coordination in
`testing/stdlib-conformance-coordination.json`: every one of the 385 normative
matrix rows has an explicit `CONF` record with status, reason, references and
commands across all 22 owners. `STD-A-CONF-001` adds the executable registry
[`testing/stdlib-conformance.json`](../../testing/stdlib-conformance.json) and
the runner `scripts/stdlib-conformance.sh`: every owner command, every unique
public runtime fixture (including platform argument sidecars), and all 206
draft cases run before promotion. The generated evidence is bound to the
current tree, manifest, contract and command-log hashes; the matrix has
385/385 verified rows and no partial or pending `CONF` cells. Codec owners
retain their external bidirectional/fragmented cases, and `std.async` retains
its seven-row contract and public fixtures. This promotes conformance for the
unpublished draft only; `STD-A-DIST-001` is the next block and the S1A seal is
still open.

The hosted bridge is intentionally a draft distribution boundary: the VM
validates the typed operation before invoking the host, and the host returns a
nominal error or a complete value. Console streams use the same `Reader` and
`Writer` tokens for stdin, stdout and stderr; filesystem and process handles
retain their existing terminal cleanup contracts.

## Evidence matrix

The owner manifest points to the executable source and fixture for each A0–A4
module. The most representative end-to-end programs are:

- `m11-std-core-001.to` — arrays, ordered maps/sets, ranges and formatting;
- `m11-std-text-001.to` — scalar/byte text operations;
- `m11-std-codecs-001.to` — JSON, MessagePack and Protobuf validation;
- `m11-std-path-001.to` and `m11-std-fs-001.to` — native-byte paths and safe
  atomic filesystem operations;
- `m11-std-console-001.to` — stable stdout output; and
- the M8 process fixtures — exact argv, pipes, cancellation and reaping.

Portable unit tests add official wire vectors, truncated/adversarial inputs,
duplicate policies, unknown Protobuf records, partial I/O, finite limits,
float rules, generator replay and deterministic shrinking. The hosted tests
exercise absent/invalid capabilities, native path bytes, separate output
channels and typed errors without publishing partial values.

## Conformance, performance and lineage

`scripts/stdlib-codec-conformance.sh` runs the three portable codec suites, the
hosted bridge test and a bidirectional external interoperability harness
against `serde_json`, `rmpv` and `prost`. The harness also records one-byte
fragmentation, truncation, finite limits and unknown-wire preservation in
`testing/stdlib-codec-conformance.json`. `scripts/stdlib-performance-report.sh` captures
27 independent samples per hot-path module from three processes, including the
recorded environment and measured dimensions. The owner coordinator in
`testing/stdlib-performance-conformance.json` rejects omitted owners, deferred
dimensions and overstated measurements. Ten portable owners carry promoted
baselines for all six workloads and eight dimensions; the remaining twelve
owners carry a normative `not-applicable` boundary to the target-qualified
`PERF-001` compiler/VM or hosted plane. The strict gate runs the report,
coordinator, negative coordinator tests, complete workspace tests and draft
conformance adapter together.

The live conformance lineage is explicitly
`conformance/draft/manifest.json` and its base suite is
`conformance/0.1/manifest.json`; both describe the same current draft. Generated
reports stay under `target/reliability/evidence` and are
reproducible from the commands recorded in the owner manifest.

The complete owner/signature/requirement coordination is kept separately in
[`docs/contracts/stdlib-matrix.md`](./stdlib-matrix.md) and
[`testing/stdlib-matrix.json`](../../testing/stdlib-matrix.json). It records
the six required cells `SPEC → IMPL/HOST → MODEL/TEST/FUZZ → PERF → CONF →
DOC`, including explicit reasons for every pending or partial cell. The
matrix currently has 22 owners (the intrinsic `std.bytes` is intentionally
visible even though the bootstrap implementation manifest still lacks its
dedicated owner record), 214 public signatures and 171 owner requirements.
This is coordination evidence, not a publication or a claim that all rows are
green.

The documentation coordination is kept in
[`testing/stdlib-documentation.json`](../../testing/stdlib-documentation.json).
Its checker validates every owner contract, boundary reference, example source,
runtime sidecar and recorded command; the negative tests and
`stdlib_documentation` Rust test reject missing examples and API overclaims.

## Coverage and release boundary

The non-regression floor is the reviewed global line baseline of 9025 basis
points (`testing/quality-baseline.json`). A fresh `cargo llvm-cov` report must
meet or exceed it after every implementation commit. S1A is a technical gate:
publication still requires final PackageId/content/API hashes, target matrix,
provider hashes, interoperability/fuzz/streaming evidence and every unchecked
item in section 20 of the standard-library specification.
