#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

scripts/doc-test.sh
scripts/doc-test-conformance-test.sh
cargo run -p tondo-reliability --locked -- check --root .
cargo run -p tondo-reliability --locked -- tracker lint --root .
cargo run -p tondo-conformance --locked -- validate \
    --root . \
    --manifest conformance/draft/manifest.json \
    --lineage draft
scripts/stdlib-spec-check.sh
scripts/stdlib-async-check.sh

echo "documentation gate: OK"
