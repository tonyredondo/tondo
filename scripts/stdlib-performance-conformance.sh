#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

config="${TONDO_STDLIB_PERF_CONFORMANCE_CONFIG:-testing/stdlib-performance-conformance.json}"
report="${TONDO_STDLIB_PERF_REPORT:-target/reliability/evidence/stdlib-performance-report.json}"
contract="${TONDO_STDLIB_PERF_CONTRACT:-testing/stdlib-performance.json}"
implementation="${TONDO_STDLIB_IMPLEMENTATION:-testing/stdlib-implementation.json}"

die() {
    echo "stdlib performance conformance: $*" >&2
    exit 1
}

[[ -f "$config" ]] || die "missing coordinator: $config"
[[ -f "$report" ]] || die "missing report: $report (run stdlib-performance-report.sh first)"
[[ -f "$contract" ]] || die "missing performance contract: $contract"
[[ -f "$implementation" ]] || die "missing implementation registry: $implementation"

jq -e \
    --slurpfile implementation "$implementation" \
    --slurpfile contract "$contract" \
    '
      .format == "tondo-stdlib-performance-conformance/1"
      and .edition == "0.1"
      and .phase == "STD-0.1A"
      and .status == "coordinated"
      and .contract == "testing/stdlib-performance.json"
      and .report == "target/reliability/evidence/stdlib-performance-report.json"
      and (.protocol.clock == "monotonic")
      and (.protocol.warmup_iterations == $contract[0].measurement.warmup_iterations)
      and (.protocol.measurement_repetitions == $contract[0].measurement.measurement_repetitions)
      and (.protocol.independent_processes == $contract[0].measurement.independent_processes)
      and (.protocol.minimum_sample_count == $contract[0].measurement.minimum_sample_count)
      and (.protocol.required_environment == $contract[0].measurement.recorded_environment)
      and (.measured_dimensions == ["throughput", "tail_latency"])
      and ((.deferred_dimensions | sort) == [
        "allocated_bytes_per_operation",
        "allocations_per_operation",
        "code_size",
        "compile_time",
        "peak_memory",
        "startup"
      ])
      and (([.owners[].id] | unique | sort) == (
        (($implementation[0].owners | map(.id))
          + ($contract[0].owner_groups | map(.owners[]) | unique))
        | unique | sort
      ))
      and all(.owners[];
        (.group | test("^a[0-4]-[a-z-]+$"))
        and (.state | ["captured-partial", "awaiting-owner-hot-path"] | index(.) != null)
        and (if .state == "captured-partial" then
          (.operation | test("^std\\.[a-z]+\\.[a-z_]+$"))
          and (.module | type == "string" and length > 0)
          and (.workload == "representative")
          and (.oracle == "portable-scalar")
          and (.dimensions == ["throughput", "tail_latency"])
          and ((.pending_dimensions | sort) == [
            "allocated_bytes_per_operation",
            "allocations_per_operation",
            "code_size",
            "compile_time",
            "peak_memory",
            "startup"
          ])
          and (.promotion == "owner-baseline-review-pending")
        else
          (.reason | type == "string" and length > 0)
          and (has("operation") | not)
        end)
      )
      and ([.owners[] | select(.state == "captured-partial") | .operation] | sort)
          == [
            "std.json.parse_encode",
            "std.math.fma",
            "std.messagepack.decode_encode",
            "std.protobuf.decode_message",
            "std.testing.generate_diff"
          ]
    ' "$config" >/dev/null || die "invalid coordinator registry"

jq -e \
    --slurpfile config "$config" \
    --slurpfile contract "$contract" \
    '
      .format == "tondo-stdlib-performance-report/1"
      and .protocol == "monotonic-3x9x3"
      and (.warmup_iterations == $config[0].protocol.warmup_iterations)
      and (.measurement_repetitions == $config[0].protocol.measurement_repetitions)
      and (.independent_processes == $config[0].protocol.independent_processes)
      and (.memory_bytes | type == "number" and . >= 0)
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
      and all(.measurements[];
        (.sample_count == $config[0].protocol.minimum_sample_count)
        and (.workload == "representative")
        and (.dimensions.tail_latency.p95 | type == "number")
        and (.dimensions.tail_latency.p99 | type == "number")
        and (.dimensions.throughput.median | type == "number")
      )
      and ([.measurements[].operation] | sort)
          == ([ $config[0].owners[] | select(.state == "captured-partial") | .operation ] | sort)
      and ([.measurements[] | {module, operation, workload}] | sort)
          == ([ $config[0].owners[]
                | select(.state == "captured-partial")
                | {module, operation, workload} ] | sort)
    ' "$report" >/dev/null || die "report does not satisfy the coordinated owner protocol"

echo "stdlib performance conformance: OK (5 captured owners; deferred owners and dimensions explicit)"
