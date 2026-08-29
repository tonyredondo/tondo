#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-async-group-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.async.Group tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.ownership.affine = false' testing/stdlib-async-group.json > "$tmp_dir/not-affine.json"
expect_failure not-affine env TONDO_STDLIB_ASYNC_GROUP_CONTRACT="$tmp_dir/not-affine.json" scripts/stdlib-async-group-check.sh

jq '.ordering.losing_select_branch_mutates = true' testing/stdlib-async-group.json > "$tmp_dir/select-mutates.json"
expect_failure select-mutates env TONDO_STDLIB_ASYNC_GROUP_CONTRACT="$tmp_dir/select-mutates.json" scripts/stdlib-async-group-check.sh

jq '.all.error_priority = "first-arrival"' testing/stdlib-async-group.json > "$tmp_dir/arrival-error.json"
expect_failure arrival-error env TONDO_STDLIB_ASYNC_GROUP_CONTRACT="$tmp_dir/arrival-error.json" scripts/stdlib-async-group-check.sh

jq '.host.status = "required"' testing/stdlib-async-group.json > "$tmp_dir/host-bridge.json"
expect_failure host-bridge env TONDO_STDLIB_ASYNC_GROUP_CONTRACT="$tmp_dir/host-bridge.json" scripts/stdlib-async-group-check.sh

for marker in \
    'pub fn group[T, E](): Group[T, E]' \
    'pub fn Group.add(var self, job: Join[T, E]): Unit' \
    'pub fn Group.all(self): Array[T] ! E suspends' \
    'pub fn Group.settle(self): Array[T ! E] suspends' \
    'pub fn Group.next(var self): Completion[T, E]? selectable' \
    'pub fn Group.cancel(self): Unit suspends'; do
    grep -Fq "$marker" docs/contracts/stdlib-async.md
done

for marker in \
    'WaitGroup' \
    'actual-terminal-completion-order' \
    'lowest-insertion-index-among-child-errors' \
    'group.select.rollback' \
    'not-applicable'; do
    grep -Fq "$marker" testing/stdlib-async-group.json
done

for marker in \
    'HirBootstrapHostFunction::AsyncGroup' \
    'HirBootstrapHostFunction::AsyncGroupAdd' \
    'HirBootstrapHostFunction::AsyncGroupAll' \
    'HirBootstrapHostFunction::AsyncGroupSettle' \
    'HirBootstrapHostFunction::AsyncGroupNext' \
    'HirBootstrapHostFunction::AsyncGroupCancel' \
    'TaskWait::Group' \
    'RuntimeGroupState' \
    'poll_group_operation'; do
    grep -Fq "$marker" \
        crates/tondo-compiler/src/hir.rs \
        crates/tondo-compiler/src/hir/lower.rs \
        crates/tondo-vm/src/runtime/execute.rs \
        || { echo "std.async.Group tests: missing implementation anchor $marker" >&2; exit 1; }
done

runtime_output="$(cargo run -q -p tondo-cli -- run tests/runtime/m11-std-async-group-001.to)"
[[ "$runtime_output" == "group-ok" ]] \
    || { echo "std.async.Group tests: runtime fixture produced unexpected output: $runtime_output" >&2; exit 1; }

jq -e '
  .task == "STD-ASYNC-GROUP-SPEC-001"
  and .surface.selectable_operations == ["group-next"]
  and .ownership.join_use_after_add == "compile-error"
  and .all.error_drains_cleanup == true
  and .settle.error_cancels_siblings == false
  and .next.losing_select_branch == "rollback-without-removal"
  and .cancel.drains_all_children == true
  and .implementation.public_api_promoted == false
  and .implementation.status == "verified-hosted-vm"
  and .implementation.native_status == "pending-native-async-runtime"
  and .promotion.implementation_pending == [
    "STD-ASYNC-GROUP-TEST-001",
    "STD-ASYNC-GROUP-PERF-001",
    "STD-ASYNC-GROUP-CONF-001",
    "STD-ASYNC-GROUP-DOC-001"
  ]
' testing/stdlib-async-group.json >/dev/null

echo "std.async.Group tests: OK (negative contract cases, hosted runtime, ownership, ordering and cancellation anchors)"
