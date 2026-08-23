#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_root="${TMPDIR:-/tmp}"
tmp_dir="$(mktemp -d "$tmp_root/tondo-stdlib-fs-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.fs owner tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq 'del(.owners[] | select(. == "std.fs"))' testing/stdlib-hosted.json \
    > "$tmp_dir/missing-owner.json"
expect_failure missing-owner env TONDO_STDLIB_HOSTED_CONTRACT="$tmp_dir/missing-owner.json" \
    scripts/stdlib-hosted-check.sh

jq 'del(.capabilities["std.fs"])' testing/stdlib-hosted.json \
    > "$tmp_dir/missing-capability.json"
expect_failure missing-capability env TONDO_STDLIB_HOSTED_CONTRACT="$tmp_dir/missing-capability.json" \
    scripts/stdlib-hosted-check.sh

jq '.capabilities["std.fs"] = ["process"]' testing/stdlib-hosted.json \
    > "$tmp_dir/wrong-capability.json"
expect_failure wrong-capability env TONDO_STDLIB_HOSTED_CONTRACT="$tmp_dir/wrong-capability.json" \
    scripts/stdlib-hosted-check.sh

jq 'del(.test_matrix[] | select(. == "cleanup-on-unwind"))' testing/stdlib-hosted.json \
    > "$tmp_dir/missing-cleanup.json"
expect_failure missing-cleanup env TONDO_STDLIB_HOSTED_CONTRACT="$tmp_dir/missing-cleanup.json" \
    scripts/stdlib-hosted-check.sh

for signature in \
    'pub type File' \
    'pub type Directory' \
    'pub type Metadata' \
    'pub enum OpenMode { Read, Write, ReadWrite, Append, Create, CreateNew }' \
    'pub enum FsError { NotFound, PermissionDenied, AlreadyExists, InvalidPath, NotDirectory, IsDirectory, Closed, ResourceLimit, Cancelled, Io }' \
    'pub fn open(path: Path, mode: OpenMode): File ! FsError' \
    'pub fn openDirectory(path: Path): Directory ! FsError' \
    'pub fn readAll(path: Path): Bytes ! FsError' \
    'pub fn writeAll(path: Path, data: Bytes): Unit ! FsError' \
    'pub fn createDirectory(path: Path, parents: Bool): Unit ! FsError' \
    'pub fn remove(path: Path): Unit ! FsError' \
    'pub fn metadata(path: Path): Metadata ! FsError' \
    'pub fn list(path: Path): Array[Path] ! FsError' \
    'pub fn rename(from: Path, to: Path): Unit ! FsError' \
    'pub fn atomicWrite(path: Path, data: Bytes): Unit ! FsError' \
    'pub fn File.read(var self, max: Int): Option[Bytes] ! FsError' \
    'pub fn File.write(var self, data: Bytes): Int ! FsError' \
    'pub fn File.flush(var self): Unit ! FsError' \
    'pub fn Directory.list(var self): Array[Path] ! FsError'; do
    grep -Fq "$signature" docs/contracts/stdlib-hosted.md
done

for symbol in \
    'IntrinsicType::File' \
    'IntrinsicType::Directory' \
    'IntrinsicType::Metadata' \
    'IntrinsicType::OpenMode' \
    'IntrinsicType::FsError' \
    'HirBootstrapHostFunction::FsOpen' \
    'HirBootstrapHostFunction::FsOpenDirectory' \
    'HirBootstrapHostFunction::FsMetadata' \
    'HirBootstrapHostFunction::FsReadAll' \
    'HirBootstrapHostFunction::FsWriteAll' \
    'HirBootstrapHostFunction::FsCreateDirectory' \
    'HirBootstrapHostFunction::FsRemove' \
    'HirBootstrapHostFunction::FsList' \
    'HirBootstrapHostFunction::FsRename' \
    'HirBootstrapHostFunction::FsAtomicWrite' \
    'HirBootstrapHostFunction::FileRead' \
    'HirBootstrapHostFunction::FileWrite' \
    'HirBootstrapHostFunction::FileFlush' \
    'HirBootstrapHostFunction::DirectoryList'; do
    grep -Fq "$symbol" crates/tondo-compiler/src/hir.rs \
        crates/tondo-compiler/src/hir/check.rs crates/tondo-compiler/src/hir/lower.rs
done

for symbol in \
    'BytecodeIntrinsicType::File' \
    'BytecodeIntrinsicType::Directory' \
    'BytecodeIntrinsicType::Metadata' \
    'BytecodeIntrinsicType::OpenMode' \
    'BytecodeIntrinsicType::FsError' \
    'RuntimeHostValueKind::File' \
    'RuntimeHostValueKind::Directory' \
    'RuntimeHostValueKind::Metadata' \
    'RuntimeHostValueKind::OpenMode' \
    'RuntimeHostValueKind::FsError'; do
    grep -Fq "$symbol" crates/tondo-vm/src/runtime/execute.rs \
        crates/tondo-vm/src/bytecode.rs crates/tondo-compiler/src/process_host.rs
done

for symbol in \
    'filesystem_path' \
    'file_id' \
    'directory_path' \
    'ensure_bytes_len' \
    'ready_fs_jobs' \
    'filesystem_preserves_native_path_bytes_and_returns_typed_errors' \
    'filesystem_directory_operations_are_atomic_and_ordered' \
    'filesystem_handles_are_typed_async_and_cleanup_is_affine' \
    'filesystem_handles_cover_modes_errors_and_cancellation'; do
    grep -Fq "$symbol" crates/tondo-compiler/src/process_host.rs
done

for symbol in \
    'filesystem_module_requires_the_explicit_target_capability' \
    'capability `filesystem` is missing'; do
    grep -Fq "$symbol" crates/tondo-compiler/src/driver.rs
done

for marker in \
    'import std.fs' \
    'fs.readAll' \
    'fs.createDirectory' \
    'fs.atomicWrite' \
    'fs.writeAll' \
    'fs.list' \
    'fs.metadata' \
    'fs.OpenMode.Append' \
    'append_file.write' \
    'append_file.flush' \
    'fs.OpenMode.Read' \
    'read_file.read' \
    'fs.openDirectory' \
    'directory.list' \
    'fs.rename' \
    'fs.remove' \
    'console.print("fs-ok'; do
    grep -Fq "$marker" tests/runtime/m11-std-fs-001.to
done

for marker in \
    'capability `filesystem`' \
    'cleanup normal' \
    'tokens stale o forjados' \
    'short writes' \
    'límites de bytes' \
    'cancelación' \
    'atomicWrite' \
    'orden lexicográfico de bytes' \
    'ResourceLimit'; do
    grep -Fq "$marker" docs/contracts/stdlib-hosted.md
done

grep -Fq 'STD-A-FS-EVIDENCE-001' docs/contracts/stdlib-s1a.md
grep -Fq 'std.fs' docs/contracts/stdlib-matrix.md
grep -Fq 'Filesystem latency' testing/stdlib-performance-conformance.json

jq -e '
  ([.rows[] | select(.owner == "std.fs")] | length) == 14
  and all(.rows[] | select(.owner == "std.fs"); .missing == [])
  and all(.rows[] | select(.owner == "std.fs"); .status == "verified")
' testing/stdlib-public-api.json >/dev/null

jq -e '
  any(.leaves[]; .id == "STD-A-FS-EVIDENCE-001" and .owners == ["std.fs"])
  and any(.owners[]; .id == "std.fs"
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

echo "std.fs owner tests: OK"
