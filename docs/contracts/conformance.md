# Tondo 0.1 live draft conformance

**Status:** active development contract; Tondo has not been published.

Tondo has one language line: the current unpublished 0.1 draft. Git is the
history. The repository does not keep selectable old grammars, source adapters,
specification snapshots, conformance revisions, promotion proofs, release
candidates, or frozen result sets while the language is still evolving.

## Live inputs

The active conformance boundary consists of:

- `TONDO_LANGUAGE_SPEC.md`, `TONDO_TESTING_SPEC.md` and
  `TONDO_TOOLCHAIN_SPEC.md`;
- `conformance/0.1/manifest.json` and its cases, expectations and fixtures;
- `conformance/draft/manifest.json` and the current case layers; and
- generated inventory, coverage and reliability evidence bound to the current
  source tree.

`tondo-conformance-maintain bless` reads the current language specification and
executes the current compiler/VM adapter. It rewrites the live suite atomically
from observed behavior. It never translates deprecated syntax before parsing.
A syntax removed from Tondo may remain only as a negative case that proves its
diagnostic.

The suite registry contains the diagnostic and panic identities demonstrated by
executable cases. The complete normative registry still lives in the language
specification; pending identities remain pending in the coverage matrix until a
case implements them. A positive neighbor is emitted only for a code that the
same live suite covers.

## Manifest and adapter contracts

The formats retain the word `draft` because no compatibility promise exists:

- `tondo-conformance-manifest-draft` for the executable suite;
- `tondo-conformance-adapter-draft` for the process adapter; and
- `tondo-conformance-result-draft/2` for composed observations.

The manifests are canonical JSON. Every source, expectation, specification,
fixture and case layer is pinned by SHA-256 before execution. Identifying arrays
and IDs are sorted and unique. Missing, extra, duplicated, reordered, stale or
cross-tree records fail closed.

The generic runner never links compiler internals. The reference adapter uses
public compiler and VM APIs and cannot skip an applicable case. `unsupported`
is a failure unless the target declaration omits a required capability and the
case proves the `E1008` boundary.

The runner observes compilation state, exit code, structured diagnostics, exact
stdout/stderr bytes, formatter bytes and typed JSON data. Diagnostic ordering,
spans, related locations, fixes and payload schemas are checked at the adapter
boundary.

`conformance/draft/manifest.json` pins the current suite and specifications
directly, without parent or revision metadata. Case layers below
`conformance/draft/layers/` add implementation evidence without creating
another language lineage.

## Coverage and evidence

`covers` requires the claimed diagnostic or panic to occur. `positive_for`
requires a neighboring case where the diagnostic does not occur. Memory cases
expose reachability, cycles, pressure and retry properties without exposing VM
addresses or collector layout. Determinism cases compile a closed project with
different source insertion orders and require identical public hashes.
Concurrency cases repeat closed outcome contracts; wall-clock timing and one
scheduler order are not normative.

`tondo-reliability layer-evidence attest` binds each Rust witness to the
inventoried source hash and content-addressed source tree. The tree is captured
before tests and must remain unchanged. `tondo-conformance run` composes the
live suite result and current layer evidence atomically.

Generated results under build output are ephemeral evidence. CI validates them
against the current tree; it does not compare them to checked-in results from an
older draft.

## Commands

From a current checkout:

```text
cargo run -p tondo-reference-adapter --bin tondo-conformance-maintain -- bless
cargo build -p tondo-conformance -p tondo-reference-adapter --bins --locked
cargo run -p tondo-reliability --locked -- quality provenance --root . \
  > target/reliability/evidence/layer-evidence-before.json
cargo test --workspace --all-targets --locked \
  2>&1 | tee target/reliability/evidence/logs/test.log
cargo run -p tondo-reliability --locked -- layer-evidence attest \
  --root . \
  --test-log target/reliability/evidence/logs/test.log \
  --before target/reliability/evidence/layer-evidence-before.json \
  --output target/reliability/evidence/layer-evidence.json
cargo run -p tondo-conformance --locked -- validate \
  --root . --manifest conformance/draft/manifest.json --lineage draft
cargo run -p tondo-conformance --locked -- run \
  --root . --manifest conformance/draft/manifest.json --lineage draft \
  --adapter target/debug/tondo-reference-adapter \
  --evidence target/reliability/evidence/layer-evidence.json \
  --output target/reliability/evidence/conformance-result.json
```

Repeated runs over an unchanged tree must produce byte-identical output.

## First release boundary

Immutable promotion proofs, candidate bundles, compatibility readers and
release notes begin only when Tondo enters its first actual release process.
That future block must define the public compatibility policy and seal a clean
snapshot deliberately. It must not infer a release lineage from pre-release
draft artifacts.
