# Tondo continuation handoff

## Task

Take full ownership of the unfinished Tondo roadmap from this repository. Start
by reconstructing and verifying the live checkout, instructions, tracker, and
evidence on the new machine. Then complete the next unlocked block,
`STD-TOML-PERF-001`, and continue one coherent tracker block at a time until a
real product/design decision or a reproducible blocker requires Tony.

Do not treat this handoff as permission to publish Tondo. Local implementation,
validation, direct commits to `main`, normal pushes, and same-session CI
follow-up are authorized for coherent roadmap blocks. External release or
package publication requires explicit human authorization.

Communicate with Tony in Spanish. Keep repository code, contracts,
documentation, tests, and commit messages in English.

## What Tondo is

Tondo is an unpublished, general-purpose, statically typed, compiled language
with automatic memory management. Its guiding phrase is "Small by design,
complete in practice." It is mainly inspired by Go, Odin, Rust, Kotlin, and
Lua, with selected ideas from Python, NumPy, and span-oriented APIs.

The only current language edition is the live Tondo 0.1 draft. There is no
released version and no legitimate v1/v2 compatibility split. Historical
syntax is not retained merely for compatibility because nothing has shipped.

Core product choices already made include:

- one canonical form when possible and minimum boilerplate;
- return types written with `:`;
- recoverable errors as values with `T ! E` and optional values with `T?`;
- explicit `pub`, private-by-default declarations;
- PascalCase for types and traits, with Go/.NET-style conventions for untyped
  identifiers;
- one canonical `for` form;
- unified generic collections: `Array[T]`, `Map[K, V]`, and `Set[T]`;
- local type inference with complete public signatures;
- immutability and value semantics by default, explicit mutation, and defined
  evaluation order;
- structured diagnostics and canonical formatting;
- no hidden dependence on ambient environment state.

Async is transparent by default. Ordinary functions use `fn`; suspension is
inferred, and public/bodyless contracts expose the postfix capability
`suspends` when necessary. Ordinary suspendible calls implicitly wait. `await`
consumes a pending `Join[T, E]`; only `spawn` deliberately preserves pending
work. `selectable` is a specific semantic capability that weakens to
`suspends`, not a synonym for it. Suspension is also inferred through `defer`.
Structured scopes, groups, channels, `select`, producer/consumer patterns, and
threads cover the advanced cases. Always read the normative async contracts
before changing this model.

The primary product is native AOT. Cranelift is the selected native backend,
and the promoted N1 target is `x86_64-unknown-linux-gnu`. The hosted bytecode VM
is the executable reference/oracle/bootstrap route and currently uses tracing
GC. The adopted native memory design is hybrid ARC plus a cycle collector.
LLVM remains an experimental comparison. Linux ARM64 is a physical candidate
smoke target; macOS and Windows jobs are portability probes, not published
targets. Never promote a model, host bridge, ABI scaffold, or contract into a
claim that the native runtime or AOT lowering is complete.

The standard library follows the same narrow promotion discipline. Its owner
mini-gate is `SPEC -> IMPL/HOST -> MODEL/TEST/FUZZ -> PERF -> CONF -> DOC`.
Projects and lockfiles use TOML; do not preserve or recreate a JSON manifest
variant. Data formats include JSON, MessagePack, Protobuf, TOML, YAML, and CBOR.
The design also includes reflection/static metaprogramming, I/O and processes,
concurrent collections, and race/leak/crash diagnostics. Always distinguish
specified behavior, a bounded reference model, hosted VM execution, native ABI
coverage, native AOT execution, and conformant promotion.

The Tondo test framework is designed to support suites and hooks, subtests,
logs/fail/skip, tags, JUnit XML, codeowners, sharding, randomization, glob and
regex filters, isolated retries, snapshots, and deterministic `synctest`.
Tracker gates M10.6/T0 record the implemented scope; do not infer product
release readiness from that alone.

## Repository identity and transfer

