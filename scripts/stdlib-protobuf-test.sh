#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_root="${TMPDIR:-/tmp}"
tmp_dir="$(mktemp -d "$tmp_root/tondo-stdlib-protobuf-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.protobuf owner tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.owner = "std.invalid"' testing/stdlib-protobuf.json \
    > "$tmp_dir/invalid-owner.json"
expect_failure invalid-owner env TONDO_STDLIB_PROTOBUF_CONTRACT="$tmp_dir/invalid-owner.json" \
    scripts/stdlib-protobuf-check.sh

jq '.test_matrix = []' testing/stdlib-protobuf.json \
    > "$tmp_dir/missing-test-matrix.json"
expect_failure missing-test-matrix env TONDO_STDLIB_PROTOBUF_CONTRACT="$tmp_dir/missing-test-matrix.json" \
    scripts/stdlib-protobuf-check.sh

jq '.scope.syntax = "proto2"' testing/stdlib-protobuf.json \
    > "$tmp_dir/invalid-syntax.json"
expect_failure invalid-syntax env TONDO_STDLIB_PROTOBUF_CONTRACT="$tmp_dir/invalid-syntax.json" \
    scripts/stdlib-protobuf-check.sh

jq '.api.manifest_file = "tondo.json"' testing/stdlib-protobuf.json \
    > "$tmp_dir/invalid-manifest.json"
expect_failure invalid-manifest env TONDO_STDLIB_PROTOBUF_CONTRACT="$tmp_dir/invalid-manifest.json" \
    scripts/stdlib-protobuf-check.sh

jq '.limits = []' testing/stdlib-protobuf.json \
    > "$tmp_dir/missing-limits.json"
expect_failure missing-limits env TONDO_STDLIB_PROTOBUF_CONTRACT="$tmp_dir/missing-limits.json" \
    scripts/stdlib-protobuf-check.sh

for marker in \
    'schema-first' \
    'tondo.lock.toml' \
    'ProtoReader[T]' \
    'ProtoWriter[T]' \
    'UnknownFields' \
    'oneof' \
    'encodeDeterministic' \
    'STD-CODEC-CONF-001'; do
    grep -Fqi "$marker" docs/contracts/stdlib-protobuf.md
done

for symbol in \
    'pub enum ProtoWireType' \
    'pub struct ProtoDescriptor' \
    'pub struct UnknownField' \
    'pub struct UnknownFields' \
    'pub enum ProtoEvent' \
    'pub struct ProtoReader' \
    'pub struct ProtoWriter' \
    'pub enum ProtoValue' \
    'pub fn from_chunks' \
    'pub fn decode_message' \
    'pub fn encode_message' \
    'pub fn encode_static' \
    'pub fn decode_static' \
    'pub fn parse_schema' \
    'pub fn parse_schema_graph' \
    'pub fn generate_tondo' \
    'pub fn check_evolution'; do
    grep -Fq "$symbol" crates/tondo-stdlib/src/protobuf_api.rs
done

for test_name in \
    'reader_emits_explicit_events_for_all_wire_widths' \
    'writer_round_trips_events_and_becomes_terminal_after_finish' \
    'dynamic_message_preserves_nested_values_and_deterministic_order' \
    'unknown_groups_keep_exact_raw_bytes_and_limits_are_enforced' \
    'malformed_varints_and_wire_types_fail_without_partial_events' \
    'schema_parser_is_bounded_and_evolution_rejects_wire_changes' \
    'typed_scalars_use_the_common_static_event_protocol' \
    'canonical_static_path_round_trips_scalar_through_wire_adapter' \
    'unknown_fields_api_is_explicitly_owned' \
    'public_error_paths_limits_and_dynamic_values_are_exercised' \
    'wire_and_typed_edge_matrix_covers_bounded_branches' \
    'writer_protocol_covers_nested_packed_sink_and_terminal_errors' \
    'schema_error_matrix_and_generation_limits_are_explicit'; do
    grep -Fq "$test_name" crates/tondo-stdlib/src/protobuf_api.rs
done

for marker in \
    'prost' \
    'protobuf-unknown-fragment-and-limit' \
    'unknown-raw-bytes' \
    'one-byte-fragments'; do
    grep -Fq "$marker" testing/stdlib-codec-conformance.json \
        crates/tondo-stdlib/tests/codec_conformance.rs
done

grep -Fq 'stdlib_codecs' fuzz/Cargo.toml
grep -Fq 'protobuf::validate' fuzz/fuzz_targets/stdlib_codecs.rs
grep -Fq 'ProtoReader::<()>::from_chunks' fuzz/fuzz_targets/stdlib_codecs.rs
grep -Fq 'STD-A-PROTOBUF-EVIDENCE-001' docs/contracts/stdlib-s1a.md
grep -Fq 'std.protobuf' docs/contracts/stdlib-matrix.md
grep -Fq 'std.protobuf' TONDO_IMPLEMENTATION_TRACKER.md

jq -e '
  ([.rows[] | select(.owner == "std.protobuf")] | length) == 15
  and any(.rows[] | select(.owner == "std.protobuf"); .symbol == "std.protobuf.decode")
' testing/stdlib-public-api.json >/dev/null

jq -e '
  any(.owners[]; .id == "std.protobuf"
    and .runtime.kind == "host"
    and (.runtime.paths | length) > 0)
' testing/stdlib-public-api-config.json >/dev/null

jq -e '
  any(.leaves[]; .id == "STD-A-PROTOBUF-EVIDENCE-001" and .owners == ["std.protobuf"])
  and any(.owners[]; .id == "std.protobuf"
    and .cells.SPEC.status == "verified"
    and .cells.IMPL.status == "verified"
    and .cells.HOST.status == "not-applicable"
    and .cells.MODEL.status == "verified"
    and .cells.TEST.status == "verified"
    and .cells.FUZZ.status == "verified"
    and .cells.PERF.status == "verified"
    and .cells.PERF.reason == null
    and .cells.CONF.status == "partial"
    and .cells.DOC.status == "verified")
' testing/stdlib-owner-evidence.json >/dev/null

echo "std.protobuf owner tests: OK"
