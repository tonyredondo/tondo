#!/usr/bin/env bash
set -euo pipefail
root="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
cd "$root"
scripts/stdlib-net-doc-check.sh
python3 -B scripts/stdlib_net_doc_test.py
mkdir -p .tmp
tmp="$(mktemp -d "$root/.tmp/net-doc-contract.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
negative_count=0
reject() {
    local name="$1"; shift
    if "$@" > "$tmp/$name.log" 2>&1; then
        echo "std.net documentation: $name unexpectedly passed" >&2; exit 1
    fi
    negative_count=$((negative_count+1))
}
for status in usage-ready verified-public-hosted-usage; do
    jq --arg status "$status" '
      .documentation.status=$status
      | .documentation.quality_gate=(if $status=="usage-ready"
        then "pending-80-percent-per-scope" else "verified-80-percent-per-scope" end)
      | .promotion.next_blocks=(if $status=="usage-ready"
        then ["STD-NET-DOC-001"] else ["STD-LOG-IMPL-001"] end)
    ' testing/stdlib-net.json > "$tmp/parent.json"
    for checker in stdlib-net-check stdlib-net-implementation-check stdlib-net-host-check \
        stdlib-net-test-check stdlib-net-performance-check stdlib-net-conformance-check stdlib-net-doc-check; do
        env TONDO_STDLIB_NET_CONTRACT="$tmp/parent.json" "scripts/$checker.sh" >/dev/null
        for mutation in \
            'status|.documentation.status="pending"' \
            'quality|.documentation.quality_gate="passed"' \
            'fixture|.documentation.fixture="other.to"' \
            'document|del(.documentation.document)' \
            'project|.documentation.project="ambient.toml"' \
            'command|.documentation.command="true"' \
            'stdout|.documentation.expected_stdout="wrong"' \
            'examples|.documentation.examples|=.[0:5]' \
            'duplicate|.documentation.examples[1]=.documentation.examples[0]' \
            'sections|.documentation.sections|=.[0:9]' \
            'provider|.documentation.providers="external-service"' \
            'public-api|.documentation.public_tondo_api="not-implemented"' \
            'private-vm|.documentation.production_vm_registration="test-only"' \
            'native-ABI|.documentation.native_abi="verified"' \
            'native-AOT|.documentation.native_aot="verified"' \
            'next|.promotion.next_blocks=[]' \
            'early-conformance|.conformance.status="adapter-ready"' \
            'early-host|.host.status="implementation-in-progress"' \
            'early-model|.model.status="ready"' \
            'early-performance|.measurement.status="measurement-ready"' \
            'unknown-field|.documentation.pid=12345'; do
            name="${mutation%%|*}"
            jq "${mutation#*|}" "$tmp/parent.json" > "$tmp/invalid.json"
            reject "$status-$name-$checker" env TONDO_STDLIB_NET_CONTRACT="$tmp/invalid.json" "scripts/$checker.sh"
        done
    done
done
# Equal documentary/source edits must still fail at actual execution.
python3 - "$tmp/altered.to" "$tmp/altered.md" <<'PY'
from pathlib import Path
import sys
before='assert(hosts[host] == some(1))'
after='assert(hosts[host] == some(999))'
Path(sys.argv[1]).write_text(Path('acceptance/projects/net-usage/src/main.to').read_text().replace(before,after))
Path(sys.argv[2]).write_text(Path('docs/contracts/stdlib-net.md').read_text().replace(before,after))
PY
reject equal-but-wrong env TONDO_STDLIB_NET_DOC_SOURCE="$tmp/altered.to" \
    TONDO_STDLIB_NET_DOCUMENT="$tmp/altered.md" scripts/stdlib-net-doc-check.sh
grep -Fq 'public Tondo example failed' "$tmp/equal-but-wrong.log"
mkdir -p "$tmp/project/src"
cp -- acceptance/projects/net-usage/src/main.to "$tmp/project/src/main.to"
target_dir="${CARGO_TARGET_DIR:-target}"
[[ "$target_dir" = /* ]] || target_dir="$root/$target_dir"
for capability in network clock console; do
    python3 - "$capability" "$tmp/project/tondo.toml" <<'PY'
from pathlib import Path
import sys
text=Path('acceptance/projects/net-usage/tondo.toml').read_text()
capability=sys.argv[1]
text=text.replace('"'+capability+'", ', '').replace(', "'+capability+'"', '')
if capability=='network':text=text.split('\n[target.network]',1)[0]+'\n'
Path(sys.argv[2]).write_text(text)
PY
    reject "$capability" timeout 90 "$target_dir/debug/tondo" check \
        --diagnostic-format json --project "$tmp/project"
    grep -Eq '"code":[[:space:]]*"E1008"' "$tmp/$capability.log"
done
cp -- acceptance/projects/net-usage/tondo.toml "$tmp/project/tondo.toml"
python3 - "$tmp/project/src/main.to" <<'PY'
from pathlib import Path
import sys
source=Path('acceptance/projects/net-usage/src/main.to').read_text()
Path(sys.argv[1]).write_text(source.replace('    defer net.TcpListener.close(listener)\n',''))
PY
reject affine-owner timeout 90 "$target_dir/debug/tondo" check \
    --diagnostic-format json --project "$tmp/project"
grep -Eq '"code":[[:space:]]*"E1404"' "$tmp/affine-owner.log"
echo "std.net documentation tests: OK ($negative_count refusals; seven fragment laws and actual public diagnostics)"
