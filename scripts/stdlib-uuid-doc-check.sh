#!/usr/bin/env bash
set -euo pipefail
root="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_UUID_CONTRACT:-testing/stdlib-uuid.json}"
document="${TONDO_STDLIB_UUID_DOCUMENT:-docs/contracts/stdlib-uuid.md}"
fixture="${TONDO_STDLIB_UUID_DOC_FIXTURE:-tests/runtime/m11-std-uuid-doc-001.to}"
die() { echo "std.uuid documentation: $*" >&2; exit 1; }
for path in "$contract" "$document" "$fixture"; do
    [[ -f "$path" ]] || die "missing input: $path"
done
TONDO_STDLIB_UUID_CONTRACT="$contract" scripts/stdlib-uuid-conformance-check.sh >/dev/null
jq -e -L scripts '
  include "stdlib_uuid_progression";
  uuid_kernel_progression and uuid_documentation_metadata
  and .conformance.status == "verified-public-hosted-vm-and-native-kernel-process"
' "$contract" >/dev/null || die "documentation progression or prerequisite differs"
for marker in \
    '## Executable usage guide for `std.uuid`' \
    '### Text and bytes' '### Sentinels and keys' '### External versions' \
    '### Names and encoding' '### Errors and results' '### Generate with providers' \
    '### Capabilities and providers' '### Limits and costs' \
    '### Executable verification' '### Promotion boundary' \
    'does not normalize' 'no strict monotonicity' 'no hidden retry' \
    'no public Tondo provider setter' 'not a secret or authenticator' \
    '16 MiB' 'Logical accounting is not RSS' 'ordinary VM records' \
    'not implemented: native UUID ABI' 'stdlib-uuid-performance.md' \
    'E1008' 'STD-UUID-DOC-001' 'STD-NET-IMPL-001'; do
    grep -Fq "$marker" "$document" || die "guide misses: $marker"
done
python3 -B scripts/stdlib_uuid_doc.py check --document "$document" --fixture "$fixture"
base="${fixture%.to}"
[[ -f "$base.exit" && -f "$base.stdout" ]] || die "fixture sidecars are missing"
cmp -s "$base.exit" <(printf '0\n') || die "fixture exit sidecar differs"
cmp -s "$base.stdout" <(printf 'uuid-doc-ok\n') || die "fixture stdout sidecar differs"
target_dir="${CARGO_TARGET_DIR:-target}"
mkdir -p .tmp
output="$(mktemp "$root/.tmp/uuid-doc-output.XXXXXX")"
trap 'rm -f -- "$output"' EXIT
CARGO_TARGET_DIR="$target_dir" timeout 90 cargo run -q -p tondo-cli --locked -- run "$fixture" > "$output" \
    || die "public Tondo example failed"
cmp -s "$base.stdout" "$output" || die "actual example output differs"
echo "std.uuid documentation: OK (six public Tondo paths; exact guide fragments; OS providers and bounded promotion)"
