#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$BASH_SOURCE[0]")/.." && pwd)"
cd "$root"
tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-sync-conformance.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.sync conformance tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.status = "open"' testing/stdlib-sync-conformance.json >"$tmp_dir/open.json"
expect_failure open-status env TONDO_STDLIB_SYNC_CONFORMANCE_CONTRACT="$tmp_dir/open.json" scripts/stdlib-sync-conformance-check.sh

jq '.cases = .cases[0:7]' testing/stdlib-sync-conformance.json >"$tmp_dir/missing-case.json"
expect_failure missing-case env TONDO_STDLIB_SYNC_CONFORMANCE_CONTRACT="$tmp_dir/missing-case.json" scripts/stdlib-sync-conformance-check.sh

jq '.rules.same_case_ids = false' testing/stdlib-sync-conformance.json >"$tmp_dir/corpus-drift.json"
expect_failure corpus-drift env TONDO_STDLIB_SYNC_CONFORMANCE_CONTRACT="$tmp_dir/corpus-drift.json" scripts/stdlib-sync-conformance-check.sh

jq '.vm.expected_stdout[1] = "compare-exchange:drift"' testing/stdlib-sync-conformance.json >"$tmp_dir/vm-drift.json"
expect_failure vm-drift env TONDO_STDLIB_SYNC_CONFORMANCE_CONTRACT="$tmp_dir/vm-drift.json" scripts/stdlib-sync-conformance-check.sh

jq '.cases[2].native_expected.timeout = false' testing/stdlib-sync-conformance.json >"$tmp_dir/native-drift.json"
expect_failure native-drift env TONDO_STDLIB_SYNC_CONFORMANCE_CONTRACT="$tmp_dir/native-drift.json" scripts/stdlib-sync-conformance-check.sh

jq '.cases[0].id = .cases[1].id' testing/stdlib-sync-conformance.json >"$tmp_dir/duplicate-id.json"
expect_failure duplicate-id env TONDO_STDLIB_SYNC_CONFORMANCE_CONTRACT="$tmp_dir/duplicate-id.json" scripts/stdlib-sync-conformance-check.sh

jq '.rules.collection_dependency = "optional"' testing/stdlib-sync-conformance.json >"$tmp_dir/collection-optional.json"
expect_failure collection-optional env TONDO_STDLIB_SYNC_CONFORMANCE_CONTRACT="$tmp_dir/collection-optional.json" scripts/stdlib-sync-conformance-check.sh

jq '.rules.native_scope = "public-lock-abi"' testing/stdlib-sync-conformance.json >"$tmp_dir/native-lock-claim.json"
expect_failure native-lock-claim env TONDO_STDLIB_SYNC_CONFORMANCE_CONTRACT="$tmp_dir/native-lock-claim.json" scripts/stdlib-sync-conformance-check.sh

jq '.native.status = "pending-native-aot"' testing/stdlib-sync-conformance.json >"$tmp_dir/native-pending.json"
expect_failure native-pending env TONDO_STDLIB_SYNC_CONFORMANCE_CONTRACT="$tmp_dir/native-pending.json" scripts/stdlib-sync-conformance-check.sh

jq '.next_blocks = ["STD-SYNC-CONF-002"]' testing/stdlib-sync-conformance.json >"$tmp_dir/stale-next.json"
expect_failure stale-next env TONDO_STDLIB_SYNC_CONFORMANCE_CONTRACT="$tmp_dir/stale-next.json" scripts/stdlib-sync-conformance-check.sh

jq '.rules.native_aot = "verified"' testing/stdlib-sync-conformance.json >"$tmp_dir/aot-claim.json"
expect_failure aot-claim env TONDO_STDLIB_SYNC_CONFORMANCE_CONTRACT="$tmp_dir/aot-claim.json" scripts/stdlib-sync-conformance-check.sh

jq '.negative_cases = .negative_cases[0:15]' testing/stdlib-sync-conformance.json >"$tmp_dir/negative-drift.json"
expect_failure negative-drift env TONDO_STDLIB_SYNC_CONFORMANCE_CONTRACT="$tmp_dir/negative-drift.json" scripts/stdlib-sync-conformance-check.sh

scripts/stdlib-sync-conformance-check.sh >/dev/null

echo "std.sync conformance tests: OK (corpus, observables, capability, dependency and native boundary reject drift)"
