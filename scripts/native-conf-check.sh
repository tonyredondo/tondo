#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_NATIVE_CONF_CONTRACT:-$root/testing/native-conf.json}"

die() { echo "native conformance coordination: $*" >&2; exit 1; }
[[ -f "$contract" ]] || die "missing coordination contract"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains trailing whitespace"

jq -e '
  .format == "tondo-native-conf/1"
  and .task == "NATIVE-CONF-001"
  and .owner == "toolchain.native_conf"
  and .edition == "0.1" and .phase == "M20" and .status == "closed"
  and .contract == "docs/contracts/native-conf.md"
  and .runner == "scripts/native-conf-test.sh"
  and .adapter == "scripts/native-conformance-adapter.sh"
  and .probe == "testing/native-conf-probe.json"
  and .inputs == [
    "testing/native-conf-adapter.json",
    "testing/native-conf-language.json",
    "testing/native-conf-testing.json",
    "testing/native-conf-stdlib.json"
  ]
  and .categories == ["language", "testing", "stdlib"]
  and .owners == ["language", "testing", "std.core", "std.hosted"]
  and .backends == ["cranelift", "llvm"]
  and .target == "x86_64-unknown-linux-gnu"
  and .oracle == "bytecode-vm-oracle"
  and .mir == "tondo-mir-backend/1"
  and (.dimensions | length == 8 and unique)
  and .cases == 9
  and (.negative_cases | length == 9 and unique)
  and .next_blocks == ["NATIVE-DIFF-001"]
' "$contract" >/dev/null || die "invalid coordination contract"

for path in \
    docs/contracts/native-conf.md \
    scripts/native-conf-test.sh \
    scripts/native-conformance-adapter.sh \
    testing/native-conf-probe.json \
    testing/native-conf-adapter.json \
    testing/native-conf-language.json \
    testing/native-conf-testing.json \
    testing/native-conf-stdlib.json; do
    [[ -f "$root/$path" ]] || die "missing coordination input: $path"
done
for marker in 'fail-closed' 'independent VM oracle' 'tondo-mir-backend/1' 'NATIVE-DIFF-001'; do
    grep -Fq "$marker" docs/contracts/native-conf.md \
        || die "coordination contract omits $marker"
done
echo "native conformance coordination: OK"
