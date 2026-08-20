#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_root="${TMPDIR:-/tmp}"
tmp_dir="$(mktemp -d "$tmp_root/tondo-stdlib-format-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.format owner tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq 'del(.owners[] | select(. == "std.format"))' testing/stdlib-core.json \
    > "$tmp_dir/missing-owner.json"
expect_failure missing-owner env TONDO_STDLIB_CORE_CONTRACT="$tmp_dir/missing-owner.json" \
    scripts/stdlib-core-check.sh

jq 'del(.test_matrix[] | select(. == "limits"))' testing/stdlib-core.json \
    > "$tmp_dir/missing-limits.json"
expect_failure missing-limits env TONDO_STDLIB_CORE_CONTRACT="$tmp_dir/missing-limits.json" \
    scripts/stdlib-core-check.sh

jq '.owner = "std.format"' testing/stdlib-core.json > "$tmp_dir/wrong-owner.json"
expect_failure wrong-owner env TONDO_STDLIB_CORE_CONTRACT="$tmp_dir/wrong-owner.json" \
    scripts/stdlib-core-check.sh

for signature in \
    'pub fn Builder.new(): Builder' \
    'pub fn Builder.append(var self, value: String): Unit ! FormatError' \
    'pub fn Builder.finish(var self): String ! FormatError' \
    'pub fn format[T: Display](value: T): String ! FormatError' \
    'pub fn join[T: Display](values: Array[T], separator: String): String ! FormatError'; do
    grep -Fq "$signature" docs/contracts/stdlib-core.md
done

for symbol in \
    'IntrinsicType::FormatBuilder' \
    'IntrinsicType::FormatError' \
    'HirBootstrapHostFunction::FormatBuilder' \
    'HirBootstrapHostFunction::FormatBuilderAppend' \
    'HirBootstrapHostFunction::FormatBuilderFinish' \
    'fn verify_format_operation'; do
    grep -Fq "$symbol" crates/tondo-compiler/src/hir/check.rs crates/tondo-compiler/src/hir/lower.rs \
        crates/tondo-compiler/src/mir/verify.rs
done

for symbol in \
    'BytecodeIntrinsicType::FormatBuilder' \
    'BytecodeIntrinsicType::FormatError' \
    'format_operations_lower_to_verified_bytecode_with_static_and_custom_display' \
    'format_operations_are_explicitly_verified_and_reject_corruption'; do
    grep -Fq "$symbol" crates/tondo-compiler/src/bytecode/lower.rs \
        crates/tondo-compiler/src/mir/verify.rs crates/tondo-vm/src/bytecode.rs
done

for symbol in \
    '"std.format.Builder.new"' \
    '"std.format.Builder.append"' \
    '"std.format.Builder.finish"' \
    'format_display_value_with_type' \
    'intrinsic_display' \
    'format_builder_host_boundaries_are_materialized_atomically'; do
    grep -Fq "$symbol" crates/tondo-compiler/src/process_host.rs crates/tondo-vm/src/runtime/execute.rs
done

for symbol in \
    'builder_and_display_are_bounded' \
    'join_preserves_order_and_separator_limits' \
    'limits_are_exact_and_rejected_appends_do_not_mutate_state' \
    'display_errors_propagate_without_exposing_partial_output' \
    'join_is_deterministic_at_every_materialization_boundary'; do
    grep -Fq "$symbol" crates/tondo-stdlib/src/format.rs
done

for marker in \
    'let rendered = format.format(label)?' \
    'let numeric = format.format(42)?' \
    'let joined = format.join([1, 2, 3], ",")?' \
    'var builder = format.Builder.new()' \
    'builder.append("ton")?' \
    'builder.append("do")?' \
    'let built = builder.finish()?'; do
    grep -Fq "$marker" tests/runtime/m11-std-format-001.to
done

grep -Fq 'no reflection' docs/contracts/stdlib-core.md
grep -Fq 'allocation' docs/contracts/stdlib-performance.md
grep -Fq 'std.format' testing/stdlib-performance-conformance.json

jq -e '
  ([.rows[] | select(.owner == "std.format")] | length) == 5
  and all(.rows[] | select(.owner == "std.format"); .missing == [])
  and all(.rows[] | select(.owner == "std.format"); .status == "verified")
' testing/stdlib-public-api.json >/dev/null

jq -e '
  any(.owners[]; .id == "std.format"
    and .cells.HOST.status == "not-applicable"
    and (.cells.HOST.reason | contains("intrinsic"))
    and .cells.MODEL.status == "verified"
    and .cells.TEST.status == "verified"
    and .cells.FUZZ.status == "verified"
    and .cells.PERF.status == "partial"
    and (.cells.PERF.reason | contains("owner-specific"))
    and .cells.CONF.status == "partial"
    and .cells.DOC.status == "verified")
' testing/stdlib-owner-evidence.json >/dev/null

echo "std.format owner tests: OK"
