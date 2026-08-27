#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_NATIVE_LINK_CONTRACT:-$root/testing/native-link.json}"
die() { echo "native link: $*" >&2; exit 1; }
[[ -f "$contract" ]] || die "missing native link contract"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains trailing whitespace"

jq -e '
  .format == "tondo-native-link/1"
  and .task == "NATIVE-LINK-001"
  and .owner == "toolchain.native_link"
  and .edition == "0.1"
  and .phase == "M20"
  and .status == "closed"
  and .contract == "docs/contracts/native-link.md"
  and .runner == "scripts/native-link-test.sh"
  and .fixture == "tests/native/native-link-001.c"
  and .inputs == ["testing/native-target-descriptor.json", "testing/native-artifact.json", "testing/native-link-plan.json"]
  and .invocation == {driver:"absolute-executable-resolved-before-invocation",arguments:"ordered-plan-tokens-only",inputs:"ordered-hash-verified-materialized-paths",output:"validated-product-path"}
  and .reproducibility == {workspaces:2,comparison:"sha256-of-product-bytes",undeclared_inputs:"rejected"}
  and (.invariants | length == 7)
  and (.negative_cases | length == 9)
  and .next_blocks == ["NATIVE-CLI-001"]
' "$contract" >/dev/null || die "invalid native link contract"

for path in docs/contracts/native-link.md scripts/native-link-test.sh tests/native/native-link-001.c; do
    [[ -f "$root/$path" ]] || die "missing native link input: $path"
done
for marker in \
    'NativeLinkPlan' \
    'NativeTargetDescriptor' \
    'NativeArtifact' \
    'validate_against' \
    'without shell' \
    'workspace'; do
    grep -Fq "$marker" crates/tondo-compiler/src/toolchain.rs docs/contracts/native-link.md scripts/native-link-test.sh \
        || die "native link boundary is missing $marker"
done
echo "native link: OK (hash-closed direct invocation and workspace reproducibility)"
