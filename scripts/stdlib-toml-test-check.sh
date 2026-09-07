#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_TOML_TEST_CONTRACT:-$root/testing/stdlib-toml-test.json}"

die() {
    echo "std.toml tests: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing testing contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "testing contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "testing contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-toml-testing/1"
  and .owner == "std.toml"
  and .parent_owner == "std"
  and .edition == "0.1"
  and .phase == "STD-0.1B"
  and .task == "STD-TOML-TEST-001"
  and .status == "verified"
  and .contract == "docs/contracts/stdlib-toml-test.md"
  and .implementation_contract == "testing/stdlib-toml.json"
  and .spec == "TONDO_STANDARD_LIBRARY_SPEC.md"
  and .language_spec == "TONDO_LANGUAGE_SPEC.md"
  and .parent_contract == "testing/stdlib-toml.json"
  and .layer == "B8"
  and .kind == "reliability-facing"
  and .target == "independent-reference-and-hosted-regression-boundary"
  and .limits.max_reference_nodes == 128
  and .limits.max_reference_scalar_bytes == 96
  and .limits.max_fuzz_input_bytes == 4096
  and .limits.max_fuzz_steps == 512
  and .limits.model_seed_count == 4096
  and .limits.fuzz_smoke_runs == 128
  and .model.status == "verified"
  and (.model.sources | type == "array" and length == 2)
  and (.model.laws | type == "array" and length >= 14)
  and .model.sequence_seeds == 4096
  and .model.oracle == "independent bounded TOML canonical-value renderer with deterministic replay"
  and .test.status == "verified"
  and (.test.sources | type == "array" and length == 2)
  and (.test.commands | type == "array" and length == 5)
  and (.test.cases | type == "array" and length >= 14)
  and .fuzz.status == "verified"
  and .fuzz.target == "stdlib_toml"
  and .fuzz.source == "fuzz/fuzz_targets/stdlib_toml.rs"
  and .fuzz.corpus == "fuzz/corpus/stdlib_toml/seed"
  and .fuzz.input_limit_bytes == 4096
  and .fuzz.step_limit == 512
  and .fuzz.smoke.runs == 128
  and .fuzz.smoke.seed == 4113
  and .fuzz.smoke.toolchain == "nightly-2026-07-28"
  and .fuzz.smoke.result == "passed"
  and .sanitization.status == "bounded-safe-rust-no-unsafe-boundary"
  and .sanitization.applicable == false
  and .sanitization.native_aot == "not-claimed"
  and .promotion.model_test_fuzz_complete == true
  and .promotion.implementation_pending == ["compiler-toml-abi", "hosted-runtime-registration", "native-aot-lowering"]
  and .promotion.next_blocks == ["STD-TOML-PERF-001"]
  and .promotion.remaining == ["STD-TOML-PERF-001", "STD-TOML-CONF-001", "STD-TOML-DOC-001"]
' "$contract" >/dev/null || die "invalid machine-readable TOML testing contract"

for path in \
    docs/contracts/stdlib-toml-test.md \
    TONDO_STANDARD_LIBRARY_SPEC.md \
    TONDO_LANGUAGE_SPEC.md \
    TONDO_IMPLEMENTATION_TRACKER.md \
    testing/stdlib-toml.json \
    testing/inventory.json \
    testing/coverage-matrix.json; do
    [[ -f "$root/$path" ]] || die "missing linked path: $path"
done

while IFS= read -r path; do
    [[ -f "$root/$path" ]] || die "missing model/test source: $path"
done < <(jq -r '.model.sources[], .test.sources[], .fuzz.source, .fuzz.corpus' "$contract")

for path in \
    scripts/stdlib-toml-test-check.sh \
    scripts/stdlib-toml-test-test.sh \
    scripts/stdlib-toml-fuzz.sh; do
    [[ -x "$root/$path" ]] || die "runner is not executable: $path"
done

grep -Fq 'name = "stdlib_toml"' "$root/fuzz/Cargo.toml" \
    || die "fuzz manifest misses stdlib_toml"
[[ -s "$root/fuzz/corpus/stdlib_toml/seed" ]] || die "TOML fuzz corpus is empty"

for marker in \
    'MAX_TOML_FUZZ_INPUT_BYTES' \
    'MAX_TOML_FUZZ_STEPS' \
    'pub enum ReferenceValue' \
    'pub fn render_canonical' \
    'pub fn run_toml_fuzz_case'; do
    grep -Fq "$marker" "$root/crates/tondo-reliability/src/toml_model.rs" \
        || die "model misses anchor: $marker"
done

for marker in \
    'bounded_reference_renderer_matches_hosted_canonical_bytes' \
    'reference_model_replay_is_deterministic_bounded_and_independent' \
    'invalid_corpus_preserves_error_kinds_paths_and_spans' \
    'one_byte_reader_and_common_decoder_preserve_boundaries' \
    'stream_limits_and_terminal_states_remain_explicit'; do
    grep -Fq "$marker" "$root/crates/tondo-reliability/tests/toml_models.rs" \
        || die "test suite misses anchor: $marker"
done

for marker in \
    'std.toml model invariant failed' \
    'std.toml replay diverged' \
    'MAX_TOML_FUZZ_INPUT_BYTES' \
    'stdlib_toml'; do
    grep -Fq "$marker" "$root/fuzz/fuzz_targets/stdlib_toml.rs" \
        || die "fuzz target misses anchor: $marker"
done

grep -Fq 'testing_contract' "$root/scripts/stdlib-toml-check.sh" \
    || die "owner check does not validate the TOML testing contract"
grep -Fq 'stdlib-toml-test.md' "$root/docs/contracts/stdlib-toml.md" \
    || die "TOML owner document does not link the testing contract"
grep -Fq 'stdlib-toml-test.json' "$root/TONDO_STANDARD_LIBRARY_SPEC.md" \
    || die "stdlib spec does not link the TOML testing contract"
grep -Fq '[x] **STD-TOML-TEST-001' "$root/TONDO_IMPLEMENTATION_TRACKER.md" \
    || die "tracker does not record the TOML testing leaf"

echo "std.toml tests: OK (independent model; hosted regressions; bounded fuzz; limits; spans and terminal security cases)"
