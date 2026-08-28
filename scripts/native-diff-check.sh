#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_NATIVE_DIFF_CONTRACT:-$root/testing/native-diff.json}"

die() { echo "native differential: $*" >&2; exit 1; }
[[ -f "$contract" ]] || die "missing differential contract"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains trailing whitespace"

jq -e '
  .format == "tondo-native-diff/1"
  and .task == "NATIVE-DIFF-001"
  and .owner == "toolchain.native_diff"
  and .edition == "0.1" and .phase == "M20" and .status == "closed"
  and .contract == "docs/contracts/native-diff.md"
  and .runner == "scripts/native-diff-test.sh"
  and .adapter == "scripts/native-conformance-adapter.sh"
  and .probe == "testing/native-conf-probe.json"
  and .parent == "testing/native-conf.json"
  and .backends == ["cranelift", "llvm"]
  and .target == "x86_64-unknown-linux-gnu"
  and .oracle == "bytecode-vm-oracle"
  and .seed == "tondo-native-diff-0.1"
  and .generation == "probe-cases-x-backends"
  and .cases == 9
  and (.properties | length == 6 and unique)
  and .executable_lane == {runner:"scripts/native-evaluation-runner.sh",status:"opt-in",toolchain:"absolute-llc-18-and-cc"}
  and (.negative_cases | length == 8 and unique)
  and .next_blocks == ["NATIVE-TARGET-001", "NATIVE-REL-001"]
' "$contract" >/dev/null || die "invalid differential contract"

for path in \
    docs/contracts/native-diff.md \
    scripts/native-diff-test.sh \
    scripts/native-conformance-adapter.sh \
    scripts/native-evaluation-runner.sh \
    testing/native-conf.json \
    testing/native-conf-probe.json; do
    [[ -f "$root/$path" ]] || die "missing differential input: $path"
done
for marker in 'deterministic' 'independent VM oracle' 'NATIVE-001' 'TONDO_NATIVE_DIFF_EXECUTABLE'; do
    grep -Fq "$marker" docs/contracts/native-diff.md \
        || die "differential contract omits $marker"
done
echo "native differential: OK"
