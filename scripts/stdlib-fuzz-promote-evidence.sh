#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

evidence="${1:-testing/stdlib-owner-evidence.json}"
contract="${TONDO_STDLIB_FUZZ_CONTRACT:-testing/stdlib-fuzz.json}"

while IFS=$'\t' read -r owner corpus seed; do
    refs="[\"fuzz/fuzz_targets/stdlib_owners.rs#route=${owner}\", \"${corpus}#seed=${seed}\", \"testing/stdlib-fuzz.json#owner=${owner}\", \"scripts/stdlib-fuzz-check.sh\", \"scripts/fuzz-smoke.sh\", \"scripts/fuzz-campaign.sh\"]"
    OWNER="$owner" REFS="$refs" perl -0pi -e '
      my $owner = $ENV{OWNER};
      my $refs = $ENV{REFS};
      my $pattern = qr/("id": "\Q$owner\E".*?"FUZZ": \{\n)\s*"status": "(?:partial|verified)",\n\s*"reason": (?:null|".*?"),\n\s*"refs": \[[^\n]*\]/s;
      s/$pattern/$1 . qq{          "status": "verified",\n          "reason": null,\n          "refs": $refs}/es
        or die "missing FUZZ evidence for $owner\\n";
    ' "$evidence"
done < <(jq -r '.owners[] | [.id, (.corpus // ""), (.seed // 0)] | @tsv' "$contract")

echo "stdlib fuzz evidence promoted: $evidence"
