#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp="$(mktemp -d "${TMPDIR:-/tmp}/tondo-async-select-conformance-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

expect_contract_failure() {
    local name="$1"
    local contract="$2"
    if TONDO_ASYNC_SELECT_CONTRACT="$contract" scripts/async-select-conformance.sh >/dev/null 2>&1; then
        echo "async select conformance contract test: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.cases[0].fixture_sha256 = ("0" * 64)' \
    testing/async-select-conformance.json > "$tmp/fixture-hash.json"
expect_contract_failure fixture-hash-mismatch "$tmp/fixture-hash.json"

jq '.cases[1].repetitions = 31' \
    testing/async-select-conformance.json > "$tmp/repetition-count.json"
expect_contract_failure repetition-count "$tmp/repetition-count.json"

jq '.pipeline[5] = "native-vm"' \
    testing/async-select-conformance.json > "$tmp/pipeline-drift.json"
expect_contract_failure pipeline-drift "$tmp/pipeline-drift.json"

jq '.invariants.native_backend_claim = true' \
    testing/async-select-conformance.json > "$tmp/native-claim.json"
expect_contract_failure native-claim "$tmp/native-claim.json"

echo "async select conformance contract tests: OK"
