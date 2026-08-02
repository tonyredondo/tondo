#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

reports="$root/target/reliability/quality"
coverage="$reports/coverage.json"
mutation_output="$reports/mutation"
mutation_report="$mutation_output/mutants.out/outcomes.json"
mutation_tmp="${TONDO_MUTATION_TMPDIR:-$reports/mutation-tmp}"
mkdir -p "$reports"
mkdir -p "$mutation_tmp"
mutation_tmp="$(cd "$mutation_tmp" && pwd)"

cargo llvm-cov \
    --workspace \
    --all-targets \
    --json \
    --output-path "$coverage"

TMPDIR="$mutation_tmp" cargo mutants \
    --workspace \
    --no-config \
    --copy-vcs true \
    --file 'crates/tondo-compiler/src/project.rs' \
    --file 'crates/tondo-conformance/src/document.rs' \
    --file 'crates/tondo-vm/src/bytecode.rs' \
    --file 'crates/tondo-vm/src/runtime/heap.rs' \
    --re '(ProjectPlan::parse|PrivilegedUnit::validate|validate_line_endings|normalize_array_index|Heap::has_capacity|Heap::ensure_capacity)' \
    --baseline run \
    --jobs 2 \
    --timeout 300 \
    --build-timeout 900 \
    --cargo-arg=--locked \
    --output "$mutation_output" \
    --no-times \
    --colors never \
    --annotations none

cargo run -p tondo-reliability --locked -- quality verify \
    --root . \
    --coverage "$coverage" \
    --mutants "$mutation_report"
