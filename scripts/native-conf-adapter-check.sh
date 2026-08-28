#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_NATIVE_CONF_ADAPTER_CONTRACT:-$root/testing/native-conf-adapter.json}"
die() { echo "native conformance adapter: $*" >&2; exit 1; }
[[ -f "$contract" ]] || die "missing adapter contract"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains trailing whitespace"
jq -e '
  .format == "tondo-native-conf-adapter/1"
  and .task == "NATIVE-CONF-ADAPTER-001"
  and .owner == "toolchain.native_conf_adapter"
  and .edition == "0.1" and .phase == "M20" and .status == "closed"
  and .contract == "docs/contracts/native-conf-adapter.md"
  and .runner == "scripts/native-conformance-adapter.sh"
  and .probe == "testing/native-conf-probe.json"
  and .protocol == "tondo-native-observation/1"
  and .backends == ["cranelift", "llvm"]
  and .targets == ["x86_64-unknown-linux-gnu"]
  and (.capabilities | length == 4)
  and (.observations | length == 8)
  and (.invariants | length == 6)
  and (.negative_cases | length == 8)
  and .next_blocks == ["NATIVE-CONF-LANGUAGE-001", "NATIVE-CONF-TESTING-001", "NATIVE-CONF-STDLIB-001"]
' "$contract" >/dev/null || die "invalid adapter contract"
for path in docs/contracts/native-conf-adapter.md scripts/native-conformance-adapter.sh testing/native-conf-probe.json; do
    [[ -f "$root/$path" ]] || die "missing adapter input: $path"
done
grep -Fq 'fail-closed' docs/contracts/native-conf-adapter.md || die "adapter is not fail-closed"
grep -Fq 'physical_paths' scripts/native-conformance-adapter.sh || die "adapter does not redact paths"
echo "native conformance adapter: OK"
