#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_REGEX_PERF_CONTRACT:-$root/testing/stdlib-regex-performance.json}"
parent="${TONDO_STDLIB_REGEX_CONTRACT:-testing/stdlib-regex.json}"
die() { echo "std.regex performance contract: $*" >&2; exit 1; }

python3 -B scripts/stdlib_regex_performance.py check-contract --contract "$contract"
for path in docs/contracts/stdlib-regex-performance.md docs/contracts/stdlib-regex.md \
    docs/contracts/stdlib-regex-test.md testing/stdlib-regex.json testing/stdlib-regex-test.json \
    TONDO_STANDARD_LIBRARY_SPEC.md TONDO_IMPLEMENTATION_TRACKER.md \
    crates/tondo-reliability/src/regex_model.rs crates/tondo-reliability/tests/regex_models.rs; do
    [[ -f "$path" ]] || die "missing linked input: $path"
done
for path in scripts/stdlib-regex-performance-check.sh scripts/stdlib-regex-performance-test.sh \
    scripts/stdlib-regex-performance.sh; do
    [[ -x "$path" ]] || die "runner is not executable: $path"
done
status="$(jq -r '.status' "$contract")"
jq -e -L scripts --arg status "$status" '
    include "stdlib_regex_progression";
    regex_kernel_progression
    and .performance.status == $status
    and .implementation.public_api_promoted == false
    and .implementation.production_vm_registration == "not-claimed"
    and .implementation.native_abi == "not-claimed"
    and .implementation.native_aot_lowering == "not-claimed"
' "$parent" >/dev/null || die "parent performance state drift"
for marker in 'STD-REGEX-PERF-001' 'logical memory' 'not RSS' '27 measurements' \
    'not a hosted VM' 'native_live_handles' 'measurement-ready' 'cumulative step count'; do
    grep -Fq "$marker" docs/contracts/stdlib-regex-performance.md || die "missing documented boundary: $marker"
done
grep -Fq 'stdlib-regex-performance.json' TONDO_STANDARD_LIBRARY_SPEC.md || die "missing spec link"
grep -Fq 'stdlib-regex-performance.md' docs/contracts/stdlib-regex.md || die "missing parent document link"
echo "std.regex performance contract: OK ($status; 19 kernel routes; public/VM/native claims excluded)"
