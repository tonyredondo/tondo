#!/usr/bin/env bash
set -euo pipefail
root="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_CHANNEL_OWNERSHIP_CONTRACT:-testing/stdlib-channel-ownership.json}"
die() { echo "channel ownership: $*" >&2; exit 1; }
jq -e --slurpfile parent testing/stdlib-channel.json '
  .format == "tondo-stdlib-channel-ownership/1"
  and .owner == "std.channel" and .edition == "0.1"
  and .task == "STD-CHANNEL-OWNERSHIP-001"
  and ((.status == "implementation-in-progress" and .quality_gate == "pending-80-percent-per-scope")
    or (.status == "verified-public-hosted-ownership" and .quality_gate == "verified-80-percent-per-scope"))
  and .parent == "testing/stdlib-channel.json"
  and .contract == "docs/contracts/stdlib-channel-ownership.md"
  and .command == "scripts/stdlib-channel-ownership-check.sh"
  and .selected_route == "public-hosted-vm" and .target == "x86_64-unknown-linux-gnu"
  and .identity == {package:"toolchain:std:0.1-bootstrap",module:"channel",namespace:"type",
    generic_arity:1,same_named_user_types:"ordinary-structural-derivation"}
  and .capabilities == {
    sender:{Copy:false,Discard:true,Equatable:false,Key:false,Send:"T: Send",Share:"T: Send"},
    receiver:{Copy:false,Discard:false,Equatable:false,Key:false,Send:"T: Send",Share:"T: Send"},
    terminal:{sender:false,receiver:true},propagation:["record","generic","option","result","array","enum","closure"],
    derivation:["hir","independent-bytecode"]}
  and .lifecycle == {fork:"explicit-new-endpoint",close:"consumes-owner",
    sender_discard:"close-last-sender-with-or-without-test-memory-budget",receiver_abandonment:"compile-error",
    defer:"reserve-live-affine-owner-and-observe-current-state",
    mixed_aggregate:"consecutive-exact-cleanup-handoffs-with-unguarded-no-op-disarms",
    ordinary_receiver_discard:"reject-pending-values",terminal_unwind:"retire-without-drain-allocation",
    blocking_worker:"forward-terminal-cleanup-to-provider",iteration:"consumes-and-closes-in-private-scope",
    collect:"consumes-and-closes-on-every-outcome",paths:["normal","recoverable-error","panic","structured-cancellation"]}
  and .public_projects == ["acceptance/projects/channel-ownership/tondo.toml","acceptance/projects/channel-slot/tondo.toml"]
  and .iterator_protocol == {close:"fn close(iterator: Self) suspends",
    implementation:"explicit-or-sealed-standard-witness",state:"final-owner-including-copy-cursors",
    source_copy_defer:"registration-snapshot",collect_routes:"shared-ordinary-mir-loop-and-guarded-cleanup"}
  and .sources == ["crates/tondo-compiler/src/hir/availability.rs","crates/tondo-compiler/src/hir/check.rs",
    "crates/tondo-compiler/src/hir/lower.rs","crates/tondo-compiler/src/hir/capabilities.rs","crates/tondo-compiler/src/hir/terminal.rs",
    "crates/tondo-compiler/src/mir/verify.rs","crates/tondo-compiler/src/mir/lower.rs","crates/tondo-compiler/src/process_host.rs","crates/tondo-vm/src/channel.rs",
    "crates/tondo-vm/src/bytecode/verify.rs","crates/tondo-vm/src/runtime/execute.rs"]
  and .promotion == {native_runtime_abi:"unchanged-prior-transport-boundary",native_aot:"not-claimed",performance:"not-measured",release:false}
  and $parent[0].ownership_verification == {task:.task,status:.status,
    contract:"testing/stdlib-channel-ownership.json",document:.contract,native_aot:"not-claimed"}
' "$contract" >/dev/null || die 'contract differs from the endpoint ownership boundary'
while IFS= read -r path; do
    test -f "$path" || die "missing input: $path"
done < <(jq -r '.sources[],.public_projects[],.contract' "$contract")
if [[ "${1:-}" == --contract-only ]]; then
    echo 'channel ownership contract: OK (exact endpoint identity; hosted ownership boundary)'
    exit 0
fi
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
cargo test -q -p tondo-compiler -p tondo-vm --lib --locked channel_endpoint_ownership
cargo test -q -p tondo-vm --lib --locked channel_endpoint_identity
cargo build -q -p tondo-cli --locked
target_dir="${CARGO_TARGET_DIR:-target}"
[[ "$target_dir" = /* ]] || target_dir="$root/$target_dir"
for project in channel-ownership channel-slot; do
    timeout 60 "$target_dir/debug/tondo" check --project "acceptance/projects/$project"
    timeout 60 "$target_dir/debug/tondo" run --project "acceptance/projects/$project"
done
echo 'channel ownership: OK (public hosted owner state and terminal paths; no native AOT promotion)'
