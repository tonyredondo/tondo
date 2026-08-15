# Fast gate contract

**Status:** accepted for Tondo 0.1 development; it does not replace a wave
gate or a promotion proof.

`scripts/fast-gate.sh` is the short feedback loop for a block-sized change. It
derives an impact set from the diff, runs the formatter, checks and tests only
the affected workspace packages, then proves that every newly executable Rust
line is covered and that changed Rust code has no surviving diff mutant. The
temporary evidence is written below `target/reliability/fast-gate/` (or the
caller-provided `CARGO_TARGET_DIR`) and is deliberately not part of
`conformance/proofs/`.

The machine-readable policy lives in `testing/fast-gate.json`. A change to a
workspace manifest, compiler/runtime frontier, normative source, conformance
records, or a full-gate script escalates automatically to `scripts/test-gate.sh`.
Unknown files remain in the impacted set and never silently bypass the
formatter or package checks. `--dry-run` is deterministic and is used by
`scripts/fast-gate-test.sh` to keep the impact classifier executable.

The changed-line coverage rule is intentionally stricter than the global
ratchet: executable lines added by a block must be covered by the package report
at 100%; non-instrumented lines (comments, declarations, and formatting-only
lines) are recorded as not applicable. The full quality gate remains the owner
of global line/function/region floors and mutation selection. A surviving diff
mutant fails the fast gate; no timeout or missing tool is converted into a
pass.

Fast evidence is draft evidence. It can accelerate local iteration and the
ordinary CI lane, but a wave is not complete until the full test gate, quality
gate, conformance ratchet, and (with `TONDO_FULL_GATE_PROMOTION=1`) the
content-addressed promotion proof have been regenerated from the final tree.
