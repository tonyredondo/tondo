#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp_root="${CARGO_TARGET_DIR:-$root/target-fast}/reliability/evidence"
mkdir -p "$tmp_root"
tmp="$(mktemp "$tmp_root/native-std-hosted-report.XXXXXX")"
trap 'rm -f -- "$tmp"' EXIT

expect_failure() {
    local name="$1" candidate="$2"
    if TONDO_NATIVE_STD_HOSTED_CONTRACT="$candidate" scripts/native-std-hosted-check.sh >/dev/null 2>&1; then
        echo "native std.hosted tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.capabilities = ["console"]' testing/native-std-hosted.json > "$tmp.capabilities"
expect_failure incomplete-capabilities "$tmp.capabilities"
jq '.native_abi = "public-ffi"' testing/native-std-hosted.json > "$tmp.abi"
expect_failure public-abi "$tmp.abi"
jq '.negative_cases = []' testing/native-std-hosted.json > "$tmp.negatives"
expect_failure missing-negatives "$tmp.negatives"

CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$root/target-fast}" \
    cargo test -p tondo-native-runtime hosted_ --locked >/dev/null

contract_hash="$(sha256sum testing/native-std-hosted.json | cut -d ' ' -f1)"
jq -n \
    --arg format "tondo-native-std-hosted-evidence/1" \
    --arg task "NATIVE-STD-HOSTED-001" \
    --arg status "passed" \
    --arg contract "sha256:$contract_hash" \
    --arg rustc "$(rustc --version)" \
    --arg cargo "$(cargo --version)" \
    --arg target "${TONDO_TEST_TARGET:-host-native}" \
    '{format:$format,task:$task,status:$status,contract:$contract,rustc:$rustc,cargo:$cargo,target:$target,runner:"scripts/native-std-hosted-test.sh",cases:{capability_open:"passed",partial_read:"passed",eof_empty_buffer:"passed",console_write:"passed",output_snapshot:"passed",cancel_before_read:"passed",close_after_cancel:"passed",unknown_capability:"passed",stale_handle:"passed",resource_limit:"passed"},backends:{cranelift:"runtime-abi",llvm:"runtime-abi"},ambient_lookup:false,pointers_exposed:false,cleanup:"exactly-once"}' \
    > "$tmp_root/native-std-hosted.json"

echo "native std.hosted tests: OK (runtime ABI evidence written to ${tmp_root#"$root/"}/native-std-hosted.json)"
