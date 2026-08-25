#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_DIAGNOSTIC_TEST_CONTRACT:-$root/testing/diagnostic-test.json}"

die() {
    echo "diagnostic test: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing runner contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with one LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-diagnostic-test/1"
  and .owner == "toolchain.diagnostics"
  and .edition == "0.1"
  and .phase == "DIAG-TEST-001"
  and .status == "implemented"
  and .contract == "docs/contracts/diagnostic-test.md"
  and .profiles == ["race", "leaks", "crash"]
  and .report == {
    format: "tondo-diagnostic-report/1",
    json_field: "diagnostics",
    junit_property: "tondo.diagnostics",
    identity: ["run_id", "attempt_id", "shard", "profile", "target", "backend", "toolchain", "source_revision"],
    statuses: ["clean", "finding", "unsupported", "failed"],
    limitations_required: true,
    artifacts_content_addressed: true,
    payloads_omitted_by_default: true
  }
  and .worker == {
    process_format: "tondo-test-worker-process/2",
    batch_format: "tondo-test-worker-batch/2",
    fresh_process_per_attempt: true,
    retry_state_isolated: true,
    repeat_state_isolated: true,
    shard_identity_preserved: true,
    suite_setup_teardown_included: true
  }
  and .limits == {
    max_profiles: 3,
    max_report_bytes: 16777216,
    max_dump_bytes: 268435456,
    fail_closed: true
  }
  and .exit_status == {
    success: 0,
    finding: 1,
    unsupported: 2,
    toolchain_failure: 3,
    precedence: ["toolchain_failure", "unsupported", "finding", "test_status"]
  }
  and ((.negative_cases | unique | length) == (.negative_cases | length))
  and (.negative_cases | length) == 15
  and .next_blocks == ["NATIVE-001"]
' "$contract" >/dev/null || die "invalid diagnostic test contract"

for path in \
    docs/contracts/diagnostic-test.md \
    crates/tondo-cli/src/main.rs \
    crates/tondo-cli/src/test_cli.rs \
    crates/tondo-compiler/src/driver.rs \
    crates/tondo-compiler/src/test_result.rs \
    crates/tondo-compiler/src/test_junit.rs; do
    [[ -f "$root/$path" ]] || die "missing runner evidence: $path"
done

grep -Fq 'fn diagnostic_reports_for' "$root/crates/tondo-cli/src/main.rs" \
    || die "diagnostic report projection is absent"
grep -Fq 'attach_worker_diagnostics' "$root/crates/tondo-cli/src/main.rs" \
    || die "worker diagnostics are not attached to attempts"
grep -Fq 'tondo.diagnostics' "$root/crates/tondo-compiler/src/test_junit.rs" \
    || die "JUnit diagnostic projection is absent"
grep -Fq 'DIAGNOSTIC_REPORT_FORMAT' "$root/crates/tondo-compiler/src/test_result.rs" \
    || die "diagnostic result schema is absent"
grep -Fq 'proceso nuevo' "$root/docs/contracts/diagnostic-test.md" \
    || die "worker isolation is absent from documentation"

echo "diagnostic test: OK (isolated workers, per-attempt reports, artifacts and JUnit/JSON projection)"
