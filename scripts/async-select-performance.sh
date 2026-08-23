#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_SELECT_PERF_CONTRACT:-$root/testing/async-select-performance.json}"
evidence_dir="${TONDO_SELECT_PERF_EVIDENCE_DIR:-$root/target/reliability/evidence}"
target_dir="${CARGO_TARGET_DIR:-target}"

die() {
    echo "async select performance: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing contract: ${contract#"$root"/}"

jq -e '
    .format == "tondo-async-select-performance/1"
    and .edition == "0.1"
    and .task == "ASYNC-SELECT-PERF-001"
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
        batch_operations: 64,
        deterministic_seed: "tondo-perf-0.1",
        outliers: "report-not-delete"
    }
    and (.metrics | sort) == [
        "allocation_count", "arm_scans", "frame_bytes", "latency",
        "registered_arms", "tail_latency", "throughput", "wakeups"
    ]
    and (.workloads | length == 9)
    and ([.workloads[].id] | unique | length == 9)
    and all(.workloads[];
        (.id | test("^[a-z0-9]+(-[a-z0-9]+)+$"))
        and (.mode == "ready" or .mode == "pending" or .mode == "direct")
        and (.arms >= 1 and .arms <= 64)
        and (.mode != "direct" or .id == "direct-ready-1")
    )
    and .report == "target/reliability/evidence/async-select-performance.json"
' "$contract" >/dev/null || die "invalid machine-readable contract"

probe_path="$root/$(jq -r '.probe.path' "$contract")"
[[ -f "$probe_path" ]] || die "missing probe: ${probe_path#"$root"/}"
expected_probe_sha="$(jq -r '.probe.sha256' "$contract")"
actual_probe_sha="$(sha256sum "$probe_path" | cut -d' ' -f1)"
[[ "$actual_probe_sha" == "$expected_probe_sha" ]] || die "probe hash mismatch"

if [[ "${TONDO_SELECT_PERF_ALLOW_DIRTY:-0}" != 1 ]]; then
    [[ -z "$(git status --porcelain)" ]] || die "workspace must be clean"
fi

tmp="$(mktemp -d "$root/.tmp/tondo-select-performance.XXXXXX")"
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
    grep -F $'TONDO_SELECT_PERF\t' "$log" >> "$samples" \
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
       | select(length == 11 and .[0] == "TONDO_SELECT_PERF")
       | {
           workload_id: .[1],
           nanos: (.[2] | tonumber),
           commits: (.[3] | tonumber),
           registrations: (.[4] | tonumber),
           waits: (.[5] | tonumber),
           wakeups: (.[6] | tonumber),
           scans: (.[7] | tonumber),
           frame_allocations: (.[8] | tonumber),
           frame_bytes: (.[9] | tonumber),
           arms: (.[10] | tonumber)
       }] as $rows
      | ($contract[0].workloads) as $workloads
      | ($rows | group_by(.workload_id) | map(
          . as $group
          | ($group[0].workload_id) as $id
          | ($workloads[] | select(.id == $id)) as $spec
          | ($group | map(.nanos) | sort) as $samples
          | ($samples | length) as $n
          | ($group | map(.commits) | unique) as $commits
          | ($group | map(.registrations) | unique) as $registrations
          | ($group | map(.waits) | unique) as $waits
          | ($group | map(.wakeups) | unique) as $wakeups
          | ($group | map(.scans) | unique) as $scans
          | ($group | map(.frame_allocations) | unique) as $frame_allocations
          | ($group | map(.frame_bytes) | unique) as $frame_bytes
          | ($group | map(.arms) | unique) as $arms
          | {
              workload_id: $id,
              mode: $spec.mode,
              arms: $spec.arms,
              sample_count: $n,
              samples_ns: $samples,
              median_ns: $samples[(($n * 0.50 | ceil) - 1)],
              p95_ns: $samples[(($n * 0.95 | ceil) - 1)],
              p99_ns: $samples[(($n * 0.99 | ceil) - 1)],
              counters: {
                commits_per_operation: $commits[0],
                registrations_per_operation: $registrations[0],
                waits_per_operation: $waits[0],
                wakeups_per_operation: $wakeups[0],
                arm_scans_per_operation: $scans[0],
                frame_allocations_per_operation: $frame_allocations[0],
                frame_bytes: $frame_bytes[0],
                arm_table_arms: $arms[0],
                managed_allocations_per_operation: 0
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
      )) as $measurements
      | {
          format: "tondo-async-select-performance-report/1",
          edition: "0.1",
          task: "ASYNC-SELECT-PERF-001",
          suite: "tondo-async-select-performance",
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
    ' < "$samples" > "$evidence_dir/async-select-performance.json"

jq -e \
    --argjson expected_ids "$workload_ids" \
    --arg probe_sha "$actual_probe_sha" \
    '
      .format == "tondo-async-select-performance-report/1"
      and .task == "ASYNC-SELECT-PERF-001"
      and .probe_sha256 == $probe_sha
      and .protocol.minimum_sample_count == 27
      and ([.measurements[].workload_id] | sort) == ($expected_ids | sort)
      and all(.measurements[];
          .sample_count == 27
          and (.samples_ns | length) == 27
          and all(.samples_ns[]; type == "number" and . > 0)
          and (.counters.commits_per_operation == 1 or .mode == "direct")
          and (.counters.frame_allocations_per_operation == 1 or .mode == "direct")
          and (.counters.frame_bytes > 0 or .mode == "direct")
          and (.counters.arm_table_arms >= 1 or .mode == "direct")
          and (.dimensions.tail_latency.p95 >= .dimensions.latency.median)
          and (.dimensions.tail_latency.p99 >= .dimensions.tail_latency.p95)
      )
      and (any(.measurements[]; .workload_id == "select-ready-1"))
      and (any(.measurements[]; .workload_id == "direct-ready-1"))
    ' "$evidence_dir/async-select-performance.json" >/dev/null \
    || die "captured report failed validation"

echo "async select performance: OK (9 workloads; 27 samples each; report: ${evidence_dir#"$root"/}/async-select-performance.json)"
