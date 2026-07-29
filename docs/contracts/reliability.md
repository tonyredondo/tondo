# Reliability and continuous testing contract

**Status:** accepted for M10.5b / Gate H0

This contract defines how Tondo turns tests into reproducible evidence. It does
not change Tondo 0.1 semantics and does not implement the future Tondo 0.2
`test` or `suite` declarations.

## Versioned evidence

The repository owns five machine-readable records:

| Record | Contract |
| --- | --- |
| `testing/inventory.json` | Every discovered logical test, repetition, source, oracle, target, edition, status, and source hash. |
| `testing/normative-evidence.json` | Reviewed requirement claims with separate positive, rejection, boundary, composition, oracle, and public-boundary evidence. |
| `testing/coverage-matrix.json` | Every extracted normative Tondo 0.1 requirement, its stable identity, classification, dimensions, and evidence or waiver. |
| `testing/quality-baseline.json` | Reviewed line/function/region coverage and the bounded mutation selection with non-regression thresholds. |
| `testing/regressions.json` | Confirmed defects tied to the lowest executable public boundary that would have detected them. |

`tondo-reliability generate` deterministically rebuilds the inventory and
matrix. `tondo-reliability check` rebuilds them in memory, compares exact bytes,
validates the regression ledger, and validates the quality baseline. A stale,
missing, duplicated, orphaned, or incomplete record is a test failure.

## Inventory semantics

One logical test is not necessarily one source file or one execution:

- Rust `#[test]` functions are discovered from every workspace crate.
- `.to` fixtures are paired with their adjacent sidecars.
- Conformance cases retain their declared repetitions, target, oracle, group,
  requirements, and pinned source hashes.
- Every Tondo fence in the normative language specification is recorded.
- Fuzz targets are campaigns, not deterministic examples.
- Tondo 0.2 testing-spec fences are `future-contract`; they are never counted
  as executable Tondo 0.1 coverage.

The M10.5b inventory contains 1,507 logical tests and 1,726 repetitions. Of
those, 1,465 are executable, 38 are future contracts, three are fuzz campaigns,
and one is a non-executable pseudocode fence. Counts are derived from entries
and cannot be edited independently.

The inventory rejects:

- duplicate stable IDs;
- `#[test]` attributes without a following function;
- fixture sidecars without a source;
- repository sources absent from the conformance manifest;
- unknown sidecar extensions;
- incomplete metadata;
- unsorted or duplicated requirements and sidecars; and
- manifest or source-hash drift.

## Normative coverage matrix

Normative paragraphs containing `debe`, `deben`, `deberá`, or `no puede` are
extracted outside code fences. Each identity is derived from edition, stable
section, heading, and ordinal. The record also pins the document hash, exact
text hash, line range, anchor, phase, and risk.

Every requirement has exactly one status:

- `covered`: explicit executable conformance evidence reaches a stable public
  oracle;
- `target-not-applicable`: the rule deliberately describes a surface absent
  from `tondo-vm-hosted`;
- `stdlib-pending`: the rule belongs to the later standard-library contract;
- `toolchain-limit`: implementation tests exist, but no executable case
  currently carries this prose requirement as a stable identity.

`toolchain-limit` is an exposed traceability gap, not a claim that the behavior
is unimplemented. A section or nearby example never counts as semantic
coverage by proximity. The current matrix reports 17 covered, eight
target-inapplicable, four standard-library-pending, and 271 explicit
toolchain-limit requirements. Later milestones must replace those limits with
reviewed claims in `testing/normative-evidence.json`; they cannot silently turn
them green.

Each requirement records six dimensions: positive behavior,
rejection/failure, material boundaries, composition, oracle, and public
boundary. Every dimension contains sorted evidence or one non-empty versioned
waiver, never both. A reviewed claim must reference executable inventory IDs,
must provide an executable oracle, and must reach the published
`tondo-vm-hosted` boundary through a conformance case, fixture, or normative
specification fence.

## Deterministic gate and campaigns

`scripts/test-gate.sh` is the canonical PR and `main` gate on Linux x86_64. It
runs:

1. formatter fixed-point check;
2. `cargo check` for every target;
3. Clippy with warnings denied;
4. every workspace test and target;
5. Rustdoc with warnings denied;
6. locked conformance runner and adapter builds;
7. exact reliability-record validation;
8. conformance-manifest validation;
9. the complete reference conformance run; and
10. byte-for-byte comparison with the versioned result.

Linux ARM64, macOS Intel, macOS Apple Silicon, and Windows run the portable
workspace tests plus a native CLI hello-world smoke test.

The PR fuzz tier uses fixed seeds and a fixed run count. Nightly jobs extend the
same targets by time and run coverage plus mutation testing. Moving a costly
case to nightly does not remove its minimized deterministic regression from the
PR gate.

Failure evidence lives under `target/reliability/`. Logs replace the physical
workspace root with `./`; metadata includes only the target, seed, and tool
versions. CI uploads logs, minimized fuzz artifacts, and quality reports, never
credentials or ambient environment dumps.

## Generators, properties, and metamorphism

`tondo_reliability::generator::Generator` is a stable seeded generator. Its
integer-expression generator is typed by construction and has a deterministic
shrink order. Byte failures have a deterministic deletion minimizer. A failure
record contains the seed, target, operation, minimized input, and observation.

Persistent properties cover:

- typed integer expressions and their reductions;
- generics, traits, patterns, error propagation, ownership, borrowing, async,
  collections, and control-flow templates;
- lossless CST partition and reconstruction;
- formatter idempotence;
- alpha-renaming and neutral parentheses;
- physical source-order permutation;
- eager versus COW value observations;
- normal versus allocation-by-allocation GC pressure; and
- canonical diagnostic stability.

