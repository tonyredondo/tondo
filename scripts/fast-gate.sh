#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
config="${TONDO_FAST_GATE_CONFIG:-testing/fast-gate.json}"
[[ -f "$config" ]] || { echo "fast gate: missing policy: $config" >&2; exit 2; }
jq -e '
    .format == "tondo-fast-gate/1"
    and .edition == "0.1"
    and .coverage.new_executable_line_min_basis_points == 10000
    and .mutation.required_for_rust_source_changes == true
    and (.shared_paths | length > 0)
    and (.packages | length > 0)
' "$config" >/dev/null || {
    echo "fast gate: invalid policy: $config" >&2
    exit 2
}
mutation_timeout="$(jq -r '.mutation.timeout_seconds' "$config")"
mutation_build_timeout="$(jq -r '.mutation.build_timeout_seconds' "$config")"

usage() {
    cat <<'EOF'
Usage: scripts/fast-gate.sh [options]

The fast gate validates only the changed surface. It escalates to the full
test gate for shared compiler/runtime/frontier changes. It never promotes a
draft conformance result and never changes the quality baseline.

Options:
  --base REF       compare REF...HEAD (default: CI base or HEAD^)
  --diff FILE      use an existing unified diff
  --dry-run        classify and print commands without executing them
  --full           force the full test gate
  --help           show this help

Environment:
  TONDO_FAST_CHANGED_FILES   newline-delimited files, intended for tests
  TONDO_FAST_GATE_DIR         evidence directory (default: target/...)
  CARGO_TARGET_DIR            build/cache directory, optionally on SSD
EOF
}

base="${TONDO_FAST_BASE:-}"
diff_file="${TONDO_FAST_DIFF:-}"
dry_run=0
force_full=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --base) shift; base="${1:?--base requires a ref}" ;;
        --diff) shift; diff_file="${1:?--diff requires a file}" ;;
        --dry-run) dry_run=1 ;;
        --full) force_full=1 ;;
        --help|-h) usage; exit 0 ;;
        *) echo "fast gate: unknown option: $1" >&2; usage >&2; exit 2 ;;
    esac
    shift
done

export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-1}"
if [[ -z "${CARGO_BUILD_JOBS:-}" ]]; then
    if command -v nproc >/dev/null 2>&1; then
        export CARGO_BUILD_JOBS="$(nproc)"
    else
        export CARGO_BUILD_JOBS=2
    fi
fi

target_dir="${CARGO_TARGET_DIR:-target}"
if [[ "$target_dir" = /* ]]; then
    gate_dir="${TONDO_FAST_GATE_DIR:-$target_dir/reliability/fast-gate}"
else
    gate_dir="${TONDO_FAST_GATE_DIR:-$root/$target_dir/reliability/fast-gate}"
fi
mkdir -p "$gate_dir"
summary="$gate_dir/summary.json"
plan="$gate_dir/plan.txt"

if [[ -z "$diff_file" ]]; then
    if [[ -n "${TONDO_FAST_CHANGED_FILES:-}" ]]; then
        diff_file="$(mktemp "${TMPDIR:-/tmp}/tondo-fast-gate.XXXXXX.diff")"
        : > "$diff_file"
        while IFS= read -r path; do
            [[ -n "$path" ]] && printf '+++ b/%s\n' "$path" >> "$diff_file"
        done <<< "$TONDO_FAST_CHANGED_FILES"
    else
        if [[ -z "$base" ]]; then
            if [[ -n "${GITHUB_BASE_REF:-}" ]]; then
                base="origin/$GITHUB_BASE_REF"
            else
                base="HEAD^"
            fi
        fi
        if git rev-parse --verify "$base" >/dev/null 2>&1; then
            diff_file="$(mktemp "${TMPDIR:-/tmp}/tondo-fast-gate.XXXXXX.diff")"
            git diff --binary --no-ext-diff "$base...HEAD" > "$diff_file"
            git diff --binary --no-ext-diff >> "$diff_file"
            git diff --binary --no-ext-diff --cached >> "$diff_file"
            while IFS= read -r untracked; do
                [[ -n "$untracked" ]] || continue
                git diff --no-index --binary /dev/null "$untracked" >> "$diff_file" || [[ "$?" -eq 1 ]]
            done < <(git ls-files --others --exclude-standard)
        else
            echo "fast gate: cannot resolve base '$base'; pass --base or --diff" >&2
            exit 2
        fi
    fi
fi

changed_file_list="$(mktemp "${TMPDIR:-/tmp}/tondo-fast-gate.XXXXXX.files")"
trap 'rm -f -- "$changed_file_list"' EXIT
if [[ -n "${TONDO_FAST_CHANGED_FILES:-}" ]]; then
    printf '%s\n' "$TONDO_FAST_CHANGED_FILES" | sed '/^$/d' | sort -u > "$changed_file_list"
else
    {
        sed -n 's#^+++ b/##p' "$diff_file"
        if [[ -n "$base" ]] && git rev-parse --verify "$base" >/dev/null 2>&1; then
            git diff --name-only --diff-filter=ACMR "$base...HEAD"
        fi
        git diff --name-only --diff-filter=ACMR
        git diff --cached --name-only --diff-filter=ACMR
        git ls-files --others --exclude-standard
    } | sed '/^$/d' | sort -u > "$changed_file_list"
fi

mapfile -t changed_files < "$changed_file_list"
rust_changed=0
full_required="$force_full"
declare -A packages=()

is_shared() {
    local path="$1"
    local prefix
    while IFS= read -r prefix; do
        [[ -n "$prefix" ]] || continue
        if [[ "$prefix" == */* && "$prefix" == */ ]]; then
            [[ "$path" == "$prefix"* ]] && return 0
        elif [[ "$path" == "$prefix" ]]; then
            return 0
        fi
    done < <(jq -r '.shared_paths[]' "$config")
    return 1
}

