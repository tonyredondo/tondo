#!/usr/bin/env bash
set -euo pipefail
root="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_LOG_IMPLEMENTATION_CONTRACT:-testing/stdlib-log-implementation.json}"
die() { echo "logging implementation: $*" >&2; exit 1; }
jq -e --slurpfile parent testing/stdlib-log.json '
  .format == "tondo-stdlib-log-implementation/1"
  and .owner == "std.log" and .edition == "0.1" and .task == "STD-LOG-IMPL-001"
  and ((.status == "implementation-in-progress" and .quality_gate == "pending-80-percent-per-scope"
    and .promotion.compiler_api == "registered-public-hosted-core-pending-quality")
    or (.status == "verified-public-hosted-core" and .quality_gate == "verified-80-percent-per-scope"
    and .promotion.compiler_api == "verified-public-hosted-core"))
  and .parent == "testing/stdlib-log.json"
  and .contract == "docs/contracts/stdlib-log-implementation.md"
  and .selected_route == "ordinary-tondo-core-and-hosted-scalar-value-kernel"
  and .capabilities == {required:[],import_effect:"none",provider_reads:false}
  and .surface.types == [$parent[0].surface.types[] | select(. != "ConsoleSink" and . != "FileSink")]
  and .surface.trait_methods == $parent[0].surface.trait_methods
  and .surface.signatures == [$parent[0].surface.signatures[] |
    select(.id != "console-sink-create" and .id != "file-sink-create")]
  and (.surface.signatures | length) == 19 and .surface.private_value_fields == true
  and .surface.private_helpers == ["std.log.Fields.__withField","std.log.LogEvent.__create"]
  and .memory == {validation_node_bytes:8,
    construction_bound:"512+4*retained-argument-bytes+referenced-Bytes-lengths",
    transport:"exact-reply-after-bounded-construction",
    failed_admission:"no-published-value-no-builder-mutation-no-live-charge",
    metrics:"logical-not-RSS-or-OS-allocator-calls"}
  and .lifecycle == {logger:"affine",custom_sink:"structural-capabilities-one-owned-logical-value",
    serialization:"bounded-single-owner-slot-and-guarded-lease",
    cancellation:"restore-owner-before-logger-drain",close:"consuming-and-exactly-once",
    Io:"visible-physical-prefix-permitted-terminal-sink",
    fatal_VM_error:"reap-pending-provider-work-and-release-owned-values-without-user-callbacks"}
  and (.sources | keys) == ["crates/tondo-compiler/src/bootstrap/log.to",
    "crates/tondo-compiler/src/driver.rs","crates/tondo-compiler/src/hir.rs",
    "crates/tondo-compiler/src/hir/check.rs","crates/tondo-compiler/src/hir/lower.rs",
    "crates/tondo-compiler/src/package.rs","crates/tondo-compiler/src/process_host.rs",
    "crates/tondo-compiler/src/process_host/log.rs","crates/tondo-compiler/src/process_host/net.rs",
    "crates/tondo-compiler/src/resolve.rs","crates/tondo-stdlib/src/log.rs",
    "crates/tondo-vm/src/runtime/execute.rs"]
  and .public_project == "acceptance/projects/stdlib-log/tondo.toml"
  and .public_fixtures == ["core.to","main.to","affine.to","queue.to","value-sink.to"]
  and .command == "scripts/stdlib-log-implementation-check.sh"
  and .promotion.public_signatures == 19 and .promotion.host_sinks == $parent[0].implementation.host
  and .promotion.native_abi == "not-implemented" and .promotion.native_aot == "not-claimed"
  and .promotion.full_owner == false and .promotion.performance == "not-measured"
  and .promotion.independent_model_fuzz_conf_doc == "pending-owner-blocks"
' "$contract" >/dev/null || die 'register differs from the locked core boundary'
while IFS=$'\t' read -r path hash; do
    test "sha256:$(sha256sum "$path" | cut -d ' ' -f 1)" = "$hash" || die "source differs: $path"
done < <(jq -r '.sources | to_entries[] | [.key,.value] | @tsv' "$contract")
for fixture in core main affine queue value-sink; do
    test -s "acceptance/projects/stdlib-log/src/$fixture.to" || die "missing public fixture: $fixture"
done
if [[ "${1:-}" == --contract-only ]]; then
    echo 'logging implementation contract: OK (19 public core operations; two private helpers; hosted boundary)'
    exit 0
fi
if [[ "$(uname -s)" == Linux && -z "${TONDO_TEST_PROCESS_CGROUP:-}" ]]; then
    die 'focused process retirement tests require scripts/test-process-scope.sh in a delegated scope'
fi
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
cargo test -q -p tondo-stdlib --lib --locked log::
cargo test -q -p tondo-compiler --lib --locked logging_
cargo test -q -p tondo-compiler --lib --locked ordinary_fatal_
cargo test -q -p tondo-compiler --lib --locked network_fatal_
cargo test -q -p tondo-vm --lib --locked fatal_entry_error_
cargo test -q -p tondo-compiler --lib --locked import_cycles_
cargo build -q -p tondo-cli --locked
target_dir="${CARGO_TARGET_DIR:-target}"
[[ "$target_dir" = /* ]] || target_dir="$root/$target_dir"
timeout 60 "$target_dir/debug/tondo" check --project acceptance/projects/stdlib-log
timeout 60 "$target_dir/debug/tondo" run --project acceptance/projects/stdlib-log
echo 'logging implementation: OK (public core/custom sinks; native promotion pending)'
