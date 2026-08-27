#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
contract="${TONDO_NATIVE_MEMORY_CONTRACT:-$root/testing/native-memory.json}"

[[ -f "$contract" ]] || { echo "missing native memory contract" >&2; exit 1; }
tail -c 1 "$contract" | cmp -s <(printf '\n') || { echo "native memory contract must end with LF" >&2; exit 1; }
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || { echo "native memory contract has trailing whitespace" >&2; exit 1; }

jq -e '
  .format == "tondo-native-memory-contract/1"
  and .owner == "toolchain.native_memory"
  and .edition == "0.1"
  and .phase == "M11"
  and .status == "closed-contract"
  and .strategy == "hybrid-arc-cycle-collector"
  and .strong_counts == "non-atomic-unshared-atomic-shared"
  and .cycle_collection == "trial-deletion-on-pressure-and-quiescence"
  and .weak_refs == "runtime-managed-weak-edges"
  and .roots == ["async-frame", "host-handle", "stack", "task", "thread"]
  and .resources == "deterministic-mir-cleanup"
  and .async_frames == "publish-roots-before-suspend"
  and .copy_on_write == "uniqueness-guarded-value-storage"
  and .cancellation == "cleanup-before-task-terminal"
  and .public_layout == "private-versioned-no-ffi-promise"
  and (.invariants | length >= 7)
  and (.invariants == (.invariants | sort | unique))
  and (.negative_cases | length == 5)
  and .next_blocks == ["NATIVE-STD-CORE-001"]
' "$contract" >/dev/null || { echo "invalid native memory contract" >&2; exit 1; }

source="$root/crates/tondo-compiler/src/toolchain.rs"
grep -Fq 'pub struct NativeMemoryContract' "$source" || { echo "missing typed memory contract" >&2; exit 1; }
grep -Fq 'NATIVE_MEMORY_CONTRACT_FORMAT' "$source" || { echo "missing memory contract format" >&2; exit 1; }
grep -Fq 'hybrid-arc-cycle-collector' "$root/docs/contracts/native-memory.md" || { echo "memory ADR is incomplete" >&2; exit 1; }
echo "native memory contract: OK"
