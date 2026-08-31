#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_SYNC_PERF_CONTRACT:-$root/testing/stdlib-sync-performance.json}"
evidence_dir="${TONDO_STDLIB_SYNC_PERF_EVIDENCE_DIR:-$root/target/reliability/evidence}"
target_dir="${CARGO_TARGET_DIR:-target}"

die() {
    echo "std.sync performance: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing contract: ${contract#"$root"/}"

jq -e '
    .format == "tondo-stdlib-sync-performance/1"
    and .edition == "0.1"
    and .phase == "STD-0.1B"
    and .task == "STD-SYNC-PERF-001"
    and .owner == "std.sync"
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
        batch_operations: 32,
        deterministic_seed: "tondo-stdlib-sync-perf-0.1",
        outliers: "report-not-delete",
        fixture_setup: "excluded-from-uncontended-latency; included-in-contended-round-trip-and-memory-counters"
    }
    and (.metrics | sort) == [
        "fairness", "latency", "live_handles", "logical_memory_bytes",
        "tail_latency", "throughput"
    ]
    and (.workloads | length) == 20
    and ([.workloads[].id] | unique | length) == 20
    and ([.workloads[].participants] | unique | sort) == [1, 8, 64]
    and all(.workloads[];
        (.id | test("^[a-z0-9]+(-[a-z0-9]+)+$"))
        and (.operation | test("^(mutex|rwlock|semaphore|condition|barrier|atomic|once)-[a-z0-9-]+$"))
        and (.mode == "uncontended" or .mode == "contended" or .mode == "ready")
        and (.participants == 1 or .participants == 8 or .participants == 64)
        and ((.mode == "contended" and .participants >= 8) or .mode != "contended")
    )
    and .invariants.operations == "stable-per-workload-across-all-27-samples"
    and .invariants.contention == "blocked-and-wakeup-counts-equal-declared-participants-or-two-generations-of-participants-minus-one-for-barrier"
    and .invariants.fairness == "zero-FIFO-registration-violations"
    and .invariants.memory == "logical-host-registry-and-queue-capacity; allocator-overhead-excluded"
    and .invariants.cleanup == "no-pending-sync-waiter-or-nonempty-queue-before-probe-return"
    and .invariants.oracle == "independent-sync-model-laws-plus-host-state-invariants"
    and .invariants.target_isolation == "reports-never-combine-targets-or-backends"
    and (.oracle.sources | index("crates/tondo-reliability/src/sync_model.rs")) != null
    and (.oracle.sources | index("crates/tondo-reliability/tests/sync_models.rs")) != null
    and .oracle.kind == "independent-bounded-model-and-host-invariant-checks"
    and .report == "target/reliability/evidence/stdlib-sync-performance.json"
' "$contract" >/dev/null || die "invalid machine-readable sync performance contract"

probe_path="$root/$(jq -r '.probe.path' "$contract")"
[[ -f "$probe_path" ]] || die "missing probe: ${probe_path#"$root"/}"
expected_probe_sha="$(jq -r '.probe.sha256' "$contract")"
actual_probe_sha="$(sha256sum "$probe_path" | cut -d' ' -f1)"
[[ "$actual_probe_sha" == "$expected_probe_sha" ]] || die "probe hash mismatch"

if [[ "${TONDO_STDLIB_SYNC_PERF_ALLOW_DIRTY:-0}" != 1 ]]; then
    [[ -z "$(git status --porcelain)" ]] || die "workspace must be clean"
fi

mkdir -p "$root/.tmp"
tmp="$(mktemp -d "$root/.tmp/tondo-stdlib-sync-performance.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
samples="$tmp/samples.tsv"
: > "$samples"
probe_test="$(jq -r '.probe.test' "$contract")"

oracle_log="$tmp/oracle.log"
CARGO_TARGET_DIR="$target_dir" cargo test -p tondo-reliability --test sync_models --locked \
    > "$oracle_log" 2>&1 || {
    cat "$oracle_log" >&2
    die "independent sync oracle failed"
}

