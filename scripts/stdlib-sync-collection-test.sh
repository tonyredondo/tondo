#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-sync-collection.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.sync collection implementation tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.status = "open"' testing/stdlib-sync-collection.json > "$tmp_dir/open.json"
expect_failure open-status \
    env TONDO_STDLIB_SYNC_COLLECTION_CONTRACT="$tmp_dir/open.json" \
    scripts/stdlib-sync-collection-check.sh

jq '.runtime.native_aot_lowering = "verified"' testing/stdlib-sync-collection.json \
    > "$tmp_dir/aot.json"
expect_failure aot-claim \
    env TONDO_STDLIB_SYNC_COLLECTION_CONTRACT="$tmp_dir/aot.json" \
    scripts/stdlib-sync-collection-check.sh

jq '.runtime.public_api_promoted = true' testing/stdlib-sync-collection.json \
    > "$tmp_dir/public.json"
expect_failure public-promotion \
    env TONDO_STDLIB_SYNC_COLLECTION_CONTRACT="$tmp_dir/public.json" \
    scripts/stdlib-sync-collection-check.sh

jq '.surface.queue.ordering = "LIFO"' testing/stdlib-sync-collection.json \
    > "$tmp_dir/queue-order.json"
expect_failure queue-order \
    env TONDO_STDLIB_SYNC_COLLECTION_CONTRACT="$tmp_dir/queue-order.json" \
    scripts/stdlib-sync-collection-check.sh

jq '.promotion.next_blocks = ["STD-SYNC-COLLECTION-TEST-001"]' \
    testing/stdlib-sync-collection.json > "$tmp_dir/next.json"
expect_failure promotion-boundary \
    env TONDO_STDLIB_SYNC_COLLECTION_CONTRACT="$tmp_dir/next.json" \
    scripts/stdlib-sync-collection-check.sh

jq '.negative_cases += ["wrong-nominal-host-kind"]' testing/stdlib-sync-collection.json \
    > "$tmp_dir/duplicate-negative.json"
expect_failure duplicate-negative \
    env TONDO_STDLIB_SYNC_COLLECTION_CONTRACT="$tmp_dir/duplicate-negative.json" \
    scripts/stdlib-sync-collection-check.sh

scripts/stdlib-sync-collection-check.sh >/dev/null

CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$root/target}" \
    cargo test -p tondo-compiler sync_collection --locked >/dev/null
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$root/target}" \
    cargo test -p tondo-compiler process_host::tests::sync_host_covers_forged_tokens_contended_paths_and_cleanup --locked >/dev/null
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$root/target}" \
    cargo test -p tondo-native-runtime native_sync_ --locked >/dev/null

echo "std.sync collection implementation tests: OK (negative contract cases; hosted VM; native ABI)"
