#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_root="${TMPDIR:-/tmp}"
tmp_dir="$(mktemp -d "$tmp_root/tondo-stdlib-json-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.json owner tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.owner = "std.invalid"' testing/stdlib-json.json \
    > "$tmp_dir/invalid-owner.json"
expect_failure invalid-owner env TONDO_STDLIB_JSON_CONTRACT="$tmp_dir/invalid-owner.json" \
    scripts/stdlib-json-check.sh

jq '.test_matrix = []' testing/stdlib-json.json \
    > "$tmp_dir/missing-test-matrix.json"
expect_failure missing-test-matrix env TONDO_STDLIB_JSON_CONTRACT="$tmp_dir/missing-test-matrix.json" \
    scripts/stdlib-json-check.sh

jq '.wire.syntax = "not-rfc8259"' testing/stdlib-json.json \
    > "$tmp_dir/invalid-wire.json"
expect_failure invalid-wire env TONDO_STDLIB_JSON_CONTRACT="$tmp_dir/invalid-wire.json" \
    scripts/stdlib-json-check.sh

jq '.policies.duplicate_keys.default = "last"' testing/stdlib-json.json \
    > "$tmp_dir/invalid-policy.json"
expect_failure invalid-policy env TONDO_STDLIB_JSON_CONTRACT="$tmp_dir/invalid-policy.json" \
    scripts/stdlib-json-check.sh

jq '.limits = []' testing/stdlib-json.json \
    > "$tmp_dir/missing-limits.json"
expect_failure missing-limits env TONDO_STDLIB_JSON_CONTRACT="$tmp_dir/missing-limits.json" \
    scripts/stdlib-json-check.sh

for marker in \
    'RFC 8259' \
    'RFC 8785' \
    'JsonReader' \
    'JsonWriter' \
    'parse_view' \
    'encode_typed' \
    'decode_typed' \
    'no hay un segundo parser' \
    'STD-CODEC-CONF-001'; do
    grep -Fqi "$marker" docs/contracts/stdlib-json.md
done

for symbol in \
    'pub type Value = JsonValue' \
    'pub type Raw = RawCodec<JsonCodec>' \
    'pub struct JsonNumber' \
    'pub struct JsonReader' \
    'pub struct JsonWriter' \
    'enum Frame' \
    'pub fn parse_with_options' \
    'pub fn parse_view' \
    'pub fn raw(' \
    'pub fn raw_unchecked' \
    'pub fn encode_typed' \
    'pub fn decode_typed'; do
    grep -Fq "$symbol" crates/tondo-stdlib/src/json_api.rs
done

for test_name in \
    'reader_and_dynamic_parser_cover_events_unicode_and_order' \
    'duplicate_policies_keep_first_position_and_reject_by_default' \
    'numbers_are_lexical_exact_and_canonical_without_float_for_integers' \
    'invalid_unicode_syntax_trailing_and_limits_are_terminal' \
    'canonical_order_writer_and_output_limits_are_checked' \
    'typed_paths_round_trip_scalars_options_and_arrays_without_dom' \
    'canonical_static_path_round_trips_without_compatibility_events' \
    'reader_from_reader_and_error_locations_are_stable' \
    'reader_covers_utf8_syntax_limits_and_terminal_reuse' \
    'reader_paths_and_all_unicode_escape_forms_are_stable' \
    'dynamic_encoding_covers_values_order_numbers_strings_and_limits' \
    'streaming_writer_rejects_invalid_sequences_and_is_terminal' \
    'typed_event_adapters_and_serialization_errors_are_exhaustive' \
    'dynamic_collector_attach_and_reader_from_chunks_cover_error_paths'; do
    grep -Fq "$test_name" crates/tondo-stdlib/src/json_api.rs
done

for marker in \
    'serde_json' \
    'json-fragment-and-limit' \
    'one-byte-fragments' \
    'terminal-invalid-input'; do
    grep -Fq "$marker" testing/stdlib-codec-conformance.json \
        crates/tondo-stdlib/tests/codec_conformance.rs
done

grep -Fq 'stdlib_codecs' fuzz/Cargo.toml
grep -Fq 'json::validate' fuzz/fuzz_targets/stdlib_codecs.rs
grep -Fq 'JsonReader::from_chunks' fuzz/fuzz_targets/stdlib_codecs.rs
grep -Fq 'STD-A-JSON-EVIDENCE-001' docs/contracts/stdlib-s1a.md
grep -Fq 'std.json' docs/contracts/stdlib-matrix.md
grep -Fq 'std.json' TONDO_IMPLEMENTATION_TRACKER.md

jq -e '
  ([.rows[] | select(.owner == "std.json")] | length) == 22
  and any(.rows[] | select(.owner == "std.json"); .symbol == "std.json.parse")
' testing/stdlib-public-api.json >/dev/null

jq -e '
  any(.owners[]; .id == "std.json"
    and .runtime.kind == "host"
    and (.runtime.paths | length) > 0)
' testing/stdlib-public-api-config.json >/dev/null

jq -e '
  any(.leaves[]; .id == "STD-A-JSON-EVIDENCE-001" and .owners == ["std.json"])
  and any(.owners[]; .id == "std.json"
    and .cells.SPEC.status == "verified"
    and .cells.IMPL.status == "verified"
    and .cells.HOST.status == "not-applicable"
    and .cells.MODEL.status == "verified"
    and .cells.TEST.status == "verified"
    and .cells.FUZZ.status == "partial"
    and .cells.PERF.status == "partial"
    and .cells.CONF.status == "partial"
    and .cells.DOC.status == "verified")
' testing/stdlib-owner-evidence.json >/dev/null

echo "std.json owner tests: OK"
