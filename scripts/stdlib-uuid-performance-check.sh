#!/usr/bin/env bash
set -euo pipefail
root="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_UUID_PERF_CONTRACT:-testing/stdlib-uuid-performance.json}"
parent="${TONDO_STDLIB_UUID_CONTRACT:-testing/stdlib-uuid.json}"
die() { echo "std.uuid performance contract: $*" >&2; exit 1; }
python3 -B scripts/stdlib_uuid_performance.py check-contract --contract "$contract"
TONDO_STDLIB_UUID_CONTRACT="$parent" scripts/stdlib-uuid-check.sh >/dev/null
jq -e --slurpfile child "$contract" '
  .measurement.status == $child[0].status
  and .host.status == "verified-production-hosted" and .model.status == "verified"
' "$parent" >/dev/null || die "parent performance state drift"
for path in docs/contracts/stdlib-uuid-performance.md docs/contracts/stdlib-uuid.md \
    docs/contracts/stdlib-uuid-test.md testing/stdlib-uuid-host.json testing/stdlib-uuid-test.json \
    TONDO_STANDARD_LIBRARY_SPEC.md TONDO_IMPLEMENTATION_TRACKER.md; do
    [[ -f "$path" ]] || die "missing linked input: $path"
done
for path in scripts/stdlib-uuid-performance-check.sh scripts/stdlib-uuid-performance-test.sh \
    scripts/stdlib-uuid-performance.sh; do
    [[ -x "$path" ]] || die "runner is not executable: $path"
done
for marker in 'STD-UUID-PERF-001' 'logical memory' 'not RSS' '27 measurements' \
    'no bytecode' 'native_live_handles' 'measurement-ready' 'not OS syscalls'; do
    grep -Fq "$marker" docs/contracts/stdlib-uuid-performance.md || die "missing documented boundary: $marker"
done
grep -Fq 'stdlib-uuid-performance.json' TONDO_STANDARD_LIBRARY_SPEC.md || die "missing spec link"
grep -Fq 'stdlib-uuid-performance.md' docs/contracts/stdlib-uuid.md || die "missing parent link"
echo "std.uuid performance contract: OK ($(jq -r .status "$contract"); 22 hosted scalar routes)"
