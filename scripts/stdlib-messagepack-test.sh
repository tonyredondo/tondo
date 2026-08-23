#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_root="${TMPDIR:-/tmp}"
tmp_dir="$(mktemp -d "$tmp_root/tondo-stdlib-messagepack-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.messagepack owner tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

die() {
    echo "std.messagepack owner tests: $*" >&2
    exit 1
}

jq '.owner = "std.invalid"' testing/stdlib-messagepack.json \
    > "$tmp_dir/invalid-owner.json"
expect_failure invalid-owner env TONDO_STDLIB_MESSAGEPACK_CONTRACT="$tmp_dir/invalid-owner.json" \
    scripts/stdlib-messagepack-check.sh

jq '.test_matrix = []' testing/stdlib-messagepack.json \
    > "$tmp_dir/missing-test-matrix.json"
expect_failure missing-test-matrix env TONDO_STDLIB_MESSAGEPACK_CONTRACT="$tmp_dir/missing-test-matrix.json" \
    scripts/stdlib-messagepack-check.sh

jq '.wire.spec = "not-messagepack"' testing/stdlib-messagepack.json \
    > "$tmp_dir/invalid-wire.json"
expect_failure invalid-wire env TONDO_STDLIB_MESSAGEPACK_CONTRACT="$tmp_dir/invalid-wire.json" \
    scripts/stdlib-messagepack-check.sh

jq '.policies.deterministic_map.rule = "layout-order"' testing/stdlib-messagepack.json \
    > "$tmp_dir/invalid-deterministic-policy.json"
expect_failure invalid-deterministic-policy env TONDO_STDLIB_MESSAGEPACK_CONTRACT="$tmp_dir/invalid-deterministic-policy.json" \
    scripts/stdlib-messagepack-check.sh

jq '.limits = []' testing/stdlib-messagepack.json \
    > "$tmp_dir/missing-limits.json"
expect_failure missing-limits env TONDO_STDLIB_MESSAGEPACK_CONTRACT="$tmp_dir/missing-limits.json" \
    scripts/stdlib-messagepack-check.sh

for marker in \
    'MessagePack' \
    'Value.Map' \
    'MessagePackReader' \
    'MessagePackWriter' \
    'un byte' \
    'encodeDeterministic' \
    'STD-CODEC-CONF-001'; do
    grep -Fqi "$marker" docs/contracts/stdlib-messagepack.md
done

for symbol in \
    'pub type Value = MessagePackValue' \
    'pub type ValueView' \
    'pub type Raw = RawCodec<MessagePackCodec>' \
    'pub enum MessagePackValue' \
    'pub struct MessagePackReader' \
    'pub struct MessagePackWriter' \
    'enum Frame' \
    'pub fn parse_view' \
    'pub fn raw(' \
    'pub fn raw_unchecked' \
    'pub fn encode_typed' \
    'pub fn decode_typed' \
    'pub fn encode_static' \
    'pub fn decode_static' \
    'pub fn from_chunks'; do
    grep -Fq "$symbol" crates/tondo-stdlib/src/messagepack_api.rs
done

for test_name in \
    'dynamic_values_round_trip_all_wire_families' \
    'policies_nonminimal_duplicates_and_unknown_ext_are_explicit' \
    'deterministic_maps_nan_and_collisions_are_stable' \
    'reader_emits_events_for_fragments_and_finishes_once' \
    'reader_and_writer_fail_terminally_on_bad_sequences' \
    'timestamps_use_the_three_standard_payload_shapes' \
    'typed_events_use_the_common_static_traits' \
    'canonical_static_path_round_trips_scalars_and_arrays' \
    'public_aliases_and_terminal_error_paths_are_exercised' \
    'parse_view_and_raw_preserve_wire_bytes_until_materialization' \
    'limits_and_trailing_data_are_bounded' \
    'wire_widths_and_encoder_prefixes_cover_every_length_family' \
    'nonminimal_width_matrix_and_resource_boundaries_are_closed' \
    'writer_events_cover_nested_keys_ownership_and_limits' \
    'option_validation_and_timestamp_error_shapes_are_observable' \
    'typed_and_host_boundaries_reject_unsupported_shapes_without_partial_values'; do
    grep -Fq "$test_name" crates/tondo-stdlib/src/messagepack_api.rs
done

for marker in \
    'rmpv' \
    'messagepack-fragment-and-limit' \
    'one-byte-fragments' \
    'unknown_preservation'; do
    grep -Fq "$marker" testing/stdlib-codec-conformance.json \
        crates/tondo-stdlib/tests/codec_conformance.rs
done

grep -Fq 'stdlib_codecs' fuzz/Cargo.toml
grep -Fq 'messagepack::validate' fuzz/fuzz_targets/stdlib_codecs.rs
grep -Fq 'MessagePackReader::from_chunks' fuzz/fuzz_targets/stdlib_codecs.rs
grep -Fq 'STD-A-MSGPACK-EVIDENCE-001' docs/contracts/stdlib-s1a.md
grep -Fq 'std.messagepack' docs/contracts/stdlib-matrix.md
grep -Fq 'std.messagepack' TONDO_IMPLEMENTATION_TRACKER.md

jq -e '
  ([.rows[] | select(.owner == "std.messagepack")] | length) == 19
  and any(.rows[] | select(.owner == "std.messagepack"); .symbol == "std.messagepack.parse")
  and any(.rows[] | select(.owner == "std.messagepack");
    .signature == "pub unsafe fn rawUnchecked(input: Bytes): Raw"
    and .status == "verified"
    and (.missing | length) == 0)
' testing/stdlib-public-api.json >/dev/null || die "public API audit lost the verified unsafe rawUnchecked boundary"

jq -e '
  any(.owners[]; .id == "std.messagepack"
    and .runtime.kind == "host"
    and (.runtime.paths | length) > 0)
' testing/stdlib-public-api-config.json >/dev/null

jq -e '
  any(.leaves[]; .id == "STD-A-MSGPACK-EVIDENCE-001" and .owners == ["std.messagepack"])
  and any(.owners[]; .id == "std.messagepack"
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

echo "std.messagepack owner tests: OK"
