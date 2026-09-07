#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_FUZZ_CONTRACT:-testing/stdlib-fuzz.json}"
evidence="${TONDO_STDLIB_OWNER_EVIDENCE:-testing/stdlib-owner-evidence.json}"
target="fuzz/fuzz_targets/stdlib_owners.rs"
contract_only=false
case "${1:-}" in
    --contract-only) contract_only=true ;;
    "") ;;
    *) echo "usage: $0 [--contract-only]" >&2; exit 1 ;;
esac

die() {
    echo "stdlib fuzz: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing contract: $contract"
if ! "$contract_only"; then
    [[ -f "$evidence" ]] || die "missing owner evidence: $evidence"
fi
[[ -f "$target" ]] || die "missing target: $target"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-fuzz/1"
  and .edition == "0.1"
  and .phase == "STD-0.1A"
  and .status == "partial"
  and .promotion == "pending"
  and .scope == "mixed component probes; no whole-owner or native promotion"
  and .target == "stdlib_owners"
  and .limits.max_input_bytes == 65536
  and .limits.max_source_bytes == 8192
  and .limits.rss_limit_mb == 4096
  and .limits.timeout_seconds == 10
  and .campaigns.smoke.script == "scripts/fuzz-smoke.sh"
  and .campaigns.nightly.script == "scripts/fuzz-campaign.sh"
  and .oracle.type == "bounded-component-probes"
  and ([.audited_sources[].path] | sort) == ["crates/tondo-reliability/src/codec_fuzz.rs", "crates/tondo-reliability/src/scalar_fuzz.rs", "fuzz/fuzz_targets/stdlib_owners.rs"]
  and all(.audited_sources[]; .sha256 | test("^[0-9a-f]{64}$"))
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
    and .status == "partial"
    and (.reason | type == "string" and length > 0)
    and (.evidence_kind | IN("constant-admission", "rust-reference-only", "kernel-smoke", "kernel-invariants", "bounded-model"))
    and .component_status == (if .evidence_kind == "kernel-invariants" then "verified" else "partial" end)
  )
' "$contract" >/dev/null || die "invalid contract"

while IFS=$'\t' read -r path expected; do
    actual="$(sha256sum "$path" | cut -d' ' -f1)"
    [[ "$actual" == "$expected" ]] || die "audited source changed; review the route scope: $path"
done < <(jq -r '.audited_sources[] | [.path, .sha256] | @tsv' "$contract")

if ! "$contract_only"; then
    jq -n -e --slurpfile contract "$contract" --slurpfile evidence "$evidence" '
      ([$contract[0].owners[].id] | sort) == ([$evidence[0].owners[].id] | sort)
      and $evidence[0].status == "component-evidence"
      and $evidence[0].promotion == "pending"
      and all($contract[0].owners[]; . as $route
        | any($evidence[0].owners[]; .id == $route.id
          and .cells.FUZZ.status == $route.status
          and .cells.FUZZ.reason == $route.reason
          and .cells.FUZZ.evidence_kind == $route.evidence_kind
          and .cells.FUZZ.component_status == $route.component_status))
    ' >/dev/null || die "owner evidence does not match the audited route scope"
fi

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
    if ! "$contract_only"; then
      jq -e --arg owner "$owner" --arg target "$target" --arg corpus "$corpus" '
      any(.owners[]; .id == $owner
        and any(.cells.FUZZ.refs[]; startswith($target))
        and any(.cells.FUZZ.refs[]; startswith($corpus))
      )
      ' "$evidence" >/dev/null || die "owner evidence is missing route references for $owner"
    fi
done

grep -Fq 'const MAX_INPUT_BYTES: usize = 64 * 1024;' "$target" || die "target input limit is not explicit"
grep -Fq 'catch_unwind(AssertUnwindSafe' "$target" || die "target no-panic oracle is not explicit"
grep -Fq 'persist_minimized_inputs' "$contract" || die "regression persistence is not declared"

echo "stdlib fuzz: OK (22 bounded component routes; whole-owner promotion pending)"
