#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_TOML_CONTRACT:-$root/testing/stdlib-toml.json}"

die() {
    echo "std.toml implementation: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing owner contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "owner contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "owner contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-owner-contract/1"
  and .owner == "std.toml"
  and .task == "STD-TOML-001"
  and .implementation.status == "verified-stdlib-kernel"
  and .implementation.public_api_promoted == false
  and .implementation.host == "not-claimed-until-compiler-toml-abi"
  and .implementation.native_aot_lowering == "not-claimed"
  and (.implementation.sources | type == "array" and length == 3)
  and (.implementation.tests | type == "array" and length == 8)
  and .implementation.fixture == null
  and .implementation.evidence_report == "target/reliability/evidence/stdlib-toml-implementation.json"
  and (.implementation.proof | type == "string" and length > 0)
  and .implementation.required_follow_ups == ["STD-TOML-PERF-001", "STD-TOML-CONF-001", "STD-TOML-DOC-001"]
  and .promotion.next_blocks == ["STD-TOML-PERF-001"]
' "$contract" >/dev/null || die "invalid machine-readable std.toml implementation state"

for path in \
    docs/contracts/stdlib-toml.md \
    TONDO_STANDARD_LIBRARY_SPEC.md \
    TONDO_IMPLEMENTATION_TRACKER.md \
    crates/tondo-stdlib/src/toml.rs \
    crates/tondo-stdlib/src/serialization.rs \
    crates/tondo-stdlib/src/lib.rs; do
    [[ -f "$root/$path" ]] || die "missing implementation input: $path"
done

while IFS= read -r path; do
    [[ -f "$root/$path" ]] || die "missing implementation source: $path"
done < <(jq -r ".implementation.sources[]" "$contract")

while IFS= read -r test; do
    file="${test%%::*}"
    name="${test##*::}"
    [[ -f "$root/$file" ]] || die "missing test source: $file"
    grep -Fq "$name" "$root/$file" || die "missing test anchor: $test"
done < <(jq -r ".implementation.tests[]" "$contract")

for marker in \
    'pub enum TomlValue' \
    'pub enum TomlEvent' \
    'pub struct TomlLimits' \
    'fn parse_temporal' \
    'fn encode_inner' \
    'pub struct TomlReader' \
    'pub struct TomlWriter' \
    'parses_toml_11_scalars_strings_and_temporal_values' \
    'duplicate_and_collision_errors_have_paths_and_spans' \
    'reader_chunking_events_and_terminal_lifecycle_are_stable'; do
    grep -Fq "$marker" "$root/crates/tondo-stdlib/src/toml.rs" \
        || die "scalar implementation anchor is missing: $marker"
done

grep -Fq 'pub struct Toml;' "$root/crates/tondo-stdlib/src/serialization.rs" \
    || die "serialization codec identity is missing"
grep -Fq 'pub mod toml;' "$root/crates/tondo-stdlib/src/lib.rs" \
    || die "stdlib module registration is missing"

for marker in \
    'verified-stdlib-kernel' \
    'host: not-claimed-until-compiler-toml-abi' \
    'native_aot_lowering: not-claimed' \
    'STD-TOML-TEST-001'; do
    grep -Fq "$marker" "$root/docs/contracts/stdlib-toml.md" \
        || die "implementation boundary misses marker: $marker"
done
grep -Fq '[x] **STD-TOML-IMPL-001' "$root/TONDO_IMPLEMENTATION_TRACKER.md" \
    || die "tracker does not record the implementation leaf"

echo "std.toml implementation: OK (stdlib kernel; compiler/host/AOT explicitly unclaimed)"
