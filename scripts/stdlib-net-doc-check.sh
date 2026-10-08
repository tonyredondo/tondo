#!/usr/bin/env bash
set -euo pipefail
root="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_NET_CONTRACT:-testing/stdlib-net.json}"
document="${TONDO_STDLIB_NET_DOCUMENT:-docs/contracts/stdlib-net.md}"
source="${TONDO_STDLIB_NET_DOC_SOURCE:-acceptance/projects/net-usage/src/main.to}"
project="${TONDO_STDLIB_NET_DOC_PROJECT:-acceptance/projects/net-usage/tondo.toml}"
die() { echo "std.net documentation: $*" >&2; exit 1; }
for path in "$contract" "$document" "$source" "$project"; do
    test -f "$path" || die "missing input: $path"
done
env TONDO_STDLIB_NET_CONTRACT="$contract" scripts/stdlib-net-conformance-check.sh >/dev/null
jq -e -L scripts '
  include "stdlib_net_testing";
  net_owner_progression and net_documentation_progression and .documentation != null
' "$contract" >/dev/null || die 'documentation progression or prerequisite differs'
for marker in \
    '## Executable usage guide for `std.net`' \
    '### Capabilities and explicit configuration' '### Values and keys' \
    '### TCP ownership and partial I/O' '### UDP message boundaries' \
    '### Errors, deadlines and cancellation' '### Selection keeps losing data' \
    '### DNS and TLS' '### Limits and costs' \
    '### Executable verification' '### Promotion boundary' \
    'no system fallback' 'configuration only' 'materializes a copy' \
    'not a mutable' '64 KiB reads' '128 DNS' 'E1008' \
    'Native networking ABI/AOT' 'STD-NET-DOC-001' 'STD-LOG-IMPL-001'; do
    grep -Fq "$marker" "$document" || die "guide misses: $marker"
done
python3 -B scripts/stdlib_net_doc.py --document "$document" --source "$source" --project "$project"
# A real delegated scope is required before any socket/process execution.
bash scripts/test-process-scope.sh true
target_dir="${CARGO_TARGET_DIR:-target}"
[[ "$target_dir" = /* ]] || target_dir="$root/$target_dir"
export CARGO_TARGET_DIR="$target_dir"
cargo build -q -p tondo-cli --locked
mkdir -p .tmp
tmp="$(mktemp -d "$root/.tmp/net-doc-project.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
mkdir "$tmp/src"
cp -- "$source" "$tmp/src/main.to"
cp -- "$project" "$tmp/tondo.toml"
timeout 90 "$target_dir/debug/tondo" check --project "$tmp" > "$tmp/check.stdout" 2> "$tmp/check.stderr" \
    || die 'public Tondo project check failed'
test ! -s "$tmp/check.stderr" || die 'unexpected project diagnostics'
timeout 90 "$target_dir/debug/tondo" run --project "$tmp" > "$tmp/run.stdout" 2> "$tmp/run.stderr" \
    || die 'public Tondo example failed'
cmp -s "$tmp/run.stdout" <(printf 'net-doc-ok\n') || die 'actual example output differs'
test ! -s "$tmp/run.stderr" || die 'unexpected runtime diagnostics'
echo 'std.net documentation: OK (six public hosted paths; exact fragments and explicit project inputs)'
