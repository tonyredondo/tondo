#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

# cargo-mutants repeatedly rewrites the same sources. Rust 1.93 can ICE while
# reusing their incremental query cache, so quality runs use clean compiler
# queries without changing incremental compilation for normal Tondo builds.
export CARGO_INCREMENTAL=0

cargo_target_dir="${CARGO_TARGET_DIR:-target}"
if [[ "$cargo_target_dir" = /* ]]; then
    reports="$cargo_target_dir/reliability/quality"
else
    reports="$root/$cargo_target_dir/reliability/quality"
fi
coverage="$reports/coverage.json"
coverage_before="$reports/coverage.before.json"
coverage_after="$reports/coverage.after.json"
coverage_binding="$reports/coverage.binding.json"
coverage_test_log="$reports/coverage-tests.log"
layer_evidence="$reports/layer-evidence.json"
mutation_output="${TONDO_MUTATION_OUTPUT:-$reports/mutation}"
mutation_report="$mutation_output/mutants.out/outcomes.json"
mutation_before="$reports/mutation.before.json"
mutation_after="$reports/mutation.after.json"
mutation_binding="$reports/mutation.binding.json"
# Keep the cargo-mutants temporary worktree outside CARGO_TARGET_DIR. The
# mutation run deliberately does not copy target artifacts, so this location
# contains only the isolated source tree and its fresh mutation builds.
mutation_tmp="${TONDO_MUTATION_TMPDIR:-$root/../tondo-mutation-tmp}"
mkdir -p "$reports"
mkdir -p "$mutation_tmp"
mutation_tmp="$(cd "$mutation_tmp" && pwd)"

cargo run -p tondo-reliability --locked -- quality provenance --root . > "$coverage_before"

cargo llvm-cov \
    --workspace \
    --all-targets \
    --json \
    --output-path "$coverage" \
    2>&1 | tee "$coverage_test_log"

cargo run -p tondo-reliability --locked -- layer-evidence attest \
    --root . \
    --test-log "$coverage_test_log" \
    --before "$coverage_before" \
    --output "$layer_evidence"

cargo run -p tondo-reliability --locked -- quality provenance --root . > "$coverage_after"
cargo run -p tondo-reliability --locked -- quality bind \
    --root . \
    --kind coverage \
    --report "$coverage" \
    --before "$coverage_before" \
    --after "$coverage_after" \
    --output "$coverage_binding"

# Fail before the expensive mutation campaign when coverage alone regresses.
cargo run -p tondo-reliability --locked -- quality verify \
    --root . \
    --coverage "$coverage" \
    --coverage-binding "$coverage_binding"

cargo run -p tondo-reliability --locked -- quality provenance --root . > "$mutation_before"

# The compiler test target is intentionally broad and can take several
# minutes under a mutated build. Do not copy the repository's target tree:
# it contains coverage and build artifacts that add tens of gigabytes
# without materially improving the clean mutation build. The quality gate
# runs one deterministic, one-per-frontier sample (six critical mutants),
# while the full 30-mutant campaign belongs to the performance lane. Use one
# mutation worker: this runner has four CPUs and 15 GiB of RAM, and several
# compiler builds in parallel page heavily enough to hit the build timeout.
TMPDIR="$mutation_tmp" env -u CARGO_TARGET_DIR cargo mutants \
    --workspace \
    --no-config \
    --copy-vcs true \
    --gitignore true \
    --error 'panic!("mutated")' \
    --file 'crates/tondo-compiler/src/project.rs' \
    --file 'crates/tondo-conformance/src/document.rs' \
    --file 'crates/tondo-vm/src/bytecode.rs' \
    --file 'crates/tondo-vm/src/runtime/heap.rs' \
    --re 'replace (PrivilegedUnit::validate -> Result<\(\), ProjectError> with Ok\(\(\)\)|ProjectPlan::parse -> Result<Self, ProjectError> with Err\(panic!\("mutated"\)\)|validate_line_endings -> Result<\(\), DocumentError> with Ok\(\(\)\)|normalize_array_index -> Option<usize> with None|Heap::ensure_capacity -> Result<\(\), VmError> with Ok\(\(\)\)|Heap::has_capacity -> bool with true)' \
    --baseline run \
    --cargo-test-arg=--lib \
    --jobs 1 \
    --timeout 600 \
    --build-timeout 900 \
    --cargo-arg=--locked \
    --output "$mutation_output" \
    --no-shuffle \
    --no-times \
    --colors never \
    --annotations none

scripts/mutation-infrastructure-check.sh "$mutation_output/mutants.out/log"

cargo run -p tondo-reliability --locked -- quality provenance --root . > "$mutation_after"
cargo run -p tondo-reliability --locked -- quality bind \
    --root . \
    --kind mutation \
    --report "$mutation_report" \
    --before "$mutation_before" \
    --after "$mutation_after" \
    --output "$mutation_binding"

cargo run -p tondo-reliability --locked -- quality verify \
    --root . \
    --coverage "$coverage" \
    --coverage-binding "$coverage_binding" \
    --mutants "$mutation_report" \
    --mutants-binding "$mutation_binding"

cargo run -p tondo-reliability --locked -- ratchet check \
    --root . \
    --coverage "$coverage" \
    --coverage-binding "$coverage_binding" \
    --mutants "$mutation_report" \
    --mutants-binding "$mutation_binding"
