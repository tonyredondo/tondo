#!/usr/bin/env bash
set -euo pipefail
root="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_LOG_TEST_CONTRACT:-testing/stdlib-log-test.json}"
parent="${TONDO_STDLIB_LOG_CONTRACT:-testing/stdlib-log.json}"
contract_only=false
case "${1:-}" in
    --contract-only) contract_only=true ;;
    "") ;;
    *) echo 'usage: stdlib-log-test-test.sh [--contract-only]' >&2; exit 1 ;;
esac
tmp="$(mktemp -d "${TMPDIR:-/tmp}/tondo-log-testing.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
reject() {
    if "$@" > "$tmp/refusal.log" 2>&1; then
        echo 'std.log testing: invalid input unexpectedly accepted' >&2; exit 1
    fi
}
bash scripts/stdlib-log-test-check.sh
# Validate a ready fixture before making negative state mutations, so a
# verified live register cannot turn an intended mutation into a no-op.
jq 'del(.measurement) | .model.status="ready" | .model.quality_gate="pending-current-source-proof"
    | .promotion.next_blocks=["STD-LOG-TEST-001"]' "$parent" > "$tmp/ready-parent.json"
jq '.status="ready" | .quality_gate="pending-current-source-proof"
    | .promotion.test_boundary_promoted=false' "$contract" > "$tmp/ready-child.json"
env TONDO_STDLIB_LOG_CONTRACT="$tmp/ready-parent.json" \
    TONDO_STDLIB_LOG_TEST_CONTRACT="$tmp/ready-child.json" bash scripts/stdlib-log-test-check.sh
while IFS= read -r mutation; do
    jq "$mutation" "$tmp/ready-child.json" > "$tmp/invalid.json"
    reject env TONDO_STDLIB_LOG_CONTRACT="$tmp/ready-parent.json" \
        TONDO_STDLIB_LOG_TEST_CONTRACT="$tmp/invalid.json" bash scripts/stdlib-log-test-check.sh
done <<'MUTATIONS'
.model.production_imports=true
.model.independent=false
.model.outside_domain="ResourceLimit"
.model.float_rendering="independent-numeric-formatting"
.model.timestamp_validation="clock-provider"
.model.unit_tests=13
.limits.max_reference_nodes=129
.limits.max_reference_height=21
.limits.max_reference_record_bytes=4096
.limits.max_fuzz_steps=513
.limits.model_seed_count=1
.test.integration_tests=8
.test.providers="external-service"
.test.process_scope="none"
.test.reference_queue_terminal_owners=1
.test.writer_close_count=0
.fuzz.smoke.result="pending"
.fuzz.smoke.runs=1
.fuzz.minimal_dependency_graph="compiler-enabled"
.fuzz.sanitizer="none"
.promotion.native_aot="verified"
.promotion.public_api_promoted=true
.promotion.test_boundary_promoted=true
.quality_gate="verified-80-percent-per-scope"
.status="verified"
.promotion.next_blocks=[]
MUTATIONS
while IFS= read -r mutation; do
    jq "$mutation" crates/tondo-reliability/tests/fixtures/log-cases.json > "$tmp/corpus.json"
    reject env TONDO_STDLIB_LOG_TEST_CORPUS="$tmp/corpus.json" bash scripts/stdlib-log-test-check.sh
