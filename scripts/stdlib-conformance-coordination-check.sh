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
  and .status == "closed-coordination"
  and .promotion.status == "not-promoted"
  and .promotion.next_coordination == "STD-DOC-001"
  and .promotion.matrix_status == "open-gaps"
  and .rules.one_owner_per_matrix_row
  and .rules.every_matrix_row_has_conf_record
  and .rules.pending_requires_reason
  and .rules.partial_requires_reason
  and .rules.refs_are_explicit
  and .rules.verified_requires_observation
  and .rules.coordination_does_not_promote
  and (.owners | type == "array" and length == 22)
  and ([.owners[].id] | unique | length) == 22
  and (.summary == {
    owners: 22,
    rows: 372,
    public_signatures: 207,
    requirements: 165,
    verified_rows: 0,
    partial_rows: 371,
    pending_rows: 1,
    owner_verified: 0,
    owner_partial: 21,
    owner_pending: 1
  })
  and all(.owners[];
    ((.id | type) == "string" and (.id | test("^std\\.[a-z]+$")))
    and (.rows | type == "array" and length > 0)
    and (.public_signatures | type == "array")
    and (.requirements | type == "array")
    and (.status | IN("verified", "partial", "pending"))
    and (.reason | type == "string" and length > 0)
    and (.evidence.status == .status)
    and (.evidence.refs | type == "array" and length > 0)
    and (.evidence.commands | type == "array" and length > 0)
    and (.evidence.cases | type == "array")
    and (.evidence.scope | type == "string" and length > 0)
    and all(.rows[];
      ((.id | type) == "string" and (.id | (startswith("signature:") or startswith("requirement:"))))
      and (.kind | IN("signature", "requirement"))
      and (.status | IN("verified", "partial", "pending", "gap"))
      and (if .status == "verified" then .reason == null else (.reason | type == "string" and length > 0) end)
      and (.refs | type == "array" and length > 0)
    )
  )
  and ([.owners[].rows[].id] | unique | length) == 372
  and ([.owners[].rows[].id] | sort) == ([.owners[].rows[].id] | unique | sort)
  and all(.owners[]; (.status != "verified" or all(.rows[]; .status == "verified")))
' "$coordination" >/dev/null || {
    echo "stdlib conformance coordination: invalid registry" >&2
    exit 1
}

jq -n -e --slurpfile coordination "$coordination" --slurpfile matrix testing/stdlib-matrix.json --slurpfile api testing/stdlib-public-api.json '
  ($coordination[0]) as $coord
  | ($matrix[0]) as $matrix
  | ($api[0]) as $api
  | ([ $coord.owners[].id ] | sort) == ([ $matrix.owners[].id ] | sort)
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

echo "stdlib conformance coordination: OK (22 owners; 372 rows; 207 signatures; 165 requirements; gaps explicit; promotion withheld)"