for process in 1 2 3; do
    log="$tmp/process-$process.log"
    CARGO_TARGET_DIR="$target_dir" cargo test -p tondo-compiler --locked \
        "$probe_test" --lib -- --exact --nocapture \
        > "$log" 2>&1 || {
        cat "$log" >&2
        die "probe failed in independent process $process"
    }
    grep -F $'TONDO_SYNC_PERF\t' "$log" >> "$samples" \
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
       | select(length == 12 and .[0] == "TONDO_SYNC_PERF")
       | {
           workload_id: .[1],
           mode: .[2],
           operation: .[3],
           participants: (.[4] | tonumber),
           nanos: (.[5] | tonumber),
           operations: (.[6] | tonumber),
           blocked: (.[7] | tonumber),
           wakeups: (.[8] | tonumber),
           fairness_violations: (.[9] | tonumber),
           logical_memory_bytes: (.[10] | tonumber),
           live_handles: (.[11] | tonumber)
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
              participants: $spec.participants,
              sample_count: $n,
              samples_ns: $samples,
              median_ns: $samples[(($n * 0.50 | ceil) - 1)],
              p95_ns: $samples[(($n * 0.95 | ceil) - 1)],
              p99_ns: $samples[(($n * 0.99 | ceil) - 1)],
              counters: {
                operations: ($group | map(.operations) | unique[0]),
                blocked: ($group | map(.blocked) | unique[0]),
                wakeups: ($group | map(.wakeups) | unique[0]),
                fairness_violations: ($group | map(.fairness_violations) | unique[0]),
                logical_memory_bytes: ($group | map(.logical_memory_bytes) | unique[0]),
                live_handles: ($group | map(.live_handles) | unique[0])
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
                  median: ((($group[0].operations * 1000000000) / ($samples[(($n * 0.50 | ceil) - 1)] | if . < 1 then 1 else . end)))
                },
                fairness: {
                  unit: "violations",
                  count: ($group | map(.fairness_violations) | unique[0])
                },
                logical_memory: {
                  unit: "bytes",
                  value: ($group | map(.logical_memory_bytes) | unique[0])
                }
              }
            }
      )) as $measurements
      | {
          format: "tondo-stdlib-sync-performance-report/1",
          edition: "0.1",
          phase: "STD-0.1B",
          task: "STD-SYNC-PERF-001",
          suite: "tondo-stdlib-sync-performance",
          owner: "std.sync",
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
    ' < "$samples" > "$evidence_dir/stdlib-sync-performance.json"

jq -e \
    --argjson expected_ids "$workload_ids" \
    --arg probe_sha "$actual_probe_sha" \
    '
      .format == "tondo-stdlib-sync-performance-report/1"
      and .task == "STD-SYNC-PERF-001"
      and .owner == "std.sync"
      and .target == "tondo-vm-hosted"
      and .backend == "bytecode-vm"
      and .probe_sha256 == $probe_sha
      and .protocol.minimum_sample_count == 27
      and ([.measurements[].workload_id] | sort) == ($expected_ids | sort)
      and (.measurements | length) == 20
      and all(.measurements[];
          .sample_count == 27
          and (.samples_ns | length) == 27
          and all(.samples_ns[]; type == "number" and . > 0)
          and .counters.operations > 0
          and .counters.logical_memory_bytes > 0
          and .counters.live_handles > 0
          and .counters.fairness_violations == 0
          and .dimensions.tail_latency.p95 >= .dimensions.latency.median
          and .dimensions.tail_latency.p99 >= .dimensions.tail_latency.p95
          and ((.mode == "contended"
                and .counters.blocked == ((.participants - (if .operation == "barrier-generation" then 1 else 0 end)) * (if .operation == "barrier-generation" then 2 else 1 end))
                and .counters.wakeups == .counters.blocked)
               or (.mode != "contended" and .counters.blocked == 0 and .counters.wakeups == 0))
      )
    ' "$evidence_dir/stdlib-sync-performance.json" >/dev/null \
    || die "captured report failed validation"

echo "std.sync performance: OK (20 workloads; 27 samples each; report: ${evidence_dir#"$root"/}/stdlib-sync-performance.json)"
