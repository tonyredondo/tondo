#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_CBOR_CONFORMANCE_CONTRACT:-testing/stdlib-cbor-conformance.json}"
parent="${TONDO_STDLIB_CBOR_CONTRACT:-testing/stdlib-cbor.json}"
python3 -B scripts/stdlib_cbor_conformance.py check-contract --contract "$contract"
status="$(jq -r '.status' "$contract")"
jq -e -L scripts --arg status "$status" '
  include "stdlib_cbor_progression";
  cbor_owner_progression and .conformance.status == $status
  and .implementation.public_api_promoted == false
  and .implementation.host == "not-claimed-until-compiler-cbor-abi"
  and .implementation.native_aot_lowering == "not-claimed"
' "$parent" >/dev/null || { echo "std.cbor conformance contract: parent progression differs" >&2; exit 1; }
for marker in 'STD-CBOR-CONF-001' 'test-only' '61 valid' '30 invalid' 'not a source-level' 'table objects' 'STD-CBOR-DOC-001'; do
    grep -Fq "$marker" docs/contracts/stdlib-cbor-conformance.md || { echo "std.cbor conformance contract: missing boundary $marker" >&2; exit 1; }
done
for runner in scripts/stdlib-cbor-conformance-check.sh scripts/stdlib-cbor-conformance-test.sh scripts/stdlib-cbor-conformance.sh; do
    [[ -x "$runner" ]] || { echo "std.cbor conformance contract: non-executable $runner" >&2; exit 1; }
done
grep -Fq 'stdlib-cbor-conformance.json' TONDO_STANDARD_LIBRARY_SPEC.md
grep -Fq 'stdlib-cbor-conformance.md' docs/contracts/stdlib-cbor.md
echo "std.cbor conformance contract: OK ($status; exact private adapter boundary)"
