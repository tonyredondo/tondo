#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_root="${TMPDIR:-/tmp}"
tmp_dir="$(mktemp -d "$tmp_root/tondo-stdlib-math-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.math owner tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq 'del(.owners[] | select(. == "std.math"))' testing/stdlib-core.json \
    > "$tmp_dir/missing-owner.json"
expect_failure missing-owner env TONDO_STDLIB_CORE_CONTRACT="$tmp_dir/missing-owner.json" \
    scripts/stdlib-core-check.sh

jq 'del(.test_matrix[] | select(. == "error-priority"))' testing/stdlib-core.json \
    > "$tmp_dir/missing-error-priority.json"
expect_failure missing-error-priority env TONDO_STDLIB_CORE_CONTRACT="$tmp_dir/missing-error-priority.json" \
    scripts/stdlib-core-check.sh

jq '.owner = "std.math"' testing/stdlib-core.json > "$tmp_dir/wrong-owner.json"
expect_failure wrong-owner env TONDO_STDLIB_CORE_CONTRACT="$tmp_dir/wrong-owner.json" \
    scripts/stdlib-core-check.sh

for signature in \
    'pub fn floor(value: Float): Float' \
    'pub fn ceil(value: Float): Float' \
    'pub fn round(value: Float): Float' \
    'pub fn truncate(value: Float): Float' \
    'pub fn sqrt(value: Float): Float ! MathError' \
    'pub fn fma(a: Float, b: Float, c: Float): Float' \
    'pub fn abs(value: Float): Float' \
    'pub fn min(a: Float, b: Float): Float' \
    'pub fn max(a: Float, b: Float): Float'; do
    grep -Fq "$signature" docs/contracts/stdlib-core.md
done

for symbol in \
    'HirBootstrapHostFunction::MathFloor' \
    'HirBootstrapHostFunction::MathCeil' \
    'HirBootstrapHostFunction::MathRound' \
    'HirBootstrapHostFunction::MathTruncate' \
    'HirBootstrapHostFunction::MathSqrt' \
    'HirBootstrapHostFunction::MathFma' \
    'HirBootstrapHostFunction::MathAbs' \
    'HirBootstrapHostFunction::MathMin' \
    'HirBootstrapHostFunction::MathMax'; do
    grep -Fq "$symbol" crates/tondo-compiler/src/hir/check.rs
    grep -Fq "$symbol" crates/tondo-compiler/src/hir/lower.rs
done

for symbol in \
    '"std.math.floor"' \
    '"std.math.ceil"' \
    '"std.math.round"' \
    '"std.math.truncate"' \
    '"std.math.sqrt"' \
    '"std.math.fma"' \
    '"std.math.abs"' \
    '"std.math.min"' \
    '"std.math.max"'; do
    grep -Fq "$symbol" crates/tondo-compiler/src/process_host.rs
done

grep -Fq 'RuntimeHostValueKind::MathError' crates/tondo-compiler/src/process_host.rs
grep -Fq 'BytecodeIntrinsicType::MathError' crates/tondo-vm/src/runtime/execute.rs
grep -Fq 'fn math_result_error' crates/tondo-compiler/src/process_host.rs
grep -Fq 'sqrt_distinguishes_domain_and_nonfinite_inputs' crates/tondo-stdlib/src/math.rs
grep -Fq 'scalar_kernels_cover_signed_zero_ties_and_nonfinite_boundaries' crates/tondo-stdlib/src/math.rs
grep -Fq 'float32_rounds_each_operation_and_preserves_ieee_special_values' \
    crates/tondo-vm/src/runtime/execute.rs
grep -Fq 'compile_time_collection_duplicates_and_nan_comparisons_are_diagnosed' \
    crates/tondo-compiler/src/hir/check.rs
grep -Fq 'numeric_context_handles_signed_minimum_unions_and_shift_rhs_types' \
    crates/tondo-compiler/src/hir/check.rs

grep -Fq 'math.floor(2.9)' tests/runtime/m11-std-math-001.to
grep -Fq 'math.sqrt(-1.0)' tests/runtime/m11-std-math-001.to
grep -Fq 'NamedSubnormal' tests/runtime/m6-num-004-ieee.to
grep -Fq 'NamedInfinity' tests/runtime/m6-num-004-ieee.to
grep -Fq 'NamedNaN' tests/runtime/m6-num-004-ieee.to
grep -Fq 'let overflow' tests/runtime/m6-num-004-ieee.to

grep -Fq 'scalar oracle' docs/contracts/stdlib-core.md
grep -Fq 'SIMD' docs/contracts/stdlib-core.md
if rg -n 'std::simd|packed_simd|target_feature.*(avx|sse|neon)|fast[-_ ]math' \
    crates/tondo-stdlib/src/math.rs crates/tondo-compiler/src/process_host.rs >/dev/null; then
    echo "std.math owner tests: unexpected alternate SIMD/fast-math path" >&2
    exit 1
fi

jq -e '
  ([.rows[] | select(.owner == "std.math")] | length) == 9
  and all(.rows[] | select(.owner == "std.math"); .missing == [])
  and all(.rows[] | select(.owner == "std.math"); .status == "verified")
' testing/stdlib-public-api.json >/dev/null

echo "std.math owner tests: OK"
