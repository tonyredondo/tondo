#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_NATIVE_AOT_PERF_CONTRACT:-$root/testing/native-aot-performance.json}"
report="${TONDO_NATIVE_AOT_PERF_REPORT:-}"

die() {
    echo "native AOT performance: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains CR or trailing whitespace"

jq -e '
  def unique_values: length == (unique | length);
  (.workloads | map(.id)) as $workload_ids
  | (.dimensions | map(.id)) as $dimension_ids
  | .format == "tondo-native-aot-performance/1"
  and .owner == "toolchain.native_evaluation"
  and .edition == "0.1"
  and .task == "NATIVE-AOT-PERF-001"
  and .phase == "NATIVE-AOT-PERF-001"
  and .status == "closed"
  and .contract == "docs/contracts/native-aot-performance.md"
  and .implementation == {
      adapter: "tools/native-evaluation/src/main.rs",
      runner: "scripts/native-aot-performance.sh",
      evidence: "target/reliability/evidence/native-aot-performance.json",
      report_format: "tondo-native-aot-performance/1"
  }
  and .input == {
      mir_format: "tondo-mir-backend/1",
      target: "host-native-target-from-runner",
      profile: "release",
      runtime_abi: "tondo-runtime-draft/1",
      stdlib: "STD-0.1A",
      candidates: ["cranelift", "llvm"],
      same_inputs: true,
      product: "complete-linked-aot-executable",
      oracle: "bytecode-vm-oracle-and-normalized-MIR-reference-interpreter",
      jit: "out-of-scope"
  }
  and .protocol == {
      warmup_iterations: 3,
      measurement_repetitions: 9,
      independent_processes: 3,
      minimum_sample_count: 27,
      fresh_runtime_processes: true,
      isolated_builds: true,
      summary: ["median", "p95", "p99"],
      seed: "tondo-native-aot-perf-0.1"
  }
  and $workload_ids == ["aot-complete-product", "aot-memory-workload"]
  and ($workload_ids | unique_values)
      and all(.workloads[];
      .required == true
      and (.class | IN("representative", "large"))
      and (.execution | type == "string" and length > 0)
      and (.fixture | type == "string" and length > 0)
      and (.dimensions | length > 0 and unique_values)
      and all(.dimensions[]; . as $id | ["compile_time", "link_time", "build_end_to_end", "code_size", "startup", "throughput", "latency", "allocation_count", "allocated_bytes", "peak_memory", "retain_operations", "release_operations", "pause_time"] | index($id) != null)
  )
  and $dimension_ids == ["compile_time", "link_time", "build_end_to_end", "code_size", "startup", "throughput", "latency", "allocation_count", "allocated_bytes", "peak_memory", "retain_operations", "release_operations", "pause_time"]
  and ($dimension_ids | unique_values)
  and all(.dimensions[]; (.unit | type == "string" and length > 0) and (.direction == "lower-is-better" or .direction == "higher-is-better") and (.source | type == "string" and length > 0))
  and .required_global_workloads == [
      "compile-empty", "compile-generic", "compile-suspension", "compile-adversarial",
      "runtime-std-core", "runtime-std-collections", "runtime-std-text", "runtime-std-codecs",
      "runtime-std-io", "runtime-async", "runtime-process", "runtime-memory-large",
      "runtime-bytes-small", "runtime-adversarial-overflow"
  ]
  and .oracle_policy == {
      runtime: "exact-language-observables-before-performance",
      native: "NATIVE-AOT-QUALITY-001",
      memory: "NATIVE-AOT-MEM-001",
      mismatch: "fail-closed"
  }
  and .vm_baseline == {
      status: "captured-separately",
      oracle: "normalized-MIR-reference-interpreter",
      covered_cases: 8,
      unsupported_cases: 20,
      sample_count: 27,
      workload: "aot-reference-interpreter-supported-subset"
  }
  and .regression_policy == {
      same_identity_only: true,
      same_workload_hash: true,
      same_target_backend_profile: true,
      no_cross_target_aggregation: true,
      improvement_does_not_cancel_regression: true,
      allocation_count_change_requires_review: true,
      unexplained_regression: "fail",
      budget_source: "PERF-001"
  }
  and .selection == {selected_backend: null, status: "human-decision-required", n1_claim: false}
  and (.invariants | length == 16 and unique_values)
  and (.negative_cases | length == 16 and unique_values)
  and .next_blocks == ["DEC-013"]
' "$contract" >/dev/null || die "invalid machine-readable contract"

for path in \
    docs/contracts/native-aot-performance.md \
    TONDO_IMPLEMENTATION_TRACKER.md \
    testing/tracker-graph.json \
    testing/performance.json \
    testing/native-aot-binary.json \
    testing/native-aot-memory.json \
    testing/native-aot-quality.json \
    tools/native-evaluation/src/main.rs \
    scripts/native-evaluation-runner.sh; do
    [[ -f "$root/$path" ]] || die "missing performance evidence: $path"
done

for path in \
    scripts/native-aot-performance.sh \
    scripts/native-aot-performance-check.sh \
    scripts/native-aot-performance-test.sh; do
    [[ -x "$root/$path" ]] || die "performance tool is not executable: $path"
done

grep -Fq 'NATIVE-AOT-PERF-001' "$root/TONDO_IMPLEMENTATION_TRACKER.md" \
    || die "tracker does not reference the performance block"
grep -Fq 'run_native_aot_performance_probe' "$root/tools/native-evaluation/src/main.rs" \
    || die "adapter has no AOT performance probe"
grep -Fq 'tondo-native-aot-performance/1' "$root/tools/native-evaluation/src/main.rs" \
    || die "adapter has no AOT performance report"
grep -Fq 'build_end_to_end_ns' "$root/tools/native-evaluation/src/main.rs" \
    || die "adapter has no end-to-end build metric"
grep -Fq -- '--aot-performance-output' "$root/tools/native-evaluation/src/main.rs" \
    || die "adapter has no performance output option"
grep -Fq 'aot-performance-output' "$root/scripts/native-evaluation-runner.sh" \
    || die "runner does not forward the performance output option"

jq -e '
  (.task_dependencies["NATIVE-AOT-PERF-001"] | index("NATIVE-AOT-BINARY-001")) != null
  and (.task_dependencies["NATIVE-AOT-PERF-001"] | index("NATIVE-AOT-MEM-001")) != null
  and (.task_dependencies["NATIVE-AOT-PERF-001"] | index("NATIVE-AOT-QUALITY-001")) != null
  and (.task_dependencies["NATIVE-AOT-PERF-001"] | index("PERF-001")) != null
  and (.task_dependencies["DEC-013"] | index("NATIVE-AOT-PERF-001")) != null
' testing/tracker-graph.json >/dev/null || die "tracker graph does not preserve performance order"

global_workloads="$(jq -c '[.workloads[].id]' testing/performance.json)"
required_workloads="$(jq -c '.required_global_workloads' "$contract")"
[[ "$global_workloads" == "$required_workloads" ]] \
    || die "required global workload frontier drifted from PERF-001"

if [[ -n "$report" ]]; then
    [[ -f "$report" ]] || die "performance report does not exist: $report"
    jq -e '
      .format == "tondo-native-aot-performance/1"
      and .phase == "NATIVE-AOT-PERF-001"
      and .status == "passed"
      and (.target | type == "string" and length > 0 and (startswith("/") | not))
      and .profile == "release"
      and .protocol == {
          warmup_iterations: 3,
          measurement_repetitions: 9,
          independent_processes: 3,
          minimum_sample_count: 27,
          fresh_processes: true,
          isolated_builds: true,
          summary: ["median", "p95", "p99"],
          seed: "tondo-native-aot-perf-0.1"
      }
      and .oracle == {
          vm: "bytecode-vm-oracle-and-normalized-MIR-reference-interpreter",
          native: "NATIVE-AOT-QUALITY-001-equivalent-linked-product",
          counters: "NATIVE-AOT-MEM-001-harness-only-observations",
          mismatch: "fail-closed"
      }
      and .vm_baseline.status == "captured-separately"
      and .vm_baseline.oracle == "normalized-MIR-reference-interpreter"
      and .vm_baseline.workload == "aot-reference-interpreter-supported-subset"
      and .vm_baseline.covered_cases == 8
      and .vm_baseline.unsupported_cases == 20
      and .vm_baseline.sample_count == 27
      and (.vm_baseline.runtime_samples | length == 27)
      and ((.vm_baseline.runtime_samples | map(.workload) | unique) == ["aot-reference-interpreter-supported-subset"])
      and ((.vm_baseline.runtime_samples | group_by(.process) | map(length)) == [9, 9, 9])
      and all(.vm_baseline.runtime_samples[]; .duration_ns > 0 and .operations > 0)
      and all(.vm_baseline.dimensions | to_entries[]; .value | .median > 0 and .p95 >= .median and .p99 >= .p95)
      and ([.workloads[].id] == ["aot-complete-product", "aot-memory-workload"])
      and .workloads[0].dimensions == ["compile_time", "link_time", "build_end_to_end", "code_size", "startup", "throughput", "latency"]
      and .comparison == {
          same_inputs: true,
          semantic_equivalence: "validated-before-measurement",
          vm_baseline: "27-sample-separate-reference-interpreter-subset",
          cross_backend_comparison: "same-target-profile-and-workload-identity-only",
          selection: "human-decision-required"
      }
      and (.candidates | map(.id) | sort) == ["cranelift", "llvm"]
      and all(.candidates[];
          .status == "passed"
          and (.toolchain | type == "string" and length > 0 and (contains("/") | not))
          and (.build_samples | length == 27)
          and (.runtime_samples | length == 27)
          and (.runtime_samples | map(.workload) | unique) == ["aot-complete-product"]
          and ([.build_samples[].process] | unique | sort) == [0, 1, 2]
          and ([.runtime_samples[].process] | unique | sort) == [0, 1, 2]
          and ((.build_samples | group_by(.process) | map(length)) == [9, 9, 9])
          and ((.runtime_samples | group_by(.process) | map(length)) == [9, 9, 9])
          and all(.build_samples[]; .repetition >= 0 and .repetition < 9 and .compile_time_ns > 0 and .link_time_ns > 0 and .build_end_to_end_ns > 0 and .debug_bytes > 0 and .stripped_bytes > 0 and .debug_bytes >= .stripped_bytes and (.object_sha256 | test("^sha256:[0-9a-f]{64}$")))
          and all(.runtime_samples[]; .repetition >= 0 and .repetition < 9 and .duration_ns > 0 and .operations > 0)
          and (.product.debug_sha256 | test("^sha256:[0-9a-f]{64}$"))
          and (.product.stripped_sha256 | test("^sha256:[0-9a-f]{64}$"))
          and .product.debug_bytes > 0
          and .product.stripped_bytes > 0
          and .product.debug_bytes >= .product.stripped_bytes
          and .product.text_bytes > 0
          and .product.reproducible_builds == true
          and ((.dimensions | keys_unsorted) == ["compile_time_ns", "link_time_ns", "build_end_to_end_ns", "code_size_bytes", "startup_ns", "throughput_ops_per_second", "latency_us", "allocation_count", "allocated_bytes", "peak_memory_bytes", "retain_operations", "release_operations", "pause_time_ns"])
          and .memory.source == "NATIVE-AOT-MEM-001"
          and .memory.sample_count == 27
          and all(.memory | to_entries[]; .key == "source" or .key == "sample_count" or (.value | .median > 0 and .p95 >= .median and .p99 >= .p95))
          and all(.dimensions | to_entries[]; .value | .median > 0 and .p95 >= .median and .p99 >= .p95)
      )
      and ([.. | objects | keys[] | select(. == "pid" or . == "path" or . == "timestamp" or . == "ambient_environment")] | length == 0)
    ' "$report" >/dev/null || die "performance report is incomplete or divergent"
    ! grep -Fq "$root" "$report" || die "performance report leaked a physical workspace path"
fi

echo "native AOT performance: OK (contract, 3x9x3 protocol, complete product dimensions including end-to-end build time and fail-closed frontier)"
