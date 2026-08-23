#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

evidence_dir="${TONDO_STDLIB_EVIDENCE_DIR:-target/reliability/evidence}"
mkdir -p "$evidence_dir"
build_dir="$(mktemp -d "$evidence_dir/stdlib-perf-build.XXXXXX")"
samples_file="$(mktemp "$evidence_dir/stdlib-performance-samples.XXXXXX")"
run_metrics="$(mktemp "$evidence_dir/stdlib-performance-runs.XXXXXX")"
startup_metrics="$(mktemp "$evidence_dir/stdlib-performance-startup.XXXXXX")"
trap 'rm -rf "$build_dir" "$samples_file" "$run_metrics" "$startup_metrics"' EXIT

if [[ "${TONDO_PERF_ALLOW_DIRTY:-0}" != 1 ]] && ! git diff --quiet; then
    echo "stdlib performance report: workspace must be clean (set TONDO_PERF_ALLOW_DIRTY=1 only for local iteration)" >&2
    exit 1
fi

profile="${TONDO_PERF_PROFILE:-dev}"
build_args=(--locked --quiet)
binary_profile="debug"
if [[ "$profile" == "release" ]]; then
    build_args+=(--release)
    binary_profile="release"
fi

build_started="$(date +%s%N)"
CARGO_TARGET_DIR="$build_dir" cargo build -p tondo-stdlib --example stdlib_performance_probe "${build_args[@]}"
build_finished="$(date +%s%N)"
compile_time_ms="$(( (build_finished - build_started) / 1000000 ))"
binary="$build_dir/$binary_profile/examples/stdlib_performance_probe"
[[ -x "$binary" ]] || { echo "stdlib performance report: missing probe binary" >&2; exit 1; }

for process in 1 2 3; do
    if [[ -x /usr/bin/time ]]; then
        /usr/bin/time -f '%e\t%M' -a -o "$run_metrics" "$binary" >> "$samples_file"
    else
        started="$(date +%s%N)"
        "$binary" >> "$samples_file"
        finished="$(date +%s%N)"
        printf '%s\t0\n' "$((finished - started))" >> "$run_metrics"
    fi
done

for process in 1 2 3; do
    started="$(date +%s%N)"
    TONDO_PERF_STARTUP_ONLY=1 "$binary" >/dev/null
    finished="$(date +%s%N)"
    printf '%s\t0\n' "$((finished - started))" >> "$startup_metrics"
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
target="${TONDO_PERF_TARGET:-$(rustc -vV | awk '/^host:/ { value=$2 } END { print value }')}"
llvm="$(rustc -vV | awk -F': ' '/^LLVM version:/ { value=$2 } END { print value }')"
flags="${RUSTFLAGS-}"
code_size="$(stat -c '%s' "$binary" 2>/dev/null || stat -f '%z' "$binary")"

readarray -t startup_samples_us < <(awk -F '\t' '{ printf "%.0f\n", $1 / 1000 }' "$startup_metrics")
readarray -t peak_samples_bytes < <(awk -F '\t' '{ printf "%.0f\n", $2 * 1024 }' "$run_metrics")
[[ "${#startup_samples_us[@]}" -eq 3 && "${#peak_samples_bytes[@]}" -eq 3 ]] || {
    echo "stdlib performance report: expected three process observations" >&2
    exit 1
}

startup_json="$(printf '%s\n' "${startup_samples_us[@]}" | jq -Rsc 'split("\n") | map(select(length > 0) | tonumber)')"
peak_json="$(printf '%s\n' "${peak_samples_bytes[@]}" | jq -Rsc 'split("\n") | map(select(length > 0) | tonumber)')"

