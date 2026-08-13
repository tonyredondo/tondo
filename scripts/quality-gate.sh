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
mutation_tmp="${TONDO_MUTATION_TMPDIR:-$reports/mutation-tmp}"
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
# minutes under a mutated build; keep the mutation gate strict without
# classifying a valid caught mutant as a timeout.
TMPDIR="$mutation_tmp" env -u CARGO_TARGET_DIR cargo mutants \
    --workspace \
    --no-config \
    --copy-vcs true \
    --copy-target true \
    --file 'crates/tondo-compiler/src/project.rs' \
    --file 'crates/tondo-conformance/src/document.rs' \
    --file 'crates/tondo-vm/src/bytecode.rs' \
    --file 'crates/tondo-vm/src/runtime/heap.rs' \
    --re '(ProjectPlan::parse|PrivilegedUnit::validate|validate_line_endings|normalize_array_index|Heap::has_capacity|Heap::ensure_capacity)' \
    --baseline run \
    --cargo-test-arg=--lib \
    --jobs 2 \
    --timeout 600 \
    --build-timeout 900 \
    --cargo-arg=--locked \
    --output "$mutation_output" \
    --no-times \
    --colors never \
    --annotations none

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
