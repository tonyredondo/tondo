# STD-0.1A implementation and S1A evidence

Status: `implemented-draft`. This document closes the technical Wave 5/S1A
gate for the current Tondo draft; it does not publish `STD-0.1.0` and does not
replace the publication checklist in `TONDO_STANDARD_LIBRARY_SPEC.md`.

## One owner, one implementation boundary

`testing/stdlib-implementation.json` is the machine-readable owner closure.
La trazabilidad por firma se comprueba además con
[`stdlib-public-api-audit.md`](./stdlib-public-api-audit.md), cuya matriz viva
puede permanecer abierta mientras los leaves de implementación sigan
pendientes.
Every owner has one canonical implementation boundary, source-controlled tests
and a proof description. Portable kernels live in `tondo-stdlib`; compiler and
VM bridges are limited to intrinsic lowering or capability-gated host effects.
There is no second public package, no ambient lookup and no general FFI ABI.

The aggregate owner graph and capability/API rules live in the single
machine-readable integration contract
[`testing/stdlib-spec.json`](../../testing/stdlib-spec.json). The strict gate
validates its topological order and links it to this document and the canonical
standard-library specification; it does not promote pending typed codecs.

The A0 `std.meta` owner has a dedicated contract at
[`testing/stdlib-meta.json`](../../testing/stdlib-meta.json) and a separate
cell record at [`testing/stdlib-owner-evidence.json`](../../testing/stdlib-owner-evidence.json).
Its build-only `HOST` cell is explicitly `not-applicable`; compile-time and
generated-source-size budgets remain visible as pending promotion evidence.
The A0 `std.reflect` owner has the same explicit boundary in
[`testing/stdlib-reflect.json`](../../testing/stdlib-reflect.json): its
metadata-only implementation has no runtime host adapter, and its roots,
privacy, artifact-local identity and no-value-reflection evidence are kept in
the same per-owner cell record. Link-work and descriptor-size budgets remain
pending promotion capture rather than being inferred from unit-test timing.
The intrinsic `std.bytes` owner is closed by
[`testing/stdlib-bytes.json`](../../testing/stdlib-bytes.json) and its
`STD-A-BYTES-EVIDENCE-001` cell record. Its evidence covers identity,
immutable snapshots, strict UTF-8, builder atomicity, limits/ranges and the
scalar-oracle hot paths; `HOST` is explicitly `not-applicable`, while dedicated
performance and fuzz promotion remain pending.
The intrinsic `std.core` owner is closed for hosted evidence by the group
contract [`testing/stdlib-core.json`](../../testing/stdlib-core.json) and its
`STD-A-CORE-EVIDENCE-001` cell record. The nine `Option`/`Result` signatures
are traced through static protocol checks, generic specialization, composition,
bytecode aggregates and VM execution. `HOST` is explicitly `not-applicable`
because the owner is compiler/VM-owned; admission fuzz covers the generated
Option/Result and protocol shapes, while operation-specific fuzz and
owner-specific performance baselines remain pending promotion.
The intrinsic `std.text` owner is closed by the shared group contract
[`testing/stdlib-core.json`](../../testing/stdlib-core.json) and its
`STD-A-TEXT-EVIDENCE-001` cell record. All fifteen `String` signatures are
traced through static protocol checking, generic specialization, compiler/VM
value construction and the Unicode runtime fixtures: scalar indexing and
slicing, iteration, search/replace, ASCII transforms and atomic invalid UTF-8
rejection are covered. `HOST` is explicitly `not-applicable`; bounded UTF-8
corpora are linked, while an operation-specific fuzz target and owner cost
baseline remain pending promotion.
The intrinsic `std.collections` owner is closed by the shared group contract
[`testing/stdlib-core.json`](../../testing/stdlib-core.json) and its
`STD-A-COLL-EVIDENCE-001` cell record. All eighteen `Array`, `Map` and `Set`
signatures are traced through HIR/MIR, bootstrap intrinsics, bytecode and VM
execution. The runtime fixture covers value semantics with internal COW,
atomic capacity errors, insertion order, key membership, replacement/removal
and lazy map/set iteration; lowering properties compare eager and COW copies
and the admission corpus exercises the intrinsic collection shapes. `HOST` is
explicitly `not-applicable`; operation-specific fuzz, owner memory/hash
baselines and global conformance promotion remain pending.
The intrinsic `std.iter` owner is closed by the shared group contract
[`testing/stdlib-core.json`](../../testing/stdlib-core.json) and its
`STD-A-ITER-EVIDENCE-001` cell record. All four `Iterator` signatures are
traced through static HIR protocol checking, MIR/bytecode lowering and VM
execution. The runtime fixture covers lazy single-consumption composition,
synchronous callbacks and closures, qualified/generic dispatch, negative
`take`, bounded `collect`, borrowed cursors, user-iterator dispatch and
exhaustion guards; the VM heap tests trace sources and callbacks and reject
malformed iterator state. `HOST` is explicitly `not-applicable`; operation
fuzz, owner retention/allocation/materialization baselines and global
conformance promotion remain pending.
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
equivalence before promotion. `HOST` is explicitly `not-applicable`; operation
fuzz, owner cost baselines and global conformance promotion remain pending.
The intrinsic `std.format` owner is closed by the shared group contract
[`testing/stdlib-core.json`](../../testing/stdlib-core.json) and its
`STD-A-FMT-EVIDENCE-001` cell record. All five signatures are traced through
static `Display` dispatch, bounded builders, HIR/MIR/bytecode verification,
the compiler/VM bridge and `m11-std-format-001.to`. Exact-limit properties,
separator boundaries, empty inputs, invalid receivers and failing `Display`
implementations prove that rejected appends do not mutate observable output.
`HOST` is explicitly `not-applicable`; operation fuzz and owner allocation or
materialization baselines remain pending promotion, while the public API and
documentation links are verified.
The portable `std.io` owner is closed by the shared group contract
[`testing/stdlib-core.json`](../../testing/stdlib-core.json) and its
`STD-A-IO-EVIDENCE-001` cell record. Reader/Writer, `IoLimits`, `readAll` and
`writeAll` are traced through HIR/lowering, bytecode/VM and
`m11-std-io-001.to`; deterministic chunk partitions, EOF, partial progress,
invalid chunks, zero-progress writers, post-write flush failures and
cancellation prove bounded atomic outcomes. `HOST` is explicitly
`not-applicable` because console, filesystem and process own the hosted
adapters; dedicated operation fuzz, owner cost baselines and global
conformance promotion remain pending.
The capability-gated `std.time` owner is closed for hosted evidence by
[`testing/stdlib-time.json`](../../testing/stdlib-time.json) and its
`STD-A-TIME-EVIDENCE-001` cell record. It separates the duration/instant/timer
model, real and virtual monotonic providers, checked limits, timer lifecycle,
and explicit `clock` capability/conformance. `HOST` is verified at the single
`process_host` boundary; provider-scoped performance and dedicated fuzz
promotion remain pending.

