#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-uuid-implementation.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT
expect_failure() {
    if "$@" >"$tmp_dir/rejected.log" 2>&1; then
        echo "std.uuid implementation tests: invalid record unexpectedly passed" >&2
        exit 1
    fi
}
for status in ready-stdlib-kernel verified-stdlib-kernel; do
    jq --arg status "$status" '
      .implementation.status = $status
      | if $status == "verified-stdlib-kernel" then
          .implementation.quality_gate = "verified-80-percent-per-scope"
          | .implementation.required_follow_ups |= map(select(. != "STD-UUID-IMPL-001"))
          | .promotion.next_blocks = ["STD-UUID-HOST-001"]
        else
          .implementation.quality_gate = "pending-80-percent-per-scope"
          | .implementation.required_follow_ups = ["STD-UUID-IMPL-001", "STD-UUID-HOST-001", "STD-UUID-TEST-001", "STD-UUID-PERF-001", "STD-UUID-CONF-001", "STD-UUID-DOC-001"]
          | .promotion.next_blocks = ["STD-UUID-IMPL-001"]
        end' testing/stdlib-uuid.json > "$tmp_dir/valid.json"
    for checker in scripts/stdlib-uuid-check.sh scripts/stdlib-uuid-implementation-check.sh; do
        env TONDO_STDLIB_UUID_CONTRACT="$tmp_dir/valid.json" bash "$checker" >/dev/null
    done
    while IFS= read -r mutation; do
        jq "$mutation" "$tmp_dir/valid.json" > "$tmp_dir/invalid.json"
        for checker in scripts/stdlib-uuid-check.sh scripts/stdlib-uuid-implementation-check.sh; do
            expect_failure env TONDO_STDLIB_UUID_CONTRACT="$tmp_dir/invalid.json" bash "$checker"
        done
    done <<'MUTATIONS'
.implementation.status = "pending-after-native-gate"
.implementation.quality_gate = "passed"
.implementation.quality_gate = (if .implementation.quality_gate == "pending-80-percent-per-scope" then "verified-80-percent-per-scope" else "pending-80-percent-per-scope" end)
.promotion.next_blocks = (if .promotion.next_blocks == ["STD-UUID-IMPL-001"] then ["STD-UUID-HOST-001"] else ["STD-UUID-IMPL-001"] end)
.implementation.required_follow_ups = []
.implementation.public_api_promoted = true
.implementation.host = "verified-production-host"
.implementation.native_aot_lowering = "verified"
.implementation.runtime_heap = "verified"
.implementation.selected_route = "native-aot"
.implementation.dependency.version = "0.10"
.implementation.dependency.features = []
.implementation.dependency.assembly = true
.implementation.generator_inputs.v4 = "ambient-rng"
.implementation.generator_inputs.v7 = "ambient-clock"
.implementation.sources |= .[0:1]
.implementation.tests |= .[0:17]
.implementation.tests[1] = .implementation.tests[0]
.implementation.tests[0] = "missing-test"
.implementation.proof = ""
.model = {status:"verified"}
.measurement = {status:"verified"}
.conformance = {status:"verified"}
.documentation = {status:"verified"}
MUTATIONS
done
bash scripts/stdlib-uuid-implementation-check.sh
cargo test -p tondo-stdlib --locked --lib uuid::tests
cargo tree -p tondo-stdlib --locked -e features -i sha1 > "$tmp_dir/features.txt"
grep -Fq 'sha1 feature "force-soft"' "$tmp_dir/features.txt"
! grep -Eq 'sha1 feature "(asm|loongarch64_asm)"' "$tmp_dir/features.txt"
echo 'std.uuid implementation tests: OK (18 kernel tests; 96 invalid records; both local promotion states; force-soft)'
