#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$BASH_SOURCE")/.." && pwd)"
cd "$root"
tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-channel-conformance.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.channel conformance tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.status = "open"' testing/stdlib-channel-conformance.json >"$tmp_dir/open.json"
expect_failure open-status env TONDO_STDLIB_CHANNEL_CONFORMANCE_CONTRACT="$tmp_dir/open.json" \
    scripts/stdlib-channel-conformance-check.sh

jq '.cases = .cases[0:7]' testing/stdlib-channel-conformance.json >"$tmp_dir/missing-case.json"
expect_failure missing-case env TONDO_STDLIB_CHANNEL_CONFORMANCE_CONTRACT="$tmp_dir/missing-case.json" \
    scripts/stdlib-channel-conformance-check.sh

jq '.rules.same_case_ids = false' testing/stdlib-channel-conformance.json >"$tmp_dir/corpus-drift.json"
expect_failure corpus-drift env TONDO_STDLIB_CHANNEL_CONFORMANCE_CONTRACT="$tmp_dir/corpus-drift.json" \
    scripts/stdlib-channel-conformance-check.sh

jq '.cases[0].id = .cases[1].id' testing/stdlib-channel-conformance.json >"$tmp_dir/duplicate-id.json"
expect_failure duplicate-id env TONDO_STDLIB_CHANNEL_CONFORMANCE_CONTRACT="$tmp_dir/duplicate-id.json" \
    scripts/stdlib-channel-conformance-check.sh

jq '.native.status = "pending-native-aot"' testing/stdlib-channel-conformance.json >"$tmp_dir/native-pending.json"
expect_failure native-pending env TONDO_STDLIB_CHANNEL_CONFORMANCE_CONTRACT="$tmp_dir/native-pending.json" \
    scripts/stdlib-channel-conformance-check.sh

jq '.cases[3].native_expected.status_code = 99' testing/stdlib-channel-conformance.json >"$tmp_dir/error-drift.json"
expect_failure error-drift env TONDO_STDLIB_CHANNEL_CONFORMANCE_CONTRACT="$tmp_dir/error-drift.json" \
    scripts/stdlib-channel-conformance-check.sh

jq '.vm.panic_expected_exit = 0' testing/stdlib-channel-conformance.json >"$tmp_dir/panic-exit.json"
expect_failure panic-exit env TONDO_STDLIB_CHANNEL_CONFORMANCE_CONTRACT="$tmp_dir/panic-exit.json" \
    scripts/stdlib-channel-conformance-check.sh

jq '.rules.native_aot = "verified"' testing/stdlib-channel-conformance.json >"$tmp_dir/aot-claim.json"
expect_failure aot-claim env TONDO_STDLIB_CHANNEL_CONFORMANCE_CONTRACT="$tmp_dir/aot-claim.json" \
    scripts/stdlib-channel-conformance-check.sh

jq '.next_blocks = ["STD-EXEC-IMPL-001"]' testing/stdlib-channel-conformance.json >"$tmp_dir/stale-next.json"
expect_failure stale-next env TONDO_STDLIB_CHANNEL_CONFORMANCE_CONTRACT="$tmp_dir/stale-next.json" \
    scripts/stdlib-channel-conformance-check.sh

jq '.report = "/tmp/stdlib-channel-conformance.json"' testing/stdlib-channel-conformance.json \
    >"$tmp_dir/physical-report.json"
expect_failure physical-report env TONDO_STDLIB_CHANNEL_CONFORMANCE_CONTRACT="$tmp_dir/physical-report.json" \
    scripts/stdlib-channel-conformance-check.sh

echo "std.channel conformance tests: OK (status, corpus, errors, panic cleanup and AOT boundary reject drift)"
