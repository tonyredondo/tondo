#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp="$(mktemp -d "${TMPDIR:-/tmp}/tondo-performance-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

expect_failure() {
    local name="$1"
    local contract="$2"
    if TONDO_PERFORMANCE_CONTRACT="$contract" scripts/performance-check.sh >/dev/null 2>&1; then
        echo "performance contract test: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '(.workloads[0].fixture_sha256) = ("0" * 64)' \
    testing/performance.json > "$tmp/hash-mismatch.json"
expect_failure fixture-hash-mismatch "$tmp/hash-mismatch.json"

jq '.workloads[1].id = .workloads[0].id' \
    testing/performance.json > "$tmp/duplicate-workload.json"
expect_failure duplicate-workload "$tmp/duplicate-workload.json"

jq '.workloads[0].bounds.steps = 0' \
    testing/performance.json > "$tmp/zero-bound.json"
expect_failure zero-bound "$tmp/zero-bound.json"

jq '.environment.identity_fields += ["timestamp"]' \
    testing/performance.json > "$tmp/unstable-identity.json"
expect_failure unstable-identity "$tmp/unstable-identity.json"

jq '.backends[1].status = "baseline-required"' \
    testing/performance.json > "$tmp/native-before-baseline.json"
expect_failure native-before-baseline "$tmp/native-before-baseline.json"

jq '.workload_classes |= map(select(.id != "adversarial"))' \
    testing/performance.json > "$tmp/missing-adversarial.json"
expect_failure missing-adversarial "$tmp/missing-adversarial.json"

echo "performance contract tests: OK"
