# Fast gate contract

**Status:** accepted for Tondo 0.1 development; it does not replace a full wave
gate.

`scripts/fast-gate.sh` is the short feedback loop for a block-sized change. It
derives an impact set from the diff and selects the smallest sufficient tier:

- `documentation`: specs, tracker, documentation and their generated evidence;
- `impacted`: implementation isolated to one or more workspace packages;
- `shared-frontier`: compiler/runtime boundaries whose effects cross packages.

The temporary evidence is written below `target/reliability/fast-gate/` (or the
caller-provided `CARGO_TARGET_DIR`) and is deliberately ephemeral.

The `documentation` tier executes `scripts/documentation-gate.sh`. It validates
typed fences, documentation conformance, normative evidence, tracker topology,
the live draft manifest and standard-library contracts. It does not run the
workspace test suite, coverage or mutation testing. A generated conformance or
evidence file remains documentary only when every other changed input belongs
to the same documentary set; a source, fixture or implementation change leaves
this tier immediately.

The `impacted` tier runs formatter, check and tests only for affected packages.
Coverage of changed executable lines and diff mutation are required only when
the diff changes production Rust. Integration tests and edits confined to the
final `#[cfg(test)] mod tests` module of an audited file run the affected package
tests without recalculating product coverage or mutants. When every Rust edit is
confined to such an inline module, the gate runs that package's library tests
instead of unrelated integration/property targets. External integration-test
changes retain their package test targets. Documentation, JSON evidence and
Markdown never trigger coverage or mutation by themselves. The conservative
list of audited inline modules lives in `testing/fast-gate.json`; an edit before
the marker is classified as production automatically.

The machine-readable policy lives in `testing/fast-gate.json`. A change to a
workspace manifest, compiler/runtime frontier, executable conformance source or
a full-gate script escalates automatically to `scripts/test-gate.sh`. Normative
sources and their generated documentary records use the documentation tier.
Unknown files remain in the impacted set and never silently bypass the
formatter or package checks. `--dry-run` is deterministic and is used by
`scripts/fast-gate-test.sh` to keep all three classifications executable.
Pull requests compare against their base SHA; pushes compare the complete
`before..head` event range rather than `HEAD^`, so publishing several local
commits together cannot omit earlier changes from the impact set.

The changed-line coverage rule is intentionally stricter than the global
ratchet: executable lines added by a block must be covered by the package report
at 100%; non-instrumented lines (comments, declarations, and formatting-only
lines) are recorded as not applicable. The full quality gate remains the owner
of global line/function/region floors and mutation selection. A surviving diff
mutant fails the fast gate; no timeout or missing tool is converted into a
pass.

Fast evidence is draft evidence. It can accelerate local iteration and the
ordinary CI lane, but a wave boundary, release candidate, baseline change or
explicit cross-platform claim is not complete until the full test gate, quality
gate and conformance ratchet have been regenerated from the final tree. Passing
time alone, a commit, a push or a documentation-only edit never justifies that
cost.