- Remote: `https://github.com/tonyredondo/tondo.git`
- Working branch: `main`
- Direct target: `origin/main`
- Implementation base before adding this handoff document:
  `3d087e21df5e73eb8d7781682628c17bb34fc37f`
- Base commit title: `Install cargo-fuzz for the test gate`
- Old-machine checkout: `/mnt/media/Tony/Projects/tondo`
- Old-machine artifact directory: `/mnt/media/Tony/Projects/tondo/target-fast`
- Rust workspace edition: 2024
- Minimum Rust version: 1.93
- Workspace package version: 0.1.0 with `publish = false`
- Rust source policy: unsafe code is forbidden at workspace level.

The handoff document itself is published in a later commit, so its own hash
cannot be embedded in its bytes. On the new machine, trust the current
fast-forward `origin/main`, verify its tip, and read `git log -1` before doing
anything. Tony's final message from the old machine should also report that
exact pushed tip and CI run.

Do not assume the old absolute checkout or artifact path exists on the new
machine. Select the real checkout explicitly, use a build/artifact directory on
the larger disk, and pass it as `CARGO_TARGET_DIR` where supported. Do not use a
clone, temporary directory, or harness checkout accidentally when an existing
task checkout has been selected.

## Instructions that govern the continuation

Before any edit on the new machine:

1. Read the user-supplied `AGENTS.md` agreement in the new chat.
2. Search only the repository root and known parent directories for a physical
   `AGENTS.md`; read every applicable one if present. No physical `AGENTS.md`
   existed in the old repository or its known parents when this file was
   created.
3. Read the local implementation procedure before changing code or docs.
4. Read the local Git procedure before staging, committing, or publishing.
5. Read the PR/CI follow-up procedure before a push or CI diagnosis.
6. Re-establish repository root, shared Git directory, branch, HEAD,
   `origin/main`, remotes, and working-tree status. Inspect every dirty-file diff
   before editing. Existing changes belong to Tony.

Do not create another branch or worktree unless Tony later authorizes it. Do
not delegate to subagents unless Tony explicitly asks. Use `apply_patch` for
source and documentation edits. Never force-push, amend, rebase, hard-reset,
broad-clean, rewrite history, weaken tests, or absorb unrelated changes.

## Workspace architecture

The Rust workspace contains these main crates:

- `crates/tondo-cli`: CLI and user-facing command orchestration.
- `crates/tondo-compiler`: lexer, parser, formatter, name/type checking, HIR,
  MIR, bytecode/native lowering, hosted bridges, and compiler diagnostics.
- `crates/tondo-conformance`: conformance runner and corpus integration.
- `crates/tondo-reliability`: independent bounded models and reliability tests.
- `crates/tondo-reference-adapter`: reference implementation adapter.
- `crates/tondo-native-runtime`: native runtime and native ABI support.
- `crates/tondo-stdlib`: standard-library kernels.
- `crates/tondo-vm`: bytecode VM and hosted runtime.

Primary specifications and project state:

- `README.md`
- `TONDO_LANGUAGE_SPEC.md`
- `TONDO_STANDARD_LIBRARY_SPEC.md`
- `TONDO_TOOLCHAIN_SPEC.md`
- `TONDO_TESTING_SPEC.md`
- `TONDO_LLM_FORM_SPEC.md`
- `TONDO_IMPLEMENTATION_TRACKER.md`
- `testing/tracker-graph.json`
- `testing/inventory.json`
- `testing/coverage-matrix.json`
- `testing/conformance-ratchet.json`
- `testing/quality-baseline.json`
- `docs/contracts/`
- `scripts/test-gate.sh`
- `scripts/quality-gate.sh`
- `scripts/fast-gate.sh`

