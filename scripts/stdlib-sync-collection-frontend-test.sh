#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-sync-collection-frontend.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.sync collection frontend tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.status = "pending"' testing/stdlib-sync-collection-frontend.json \
    > "$tmp_dir/pending.json"
expect_failure pending-status \
    env TONDO_STDLIB_SYNC_COLLECTION_FRONTEND_CONTRACT="$tmp_dir/pending.json" \
    scripts/stdlib-sync-collection-frontend-check.sh

jq '.surface.qualified_only = false' testing/stdlib-sync-collection-frontend.json \
    > "$tmp_dir/unqualified.json"
expect_failure unqualified-sugar \
    env TONDO_STDLIB_SYNC_COLLECTION_FRONTEND_CONTRACT="$tmp_dir/unqualified.json" \
    scripts/stdlib-sync-collection-frontend-check.sh

jq '.surface.map.malformed = "accepted"' testing/stdlib-sync-collection-frontend.json \
    > "$tmp_dir/map-shape.json"
expect_failure map-shape \
    env TONDO_STDLIB_SYNC_COLLECTION_FRONTEND_CONTRACT="$tmp_dir/map-shape.json" \
    scripts/stdlib-sync-collection-frontend-check.sh

jq '.implementation.tests = .implementation.tests[0:4]' \
    testing/stdlib-sync-collection-frontend.json > "$tmp_dir/tests.json"
expect_failure missing-test \
    env TONDO_STDLIB_SYNC_COLLECTION_FRONTEND_CONTRACT="$tmp_dir/tests.json" \
    scripts/stdlib-sync-collection-frontend-check.sh

jq '.promotion.next_blocks = ["STD-SYNC-COLLECTION-IMPL-001"]' \
    testing/stdlib-sync-collection-frontend.json > "$tmp_dir/promotion.json"
expect_failure promotion-boundary \
    env TONDO_STDLIB_SYNC_COLLECTION_FRONTEND_CONTRACT="$tmp_dir/promotion.json" \
    scripts/stdlib-sync-collection-frontend-check.sh

jq '.negative_cases += ["unexpected-case"]' \
    testing/stdlib-sync-collection-frontend.json > "$tmp_dir/negative-cases.json"
expect_failure negative-case-drift \
    env TONDO_STDLIB_SYNC_COLLECTION_FRONTEND_CONTRACT="$tmp_dir/negative-cases.json" \
    scripts/stdlib-sync-collection-frontend-check.sh

scripts/stdlib-sync-collection-frontend-check.sh >/dev/null
echo "std.sync collection frontend tests: OK (contract negatives and promotion boundary)"
