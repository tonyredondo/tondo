#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

coordination="${TONDO_STDLIB_TEST_COORDINATION:-testing/stdlib-test-coordination.json}"
[[ -f "$coordination" ]] || {
    echo "stdlib test coordination: missing registry: ${coordination#"$root"/}" >&2
    exit 1
}

generated="$(mktemp "${TMPDIR:-/tmp}/tondo-stdlib-test-coordination.XXXXXX.json")"
trap 'rm -f "$generated"' EXIT
scripts/stdlib-test-coordination-generate.sh "$generated" >/dev/null
cmp -s "$generated" "$coordination" || {
    echo "stdlib test coordination: registry is stale; run scripts/stdlib-test-coordination-generate.sh" >&2
    exit 1
}

jq -e '
  .format == "tondo-stdlib-test-coordination/1"
  and .edition == "0.1"
  and .phase == "STD-0.1A"
  and .status == "closed-coordination"
  and .next_coordination == "STD-CONF-001"
  and .rules.one_owner_per_surface
  and .rules.every_surface_has_model_law
  and .rules.every_owner_has_test_commands
  and .rules.fuzz_gaps_require_reason
  and .rules.partial_fuzz_is_not_promotion
  and (.owners | type == "array" and length == 22)
  and ([.owners[].id] | unique | length) == 22
  and (.summary.owners == 22)
  and (.summary.public_signatures == 214)
  and (.summary.owner_requirements == 171)
  and (.summary.model_laws == 66)
  and (.summary.fuzz_verified == 1)
  and (.summary.fuzz_partial == 21)
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
    and (.test.status == "verified")
    and (.test.commands | type == "array" and length > 0)
    and (.test.refs | type == "array" and length > 0)
    and (.fuzz.status | ["verified", "partial"] | index(.) != null)
    and (.fuzz.campaigns | type == "array" and length > 0)
    and (.fuzz.refs | type == "array" and length > 0)
    and (if .fuzz.status == "partial" then (.fuzz.reason | type == "string" and length > 0) else .fuzz.reason == null end)
  )
  and ([.owners[].public_api[].id] | unique | length) == 214
  and ([.owners[].public_api[].id] | unique | sort) == ([.owners[].public_api[].id] | sort)
' "$coordination" >/dev/null || {
    echo "stdlib test coordination: invalid registry" >&2
    exit 1
}

jq -n -e --slurpfile coordination "$coordination" --slurpfile evidence testing/stdlib-owner-evidence.json --slurpfile api testing/stdlib-public-api.json --slurpfile matrix testing/stdlib-matrix.json '
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
    and ($owner.requirements | sort) == ([ $matrix.rows[] | select(.owner == $owner_id and .kind == "requirement") | .id ] | sort)
    and ($owner.fuzz.status == (first($evidence.owners[] | select(.id == $owner_id)).cells.FUZZ.status))
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

echo "stdlib test coordination: OK (22 owners; 214 public signatures; 171 owner requirements; 66 model laws; fuzz gaps explicit)"
