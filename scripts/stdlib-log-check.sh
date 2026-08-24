#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_LOG_CONTRACT:-$root/testing/stdlib-log.json}"

die() {
    echo "std.log contract: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing owner contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "owner contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "owner contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-owner-contract/1"
  and .owner == "std.log"
  and .parent_owner == "std"
  and .edition == "0.1"
  and .phase == "STD-0.1B"
  and .task == "STD-LOG-001"
  and .status == "contract-locked"
  and .contract == "docs/contracts/stdlib-log.md"
  and .spec == "TONDO_STANDARD_LIBRARY_SPEC.md"
  and .language_spec == "TONDO_LANGUAGE_SPEC.md"
  and .layer == "B0"
  and .kind == "runtime-facing"
  and .target == "tondo-vm-hosted-and-native"
  and .dependencies == ["std.bytes", "std.encoding", "std.io", "std.path", "std.console", "std.fs", "std.net", "std.time"]
  and .capabilities.required == []
  and .capabilities.optional == ["console", "filesystem", "network", "civil-clock"]
  and .capabilities.source_sets.core == []
  and .capabilities.source_sets.console == ["console"]
  and .capabilities.source_sets.file == ["filesystem"]
  and .capabilities.source_sets.network == ["network"]
  and .capabilities.source_sets.timestamp == ["civil-clock"]
  and .capabilities.import_effect == "none"
  and .capabilities.ambient_lookup == false
  and .capabilities.compile_time_query == false
  and .capabilities.missing_capability == "static-E1008"
  and ((.capabilities.forbidden | unique | length) == (.capabilities.forbidden | length))
  and ((.capabilities.forbidden | index("global-logger")) != null)
  and ((.capabilities.forbidden | index("environment-config")) != null)
  and ((.capabilities.forbidden | index("unbounded-queue")) != null)
  and ((.capabilities.forbidden | index("hidden-thread")) != null)
  and ((.capabilities.forbidden | index("heuristic-redaction")) != null)
  and .event.levels == ["Trace", "Debug", "Info", "Warn", "Error"]
  and .event.ordering == "Trace-less-than-Debug-less-than-Info-less-than-Warn-less-than-Error"
  and .event.minimum_filter == "event-level-greater-than-or-equal-to-minimumLevel"
  and .event.fatal_level == "forbidden"
  and .event.timestamp == "explicit-optional-UtcDateTime-no-ambient-clock"
  and .values.variants == ["Null", "Bool", "Int", "UInt", "Float", "Text", "Bytes", "Array", "Object", "Redacted"]
  and .values.float_policy == "finite-only"
  and .values.bytes_policy == "preserve-and-json-base64"
  and .values.redacted_policy == "explicit-token-no-heuristic-scanning"
  and .fields.container == "ordered-unique-map"
  and .fields.duplicate == "reject-without-mutation"
  and .fields.text_order == "insertion"
  and .fields.json_order == "utf8-byte-lexicographic"
  and .fields.partial_mutation == false
  and .formats.closed == ["Text", "JsonLines"]
  and .formats.text.record == "one-utf8-line-per-event"
  and .formats.text.final_lf == true
  and .formats.json_lines.schema == "tondo-log-event-0.1/1"
  and .formats.json_lines.root_order == ["schema", "level", "target", "message", "time", "fields"]
  and .formats.json_lines.final_lf == true
  and .backpressure.policies == ["Block", "Reject", "Drop"]
  and .backpressure.block == "suspends-until-accepted-and-cancel-safe"
  and .backpressure.reject == "LogError.Backpressure-no-consume"
  and .backpressure.drop == "LogReceipt.Dropped-observable"
  and .backpressure.drop_oldest == "forbidden"
  and .backpressure.queue == "finite-explicit-capacity"
  and .backpressure.unbounded == false
  and .sinks.protocol == "LogSink"
  and .sinks.methods == ["write", "flush", "close"]
  and .sinks.write == "linearizable-and-suspendible"
  and .sinks.flush == "all-accepted-before-call-reach-writer"
  and .sinks.close == "terminal-drain-flush-consume"
  and .sinks.console.capability == "console"
  and .sinks.console.streams == ["Stdout", "Stderr"]
  and .sinks.file.capability == "filesystem"
  and .sinks.file.modes == ["Append", "Truncate"]
  and .sinks.file.rotation == "forbidden"
  and .sinks.network.capability == "network"
  and .sinks.network.dns == "forbidden"
  and .sinks.network.tls == "owned-by-std.net"
  and .sinks.network.retry == "forbidden"
  and .api.module == "std.log"
  and ([.api.functions[]] | sort) == ["ConsoleSink.create", "Fields.count", "Fields.empty", "Fields.get", "Fields.put", "FileSink.create", "LogEvent.create", "LogEvent.fields", "LogEvent.level", "LogEvent.message", "LogEvent.target", "LogEvent.timestamp", "LogLimits.create", "LogLimits.defaults", "Logger.close", "Logger.create", "Logger.emit", "Logger.enabled", "Logger.flush", "LoggerOptions.create", "SinkOptions.create"]
  and .api.annotations == []
  and .api.direct_call_waits == true
  and .api.explicit_await_direct_call == "forbidden"
  and .api.selectable_operations == []
  and .api.no_async_duplicate_api == true
  and .api.no_global_logger == true
  and (.surface.types | length) == 17
  and ([.surface.types[]] | sort) == ["Backpressure", "ConsoleSink", "ConsoleStream", "Fields", "FileMode", "FileSink", "LogError", "LogEvent", "LogFormat", "LogLevel", "LogLimits", "LogReceipt", "LogSink", "LogValue", "LoggerOptions", "Logger[S]", "SinkOptions"]
  and (.surface.trait_methods | length) == 3
  and ([.surface.trait_methods[].id] | unique | length) == 3
  and ([.surface.signatures | length] | first) == 21
  and ([.surface.signatures[].id] | unique | length) == 21
  and ([.surface.signatures[] | select(.id == "logger-emit" and .effect == "suspends")] | length) == 1
  and ([.surface.signatures[] | select(.id == "logger-flush" and .effect == "suspends")] | length) == 1
  and ([.surface.signatures[] | select(.id == "logger-close" and .effect == "suspends")] | length) == 1
  and ([.surface.signatures[] | select(.id == "console-sink-create" and .effect == "console")] | length) == 1
  and ([.surface.signatures[] | select(.id == "file-sink-create" and .effect == "filesystem")] | length) == 1
  and ([.surface.signatures[] | select(.effect == "selectable")] | length) == 0
  and .surface.direct_call_waits == true
  and .surface.explicit_await_direct_call == "forbidden"
  and .surface.selectable_operations == []
  and .surface.no_async_duplicate_api == true
  and .ownership.event_copyable == true
  and .ownership.event_sendable == true
  and .ownership.event_shareable == true
  and .ownership.logger_affine == true
  and .ownership.logger_sendable == true
  and .ownership.logger_shareable == true
  and .ownership.logger_copyable == false
  and .ownership.logger_cloneable == false
  and .ownership.sink_affine == true
  and .ownership.sink_copyable == false
  and .ownership.sink_cloneable == false
  and .ownership.sink_close_terminal == true
  and .ownership.event_no_mutable_alias == true
  and .ownership.failed_emit_event == "not-published-and-logical-owner-preserved"
  and .ownership.close_waits_in_flight == true
  and ([.limits[].id] | sort) == ["max_depth", "max_event_bytes", "max_field_key_bytes", "max_fields", "max_queue_entries", "max_string_bytes"]
  and .errors.type == "LogError"
  and .errors.location == "event-field-format-sink-boundary"
  and .errors.partial_success == false
  and (.errors.kinds | length) == 13
  and ((.errors.kinds | unique | length) == (.errors.kinds | length))
  and ((.errors.kinds | index("Backpressure")) != null)
  and ((.errors.kinds | index("CapabilityMissing")) != null)
  and ((.errors.kinds | index("NonFiniteValue")) != null)
  and .performance.filtered_event_cost == "level-comparison-only"
  and .performance.scalar_oracle == true
  and .performance.iterative_value_frames == true
  and .performance.simd_allowed_after_equivalence == true
  and .performance.queue_capacity == "finite-explicit"
  and .performance.concurrency == "per-sink-linearization"
  and .performance.claims_before_perf_gate == "forbidden"
  and ([.test_matrix[].id] | unique | length) == 10
  and all(.test_matrix[]; .required == true and (.observables | length) > 0)
  and ([.corpora[].id] | length) == 8
  and ([.corpora[].id] | unique | length) == 8
  and all(.corpora[]; .required == true and (.focus | length) > 0)
  and ((.exclusions | unique | length) == (.exclusions | length))
  and ([.promotion.gates[].id] == ["design", "implementation", "conformance", "performance", "promote"])
  and .promotion.next_blocks == ["DIAG-RUNTIME-001"]
  and .implementation.status == "pending-after-native-gate"
  and .implementation.public_api_promoted == false
  and .implementation.host == "required-after-native-gate"
  and .implementation.required_follow_ups == ["STD-LOG-IMPL-001", "STD-LOG-HOST-001", "STD-LOG-TEST-001", "STD-LOG-PERF-001", "STD-LOG-CONF-001", "STD-LOG-DOC-001"]
' "$contract" >/dev/null || die "invalid machine-readable std.log contract"

for path in \
    docs/contracts/stdlib-log.md \
    TONDO_STANDARD_LIBRARY_SPEC.md \
    TONDO_IMPLEMENTATION_TRACKER.md; do
    [[ -f "$root/$path" ]] || die "missing linked contract: $path"
done

for marker in \
    'STD-LOG-001' \
    'LogLevel' \
    'LogValue' \
    'LogEvent' \
    'LogSink' \
    'JsonLines' \
    'Backpressure' \
    'Block' \
    'Reject' \
    'Drop' \
    'LogReceipt.Dropped' \
    'ConsoleSink' \
    'FileSink' \
    'Logger.enabled' \
    'Logger.emit' \
    'No hay logger global' \
    'no se publica parcialmente' \
    'heurística'; do
    grep -Fq "$marker" "$root/docs/contracts/stdlib-log.md" \
        || die "contract document misses marker: $marker"
done

grep -Fq 'testing/stdlib-log.json' "$root/TONDO_STANDARD_LIBRARY_SPEC.md" \
    || die "main stdlib spec does not link the log registry"

echo "std.log contract: OK (structured events; explicit sinks; visible backpressure; no global state)"
