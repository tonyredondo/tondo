#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-hosted-implementation-coordination-test.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT

scripts/stdlib-hosted-implementation-coordination-check.sh

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "stdlib hosted implementation coordination tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq 'del(.owners[0])' testing/stdlib-hosted-implementation-coordination.json > "$tmp/missing-owner.json"
expect_failure missing-owner env \
    TONDO_STDLIB_HOSTED_IMPLEMENTATION_COORDINATION="$tmp/missing-owner.json" \
    scripts/stdlib-hosted-implementation-coordination-check.sh

jq '.owners[0].capability = ["filesystem"]' \
    testing/stdlib-hosted-implementation-coordination.json > "$tmp/wrong-capability.json"
expect_failure wrong-capability env \
    TONDO_STDLIB_HOSTED_IMPLEMENTATION_COORDINATION="$tmp/wrong-capability.json" \
    scripts/stdlib-hosted-implementation-coordination-check.sh

jq '.owners[2].host.status = "not-applicable"' \
    testing/stdlib-hosted-implementation-coordination.json > "$tmp/missing-host.json"
expect_failure missing-host env \
    TONDO_STDLIB_HOSTED_IMPLEMENTATION_COORDINATION="$tmp/missing-host.json" \
    scripts/stdlib-hosted-implementation-coordination-check.sh

jq '.owners[0].public_api.signatures[0].status = "open-gaps"' \
    testing/stdlib-hosted-implementation-coordination.json > "$tmp/signature-gap.json"
expect_failure signature-gap env \
    TONDO_STDLIB_HOSTED_IMPLEMENTATION_COORDINATION="$tmp/signature-gap.json" \
    scripts/stdlib-hosted-implementation-coordination-check.sh

for mutation in \
    '.rows |= map(select(.owner != "std.console"))' \
    '(.owners[] | select(.id == "std.console") | .owner_missing) = ["implementation-route-missing"]' \
    '.owners |= map(select(.id != "std.console"))'; do
    jq "$mutation" testing/stdlib-public-api.json > "$tmp/api-gap.json"
    TONDO_STDLIB_PUBLIC_API="$tmp/api-gap.json" \
        scripts/stdlib-hosted-implementation-coordination-generate.sh "$tmp/from-gap.json"
    jq -e '.status == "open-gaps" and (.owners | length) == 4
        and (.owners[] | select(.id == "std.console") | .public_api.status == "open-gaps")' \
        "$tmp/from-gap.json" >/dev/null
    TONDO_STDLIB_PUBLIC_API="$tmp/api-gap.json" \
        TONDO_STDLIB_HOSTED_IMPLEMENTATION_COORDINATION="$tmp/from-gap.json" \
        scripts/stdlib-hosted-implementation-coordination-check.sh
done

jq '(.owners[] | select(.id == "std.fs") | .cells.HOST.status) = "pending"' \
    testing/stdlib-owner-evidence.json > "$tmp/pending-host.json"
TONDO_STDLIB_OWNER_EVIDENCE="$tmp/pending-host.json" \
    scripts/stdlib-hosted-implementation-coordination-generate.sh "$tmp/from-pending-host.json"
jq -e '.status == "open-gaps"' "$tmp/from-pending-host.json" >/dev/null
TONDO_STDLIB_OWNER_EVIDENCE="$tmp/pending-host.json" \
    TONDO_STDLIB_HOSTED_IMPLEMENTATION_COORDINATION="$tmp/from-pending-host.json" \
    scripts/stdlib-hosted-implementation-coordination-check.sh

echo "stdlib hosted implementation coordination tests: OK"