The executable state already includes the Unicode frontend, a lossless CST and
recoverable parser, canonical formatter, resolution and visibility, type
interning, generics/traits/coherence, HIR/MIR/bytecode verification, and VM
execution. Implemented language/runtime areas include ownership and capability
checking (`Copy`, `Discard`, `Equatable`, `Key`, `Send`, `Share`), Option and
Result, match, collections, the numeric matrix, Unicode strings, bytes and
processes, structured async execution and `select`, static reflection and
derive, and the first-class test framework. Treat exact specs, tracker entries,
and executable evidence as authoritative over this summary.

## Verified state at handoff creation

The old-machine checkout was observed clean on `main`, with local HEAD exactly
equal to `origin/main` at the implementation base above. Before this handoff
file was added, no unrelated local changes were present.

The tracker and reliability state were rechecked immediately before writing
this document:

- Tracker: 646 tasks, 578 completed, 68 pending, 12 gates.
- Reliability evidence: current, with 2,862 tests and 437 requirements.
- Ready tracker tasks:
  `ARC-003`, `CONF-SEAL-FINAL-001`, `COW-NATIVE-001`, `ESCAPE-001`,
  `INCR-001`, `LSP-001`, `STD-CBOR-IMPL-001`,
  `STD-CIVIL-TIME-IMPL-001`, `STD-LOG-IMPL-001`, `STD-NET-IMPL-001`,
  `STD-REGEX-IMPL-001`, `STD-TOML-PERF-001`, `STD-UUID-IMPL-001`,
  `TLF-BENCH-REPRO-001`, and `TLF-CODEC-001`.
- The roadmap's explicit next owner block is `STD-TOML-PERF-001`. Do not pick a
  different ready task merely because the graph permits parallel work.

Current quality thresholds and last observed campaign:

- Formal global line-coverage baseline: 208,307 covered of 229,884 lines,
  9,061 basis points (90.61%).
- Function baseline: 8,701 basis points.
- Region baseline: 8,891 basis points.
- Allowed coverage drop: zero basis points.
- Mutation baseline: 6/6 selected critical mutants caught, 10,000 basis points,
  using cargo-mutants 27.1.0.
- Latest full workspace coverage campaign from the TOML test block: 246,652
  covered of 272,127 lines, 90.6385%; formal verification reported 9,063 basis
  points and passed.
- No latest-block local mutation campaign was run because of old-machine disk
  safety. Do not claim one. Use the larger disk on the new machine for heavy
  coverage and mutation artifacts.

Milestone state, at the level recorded by the tracker:

- M0 foundation: complete.
- M1 parser/formatter: complete.
- M2 bootstrap semantics: complete.
- M3 MIR/bytecode/VM: complete.
- M4 generics/traits/closures: complete.
- M5 ownership/borrows/memory: complete.
- M6 collections/numbers/text: complete.
- M7 async/select: implemented through the explicitly recorded hosted/runtime
  and conformance boundaries.
- M8 scripts/processes: complete.
- M9 unsafe/targets/toolchain: complete.
- M10 live corpus: complete.
- M10.5 reliability/testing: complete.
- M10.5c conformance infrastructure: complete; T0 is current.
- M10.6 first-class testing: complete; T0 is live.
- M10.7 static metaprogramming: complete.
- STD-0.1A: sealed technical draft; S1A is complete.
- M11: Cranelift Gate N1 is closed for `x86_64-unknown-linux-gnu`; later
  optimization work remains.
- STD-0.1B: active Wave 8; many owner leaves remain.
- G5 final-candidate seal: intentionally pending until the first real release
  candidate.

These are tracker promotions, not a released product. Confirm the underlying
contracts and evidence before relying on any individual capability.

## Last ten completed roadmap blocks

Listed newest first. Supporting repair commits are included where they are part
of the same coherent block.

1. `STD-TOML-TEST-001`: independent model, regression corpus, limits, spans,
   common adapters, 4,096 deterministic model seeds, and bounded fuzz at 128
   runs. The promotion is the hosted/kernel test boundary only. Commit
   `c70571e`; CI cargo-fuzz installation fix `3d087e2`.
