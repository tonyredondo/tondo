#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

contract="$root/testing/stdlib-encoding-performance.json"
if [[ -n "$(printenv TONDO_STDLIB_ENCODING_PERF_CONTRACT || true)" ]]; then
    contract="$(printenv TONDO_STDLIB_ENCODING_PERF_CONTRACT)"
fi
evidence_dir="$root/target/reliability/evidence"
if [[ -n "$(printenv TONDO_STDLIB_ENCODING_PERF_EVIDENCE_DIR || true)" ]]; then
    evidence_dir="$(printenv TONDO_STDLIB_ENCODING_PERF_EVIDENCE_DIR)"
fi
target_dir="$(printenv CARGO_TARGET_DIR || printf '%s' target)"

die() {
    echo "std.encoding performance: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing performance contract: $contract"
scripts/stdlib-encoding-performance-check.sh >/dev/null \
    || die "performance contract check failed"

if [[ "$(printenv TONDO_STDLIB_ENCODING_PERF_ALLOW_DIRTY || printf '%s' 0)" != 1 ]]; then
    [[ -z "$(git status --porcelain)" ]] || die "workspace must be clean"
fi

mkdir -p "$root/.tmp"
tmp="$(mktemp -d "$root/.tmp/tondo-stdlib-encoding-performance.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
samples="$tmp/samples.tsv"
: > "$samples"
probe_test="$(jq -r '.probe.test' "$contract")"

oracle_log="$tmp/oracle.log"
CARGO_TARGET_DIR="$target_dir" cargo test -p tondo-reliability --test encoding_models --locked \
    > "$oracle_log" 2>&1 || {
    cat "$oracle_log" >&2
    die "independent encoding oracle failed"
}

for process in 1 2 3; do
    log="$tmp/process-$process.log"
    CARGO_TARGET_DIR="$target_dir" cargo test -p tondo-compiler --locked \
        "$probe_test" --lib -- --exact --nocapture \
        > "$log" 2>&1 || {
        cat "$log" >&2
        die "probe failed in independent process $process"
    }
    grep -F $'TONDO_ENCODING_PERF\t' "$log" >> "$samples" \
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
flags="$(printenv RUSTFLAGS || true)"
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
       | select(length == 16 and .[0] == "TONDO_ENCODING_PERF")
       | {
           workload_id: .[1],
           codec: .[2],
           operation: .[3],
           policy: .[4],
           size_class: .[5],
           input_bytes: (.[6] | tonumber),
           output_bytes: (.[7] | tonumber),
           chunk_bytes: (.[8] | tonumber),
           dispatch: .[9],
           nanos: (.[10] | tonumber),
           operations: (.[11] | tonumber),
           bytes_copied: (.[12] | tonumber),
           allocations: (.[13] | tonumber),
           logical_memory_bytes: (.[14] | tonumber),
           live_handles: (.[15] | tonumber)
       }] as $rows
      | ($contract[0].workloads) as $workloads
      | ($rows | sort_by(.workload_id) | group_by(.workload_id) | map(
          . as $group
          | ($group[0].workload_id) as $id
          | ($workloads[] | select(.id == $id)) as $spec
          | ($group | map(.nanos) | sort) as $samples
          | ($samples | length) as $n
          | ($group | map(.bytes_copied) | unique[0]) as $bytes_copied
          | {
              workload_id: $id,
              codec: $spec.codec,
              operation: $spec.operation,
              policy: $spec.policy,
              size_class: $spec.size_class,
              payload_bytes: $spec.payload_bytes,
              chunk_bytes: $spec.chunk_bytes,
              dispatch: $spec.dispatch,
              sample_count: $n,
              samples_ns: $samples,
              median_ns: $samples[(($n * 0.50 | ceil) - 1)],
              p95_ns: $samples[(($n * 0.95 | ceil) - 1)],
              p99_ns: $samples[(($n * 0.99 | ceil) - 1)],
              counters: {
                input_bytes: ($group | map(.input_bytes) | unique[0]),
                output_bytes: ($group | map(.output_bytes) | unique[0]),
                operations: ($group | map(.operations) | unique[0]),
                bytes_copied: $bytes_copied,
                allocations: ($group | map(.allocations) | unique[0]),
                logical_memory_bytes: ($group | map(.logical_memory_bytes) | unique[0]),
                live_handles: ($group | map(.live_handles) | unique[0])
              },
              stable: {
                codec: (($group | map(.codec) | unique | length) == 1),
                operation: (($group | map(.operation) | unique | length) == 1),
                policy: (($group | map(.policy) | unique | length) == 1),
                size_class: (($group | map(.size_class) | unique | length) == 1),
                input_bytes: (($group | map(.input_bytes) | unique | length) == 1),
                output_bytes: (($group | map(.output_bytes) | unique | length) == 1),
                chunk_bytes: (($group | map(.chunk_bytes) | unique | length) == 1),
                dispatch: (($group | map(.dispatch) | unique | length) == 1),
                operations: (($group | map(.operations) | unique | length) == 1),
                bytes_copied: (($group | map(.bytes_copied) | unique | length) == 1),
                allocations: (($group | map(.allocations) | unique | length) == 1),
                logical_memory_bytes: (($group | map(.logical_memory_bytes) | unique | length) == 1),
                live_handles: (($group | map(.live_handles) | unique | length) == 1)
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
                  unit: "bytes_per_second",
                  median: (($bytes_copied * 1000000000)
                    / ($samples[(($n * 0.50 | ceil) - 1)] | if . < 1 then 1 else . end))
                },
                bytes_copied: {unit: "bytes", count: $bytes_copied},
                allocations: {unit: "logical-host-values", count: ($group | map(.allocations) | unique[0])},
                logical_memory: {unit: "bytes", value: ($group | map(.logical_memory_bytes) | unique[0])},
                dispatch: {unit: "route", selected: ($group | map(.dispatch) | unique[0])},
                live_handles: {unit: "handles", count: ($group | map(.live_handles) | unique[0])}
              }
            }
      )) as $measurements
      | {
          format: "tondo-stdlib-encoding-performance-report/1",
          edition: "0.1",
          phase: "STD-0.1B",
          task: "STD-ENCODING-PERF-001",
          suite: "tondo-stdlib-encoding-performance",
          owner: "std.encoding",
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
          dispatch: $contract[0].dispatch,
          measurements: $measurements
        }
    ' < "$samples" > "$evidence_dir/stdlib-encoding-performance.json"

