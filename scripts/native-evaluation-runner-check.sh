#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_NATIVE_EVALUATION_RUNNER_CONTRACT:-$root/testing/native-evaluation-runner.json}"

die() {
    echo "native evaluation runner: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing runner contract"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains trailing whitespace"

jq -e '
  .format == "tondo-native-evaluation-runner/1"
  and .phase == "NATIVE-BACKEND-ADAPTER-001"
  and .status == "evaluation-ready"
  and .runner == "scripts/native-evaluation-runner.sh"
  and .report == "target/reliability/evidence/native-evaluation-runner.json"
  and .parent_contract == "testing/native-evaluation-fast.json"
  and .adapter_format == "tondo-mir-backend/1"
  and .debug_format == "tondo-mir-debug/1"
  and .debug_report_field == "debug_metadata"
  and .thread_report_field == "native_thread_runs"
  and .std_core_report_field == "native_std_core_runs"
  and .std_core_contract == "testing/native-std-core.json"
  and .std_core_probe == "tests/native/native-std-core-001.to"
  and .std_core_case_count == 14
  and .lowering_report_field == "native_lowering_runs"
  and .aot_binary_report_field == "native_aot_binary"
  and .aot_binary_contract == "testing/native-aot-binary.json"
  and .aot_binary_phase == "NATIVE-AOT-BINARY-001"
  and .aot_memory_report_field == "native_aot_memory"
  and .aot_memory_contract == "testing/native-aot-memory.json"
  and .aot_memory_phase == "NATIVE-AOT-MEM-001"
  and .diagnostic_report_field == "native_diagnostics"
  and .diagnostic_phase == "DIAG-NATIVE-001"
  and .lowering_case == "deferred-task-call"
  and .oracle == "bytecode-vm-scalar-and-managed-result-oracle"
  and .candidates == ["cranelift", "llvm"]
  and .toolchain_policy == {
      llvm: "explicit-absolute-llc-18",
      linker: "explicit-absolute-cc",
      ambient_path_lookup: "forbidden",
      physical_paths_in_report: "forbidden"
  }
  and .native_semantics == "scalar-and-managed-result-checked-arithmetic-logical-conversions-control-flow-host-calls-cleanup-ownership-async-thread-select-std-core-and-traps"
  and (.negative_cases | length == 11)
' "$contract" >/dev/null || die "invalid runner contract"

for path in \
    scripts/native-evaluation-runner.sh \
    tools/native-evaluation/src/main.rs \
    testing/native-evaluation-fast.json \
    testing/native-std-core.json; do
    [[ -f "$root/$path" ]] || die "missing runner input: $path"
done
[[ -f "$root/testing/native-aot-binary.json" ]] || die "missing linked-product contract"
[[ -x "$root/scripts/native-aot-binary-check.sh" ]] || die "missing linked-product check"
[[ -x "$root/scripts/native-aot-binary-test.sh" ]] || die "missing linked-product tests"
[[ -f "$root/testing/native-aot-memory.json" ]] || die "missing AOT memory contract"
[[ -x "$root/scripts/native-aot-memory-check.sh" ]] || die "missing AOT memory check"
[[ -x "$root/scripts/native-aot-memory-test.sh" ]] || die "missing AOT memory tests"
[[ -f "$root/tests/native/native-std-core-001.to" ]] || die "missing std.core native fixture"

grep -Fq -- '--cc' tools/native-evaluation/src/main.rs \
    || die "adapter has no explicit linker argument"
grep -Fq -- '--strip' tools/native-evaluation/src/main.rs \
    || die "adapter has no explicit strip argument"
grep -Fq -- '--readelf' tools/native-evaluation/src/main.rs \
    || die "adapter has no explicit section-reader argument"
grep -Fq 'vm_scalar' crates/tondo-compiler/examples/native_mir_probe.rs \
    || die "probe has no VM scalar observations"
grep -Fq 'vm_result' tools/native-evaluation/src/main.rs \
    || die "adapter does not report VM scalar results"
grep -Fq 'vm_managed' crates/tondo-compiler/examples/native_mir_probe.rs \
    || die "probe has no VM managed observations"
grep -Fq 'native_managed_runs' tools/native-evaluation/src/main.rs \
    || die "adapter does not report managed native results"
grep -Fq 'native_std_core_runs' tools/native-evaluation/src/main.rs \
    || die "adapter does not report native std.core results"
grep -Fq 'run_native_std_core_probe' tools/native-evaluation/src/main.rs \
    || die "adapter has no native std.core probe"
grep -Fq 'native_runtime_runs' tools/native-evaluation/src/main.rs \
    || die "adapter does not report runtime contract results"
grep -Fq 'native_select_runs' tools/native-evaluation/src/main.rs \
    || die "adapter does not report native selection results"
grep -Fq 'native_thread_runs' tools/native-evaluation/src/main.rs \
    || die "adapter does not report native thread results"
grep -Fq 'native_lowering_runs' tools/native-evaluation/src/main.rs \
    || die "adapter does not report coordinated lowering results"
grep -Fq 'native_aot_binary' tools/native-evaluation/src/main.rs \
    || die "adapter does not report linked AOT products"
grep -Fq 'run_native_aot_binary_probe' tools/native-evaluation/src/main.rs \
    || die "adapter has no linked AOT product probe"
grep -Fq 'run_native_aot_memory_probe' tools/native-evaluation/src/main.rs \
    || die "adapter has no AOT memory probe"
grep -Fq 'native_aot_memory' tools/native-evaluation/src/main.rs \
    || die "adapter has no AOT memory report field"
grep -Fq 'native_diagnostics' tools/native-evaluation/src/main.rs \
    || die "adapter does not report native diagnostics"
grep -Fq 'run_native_diagnostics_probe' tools/native-evaluation/src/main.rs \
    || die "adapter has no native diagnostics probe"
grep -Fq 'tondo_rt_diag_field' tools/native-evaluation/src/main.rs \
    || die "adapter does not expose diagnostic fields"
grep -Fq 'tondo_rt_task_complete' tools/native-evaluation/src/main.rs \
    || die "adapter does not expose deferred task completion"
grep -Fq 'std::thread::Builder' crates/tondo-native-runtime/src/lib.rs \
    || die "runtime does not launch an OS worker"
grep -Fq 'pthread_create' tools/native-evaluation/src/main.rs \
    || die "native runner does not launch an OS worker"
grep -Fq 'scalar-and-managed-result-checked-arithmetic-logical-conversions-control-flow-host-calls-cleanup-ownership-async-thread-select-std-core-and-traps' \
    tools/native-evaluation/src/main.rs \
    || die "adapter has no executable scalar evidence state"

echo "native evaluation runner: OK (explicit toolchain and scalar oracle contract)"
