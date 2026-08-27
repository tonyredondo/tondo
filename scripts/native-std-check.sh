#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_NATIVE_STD_CONTRACT:-$root/testing/native-std.json}"

die() { echo "native std coordination: $*" >&2; exit 1; }
[[ -f "$contract" ]] || die "missing coordination contract"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains trailing whitespace"

jq -e '
  .format == "tondo-native-std/1"
  and .task == "NATIVE-STD-001"
  and .owner == "toolchain.native_std"
  and .edition == "0.1"
  and .phase == "M20"
  and .status == "closed"
  and .contract == "docs/contracts/native-std.md"
  and .runner == "scripts/native-std-test.sh"
  and .report == "target/reliability/evidence/native-std.json"
  and .inputs == ["testing/native-std-core.json", "testing/native-std-hosted.json"]
  and .owners == ["std.core", "std.hosted"]
  and .backends == ["cranelift", "llvm"]
  and .shared_contract == {
    carrier: "tondo_rt_result_new/tag/payload",
    errors: "opaque-result-with-private-status",
    cleanup: "arc-and-host-terminal-exactly-once",
    mir: "tondo-mir-backend/1"
  }
  and (.parity_dimensions | length == 8)
  and (.cases | length == 4)
  and (.negative_cases | length == 8)
  and .next_blocks == ["NATIVE-LINK-001"]
' "$contract" >/dev/null || die "invalid coordination contract"

for path in docs/contracts/native-std.md scripts/native-std-test.sh testing/native-std-core.json testing/native-std-hosted.json; do
    [[ -f "$root/$path" ]] || die "missing coordination input: $path"
done
for marker in 'tondo_rt_result_new' 'tondo-mir-backend/1' 'common-mir' 'capability'; do
    grep -Fq "$marker" docs/contracts/native-std.md \
        || die "coordination contract omits $marker"
done
grep -Fq 'std.core' scripts/native-std-test.sh || die "runner omits std.core"
grep -Fq 'std.hosted' scripts/native-std-test.sh || die "runner omits std.hosted"
grep -Fq 'cranelift' scripts/native-std-test.sh || die "runner omits Cranelift"
grep -Fq 'llvm' scripts/native-std-test.sh || die "runner omits LLVM"

echo "native std coordination: OK (Core/Hosted share carrier, MIR and cleanup)"
