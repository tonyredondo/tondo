#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_root="${TMPDIR:-/tmp}"
tmp_dir="$(mktemp -d "$tmp_root/tondo-stdlib-console-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.console owner tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq 'del(.owners[] | select(. == "std.console"))' testing/stdlib-hosted.json \
    > "$tmp_dir/missing-owner.json"
expect_failure missing-owner env TONDO_STDLIB_HOSTED_CONTRACT="$tmp_dir/missing-owner.json" \
    scripts/stdlib-hosted-check.sh

jq 'del(.capabilities["std.console"])' testing/stdlib-hosted.json \
    > "$tmp_dir/missing-capability.json"
expect_failure missing-capability env TONDO_STDLIB_HOSTED_CONTRACT="$tmp_dir/missing-capability.json" \
    scripts/stdlib-hosted-check.sh

jq '.capabilities["std.console"] = ["filesystem"]' testing/stdlib-hosted.json \
    > "$tmp_dir/wrong-capability.json"
expect_failure wrong-capability env TONDO_STDLIB_HOSTED_CONTRACT="$tmp_dir/wrong-capability.json" \
    scripts/stdlib-hosted-check.sh

jq '.test_matrix |= map(select(. != "partial-io"))' testing/stdlib-hosted.json \
    > "$tmp_dir/missing-partial-io.json"
expect_failure missing-partial-io env TONDO_STDLIB_HOSTED_CONTRACT="$tmp_dir/missing-partial-io.json" \
    scripts/stdlib-hosted-check.sh

for signature in \
    'pub fn stdin(): std.io.Reader ! ConsoleError' \
    'pub fn stdout(): std.io.Writer ! ConsoleError' \
    'pub fn stderr(): std.io.Writer ! ConsoleError' \
    'pub fn readLine(var input: std.io.Reader): String? ! ConsoleError' \
    'pub fn print(value: String): Unit ! ConsoleError' \
    'pub fn println(value: String): Unit ! ConsoleError' \
    'pub fn flush(): Unit ! ConsoleError' \
    'pub enum ConsoleError { Unavailable, Closed, Cancelled, Io(std.io.IoError) }'; do
    grep -Fq "$signature" docs/contracts/stdlib-hosted.md
done

for symbol in \
    'IntrinsicType::Reader' \
    'IntrinsicType::Writer' \
    'IntrinsicType::ConsoleError' \
    'HirBootstrapHostFunction::ConsoleStdin' \
    'HirBootstrapHostFunction::ConsoleStdout' \
    'HirBootstrapHostFunction::ConsoleStderr' \
    'HirBootstrapHostFunction::ConsoleReadLine' \
    'HirBootstrapHostFunction::ConsolePrint' \
    'HirBootstrapHostFunction::ConsolePrintln' \
    'HirBootstrapHostFunction::ConsoleFlush'; do
    grep -Fq "$symbol" crates/tondo-compiler/src/hir.rs \
        crates/tondo-compiler/src/hir/check.rs crates/tondo-compiler/src/hir/lower.rs
done

for symbol in \
    'BytecodeIntrinsicType::Reader' \
    'BytecodeIntrinsicType::Writer' \
    'BytecodeIntrinsicType::ConsoleError' \
    'RuntimeHostValueKind::Reader' \
    'RuntimeHostValueKind::Writer' \
    'RuntimeHostValueKind::ConsoleError' \
    'std.console.stdin' \
    'std.console.stdout' \
    'std.console.stderr' \
    'std.console.readLine' \
    'std.console.print' \
    'std.console.println' \
    'std.console.flush'; do
    grep -Fq "$symbol" crates/tondo-vm/src/runtime/execute.rs \
        crates/tondo-vm/src/bytecode.rs crates/tondo-compiler/src/process_host.rs
done

for symbol in \
    'console_println_uses_a_stable_lf_newline' \
    'console_streams_preserve_partial_reads_and_separate_output_channels' \
    'console_failures_are_typed_atomic_and_redacted' \
    'io_limits_helpers_are_bounded_and_atomic_at_the_public_host_boundary'; do
    grep -Fq "$symbol" crates/tondo-compiler/src/process_host.rs
done

for symbol in \
    'test_operation_checks_console_stream_protocol_through_the_hir' \
    'bootstrap_standard_modules_follow_the_closed_target_capabilities'; do
    grep -Fq "$symbol" crates/tondo-compiler/src/driver.rs
done

for marker in \
    'var input = console.stdin()?' \
    'let line = console.readLine(var input)?' \
    'let _stdout = console.stdout()?' \
    'let _stderr = console.stderr()?' \
    'console.print("line-one")' \
    'console.println("line-two")' \
    'console.flush()'; do
    grep -Fq "$marker" tests/runtime/m11-std-console-001.to
done

grep -Fq 'capability `console` is missing' crates/tondo-compiler/src/driver.rs
grep -Fq 'partial I/O' docs/contracts/stdlib-hosted.md
grep -Fq 'no asumen terminal' docs/contracts/stdlib-hosted.md
grep -Fq 'std.console' testing/stdlib-performance-conformance.json
grep -Fq 'fragmented_stream' docs/contracts/stdlib-performance.md
grep -Fq 'STD-A-CONSOLE-EVIDENCE-001' docs/contracts/stdlib-s1a.md

jq -e '
  ([.rows[] | select(.owner == "std.console")] | length) == 7
  and all(.rows[] | select(.owner == "std.console"); .missing == [])
  and all(.rows[] | select(.owner == "std.console"); .status == "verified")
' testing/stdlib-public-api.json >/dev/null

jq -e '
  any(.leaves[]; .id == "STD-A-CONSOLE-EVIDENCE-001" and .owners == ["std.console"])
  and any(.owners[]; .id == "std.console"
    and .cells.SPEC.status == "verified"
    and .cells.IMPL.status == "verified"
    and .cells.HOST.status == "verified"
    and .cells.MODEL.status == "verified"
    and .cells.TEST.status == "verified"
    and .cells.FUZZ.status == "verified"
    and .cells.PERF.status == "not-applicable"
    and .cells.CONF.status == "verified"
    and .cells.CONF.reason == null
    and .cells.DOC.status == "verified")
' testing/stdlib-owner-evidence.json >/dev/null

echo "std.console owner tests: OK"
