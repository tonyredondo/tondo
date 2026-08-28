#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
scripts/native-conf-adapter-check.sh
target_dir="${CARGO_TARGET_DIR:-$root/target-fast}"
tmp="$(mktemp -d "$target_dir/reliability/native-conf-adapter.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
for backend in cranelift llvm; do
    for category in language testing stdlib; do
        scripts/native-conformance-adapter.sh \
            --backend "$backend" \
            --target x86_64-unknown-linux-gnu \
            --category "$category" \
            --output "$tmp/$backend-$category.json"
        jq -e --arg b "$backend" --arg c "$category" \
            '.backend == $b and .category == $c and .status == "passed" and (.observations | length > 0) and (.physical_paths == [])' \
            "$tmp/$backend-$category.json" >/dev/null
    done
done
if scripts/native-conformance-adapter.sh --backend custom --target x86_64-unknown-linux-gnu --category language --output "$tmp/invalid.json" >/dev/null 2>&1; then
    echo "native conformance adapter tests: unknown backend unexpectedly accepted" >&2
    exit 1
fi
if scripts/native-conformance-adapter.sh --backend llvm --target unknown --category language --output "$tmp/invalid.json" >/dev/null 2>&1; then
    echo "native conformance adapter tests: unknown target unexpectedly accepted" >&2
    exit 1
fi
echo "native conformance adapter tests: OK (six independent owner/backend observations)"
