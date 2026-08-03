#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
contract="$root/testing/stdlib-hosted.json"
[[ -f "$contract" ]] || { echo "missing hosted owner contract" >&2; exit 1; }
tail -c 1 "$contract" | cmp -s <(printf '\n') || { echo "hosted contract must end with LF" >&2; exit 1; }
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || { echo "hosted contract has whitespace" >&2; exit 1; }
jq -e '
  .format == "tondo-stdlib-owner-contract/1" and
  .owner == "std.hosted-group" and .edition == "0.1" and
  .phase == "STD-0.1A" and .status == "draft-contract" and
  .contract == "docs/contracts/stdlib-hosted.md" and
  .owners == ["std.console","std.path","std.fs","std.process"] and
  .capabilities["std.console"] == ["console"] and
  .capabilities["std.path"] == [] and
  .capabilities["std.fs"] == ["filesystem"] and
  .capabilities["std.process"] == ["process"] and
  (.invariants | length) == 7 and (.test_matrix | length) == 7 and
  .promotion_next == "STD-IMPL-002"
' "$contract" >/dev/null
[[ -s "$root/docs/contracts/stdlib-hosted.md" ]] || { echo "missing hosted contract document" >&2; exit 1; }
echo "std.hosted owner contract: OK"
