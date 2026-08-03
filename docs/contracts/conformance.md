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
- `tondo-conformance-result-draft` for observations.

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

`conformance/draft/manifest.json` is the only active lineage manifest. It pins
the four normative documents, the open state, the pending implementation tasks,
and optional case layers. It has no release tag, release commit, or parallel
lineage selector. The CLI accepts only `--lineage draft` so a stale historical
identity cannot be selected accidentally.

Case layers are added in the same change as the implementation slice they
exercise. Their IDs, task IDs, and requirement IDs are sorted and unique, and
their manifests live below `conformance/draft/layers/`. A layer declaration
without executable evidence never turns a requirement green.

Until the new draft features have executable layers, `run --lineage draft`
executes the pinned bootstrap regression suite and reports its exact result.
The reliability matrix separately marks requirements added or changed since
that corpus as pending; the regression run is not presented as complete
language conformance.

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
cargo run -p tondo-conformance --locked -- validate \
  --root . --manifest conformance/draft/manifest.json --lineage draft
cargo run -p tondo-conformance --locked -- run \
  --root . --manifest conformance/draft/manifest.json --lineage draft \
  --adapter target/debug/tondo-reference-adapter \
  --output conformance/0.1/results/tondo-reference-draft-tondo-vm-hosted.json
~~~

Repeated runs over an unchanged tree must produce byte-identical output. The
`seal` command promotes the completed draft into one immutable candidate; it
does not publish a language release. It fails closed unless the draft has no
pending tasks, the current ratchet has validated coverage and mutation
identities, and the supplied fresh reference result matches the pinned
bootstrap regression exactly.

The promotion is a canonical, content-addressed bundle. Its manifest fixes the
exact draft revision and hash, target, adapter implementation, four normative
specifications, case layers, regression inputs, reference result, ratchet,
reliability records, and reference-adapter source. Each byte sequence lives at
`objects/<sha256>` and every source provenance points to that object. Objects
are written and synchronized in a sibling staging directory, the manifest is
written last, and one atomic directory rename makes the candidate visible.
An identical destination is idempotent; a partial, altered, extra, symlinked,
or different destination is rejected rather than repaired or overwritten.

Verification follows only the candidate's closed object graph. It deliberately
does not consult live draft files, so later work cannot silently change an
already promoted candidate. `conformance/0.1` remains an immutable regression
input and is never used as the candidate destination.

~~~text
cargo run -p tondo-conformance --locked -- seal \
  --root . --manifest conformance/draft/manifest.json --lineage draft \
  --result target/reliability/evidence/conformance-result.json \
  --output conformance/candidate
cargo run -p tondo-conformance --locked -- verify-candidate \
  --root . --candidate conformance/candidate
~~~
