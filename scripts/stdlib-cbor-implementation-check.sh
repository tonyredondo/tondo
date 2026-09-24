#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_CBOR_CONTRACT:-$root/testing/stdlib-cbor.json}"

die() {
    echo "std.cbor implementation: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing owner contract"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "owner contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "owner contract contains CR or trailing whitespace"

jq -e '
  .owner == "std.cbor"
  and .task == "STD-CBOR-001"
  and .implementation.status == "verified-stdlib-kernel"
  and .implementation.public_api_promoted == false
  and .implementation.host == "not-claimed-until-compiler-cbor-abi"
  and .implementation.native_aot_lowering == "not-claimed"
  and .implementation.sources == ["crates/tondo-stdlib/src/cbor.rs", "crates/tondo-stdlib/src/serialization.rs", "crates/tondo-stdlib/src/lib.rs"]
  and (.implementation.tests | length) == 14
  and ([.implementation.tests[]] | unique | length) == 14
  and .implementation.fixture == null
  and .implementation.evidence_report == "target/reliability/evidence/stdlib-cbor-implementation.json"
  and (.implementation.proof | type == "string" and length > 0)
  and .implementation.required_follow_ups == ["STD-CBOR-TEST-001", "STD-CBOR-PERF-001", "STD-CBOR-CONF-001", "STD-CBOR-DOC-001"]
  and .promotion.next_blocks == ["STD-CBOR-TEST-001"]
' "$contract" >/dev/null || die "invalid machine-readable implementation state"

while IFS= read -r path; do
    [[ -f "$root/$path" ]] || die "missing implementation source: $path"
done < <(jq -r '.implementation.sources[]' "$contract")

while IFS= read -r anchor; do
    path="${anchor%%::*}"
    name="${anchor##*::}"
    [[ -f "$root/$path" ]] || die "missing test source: $path"
    grep -Fq "fn $name(" "$root/$path" || die "missing test anchor: $anchor"
done < <(jq -r '.implementation.tests[]' "$contract")

for marker in \
    'pub enum CborValue' \
    'pub enum CborEvent' \
    'pub struct CborReader' \
    'pub struct CborWriter' \
    'pub fn encode_typed' \
    'pub fn decode_typed' \
    'fn encode_deterministic_inner' \
    'fn scan_events_inner'; do
    grep -Fq "$marker" crates/tondo-stdlib/src/cbor.rs || die "missing kernel anchor: $marker"
done
grep -Fq 'pub struct Cbor;' crates/tondo-stdlib/src/serialization.rs || die "missing static codec identity"
grep -Fq 'pub mod cbor;' crates/tondo-stdlib/src/lib.rs || die "missing stdlib module registration"
grep -Fq 'verified-stdlib-kernel' docs/contracts/stdlib-cbor.md || die "missing documented kernel boundary"
grep -Fq 'not-claimed-until-compiler-cbor-abi' docs/contracts/stdlib-cbor.md || die "missing documented host boundary"
grep -Fq '[x] **STD-CBOR-IMPL-001' TONDO_IMPLEMENTATION_TRACKER.md || die "tracker does not record implementation"
grep -Fq 'STD-CBOR-TEST-001' TONDO_STANDARD_LIBRARY_SPEC.md || die "spec does not record follow-up"

echo "std.cbor implementation: OK (bounded Rust kernel; public/host/native boundaries unclaimed)"
