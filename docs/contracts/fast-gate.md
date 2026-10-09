# Fast gate contract

**Status:** accepted for Tondo 0.1 development; it does not replace a full wave
gate.

`scripts/fast-gate.sh` is the short feedback loop for a block-sized change. It
derives an impact set from the diff and selects the smallest sufficient tier:

- `documentation`: specs, tracker, documentation and their generated evidence;
- `impacted`: implementation isolated to one or more workspace packages;
- `shared-frontier`: compiler/runtime boundaries whose effects cross packages.

The temporary evidence is written below `target/reliability/fast-gate/` (or the
caller-provided `CARGO_TARGET_DIR`) and is deliberately ephemeral. Every gate
exports one explicit Cargo target directory, including nested scripts that
previously defaulted to `target-fast`. CI caches downloaded sources and a
bounded selection of compiled third-party dependencies through
`scripts/ci_build_cache.py`. The artifact payload is at most 4 GiB; workspace
libraries/test executables, incremental directories, coverage profiles,
mutation builds and execution evidence are excluded. An unrestricted target
tree is never cached. Cached dependency units still pass Cargo's normal
freshness checks; a cache is not test proof.
The native worker publishes the full-plan dependency cache, so other
workers' failures do not discard a successfully validated dependency build.

The selector runs before Linux execution. A fast plan gets one worker. A full
plan gets four ordinary public Linux workers, followed by a dependent strict
closure. Each worker retains the existing finite 55-minute limit; the closure
has 30 minutes. Portable jobs retain 45 minutes and deterministic fuzz retains
30 minutes. All required commands, samples and quality floors remain mandatory;
a failed, missing, cancelled or timed-out worker cannot produce a green strict
check. This is an orchestration change, not a performance SLO or target
promotion.

Observed Linux execution on standard GitHub runners, measured from workflow
creation until the required strict check completed:

