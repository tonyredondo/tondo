#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_ENCODING_TEST_CONTRACT:-$root/testing/stdlib-encoding-test.json}"

die() {
    echo "std.encoding tests: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing testing contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "testing contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "testing contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-encoding-testing/1"
  and .owner == "std.encoding"
  and .parent_owner == "std"
  and .edition == "0.1"
  and .phase == "STD-0.1B"
  and .task == "STD-ENCODING-TEST-001"
  and .status == "verified"
  and .contract == "docs/contracts/stdlib-encoding-test.md"
  and .implementation_contract == "testing/stdlib-encoding.json"
  and .spec == "TONDO_STANDARD_LIBRARY_SPEC.md"
  and .language_spec == "TONDO_LANGUAGE_SPEC.md"
  and .parent_contract == "testing/stdlib-encoding.json"
  and .layer == "B6"
  and .kind == "reliability-facing"
  and .target == "independent-reference-and-hosted-regression-boundary"
  and .limits.max_reference_payload_bytes == 96
  and .limits.max_exhaustive_vector_bytes == 8
  and .limits.max_fuzz_input_bytes == 4096
  and .limits.max_fuzz_steps == 512
  and .limits.model_seed_count == 4096
  and .limits.fuzz_smoke_runs == 128
  and .model.status == "verified"
  and (.model.sources | type == "array" and length == 2)
  and (.model.laws | type == "array" and length >= 15)
  and .model.sequence_seeds == 4096
  and .model.oracle == "independent bounded Base64/hex wire model with exact error kinds and offsets"
  and .model.command == "cargo test -p tondo-reliability --test encoding_models --locked"
  and .test.status == "verified"
  and (.test.sources | type == "array" and length == 3)
  and (.test.commands | type == "array" and length == 5)
  and (.test.cases | type == "array" and length >= 14)
  and .test.oracle == "production scalar materialized and incremental paths agree with the independent model on bytes, errors and offsets"
  and .fuzz.status == "verified"
  and .fuzz.target == "stdlib_encoding"
  and .fuzz.source == "fuzz/fuzz_targets/stdlib_encoding.rs"
  and .fuzz.corpus == "fuzz/corpus/stdlib_encoding/seed"
  and .fuzz.input_limit_bytes == 4096
  and .fuzz.step_limit == 512
  and .fuzz.smoke.runs == 128
  and .fuzz.smoke.seed == 4105
  and .fuzz.smoke.toolchain == "nightly-2026-07-28"
  and .fuzz.smoke.result == "passed"
  and .fuzz.oracle == "panic-free deterministic replay, scalar/reference byte equality, exact wire errors and chunk invariance"
  and .fuzz.command == "TONDO_ENCODING_FUZZ_RUNS=128 scripts/stdlib-encoding-fuzz.sh"
  and .sanitization.status == "bounded-safe-rust-no-unsafe-boundary"
  and .sanitization.applicable == false
  and .sanitization.native_aot == "not-claimed"
  and .promotion.model_test_fuzz_complete == true
  and .promotion.implementation_pending == []
  and .promotion.next_blocks == ["STD-YAML-IMPL-001"]
  and .promotion.remaining == []
' "$contract" >/dev/null || die "invalid machine-readable encoding testing contract"

for path in \
    docs/contracts/stdlib-encoding-test.md \
    TONDO_STANDARD_LIBRARY_SPEC.md \
    TONDO_LANGUAGE_SPEC.md \
    TONDO_IMPLEMENTATION_TRACKER.md \
    testing/stdlib-encoding.json \
    testing/inventory.json \
    testing/coverage-matrix.json; do
    [[ -f "$root/$path" ]] || die "missing linked path: $path"
done

while IFS= read -r path; do
    [[ -f "$root/$path" ]] || die "missing model/test source: $path"
done < <(jq -r '.model.sources[], .test.sources[], .fuzz.source, .fuzz.corpus' "$contract")

for path in \
    scripts/stdlib-encoding-test-check.sh \
    scripts/stdlib-encoding-test-test.sh \
    scripts/stdlib-encoding-fuzz.sh; do
    [[ -x "$root/$path" ]] || die "runner is not executable: $path"
done

grep -Fq 'name = "stdlib_encoding"' "$root/fuzz/Cargo.toml" \
    || die "fuzz manifest misses stdlib_encoding"
[[ -s "$root/fuzz/corpus/stdlib_encoding/seed" ]] || die "encoding fuzz corpus is empty"

for marker in \
    'MAX_ENCODING_FUZZ_INPUT_BYTES' \
    'MAX_ENCODING_FUZZ_STEPS' \
    'pub enum ReferenceCodec' \
    'fn decode_base64' \
    'fn decode_hex' \
    'pub fn run_encoding_fuzz_case'; do
    grep -Fq "$marker" "$root/crates/tondo-reliability/src/encoding_model.rs" \
        || die "model misses anchor: $marker"
done

for marker in \
    'official_vectors_match_the_independent_model_at_every_chunk_boundary' \
    'invalid_padding_alphabet_case_and_length_errors_are_byte_exact' \
    'limits_are_checked_before_publication_and_errors_close_the_handle' \
    'bounded_model_replay_is_deterministic_and_has_explicit_limits'; do
    grep -Fq "$marker" "$root/crates/tondo-reliability/tests/encoding_models.rs" \
        || die "test suite misses anchor: $marker"
done

for marker in \
    'std.encoding model invariant failed' \
    'std.encoding replay diverged' \
    'MAX_ENCODING_FUZZ_INPUT_BYTES' \
    'stdlib_encoding'; do
    grep -Fq "$marker" "$root/fuzz/fuzz_targets/stdlib_encoding.rs" \
        || die "fuzz target misses anchor: $marker"
done

jq -e '
  .testing_contract == "testing/stdlib-encoding-test.json"
  and .testing_document == "docs/contracts/stdlib-encoding-test.md"
  and .implementation.required_follow_ups == []
  and .promotion.next_blocks == ["STD-YAML-IMPL-001"]
' "$root/testing/stdlib-encoding.json" >/dev/null \
    || die "parent encoding registry does not expose the promoted testing boundary"

grep -Fq 'stdlib-encoding-test.json' "$root/TONDO_STANDARD_LIBRARY_SPEC.md" \
    || die "stdlib spec does not link the encoding testing contract"
grep -Fq 'stdlib-encoding-test.md' "$root/docs/contracts/stdlib-encoding.md" \
    || die "encoding document does not link the testing contract"
grep -Fq '[x] **STD-ENCODING-TEST-001' "$root/TONDO_IMPLEMENTATION_TRACKER.md" \
    || die "tracker does not record the encoding testing leaf"

echo "std.encoding tests: OK (independent wire model; chunk/error/limit cases; bounded fuzz boundary)"
