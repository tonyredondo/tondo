#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_CBOR_CONTRACT:-$root/testing/stdlib-cbor.json}"

die() {
    echo "std.cbor contract: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing owner contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "owner contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "owner contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-owner-contract/1"
  and .owner == "std.cbor"
  and .parent_owner == "std"
  and .edition == "0.1"
  and .phase == "STD-0.1B"
  and .task == "STD-CBOR-001"
  and .status == "contract-locked"
  and .contract == "docs/contracts/stdlib-cbor.md"
  and .spec == "TONDO_STANDARD_LIBRARY_SPEC.md"
  and .language_spec == "TONDO_LANGUAGE_SPEC.md"
  and .layer == "B8"
  and .kind == "runtime-facing"
  and .target == "tondo-vm-hosted-and-native"
  and .dependencies == ["std.serialization", "std.io", "std.bytes"]
  and .capabilities.required == []
  and .capabilities.optional == []
  and .capabilities.import_effect == "none"
  and .capabilities.ambient_lookup == false
  and ((.capabilities.forbidden | unique | length) == (.capabilities.forbidden | length))
  and ((.capabilities.forbidden | index("environment")) != null)
  and ((.capabilities.forbidden | index("includes")) != null)
  and ((.capabilities.forbidden | index("reflection-registry")) != null)
  and ((.capabilities.forbidden | index("timezone-lookup")) != null)
  and .wire.spec == "RFC 8949"
  and .wire.rfc == "8949"
  and .wire.encoding == "binary"
  and .wire.root == "single-data-item"
  and .wire.trailing_data == "reject"
  and .wire.major_types == ["unsigned-integer", "negative-integer", "byte-string", "text-string", "array", "map", "tag", "simple-or-float"]
  and .wire.length_forms == ["definite", "indefinite"]
  and .wire.indefinite_kinds == ["byte-string", "text-string", "array", "map"]
  and .wire.break == "only-closes-indefinite-frame"
  and .wire.text_encoding == "utf-8"
  and .wire.map_keys == "arbitrary-data-items"
  and .wire.simple_values == "preserve-well-formed-unassigned"
  and .wire.special_simple_values == ["false", "true", "null", "undefined"]
  and .wire.float_widths == ["float16", "float32", "float64"]
  and .wire.tags == "unsigned-64-bit-preserved"
  and .wire.ordinary_non_minimal == "accept"
  and .wire.ordinary_nan == "preserve-bits"
  and .values.dynamic_type == "CborValue"
  and .values.view_type == "CborValueView"
  and .values.raw_type == "CborRaw"
  and .values.map_representation == "ordered-pairs-arbitrary-keys"
  and .values.integer_kinds == ["UInt", "Negative"]
  and .values.float_kinds == ["Float16", "Float32", "Float64"]
  and .values.undefined_distinct_from_null == true
  and .values.shared_identity == false
  and .values.bignum_tags == "preserve-generic-tagged-bytes"
  and .api.module == "std.cbor"
  and ([.api.functions[]] | sort) == ["decode", "encode", "encodeDeterministic", "parse", "parseView", "raw", "rawUnchecked", "validate"]
  and .api.reader_methods == ["fromBytes", "fromReader", "next", "own", "finish"]
  and .api.writer_methods == ["toWriter", "write", "finish"]
  and .api.terminal_state == "error-or-finish-terminal"
  and .api.end_of_document == "next-returns-none-once"
  and .api.typed_dispatch == "Encode-Decode-static-no-dynamic-dom"
  and .api.annotations == ["@name", "@cbor", "@ignore"]
  and ([.surface.types[]] | sort) == ["Cbor", "CborDecodeOptions", "CborDuplicatePolicy", "CborEncodeOptions", "CborEntry", "CborError", "CborErrorKind", "CborEvent", "CborFloat16", "CborIndefinitePolicy", "CborLimits", "CborNonMinimalPolicy", "CborPath", "CborRaw", "CborReader", "CborTag", "CborUnknownTagPolicy", "CborValue", "CborValueView", "CborWriter"]
  and (.surface.signatures | length) == 21
  and ([.surface.signatures[].id] | unique | length) == 21
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
  and .streaming.event_model == "single-root-explicit-tags-and-indefinite-frames"
  and .streaming.reader_inputs == ["Bytes", "Reader"]
  and .streaming.writer_outputs == ["BytesBuilder", "Writer"]
  and .streaming.payload_lifetime == "until-next-event"
  and .streaming.materialization == "explicit-own"
  and .streaming.reader_state == "bounded-explicit-stack"
  and .streaming.writer_state == "bounded-explicit-stack"
  and .streaming.typed_mode == "direct-events-no-dynamic-dom"
  and .streaming.partial_failure == "terminal-error-no-partial-success"
  and .streaming.chunk_boundary_invariant == true
  and .streaming.empty_chunk == "no-state-change"
  and .streaming.indefinite_frames == ["bytes", "text", "array", "map"]
  and .streaming.break_validation == "only-current-indefinite-frame"
  and .streaming.finish_required == true
  and .streaming.error_state == "terminal"
  and .streaming.post_finish == "CborError.Closed"
  and .streaming.resource_limit_write == "atomic-no-state-change"
  and .streaming.partial_tondo_result == "never-published"
  and .streaming.stack == "explicit-bounded-frames-and-worklists"
  and .policies.dynamic_map_duplicates.default == "preserve"
  and .policies.typed_map_duplicates.default == "reject"
  and .policies.unknown_tags.default == "preserve"
  and .policies.non_minimal_decode.default == "accept"
  and .policies.indefinite_decode.default == "accept"
  and .policies.deterministic_maps.rule == "bytewise-lexicographic-deterministic-key-encoding"
  and .policies.deterministic_maps.collision == "reject"
  and .policies.deterministic_indefinite == "reject"
  and .policies.deterministic_nan == "one-quiet-nan-float16"
  and .deterministic.operation == "encodeDeterministic"
  and .deterministic.universal_canonical == false
  and .deterministic.preferred_integer_and_length == true
  and .deterministic.preferred_tag_argument == true
  and .deterministic.indefinite_items == "reject"
  and .deterministic.float_rule == "shortest-width-preserving-value-and-sign"
  and .deterministic.nan_rule == "one-quiet-nan-float16-0x7e00"
  and .deterministic.negative_zero == "preserve"
  and .deterministic.array_order == "preserve"
  and .deterministic.tag_order == "preserve-nesting"
  and .deterministic.map_order == "deterministic-key-encoding-bytes"
  and .deterministic.key_collision == "reject"
  and .deterministic.streaming_input == "already-sorted-keys-and-definite-frames"
  and ([.limits[].id] | sort) == ["max_array_items", "max_byte_string_bytes", "max_chunks", "max_depth", "max_document_bytes", "max_events", "max_map_pairs", "max_output_bytes", "max_simple_values", "max_string_bytes", "max_tags", "vm_heap"]
  and .errors.type == "CborError"
  and .errors.location == "half-open-UTF-8-byte-span-plus-stable-path"
  and .errors.path == "ArrayIndex-MapEntry-MapKey-MapValue-Tag"
  and .errors.partial_success == false
  and (.errors.kinds | length) == 25
  and ((.errors.kinds | unique | length) == (.errors.kinds | length))
  and .performance.scalar_oracle == true
  and .performance.simd_allowed_after_equivalence == true
  and .performance.dispatch == "target-declared-and-input-size-based"
  and .performance.parser_stack == "explicit-worklist"
  and .performance.streaming_allocation == "bounded-by-chunk-and-limits"
  and .performance.claims_before_perf_gate == "forbidden"
  and ([.test_matrix[].id] | unique | length) == 10
  and all(.test_matrix[]; .required == true and (.observables | length) > 0)
  and ([.corpora[].id] | length) == 7
  and ([.corpora[].id] | unique | length) == 7
  and ([.corpora[].id] | unique) == ["deterministic-vectors", "indefinite-and-fragments", "integer-float-boundaries", "malformed-and-limits", "maps-and-duplicates", "rfc-8949-wire-model", "tags-and-undefined"]
  and all(.corpora[]; .source == "owner-generated" or (.source | startswith("https://www.rfc-editor.org/")))
  and all(.corpora[]; .required == true and (.focus | length) > 0)
  and ((.exclusions | unique | length) == (.exclusions | length))
  and ([.promotion.gates[].id] == ["design", "implementation", "conformance", "performance", "promote"])
  and .implementation.status == "pending-after-native-gate"
  and .implementation.public_api_promoted == false
  and .implementation.host == "required-after-native-gate"
  and .implementation.required_follow_ups == ["STD-CBOR-IMPL-001", "STD-CBOR-TEST-001", "STD-CBOR-PERF-001", "STD-CBOR-CONF-001", "STD-CBOR-DOC-001"]
  and .promotion.next_blocks == ["STD-REGEX-001", "STD-ID-001", "STD-LOG-001", "DIAG-RUNTIME-001"]
' "$contract" >/dev/null || die "invalid machine-readable std.cbor contract"

for path in \
    docs/contracts/stdlib-cbor.md \
    TONDO_STANDARD_LIBRARY_SPEC.md \
    TONDO_IMPLEMENTATION_TRACKER.md; do
    [[ -f "$root/$path" ]] || die "missing linked contract: $path"
done

for marker in \
    'STD-CBOR-001' \
    'CborValue' \
    'CborTag' \
    'CborFloat16' \
    'CborEvent' \
    'CborLimits' \
    'encodeDeterministic' \
    'CborReader.fromReader' \
    'CborWriter.toWriter' \
    'StartBytes' \
    'StartText' \
    'Negative(UInt64)' \
    'IndefiniteNotAllowed' \
    'DeterministicKeyCollision' \
    'RFC 8949' \
    'frames y worklists explícitos' \
    'tondo.toml' \
    'CborRaw' \
    'NoProgress'; do
    grep -Fq "$marker" "$root/docs/contracts/stdlib-cbor.md" \
        || die "contract document misses marker: $marker"
done

grep -Fq 'testing/stdlib-cbor.json' "$root/TONDO_STANDARD_LIBRARY_SPEC.md" \
    || die "main stdlib spec does not link the CBOR registry"

echo "std.cbor contract: OK (RFC 8949; tags; indefinite frames; deterministic mode; bounded streaming)"
