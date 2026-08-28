# Gate N1: native backend promotion

`N1` is the promotion gate for Tondo 0.1's first native AOT backend. It is a
composition gate, not another lowering implementation: every prerequisite
campaign must already have produced a passing report for the exact Git
revision being promoted.

The machine-readable contract is
[`testing/native-n1.json`](../../testing/native-n1.json). The runner is
[`scripts/native-n1.sh`](../../scripts/native-n1.sh), and the generated
promotion record is written to
`target/reliability/evidence/native-n1.json`. The record is evidence for the
unpublished 0.1 development tree; it is not a release, a stable object layout,
or a public FFI ABI.

## Promotion scope

N1 promotes **Cranelift** for the primary product target
`x86_64-unknown-linux-gnu`. The VM remains the independent language oracle and
JIT remains outside the 0.1 product. LLVM stays available as the differential
comparison backend and is not a silent fallback.

The Linux ARM64 entry (`aarch64-unknown-linux-gnu`) is intentionally recorded
as a physical candidate smoke. The current smoke proves a real ELF link and
execution on ARM64 hardware, but it does not yet run the complete Tondo AOT
corpus there. It therefore cannot be presented as a promoted target. The same
rule applies to portable object probes on Windows and macOS: portability is
validated, but a target is not promoted without its own complete AOT corpus.

## Required evidence

The runner consumes the exact-revision reports for:

- the real-MIR evaluation and executable differential runner;
- AOT lowering, linked binary, memory/ARC, quality and performance campaigns;
- native conformance and differential testing;
- the reproducible native package;
- the physical primary-target smoke; and
- the ARM64 candidate smoke.

It also revalidates the corresponding contracts and requires:

- Cranelift selected by `DEC-013`, with no fallback or public ABI claim;
- the same verified MIR and VM oracle across both comparison backends;
- zero admitted `unsupported` cases, divergences, or sanitizer failures;
- coverage/mutation quality evidence with the normal baseline unchanged;
- a clean workspace and path-free reports; and
- a reproducible package with no undeclared environment or timestamp input.

Every dynamic report carries `source_revision`; N1 rejects a report from any
other commit. The generated record stores SHA-256 hashes for all reports and
contract inputs, so a reviewer can verify the composition without trusting a
hand-edited status field.

## Fail-closed boundary

N1 fails if a report is missing, stale, incomplete, path-bearing, divergent or
claims a target beyond its evidence. Mutating the contract, backend, target
classification, quality result, diagnostics result, package, source revision
or workspace state is covered by the negative suite in
`scripts/native-n1-test.sh`.

No N1 result promotes STD-0.1B, G5, S1, TLF, a release package or a public ABI.
After N1, the critical path continues with `STD-ASYNC-GROUP-IMPL-001` and the
remaining STD-0.1B implementation leaves. Native ARC elimination, COW,
escape analysis, incremental compilation and LSP remain post-N1 optimization
or tooling work.

## Verification commands

```text
scripts/native-n1-check.sh
scripts/native-n1-test.sh
scripts/native-n1.sh
```

`native-n1.sh` is normally run by the final promotion job after the native
evaluation and physical-target jobs have uploaded their reports. The ARM64
report is passed through `TONDO_NATIVE_N1_ARM64_EVIDENCE` when the artifact is
not at its default `target/platform-test/linux-aarch64/native-target.json`
path.
