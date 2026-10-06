#!/usr/bin/env bash
set -euo pipefail
root="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
cd "$root"
mkdir -p .tmp
tmp="$(mktemp -d "$root/.tmp/uuid-doc-contract.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
negative_count=0
expect_failure() {
    local name="$1"; shift
    if "$@" > "$tmp/$name.log" 2>&1; then
        echo "std.uuid documentation tests: $name unexpectedly passed" >&2; exit 1
    fi
    negative_count=$((negative_count+1))
}
scripts/stdlib-uuid-doc-check.sh
for status in usage-ready verified-public-hosted-usage; do
    jq --arg status "$status" '
      .documentation.status=$status
      | .documentation.quality_gate=(if $status == "usage-ready" then "pending-80-percent-per-scope" else "verified-80-percent-per-scope" end)
      | .promotion.next_blocks=(if $status == "usage-ready" then ["STD-UUID-DOC-001"] else ["STD-NET-IMPL-001"] end)
      | .implementation.required_follow_ups=(if $status == "usage-ready" then ["STD-UUID-DOC-001"] else [] end)
    ' testing/stdlib-uuid.json > "$tmp/parent.json"
    for checker in stdlib-uuid-check stdlib-uuid-implementation-check stdlib-uuid-host-check \
        stdlib-uuid-test-check stdlib-uuid-performance-check stdlib-uuid-conformance-check stdlib-uuid-doc-check; do
        env TONDO_STDLIB_UUID_CONTRACT="$tmp/parent.json" "scripts/$checker.sh" >/dev/null
        for mutation in \
            'status|.documentation.status="pending"' \
            'quality|.documentation.quality_gate="passed"' \
            'fixture|.documentation.fixture="other.to"' \
            'document|del(.documentation.document)' \
            'command|.documentation.command="true"' \
            'stdout|.documentation.expected_stdout="wrong"' \
            'example|.documentation.examples=.documentation.examples[0:5]' \
            'duplicate|.documentation.examples[1]=.documentation.examples[0]' \
            'section|.documentation.sections=.documentation.sections[0:9]' \
            'providers|.documentation.providers="sealed-test-only"' \
            'public-api|.documentation.public_tondo_api="not-implemented"' \
            'private-vm|.documentation.production_vm_registration="test-only"' \
            'native-ABI|.documentation.native_abi="verified"' \
            'native-AOT|.documentation.native_aot="verified"' \
            'next|.promotion.next_blocks=["STD-UUID-CONF-001"]' \
            'followups|.implementation.required_follow_ups=["STD-UUID-HOST-001"]' \
            'early-conformance|.conformance.status="adapter-ready"' \
            'early-host|.host.status="ready-production-hosted"' \
            'early-model|.model.status="ready"' \
            'early-performance|.measurement.status="measurement-ready"' \
            'unknown-field|.documentation.pid=12345'; do
            name="${mutation%%|*}"
            jq "${mutation#*|}" "$tmp/parent.json" > "$tmp/invalid.json"
            expect_failure "$status-$name-$checker" env TONDO_STDLIB_UUID_CONTRACT="$tmp/invalid.json" "scripts/$checker.sh"
        done
    done
done
for section in 'Text and bytes' 'Sentinels and keys' 'External versions' 'Names and encoding' \
    'Errors and results' 'Generate with providers' 'Capabilities and providers' \
    'Limits and costs' 'Executable verification' 'Promotion boundary'; do
    sed "/^### $section$/d" docs/contracts/stdlib-uuid.md > "$tmp/missing-section.md"
    expect_failure missing-section env TONDO_STDLIB_UUID_DOCUMENT="$tmp/missing-section.md" scripts/stdlib-uuid-doc-check.sh
done
for fragment in text-and-bytes sentinels-and-keys external-versions names-and-encoding \
    errors-and-results generate-with-providers entrypoint; do
    python3 - "$fragment" "$tmp/drift.md" <<'PY'
from pathlib import Path
import sys
text = Path('docs/contracts/stdlib-uuid.md').read_text()
marker = '<!-- uuid-doc:' + sys.argv[1] + ' -->\n~~~tondo\n'
Path(sys.argv[2]).write_text(text.replace(marker, marker + '// source drift\n', 1))
PY
    expect_failure fragment-drift env TONDO_STDLIB_UUID_DOCUMENT="$tmp/drift.md" scripts/stdlib-uuid-doc-check.sh
done
for marker in 'does not normalize' 'no strict monotonicity' 'no hidden retry' \
    'no public Tondo provider setter' 'Logical accounting is not RSS' 'not implemented: native UUID ABI'; do
    grep -Fv "$marker" docs/contracts/stdlib-uuid.md > "$tmp/missing-policy.md"
    expect_failure missing-policy env TONDO_STDLIB_UUID_DOCUMENT="$tmp/missing-policy.md" scripts/stdlib-uuid-doc-check.sh
