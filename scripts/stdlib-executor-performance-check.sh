#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_EXECUTOR_PERF_CONTRACT:-$root/testing/stdlib-executor-performance.json}"

die() {
    echo "std.executor performance: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing performance contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "performance contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "performance contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-executor-performance/1"
  and .edition == "0.1"
  and .phase == "STD-0.1B"
  and .task == "STD-EXEC-PERF-001"
  and .owner == "std.executor"
  and (.status == "contract-locked" or .status == "verified-hosted-vm-and-native-token-x86_64-linux")
  and (.targets | keys | sort) == ["hosted_vm", "native_runtime"]
  and .targets.hosted_vm == {
      target: "tondo-vm-hosted",
      backend: "bytecode-vm",
      profile: "test",
      probe: .targets.hosted_vm.probe
    }
  and (.targets.hosted_vm.probe.path == "crates/tondo-vm/src/runtime/execute.rs")
  and (.targets.hosted_vm.probe.test == "runtime::execute::tests::executor_performance::executor_performance_probe")
  and (.targets.hosted_vm.probe.sha256 | test("^[0-9a-f]{64}$"))
  and .targets.native_runtime == {
      target: "x86_64-unknown-linux-gnu",
      backend: "native-runtime-private-token-lane",
      profile: "test",
      probe: .targets.native_runtime.probe
    }
  and (.targets.native_runtime.probe.path == "crates/tondo-native-runtime/src/lib.rs")
  and (.targets.native_runtime.probe.test == "tests::native_blocking_performance_probe")
  and (.targets.native_runtime.probe.sha256 | test("^[0-9a-f]{64}$"))
  and .protocol == {
      clock: "monotonic",
      warmup_iterations: 3,
      measurement_repetitions: 9,
      independent_processes: 3,
      minimum_sample_count: 27,
      batch_operations: 1,
      deterministic_seed: "tondo-stdlib-executor-perf-0.1",
      outliers: "report-not-delete",
      fixture_setup: "excluded-from-operation-latency; included-in-logical-memory-and-worker-start-counters"
    }
  and (.identity_fields | sort) == [
      "backend", "flags", "git_revision", "probe_sha256", "profile",
      "suite", "target", "toolchain", "workload_id"
    ]
  and (.forbidden_identity | sort) == [
      "ambient_environment", "cpu_frequency", "path", "pid", "timestamp"
    ]
  and (.metrics | sort) == [
      "active_peak", "backpressure", "latency", "live_handles",
      "logical_memory_bytes", "queued_peak", "scheduling", "startup",
      "tail_latency", "throughput", "wakeup_bridge"
    ]
  and (.workloads | length) == 12
  and ([.workloads[].id] | unique | length) == 12
  and ([.workloads[] | select(.target == "hosted-vm")] | length) == 6
  and ([.workloads[] | select(.target == "native-runtime")] | length) == 6
  and all(.workloads[];
      (.id | test("^[a-z0-9]+(-[a-z0-9]+)+$"))
      and (.target == "hosted-vm" or .target == "native-runtime")
      and (.operation == "startup" or .operation == "roundtrip" or .operation == "throughput" or .operation == "saturation" or .operation == "drain")
      and (.workers == 1 or .workers == 4)
      and (.capacity > 0)
      and (.operations > 0)
      and (if .operation == "startup" then .operations == 1
           elif .operation == "roundtrip" then (.operations == 1 or .operations == 4)
           elif .operation == "throughput" then .workers == 4 and .capacity == 32 and .operations == 32
           elif .operation == "saturation" then .workers == 1 and .capacity == 1 and .operations == 8
           else .workers == 4 and .capacity == 8 and .operations == 8
           end)
    )
  and .strategy.hosted_vm == "actual-child-engine-blocking-job-and-owner-bridge"
  and .strategy.native_runtime == "private-opaque-token-worker-lane-x86_64-linux"
  and .strategy.native_aot == "not-claimed"
  and .strategy.aggregation == "target-and-backend-separated"
  and .strategy.wakeup_measurement == "owner-wait-and-completion-observations-not-kernel-wakeup-counts"
  and .invariants.operations == "stable-per-workload-across-all-27-samples"
  and .invariants.admission == "accepted-equals-declared-operations-and-pending-is-explicit"
  and .invariants.worker_limit == "active-peak-never-exceeds-workers"
  and .invariants.queue_limit == "queued-peak-never-exceeds-capacity"
  and .invariants.wakeup_bridge == "wait-and-completion-counters-are-observations-not-kernel-wakeup-claims"
  and .invariants.memory == "logical-state-worker-and-envelope-capacity; allocator-overhead-and-rss-excluded"
  and .invariants.cleanup == "hosted-bridge-closes-and-native-live-handles-return-to-zero"
  and .invariants.quantiles == "median-less-than-or-equal-p95-less-than-or-equal-p99"
  and .invariants.target_isolation == "hosted-vm-and-native-token-lane-are-never-aggregated"
  and .invariants.aot_boundary == "native-aot-callable-lowering-and-public-abi-remain-not-claimed"
  and (.oracle.source | index("crates/tondo-reliability/src/executor_model.rs")) != null
  and (.oracle.source | index("crates/tondo-reliability/tests/models.rs")) != null
  and .oracle.command == "cargo test -p tondo-reliability --test models executor_model_sequences_are_bounded_replayable_and_cleanup_complete --locked"
  and .oracle.kind == "independent-bounded-executor-model-and-runtime-invariant-checks"
  and .report == "target/reliability/evidence/stdlib-executor-performance.json"
' "$contract" >/dev/null || die "invalid machine-readable performance contract"

for target in hosted_vm native_runtime; do
    probe_path="$root/$(jq -r ".targets.${target}.probe.path" "$contract")"
    [[ -f "$probe_path" ]] || die "missing ${target} probe: ${probe_path#"$root"/}"
    expected_sha="$(jq -r ".targets.${target}.probe.sha256" "$contract")"
    actual_sha="$(sha256sum "$probe_path" | cut -d' ' -f1)"
    [[ "$actual_sha" == "$expected_sha" ]] || die "${target} probe hash mismatch"
done

[[ -f "$root/crates/tondo-vm/src/runtime/executor_performance.rs" ]] \
    || die "missing hosted performance helper module"
grep -Fq 'mod executor_performance' "$root/crates/tondo-vm/src/runtime/execute.rs" \
    || die "missing hosted performance module anchor"
grep -Fq 'executor_performance_probe' "$root/crates/tondo-vm/src/runtime/executor_performance.rs" \
    || die "missing hosted performance probe anchor"
grep -Fq 'native_blocking_performance_probe' "$root/crates/tondo-native-runtime/src/lib.rs" \
    || die "missing native performance probe anchor"
grep -Fq 'BlockingExecutionBridge::new' "$root/crates/tondo-vm/src/runtime/executor_performance.rs" \
    || die "hosted probe does not exercise the blocking bridge"
grep -Fq 'tondo_rt_blocking_pool_submit' "$root/crates/tondo-native-runtime/src/lib.rs" \
    || die "native blocking submission anchor is missing"

for script in \
    scripts/stdlib-executor-performance-check.sh \
    scripts/stdlib-executor-performance-test.sh \
    scripts/stdlib-executor-performance.sh; do
    [[ -x "$root/$script" ]] || die "performance script is not executable: $script"
done

echo "std.executor performance contract: OK (hosted VM and private native token lane remain target-separated)"
