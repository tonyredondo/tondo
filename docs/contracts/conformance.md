# Tondo 0.1 conformance suite

**Status:** pinned by the internal Tondo 0.1.0 checkpoint

`tondo-conformance-0.1` is an immutable corpus and a runner protocol, not a
test-only view of the reference compiler. The suite distribution contains:

- one canonical `tondo-conformance-manifest-0.1/1`;
- every source, expectation, specification and fixture input pinned by SHA-256;
- a generic runner that has no dependency on the Tondo compiler or VM; and
- an implementation-specific adapter speaking
  `tondo-conformance-adapter-0.1/1`.

This contract describes the corpus at Git tag `v0.1.0`. The current Tondo 0.1
draft has added M10.7 metaprogramming and M10.6 testing requirements, including
`defer await`, `suite`, and `test`. Those rules belong to the separate open
lineage `tondo-0.1-live`; they do not retroactively change the checkpoint or
inherit its evidence merely because they remain in edition 0.1.

The manifest is compact canonical JSON. Identifying arrays and case IDs are
sorted and unique. Paths are logical, relative and portable. Loading the suite
checks every referenced byte string before an adapter is started. A missing
expectation never means success.

The checkpoint's hosted process cases themselves preserve Linux command paths
and POSIX shell payloads from the internal `v0.1.0` run. Consequently, every
host can load and validate the checkpoint identity, but reproducing its exact
reference result is Linux-only. The immutable case bytes are not translated or
silently skipped by the runner. Current cross-platform behavior is exercised
by the live repository fixtures, whose paired `.args-unix` and `.args-windows`
sidecars provide explicit host commands without changing Tondo source.

## Checkpoint and live lineage

`conformance/0.1/manifest.json` is the immutable checkpoint identity. Its
language specification hash refers to the bytes at tag `v0.1.0`, not to the
working tree. The repository materializes those exact bytes at
`conformance/checkpoints/v0.1.0/TONDO_LANGUAGE_SPEC.md`; the materialization
script verifies the tag, commit and SHA-256 before creating or accepting the
snapshot. The snapshot is content evidence and must never be normalized by
checkout line-ending rules.

`conformance/live/manifest.json` is canonical pretty JSON and is the only entry
point for selecting either lineage from a current checkout. It pins:

- its format, edition, monotonically increasing revision and open state;
- the exact checkpoint tag, commit, manifest and specification snapshot;
- the four current language, standard-library, testing and toolchain specs;
- a content-addressed parent for every revision after revision 1;
- sorted case layers, each tied to non-empty implementation tasks and
  normative requirement IDs; and
- all tasks that still prevent sealing.

There is no default lineage. Every CLI, reliability tool, script and CI job
must explicitly select `checkpoint` or `live`. Loading the historical manifest
directly from a changed working tree is intentionally rejected because its
specification hash no longer matches. Checkpoint selection substitutes only
the pinned historical snapshot and hash-checks every other referenced byte;
live selection never presents the checkpoint cases as evidence for a new or
changed requirement.

A live case layer is admissible only in the same change as the implementation
slice it names. Its ID, tasks and requirement IDs are sorted and unique, and
its manifest is pinned below `conformance/live/layers/`. Until the incremental
case runner and evidence ratchet are activated by `CONF-RATCHET-001`, the
current revision contains no live layers and only the checkpoint lineage can be
executed. Merely listing a task or requirement can never make the coverage
matrix green.

Each accepted live revision changes the manifest hash. CI copies the exact
manifest to
`target/reliability/evidence/live-manifest-<sha256>.json` before running the
gate and retains it as an artifact, so a later revision cannot erase the
evidence for an earlier one.

## Observation boundary

Every adapter returns the same closed observation:

- compilation state;
- process exit code;
- complete structured diagnostics;
- exact stdout and stderr bytes encoded as hexadecimal;
- exact formatter bytes when applicable; and
- one typed JSON payload for semantic, memory, build or documentation data.

The runner compares every normative observable with one exact pattern or a
closed set of permitted patterns. Diagnostic codes, order, schema, IDs, spans,
related locations and fixes are always checked. Exact diagnostic text is pinned
only by cases whose purpose is that structured contract, because conforming
implementations may phrase other messages differently. Concurrency cases may
use a closed set of outcomes; they never assert wall time or one scheduler
order. An adapter cannot skip an applicable case: `unsupported` is a suite
failure unless the manifest omitted the case because the target declaration
lacks its capability.

