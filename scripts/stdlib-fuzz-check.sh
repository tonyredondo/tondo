#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_FUZZ_CONTRACT:-testing/stdlib-fuzz.json}"
evidence="${TONDO_STDLIB_OWNER_EVIDENCE:-testing/stdlib-owner-evidence.json}"
target="fuzz/fuzz_targets/stdlib_owners.rs"

die() {
    echo "stdlib fuzz: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing contract: $contract"
[[ -f "$evidence" ]] || die "missing owner evidence: $evidence"
[[ -f "$target" ]] || die "missing target: $target"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-fuzz/1"
  and .edition == "0.1"
  and .phase == "STD-0.1A"
  and .status == "promoted"
  and .target == "stdlib_owners"
  and .limits.max_input_bytes == 65536
  and .limits.max_source_bytes == 8192
  and .limits.rss_limit_mb == 4096
  and .limits.timeout_seconds == 10
  and .campaigns.smoke.script == "scripts/fuzz-smoke.sh"
  and .campaigns.nightly.script == "scripts/fuzz-campaign.sh"
  and .oracle.type == "bounded-no-panic-and-owner-invariants"
  and .regressions.persist_minimized_inputs
  and (.owners | type == "array" and length == 22)
  and ([.owners[].id] | unique | length) == 22
  and ([.owners[].route] | unique | length) == 22
  and all(.owners[];
    (.id | test("^std\\.[a-z]+$"))
    and .id == .route
    and (.corpus | startswith("fuzz/corpus/stdlib_owners/"))
    and (.seed | type == "number" and . >= 0)
    and (.limits | type == "string" and length > 0)
    and (.oracle | type == "string" and length > 0)
  )
' "$contract" >/dev/null || die "invalid contract"

mapfile -t owners < <(jq -r '.owners[].id' "$contract")
mapfile -t routes < <(sed -n '/pub const OWNER_ROUTES/,/];/s/.*"\(std\.[a-z]*\)".*/\1/p' "$target")
[[ "${#routes[@]}" -eq 22 ]] || die "target must declare 22 owner routes"
for owner in "${owners[@]}"; do
    printf '%s\n' "${routes[@]}" | grep -Fxq "$owner" || die "target has no route for $owner"
    corpus="$(jq -r --arg owner "$owner" '.owners[] | select(.id == $owner) | .corpus' "$contract")"
    [[ -s "$corpus" ]] || die "owner corpus is missing or empty: $corpus"
    first_byte="$(od -An -tu1 -N1 "$corpus" | tr -d '[:space:]')"
    [[ -n "$first_byte" ]] || die "owner corpus has no route selector: $corpus"
    index=$((first_byte % 22))
    expected="${routes[$index]}"
    [[ "$expected" == "$owner" ]] || die "corpus selector for $owner resolves to $expected"
    jq -e --arg owner "$owner" --arg target "$target" --arg corpus "$corpus" '
      any(.owners[]; .id == $owner
        and .cells.FUZZ.status == "verified"
        and .cells.FUZZ.reason == null
        and any(.cells.FUZZ.refs[]; startswith($target))
        and any(.cells.FUZZ.refs[]; startswith($corpus))
      )
    ' "$evidence" >/dev/null || die "owner evidence is not promoted for $owner"
done

grep -Fq 'const MAX_INPUT_BYTES: usize = 64 * 1024;' "$target" || die "target input limit is not explicit"
grep -Fq 'catch_unwind(AssertUnwindSafe' "$target" || die "target no-panic oracle is not explicit"
grep -Fq 'persist_minimized_inputs' "$contract" || die "regression persistence is not declared"

echo "stdlib fuzz: OK (22 owner routes, bounded corpora, executable oracle, persistent regressions)"
