#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-channel-test.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.channel tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.status = "contract-locked"' testing/stdlib-channel-test.json \
    >"$tmp_dir/status.json"
expect_failure status \
    env TONDO_STDLIB_CHANNEL_TEST_CONTRACT="$tmp_dir/status.json" \
    scripts/stdlib-channel-test-check.sh

jq '.limits.max_fuzz_steps = 1024' testing/stdlib-channel-test.json \
    >"$tmp_dir/fuzz-limit.json"
expect_failure fuzz-limit \
    env TONDO_STDLIB_CHANNEL_TEST_CONTRACT="$tmp_dir/fuzz-limit.json" \
    scripts/stdlib-channel-test-check.sh

jq '.model.laws = .model.laws[:4]' testing/stdlib-channel-test.json \
    >"$tmp_dir/laws.json"
expect_failure laws \
    env TONDO_STDLIB_CHANNEL_TEST_CONTRACT="$tmp_dir/laws.json" \
    scripts/stdlib-channel-test-check.sh

jq '.fuzz.smoke.result = "failed"' testing/stdlib-channel-test.json \
    >"$tmp_dir/fuzz-result.json"
expect_failure fuzz-result \
    env TONDO_STDLIB_CHANNEL_TEST_CONTRACT="$tmp_dir/fuzz-result.json" \
    scripts/stdlib-channel-test-check.sh

jq '.sanitization.native_aot = "verified"' testing/stdlib-channel-test.json \
    >"$tmp_dir/aot.json"
expect_failure aot-claim \
    env TONDO_STDLIB_CHANNEL_TEST_CONTRACT="$tmp_dir/aot.json" \
    scripts/stdlib-channel-test-check.sh

jq '.promotion.next_blocks = ["STD-CHANNEL-TEST-001"]' \
    testing/stdlib-channel-test.json >"$tmp_dir/promotion.json"
expect_failure promotion \
    env TONDO_STDLIB_CHANNEL_TEST_CONTRACT="$tmp_dir/promotion.json" \
    scripts/stdlib-channel-test-check.sh

jq '.fuzz.source = "fuzz/fuzz_targets/missing.rs"' \
    testing/stdlib-channel-test.json >"$tmp_dir/missing-source.json"
expect_failure missing-source \
    env TONDO_STDLIB_CHANNEL_TEST_CONTRACT="$tmp_dir/missing-source.json" \
    scripts/stdlib-channel-test-check.sh

bash -n scripts/stdlib-channel-test-check.sh \
    scripts/stdlib-channel-test-test.sh \
    scripts/stdlib-channel-fuzz.sh
scripts/stdlib-channel-test-check.sh >/dev/null
scripts/stdlib-channel-check.sh >/dev/null
scripts/stdlib-channel-implementation-check.sh >/dev/null
scripts/stdlib-channel-async-iter-check.sh >/dev/null

target_dir="${CARGO_TARGET_DIR:-$root/target}"
CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-reliability \
    --lib channel_model --locked
CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-reliability \
    --test channel_models --locked
CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-compiler \
    channel_host_ --locked
CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-native-runtime \
    native_channel_ --locked
CARGO_TARGET_DIR="$target_dir" cargo check -q \
    --manifest-path fuzz/Cargo.toml --bin stdlib_channel --locked

echo "std.channel tests: OK (negative contract cases; model; hosted/native regressions; fuzz harness)"
