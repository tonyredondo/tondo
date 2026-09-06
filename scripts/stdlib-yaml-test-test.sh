#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-yaml-test.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.yaml tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.status = "contract-locked"' testing/stdlib-yaml-test.json \
    > "$tmp_dir/status.json"
expect_failure status env \
    TONDO_STDLIB_YAML_TEST_CONTRACT="$tmp_dir/status.json" \
    scripts/stdlib-yaml-test-check.sh

jq '.limits.max_fuzz_steps = 1024' testing/stdlib-yaml-test.json \
    > "$tmp_dir/step-limit.json"
expect_failure step-limit env \
    TONDO_STDLIB_YAML_TEST_CONTRACT="$tmp_dir/step-limit.json" \
    scripts/stdlib-yaml-test-check.sh

jq '.fuzz.smoke.result = "failed"' testing/stdlib-yaml-test.json \
    > "$tmp_dir/fuzz-result.json"
expect_failure fuzz-result env \
    TONDO_STDLIB_YAML_TEST_CONTRACT="$tmp_dir/fuzz-result.json" \
    scripts/stdlib-yaml-test-check.sh

jq '.sanitization.native_aot = "verified"' testing/stdlib-yaml-test.json \
    > "$tmp_dir/aot.json"
expect_failure aot-claim env \
    TONDO_STDLIB_YAML_TEST_CONTRACT="$tmp_dir/aot.json" \
    scripts/stdlib-yaml-test-check.sh

jq '.promotion.next_blocks = ["STD-YAML-TEST-001"]' testing/stdlib-yaml-test.json \
    > "$tmp_dir/promotion.json"
expect_failure promotion env \
    TONDO_STDLIB_YAML_TEST_CONTRACT="$tmp_dir/promotion.json" \
    scripts/stdlib-yaml-test-check.sh

jq '.model.sources = .model.sources[0:1]' testing/stdlib-yaml-test.json \
    > "$tmp_dir/missing-model.json"
expect_failure missing-model env \
    TONDO_STDLIB_YAML_TEST_CONTRACT="$tmp_dir/missing-model.json" \
    scripts/stdlib-yaml-test-check.sh

jq '.parent_contract = "testing/other-test.json"' testing/stdlib-yaml-test.json \
    > "$tmp_dir/parent-link.json"
expect_failure parent-link env \
    TONDO_STDLIB_YAML_TEST_CONTRACT="$tmp_dir/parent-link.json" \
    scripts/stdlib-yaml-test-check.sh

scripts/stdlib-yaml-test-check.sh >/dev/null

target_dir="${CARGO_TARGET_DIR:-$root/target}"
CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-reliability \
    --test yaml_models --locked
CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-stdlib \
    yaml:: --locked
CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-compiler \
    'process_host::tests::yaml_' --locked
CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-vm \
    default_host_and_closed_runtime_helpers_have_explicit_boundaries --locked
CARGO_TARGET_DIR="$target_dir" cargo check -q \
    --manifest-path fuzz/Cargo.toml --bin stdlib_yaml --locked

echo "std.yaml tests: OK (negative contract cases; independent model; scalar/hosted regressions; fuzz harness)"
