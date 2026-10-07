#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/tondo-select-transactions.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
bash scripts/async-select-transactions-check.sh >/dev/null
for mutation in '.protocol.self_rendezvous = true' '.protocol.joint_result_admission = false' \
    '.protocol.loser_consumes_payload = true' '.protocol.source_checkpoint = "after-continuation"' \
    '.protocol.artifact_prefix_verification = false' '.native_abi_or_aot = true' '.model.seeds = 1' \
    '.status = "unsupported-promotion"'; do
    jq "$mutation" testing/async-select-transactions.json > "$tmp/invalid.json"
    if TONDO_SELECT_TRANSACTIONS_CONTRACT="$tmp/invalid.json" bash scripts/async-select-transactions-check.sh >/dev/null 2>&1; then
        echo "invalid transaction contract accepted: $mutation" >&2
        exit 1
    fi
done
mkdir "$tmp/no-rg" "$tmp/no-grep"
for utility in bash jq tail cmp grep dirname; do
    utility_path="$(command -v "$utility")"
    ln -s "$utility_path" "$tmp/no-rg/$utility"
    [[ "$utility" == grep ]] || ln -s "$utility_path" "$tmp/no-grep/$utility"
done
env PATH="$tmp/no-rg" bash scripts/async-select-transactions-check.sh
if env PATH="$tmp/no-grep" bash scripts/async-select-transactions-check.sh > "$tmp/no-grep.log" 2>&1; then
    echo 'transaction contract accepted without its required search utility' >&2
    exit 1
fi
echo 'hosted selection transaction contract tests: OK'
