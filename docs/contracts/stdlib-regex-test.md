# Regex independent model and bounded testing

This contract belongs to `STD-REGEX-TEST-001` and
`testing/stdlib-regex-test.json`. Its parent is `stdlib-regex.md`. The promoted
boundary is the independent model, retained regression corpus and bounded fuzz
against the Rust scalar kernel. Public Tondo calls, production VM registration,
native ABI, native AOT, SIMD and performance are not claimed.

## Independent oracle

`crates/tondo-reliability/src/regex_model.rs` and `regex_model/grammar.rs`
use only the Rust standard library. They implement their own recursive grammar
and ordered path enumeration, without importing the production parser, NFA,
Unicode tables or kernel. This deliberately different algorithm is bounded;
it is not a production matcher or a claim of linear model complexity.

The domain has 128 pattern bytes and AST nodes, depth 8, eight captures, finite
repeat bounds of eight, 96 input bytes and 32 input scalars, 65,536 evaluated
path steps, 128 replacement bytes and 4,096 output bytes. `Limit` means outside
that domain; it is not
a malformed-pattern verdict or the kernel's step counter. `OutsideDomain`
covers unsupported model features, including Unicode properties and full
Unicode folding. ASCII shorthands, word boundaries and case folding are compared
only on ASCII input. Unicode literals, scalar ranges, anchors, dot and zero-width
progress are compared without those ASCII restrictions when folding is disabled.

The oracle selects the leftmost start, alternatives in pattern order and local
greedy/lazy priorities, including `ungreedy`. Full matching filters candidates
for end-of-input before selection. Captures distinguish absence from a
participating empty match and retain earlier iterations' captures. Unbounded
nullable repetition permits one no-progress exit after its mandatory minimum;
bounded repetition respects its finite count. The reference enumerator is eager
for comparison; the production cursor remains lazy and fallible.

Replacement has an independent tokenizer for `$0`, `$N`, `${name}` and `$$`.
It checks templates before searching, inserts empty text for absent captures,
copies unmatched text, advances empty matches by a Unicode scalar and discards
the output on a limit error. All spans are half-open UTF-8 byte ranges.

## Retained vectors and generated comparisons

`crates/tondo-reliability/tests/fixtures/regex-cases.json` retains 41 valid
and 33 invalid cases with unique identities. Thirty-two valid cases fit the
independent model. Nine separately authored Unicode vectors cover Unicode 16
Tulu Tigalari Script/Script_Extensions, Letter and Alphabetic, marks in `word`,
Join_Control exclusion, Kelvin simple folding and the absence of multi-scalar
sharp-S folding. These are exact owner expectations, not a second full Unicode
table implementation. A further vector verifies that no normalization occurs.

Ten integration tests compare 4,096 deterministic generated seeds across
compile metadata, find, full matching, every capture, enumeration and both
replacement operations. They also check the retained errors, scalar spans,
reference bounds, replay limits, independent cursors, concurrent immutable
program use, owned captures after program destruction, atomic output errors,
all twelve limit identities and all five options. The ten canonical kernel
tests remain part of the proof.

The independent grammar exposed a production class-admission defect:
`[a-b-c]` accepted an unescaped middle literal hyphen. The correction rejects
only a verbatim middle hyphen, preserving first, last, escaped and hex forms;
negated and non-ASCII-prefix regressions verify the exact offending byte span.

## Bounded fuzz and promotion

`scripts/stdlib-regex-fuzz.sh` runs `stdlib_regex` with
`nightly-2026-07-28`, 128 runs, seed 4113, 4,096 input bytes, at most 512
reference replay steps, ten seconds per input and 4,096 MiB RSS. Each smoke
starts from the retained seed and all 74 vectors in a fresh artifact directory.
Generated discoveries are retained separately and do not alter the next run's
starting corpus. The fixture envelope is one option byte followed by
NUL-separated pattern, input and template UTF-8 bytes.

The harness compares independent generated operations and repeated replay
summaries, then checks arbitrary bounded patterns, compile/error repeatability,
matching, replacement, capture boundaries and terminal iterator errors.
Reference limits and out-of-domain results are never promoted to kernel
rejections. Arbitrary kernel operations use explicit smaller limits; a kernel
limit error does not contradict a successful bounded model operation.

The minimal fuzz graph links stdlib without compiler, VM, conformance runner
or reliability CLI. Both that graph and the ordinary full fuzz graph must
build. Sanitizers remain enabled. A 128-run smoke is finite evidence, not a
claim of exhaustive testing or freedom from all defects.

`ready` records implemented testing with the consolidated quality and functional
proof still pending. `verified` requires the full functional gate, current
source-bound coverage at or above 80% in every global/risk dimension and all six
selected critical mutants caught. Contract negatives exercise both states and
refuse invented public/native promotion. The next owner is
`STD-REGEX-PERF-001`; shared conformance and usage follow separately.
