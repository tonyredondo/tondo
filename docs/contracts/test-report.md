# Canonical test JSON reports and lists

**Status:** implemented for `UTEST-REPORT-001`

`tondo_compiler::test_report` is the single presentation boundary for the
testing runner. It consumes the validated `TestResultTree` from
`test_result`, never reruns a body, and emits both interactive JSON and file
reports through the same canonical serializer.

## Formats

- `tondo-test-json-v1` is the JSON serialization contract.
- `tondo-test-report-0.1/7` is the complete execution report.
- `tondo-test-list-0.1/6` is the descriptor-only result of `--list`.

The serializer emits compact UTF-8 JSON with no BOM, no whitespace outside
strings, and one final LF. It rejects missing, duplicated, or additional line
breaks and rejects non-canonical key/array order when parsing. Maps use their
bytewise `BTreeMap` order; node arrays are sorted by ID and attempt evidence is
sorted by its normative name/index keys. Decimal quantities that must not lose
precision in JSON, including artifact sizes and aggregate artifact bytes, are
encoded as canonical decimal strings.

## Report envelope

`TestReport` keeps the result tree and explicit invocation metadata together:

- target/profile/capabilities and compilation state;
- selection (`all`, `filter`, `glob`, or `exact`);
- CODEOWNERS mode, logical source and content hash;
- public input hash, secret-profile hash/count and reproducibility class;
- shard and order/seed algorithms;
- retry rounds and fresh-worker identity, repeat count and isolation;
- artifact and snapshot store identities/policy;
- effective skip/flaky policy and resource-profile limits;
- separate `suites` and `tests` arrays, execution plan and derived summary.

The public node `kind` is the source class (`unit` or `integration`); array
membership carries the structural suite/test kind. Node IDs must contain the
same source class, so a report cannot silently relabel an integration node.
Each attempt remains isolated and retains phase, causal `blocked_by`, failure,
skip, tags, logs, streams, artifacts, snapshots and virtual-time observations.
The existing result-model validator re-derives aggregate status, decisive
attempt and all summary counters before a report can be constructed or parsed.

No secret values, physical paths, PIDs, timestamps, wall-clock durations,
attachment bytes or complete snapshot values enter the canonical report.

## Lists

`TestList` shares the common metadata and carries the snapshot-store identity,
the exact execution plan, and descriptor-only suite/test arrays. It intentionally
omits status, attempts, lifecycle payloads, runtime tags/logs, artifacts,
snapshots, blocking causes and streams. Empty selections and valid empty shards
remain representable without inventing an execution result.

`TestReport::canonical_bytes` and `TestList::canonical_bytes` are the only
serialization paths. `parse` validates the trailing-LF/canonical-byte
contract, the closed metadata vocabulary, node identity/source class, tree
references and the result-model invariants before returning a typed value.

