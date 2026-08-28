#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_NATIVE_TARGET_ARM64_REGISTRY:-$root/testing/native-target-aarch64.json}"

die() { echo "native target ARM64 registry: $*" >&2; exit 1; }
[[ -f "$contract" ]] || die "missing target registry"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "registry must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "registry contains trailing whitespace"

jq -e '
  .format == "tondo-native-target-registry/1"
  and .task == "NATIVE-TARGET-002"
  and .owner == "toolchain.native_target"
  and .edition == "0.1" and .phase == "M20" and .status == "closed"
  and .contract == "docs/contracts/native-target.md"
  and .runner == "scripts/native-target-aarch64-test.sh"
  and .descriptor == "testing/native-target-descriptor.json"
  and (.targets | length == 1)
  and .targets[0] == {
    id:"tondo-native-linux-aarch64-release",
    triple:"aarch64-unknown-linux-gnu",
    architecture:"aarch64",
    os:"linux",
    object_format:"elf",
    profile:"release",
    capabilities:["clock","console","filesystem","process"],
    backends:["cranelift"],
    smoke_fixture:"testing/native/native-link-001.c",
    artifact_kind:"executable"
  }
  and .policy == {
    registry:"closed-targets-only",
    cross_compile_is_smoke:false,
    path_lookup:"forbidden",
    environment_lookup:"forbidden",
    physical_target_smoke:"required"
  }
  and (.negative_cases | length == 8 and unique)
  and .next_blocks == ["N1"]
' "$contract" >/dev/null || die "invalid target registry"

for path in docs/contracts/native-target.md scripts/native-target-aarch64-test.sh \
    testing/native-target-descriptor.json testing/native/native-link-001.c; do
    [[ -f "$root/$path" ]] || die "missing target input: $path"
done
for marker in 'NATIVE-TARGET-002' 'aarch64-unknown-linux-gnu' 'physical smoke' \
    'Cross-compilation' 'pending-gate-n1'; do
    grep -Fqi "$marker" docs/contracts/native-target.md \
        || die "target contract omits $marker"
done
echo "native target ARM64 registry: OK"
