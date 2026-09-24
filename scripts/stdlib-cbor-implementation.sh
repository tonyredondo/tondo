#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_CBOR_CONTRACT:-$root/testing/stdlib-cbor.json}"
target_dir="${CARGO_TARGET_DIR:-target}"
if [[ "$target_dir" != /* ]]; then
    target_dir="$root/$target_dir"
fi
evidence_dir="${TONDO_STDLIB_EVIDENCE_DIR:-$target_dir/reliability/evidence}"
logs_dir="$evidence_dir/stdlib-cbor-implementation-logs"
mkdir -p "$logs_dir"

die() {
    echo "std.cbor implementation: $*" >&2
    exit 1
}

TONDO_STDLIB_CBOR_CONTRACT="$contract" scripts/stdlib-cbor-implementation-check.sh

stdlib_test_log="$logs_dir/stdlib-tests.log"
CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-stdlib --locked cbor::tests \
    >"$stdlib_test_log" 2>&1 \
    || { cat "$stdlib_test_log" >&2; die "CBOR stdlib tests failed"; }

clippy_log="$logs_dir/clippy.log"
CARGO_TARGET_DIR="$target_dir" cargo clippy -q -p tondo-stdlib --locked -- -D warnings \
    >"$clippy_log" 2>&1 \
    || { cat "$clippy_log" >&2; die "CBOR stdlib Clippy failed"; }

source_revision="$(git rev-parse HEAD)"
contract_sha256="$(sha256sum "$contract" | cut -d" " -f1)"
stdlib_test_sha256="$(sha256sum "$stdlib_test_log" | cut -d" " -f1)"
clippy_sha256="$(sha256sum "$clippy_log" | cut -d" " -f1)"
jq -n \
    --arg revision "$source_revision" \
    --arg contract_sha256 "$contract_sha256" \
    --arg stdlib_test_sha256 "$stdlib_test_sha256" \
    --arg clippy_sha256 "$clippy_sha256" \
    '
      {
        format:"tondo-stdlib-cbor-implementation-evidence/1",
        task:"STD-CBOR-IMPL-001",
        status:"kernel-passed-quality-pending",
        quality_gate:"pending-80-percent-per-scope",
        source_revision:$revision,
        contract_sha256:("sha256:" + $contract_sha256),
        scalar_tests:{package:"tondo-stdlib",filter:"cbor::tests",status:"passed",log_sha256:("sha256:" + $stdlib_test_sha256)},
        clippy:{package:"tondo-stdlib",status:"passed",log_sha256:("sha256:" + $clippy_sha256)},
        public_boundary:{api_promoted:false,host:"not-claimed-until-compiler-cbor-abi",native_runtime:"not-claimed",native_aot_lowering:"not-claimed"},
        fixture:null,
        physical_paths:[],
        timestamps:false,
        addresses:[],
        divergences:[]
      }
    ' >"$evidence_dir/stdlib-cbor-implementation.json"

echo "std.cbor implementation: kernel checks passed, quality pending (report: $evidence_dir/stdlib-cbor-implementation.json)"
