#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-implementation-coordination-test.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT

# Rejection probes are meaningful only after accepting a valid baseline.
scripts/stdlib-implementation-coordination-check.sh

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "stdlib implementation coordination tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq 'del(.owners[0])' testing/stdlib-implementation-coordination.json > "$tmp/missing-owner.json"
expect_failure missing-owner env \
    TONDO_STDLIB_IMPLEMENTATION_COORDINATION="$tmp/missing-owner.json" \
    scripts/stdlib-implementation-coordination-check.sh

jq '.owners[0].public_api.signatures[0].status = "open-gaps"' \
    testing/stdlib-implementation-coordination.json > "$tmp/signature-gap.json"
expect_failure signature-gap env \
    TONDO_STDLIB_IMPLEMENTATION_COORDINATION="$tmp/signature-gap.json" \
    scripts/stdlib-implementation-coordination-check.sh

# Mutate the generator inputs, not only its output: regeneration must retain
# missing owners and empty surfaces as explicit gaps, even with a stale status.
for mutation in \
    '.rows |= map(select(.owner != "std.core"))' \
    '(.owners[] | select(.id == "std.core") | .owner_missing) = ["implementation-route-missing"]' \
    '.owners |= map(select(.id != "std.core"))'; do
    jq "$mutation" testing/stdlib-public-api.json > "$tmp/api-gap.json"
    TONDO_STDLIB_PUBLIC_API="$tmp/api-gap.json" \
        scripts/stdlib-implementation-coordination-generate.sh "$tmp/from-gap.json"
    jq -e '.status == "open-gaps" and (.owners | length) == 8
        and (.owners[] | select(.id == "std.core") | .public_api.status == "open-gaps")' \
        "$tmp/from-gap.json" >/dev/null
    TONDO_STDLIB_PUBLIC_API="$tmp/api-gap.json" \
        TONDO_STDLIB_IMPLEMENTATION_COORDINATION="$tmp/from-gap.json" \
        scripts/stdlib-implementation-coordination-check.sh
done

# An already verified global index is not a rejection probe. Introduce a real
# input gap, generate its valid observation, then forge only the promotion.
jq -e 'any(.rows[]; .symbol == "std.core.Option.some" and .status == "verified")' \
    testing/stdlib-public-api.json >/dev/null
jq '.status = "open-gaps" | .summary.gaps = 1
    | (.rows[] | select(.symbol == "std.core.Option.some") | .status) = "open-gaps"' \
    testing/stdlib-public-api.json > "$tmp/global-api-gap.json"
TONDO_STDLIB_PUBLIC_API="$tmp/global-api-gap.json" \
    scripts/stdlib-implementation-coordination-generate.sh "$tmp/global-gap.json"
TONDO_STDLIB_PUBLIC_API="$tmp/global-api-gap.json" \
    TONDO_STDLIB_IMPLEMENTATION_COORDINATION="$tmp/global-gap.json" \
    scripts/stdlib-implementation-coordination-check.sh
jq '.global_public_api.status = "verified" | .global_public_api.gaps = 0' \
    "$tmp/global-gap.json" > "$tmp/global-promotion.json"
expect_failure global-promotion env \
    TONDO_STDLIB_PUBLIC_API="$tmp/global-api-gap.json" \
    TONDO_STDLIB_IMPLEMENTATION_COORDINATION="$tmp/global-promotion.json" \
    scripts/stdlib-implementation-coordination-check.sh

echo "stdlib implementation coordination tests: OK"
