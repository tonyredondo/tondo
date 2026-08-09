#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

evidence="${TONDO_STDLIB_OWNER_EVIDENCE:-testing/stdlib-owner-evidence.json}"
contract="${TONDO_STDLIB_META_CONTRACT:-testing/stdlib-meta.json}"

die() {
    echo "stdlib owner evidence: $*" >&2
    exit 1
}

[[ -f "$evidence" ]] || die "missing evidence registry: $evidence"
[[ -f "$contract" ]] || die "missing owner contract: $contract"
tail -c 1 "$evidence" | cmp -s <(printf '\n') || die "evidence registry must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$evidence" >/dev/null || die "evidence registry contains CR or trailing whitespace"

jq -e --slurpfile contract "$contract" '
  .format == "tondo-stdlib-owner-evidence/1"
  and .edition == "0.1"
  and .phase == "STD-0.1A"
  and .status == "draft-evidence"
  and .leaf == "STD-A-META-EVIDENCE-001"
  and .contract == "testing/stdlib-meta.json"
  and .coverage_floor_basis_points == 9025
  and .historical_manifest_immutable == true
  and ([.owners[].id] | unique) == ["std.meta"]
  and all(.owners[];
    .layer == "A0"
    and ((.cells | keys_unsorted) | sort) == ["CONF", "DOC", "FUZZ", "HOST", "IMPL", "MODEL", "PERF", "SPEC", "TEST"]
    and all(.cells[];
      (.status | ["verified", "partial", "pending", "not-applicable", "gap"] | index(.)) != null
      and (.refs | type == "array" and length > 0 and all(.[]; type == "string" and length > 0))
      and (if .status == "verified" then .reason == null else (.reason | type == "string" and length > 0) end)
    )
    and .cells.HOST.status == "not-applicable"
    and (.cells.HOST.reason | contains("build-only"))
    and .cells.PERF.status == "partial"
    and (.cells.PERF.reason | contains("baseline"))
    and .cells.CONF.status == "partial"
    and .budgets.compile_time.unit == "milliseconds"
    and .budgets.compile_time.direction == "lower-is-better"
    and .budgets.compile_time.max_regression_basis_points == 1000
    and .budgets.generated_source_bytes.unit == "bytes"
    and .budgets.generated_source_bytes.max_regression_basis_points == 500
    and ([.commands[] | select(startswith("scripts/"))] | sort) == [
      "scripts/stdlib-meta-check.sh", "scripts/stdlib-owner-evidence-check.sh"
    ]
  )
  and ($contract[0].owner == "std.meta")
  and ($contract[0].test_matrix | length) == 6
' "$evidence" >/dev/null || die "invalid evidence registry"

while IFS= read -r ref; do
    base="${ref%%#*}"
    [[ -e "$root/$base" ]] || die "missing evidence reference: $ref"
done < <(jq -r '.owners[].cells[] | .refs[]' "$evidence")

echo "stdlib owner evidence: OK (std.meta; HOST not-applicable; nine cells explicit)"
