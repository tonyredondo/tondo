#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_ASYNC_GROUP_PERF_CONTRACT:-$root/testing/stdlib-async-group-performance.json}"
evidence_dir="${TONDO_ASYNC_GROUP_PERF_EVIDENCE_DIR:-$root/target/reliability/evidence}"
target_dir="${CARGO_TARGET_DIR:-target}"

die() {
    echo "async Group performance: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing contract: ${contract#"$root"/}"

jq -e '
    .format == "tondo-stdlib-async-group-performance/1"
    and .edition == "0.1"
    and .phase == "STD-0.1B"
    and .task == "STD-ASYNC-GROUP-PERF-001"
    and .owner == "std.async.group"
    and .status == "verified-hosted-vm"
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
        batch_operations: 1,
        deterministic_seed: "tondo-async-group-perf-0.1",
        outliers: "report-not-delete",
        fixture_setup: "excluded-from-timed-latency; included-in-allocation-and-storage-counters"
    }
    and (.identity_fields | sort) == [
        "backend", "flags", "git_revision", "probe_sha256", "profile",
        "suite", "target", "toolchain", "workload_id"
    ]
    and (.forbidden_identity | sort) == [
        "ambient_environment", "cpu_frequency", "path", "pid", "timestamp"
    ]
    and (.metrics | sort) == [
        "allocation_count", "child_scans", "group_state_bytes", "latency",
        "tail_latency", "throughput", "wakeups"
    ]
    and (.workloads | length == 18)
    and ([.workloads[].id] | unique | length == 18)
    and all(.workloads[];
        (.id | test("^[a-z0-9]+(-[a-z0-9]+)+$"))
        and (.operation == "add" or .operation == "all" or .operation == "settle" or .operation == "next" or .operation == "cancel")
        and (.mode == "ready" or .mode == "pending")
        and (.cardinality == 1 or .cardinality == 8 or .cardinality == 64)
        and ((.operation == "next" and (.mode == "ready" or .mode == "pending")) or (.operation != "next"))
    )
    and .invariants.add_count == "cardinality"
    and .invariants.all_count == "one-ready-poll"
    and .invariants.settle_count == "one-ready-poll"
    and .invariants.next_ready_count == "one-poll"
    and .invariants.next_pending_count == "pending-poll-plus-ready-poll"
    and .invariants.cancel_count == "pending-poll-plus-terminal-drain-poll"
    and .invariants.child_scans == "linear-in-cardinality-per-poll"
    and .invariants.wakeup == "one-per-pending-next-round-trip"
    and .invariants.state_bytes == "logical-size-of-group-state-and-vector-capacity; allocator-overhead-excluded"
    and .invariants.managed_allocations == "reported-from-vm-heap-with-fixture-setup"
    and .invariants.cleanup == "every-measured-group-is-consumed-or-removed-before-probe-return"
    and .report == "target/reliability/evidence/stdlib-async-group-performance.json"
' "$contract" >/dev/null || die "invalid machine-readable performance contract"

probe_path="$root/$(jq -r '.probe.path' "$contract")"
[[ -f "$probe_path" ]] || die "missing probe: ${probe_path#"$root"/}"
expected_probe_sha="$(jq -r '.probe.sha256' "$contract")"
actual_probe_sha="$(sha256sum "$probe_path" | cut -d' ' -f1)"
[[ "$actual_probe_sha" == "$expected_probe_sha" ]] || die "probe hash mismatch"

if [[ "${TONDO_ASYNC_GROUP_PERF_ALLOW_DIRTY:-0}" != 1 ]]; then
    [[ -z "$(git status --porcelain)" ]] || die "workspace must be clean"
fi

mkdir -p "$root/.tmp"
tmp="$(mktemp -d "$root/.tmp/tondo-async-group-performance.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
samples="$tmp/samples.tsv"
: > "$samples"

for process in 1 2 3; do
    log="$tmp/process-$process.log"
    CARGO_TARGET_DIR="$target_dir" cargo test -p tondo-vm --locked \
        "$(jq -r '.probe.test' "$contract")" --lib -- --exact --nocapture \
        > "$log" 2>&1 || {
        cat "$log" >&2
        die "probe failed in independent process $process"
    }
    grep -F $'TONDO_GROUP_PERF\t' "$log" >> "$samples" \
        || die "probe emitted no samples in independent process $process"
