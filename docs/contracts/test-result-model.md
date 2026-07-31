# Test result model and worker protocol

**Status:** implemented as the pure boundary for `UTEST-RESULT-MODEL-001`

`tondo_compiler::test_result` owns two deterministic contracts without running
test bodies:

- `TestResultTree`, the single validated tree consumed by all reporters; and
- `ProtocolSession`, the direction-aware validator for the versioned
  coordinator/worker wire protocol.

## Result tree

The report format is `tondo-test-report-0.1/7`. Each suite/test node has a
stable ID, parent, source metadata, owners, an aggregate status and a non-empty
ordered list of attempts. Attempts preserve iteration, retry round and retry
unit, causal `blocked_by`, phase, failure/skip payloads, tags, logs,
artifacts, snapshots, virtual-time observations and output streams.

`TestResultTree::assemble` derives status, decisive attempt and every summary
counter once. `TestResultTree::parse` rejects unknown fields, duplicate IDs,
broken parents or blocked causes, invalid phase/status combinations, non-
contiguous attempts, invalid artifact/snapshot descriptors and summaries that
do not match the tree. Canonical bytes sort node IDs and per-attempt evidence
names while retaining observed log order.

The aggregate rules are the normative 9.1 rules: an all-passing history is
`passed`, a later pass after any non-pass is `flaky-pass`, and an unresolved
node points at the most recent executed failure (or its final blocked/skip
attempt). Blocked attempts are counted once and never duplicate their suite's
cause.

## Worker protocol

The wire format is `tondo-test-worker-0.1/1`. Coordinator frames use tagged
`hello`, `run`, `cancel` and `shutdown` commands; worker frames use tagged
`ready`, `started`, `attempt`, `finished`, `cancelled`, `closed` and `error`
events. Both carry the run ID and independent one-based contiguous sequences.
Hello closes target, plan hash and all positive resource limits. A session
requires `hello`/`ready` before `run`, returns to idle after `finished`, and
requires cancellation acknowledgement and a final `closed` event before the
worker can be discarded. Fatal worker errors close the session.

The protocol is value-free with respect to test inputs and has no host I/O.
Seven compiler tests cover aggregate/flaky derivation, summary and schema
rejection, blocked causality, canonical ordering, handshake sequencing,
cancellation/closure and invalid limits/units/unknown fields.
