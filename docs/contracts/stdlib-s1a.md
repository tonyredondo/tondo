# STD-0.1A implementation and S1A evidence

Status: `implemented-draft`. This document closes the technical Wave 5/S1A
gate for the current Tondo draft; it does not publish `STD-0.1.0` and does not
replace the publication checklist in `TONDO_STANDARD_LIBRARY_SPEC.md`.

## One owner, one implementation boundary

`testing/stdlib-implementation.json` is the machine-readable owner closure.
Every owner has one canonical implementation boundary, source-controlled tests
and a proof description. Portable kernels live in `tondo-stdlib`; compiler and
VM bridges are limited to intrinsic lowering or capability-gated host effects.
There is no second public package, no ambient lookup and no general FFI ABI.

The aggregate owner graph and capability/API rules live in the single
machine-readable integration contract
[`testing/stdlib-spec.json`](../../testing/stdlib-spec.json). The strict gate
validates its topological order and links it to this document and the canonical
standard-library specification; it does not promote pending typed codecs.

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

`scripts/stdlib-codec-conformance.sh` runs the three portable codec suites and
the hosted bridge test, while `scripts/stdlib-performance-report.sh` captures
27 independent samples per hot-path module from three processes. The strict
gate runs both reports together with the complete workspace tests and draft
conformance adapter.

The live conformance lineage is explicitly
`conformance/draft/manifest.json`. `conformance/0.1/manifest.json` and its
results remain historical and immutable; S1A never mixes them with the live
draft. Generated reports stay under `target/reliability/evidence` and are
reproducible from the commands recorded in the owner manifest.

## Coverage and release boundary

The non-regression floor is the reviewed global line baseline of 9025 basis
points (`testing/quality-baseline.json`). A fresh `cargo llvm-cov` report must
meet or exceed it after every implementation commit. S1A is a technical gate:
publication still requires final PackageId/content/API hashes, target matrix,
provider hashes, interoperability/fuzz/streaming evidence and every unchecked
item in section 20 of the standard-library specification.
