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

jq -e '
  .format == "tondo-stdlib-group-testing/1"
  and .edition == "0.1"
  and .phase == "STD-0.1B"
  and .owner == "std.async.group"
  and .task == "STD-ASYNC-GROUP-TEST-001"
  and .status == "verified"
  and .contract == "testing/stdlib-async-group.json"
  and .limits.max_children == 64
  and .limits.max_fuzz_input_bytes == 4096
  and .limits.max_fuzz_steps == 1024
  and .limits.model_seed_count == 4096
  and .limits.model_max_generated_bytes == 256
  and .model.status == "verified"
  and (.model.source | type == "array" and length == 2)
  and (.model.laws | type == "array" and length >= 8)
  and .model.sequence_seeds == 4096
  and .test.status == "verified"
  and (.test.sources | type == "array" and length > 0)
  and (.test.commands | type == "array" and length > 0)
  and (.test.cases | type == "array" and length >= 10)
  and .test.runtime_output == "group-ok"
  and .fuzz.status == "verified"
  and .fuzz.target == "stdlib_async_group"
  and .fuzz.source == "fuzz/fuzz_targets/stdlib_async_group.rs"
  and .fuzz.corpus == "fuzz/corpus/stdlib_async_group/seed"
  and .fuzz.input_limit_bytes == 4096
  and .fuzz.step_limit == 1024
  and .fuzz.smoke.runs == 128
  and .fuzz.smoke.seed == 4101
  and .fuzz.smoke.result == "passed"
  and .promotion.model_test_fuzz_complete == true
  and .promotion.remaining == [
    "STD-ASYNC-GROUP-PERF-001",
    "STD-ASYNC-GROUP-CONF-001",
    "STD-ASYNC-GROUP-DOC-001"
  ]
' testing/stdlib-async-group-test.json >/dev/null

for path in \
    crates/tondo-reliability/src/group_model.rs \
    crates/tondo-reliability/tests/models.rs \
    fuzz/fuzz_targets/stdlib_async_group.rs \
    fuzz/corpus/stdlib_async_group/seed \
    scripts/stdlib-async-group-fuzz.sh; do
    [[ -e "$path" ]] || { echo "std.async.Group tests: missing evidence path $path" >&2; exit 1; }
done

[[ -x scripts/stdlib-async-group-fuzz.sh ]] \
    || { echo "std.async.Group tests: fuzz runner is not executable" >&2; exit 1; }
grep -Fq 'name = "stdlib_async_group"' fuzz/Cargo.toml

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
    "STD-ASYNC-GROUP-PERF-001",
    "STD-ASYNC-GROUP-CONF-001",
    "STD-ASYNC-GROUP-DOC-001"
  ]
' testing/stdlib-async-group.json >/dev/null

cargo test -p tondo-reliability --test models \
    group_model_sequences_are_bounded_replayable_and_cleanup_complete --locked >/dev/null
cargo test -p tondo-vm --lib runtime::execute::tests::group_ --locked >/dev/null

echo "std.async.Group tests: OK (negative contract cases, hosted runtime, ownership, ordering and cancellation anchors)"
