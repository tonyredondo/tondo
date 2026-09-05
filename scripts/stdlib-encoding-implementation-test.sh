#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-encoding-implementation-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.encoding implementation tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.implementation.status = "pending-after-native-gate"' testing/stdlib-encoding.json > "$tmp_dir/pending.json"
expect_failure pending-status env TONDO_STDLIB_ENCODING_CONTRACT="$tmp_dir/pending.json" scripts/stdlib-encoding-implementation-check.sh

jq '.implementation.host = "required-after-native-gate"' testing/stdlib-encoding.json > "$tmp_dir/host-pending.json"
expect_failure pending-host env TONDO_STDLIB_ENCODING_CONTRACT="$tmp_dir/host-pending.json" scripts/stdlib-encoding-implementation-check.sh

jq '.implementation.native_aot_lowering = "verified"' testing/stdlib-encoding.json > "$tmp_dir/native-claim.json"
expect_failure native-claim env TONDO_STDLIB_ENCODING_CONTRACT="$tmp_dir/native-claim.json" scripts/stdlib-encoding-implementation-check.sh

jq '.implementation.fixture.stdout = "wrong"' testing/stdlib-encoding.json > "$tmp_dir/wrong-fixture.json"
expect_failure wrong-fixture env TONDO_STDLIB_ENCODING_CONTRACT="$tmp_dir/wrong-fixture.json" scripts/stdlib-encoding-implementation-check.sh

jq '.implementation.sources = .implementation.sources[0:11]' testing/stdlib-encoding.json > "$tmp_dir/missing-source.json"
expect_failure missing-source env TONDO_STDLIB_ENCODING_CONTRACT="$tmp_dir/missing-source.json" scripts/stdlib-encoding-implementation-check.sh

jq '.implementation.tests = .implementation.tests[0:13]' testing/stdlib-encoding.json > "$tmp_dir/missing-test.json"
expect_failure missing-test env TONDO_STDLIB_ENCODING_CONTRACT="$tmp_dir/missing-test.json" scripts/stdlib-encoding-implementation-check.sh

jq -e '
  .implementation.status == "verified-hosted-vm"
  and .implementation.public_api_promoted == false
  and .implementation.native_aot_lowering == "not-claimed"
  and .implementation.required_follow_ups == ["STD-ENCODING-PERF-001", "STD-ENCODING-CONF-001", "STD-ENCODING-DOC-001"]
  and .promotion.next_blocks == ["STD-ENCODING-PERF-001"]
' testing/stdlib-encoding.json >/dev/null

for marker in \
    "base64_streaming_is_chunk_invariant_and_terminal" \
    "writer_no_progress_and_io_errors_are_terminal" \
    "encoding_host_io_limits_and_affine_cleanup"; do
    grep -Fq "$marker" \
        crates/tondo-stdlib/src/encoding.rs \
        crates/tondo-compiler/src/process_host.rs \
        || { echo "std.encoding implementation tests: missing marker $marker" >&2; exit 1; }
done

echo "std.encoding implementation tests: OK (state, boundary and evidence negatives)"
