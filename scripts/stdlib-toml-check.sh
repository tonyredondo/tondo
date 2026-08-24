#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_TOML_CONTRACT:-$root/testing/stdlib-toml.json}"

die() {
    echo "std.toml contract: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing owner contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "owner contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "owner contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-owner-contract/1"
  and .owner == "std.toml"
  and .parent_owner == "std"
  and .edition == "0.1"
  and .phase == "STD-0.1B"
  and .task == "STD-TOML-001"
  and .status == "contract-locked"
  and .contract == "docs/contracts/stdlib-toml.md"
  and .spec == "TONDO_STANDARD_LIBRARY_SPEC.md"
  and .language_spec == "TONDO_LANGUAGE_SPEC.md"
  and .layer == "B8"
  and .kind == "runtime-facing"
  and .target == "tondo-vm-hosted-and-native"
  and .dependencies == ["std.serialization", "std.io", "std.bytes", "std.time"]
  and .capabilities.required == []
  and .capabilities.optional == []
  and .capabilities.import_effect == "none"
  and .capabilities.ambient_lookup == false
  and ((.capabilities.forbidden | unique | length) == (.capabilities.forbidden | length))
  and ((.capabilities.forbidden | index("environment")) != null)
  and ((.capabilities.forbidden | index("timezone-lookup")) != null)
  and ((.capabilities.forbidden | index("includes")) != null)
  and .wire.toml_version == "1.1.0"
  and .wire.encoding == "UTF-8"
  and .wire.case_sensitive == true
  and .wire.newlines == ["LF", "CRLF"]
  and .wire.whitespace == ["space", "tab"]
  and .wire.comments == "discarded"
  and .wire.documents == "single-root-no-markers"
  and .wire.keys == ["bare", "quoted", "dotted"]
  and .wire.strings == ["basic", "multiline-basic", "literal", "multiline-literal"]
  and .wire.escapes == ["b", "t", "n", "f", "r", "e", "xHH", "uHHHH", "UHHHHHHHH", "quote", "backslash"]
  and .wire.integers == ["decimal", "hexadecimal", "octal", "binary"]
  and .wire.floats == ["fraction", "exponent", "inf", "nan"]
  and .wire.date_time == ["offset-date-time", "local-date-time", "local-date", "local-time"]
  and .wire.arrays == "ordered-heterogeneous-trailing-comma"
  and .wire.inline_tables == "closed-trailing-comma"
  and .wire.tables == ["table", "array-of-tables"]
  and .surface.types == ["Toml", "TomlOffsetDateTime", "TomlValue", "TomlValueView", "TomlScalar", "TomlEvent", "TomlLimits", "TomlOptions", "TomlPathSegment", "TomlSpan", "TomlErrorKind", "TomlError", "TomlReader", "TomlWriter"]
  and (.surface.signatures | length) == 19
  and ([.surface.signatures[].id] | unique | length) == 19
  and all(.surface.signatures[]; (.signature | type == "string" and length > 0) and (.kind | type == "string" and length > 0) and (.effect | type == "string" and length > 0))
  and ([.surface.signatures[] | select(.effect == "suspends") | .id] | sort) == ["reader-finish", "reader-from-reader", "reader-next", "writer-finish", "writer-to-writer", "writer-write"]
  and .surface.direct_call_waits == false
  and .surface.explicit_await_direct_call == "forbidden"
  and .surface.explicit_await_join == "unchanged-by-language"
  and .surface.inference_by_name == false
  and .surface.bodyless_requires_effect == true
  and .surface.bodyful_inference == "allowed"
  and .surface.selectable_operations == []
  and .surface.no_async_duplicate_api == true
  and .dynamic_model.root == "ordered-table"
  and .dynamic_model.table == "ordered-map-string-keyed"
  and .dynamic_model.array == "ordered-heterogeneous-values"
  and .dynamic_model.integer == "Int64-or-UInt64-lossless"
  and .dynamic_model.float == "Float64-including-inf-and-nan"
  and .dynamic_model.offset_datetime == "TomlOffsetDateTime-fixed-offset"
  and .dynamic_model.shared_identity == false
  and .dynamic_model.binary == "not-a-wire-type"
  and .date_time.fraction_digits == "1..9"
  and .date_time.more_than_nine_digits == "reject"
  and .date_time.timezone_lookup == false
  and .date_time.clock_capabilities == []
  and .duplicate_semantics.key == "reject"
  and .duplicate_semantics.table == "reject"
  and .duplicate_semantics.array_table_row == "append-in-source-order"
  and .duplicate_semantics.inline_table_extension == "reject"
  and .duplicate_semantics.partial_success == false
  and .streaming.materialized_is_collector == true
  and .streaming.chunk_boundary_invariant == true
  and .streaming.single_root == true
  and .streaming.event_model == "explicit-key-table-array-inline-table-frames"
  and .streaming.reader_input == "std.io.Reader-until-EOF"
  and .streaming.writer_output == "std.io.Writer-via-writeAll"
  and .streaming.empty_chunk == "no-state-change"
  and .streaming.finish_required == true
  and .streaming.error_state == "terminal"
  and .streaming.post_finish == "TomlError.Closed"
  and .streaming.resource_limit_write == "atomic-no-state-change"
  and .streaming.partial_tondo_result == "never-published"
  and .streaming.stack == "explicit-bounded-frames-and-worklists"
  and .ownership.value_owner == "TomlValue"
  and .ownership.options_copy == true
  and .ownership.reader_writer_affine == true
  and .ownership.reader_writer_copy == false
  and .ownership.reader_writer_share == false
  and .ownership.reader_writer_send == true
  and .ownership.view_borrow == "ends-at-reader-advance-or-operation-return"
  and .ownership.input_alias == "never-retained"
  and .ownership.comments_storage == "discarded"
  and .ownership.table_identity == "logical-values-no-shared-mutable-memory"
  and ([.limits[].id] | sort) == ["max_array_elements", "max_array_table_rows", "max_depth", "max_input_bytes", "max_key_bytes", "max_nodes", "max_path_segments", "max_scalar_bytes", "max_string_bytes", "max_tables", "vm_heap"]
  and .errors.type == "TomlError"
  and .errors.location == "half-open-UTF-8-byte-span-plus-one-based-line-column"
  and .errors.path == "stable-Key-or-Index-array"
  and .errors.partial_success == false
  and (.errors.kinds | length) == 36
  and ((.errors.kinds | unique | length) == (.errors.kinds | length))
  and .performance.scalar_oracle == true
  and .performance.simd_allowed_after_equivalence == true
  and .performance.dispatch == "target-declared-and-chunk-size-based"
  and .performance.parser_stack == "explicit-worklist"
  and .performance.streaming_allocation == "bounded-by-chunk-and-limits"
  and .performance.claims_before_perf_gate == "forbidden"
  and ([.test_matrix[].id] | unique | length) == 8
  and all(.test_matrix[]; .required == true and (.observables | length) > 0)
  and (([.corpora[].id] | unique | length) == (.corpora | length))
  and ([.corpora[].id] | unique) == ["date-time-and-numeric-boundaries", "fragmentation-and-adversarial-limits", "invalid-duplicates-and-security", "toml-1.1-valid"]
  and all(.corpora[]; .source == "owner-generated" and .required == true and (.focus | length) > 0)
  and ((.exclusions | unique | length) == (.exclusions | length))
  and ([.promotion.gates[].id] == ["design", "implementation", "conformance", "performance", "promote"])
  and .implementation.status == "pending-after-native-gate"
  and .implementation.public_api_promoted == false
  and .implementation.host == "required-after-native-gate"
  and .implementation.required_follow_ups == ["STD-TOML-IMPL-001", "STD-TOML-TEST-001", "STD-TOML-PERF-001", "STD-TOML-CONF-001", "STD-TOML-DOC-001"]
  and .promotion.next_blocks == ["STD-LOG-001", "DIAG-RUNTIME-001"]
' "$contract" >/dev/null || die "invalid machine-readable std.toml contract"

for path in \
    docs/contracts/stdlib-toml.md \
    TONDO_STANDARD_LIBRARY_SPEC.md \
    TONDO_IMPLEMENTATION_TRACKER.md; do
    [[ -f "$root/$path" ]] || die "missing linked contract: $path"
done

for marker in \
    'STD-TOML-001' \
    'pub type TomlValue' \
    'pub type TomlOffsetDateTime' \
    'pub enum TomlEvent' \
    'pub type TomlLimits' \
    'pub fn parseView' \
    'pub fn encodeCanonical' \
    'TomlReader.fromReader' \
    'TomlWriter.toWriter' \
    'TomlLimits.maxArrayTableRows' \
    'TomlError.span' \
    'DateTimePrecision' \
    'InlineTableExtension' \
    'TOML v1.1.0' \
    'frames/worklists explícitos' \
    'tondo.toml' \
    'NoProgress'; do
    grep -Fq "$marker" "$root/docs/contracts/stdlib-toml.md" \
        || die "contract document misses marker: $marker"
done

grep -Fq 'testing/stdlib-toml.json' "$root/TONDO_STANDARD_LIBRARY_SPEC.md" \
    || die "main stdlib spec does not link the TOML registry"

echo "std.toml contract: OK (TOML 1.1.0; typed/dynamic/streaming; spans; toolchain boundary)"
