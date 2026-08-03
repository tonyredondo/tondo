#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

manifest="testing/stdlib-implementation.json"
[[ -f "$manifest" ]] || { echo "missing stdlib implementation evidence" >&2; exit 1; }
tail -c 1 "$manifest" | cmp -s <(printf '\n') || {
    echo "stdlib implementation evidence must end with LF" >&2
    exit 1
}
! grep -nE $'\r|[[:blank:]]$' "$manifest" >/dev/null || {
    echo "stdlib implementation evidence has whitespace" >&2
    exit 1
}

jq -e '
  .format == "tondo-stdlib-implementation-evidence/1"
  and .edition == "0.1"
  and .phase == "STD-0.1A"
  and .status == "implemented-draft"
  and .public_release == false
  and (.owners | length) == 20
  and ([.owners[].id] | unique | length) == 20
  and all(.owners[]; (.layer | test("^A[0-4]$")) and (.implementation | length > 0) and (.tests | length > 0) and (.proof | length > 0))
  and (.evidence_commands | length) == 4
  and .conformance_lineage == "conformance/draft/manifest.json"
  and .historical_manifest_immutable == true
  and .coverage_floor_basis_points == 9025
  and .release_gate == "STD-0.1 publication checklist remains separate and open"
' "$manifest" >/dev/null

while IFS= read -r path; do
    [[ -e "$path" ]] || { echo "missing implementation evidence path: $path" >&2; exit 1; }
done < <(
    jq -r '.owners[] | .implementation[], .tests[]' "$manifest"
)

echo "stdlib implementation evidence: OK"
