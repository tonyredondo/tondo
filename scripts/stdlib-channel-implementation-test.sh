#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp_root="${TMPDIR:-/tmp}"
tmp_dir="$(mktemp -d "$tmp_root/tondo-stdlib-channel-implementation.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.channel implementation tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.implementation.status = "pending"' testing/stdlib-channel.json \
    >"$tmp_dir/pending.json"
expect_failure pending-implementation \
    env TONDO_STDLIB_CHANNEL_CONTRACT="$tmp_dir/pending.json" \
    scripts/stdlib-channel-implementation-check.sh

jq '.host.cooperative_model = "blocking-worker"' testing/stdlib-channel.json \
    >"$tmp_dir/blocking-host.json"
expect_failure blocking-cooperative-host \
    env TONDO_STDLIB_CHANNEL_CONTRACT="$tmp_dir/blocking-host.json" \
    scripts/stdlib-channel-implementation-check.sh

jq '.implementation.native_aot_lowering = "verified"' testing/stdlib-channel.json \
    >"$tmp_dir/aot-claimed.json"
expect_failure aot-claimed \
    env TONDO_STDLIB_CHANNEL_CONTRACT="$tmp_dir/aot-claimed.json" \
    scripts/stdlib-channel-implementation-check.sh

jq '.implementation.native_probe.cases = 3' testing/stdlib-channel.json \
    >"$tmp_dir/missing-native-case.json"
expect_failure missing-native-case \
    env TONDO_STDLIB_CHANNEL_CONTRACT="$tmp_dir/missing-native-case.json" \
    scripts/stdlib-channel-implementation-check.sh

jq '.implementation.required_follow_ups = .implementation.required_follow_ups[1:]' \
    testing/stdlib-channel.json >"$tmp_dir/missing-follow-up.json"
expect_failure missing-follow-up \
    env TONDO_STDLIB_CHANNEL_CONTRACT="$tmp_dir/missing-follow-up.json" \
    scripts/stdlib-channel-implementation-check.sh

jq '.promotion.next_blocks = ["DIAG-RUNTIME-001"]' testing/stdlib-channel.json \
    >"$tmp_dir/stale-next.json"
expect_failure stale-next \
    env TONDO_STDLIB_CHANNEL_CONTRACT="$tmp_dir/stale-next.json" \
    scripts/stdlib-channel-implementation-check.sh

bash -n scripts/stdlib-channel-implementation-check.sh \
    scripts/stdlib-channel-implementation.sh
scripts/stdlib-channel-check.sh >/dev/null
scripts/stdlib-channel-implementation-check.sh >/dev/null

echo "std.channel implementation tests: OK (status, native boundary, follow-ups and promotion negatives rejected)"