jq -e \
    --argjson expected_ids "$workload_ids" \
    --arg probe_sha "$(jq -r '.probe.sha256' "$contract")" \
    '
      .format == "tondo-stdlib-encoding-performance-report/1"
      and .task == "STD-ENCODING-PERF-001"
      and .owner == "std.encoding"
      and .target == "tondo-vm-hosted"
      and .backend == "bytecode-vm"
      and .probe_sha256 == $probe_sha
      and .protocol.minimum_sample_count == 27
      and .protocol.batch_operations == 64
      and .strategy.native_aot == "not-claimed"
      and .strategy.native_runtime_abi == "not-measured-by-this-hosted-report"
      and .strategy.simd == "not-measured-no-optimized-route"
      and .dispatch.mode == "scalar-only"
      and .dispatch.selected_route == "scalar"
      and ([.measurements[].workload_id] | sort) == ($expected_ids | sort)
      and (.measurements | length) == 16
      and all(.measurements[];
          .sample_count == 27
          and (.samples_ns | length) == 27
          and all(.samples_ns[]; type == "number" and . > 0)
          and all(.stable[]; . == true)
          and .counters.operations == 64
          and .counters.allocations > 0
          and .counters.logical_memory_bytes > 0
          and .counters.live_handles == 0
          and .dispatch == "scalar-fixed-target"
          and .dimensions.tail_latency.p95 >= .dimensions.latency.median
          and .dimensions.tail_latency.p99 >= .dimensions.tail_latency.p95
          and .dimensions.throughput.median >= 0
      )
    ' "$evidence_dir/stdlib-encoding-performance.json" >/dev/null \
    || die "captured report failed validation"

echo "std.encoding performance: OK (16 workloads; 27 samples each; report: $evidence_dir/stdlib-encoding-performance.json)"
