#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$BASH_SOURCE")/.." && pwd)"
cd "$root"

matrix="$(printenv TONDO_STDLIB_MATRIX || true)"
[[ -n "$matrix" ]] || matrix="testing/stdlib-matrix.json"
tmp_root="$(printenv TMPDIR || true)"
[[ -n "$tmp_root" ]] || tmp_root="/tmp"
tmp_dir="$(mktemp -d "$tmp_root/tondo-stdlib-matrix-check.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT
generated="$tmp_dir/matrix.json"

die() {
    echo "stdlib matrix: $*" >&2
    exit 1
}

[[ -f "$matrix" ]] || die "missing normative matrix: $matrix"
tail -c 1 "$matrix" | cmp -s <(printf '\n') || die "matrix must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$matrix" >/dev/null || die "matrix contains CR or trailing whitespace"

scripts/stdlib-matrix-generate.sh "$generated" >/dev/null
cmp -s "$generated" "$matrix" || die "matrix is stale; run scripts/stdlib-matrix-generate.sh"

jq -e \
    --slurpfile implementation testing/stdlib-implementation.json \
    --slurpfile integration testing/stdlib-spec.json \
    --slurpfile performance testing/stdlib-performance.json \
    --slurpfile api testing/stdlib-public-api.json \
    '
      . as $matrix
      | def expected_owners:
        (($integration[0].owner_contracts | map(.id)) + ["std.bytes"] | unique | sort);
      def stage_ids: ["SPEC", "IMPL/HOST", "MODEL/TEST/FUZZ", "PERF", "CONF", "DOC"];
      .format == "tondo-stdlib-normative-matrix/1"
      and .edition == "0.1"
      and .phase == "STD-0.1A"
      and .catalogs.current == "STD-0.1A"
      and .catalogs.future_closed == "STD-0.1B"
      and .rules.required_stages == stage_ids
      and .rules.one_owner_per_signature == true
      and .rules.one_owner_per_requirement == true
      and .rules.pending_requires_reason == true
      and .rules.not_applicable_requires_reason == true
      and .rules.stage_refs_are_explicit == true
      and .rules.dimensions_are_owner_scoped == true
      and ([.owners[].id] | sort) == expected_owners
      and ([.owners[].id] | unique | length) == (.owners | length)
      and ([.rows[].id] | unique | length) == (.rows | length)
      and .summary.owners == (.owners | length)
      and .summary.signatures == ([.rows[] | select(.kind == "signature")] | length)
      and .summary.requirements == ([.rows[] | select(.kind == "requirement")] | length)
      and .summary.rows == (.rows | length)
      and .summary.verified_rows + .summary.open_rows == .summary.rows
      and .status == (if .summary.open_rows > 0 then "open-gaps" else "verified" end)
      and all(.owners[];
        . as $owner
        | ($owner.layer | test("^A[0-4]$"))
        and ($owner.dimensions.required | length > 0)
        and ($owner.dimensions.required | unique | length) == ($owner.dimensions.required | length)
        and ($owner.dimensions.observed | unique | length) == ($owner.dimensions.observed | length)
        and ($owner.dimensions.pending | unique | length) == ($owner.dimensions.pending | length)
        and (($owner.stages | keys_unsorted) | sort) == (stage_ids | sort)
        and all(stage_ids[]; . as $id |
          ($owner.stages[$id].status | ["verified", "partial", "pending", "not-applicable", "gap"] | index(.) != null)
          and (if $owner.stages[$id].status == "verified" then ($owner.stages[$id].reason == null)
               else ($owner.stages[$id].reason | type == "string" and length > 0)
               end)
          and ($owner.stages[$id].refs | type == "array" and all(.[]; type == "string" and length > 0))
        )
      )
      and all(.rows[];
        . as $row
        | ($row.owner | startswith("std."))
        and (any($integration[0].owner_contracts[]; .id == $row.owner) or $row.owner == "std.bytes")
        and ($row.scope == "STD-0.1A")
        and ($row.dimensions_ref == ("owner:" + $row.owner))
        and (($row.stage_refs | keys_unsorted) | sort) == (stage_ids | sort)
        and all(stage_ids[]; . as $id | ($row.stage_refs[$id] | type == "string" and length > 0))
        and (if $row.kind == "signature" then
              ($row.signature | startswith("pub "))
              and ($row.symbol | startswith("std."))
              and ($row.source.audit | startswith("testing/stdlib-public-api.json#rows/"))
            elif $row.kind == "requirement" then
              ($row.requirement.id | type == "string" and length > 0)
              and ($row.source.contract | type == "string" and length > 0)
            else false end)
        and ($row.status | ["verified", "open-gaps"] | index(.)) != null
      )
      and ([.rows[] | select(.kind == "signature") | .source.audit | sub("^testing/stdlib-public-api.json#rows/"; "")] | sort)
          == ([$api[0].rows[].id] | sort)
      and all($matrix.owners[]; (.signature_rows | all(.[]; . as $id | any($matrix.rows[]; .id == $id))) and (.requirement_rows | all(.[]; . as $id | any($matrix.rows[]; .id == $id))))
    ' "$matrix" >/dev/null || die "invalid normative matrix structure"

while IFS= read -r ref; do
    base="$(printf '%s' "$ref" | sed 's/#.*//')"
    [[ -e "$root/$base" ]] || die "matrix references missing path: $ref"
done < <(jq -r '.owners[].stages[] | .refs[]' "$matrix")

summary="$(jq -r '"\(.summary.owners) owners; \(.summary.signatures) signatures; \(.summary.requirements) requirements"' "$matrix")"
echo "stdlib normative matrix: OK ($summary; all owner stages explicit)"
