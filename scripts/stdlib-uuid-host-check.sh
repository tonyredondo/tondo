#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_UUID_HOST_CONTRACT:-$root/testing/stdlib-uuid-host.json}"
parent="${TONDO_STDLIB_UUID_CONTRACT:-$root/testing/stdlib-uuid.json}"
die() { echo "std.uuid host: $*" >&2; exit 1; }
[[ -f "$contract" ]] || die "missing host contract"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains CR or trailing whitespace"
TONDO_STDLIB_UUID_CONTRACT="$parent" bash scripts/stdlib-uuid-check.sh >/dev/null
jq -e -L scripts --slurpfile parent "$parent" '
  include "stdlib_uuid_host";
  uuid_host_boundary
  and .status == $parent[0].host.status
  and .quality_gate == $parent[0].host.quality_gate
  and .selected_route == $parent[0].host.selected_route
  # This child retains the follow-ups recorded at its HOST promotion. The
  # parent progression validates their subsequently completed TEST/PERF leaves.
  and .required_follow_ups == (if $parent[0].measurement != null then
    ["STD-UUID-TEST-001", "STD-UUID-PERF-001", "STD-UUID-CONF-001", "STD-UUID-DOC-001"]
    else $parent[0].implementation.required_follow_ups end)
' "$contract" >/dev/null || die "invalid host boundary or parent progression"
while IFS= read -r path; do
    [[ -f "$path" ]] || die "missing host source/fixture: $path"
done < <(jq -r '.sources[], .fixtures[]' "$contract")
while IFS= read -r anchor; do
    path="${anchor%%::*}"; name="${anchor##*::}"
    grep -Fq "fn $name(" "$path" || die "missing host test: $anchor"
done < <(jq -r '.tests[]' "$contract")
grep -Fxq 'getrandom = "=0.4.3"' Cargo.toml || die "entropy dependency is not exactly pinned"
grep -Fxq 'getrandom.workspace = true' crates/tondo-compiler/Cargo.toml || die "host entropy dependency missing"
for marker in 'ready-production-hosted' 'verified-production-hosted' 'SystemTime::now' '182' 'E1008' 'native runtime ABI' 'once-only' 'RSS'; do
    grep -Fq "$marker" docs/contracts/stdlib-uuid-host.md || die "undocumented host boundary: $marker"
done
echo "std.uuid host: OK ($(jq -r '.status' "$contract"); hosted scalar; native ABI/AOT not claimed)"
