#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$BASH_SOURCE")/.." && pwd)"
cd "$root"

coordination="${TONDO_STDLIB_CONFORMANCE_COORDINATION:-testing/stdlib-conformance-coordination.json}"
[[ -f "$coordination" ]] || {
    echo "stdlib conformance coordination: missing registry: ${coordination#"$root"/}" >&2
    exit 1
}

generated="$(mktemp "${TMPDIR:-/tmp}/tondo-stdlib-conformance-coordination.XXXXXX.json")"
trap 'rm -f "$generated"' EXIT
scripts/stdlib-conformance-coordination-generate.sh "$generated" >/dev/null
cmp -s "$generated" "$coordination" || {
    echo "stdlib conformance coordination: registry is stale; run scripts/stdlib-conformance-coordination-generate.sh" >&2
    exit 1
}

jq -e '
  .format == "tondo-stdlib-conformance-coordination/1"
  and .edition == "0.1"
  and .phase == "STD-0.1A"
  and .status == "planned"
  and .promotion.status == "pending"
  and .promotion.next_coordination == "STD-S1A-SEAL-001"
  and (.promotion.reason | type == "string" and length > 0)
  and .rules.one_owner_per_matrix_row
  and .rules.every_matrix_row_has_conf_record
  and .rules.pending_requires_reason
  and .rules.partial_requires_reason
  and .rules.refs_are_explicit
  and .rules.verified_requires_observation
  and .rules.coordination_does_not_promote
  and .rules.execution_registry == "testing/stdlib-conformance.json"
  and (.owners | type == "array" and length == 22)
  and ([.owners[].id] | unique | length) == 22
  and (.summary == {
    owners: (.owners | length),
    rows: ([.owners[].rows[]] | length),
    public_signatures: ([.owners[].rows[] | select(.kind == "signature")] | length),
    requirements: ([.owners[].rows[] | select(.kind == "requirement")] | length),
    verified_rows: ([.owners[].rows[] | select(.status == "verified")] | length),
    partial_rows: ([.owners[].rows[] | select(.status == "partial")] | length),
    pending_rows: ([.owners[].rows[] | select(.status == "pending")] | length),
    owner_verified: ([.owners[] | select(.status == "verified")] | length),
    owner_partial: ([.owners[] | select(.status == "partial")] | length),
    owner_pending: ([.owners[] | select(.status == "pending")] | length)
  })
  and all(.owners[];
    ((.id | type) == "string" and (.id | test("^std\\.[a-z]+$")))
    and (.rows | type == "array" and length > 0)
    and (.public_signatures | type == "array")
    and (.requirements | type == "array")
    and (.status | IN("pending", "partial"))
    and (.reason | type == "string" and length > 0)
    and (.evidence.status == .status)
    and (.evidence.refs | type == "array" and length > 0)
    and (.evidence.commands | type == "array" and length > 0)
    and (.evidence.cases | type == "array" and length > 0)
    and (.evidence.scope | type == "string" and length > 0)
    and all(.rows[];
      ((.id | type) == "string" and (.id | (startswith("signature:") or startswith("requirement:"))))
      and (.kind | IN("signature", "requirement"))
      and (.status | IN("pending", "partial"))
      and (.reason | type == "string" and length > 0)
      and (.refs | type == "array" and length > 0)
    )
  )
  and ([.owners[].rows[].id] | unique | length) == .summary.rows
  and ([.owners[].rows[].id] | sort) == ([.owners[].rows[].id] | unique | sort)

' "$coordination" >/dev/null || {
    echo "stdlib conformance coordination: invalid registry" >&2
    exit 1
}

jq -n -e --slurpfile coordination "$coordination" --slurpfile matrix testing/stdlib-matrix.json --slurpfile api testing/stdlib-public-api.json '
  ($coordination[0]) as $coord
  | ($matrix[0]) as $matrix
  | ($api[0]) as $api
  | ($coord.promotion.matrix_status == $matrix.status)
  and ([ $coord.owners[].id ] | sort) == ([ $matrix.owners[].id ] | sort)
  and ([ $coord.owners[].rows[].id ] | sort) == ([ $matrix.rows[].id ] | sort)
  and all($coord.owners[];
    . as $owner
    | ([ $matrix.rows[] | select(.owner == $owner.id) ] | sort_by(.id)) as $expected
    | ([ $owner.rows[] ] | sort_by(.id)) as $actual
    | (first($matrix.owners[] | select(.id == $owner.id)).stages.CONF) as $conf
    | ($actual | map({id, kind, status, reason, refs})) == ($expected | map({id, kind} + {status: $conf.status, reason: $conf.reason, refs: $conf.refs}))
    and (.public_signatures | sort) == ([$api.rows[] | select(.owner == $owner.id) | .id] | sort)
    and (.requirements | sort) == ([$expected[] | select(.kind == "requirement") | .id] | sort)
  )
  and ([ $api.rows[].id ] | sort) == ([$coord.owners[].public_signatures[]] | sort)
' >/dev/null || {
    echo "stdlib conformance coordination: registry does not match matrix or API" >&2
    exit 1
}

while IFS= read -r ref; do
    base="${ref%%#*}"
    [[ -e "$root/$base" ]] || {
        echo "stdlib conformance coordination: missing reference: $ref" >&2
        exit 1
    }
done < <(jq -r '.sources[]?, .owners[].rows[].refs[]?, .owners[].evidence.refs[]?' "$coordination")

while IFS= read -r command; do
    if [[ "$command" == scripts/* ]]; then
        [[ -x "$root/$command" ]] || {
            echo "stdlib conformance coordination: command is not executable: $command" >&2
            exit 1
        }
    fi
done < <(jq -r '.owners[].evidence.commands[]' "$coordination")

echo "stdlib conformance coordination: consistent case plan; execution promotion remains pending"
