#!/usr/bin/env bash
set -euo pipefail
root="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_UUID_CONFORMANCE_CONTRACT:-testing/stdlib-uuid-conformance.json}"
parent="${TONDO_STDLIB_UUID_CONTRACT:-testing/stdlib-uuid.json}"
die() { echo "std.uuid conformance: $*" >&2; exit 1; }
python3 -B scripts/stdlib_uuid_conformance.py check-contract --contract "$contract"
TONDO_STDLIB_UUID_CONTRACT="$parent" scripts/stdlib-uuid-check.sh >/dev/null
jq -e --slurpfile child "$contract" '
  .conformance.status == $child[0].status
  and .host.status == "verified-production-hosted"
  and .model.status == "verified"
  and .measurement.status == "verified-hosted-scalar-baseline"
' "$parent" >/dev/null || die "parent conformance prerequisites or status differ"
for path in docs/contracts/stdlib-uuid-conformance.md docs/contracts/stdlib-uuid.md \
    TONDO_STANDARD_LIBRARY_SPEC.md TONDO_IMPLEMENTATION_TRACKER.md; do
    [[ -f "$path" ]] || die "missing linked input: $path"
done
for path in scripts/stdlib-uuid-conformance-check.sh scripts/stdlib-uuid-conformance-test.sh \
    scripts/stdlib-uuid-conformance.sh; do
    [[ -x "$path" ]] || die "runner is not executable: $path"
done
for marker in 'STD-UUID-CONF-001' 'adapter-ready' 'seventy-seven' 'E1008' \
    'v4-length-17' 'v5-name-limit' 'native UUID runtime ABI' 'RSS' 'first value'; do
    grep -Fq "$marker" docs/contracts/stdlib-uuid-conformance.md || die "missing documented boundary: $marker"
done
grep -Fq 'stdlib-uuid-conformance.json' TONDO_STANDARD_LIBRARY_SPEC.md || die "missing spec link"
grep -Fq 'stdlib-uuid-conformance.md' docs/contracts/stdlib-uuid.md || die "missing parent contract link"
echo "std.uuid conformance contract: OK ($(jq -r .status "$contract"); public VM and native Rust reference)"
