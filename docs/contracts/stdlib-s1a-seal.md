# STD-S1A seal contract

`STD-S1A-SEAL-001` is the final technical gate for the unpublished
`STD-0.1A` standard-library foundation. It is deliberately separate from the
language candidate seal (`CONF-SEAL-FINAL-001`/G5), the native backend (N1) and
the Tondo LLM Form companion (L0). A passing seal is evidence for the current
draft, not a release or a compatibility promise.

## Inputs and gate

The machine-readable contract is
[`testing/stdlib-s1a-seal.json`](../../testing/stdlib-s1a-seal.json). The
seal consumes the tracked owner, API, matrix, conformance, performance,
documentation, distribution and async/select registries. It also consumes
fresh reports from the current clean Git revision:

- `stdlib-conformance.json` (22 owners, 385 rows and 206 draft cases);
- `stdlib-performance-report.json` plus its promoted coordinator;
- `async-select-conformance.json` and `async-select-performance.json`; and
- the reproducible VM distribution evidence and archive.

The runner executes the contract checks and negative-test suites, then rejects
the seal if any of these is true:

- the working tree is dirty or a report is bound to another revision/tree;
- the strict public API audit is not `214/214` with zero gaps;
- the normative matrix contains an applicable open cell;
- FUZZ has fewer than 22 verified owners, PERF has a deferred dimension, or
  CONF/DIST/async-select evidence is not passed and draft-only;
- the archive is not the byte-identical result of two clean snapshots; or
- metadata claims G5, a native backend, TLF or a public release.

No state is promoted by editing a JSON status. Every claim is derived from the
registries and executable reports named above.

## Content-addressed bundle

`scripts/stdlib-s1a-seal.sh` creates
`target/reliability/evidence/stdlib-s1a-seal/`. It copies the closed inputs,
fresh reports, the VM distribution archive and the command transcripts into a
canonical directory. `metadata/manifest.json` sorts every payload by relative
path and records its SHA-256 and byte length. The payload hash is the SHA-256
of those canonical records, and the archive is named
`tondo-stdlib-s1a-<payload-sha256>.tar` with deterministic USTAR metadata.

The outer `seal.json` records the Git revision, the reliability tree hash,
manifest/payload/archive hashes, the exact bundle ID and the explicit
`public_release=false`, `g5=false`, `native_backend=false` and `tlf=false`
claims. The archive contains no `.git`, `target` or ambient workspace state.

`scripts/stdlib-s1a-seal-check.sh` verifies the outer record, canonical
manifest, every payload hash and the archive without reading the live source
tree. This permits a reviewer or CI job to copy the bundle elsewhere and
verify its integrity independently. The distribution archive remains nested
as evidence; it is not published by this gate.

The generated directory is intentionally under `target/` and is not a tracked
release artifact. A future release process may attach a reviewed bundle, but
that action requires the separate G5/S1 and publication decisions.

## Verification commands

```text
scripts/stdlib-s1a-seal.sh
scripts/stdlib-s1a-seal-check.sh
scripts/stdlib-s1a-seal-test.sh
```

The full `scripts/test-gate.sh` runs the seal after the S1A conformance,
performance, distribution and async-select steps. The seal has no dependency
on a native compiler, TLF codec, release credentials or an external service.
