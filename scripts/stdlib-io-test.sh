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
    'pub fn readAll[R: Reader](reader: var R, policy: IoLimits): Bytes ! IoError' \
    'pub fn writeAll[W: Writer](writer: var W, data: Bytes): Unit ! IoError'; do
    grep -Fq "$signature" docs/contracts/stdlib-core.md
done

for symbol in \
    'IntrinsicType::Reader' \
    'IntrinsicType::Writer' \
    'HirBootstrapHostFunction::ReaderRead' \
    'HirBootstrapHostFunction::WriterWrite' \
    'HirBootstrapHostFunction::WriterFlush' \
    'HirBootstrapHostFunction::IoReadAll' \
    'HirBootstrapHostFunction::IoWriteAll'; do
    grep -Fq "$symbol" crates/tondo-compiler/src/hir.rs \
        crates/tondo-compiler/src/hir/check.rs crates/tondo-compiler/src/hir/lower.rs
done

for symbol in \
    'BytecodeIntrinsicType::Reader' \
    'BytecodeIntrinsicType::Writer' \
    'RuntimeHostValueKind::Reader' \
    'RuntimeHostValueKind::Writer'; do
    grep -Fq "$symbol" crates/tondo-vm/src/runtime/execute.rs \
        crates/tondo-vm/src/bytecode.rs
done

for declaration in \
    'pub trait Reader' \
    'pub trait Writer' \
    'pub enum ReadResult { Data(bytes.Bytes), Eof }' \
    'pub enum IoError { Closed, Cancelled, InvalidData, ResourceLimit, Host }' \
    'pub type IoLimits = { priv maxBytes: Int, priv maxRead: Int }' \
    'pub fn readAll[R: Reader](reader: var R, policy: IoLimits)' \
    'pub fn writeAll[W: Writer](writer: var W, data: bytes.Bytes)'; do
    grep -Fq "$declaration" crates/tondo-compiler/src/bootstrap/io.to
done

# Execute implementations supplied by ordinary Tondo source. Host registration
# symbols alone cannot establish generic protocol behavior or nominal shapes.
cargo test --locked -p tondo-cli --test cli io_static_

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
  ([.rows[] | select(.owner == "std.io")] | length) == 7
  and ([.rows[] | select(.owner == "std.io" and .declaring_trait != null)
    | {trait: .declaring_trait, signature}]
    | sort_by(.signature)) == ([
      {trait: "Reader", signature: "fn read(var self, max: Int): ReadResult ! IoError suspends"},
      {trait: "Writer", signature: "fn write(var self, data: Bytes): Int ! IoError suspends"},
      {trait: "Writer", signature: "fn flush(var self): Unit ! IoError suspends"}
    ] | sort_by(.signature))
  and all(.rows[] | select(.owner == "std.io"); .missing == [])
  and all(.rows[] | select(.owner == "std.io"); .status == "verified")
' testing/stdlib-public-api.json >/dev/null

jq -e '
  any(.owners[]; .id == "std.io"
    and .cells.HOST.status == "not-applicable"
    and (.cells.HOST.reason | contains("portable"))
    and .cells.MODEL.status == "verified"
    and .cells.TEST.status == "verified"
    and .cells.FUZZ.status == "partial"
    and .cells.FUZZ.component_status == "verified"
    and .cells.FUZZ.evidence_kind == "kernel-invariants"
    and (.cells.FUZZ.reason | type == "string" and length > 0)
    and .cells.PERF.status == "verified"
    and .cells.PERF.reason == null
    and .cells.CONF.status == "pending"
    and (.cells.CONF.reason | type == "string" and length > 0)
    and .cells.DOC.status == "verified")
' testing/stdlib-owner-evidence.json >/dev/null

scripts/stdlib-owner-evidence-check.sh >/dev/null

echo "std.io component tests: OK (whole-owner FUZZ and CONF promotion remain pending)"
