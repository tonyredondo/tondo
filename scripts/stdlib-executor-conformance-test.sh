#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-executor-conformance.XXXXXX")"
trap 'rm -rf -- "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.executor conformance tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.status = "open"' testing/stdlib-executor-conformance.json >"$tmp_dir/open.json"
expect_failure open-status env TONDO_STDLIB_EXECUTOR_CONFORMANCE_CONTRACT="$tmp_dir/open.json" \
    scripts/stdlib-executor-conformance-check.sh

jq '.cases = .cases[0:7]' testing/stdlib-executor-conformance.json >"$tmp_dir/missing-case.json"
expect_failure missing-case env TONDO_STDLIB_EXECUTOR_CONFORMANCE_CONTRACT="$tmp_dir/missing-case.json" \
    scripts/stdlib-executor-conformance-check.sh

jq '.rules.same_case_ids = false' testing/stdlib-executor-conformance.json >"$tmp_dir/corpus-drift.json"
expect_failure corpus-drift env TONDO_STDLIB_EXECUTOR_CONFORMANCE_CONTRACT="$tmp_dir/corpus-drift.json" \
    scripts/stdlib-executor-conformance-check.sh

jq '.cases[0].id = .cases[1].id' testing/stdlib-executor-conformance.json >"$tmp_dir/duplicate-id.json"
expect_failure duplicate-id env TONDO_STDLIB_EXECUTOR_CONFORMANCE_CONTRACT="$tmp_dir/duplicate-id.json" \
    scripts/stdlib-executor-conformance-check.sh

jq '.native.status = "pending-native-aot"' testing/stdlib-executor-conformance.json >"$tmp_dir/native-pending.json"
expect_failure native-pending env TONDO_STDLIB_EXECUTOR_CONFORMANCE_CONTRACT="$tmp_dir/native-pending.json" \
    scripts/stdlib-executor-conformance-check.sh

jq '.static_capability.codes = ["E1007"]' testing/stdlib-executor-conformance.json >"$tmp_dir/capability-drift.json"
expect_failure capability-drift env TONDO_STDLIB_EXECUTOR_CONFORMANCE_CONTRACT="$tmp_dir/capability-drift.json" \
    scripts/stdlib-executor-conformance-check.sh

jq '.cases[3].native_expected.force_kill = true' testing/stdlib-executor-conformance.json >"$tmp_dir/force-kill-claim.json"
expect_failure force-kill-claim env TONDO_STDLIB_EXECUTOR_CONFORMANCE_CONTRACT="$tmp_dir/force-kill-claim.json" \
    scripts/stdlib-executor-conformance-check.sh

jq '.rules.capability = "ambient-lookup"' testing/stdlib-executor-conformance.json >"$tmp_dir/ambient-capability.json"
expect_failure ambient-capability env TONDO_STDLIB_EXECUTOR_CONFORMANCE_CONTRACT="$tmp_dir/ambient-capability.json" \
    scripts/stdlib-executor-conformance-check.sh

jq '.cases[2].native_expected.result_payload = 99' testing/stdlib-executor-conformance.json >"$tmp_dir/payload-drift.json"
expect_failure payload-drift env TONDO_STDLIB_EXECUTOR_CONFORMANCE_CONTRACT="$tmp_dir/payload-drift.json" \
    scripts/stdlib-executor-conformance-check.sh

jq '.cases[7].native_expected.native_aot = "verified"' testing/stdlib-executor-conformance.json >"$tmp_dir/aot-claim.json"
expect_failure aot-claim env TONDO_STDLIB_EXECUTOR_CONFORMANCE_CONTRACT="$tmp_dir/aot-claim.json" \
    scripts/stdlib-executor-conformance-check.sh

jq '.next_blocks = ["STD-EXEC-DOC-001"]' testing/stdlib-executor-conformance.json >"$tmp_dir/stale-next.json"
expect_failure stale-next env TONDO_STDLIB_EXECUTOR_CONFORMANCE_CONTRACT="$tmp_dir/stale-next.json" \
    scripts/stdlib-executor-conformance-check.sh

jq '.report = "/tmp/stdlib-executor-conformance.json"' testing/stdlib-executor-conformance.json \
    >"$tmp_dir/physical-report.json"
expect_failure physical-report env TONDO_STDLIB_EXECUTOR_CONFORMANCE_CONTRACT="$tmp_dir/physical-report.json" \
    scripts/stdlib-executor-conformance-check.sh

echo "std.executor conformance tests: OK (status, corpus, capability, lifecycle, cleanup and AOT boundary reject drift)"