2. `STD-TOML-IMPL-001`: TOML 1.1 kernel with typed and dynamic values/views,
   date/time values, tables, arrays of tables, canonical encoder, events,
   Reader, and Writer. Commit `66a4371`; executable-mode fix `8eff5f1`.
3. `STD-YAML-DOC-001`: executable safe-subset guide and honest usage/promotion
   boundary. Commit `42efce2`.
4. `STD-YAML-CONF-001`: common six-case VM/native conformance corpus. Commit
   `d2e1cac`; promotion-check alignment `9a6d3e0`.
5. `STD-YAML-PERF-001`: target-qualified hosted scalar campaign with 13
   workloads and 27 retained samples per workload. Closure commit `6b43fa2`,
   with its preceding probe/pin/mode support commits.
6. `STD-YAML-TEST-001`: independent bounded YAML model, corpus, regression, and
   fuzz boundary. Commit `e219fee`.
7. `STD-YAML-IMPL-001`: YAML 1.2 Core scalar hosted bridge and kernel, with
   explicit lowering/lint boundaries. Primary commit `f9cdea6`, followed by the
   relevant lint/lowering guard commits.
8. `STD-ENCODING-DOC-001`: executable Base64/hex usage contract. Commit
   `962fe95`.
9. `STD-ENCODING-CONF-001`: six-case common VM/native corpus. Commit `9dd62b4`.
10. `STD-ENCODING-PERF-001`: target-qualified hosted scalar 16-workload
    baseline. Commit `c8ccbe3`.

Immediately older relevant encoding blocks are `STD-ENCODING-TEST-001` at
`9366900` and the hosted implementation at `378f0df`.

## Exact boundary of the latest completed block

`STD-TOML-TEST-001` is complete. Its key files are:

- `testing/stdlib-toml-test.json`
- `crates/tondo-reliability/src/toml_model.rs`
- `crates/tondo-reliability/tests/toml_models.rs`
- `crates/tondo-stdlib/src/toml.rs`
- `fuzz/fuzz_targets/stdlib_toml.rs`
- `fuzz/corpus/stdlib_toml/seed`
- `docs/contracts/stdlib-toml-test.md`
- `docs/contracts/stdlib-toml.md`
- `scripts/stdlib-toml-test-check.sh`
- `scripts/stdlib-toml-test-test.sh`
- `scripts/stdlib-toml-fuzz.sh`

Fixed limits/evidence include reference nodes 128, scalar bytes 96, fuzz input
4,096 bytes, fuzz steps 512, 4,096 model seeds, 128 fuzz runs, toolchain
`nightly-2026-07-28`, and fuzz seed 4,113. A production defect in nested inline
tables inside arrays/event reconstruction was fixed as part of the block.

Actually observed checks for that block include:

- all focused TOML scripts passed;
- 16 focused stdlib TOML tests passed;
- full `tondo-stdlib`: 148 unit tests plus 3 codec conformance tests passed;
- full `tondo-reliability`: 75 unit tests, 6 main tests, and its integration
  suites, including 5 TOML model tests, passed;
- formatting, checking, and clippy passed;
- bounded fuzz completed 128 runs;
- formal coverage verification passed at 9,063 basis points.

The promotion explicitly covers the independent model, tests, fuzzing, and the
hosted scalar kernel. It does not claim a public compiler API, compiler
intrinsic, hosted-runtime registration as a public surface, native ABI, native
AOT lowering, SIMD, or the performance boundary. The current test register
still lists compiler TOML ABI, hosted runtime registration, and native AOT
lowering as implementation boundaries. The tracker nevertheless unlocks TOML
performance because it depends on the completed TOML implementation, not on a
fictional native promotion.

`std.toml` is a data codec. The toolchain's `tondo.toml` project manifest is a
separate owner and contract. Never route project manifests through `std.toml`
automatically or merge the two concepts.

## CI state and known branch-health issue

For exact implementation base `3d087e21df5e73eb8d7781682628c17bb34fc37f`:

