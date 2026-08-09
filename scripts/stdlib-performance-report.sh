#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

evidence_dir="${TONDO_STDLIB_EVIDENCE_DIR:-target/reliability/evidence}"
mkdir -p "$evidence_dir"
samples_file="$(mktemp "$evidence_dir/stdlib-performance-samples.XXXXXX")"
trap 'rm -f "$samples_file"' EXIT

for process in 1 2 3; do
    cargo run -p tondo-stdlib --example stdlib_performance_probe --locked --quiet \
        >> "$samples_file"
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
profile="${TONDO_PERF_PROFILE:-dev}"
flags="${RUSTFLAGS-}"
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
    --arg backend "portable-scalar" \
    '
      [inputs | split("\t") | select(length == 4) | {
          module: .[0], operation: .[1], workload: .[2], nanos: (.[3] | tonumber)
      }]
      | group_by([.module, .operation, .workload])
      | map(. as $rows
          | ($rows | map(.nanos) | sort) as $samples
          | ($samples | length) as $n
          | ($samples[(($n * 0.50 | ceil) - 1)]) as $median
          | ($samples[(($n * 0.95 | ceil) - 1)]) as $p95
          | ($samples[(($n * 0.99 | ceil) - 1)]) as $p99
          | {module: $rows[0].module,
             operation: $rows[0].operation,
             workload: $rows[0].workload,
             samples: $samples,
             median_ns: $median,
             p95_ns: $p95,
             p99_ns: $p99,
             sample_count: $n,
             dimensions: {
               tail_latency: {unit:"nanoseconds", p95: $p95, p99: $p99},
               throughput: {
                 unit:"operations_per_second",
                 median: (1000000000 / ($median | if . < 1 then 1 else . end))
               }
             }})
      | {format:"tondo-stdlib-performance-report/1",
         protocol:"monotonic-3x9x3",
         revision:$revision,
         cpu_model:$cpu,
         cpu_features:$cpu_features,
         memory_bytes:$memory_bytes,
         os:$os,
         kernel:$kernel,
         target:$target,
         backend:$backend,
         profile:$profile,
         rustc:$rustc,
         llvm:$llvm,
         cargo:$cargo,
         flags:$flags,
         git_revision:$revision,
         warmup_iterations:3,
         measurement_repetitions:9,
         independent_processes:3,
         measurements:.}
    ' < "$samples_file" > "$evidence_dir/stdlib-performance-report.json"

jq -e '
    .format == "tondo-stdlib-performance-report/1"
    and .protocol == "monotonic-3x9x3"
    and .warmup_iterations == 3
    and .measurement_repetitions == 9
    and .independent_processes == 3
    and (.measurements | length) > 0
    and all(.measurements[];
        (.sample_count == 27)
        and ((.samples | length) == .sample_count)
        and (.module | type == "string" and length > 0)
        and (.operation | type == "string" and length > 0)
        and (.workload | type == "string" and length > 0)
        and (.dimensions.tail_latency.p95 | type == "number")
        and (.dimensions.tail_latency.p99 | type == "number")
        and (.dimensions.throughput.median | type == "number")
    )
    and (.cpu_model | type == "string")
    and (.cpu_features | type == "string")
    and (.os | type == "string")
    and (.kernel | type == "string")
    and (.target | type == "string")
    and (.backend | type == "string")
    and (.profile | type == "string")
    and (.rustc | type == "string")
    and (.llvm | type == "string")
    and (.cargo | type == "string")
    and (.flags | type == "string")
    and (.git_revision | type == "string")
    and (.memory_bytes | type == "number" and . >= 0)
' \
    "$evidence_dir/stdlib-performance-report.json" >/dev/null
echo "stdlib performance report: OK"
