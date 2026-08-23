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
      and .status == "promoted"
      and .contract == "testing/stdlib-performance.json"
      and .report == "target/reliability/evidence/stdlib-performance-report.json"
      and (.protocol.clock == "monotonic")
      and (.protocol.warmup_iterations == $contract[0].measurement.warmup_iterations)
      and (.protocol.measurement_repetitions == $contract[0].measurement.measurement_repetitions)
      and (.protocol.independent_processes == $contract[0].measurement.independent_processes)
      and (.protocol.minimum_sample_count == $contract[0].measurement.minimum_sample_count)
      and (.protocol.required_environment == $contract[0].measurement.recorded_environment)
      and (.measured_dimensions == ($contract[0].dimensions | map(.id)))
      and (.deferred_dimensions == [])
      and (([.owners[].id] | unique | sort) == (
        (($implementation[0].owners | map(.id))
          + ($contract[0].owner_groups | map(.owners[]) | unique))
        | unique | sort
      ))
      and all(.owners[];
        (.group | test("^a[0-4]-[a-z-]+$"))
        and (.state | ["captured", "not-applicable"] | index(.) != null)
        and (if .state == "captured" then
          (.operation | test("^std\\.[a-z]+\\.[a-z_]+$"))
          and (.module | type == "string" and length > 0)
          and ((.workloads | sort) == ($contract[0].workload_classes | map(.id) | sort))
          and (.oracle == "portable-scalar")
          and ((.dimensions | sort) == ($contract[0].dimensions | map(.id) | sort))
          and (.promotion == "reviewed-baseline")
        else
          (.reason | type == "string" and length > 0)
          and (.basis == "PERF-001 target-qualified compiler/VM campaign"
               or .basis == "PERF-001 target-qualified hosted campaign"
               or .basis == "PERF-001 target-qualified compile campaign")
          and (has("operation") | not)
        end)
      )
      and ([.owners[] | select(.state == "captured") | .operation] | sort) == [
        "std.bytes.copy_hash",
        "std.format.join",
        "std.io.read_write_all",
        "std.json.parse_encode",
        "std.math.fma",
        "std.messagepack.decode_encode",
        "std.path.lexical",
        "std.protobuf.decode_message",
        "std.serialization.events",
        "std.testing.generate_diff"
      ]
    ' "$config" >/dev/null || die "invalid promoted coordinator registry"

jq -e \
    --slurpfile config "$config" \
    --slurpfile contract "$contract" \
    '
      . as $report
      | .format == "tondo-stdlib-performance-report/1"
      and .protocol == "monotonic-3x9x3"
      and (.warmup_iterations == $config[0].protocol.warmup_iterations)
      and (.measurement_repetitions == $config[0].protocol.measurement_repetitions)
      and (.independent_processes == $config[0].protocol.independent_processes)
      and (.measured_dimensions == $config[0].measured_dimensions)
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
      and (.campaign.allocation_observation == "logical-owned-buffer")
      and (.campaign.code_size_bytes | type == "number" and . > 0)
      and (.campaign.compile_time_ms | type == "number" and . >= 0)
      and (.campaign.startup_samples_us | length == 3)
      and (.campaign.peak_memory_samples_bytes | length == 3)
      and all(.measurements[];
        (.sample_count == $config[0].protocol.minimum_sample_count)
        and (.workload | ["adversarial", "empty", "fragmented_stream", "large", "representative", "small"] | index(.) != null)
        and (.dimensions | keys_unsorted | sort) == ($config[0].measured_dimensions | sort)
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
      and ([.measurements[].operation] | unique | sort)
          == ([ $config[0].owners[] | select(.state == "captured") | .operation ] | sort)
      and all($config[0].owners[] | select(.state == "captured");
        (.operation) as $operation
        | ([ $report.measurements[] | select(.operation == $operation) | .workload ] | sort)
          == (.workloads | sort)
      )
    ' "$report" >/dev/null || die "report does not satisfy the promoted owner protocol"

echo "stdlib performance conformance: OK (10 captured owners, 12 normative not-applicable owners, all eight dimensions)"
