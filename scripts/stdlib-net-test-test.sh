#!/usr/bin/env bash
set -euo pipefail
root="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_NET_TEST_CONTRACT:-testing/stdlib-net-test.json}"
parent="${TONDO_STDLIB_NET_CONTRACT:-testing/stdlib-net.json}"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/tondo-net-test-contract.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
reject() {
    if "$@" > "$tmp/refusal.log" 2>&1; then
        echo 'std.net tests: invalid boundary unexpectedly accepted' >&2; exit 1
    fi
}
bash scripts/stdlib-net-test-check.sh
# State mutations need a valid ready fixture even after the live owner closes.
# Mutating an already verified value to itself cannot be a negative test.
jq 'del(.measurement,.conformance) | .model.status="ready" | .model.quality_gate="pending-current-source-proof"
    | .promotion.next_blocks=(if .host.status=="implementation-in-progress"
      then ["STD-NET-HOST-001"] else ["STD-NET-TEST-001"] end)
    ' "$parent" > "$tmp/ready-parent.json"
jq '.status="ready" | .quality_gate="pending-current-source-proof"
    | .promotion.test_boundary_promoted=false
    ' "$contract" > "$tmp/ready-child.json"
env TONDO_STDLIB_NET_CONTRACT="$tmp/ready-parent.json" \
    TONDO_STDLIB_NET_TEST_CONTRACT="$tmp/ready-child.json" bash scripts/stdlib-net-test-check.sh
while IFS= read -r mutation; do
    jq "$mutation" "$tmp/ready-child.json" > "$tmp/invalid.json"
    reject env TONDO_STDLIB_NET_CONTRACT="$tmp/ready-parent.json" \
        TONDO_STDLIB_NET_TEST_CONTRACT="$tmp/invalid.json" bash scripts/stdlib-net-test-check.sh
done <<'MUTATIONS'
.model.production_imports = true
.model.os_dns_tls_packet_parser = true
.model.outside_domain = "ResourceLimit"
.model.tls_oracle = "certificate-validation"
.limits.max_reference_bytes = 129
.limits.max_fuzz_steps = 513
.limits.model_seed_count = 1
.test.integration_tests = 8
.test.providers = "external-service"
.test.process_scope = "none"
.test.tls_versions = ["1.3"]
.fuzz.smoke.result = "pending"
.fuzz.smoke.runs = 1
.fuzz.minimal_dependency_graph = "compiler-enabled"
.promotion.native_aot = "verified"
.promotion.public_api_promoted = true
.promotion.test_boundary_promoted = true
.quality_gate = "verified-80-percent-per-scope"
.status = "verified"
.promotion.next_blocks = []
MUTATIONS
while IFS= read -r mutation; do
    jq "$mutation" crates/tondo-reliability/tests/fixtures/net-cases.json > "$tmp/corpus.json"
    reject env TONDO_STDLIB_NET_TEST_CORPUS="$tmp/corpus.json" bash scripts/stdlib-net-test-check.sh
done <<'MUTATIONS'
.cases |= .[0:39]
.cases[1].id = .cases[0].id
.cases[0].operation = "unknown"
.cases[0].text = 0
.cases[0].error = "Host"
.cases[2].error = "unknown"
.cases[13].bytes_hex = "zz"
.cases[13].eof = "yes"
.cases[30].cancelled = 1
.cases[37].addresses = [[256,443]]
.cases[37].addresses = [[2,-1]]
.cases[37].addresses = [[2]]
MUTATIONS
# Validate promotion combinations without mutating the owner register or
# depending on whatever final promotion state happens to be checked out.
for host in implementation-in-progress verified-production-hosted; do
    for model in ready verified; do
        jq --arg host "$host" --arg model "$model" '
          del(.measurement,.conformance) | .host.status=$host | .host.quality_gate=(if $host=="implementation-in-progress"
            then "pending-current-source-proof" else "verified-80-percent-per-scope" end)
          | .model.status=$model | .model.quality_gate=(if $model=="ready"
            then "pending-current-source-proof" else "verified-80-percent-per-scope" end)
          | .promotion.next_blocks=(if $host=="implementation-in-progress" then ["STD-NET-HOST-001"]
            elif $model=="ready" then ["STD-NET-TEST-001"] else ["STD-NET-PERF-001"] end)
        ' testing/stdlib-net.json > "$tmp/parent.json"
        if [[ "$host" == implementation-in-progress && "$model" == verified ]]; then
            reject env TONDO_STDLIB_NET_CONTRACT="$tmp/parent.json" bash scripts/stdlib-net-check.sh
        else
            env TONDO_STDLIB_NET_CONTRACT="$tmp/parent.json" bash scripts/stdlib-net-check.sh >/dev/null
            jq --arg model "$model" '
              .status=$model | .quality_gate=(if $model=="ready" then "pending-current-source-proof"
                else "verified-80-percent-per-scope" end)
              | .promotion.test_boundary_promoted=($model=="verified")
            ' testing/stdlib-net-test.json > "$tmp/child.json"
            env TONDO_STDLIB_NET_CONTRACT="$tmp/parent.json" TONDO_STDLIB_NET_TEST_CONTRACT="$tmp/child.json" \
                bash scripts/stdlib-net-test-check.sh >/dev/null
        fi
    done
done
mkdir "$tmp/no-rg" "$tmp/no-grep"
for utility in bash jq tail cmp grep dirname; do
    utility_path="$(command -v "$utility")"
    ln -s "$utility_path" "$tmp/no-rg/$utility"
    [[ "$utility" == grep ]] || ln -s "$utility_path" "$tmp/no-grep/$utility"
done
env PATH="$tmp/no-rg" bash scripts/stdlib-net-test-check.sh
reject env PATH="$tmp/no-grep" bash scripts/stdlib-net-test-check.sh
graph="$tmp/minimal-graph.txt"
cargo tree --manifest-path fuzz/Cargo.toml --no-default-features --locked -e normal --prefix none -p tondo-fuzz > "$graph"
if grep -Eq '^tondo-(compiler|vm|conformance|reliability) ' "$graph"; then
    echo 'std.net tests: minimal fuzz graph imports a hosted dependency' >&2; exit 1
else [[ "$?" == 1 ]] || exit 1; fi
cargo test -p tondo-reliability --locked --lib net_model
cargo test -p tondo-reliability --locked --test net_kernel_models --test net_corpus --test net_hosted_models
cargo test -p tondo-compiler --locked --lib net_provider::
cargo test -p tondo-compiler --locked --lib network_
echo 'std.net tests: OK (independent reference, exact regressions, controlled hosted execution and invalid promotions)'
