#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
checker="$root/scripts/mutation-infrastructure-check.sh"
workspace="$(mktemp -d "${TMPDIR:-/tmp}/tondo-mutation-infrastructure.XXXXXX")"
trap 'rm -rf -- "$workspace"' EXIT

mkdir -p "$workspace/log"
printf '%s\n' 'error[E0277]: a deliberately mutated type does not implement Default' \
    > "$workspace/log/unviable.log"
"$checker" "$workspace/log" >/dev/null

mkdir -p "$workspace/bin"
ln -s "$(command -v bash)" "$workspace/bin/bash"
PATH="$workspace/bin" "$checker" "$workspace/log" >/dev/null

for signature in \
    'rustc interrupted by SIGILL, printing backtrace' \
    'internal compiler error: unexpected query cycle' \
    'LLVM ERROR: out of memory' \
    'rustc-LLVM ERROR: broken module'; do
    printf '%s\n' "$signature" > "$workspace/log/crash.log"
    if "$checker" "$workspace/log" >/dev/null 2>&1; then
        echo "mutation infrastructure checker accepted '$signature'" >&2
        exit 1
    fi
done

rm "$workspace/log/crash.log"
printf '%s\n' 'ordinary test failure' > "$workspace/log/caught.log"
"$checker" "$workspace/log" >/dev/null

if "$checker" "$workspace/missing" >/dev/null 2>&1; then
    echo 'mutation infrastructure checker accepted a missing log directory' >&2
    exit 1
fi

echo 'mutation infrastructure checker tests: OK'
