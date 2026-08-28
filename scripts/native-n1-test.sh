#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="$root/testing/native-n1.json"
report="${TONDO_NATIVE_N1_REPORT:-$root/target/reliability/evidence/native-n1.json}"
tmp_root="$root/.tmp"
mkdir -p "$tmp_root"
tmp="$(mktemp -d "$tmp_root/tondo-native-n1-test.XXXXXX")"
dirty_sentinel="$root/.native-n1-dirty-${BASHPID}"
trap 'rm -rf -- "$tmp" "$dirty_sentinel"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "native N1 tests: negative case passed unexpectedly: $name" >&2
        exit 1
    fi
}

[[ -f "$report" ]] || {
    echo "native N1 tests: generated report is missing" >&2
    exit 1
}

expect_failure report-missing env TONDO_NATIVE_N1_REPORT="$tmp/missing.json" \
    "$root/scripts/native-n1-check.sh"

jq '.format = "tondo-native-n1-mutated/1"' "$contract" > "$tmp/contract-format.json"
expect_failure contract-format-drift env TONDO_NATIVE_N1_CONTRACT="$tmp/contract-format.json" \
    "$root/scripts/native-n1-check.sh" --contract-only

jq '.status = "failed"' "$report" > "$tmp/report-status.json"
expect_failure report-status-drift env TONDO_NATIVE_N1_REPORT="$tmp/report-status.json" \
    "$root/scripts/native-n1-check.sh"

jq '.source_revision = ("0" * 40)' "$report" > "$tmp/report-stale.json"
expect_failure source-revision-drift env TONDO_NATIVE_N1_REPORT="$tmp/report-stale.json" \
    "$root/scripts/native-n1-check.sh"

jq '.backend.selected = "llvm"' "$report" > "$tmp/backend-drift.json"
expect_failure backend-drift env TONDO_NATIVE_N1_REPORT="$tmp/backend-drift.json" \
    "$root/scripts/native-n1-check.sh"

jq '.targets[0].promotion = "candidate-smoke-only"' "$report" > "$tmp/target-not-promoted.json"
expect_failure published-target-missing env TONDO_NATIVE_N1_REPORT="$tmp/target-not-promoted.json" \
    "$root/scripts/native-n1-check.sh"

jq '.targets[1].promotion = "promoted"' "$report" > "$tmp/candidate-promoted.json"
expect_failure candidate-target-promoted env TONDO_NATIVE_N1_REPORT="$tmp/candidate-promoted.json" \
    "$root/scripts/native-n1-check.sh"

jq '.summary.divergences = 1' "$report" > "$tmp/divergence.json"
expect_failure divergent-quality env TONDO_NATIVE_N1_REPORT="$tmp/divergence.json" \
    "$root/scripts/native-n1-check.sh"

jq '.summary.quality_mutation_caught = 5' "$report" > "$tmp/quality-regression.json"
expect_failure quality-regression env TONDO_NATIVE_N1_REPORT="$tmp/quality-regression.json" \
    "$root/scripts/native-n1-check.sh"

jq '.summary.unsupported_admitted = 1' "$report" > "$tmp/diagnostic-unsupported.json"
expect_failure diagnostic-unsupported env TONDO_NATIVE_N1_REPORT="$tmp/diagnostic-unsupported.json" \
    "$root/scripts/native-n1-check.sh"

jq '(.reports[] | select(.id == "NATIVE-REL-001") | .status) = "failed"' \
    "$report" > "$tmp/package-not-reproducible.json"
expect_failure package-not-reproducible env TONDO_NATIVE_N1_REPORT="$tmp/package-not-reproducible.json" \
    "$root/scripts/native-n1-check.sh"

jq '.claims.public_abi = true' "$report" > "$tmp/public-abi.json"
expect_failure public-abi-claim env TONDO_NATIVE_N1_REPORT="$tmp/public-abi.json" \
    "$root/scripts/native-n1-check.sh"

jq '.physical_paths = ["/tmp/private"]' "$report" > "$tmp/path-leak.json"
expect_failure physical-path-leak env TONDO_NATIVE_N1_REPORT="$tmp/path-leak.json" \
    "$root/scripts/native-n1-check.sh"

jq '.criteria[0].status = "failed"' "$report" > "$tmp/aot-incomplete.json"
expect_failure aot-campaign-incomplete env TONDO_NATIVE_N1_REPORT="$tmp/aot-incomplete.json" \
    "$root/scripts/native-n1-check.sh"

printf 'dirty\n' > "$dirty_sentinel"
expect_failure dirty-workspace "$root/scripts/native-n1-check.sh"
rm -f -- "$dirty_sentinel"

scripts/native-n1-check.sh >/dev/null
echo "native N1 tests: OK (14 contract, provenance, target, quality, package, privacy and workspace negatives rejected)"
