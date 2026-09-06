#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_YAML_PERF_CONTRACT:-$root/testing/stdlib-yaml-performance.json}"

die() {
    echo "std.yaml performance contract: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing performance contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "performance contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "performance contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-yaml-performance/1"
  and .edition == "0.1"
  and .phase == "STD-0.1B"
  and .task == "STD-YAML-PERF-001"
  and .owner == "std.yaml"
  and .status == "pending-evidence"
  and .target == "tondo-vm-hosted"
  and .backend == "bytecode-vm"
  and .profile == "test"
  and (.probe.path | type == "string" and length > 0)
  and .probe.test == "process_host::tests::yaml_performance_probe"
  and (.probe.sha256 | test("^[0-9a-f]{64}$"))
  and .protocol == {
    clock: "monotonic",
    warmup_iterations: 3,
    measurement_repetitions: 9,
    independent_processes: 3,
    minimum_sample_count: 27,
    batch_operations: 16,
    deterministic_seed: "tondo-stdlib-yaml-perf-0.1",
    outliers: "report-not-delete",
    fixture_setup: "excluded-from-timed-latency; included-in-allocation-and-memory-counters"
  }
  and ([.identity_fields[]] | sort) == [
    "backend", "flags", "git_revision", "probe_sha256", "profile",
    "suite", "target", "toolchain", "workload_id"
  ]
  and ([.forbidden_identity[]] | sort) == [
    "ambient_environment", "cpu_frequency", "path", "pid", "timestamp"
  ]
  and ([.metrics[]] | sort) == [
    "adversarial_rejection", "aliases", "allocations", "bytes_copied",
    "depth", "expanded_nodes", "latency", "live_handles",
    "logical_memory_bytes", "tail_latency", "throughput"
  ]
  and ([.workloads[].id] | sort) == [
    "encode-canonical-core",
    "encode-nested-medium",
    "parse-alias-expanded",
    "parse-all-documents",
    "parse-block-scalars",
    "parse-core-small",
    "parse-nested-medium",
    "parse-view-nested",
    "stream-reader-events",
    "stream-writer-events",
    "validate-alias-limit",
    "validate-depth-limit",
    "validate-syntax-reject"
  ]
  and (.workloads | length) == 13
  and ([.workloads[].id] | unique | length) == 13
  and all(.workloads[];
    (.id | test("^[a-z0-9]+(-[a-z0-9]+)+$"))
    and (.operation == "materialized-parse"
      or .operation == "materialized-parse-all"
      or .operation == "borrowed-parse-view"
      or .operation == "adversarial-reject"
      or .operation == "materialized-encode"
      or .operation == "canonical-encode"
      or .operation == "stream-reader-events"
      or .operation == "stream-writer-events")
    and (.shape == "core" or .shape == "nested" or .shape == "alias"
      or .shape == "block-scalars" or .shape == "documents"
      or .shape == "alias-limit" or .shape == "depth-limit"
      or .shape == "syntax-reject")
    and (.size_class == "small" or .size_class == "medium")
    and (.payload_bytes | type == "number" and . >= 0 and . <= 4096)
    and (.chunk_bytes | type == "number" and . >= 0 and . <= 4096)
    and (.depth | type == "number" and . >= 1 and . <= 256)
    and (.aliases | type == "number" and . >= 0 and . <= 65536)
    and (.expanded_nodes | type == "number" and . >= 0 and . <= 4194304)
    and (.expected_error == null or (.expected_error | type == "string" and length > 0))
    and (.dispatch == "scalar-fixed-target")
    and (if .operation == "adversarial-reject" then .expected_error != null else .expected_error == null end)
    and (if .operation == "stream-reader-events" then .chunk_bytes == 1 else true end)
  )
  and .strategy.hosted_vm == "scalar-kernel-host-bridge-baseline"
  and .strategy.native_runtime_abi == "not-measured-by-this-hosted-report"
  and .strategy.native_aot == "not-claimed"
  and .strategy.simd == "not-measured-no-optimized-route"
  and .strategy.selection == "hosted-scalar-baseline"
  and (.strategy.selection_reason | type == "string" and length > 0)
  and .dispatch == {
    mode: "scalar-only",
    selection: "target-declared-and-workload-size",
    selected_route: "scalar",
    multiversion: "not-claimed",
    size_classes: ["small", "medium"]
  }
  and .invariants.operations == "stable-per-workload-across-all-27-samples"
  and .invariants.bytes_copied == "host-input-plus-materialized-value-or-output; event-payload-bytes-counted-once"
  and .invariants.allocations == "logical-host-values-including-fixture; allocator-overhead-excluded"
  and .invariants.logical_memory == "host-registry-YamlValue-Bytes-and-stream-output; RSS-and-allocator-overhead-excluded"
  and .invariants.live_handles == "zero-YamlReader-and-YamlWriter-handles-before-probe-return"
  and .invariants.depth == "declared-structural-depth-of-each-bounded-fixture"
  and .invariants.aliases == "declared-source-aliases-or-rejection-at-alias-budget"
  and .invariants.expanded_nodes == "declared-bounded-materialized-value-nodes; rejected-expansions-report-zero"
  and .invariants.adversarial_rejection == "invalid-input-and-limit-workloads-reject-on-every-operation-without-partial-value"
  and .invariants.dispatch == "every-sample-reports-scalar-fixed-target"
  and .invariants.target_isolation == "reports-never-combine-targets-or-backends"
  and .invariants.oracle == "independent-bounded-YAML-model-plus-exact-host-output-and-error-checks"
  and .oracle.kind == "independent-bounded-yaml-model-and-host-invariant-checks"
  and (.oracle.sources | type == "array" and length == 2)
  and (.oracle.sources | index("crates/tondo-reliability/src/yaml_model.rs")) != null
  and (.oracle.sources | index("crates/tondo-reliability/tests/yaml_models.rs")) != null
  and .report == "target/reliability/evidence/stdlib-yaml-performance.json"
' "$contract" >/dev/null || die "invalid machine-readable YAML performance contract"

probe_path="$root/$(jq -r '.probe.path' "$contract")"
[[ -f "$probe_path" ]] || die "missing probe: ${probe_path#"$root"/}"
expected_probe_sha="$(jq -r '.probe.sha256' "$contract")"
actual_probe_sha="$(sha256sum "$probe_path" | cut -d' ' -f1)"
[[ "$actual_probe_sha" == "$expected_probe_sha" ]] || die "probe hash mismatch"

for path in \
    docs/contracts/stdlib-yaml-performance.md \
    docs/contracts/stdlib-yaml.md \
    docs/contracts/stdlib-yaml-test.md \
    TONDO_STANDARD_LIBRARY_SPEC.md \
    TONDO_LANGUAGE_SPEC.md \
    TONDO_IMPLEMENTATION_TRACKER.md \
    testing/stdlib-yaml.json \
    testing/stdlib-yaml-test.json \
    testing/inventory.json \
    testing/coverage-matrix.json; do
    [[ -f "$root/$path" ]] || die "missing linked path: $path"
done

for path in \
    scripts/stdlib-yaml-performance-check.sh \
    scripts/stdlib-yaml-performance-test.sh \
    scripts/stdlib-yaml-performance.sh; do
    [[ -x "$root/$path" ]] || die "runner is not executable: $path"
done

for marker in \
    'yaml_performance_probe' \
    'YAML_PERF_BATCH' \
    'yaml_performance_logical_bytes' \
    'yaml_performance_live_handles' \
    'TONDO_YAML_PERF'; do
    grep -Fq "$marker" "$probe_path" || die "probe misses anchor: $marker"
done

jq -e '
  .performance.task == "STD-YAML-PERF-001"
  and .performance.contract == "testing/stdlib-yaml-performance.json"
  and .performance.document == "docs/contracts/stdlib-yaml-performance.md"
  and .performance.status == "pending-evidence"
  and .performance.target == "tondo-vm-hosted"
  and .performance.native_aot == "not-claimed"
  and .performance.workloads == 13
  and .performance.samples_per_workload == 27
  and .performance.dispatch_mode == "scalar-only"
  and .performance.required_measurements == [
    "throughput", "tail-latency", "allocations", "bytes-copied", "depth",
    "alias-expansion", "adversarial-rejection"
  ]
  and .implementation.native_aot_lowering == "not-claimed"
  and .promotion.next_blocks == ["STD-YAML-PERF-001"]
' testing/stdlib-yaml.json >/dev/null || die "parent YAML registry has a stale performance frontier"

jq -e '
  .promotion.next_blocks == ["STD-YAML-PERF-001"]
  and .promotion.implementation_pending == []
' testing/stdlib-yaml-test.json >/dev/null || die "YAML testing registry has a stale promotion frontier"

for marker in \
    'STD-YAML-PERF-001' \
    'scalar-kernel-host-bridge-baseline' \
    'logical memory' \
    'adversarial' \
    'tail latency' \
    'native AOT' \
    'not-claimed' \
    'stdlib-yaml-performance.json'; do
    grep -Fq "$marker" docs/contracts/stdlib-yaml-performance.md \
        || die "performance document misses marker: $marker"
done
grep -Fq 'stdlib-yaml-performance.json' TONDO_STANDARD_LIBRARY_SPEC.md \
    || die "stdlib spec does not link the YAML performance contract"
grep -Fq 'stdlib-yaml-performance.md' docs/contracts/stdlib-yaml.md \
    || die "YAML document does not link the performance contract"
grep -Fq 'STD-YAML-PERF-001' TONDO_IMPLEMENTATION_TRACKER.md \
    || die "tracker does not record the YAML performance leaf"

echo "std.yaml performance contract: OK (hosted scalar baseline; 13 workloads; adversarial boundary explicit)"
