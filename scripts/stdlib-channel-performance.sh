#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_CHANNEL_PERF_CONTRACT:-$root/testing/stdlib-channel-performance.json}"
evidence_dir="${TONDO_STDLIB_CHANNEL_PERF_EVIDENCE_DIR:-$root/target/reliability/evidence}"
target_dir="${CARGO_TARGET_DIR:-target}"

die() {
    echo "std.channel performance: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing performance contract: ${contract#"$root"/}"
scripts/stdlib-channel-performance-check.sh >/dev/null \
    || die "performance contract check failed"

if [[ "${TONDO_STDLIB_CHANNEL_PERF_ALLOW_DIRTY:-0}" != 1 ]]; then
    [[ -z "$(git status --porcelain)" ]] || die "workspace must be clean"
fi

mkdir -p "$root/.tmp"
tmp="$(mktemp -d "$root/.tmp/tondo-stdlib-channel-performance.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
samples="$tmp/samples.tsv"
: > "$samples"
probe_test="$(jq -r '.probe.test' "$contract")"

oracle_log="$tmp/oracle.log"
CARGO_TARGET_DIR="$target_dir" cargo test -p tondo-reliability --test channel_models --locked \
    > "$oracle_log" 2>&1 || {
    cat "$oracle_log" >&2
    die "independent channel oracle failed"
}

for process in 1 2 3; do
    log="$tmp/process-$process.log"
    CARGO_TARGET_DIR="$target_dir" cargo test -p tondo-compiler --locked \
        "$probe_test" --lib -- --exact --nocapture \
        > "$log" 2>&1 || {
        cat "$log" >&2
        die "probe failed in independent process $process"
    }
    grep -F $'TONDO_CHANNEL_PERF\t' "$log" >> "$samples" \
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
    --arg probe_sha "$(jq -r '.probe.sha256' "$contract")" \
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
       | select(length == 16 and .[0] == "TONDO_CHANNEL_PERF")
       | {
           workload_id: .[1],
           topology: .[2],
           mode: .[3],
           capacity: (if .[4] == "unbounded" then null else (.[4] | tonumber) end),
           producers: (.[5] | tonumber),
           consumers: (.[6] | tonumber),
           nanos: (.[7] | tonumber),
           operations: (.[8] | tonumber),
           blocked: (.[9] | tonumber),
           backpressure: (.[10] | tonumber),
           wakeups: (.[11] | tonumber),
           allocations: (.[12] | tonumber),
           logical_memory_bytes: (.[13] | tonumber),
           live_handles: (.[14] | tonumber),
           queued_peak: (.[15] | tonumber)
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
              topology: $spec.topology,
              mode: $spec.mode,
              capacity: $spec.capacity,
              producers: $spec.producers,
              consumers: $spec.consumers,
              sample_count: $n,
              samples_ns: $samples,
              median_ns: $samples[(($n * 0.50 | ceil) - 1)],
              p95_ns: $samples[(($n * 0.95 | ceil) - 1)],
              p99_ns: $samples[(($n * 0.99 | ceil) - 1)],
              counters: {
                operations: ($group | map(.operations) | unique[0]),
                blocked: ($group | map(.blocked) | unique[0]),
                backpressure: ($group | map(.backpressure) | unique[0]),
                wakeups: ($group | map(.wakeups) | unique[0]),
                allocations: ($group | map(.allocations) | unique[0]),
                logical_memory_bytes: ($group | map(.logical_memory_bytes) | unique[0]),
                live_handles: ($group | map(.live_handles) | unique[0]),
                queued_peak: ($group | map(.queued_peak) | unique[0])
              },
              stable: {
                operations: (($group | map(.operations) | unique | length) == 1),
                blocked: (($group | map(.blocked) | unique | length) == 1),
                backpressure: (($group | map(.backpressure) | unique | length) == 1),
                wakeups: (($group | map(.wakeups) | unique | length) == 1),
                allocations: (($group | map(.allocations) | unique | length) == 1),
                logical_memory_bytes: (($group | map(.logical_memory_bytes) | unique | length) == 1),
                live_handles: (($group | map(.live_handles) | unique | length) == 1),
                queued_peak: (($group | map(.queued_peak) | unique | length) == 1)
              },
              dimensions: {
                latency: {
                  unit: "nanoseconds",
                  median: $samples[(($n * 0.50 | ceil) - 1)]
                },
                tail_latency: {
                  unit: "nanoseconds",
                  p95: $samples[(($n * 0.95 | ceil) - 1)],
                  p99: $samples[(($n * 0.99 | ceil) - 1)]
                },
                throughput: {
                  unit: "operations_per_second",
                  median: (($group[0].operations * 1000000000) / ($samples[(($n * 0.50 | ceil) - 1)] | if . < 1 then 1 else . end))
                },
                allocations: {unit: "logical-host-values", count: ($group | map(.allocations) | unique[0])},
                backpressure: {unit: "events", count: ($group | map(.backpressure) | unique[0])},
                logical_memory: {unit: "bytes", value: ($group | map(.logical_memory_bytes) | unique[0])},
                queued_peak: {unit: "values", count: ($group | map(.queued_peak) | unique[0])},
                wakeups: {unit: "events", count: ($group | map(.wakeups) | unique[0])}
              }
            }
      )) as $measurements
      | {
          format: "tondo-stdlib-channel-performance-report/1",
          edition: "0.1",
          phase: "STD-0.1B",
          task: "STD-CHANNEL-PERF-001",
          suite: "tondo-stdlib-channel-performance",
          owner: "std.channel",
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
          strategy: $contract[0].strategy,
          measurements: $measurements
        }
    ' < "$samples" > "$evidence_dir/stdlib-channel-performance.json"

