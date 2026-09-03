#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp_root="${TMPDIR:-/tmp}"
tmp_dir="$(mktemp -d "$tmp_root/tondo-stdlib-executor-implementation.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.executor implementation tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.implementation.observed.status = "partial-hosted-cooperative-pool"' testing/stdlib-executor.json \
    >"$tmp_dir/stale-partial.json"
expect_failure stale-partial-status \
    env TONDO_STDLIB_EXECUTOR_CONTRACT="$tmp_dir/stale-partial.json" \
    scripts/stdlib-executor-implementation-check.sh

jq '.implementation.observed.native_aot_lowering = "verified"' testing/stdlib-executor.json \
    >"$tmp_dir/aot-claimed.json"
expect_failure aot-claimed \
    env TONDO_STDLIB_EXECUTOR_CONTRACT="$tmp_dir/aot-claimed.json" \
    scripts/stdlib-executor-implementation-check.sh

jq '.implementation.observed.selectable_actor_send = "not-claimed"' testing/stdlib-executor.json \
    >"$tmp_dir/selectable-not-claimed.json"
expect_failure selectable-not-claimed \
    env TONDO_STDLIB_EXECUTOR_CONTRACT="$tmp_dir/selectable-not-claimed.json" \
    scripts/stdlib-executor-implementation-check.sh

jq '.implementation.observed.resolved_decision = ""' testing/stdlib-executor.json \
    >"$tmp_dir/missing-decision.json"
expect_failure missing-decision \
    env TONDO_STDLIB_EXECUTOR_CONTRACT="$tmp_dir/missing-decision.json" \
    scripts/stdlib-executor-implementation-check.sh

jq '.implementation.observed.fixture.stdout = "wrong"' testing/stdlib-executor.json \
    >"$tmp_dir/stale-fixture.json"
expect_failure stale-fixture \
    env TONDO_STDLIB_EXECUTOR_CONTRACT="$tmp_dir/stale-fixture.json" \
    scripts/stdlib-executor-implementation-check.sh

jq '.implementation.observed.remaining = ["STD-EXEC-CONF-001"]' testing/stdlib-executor.json \
    >"$tmp_dir/missing-followups.json"
expect_failure missing-followups \
    env TONDO_STDLIB_EXECUTOR_CONTRACT="$tmp_dir/missing-followups.json" \
    scripts/stdlib-executor-implementation-check.sh

bash -n \
    scripts/stdlib-executor-implementation-check.sh \
    scripts/stdlib-executor-implementation-test.sh \
    scripts/stdlib-executor-implementation.sh
scripts/stdlib-executor-check.sh >/dev/null
scripts/stdlib-executor-implementation-check.sh >/dev/null

echo "std.executor implementation tests: OK (stale partial status, AOT boundary, decision and fixture negatives rejected)"
