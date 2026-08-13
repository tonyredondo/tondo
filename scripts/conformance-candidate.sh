#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

mode="${1:-check}"
revision="$(jq -r '.revision' conformance/draft/manifest.json)"
candidate="conformance/candidates/revision-$revision"

case "$mode" in
    check)
        cargo run -p tondo-reliability --locked -- candidate verify \
            --root . \
            --candidate "$candidate"
        ;;
    generate)
        cargo_target_dir="${CARGO_TARGET_DIR:-target}"
        stage="target/reliability/candidate-inputs"
        mkdir -p "$stage"
        cp "$cargo_target_dir/reliability/quality/coverage.json" "$stage/coverage.json"
        cp "$cargo_target_dir/reliability/quality/coverage.binding.json" "$stage/coverage-binding.json"
        cp "$cargo_target_dir/reliability/quality/mutation/mutants.out/outcomes.json" "$stage/mutation.json"
        cp "$cargo_target_dir/reliability/quality/mutation.binding.json" "$stage/mutation-binding.json"
        cp "$cargo_target_dir/reliability/quality/layer-evidence.json" "$stage/layer-evidence.json"
        cp "$cargo_target_dir/reliability/evidence/doc-test.json" "$stage/doc-test.json"
        cp testing/doc-test-runtime-links.json "$stage/doc-test-runtime-links.json"
        cargo run -p tondo-reliability --locked -- candidate seal \
            --root . \
            --proof "conformance/proofs/revision-$revision" \
            --coverage "$stage/coverage.json" \
            --coverage-binding "$stage/coverage-binding.json" \
            --mutants "$stage/mutation.json" \
            --mutants-binding "$stage/mutation-binding.json" \
            --layer-evidence "$stage/layer-evidence.json" \
            --doc-test "$stage/doc-test.json" \
            --doc-test-links "$stage/doc-test-runtime-links.json" \
            --output "$candidate"
        ;;
    *)
        echo "usage: scripts/conformance-candidate.sh [check|generate]" >&2
        exit 2
        ;;
esac
