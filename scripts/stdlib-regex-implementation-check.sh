#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_REGEX_CONTRACT:-$root/testing/stdlib-regex.json}"
die() { echo "std.regex implementation: $*" >&2; exit 1; }
TONDO_STDLIB_REGEX_CONTRACT="$contract" bash scripts/stdlib-regex-check.sh >/dev/null
jq -e -L scripts 'include "stdlib_regex_progression"; regex_kernel_progression' "$contract" >/dev/null || die "invalid kernel promotion"
while IFS= read -r path; do
    [[ -f "$path" ]] || die "missing kernel source: $path"
done < <(jq -r '.implementation.sources[]' "$contract")
while IFS= read -r anchor; do
    path="${anchor%%::*}"; name="${anchor##*::}"
    grep -Fq "fn $name(" "$path" || die "missing kernel test: $anchor"
done < <(jq -r '.implementation.tests[]' "$contract")
grep -Fxq 'regex-syntax = "=0.8.10"' Cargo.toml || die "parser dependency is not exactly pinned"
grep -Fxq 'regex-syntax.workspace = true' crates/tondo-stdlib/Cargo.toml || die "kernel dependency missing"
grep -Fxq 'pub mod regex;' crates/tondo-stdlib/src/lib.rs || die "kernel not exported"
for marker in 'ready-stdlib-kernel' 'verified-stdlib-kernel' 'cumulative' 'logical storage' 'global allocator' 'native ABI'; do
    grep -Fq "$marker" docs/contracts/stdlib-regex.md || die "undocumented kernel boundary: $marker"
done
if [[ "$contract" == "$root/testing/stdlib-regex.json" ]]; then
    if [[ "$(jq -r '.implementation.status' "$contract")" == verified-stdlib-kernel ]]; then
        grep -Fq '[x] **STD-REGEX-IMPL-001' TONDO_IMPLEMENTATION_TRACKER.md || die "verified kernel left open in tracker"
    else
        grep -Fq '[ ] **STD-REGEX-IMPL-001' TONDO_IMPLEMENTATION_TRACKER.md || die "unverified kernel closed in tracker"
    fi
fi
echo "std.regex implementation: OK ($(jq -r '.implementation.status' "$contract"); Rust kernel only)"
