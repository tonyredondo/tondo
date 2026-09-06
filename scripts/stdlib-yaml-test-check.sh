#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_YAML_TEST_CONTRACT:-$root/testing/stdlib-yaml-test.json}"

die() {
    echo "std.yaml tests: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing testing contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "testing contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "testing contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-yaml-testing/1"
  and .owner == "std.yaml"
  and .parent_owner == "std"
  and .edition == "0.1"
  and .phase == "STD-0.1B"
  and .task == "STD-YAML-TEST-001"
  and .status == "verified"
  and .contract == "docs/contracts/stdlib-yaml-test.md"
  and .implementation_contract == "testing/stdlib-yaml.json"
  and .spec == "TONDO_STANDARD_LIBRARY_SPEC.md"
  and .language_spec == "TONDO_LANGUAGE_SPEC.md"
  and .parent_contract == "testing/stdlib-yaml.json"
  and .layer == "B7"
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
  and (.model.laws | type == "array" and length >= 16)
  and .model.sequence_seeds == 4096
  and .model.oracle == "independent bounded YAML Core scalar and canonical-value model with deterministic malformed replay"
  and .model.command == "cargo test -p tondo-reliability --test yaml_models --locked"
  and .test.status == "verified"
  and (.test.sources | type == "array" and length == 3)
  and (.test.commands | type == "array" and length == 6)
  and (.test.cases | type == "array" and length >= 12)
  and .test.oracle == "independent model, scalar stdlib tests and hosted regression suites agree on values, canonical bytes, errors and terminal state"
  and .fuzz.status == "verified"
  and .fuzz.target == "stdlib_yaml"
  and .fuzz.source == "fuzz/fuzz_targets/stdlib_yaml.rs"
  and .fuzz.corpus == "fuzz/corpus/stdlib_yaml/seed"
  and .fuzz.input_limit_bytes == 4096
  and .fuzz.step_limit == 512
  and .fuzz.smoke.runs == 128
  and .fuzz.smoke.seed == 4107
  and .fuzz.smoke.toolchain == "nightly-2026-07-28"
  and .fuzz.smoke.result == "passed"
  and .fuzz.oracle == "panic-free deterministic replay, independent canonical bytes, production parse/re-encode stability and explicit bounds"
  and .fuzz.command == "TONDO_YAML_FUZZ_RUNS=128 scripts/stdlib-yaml-fuzz.sh"
  and .sanitization.status == "bounded-safe-rust-no-unsafe-boundary"
  and .sanitization.applicable == false
  and .sanitization.native_aot == "not-claimed"
  and .promotion.model_test_fuzz_complete == true
  and .promotion.implementation_pending == []
  and .promotion.next_blocks == ["STD-YAML-CONF-001"]
  and .promotion.remaining == []
' "$contract" >/dev/null || die "invalid machine-readable YAML testing contract"

for path in \
    docs/contracts/stdlib-yaml-test.md \
    TONDO_STANDARD_LIBRARY_SPEC.md \
    TONDO_LANGUAGE_SPEC.md \
    TONDO_IMPLEMENTATION_TRACKER.md \
    testing/stdlib-yaml.json \
    testing/inventory.json \
    testing/coverage-matrix.json; do
    [[ -f "$root/$path" ]] || die "missing linked path: $path"
done

while IFS= read -r path; do
    [[ -f "$root/$path" ]] || die "missing model/test source: $path"
done < <(jq -r '.model.sources[], .test.sources[], .fuzz.source, .fuzz.corpus' "$contract")

for path in \
    scripts/stdlib-yaml-test-check.sh \
    scripts/stdlib-yaml-test-test.sh \
    scripts/stdlib-yaml-fuzz.sh; do
    [[ -x "$root/$path" ]] || die "runner is not executable: $path"
done

grep -Fq 'name = "stdlib_yaml"' "$root/fuzz/Cargo.toml" \
    || die "fuzz manifest misses stdlib_yaml"
[[ -s "$root/fuzz/corpus/stdlib_yaml/seed" ]] || die "YAML fuzz corpus is empty"

for marker in \
    'MAX_YAML_FUZZ_INPUT_BYTES' \
    'MAX_YAML_FUZZ_STEPS' \
    'pub enum ReferenceValue' \
    'pub fn parse_core_scalar' \
    'pub fn render_canonical' \
    'pub fn run_yaml_fuzz_case'; do
    grep -Fq "$marker" "$root/crates/tondo-reliability/src/yaml_model.rs" \
        || die "model misses anchor: $marker"
done

for marker in \
    'core_scalar_model_matches_production' \
    'canonical_model_matches_production_and_is_idempotent' \
    'invalid_scalar_model_and_security_boundaries_match_production' \
    'one_byte_reader_and_event_decoder_preserve_stream_boundaries' \
    'bounded_yaml_model_replay_is_deterministic_and_bounded' \
    'stream_limits_and_terminal_rejection_are_explicit'; do
    grep -Fq "$marker" "$root/crates/tondo-reliability/tests/yaml_models.rs" \
        || die "test suite misses anchor: $marker"
done

for marker in \
    'std.yaml model invariant failed' \
    'std.yaml replay diverged' \
    'MAX_YAML_FUZZ_INPUT_BYTES' \
    'stdlib_yaml'; do
    grep -Fq "$marker" "$root/fuzz/fuzz_targets/stdlib_yaml.rs" \
        || die "fuzz target misses anchor: $marker"
done

jq -e '
  .testing_contract == "testing/stdlib-yaml-test.json"
  and .testing_document == "docs/contracts/stdlib-yaml-test.md"
  and .implementation.required_follow_ups == ["STD-YAML-CONF-001", "STD-YAML-DOC-001"]
  and .promotion.next_blocks == ["STD-YAML-CONF-001"]
' "$root/testing/stdlib-yaml.json" >/dev/null \
    || die "parent YAML registry does not expose the promoted testing boundary"

grep -Fq 'stdlib-yaml-test.json' "$root/TONDO_STANDARD_LIBRARY_SPEC.md" \
    || die "stdlib spec does not link the YAML testing contract"
grep -Fq 'stdlib-yaml-test.md' "$root/docs/contracts/stdlib-yaml.md" \
    || die "YAML owner document does not link the testing contract"
grep -Fq '[x] **STD-YAML-TEST-001' "$root/TONDO_IMPLEMENTATION_TRACKER.md" \
    || die "tracker does not record the YAML testing leaf"

echo "std.yaml tests: OK (independent Core model; hosted regressions; bounded fuzz and terminal/security cases)"
