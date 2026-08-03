#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
contract="$root/testing/stdlib-json.json"

if [[ ! -f "$contract" ]]; then
    echo "missing std.json owner contract: ${contract#"$root"/}" >&2
    exit 1
fi

if ! tail -c 1 "$contract" | cmp -s <(printf '\n'); then
    echo "std.json owner contract must end with one LF" >&2
    exit 1
fi

if grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null; then
    echo "std.json owner contract contains CR or trailing whitespace" >&2
    exit 1
fi

jq -e '
    def unique_values: length == (unique | length);

    .format == "tondo-stdlib-owner-contract/1"
    and .owner == "std.json"
    and .edition == "0.1"
    and .phase == "STD-0.1A"
    and .status == "draft-contract"
    and .wire.syntax == "RFC8259"
    and .wire.text_encoding == "utf-8"
    and .wire.whitespace == ["space", "tab", "line-feed", "carriage-return"]
    and .wire.single_document == true
    and .wire.comments == false
    and .wire.trailing_commas == false
    and .wire.nan_infinity == false
    and .wire.canonicalization == "RFC8785"
    and .values.kinds == ["null", "bool", "number", "string", "array", "object"]
    and .values.dynamic_type == "JsonValue"
    and .values.number_type == "JsonNumber"
    and .values.event_type == "JsonEvent"
    and .values.path_type == "JsonPath"
    and .values.object_representation == "ordered-unique-members"
    and .values.object_lookup == "key-equality-after-utf8-decoding"
    and .number.grammar == "RFC8259"
    and .number.representation == "validated-decimal-token"
    and .number.preserve_lexeme == true
    and .number.typed_conversion == "exact-mathematical-value-before-narrowing"
    and .number.float_policy == "finite-only"
    and .number.nonfinite_literals == "reject"
    and .number.negative_zero == "preserve-until-canonicalization"
    and .number.integer_conversion_intermediate == "never-float"
    and .typed.traits == ["Serialize", "Deserialize"]
    and .typed.dispatch == "compile-time-static"
    and .typed.decode_requires_dom == false
    and .typed.encode_requires_dom == false
    and .typed.reflection == "forbidden"
    and .typed.missing_option == "none"
    and .typed.ordinary_field_order == "declaration-order"
    and .typed.canonical_field_order == "RFC8785-property-order"
    and .typed.unknown_capture == "declared-extras-only"
    and .typed.parser_stack == "explicit-bounded-stack"
    and .streaming.event_kinds == [
        "start-array", "end-array", "start-object", "end-object", "key",
        "null", "bool", "number", "string"
    ]
    and .streaming.reader_inputs == ["Bytes", "Reader"]
    and .streaming.writer_outputs == ["BytesBuilder", "Writer"]
    and .streaming.event_payload_lifetime == "until-next-event"
    and .streaming.event_string_representation == "utf8-view"
    and .streaming.event_number_representation == "validated-number-view"
    and .streaming.materialization == "explicit-own"
    and .streaming.reader_state == "bounded-explicit-stack"
    and .streaming.writer_state == "bounded-explicit-stack"
    and .streaming.typed_mode == "direct-events-no-dom"
    and .streaming.partial_failure == "terminal-error-no-partial-success"
    and .streaming.fragmentation_classes == [
        "one-byte", "token-boundary", "utf8-boundary", "large-chunk"
    ]
    and .policies.duplicate_keys.default == "reject"
    and .policies.duplicate_keys.allowed == ["reject", "first", "last"]
    and .policies.duplicate_keys.last_replacement_position == "retain-first-position"
    and .policies.unknown_fields.default == "reject"
    and .policies.unknown_fields.allowed == ["reject", "ignore", "capture"]
    and .policies.unknown_fields.capture_requires == "declared-extras-field"
    and .policies.missing_fields.required == "error"
    and .policies.missing_fields.option == "none"
    and .policies.missing_fields.implicit_default == "forbidden"
    and .policies.trailing_data == "reject"
    and .policies.ordinary_order == "declaration-or-insertion"
    and .policies.canonical_order == "RFC8785"
    and (.limits | map(.id)) == [
        "max_document_bytes", "max_depth", "max_array_items",
        "max_object_members", "max_string_bytes", "max_number_bytes",
        "max_events", "max_output_bytes"
    ]
    and all(.limits[]; (.id | length) > 0 and (.unit | length) > 0
        and (.scope | length) > 0 and (.check | length) > 0)
    and .errors == [
        "InvalidUtf8", "InvalidSyntax", "UnexpectedEof", "InvalidEscape",
        "InvalidUnicodeScalar", "InvalidNumber", "DuplicateKey", "UnknownField",
        "MissingField", "TypeMismatch", "NumberRange", "LimitExceeded", "IoError",
        "TrailingData", "CanonicalizationError"
    ]
    and (.errors | unique_values)
    and .paths.root == "$"
    and .paths.segments == ["object-key", "array-index"]
    and .paths.object_rendering == "[escaped-key]"
    and .paths.array_rendering == "[decimal-index]"
    and .paths.position == "byte-offset-and-line-column"
    and .paths.secret_policy == "no-input-snippet"
    and (.corpora | map(.id)) == [
        "rfc8259-valid", "rfc8259-invalid", "rfc8785-canonical", "unicode-escapes",
        "number-boundaries", "limits-and-truncation", "typed-policies", "stream-fragments"
    ]
    and all(.corpora[]; .required == true and (.focus | length) > 0 and (.focus | unique_values))
    and .corpora[0].source == "https://www.rfc-editor.org/rfc/rfc8259.html"
    and .corpora[1].source == "https://www.rfc-editor.org/rfc/rfc8259.html"
    and .corpora[2].source == "https://www.rfc-editor.org/rfc/rfc8785.html"
    and all(.corpora[3:][]; .source == "owner-generated")
    and (.test_matrix | map(.id)) == [
        "syntax", "unicode", "numbers", "dynamic-value", "typed-direct",
        "streaming", "limits", "canonical", "interoperability"
    ]
    and all(.test_matrix[]; .required == true and (.observables | length) > 0
        and (.observables | unique_values))
    and (.promotion.gates | map(.id)) == ["design", "implementation", "conformance", "promote"]
    and .promotion.gates[0].requires == [
        "wire-rules", "type-model", "policy-defaults", "limit-identities"
    ]
    and .promotion.gates[1].requires == [
        "typed-no-dom", "explicit-parser-stack", "bounded-reader-writer", "stable-errors"
    ]
    and .promotion.gates[2].requires == [
        "rfc-corpus", "stream-fragment-equivalence", "canonical-vectors",
        "interop-independent-implementations"
    ]
    and .promotion.gates[3].requires == [
        "no-dom-typed-proof", "limits-proven-finite", "reviewed-errors-and-paths",
        "STD-PERF-001-report"
    ]
    and .promotion.next_coordination == "STD-CODEC-CONF-001"
' "$contract" >/dev/null

echo "std.json owner contract: OK"
