#!/usr/bin/env bash
set -euo pipefail
root="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
cd "$root"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/tondo-log-host-contract.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
scripts/stdlib-log-host-check.sh --contract-only >/dev/null
for mutation in '.surface.constructors = 1' '.surface.file_constructor = "first-emit"' \
    '.surface.console_constructor = "suspends"' '.surface.builtin_copy_or_discard = true' \
    '.providers.append = "existing-only"' '.providers.parent_creation = true' \
    '.providers.hidden_worker = true' '.providers.rotation_or_retry = true' \
    '.queue.batching = true' '.queue.short_write = "restart-from-zero"' \
    '.queue.drain = ["worker"]' '.admission.error_bytes = 0' \
    '.admission.liveness = "test-only"' '.native_abi_or_aot = true' \
    '.promotion.performance = "verified"' '.promotion.full_owner = true' \
    '.fixtures |= .[1:]' '.tests |= .[1:]' \
    '.status = "verified-production-hosted" | .quality_gate = "pending-current-source-proof"'; do
    jq "$mutation" testing/stdlib-log-host.json > "$tmp/invalid.json"
    if TONDO_STDLIB_LOG_HOST_CONTRACT="$tmp/invalid.json" scripts/stdlib-log-host-check.sh --contract-only > "$tmp/output" 2>&1; then
        echo "logging host accepted contract drift: $mutation" >&2
        exit 1
    fi
done
jq '.status = "verified-production-hosted" | .quality_gate = "verified-80-percent-per-scope"' testing/stdlib-log-host.json > "$tmp/verified-host.json"
jq '.implementation.host = "verified-production-hosted"' testing/stdlib-log.json > "$tmp/verified-parent.json"
jq '.promotion.host_sinks = "verified-production-hosted"' testing/stdlib-log-implementation.json > "$tmp/verified-core.json"
env TONDO_STDLIB_LOG_HOST_CONTRACT="$tmp/verified-host.json" \
    TONDO_STDLIB_LOG_CONTRACT="$tmp/verified-parent.json" \
    TONDO_STDLIB_LOG_IMPLEMENTATION_CONTRACT="$tmp/verified-core.json" \
    scripts/stdlib-log-host-check.sh --contract-only
if env TONDO_STDLIB_LOG_HOST_CONTRACT="$tmp/verified-host.json" \
    scripts/stdlib-log-host-check.sh --contract-only > "$tmp/mismatched.log" 2>&1; then
    echo 'logging host accepted mismatched parent/host status' >&2
    exit 1
fi
echo 'logging host contract tests: OK (providers; capacity; offsets; lifetime; promotion refusals)'
