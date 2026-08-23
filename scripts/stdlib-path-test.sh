#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_root="${TMPDIR:-/tmp}"
tmp_dir="$(mktemp -d "$tmp_root/tondo-stdlib-path-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.path owner tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq 'del(.owners[] | select(. == "std.path"))' testing/stdlib-hosted.json \
    > "$tmp_dir/missing-owner.json"
expect_failure missing-owner env TONDO_STDLIB_HOSTED_CONTRACT="$tmp_dir/missing-owner.json" \
    scripts/stdlib-hosted-check.sh

jq 'del(.test_matrix[] | select(. == "empty-input"))' testing/stdlib-hosted.json \
    > "$tmp_dir/missing-empty-input.json"
expect_failure missing-empty-input env TONDO_STDLIB_HOSTED_CONTRACT="$tmp_dir/missing-empty-input.json" \
    scripts/stdlib-hosted-check.sh

jq '.capabilities["std.path"] = ["filesystem"]' testing/stdlib-hosted.json \
    > "$tmp_dir/wrong-capability.json"
expect_failure wrong-capability env TONDO_STDLIB_HOSTED_CONTRACT="$tmp_dir/wrong-capability.json" \
    scripts/stdlib-hosted-check.sh

for signature in \
    'pub fn Path.fromString(value: String): Path ! PathError' \
    'pub fn Path.fromBytes(value: Bytes): Path ! PathError' \
    'pub fn Path.join(self, component: String): Path ! PathError' \
    'pub fn Path.parent(self): Path?' \
    'pub fn Path.fileName(self): String?' \
    'pub fn Path.extension(self): String?' \
    'pub fn Path.kind(self): Bool' \
    'pub fn Path.isEmpty(self): Bool' \
    'pub fn Path.toString(self): String ! PathError' \
    'pub fn Path.toBytes(self): Bytes'; do
    grep -Fq "$signature" docs/contracts/stdlib-hosted.md
done

for symbol in \
    'IntrinsicType::Path' \
    'IntrinsicType::PathError' \
    'HirBootstrapHostFunction::PathFromString' \
    'HirBootstrapHostFunction::PathFromBytes' \
    'HirBootstrapHostFunction::PathJoin' \
    'HirBootstrapHostFunction::PathParent' \
    'HirBootstrapHostFunction::PathFileName' \
    'HirBootstrapHostFunction::PathExtension' \
    'HirBootstrapHostFunction::PathKind' \
    'HirBootstrapHostFunction::PathToString' \
    'HirBootstrapHostFunction::PathToBytes'; do
    grep -Fq "$symbol" crates/tondo-compiler/src/hir.rs \
        crates/tondo-compiler/src/hir/check.rs crates/tondo-compiler/src/hir/lower.rs
done

for symbol in \
    'BytecodeIntrinsicType::Path' \
    'BytecodeIntrinsicType::PathError' \
    'RuntimeHostValueKind::Path' \
    'RuntimeHostValueKind::PathError' \
    'path_host_boundary_preserves_native_bytes_and_lexical_semantics'; do
    grep -Fq "$symbol" crates/tondo-vm/src/runtime/execute.rs \
        crates/tondo-vm/src/bytecode.rs crates/tondo-compiler/src/process_host.rs
done

for symbol in \
    'from_string' \
    'from_bytes' \
    'pub fn join' \
    'pub fn parent' \
    'pub fn file_name' \
    'pub fn extension' \
    'pub fn is_absolute' \
    'pub fn to_bytes' \
    'pub fn to_string' \
    'bounded_native_corpus_preserves_bytes_and_never_normalizes' \
    'utf8_and_native_bytes_round_trip_without_normalization' \
    'limits_are_exact_and_rejections_are_atomic'; do
    grep -Fq "$symbol" crates/tondo-stdlib/src/path.rs
done

for marker in \
    'let root = path.fromString("/tmp")?' \
    'let native = path.fromBytes' \
    'match native.toString()' \
    'String(file.toBytes())? == "/tmp/tondo.txt"'; do
    grep -Fq "$marker" tests/runtime/m11-std-path-001.to
done

grep -Fq 'no consultan el filesystem' docs/contracts/stdlib-hosted.md
grep -Fq 'NFC' docs/contracts/stdlib-hosted.md
grep -Fq 'native bytes' docs/contracts/stdlib-s1a.md
grep -Fq 'std.path' testing/stdlib-performance-conformance.json

jq -e '
  ([.rows[] | select(.owner == "std.path")] | length) == 10
  and all(.rows[] | select(.owner == "std.path"); .missing == [])
  and all(.rows[] | select(.owner == "std.path"); .status == "verified")
' testing/stdlib-public-api.json >/dev/null

jq -e '
  any(.owners[]; .id == "std.path"
    and .cells.HOST.status == "not-applicable"
    and (.cells.HOST.reason | contains("purely lexical"))
    and .cells.MODEL.status == "verified"
    and .cells.TEST.status == "verified"
    and .cells.FUZZ.status == "verified"
    and .cells.PERF.status == "verified"
    and .cells.PERF.reason == null
    and .cells.CONF.status == "verified"
    and .cells.CONF.reason == null
    and .cells.DOC.status == "verified")
' testing/stdlib-owner-evidence.json >/dev/null

echo "std.path owner tests: OK"
