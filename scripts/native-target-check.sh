#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_NATIVE_TARGET_REGISTRY:-$root/testing/native-target.json}"

die() { echo "native target registry: $*" >&2; exit 1; }
[[ -f "$contract" ]] || die "missing target registry"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "registry must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "registry contains trailing whitespace"

jq -e '
  .format == "tondo-native-target-registry/1"
  and .task == "NATIVE-TARGET-001"
  and .owner == "toolchain.native_target"
  and .edition == "0.1" and .phase == "M20" and .status == "closed"
  and .contract == "docs/contracts/native-target.md"
  and .runner == "scripts/native-target-test.sh"
  and .descriptor == "testing/native-target-descriptor.json"
  and (.targets | length == 1)
  and .targets[0] == {
    id:"tondo-native-linux-x86-64-release",
    triple:"x86_64-unknown-linux-gnu",
    architecture:"x86_64",
    os:"linux",
    object_format:"elf",
    profile:"release",
    capabilities:["clock","console","filesystem","process"],
    backends:["cranelift","llvm"],
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
  and .next_blocks == ["NATIVE-REL-001"]
' "$contract" >/dev/null || die "invalid target registry"

for path in docs/contracts/native-target.md scripts/native-target-test.sh \
    testing/native-target-descriptor.json testing/native/native-link-001.c; do
    [[ -f "$root/$path" ]] || die "missing target input: $path"
done
for marker in 'physical smoke' 'Cross-compilation' 'x86_64-unknown-linux-gnu' 'ambient'; do
    grep -Fqi "$marker" docs/contracts/native-target.md \
        || die "target contract omits $marker"
done
echo "native target registry: OK"
