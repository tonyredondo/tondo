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
mkdir "$tmp/no-rg" "$tmp/no-grep"
for utility in bash jq tail cmp grep dirname; do
    utility_path="$(command -v "$utility")"
    ln -s "$utility_path" "$tmp/no-rg/$utility"
    [[ "$utility" == grep ]] || ln -s "$utility_path" "$tmp/no-grep/$utility"
done
env PATH="$tmp/no-rg" bash scripts/stdlib-net-host-check.sh
if env PATH="$tmp/no-grep" bash scripts/stdlib-net-host-check.sh > "$tmp/no-grep.log" 2>&1; then
    echo 'network hosted contract accepted without its required search utility' >&2
    exit 1
fi
echo 'std.net public hosted contract tests: OK'
