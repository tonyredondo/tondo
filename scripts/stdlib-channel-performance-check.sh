#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_CHANNEL_PERF_CONTRACT:-$root/testing/stdlib-channel-performance.json}"

die() {
    echo "std.channel performance contract: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing performance contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "performance contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "performance contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-channel-performance/1"
  and .edition == "0.1"
  and .phase == "STD-0.1B"
  and .task == "STD-CHANNEL-PERF-001"
  and .owner == "std.channel"
  and .status == "verified-hosted-vm-baseline"
  and .target == "tondo-vm-hosted"
  and .backend == "bytecode-vm"
  and .profile == "test"
  and (.probe.path | type == "string" and length > 0)
  and (.probe.test | type == "string" and length > 0)
  and (.probe.sha256 | test("^[0-9a-f]{64}$"))
  and .protocol == {
    clock: "monotonic",
    warmup_iterations: 3,
    measurement_repetitions: 9,
    independent_processes: 3,
    minimum_sample_count: 27,
    batch_operations: 16,
    deterministic_seed: "tondo-stdlib-channel-perf-0.1",
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
    "allocations", "backpressure", "latency", "live_handles",
    "logical_memory_bytes", "queued_peak", "tail_latency", "throughput", "wakeups"
  ]
  and (.workloads | length) == 9
  and ([.workloads[].id] | unique | length) == 9
  and all(.workloads[];
    (.id | test("^[a-z0-9]+(-[a-z0-9]+)+$"))
    and (.topology == "1:1" or .topology == "n:1" or .topology == "n:m")
    and (.mode == "rendezvous" or .mode == "buffered" or .mode == "unbounded" or .mode == "backpressure" or .mode == "close-wakeup")
    and (.capacity == null or .capacity == 0 or .capacity == 1 or .capacity == 8)
    and (.producers == 1 or .producers == 8)
    and (.consumers == 1 or .consumers == 4)
  )
  and .strategy.hosted_vm == "scheduler-owned-single-worker-channel-baseline"
  and .strategy.native_runtime_abi == "not-measured-by-this-hosted-report"
  and .strategy.native_aot == "not-claimed"
  and .strategy.algorithmic_fast_paths == "deferred-to-native-targeted-performance-campaign"
  and .strategy.selection == "hosted-baseline-selected-until-native-targets-have-comparable-concurrent-evidence"
  and (.strategy.selection_reason | type == "string" and length > 0)
  and .invariants.operations == "stable-per-workload-across-all-27-samples"
  and .invariants.topologies == "1:1, n:1 and n:m are explicit"
  and .invariants.fifo == "received-values-follow-commit-order"
  and .invariants.backpressure == "blocked-sends-retain-payload-and-wake-exactly-once"
  and .invariants.wakeup == "one-per-completed-pending-waiter"
  and .invariants.memory == "logical-channel-state-and-waiter-capacity; allocator-overhead-excluded"
  and .invariants.cleanup == "no-pending-channel-waiter-or-live-endpoint-before-probe-return"
  and .invariants.oracle == "independent-bounded-channel-model-plus-host-invariant-checks"
  and .invariants.target_isolation == "reports-never-combine-targets-or-backends"
  and .oracle.kind == "independent-bounded-channel-model-and-host-invariant-checks"
  and (.oracle.sources | type == "array" and length == 2)
  and (.oracle.sources | index("crates/tondo-reliability/src/channel_model.rs")) != null
  and (.oracle.sources | index("crates/tondo-reliability/tests/channel_models.rs")) != null
  and .report == "target/reliability/evidence/stdlib-channel-performance.json"
' "$contract" >/dev/null || die "invalid machine-readable channel performance contract"

probe_path="$root/$(jq -r '.probe.path' "$contract")"
[[ -f "$probe_path" ]] || die "missing probe: ${probe_path#"$root"/}"
expected_probe_sha="$(jq -r '.probe.sha256' "$contract")"
actual_probe_sha="$(sha256sum "$probe_path" | cut -d' ' -f1)"
[[ "$actual_probe_sha" == "$expected_probe_sha" ]] || die "probe hash mismatch"

for path in \
    docs/contracts/stdlib-channel-performance.md \
    docs/contracts/stdlib-channel.md \
    TONDO_STANDARD_LIBRARY_SPEC.md \
    TONDO_LANGUAGE_SPEC.md \
    TONDO_IMPLEMENTATION_TRACKER.md \
    testing/stdlib-channel.json \
    testing/stdlib-channel-test.json \
    testing/stdlib-channel-async-iter.json \
    testing/inventory.json \
    testing/coverage-matrix.json; do
    [[ -f "$root/$path" ]] || die "missing linked path: $path"
done

for path in \
    scripts/stdlib-channel-performance-check.sh \
    scripts/stdlib-channel-performance-test.sh \
    scripts/stdlib-channel-performance.sh; do
    [[ -x "$root/$path" ]] || die "runner is not executable: $path"
done

for marker in \
    'channel_performance_probe' \
    'CHANNEL_PERF_BATCH' \
    'channel_performance_logical_bytes' \
    'channel_performance_close_wakeup' \
    'TONDO_CHANNEL_PERF'; do
    grep -Fq "$marker" "$probe_path" || die "probe misses anchor: $marker"
done

jq -e '
  .performance.task == "STD-CHANNEL-PERF-001"
  and .performance.contract == "testing/stdlib-channel-performance.json"
  and .performance.document == "docs/contracts/stdlib-channel-performance.md"
  and .performance.status == "verified-hosted-vm-baseline"
  and .performance.target == "tondo-vm-hosted"
  and .performance.native_aot == "not-claimed"
  and .performance.workloads == 9
  and .performance.samples_per_workload == 27
  and .implementation.algorithmic_fast_paths == "deferred-to-native-targeted-performance-campaign"
  and .implementation.required_follow_ups == ["STD-CHANNEL-DOC-001"]
  and .promotion.implementation_pending == ["STD-CHANNEL-DOC-001"]
  and .promotion.next_blocks == ["STD-CHANNEL-DOC-001"]
' testing/stdlib-channel.json >/dev/null || die "parent channel registry has a stale performance frontier"

jq -e '
  .promotion.next_blocks == ["STD-CHANNEL-DOC-001"]
  and .promotion.implementation_pending == []
' testing/stdlib-channel-test.json >/dev/null || die "channel testing registry has a stale promotion frontier"

jq -e '.promotion.next_blocks == ["STD-CHANNEL-DOC-001"]' \
    testing/stdlib-channel-async-iter.json >/dev/null \
    || die "channel AsyncIterator registry has a stale promotion frontier"

for marker in \
    'STD-CHANNEL-PERF-001' \
    'scheduler-owned-single-worker-channel-baseline' \
    'logical memory' \
    'backpressure' \
    'tail latency' \
    'native AOT' \
    'not-claimed' \
    'stdlib-channel-performance.json'; do
    grep -Fq "$marker" docs/contracts/stdlib-channel-performance.md \
        || die "performance document misses marker: $marker"
done
grep -Fq 'stdlib-channel-performance.json' TONDO_STANDARD_LIBRARY_SPEC.md \
    || die "stdlib spec does not link the channel performance contract"
grep -Fq 'stdlib-channel-performance.md' docs/contracts/stdlib-channel.md \
    || die "channel document does not link the channel performance contract"
grep -Fq 'STD-CHANNEL-PERF-001' TONDO_IMPLEMENTATION_TRACKER.md \
    || die "tracker does not record the channel performance leaf"

echo "std.channel performance contract: OK (hosted baseline; 9 workloads; deferred native fast paths explicit)"
