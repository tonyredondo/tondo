#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-uuid-host.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT
expect_failure() {
    if "$@" >"$tmp_dir/rejected.log" 2>&1; then
        echo "std.uuid host tests: invalid record unexpectedly passed" >&2
        exit 1
    fi
}
negative_count=0
for status in ready-production-hosted verified-production-hosted; do
    jq --arg status "$status" '
      .status = $status
      | .quality_gate = (if $status == "verified-production-hosted" then "verified-80-percent-per-scope" else "pending-80-percent-per-scope" end)
      | .required_follow_ups = (["STD-UUID-TEST-001", "STD-UUID-PERF-001", "STD-UUID-CONF-001", "STD-UUID-DOC-001"] | if $status == "ready-production-hosted" then ["STD-UUID-HOST-001"] + . else . end)
    ' testing/stdlib-uuid-host.json > "$tmp_dir/host.json"
    jq --slurpfile host "$tmp_dir/host.json" '
      del(.model)
      | .host.status = $host[0].status | .host.quality_gate = $host[0].quality_gate
      | .implementation.host = $host[0].status
      | .implementation.runtime_heap = (if $host[0].status == "verified-production-hosted" then "verified-hosted-admission" else "ready-hosted-admission" end)
      | .implementation.required_follow_ups = $host[0].required_follow_ups
      | .promotion.next_blocks = [$host[0].required_follow_ups[0]]
    ' testing/stdlib-uuid.json > "$tmp_dir/parent.json"
    env TONDO_STDLIB_UUID_CONTRACT="$tmp_dir/parent.json" TONDO_STDLIB_UUID_HOST_CONTRACT="$tmp_dir/host.json" bash scripts/stdlib-uuid-host-check.sh >/dev/null
    while IFS= read -r mutation; do
        jq "$mutation" "$tmp_dir/host.json" > "$tmp_dir/invalid.json"
        expect_failure env TONDO_STDLIB_UUID_CONTRACT="$tmp_dir/parent.json" TONDO_STDLIB_UUID_HOST_CONTRACT="$tmp_dir/invalid.json" bash scripts/stdlib-uuid-host-check.sh
        negative_count=$((negative_count + 1))
    done <<'MUTATIONS'
.status = "published"
.quality_gate = "passed"
.quality_gate = (if .quality_gate == "pending-80-percent-per-scope" then "verified-80-percent-per-scope" else "pending-80-percent-per-scope" end)
.selected_route = "native-AOT"
.target = "all-targets"
.public_registration = "model-only"
.public_api_promoted = true
.storage.carrier = "signed-Int128"
.storage.host_handle = true
.providers.entropy.dependency.version = "0.4"
.providers.entropy.dependency.features = ["custom"]
.providers.entropy.calls_per_generation = 2
.providers.entropy.v7_bytes = 16
.providers.entropy.partial_bytes_published = true
.providers.entropy.tondo_retry = true
.providers.entropy.unsupported = "EntropyFailure"
.providers.clock.conversion = "truncate-before-range-check"
.providers.clock.precedes_entropy = false
.providers.clock.strict_monotonicity = true
.capabilities.v4 = []
.capabilities.v7 = ["entropy"]
.capabilities.checked_references = ["direct-call"]
.capabilities.import_effect = "read-entropy"
.capabilities.suspends = true
.testing_provider.public_tondo_setter = true
.testing_provider.clock_rows = 257
.testing_provider.exhaustion = "OS-fallback"
.testing_provider.capabilities_granted = true
.admission.detached_reply_max_bytes = 150
.admission.typed_vm_result = "after-provider"
.admission.clock_or_entropy_consumed_on_admission_refusal = true
.admission.memory_measurement = "RSS"
.errors.display = "errno"
.errors.partial_success = true
.sources[1] = .sources[0]
.fixtures = []
.tests |= .[0:22]
.tests[1] = .tests[0]
.tests[0] = "missing-test"
.not_claimed |= map(select(. != "native-AOT"))
.proof = ""
.required_follow_ups = []
MUTATIONS
    jq '.host.status = "verified-native-host"' "$tmp_dir/parent.json" > "$tmp_dir/invalid-parent.json"
    expect_failure env TONDO_STDLIB_UUID_CONTRACT="$tmp_dir/invalid-parent.json" TONDO_STDLIB_UUID_HOST_CONTRACT="$tmp_dir/host.json" bash scripts/stdlib-uuid-host-check.sh
    negative_count=$((negative_count + 1))
done
bash scripts/stdlib-uuid-host-check.sh
cargo test -p tondo-compiler --locked --lib uuid
cargo test -p tondo-vm --locked --lib uuid
echo "std.uuid host tests: OK (23 focused compiler/VM tests; $negative_count invalid records; both local progression states)"
