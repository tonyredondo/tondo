#!/usr/bin/env bash
set -euo pipefail
root="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_NET_CONFORMANCE_CONTRACT:-testing/stdlib-net-conformance.json}"
parent="${TONDO_STDLIB_NET_CONTRACT:-testing/stdlib-net.json}"
die() { echo "std.net conformance: $*" >&2; exit 1; }
python3 -B scripts/stdlib_net_conformance.py check-contract --contract "$contract"
env TONDO_STDLIB_NET_CONTRACT="$parent" scripts/stdlib-net-performance-check.sh >/dev/null
jq -e --slurpfile child "$contract" '
  .conformance.status == $child[0].status
  and .model.status == "verified"
  and .measurement.status == "verified-hosted-scalar-baseline"
' "$parent" >/dev/null || die 'parent conformance prerequisites or status differ'
for path in scripts/stdlib-net-conformance.sh scripts/stdlib-net-conformance-check.sh \
    scripts/stdlib-net-conformance-test.sh; do
    test -x "$path" || die "missing executable: $path"
done
for marker in 'native Tondo network target' 'kernel-only' 'VM-only' '80%' 'STD-NET-DOC-001'; do
    grep -Fq "$marker" docs/contracts/stdlib-net-conformance.md || die "missing boundary: $marker"
done
grep -Fq 'stdlib-net-conformance.json' TONDO_STANDARD_LIBRARY_SPEC.md || die 'missing spec link'
grep -Fq 'stdlib-net-conformance.md' docs/contracts/stdlib-net.md || die 'missing parent link'
echo 'std.net conformance contract: OK (public hosted VM and qualified native Rust reference)'