- Normal push workflow run 34094628934 completed successfully. The strict Linux
  x86_64 job passed; portable and deterministic fuzz jobs were skipped on a
  normal push by design.
- Manual full workflow run 34094778002 completed with an overall failure.
  Deterministic fuzz smoke passed in 7m08s and strict Linux x86_64 passed in
  30m54s. Linux ARM64, macOS Apple Silicon, macOS Intel, and Windows x86_64
  portable jobs failed.

The portable failure is real and must not be called global green CI, but it was
not introduced by `STD-TOML-TEST-001` or its cargo-fuzz workflow repair. The
current evidence points to stale native-runtime test counters in
`crates/tondo-native-runtime/src/lib.rs`:

- On macOS and Windows,
  `native_blocking_handles_close_invalid_and_pending_states_explicitly` near
  line 8024 expected 26 and observed 23.
- On Linux ARM64,
  `native_blocking_pool_preserves_managed_payloads_and_rejects_invalid_budget`
  near line 7941 expected 24 and observed 23.
- The global native-runtime test mutex then becomes poisoned, causing dozens of
  cascading `PoisonError` failures.
- Because the Rust test command aborts, the portable report directory is not
  produced and artifact upload also fails.

An older full workflow, run 33187451443, was green before later native-runtime
evolution. Do not disable the tests, weaken counters, ignore the poisoned lock,
or attribute the problem to CI infrastructure without a focused reproduction.
Before any global-green claim, inspect the actual allocator/handle counter
contract, reproduce the first assertion failure on an available target, and
fix the underlying expectation or implementation with direct evidence. Keep
that repair separate from the TOML performance promotion unless a real
dependency forces them together.

## Remaining roadmap: all 68 pending tasks

### Candidate and release gates: 6

- `CONF-SEAL-FINAL-001`: intentionally reserved for the first real candidate.
- `STD-S1-SEAL-001`
- `REL-0.1-RC-001`
- `REL-SUPPLY-001`: includes a human license decision; do not infer it.
- `REL-INSTALL-001`
- `REL-PUBLISH-001`: requires explicit human publication authorization.

### Post-N1 optimization and tooling: 5

- `ARC-003`
- `COW-NATIVE-001`
- `ESCAPE-001`
- `INCR-001`
- `LSP-001`

### STD-0.1B owner leaves and coordination: 45

- Civil time: `STD-CIVIL-TIME-IMPL-001`,
  `STD-CIVIL-TIME-HOST-001`, `STD-CIVIL-TIME-TEST-001`,
  `STD-CIVIL-TIME-PERF-001`, `STD-CIVIL-TIME-CONF-001`,
  `STD-CIVIL-TIME-DOC-001`.
- TOML: `STD-TOML-PERF-001`, `STD-TOML-CONF-001`,
  `STD-TOML-DOC-001`.
- CBOR: `STD-CBOR-IMPL-001`, `STD-CBOR-TEST-001`,
  `STD-CBOR-PERF-001`, `STD-CBOR-CONF-001`, `STD-CBOR-DOC-001`.
- Regex: `STD-REGEX-IMPL-001`, `STD-REGEX-TEST-001`,
  `STD-REGEX-PERF-001`, `STD-REGEX-CONF-001`, `STD-REGEX-DOC-001`.
- UUID: `STD-UUID-IMPL-001`, `STD-UUID-HOST-001`,
  `STD-UUID-TEST-001`, `STD-UUID-PERF-001`, `STD-UUID-CONF-001`,
  `STD-UUID-DOC-001`.
- Networking: `STD-NET-IMPL-001`, `STD-NET-HOST-001`,
  `STD-NET-TEST-001`, `STD-NET-PERF-001`, `STD-NET-CONF-001`,
  `STD-NET-DOC-001`.
- Logging: `STD-LOG-IMPL-001`, `STD-LOG-HOST-001`,
  `STD-LOG-TEST-001`, `STD-LOG-PERF-001`, `STD-LOG-CONF-001`,
  `STD-LOG-DOC-001`.
