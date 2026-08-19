#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$BASH_SOURCE")/.." && pwd)"
cd "$root"

documentation="${TONDO_STDLIB_DOCUMENTATION:-testing/stdlib-documentation.json}"
[[ -f "$documentation" ]] || {
    echo "stdlib documentation: missing registry: ${documentation#"$root"/}" >&2
    exit 1
}

generated="$(mktemp "${TMPDIR:-/tmp}/tondo-stdlib-documentation.XXXXXX.json")"
trap 'rm -f "$generated"' EXIT
scripts/stdlib-documentation-generate.sh "$generated" >/dev/null
cmp -s "$generated" "$documentation" || {
    echo "stdlib documentation: registry is stale; run scripts/stdlib-documentation-generate.sh" >&2
    exit 1
}

jq -e '
  .format == "tondo-stdlib-documentation/1"
  and .edition == "0.1"
  and .phase == "STD-0.1A"
  and .status == "closed-coordination"
  and .promotion.status == "not-published"
  and (.promotion.reason | contains("does not promote"))
  and .rules.one_owner_per_record
  and .rules.every_owner_has_contract
  and .rules.every_owner_has_example
  and .rules.runtime_examples_require_sidecar
  and .rules.non_runtime_examples_require_reason
  and .rules.boundary_statuses_are_explicit
  and .rules.api_gaps_are_not_promoted
  and .rules.unpublished_claim_is_required
  and (.owners | type == "array" and length == 22)
  and ([.owners[].id] | unique | length) == 22
  and (.summary == {
    owners: 22,
    examples: 32,
    runtime_examples: 26,
    external_examples: 4,
    compiler_examples: 2,
    api_complete: 18,
    api_partial: 1,
    api_not_applicable: 3
  })
  and all(.owners[];
    . as $owner
    | ((.id | type) == "string" and (.id | test("^std\\.[a-z]+$")))
    and .status == "documented-draft"
    and (.contract | type == "string" and startswith("docs/contracts/"))
    and (.docs | type == "array" and length > 0)
    and (.runtime_applicable | type == "boolean")
    and (if .runtime_applicable then .runtime_reason == null else (.runtime_reason | type == "string" and length > 0) end)
    and (.boundary.kernel.status | IN("verified", "partial", "pending", "gap"))
    and (.boundary.kernel.refs | type == "array" and length > 0)
    and (.boundary.bridge.status | IN("verified", "partial", "pending", "not-applicable"))
    and (.boundary.bridge.refs | type == "array" and length > 0)
    and (.boundary.public_api.status | IN("complete", "partial", "not-applicable"))
    and (.boundary.public_api.signatures | type == "array")
    and (.boundary.public_api.verified_signatures | type == "array")
    and all($owner.boundary.public_api.verified_signatures[]; . as $id | any($owner.boundary.public_api.signatures[]; . == $id))
    and (if .boundary.public_api.status == "complete" then (.boundary.public_api.signatures | length) > 0 and (.boundary.public_api.verified_signatures | length) == (.boundary.public_api.signatures | length)
         elif .boundary.public_api.status == "partial" then (.boundary.public_api.reason | type == "string" and length > 0)
         else (.boundary.public_api.reason | type == "string" and length > 0)
         end)
    and (.examples | type == "array" and length > 0)
    and (if .runtime_applicable then any(.examples[]; .kind == "runtime" or .kind == "acceptance") else (all(.examples[]; .kind != "runtime")) end)
    and all($owner.examples[];
      (.id | type == "string" and length > 0)
      and (.kind | IN("runtime", "acceptance", "external", "compiler"))
      and (.source | type == "string" and length > 0)
      and (.command | type == "string" and length > 0)
      and .status == "verified"
      and (.verification | type == "string" and length > 0)
    )
    and (.documentation_claim | contains("unpublished draft") and contains("not a release"))
  )
' "$documentation" >/dev/null || {
    echo "stdlib documentation: invalid registry" >&2
    exit 1
}

jq -n -e --slurpfile documentation "$documentation" --slurpfile matrix testing/stdlib-matrix.json --slurpfile conformance testing/stdlib-conformance-coordination.json --slurpfile evidence testing/stdlib-owner-evidence.json --slurpfile api testing/stdlib-public-api.json '
  ($documentation[0]) as $docs
  | ($matrix[0]) as $matrix
  | ($conformance[0]) as $conformance
  | ($evidence[0]) as $evidence
  | ($api[0]) as $api
  | ([ $docs.owners[].id ] | sort) == ([ $matrix.owners[].id ] | sort)
  and ([ $docs.owners[].id ] | sort) == ([ $conformance.owners[].id ] | sort)
  and all($docs.owners[];
    . as $owner
    | ([ $api.rows[] | select(.owner == $owner.id) | .id ] | sort) == ($owner.boundary.public_api.signatures | sort)
    and ([ $api.rows[] | select(.owner == $owner.id and ((.missing // []) | length == 0)) | .id ] | sort) == ($owner.boundary.public_api.verified_signatures | sort)
    and (first($evidence.owners[] | select(.id == $owner.id)) // null) as $evidence_owner
    | (if $evidence_owner == null then ($owner.contract | startswith("docs/contracts/")) else any($evidence_owner.cells.DOC.refs[]; . == $owner.contract) end)
    and any($conformance.owners[]; .id == $owner.id and .status == $owner.conformance.status)
  )
' >/dev/null || {
    echo "stdlib documentation: registry does not match matrix, conformance, API, or owner evidence" >&2
    exit 1
}

while IFS= read -r ref; do
    base="${ref%%#*}"
    [[ -e "$root/$base" ]] || {
        echo "stdlib documentation: missing reference: $ref" >&2
        exit 1
    }
done < <(jq -r '.sources[]?, .owners[].contract, .owners[].docs[]?, .owners[].boundary.kernel.refs[]?, .owners[].boundary.bridge.refs[]?, .owners[].boundary.public_api.refs[]?, .owners[].conformance.refs[]?' "$documentation")

while IFS= read -r command; do
    if [[ "$command" == scripts/* ]]; then
        [[ -x "$root/$command" ]] || {
            echo "stdlib documentation: command is not executable: $command" >&2
            exit 1
        }
    fi
done < <(jq -r '.owners[].examples[].command' "$documentation")

while IFS= read -r example; do
    kind="$(jq -r '.kind' <<< "$example")"
    source="$(jq -r '.source' <<< "$example")"
    [[ -f "$root/$source" ]] || {
        echo "stdlib documentation: missing example source: $source" >&2
        exit 1
    }
    if [[ "$kind" == "runtime" ]]; then
        base="${source%.to}"
        [[ -f "$root/$base.exit" ]] || {
            echo "stdlib documentation: runtime example lacks exit sidecar: $source" >&2
            exit 1
        }
        [[ -f "$root/$base.stdout" || -f "$root/$base.codes" ]] || {
            echo "stdlib documentation: runtime example lacks stdout/codes sidecar: $source" >&2
            exit 1
        }
    fi
done < <(jq -c '.owners[].examples[]' "$documentation")

echo "stdlib documentation: OK (22 owners; 32 examples; boundaries and unpublished claim explicit)"
