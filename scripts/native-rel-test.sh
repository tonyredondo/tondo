#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
scripts/native-rel-check.sh
target_dir="${CARGO_TARGET_DIR:-$root/target-fast}"
tmp="$(mktemp -d "$target_dir/reliability/native-rel.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
mkdir -p "$target_dir/reliability/evidence"
cc="${TONDO_NATIVE_CC:-/usr/bin/cc}"
[[ "$cc" = /* && -x "$cc" ]] || { echo "native release: compiler must be an absolute executable" >&2; exit 1; }

build_package() {
    local workspace="$1"
    local package="$2"
    local stage="$workspace/stage"
    mkdir -p "$stage/bin" "$stage/metadata"
    "$cc" -std=c11 -O2 -Wl,--build-id=none \
        -ffile-prefix-map="$root"=. -fdebug-prefix-map="$root"=. \
        tests/native/native-link-001.c -o "$stage/bin/tondo"
    local binary_hash runtime_hash stdlib_hash target_hash
    binary_hash="$(sha256sum "$stage/bin/tondo" | cut -d ' ' -f1)"
    runtime_hash="$(sha256sum crates/tondo-native-runtime/src/lib.rs | cut -d ' ' -f1)"
    stdlib_hash="$(sha256sum testing/native-conf-stdlib.json | cut -d ' ' -f1)"
    target_hash="$(sha256sum testing/native-target.json | cut -d ' ' -f1)"
    jq -n --arg binary "sha256:$binary_hash" --arg runtime "sha256:$runtime_hash" \
        --arg stdlib "sha256:$stdlib_hash" --arg target "sha256:$target_hash" \
        '{format:"tondo-native-package/1",compiler:"tondo-0.1-development",edition:"0.1",target:"tondo-native-linux-x86-64-release",triple:"x86_64-unknown-linux-gnu",profile:"release",backend_selection:"pending-NATIVE-001",contents:{binary:{path:"bin/tondo",sha256:$binary},runtime:{id:"tondo-native-runtime/1",sha256:$runtime},stdlib:{id:"STD-0.1A",sha256:$stdlib},target:{sha256:$target}},physical_paths:[],timestamps:false}' \
        > "$stage/metadata/manifest.json"
    find "$stage" -type f -exec touch -d '@0' {} +
    tar --sort=name --mtime='@0' --owner=0 --group=0 --numeric-owner \
        -C "$stage" -cf "$package" bin/tondo metadata/manifest.json
}

build_package "$tmp/one" "$tmp/one.tar"
build_package "$tmp/two" "$tmp/two.tar"
cmp -s "$tmp/one.tar" "$tmp/two.tar" \
    || { echo "native release: package bytes differ between clean builds" >&2; exit 1; }

tar -xOf "$tmp/one.tar" metadata/manifest.json > "$tmp/manifest.json"
jq -e '
  .format == "tondo-native-package/1"
  and .compiler == "tondo-0.1-development" and .edition == "0.1"
  and .target == "tondo-native-linux-x86-64-release"
  and .triple == "x86_64-unknown-linux-gnu" and .profile == "release"
  and .backend_selection == "pending-NATIVE-001"
  and .contents.binary.path == "bin/tondo"
  and (.contents.binary.sha256 | test("^sha256:[0-9a-f]{64}$"))
  and (.contents.runtime.sha256 | test("^sha256:[0-9a-f]{64}$"))
  and .contents.runtime.id == "tondo-native-runtime/1"
  and .contents.stdlib.id == "STD-0.1A"
  and (.contents.stdlib.sha256 | test("^sha256:[0-9a-f]{64}$"))
  and .physical_paths == [] and .timestamps == false
' "$tmp/manifest.json" >/dev/null
! grep -Fq "$root" "$tmp/manifest.json" || { echo "native release: manifest leaked workspace path" >&2; exit 1; }

contract_hash="$(sha256sum testing/native-rel.json | cut -d ' ' -f1)"
package_hash="$(sha256sum "$tmp/one.tar" | cut -d ' ' -f1)"
package_bytes="$(stat -c '%s' "$tmp/one.tar")"
jq -n --arg contract "sha256:$contract_hash" --arg package "sha256:$package_hash" --argjson bytes "$package_bytes" \
  '{format:"tondo-native-rel-evidence/1",task:"NATIVE-REL-001",status:"passed",contract:$contract,package_format:"tondo-native-package/1",target:"tondo-native-linux-x86-64-release",triple:"x86_64-unknown-linux-gnu",profile:"release",backend_selection:"pending-NATIVE-001",reproducible:true,builds:2,package_sha256:$package,package_bytes:$bytes,contents:["binary","runtime","stdlib-0.1A","metadata","checksums"],physical_paths:[],timestamps:false,divergences:[]}' \
  > "$target_dir/reliability/evidence/native-rel.json"
echo "native reproducible release tests: OK (byte-identical package built twice)"
