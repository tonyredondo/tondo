#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
contract="$root/testing/stdlib-messagepack.json"

if [[ ! -f "$contract" ]]; then
    echo "missing std.messagepack owner contract: ${contract#"$root"/}" >&2
    exit 1
fi

if ! tail -c 1 "$contract" | cmp -s <(printf '\n'); then
    echo "std.messagepack owner contract must end with one LF" >&2
    exit 1
fi

if grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null; then
    echo "std.messagepack owner contract contains CR or trailing whitespace" >&2
    exit 1
fi

jq -e '
    def unique_values: length == (unique | length);

    .format == "tondo-stdlib-owner-contract/1"
    and .owner == "std.messagepack"
    and .edition == "0.1"
    and .phase == "STD-0.1A"
    and .status == "draft-contract"
    and .wire.spec == "MessagePack"
    and .wire.data_model == [
        "nil", "bool", "int", "uint", "float32", "float64", "str", "bin",
        "array", "map", "ext"
    ]
    and .wire.text_encoding == "utf-8"
    and .wire.ordinary_accepts_non_minimal == true
    and .wire.ordinary_preserves_order == true
    and .wire.unknown_ext == "preserve"
    and .wire.timestamp_type_code == -1
    and .values.dynamic_type == "MessagePackValue"
    and .values.ext_type == "Ext"
    and .values.timestamp_type == "MessagePackTimestamp"
    and .values.map_representation == "ordered-pairs-arbitrary-keys"
    and .values.integer_kinds == ["Int", "UInt"]
    and .values.float_kinds == ["Float32", "Float64"]
    and .values.string_type == "utf8-str"
    and .values.binary_type == "bin-bytes"
    and .ext.type_code == "signed-int8"
    and .ext.payload == "Bytes"
    and .ext.fixed_lengths == [1, 2, 4, 8, 16]
    and .ext.variable_forms == ["ext8", "ext16", "ext32"]
    and .ext.unknown_policy.default == "preserve"
    and .ext.unknown_policy.allowed == ["preserve", "reject"]
    and .ext.timestamp_conversion == "explicit-only"
    and .ext.reflection == "forbidden"
    and .typed.traits == ["Serialize", "Deserialize"]
    and .typed.dispatch == "compile-time-static"
    and .typed.decode_requires_dynamic_dom == false
    and .typed.encode_requires_dynamic_dom == false
    and .typed.map_duplicate_default == "reject"
    and .typed.integer_conversion == "exact-before-narrowing"
    and .typed.parser_stack == "explicit-bounded-stack"
    and .streaming.event_kinds == [
        "nil", "bool", "int", "uint", "float32", "float64", "string", "binary",
        "start-array", "end-array", "start-map", "map-key", "end-map", "ext"
    ]
    and .streaming.reader_inputs == ["Bytes", "Reader"]
    and .streaming.writer_outputs == ["BytesBuilder", "Writer"]
    and .streaming.payload_lifetime == "until-next-event"
    and .streaming.materialization == "explicit-own"
    and .streaming.reader_state == "bounded-explicit-stack"
    and .streaming.writer_state == "bounded-explicit-stack"
    and .streaming.typed_mode == "direct-events-no-dynamic-dom"
    and .streaming.partial_failure == "terminal-error-no-partial-success"
    and .streaming.fragmentation_classes == [
        "one-byte", "tag-boundary", "payload-boundary", "large-chunk"
    ]
    and .policies.dynamic_map_duplicates.default == "preserve"
    and .policies.dynamic_map_duplicates.allowed == ["preserve", "reject", "first", "last"]
    and .policies.dynamic_map_duplicates.last_replacement_position == "retain-first-position"
    and .policies.typed_map_duplicates.default == "reject"
    and .policies.typed_map_duplicates.allowed == ["reject", "first", "last"]
    and .policies.non_minimal_decode.default == "accept"
    and .policies.non_minimal_decode.allowed == ["accept", "reject"]
    and .policies.unknown_extensions.default == "preserve"
    and .policies.unknown_extensions.allowed == ["preserve", "reject"]
    and .policies.deterministic_map.rule == "sort-by-deterministic-key-bytes"
    and .policies.deterministic_map.collision == "reject"
    and .policies.nan.ordinary == "preserve-bits"
    and .policies.nan.deterministic == "one-quiet-nan"
    and .policies.zero_sign.ordinary == "preserve"
    and .policies.zero_sign.deterministic == "preserve"
    and .deterministic.operation == "encodeDeterministic"
    and .deterministic.universal_canonical == false
    and .deterministic.integer_lengths == "shortest-valid"
    and .deterministic.float32_rule == "exact-and-sign-preserving-else-float64"
    and .deterministic.nan_rule == "one-quiet-nan"
    and .deterministic.array_order == "preserve"
    and .deterministic.extension_order == "preserve-type-and-payload"
    and .deterministic.map_order == "deterministic-key-encoding-bytes"
    and .deterministic.key_collision == "reject"
    and .deterministic.streaming_input == "already-sorted-keys"
    and (.limits | map(.id)) == [
        "max_document_bytes", "max_depth", "max_array_items", "max_map_pairs",
        "max_string_bytes", "max_binary_bytes", "max_ext_bytes", "max_events",
        "max_output_bytes"
    ]
    and all(.limits[]; (.id | length) > 0 and (.unit | length) > 0
        and (.scope | length) > 0 and (.check | length) > 0)
    and .errors == [
        "UnexpectedEof", "InvalidTag", "InvalidUtf8", "InvalidLength",
        "NonMinimalEncoding", "InvalidExtension", "TypeMismatch", "DuplicateKey",
        "NumberRange", "DeterministicKeyCollision", "OutOfOrderKey", "LimitExceeded",
        "IoError", "TrailingData"
    ]
    and (.errors | unique_values)
    and .paths.root == "$"
    and .paths.segments == ["array-index", "map-entry", "map-key", "map-value"]
    and .paths.position == "byte-offset"
    and .paths.secret_policy == "no-payload-snippet"
    and (.corpora | map(.id)) == [
        "msgpack-spec-model", "minimal-encodings", "float-bits", "utf8-and-binary",
        "arbitrary-map-keys", "extensions-and-timestamp", "stream-fragments",
        "limits-and-truncation", "deterministic-encoding"
    ]
    and all(.corpora[]; .required == true and (.focus | length) > 0
        and (.focus | unique_values))
    and .corpora[0].source == "https://github.com/msgpack/msgpack/blob/master/spec.md"
    and all(.corpora[1:][]; .source == "owner-generated")
    and (.test_matrix | map(.id)) == [
        "wire-model", "minimal-forms", "numeric-bits", "text-and-binary", "maps",
        "typed-direct", "streaming", "extensions", "limits", "deterministic",
        "interoperability"
    ]
    and all(.test_matrix[]; .required == true and (.observables | length) > 0
        and (.observables | unique_values))
    and (.promotion.gates | map(.id)) == ["design", "implementation", "conformance", "promote"]
    and .promotion.gates[0].requires == [
        "wire-model", "dynamic-map-policy", "deterministic-rule", "limit-identities"
    ]
    and .promotion.gates[1].requires == [
        "typed-no-dom", "explicit-parser-stack", "bounded-reader-writer", "stable-ext-policy"
    ]
    and .promotion.gates[2].requires == [
        "spec-corpus", "fragment-equivalence", "deterministic-vectors",
        "interop-independent-implementations"
    ]
    and .promotion.gates[3].requires == [
        "arbitrary-key-proof", "unknown-ext-preservation", "limits-proven-finite",
        "STD-PERF-001-report"
    ]
    and .promotion.next_coordination == "STD-CODEC-CONF-001"
' "$contract" >/dev/null

echo "std.messagepack owner contract: OK"