- Coordination: `STD-B-OWNER-MATRIX-001`, `STD-B-IMPL-001`,
  `STD-B-HOST-001`, `STD-B-TEST-001`, `STD-B-PERF-001`,
  `STD-B-CONF-001`, `STD-B-DOC-001`, `DIAG-STDLIB-001`.

### Optional Tondo LLM Form companion lane: 12

- `TLF-BENCH-REPRO-001`
- `TLF-CODEC-001`
- `TLF-CANON-001`
- `TLF-MAP-001`
- `TLF-DIAG-001`
- `TLF-CLI-001`
- `TLF-PROP-001`
- `TLF-FUZZ-001`
- `TLF-EVAL-001`
- `TLF-CONF-001`
- `TLF-BUNDLE-001`
- `TLF-REL-001`

TLF is an optional companion and never changes the identity or hash of the base
Tondo 0.1 candidate.

## Immediate next block: `STD-TOML-PERF-001`

Tracker wording: measure TOML and fix parsing/encoding throughput, tail
latency, memory, allocations, and adversarial documents.

The intended scope is a target-qualified hosted scalar performance boundary,
not premature optimization and not a native-runtime promotion. Before choosing
the exact workload table, inspect and reuse the established patterns in:

- `testing/stdlib-yaml-performance.json`
- `docs/contracts/stdlib-yaml-performance.md`
- `scripts/stdlib-yaml-performance-check.sh`
- `scripts/stdlib-yaml-performance-test.sh`
- `scripts/stdlib-yaml-performance.sh`
- `testing/stdlib-encoding-performance.json`
- `docs/contracts/stdlib-encoding-performance.md`
- the corresponding `stdlib-encoding-performance*.sh` scripts;
- `testing/stdlib-toml.json`
- `testing/stdlib-toml-test.json`
- `docs/contracts/stdlib-toml.md`
- `docs/contracts/stdlib-toml-test.md`
- `crates/tondo-stdlib/src/toml.rs`
- `crates/tondo-reliability/src/toml_model.rs`
- `crates/tondo-reliability/tests/toml_models.rs`
- `crates/tondo-compiler/src/process_host.rs`.

Recommended implementation sequence:

1. Re-read the current specs, tracker entry, TOML contracts/registers, testing
   register, and the analogous YAML/encoding performance campaigns.
2. Define a reproducible, target-qualified protocol with monotonic timing,
   warmups, multiple measured repetitions in independent processes, retained
   outliers, deterministic fixtures, provenance-bound probe hash and Git
   revision, and no ambient path/PID/timestamp in report identity.
3. Cover materialized parse, borrowed/view parse where the current TOML API
   supports it, normal encode, canonical encode, event Reader/Writer paths, and
   adversarial bounded rejection. Include representative small and larger
   nested tables, arrays of tables, inline tables, arrays, strings/Unicode,
   numeric values, and local date/time forms. Exercise depth/node/scalar limits
   and malformed documents without publishing partial values.
4. Record latency distribution (median/P95/P99), throughput, bytes copied,
   logical allocations, peak logical memory, structural counters, rejection
   counters, selected dispatch, and terminal live-handle count where the actual
   bridge exposes them. Define every metric honestly; logical memory is not
   RSS, and logical allocations are not OS allocator calls.
5. Use the independent bounded TOML model and exact host outputs/errors as the
   oracle before timing. Fixture setup may be excluded from timed latency only
   if it remains represented in allocation/memory counters, matching the
   established campaign contract.
6. Add focused positive, negative, adversarial, lifecycle, determinism,
   provenance, and regression tests for the report/contract. Do not copy the
   YAML workload count blindly; choose the smallest complete TOML set and
   justify it in the contract.
7. Keep the selected route explicitly `hosted scalar` unless executable
   evidence genuinely establishes another route. State native runtime ABI,
   native AOT, SIMD, multiversion dispatch, and code-size claims as unmeasured
   or not claimed when that is the truth.
