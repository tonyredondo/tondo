#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_NATIVE_CONF_TESTING_CONTRACT:-$root/testing/native-conf-testing.json}"
die() { echo "native testing conformance: $*" >&2; exit 1; }
[[ -f "$contract" ]] || die "missing testing contract"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains trailing whitespace"
jq -e '
  .format == "tondo-native-conf-testing/1" and .task == "NATIVE-CONF-TESTING-001"
  and .owner == "toolchain.native_conf_testing" and .edition == "0.1" and .phase == "M20"
  and .status == "closed" and .contract == "docs/contracts/native-conf-testing.md"
  and .runner == "scripts/native-conf-testing-test.sh" and .adapter == "scripts/native-conformance-adapter.sh"
  and .probe == "testing/native-conf-probe.json" and .category == "testing"
  and .backends == ["cranelift", "llvm"] and .target == "x86_64-unknown-linux-gnu"
  and .cases == ["testing-pass", "testing-fail", "testing-isolation"]
  and (.dimensions | length == 6) and (.negative_cases | length == 6)
  and .next_blocks == ["NATIVE-CONF-TESTING-001", "NATIVE-CONF-STDLIB-001"]
' "$contract" >/dev/null || die "invalid testing contract"
for path in docs/contracts/native-conf-testing.md scripts/native-conf-testing-test.sh scripts/native-conformance-adapter.sh testing/native-conf-probe.json; do
    [[ -f "$root/$path" ]] || die "missing testing input: $path"
done
grep -Fq 'P0007' docs/contracts/native-conf-testing.md || die "testing failure code is undocumented"
grep -Fq 'cleanup' docs/contracts/native-conf-testing.md || die "testing cleanup is undocumented"
echo "native testing conformance: OK"
