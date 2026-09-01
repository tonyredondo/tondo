#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$BASH_SOURCE")/.." && pwd)"
cd "$root"
tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-sync-collection-conformance.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.sync collection conformance tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.status = "open"' testing/stdlib-sync-collection-conformance.json >"$tmp_dir/open.json"
expect_failure open-status env TONDO_STDLIB_SYNC_COLLECTION_CONFORMANCE_CONTRACT="$tmp_dir/open.json" \
    scripts/stdlib-sync-collection-conformance-check.sh

jq '.cases = .cases[0:7]' testing/stdlib-sync-collection-conformance.json >"$tmp_dir/missing-case.json"
expect_failure missing-case env TONDO_STDLIB_SYNC_COLLECTION_CONFORMANCE_CONTRACT="$tmp_dir/missing-case.json" \
    scripts/stdlib-sync-collection-conformance-check.sh

jq '.rules.same_case_ids = false' testing/stdlib-sync-collection-conformance.json >"$tmp_dir/corpus-drift.json"
expect_failure corpus-drift env TONDO_STDLIB_SYNC_COLLECTION_CONFORMANCE_CONTRACT="$tmp_dir/corpus-drift.json" \
    scripts/stdlib-sync-collection-conformance-check.sh

jq '.cases[4].vm_observable = "cursor-horizon:drift"' testing/stdlib-sync-collection-conformance.json >"$tmp_dir/observable-drift.json"
expect_failure observable-drift env TONDO_STDLIB_SYNC_COLLECTION_CONFORMANCE_CONTRACT="$tmp_dir/observable-drift.json" \
    scripts/stdlib-sync-collection-conformance-check.sh

jq '.cases[7].native_expected.cleanup = false' testing/stdlib-sync-collection-conformance.json >"$tmp_dir/cleanup-drift.json"
expect_failure cleanup-drift env TONDO_STDLIB_SYNC_COLLECTION_CONFORMANCE_CONTRACT="$tmp_dir/cleanup-drift.json" \
    scripts/stdlib-sync-collection-conformance-check.sh

jq '.cases[0].id = .cases[1].id' testing/stdlib-sync-collection-conformance.json >"$tmp_dir/duplicate-id.json"
expect_failure duplicate-id env TONDO_STDLIB_SYNC_COLLECTION_CONFORMANCE_CONTRACT="$tmp_dir/duplicate-id.json" \
    scripts/stdlib-sync-collection-conformance-check.sh

jq '.native.status = "pending-native-aot"' testing/stdlib-sync-collection-conformance.json >"$tmp_dir/native-pending.json"
expect_failure native-pending env TONDO_STDLIB_SYNC_COLLECTION_CONFORMANCE_CONTRACT="$tmp_dir/native-pending.json" \
    scripts/stdlib-sync-collection-conformance-check.sh

jq '.next_blocks = ["STD-SYNC-COLLECTION-DOC-001"]' testing/stdlib-sync-collection-conformance.json >"$tmp_dir/stale-next.json"
expect_failure stale-next env TONDO_STDLIB_SYNC_COLLECTION_CONFORMANCE_CONTRACT="$tmp_dir/stale-next.json" \
    scripts/stdlib-sync-collection-conformance-check.sh

jq '.rules.native_aot = "verified"' testing/stdlib-sync-collection-conformance.json >"$tmp_dir/aot-claim.json"
expect_failure aot-claim env TONDO_STDLIB_SYNC_COLLECTION_CONFORMANCE_CONTRACT="$tmp_dir/aot-claim.json" \
    scripts/stdlib-sync-collection-conformance-check.sh

scripts/stdlib-sync-collection-conformance-check.sh >/dev/null

echo "std.sync collection conformance tests: OK (corpus, observables, cleanup and AOT boundary reject drift)"
