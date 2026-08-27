# Native STD-0.1A coordination contract

`NATIVE-STD-001` is the coordination gate for the two native standard-library
owners already implemented: `std.core` and the hosted owner group. It does not
add a second API. It proves that both candidates consume the same normalized
`tondo-mir-backend/1` MIR and the same private Result carrier, and that Hosted keeps the static
capability boundary enforced by the VM.

## Shared boundary

Core (`Option`/`Result`) and Hosted (capability handles and byte buffers) use
`tondo_rt_result_new`, `tondo_rt_result_tag` and `tondo_rt_result_payload` for
managed observations. Their errors are tags/payloads in that carrier; no
Cranelift- or LLVM-specific public type is emitted. Core payloads, host buffers
and host handles are retained/released by the same ARC and cleanup edges.

Hosted capabilities remain admission facts, not runtime feature probes. The
coordinator therefore runs the Core and Hosted contracts independently and
then compares their shared dimensions. A missing capability, stale handle,
invalid result tag or cleanup mismatch fails the whole coordination run.

## Evidence and non-goals

[`scripts/native-std-test.sh`](../../scripts/native-std-test.sh) executes the
two owner suites, checks that both reports are present and writes
`target/reliability/evidence/native-std.json`. The report records each owner,
the shared carrier/MIR identities and the per-backend route (`common-mir`), but
does not claim complete application conformance, physical target coverage or a
selected backend. Those are the following Link/CLI, conformance, differential
and target blocks.
