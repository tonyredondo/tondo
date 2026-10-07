#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/tondo-net-host-contract.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
bash scripts/stdlib-net-host-check.sh >/dev/null
for mutation in '.surface.operations = 39' '.providers.resolver = "system"' \
    '.providers.import_effects = true' '.providers.blocking_fallback = true' \
    '.providers.tls_versions = ["1.3"]' '.admission.tls_consumes_tcp_on_error = false' \
    '.native_abi_or_aot = true' '.public_api_promoted = true' '.quality_gate = "unsupported-proof"'; do
    jq "$mutation" testing/stdlib-net-host.json > "$tmp/invalid.json"
    if TONDO_STDLIB_NET_HOST_CONTRACT="$tmp/invalid.json" bash scripts/stdlib-net-host-check.sh >/dev/null 2>&1; then
        echo "invalid hosted contract accepted: $mutation" >&2
        exit 1
    fi
done
echo 'std.net public hosted contract tests: OK'
