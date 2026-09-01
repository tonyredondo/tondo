#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_SYNC_COLLECTION_TEST_CONTRACT:-$root/testing/stdlib-sync-collection-test.json}"

die() {
    echo "std.sync collection tests: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing collection test contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-sync-collection-testing/1"
  and .owner == "std.sync.collection"
  and .parent_owner == "std.sync"
  and .edition == "0.1"
  and .phase == "STD-0.1B"
  and .task == "STD-SYNC-COLLECTION-TEST-001"
  and .status == "verified"
  and .contract == "docs/contracts/stdlib-sync-collection-test.md"
  and .implementation_contract == "testing/stdlib-sync-collection.json"
  and .iteration_contract == "testing/stdlib-sync-collection-iter.json"
  and .spec == "TONDO_STANDARD_LIBRARY_SPEC.md"
  and .language_spec == "TONDO_LANGUAGE_SPEC.md"
  and .parent_contract == "testing/stdlib-sync.json"
  and .layer == "B2"
  and .kind == "reliability-facing"
  and .target == "reference-model-and-hosted-native-regression-boundary"
  and .limits.max_collection_entries == 64
  and .limits.max_collection_handles == 128
  and .limits.max_fuzz_input_bytes == 4096
  and .limits.max_fuzz_steps == 512
  and .limits.max_history_operations == 12
  and .limits.model_seed_count == 4096
  and .limits.fuzz_smoke_runs == 128
  and .model.status == "verified"
  and (.model.sources | type == "array" and length == 2)
  and (.model.owners | sort) == ["Array", "Map", "Queue", "Set", "Stack"]
  and (.model.laws | type == "array" and length >= 14)
  and .model.sequence_seeds == 4096
  and .model.oracle == "independent bounded sequential state, exhaustive history search and cursor invariants"
  and .test.status == "verified"
  and (.test.sources | type == "array" and length >= 4)
  and (.test.commands | type == "array" and length == 5)
  and (.test.cases | type == "array" and length >= 10)
  and .test.oracle == "runtime regression suites and independent model observations agree on observable outcomes"
  and .fuzz.status == "verified"
  and .fuzz.target == "stdlib_sync_collections"
  and .fuzz.source == "fuzz/fuzz_targets/stdlib_sync_collections.rs"
  and .fuzz.corpus == "fuzz/corpus/stdlib_sync_collections/seed"
  and .fuzz.input_limit_bytes == 4096
  and .fuzz.step_limit == 512
  and .fuzz.smoke.runs == 128
  and .fuzz.smoke.seed == 4103
  and .fuzz.smoke.result == "passed"
  and .fuzz.oracle == "panic-free bounded replay, per-owner invariants, finite cursor teardown and zero live tokens"
  and .sanitization.status == "bounded-safe-rust-no-unsafe-boundary"
  and .sanitization.applicable == false
  and .sanitization.native_aot == "not-claimed"
  and .promotion.model_test_fuzz_complete == true
  and .promotion.implementation_pending == []
  and .promotion.next_blocks == ["STD-SYNC-COLLECTION-CONF-001"]
  and .promotion.remaining == [
    "STD-SYNC-COLLECTION-CONF-001",
    "STD-SYNC-CONF-001",
    "STD-SYNC-DOC-001"
  ]
' "$contract" >/dev/null || die "invalid machine-readable collection test contract"

for path in \
    docs/contracts/stdlib-sync-collection-test.md \
    TONDO_STANDARD_LIBRARY_SPEC.md \
    TONDO_LANGUAGE_SPEC.md \
    TONDO_IMPLEMENTATION_TRACKER.md \
    testing/stdlib-sync.json \
    testing/stdlib-sync-collection.json \
    testing/stdlib-sync-collection-iter.json \
    testing/inventory.json \
    testing/coverage-matrix.json; do
    [[ -f "$root/$path" ]] || die "missing linked path: $path"
done

while IFS= read -r path; do
    [[ -f "$root/$path" ]] || die "missing model/test source: $path"
done < <(jq -r '.model.sources[], .test.sources[], .fuzz.source, .fuzz.corpus' "$contract")

for path in \
    scripts/stdlib-sync-collection-test-check.sh \
    scripts/stdlib-sync-collection-test-test.sh \
    scripts/stdlib-sync-collection-fuzz.sh; do
    [[ -x "$root/$path" ]] || die "runner is not executable: $path"
done

grep -Fq 'name = "stdlib_sync_collections"' "$root/fuzz/Cargo.toml" \
    || die "fuzz manifest misses stdlib_sync_collections"
for marker in \
    'MAX_COLLECTION_ENTRIES' \
    'MAX_COLLECTION_HANDLES' \
    'MAX_COLLECTION_FUZZ_STEPS' \
    'MAX_LINEARIZABILITY_OPS' \
    'pub enum CollectionKind' \
    'pub enum CollectionAction' \
    'pub struct CursorModel' \
    'pub struct SharedCollectionModel' \
    'pub struct HistoryOperation' \
    'pub fn is_linearizable' \
    'pub fn run_collection_fuzz_case'; do
    grep -Fq "$marker" "$root/crates/tondo-reliability/src/sync_collection_model.rs" \
        || die "model misses anchor: $marker"
done
for marker in \
    'std.sync collection model panicked' \
    'std.sync collection model replay diverged' \
    'stdlib_sync_collections'; do
    grep -Fq "$marker" \
        "$root/fuzz/fuzz_targets/stdlib_sync_collections.rs" \
        "$root/scripts/stdlib-sync-collection-fuzz.sh" \
        || die "fuzz lane misses anchor: $marker"
done

jq -e '
  .collections.test_contract == "testing/stdlib-sync-collection-test.json"
  and .collections.test_document == "docs/contracts/stdlib-sync-collection-test.md"
  and .collections.runtime_status == "verified-hosted-vm-and-native-runtime-abi"
  and .promotion.next_blocks == ["STD-SYNC-COLLECTION-CONF-001"]
  and (.promotion.implementation_pending | index("STD-SYNC-COLLECTION-TEST-001")) == null
' "$root/testing/stdlib-sync.json" >/dev/null \
    || die "parent std.sync registry does not expose the promoted test boundary"

jq -e '
  .testing_contract == "testing/stdlib-sync-collection-test.json"
  and .testing_document == "docs/contracts/stdlib-sync-collection-test.md"
  and .promotion.next_blocks == ["STD-SYNC-COLLECTION-CONF-001"]
  and (.promotion.remaining | index("STD-SYNC-COLLECTION-TEST-001")) == null
' "$root/testing/stdlib-sync-collection.json" >/dev/null \
    || die "collection implementation registry does not link its test follow-up"

jq -e '
  .testing_contract == "testing/stdlib-sync-collection-test.json"
  and .testing_document == "docs/contracts/stdlib-sync-collection-test.md"
  and .promotion.next_blocks == ["STD-SYNC-COLLECTION-CONF-001"]
' "$root/testing/stdlib-sync-collection-iter.json" >/dev/null \
    || die "collection iteration registry does not link its test follow-up"

grep -Fq 'stdlib-sync-collection-test.json' "$root/TONDO_STANDARD_LIBRARY_SPEC.md" \
    || die "stdlib spec does not link the collection test contract"
grep -Fq 'stdlib-sync-collection-test.md' "$root/docs/contracts/stdlib-sync.md" \
    || die "parent sync document does not link the collection test contract"
grep -Fq 'STD-SYNC-COLLECTION-TEST-001' "$root/TONDO_IMPLEMENTATION_TRACKER.md" \
    || die "tracker does not record the collection test leaf"

echo "std.sync collection tests: OK (sequential model; linearizability histories; cursor and ownership fuzz boundary)"
