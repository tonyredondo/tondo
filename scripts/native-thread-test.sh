#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp_root="$root/.tmp"
mkdir -p "$tmp_root"
tmp="$(mktemp -d "$tmp_root/tondo-native-thread-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

expect_failure() {
    local name="$1"
    local candidate="$2"
    if TONDO_NATIVE_THREAD_CONTRACT="$candidate" \
        scripts/native-thread-check.sh >/dev/null 2>&1; then
        echo "native thread tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.status = "open"' testing/native-thread.json > "$tmp/open.json"
expect_failure status "$tmp/open.json"

jq '.lane.native = "cooperative-only"' testing/native-thread.json > "$tmp/no-worker.json"
expect_failure physical-worker "$tmp/no-worker.json"

jq '.worker.distinct_thread = "not-required"' testing/native-thread.json > "$tmp/not-distinct.json"
expect_failure distinct-worker "$tmp/not-distinct.json"

jq '.implementation.report_field = "native_runtime_runs"' testing/native-thread.json > "$tmp/no-report.json"
expect_failure report-field "$tmp/no-report.json"

jq '.next_blocks = ["NATIVE-THREAD-001"]' testing/native-thread.json > "$tmp/stale-next.json"
expect_failure stale-next "$tmp/stale-next.json"

jq '.lane.adapter_body_boundary = "deferred-body-executed"' testing/native-thread.json > "$tmp/overclaim.json"
expect_failure eager-body-overclaim "$tmp/overclaim.json"

echo "native thread tests: OK (contract rejects missing worker, identity, evidence and boundary guarantees)"
