#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
scripts/native-conf-stdlib-check.sh
target_dir="${CARGO_TARGET_DIR:-$root/target-fast}"
tmp="$(mktemp -d "$target_dir/reliability/native-conf-stdlib.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
for backend in cranelift llvm; do
    scripts/native-conformance-adapter.sh --backend "$backend" --target x86_64-unknown-linux-gnu --category stdlib --output "$tmp/$backend.json"
    jq -e --arg b "$backend" '
      .backend == $b and .category == "stdlib" and .status == "passed"
      and ([.observations[].id] | sort) == ["stdlib-cleanup", "stdlib-core", "stdlib-hosted"]
      and ([.observations[] | select(.native == .oracle and .status == "passed")] | length == 3)
      and ([.observations[].oracle | select(.resources_released == 1)] | length == 1)
    ' "$tmp/$backend.json" >/dev/null
done
contract_hash="$(sha256sum testing/native-conf-stdlib.json | cut -d ' ' -f1)"
mkdir -p "$target_dir/reliability/evidence"
jq -n --arg contract "sha256:$contract_hash" \
  '{format:"tondo-native-conf-stdlib-evidence/1",task:"NATIVE-CONF-STDLIB-001",status:"passed",contract:$contract,target:"x86_64-unknown-linux-gnu",owners:["std.core","std.hosted"],backends:{cranelift:{status:"passed",cases:3},llvm:{status:"passed",cases:3}},oracle:"bytecode-vm-oracle",capabilities:["clock","console","filesystem","process"],cleanup:"exactly-once",physical_paths:[],divergences:[]}' \
  > "$target_dir/reliability/evidence/native-conf-stdlib.json"
echo "native stdlib conformance tests: OK (3 owner cases x 2 candidates)"
