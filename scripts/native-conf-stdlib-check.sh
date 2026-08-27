#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_NATIVE_CONF_STDLIB_CONTRACT:-$root/testing/native-conf-stdlib.json}"
die() { echo "native stdlib conformance: $*" >&2; exit 1; }
[[ -f "$contract" ]] || die "missing stdlib contract"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains trailing whitespace"
jq -e '
  .format == "tondo-native-conf-stdlib/1" and .task == "NATIVE-CONF-STDLIB-001"
  and .owner == "toolchain.native_conf_stdlib" and .edition == "0.1" and .phase == "M20"
  and .status == "closed" and .contract == "docs/contracts/native-conf-stdlib.md"
  and .runner == "scripts/native-conf-stdlib-test.sh" and .adapter == "scripts/native-conformance-adapter.sh"
  and .probe == "testing/native-conf-probe.json" and .category == "stdlib"
  and .backends == ["cranelift", "llvm"] and .target == "x86_64-unknown-linux-gnu"
  and .owners == ["std.core", "std.hosted"] and (.capabilities | length == 4)
  and .cases == ["stdlib-core", "stdlib-hosted", "stdlib-cleanup"]
  and (.dimensions | length == 6) and (.negative_cases | length == 6)
  and .next_blocks == ["NATIVE-CONF-001"]
' "$contract" >/dev/null || die "invalid stdlib contract"
for path in docs/contracts/native-conf-stdlib.md scripts/native-conf-stdlib-test.sh scripts/native-conformance-adapter.sh testing/native-conf-probe.json; do
    [[ -f "$root/$path" ]] || die "missing stdlib input: $path"
done
grep -Fq 'capability' docs/contracts/native-conf-stdlib.md || die "stdlib capability boundary is undocumented"
grep -Fq 'cleanup' docs/contracts/native-conf-stdlib.md || die "stdlib cleanup is undocumented"
echo "native stdlib conformance: OK"
