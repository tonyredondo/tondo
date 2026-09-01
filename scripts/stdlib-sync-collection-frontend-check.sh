#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_SYNC_COLLECTION_FRONTEND_CONTRACT:-$root/testing/stdlib-sync-collection-frontend.json}"

die() {
    echo "std.sync collection frontend: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-sync-collection-frontend/1"
  and .edition == "0.1"
  and .phase == "STD-0.1B"
  and .owner == "std.sync"
  and .parent_task == "STD-SYNC-001"
  and .task == "STD-SYNC-COLLECTION-FRONTEND-001"
  and .status == "verified"
  and .contract == "docs/contracts/stdlib-sync-collection-frontend.md"
  and .spec == "TONDO_STANDARD_LIBRARY_SPEC.md"
  and .language_spec == "TONDO_LANGUAGE_SPEC.md"
  and .implementation.status == "verified-frontend-lowering-consumed"
  and .implementation.public_api_promoted == false
  and .implementation.syntax == "lossless-BracketPostfix-without-new-keywords"
  and .implementation.resolution == "external-nominal-identity"
  and .implementation.formatter == "canonical-lossless-round-trip"
  and .implementation.hir_marker == "std.sync.collectionLiteral"
  and .implementation.mir_boundary == "verified-hosted-runtime-boundary"
  and .implementation.runtime == "verified-by-STD-SYNC-COLLECTION-IMPL-001"
  and (.implementation.sources | type == "array" and length == 7)
  and (.implementation.tests | type == "array" and length == 5)
  and .surface.identities == [
    "std.sync.Array[T]",
    "std.sync.Map[K, V]",
    "std.sync.Set[T]",
    "std.sync.Stack[T]",
    "std.sync.Queue[T]"
  ]
  and .surface.literal_forms == [
    "sync.Array[...]",
    "sync.Map[...]",
    "sync.Set[...]",
    "sync.Stack[...]",
    "sync.Queue[...]"
  ]
  and .surface.qualified_only == true
  and .surface.aliases_resolve_by_identity == true
  and .surface.user_paths_opt_in == false
  and .surface.global_aliases == ["SArray", "SMap", "SSet"]
  and .surface.global_aliases_status == "forbidden"
  and .surface.type_position == "generic-application"
  and .surface.expression_position == "qualified-literal"
  and .surface.runtime_type_heuristics == false
  and .surface.cst_shape == "PathExpr-plus-BracketPostfix"
  and .surface.no_new_keywords == true
  and .surface.trailing_comma == true
  and .surface.evaluation_order == "left-to-right"
  and .surface.empty.array_set_stack_queue == "requires-expected-sync-nominal-type"
  and .surface.empty.map == "sync.Map[:]"
  and .surface.map.entry == "key:value"
  and .surface.map.single_entry == "sync.Map[key:value,]"
  and .surface.map.multiple_entries == "sync.Map[key:value, key:value,]"
  and .surface.map.malformed == "E1102"
  and .surface.diagnostics.missing_context == "E1101"
  and .surface.diagnostics.invalid_shape == "E1102"
  and .surface.diagnostics.duplicate_map_key == "E1116"
  and .surface.diagnostics.duplicate_set_value == "W1011"
  and .surface.duplicates == "ordinary-constant-collection-rules"
  and .surface.recovery == "typed-error-expression-without-partial-lowering"
  and .promotion.frontend_complete == true
  and .promotion.runtime_complete == true
  and .promotion.implementation_block == "STD-SYNC-COLLECTION-IMPL-001"
  and .promotion.next_blocks == ["STD-SYNC-COLLECTION-PERF-001"]
  and ((.negative_cases | unique | length) == (.negative_cases | length))
  and (.negative_cases | length) == 13
' "$contract" >/dev/null || die "invalid machine-readable frontend contract"

for path in \
    docs/contracts/stdlib-sync-collection-frontend.md \
    TONDO_STANDARD_LIBRARY_SPEC.md \
    TONDO_LANGUAGE_SPEC.md \
    TONDO_IMPLEMENTATION_TRACKER.md \
    testing/stdlib-sync.json; do
    [[ -f "$root/$path" ]] || die "missing linked path: $path"
done

while IFS= read -r path; do
    [[ -f "$root/$path" ]] || die "missing implementation source: $path"
done < <(jq -r '.implementation.sources[]' "$contract")

while IFS= read -r test; do
    file="${test%%::*}"
    name="${test##*::}"
    [[ -f "$root/$file" ]] || die "missing test source: $file"
    grep -Fq "$name" "$root/$file" || die "missing test anchor: $test"
done < <(jq -r '.implementation.tests[]' "$contract")

for marker in \
    '# Contrato de frontend de colecciones compartidas' \
    'STD-SYNC-COLLECTION-FRONTEND-001' \
    'PathExpr + BracketPostfix' \
    'std.sync.collectionLiteral' \
    'sync.Map[:]' \
    'E1101' \
    'E1102' \
    'E1116' \
    'W1011' \
    'STD-SYNC-COLLECTION-IMPL-001'; do
    grep -Fq "$marker" "$root/docs/contracts/stdlib-sync-collection-frontend.md" \
        || die "frontend document misses marker: $marker"
done

grep -Fq 'testing/stdlib-sync-collection-frontend.json' "$root/TONDO_STANDARD_LIBRARY_SPEC.md" \
    || die "main stdlib spec does not link the frontend contract"
grep -Fq 'STD-SYNC-COLLECTION-FRONTEND-001' "$root/TONDO_LANGUAGE_SPEC.md" \
    || die "language spec does not record the frontend implementation boundary"
grep -Fq 'STD-SYNC-COLLECTION-FRONTEND-001' "$root/TONDO_IMPLEMENTATION_TRACKER.md" \
    || die "tracker does not record the frontend block"

jq -e '
  .frontend.task == "STD-SYNC-COLLECTION-FRONTEND-001"
  and .frontend.status == "verified"
  and .frontend.contract == "testing/stdlib-sync-collection-frontend.json"
  and .frontend.runtime_lowering == "verified-hosted-runtime-boundary"
  and .frontend.implementation_contract == "testing/stdlib-sync-collection.json"
  and .promotion.next_blocks == ["STD-SYNC-COLLECTION-PERF-001"]
  and (.promotion.implementation_pending | index("STD-SYNC-COLLECTION-FRONTEND-001")) == null
' "$root/testing/stdlib-sync.json" >/dev/null \
    || die "parent std.sync registry does not promote the frontend"

echo "std.sync collection frontend: OK (qualified literals; nominal identity; HIR boundary)"