done <<'MUTATIONS'
.cases |= .[0:33]
.cases[1].id=.cases[0].id
.cases[0].operation="unknown"
.cases[0].format="Binary"
.cases[0].event.message=0
.cases[0].event.fields[0]=["bad",{"kind":"Future"}]
.cases[0].event.fields[0]=["bad",{"kind":"Int","value":"9223372036854775808"}]
.cases[0].event.fields[0]=["bad",{"kind":"UInt","value":"18446744073709551616"}]
.cases[0].event.fields[0]=["bad",{"kind":"Bytes","value":"f"}]
.cases[0].event.fields[0]=["bad",{"kind":"Bool","value":"true"}]
.cases[0].event.timestamp="ambient-now"
.cases[0].event.timestamp="2000-02-30T12:34:56Z"
.cases[0].event.extra=true
.cases[0].record="missing-lf"
.cases[0].record="two\nlines\n"
.cases[0].error="Host"
.cases[0].limits={event:-1}
.cases[0].limits={event:1e100}
.cases[0].limits={unknown:1}
.cases[0].extra=true
.cases[0]=0
.cases="invalid"
MUTATIONS
for host in pending-STD-LOG-HOST-001 verified-production-hosted; do
    for model in ready verified; do
        jq --arg host "$host" --arg model "$model" '
          del(.measurement) | .implementation.host=$host | .model.status=$model
          | .model.quality_gate=(if $model=="ready" then "pending-current-source-proof"
            else "verified-80-percent-per-scope" end)
          | .promotion.next_blocks=(if $host=="pending-STD-LOG-HOST-001"
            then ["STD-LOG-HOST-001"] elif $model=="ready"
            then ["STD-LOG-TEST-001"] else ["STD-LOG-PERF-001"] end)
        ' "$parent" > "$tmp/parent.json"
        jq --arg host "$host" '.promotion.host_sinks=$host' \
            testing/stdlib-log-implementation.json > "$tmp/core.json"
        jq --arg host "$host" '.status=(if $host=="pending-STD-LOG-HOST-001"
            then "implementation-in-progress" else "verified-production-hosted" end)
          | .quality_gate=(if $host=="pending-STD-LOG-HOST-001"
            then "pending-current-source-proof" else "verified-80-percent-per-scope" end)
        ' testing/stdlib-log-host.json > "$tmp/host.json"
        jq --arg model "$model" '.status=$model
          | .quality_gate=(if $model=="ready" then "pending-current-source-proof"
            else "verified-80-percent-per-scope" end)
          | .promotion.test_boundary_promoted=($model=="verified")
        ' "$contract" > "$tmp/child.json"
        command=(env TONDO_STDLIB_LOG_CONTRACT="$tmp/parent.json"
            TONDO_STDLIB_LOG_IMPLEMENTATION_CONTRACT="$tmp/core.json"
            TONDO_STDLIB_LOG_HOST_CONTRACT="$tmp/host.json"
            TONDO_STDLIB_LOG_TEST_CONTRACT="$tmp/child.json"
            bash scripts/stdlib-log-test-check.sh)
        if [[ "$host" == pending-STD-LOG-HOST-001 && "$model" == verified ]]; then
            reject "${command[@]}"
        else
            "${command[@]}" >/dev/null
        fi
    done
done
jq '.implementation.status="core-implementation-in-progress"
    | .promotion.next_blocks=["STD-LOG-IMPL-001"]' "$tmp/ready-parent.json" > "$tmp/pending-core-parent.json"
jq '.status="implementation-in-progress" | .quality_gate="pending-80-percent-per-scope"
    | .promotion.compiler_api="registered-public-hosted-core-pending-quality"
    ' testing/stdlib-log-implementation.json > "$tmp/pending-core.json"
reject env TONDO_STDLIB_LOG_CONTRACT="$tmp/pending-core-parent.json" \
    TONDO_STDLIB_LOG_IMPLEMENTATION_CONTRACT="$tmp/pending-core.json" \
    TONDO_STDLIB_LOG_TEST_CONTRACT="$tmp/ready-child.json" bash scripts/stdlib-log-test-check.sh
mkdir "$tmp/no-rg" "$tmp/no-grep"
for utility in bash env jq tail cmp grep dirname; do
    utility_path="$(command -v "$utility")"
    ln -s "$utility_path" "$tmp/no-rg/$utility"
    [[ "$utility" == grep ]] || ln -s "$utility_path" "$tmp/no-grep/$utility"
done
env PATH="$tmp/no-rg" bash scripts/stdlib-log-test-check.sh
reject env PATH="$tmp/no-grep" bash scripts/stdlib-log-test-check.sh
if "$contract_only"; then
    echo 'std.log testing contract: OK (invalid schemas, coherent promotion and required inspection tools)'
    exit 0
fi
graph="$tmp/minimal-graph.txt"
cargo tree --manifest-path fuzz/Cargo.toml --no-default-features --locked -e normal --prefix none -p tondo-fuzz > "$graph"
if grep -Eq '^tondo-(compiler|vm|conformance|reliability) ' "$graph"; then
    echo 'std.log testing: minimal fuzz graph imports a hosted dependency' >&2; exit 1
else [[ "$?" == 1 ]] || exit 1; fi
cargo test -p tondo-reliability --locked --lib log_model
cargo test -p tondo-reliability --locked --test log_kernel_models --test log_corpus --test log_hosted_models
cargo test -p tondo-stdlib --locked --lib log::
echo 'std.log testing: OK (independent reference, exact regressions, hosted delivery and invalid promotions)'
