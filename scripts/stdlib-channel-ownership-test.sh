#!/usr/bin/env bash
set -euo pipefail
root="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
cd "$root"
work="$(mktemp -d "${TMPDIR:-/tmp}/tondo-channel-ownership.XXXXXX")"
trap 'rm -rf "$work"' EXIT
scripts/stdlib-channel-ownership-check.sh --contract-only >/dev/null
count=0
for filter in \
    '.capabilities.sender.Copy=true' \
    '.capabilities.receiver.Discard=true' \
    '.capabilities.receiver.Share="T: Share"' \
    '.capabilities.terminal.receiver=false' \
    '.capabilities.derivation=["hir"]' \
    '.identity.namespace="trait"' \
    '.identity.generic_arity=0' \
    '.lifecycle.defer="copy-snapshot"' \
    '.lifecycle.mixed_aggregate="ignore-pending-handoffs"' \
    '.lifecycle.terminal_unwind="allocate-drain"' \
    '.lifecycle.sender_discard="test-memory-budget-only"' \
    '.lifecycle.iteration="borrow-and-leave-open"' \
    '.lifecycle.collect="leave-open-at-positive-limit"' \
    '.iterator_protocol.close="fn close(self)"' \
    '.iterator_protocol.state="registration-snapshot"' \
    '.iterator_protocol.source_copy_defer="final-state"' \
    '.iterator_protocol.collect_routes="spawn-without-close"' \
    '.promotion.native_aot="verified"' \
    '.promotion.release=true' \
    '.status=(if .status == "implementation-in-progress" then "verified-public-hosted-ownership" else "implementation-in-progress" end)' \
    '.quality_gate=(if .status == "implementation-in-progress" then "verified-80-percent-per-scope" else "pending-80-percent-per-scope" end)'; do
    jq "$filter" testing/stdlib-channel-ownership.json > "$work/invalid.json"
    if TONDO_STDLIB_CHANNEL_OWNERSHIP_CONTRACT="$work/invalid.json" \
        scripts/stdlib-channel-ownership-check.sh --contract-only > "$work/output" 2>&1; then
        echo "channel ownership contract accepted drift: $filter" >&2
        exit 1
    fi
    count=$((count+1))
done
echo "channel ownership contract tests: OK ($count meaningful refusals)"