The seed is part of the failure, never an implicit global random state.

## Models

Model tests compare implementation observations to independent, simple state:

- insertion-ordered `Map` operation sequences;
- generated `Array` writes and arithmetic;
- `Set` uniqueness, insertion order, and membership;
- exclusive integer `Range` order;
- Unicode-scalar `String` indexing and slicing;
- slice snapshots and logical-copy independence;
- 50,000 mathematical array index/slice normalizations;
- loan and structured-concurrency states, including active `Join` loans,
  terminal await/cancel, atomic invalid transitions, and LIFO cleanup; and
- the real collector's reachable roots, unreachable cycles, sustained
  pressure, and retry-before-OOM scenarios.

The model is source-level. Heap handles, allocation counts, COW storage IDs, or
scheduler-private counters cannot become language semantics.

## Fuzzing

The isolated `fuzz/` workspace pins its dependency lockfile and contains:

- `frontend`: arbitrary bytes through lexer, parser, CST partition,
  reconstruction, formatting, reparsing, and fixed-point checks in module,
  script, and fragment modes;
- `protocols`: project plan, privileged unit, interface, artifact, conformance
  manifest, adapter request/response decoders, and diagnostics JSON emitted
  from bounded arbitrary source bytes;
- `admission`: typed generated programs through HIR, MIR, bytecode, and runtime
  admission, plus bounded structural bytecode type/catalog mutations.

Inputs are bounded before parsing and verifiers retain explicit work limits.
The repository corpus includes valid, recoverable, unknown-field, and
structural seeds. PR seeds are 1001–1003 with 128 executions per target;
nightly seeds are 2001–2003 with a 180-second campaign per target. Generated
inputs are written under `target/reliability/fuzz-corpus/`; the six reviewed
seed files remain read-only. The nightly toolchain is pinned to
`nightly-2026-07-28` and `cargo-fuzz 0.13.2`.

A crash is not closed merely because the fuzzer no longer finds it. Its
minimized input must enter the corpus or regression ledger and the ordinary
deterministic gate.

## Coverage and mutation

`cargo-llvm-cov 0.8.7` measures every workspace target. The baseline stores
lines, functions, and regions globally and separately for parser, checkers,
HIR/MIR/bytecode verifiers, heap, execution, and untrusted protocols. No source
is excluded to improve the percentage.

M10.5b executes the complete 205-case published conformance suite in-process,
so compiler, adapter, protocol, semantic-query, and VM work contributes to the
same instrumented report instead of disappearing behind a subprocess. It also
adds closed negative and boundary contracts for the CLI, canonical artifacts,
manifest and adapter protocols, semantic snapshots, bytecode verification and
disassembly, managed runtime values, and the reliability tooling itself.

The reviewed M10.5b observation is 90.08% of lines (119,622/132,793), 86.42%
of functions (7,866/9,102), and 88.15% of regions (169,052/191,782). The
machine gate deliberately truncates those observations to exact non-regression
floors of 9,008, 8,642, and 8,814 basis points. Its line floors by risk are:

- parser: 9,451 basis points;
- checkers: 9,064;
- HIR/MIR/bytecode verifiers: 8,868;
- heap and managed values: 9,770;
- lowering and execution: 8,912; and
- untrusted artifacts, projects, conformance, adapters, and reliability
  protocols: 9,106.

Function and region floors for every risk scope remain machine-readable in
`testing/quality-baseline.json`. Every floor is the reviewed observed value; a
decrease fails.

The Rust 1.93.0 / LLVM 21.1.8 report exposes `branches` and `mcdc` fields but
contains zero instrumented units for both. They are therefore recorded as
unsupported by this measurement, not as 0% coverage and not as green metrics.
Until the pinned Rust coverage pipeline produces a stable non-zero
instrumentation domain, branch-heavy behavior is defended by region coverage,
closed decision matrices, model/property tests, and mutation testing. Enabling
either numeric gate requires a reviewed toolchain report with actual units.

`cargo-mutants 27.1.0` runs a bounded, explicit 28-mutant selection over:

- project and privileged-unit admission;
- documentation line-ending admission;
- array index normalization; and
- heap capacity enforcement.

The campaign copies VCS metadata because the process fixture intentionally
executes `git log`. Of 28 generated mutations, 27 are executable and all 27
must be caught; one `ProjectPlan::parse` replacement is unviable because the
return type has no valid default. A changed selection, an additional unviable
mutant, any survivor, any timeout, or a lower score requires review and a new
baseline.

## Regression rule

Every confirmed bug receives:

- a stable regression identity;
- its discovery mechanism;
- the smallest reproducer;
- the lowest public boundary that should reject or expose it;
- an executable inventory test;
- its source path and fixed milestone; and
- sorted persistent evidence.

The ledger validator rejects unknown, future-only, moved, unsorted, duplicate,
or missing tests. Internal tests may localize the cause, but they cannot replace
the public-boundary regression.

## Reproduction and updates

The ordinary local commands are:

~~~text
bash scripts/test-gate.sh
TONDO_FUZZ_RUNS=128 bash scripts/fuzz-smoke.sh
bash scripts/quality-gate.sh
~~~

Updating a quality threshold is a reviewed baseline change, not an automatic
snapshot update. Generate a fresh report, inspect uncovered risk scopes and all
mutation outcomes, close real gaps, then run:

~~~text
cargo run -p tondo-reliability --locked -- quality capture \
  --root . \
  --coverage target/reliability/quality/coverage.json \
  --mutants target/reliability/quality/mutation/mutants.out/outcomes.json \
  --revision M10.5b-H0-COV90
~~~

The strict gate must remain green after any regenerated record.
