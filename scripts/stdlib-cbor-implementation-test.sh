#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-cbor-implementation-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.cbor implementation tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.implementation.status = "pending-after-native-gate"' testing/stdlib-cbor.json > "$tmp_dir/pending.json"
expect_failure pending env TONDO_STDLIB_CBOR_CONTRACT="$tmp_dir/pending.json" scripts/stdlib-cbor-implementation-check.sh

jq '.implementation.quality_gate = "passed"' testing/stdlib-cbor.json > "$tmp_dir/quality.json"
expect_failure quality env TONDO_STDLIB_CBOR_CONTRACT="$tmp_dir/quality.json" scripts/stdlib-cbor-implementation-check.sh

jq '.implementation.public_api_promoted = true' testing/stdlib-cbor.json > "$tmp_dir/public.json"
expect_failure public env TONDO_STDLIB_CBOR_CONTRACT="$tmp_dir/public.json" scripts/stdlib-cbor-implementation-check.sh

jq '.implementation.host = "verified-production-host"' testing/stdlib-cbor.json > "$tmp_dir/host.json"
expect_failure host env TONDO_STDLIB_CBOR_CONTRACT="$tmp_dir/host.json" scripts/stdlib-cbor-implementation-check.sh

jq '.implementation.native_aot_lowering = "verified"' testing/stdlib-cbor.json > "$tmp_dir/native.json"
expect_failure native env TONDO_STDLIB_CBOR_CONTRACT="$tmp_dir/native.json" scripts/stdlib-cbor-implementation-check.sh

jq '.implementation.sources = .implementation.sources[0:2]' testing/stdlib-cbor.json > "$tmp_dir/source.json"
expect_failure source env TONDO_STDLIB_CBOR_CONTRACT="$tmp_dir/source.json" scripts/stdlib-cbor-implementation-check.sh

jq '.implementation.tests = .implementation.tests[0:13]' testing/stdlib-cbor.json > "$tmp_dir/test.json"
expect_failure test env TONDO_STDLIB_CBOR_CONTRACT="$tmp_dir/test.json" scripts/stdlib-cbor-implementation-check.sh

jq '.promotion.next_blocks = ["STD-CBOR-PERF-001"]' testing/stdlib-cbor.json > "$tmp_dir/next.json"
expect_failure next env TONDO_STDLIB_CBOR_CONTRACT="$tmp_dir/next.json" scripts/stdlib-cbor-implementation-check.sh

scripts/stdlib-cbor-implementation-check.sh
echo "std.cbor implementation tests: OK (promotion and evidence negatives)"