package_for() {
    local path="$1" prefix package
    while IFS=$'\t' read -r prefix package; do
        [[ -n "$prefix" && -n "$package" ]] || continue
        if [[ "$path" == "$prefix"* ]]; then
            printf '%s' "$package"
            return 0
        fi
    done < <(jq -r '.packages[] | [.prefix, .package] | @tsv' "$config")
    return 1
}

for path in "${changed_files[@]}"; do
    [[ "$path" == *.rs ]] && rust_changed=1
    if is_shared "$path"; then
        full_required=1
    fi
    if package="$(package_for "$path" 2>/dev/null)"; then
        packages["$package"]=1
    fi
done

scope="impacted"
if (( full_required )); then scope="shared-frontier"; fi

run() {
    local label="$1"
    shift
    printf '%s' "$label" >> "$plan"
    printf ' %q' "$@" >> "$plan"
    printf '\n' >> "$plan"
    if (( dry_run )); then
        return 0
    fi
    echo "::group::$label"
    "$@"
    echo "::endgroup::"
}

: > "$plan"
if (( ${#changed_files[@]} == 0 )); then
    if (( full_required )); then
        run full-test-gate bash scripts/test-gate.sh
    else
        echo "fast gate: no changed files; running formatter only"
        run fmt cargo fmt --all -- --check
    fi
else
    run fmt cargo fmt --all -- --check
    if (( full_required )); then
        run full-test-gate bash scripts/test-gate.sh
    else
        for package in "${!packages[@]}"; do
            run "check-$package" cargo check -p "$package" --all-targets --locked
            run "test-$package" cargo test -p "$package" --all-targets --locked
        done
        if (( rust_changed )); then
            if ! command -v cargo-llvm-cov >/dev/null 2>&1; then
                echo "fast gate: cargo-llvm-cov is required for changed-line coverage" >&2
                exit 1
            fi
            coverage_report="$gate_dir/coverage.json"
            if (( ${#packages[@]} == 1 )); then
                only_package="${!packages[@]}"
                run changed-line-coverage cargo llvm-cov --package "$only_package" \
                    --all-targets --json --output-path "$coverage_report" --no-clean
            else
                run changed-line-coverage cargo llvm-cov --workspace \
                    --all-targets --json --output-path "$coverage_report" --no-clean
            fi
            if (( ! dry_run )); then
                bash scripts/fast-coverage-check.sh "$diff_file" "$coverage_report" "$root"
            fi
            if ! command -v cargo-mutants >/dev/null 2>&1; then
                echo "fast gate: cargo-mutants is required for changed Rust source" >&2
                exit 1
            fi
            mutation_output="$gate_dir/mutation"
            run diff-mutation cargo mutants --in-diff "$diff_file" --baseline run \
                --jobs 1 --timeout "$mutation_timeout" --build-timeout "$mutation_build_timeout" --cargo-arg=--locked \
                --output "$mutation_output" --no-times --colors never --annotations none
        fi
    fi
fi

if (( dry_run )); then
    echo "fast gate: dry-run scope=$scope files=${#changed_files[@]}"
    cat "$plan"
    exit 0
fi

jq -n \
    --arg format "tondo-fast-gate-evidence/1" \
    --arg scope "$scope" \
    --arg head "$(git rev-parse HEAD)" \
    --arg base "${base:-explicit-diff}" \
    --argjson rust_changed "$rust_changed" \
    --argjson full_required "$full_required" \
    --argjson files "$(jq -Rsc 'split("\n") | map(select(length > 0))' "$changed_file_list")" \
    '{format:$format,scope:$scope,head:$head,base:$base,rust_changed:$rust_changed,full_required:$full_required,changed_files:$files}' \
    > "$summary"
echo "fast gate: OK scope=$scope files=${#changed_files[@]} evidence=$summary"
