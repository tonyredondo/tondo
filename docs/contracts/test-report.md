# Canonical test JSON reports and lists

**Status:** implemented for `UTEST-REPORT-001`

`tondo_compiler::test_report` is the single presentation boundary for the
testing runner. It consumes the validated `TestResultTree` from
`test_result`, never reruns a body, and emits both interactive JSON and file
reports through the same canonical serializer.

## Formats

- `tondo-test-json-v1` is the JSON serialization contract.
- `tondo-test-report-0.1/8` is the complete execution report.
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

## Exact output streams

Every attempt's stdout and stderr is a closed `{encoding, data}` object, in
that field order. `encoding` is `utf8` when the complete stream is valid UTF-8,
including empty output, and `base64` otherwise. Base64 is standard, padded,
without whitespace and with zero unused bits. A Base64 representation of valid
UTF-8 is rejected, so each byte sequence has one representation. Old plain
strings, unknown fields/encodings and malformed data are rejected.

`test_output::CapturedOutput` retains original bytes through writes, suite
phase concatenation, worker transport and report assembly. A scalar split
between writes or phases is classified only after concatenation. JSON and
Base64 expansion do not consume the program's raw-byte output quota. Rejected
writes publish no prefix; prior successful writes remain intact. Binary human
output uses an explicit `[base64]` label and encoded data.

## Human output

The CLI human reporter consumes this same validated report and descriptor list.
It prints suite and test identities and statuses; failures, skips, flaky results
and unstable repeats expose every attempt with separate logs, stdout and stderr.
Blocking rows identify their causal suite attempt rather than duplicating its
metadata. Owners, tags, artifact descriptors and non-matched snapshot descriptors
accompany these attempts. Passing payloads and matched snapshots require
`--show-output`; nonempty virtual-time observations always name the domains and
final virtual nanoseconds. Binary streams retain the explicit Base64 label.
Metadata and individual log records use JSON escaping so embedded newlines do not
invent report rows; artifact bodies are never loaded for human presentation.
The human list includes nonempty static owners, and both human views print the
effective random-order seed for replay. Output failures propagate as CLI errors.

## Lists

`TestList` shares the common metadata and carries the snapshot-store identity,
the exact execution plan, and descriptor-only suite/test arrays. It intentionally
omits status, attempts, lifecycle payloads, runtime tags/logs, artifacts,
snapshots, blocking causes and streams. Empty selections and valid empty shards
remain representable without inventing an execution result.
The CLI builds the selected ancestor suites and leaf parent links before
presentation and uses the same canonical/random scheduler as execution. Listing
therefore exposes the same leaf plan for the same selection, shard and seed,
without opening execution providers or running suite setup.

`TestReport::canonical_bytes` and `TestList::canonical_bytes` are the only
serialization paths. `parse` validates the trailing-LF/canonical-byte
contract, the closed metadata vocabulary, node identity/source class, tree
references and the result-model invariants before returning a typed value.
