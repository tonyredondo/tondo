#!/usr/bin/env bash
set -euo pipefail

logs="${1:-}"
if [[ -z "$logs" || ! -d "$logs" ]]; then
    echo "usage: scripts/mutation-infrastructure-check.sh <cargo-mutants-log-directory>" >&2
    exit 2
fi

patterns=(
    'rustc interrupted by SIG'
    'internal compiler error:'
    'LLVM ERROR:'
    'rustc-LLVM ERROR:'
)

for pattern in "${patterns[@]}"; do
    if rg --fixed-strings --quiet --glob '*.log' "$pattern" "$logs"; then
        echo "mutation infrastructure failure: cargo-mutants log contains '$pattern'" >&2
        exit 1
    fi
done

echo "mutation infrastructure logs: OK"