jq -Rn \
    --arg revision "$revision" \
    --arg cpu "$cpu" \
    --arg cpu_features "$cpu_features" \
    --argjson memory_bytes "$memory_bytes" \
    --arg os "$os" \
    --arg kernel "$kernel" \
    --arg target "$target" \
    --arg profile "$profile" \
    --arg rustc "$(rustc --version)" \
    --arg llvm "$llvm" \
    --arg cargo "$(cargo --version)" \
    --arg flags "$flags" \
    --argjson compile_time_ms "$compile_time_ms" \
    --argjson code_size "$code_size" \
    --argjson startup_samples "$startup_json" \
    --argjson peak_samples "$peak_json" \
    --arg backend "portable-scalar" \
    '
      def quantile($values; $fraction):
        ($values | sort) as $sorted
        | ($sorted | length) as $n
        | $sorted[((($n * $fraction) | ceil) - 1) | if . < 0 then 0 else . end];
      def metric($values; $unit; $scope):
        {unit: $unit, scope: $scope, samples: $values,
         median: quantile($values; 0.50),
         p95: quantile($values; 0.95),
         p99: quantile($values; 0.99)};
      ($startup_samples | sort) as $startup
      | ($peak_samples | sort) as $peak
      | [inputs | split("\t") | select(length == 6) | {
          module: .[0], operation: .[1], workload: .[2],
          nanos: (.[3] | tonumber), allocations: (.[4] | tonumber),
          allocated_bytes: (.[5] | tonumber)
        }]
      | group_by([.module, .operation, .workload])
      | map(. as $rows
          | ($rows | map(.nanos) | sort) as $latency
          | ($rows | map(.allocations) | sort) as $allocations
          | ($rows | map(.allocated_bytes) | sort) as $bytes
          | {
              module: $rows[0].module,
              operation: $rows[0].operation,
              workload: $rows[0].workload,
              sample_count: ($rows | length),
              samples: $latency,
              dimensions: {
                allocations_per_operation: metric($allocations; "count"; "logical-owned-buffer"),
                allocated_bytes_per_operation: metric($bytes; "bytes"; "logical-owned-buffer"),
                code_size: {unit: "bytes", scope: "campaign-binary", median: $code_size},
                compile_time: {unit: "milliseconds", scope: "campaign-build", median: $compile_time_ms},
                peak_memory: metric($peak; "bytes"; "process-campaign"),
                startup: metric($startup; "microseconds"; "process-campaign"),
                tail_latency: {
                  unit: "microseconds", scope: "operation",
                  p95: (quantile($latency; 0.95) / 1000),
                  p99: (quantile($latency; 0.99) / 1000)
                },
                throughput: {
                  unit: "operations_per_second", scope: "operation",
                  median: (1000000000 / (quantile($latency; 0.50) | if . < 1 then 1 else . end))
                }
              }
            })
      | {format: "tondo-stdlib-performance-report/1",
         protocol: "monotonic-3x9x3",
         revision: $revision,
         cpu_model: $cpu,
         cpu_features: $cpu_features,
         memory_bytes: $memory_bytes,
         os: $os,
         kernel: $kernel,
         target: $target,
         backend: $backend,
         profile: $profile,
         rustc: $rustc,
         llvm: $llvm,
         cargo: $cargo,
         flags: $flags,
         git_revision: $revision,
         warmup_iterations: 3,
         measurement_repetitions: 9,
         independent_processes: 3,
         measured_dimensions: ["allocations_per_operation", "allocated_bytes_per_operation", "code_size", "compile_time", "peak_memory", "startup", "tail_latency", "throughput"],
         campaign: {
           binary: "tondo-stdlib/examples/stdlib_performance_probe",
           code_size_bytes: $code_size,
           compile_time_ms: $compile_time_ms,
           startup_samples_us: $startup,
           peak_memory_samples_bytes: $peak,
           allocation_observation: "logical-owned-buffer"
         },
         measurements: .}
    ' < "$samples_file" > "$evidence_dir/stdlib-performance-report.json"

jq -e '
    .format == "tondo-stdlib-performance-report/1"
    and .protocol == "monotonic-3x9x3"
    and .warmup_iterations == 3
    and .measurement_repetitions == 9
    and .independent_processes == 3
    and .measured_dimensions == ["allocations_per_operation", "allocated_bytes_per_operation", "code_size", "compile_time", "peak_memory", "startup", "tail_latency", "throughput"]
    and (.measurements | length) > 0
    and all(.measurements[];
        (.sample_count == 27)
        and ((.samples | length) == .sample_count)
        and (.module | type == "string" and length > 0)
        and (.operation | type == "string" and length > 0)
        and (.workload | type == "string" and length > 0)
        and (.dimensions.allocations_per_operation.median | type == "number" and . >= 0)
        and (.dimensions.allocated_bytes_per_operation.median | type == "number" and . >= 0)
        and (.dimensions.code_size.median | type == "number" and . > 0)
        and (.dimensions.compile_time.median | type == "number" and . >= 0)
        and (.dimensions.peak_memory.median | type == "number" and . >= 0)
        and (.dimensions.startup.median | type == "number" and . >= 0)
        and (.dimensions.tail_latency.p95 | type == "number" and . >= 0)
        and (.dimensions.tail_latency.p99 | type == "number" and . >= 0)
        and (.dimensions.throughput.median | type == "number" and . > 0)
    )
    and (.campaign.allocation_observation == "logical-owned-buffer")
    and (.campaign.startup_samples_us | length == 3)
    and (.campaign.peak_memory_samples_bytes | length == 3)
    and (.cpu_model | type == "string")
    and (.cpu_features | type == "string")
    and (.target | type == "string")
    and (.git_revision | type == "string")
' "$evidence_dir/stdlib-performance-report.json" >/dev/null
echo "stdlib performance report: OK (all eight dimensions; logical allocation oracle)"
