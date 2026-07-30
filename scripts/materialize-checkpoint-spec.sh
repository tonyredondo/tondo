#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tag="v0.1.0"
commit="2aec7e845ef62582015673c677c2884b97b0b8f9"
logical_path="TONDO_LANGUAGE_SPEC.md"
expected_sha256="ded4e17ab57836d032e5fb9e5be5dba03fc83ac6ff74cee90ab1bb7f8e5c7084"
output="conformance/checkpoints/v0.1.0/TONDO_LANGUAGE_SPEC.md"
mode="${1:-generate}"

sha256_file() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d ' ' -f 1
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | cut -d ' ' -f 1
    else
        echo "neither sha256sum nor shasum is available" >&2
        return 1
    fi
}

if [[ "$mode" != "generate" && "$mode" != "--check" ]]; then
    echo "usage: scripts/materialize-checkpoint-spec.sh [--check]" >&2
    exit 2
fi

actual_commit="$(git rev-parse "${tag}^{commit}")"
if [[ "$actual_commit" != "$commit" ]]; then
    echo "$tag resolves to $actual_commit, expected $commit" >&2
    exit 1
fi

temporary="$(mktemp)"
trap 'rm -f "$temporary"' EXIT
git show "${tag}:${logical_path}" > "$temporary"

actual_sha256="$(sha256_file "$temporary")"
if [[ "$actual_sha256" != "$expected_sha256" ]]; then
    echo "$tag:$logical_path has SHA-256 $actual_sha256, expected $expected_sha256" >&2
    exit 1
fi

if [[ "$mode" == "--check" ]]; then
    if [[ ! -f "$output" ]] || ! cmp -s "$temporary" "$output"; then
        echo "$output does not match $tag:$logical_path" >&2
        exit 1
    fi
    echo "$output matches $tag:$logical_path ($expected_sha256)"
    exit 0
fi

mkdir -p "$(dirname "$output")"
if [[ -f "$output" ]] && cmp -s "$temporary" "$output"; then
    echo "$output is already current"
else
    mv "$temporary" "$output"
    trap - EXIT
    echo "materialized $output ($expected_sha256)"
fi
