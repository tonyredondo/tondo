#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
scripts/native-target-check.sh
scripts/native-target-descriptor-check.sh
target_dir="${CARGO_TARGET_DIR:-$root/target-fast}"
evidence="$target_dir/reliability/evidence"
mkdir -p "$evidence"

host="$(rustc -vV | sed -n 's/^host: //p')"
[[ "$host" == "x86_64-unknown-linux-gnu" ]] \
    || { echo "native target smoke: host $host is not the admitted target" >&2; exit 1; }
cc="${TONDO_NATIVE_CC:-/usr/bin/cc}"
[[ "$cc" = /* && -x "$cc" ]] || { echo "native target smoke: compiler must be an absolute executable" >&2; exit 1; }

TONDO_NATIVE_CC="$cc" CARGO_TARGET_DIR="$target_dir" scripts/native-link-test.sh >/dev/null
jq -e '
  .format == "tondo-native-link-evidence/1"
  and .task == "NATIVE-LINK-001"
  and .status == "passed"
  and .driver_absolute == true and .shell == false and .ambient_lookup == false
  and .output_verified == true and .inputs_hash_verified == true
  and .product_bytes > 0
  and (.product_sha256 | test("^sha256:[0-9a-f]{64}$"))
' "$evidence/native-link.json" >/dev/null

contract_hash="$(sha256sum testing/native-target.json | cut -d ' ' -f1)"
descriptor_hash="$(sha256sum testing/native-target-descriptor.json | cut -d ' ' -f1)"
jq -n --arg contract "sha256:$contract_hash" --arg descriptor "sha256:$descriptor_hash" \
  --arg host "$host" --arg source_revision "$(git rev-parse HEAD)" \
  '{format:"tondo-native-target-evidence/1",task:"NATIVE-TARGET-001",status:"passed",contract:$contract,descriptor:$descriptor,source_revision:$source_revision,target:"tondo-native-linux-x86-64-release",triple:$host,object_format:"elf",profile:"release",physical_smoke:true,artifact_kind:"executable",backends:["cranelift","llvm"],capabilities:["clock","console","filesystem","process"],cross_compile_is_smoke:false,path_lookup:false,environment_lookup:false,product:{status:"passed",bytes:.},physical_paths:[]}' \
  > "$evidence/native-target.json"
# Replace the intentionally unused jq input placeholder with a stable product
# marker; no physical path or machine-specific size enters the identity.
jq '.product = {status:"passed",verified_by:"native-link-evidence"}' \
  "$evidence/native-target.json" > "$evidence/native-target.tmp"
mv "$evidence/native-target.tmp" "$evidence/native-target.json"
echo "native target smoke tests: OK (x86_64-unknown-linux-gnu physical target)"
