# `std.regex` bounded VM/native process conformance

`STD-REGEX-CONF-001` compares seven shared case groups through verified hosted
bytecode and a fresh native Rust process. The register is
`testing/stdlib-regex-conformance.json`. Its
`verified-hosted-vm-adapter-and-native-stdlib-process` state follows the clean
capture, focused checks and source-bound quality/full gates. The earlier
`adapter-ready` state does not support promotion.
The next owner leaf is `STD-REGEX-DOC-001`.

Both adapters compile the same shared assertions and retained
`crates/tondo-reliability/tests/fixtures/regex-cases.json` bytes. There are 41
valid and 33 invalid vectors. Before VM execution, the independent grammar/path
model checks the 32 valid vectors within its domain; the nine Unicode vectors
retain their separately authored expectations. The full model suite also
compares all pure operations over 4,096 deterministic seeds. No independent
second complete Unicode table implementation is claimed.

The VM invokes a bodyless **test-only** callable and admits its computed String
through verified dispatch. The regex operation and assertions execute in the
Rust kernel. This does not implement source-level `std.regex` calls, production
VM registration or passage of a typed `RegexMatch` through the public VM API.
The native adapter is a Rust executable using the same kernel; it executes
neither a regex native ABI nor Cranelift regex lowering. SIMD and multiversion
dispatch remain absent. The runtime table counter returns zero after each
case. Regex owns no objects in that table, so that observation does not measure
regex allocations, ARC, cycle collection, allocator peak or RSS.

The case groups cover independently observable contracts:

- `retained-vectors`: all 41 valid first captures and full-match results, the
  retained complete iteration/replacement outputs, and all 33 nominal compile
  refusals with repeatable phase and UTF-8 byte spans.
- `capture-priorities`: local alternative and greedy/lazy priorities, full
  search, absent versus empty captures, last repetition captures, and shared
  immutable Rust `Arc` access. This is not a new `Regex::clone` API.
- `unicode-options`: Unicode 16 Tulu Tigalari, simple Kelvin folding without
  sharp-s expansion, no implicit normalization, and all five options.
- `lazy-lifecycle`: empty matches at UTF-8 offsets 0, 2 and 6, EOF once, one
  match retained before a match-limit error at byte 1, twice-checked fusion,
  retained span usability and subsequent independent search.
- `replacement`: named/numbered captures and literal dollar, first/all
  replacements, zero-width scalar progress, invalid templates even without a
  match, atomic output refusal at byte 2 and later successful reuse.
- `limits-errors`: exact pattern, program, input and step limit kind/phase/
  zero-width byte span/descriptor; middle-hyphen refusal at bytes 6..7 and
  invalid scalar slicing. Match/output limits are in the preceding groups.
- `route-boundary`: fixed scalar kernel and explicit unimplemented public,
  production VM, native ABI/AOT routes.

The capture uses fresh processes, exact ordered outputs, actual model/kernel/
VM prerequisite tests, pinned toolchain, recorded target/profile/incremental
flag, and source hashes before/after. A clean capture binds every declared
source to committed bytes and its Git tree. Reuse across later Markdown and
testing-register JSON changes is permitted only when the declared source hashes
and contract still agree. Other code/build differences from the captured Git
revision, including unlisted runtime submodules, invalidate the report.
Development evidence must remain
dirty and cannot promote the leaf. Artifact paths, timestamps, addresses and
process IDs stay outside report identity. Unsupported build overrides fail.

`scripts/stdlib-regex-conformance.sh` produces the comparison;
`scripts/stdlib-regex-conformance-test.sh` checks positive and negative
contracts. Missing/duplicate cases, changed observables/errors, false cleanup,
nonzero counters, source or identity drift and unsupported promotion claims
fail. The required consolidated quality campaign preserves every global/risk
line/function/region floor of 80% and the six critical mutation baseline.
The clean-source comparison passes all seven case groups. The consolidated
workspace campaign covers 292,638 of 319,506 lines (91.5908%); every global and
risk-scope line/function/region dimension meets the 80% floor, with all six
critical mutants caught. The formal coverage report excludes example and
integration-test source lines; the adapters and their assertions have separate
execution proof from the focused comparison and full functional gate.
That gate also passes the 206-case draft corpus, async/select repetitions and
the 128-run seeded regex fuzz smoke. Exact-SHA CI remains required for tracker
closure. Public Tondo, native ABI/AOT and release promotion remain outside this
proof.