done
base="$tmp/altered"
cp -- tests/runtime/m11-std-uuid-doc-001.to "$base.to"
cp -- tests/runtime/m11-std-uuid-doc-001.stdout "$base.stdout"
cp -- tests/runtime/m11-std-uuid-doc-001.exit "$base.exit"
for extension in stdout exit; do
    printf '%s' "$(cat "$base.$extension")" > "$base.$extension"
    expect_failure missing-LF env TONDO_STDLIB_UUID_DOC_FIXTURE="$base.to" scripts/stdlib-uuid-doc-check.sh
    cp -- "tests/runtime/m11-std-uuid-doc-001.$extension" "$base.$extension"
    printf '\n' >> "$base.$extension"
    expect_failure extra-LF env TONDO_STDLIB_UUID_DOC_FIXTURE="$base.to" scripts/stdlib-uuid-doc-check.sh
    cp -- "tests/runtime/m11-std-uuid-doc-001.$extension" "$base.$extension"
done
python3 - "$base.to" "$tmp/equal-but-wrong.md" <<'PY'
from pathlib import Path
import sys
correct = '2ed6657d-e927-568b-95e1-2665a8aea6a2'
wrong = '00000000-0000-0000-0000-000000000000'
source = Path(sys.argv[1])
source.write_text(source.read_text().replace(correct, wrong))
Path(sys.argv[2]).write_text(Path('docs/contracts/stdlib-uuid.md').read_text().replace(correct, wrong))
PY
expect_failure equal-but-wrong env TONDO_STDLIB_UUID_DOCUMENT="$tmp/equal-but-wrong.md" \
    TONDO_STDLIB_UUID_DOC_FIXTURE="$base.to" scripts/stdlib-uuid-doc-check.sh
grep -Fq 'public Tondo example failed' "$tmp/equal-but-wrong.log" || { cat "$tmp/equal-but-wrong.log"; exit 1; }
python3 - "$base.to" "$tmp/skipped.md" <<'PY'
from pathlib import Path
import sys
call = '    sentinelsAndKeys()?\n'
source = Path('tests/runtime/m11-std-uuid-doc-001.to').read_text()
Path(sys.argv[1]).write_text(source.replace(call, ''))
Path(sys.argv[2]).write_text(Path('docs/contracts/stdlib-uuid.md').read_text().replace(call, ''))
PY
expect_failure equally-skipped env TONDO_STDLIB_UUID_DOCUMENT="$tmp/skipped.md" \
    TONDO_STDLIB_UUID_DOC_FIXTURE="$base.to" scripts/stdlib-uuid-doc-check.sh
grep -Fq 'entrypoint must execute every example' "$tmp/equally-skipped.log" || { cat "$tmp/equally-skipped.log"; exit 1; }
mkdir -p "$tmp/project/src"
cp -- tests/runtime/m11-std-uuid-doc-001.to "$tmp/project/src/main.to"
python3 -B scripts/stdlib_uuid_doc.py project > "$tmp/project/tondo.toml"
timeout 90 cargo run -q -p tondo-cli --locked -- check --project "$tmp/project"
output="$(timeout 90 cargo run -q -p tondo-cli --locked -- run --project "$tmp/project")"
[[ "$output" == uuid-doc-ok ]] || { echo "documented project output differs" >&2; exit 1; }
for capabilities in '["console"]' '["console","entropy"]' '["civil-clock","console"]'; do
    python3 - "$capabilities" > "$tmp/project/tondo.toml" <<'PY'
import json
import sys
print('[package]\nname = "uuid_usage"\n\n[target]\ncapabilities = ' + json.dumps(json.loads(sys.argv[1])))
PY
    expect_failure missing-capability timeout 90 cargo run -q -p tondo-cli --locked -- \
        check --diagnostic-format json --project "$tmp/project"
    grep -Eq '"code":[[:space:]]*"E1008"' "$tmp/missing-capability.log" || { cat "$tmp/missing-capability.log"; exit 1; }
done
python3 -B scripts/stdlib_uuid_doc.py pure-source > "$tmp/project/src/main.to"
printf '[package]\nname = "uuid_usage"\n\n[target]\ncapabilities = ["console"]\n' > "$tmp/project/tondo.toml"
output="$(timeout 90 cargo run -q -p tondo-cli --locked -- run --project "$tmp/project")"
[[ "$output" == uuid-doc-ok ]] || { echo "core/v5 consumed a provider capability" >&2; exit 1; }
bash -n scripts/stdlib-uuid-doc-check.sh scripts/stdlib-uuid-doc-test.sh
echo "std.uuid documentation tests: OK ($negative_count refusals; exact fragments; actual project, provider grants, pure core and assertion execution)"
