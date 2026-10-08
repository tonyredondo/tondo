#!/usr/bin/env bash
set -euo pipefail
root="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
cd "$root"
mkdir -p .tmp
work="$(mktemp -d "$root/.tmp/civil-core-contract.XXXXXX")"
trap 'rm -rf -- "$work"' EXIT
contract="${TONDO_STDLIB_CIVIL_CORE_CONTRACT:-testing/stdlib-civil-time-core.json}"
TONDO_STDLIB_CIVIL_CORE_CONTRACT="$contract" scripts/stdlib-civil-time-core-check.sh --contract-only >/dev/null
# Promotion negatives must still change their input after the live block closes.
jq '.status="implementation-in-progress" | .quality_gate="pending-80-percent-per-scope"
    | .promotion.compiler_api="registered-public-hosted-pending-quality"' "$contract" > "$work/ready.json"
TONDO_STDLIB_CIVIL_CORE_CONTRACT="$work/ready.json" scripts/stdlib-civil-time-core-check.sh --contract-only >/dev/null
count=0
for filter in \
    '.capabilities.required=["clock"]' \
    '.capabilities.provider_reads=true' \
    '.surface.private_value_fields=false' \
    '.surface.signatures=.surface.signatures[:-1]' \
    '.surface.signatures[0].effect="civil-clock"' \
    '.sources["crates/tondo-stdlib/src/civil_time.rs"]="sha256:0000"' \
    '.model.cases=64' \
    '.model.case_limit=4096' \
    '.memory.rss="measured"' \
    '.memory.reply_descriptor_bound=7' \
    '.promotion.full_owner=true' \
    '.promotion.native_aot="verified"' \
    '.promotion.zones="verified"' \
    '.status="verified-public-hosted-core"' \
    '.promotion.compiler_api="verified-public-hosted-core"'; do
    jq "$filter" "$work/ready.json" > "$work/invalid.json"
    if TONDO_STDLIB_CIVIL_CORE_CONTRACT="$work/invalid.json" \
        scripts/stdlib-civil-time-core-check.sh --contract-only > "$work/output" 2>&1; then
        echo "civil core contract accepted drift: $filter" >&2
        exit 1
    fi
    count=$((count+1))
done
echo "civil core contract tests: OK ($count meaningful refusals)"
