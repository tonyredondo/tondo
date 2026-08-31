#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-sync-collection-iter.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.sync collection iteration tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.status = "open"' testing/stdlib-sync-collection-iter.json > "$tmp_dir/open.json"
expect_failure open-status \
    env TONDO_STDLIB_SYNC_COLLECTION_ITER_CONTRACT="$tmp_dir/open.json" \
    scripts/stdlib-sync-collection-iter-check.sh

jq '.runtime.native_aot_lowering = "verified"' testing/stdlib-sync-collection-iter.json \
    > "$tmp_dir/aot.json"
expect_failure aot-claim \
    env TONDO_STDLIB_SYNC_COLLECTION_ITER_CONTRACT="$tmp_dir/aot.json" \
    scripts/stdlib-sync-collection-iter-check.sh

jq '.semantics.horizon_capture = "copies-content"' testing/stdlib-sync-collection-iter.json \
    > "$tmp_dir/materialized.json"
expect_failure materialized-cursor \
    env TONDO_STDLIB_SYNC_COLLECTION_ITER_CONTRACT="$tmp_dir/materialized.json" \
    scripts/stdlib-sync-collection-iter-check.sh

jq '.ownership.loaned_bindings = "allowed"' testing/stdlib-sync-collection-iter.json \
    > "$tmp_dir/loaned.json"
expect_failure loaned-binding \
    env TONDO_STDLIB_SYNC_COLLECTION_ITER_CONTRACT="$tmp_dir/loaned.json" \
    scripts/stdlib-sync-collection-iter-check.sh

jq '.promotion.next_blocks = ["STD-SYNC-COLLECTION-ITER-001"]' \
    testing/stdlib-sync-collection-iter.json > "$tmp_dir/promotion.json"
expect_failure promotion-boundary \
    env TONDO_STDLIB_SYNC_COLLECTION_ITER_CONTRACT="$tmp_dir/promotion.json" \
    scripts/stdlib-sync-collection-iter-check.sh

jq '.negative_cases += ["ref-binding-rejected"]' \
    testing/stdlib-sync-collection-iter.json > "$tmp_dir/duplicate-negative.json"
expect_failure duplicate-negative \
    env TONDO_STDLIB_SYNC_COLLECTION_ITER_CONTRACT="$tmp_dir/duplicate-negative.json" \
    scripts/stdlib-sync-collection-iter-check.sh

scripts/stdlib-sync-collection-iter-check.sh >/dev/null

target_dir="${CARGO_TARGET_DIR:-$root/target}"
CARGO_TARGET_DIR="$target_dir" \
    cargo test -p tondo-compiler \
        hir::check::tests::sync_collection_direct_iteration_is_value_only_and_suspendable \
        --locked -- --exact >/dev/null
CARGO_TARGET_DIR="$target_dir" \
    cargo test -p tondo-compiler \
        driver::tests::sync_collection_direct_for_uses_finite_host_cursor_order \
        --locked -- --exact >/dev/null
CARGO_TARGET_DIR="$target_dir" \
    cargo test -p tondo-compiler \
        process_host::tests::sync_collection_cursor_preserves_order_horizon_and_reinsertion_boundary \
        --locked -- --exact >/dev/null
CARGO_TARGET_DIR="$target_dir" \
    cargo test -p tondo-native-runtime \
        tests::native_sync_cursor_is_finite_ordered_and_generation_safe \
        --locked -- --exact >/dev/null

echo "std.sync collection iteration tests: OK (negative contract cases; hosted VM; native private ABI)"
