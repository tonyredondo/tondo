#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_NATIVE_STD_CORE_CONTRACT:-$root/testing/native-std-core.json}"

die() {
    echo "native std.core: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing std.core contract"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains trailing whitespace"

jq -e '
  .format == "tondo-native-std-core/1"
  and .task == "NATIVE-STD-CORE-001"
  and .edition == "0.1"
  and .phase == "M20"
  and .status == "closed"
  and .contract == "docs/contracts/native-std-core.md"
  and .fixture == "tests/native/native-std-core-001.to"
  and .runner == "scripts/native-evaluation-runner.sh"
  and .report == "target/reliability/evidence/native-evaluation-runner.json"
  and .report_field == "native_std_core_runs"
  and .backends == ["cranelift", "llvm"]
  and (.apis | length == 9)
  and (.cases | length == 14)
  and (.invariants | length == 8)
  and (.negative_cases | length == 8)
  and .next_blocks == ["NATIVE-STD-HOSTED-001"]
' "$contract" >/dev/null || die "invalid std.core contract"

for path in \
    docs/contracts/native-std-core.md \
    tests/native/native-std-core-001.to \
    scripts/native-evaluation-runner.sh \
    tools/native-evaluation/src/main.rs \
    crates/tondo-compiler/src/mir.rs; do
    [[ -f "$root/$path" ]] || die "missing std.core input: $path"
done

grep -Fq 'MirBackendOperand::Projection' crates/tondo-compiler/src/mir.rs \
    || die "MIR has no projection boundary"
for kind in option-value result-ok-value result-err-value; do
    grep -Fq "\"$kind\"" crates/tondo-compiler/src/mir.rs \
        || die "MIR does not publish projection kind: $kind"
done
grep -Fq 'backend_function_values' crates/tondo-compiler/src/mir.rs \
    || die "MIR does not resolve static mapper values"
grep -Fq 'tondo_rt_result_payload' tools/native-evaluation/src/main.rs \
    || die "native adapter has no payload ABI"
grep -Fq 'run_native_std_core_probe' tools/native-evaluation/src/main.rs \
    || die "native adapter has no std.core probe"
grep -Fq 'native_std_core_runs' tools/native-evaluation/src/main.rs \
    || die "native report has no std.core field"
grep -Fq -- '--std-core-probe' tools/native-evaluation/src/main.rs \
    || die "native adapter has no std.core probe option"
grep -Fq 'native_std_core_runs' scripts/native-evaluation-runner.sh \
    || die "runner does not assert std.core evidence"
grep -Fq 'Option.map' docs/contracts/native-std-core.md \
    || die "std.core contract omits Option.map"
grep -Fq 'Result.mapErr' docs/contracts/native-std-core.md \
    || die "std.core contract omits Result.mapErr"

echo "native std.core: OK (opaque Option/Result carrier, direct mappers and both native candidates)"
