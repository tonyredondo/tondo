#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_NATIVE_CONF_LANGUAGE_CONTRACT:-$root/testing/native-conf-language.json}"
die() { echo "native language conformance: $*" >&2; exit 1; }
[[ -f "$contract" ]] || die "missing language contract"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains trailing whitespace"
jq -e '
  .format == "tondo-native-conf-language/1" and .task == "NATIVE-CONF-LANGUAGE-001"
  and .owner == "toolchain.native_conf_language" and .edition == "0.1" and .phase == "M20"
  and .status == "closed" and .contract == "docs/contracts/native-conf-language.md"
  and .runner == "scripts/native-conf-language-test.sh"
  and .adapter == "scripts/native-conformance-adapter.sh"
  and .probe == "testing/native-conf-probe.json" and .category == "language"
  and .backends == ["cranelift", "llvm"] and .target == "x86_64-unknown-linux-gnu"
  and .cases == ["language-scalar", "language-result-error", "language-panic"]
  and (.dimensions | length == 5) and (.negative_cases | length == 6)
  and .next_blocks == ["NATIVE-CONF-TESTING-001", "NATIVE-CONF-STDLIB-001"]
' "$contract" >/dev/null || die "invalid language contract"
for path in docs/contracts/native-conf-language.md scripts/native-conf-language-test.sh scripts/native-conformance-adapter.sh testing/native-conf-probe.json; do
    [[ -f "$root/$path" ]] || die "missing language input: $path"
done
grep -Fq 'VM oracle' docs/contracts/native-conf-language.md || die "language oracle is undocumented"
echo "native language conformance: OK"
