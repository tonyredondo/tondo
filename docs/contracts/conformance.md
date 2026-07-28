# Tondo 0.1 conformance suite

**Status:** M10 portable contract

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
