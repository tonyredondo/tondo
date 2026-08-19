#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_root="${TMPDIR:-/tmp}"
tmp_dir="$(mktemp -d "$tmp_root/tondo-stdlib-test-coordination-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "stdlib test coordination: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.owners = .owners[1:]' testing/stdlib-test-coordination.json \
    > "$tmp_dir/missing-owner.json"
expect_failure missing-owner env TONDO_STDLIB_TEST_COORDINATION="$tmp_dir/missing-owner.json" \
    scripts/stdlib-test-coordination-check.sh

jq '.owners[0].public_api[0].id = "signature:invalid"' testing/stdlib-test-coordination.json \
    > "$tmp_dir/missing-signature.json"
expect_failure missing-signature env TONDO_STDLIB_TEST_COORDINATION="$tmp_dir/missing-signature.json" \
    scripts/stdlib-test-coordination-check.sh

jq '.owners[0].model.laws = []' testing/stdlib-test-coordination.json \
    > "$tmp_dir/missing-model-law.json"
expect_failure missing-model-law env TONDO_STDLIB_TEST_COORDINATION="$tmp_dir/missing-model-law.json" \
    scripts/stdlib-test-coordination-check.sh

jq '.owners[0].fuzz.reason = null' testing/stdlib-test-coordination.json \
    > "$tmp_dir/missing-fuzz-reason.json"
expect_failure missing-fuzz-reason env TONDO_STDLIB_TEST_COORDINATION="$tmp_dir/missing-fuzz-reason.json" \
    scripts/stdlib-test-coordination-check.sh

jq '.next_coordination = "STD-TEST-001"' testing/stdlib-test-coordination.json \
    > "$tmp_dir/stale-coordination.json"
expect_failure stale-coordination env TONDO_STDLIB_TEST_COORDINATION="$tmp_dir/stale-coordination.json" \
    scripts/stdlib-test-coordination-check.sh

for owner in \
    std.meta std.reflect std.bytes std.time std.env std.core std.text \
    std.collections std.iter std.math std.format std.io std.console std.path \
    std.fs std.process std.serialization std.json std.messagepack std.protobuf \
    std.testing std.async; do
    jq -e --arg owner "$owner" 'any(.owners[]; .id == $owner and (.model.laws | length) >= 3 and (.test.commands | length) > 0 and (.fuzz.campaigns | length) > 0)' \
        testing/stdlib-test-coordination.json >/dev/null
done

jq -e '
  .summary == {
    owners: 22,
    public_signatures: 214,
    owner_requirements: 171,
    model_laws: 66,
    fuzz_verified: 1,
    fuzz_partial: 21
  }
' testing/stdlib-test-coordination.json >/dev/null

echo "stdlib test coordination tests: OK"
