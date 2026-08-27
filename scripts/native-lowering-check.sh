#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_NATIVE_LOWERING_CONTRACT:-$root/testing/native-lowering.json}"

die() {
    echo "native lowering: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with one LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains CR or trailing whitespace"

jq -e '
  def unique_values: length == (unique | length);
  .format == "tondo-native-lowering/1"
  and .owner == "toolchain.native_lowering"
  and .edition == "0.1"
  and .task == "NATIVE-002"
  and .status == "closed"
  and .implementation.mir == "crates/tondo-compiler/src/mir.rs"
  and .implementation.adapter == "tools/native-evaluation/src/main.rs"
  and .implementation.runtime == "crates/tondo-native-runtime/src/lib.rs"
  and .implementation.runner == "scripts/native-evaluation-runner.sh"
  and .implementation.evidence == "target/reliability/evidence/native-evaluation-runner.json"
  and .implementation.report_field == "native_lowering_runs"
  and .input == {
      mir_format: "tondo-mir-backend/1",
      debug_format: "tondo-mir-debug/1",
      oracle: "verified-MIR-and-bytecode-VM",
      candidates: ["cranelift", "llvm"]
  }
  and .slices == [
      "NATIVE-LOWER-CALLS-001",
      "NATIVE-LOWER-CONTROL-001",
      "NATIVE-LOWER-CLEANUP-001",
      "NATIVE-LOWER-OWNERSHIP-001",
      "NATIVE-LOWER-ASYNC-001",
      "NATIVE-LOWER-DEBUG-001",
      "NATIVE-SELECT-001",
      "NATIVE-THREAD-001"
  ]
  and .deferred.spawn == "direct-task-call-publishes-pending-before-body"
  and .deferred.captures == "immutable-scalar-constants-only"
  and .deferred.join == "evaluate-once-complete-then-await-consume"
  and .deferred.completion == "tondo_rt_task_complete(task,value)"
  and .deferred.pending_state == 0
  and .deferred.joined_state == 3
  and .deferred.unsupported == "explicit-trap-and-report"
  and .corpus.native_cases == ["deferred-task-call"]
  and .corpus.required_runtime_cases == 21
  and .corpus.required_select_cases == 8
  and .corpus.required_thread_cases == 5
  and .corpus.fresh_process_per_case == true
  and .corpus.oracle == "same-runtime-state-machine-in-both-candidates"
  and (.invariants | length == 9 and unique_values)
  and (.negative_cases | length == 10 and unique_values)
  and .next_blocks == ["NATIVE-STD-CORE-001"]
' "$contract" >/dev/null || die "invalid machine-readable contract"

for path in \
    docs/contracts/native-lowering.md \
    crates/tondo-compiler/src/mir.rs \
    tools/native-evaluation/src/main.rs \
    crates/tondo-native-runtime/src/lib.rs \
    scripts/native-evaluation-runner.sh \
    testing/native-evaluation-runner.json; do
    [[ -f "$root/$path" ]] || die "missing native lowering input: $path"
done

grep -Fq 'deferred_call_body' "$root/tools/native-evaluation/src/main.rs" \
    || die "adapter has no deferred callable-body path"
grep -Fq 'lower_cranelift_invoke' "$root/tools/native-evaluation/src/main.rs" \
    || die "Cranelift coordinator path is missing"
grep -Fq 'llvm_deferred_join' "$root/tools/native-evaluation/src/main.rs" \
    || die "LLVM coordinator path is missing"
grep -Fq 'tondo_rt_task_complete' "$root/tools/native-evaluation/src/main.rs" \
    || die "adapter has no task completion ABI"
grep -Fq 'fn task_complete' "$root/crates/tondo-native-runtime/src/lib.rs" \
    || die "runtime has no task completion transition"
grep -Fq 'native_lowering_runs' "$root/scripts/native-evaluation-runner.sh" \
    || die "runner does not validate native lowering evidence"
grep -Fq 'NATIVE-002' "$root/docs/contracts/native-lowering.md" \
    || die "contract does not identify NATIVE-002"

echo "native lowering: OK (coordinated MIR, deferred task body, await consumption and fail-closed slices)"
