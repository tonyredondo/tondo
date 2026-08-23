#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_CONFORMANCE_CONTRACT:-testing/stdlib-conformance.json}"
evidence_dir="${TONDO_STDLIB_EVIDENCE_DIR:-target/reliability/evidence}"
logs_dir="$evidence_dir/stdlib-conformance-logs"
result="$evidence_dir/stdlib-conformance.json"
mkdir -p "$evidence_dir" "$logs_dir"

die() {
    echo "stdlib conformance: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing contract: ${contract#"$root/"}"
[[ -f "$evidence_dir/layer-evidence.json" ]] || die "missing current layer evidence; run the test gate first"

jq -e '
  .format == "tondo-stdlib-conformance/1"
  and .edition == "0.1"
  and .phase == "STD-0.1A"
  and .status == "promoted"
  and .runner.lineage == "draft"
  and .runner.full_suite_case_count == 206
  and (.owners | type == "array" and length == 22)
  and ([.owners[].id] | unique | length) == 22
  and all(.owners[]; .status == "verified" and (.owner_command | startswith("scripts/")) and (.cases | length > 0))
' "$contract" >/dev/null || die "invalid public conformance contract"

current_tree_sha256="$(cargo run -p tondo-reliability --locked -- quality provenance --root . | jq -r '.tree_sha256')"
layer_tree_sha256="$(jq -r '.tree_sha256' "$evidence_dir/layer-evidence.json")"
[[ "$current_tree_sha256" == "$layer_tree_sha256" ]] || die "layer evidence is stale for the current source tree"

command_results="$(mktemp "$evidence_dir/stdlib-conformance-commands.XXXXXX")"
case_results="$(mktemp "$evidence_dir/stdlib-conformance-cases.XXXXXX")"
trap 'rm -f "$command_results" "$case_results"' EXIT

record_command() {
    local id="$1"
    local log="$2"
    local command_text="$3"
    local sha
    sha="$(sha256sum "$log" | cut -d' ' -f1)"
    jq -n \
        --arg id "$id" \
        --arg status "passed" \
        --arg log "${log#"$root/"}" \
        --arg command "$command_text" \
        --arg sha "$sha" \
        '{id:$id,status:$status,log:$log,command:$command,log_sha256:$sha}' >> "$command_results"
}

run_logged() {
    local id="$1"
    shift
    local log="$logs_dir/$id.log"
    local command_text
    printf -v command_text '%q ' "$@"
    if "$@" >"$log" 2>&1; then
        record_command "$id" "$log" "${command_text% }"
    else
        cat "$log" >&2
        die "command failed: $id"
    fi
}

run_runtime_case() {
    local id="$1"
    local source="$2"
    local base="${source%.to}"
    local out="$logs_dir/$id.stdout"
    local err="$logs_dir/$id.stderr"
    local log="$logs_dir/$id.log"
    local expected_exit
    local actual_exit
    local args_file
    local -a program_args=()
    if [[ "${OSTYPE:-}" == msys* || "${OSTYPE:-}" == cygwin* || "${OSTYPE:-}" == win32* ]]; then
        args_file="$base.args-windows"
    else
        args_file="$base.args-unix"
    fi
    if [[ -f "$args_file" ]]; then
        while IFS= read -r argument || [[ -n "$argument" ]]; do
            program_args+=("$argument")
        done <"$args_file"
    fi
    local -a cli=(cargo run -p tondo-cli --locked --quiet -- run "$source" -- "${program_args[@]}")
    local command_text
    printf -v command_text '%q ' "${cli[@]}"
    command_text="${command_text% } </dev/null"
    expected_exit="$(tr -d '[:space:]' <"$base.exit")"
    set +e
    "${cli[@]}" </dev/null >"$out" 2>"$err"
    actual_exit=$?
    set -e
    cat "$err" >"$log"
    printf '\n--- stdout ---\n' >>"$log"
    cat "$out" >>"$log"
    [[ "$actual_exit" == "$expected_exit" ]] || {
        cat "$log" >&2
        die "runtime case $id returned $actual_exit, expected $expected_exit"
    }
    if [[ -f "$base.stdout" ]]; then
        cmp -s "$out" "$base.stdout" || {
            cat "$log" >&2
            die "runtime case $id stdout differs from its sidecar"
        }
    elif [[ -f "$base.codes" ]]; then
        grep -Fxq "$actual_exit" "$base.codes" || die "runtime case $id has no accepted exit code"
    else
        die "runtime case $id has no exit or output sidecar"
    fi
    record_command "case-$id" "$log" "$command_text"
    jq -n --arg id "$id" --arg source "$source" --arg status "passed" --arg log "${log#"$root/"}" \
        '{id:$id,source:$source,status:$status,log:$log}' >> "$case_results"
}

run_logged draft-validate \
    cargo run -p tondo-conformance --locked -- validate \
    --root . --manifest conformance/draft/manifest.json --lineage draft
run_logged draft-run \
    cargo run -p tondo-conformance --locked -- run \
    --root . --manifest conformance/draft/manifest.json --lineage draft \
    --adapter target/debug/tondo-reference-adapter \
    --evidence "$evidence_dir/layer-evidence.json" \
    --output "$evidence_dir/conformance-result.json"

while IFS=$'\t' read -r owner command; do
    run_logged "owner-${owner#std.}" "$root/$command"
done < <(jq -r '.owners[] | [.id, .owner_command] | @tsv' "$contract")

while IFS=$'\t' read -r id source; do
    run_runtime_case "$id" "$source"
# Several owner rows intentionally point at one shared public fixture (the
# codec fixture is the current example).  Execute each source once; the
# owner contract still records every case-to-owner relationship.
done < <(jq -r '.owners[].cases[] | select(.kind == "runtime") | [.id, .source] | @tsv' "$contract" \
    | sort -t $'\t' -k2,2 -u)

while IFS= read -r command; do
    [[ -n "$command" ]] || continue
    run_logged "case-command-$(printf '%s' "$command" | sha256sum | cut -c1-12)" bash -lc "$command"
done < <(jq -r '.owners[].cases[] | select(.kind != "runtime") | .command' "$contract" | sort -u)

revision="$(git rev-parse HEAD)"
contract_sha256="$(sha256sum "$contract" | cut -d' ' -f1)"
manifest_sha256="$(sha256sum conformance/0.1/manifest.json | cut -d' ' -f1)"
result_sha256="$(sha256sum "$evidence_dir/conformance-result.json" | cut -d' ' -f1)"
jq -e \
    --arg revision "$revision" \
    --arg manifest_sha256 "$manifest_sha256" \
    --arg result_sha256 "$result_sha256" \
    '.format == "tondo-conformance-result-draft/2"
     and .suite == "tondo-conformance-draft"
     and .edition == "0.1"
     and .manifest_sha256 == $manifest_sha256
     and .passed == true
     and (.cases | length) == 206' "$evidence_dir/conformance-result.json" >/dev/null || die "draft suite result is not a passed 206-case observation"

commands_json="$(jq -s '.' "$command_results")"
cases_json="$(jq -s '.' "$case_results")"
jq -n \
    --slurpfile contract "$contract" \
    --arg revision "$revision" \
    --arg current_tree_sha256 "$current_tree_sha256" \
    --arg manifest_sha256 "$manifest_sha256" \
    --arg result_sha256 "$result_sha256" \
    --arg contract_sha256 "$contract_sha256" \
    --argjson commands "$commands_json" \
    --argjson cases "$cases_json" \
    --slurpfile result "$evidence_dir/conformance-result.json" \
    '
      ($contract[0]) as $contract
      | {
          format: "tondo-stdlib-conformance-evidence/1",
          edition: "0.1",
          phase: "STD-0.1A",
          status: "passed",
          revision: $revision,
          tree_sha256: $current_tree_sha256,
          contract_sha256: $contract_sha256,
          manifest_sha256: $manifest_sha256,
          full_suite: {
            cases: ($result[0].cases | length),
            passed: $result[0].passed,
            result_sha256: $result_sha256
          },
          commands: $commands,
          cases: $cases,
          owners: ($contract.owners | map({
            id,
            status: "passed",
            rows,
            case_ids: (.cases | map(.id)),
            owner_command,
            refs
          }))
        }
    ' > "$result"

echo "stdlib conformance: OK (22 owners; 385 rows; 206 draft cases; runtime sidecars compared; report: ${result#"$root/"})"
