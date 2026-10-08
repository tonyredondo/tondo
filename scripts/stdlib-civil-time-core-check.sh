#!/usr/bin/env bash
set -euo pipefail
root="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_CIVIL_CORE_CONTRACT:-testing/stdlib-civil-time-core.json}"
die() { echo "civil core: $*" >&2; exit 1; }
jq -e --slurpfile parent testing/stdlib-civil-time.json '
  .format == "tondo-stdlib-civil-core/1"
  and .owner == "std.time.civil" and .edition == "0.1"
  and .task == "STD-CIVIL-TIME-CORE-001"
  and ((.status == "implementation-in-progress" and .quality_gate == "pending-80-percent-per-scope")
    or (.status == "verified-public-hosted-core" and .quality_gate == "verified-80-percent-per-scope"))
  and .parent == "testing/stdlib-civil-time.json"
  and .contract == "docs/contracts/stdlib-civil-time-core.md"
  and .selected_route == "hosted-scalar-value-kernel"
  and .capabilities == {required:[],import_effect:"none",provider_reads:false}
  and .surface.types == ["Date","Time","UtcDateTime","CivilError","MonthPolicy"]
  and .surface.private_value_fields == true
  and .surface.signatures == [$parent[0].surface.signatures[] | select(
    ((.id | startswith("date-")) or (.id | startswith("time-")) or (.id | startswith("utc-")))
    and .id != "utc-in-zone")]
  and (.surface.signatures | length) == 24
  and all(.surface.signatures[]; .effect == "pure")
  and .model == {generator:"scripts/civil_core_oracle.py",fixture:"crates/tondo-reliability/tests/fixtures/civil-core-oracle.json",
    rust_test:"crates/tondo-reliability/tests/civil_core_models.rs",cases:1272,case_limit:2048,clock_reads:false,provider_reads:false}
  and .public_project == "acceptance/projects/civil-core/tondo.toml"
  and .command == "scripts/stdlib-civil-time-core-check.sh"
  and .memory == {reply_descriptor_bound:8,reply_payload_bound_bytes:30,
    metric:"conservative-logical-construction-and-exact-transport-admission",os_allocations:"not-measured",rss:"not-measured"}
  and (.sources | keys) == ["crates/tondo-compiler/src/hir.rs","crates/tondo-compiler/src/hir/check.rs",
    "crates/tondo-compiler/src/hir/lower.rs","crates/tondo-compiler/src/process_host.rs",
    "crates/tondo-compiler/src/process_host/civil_time.rs","crates/tondo-compiler/src/resolve.rs","crates/tondo-stdlib/src/civil_time.rs"]
  and .promotion.native_abi == "not-implemented" and .promotion.native_aot == "not-claimed"
  and .promotion.zones == "not-implemented" and .promotion.civil_provider == "not-implemented"
  and .promotion.full_owner == false and .promotion.performance == "not-measured"
  and .promotion.full_civil_test_fuzz_conf_doc == "pending"
  and ((.status == "implementation-in-progress" and .promotion.compiler_api == "registered-public-hosted-pending-quality")
    or (.status == "verified-public-hosted-core" and .promotion.compiler_api == "verified-public-hosted-core"))
' "$contract" >/dev/null || die 'contract differs from the locked pure boundary'
while IFS=$'\t' read -r path hash; do
    test "sha256:$(sha256sum "$path" | cut -d ' ' -f 1)" = "$hash" || die "source differs: $path"
done < <(jq -r '.sources | to_entries[] | [.key,.value] | @tsv' "$contract")
python3 -B scripts/civil_core_oracle.py --check
if [[ "${1:-}" == --contract-only ]]; then
    echo 'civil core contract: OK (24 pure operations; source bindings and bounded oracle current)'
    exit 0
fi
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
cargo test -q -p tondo-stdlib --lib --locked civil_time::
cargo test -q -p tondo-compiler --lib --locked civil_core_
cargo test -q -p tondo-compiler --lib --locked process_host::civil_time::
cargo test -q -p tondo-reliability --test civil_core_models --locked
cargo build -q -p tondo-cli --locked
target_dir="${CARGO_TARGET_DIR:-target}"
[[ "$target_dir" = /* ]] || target_dir="$root/$target_dir"
timeout 60 "$target_dir/debug/tondo" check --project acceptance/projects/civil-core
timeout 60 "$target_dir/debug/tondo" run --project acceptance/projects/civil-core
echo 'civil core: OK (24 public hosted pure operations; 1272 independent oracle cases; no native Tondo promotion)'
