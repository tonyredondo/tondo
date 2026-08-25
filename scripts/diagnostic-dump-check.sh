#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_DIAGNOSTIC_DUMP_CONTRACT:-$root/testing/diagnostic-dump.json}"

die() {
    echo "diagnostic dump: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing dump registry: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "registry must end with one LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "registry contains CR or trailing whitespace"

jq -e '
  .format == "tondo-diagnostic-dump/1"
  and .owner == "toolchain.diagnostics"
  and .edition == "0.1"
  and .phase == "DUMP-001"
  and .status == "implemented"
  and .contract == "docs/contracts/diagnostic-dump.md"
  and .schema == "tondo-dump/1"
  and .extension == ".tdump"
  and .encoding == "canonical-json-utf8"
  and .content_address == "sha256"
  and .termination_reasons == [
    "panic", "fatal-signal", "abort", "returned", "cancelled", "timeout",
    "resource-limit"
  ]
  and .required_sections == [
    "header", "termination", "identity", "stacks", "heap_summary",
    "resource_ledger", "scheduler_tail", "redaction", "limitations"
  ]
  and .optional_sections == ["registers", "source_maps", "retainers"]
  and .user_payloads == "omitted-by-default"
  and .analyzer.command == "tondo dump analyze <file.tdump>"
  and .analyzer.formats == ["human", "json"]
  and .analyzer.network == false
  and .analyzer.executes_dump_code == false
  and .analyzer.invalid_input_exit_status == 2
  and .limits == {max_dump_bytes: 268435456, fail_closed: true, canonical_bytes_required: true}
  and .privacy == {
    payloads: "redacted-by-default",
    secrets: "never-emitted-by-default",
    logical_paths_only: true,
    network_upload: false
  }
  and .hosted_scope == {
    logical_trace_capture: true,
    physical_registers: false,
    native_unwind: false,
    signal_safe_capture: "DIAG-NATIVE-001"
  }
  and .public_stdlib_api == false
  and .source_keywords_added == false
  and .next_blocks == ["NATIVE-001"]
' "$contract" >/dev/null || die "invalid dump registry"

for path in \
    docs/contracts/diagnostic-dump.md \
    crates/tondo-vm/src/runtime/dump.rs \
    crates/tondo-vm/src/runtime.rs \
    crates/tondo-cli/src/main.rs; do
    [[ -f "$root/$path" ]] || die "missing dump evidence: $path"
done

grep -Fq 'pub fn capture_dump' "$root/crates/tondo-vm/src/runtime/dump.rs" \
    || die "dump capture entry point is absent"
grep -Fq 'pub fn analyze_dump' "$root/crates/tondo-vm/src/runtime/dump.rs" \
    || die "dump analyzer entry point is absent"
grep -Fq 'pub fn decode' "$root/crates/tondo-vm/src/runtime/dump.rs" \
    || die "dump integrity reader is absent"
grep -Fq 'tondo dump analyze' "$root/crates/tondo-cli/src/main.rs" \
    || die "CLI dump analyzer is absent"
! grep -Fq 'pub mod dump;' "$root/crates/tondo-vm/src/runtime.rs" \
    || die "dump internals must remain behind the runtime facade"

echo "diagnostic dump: OK (hosted logical capture, canonical hash, redaction and offline analyzer)"