done

revision="$(git rev-parse HEAD)"
cpu="$(grep -m1 'model name' /proc/cpuinfo 2>/dev/null | cut -d: -f2- | sed 's/^ *//' || uname -m)"
cpu_features="$(grep -m1 '^Features\|^flags' /proc/cpuinfo 2>/dev/null | cut -d: -f2- | sed 's/^ *//' || true)"
if [[ -z "$cpu_features" ]] && command -v sysctl >/dev/null 2>&1; then
    cpu_features="$(sysctl -n machdep.cpu.features 2>/dev/null || true)"
fi
[[ -n "$cpu_features" ]] || cpu_features="unavailable"

if [[ -r /proc/meminfo ]]; then
    memory_bytes="$(awk '/^MemTotal:/ { print $2 * 1024; exit }' /proc/meminfo)"
elif command -v sysctl >/dev/null 2>&1; then
    memory_bytes="$(sysctl -n hw.memsize 2>/dev/null || printf '0')"
else
    memory_bytes=0
fi
[[ -n "$memory_bytes" ]] || memory_bytes=0

os="$(uname -s)"
kernel="$(uname -r)"
target="$(rustc -vV | awk '/^host:/ { value=$2 } END { print value }')"
rustc_version="$(rustc --version)"
llvm="$(rustc -vV | awk -F': ' '/^LLVM version:/ { value=$2 } END { print value }')"
cargo_version="$(cargo --version)"
flags="${RUSTFLAGS-}"
workload_ids="$(jq -c '[.workloads[].id]' "$contract")"

mkdir -p "$evidence_dir"
jq -Rn \
    --slurpfile contract "$contract" \
    --arg revision "$revision" \
    --arg probe_sha "$actual_probe_sha" \
    --arg cpu "$cpu" \
    --arg cpu_features "$cpu_features" \
    --argjson memory_bytes "$memory_bytes" \
    --arg os "$os" \
    --arg kernel "$kernel" \
    --arg target "$target" \
    --arg rustc "$rustc_version" \
    --arg llvm "$llvm" \
    --arg cargo "$cargo_version" \
    --arg flags "$flags" \
    '
      [inputs
       | split("\t")
       | select(length == 19 and .[0] == "TONDO_GROUP_PERF")
       | {
           workload_id: .[1],
           nanos: (.[2] | tonumber),
           cardinality: (.[3] | tonumber),
           adds: (.[4] | tonumber),
           all_operations: (.[5] | tonumber),
           settle_operations: (.[6] | tonumber),
           next_operations: (.[7] | tonumber),
           cancel_operations: (.[8] | tonumber),
           child_scans: (.[9] | tonumber),
           waits: (.[10] | tonumber),
           wakeups: (.[11] | tonumber),
           cancellation_requests: (.[12] | tonumber),
           managed_allocations: (.[13] | tonumber),
           state_allocations: (.[14] | tonumber),
           child_buffer_grows: (.[15] | tonumber),
           waiter_buffer_grows: (.[16] | tonumber),
           peak_children: (.[17] | tonumber),
           peak_state_bytes: (.[18] | tonumber)
       }] as $rows
      | ($contract[0].workloads) as $workloads
      | ($rows | group_by(.workload_id) | map(
          . as $group
          | ($group[0].workload_id) as $id
          | ($workloads[] | select(.id == $id)) as $spec
          | ($group | map(.nanos) | sort) as $samples
          | ($samples | length) as $n
          | {
              workload_id: $id,
              operation: $spec.operation,
              mode: $spec.mode,
              cardinality: $spec.cardinality,
              sample_count: $n,
              samples_ns: $samples,
              median_ns: $samples[(($n * 0.50 | ceil) - 1)],
              p95_ns: $samples[(($n * 0.95 | ceil) - 1)],
              p99_ns: $samples[(($n * 0.99 | ceil) - 1)],
              counters: {
                adds_per_workload: ($group | map(.adds) | unique[0]),
                all_operations: ($group | map(.all_operations) | unique[0]),
                settle_operations: ($group | map(.settle_operations) | unique[0]),
                next_operations: ($group | map(.next_operations) | unique[0]),
                cancel_operations: ($group | map(.cancel_operations) | unique[0]),
                child_scans: ($group | map(.child_scans) | unique[0]),
                waits: ($group | map(.waits) | unique[0]),
                wakeups: ($group | map(.wakeups) | unique[0]),
                cancellation_requests: ($group | map(.cancellation_requests) | unique[0]),
                managed_allocations: ($group | map(.managed_allocations) | unique[0]),
                state_allocations: ($group | map(.state_allocations) | unique[0]),
                child_buffer_grows: ($group | map(.child_buffer_grows) | unique[0]),
                waiter_buffer_grows: ($group | map(.waiter_buffer_grows) | unique[0]),
                peak_children: ($group | map(.peak_children) | unique[0]),
                peak_state_bytes: ($group | map(.peak_state_bytes) | unique[0])
              },
              dimensions: {
                latency: {unit: "nanoseconds", median: $samples[(($n * 0.50 | ceil) - 1)]},
                tail_latency: {
                  unit: "nanoseconds",
                  p95: $samples[(($n * 0.95 | ceil) - 1)],
                  p99: $samples[(($n * 0.99 | ceil) - 1)]
                },
                throughput: {
                  unit: "operations_per_second",
                  median: (1000000000 / ($samples[(($n * 0.50 | ceil) - 1)] | if . < 1 then 1 else . end))
                }
              }
            }
          )
      ) as $measurements
      | {
          format: "tondo-stdlib-async-group-performance-report/1",
          edition: "0.1",
          phase: "STD-0.1B",
          task: "STD-ASYNC-GROUP-PERF-001",
          suite: "tondo-stdlib-async-group-performance",
          owner: "std.async.group",
          target: "tondo-vm-hosted",
          backend: "bytecode-vm",
          profile: "test",
          probe_sha256: $probe_sha,
          git_revision: $revision,
          cpu_model: $cpu,
          cpu_features: $cpu_features,
          memory_bytes: $memory_bytes,
          os: $os,
          kernel: $kernel,
          rustc: $rustc,
          llvm: $llvm,
          cargo: $cargo,
          flags: $flags,
          protocol: $contract[0].protocol,
          measurements: $measurements
        }
    ' < "$samples" > "$evidence_dir/stdlib-async-group-performance.json"

