#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$BASH_SOURCE")/.." && pwd)"
cd "$root"
tmp_root="$(printenv TMPDIR || printf /tmp)"
tmp_dir="$(mktemp -d "$tmp_root/tondo-stdlib-async-group-conformance.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.async.Group conformance tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.status = "open"' testing/stdlib-async-group-conformance.json >"$tmp_dir/open.json"
expect_failure open env TONDO_STDLIB_ASYNC_GROUP_CONFORMANCE_CONTRACT="$tmp_dir/open.json" \
    scripts/stdlib-async-group-conformance-check.sh

jq '.cases = .cases[0:7]' testing/stdlib-async-group-conformance.json >"$tmp_dir/missing-case.json"
expect_failure missing-case env TONDO_STDLIB_ASYNC_GROUP_CONFORMANCE_CONTRACT="$tmp_dir/missing-case.json" \
    scripts/stdlib-async-group-conformance-check.sh

jq '.rules.same_case_ids = false' testing/stdlib-async-group-conformance.json >"$tmp_dir/corpus-drift.json"
expect_failure corpus-drift env TONDO_STDLIB_ASYNC_GROUP_CONFORMANCE_CONTRACT="$tmp_dir/corpus-drift.json" \
    scripts/stdlib-async-group-conformance-check.sh

jq '.native.status = "pending-native-async-runtime"' testing/stdlib-async-group-conformance.json >"$tmp_dir/native-pending.json"
expect_failure native-pending env TONDO_STDLIB_ASYNC_GROUP_CONFORMANCE_CONTRACT="$tmp_dir/native-pending.json" \
    scripts/stdlib-async-group-conformance-check.sh

jq '.cases[2].native_expected.error_payload = 99' testing/stdlib-async-group-conformance.json >"$tmp_dir/error-drift.json"
expect_failure error-drift env TONDO_STDLIB_ASYNC_GROUP_CONFORMANCE_CONTRACT="$tmp_dir/error-drift.json" \
    scripts/stdlib-async-group-conformance-check.sh

echo "std.async.Group conformance tests: OK (status, corpus, native boundary and oracle drift rejected)"
