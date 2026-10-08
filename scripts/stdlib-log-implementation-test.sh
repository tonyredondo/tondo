#!/usr/bin/env bash
set -euo pipefail
root="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
cd "$root"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/tondo-log-implementation.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
scripts/stdlib-log-implementation-check.sh --contract-only
reject() {
    jq "$1" testing/stdlib-log-implementation.json > "$tmp/contract.json"
    if TONDO_STDLIB_LOG_IMPLEMENTATION_CONTRACT="$tmp/contract.json" \
        scripts/stdlib-log-implementation-check.sh --contract-only > "$tmp/output" 2>&1; then
        echo "logging implementation accepted contract drift: $1" >&2
        exit 1
    fi
}
reject '.surface.signatures |= .[1:]'
reject '.surface.private_helpers += ["std.log.Logger.emit"]'
reject '.surface.signatures[14].effect = "pure"'
reject '.memory.validation_node_bytes = 0'
reject '.memory.failed_admission = "partial-publication"'
reject '.lifecycle.logger = "Copy"'
reject '.lifecycle.close = "borrowed"'
reject '.lifecycle.Io = "silent-prefix"'
reject '.capabilities.provider_reads = true'
reject '.promotion.native_aot = "verified"'
reject '.promotion.host_sinks = "verified"'
reject '.status = "verified-public-hosted-core" | .quality_gate = "pending-80-percent-per-scope"'
reject '.sources["crates/tondo-stdlib/src/log.rs"] = "sha256:wrong"'
echo 'logging implementation contract tests: OK (surface, limits, lifecycle, capabilities and provenance refusals)'
