#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_NATIVE_ARC_CONTRACT:-$root/testing/native-arc.json}"

die() {
    echo "native ARC: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with one LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains CR or trailing whitespace"

jq -e '
  def unique_values: length == (unique | length);
  .format == "tondo-native-arc/1"
  and .owner == "toolchain.native_memory"
  and .edition == "0.1"
  and .phase == "M11"
  and ((.status == "arc-001-closed"
        and .implemented_blocks == ["ARC-001"]
        and .pending_blocks == ["ARC-002"]
        and .corpus.arc_002_cases == []
        and .next_blocks == ["ARC-002"])
       or (.status == "closed"
        and .implemented_blocks == ["ARC-001", "ARC-002"]
        and .pending_blocks == []
        and (.corpus.arc_002_cases | length == 3 and unique_values)
        and .next_blocks == ["DIAG-NATIVE-001"]))
  and .implementation.runtime == "crates/tondo-native-runtime/src/lib.rs"
  and .implementation.tests == "crates/tondo-native-runtime/src/lib.rs"
  and .implementation.contract == "docs/contracts/native-arc.md"
  and .ownership.local == "checked-u32-strong-count"
  and .ownership.shared == "atomic-u32-compare-update-count"
  and .ownership.payload_edges == "retain-on-publish-transfer-on-consume-release-on-terminal"
  and .ownership.roots == "frame-root-counts-and-runtime-pins"
  and .ownership.frames == "normal-and-abort-cleanup-share-one-exact-once-path"
  and .ownership.scope == "child-edge-and-cancel-before-scope-release"
  and .ownership.select == "registration-retain-and-owned-arm-discard"
  and .ownership.workers == "runtime-pin-until-logical-terminal"
  and .cycle_collection.algorithm == "trial-deletion-of-unreachable-strong-components"
  and .cycle_collection.trigger == "explicit-quiescence-or-256-allocation-pressure"
  and .cycle_collection.weak_refs == "tombstone-metadata-with-acquire-strong-upgrade"
  and .cycle_collection.finalizers == "collector-never-runs-user-cleanup"
  and (.corpus.arc_001_cases | length == 7 and unique_values)
  and (.invariants | length == 8 and unique_values)
  and (.negative_cases | length == 5 and unique_values)
' "$contract" >/dev/null || die "invalid machine-readable contract"

for path in \
    crates/tondo-native-runtime/src/lib.rs \
    docs/contracts/native-arc.md; do
    [[ -f "$root/$path" ]] || die "missing native ARC input: $path"
done

for symbol in \
    tondo_rt_retain \
    tondo_rt_release \
    tondo_rt_mark_shared \
    tondo_rt_weak_new \
    tondo_rt_weak_upgrade \
    tondo_rt_collect_cycles \
    tondo_rt_quiesce; do
    grep -Fq "$symbol" "$root/crates/tondo-native-runtime/src/lib.rs" \
        || die "runtime symbol is missing: $symbol"
done

grep -Fq 'AtomicU32' "$root/crates/tondo-native-runtime/src/lib.rs" \
    || die "shared ARC count is not atomic"
grep -Fq 'fetch_update' "$root/crates/tondo-native-runtime/src/lib.rs" \
    || die "shared ARC update is not checked"
grep -Fq 'fn collect_cycles' "$root/crates/tondo-native-runtime/src/lib.rs" \
    || die "cycle collector is missing"
grep -Fq 'cleanup_destroyed_object' "$root/crates/tondo-native-runtime/src/lib.rs" \
    || die "terminal cleanup hook is missing"
grep -Fq 'arc_cycle_collection_reclaims_independent_cycles' \
    "$root/crates/tondo-native-runtime/src/lib.rs" \
    || die "ARC cycle corpus is missing"

echo "native ARC contract: OK (checked ownership, roots, terminal cleanup and weak/cycle boundary)"
