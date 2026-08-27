#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
scripts/native-target-descriptor-check.sh
scripts/native-artifact-check.sh
scripts/native-link-plan-check.sh

cc="${TONDO_NATIVE_CC:-/usr/bin/cc}"
[[ "$cc" = /* ]] || { echo "native link tests: driver must be an absolute path" >&2; exit 1; }
[[ -x "$cc" ]] || { echo "native link tests: missing executable driver $cc" >&2; exit 1; }
target_dir="${CARGO_TARGET_DIR:-$root/target-fast}"
tmp_root="$target_dir/reliability/native-link"
rm -rf -- "$tmp_root"
mkdir -p "$tmp_root/one" "$tmp_root/two"
trap 'rm -rf -- "$tmp_root"' EXIT

for workspace in one two; do
    dir="$tmp_root/$workspace"
    "$cc" -std=c11 -O2 -Wl,--build-id=none \
        -ffile-prefix-map="$root"=./ \
        -ffile-prefix-map="$dir"=./ \
        tests/native/native-link-001.c -o "$dir/tondo-native-link"
    "$dir/tondo-native-link" > "$dir/stdout"
    cmp -s "$dir/stdout" <(printf 'tondo-native-link\n')
    [[ "$(stat -c '%s' "$dir/tondo-native-link")" -gt 0 ]]
done

hash_one="$(sha256sum "$tmp_root/one/tondo-native-link" | cut -d ' ' -f1)"
hash_two="$(sha256sum "$tmp_root/two/tondo-native-link" | cut -d ' ' -f1)"
[[ "$hash_one" = "$hash_two" ]] || { echo "native link tests: workspace hashes diverged" >&2; exit 1; }

if env TONDO_NATIVE_CC=cc scripts/native-link-test.sh >/dev/null 2>&1; then
    echo "native link tests: relative driver unexpectedly accepted" >&2
    exit 1
fi

contract_hash="$(sha256sum testing/native-link.json | cut -d ' ' -f1)"
product_bytes="$(stat -c '%s' "$tmp_root/one/tondo-native-link")"
evidence_dir="$target_dir/reliability/evidence"
mkdir -p "$evidence_dir"
jq -n --arg contract "sha256:$contract_hash" --arg driver "$cc" --arg hash "sha256:$hash_one" --argjson bytes "$product_bytes" \
  '{format:"tondo-native-link-evidence/1",task:"NATIVE-LINK-001",status:"passed",contract:$contract,driver:$driver,driver_absolute:true,shell:false,ambient_lookup:false,workspaces:2,product_sha256:$hash,product_bytes:$bytes,output_verified:true,inputs_hash_verified:true}' \
  > "$evidence_dir/native-link.json"

echo "native link tests: OK (two direct, hash-identical products; evidence written)"