The capability-gated `std.env` owner is closed for hosted evidence by
[`testing/stdlib-env.json`](../../testing/stdlib-env.json) and its
`STD-A-ENV-EVIDENCE-001` cell record. It separates the runtime capability and
availability boundary, sealed snapshots, ordered arguments, raw/text values,
strict names, missing-entry options, independent copies and atomic limits.
`HOST` is verified at `process_host`; ambient-environment isolation is tested
explicitly, while dedicated fuzz and capability-scoped performance promotion
remain pending.

The pure `std.path` owner is closed for hosted evidence by its shared contract
[`testing/stdlib-hosted.json`](../../testing/stdlib-hosted.json) and the
`STD-A-PATH-EVIDENCE-001` cell record. Its model keeps native bytes and UTF-8
as an exact lexical snapshot, makes NFC/NFD and `.`/`..` preservation explicit,
and rejects NUL, separators in components and the 32 KiB resource boundary
atomically. The bounded deterministic corpus and compiler/VM host fixture
exercise every public operation without touching the filesystem; `HOST` is
explicitly `not-applicable`. A dedicated operation fuzz target, owner cost
baselines and global conformance promotion remain pending.

The capability-gated `std.console` owner is closed for hosted evidence by the
same shared contract and the `STD-A-CONSOLE-EVIDENCE-001` cell record. Its
seven public signatures reuse the single `std.io` Reader/Writer model while
keeping stdin, stdout and stderr as distinct tokens. The static capability
check rejects a target without `console`; the host fixture covers partial
reads, EOF, stable LF output, explicit flush, invalid UTF-8, wrong-stream
handles and typed/redacted failures without publishing partial state. `HOST`
is verified at the compiler/VM boundary. Dedicated operation fuzz, target
hot-path cost baselines and global conformance promotion remain pending.

