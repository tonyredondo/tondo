#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_root="${TMPDIR:-/tmp}"
tmp_dir="$(mktemp -d "$tmp_root/tondo-stdlib-process-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.process owner tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq 'del(.owners[] | select(. == "std.process"))' testing/stdlib-hosted.json \
    > "$tmp_dir/missing-owner.json"
expect_failure missing-owner env TONDO_STDLIB_HOSTED_CONTRACT="$tmp_dir/missing-owner.json" \
    scripts/stdlib-hosted-check.sh

jq 'del(.capabilities["std.process"])' testing/stdlib-hosted.json \
    > "$tmp_dir/missing-capability.json"
expect_failure missing-capability env TONDO_STDLIB_HOSTED_CONTRACT="$tmp_dir/missing-capability.json" \
    scripts/stdlib-hosted-check.sh

jq '.capabilities["std.process"] = ["filesystem"]' testing/stdlib-hosted.json \
    > "$tmp_dir/wrong-capability.json"
expect_failure wrong-capability env TONDO_STDLIB_HOSTED_CONTRACT="$tmp_dir/wrong-capability.json" \
    scripts/stdlib-hosted-check.sh

for invariant in \
    'bounded-resources' \
    'explicit-capability-boundary' \
    'terminal-cleanup' \
    'argv-preserved' \
    'shell-explicit' \
    'streams-separate-and-combined' \
    'typed-stderr-redirection'; do
    jq --arg invariant "$invariant" \
        '.invariants |= map(select(. != $invariant))' testing/stdlib-hosted.json \
        > "$tmp_dir/missing-invariant-$invariant.json"
    expect_failure "missing-$invariant" \
        env TONDO_STDLIB_HOSTED_CONTRACT="$tmp_dir/missing-invariant-$invariant.json" \
        scripts/stdlib-hosted-check.sh
done

for test_case in \
    'partial-io' \
    'cancellation' \
    'cleanup-on-unwind' \
    'combined-output-order' \
    'pipeline-redirection' \
    'host-error-redaction'; do
    jq --arg test_case "$test_case" \
        '.test_matrix |= map(select(. != $test_case))' testing/stdlib-hosted.json \
        > "$tmp_dir/missing-test-$test_case.json"
    expect_failure "missing-$test_case" \
        env TONDO_STDLIB_HOSTED_CONTRACT="$tmp_dir/missing-test-$test_case.json" \
        scripts/stdlib-hosted-check.sh
done

for signature in \
    'pub type Command' \
    'pub type Pipeline' \
    'pub type ProcessHandle' \
    'pub type ExitStatus' \
    'pub type ProcessOutput' \
    'pub enum ProcessError { Unavailable, PermissionDenied, InvalidArgument, Spawn, Io, Cancelled, ResourceLimit }' \
    'pub enum ProcessExitError { NonZero(ProcessOutput), Signalled(ProcessOutput) }' \
    'pub fn command(program: String, arguments: ...String): Command ! ProcessError' \
    'pub fn shell(command: String): Command ! ProcessError' \
    'pub fn pipe(left: Command, right: Command): Pipeline ! ProcessError' \
    'pub fn Command.mergeStderr(self): Command' \
    'pub fn Pipeline.mergeStderr(self): Pipeline' \
    'pub fn Command.run(self): ExitStatus ! ProcessError' \
    'pub fn Command.output(self): ProcessOutput ! ProcessError' \
    'pub fn Command.check(self): ProcessOutput ! (ProcessError | ProcessExitError)' \
    'pub fn Command.start(self): ProcessHandle ! ProcessError' \
    'pub fn ProcessHandle.wait(var self): ExitStatus ! ProcessError' \
    'pub fn ProcessHandle.cancel(var self): Unit' \
    'pub fn ProcessOutput.stdout(self): Bytes' \
    'pub fn ProcessOutput.stderr(self): Bytes' \
    'pub fn ProcessOutput.combined(self): Bytes' \
    'pub fn ProcessOutput.statuses(self): Array[ExitStatus]' \
    'pub fn ExitStatus.code(self): Int?' \
    'pub fn ExitStatus.success(self): Bool'; do
    grep -Fq "$signature" docs/contracts/stdlib-hosted.md
done

for symbol in \
    'IntrinsicType::Command' \
    'IntrinsicType::Pipeline' \
    'IntrinsicType::ExitStatus' \
    'IntrinsicType::ProcessOutput' \
    'IntrinsicType::ProcessHandle' \
    'IntrinsicType::ProcessError' \
    'IntrinsicType::ProcessExitError' \
    'HirBootstrapHostFunction::ProcessCommand' \
    'HirBootstrapHostFunction::ProcessShell' \
    'HirBootstrapHostFunction::ProcessPipe' \
    'HirBootstrapHostFunction::CommandStart' \
    'HirBootstrapHostFunction::CommandStatus' \
    'HirBootstrapHostFunction::CommandOutput' \
    'HirBootstrapHostFunction::CommandRun' \
    'HirBootstrapHostFunction::CommandCheck' \
    'HirBootstrapHostFunction::CommandMergeStderr' \
    'HirBootstrapHostFunction::PipelineStart' \
    'HirBootstrapHostFunction::PipelineStatus' \
    'HirBootstrapHostFunction::PipelineOutput' \
    'HirBootstrapHostFunction::PipelineRun' \
    'HirBootstrapHostFunction::PipelineCheck' \
    'HirBootstrapHostFunction::PipelineMergeStderr' \
    'HirBootstrapHostFunction::ProcessHandleStatus' \
    'HirBootstrapHostFunction::ProcessHandleOutput' \
    'HirBootstrapHostFunction::ProcessHandleRun' \
    'HirBootstrapHostFunction::ProcessHandleCheck' \
    'HirBootstrapHostFunction::ProcessHandleCancel' \
    'HirBootstrapHostFunction::ProcessOutputStdout' \
    'HirBootstrapHostFunction::ProcessOutputStderr' \
    'HirBootstrapHostFunction::ProcessOutputCombined' \
    'HirBootstrapHostFunction::ProcessOutputStatuses' \
    'HirBootstrapHostFunction::ExitStatusCode' \
    'HirBootstrapHostFunction::ExitStatusSuccess'; do
    grep -Fq "$symbol" crates/tondo-compiler/src/hir.rs \
        crates/tondo-compiler/src/hir/check.rs crates/tondo-compiler/src/hir/lower.rs
