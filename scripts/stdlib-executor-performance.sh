#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_EXECUTOR_PERF_CONTRACT:-$root/testing/stdlib-executor-performance.json}"
evidence_dir="${TONDO_STDLIB_EXECUTOR_PERF_EVIDENCE_DIR:-$root/target/reliability/evidence}"
target_dir="${CARGO_TARGET_DIR:-target}"

die() {
    echo "std.executor performance: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing performance contract: ${contract#"$root"/}"
TONDO_STDLIB_EXECUTOR_PERF_CONTRACT="$contract" \
    scripts/stdlib-executor-performance-check.sh >/dev/null \
    || die "performance contract check failed"

if [[ "${TONDO_STDLIB_EXECUTOR_PERF_ALLOW_DIRTY:-0}" != 1 ]]; then
    [[ -z "$(git status --porcelain)" ]] || die "workspace must be clean"
fi

mkdir -p "$root/.tmp"
tmp="$(mktemp -d "$root/.tmp/tondo-stdlib-executor-performance.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
samples="$tmp/samples.tsv"
unsupported="$tmp/unsupported.tsv"
: >"$samples"
: >"$unsupported"

hosted_probe_test="$(jq -r '.targets.hosted_vm.probe.test' "$contract")"
native_probe_test="$(jq -r '.targets.native_runtime.probe.test' "$contract")"
host_target="$(rustc -vV | awk '/^host:/ { value=$2 } END { print value }')"
native_expected=0
if [[ "$host_target" == "x86_64-unknown-linux-gnu" ]]; then
    native_expected=1
fi
native_supported_json=false
if [[ "$native_expected" == 1 ]]; then
    native_supported_json=true
fi

oracle_log="$tmp/oracle.log"
CARGO_TARGET_DIR="$target_dir" cargo test -p tondo-reliability --test models \
    executor_model_sequences_are_bounded_replayable_and_cleanup_complete --locked \
    >"$oracle_log" 2>&1 || {
    cat "$oracle_log" >&2
    die "independent executor oracle failed"
}

for process in 1 2 3; do
    hosted_log="$tmp/hosted-$process.log"
    CARGO_TARGET_DIR="$target_dir" cargo test -p tondo-vm --locked \
        "$hosted_probe_test" --lib -- --exact --nocapture \
        >"$hosted_log" 2>&1 || {
        cat "$hosted_log" >&2
        die "hosted probe failed in independent process $process"
    }
    grep -F $'TONDO_EXECUTOR_PERF\thosted-vm\t' "$hosted_log" >>"$samples" \
        || die "hosted probe emitted no samples in independent process $process"

    native_log="$tmp/native-$process.log"
    CARGO_TARGET_DIR="$target_dir" cargo test -p tondo-native-runtime --locked \
        "$native_probe_test" --lib -- --exact --nocapture \
        >"$native_log" 2>&1 || {
        cat "$native_log" >&2
        die "native probe failed in independent process $process"
    }
    if grep -F $'TONDO_EXECUTOR_PERF\tnative-runtime\t' "$native_log" >>"$samples"; then
        :
    elif grep -F $'TONDO_EXECUTOR_PERF_UNSUPPORTED\tnative-runtime\t' "$native_log" >>"$unsupported"; then
        :
    else
        cat "$native_log" >&2
        die "native probe emitted neither samples nor an unsupported marker in process $process"
    fi
done

native_rows="$(grep -F $'TONDO_EXECUTOR_PERF\tnative-runtime\t' "$samples" | wc -l || true)"
if [[ "$native_expected" == 1 && "$native_rows" == 0 ]]; then
    die "x86_64 Linux expected native samples but the probe was unsupported"
fi
if [[ "$native_expected" == 0 && "$native_rows" != 0 ]]; then
    die "native samples were emitted on an unsupported target"
fi

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
rustc_version="$(rustc --version)"
llvm="$(rustc -vV | awk -F': ' '/^LLVM version:/ { value=$2 } END { print value }')"
cargo_version="$(cargo --version)"
flags="${RUSTFLAGS-}"
hosted_probe_sha="$(jq -r '.targets.hosted_vm.probe.sha256' "$contract")"
native_probe_sha="$(jq -r '.targets.native_runtime.probe.sha256' "$contract")"
if [[ "$native_expected" == 1 ]]; then
    expected_ids="$(jq -c '[.workloads[].id]' "$contract")"
else
    expected_ids="$(jq -c '[.workloads[] | select(.target == "hosted-vm") | .id]' "$contract")"
fi

mkdir -p "$evidence_dir"
jq -Rn \
    --slurpfile contract "$contract" \
    --arg revision "$revision" \
    --arg hosted_probe_sha "$hosted_probe_sha" \
    --arg native_probe_sha "$native_probe_sha" \
    --arg cpu "$cpu" \
    --arg cpu_features "$cpu_features" \
    --argjson memory_bytes "$memory_bytes" \
    --arg os "$os" \
    --arg kernel "$kernel" \
    --arg host_target "$host_target" \
    --arg rustc "$rustc_version" \
    --arg llvm "$llvm" \
    --arg cargo "$cargo_version" \
    --arg flags "$flags" \
    --argjson native_supported "$native_supported_json" \
    '
      [inputs
       | split("\t")
       | select(length == 17 and .[0] == "TONDO_EXECUTOR_PERF")
       | {
           target: .[1],
           workload_id: .[2],
           operation: .[3],
           workers: (.[4] | tonumber),
           capacity: (.[5] | tonumber),
           nanos: (.[6] | tonumber),
           operations: (.[7] | tonumber),
           accepted: (.[8] | tonumber),
           pending: (.[9] | tonumber),
           waits: (.[10] | tonumber),
           bridge_events: (.[11] | tonumber),
           queued_peak: (.[12] | tonumber),
           active_peak: (.[13] | tonumber),
           worker_starts: (.[14] | tonumber),
           logical_memory_bytes: (.[15] | tonumber),
           live_handles: (.[16] | tonumber)
       }] as $rows
      | ($contract[0].workloads
         | if $native_supported then . else map(select(.target == "hosted-vm")) end) as $workloads
      | ($rows | group_by(.workload_id) | map(
          . as $group
          | ($group[0].workload_id) as $id
          | ($workloads[] | select(.id == $id)) as $spec
          | ($group | map(.nanos) | sort) as $samples
          | ($samples | length) as $n
          | {
              workload_id: $id,
              target: $spec.target,
              backend: (if $spec.target == "hosted-vm" then "bytecode-vm" else "native-runtime-private-token-lane" end),
              operation: $spec.operation,
              workers: $spec.workers,
              capacity: $spec.capacity,
              operations: $spec.operations,
              sample_count: $n,
              samples_ns: $samples,
              median_ns: $samples[(($n * 0.50 | ceil) - 1)],
              p95_ns: $samples[(($n * 0.95 | ceil) - 1)],
              p99_ns: $samples[(($n * 0.99 | ceil) - 1)],
              counters: {
                operations: ($group | map(.operations) | unique[0]),
                accepted: ($group | map(.accepted) | unique[0]),
                pending_min: ($group | map(.pending) | min),
                pending_max: ($group | map(.pending) | max),
                waits_min: ($group | map(.waits) | min),
                waits_max: ($group | map(.waits) | max),
                bridge_events: ($group | map(.bridge_events) | unique[0]),
                queued_peak_max: ($group | map(.queued_peak) | max),
                active_peak_max: ($group | map(.active_peak) | max),
                worker_starts_min: ($group | map(.worker_starts) | min),
                worker_starts_max: ($group | map(.worker_starts) | max),
                logical_memory_bytes_min: ($group | map(.logical_memory_bytes) | min),
                logical_memory_bytes_max: ($group | map(.logical_memory_bytes) | max),
                live_handles_max: ($group | map(.live_handles) | max)
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
                  median: (($spec.operations * 1000000000) / ($samples[(($n * 0.50 | ceil) - 1)] | if . < 1 then 1 else . end))
                },
                scheduling: {unit: "accepted-jobs", count: ($group | map(.accepted) | unique[0])},
                backpressure: {unit: "pending-admission-attempts", min: ($group | map(.pending) | min), max: ($group | map(.pending) | max)},
                wakeup_bridge: {unit: "owner-wait-and-completion-events", waits_max: ($group | map(.waits) | max), completions: ($group | map(.bridge_events) | unique[0])},
                logical_memory: {unit: "bytes", min: ($group | map(.logical_memory_bytes) | min), max: ($group | map(.logical_memory_bytes) | max)}
              }
            }
      )) as $measurements
      | {
          format: "tondo-stdlib-executor-performance-report/1",
          edition: "0.1",
          phase: "STD-0.1B",
          task: "STD-EXEC-PERF-001",
          suite: "tondo-stdlib-executor-performance",
          owner: "std.executor",
          host_target: $host_target,
          native_supported: $native_supported,
          native_target: "x86_64-unknown-linux-gnu",
          backend_lanes: {hosted_vm: "bytecode-vm", native_runtime: "native-runtime-private-token-lane"},
          profile: "test",
          probe_sha256: {hosted_vm: $hosted_probe_sha, native_runtime: $native_probe_sha},
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
          strategy: $contract[0].strategy,
          measurements: $measurements
        }
    ' <"$samples" >"$evidence_dir/stdlib-executor-performance.json"

