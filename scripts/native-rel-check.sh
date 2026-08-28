#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_NATIVE_REL_CONTRACT:-$root/testing/native-rel.json}"

die() { echo "native reproducible release: $*" >&2; exit 1; }
[[ -f "$contract" ]] || die "missing release contract"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains trailing whitespace"

jq -e '
  .format == "tondo-native-rel/1"
  and .task == "NATIVE-REL-001"
  and .owner == "toolchain.native_release"
  and .edition == "0.1" and .phase == "M20" and .status == "closed"
  and .contract == "docs/contracts/native-rel.md"
  and .runner == "scripts/native-rel-test.sh"
  and .package_format == "tondo-native-package/1"
  and .inputs == [
    "testing/native-target.json",
    "testing/native-diff.json",
    "testing/native-conf.json",
    "testing/native-publish.json",
    "testing/native-link.json",
    "crates/tondo-native-runtime/src/lib.rs"
  ]
  and .target == "tondo-native-linux-x86-64-release"
  and .triple == "x86_64-unknown-linux-gnu"
  and .profile == "release"
  and .backend_selection == "cranelift"
  and .promotion == "pending-gate-n1"
  and .contents == ["binary", "runtime", "stdlib-0.1A", "metadata", "checksums"]
  and .reproducibility == {
    archive:"deterministic-tar",
    mtime:"epoch-zero",
    owners:"numeric-zero",
    workspace_paths:"forbidden",
    environment:"undeclared-inputs-forbidden",
    comparison:"byte-identical-package-twice"
  }
  and (.negative_cases | length == 8 and unique)
  and .next_blocks == ["N1"]
' "$contract" >/dev/null || die "invalid release contract"

for path in docs/contracts/native-rel.md scripts/native-rel-test.sh \
    testing/native-target.json testing/native-diff.json testing/native-conf.json \
    testing/native-publish.json testing/native-link.json \
    crates/tondo-native-runtime/src/lib.rs; do
    [[ -f "$root/$path" ]] || die "missing release input: $path"
done
for marker in 'deterministic' 'epoch-zero' 'STD-0.1A' 'Cranelift' 'pending-gate-n1' 'physical workspace'; do
    grep -Fq "$marker" docs/contracts/native-rel.md \
        || die "release contract omits $marker"
done
echo "native reproducible release: OK"
