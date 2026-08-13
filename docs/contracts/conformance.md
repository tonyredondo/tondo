# Tondo 0.1 draft conformance

**Status:** active development contract; Tondo has not been published.

There is one current language line: the unpublished Tondo 0.1 draft. The
bootstrap corpus is a regression suite for behavior that is already executable;
it is not a second language version, release, or parser. Git history is the
only place where older drafts are historical artifacts.

## Records and boundaries

`tondo-conformance-draft` is the implementation-independent runner and corpus
protocol. Its records use explicit `draft` identifiers:

- `tondo-conformance-manifest-draft` for the suite manifest;
- `tondo-conformance-adapter-draft` for the adapter protocol; and
- `tondo-conformance-result-draft/2` for composed observations. The unversioned
  `tondo-conformance-result-draft` shape remains only as the immutable bootstrap
  regression input.

The manifest is compact canonical JSON. Identifying arrays and case IDs are
sorted and unique. Every source, expectation, specification, and fixture is
pinned by SHA-256 before an adapter starts. A missing expectation is always a
failure.

The generic runner never links compiler internals. The reference adapter may
use the public compiler and VM APIs, but it cannot skip an applicable case.
`unsupported` is a failure unless the target declaration explicitly omits the
required capability and the case proves the `E1008` boundary.

The runner observes compilation state, exit code, structured diagnostics, exact
stdout/stderr bytes, formatter bytes, and one typed JSON payload for semantic,
memory, determinism, build, or documentation data. Diagnostic codes, ordering,
spans, IDs, related locations, fixes, and payload schemas are checked at the
public adapter boundary.

The `0.1/1` suffixes in auxiliary observation schemas are schema revisions, not
Tondo language releases. They may change while this draft is unpublished; a
future release will deliberately choose its public compatibility policy.

## Single draft manifest

`conformance/draft/manifest.json` is the only active G5 lineage manifest. It pins
language, testing and toolchain, the open state, pending implementation tasks,
and optional case layers. It has no release tag, release commit, or parallel
lineage selector. Standard Library uses its separate S1A/S1 contract; pinning
its document never implies G5 coverage. The CLI accepts only `--lineage draft` so a stale historical
identity cannot be selected accidentally.

Case layers are added in the same change as the implementation slice they
exercise. Their IDs, task IDs, and requirement IDs are sorted and unique, and
their manifests live below `conformance/draft/layers/`. A layer declaration
without inventory evidence never turns a requirement green. Inventory identity
alone does not attest execution: `CONF-LAYER-RESULT-001` must bind every layer
case to a fresh composed result before final sealing.

`tondo-reliability layer-evidence attest` consumes the fresh `cargo test` log,
requires every Rust witness named by every active layer to have passed exactly
once, and binds each observation to its inventoried source hash and the
content-addressed source tree. The input tree is captured before the test run
and must be unchanged afterward. A campaign-only identifier is never promoted
as an individual execution witness.

`run --lineage draft` then executes the complete pinned bootstrap regression
suite and composes it atomically with that evidence report. The result fixes the
draft lineage, revision, manifest hash, inventory hash, source-tree identity,
all layers, all cases and every evidence observation in their canonical order.
Missing, extra, duplicated, reordered, stale or cross-tree records fail closed.
Removing the composed fields reproduces the immutable bootstrap result byte for
byte; the regression input is never rewritten.

The bootstrap suite is stored at `conformance/0.1/manifest.json` and uses the
specification snapshot in `conformance/baseline/TONDO_LANGUAGE_SPEC.md`. Those
files are regression inputs only. They have no public release identity and may
be replaced by the first complete draft corpus once the corresponding features
are implemented.

## Coverage contract

`covers` requires the claimed diagnostic or panic to occur in the observation.
`positive_for` requires a neighboring case where that code does not occur.
The draft suite requires the complete registry, warning profiles, formatter,
semantic queries, memory scenarios, concurrency repetitions, and determinism
case for the executable subset. The coverage matrix and ratchet are the source
of truth for requirements that are still pending.

Memory cases expose only reachability, cycles, pressure, and retry properties;
they never expose VM addresses or collector layout. Determinism cases compile a
closed project twice with canonical and inverse source insertion order and
require equal interface, artifact, and diagnostic hashes. Concurrency cases
repeat their closed outcome contract at least 32 times; wall-clock timing and a
particular scheduler order are never normative.