The capability-gated `std.fs` owner is closed for hosted evidence by the same
shared contract and the `STD-A-FS-EVIDENCE-001` cell record. Its fourteen public
signatures are traced through the static `filesystem` capability gate, the
affine `File`/`Directory` model, HIR/lowering, bytecode/VM and the process-host
adapter. The runtime fixture covers native-byte paths, ordered directory
listing, typed errors, short reads/writes, bounded materialization,
`atomicWrite`, stale-token rejection, cancellation and cleanup on unwind;
`HOST` is verified. Dedicated operation fuzz, target hot-path cost baselines and
global conformance promotion remain pending.

The capability-gated `std.process` owner is closed for hosted evidence by the
`STD-A-PROC-EVIDENCE-001` cell record. Its seventeen public signatures are
traced through the explicit `process` capability, inert `Command`/`Pipeline`
plans, terminal `ProcessHandle`, HIR/lowering, bytecode/VM and the
`process_host` adapter. M8 fixtures and host tests cover exact argv, explicit
shell use, all four pipe shapes, bounded backpressure, separate and combined
output, typed stderr redirection, exit data versus recoverable errors,
cancellation, panic/unwind cleanup and child reaping; `HOST` is verified.
Dedicated operation fuzz, target hot-path cost baselines and global conformance
promotion remain pending.

The portable `std.serialization` owner is closed for evidence by the
`STD-A-SER-EVIDENCE-001` cell record. Its common `Encoder`/`Decoder` and
`Encode`/`Decode` protocols, explicit event frames, dynamic value views, raw
bytes, bounded chunking and publish-after-validation rule are traced through
the stdlib kernel. The hermetic build-only providers preserve codec identity,
field order, attributes, source maps and diagnostics for records, enums,
newtypes and generics; tests cover limits, duplicate fields, lengths and
atomic failures. `HOST` is not applicable; dedicated event-protocol fuzz,
target hot-path baselines and global conformance promotion remain pending.

The portable `std.json` owner is closed for evidence by the
`STD-A-JSON-EVIDENCE-001` cell record. Its typed, dynamic and streaming routes
share the explicit-frame parser and bounded writer; tests cover exact decimal
numbers, Unicode, duplicate-field policies, JCS/RFC 8785 canonicalization,
limits, terminal errors, one-byte fragmentation and bidirectional
`serde_json` interoperability. `HOST` is not applicable because the compiler
bridge has no ambient capability or target-specific codec semantics. Dedicated
operation fuzz, allocation/memory and target baselines, and global
conformance promotion remain pending.

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
`testing/stdlib-performance-conformance.json` rejects omitted owners and
overstated dimensions; deferred owners carry an explicit reason until their
workload identity is reviewed. The strict gate runs the report, coordinator,
negative coordinator tests, complete workspace tests and draft conformance
adapter together.

The live conformance lineage is explicitly
`conformance/draft/manifest.json`. `conformance/0.1/manifest.json` and its
results remain historical and immutable; S1A never mixes them with the live
draft. Generated reports stay under `target/reliability/evidence` and are
reproducible from the commands recorded in the owner manifest.

The complete owner/signature/requirement coordination is kept separately in
[`docs/contracts/stdlib-matrix.md`](./stdlib-matrix.md) and
[`testing/stdlib-matrix.json`](../../testing/stdlib-matrix.json). It records
the six required cells `SPEC → IMPL/HOST → MODEL/TEST/FUZZ → PERF → CONF →
DOC`, including explicit reasons for every pending or partial cell. The
matrix currently has 22 owners (the intrinsic `std.bytes` is intentionally
visible even though the bootstrap implementation manifest still lacks its
dedicated owner record), 207 public signatures and 165 owner requirements.
This is coordination evidence, not a publication or a claim that all rows are
green.

## Coverage and release boundary

The non-regression floor is the reviewed global line baseline of 9025 basis
points (`testing/quality-baseline.json`). A fresh `cargo llvm-cov` report must
meet or exceed it after every implementation commit. S1A is a technical gate:
publication still requires final PackageId/content/API hashes, target matrix,
provider hashes, interoperability/fuzz/streaming evidence and every unchecked
item in section 20 of the standard-library specification.
