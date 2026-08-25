#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_DIAGNOSTIC_CONTRACT:-$root/testing/diagnostic-tooling.json}"

die() {
    echo "diagnostic contract: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with one LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-diagnostic-tooling/1"
  and .owner == "toolchain.diagnostics"
  and .edition == "0.1"
  and .phase == "DIAG-SPEC-001"
  and .status == "contract-locked"
  and .contract == "docs/contracts/diagnostic-tooling.md"
  and .rfc == "docs/rfc/019-diagnostic-tooling.md"
  and .compilation_diagnostics_contract == "docs/contracts/diagnostics-json.md"
  and .public_stdlib_api == false
  and .runtime_implementation == "implemented-vm-hosted"
  and .runtime_contract == "docs/contracts/diagnostic-runtime.md"
  and ([.profiles[].id] == ["race", "leaks", "crash"])
  and all(.profiles[]; (.finding_kind | type == "string" and length > 0)
      and (.event_sources | type == "array" and length > 0 and (unique | length == length))
      and (.required_context | type == "array" and length > 0 and (unique | length == length))
      and .unsupported_code == "unsupported-diagnostic-profile")
  and .all.expands_to == ["race", "leaks", "crash"]
  and .all.report_order == ["race", "leaks", "crash"]
  and .report.format == "tondo-diagnostic-report/1"
  and .report.observed_only == true
  and .report.limitations_required == true
  and .report.status_values == ["clean", "finding", "unsupported", "failed"]
  and .report.identity_fields == [
    "run_id", "attempt_id", "shard", "profile", "target", "backend",
    "toolchain", "source_revision"
  ]
  and (.report.required_fields | index("run_id") != null)
  and (.report.required_fields | index("attempt_id") != null)
  and (.report.required_fields | index("limitations") != null)
  and (.report.required_fields | index("program_exit_status") != null)
  and (.report.required_fields | index("command_exit_status") != null)
  and .report.artifact_hash == "sha256"
  and .report.ordering == "identity-then-observation-kind-then-source-span"
  and .report.no_payloads_by_default == true
  and .dump.format == "tondo-dump/1"
  and .dump.extension == ".tdump"
  and .dump.content_address == "sha256"
  and .dump.required_sections == [
    "header", "termination", "identity", "stacks", "heap_summary",
    "resource_ledger", "scheduler_tail", "redaction", "limitations"
  ]
  and .dump.user_payloads == "omitted-by-default"
  and .dump.analyzer.command == "tondo dump analyze <file.tdump>"
  and .dump.analyzer.formats == ["human", "json"]
  and .dump.analyzer.network == false
  and .dump.analyzer.executes_dump_code == false
  and .cli.diagnostics_option == "--diagnostics"
  and .cli.profile_separator == ","
  and .cli.commands == [
    "tondo run --diagnostics <race|leaks|crash|all>[,...] ...",
    "tondo test --diagnostics <race|leaks|crash|all>[,...] ...",
    "tondo dump analyze <file.tdump> [--format human|json]"
  ]
  and .cli.project_configuration == "forbidden"
  and .cli.environment_configuration == "forbidden"
  and .cli.compilation_diagnostics_unchanged == true
  and .cli.stdlib_api_added == false
  and .cli.source_keywords_added == false
  and .exit_status == {
    success: 0,
    finding: 1,
    unsupported_or_invalid_profile: 2,
    toolchain_failure: 3,
    panic_without_dynamic_finding: 101,
    precedence: ["toolchain_failure", "unsupported_or_invalid_profile", "finding", "program_exit_status"],
    program_exit_status_preserved_in_report: true
  }
  and .limits == {
    max_profiles_per_invocation: 3,
    max_report_bytes: 16777216,
    max_dump_bytes: 268435456,
    max_observations: 100000,
    max_events: 1000000,
    max_stack_depth: 256,
    max_retainers_per_object: 256,
    max_scheduler_tail_events: 4096,
    limits_are_fail_closed: true,
    truncation_is_reported: true
  }
  and .privacy == {
    payloads: "redacted-by-default",
    logical_paths_only: true,
    secrets: "never-emitted-by-default",
    network_upload: false,
    environment_reads: "allowlisted-toolchain-identity-only",
    redaction_and_truncation_are_reported: true
  }
  and .lifecycle.fresh_process_per_attempt == true
  and .lifecycle.retry_state_isolated == true
  and .lifecycle.shard_state_isolated == true
  and .lifecycle.suite_state_isolated == true
  and .lifecycle.quiescence_required_for_leaks == true
  and .lifecycle.signal_path_async_signal_safe_only == true
  and .boundaries == {
    compilation_diagnostics_schema_unchanged: true,
    no_parallel_stdlib_api: true,
    vm_first: true,
    native_parity_required_before_native_gate: true,
    clean_report_is_not_static_proof: true,
    unsupported_is_explicit: true
  }
  and ((.negative_cases | unique | length) == (.negative_cases | length))
  and (.negative_cases | length) == 18
  and .next_blocks == ["DIAG-TEST-001", "DIAG-CI-001"]
' "$contract" >/dev/null || die "invalid machine-readable contract"

for path in \
    docs/contracts/diagnostic-tooling.md \
    docs/contracts/diagnostic-runtime.md \
    docs/rfc/019-diagnostic-tooling.md \
    docs/contracts/diagnostics-json.md; do
    [[ -f "$root/$path" ]] || die "missing linked contract: $path"
done

grep -Fq 'tondo-diagnostic-report/1' "$root/docs/contracts/diagnostic-tooling.md" \
    || die "diagnostic report format is absent from the normative contract"
grep -Fq 'tondo-dump/1' "$root/docs/contracts/diagnostic-tooling.md" \
    || die "dump format is absent from the normative contract"
grep -Fq 'DEC-018' "$root/docs/rfc/019-diagnostic-tooling.md" \
    || die "RFC does not identify DEC-018"

echo "diagnostic contract: OK (three profiles; report/dump envelopes frozen; no stdlib API or keyword additions)"
