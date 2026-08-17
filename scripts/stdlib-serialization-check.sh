#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
contract="$root/testing/stdlib-serialization.json"
document="$root/docs/contracts/stdlib-serialization.md"

[[ -f "$contract" ]] || { echo "missing serialization owner contract" >&2; exit 1; }
[[ -s "$document" ]] || { echo "missing serialization owner document" >&2; exit 1; }
tail -c 1 "$contract" | cmp -s <(printf '\n') || {
    echo "serialization contract must end with LF" >&2
    exit 1
}
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || {
    echo "serialization contract contains CR or trailing whitespace" >&2
    exit 1
}

jq -e '
  .format == "tondo-stdlib-owner-contract/1"
  and .owner == "std.serialization"
  and .edition == "0.1"
  and .phase == "STD-0.1A"
  and .status == "closed-contract"
  and .contract == "docs/contracts/stdlib-serialization.md"
  and .traits == ["Encode[C]", "Decode[C]"]
  and .protocols == ["Encoder[C, E]", "Decoder[C, E]"]
  and .dispatch == "compile-time-static"
  and .dom.typed_encode == "static-derive-direct"
  and .dom.typed_decode == "static-derive-atomic"
  and .dom.dynamic_values == "std.serialization.Value-json-messagepack; protocol-owned-protobuf"
  and .dom.reflection == "forbidden"
  and .legacy_abi.status == "rust-internal-compatibility-only"
  and .legacy_abi.canonical_replacement == [
    "Encode[C]", "Decode[C]", "Encoder[C, E]", "Decoder[C, E]"
  ]
  and .legacy_abi.symbols == [
    "Serializer", "Deserializer", "Serialize", "Deserialize",
    "serialize_value", "deserialize_value", "JsonValue",
    "MessagePackValue"
  ]
  and .legacy_abi.tondo_public_surface == "forbidden"
  and .legacy_abi.public_api_audit == "excluded-with-reason"
  and .events == [
    "null", "bool", "int64", "uint64", "float32", "float64", "string", "bytes",
    "start-array", "end-array", "start-map", "map-key", "end-map",
    "start-record", "field", "end-record", "start-enum", "end-enum"
  ]
  and .containers.array_length == "optional-exact-count"
  and .containers.map_shape == "map-key-key-value"
  and .containers.map_keys == "arbitrary-event-value"
  and .containers.record_shape == "unique-field-then-value"
  and .containers.enum_shape == "zero-or-one-payload-value"
  and .containers.stack == "explicit-bounded-frames"
  and .containers.root == "exactly-one-value"
  and .payloads.string == "temporary-until-next-event"
  and .payloads.bytes == "temporary-until-next-event"
  and .payloads.materialization == "explicit-own"
  and .payloads.numeric_width == "preserve-float32-and-float64-bits"
  and .atomicity.decode == "publish-after-complete-validation"
  and .atomicity.writer_error == "terminal-no-partial-success"
  and .atomicity.reader_error == "terminal-no-partial-success"
  and .atomicity.derive_output == "atomic-source-publication"
  and .derive.provider == "build-only-hermetic"
  and .derive.output == "ordinary-tondo-impl"
  and .derive.field_order.encode == "declaration-order"
  and .derive.field_order.decode == "order-independent-known-fields"
  and .derive.private_fields == "explicit-authorized-view-only"
  and .derive.custom_wire_names == "manual-impl-or-explicit-dto"
  and .derive.protobuf_field_numbers == "explicit-@proto-number"
  and .derive.record_machine == "static-seen-slots-no-dom"
  and .derive.unknown_field_policy == "strict-UnknownField"
  and .derive.duplicate_field_policy == "DuplicateField"
  and .derive.option_presence == "missing-Option-is-none"
  and .derive.ignored_field_policy == "consume-and-publish-none"
  and .derive.codec_policies == [
    "@json(base64)-RFC4648-canonical",
    "@messagepack(binary)-native-bin",
    "@proto(number)-explicit-wire-token"
  ]
  and .derive.minimum_bounds == true
  and .derive.source_maps == true
  and (.limits | map(.id)) == [
    "max_input_bytes", "max_output_bytes", "max_depth", "max_events",
    "max_container_items", "max_string_bytes", "max_payload_bytes"
  ]
  and all(.limits[]; (.unit | length) > 0 and (.check | length) > 0)
  and .errors == [
    "UnexpectedEvent", "TypeMismatch", "MissingField", "DuplicateField",
    "UnknownField",
    "InvalidContainerLength", "LimitExceeded", "InvalidPath", "IoError"
  ]
  and (.test_matrix | map(.id)) == [
    "scalar-events", "arrays-and-maps", "records-and-enums", "chunking",
    "limits", "derive", "typed-path"
  ]
  and all(.test_matrix[]; (.observables | length) > 0)
  and (.promotion.gates | map(.id)) == ["design", "implementation", "integration", "promote"]
  and .promotion.next == "STD-JSON-IMPL-001"
' "$contract" >/dev/null

grep -q 'std.derive.serialization.Encode' "$document"
grep -q 'std.derive.serialization.Decode' "$document"
grep -q 'Value dinámico' "$document"
grep -q '@ignore' "$document"
grep -q 'MapKey' "$document"
grep -q 'StartRecord' "$document"
grep -q 'StartEnum' "$document"
grep -q 'fn base64' "$document"
grep -q 'cualquier orden' "$document"
grep -q 'Máquina wire estática' "$document"

jq -e '
  any(.owners[]; .id == "std.serialization"
    and .runtime.kind == "not-applicable"
    and (.runtime.reason | contains("legacy Rust ABI is compatibility-only"))
    and .legacy_abi.status == "excluded-with-reason"
    and .legacy_abi.reason == "Rust-only compatibility bridge; canonical Tondo surface is Encode/Decode")
' testing/stdlib-public-api-config.json >/dev/null

jq -e '
  ([.rows[] | select(.owner == "std.serialization")] | length) == 0
' testing/stdlib-public-api.json >/dev/null

echo "std.serialization owner contract: OK"