Semantic observations use the closed
`tondo-semantic-observation-0.1/1` schema. The runner validates query/result
cardinality, result tags, exact top-level keys, stable `sem:` identities,
logical spans, repeated-field ordering, formatter bytes, and the nested
ownership schema before comparing the pinned observation. A diagnostic fix
marked `safe` is not accepted on shape alone: for check actions the runner
applies its edits by byte range to the exact request snapshot, invokes the
adapter again, and requires an error-free successful check.

## Separation

The generic runner receives source bytes over the adapter protocol and never
links compiler internals. The reference adapter may use public embedding APIs.
Collector cases use a separately compiled private adapter that exercises the
real collector while exposing only reachability and retry observations. Fixture
declarations from specification appendix C are available only to document-fence
requests and cannot be selected by an ordinary source action.

Coverage claims are data. `covers` requires the claimed normative code to occur
in the observation. `positive_for` requires a neighboring case where that code
does not occur. Checkpoint validation additionally requires complete registry,
warning-profile, panic, formatter, query, memory, concurrency and determinism
coverage before Gate G5 can close.

## Closed auxiliary observations

The checkpoint suite uses three additional closed schemas:

- `tondo-semantic-observation-0.1/1` for public semantic queries;
- `tondo-memory-observation-0.1/1` for the four private collector scenarios;
  and
- `tondo-determinism-observation-0.1/1` for closed-project builds under a
  perturbed source insertion order.

The memory schema records the scenario, allocations, collections, reclaimed
objects, peak live objects, and the exact root/cycle/retry property being
proved. It does not expose VM addresses or private collector layout.

The determinism adapter compiles the same declared project twice: once with the
canonical logical source order and once with its exact inverse. Only insertion
order changes. The schema records SHA-256 hashes of the interface, artifact,
and diagnostics for both executions, and the runner requires every pair to be
equal. The case repeats three times.

Concurrency cases run 32 repetitions each. This is a bounded stability
calibration, not a probabilistic replacement for the closed outcome contract:
each repetition must independently match an allowed observation and preserve
structured cleanup.

## Checkpoint validation

Loading the Tondo 0.1 checkpoint manifest verifies all hashes and additionally
requires:

- the exact target `tondo-vm-hosted`, profile `hosted`, and capabilities
  `[console, process]`;
- at least one case in every one of the ten groups;
- primary and positive-neighbor coverage for all 78 `E` diagnostics;
- all normative `P` classes and every warning in the `core` profile;
- exact formatter bytes and idempotence;
- exact semantic schemas, stable IDs, spans, ordering, and replayed safe fixes;
- all four private memory scenarios exactly once;
- one three-repetition closed-project determinism case;
- at least 32 repetitions for every concurrency case; and
- an `E1008` boundary proof for every target capability that a case omits.

The generic runner validates this contract without linking compiler or VM
internals. The checkpoint result uses
`tondo-conformance-result-0.1/1`; it records the manifest hash, exact adapter
description, target declaration, per-case repetition count, and canonical
observation hashes.

From the current checkout, checkpoint identity validation is portable. The
exact reference execution commands below require Linux because the historical
hosted process payloads are Linux-specific:

~~~text
bash scripts/materialize-checkpoint-spec.sh --check
cargo build -p tondo-conformance -p tondo-reference-adapter --bins --locked
cargo run -p tondo-conformance --locked -- validate \
  --root . \
  --manifest conformance/live/manifest.json \
  --lineage checkpoint
cargo run -p tondo-conformance --locked -- run \
  --root . \
  --manifest conformance/live/manifest.json \
  --lineage checkpoint \
  --adapter target/debug/tondo-reference-adapter \
  --output conformance/0.1/results/tondo-reference-0.1.0-tondo-vm-hosted.json
~~~

Running the final command twice against an unchanged tree must produce
byte-identical result files.

The open draft is validated independently:

~~~text
cargo run -p tondo-conformance --locked -- validate \
  --root . \
  --manifest conformance/live/manifest.json \
  --lineage live
cargo run -p tondo-conformance --locked -- seal \
  --root . \
  --manifest conformance/live/manifest.json \
  --lineage live
~~~

`seal` is a non-mutating preflight. It fails while pending tasks remain and
does not create, overwrite or promote any manifest. `CONF-SEAL-001` owns the
later atomic promotion after all live requirements and cases are complete.
