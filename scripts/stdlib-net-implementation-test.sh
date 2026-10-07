#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-net-implementation.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT
expect_failure() {
    if "$@" >"$tmp_dir/rejected.log" 2>&1; then
        echo 'std.net implementation: invalid state unexpectedly passed' >&2
        exit 1
    fi
}
for status in ready-kernel-private-provider verified-kernel-private-provider; do
    jq --arg status "$status" '
      .implementation.status = $status
      | .host.status = "implementation-in-progress"
      | .host.quality_gate = "pending-current-source-proof"
      | .model = null
      | .implementation.quality_gate = (if $status == "ready-kernel-private-provider"
          then "pending-80-percent-per-scope" else "verified-80-percent-per-scope" end)
      | .implementation.required_follow_ups = (if $status == "ready-kernel-private-provider"
          then ["STD-NET-IMPL-001", "STD-NET-HOST-001", "STD-NET-TEST-001", "STD-NET-PERF-001", "STD-NET-CONF-001", "STD-NET-DOC-001"]
          else ["STD-NET-HOST-001", "STD-NET-TEST-001", "STD-NET-PERF-001", "STD-NET-CONF-001", "STD-NET-DOC-001"] end)
      | .promotion.next_blocks = (if $status == "ready-kernel-private-provider"
          then ["STD-NET-IMPL-001"] else ["STD-NET-HOST-001"] end)
    ' testing/stdlib-net.json > "$tmp_dir/valid.json"
    env TONDO_STDLIB_NET_CONTRACT="$tmp_dir/valid.json" bash scripts/stdlib-net-implementation-check.sh >/dev/null
    while IFS= read -r mutation; do
        jq "$mutation" "$tmp_dir/valid.json" > "$tmp_dir/invalid.json"
        expect_failure env TONDO_STDLIB_NET_CONTRACT="$tmp_dir/invalid.json" bash scripts/stdlib-net-implementation-check.sh
    done <<'MUTATIONS'
.implementation.status = "verified-production-hosted"
.implementation.quality_gate = "passed"
.implementation.quality_gate = (if .implementation.quality_gate == "pending-80-percent-per-scope" then "verified-80-percent-per-scope" else "pending-80-percent-per-scope" end)
.implementation.required_follow_ups = []
.promotion.next_blocks = []
.implementation.public_api_promoted = true
.implementation.compiler_api = "verified"
.implementation.runtime_registration = "verified"
.implementation.runtime_heap_admission = "verified"
.implementation.native_abi = "verified"
.implementation.native_aot = "verified"
.implementation.independent_model = "verified"
.implementation.performance = "measured"
.implementation.selected_route = "native-aot"
.implementation.sources |= .[0:8]
.implementation.tests |= .[0:38]
.implementation.tests[1] = .implementation.tests[0]
.implementation.tests[0] = "missing::missing"
.implementation.provider_limits.max_pending_operations = 0
.implementation.provider_limits.memory_metric = "RSS"
.target_configuration.system_config_fallback = true
.private_provider_draft.dns_udp_retransmits = true
.private_provider_draft.tls_versions = ["1.0"]
.private_provider_draft.tls_pin = "skip-verification"
MUTATIONS
done
bash scripts/stdlib-net-implementation-check.sh
cargo test -p tondo-stdlib --locked --lib net::tests
cargo test -p tondo-compiler --locked --lib net_provider:: -- --nocapture
cargo test -p tondo-compiler --locked --lib toolchain::tests::network_target_
cargo test -p tondo-cli --locked --bin tondo project_discovery::tests::network_target_
echo 'std.net implementation tests: OK (39 focused tests; 48 invalid states; both local promotion states)'
