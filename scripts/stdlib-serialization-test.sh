#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_root="${TMPDIR:-/tmp}"
tmp_dir="$(mktemp -d "$tmp_root/tondo-stdlib-serialization-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.serialization owner tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.owners |= map(select(. != "std.serialization"))' testing/stdlib-core.json \
    > "$tmp_dir/missing-owner.json"
expect_failure missing-owner \
    env TONDO_STDLIB_CORE_CONTRACT="$tmp_dir/missing-owner.json" \
    scripts/stdlib-core-check.sh

jq '.test_matrix = []' testing/stdlib-core.json > "$tmp_dir/missing-test-matrix.json"
expect_failure missing-test-matrix \
    env TONDO_STDLIB_CORE_CONTRACT="$tmp_dir/missing-test-matrix.json" \
    scripts/stdlib-core-check.sh

jq '.owners |= map(if . == "std.serialization" then "std.invalid" else . end)' \
    testing/stdlib-core.json > "$tmp_dir/invalid-owner.json"
expect_failure invalid-owner \
    env TONDO_STDLIB_CORE_CONTRACT="$tmp_dir/invalid-owner.json" \
    scripts/stdlib-core-check.sh

for marker in \
    'pub trait Encoder[C, E]' \
    'pub trait Decoder[C, E]' \
    'pub trait Encode[C]' \
    'pub trait Decode[C]' \
    'StartRecord' \
    'StartEnum' \
    'fn own(var self, event: SerializationEvent)' \
    'fn reject(var self, error: SerializationError): E' \
    'construcción atómica' \
    'frames explícitos'; do
    grep -Fq "$marker" docs/contracts/stdlib-serialization.md
done

for symbol in \
    'pub trait Encoder' \
    'pub trait Decoder' \
    'pub trait Encode' \
    'pub trait Decode' \
    'fn reject(&mut self, error: SerializationError) -> E' \
    'pub fn validate_events' \
    'struct Frame' \
    'pub struct EventSerializer' \
    'pub struct EventDeserializer' \
    'publish'; do
    grep -Fq "$symbol" crates/tondo-stdlib/src/serialization.rs
done

for symbol in \
    'ENCODE_TRAIT' \
    'DECODE_TRAIT' \
    'ENCODE_PROVIDER' \
    'DECODE_PROVIDER' \
    'execute_derive_plan' \
    'generated_source_mappings'; do
    grep -Fq "$symbol" crates/tondo-compiler/src/serialization_derive.rs \
        crates/tondo-compiler/src/meta_derive.rs
done

for test_name in \
    'accepts_nested_arrays_and_maps' \
    'enforces_depth_events_and_bytes' \
    'typed_protocol_publishes_only_after_complete_validation' \
    'canonical_protocol_rejects_declared_array_length_mismatch_atomically' \
    'dynamic_value_views_and_raw_bytes_are_owned_or_borrowed_explicitly' \
    'canonical_protocol_keeps_bytes_unit_and_maps_explicit' \
    'record_provider_is_deterministic_and_maps_to_target' \
    'deserialize_provider_and_enum_shapes_are_generated' \
    'specialized_codecs_and_field_annotations_are_deterministic' \
    'provider_rejects_missing_targets_bounds_and_member_names'; do
    grep -Fq "$test_name" crates/tondo-stdlib/src/serialization.rs \
        crates/tondo-compiler/src/serialization_derive.rs
done

jq -e '
  any(.owners[]; .id == "std.serialization"
    and .runtime.kind == "not-applicable"
    and (.runtime.paths | length) == 0)
' testing/stdlib-public-api-config.json >/dev/null
jq -e '
  ([.rows[] | select(.owner == "std.serialization")] | length) == 0
' testing/stdlib-public-api.json >/dev/null

grep -Fq 'stdlib_codecs' fuzz/Cargo.toml
grep -Fq 'one-byte-fragments' testing/stdlib-codec-conformance.json
grep -Fq 'STD-A-SER-EVIDENCE-001' docs/contracts/stdlib-s1a.md
grep -Fq 'std.serialization' docs/contracts/stdlib-matrix.md
grep -Fq 'event protocol' testing/stdlib-performance-conformance.json

jq -e '
  .owners == ["std.core","std.text","std.collections","std.iter","std.math","std.format","std.io","std.serialization"]
  and (.test_matrix | length) == 7
' testing/stdlib-core.json >/dev/null

echo "std.serialization owner tests: OK"
