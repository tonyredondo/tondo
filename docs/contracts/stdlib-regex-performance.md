# `std.regex` direct Rust kernel performance

`STD-REGEX-PERF-001` establishes a target-qualified baseline of the existing
ordered Thompson NFA. The selected route is `scalar-fixed-target`, backend
`rust-stdlib-kernel`, target `x86_64-unknown-linux-gnu`, and Rust `test` profile.
The default profile is unoptimized; explicit Rust flags remain recorded in the
measurement identity. This is not a hosted VM measurement or a native Tondo benchmark.
Public compiler calls, production VM registration, native ABI/AOT, SIMD,
multiversion dispatch and code size remain unclaimed or unmeasured.
`native_live_handles` is unmeasured and must remain `null`.

The register advances from `measurement-ready` to
`verified-stdlib-kernel-baseline` after the clean-source capture and the
required functional and source-bound quality gates. A development report
cannot close this leaf. The campaign uses committed probe bytes from `495a573`;
its 19 workloads retain 513 actual measurements. Workspace quality verifies
292,637 of 319,506 lines (91.5905%), every global/risk line/function/region
dimension at or above 80%, and all six selected critical mutants caught. The
full gate includes the 206-case draft suite and async/select repetitions.

## Protocol and independent expectations

The test-only probe lives in `crates/tondo-stdlib/src/regex/performance.rs`.
Its private program inspection is compiled only by the Rust test harness;
it adds no public Tondo or Rust diagnostic API. The runner first executes the
independent model suite and then the exact fixture, lifecycle and batch tests.

Three independent processes each execute three warmup batches and nine
measured batches per workload. Each batch contains sixteen operations.
`Instant` provides monotonic timing. All 27 measurements retain process
ordinals 1–3 and repetition ordinals 0–8, including outliers. Median, P95 and
P99 use nearest rank. With 27 samples P99 is the maximum; this bounded
campaign does not guarantee production tail latency. Exact comparisons occur
before timing, during warmups and after each measured batch. Status assertions,
`black_box`, cursor termination checks and result destruction stay timed.

Nineteen workloads cover the distinct executable routes and refusals:

| Family | Cases | Purpose |
| --- | ---: | --- |
| Compile | 3 | Literal; named captures and finite repeats; Unicode 16 property tables |
| Search and captures | 5 | Local alternative priority; full matching; UTF-8 captures; fixed ambiguous program at 64 and 1,024 scalars |
| Lazy iteration | 3 | Dense captures; zero-width scalar progress; bounded suffix rescanning |
| Replacement | 2 | First/all matches with named captures and literal dollar |
| Rejection | 6 | Closed syntax, program, input, step, match and output limits |

Nine successful fixtures derive their complete expectations from the
independent bounded grammar/path model. The Unicode property compile fixture
uses separately authored Unicode 16 expectations, including an assigned Tulu
Tigalari scalar and an ASCII refusal; no second complete Unicode table oracle
is claimed. The two ambiguous searches require a final `b` that is absent
from all-`a` inputs. The enumeration fixture has no `z`, so its first alternative
cannot succeed and the local fallback publishes one `a` at each offset.
Their bounded representatives additionally run the independent path oracle.
The larger authored fixtures are explicitly outside its 32-scalar domain.
Reference limits never imply production rejection.

Every error expectation specifies nominal kind, phase, byte offset, span and
limit. The match-limit iterator retains its first valid match, publishes one
error and then remains fused. The output-limit replacement returns no partial
String. Successful empty matches progress through UTF-8 scalar boundaries and
publish EOF once. The tests compare complete captures, including absent
groups, and verify the terminal cursor twice.

The capture workload times `Regex::find` constructing the complete capture
payload. Individual capture access and exact span comparison occur outside
timing; no separate capture-lookup throughput is claimed.

The scale pair measures fixed-program single-search work. The suffix-rescan
fixture separately records iteration's cumulative step count. Neither the
campaign nor the parent kernel contract claims total enumeration linear in
input when each next match can rescan the remaining suffix. This baseline
selects no new optimization or dispatch route and claims no speedup.

## Measured and modeled counters

Latency is measured. `program_states`, `semantic_states`, `class_ranges` and
`capture_slots` inspect the actual compiled program. Program and capture
counts are zero when compilation refuses publication. `matching_steps` reads
the actual search/cursor budget counter outside timing. Compile and replacement
do not expose that counter, so their value is `null`, never an invented zero.
Input refusal occurs before search and legitimately records zero steps.

`matches` counts delivered match records per operation. Boolean full matching
and String replacement publish no match records. Iteration refusal records
its valid prefix. Structural counts and steps are per operation; sixteen
operations form each timed batch. `adversarial_rejections` counts the sixteen
nominal refusals in a rejecting batch. `terminal_open_cursors` is zero only
after checking fusion and lexical drop; it is not a native handle counter.

`bytes_copied` counts selected payload transport per batch: the retained
pattern copy for successful compilation and final emitted replacement bytes.
Span-based matching copies no input text. Refusals publish no payload.
Private parser/capture-vector copies and unpublished staging are excluded.

`allocations` counts selected owned fixture, program and result identities,
including patterns, inputs, templates, name/program/range tables, named strings,
capture vectors and result containers. It does not count allocator calls,
capacity growth or every transient thread duplication.

`logical_memory_bytes` models retained fixture storage, one program, the
engine's conservative search admission reservation and one operation result.
Expected payload identities retained by the fixture remain represented even
though fixture/oracle setup is outside timed latency. The compile route has
one resulting program; search retains one compiled program. This is modeled
logical memory, not RSS, observed allocator peak or an OOM recovery guarantee.
Tests bind its search reservation to the actual admitted heap boundary.

The model excludes oracle temporaries, dependency parser/compile worklists,
replacement tokens and unpublished output staging, capacity slack, Arc control
blocks, other workloads and OS/native allocation state. Every report retains
these exclusions explicitly. The pinned Rust target/profile fixes the layout
used by `size_of`; no cross-target layout equivalence is implied.

Throughput reports batch operations per second and selected input/output bytes
per second using the median batch latency. Compilation uses pattern bytes,
replacement uses emitted bytes, and matching/refusal uses attempted input
bytes. Attempted-byte throughput is descriptive and is not successful output
throughput for a rejecting workload.

## Provenance and closure

Each workload identity binds suite/id, probe hash, source tree, target,
backend, profile, toolchain, both Rust flag sources, incremental setting and
Git revision. CPU/OS descriptors stay outside identity. Paths, PIDs,
timestamps and ambient environment state never enter it. Unsupported compiler,
profile and target overrides are refused. Sources and revision are checked
before and after capture; a clean capture also verifies that its revision
contains the exact probe. Dirty captures are development-only.

Run `scripts/stdlib-regex-performance-check.sh`,
`scripts/stdlib-regex-performance-test.sh`, and
`scripts/stdlib-regex-performance.sh`. The JSON report is retained in the
selected target directory under `reliability/evidence/stdlib-regex-performance.json`.
Focused positive, negative, provenance, retained-outlier, lifecycle and state
transition checks precede the required consolidated quality/full functional
gates. Promotion keeps every global/risk coverage dimension at or above 80%
and preserves the selected mutation baseline. Shared conformance and usage
documentation remain their separate owner leaves.
