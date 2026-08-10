#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_root="${TMPDIR:-/tmp}"
tmp_dir="$(mktemp -d "$tmp_root/tondo-stdlib-iter-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.iter owner tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq 'del(.owners[] | select(. == "std.iter"))' testing/stdlib-core.json \
    > "$tmp_dir/missing-owner.json"
expect_failure missing-owner env TONDO_STDLIB_CORE_CONTRACT="$tmp_dir/missing-owner.json" \
    scripts/stdlib-core-check.sh

jq 'del(.test_matrix[] | select(. == "composition"))' testing/stdlib-core.json \
    > "$tmp_dir/missing-composition.json"
expect_failure missing-composition env TONDO_STDLIB_CORE_CONTRACT="$tmp_dir/missing-composition.json" \
    scripts/stdlib-core-check.sh

jq '.owner = "std.iter"' testing/stdlib-core.json > "$tmp_dir/wrong-owner.json"
expect_failure wrong-owner env TONDO_STDLIB_CORE_CONTRACT="$tmp_dir/wrong-owner.json" \
    scripts/stdlib-core-check.sh

for signature in \
    'pub fn Iterator.map[T, U](self, fn(T): U): Iterator[U]' \
    'pub fn Iterator.filter[T](self, fn(T): Bool): Iterator[T]' \
    'pub fn Iterator.take[T](self, count: Int): Iterator[T]' \
    'pub fn Iterator.collect[T](self): Array[T] ! CollectionError'; do
    grep -Fq "$signature" docs/contracts/stdlib-core.md
done

for symbol in \
    'HirPreludeTraitMethod::IteratorNext' \
    'HirBootstrapHostFunction::IterMap' \
    'HirBootstrapHostFunction::IterFilter' \
    'HirBootstrapHostFunction::IterTake' \
    'HirBootstrapHostFunction::IterCollect'; do
    grep -Fq "$symbol" crates/tondo-compiler/src/hir/lower.rs
done

for symbol in \
    '"std.iter.map"' \
    '"std.iter.filter"' \
    '"std.iter.take"' \
    '"std.iter.collect"' \
    'IteratorAdapter::Map' \
    'IteratorAdapter::Filter' \
    'IteratorAdapter::Take' \
    'fn iterator_adapter_next' \
    'fn next_owned_iterator_value' \
    'BytecodeTerminatorKind::IteratorNext'; do
    grep -Fq "$symbol" crates/tondo-vm/src/runtime/execute.rs crates/tondo-compiler/src/bytecode/lower.rs
done

for marker in \
    'let mapped = [1, 2, 3, 4].map(plus_one)' \
    'let filtered = [1, 2, 3, 4].filter(is_even)' \
    'let closure_result = [2, 3].map(double)' \
    'let qualified = iter.map' \
    'let static_take = iter.take' \
    'let static_collect = iter.collect' \
    'let empty = [1, 2].take(-1)' \
    'let chained = (1 ..= 5).map(plus_one).filter(is_even).take(2)'; do
    grep -Fq "$marker" tests/runtime/m11-std-iter-001.to
done

for symbol in \
    'borrowed_intrinsic_iteration_executes_without_consuming_collection_elements' \
    'user_iterator_for_loops_lower_through_static_next_dispatch' \
    'iterator_exhaustion_guards_are_specialized_and_reverified' \
    'iterator_adapters_trace_source_and_callbacks' \
    'iterator_contract_helpers_reject_malformed_descriptors_and_states'; do
    grep -Fq "$symbol" crates/tondo-compiler/src/bytecode/lower.rs crates/tondo-vm/src/runtime/heap.rs crates/tondo-vm/src/runtime/execute.rs
done

jq -e '
  ([.rows[] | select(.owner == "std.iter")] | length) == 4
  and all(.rows[] | select(.owner == "std.iter"); .missing == [])
' testing/stdlib-public-api.json >/dev/null

echo "std.iter owner tests: OK"
