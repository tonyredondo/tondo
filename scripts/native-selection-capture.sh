#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
scripts/native-selection-check.sh
contract="${TONDO_NATIVE_SELECTION_CONTRACT:-$root/testing/native-selection.json}"
# The fast and executable lanes both publish their reports under the normal
# target by default.  Reuse mode must inspect that same directory; otherwise a
# standalone promotion bind would look in a different, empty target and fail
# even though the preceding lanes passed.
target_dir="${CARGO_TARGET_DIR:-target}"
evidence="$target_dir/reliability/evidence"
mkdir -p "$evidence"

# The capture is opt-in because the physical lane compiles 178+ fresh native
# subprocesses. A caller may reuse reports from the immediately preceding
# capture, but a missing report always fails closed.
if [[ "${TONDO_NATIVE_SELECTION_REUSE:-0}" != 1 ]]; then
    TONDO_LLVM_LLC="${TONDO_LLVM_LLC:-/usr/bin/llc}" \
    CARGO_TARGET_DIR="$target_dir" scripts/native-evaluation-fast.sh >/dev/null
    TONDO_LLVM_LLC="${TONDO_LLVM_LLC:-/usr/bin/llc}" \
    TONDO_NATIVE_CC="${TONDO_NATIVE_CC:-/usr/bin/cc}" \
    CARGO_TARGET_DIR="$target_dir" scripts/native-evaluation-runner.sh >/dev/null
fi

fast="$evidence/native-evaluation-fast.json"
runner="$evidence/native-evaluation-runner.json"
[[ -f "$fast" ]] || { echo "native selection readiness: missing fast report" >&2; exit 1; }
[[ -f "$runner" ]] || { echo "native selection readiness: missing executable report" >&2; exit 1; }

jq -e '
  .status == "passed"
  and ([.candidates[] | select(.status == "measured")] | length == 2)
  and all(.candidates[] | select(.status == "measured");
    (.samples | length) == 12 and all(.samples[]; .compile_time_ns > 0 and .code_size_bytes > 0))
  and .promotion.selected_backend == null
' "$fast" >/dev/null || { echo "native selection readiness: invalid fast report" >&2; exit 1; }

jq -e '
  .status == "passed" and .phase == "NATIVE-001"
  and ([.native_runs[] | select(.cranelift == "passed" and .llvm == "passed")] | length == 118)
  and ([.native_managed_runs[] | select(.cranelift == "passed" and .llvm == "passed")] | length == 3)
  and ([.native_runtime_runs[] | select(.cranelift == "passed" and .llvm == "passed")] | length == 21)
  and ([.native_select_runs[] | select(.cranelift == "passed" and .llvm == "passed")] | length == 8)
  and ([.native_thread_runs[] | select(.cranelift == "passed" and .llvm == "passed")] | length == 5)
  and ([.native_std_core_runs[] | select(.cranelift == "passed" and .llvm == "passed")] | length == 14)
  and ([.native_lowering_runs[] | select(.cranelift == "passed" and .llvm == "passed")] | length == 1)
  and ([.native_diagnostics.cases[] | select(.cranelift == "passed" and .llvm == "passed")] | length == 8)
  and all(.native_runs[]; .oracle_status == "returned" or .oracle_status == "trapped")
  and (.native_diagnostics.status == "passed")
' "$runner" >/dev/null || { echo "native selection readiness: invalid executable report" >&2; exit 1; }
! grep -Fq "$root" "$fast" || { echo "native selection readiness: fast report leaked path" >&2; exit 1; }
! grep -Fq "$root" "$runner" || { echo "native selection readiness: executable report leaked path" >&2; exit 1; }

selection_status="$(jq -r '.selection.status' "$contract")"
selected_backend="$(jq -r '.selection.selected_backend' "$contract")"
[[ "$selection_status" == "selected" && "$selected_backend" == "cranelift" ]] \
    || { echo "native selection readiness: decision record does not select Cranelift" >&2; exit 1; }

fast_hash="$(sha256sum "$fast" | cut -d ' ' -f1)"
runner_hash="$(sha256sum "$runner" | cut -d ' ' -f1)"
target="$(jq -r '.target' "$runner")"
[[ "$target" == "x86_64-unknown-linux-gnu" ]] || { echo "native selection readiness: target drift" >&2; exit 1; }

jq -n --arg fast "sha256:$fast_hash" --arg runner "sha256:$runner_hash" --arg target "$target" \
  --slurpfile f "$fast" --slurpfile r "$runner" --slurpfile s "$contract" \
  '{format:"tondo-native-selection-evidence/1",task:"NATIVE-001",status:$s[0].status,target:$target,candidates:{cranelift:{status:"selected",compile_time_ns_median_sum:($f[0].candidates[]|select(.id=="cranelift")|[.summary[].compile_time_ns.median]|add),code_size_bytes_sum:($f[0].candidates[]|select(.id=="cranelift")|[.summary[].code_size_bytes]|add)},llvm:{status:"experimental",compile_time_ns_median_sum:($f[0].candidates[]|select(.id=="llvm")|[.summary[].compile_time_ns.median]|add),code_size_bytes_sum:($f[0].candidates[]|select(.id=="llvm")|[.summary[].code_size_bytes]|add)}},executable_counts:{scalar:118,managed:3,runtime:21,select:8,thread:5,std_core:14,lowering:1,diagnostics:8},reports:{fast:$fast,executable:$runner},selection:$s[0].selection,physical_paths:[],divergences:[]}' \
  > "$evidence/native-selection.json"
echo "native selection readiness: PASS (Cranelift selected for 0.1 AOT; independent Gate N1 promotion is recorded separately)"
