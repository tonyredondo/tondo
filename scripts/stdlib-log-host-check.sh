#!/usr/bin/env bash
set -euo pipefail
root="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_LOG_HOST_CONTRACT:-testing/stdlib-log-host.json}"
die() { echo "logging host: $*" >&2; exit 1; }
scripts/stdlib-log-check.sh >/dev/null
jq -e --slurpfile parent "${TONDO_STDLIB_LOG_CONTRACT:-testing/stdlib-log.json}" '
  .format == "tondo-stdlib-host-boundary/1" and .owner == "std.log"
  and .task == "STD-LOG-HOST-001" and .parent == "testing/stdlib-log.json"
  and .contract == "docs/contracts/stdlib-log-host.md"
  and .target == "tondo-vm-hosted"
  and .selected_route == "ordinary-tondo-sinks-and-hosted-scalar-providers"
  and .public_api_promoted == false and .native_abi_or_aot == false
  and ((.status == "implementation-in-progress" and .quality_gate == "pending-current-source-proof"
    and $parent[0].implementation.host == "pending-STD-LOG-HOST-001")
    or (.status == "verified-production-hosted" and .quality_gate == "verified-80-percent-per-scope"
    and $parent[0].implementation.host == "verified-production-hosted"))
  and .surface == {
    builtin_types:["ConsoleSink","FileSink"], constructors:2, sink_implementations:2,
    private_delivery_helpers:["std.log.LogEvent.__format","std.log.FileSink.__openFile",
      "std.log.FileSink.__closeFile","std.log.ConsoleSink.__closeOutput"],
    console_constructor:"non-suspendible-console", file_constructor:"suspends-immediate-open",
    missing_capability:"static-E1008", builtin_capabilities:["Send","Share"], builtin_copy_or_discard:false}
  and .providers == {
    console:"explicit-stdout-or-stderr-existing-hosted-writer", filesystem:"explicit-path-existing-file-provider",
    append:"atomic-create-if-missing-append-if-existing", truncate:"create-or-truncate", parent_creation:false,
    network:"caller-owned-LogSink-explicit-transport-and-wire-protocol", ambient_lookup:false,
    hidden_worker:false, rotation_or_retry:false}
  and .queue == {capacity:"SinkOptions.capacity-in-records", accepted:"fully-formatted-record-admitted",
    drain:["flush","consuming-close","Block-full-oldest-record"],
    reject_or_drop:"full-queue-preserves-previously-accepted-records", batching:false,
    short_write:"advance-retained-record-offset", cancellation:"restore-guarded-owner-resume-without-replay",
    Io:"visible-possible-prefix-terminal-sink"}
  and .admission == {error_bytes:72, format_workspace:"core-construction-bound-plus-maxEventBytes",
    publication:"typed-Bytes-before-new-host-identity",
    file_open:"typed-file-result-and-path-storage-before-create-or-truncate",
    close:"typed-result-before-unconditional-writer-retirement",
    liveness:"Bytes-Path-Writer-File-roots-independent-of-test-account", metrics:"logical-not-RSS-or-OS-allocator-calls"}
  and .public_project == "acceptance/projects/stdlib-log-host/tondo.toml"
  and (.fixtures | length) == 9 and (.fixtures | unique | length) == 9
  and .command_output == {streams:"separate-selected-stdout-and-stderr",
    observations:["wire-conformance","reliability","fixture-sidecars"],
    runtime_error:"records-before-diagnostics",
    vm_resource_failure:"preserve-completed-records-no-user-cleanup"}
  and (.tests | length) == 20 and (.tests | unique | length) == 20
  and .promotion == {independent_model_fuzz:"testing/stdlib-log-test.json", performance:"not-measured",
    common_conformance:"pending-STD-LOG-CONF-001", usage_documentation:"pending-STD-LOG-DOC-001", full_owner:false}
' "$contract" >/dev/null || die 'register differs from the hosted boundary'
while IFS= read -r path; do test -s "$path" || die "missing artifact: $path"; done < <(jq -r '.fixtures[],.contract,.public_project' "$contract")
while IFS= read -r anchor; do
    path="${anchor%%::*}"; name="${anchor##*::}"
    grep -Fq "fn $name(" "$path" || die "missing test: $anchor"
done < <(jq -r '.tests[]' "$contract")
for name in log_buffer log_console log_file; do
    test -s "crates/tondo-compiler/src/bootstrap/$name.to" || die "missing sink source: $name"
done
if [[ "${1:-}" == --contract-only ]]; then
    echo 'logging host contract: OK (explicit queued sinks; hosted scalar; no native promotion)'
    exit 0
fi
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
cargo test -q -p tondo-compiler --lib --locked logging_
cargo build -q -p tondo-cli --locked
target_dir="${CARGO_TARGET_DIR:-target}"
[[ "$target_dir" = /* ]] || target_dir="$root/$target_dir"
timeout 60 "$target_dir/debug/tondo" check --project acceptance/projects/stdlib-log-host
timeout 60 "$target_dir/debug/tondo" run --project acceptance/projects/stdlib-log-host
echo 'logging host: OK (public sinks; queue policies; lifecycle; caller-owned network)'
