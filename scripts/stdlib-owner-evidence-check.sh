#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

evidence="${TONDO_STDLIB_OWNER_EVIDENCE:-testing/stdlib-owner-evidence.json}"

die() {
    echo "stdlib owner evidence: $*" >&2
    exit 1
}

[[ -f "$evidence" ]] || die "missing evidence registry: $evidence"
tail -c 1 "$evidence" | cmp -s <(printf '\n') || die "evidence registry must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$evidence" >/dev/null || die "evidence registry contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-owner-evidence/1"
  and .edition == "0.1"
  and .phase == "STD-0.1A"
  and .status == "promoted-evidence"
  and (.leaves | type == "array" and length > 0)
  and ([.leaves[].id] | unique | length) == (.leaves | length)
  and all(.leaves[];
    (.id | type == "string" and startswith("STD-A-"))
    and (.contract | type == "string" and endswith(".json"))
    and (.owners | type == "array" and length > 0 and (unique | length) == length)
  )
  and ([.owners[].id] | unique | length) == (.owners | length)
  and all(.owners[];
    (.id | type == "string" and test("^std\\.[a-z]+$"))
    and (.layer | test("^A[0-4]$"))
    and ((.cells | keys_unsorted) | sort) == ["CONF", "DOC", "FUZZ", "HOST", "IMPL", "MODEL", "PERF", "SPEC", "TEST"]
    and all(.cells[];
      (.status | ["verified", "partial", "pending", "not-applicable", "gap"] | index(.)) != null
      and (.refs | type == "array" and length > 0 and all(.[]; type == "string" and length > 0))
      and (if .status == "verified" then .reason == null else (.reason | type == "string" and length > 0) end)
    )
    and (.cells.PERF.status | ["verified", "not-applicable"] | index(.) != null)
    and (if .cells.PERF.status == "verified" then .cells.PERF.reason == null else (.cells.PERF.reason | length > 0) end)
    and any(.cells.PERF.refs[]; contains("stdlib-performance"))
    and (if (.id | IN("std.meta", "std.reflect", "std.bytes", "std.core", "std.text", "std.collections", "std.iter", "std.math", "std.format", "std.io", "std.async", "std.path", "std.serialization", "std.json", "std.messagepack", "std.protobuf")) then
          .cells.HOST.status == "not-applicable"
          and (.cells.HOST.reason | type == "string" and length > 0)
        else true end)
    and (.budgets | type == "object" and length > 0)
    and all(.budgets[];
      (.unit | type == "string" and length > 0)
      and .direction == "lower-is-better"
      and (.max_regression_basis_points | type == "number" and . >= 0)
      and .state == "baseline-recorded"
    )
    and (.commands | type == "array" and length > 0 and all(.[]; type == "string" and length > 0))
  )
  and ([.leaves[].owners[]] | sort) == ([.owners[].id] | sort)
  and (any(.leaves[]; .id == "STD-A-META-EVIDENCE-001" and .owners == ["std.meta"]))
  and (any(.leaves[]; .id == "STD-A-REFLECT-EVIDENCE-001" and .owners == ["std.reflect"]))
  and (any(.leaves[]; .id == "STD-A-BYTES-EVIDENCE-001" and .owners == ["std.bytes"]))
    and (any(.leaves[]; .id == "STD-A-TESTING-EVIDENCE-001" and .owners == ["std.testing"]))
    and (all(.owners[]; .cells.FUZZ.status == "verified" and .cells.FUZZ.reason == null))
    and (all(.owners[];
      .cells.CONF.status == "verified"
      and .cells.CONF.reason == null
      and any(.cells.CONF.refs[]; startswith("testing/stdlib-conformance.json#owners/"))
    ))
' "$evidence" >/dev/null || die "invalid promoted evidence registry"

while IFS=$'\t' read -r leaf contract owner; do
    [[ -f "$root/$contract" ]] || die "missing owner contract: $contract"
    jq -e --arg owner "$owner" '
      .format == "tondo-stdlib-owner-contract/1"
      and (.owner == $owner or ((.owners // []) | (type == "array" and (index($owner) != null))))
      and (.test_matrix | type == "array" and length > 0)
    ' "$root/$contract" >/dev/null || die "owner contract does not match evidence leaf: $leaf"
done < <(jq -r '.leaves[] | . as $leaf | .owners[] | [$leaf.id, $leaf.contract, .] | @tsv' "$evidence")

while IFS= read -r ref; do
    base="${ref%%#*}"
    [[ -e "$root/$base" ]] || die "missing evidence reference: $ref"
done < <(jq -r '.owners[].cells[] | .refs[]' "$evidence")

while IFS= read -r command; do
    if [[ "$command" == scripts/* ]]; then
        [[ -x "$root/$command" ]] || die "evidence command is not executable: $command"
    fi
done < <(jq -r '.owners[].commands[]' "$evidence")

echo "stdlib owner evidence: OK (22 owners; CONF/PERF promoted or normative not-applicable; all FUZZ routes promoted)"
