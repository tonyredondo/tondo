#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-toml-test.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.toml tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.status = "contract-locked"' testing/stdlib-toml-test.json > "$tmp_dir/status.json"
expect_failure status env TONDO_STDLIB_TOML_TEST_CONTRACT="$tmp_dir/status.json" scripts/stdlib-toml-test-check.sh

jq '.limits.max_fuzz_steps = 1024' testing/stdlib-toml-test.json > "$tmp_dir/steps.json"
expect_failure steps env TONDO_STDLIB_TOML_TEST_CONTRACT="$tmp_dir/steps.json" scripts/stdlib-toml-test-check.sh

jq '.fuzz.smoke.result = "failed"' testing/stdlib-toml-test.json > "$tmp_dir/fuzz.json"
expect_failure fuzz-result env TONDO_STDLIB_TOML_TEST_CONTRACT="$tmp_dir/fuzz.json" scripts/stdlib-toml-test-check.sh

jq '.sanitization.native_aot = "verified"' testing/stdlib-toml-test.json > "$tmp_dir/aot.json"
expect_failure aot-claim env TONDO_STDLIB_TOML_TEST_CONTRACT="$tmp_dir/aot.json" scripts/stdlib-toml-test-check.sh

jq '.promotion.next_blocks = ["STD-TOML-TEST-001"]' testing/stdlib-toml-test.json > "$tmp_dir/promotion.json"
expect_failure promotion env TONDO_STDLIB_TOML_TEST_CONTRACT="$tmp_dir/promotion.json" scripts/stdlib-toml-test-check.sh

jq '.model.sources = .model.sources[0:1]' testing/stdlib-toml-test.json > "$tmp_dir/model.json"
expect_failure missing-model env TONDO_STDLIB_TOML_TEST_CONTRACT="$tmp_dir/model.json" scripts/stdlib-toml-test-check.sh

jq '.parent_contract = "testing/other-test.json"' testing/stdlib-toml-test.json > "$tmp_dir/parent.json"
expect_failure parent-link env TONDO_STDLIB_TOML_TEST_CONTRACT="$tmp_dir/parent.json" scripts/stdlib-toml-test-check.sh

scripts/stdlib-toml-test-check.sh >/dev/null

target_dir="${CARGO_TARGET_DIR:-$root/target}"
CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-reliability --test toml_models --locked
CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-stdlib toml::tests --locked
CARGO_TARGET_DIR="$target_dir" cargo check -q --manifest-path fuzz/Cargo.toml --bin stdlib_toml --locked

echo "std.toml tests: OK (negative testing contract; independent model; hosted regressions; fuzz harness)"