done

if grep -Fq 'HirBootstrapHostFunction::ProcessArgs' crates/tondo-compiler/src/hir.rs \
    crates/tondo-compiler/src/hir/check.rs crates/tondo-compiler/src/hir/lower.rs \
    || grep -Fq 'HirBootstrapHostFunction::ProcessCmd' crates/tondo-compiler/src/hir.rs \
        crates/tondo-compiler/src/hir/check.rs crates/tondo-compiler/src/hir/lower.rs; then
    echo "std.process owner tests: removed process.args/process.cmd host symbols remain" >&2
    exit 1
fi

for symbol in \
    'BytecodeIntrinsicType::Command' \
    'BytecodeIntrinsicType::Pipeline' \
    'BytecodeIntrinsicType::ExitStatus' \
    'BytecodeIntrinsicType::ProcessOutput' \
    'BytecodeIntrinsicType::ProcessHandle' \
    'BytecodeIntrinsicType::ProcessError' \
    'BytecodeIntrinsicType::ProcessExitError' \
    'BytecodeBootstrapHostFunction::ProcessPipe' \
    'BytecodeBootstrapHostFunction::CommandMergeStderr' \
    'BytecodeBootstrapHostFunction::PipelineMergeStderr' \
    'BytecodeBootstrapHostFunction::ProcessOutputCombined'; do
    grep -Fq "$symbol" crates/tondo-compiler/src/bytecode/lower.rs \
        crates/tondo-vm/src/bytecode.rs crates/tondo-vm/src/runtime/execute.rs
done

for symbol in \
    'process_error' \
    'ProcessGroup::spawn' \
    'backpressure' \
    'downstream_closed_pipe' \
    'configure_process_group' \
    'process_output_preserves_separate_streams_and_terminal_combined_bytes' \
    'merge_stderr_feeds_the_next_stage_like_bash_pipe_ampersand' \
    'separate_pipeline_stderr_is_concatenated_by_stage_order' \
    'all_four_pipe_shapes_preserve_stage_order' \
    'kernel_pipes_drain_output_larger_than_their_backpressure_window' \
    'exit_status_is_data_while_check_and_spawn_failures_are_typed_errors' \
    'plan_construction_is_inert_and_exact_arguments_bypass_the_shell' \
    'cancel_and_host_drop_reap_started_children'; do
    grep -Fq "$symbol" crates/tondo-compiler/src/process_host.rs
done

for fixture in \
    tests/runtime/m8-process-001.to \
    tests/runtime/m8-process-cancel.to \
    tests/runtime/m8-process-check-error.to \
    tests/runtime/m8-process-panic-cleanup.to \
    tests/runtime/m8-process-spawn-error.to \
    tests/runtime/m8-spec-24-17.to \
    tests/compile-fail/m8-process-closed-api.to \
    tests/compile-fail/m8-process-dropped-handle.to \
    tests/compile-fail/m8-process-invalid-pipe.to \
    tests/compile-fail/m8-process-method-requires-import.to; do
    [[ -f "$fixture" ]]
done

for marker in \
    'process.command(' \
    'process.shell(' \
    'source | first' \
    'mergeStderr()' \
    'streams.stdout' \
    'streams.stderr' \
    'streams.combined' \
    'streams.statuses' \
    'ProcessExitError' \
    'sleeping.cancel()' \
    'console.print("process-ok'; do
    grep -Fq "$marker" tests/runtime/m8-process-001.to
done

grep -Fq 'capability `process` is missing' crates/tondo-compiler/src/driver.rs
grep -Fq 'backpressure bounded' docs/contracts/stdlib-hosted.md
grep -Fq 'idempotent cleanup' docs/contracts/process-host.md
grep -Fq 'reap every child' docs/contracts/process-host.md
grep -Fq 'STD-A-PROC-EVIDENCE-001' docs/contracts/stdlib-s1a.md
grep -Fq 'std.process' docs/contracts/stdlib-matrix.md
grep -Fq 'deterministic child fixtures' testing/stdlib-performance-conformance.json

jq -e '
  ([.rows[] | select(.owner == "std.process")] | length) == 17
  and all(.rows[] | select(.owner == "std.process"); .missing == [])
  and all(.rows[] | select(.owner == "std.process"); .status == "verified")
' testing/stdlib-public-api.json >/dev/null

jq -e '
  any(.leaves[]; .id == "STD-A-PROC-EVIDENCE-001" and .owners == ["std.process"])
  and any(.owners[]; .id == "std.process"
    and .cells.SPEC.status == "verified"
    and .cells.IMPL.status == "verified"
    and .cells.HOST.status == "verified"
    and .cells.MODEL.status == "verified"
    and .cells.TEST.status == "verified"
    and .cells.FUZZ.status == "verified"
    and .cells.PERF.status == "partial"
    and .cells.CONF.status == "partial"
    and .cells.DOC.status == "verified")
' testing/stdlib-owner-evidence.json >/dev/null

echo "std.process owner tests: OK"
