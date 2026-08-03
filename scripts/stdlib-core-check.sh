#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
contract="$root/testing/stdlib-core.json"
[[ -f "$contract" ]] || { echo "missing core owner contract" >&2; exit 1; }
tail -c 1 "$contract" | cmp -s <(printf '\n') || { echo "core contract must end with LF" >&2; exit 1; }
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || { echo "core contract has whitespace" >&2; exit 1; }
jq -e '
  .format == "tondo-stdlib-owner-contract/1" and
  .owner == "std.core-group" and .edition == "0.1" and
  .phase == "STD-0.1A" and .status == "draft-contract" and
  .contract == "docs/contracts/stdlib-core.md" and
  .owners == ["std.core","std.text","std.collections","std.iter","std.math","std.format","std.io","std.serialization"] and
  (.invariants | length) == 8 and (.test_matrix | length) == 7 and
  .promotion_next == "STD-IMPL-001"
' "$contract" >/dev/null
[[ -s "$root/docs/contracts/stdlib-core.md" ]] || { echo "missing core contract document" >&2; exit 1; }
echo "std.core owner contract: OK"
