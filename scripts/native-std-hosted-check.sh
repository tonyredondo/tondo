#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_NATIVE_STD_HOSTED_CONTRACT:-$root/testing/native-std-hosted.json}"

die() {
    echo "native std.hosted: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing hosted contract"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains trailing whitespace"

jq -e '
  .format == "tondo-native-std-hosted/1"
  and .task == "NATIVE-STD-HOSTED-001"
  and .owner == "toolchain.native_std_hosted"
  and .edition == "0.1"
  and .phase == "M20"
  and .status == "closed"
  and .contract == "docs/contracts/native-std-hosted.md"
  and .fixture == "tests/native/native-std-hosted-001.to"
  and .runner == "scripts/native-std-hosted-test.sh"
  and .report == "target/reliability/evidence/native-std-hosted.json"
  and .capabilities == ["clock", "console", "filesystem", "process"]
  and .handles == "opaque-affine-u64"
  and .buffers == "bounded-immutable-byte-carriers"
  and (.cases | length == 10)
  and (.errors | sort) == ["cancelled", "closed", "invalid-handle", "limit", "unsupported"]
  and (.invariants | length == 8)
  and .native_abi == "tondo_rt_host_open/read/write/output/cancel/close/status"
  and .oracle == "same-status-and-byte-observations-as-hosted-contract"
  and (.negative_cases | length == 10)
  and .next_blocks == ["NATIVE-STD-001"]
' "$contract" >/dev/null || die "invalid hosted contract"

for path in \
    docs/contracts/native-std-hosted.md \
    tests/native/native-std-hosted-001.to \
    crates/tondo-native-runtime/src/lib.rs \
    scripts/native-std-hosted-test.sh; do
    [[ -f "$root/$path" ]] || die "missing hosted input: $path"
done

for marker in \
    'tondo_rt_host_open' \
    'tondo_rt_host_read' \
    'tondo_rt_host_write' \
    'tondo_rt_host_cancel' \
    'tondo_rt_host_close' \
    'tondo_rt_buffer_from_byte' \
    'STATUS_HOST_LIMIT' \
    'HOST_MAX_BYTES'; do
    grep -Fq "$marker" crates/tondo-native-runtime/src/lib.rs \
        || die "runtime is missing hosted marker: $marker"
done
grep -Fq 'ambient provider lookup' docs/contracts/native-std-hosted.md || die "hosted contract permits ambient lookup"
grep -Fq 'partial' docs/contracts/native-std-hosted.md || die "hosted contract omits partial I/O"
grep -Fq 'NATIVE-STD-001' docs/contracts/native-std-hosted.md || die "hosted next block is missing"

echo "native std.hosted: OK (explicit capabilities, opaque buffers, partial I/O and terminal cleanup)"
