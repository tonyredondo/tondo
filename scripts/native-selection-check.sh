#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_NATIVE_SELECTION_CONTRACT:-$root/testing/native-selection.json}"

die() { echo "native selection readiness: $*" >&2; exit 1; }
[[ -f "$contract" ]] || die "missing selection contract"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains trailing whitespace"

jq -e '
  .format == "tondo-native-selection/1"
  and .task == "NATIVE-001"
  and .owner == "toolchain.native_evaluation"
  and .edition == "0.1" and .phase == "M20"
  and .status == "ready-for-decision"
  and .contract == "docs/contracts/native-selection.md"
  and .runner == "scripts/native-selection-capture.sh"
  and .fast_report == "target/reliability/evidence/native-evaluation-fast.json"
  and .executable_report == "target/reliability/evidence/native-evaluation-runner.json"
  and .evaluation_report == "target/reliability/evidence/native-evaluation.json"
  and .candidates == ["cranelift", "llvm"] and .excluded == ["custom"]
  and .target == "x86_64-unknown-linux-gnu"
  and .oracle == "bytecode-vm-oracle"
  and .selection == {
    selected_backend:null,
    status:"human-decision-required",
    n1_claim:false,
    required_inputs:["full-native-evidence","repeated-performance-capture","quality-gate","DEC-013-record"]
  }
  and .required_executable_counts == {
    scalar:118, managed:3, runtime:21, select:8,
    thread:5, std_core:14, lowering:1, diagnostics:8
  }
  and (.negative_cases | length == 10 and unique)
  and .next_blocks == ["DEC-013"]
' "$contract" >/dev/null || die "invalid selection contract"

for path in docs/contracts/native-selection.md scripts/native-selection-capture.sh \
    testing/native-evaluation-fast.json testing/native-evaluation-runner.json \
    testing/native-evaluation.json; do
    [[ -f "$root/$path" ]] || die "missing selection input: $path"
done
for marker in 'human decision' 'selected_backend' 'n1_claim' 'fail-closed' 'DEC-013'; do
    grep -Fq "$marker" docs/contracts/native-selection.md \
        || die "selection contract omits $marker"
done
echo "native selection readiness: OK"
