#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
scripts/native-target-aarch64-check.sh
scripts/native-target-descriptor-check.sh

host="$(rustc -vV | sed -n 's/^host: //p')"
[[ "$host" == "aarch64-unknown-linux-gnu" ]] \
    || { echo "native target ARM64 smoke: host $host is not the admitted target" >&2; exit 1; }
cc="${TONDO_NATIVE_CC:-/usr/bin/cc}"
[[ "$cc" = /* && -x "$cc" ]] \
    || { echo "native target ARM64 smoke: compiler must be an absolute executable" >&2; exit 1; }

target_dir="${CARGO_TARGET_DIR:-$root/target-fast}"
if [[ "$target_dir" != /* ]]; then
    target_dir="$root/$target_dir"
fi
evidence="$target_dir/reliability/evidence"
mkdir -p "$evidence"

TONDO_NATIVE_CC="$cc" CARGO_TARGET_DIR="$target_dir" scripts/native-link-test.sh >/dev/null
jq -e '
  .format == "tondo-native-link-evidence/1"
  and .task == "NATIVE-LINK-001"
  and .status == "passed"
  and .driver_absolute == true and .shell == false and .ambient_lookup == false
  and .output_verified == true and .inputs_hash_verified == true
  and .product_bytes > 0
  and (.product_sha256 | test("^sha256:[0-9a-f]{64}$"))
' "$evidence/native-link.json" >/dev/null || {
    echo "native target ARM64 smoke: link evidence failed validation" >&2
    exit 1
}

contract_hash="$(sha256sum testing/native-target-aarch64.json | cut -d ' ' -f1)"
descriptor_hash="$(sha256sum testing/native-target-descriptor.json | cut -d ' ' -f1)"
report="${TONDO_NATIVE_TARGET_EVIDENCE:-$target_dir/reliability/evidence/native-target-aarch64.json}"
mkdir -p "$(dirname "$report")"
jq -n \
    --arg contract "sha256:$contract_hash" \
    --arg descriptor "sha256:$descriptor_hash" \
    --arg host "$host" \
    --arg source_revision "$(git rev-parse HEAD)" \
    '{format:"tondo-native-target-evidence/1",task:"NATIVE-TARGET-002",status:"passed",contract:$contract,descriptor:$descriptor,source_revision:$source_revision,target:"tondo-native-linux-aarch64-release",triple:$host,object_format:"elf",profile:"release",physical_smoke:true,artifact_kind:"executable",backends:["cranelift"],capabilities:["clock","console","filesystem","process"],cross_compile_is_smoke:false,path_lookup:false,environment_lookup:false,product:{status:"passed",verified_by:"native-link-evidence"},physical_paths:[]}' \
    > "$report"

echo "native target ARM64 smoke tests: OK (aarch64-unknown-linux-gnu physical target; report: ${report#"$root/"})"
