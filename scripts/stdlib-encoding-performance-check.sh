#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_ENCODING_PERF_CONTRACT:-$root/testing/stdlib-encoding-performance.json}"

die() {
    echo "std.encoding performance contract: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing performance contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "performance contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "performance contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-encoding-performance/1"
  and .edition == "0.1"
  and .phase == "STD-0.1B"
  and .task == "STD-ENCODING-PERF-001"
  and .owner == "std.encoding"
  and .status == "verified-hosted-vm-baseline"
  and .target == "tondo-vm-hosted"
  and .backend == "bytecode-vm"
  and .profile == "test"
  and (.probe.path | type == "string" and length > 0)
  and .probe.test == "process_host::tests::encoding_performance_probe"
  and (.probe.sha256 | test("^[0-9a-f]{64}$"))
  and .protocol == {
    clock: "monotonic",
    warmup_iterations: 3,
    measurement_repetitions: 9,
    independent_processes: 3,
    minimum_sample_count: 27,
    batch_operations: 64,
    deterministic_seed: "tondo-stdlib-encoding-perf-0.1",
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
    "allocations", "bytes_copied", "dispatch", "latency",
    "live_handles", "logical_memory_bytes", "tail_latency", "throughput"
  ]
  and ([.workloads[].id] | sort) == [
    "base64-standard-decode-small",
    "base64-standard-encode-empty",
    "base64-standard-encode-large",
    "base64-standard-encode-quantum",
    "base64-standard-encode-small",
    "base64-standard-stream-encode-large",
    "base64-stream-decode-quantum",
    "base64-stream-encode-byte",
    "base64-url-encode-large",
    "base64-url-unpadded-decode-large",
    "hex-any-decode-large",
    "hex-lower-encode-large",
    "hex-lower-encode-small",
    "hex-stream-decode-nibble",
    "hex-stream-encode-byte",
    "hex-upper-decode-small"
  ]
  and (.workloads | length) == 16
  and ([.workloads[].id] | unique | length) == 16
  and all(.workloads[];
    (.id | test("^[a-z0-9]+(-[a-z0-9]+)+$"))
    and (.codec == "base64" or .codec == "hex")
    and (.operation == "materialized-encode"
      or .operation == "materialized-decode"
      or .operation == "stream-encode"
      or .operation == "stream-decode")
    and (.policy == "standard" or .policy == "url-safe"
      or .policy == "url-safe-unpadded" or .policy == "lower"
      or .policy == "upper" or .policy == "any-case")
    and (.size_class == "empty" or .size_class == "quantum"
      or .size_class == "small" or .size_class == "large")
    and (.payload_bytes | type == "number" and . >= 0 and . <= 4096)
    and (.chunk_bytes | type == "number" and . >= 0 and . <= 4096)
    and (.dispatch == "scalar-fixed-target")
    and (if .operation | startswith("materialized-") then .chunk_bytes == 0 else .chunk_bytes > 0 end)
  )
  and all(.workloads[] | select(.codec == "base64");
    .policy == "standard" or .policy == "url-safe" or .policy == "url-safe-unpadded"
  )
  and all(.workloads[] | select(.codec == "hex");
    .policy == "lower" or .policy == "upper" or .policy == "any-case"
  )
  and .strategy.hosted_vm == "scalar-kernel-host-bridge-baseline"
  and .strategy.native_runtime_abi == "not-measured-by-this-hosted-report"
  and .strategy.native_aot == "not-claimed"
  and .strategy.simd == "not-measured-no-optimized-route"
  and .strategy.multiversion_dispatch == "scalar-fixed-target; size classes reported; optimized dispatch not claimed"
  and .strategy.selection == "hosted-scalar-baseline"
  and (.strategy.selection_reason | type == "string" and length > 0)
  and .dispatch == {
    mode: "scalar-only",
    selection: "target-declared-and-size-class",
    selected_route: "scalar",
    multiversion: "not-claimed",
    size_classes: ["empty", "quantum", "small", "large"]
  }
  and .invariants.operations == "stable-per-workload-across-all-27-samples"
  and .invariants.bytes_copied == "bridge-input-plus-published-output; stream chunks counted once"
  and .invariants.logical_memory == "host-value-registry-and-Bytes-capacity; RSS-and-allocator-overhead-excluded"
  and .invariants.cleanup == "no-live-encoding-handle-before-probe-return"
  and .invariants.dispatch == "every-sample-reports-scalar-fixed-target"
  and .invariants.target_isolation == "reports-never-combine-targets-or-backends"
  and .invariants.oracle == "independent-bounded-encoding-model-plus-exact-host-output-checks"
  and .oracle.kind == "independent-bounded-encoding-model-and-host-invariant-checks"
  and (.oracle.sources | type == "array" and length == 2)
  and (.oracle.sources | index("crates/tondo-reliability/src/encoding_model.rs")) != null
  and (.oracle.sources | index("crates/tondo-reliability/tests/encoding_models.rs")) != null
  and .report == "target/reliability/evidence/stdlib-encoding-performance.json"
' "$contract" >/dev/null || die "invalid machine-readable encoding performance contract"

probe_path="$root/$(jq -r '.probe.path' "$contract")"
[[ -f "$probe_path" ]] || die "missing probe: ${probe_path#"$root"/}"
expected_probe_sha="$(jq -r '.probe.sha256' "$contract")"
actual_probe_sha="$(sha256sum "$probe_path" | cut -d' ' -f1)"
[[ "$actual_probe_sha" == "$expected_probe_sha" ]] || die "probe hash mismatch"

for path in docs/contracts/stdlib-encoding-performance.md docs/contracts/stdlib-encoding.md TONDO_STANDARD_LIBRARY_SPEC.md TONDO_LANGUAGE_SPEC.md TONDO_IMPLEMENTATION_TRACKER.md testing/stdlib-encoding.json testing/stdlib-encoding-test.json testing/inventory.json testing/coverage-matrix.json; do
    [[ -f "$root/$path" ]] || die "missing linked path: $path"
done

for path in scripts/stdlib-encoding-performance-check.sh scripts/stdlib-encoding-performance-test.sh scripts/stdlib-encoding-performance.sh; do
    [[ -x "$root/$path" ]] || die "runner is not executable: $path"
done

for marker in 'encoding_performance_probe' 'ENCODING_PERF_BATCH' 'encoding_performance_logical_bytes' 'encoding_performance_live_handles' 'TONDO_ENCODING_PERF'; do
    grep -Fq "$marker" "$probe_path" || die "probe misses anchor: $marker"
done

jq -e '
  .performance.task == "STD-ENCODING-PERF-001"
  and .performance.contract == "testing/stdlib-encoding-performance.json"
  and .performance.document == "docs/contracts/stdlib-encoding-performance.md"
  and .performance.status == "verified-hosted-vm-baseline"
  and .performance.target == "tondo-vm-hosted"
  and .performance.native_aot == "not-claimed"
  and .performance.workloads == 16
  and .performance.samples_per_workload == 27
  and .performance.dispatch_mode == "scalar-only"
  and .implementation.native_aot_lowering == "not-claimed"
  and .promotion.implementation_pending == []
  and .promotion.next_blocks == ["STD-ENCODING-CONF-001"]
' testing/stdlib-encoding.json >/dev/null || die "parent encoding registry has a stale performance frontier"

jq -e '
  .promotion.next_blocks == ["STD-ENCODING-CONF-001"]
  and .promotion.implementation_pending == []
' testing/stdlib-encoding-test.json >/dev/null || die "encoding testing registry has a stale performance frontier"

for marker in 'STD-ENCODING-PERF-001' 'scalar-kernel-host-bridge-baseline' 'logical memory' 'tail latency' 'native AOT' 'not-claimed' 'stdlib-encoding-performance.json'; do
    grep -Fq "$marker" docs/contracts/stdlib-encoding-performance.md ||
        die "performance document misses marker: $marker"
done
grep -Fq 'stdlib-encoding-performance.json' TONDO_STANDARD_LIBRARY_SPEC.md ||
    die "stdlib spec does not link the encoding performance contract"
grep -Fq 'stdlib-encoding-performance.md' docs/contracts/stdlib-encoding.md ||
    die "encoding document does not link the performance contract"
grep -Fq 'STD-ENCODING-PERF-001' TONDO_IMPLEMENTATION_TRACKER.md ||
    die "tracker does not record the encoding performance leaf"

echo "std.encoding performance contract: OK (hosted scalar baseline; 16 workloads; native/SIMD frontier explicit)"
