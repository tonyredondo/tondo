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

logs_contain() {
    local pattern="$1"
    local log line
    local files

    shopt -s globstar nullglob
    files=("$logs"/**/*.log)
    shopt -u globstar nullglob

    for log in "${files[@]}"; do
        while IFS= read -r line || [[ -n "$line" ]]; do
            if [[ "$line" == *"$pattern"* ]]; then
                return 0
            fi
        done < "$log"
    done
    return 1
}

for pattern in "${patterns[@]}"; do
    if logs_contain "$pattern"; then
        echo "mutation infrastructure failure: cargo-mutants log contains '$pattern'" >&2
        exit 1
    fi
done

echo "mutation infrastructure logs: OK"
