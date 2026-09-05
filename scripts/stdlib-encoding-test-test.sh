#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-encoding-test.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.encoding tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.status = "contract-locked"' testing/stdlib-encoding-test.json \
    >"$tmp_dir/status.json"
expect_failure status \
    env TONDO_STDLIB_ENCODING_TEST_CONTRACT="$tmp_dir/status.json" \
    scripts/stdlib-encoding-test-check.sh

jq '.limits.max_fuzz_steps = 1024' testing/stdlib-encoding-test.json \
    >"$tmp_dir/step-limit.json"
expect_failure step-limit \
    env TONDO_STDLIB_ENCODING_TEST_CONTRACT="$tmp_dir/step-limit.json" \
    scripts/stdlib-encoding-test-check.sh

jq '.fuzz.smoke.result = "failed"' testing/stdlib-encoding-test.json \
    >"$tmp_dir/fuzz-result.json"
expect_failure fuzz-result \
    env TONDO_STDLIB_ENCODING_TEST_CONTRACT="$tmp_dir/fuzz-result.json" \
    scripts/stdlib-encoding-test-check.sh

jq '.sanitization.native_aot = "verified"' testing/stdlib-encoding-test.json \
    >"$tmp_dir/aot.json"
expect_failure aot-claim \
    env TONDO_STDLIB_ENCODING_TEST_CONTRACT="$tmp_dir/aot.json" \
    scripts/stdlib-encoding-test-check.sh

jq '.promotion.next_blocks = ["STD-ENCODING-TEST-001"]' testing/stdlib-encoding-test.json \
    >"$tmp_dir/promotion.json"
expect_failure promotion \
    env TONDO_STDLIB_ENCODING_TEST_CONTRACT="$tmp_dir/promotion.json" \
    scripts/stdlib-encoding-test-check.sh

jq '.model.sources = .model.sources[0:1]' testing/stdlib-encoding-test.json \
    >"$tmp_dir/missing-model.json"
expect_failure missing-model \
    env TONDO_STDLIB_ENCODING_TEST_CONTRACT="$tmp_dir/missing-model.json" \
    scripts/stdlib-encoding-test-check.sh

bash -n scripts/stdlib-encoding-test-check.sh \
    scripts/stdlib-encoding-test-test.sh \
    scripts/stdlib-encoding-fuzz.sh
scripts/stdlib-encoding-test-check.sh >/dev/null

target_dir="${CARGO_TARGET_DIR:-$root/target}"
CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-reliability \
    --test encoding_models --locked
CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-stdlib \
    encoding:: --locked
CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-compiler \
    encoding_host_ --locked
CARGO_TARGET_DIR="$target_dir" cargo check -q \
    --manifest-path fuzz/Cargo.toml --bin stdlib_encoding --locked

echo "std.encoding tests: OK (negative contract cases; independent model; hosted regressions; fuzz harness)"