8. Update the TOML parent contract, standard-library spec, tracker, test
   inventory, coverage matrix, conformance manifest/ratchet, and generated
   provenance through repository-supported tools. Do not hand-edit generated
   evidence when a generator exists.
9. During development run the smallest focused checks. Before promotion, run
   the performance contract checker, focused tests, the reproducible campaign,
   relevant stdlib/compiler/reliability tests, formatting, check, clippy,
   documentation/conformance checks, coverage verification, and the
   repository-required gate. Preserve or improve the 90.61% baseline; never
   lower a threshold or hide a regression.
10. Inspect the complete diff, finish one coherent block, commit it directly to
    `main` with an accurate English message, normal-push to `origin/main`, and
    follow CI for the exact pushed SHA. Confirm the remote ref, not just local
    command success.

Set explicit time, sample, and disk budgets before broad benchmarks, coverage,
mutation, or full portable campaigns. Heavy artifacts belong on the new
machine's large disk, for example through a task-scoped `CARGO_TARGET_DIR`.
Performance generation commonly requires a clean tree; use the repository's
documented dirty-tree override only during local iteration and regenerate the
promoted evidence from the clean committed tree before final ratchet checks.

After TOML performance, the owner order is:

1. `STD-TOML-CONF-001`
2. `STD-TOML-DOC-001`
3. Re-read the tracker and take the next explicit unlocked owner block.

## Definition of done for every block

A block is complete only when all of the following are true:

- the requested executable behavior or artifact exists with no placeholder,
  silent fallback, or unexplained partial work introduced by the block;
- positive, negative, edge, lifecycle/concurrency, and regression paths are
  covered where meaningful;
- language/compiler/runtime/stdlib/docs/contracts/tests are synchronized only
  to the exact boundary actually implemented;
- inventories, matrices, conformance manifests, ratchets, and provenance are
  regenerated by repository tools and current;
- the effective diff is inspected and contains no unrelated user work;
- relevant focused checks and the required repository gate actually pass;
- coverage remains at or above its locked baseline and mutation evidence is not
  weakened;
- scripts added as executables are recorded as Git mode `100755` before commit;
- the coherent block is committed and normally pushed directly to `main`;
- local HEAD, `origin/main`, and the remote `refs/heads/main` resolve to the
  exact pushed SHA;
- CI for that exact SHA is observed and classified; portable failures are not
  hidden behind a green strict job;
- the tree is clean and the next tracker block is named.

Never claim a command, test, benchmark, coverage result, commit, push, or CI
result unless it was observed. A contract or model is not runtime execution;
hosted execution is not native AOT; a green subset is not global green CI.

## Resume checklist for the receiving agent

1. Confirm this handoff's final published SHA from Tony's transfer message.
2. In the intended checkout, inspect repository root, common Git directory,
   branch, HEAD, worktree status, remotes, and `origin/main`.
3. Fetch normally and stop on divergent history, unexpected local changes, or
   a remote tip that cannot be reconciled without history rewriting.
4. Read all applicable instructions and the implementation/Git/CI procedures.
5. Re-run tracker lint and the reliability current-evidence check; counts may
   legitimately have advanced after this document, so the live tracker wins.
6. Inspect the exact normal-push CI for the handoff commit and retain the known
   full portable failure as open until independently fixed and verified.
7. Start `STD-TOML-PERF-001` from its contracts and analogues. Do not start a
   parallel ready task.
8. Continue block by block, committing, pushing, and following exact-SHA CI
   after each coherent promotion.

Stop and ask Tony only for a real product/design/architecture/dependency/data
decision, explicit publication authorization, an unavoidable high-impact
action, conflicting worktree ownership, or a blocker reproduced after the
allowed evidence-backed correction attempts. Lack of disk on the old machine
is not a blocker on the new one; redirect task artifacts to the larger disk.
