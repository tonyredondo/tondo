#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_UUID_CONTRACT:-$root/testing/stdlib-uuid.json}"
die() { echo "std.uuid implementation: $*" >&2; exit 1; }
TONDO_STDLIB_UUID_CONTRACT="$contract" bash scripts/stdlib-uuid-check.sh >/dev/null
while IFS= read -r path; do
    [[ -f "$path" ]] || die "missing kernel source: $path"
done < <(jq -r '.implementation.sources[]' "$contract")
while IFS= read -r anchor; do
    path="${anchor%%::*}"; name="${anchor##*::}"
    grep -Fq "fn $name(" "$path" || die "missing kernel test: $anchor"
done < <(jq -r '.implementation.tests[]' "$contract")
grep -Fxq 'sha1 = { version = "=0.10.6", features = ["force-soft"] }' Cargo.toml || die "dependency version/route changed"
grep -Fxq 'sha1.workspace = true' crates/tondo-stdlib/Cargo.toml || die "missing kernel dependency"
grep -Fxq 'pub mod uuid;' crates/tondo-stdlib/src/lib.rs || die "kernel not exported"
for marker in 'ready-stdlib-kernel' 'verified-stdlib-kernel' 'not-claimed-until-uuid-host' 'try_to_string' 'No provider calls' 'native ABI'; do
    grep -Fq "$marker" docs/contracts/stdlib-uuid.md || die "undocumented kernel boundary: $marker"
done
echo "std.uuid implementation: OK ($(jq -r '.implementation.status' "$contract"); explicit-input Rust kernel only)"