jq -e \
    --argjson expected_ids "$workload_ids" \
    --arg probe_sha "$actual_probe_sha" \
    '
      .format == "tondo-stdlib-async-group-performance-report/1"
      and .task == "STD-ASYNC-GROUP-PERF-001"
      and .owner == "std.async.group"
      and .target == "tondo-vm-hosted"
      and .backend == "bytecode-vm"
      and .probe_sha256 == $probe_sha
      and .protocol.minimum_sample_count == 27
      and ([.measurements[].workload_id] | sort) == ($expected_ids | sort)
      and all(.measurements[];
          .sample_count == 27
          and (.samples_ns | length) == 27
          and all(.samples_ns[]; type == "number" and . > 0)
          and .counters.adds_per_workload == .cardinality
          and .counters.peak_children == .cardinality
          and .counters.state_allocations >= 1
          and .counters.peak_state_bytes > 0
          and (.counters.child_scans >= .cardinality or .operation == "add")
          and (.dimensions.tail_latency.p95 >= .dimensions.latency.median)
          and (.dimensions.tail_latency.p99 >= .dimensions.tail_latency.p95)
          and (if .operation == "add" then
                 .counters.all_operations == 0
                 and .counters.settle_operations == 0
                 and .counters.next_operations == 0
                 and .counters.cancel_operations == 0
                 and .counters.waits == 0
                 and .counters.wakeups == 0
               elif .operation == "all" then
                 .counters.all_operations == 1
               elif .operation == "settle" then
                 .counters.settle_operations == 1
               elif .operation == "next" and .mode == "ready" then
                 .counters.next_operations == 1
               elif .operation == "next" then
                 .counters.next_operations == 2
                 and .counters.waits == 1
                 and .counters.wakeups == 1
               else
                 .counters.cancel_operations == 2
                 and .counters.cancellation_requests >= 1
               end)
      )
    ' "$evidence_dir/stdlib-async-group-performance.json" >/dev/null \
    || die "captured report failed validation"

echo "async Group performance: OK (18 workloads; 27 samples each; report: ${evidence_dir#"$root"/}/stdlib-async-group-performance.json)"
