#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
contract="${TONDO_NATIVE_ABI_CONTRACT:-$root/testing/native-abi.json}"

[[ -f "$contract" ]] || { echo "missing native ABI contract" >&2; exit 1; }
tail -c 1 "$contract" | cmp -s <(printf '\n') || { echo "native ABI contract must end with LF" >&2; exit 1; }
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || { echo "native ABI contract has trailing whitespace" >&2; exit 1; }

jq -e '
  .format == "tondo-native-abi-contract/1"
  and .owner == "toolchain.native_abi"
  and .edition == "0.1"
  and .phase == "M11"
  and .status == "closed-contract"
  and .version == "tondo-native-runtime-abi/1"
  and .calling_convention == "verified-direct-and-runtime-call-lowering"
  and .value_representation == "private-descriptor-backed-managed-values"
  and .result_representation == "scalar-or-runtime-result-record"
  and .ownership == "mir-edge-retain-release-and-resource-terminal"
  and .unwind == "explicit-normal-unwind-abort"
  and .async_frames == "frame-task-waker-registry"
  and .diagnostics == "source-span-task-thread-crash-envelope"
  and .host_handles == "opaque-capability-indexed"
  and .sync_bridge.status == "verified-private-scalar-lane"
  and .sync_bridge.atomic_symbols == [
    "tondo_rt_atomic_new",
    "tondo_rt_atomic_load",
    "tondo_rt_atomic_store",
    "tondo_rt_atomic_swap",
    "tondo_rt_atomic_compare_exchange"
  ]
  and .sync_bridge.parking_symbols == [
    "tondo_rt_sync_park_new",
    "tondo_rt_sync_park_epoch",
    "tondo_rt_sync_park_wait",
    "tondo_rt_sync_park_wake",
    "tondo_rt_sync_park_waiters"
  ]
  and .sync_bridge.memory_order_codes == {Relaxed: 0, Acquire: 1, Release: 2, AcqRel: 3, SeqCst: 4}
  and .sync_bridge.atomic_value_lane == "u64"
  and .sync_bridge.parking_signal == "epoch-condvar"
  and .sync_bridge.cooperative_wait == "poll-and-scheduler-park"
  and .sync_bridge.blocking_wait == "native-workers-only"
  and .sync_bridge.generic_values == "pending-native-lowering"
  and .direct_calls == "verified-ordinal-resolved-private-symbols"
  and .visibility == "compiler-runtime-only"
  and .public_ffi == "forbidden"
  and (.invariants | length >= 8)
  and (.invariants == (.invariants | sort | unique))
  and (.negative_cases | length == 7)
  and .next_blocks == []
' "$contract" >/dev/null || { echo "invalid native ABI contract" >&2; exit 1; }

source="$root/crates/tondo-compiler/src/toolchain.rs"
grep -Fq 'pub struct NativeAbiContract' "$source" || { echo "missing typed ABI contract" >&2; exit 1; }
grep -Fq 'NATIVE_ABI_CONTRACT_FORMAT' "$source" || { echo "missing ABI contract format" >&2; exit 1; }
grep -Fq 'private-versioned-no-ffi-promise' "$root/docs/contracts/native-memory.md" || { echo "memory visibility boundary is incomplete" >&2; exit 1; }
grep -Fq 'verified-ordinal-resolved-private-symbols' "$root/docs/contracts/native-abi.md" || { echo "ABI direct-call boundary is incomplete" >&2; exit 1; }
for symbol in \
    tondo_rt_atomic_new \
    tondo_rt_atomic_load \
    tondo_rt_atomic_store \
    tondo_rt_atomic_swap \
    tondo_rt_atomic_compare_exchange \
    tondo_rt_sync_park_new \
    tondo_rt_sync_park_epoch \
    tondo_rt_sync_park_wait \
    tondo_rt_sync_park_wake \
    tondo_rt_sync_park_waiters; do
    grep -Fq "$symbol" "$root/crates/tondo-native-runtime/src/lib.rs" \
        || { echo "native sync ABI symbol is missing: $symbol" >&2; exit 1; }
done
grep -Fq 'cooperative VM never calls the blocking wait symbol' \
    "$root/docs/contracts/native-abi.md" \
    || { echo "native sync ABI does not document cooperative non-blocking wait" >&2; exit 1; }
echo "native ABI contract: OK"
