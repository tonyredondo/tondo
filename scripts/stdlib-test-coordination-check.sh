#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

coordination="${TONDO_STDLIB_TEST_COORDINATION:-testing/stdlib-test-coordination.json}"
evidence="${TONDO_STDLIB_OWNER_EVIDENCE:-testing/stdlib-owner-evidence.json}"
[[ -f "$coordination" ]] || {
    echo "stdlib test coordination: missing registry: ${coordination#"$root"/}" >&2
    exit 1
}

generated="$(mktemp "${TMPDIR:-/tmp}/tondo-stdlib-test-coordination.XXXXXX.json")"
trap 'rm -f "$generated"' EXIT
TONDO_STDLIB_OWNER_EVIDENCE="$evidence" scripts/stdlib-owner-evidence-check.sh >/dev/null
scripts/stdlib-test-coordination-generate.sh "$generated" >/dev/null
cmp -s "$generated" "$coordination" || {
    echo "stdlib test coordination: registry is stale; run scripts/stdlib-test-coordination-generate.sh" >&2
    exit 1
}

jq -e '
  .format == "tondo-stdlib-test-coordination/1"
  and .edition == "0.1"
  and .phase == "STD-0.1A"
  and .status == "open-coordination"
  and .next_coordination == "STD-A-FUZZ-001"
  and .rules.one_owner_per_surface
  and .rules.every_surface_has_model_law
  and .rules.every_owner_has_test_commands
  and .rules.fuzz_gaps_require_reason
  and .rules.partial_fuzz_is_not_promotion
  and .rules.model_laws_do_not_promote_public_implementation
  and (.owners | type == "array" and length == 22)
  and ([.owners[].id] | unique | length) == 22
  and (.summary.owners == 22)
  and (.summary.public_signatures == 298)
  and (.summary.owner_requirements == 171)
  and (.summary.model_laws == 66)
  and (.summary.fuzz_verified == 0)
  and (.summary.fuzz_partial == 22)
  and (.summary.fuzz_component_verified == 9)
  and all(.owners[];
    (.id | type == "string" and test("^std\\.[a-z]+$"))
    and (.leaf | type == "string" and startswith("STD-A-"))
    and (.contract | type == "string" and endswith(".json"))
    and (.public_api | type == "array")
    and (.requirements | type == "array")
    and ((.public_api | length) > 0 or (.requirements | length) > 0)
    and (.model.status == "verified")
    and (.model.laws | type == "array" and length >= 3 and all(.[]; type == "string" and length > 0))
    and (.model.refs | type == "array" and length > 0)
    and (.test.status | IN("verified", "partial"))
    and (if .test.status == "verified" then .test.reason == null
         else (.test.reason | type == "string" and length > 0) end)
    and (.test.commands | type == "array" and length > 0)
    and (.test.refs | type == "array" and length > 0)
    and (.fuzz.status == "partial")
    and (.fuzz.campaigns | type == "array" and length > 0)
    and (.fuzz.refs | type == "array" and length > 0)
    and (.fuzz.reason | type == "string" and length > 0)
  )
  and ([.owners[].public_api[].id] | unique | length) == 298
  and ([.owners[].public_api[].id] | unique | sort) == ([.owners[].public_api[].id] | sort)
' "$coordination" >/dev/null || {
    echo "stdlib test coordination: invalid registry" >&2
    exit 1
}

jq -n -e --slurpfile coordination "$coordination" --slurpfile evidence "$evidence" --slurpfile api testing/stdlib-public-api.json --slurpfile matrix testing/stdlib-matrix.json '
  ($coordination[0]) as $coord
  | ($evidence[0]) as $evidence
  | ($api[0]) as $api
  | ($matrix[0]) as $matrix
  | ([ $evidence.owners[].id ] | sort) == ([ $coord.owners[].id ] | sort)
  and ([ $coord.owners[].public_api[] | .id ] | sort) == ([ $api.rows[].id ] | sort)
  and all($coord.owners[];
    . as $owner
    | ($owner.id) as $owner_id
    | ($owner.public_api | map(.id) | sort) == ([ $api.rows[] | select(.owner == $owner_id) | .id ] | sort)
    and all($owner.public_api[]; . as $surface | any($api.rows[]; .id == $surface.id and .status == $surface.implementation_status and .missing == $surface.missing))
    and ($owner.requirements | sort) == ([ $matrix.rows[] | select(.owner == $owner_id and .kind == "requirement") | .id ] | sort)
    and (first($evidence.owners[] | select(.id == $owner_id)) as $origin
      | $owner.test == {
          status: $origin.cells.TEST.status, reason: $origin.cells.TEST.reason,
          commands: $origin.commands, refs: $origin.cells.TEST.refs
        })
    and (first($evidence.owners[] | select(.id == $owner_id)).cells.FUZZ as $fuzz
      | $owner.fuzz.status == $fuzz.status and $owner.fuzz.reason == $fuzz.reason
      and $owner.fuzz.evidence_kind == $fuzz.evidence_kind
      and $owner.fuzz.component_status == $fuzz.component_status
      and $owner.fuzz.refs == $fuzz.refs)
  )
' >/dev/null || {
    echo "stdlib test coordination: registry does not match owner evidence, API, or matrix" >&2
    exit 1
}

while IFS= read -r ref; do
    base="${ref%%#*}"
    [[ -e "$root/$base" ]] || {
        echo "stdlib test coordination: missing reference: $ref" >&2
        exit 1
    }
done < <(jq -r '.owners[] | (.model.refs[]), (.test.refs[]), (.fuzz.refs[])' "$coordination")

while IFS= read -r command; do
    if [[ "$command" == scripts/* ]]; then
        [[ -x "$root/$command" ]] || {
            echo "stdlib test coordination: command is not executable: $command" >&2
            exit 1
        }
    fi
done < <(jq -r '.owners[].test.commands[]' "$coordination")

echo "stdlib test coordination: OK (22 owners; 298 signatures; 171 requirements; 66 declared model laws; 9 bounded fuzz components; promotion open)"
