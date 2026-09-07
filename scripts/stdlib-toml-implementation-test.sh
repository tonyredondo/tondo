#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-toml-implementation-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.toml implementation tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.implementation.status = "pending-after-native-gate"' testing/stdlib-toml.json > "$tmp_dir/pending.json"
expect_failure pending env TONDO_STDLIB_TOML_CONTRACT="$tmp_dir/pending.json" scripts/stdlib-toml-implementation-check.sh

jq '.implementation.host = "required-after-native-gate"' testing/stdlib-toml.json > "$tmp_dir/host.json"
expect_failure host env TONDO_STDLIB_TOML_CONTRACT="$tmp_dir/host.json" scripts/stdlib-toml-implementation-check.sh

jq '.implementation.native_aot_lowering = "verified"' testing/stdlib-toml.json > "$tmp_dir/native.json"
expect_failure native env TONDO_STDLIB_TOML_CONTRACT="$tmp_dir/native.json" scripts/stdlib-toml-implementation-check.sh

jq '.implementation.public_api_promoted = true' testing/stdlib-toml.json > "$tmp_dir/public.json"
expect_failure public env TONDO_STDLIB_TOML_CONTRACT="$tmp_dir/public.json" scripts/stdlib-toml-implementation-check.sh

jq '.implementation.sources = .implementation.sources[0:2]' testing/stdlib-toml.json > "$tmp_dir/source.json"
expect_failure source env TONDO_STDLIB_TOML_CONTRACT="$tmp_dir/source.json" scripts/stdlib-toml-implementation-check.sh

jq '.implementation.tests = .implementation.tests[0:7]' testing/stdlib-toml.json > "$tmp_dir/test.json"
expect_failure test env TONDO_STDLIB_TOML_CONTRACT="$tmp_dir/test.json" scripts/stdlib-toml-implementation-check.sh

jq -e '
  .implementation.status == "verified-stdlib-kernel"
  and .implementation.public_api_promoted == false
  and .implementation.host == "not-claimed-until-compiler-toml-abi"
  and .implementation.native_aot_lowering == "not-claimed"
  and .implementation.fixture == null
  and .implementation.required_follow_ups == ["STD-TOML-TEST-001", "STD-TOML-PERF-001", "STD-TOML-CONF-001", "STD-TOML-DOC-001"]
  and .promotion.next_blocks == ["STD-TOML-TEST-001"]
' testing/stdlib-toml.json >/dev/null

for marker in \
    'parses_toml_11_scalars_strings_and_temporal_values' \
    'tables_arrays_of_tables_and_inline_tables_are_ordered' \
    'duplicate_and_collision_errors_have_paths_and_spans' \
    'invalid_numbers_dates_escapes_and_limits_are_rejected_atomically' \
    'canonical_encoding_round_trips_and_sorts_keys' \
    'reader_chunking_events_and_terminal_lifecycle_are_stable' \
    'writer_and_common_typed_protocol_reject_partial_streams' \
    'reader_io_and_view_keep_contract_boundaries'; do
    grep -Fq "$marker" crates/tondo-stdlib/src/toml.rs \
        || { echo "std.toml implementation tests: missing test marker $marker" >&2; exit 1; }
done

echo "std.toml implementation tests: OK (kernel state, boundary and evidence negatives)"
