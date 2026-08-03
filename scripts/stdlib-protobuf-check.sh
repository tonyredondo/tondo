#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
contract="$root/testing/stdlib-protobuf.json"

if [[ ! -f "$contract" ]]; then
    echo "missing std.protobuf owner contract: ${contract#"$root"/}" >&2
    exit 1
fi

if ! tail -c 1 "$contract" | cmp -s <(printf '\n'); then
    echo "std.protobuf owner contract must end with one LF" >&2
    exit 1
fi

if grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null; then
    echo "std.protobuf owner contract contains CR or trailing whitespace" >&2
    exit 1
fi

jq -e '
    def unique_values: length == (unique | length);

    .format == "tondo-stdlib-owner-contract/1"
    and .owner == "std.protobuf"
    and .edition == "0.1"
    and .phase == "STD-0.1A"
    and .status == "draft-contract"
    and .scope.syntax == "proto3"
    and .scope.input_extension == ".proto"
    and .scope.schema_first == true
    and .scope.build_time_generator == true
    and .scope.runtime_codegen == false
    and .scope.runtime_reflection == false
    and .scope.unsupported == [
        "proto2", "editions", "services", "grpc", "protojson", "text-format"
    ]
    and .scope.imports == "declared-closed-graph"
    and .scope.well_known_types == "declared-input-only"
    and .scope.baseline_format == "toolchain-toml"
    and .scope.ambient_inputs == false
    and .wire.spec == "Protocol Buffers"
    and .wire.tag_formula == "(field_number << 3) | wire_type"
    and .wire.field_number_min == 1
    and .wire.field_number_max == 536870911
    and .wire.reserved_field_number_range == [19000, 19999]
    and .wire.known_wire_types == [0, 1, 2, 5]
    and .wire.unknown_group_wire_types == [3, 4]
    and .wire.varint_max_bytes == 10
    and .wire.length_prefix == "uint32-varint"
    and .wire.length_max_bytes == 2147483647
    and .wire.ordinary_accepts_non_minimal_varints == true
    and .wire.ordinary_preserves_unknown_records == true
    and .wire.ordinary_known_field_order == "field-number"
    and .wire.string_encoding == "utf-8"
    and .wire.bytes_encoding == "arbitrary-octets"
    and .generated.traits == ["Serialize", "Deserialize"]
    and .generated.dispatch == "compile-time-static"
    and .generated.message_kind == "nominal-record"
    and .generated.unknown_type == "UnknownFields"
    and .generated.unknown_representation == "owned-raw-record-sequence"
    and .generated.unknown_default == "preserve"
    and .generated.unknown_discard == "explicit-discardUnknown"
    and .generated.descriptor == "explicit-root-only"
    and .generated.api_hash == "generated-output"
    and .generated.output_identity == [
        "package", "logical-path", "fully-qualified-name", "field-number"
    ]
    and .generated.dynamic_dom == false
    and (.scalar_mapping | map(.proto)) == [
        "int32", "int64", "uint32", "uint64", "sint32", "sint64", "fixed32",
        "sfixed32", "fixed64", "sfixed64", "float", "double", "bool", "string",
        "bytes", "enum", "message"
    ]
    and all(.scalar_mapping[]; (.proto | length) > 0 and (.tondo | length) > 0
        and (.wire | length) > 0)
    and .presence.implicit_scalar == "value-default-and-omit-default"
    and .presence.optional_scalar == "T?"
    and .presence.message == "Message?"
    and .presence.repeated == "Array[T]-empty-not-none"
    and .presence.map == "Map[K,V]-empty-not-none"
    and .presence.oneof == "generated-enum-with-None"
    and .presence.optional_message_same_as_implicit_message == true
    and .repeated.generated_type == "Array[T]"
    and .repeated.order == "preserve"
    and .repeated.numeric_packed_default == true
    and .repeated.decode_accepts == ["packed", "unpacked"]
    and .repeated.multiple_packed_records == "concatenate"
    and .repeated.packed_payload == "whole-elements-only"
    and .maps.generated_type == "Map[K,V]"
    and .maps.key_types == ["Bool", "Int32", "Int64", "UInt32", "UInt64", "String"]
    and .maps.key_restrictions == ["no-float", "no-bytes", "no-enum", "no-message", "no-map"]
    and .maps.wire_shape == "repeated-length-delimited-entry"
    and .maps.entry_key_field == 1
    and .maps.entry_value_field == 2
    and .maps.duplicate_policy == "last-wins"
    and .maps.ordinary_order == "not-guaranteed"
    and .maps.deterministic_order == "canonical-key-order"
    and .oneof.generated_type == "nominal-enum-with-None"
    and .oneof.repeated_allowed == false
    and .oneof.map_allowed == false
    and .oneof.setter_policy == "clear-previous"
    and .oneof.decode_policy == "last-member-wins"
    and .oneof.message_merge == "merge-same-member-before-publish"
    and .oneof.default_value_is_present == true
    and .enums.mode == "open"
    and .enums.wire_type == "Int32"
    and .enums.unknown_representation == "preserve-number"
    and .enums.known_projection == "known-returns-option"
    and .enums.repeated_unknowns == "remain-in-array"
    and .enums.map_value_unknowns == "remain-in-map-value"
    and .unknown_fields.record == "field-number-wire-type-tag-bytes-payload-bytes"
    and .unknown_fields.order == "capture-order"
    and .unknown_fields.groups == "preserve-matching-raw-group"
    and .unknown_fields.ordinary_encode == "known-fields-then-captured-unknowns"
    and .unknown_fields.deterministic_encode == "sort-after-known-fields"
    and .unknown_fields.payload == "exact-bytes"
    and .unknown_fields.drop == "explicit-only"
    and .streaming.schema_bound == true
    and .streaming.frame_required == true
    and .streaming.reader_inputs == ["Bytes", "Reader"]
    and .streaming.writer_outputs == ["BytesBuilder", "Writer"]
    and .streaming.event_kinds == [
        "start-message", "end-message", "field", "varint", "fixed32", "fixed64",
        "start-length-delimited", "end-length-delimited", "start-packed", "end-packed",
        "unknown-record"
    ]
    and .streaming.payload_lifetime == "until-next-event"
    and .streaming.materialization == "explicit-own"
    and .streaming.reader_state == "bounded-explicit-stack"
    and .streaming.writer_state == "bounded-explicit-stack"
    and .streaming.typed_mode == "direct-schema-dispatch-no-dom"
    and .streaming.partial_failure == "terminal-error-no-partial-success"
    and .streaming.fragmentation_classes == [
        "one-byte", "tag-boundary", "varint-boundary", "length-boundary", "payload-boundary", "large-chunk"
    ]
    and .policies.wire_type_mismatch.default == "preserve-unknown"
    and .policies.wire_type_mismatch.allowed == ["preserve-unknown", "reject"]
    and .policies.non_minimal_varint.default == "accept"
    and .policies.non_minimal_varint.allowed == ["accept", "reject"]
    and .policies.unknown_fields.default == "preserve"
    and .policies.unknown_fields.allowed == ["preserve", "discard"]
    and .policies.unknown_groups.default == "preserve"
    and .policies.unknown_groups.allowed == ["preserve", "reject"]
    and .policies.duplicate_singular == "last-wins"
    and .policies.duplicate_messages == "merge"
    and .policies.duplicate_oneof == "last-member-wins"
    and .policies.duplicate_maps == "last-wins"
    and .policies.bool_values.allowed == [0, 1]
    and .policies.bool_values.invalid_policy == "error"
    and .deterministic.operation == "encodeDeterministic"
    and .deterministic.universal_canonical == false
    and .deterministic.field_order == "ascending-field-number"
    and .deterministic.varints == "shortest-valid"
    and .deterministic.lengths == "shortest-valid"
    and .deterministic.repeated_order == "preserve"
    and .deterministic.packed_policy == "schema-policy"
    and .deterministic.map_key_order.bool == "false-before-true"
    and .deterministic.map_key_order.signed == "numeric-ascending"
    and .deterministic.map_key_order.unsigned == "numeric-ascending"
    and .deterministic.map_key_order.string == "utf8-byte-lexicographic"
    and .deterministic.oneof == "selected-field-number"
    and .deterministic.float_bits == "preserve-ieee-bits"
    and .deterministic.unknown_order == "field-number-wire-type-raw-bytes"
    and .deterministic.nested_messages == "recursive"
    and .deterministic.streaming_input == "schema-ordered-fields-and-sorted-maps"
    and .evolution.baseline == "declared-locked-toml"
    and .evolution.safe == [
        "add-field", "add-enum-value", "add-optional", "reserve-deleted-field",
        "toggle-packed-compatible-repeated-numeric"
    ]
    and .evolution.conditional == [
        "same-wire-family-type-change", "map-and-repeated-entry-equivalence",
        "enum-and-integer-wire-equivalence"
    ]
    and .evolution.unsafe == [
        "change-field-number", "reuse-field-number", "remove-reservation",
        "change-wire-family", "reuse-reserved-name", "change-map-key-type",
        "incompatible-repeated-change", "split-oneof", "merge-oneof",
        "change-oneof-membership"
    ]
    and .evolution.conditional_requires == "explicit-waiver"
    and .evolution.unsafe_result == "build-error"
    and .evolution.baseline_input == "no-ambient-descriptor"
    and (.limits | map(.id)) == [
        "max_schema_bytes", "max_imports", "max_generated_types", "max_generated_bytes",
        "max_message_bytes", "max_depth", "max_fields", "max_repeated_items",
        "max_map_entries", "max_string_bytes", "max_bytes_field_bytes", "max_packed_bytes",
        "max_unknown_bytes", "max_varint_bytes", "max_events", "max_output_bytes"
    ]
    and all(.limits[]; (.id | length) > 0 and (.unit | length) > 0
        and (.scope | length) > 0 and (.check | length) > 0)
    and .errors.wire == [
        "UnexpectedEof", "InvalidTag", "InvalidWireType", "InvalidVarint", "InvalidLength",
        "InvalidUtf8", "TypeMismatch", "InvalidPacked", "NumberRange", "InvalidFieldNumber",
        "InvalidGroup", "LimitExceeded", "IoError", "TrailingData", "SchemaMismatch"
    ]
    and .errors.build == [
        "ProtoSyntaxUnsupported", "ProtoImportNotDeclared", "ProtoNameCollision",
        "ProtoFieldNumberConflict", "ProtoReservedReuse", "ProtoSchemaDrift",
        "ProtoWireIncompatible", "ProtoGeneratorOutputCollision", "ProtoGenerationLimit"
    ]
    and (.errors.wire | unique_values)
    and (.errors.build | unique_values)
    and .paths.root == "$"
    and .paths.segments == [
        "message", "field-number", "repeated-index", "map-key", "map-value",
        "oneof-case", "unknown-field"
    ]
    and .paths.position == "byte-offset-and-schema-location"
    and .paths.secret_policy == "no-payload-snippet"
    and (.corpora | map(.id)) == [
        "protobuf-official-encoding", "protobuf-official-proto3", "protobuf-official-enums",
        "scalar-wire-boundaries", "presence-and-oneof", "repeated-packed-and-maps",
        "unknown-fields-and-groups", "stream-fragments-and-limits", "schema-evolution",
        "deterministic-output"
    ]
    and all(.corpora[]; .required == true and (.focus | length) > 0
        and (.focus | unique_values))
    and .corpora[0].source == "https://protobuf.dev/programming-guides/encoding/"
    and .corpora[1].source == "https://protobuf.dev/programming-guides/proto3/"
    and .corpora[2].source == "https://protobuf.dev/programming-guides/enum/"
    and all(.corpora[3:][]; .source == "owner-generated")
    and (.test_matrix | map(.id)) == [
        "schema-inputs", "wire-model", "scalar-mapping", "presence", "repeated-and-packed",
        "maps", "open-enums", "unknown-fields", "typed-direct", "streaming", "limits",
        "evolution", "deterministic", "interoperability"
    ]
    and all(.test_matrix[]; .required == true and (.observables | length) > 0
        and (.observables | unique_values))
    and (.promotion.gates | map(.id)) == [
        "design", "implementation", "conformance", "evolution", "promote"
    ]
    and .promotion.gates[0].requires == [
        "proto3-model", "scalar-mapping", "presence-policy", "unknown-policy", "schema-identity"
    ]
    and .promotion.gates[1].requires == [
        "deterministic-generator", "typed-no-dom", "explicit-parser-stack",
        "bounded-reader-writer", "direct-schema-dispatch"
    ]
    and .promotion.gates[2].requires == [
        "official-wire-corpus", "packed-unpacked-equivalence", "unknown-enum-vectors",
        "interop-independent-implementations"
    ]
    and .promotion.gates[3].requires == [
        "safe-schema-vectors", "unsafe-schema-rejection", "reserved-number-proof",
        "unknown-field-round-trip"
    ]
    and .promotion.gates[4].requires == [
        "generated-output-stability", "wire-compatibility", "finite-limits", "STD-PERF-001-report"
    ]
    and .promotion.next_coordination == "STD-CODEC-CONF-001"
' "$contract" >/dev/null

echo "std.protobuf owner contract: OK"
