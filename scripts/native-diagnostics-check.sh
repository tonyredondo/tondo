#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_NATIVE_DIAGNOSTICS_CONTRACT:-$root/testing/native-diagnostics.json}"

die() {
    echo "native diagnostics: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract has CR or trailing whitespace"

jq -e '
  def unique_values: length == (unique | length);
  .format == "tondo-native-diagnostics/1"
  and .owner == "toolchain.diagnostics"
  and .edition == "0.1"
  and .phase == "DIAG-NATIVE-001"
  and .status == "closed"
  and .contract == "docs/contracts/native-diagnostics.md"
  and .runner == "scripts/native-diagnostics.sh"
  and .checker == "scripts/native-diagnostics-check.sh"
  and .adapter == "tools/native-evaluation/src/main.rs"
  and .report == "target/reliability/evidence/native-evaluation-runner.json"
  and .report_field == "native_diagnostics"
  and .oracle == "hosted-diagnostic-contract-fixtures"
  and .backends == ["cranelift", "llvm"]
  and .envelope.format == "tondo-diagnostic-report/1"
  and .envelope.status_values == ["clean", "finding", "captured", "unsupported"]
  and .envelope.identity_fields == ["format", "profile", "case", "mode", "status"]
  and (.envelope.logical_observations | length == 11 and unique_values)
  and (.envelope.privacy_fields | length == 4 and unique_values)
  and .envelope.physical_data == "forbidden"
  and .corpus.race == ["race-conflict", "race-clean"]
  and .corpus.leaks == ["leak-growth", "leak-clean", "arc-cycle-reclaimed"]
  and .corpus.crash == ["crash-dump", "crash-corruption-rejected", "crash-limit-enforced"]
  and .runtime.crate == "crates/tondo-native-runtime/src/lib.rs"
  and .runtime.probe == "tondo_rt_diag_probe"
  and .runtime.field_reader == "tondo_rt_diag_field"
  and .runtime.reset == "tondo_rt_diag_reset"
  and .runtime.profiles == {race: 0, leaks: 1, crash: 2}
  and .runtime.status_codes == {clean: 0, finding: 1, captured: 2, unsupported: 3}
  and (.invariants | length == 9 and unique_values)
  and (.negative_cases | length == 8 and unique_values)
  and .next_blocks == ["NATIVE-STD-HOSTED-001"]
' "$contract" >/dev/null || die "invalid machine-readable contract"

for path in \
    docs/contracts/native-diagnostics.md \
    tools/native-evaluation/src/main.rs \
    crates/tondo-native-runtime/src/lib.rs \
    scripts/native-diagnostics.sh; do
    [[ -f "$root/$path" ]] || die "missing evidence: $path"
done
[[ -x "$root/scripts/native-diagnostics.sh" ]] || die "native diagnostics runner is not executable"

runtime="$root/crates/tondo-native-runtime/src/lib.rs"
adapter="$root/tools/native-evaluation/src/main.rs"
for symbol in tondo_rt_diag_reset tondo_rt_diag_probe tondo_rt_diag_field; do
    grep -Fq "$symbol" "$runtime" || die "runtime symbol is missing: $symbol"
done
for symbol in native_diagnostics run_native_diagnostics_probe NativeDiagnosticEnvelope \
    tondo_rt_diag_race tondo_rt_diag_leak tondo_rt_diag_dump; do
    grep -Fq "$symbol" "$adapter" || die "adapter diagnostic symbol is missing: $symbol"
done
grep -Fq 'DIAG-NATIVE-001' "$adapter" || die "adapter has no phase identity"

report="${TONDO_NATIVE_DIAGNOSTICS_REPORT:-}"
if [[ -n "$report" ]]; then
    [[ -f "$report" ]] || die "report does not exist: $report"
    jq -e --arg root "$root" '
      .native_diagnostics.format == "tondo-native-diagnostics/1"
      and .native_diagnostics.phase == "DIAG-NATIVE-001"
      and .native_diagnostics.status == "passed"
      and .native_diagnostics.oracle == "hosted-diagnostic-contract-fixtures"
      and .native_diagnostics.backends == ["cranelift", "llvm"]
      and ([.native_diagnostics.cases[].case] == [
        "race-conflict", "race-clean", "leak-growth", "leak-clean",
        "arc-cycle-reclaimed", "crash-dump", "crash-corruption-rejected",
        "crash-limit-enforced"
      ])
      and all(.native_diagnostics.cases[];
        .cranelift == "passed"
        and .llvm == "passed"
        and .envelope.format == "tondo-diagnostic-report/1"
        and (.envelope.status | IN("clean", "finding", "captured", "unsupported"))
        and .envelope.redacted == true
        and .envelope.payloads_omitted == true
      )
      and ((.native_diagnostics | tostring | contains($root)) | not)
    ' "$report" >/dev/null || die "native report does not prove diagnostic parity"
fi

echo "native diagnostics: OK (contract, private ABI, executable backends and optional parity report)"