## Commands

From a current checkout:

~~~text
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
~~~

Repeated runs over an unchanged tree must produce byte-identical output. The
`seal-proof` command produces an immutable proof of the promotion mechanism; it
is not a release candidate and does not publish a language release. It fails
closed unless the draft has no
pending tasks, the current ratchet has validated coverage and mutation
identities, and the supplied fresh reference result matches the pinned
bootstrap regression exactly.

The proof is a canonical, content-addressed bundle. Its manifest fixes the
exact draft revision and hash, target, adapter implementation, three G5
specifications, its canonical parent chain through revision 1, case layers,
regression inputs, reference result, ratchet, reliability records, and
reference-adapter source. Verification reconstructs the exact allowed
role/path set and rejects omissions, substitutions, or extra provenance rather
than merely checking that named hashes exist. Each byte sequence lives at
`objects/<sha256>` and every source provenance points to that object. Objects
are written and synchronized in a sibling staging directory, the manifest is
written last, and one atomic directory rename makes the proof visible.
An identical destination is idempotent; a partial, altered, extra, symlinked,
or different destination is rejected rather than repaired or overwritten.

Verification follows only the proof's closed object graph. It deliberately
does not consult live draft files, so later work cannot silently change an
already published proof. `conformance/0.1` remains an immutable regression
input and is never used as the proof destination. Each revision uses its own
`conformance/proofs/revision-<N>` directory; a later proof never replaces an
older one.

~~~text
cargo run -p tondo-conformance --locked -- seal-proof \
  --root . --manifest conformance/draft/manifest.json --lineage draft \
  --result target/reliability/evidence/conformance-result.json \
  --output conformance/proofs/revision-<N>
cargo run -p tondo-conformance --locked -- verify-proof \
  --root . --proof conformance/proofs/revision-<N>
~~~

## Final candidate bundle

A promotion proof demonstrates that the content-addressed sealing mechanism is
closed and reproducible. It does not, by itself, attest that every applicable
language, testing and toolchain requirement is covered. The final candidate is
therefore a distinct `tondo-conformance-candidate/2` bundle. Its explicit state
is `candidate`, its gates are exactly `G5` and `T0`, and creating it still does
not publish Tondo or create a language release.

The candidate embeds the complete promotion-proof object graph plus the fresh
raw coverage and mutation reports, their provenance bindings, the composed
layer-evidence report, the complete doc-test report and the typed-fence runtime
link registry. Its verifier follows only these archived objects and fails
closed unless all of the following hold together:

- the embedded promotion proof verifies offline and has the same lineage,
  target and adapter identity as the candidate;
- the composed result, layer evidence and quality ratchet share the exact draft
  revision, manifest, inventory, source tree and measured input set;
- every applicable `TL01`, `TT01` and `TC01` requirement is `covered` or an
  individually justified `target-not-applicable`; only the `TL01-26-*` Standard
  Library boundary may remain `stdlib-pending`, because S1A/S1 is sealed
  independently;
- the raw quality reports match their bindings and the ratchet, then satisfy the
  checked-in coverage and mutation baseline; and
- every doc-test record has the required category-specific outcome, every typed
  fence has exactly one typed link, and every runtime link names executable
  inventoried evidence. A static-only link needs an explicit non-empty reason.

The destination is immutable and content-addressed in the same way as a
promotion proof: objects are synchronized before the manifest, publication is
one atomic rename, identical regeneration is idempotent, and a partial,
different, extra or symlinked destination is rejected. Each draft revision uses
`conformance/candidates/revision-<N>`; no candidate overwrites another revision.

After the ordinary quality, composed-conformance and doc-test gates have
produced their fresh evidence, the repository wrapper seals and verifies the
current revision:

~~~text
scripts/conformance-candidate.sh generate
scripts/conformance-candidate.sh check
~~~

The equivalent explicit commands are `tondo-reliability candidate seal` with a
workspace-relative path for every input and `tondo-reliability candidate
verify --root . --candidate conformance/candidates/revision-<N>`. Requiring
relative normal paths prevents the manifest from depending on a developer's
machine layout while still allowing build artifacts to live on an external
disk before they are copied into the repository-local staging directory.
