#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
contract="${TONDO_PERFORMANCE_CONTRACT:-$root/testing/performance.json}"

die() {
    echo "performance contract: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing contract: ${contract#"$root"/}"

jq -e '
    def unique_values: length == (unique | length);

    .dimensions as $dimensions
    | .workload_classes as $classes
    | .workloads as $workloads
    | .backends as $backends
    | ($dimensions | map(.id)) as $dimension_ids
    | ($classes | map(.id)) as $class_ids
    | ($workloads | map(.id)) as $workload_ids
    | .format == "tondo-performance/1"
    and .edition == "0.1"
    and .phase == "PERF-001"
    and .status == "design-locked"
    and .purpose == "baseline-before-native-lowering"
    and .contract == "docs/contracts/performance.md"
    and .stdlib_contract == "testing/stdlib-performance.json"
    and .measurement.clock == "monotonic"
    and .measurement.warmup_iterations == 3
    and .measurement.measurement_repetitions == 9
    and .measurement.independent_processes == 3
    and .measurement.minimum_sample_count == 27
    and .measurement.summary == ["median", "p95", "p99"]
    and .measurement.outliers == "report-not-delete"
    and .measurement.clean_workspace == true
    and .measurement.pinned_toolchain == true
    and .measurement.deterministic_seed == "tondo-perf-0.1"
    and .environment.required_recorded == [
        "cpu_model", "cpu_features", "memory_bytes", "os", "kernel",
        "target", "backend", "profile", "rustc", "llvm", "cargo",
        "flags", "git_revision"
    ]
    and .environment.identity_fields == [
        "suite", "workload_id", "fixture_sha256", "target", "backend",
        "profile", "toolchain", "flags", "git_revision"
    ]
    and .environment.forbidden_identity == [
        "ambient_environment", "cpu_frequency", "pid", "path", "timestamp"
    ]
    and ($dimension_ids == [
        "compile_time", "code_size", "startup", "throughput", "latency",
        "allocation_count", "allocated_bytes", "peak_memory",
        "retain_operations", "release_operations", "pause_time"
    ])
    and all($dimensions[];
        (.unit | type == "string" and length > 0)
        and (.direction == "lower-is-better" or .direction == "higher-is-better")
        and (.max_regression_basis_points >= 0 and .max_regression_basis_points <= 1000)
        and (.planes | length > 0 and unique_values)
        and all(.planes[]; ["compile", "runtime"] | index(.) != null)
        and (.id != "allocation_count" or .max_regression_basis_points == 0)
    )
    and ($class_ids == ["adversarial", "empty", "large", "representative", "small"])
    and all($classes[]; .required == true and (.description | type == "string" and length > 0))
    and ($workload_ids == [
        "compile-empty", "compile-generic", "compile-suspension", "compile-adversarial",
        "runtime-std-core", "runtime-std-collections", "runtime-std-text",
        "runtime-std-codecs", "runtime-std-io", "runtime-async", "runtime-process",
        "runtime-memory-large", "runtime-bytes-small", "runtime-adversarial-overflow"
    ])
    and ($workload_ids | unique_values)
    and all($workloads[];
        .required == true
        and (.id | test("^[a-z0-9]+(-[a-z0-9]+)+$"))
        and (.plane | ["compile", "runtime"] | index(.) != null)
        and (.suite | type == "string" and length > 0)
        and (.class as $class | any($class_ids[]; . == $class))
        and (.fixture_path | test("^[^/][^\\\\]*\\.to$"))
        and (.fixture_sha256 | test("^[0-9a-f]{64}$"))
        and (.backend_scope | length > 0 and unique_values)
        and all(.backend_scope[]; ["compiler-reference", "vm-hosted"] | index(.) != null)
        and (.dimensions | length > 0 and unique_values)
        and all(.dimensions[]; . as $id | any($dimension_ids[]; . == $id))
        and (.bounds.source_bytes > 0 and .bounds.steps > 0 and .bounds.memory_bytes > 0
             and .bounds.output_bytes > 0 and .bounds.duration_ms > 0)
        and (.bounds.source_bytes <= 1048576 and .bounds.steps <= 50000000
             and .bounds.memory_bytes <= 536870912 and .bounds.output_bytes <= 16777216
             and .bounds.duration_ms <= 30000)
        and (if .plane == "compile" then
            (.backend_scope == ["compiler-reference"])
            and ((.dimensions | index("compile_time")) != null)
        else
            (.backend_scope == ["vm-hosted"])
            and ((.dimensions | index("throughput")) != null or .class == "adversarial")
        end)
    )
    and ($backends | map(.id)) == ["vm-hosted", "native"]
    and $backends[0].status == "baseline-required"
    and $backends[0].oracle == "language-observables-and-instrumented-counters"
    and $backends[1].status == "candidates-by-NATIVE-001-fast-lane-capture-deferred"
    and $backends[1].oracle == "must-match-vm-hosted-before-comparison"
    and .oracle_policy.compile == "canonical-interface-artifact-and-diagnostic-observations"
    and .oracle_policy.runtime == "exact-language-observables-before-performance"
    and .oracle_policy.instrumented_counters == "harness-only-not-language-semantics"
    and .oracle_policy.native_comparison == "semantic-equivalence-first"
    and .oracle_policy.mismatch == "fail-the-performance-gate"
    and .baseline.status == "capture-required-before-native-lowering"
    and .baseline.report == "target/reliability/evidence/performance-baseline.json"
    and .baseline.numbers == "not-present-in-design-contract"
    and .baseline.identity_must_match == true
    and .baseline.capture_before == ["NATIVE-ABI-001", "optimization-promotion"]
    and .regression_policy.same_identity_only == true
    and .regression_policy.same_workload_hash == true
    and .regression_policy.same_target_backend_profile == true
    and .regression_policy.no_cross_target_aggregation == true
    and .regression_policy.improvement_does_not_cancel_regression == true
    and .regression_policy.allocation_count_change_requires_review == true
    and .regression_policy.budget_override == "stricter-only-unless-new-reviewed-baseline"
    and .regression_policy.unexplained_regression == "fail"
    and (.gate_sequence | map(.id)) == ["design", "capture", "compare", "promote"]
    and .gate_sequence[0].requires == ["workload-identities", "bounds", "dimension-budgets", "oracle"]
    and .gate_sequence[1].requires == ["clean-workspace", "pinned-toolchain", "recorded-environment", "repeated-samples"]
    and .gate_sequence[2].requires == ["exact-oracle-equivalence", "same-identity", "applicable-budgets", "no-overflow-of-bounds"]
    and .gate_sequence[3].requires == ["reviewed-baseline", "reproducible-report", "no-unexplained-regression", "ci-evidence"]
    and .next_blocks == ["DIAG-NATIVE-001"]
' "$contract" >/dev/null || die "invalid machine-readable contract"

while IFS=$'\t' read -r fixture_path expected_sha; do
    [[ -n "$fixture_path" && -n "$expected_sha" ]] || die "empty fixture record"
    fixture="$root/$fixture_path"
    [[ -f "$fixture" ]] || die "missing fixture: $fixture_path"
    actual_sha="$(sha256sum "$fixture" | cut -d' ' -f1)"
    [[ "$actual_sha" == "$expected_sha" ]] || die "fixture hash mismatch: $fixture_path"
done < <(jq -r '.workloads[] | [.fixture_path, .fixture_sha256] | @tsv' "$contract")

echo "performance contract: OK (14 hash-pinned workloads; candidate fast lane, full native capture deferred)"
