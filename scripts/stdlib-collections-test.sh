#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_root="${TMPDIR:-/tmp}"
tmp_dir="$(mktemp -d "$tmp_root/tondo-stdlib-collections-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.collections owner tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq 'del(.owners[] | select(. == "std.collections"))' testing/stdlib-core.json \
    > "$tmp_dir/missing-owner.json"
expect_failure missing-owner env TONDO_STDLIB_CORE_CONTRACT="$tmp_dir/missing-owner.json" \
    scripts/stdlib-core-check.sh

jq 'del(.test_matrix[] | select(. == "ownership"))' testing/stdlib-core.json \
    > "$tmp_dir/missing-ownership.json"
expect_failure missing-ownership env TONDO_STDLIB_CORE_CONTRACT="$tmp_dir/missing-ownership.json" \
    scripts/stdlib-core-check.sh

jq '.owner = "std.collections"' testing/stdlib-core.json > "$tmp_dir/wrong-owner.json"
expect_failure wrong-owner env TONDO_STDLIB_CORE_CONTRACT="$tmp_dir/wrong-owner.json" \
    scripts/stdlib-core-check.sh

for signature in \
    'pub fn Array.new[T](): Array[T]' \
    'pub fn Array.withCapacity[T](capacity: Int): Array[T] ! CollectionError' \
    'pub fn Array.length[T](self): Int' \
    'pub fn Array.get[T](self, index: Int): T?' \
    'pub fn Array.slice[T](self, start: Int, end: Int): Array[T] ! CollectionError' \
    'pub fn Array.push[T](var self, value: T): Unit ! CollectionError' \
    'pub fn Array.pop[T](var self): T?' \
    'pub fn Map.new[K: Key, V](): Map[K, V]' \
    'pub fn Map.get[K: Key, V](self, key: K): V?' \
    'pub fn Map.insert[K: Key, V](var self, key: K, value: V): V?' \
    'pub fn Map.remove[K: Key, V](var self, key: K): V?' \
    'pub fn Map.contains[K: Key, V](self, key: K): Bool' \
    'pub fn Map.entries[K: Key, V](self): Iterator[(K, V)]' \
    'pub fn Set.new[K: Key](): Set[K]' \
    'pub fn Set.insert[K: Key](var self, value: K): Bool' \
    'pub fn Set.remove[K: Key](var self, value: K): Bool' \
    'pub fn Set.contains[K: Key](self, value: K): Bool' \
    'pub fn Set.values[K: Key](self): Iterator[K]'; do
    grep -Fq "$signature" docs/contracts/stdlib-core.md
done

for symbol in \
    'HirBootstrapHostFunction::CollectionArrayNew' \
    'HirBootstrapHostFunction::CollectionArrayWithCapacity' \
    'HirBootstrapHostFunction::CollectionArrayLength' \
    'HirBootstrapHostFunction::CollectionArrayGet' \
    'HirBootstrapHostFunction::CollectionArraySlice' \
    'HirBootstrapHostFunction::CollectionArrayPush' \
    'HirBootstrapHostFunction::CollectionArrayPop' \
    'HirBootstrapHostFunction::CollectionMapNew' \
    'HirBootstrapHostFunction::CollectionMapGet' \
    'HirBootstrapHostFunction::CollectionMapInsert' \
    'HirBootstrapHostFunction::CollectionMapRemove' \
    'HirBootstrapHostFunction::CollectionMapContains' \
    'HirBootstrapHostFunction::CollectionMapEntries' \
    'HirBootstrapHostFunction::CollectionSetNew' \
    'HirBootstrapHostFunction::CollectionSetInsert' \
    'HirBootstrapHostFunction::CollectionSetRemove' \
    'HirBootstrapHostFunction::CollectionSetContains' \
    'HirBootstrapHostFunction::CollectionSetValues'; do
    grep -Fq "$symbol" crates/tondo-compiler/src/hir/lower.rs
done

grep -Fq 'values.slice(0, 1)' tests/runtime/m11-std-collections-001.to
grep -Fq 'Array.withCapacity[Int](-1)' tests/runtime/m11-std-collections-001.to
grep -Fq 'map.insert("one", 10)' tests/runtime/m11-std-collections-001.to
grep -Fq 'for entry in map.entries()' tests/runtime/m11-std-collections-001.to
grep -Fq 'not set.insert(3)' tests/runtime/m11-std-collections-001.to
grep -Fq 'for value in set.values()' tests/runtime/m11-std-collections-001.to

grep -Fq 'collection_copy_profile_justifies_cow_with_reproducible_workloads' \
    crates/tondo-compiler/src/bytecode/lower.rs
grep -Fq 'eager_and_cow_match_the_same_value_copy_observable_corpus' \
    crates/tondo-compiler/src/bytecode/lower.rs
grep -Fq 'HirContainmentKind::MapKey' crates/tondo-compiler/src/bytecode/lower.rs
grep -Fq 'find_map_entry' crates/tondo-vm/src/runtime/execute.rs
grep -Fq 'BytecodeIntrinsicType::Map' fuzz/fuzz_targets/admission.rs
grep -Fq 'BytecodeIntrinsicType::Set' fuzz/fuzz_targets/admission.rs
grep -Fq 'BytecodeIntrinsicType::Array' fuzz/fuzz_targets/admission.rs
[[ -s fuzz/corpus/admission/scalar-seed ]]
[[ -s fuzz/corpus/admission/structural-seed ]]

jq -e '
  ([.rows[] | select(.owner == "std.collections")] | length) == 18
  and all(.rows[] | select(.owner == "std.collections"); .missing == [])
' testing/stdlib-public-api.json >/dev/null

echo "std.collections owner tests: OK"
