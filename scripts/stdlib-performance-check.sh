#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
contract="$root/testing/stdlib-performance.json"

if [[ ! -f "$contract" ]]; then
    echo "missing standard-library performance contract: ${contract#"$root"/}" >&2
    exit 1
fi

jq -e '
    def unique_values: length == (unique | length);

    .dimensions as $dimensions
    | .workload_classes as $workloads
    | ($dimensions | map(.id)) as $dimension_ids
    | ($workloads | map(.id)) as $workload_ids
    | .format == "tondo-stdlib-performance/1"
    and .edition == "0.1"
    and .phase == "STD-0.1A"
    and .status == "draft-contract"
    and .measurement.clock == "monotonic"
    and .measurement.warmup_iterations == 3
    and .measurement.measurement_repetitions == 9
    and .measurement.independent_processes == 3
    and .measurement.summary == ["median", "p95", "p99"]
    and .measurement.outliers == "report-not-delete"
    and .measurement.minimum_sample_count == 27
    and .measurement.recorded_environment == [
        "cpu_model", "cpu_features", "memory_bytes", "os", "kernel",
        "target", "backend", "profile", "rustc", "llvm", "cargo",
        "flags", "git_revision"
    ]
    and (.dimensions | map(.id)) == [
        "allocations_per_operation", "allocated_bytes_per_operation", "code_size",
        "compile_time", "peak_memory", "startup", "tail_latency", "throughput"
    ]
    and all(.dimensions[]; (.direction == "lower-is-better" or .direction == "higher-is-better")
        and (.max_regression_basis_points >= 0 and .max_regression_basis_points <= 1000)
        and (.id != "allocations_per_operation" or .max_regression_basis_points == 0))
    and (.workload_classes | map(.id)) == [
        "adversarial", "empty", "fragmented_stream", "large", "representative", "small"
    ]
    and all(.workload_classes[]; .required == true and (.description | length) > 0)
    and .oracle_policy.required == true
    and .oracle_policy.implementation == "portable-scalar"
    and .oracle_policy.comparison == "exact-observation"
    and .oracle_policy.observables == [
        "allocation-contract", "bytes", "error", "error-path", "ordering",
        "overflow", "ownership", "value"
    ]
    and .oracle_policy.optimized_route == "must-fallback-to-oracle-compatible-portable-route"
    and .kernel_policy.allowed == [
        "automatic-vectorization", "lookup-table", "simd", "specialization",
        "target-multiversioning", "word-at-a-time"
    ]
    and .kernel_policy.forbidden == [
        "different-error-order", "semantic-difference", "silent-overflow-change",
        "target-required-without-fallback", "unbounded-probing"
    ]
    and (.owner_groups | map(.id)) == ["a0-foundation", "a1-values-protocols", "a2-host", "a3-codecs", "a4-testing"]
    and all(.owner_groups[]; . as $group
        | ($group.owners | length) > 0
        and ($group.owners == ($group.owners | sort | unique))
        and ($group.required_dimensions | length) > 0
        and ($group.required_workloads | length) > 0
        and ($group.required_dimensions | unique_values)
        and ($group.required_workloads | unique_values)
        and all($group.required_dimensions[]; . as $id | any($dimension_ids[]; . == $id))
        and all($group.required_workloads[]; . as $id | any($workload_ids[]; . == $id)))
    and (.gate_sequence | map(.id)) == ["design", "capture", "compare", "promote"]
    and .gate_sequence[0].requires == ["owner-contract", "scalar-oracle", "workload-identities"]
    and .gate_sequence[1].requires == ["clean-workspace", "pinned-toolchain", "recorded-environment", "repeated-samples"]
    and .gate_sequence[2].requires == ["all-applicable-targets", "exact-oracle-equivalence", "regression-budgets"]
    and .gate_sequence[3].requires == ["no-unexplained-regression", "reproducible-report", "reviewed-baseline"]
' "$contract" >/dev/null

echo "stdlib performance contract: OK"
