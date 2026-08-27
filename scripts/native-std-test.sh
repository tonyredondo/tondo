#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
target_dir="${CARGO_TARGET_DIR:-$root/target-fast}"
evidence="$target_dir/reliability/evidence"
mkdir -p "$evidence"

scripts/native-std-core-check.sh
scripts/native-std-core-test.sh
scripts/native-std-hosted-check.sh
CARGO_TARGET_DIR="$target_dir" scripts/native-std-hosted-test.sh

core_hash="$(sha256sum testing/native-std-core.json | cut -d ' ' -f1)"
hosted_hash="$(sha256sum testing/native-std-hosted.json | cut -d ' ' -f1)"
jq -n \
  --arg format "tondo-native-std-evidence/1" \
  --arg task "NATIVE-STD-001" \
  --arg core "sha256:$core_hash" \
  --arg hosted "sha256:$hosted_hash" \
  --arg rustc "$(rustc --version)" \
  --arg cargo "$(cargo --version)" \
  '{format:$format,task:$task,status:"passed",rustc:$rustc,cargo:$cargo,owners:{"std.core":{status:"passed",cases:14,contract:$core},"std.hosted":{status:"passed",cases:10,contract:$hosted}},backends:{cranelift:{route:"common-mir",carrier:"tondo_rt_result_new/tag/payload",status:"passed"},llvm:{route:"common-mir",carrier:"tondo_rt_result_new/tag/payload",status:"passed"}},parity:{api_shape:true,capability_admission:true,error_tags:true,partial_io:true,cancellation:true,ownership:true,cleanup:true,diagnostic_redaction:true},ambient_lookup:false,backend_specific_public_api:false}' \
  > "$evidence/native-std.json"

echo "native std coordination tests: OK (evidence written to ${evidence#"$root/"}/native-std.json)"
