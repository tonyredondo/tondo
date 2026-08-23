#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_root="${TMPDIR:-/tmp}"
tmp_dir="$(mktemp -d "$tmp_root/tondo-stdlib-io-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.io owner tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq 'del(.owners[] | select(. == "std.io"))' testing/stdlib-core.json \
    > "$tmp_dir/missing-owner.json"
expect_failure missing-owner env TONDO_STDLIB_CORE_CONTRACT="$tmp_dir/missing-owner.json" \
    scripts/stdlib-core-check.sh

jq 'del(.test_matrix[] | select(. == "async-cancellation"))' testing/stdlib-core.json \
    > "$tmp_dir/missing-cancellation.json"
expect_failure missing-cancellation env TONDO_STDLIB_CORE_CONTRACT="$tmp_dir/missing-cancellation.json" \
    scripts/stdlib-core-check.sh

jq '.owner = "std.io"' testing/stdlib-core.json > "$tmp_dir/wrong-owner.json"
expect_failure wrong-owner env TONDO_STDLIB_CORE_CONTRACT="$tmp_dir/wrong-owner.json" \
    scripts/stdlib-core-check.sh

for signature in \
    'pub fn defaultLimits(): IoLimits' \
    'pub fn limits(maxBytes: Int, maxRead: Int): IoLimits ! IoError' \
    'pub fn readAll[R: Reader](var reader: R, limits: IoLimits): Bytes ! IoError' \
    'pub fn writeAll(var writer: Writer, data: Bytes): Unit ! IoError'; do
    grep -Fq "$signature" docs/contracts/stdlib-core.md
done

for symbol in \
    'IntrinsicType::Reader' \
    'IntrinsicType::Writer' \
    'IntrinsicType::IoLimits' \
    'HirBootstrapHostFunction::ReaderRead' \
    'HirBootstrapHostFunction::WriterWrite' \
    'HirBootstrapHostFunction::WriterFlush' \
    'HirBootstrapHostFunction::IoLimitsDefault' \
    'HirBootstrapHostFunction::IoLimitsNew' \
    'HirBootstrapHostFunction::IoReadAll' \
    'HirBootstrapHostFunction::IoWriteAll'; do
    grep -Fq "$symbol" crates/tondo-compiler/src/hir.rs \
        crates/tondo-compiler/src/hir/check.rs crates/tondo-compiler/src/hir/lower.rs
done

for symbol in \
    'BytecodeIntrinsicType::Reader' \
    'BytecodeIntrinsicType::Writer' \
    'BytecodeIntrinsicType::IoLimits' \
    'RuntimeHostValueKind::Reader' \
    'RuntimeHostValueKind::Writer' \
    'RuntimeHostValueKind::IoLimits' \
    'RuntimeHostValueKind::IoError'; do
    grep -Fq "$symbol" crates/tondo-vm/src/runtime/execute.rs \
        crates/tondo-vm/src/bytecode.rs
done

for symbol in \
    'std.io.defaultLimits' \
    'std.io.limits' \
    'std.io.readAll' \
    'std.io.writeAll' \
    'std.io.Reader.read' \
    'std.io.Writer.write' \
    'std.io.Writer.flush' \
    'fn reader_state' \
    'fn writer_stream' \
    'fn io_limits'; do
    grep -Fq "$symbol" crates/tondo-compiler/src/process_host.rs
done

for symbol in \
    'read_all_handles_short_reads_and_eof' \
    'read_all_rejects_zero_limits_and_overflow' \
    'readers_reject_invalid_chunk_sizes' \
    'write_all_handles_short_writes_and_flushes' \
    'writer_rejects_zero_capacity' \
    'limits_require_positive_bounds_and_allow_clamping' \
    'cancellation_is_propagated_without_partial_success' \
    'chunk_partition_fuzz_is_bounded_and_deterministic' \
    'read_all_rejects_empty_and_oversized_chunks' \
    'read_all_propagates_errors_after_consuming_a_data_chunk' \
    'write_all_rejects_no_progress_and_overreported_writes' \
    'write_all_propagates_flush_errors_after_all_bytes_are_accepted' \
    'default_writer_accepts_full_writes_and_flushes'; do
    grep -Fq "$symbol" crates/tondo-stdlib/src/io.rs
done

for symbol in \
    'console_streams_preserve_partial_reads_and_separate_output_channels' \
    'io_limits_helpers_are_bounded_and_atomic_at_the_public_host_boundary' \
    'format_builder_host_boundaries_are_materialized_atomically'; do
    grep -Fq "$symbol" crates/tondo-compiler/src/process_host.rs \
        crates/tondo-vm/src/runtime/execute.rs
done

for marker in \
    'var input = console.stdin()?' \
    'let limits = io.defaultLimits()' \
    'let data = io.readAll(var input, limits)?' \
    'var output = console.stdout()?' \
    'io.writeAll(var output, bytes.Bytes("io-ok")?)?'; do
    grep -Fq "$marker" tests/runtime/m11-std-io-001.to
done

grep -Fq 'partial I/O' docs/contracts/stdlib-core.md
grep -Fq 'Reader/Writer' docs/contracts/stdlib-s1a.md
grep -Fq 'std.io' testing/stdlib-performance-conformance.json
grep -Fq 'fragmented_stream' docs/contracts/stdlib-performance.md

jq -e '
  ([.rows[] | select(.owner == "std.io")] | length) == 4
  and all(.rows[] | select(.owner == "std.io"); .missing == [])
  and all(.rows[] | select(.owner == "std.io"); .status == "verified")
' testing/stdlib-public-api.json >/dev/null

jq -e '
  any(.owners[]; .id == "std.io"
    and .cells.HOST.status == "not-applicable"
    and (.cells.HOST.reason | contains("portable"))
    and .cells.MODEL.status == "verified"
    and .cells.TEST.status == "verified"
    and .cells.FUZZ.status == "verified"
    and .cells.PERF.status == "verified"
    and .cells.PERF.reason == null
    and .cells.CONF.status == "partial"
    and .cells.DOC.status == "verified")
' testing/stdlib-owner-evidence.json >/dev/null

echo "std.io owner tests: OK"
