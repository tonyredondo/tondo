#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

mode="${1:-check}"
help_requested=0
if [[ "$mode" == --help || "$mode" == -h ]]; then
    mode=check
    help_requested=1
fi
if (($# > 0)); then
    shift
fi

usage() {
    cat >&2 <<'EOF'
usage: scripts/conformance-candidate.sh [check|generate] [options]

Options:
  --revision N       select a historical candidate revision (check only)
  --candidate PATH   select an explicit candidate directory

Environment:
  TONDO_CONFORMANCE_REVISION   default revision override
  TONDO_CONFORMANCE_CANDIDATE   default candidate directory override
EOF
}

if ((help_requested)); then
    usage
    exit 0
fi

revision_override="${TONDO_CONFORMANCE_REVISION:-}"
candidate_override="${TONDO_CONFORMANCE_CANDIDATE:-}"
while (($# > 0)); do
    case "$1" in
        --revision)
            (($# >= 2)) || { usage; exit 2; }
            [[ "$2" =~ ^[1-9][0-9]*$ ]] || {
                echo "candidate: revision must be a positive integer" >&2
                exit 2
            }
            [[ -z "$revision_override" ]] || {
                echo "candidate: revision specified more than once" >&2
                exit 2
            }
            revision_override="$2"
            shift 2
            ;;
        --candidate)
            (($# >= 2)) || { usage; exit 2; }
            [[ -z "$candidate_override" ]] || {
                echo "candidate: directory specified more than once" >&2
                exit 2
            }
            candidate_override="$2"
            shift 2
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            usage
            exit 2
            ;;
    esac
done

revision="$(jq -r '.revision' conformance/draft/manifest.json)"
if [[ -n "$revision_override" ]]; then
    [[ "$revision_override" =~ ^[1-9][0-9]*$ ]] || {
        echo "candidate: revision must be a positive integer" >&2
        exit 2
    }
    [[ "$mode" == check ]] || {
        echo "candidate: --revision is only valid with check" >&2
        exit 2
    }
    [[ -z "$candidate_override" ]] || {
        echo "candidate: use either --revision or --candidate, not both" >&2
        exit 2
    }
    candidate="conformance/candidates/revision-$revision_override"
else
    candidate="${candidate_override:-conformance/candidates/revision-$revision}"
fi

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
        usage
        exit 2
        ;;
esac
