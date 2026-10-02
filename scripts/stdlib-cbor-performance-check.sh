#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_CBOR_PERF_CONTRACT:-$root/testing/stdlib-cbor-performance.json}"
parent="${TONDO_STDLIB_CBOR_CONTRACT:-testing/stdlib-cbor.json}"
die() { echo "std.cbor performance contract: $*" >&2; exit 1; }

python3 -B scripts/stdlib_cbor_performance.py check-contract --contract "$contract"
for path in docs/contracts/stdlib-cbor-performance.md docs/contracts/stdlib-cbor.md \
    docs/contracts/stdlib-cbor-test.md testing/stdlib-cbor.json testing/stdlib-cbor-test.json \
    TONDO_STANDARD_LIBRARY_SPEC.md TONDO_IMPLEMENTATION_TRACKER.md \
    crates/tondo-reliability/src/cbor_model.rs crates/tondo-reliability/tests/cbor_models.rs; do
    [[ -f "$path" ]] || die "missing linked input: $path"
done
for path in scripts/stdlib-cbor-performance-check.sh scripts/stdlib-cbor-performance-test.sh \
    scripts/stdlib-cbor-performance.sh; do
    [[ -x "$path" ]] || die "runner is not executable: $path"
done
status="$(jq -r '.status' "$contract")"
jq -e -L scripts --arg status "$status" '
    include "stdlib_cbor_progression";
    cbor_owner_progression
    and .performance.status == $status
    and .implementation.public_api_promoted == false
    and .implementation.host == "not-claimed-until-compiler-cbor-abi"
    and .implementation.native_aot_lowering == "not-claimed"
' "$parent" >/dev/null || die "parent performance state drift"
for marker in 'STD-CBOR-PERF-001' 'logical allocations' 'modeled logical memory' \
    'tail-latency' 'not a hosted VM' 'native_live_handles' 'measurement-ready'; do
    grep -Fq "$marker" docs/contracts/stdlib-cbor-performance.md || die "missing documented boundary: $marker"
done
grep -Fq 'stdlib-cbor-performance.json' TONDO_STANDARD_LIBRARY_SPEC.md || die "missing spec link"
grep -Fq 'stdlib-cbor-performance.md' docs/contracts/stdlib-cbor.md || die "missing parent document link"
echo "std.cbor performance contract: OK ($status; 15 kernel routes; public/VM/native claims excluded)"