jq -e \
    --argjson expected_ids "$expected_ids" \
    --arg hosted_probe_sha "$hosted_probe_sha" \
    --arg native_probe_sha "$native_probe_sha" \
    --argjson native_supported "$native_supported_json" \
    '
      .format == "tondo-stdlib-executor-performance-report/1"
      and .task == "STD-EXEC-PERF-001"
      and .owner == "std.executor"
      and .native_supported == $native_supported
      and .native_target == "x86_64-unknown-linux-gnu"
      and .probe_sha256.hosted_vm == $hosted_probe_sha
      and .probe_sha256.native_runtime == $native_probe_sha
      and .protocol.minimum_sample_count == 27
      and .strategy.native_aot == "not-claimed"
      and .strategy.aggregation == "target-and-backend-separated"
      and ([.measurements[].workload_id] | sort) == ($expected_ids | sort)
      and (.measurements | length) == (if $native_supported then 12 else 6 end)
      and (([.measurements[].target] | unique | sort) == (if $native_supported then ["hosted-vm", "native-runtime"] else ["hosted-vm"] end))
      and all(.measurements[];
          .sample_count == 27
          and (.samples_ns | length) == 27
          and all(.samples_ns[]; type == "number" and . > 0)
          and .counters.operations == .operations
          and (if .operation == "startup" then
                 .counters.accepted == 0
                 and .counters.bridge_events == 0
               else
                 .counters.accepted == .operations
                 and .counters.bridge_events == .operations
               end)
          and .counters.active_peak_max <= .workers
          and .counters.queued_peak_max <= .capacity
          and .counters.worker_starts_min == .workers
          and .counters.worker_starts_max == .workers
          and .counters.logical_memory_bytes_min > 0
          and .counters.live_handles_max == 0
          and .dimensions.tail_latency.p95 >= .dimensions.latency.median
          and .dimensions.tail_latency.p99 >= .dimensions.tail_latency.p95
          and (if .operation == "startup" then
                 .counters.accepted == 0
                 and .counters.bridge_events == 0
               elif .operation == "saturation" then
                 .counters.pending_max > 0
                 and .counters.waits_max > 0
               else
                 .counters.waits_max > 0
               end)
      )
    ' "$evidence_dir/stdlib-executor-performance.json" >/dev/null \
    || die "captured report failed validation"

echo "std.executor performance: OK (hosted and native lanes; 27 samples per supported workload; report: ${evidence_dir#"$root"/}/stdlib-executor-performance.json)"
