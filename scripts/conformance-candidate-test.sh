#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

help_output="$(scripts/conformance-candidate.sh --help 2>&1)"
grep -Fq -- '--revision N' <<<"$help_output"
grep -Fq -- '--candidate PATH' <<<"$help_output"

scripts/conformance-candidate.sh check --revision 24 >/dev/null
scripts/conformance-candidate.sh check --candidate conformance/candidates/revision-24 >/dev/null

if scripts/conformance-candidate.sh check --revision 0 >/dev/null 2>&1; then
    echo "conformance candidate test: invalid revision unexpectedly passed" >&2
    exit 1
fi
if scripts/conformance-candidate.sh generate --revision 24 >/dev/null 2>&1; then
    echo "conformance candidate test: generate accepted historical revision selector" >&2
    exit 1
fi

echo "conformance candidate tests: OK"