| Execution | Source | Canonical steps | Workspace tests | Wall time |
| --- | --- | ---: | ---: | ---: |
| [Serial gate](https://github.com/tonyredondo/tondo/actions/runs/37955205979) | `71020bc21c3b1d89a4dde69bc92ee57dbe811cb8` | 367 | 2,921 | 54m 42s |
| [Partitioned gate](https://github.com/tonyredondo/tondo/actions/runs/37981051112) | `625a2327b7011715ce00c444921f276dac031982` | 373 | 2,939 | 35m 56s |
| [Suite sharding and current-run reuse](https://github.com/tonyredondo/tondo/actions/runs/37995359543) | `e99988f38062a30354458a856edb60c3577f2cbe` | 374 | 2,939 | 37m 24s |

The first partitioned run reduced the observed required-check wait by 34.3%.
These are actual runs with different source revisions and cache states, rather
than a controlled benchmark
or a latency guarantee. The second execution validated all five receipts and
all 373 command logs; every required worker and the strict closure passed.
Its longest worker took 22m 18s and the dependent closure took 13m 23s. The
native worker restored 2,424 validated dependency files and retained a
941,199,684-byte payload, within the 4 GiB bound. The normal-push portable and
deterministic fuzz jobs were skipped by policy; this comparison does not claim
portable execution. Current-source quality was measured once for the source
block: 91.67% global line coverage, every locked 80% risk floor satisfied, and
all six selected critical mutants caught. Documentary closure reuses those
unchanged source bindings.

The suite-sharding run validated all five receipts, 374 canonical command logs,
80 suites and all 2,939 workspace tests. Foundation's complete test step,
including compilation and discovery, fell from 17m 35s to 13m 06s (25.4%).
The strict closure fell from 13m 23s to 9m 42s (27.5%); its stdlib conformance
step fell from 480.06 to 316.53 seconds. Its report verified 26 reused commands,
including all 22 owner commands and the explicit 206-case draft corpus run.
However, the stdlib worker increased from 16m 41s to 27m 29s, so the required
total wait increased by 1m 28s. That worker used an AMD EPYC 7763 rather than
the earlier AMD EPYC 9V45. These observations demonstrate the executed reuse
and lower test/closure times, but do not establish a total CI improvement or
attribute the stdlib increase to a single cause. Normal-push portable and fuzz
jobs remained policy skips. The unchanged quality policy passed once for this
source: 91.67% line coverage, all locked 80% risk floors and six of six critical
mutants caught; this documentary closure reuses those bindings.

`scripts/test_gate_partitions.py` derives disjoint worker plans from the
canonical `scripts/test-gate.sh` command list. Foundation owns workspace
validation and layer evidence; native owns native checks; runtime owns async,
channel, synchronization and executor owners; stdlib owns the other library
owners. The final part begins at the public API audit and includes global
conformance, distribution, performance aggregation and the S1A gate. It waits
for all four workers and their current layer/owner evidence. The unpartitioned
local gate remains available.

Every partition records only successfully completed named steps and hashes
their actual logs. Receipts bind Git revision/tree, Rust/Cargo versions and
test/build inputs. The strict closure validates all four receipts, refuses
duplicate/missing steps, drifted sources and conflicting files, and merges
evidence only after validation. Its final receipt must cover all remaining
commands. The union must equal the complete canonical gate exactly once.
Before workspace validation, a read-only standard package pin check catches
stale conformance fixture identities. Regeneration remains explicit, with
inspection of the generated changes; CI never approves its own expectations.
Fresh checkout, current worker success and unchanged source remain required;
cached reports or an old successful CI cannot satisfy these checks. Successful
fast evidence also binds the exact checkout and executed command plan.

Foundation compiles the complete workspace with `cargo test --workspace
--all-targets --locked --no-run --message-format=json`, then executes its libtest
binaries in four shards within that same runner. Cargo metadata and successful
build messages must identify exactly the same target set, including example
harnesses selected by `--all-targets`. Each child retains its package working
directory and Cargo binary paths. Each suite has at most four test threads;
existing delegated process isolation remains mandatory. Duration hints in
`testing/rust-suite-durations.json` choose a deterministic longest-first
assignment, with a one-second hint for new targets. Hints are scheduling data,
never successful execution evidence. Refresh them from the per-suite `seconds`
in a verified execution summary and retain its source/run reference.

Discovery records every test name before execution. The runner requires every
suite and every discovered test exactly once, a successful exit and terminal
summary, and no ignored, filtered or failed tests. It hashes the binaries and
retains individual raw logs, the complete plan and execution summary. Actual
logs are replayed in stable order for layer attestation. Missing results,
modified logs or changed source invalidate the aggregate.

A controlled local probe on unchanged `c17a4532`, with the same 80 binaries,
2,939 tests, four CPUs and 15 GiB, took 458.37 seconds serially and 141.19 seconds
with four shards: 69.2% less execution time. Compilation was excluded from this
comparison. This is feasibility evidence. In the verified hosted run above,
the integrated runner's shard execution took 505.62 seconds, separately from
compilation, discovery and binary hashing; its complete test step took 786.34
seconds. The local probe is not a hosted latency guarantee.

Partition receipt format `tondo-test-gate-partition/2` additionally binds the
workflow run ID/attempt and foundation's corpus, layer and Rust-suite outputs.
Foundation owns the single explicit gate command for the 206-case draft corpus;
corpus execution inside required Rust integration tests remains separate.
Final's explicit `stdlib-conformance.sh --reuse-current-gate` mode validates all
four complete
worker receipts and their actual logs before reusing the 22 identical owner
commands and that corpus execution. Its report records the actual producer
command and log hash. Commands with different inputs and cases without a
completed exact producer still execute normally. A required producer that is
missing, failed, from another attempt or altered causes failure; it never falls
back to an old report. The ordinary standalone conformance path still executes
its commands. The full canonical plan now has 374 steps, each owned exactly once.

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

Tracker, inventory, coverage matrix and their generated documentary records
continue to receive documentation/coherence checks when mixed with package
tests. Regenerating those records does not independently require every owner
campaign. A real shared frontend/runtime input, a quality baseline/ratchet or
an unmapped executable input still selects the complete gate. Package execution
order is sorted, so repeated plans are deterministic.

The machine-readable policy lives in `testing/fast-gate.json`. A change to a
workspace manifest, compiler/runtime frontier (including the monolithic
`crates/tondo-compiler/src/mir.rs` module), executable conformance source or a
full-gate script escalates automatically to `scripts/test-gate.sh`. Normative
sources and their generated documentary records use the documentation tier.
Files without an explicit documentary, policy, evaluation or package owner
run the full test gate. This includes otherwise unmapped JSON contracts,
checker scripts and executable fixtures. An explicit shared path still
requires the full gate when it also appears in the gate-policy list; policy
self-tests cannot replace its integration checks. The smaller policy route
is reserved for the fast selector and its own executable contract tests.
Deletions remain in the impact set; renames include the removed and added
paths. Generated diffs use explicit `a/` and `b/` prefixes regardless of the
caller's Git display configuration. Explicit binary, quoted-path or
metadata-only patch sections conservatively require the full gate when their
paths cannot be classified. A nonempty patch never becomes a formatter-only
success because its new file path is `/dev/null`.

`--dry-run` is deterministic and is used by
`scripts/fast-gate-test.sh` to keep all three classifications executable.
Pull requests compare against their base SHA; pushes compare the complete
`before..head` event range rather than `HEAD^`, so publishing several local
commits together cannot omit earlier changes from the impact set.

CI derives tool prerequisites from that same command plan. A full-gate plan
installs the pinned fuzz toolchain even when no Rust source changed. Coverage
and mutation tools are installed when the selected plan calls them; filename
extensions do not decide whether those commands have their dependencies.

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
