#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_NATIVE_AOT_MEMORY_CONTRACT:-$root/testing/native-aot-memory.json}"

die() {
    echo "native AOT memory: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with one LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains CR or trailing whitespace"

jq -e '
  def unique_values: length == (unique | length);
  .format == "tondo-native-aot-memory/1"
  and .owner == "toolchain.native_evaluation"
  and .edition == "0.1"
  and .task == "NATIVE-AOT-MEM-001"
  and .status == "closed"
  and .implementation.adapter == "tools/native-evaluation/src/main.rs"
  and .implementation.runtime == "tools/native-evaluation/src/main.rs:native_runtime_c_source"
  and .implementation.runner == "scripts/native-evaluation-runner.sh"
  and .implementation.evidence == "target/reliability/evidence/native-evaluation-runner.json"
  and .implementation.report_field == "native_aot_memory"
  and .implementation.sample_format == "tondo-native-aot-memory-sample/1"
  and .input == {
      mir_format: "tondo-mir-backend/1",
      target: "host-native-target-from-runner",
      profile: "release",
      runtime_abi: "tondo-runtime-draft/1",
      stdlib: "STD-0.1A",
      candidates: ["cranelift", "llvm"],
      product: "complete-linked-aot-executable",
      same_inputs: true,
      instrumentation: "process-local-runtime-counters-only"
  }
  and .protocol == {
      warmup_iterations: 3,
      measurement_repetitions: 9,
      independent_processes: 3,
      minimum_sample_count: 27,
      fresh_processes: true,
      summary: ["median", "p95", "p99"],
      seed: "tondo-native-aot-memory-0.1"
  }
  and .dimensions == [
      "allocation_count", "allocated_bytes", "peak_live_bytes", "live_bytes",
      "retain_local", "retain_atomic", "release_local", "release_atomic",
      "cycles_reclaimed", "weak_upgrades", "pause_ns",
      "concurrency_operations", "rss_peak_bytes"
  ]
  and .oracle == {
      vm: "exact-values-errors-ownership-cancellation-and-exit-status",
      native: "instrumented-linked-product-must-match-vm-observables",
      counters: "native-only-not-language-semantics",
      mismatch: "fail-closed"
  }
  and (.invariants | length == 15 and unique_values)
  and (.negative_cases | length == 14 and unique_values)
  and .next_blocks == ["NATIVE-AOT-QUALITY-001"]
' "$contract" >/dev/null || die "invalid machine-readable contract"

for path in \
    docs/contracts/native-aot-memory.md \
    TONDO_IMPLEMENTATION_TRACKER.md \
    testing/tracker-graph.json \
    tools/native-evaluation/src/main.rs \
    scripts/native-evaluation-runner.sh \
    testing/native-evaluation-runner.json; do
    [[ -f "$root/$path" ]] || die "missing AOT memory evidence: $path"
done

grep -Fq 'NATIVE-AOT-MEM-001' "$root/TONDO_IMPLEMENTATION_TRACKER.md" \
    || die "tracker does not reference the AOT memory block"
grep -Fq 'run_native_aot_memory_probe' "$root/tools/native-evaluation/src/main.rs" \
    || die "adapter has no AOT memory probe"
grep -Fq 'tondo-native-aot-memory/1' "$root/tools/native-evaluation/src/main.rs" \
    || die "adapter has no AOT memory report"
grep -Fq 'tondo_rt_memory_metric' "$root/tools/native-evaluation/src/main.rs" \
    || die "runtime harness has no memory metric ABI"
grep -Fq 'native_aot_memory' "$root/scripts/native-evaluation-runner.sh" \
    || die "runner does not validate AOT memory evidence"

jq -e '
  (.task_dependencies["NATIVE-AOT-MEM-001"] | index("NATIVE-AOT-LOWER-001")) != null
  and (.task_dependencies["NATIVE-AOT-MEM-001"] | index("ARC-002")) != null
  and (.task_dependencies["NATIVE-AOT-MEM-001"] | index("DIAG-NATIVE-001")) != null
  and (.task_dependencies["NATIVE-AOT-PERF-001"] | index("NATIVE-AOT-MEM-001")) != null
' testing/tracker-graph.json >/dev/null || die "tracker graph does not preserve memory evidence order"

echo "native AOT memory: OK (ARC counters, cycles, weak upgrades, fresh-process samples and RSS contract)"
