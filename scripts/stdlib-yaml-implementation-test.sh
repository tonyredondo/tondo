#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-yaml-implementation-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.yaml implementation tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.implementation.status = "pending-after-native-gate"' testing/stdlib-yaml.json > "$tmp_dir/pending.json"
expect_failure pending-status env TONDO_STDLIB_YAML_CONTRACT="$tmp_dir/pending.json" scripts/stdlib-yaml-implementation-check.sh

jq '.implementation.host = "required-after-native-gate"' testing/stdlib-yaml.json > "$tmp_dir/host-pending.json"
expect_failure pending-host env TONDO_STDLIB_YAML_CONTRACT="$tmp_dir/host-pending.json" scripts/stdlib-yaml-implementation-check.sh

jq '.implementation.native_aot_lowering = "verified"' testing/stdlib-yaml.json > "$tmp_dir/native-claim.json"
expect_failure native-claim env TONDO_STDLIB_YAML_CONTRACT="$tmp_dir/native-claim.json" scripts/stdlib-yaml-implementation-check.sh

jq '.implementation.fixture.stdout = "wrong"' testing/stdlib-yaml.json > "$tmp_dir/wrong-fixture.json"
expect_failure wrong-fixture env TONDO_STDLIB_YAML_CONTRACT="$tmp_dir/wrong-fixture.json" scripts/stdlib-yaml-implementation-check.sh

jq '.implementation.sources = .implementation.sources[0:16]' testing/stdlib-yaml.json > "$tmp_dir/missing-source.json"
expect_failure missing-source env TONDO_STDLIB_YAML_CONTRACT="$tmp_dir/missing-source.json" scripts/stdlib-yaml-implementation-check.sh

jq '.implementation.tests = .implementation.tests[0:10]' testing/stdlib-yaml.json > "$tmp_dir/missing-test.json"
expect_failure missing-test env TONDO_STDLIB_YAML_CONTRACT="$tmp_dir/missing-test.json" scripts/stdlib-yaml-implementation-check.sh

jq '.implementation.required_follow_ups = []' testing/stdlib-yaml.json > "$tmp_dir/missing-follow-up.json"
expect_failure missing-follow-up env TONDO_STDLIB_YAML_CONTRACT="$tmp_dir/missing-follow-up.json" scripts/stdlib-yaml-implementation-check.sh

jq -e '
  .implementation.status == "verified-hosted-vm"
  and .implementation.public_api_promoted == false
  and .implementation.native_aot_lowering == "not-claimed"
  and .implementation.required_follow_ups == ["STD-YAML-TEST-001", "STD-YAML-PERF-001", "STD-YAML-CONF-001", "STD-YAML-DOC-001"]
  and .promotion.next_blocks == ["STD-YAML-TEST-001"]
' testing/stdlib-yaml.json >/dev/null

for marker in \
    "core_block_flow_and_quotes_round_trip" \
    "aliases_are_copied_and_cycles_are_rejected" \
    "tags_security_and_limits_are_enforced" \
    "canonical_order_and_reader_lifecycle_are_stable" \
    "typed_static_protocol_uses_common_events"; do
    grep -Fq "$marker" crates/tondo-stdlib/src/yaml.rs \
        || { echo "std.yaml implementation tests: missing scalar marker $marker" >&2; exit 1; }
done

for marker in \
    "yaml_public_host_surface_materializes_typed_values_and_streams" \
    "yaml_host_rejects_invalid_limits_and_forged_events_atomically" \
    "yaml_error_kind_ordinal"; do
    grep -Fq "$marker" crates/tondo-compiler/src/process_host.rs \
        || { echo "std.yaml implementation tests: missing host marker $marker" >&2; exit 1; }
done

echo "std.yaml implementation tests: OK (state, boundary, limits and evidence negatives)"
