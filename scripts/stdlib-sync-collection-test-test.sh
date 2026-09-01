#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-sync-collection-test.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.sync collection tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.status = "contract-locked"' testing/stdlib-sync-collection-test.json \
    > "$tmp_dir/status.json"
expect_failure status env \
    TONDO_STDLIB_SYNC_COLLECTION_TEST_CONTRACT="$tmp_dir/status.json" \
    scripts/stdlib-sync-collection-test-check.sh

jq '.limits.max_history_operations = 13' testing/stdlib-sync-collection-test.json \
    > "$tmp_dir/history-limit.json"
expect_failure history-limit env \
    TONDO_STDLIB_SYNC_COLLECTION_TEST_CONTRACT="$tmp_dir/history-limit.json" \
    scripts/stdlib-sync-collection-test-check.sh

jq '.fuzz.step_limit = 1024' testing/stdlib-sync-collection-test.json \
    > "$tmp_dir/fuzz-limit.json"
expect_failure fuzz-limit env \
    TONDO_STDLIB_SYNC_COLLECTION_TEST_CONTRACT="$tmp_dir/fuzz-limit.json" \
    scripts/stdlib-sync-collection-test-check.sh

jq '.sanitization.native_aot = "verified"' testing/stdlib-sync-collection-test.json \
    > "$tmp_dir/aot.json"
expect_failure aot-claim env \
    TONDO_STDLIB_SYNC_COLLECTION_TEST_CONTRACT="$tmp_dir/aot.json" \
    scripts/stdlib-sync-collection-test-check.sh

jq '.promotion.next_blocks = ["STD-SYNC-COLLECTION-TEST-001"]' \
    testing/stdlib-sync-collection-test.json > "$tmp_dir/promotion.json"
expect_failure promotion-boundary env \
    TONDO_STDLIB_SYNC_COLLECTION_TEST_CONTRACT="$tmp_dir/promotion.json" \
    scripts/stdlib-sync-collection-test-check.sh

jq '.model.owners += ["Other"]' \
    testing/stdlib-sync-collection-test.json > "$tmp_dir/unknown-owner.json"
expect_failure unknown-owner env \
    TONDO_STDLIB_SYNC_COLLECTION_TEST_CONTRACT="$tmp_dir/unknown-owner.json" \
    scripts/stdlib-sync-collection-test-check.sh

scripts/stdlib-sync-collection-test-check.sh >/dev/null
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$root/target}" \
    cargo test -p tondo-reliability --test sync_collection_models --locked >/dev/null
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$root/target}" \
    cargo test -p tondo-native-runtime native_sync_ --locked >/dev/null
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$root/target}" \
    cargo test -p tondo-compiler process_host::tests::sync_ --locked >/dev/null
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$root/target}" \
    cargo check --manifest-path fuzz/Cargo.toml --bin stdlib_sync_collections --locked >/dev/null

echo "std.sync collection tests: OK (negative contract cases; independent models; hosted/native regressions; fuzz harness)"