jq -e \
    --argjson expected_ids "$workload_ids" \
    --arg probe_sha "$(jq -r '.probe.sha256' "$contract")" \
    '
      .format == "tondo-stdlib-channel-performance-report/1"
      and .task == "STD-CHANNEL-PERF-001"
      and .owner == "std.channel"
      and .target == "tondo-vm-hosted"
      and .backend == "bytecode-vm"
      and .probe_sha256 == $probe_sha
      and .protocol.minimum_sample_count == 27
      and .strategy.native_aot == "not-claimed"
      and .strategy.native_runtime_abi == "not-measured-by-this-hosted-report"
      and ([.measurements[].workload_id] | sort) == ($expected_ids | sort)
      and (.measurements | length) == 9
      and all(.measurements[];
          .sample_count == 27
          and (.samples_ns | length) == 27
          and all(.samples_ns[]; type == "number" and . > 0)
          and all(.stable[]; . == true)
          and .counters.operations > 0
          and .counters.allocations > 0
          and .counters.logical_memory_bytes > 0
          and .counters.live_handles == 0
          and .dimensions.tail_latency.p95 >= .dimensions.latency.median
          and .dimensions.tail_latency.p99 >= .dimensions.tail_latency.p95
          and (if .mode == "rendezvous" then
                 .counters.blocked > 0
                 and .counters.backpressure > 0
                 and .counters.wakeups > 0
               elif .mode == "buffered" then
                 .counters.blocked == 0
                 and .counters.backpressure == 0
                 and .counters.wakeups == 0
                 and .counters.queued_peak > 0
               elif .mode == "unbounded" then
                 .counters.blocked == 0
                 and .counters.backpressure == 0
                 and .counters.wakeups == 0
                 and .counters.queued_peak > 0
               elif .mode == "backpressure" then
                 .counters.blocked > 0
                 and .counters.backpressure > 0
                 and .counters.wakeups > 0
               else
                 .counters.blocked == 16
                 and .counters.backpressure == 16
                 and .counters.wakeups == 16
               end)
      )
    ' "$evidence_dir/stdlib-channel-performance.json" >/dev/null \
    || die "captured report failed validation"

echo "std.channel performance: OK (9 workloads; 27 samples each; report: ${evidence_dir#"$root"/}/stdlib-channel-performance.json)"
