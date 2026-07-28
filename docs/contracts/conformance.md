# Tondo 0.1 conformance suite

**Status:** released with Tondo 0.1.0

`tondo-conformance-0.1` is an immutable corpus and a runner protocol, not a
test-only view of the reference compiler. The suite distribution contains:

- one canonical `tondo-conformance-manifest-0.1/1`;
- every source, expectation, specification and fixture input pinned by SHA-256;
- a generic runner that has no dependency on the Tondo compiler or VM; and
- an implementation-specific adapter speaking
  `tondo-conformance-adapter-0.1/1`.

The manifest is compact canonical JSON. Identifying arrays and case IDs are
sorted and unique. Paths are logical, relative and portable. Loading the suite
checks every referenced byte string before an adapter is started. A missing
expectation never means success.

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
does not occur. Release validation additionally requires complete registry,
warning-profile, panic, formatter, query, memory, concurrency and determinism
coverage before Gate G5 can close.

## Closed auxiliary observations

The release suite uses three additional closed schemas:

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

## Release validation

Loading the Tondo 0.1 release manifest verifies all hashes and additionally
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
internals. The release result uses
`tondo-conformance-result-0.1/1`; it records the manifest hash, exact adapter
description, target declaration, per-case repetition count, and canonical
observation hashes.

From a repository root, the portable verification commands are:

~~~text
cargo build -p tondo-conformance -p tondo-reference-adapter --bins --locked
cargo run -p tondo-conformance --locked -- validate \
  --root . \
  --manifest conformance/0.1/manifest.json
cargo run -p tondo-conformance --locked -- run \
  --root . \
  --manifest conformance/0.1/manifest.json \
  --adapter target/debug/tondo-reference-adapter \
  --output conformance/0.1/results/tondo-reference-0.1.0-tondo-vm-hosted.json
~~~

Running the final command twice against an unchanged tree must produce
byte-identical result files.
