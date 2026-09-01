#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_S1A_SEAL_CONTRACT:-$root/testing/stdlib-s1a-seal.json}"
output_dir="${TONDO_STDLIB_S1A_SEAL_DIR:-$root/target/reliability/evidence/stdlib-s1a-seal}"

die() {
    echo "stdlib S1A seal: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing seal contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "seal contract must end with one LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "seal contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-s1a-seal-contract/1"
  and .owner == "toolchain.std_s1a_seal"
  and .edition == "0.1"
  and .phase == "STD-0.1A"
  and .status == "closed-contract"
  and .bundle.format == "tondo-stdlib-s1a-seal/1"
  and .bundle.public_release == false
  and (.bundle.separate_from | sort) == ["G5", "N1", "TLF"]
  and .public_release == false
' "$contract" >/dev/null || die "invalid seal contract"

if [[ -n "$(git status --porcelain=v1 --untracked-files=all)" ]]; then
    die "workspace must be clean before sealing"
fi

while IFS= read -r input; do
    [[ -n "$input" ]] || continue
    [[ "$input" != /* && "$input" != *..* ]] || die "invalid input path in contract: $input"
    [[ -f "$root/$input" ]] || die "missing required input: $input"
done < <(jq -r '.required_inputs[]' "$contract")

revision="$(git rev-parse HEAD)"
tree_sha256="$(cargo run -p tondo-reliability --locked -- quality provenance --root . | jq -r '.tree_sha256')"
[[ "$revision" =~ ^[0-9a-f]{40}$ ]] || die "invalid Git revision"
[[ "$tree_sha256" =~ ^[0-9a-f]{64}$ ]] || die "invalid reliability tree hash"

mkdir -p "$root/.tmp"
work="$(mktemp -d "$root/.tmp/tondo-stdlib-s1a-seal.XXXXXX")"
trap 'rm -rf -- "$work"' EXIT
logs="$work/logs"
commands_ndjson="$work/commands.ndjson"
mkdir -p "$logs"
: > "$commands_ndjson"

run_check() {
    local command="$1"
    local id="$2"
    local log="$logs/$id.log"
    if bash -lc "$command" >"$log" 2>&1; then
        jq -cn --arg id "$id" --arg command "$command" --arg status "passed" \
            --arg log "commands/$id.log" --arg sha256 "$(sha256sum "$log" | cut -d' ' -f1)" \
            '{id:$id,command:$command,status:$status,log:$log,log_sha256:$sha256}' >> "$commands_ndjson"
    else
        cat "$log" >&2
        die "required check failed: $command"
    fi
}

check_index=0
while IFS= read -r command; do
    [[ -n "$command" ]] || continue
    check_index=$((check_index + 1))
    printf -v id 'check-%02d' "$check_index"
    run_check "$command" "$id"
done < <(jq -r '.required_checks[]' "$contract")

public_api="testing/stdlib-public-api.json"
matrix="testing/stdlib-matrix.json"
evidence="testing/stdlib-owner-evidence.json"
evidence_root="${TONDO_STDLIB_EVIDENCE_DIR:-target/reliability/evidence}"
conformance="$evidence_root/stdlib-conformance.json"
distribution="$evidence_root/stdlib-distribution/stdlib-distribution.json"
performance="$evidence_root/stdlib-performance-report.json"
async_select="$evidence_root/async-select-conformance.json"
async_perf="$evidence_root/async-select-performance.json"

jq -e '
  .format == "tondo-stdlib-public-api-audit/1"
  and .status == "verified"
  and .summary.signatures == 214
  and .summary.verified == 214
  and .summary.gaps == 0
' "$public_api" >/dev/null || die "public API audit is not strict 214/214"

jq -e '
  .format == "tondo-stdlib-normative-matrix/1"
  and .status == "verified"
  and .summary.owners == 22
  and .summary.requirements == 171
  and .summary.rows == 385
  and .summary.open_rows == 0
  and all(.rows[]; .status == "verified")
  and all(.owners[].stages[]; .status == "verified" or .status == "not-applicable")
' "$matrix" >/dev/null || die "normative matrix has an open applicable cell"

jq -e '
  .format == "tondo-stdlib-owner-evidence/1"
  and .status == "promoted-evidence"
  and (.owners | length) == 22
  and all(.owners[]; .cells.FUZZ.status == "verified" and .cells.FUZZ.reason == null)
' "$evidence" >/dev/null || die "FUZZ evidence is not promoted for all owners"

jq -e \
    --arg revision "$revision" \
    --arg tree "$tree_sha256" \
    '.format == "tondo-stdlib-conformance-evidence/1"
     and .status == "passed"
     and .revision == $revision
     and .tree_sha256 == $tree
     and .full_suite.passed == true
     and .full_suite.cases == 206
     and (.owners | length) == 22' \
    "$conformance" >/dev/null || die "conformance evidence is stale or incomplete"

jq -e \
    --arg revision "$revision" \
    '.format == "tondo-stdlib-performance-report/1"
     and .git_revision == $revision
     and (.measurements | length) == 60' \
    "$performance" >/dev/null || die "performance evidence is stale or incomplete"

jq -e \
    --arg revision "$revision" \
    '.format == "tondo-async-select-conformance/1"
     and .status == "passed"
     and .revision == $revision
     and .suite_case_count == 206
     and (.selected_cases | length) == 3' \
    "$async_select" >/dev/null || die "async-select conformance evidence is stale or incomplete"

jq -e \
    --arg revision "$revision" \
    '.format == "tondo-async-select-performance-report/1"
     and .git_revision == $revision
     and (.measurements | length) > 0' \
    "$async_perf" >/dev/null || die "async-select performance evidence is stale or incomplete"

resolve_path() {
    case "$1" in
        /*) printf '%s\n' "$1" ;;
        *) printf '%s/%s\n' "$root" "$1" ;;
    esac
}

dist_archive="$(jq -r '.archive' "$distribution")"
dist_archive_path="$(resolve_path "$dist_archive")"
[[ -f "$dist_archive_path" ]] || die "distribution archive is missing: $dist_archive"
dist_archive_sha256="$(sha256sum "$dist_archive_path" | cut -d' ' -f1)"
jq -e \
    --arg archive_sha256 "$dist_archive_sha256" \
    '.format == "tondo-stdlib-distribution-evidence/1"
     and .status == "promoted-draft"
     and .package_id == "toolchain:std:0.1-bootstrap"
     and .public_release == false
     and .byte_identical == true
     and .clean_source_workspaces == 2
     and .archive_sha256 == $archive_sha256' \
    "$distribution" >/dev/null || die "distribution evidence is not reproducible draft evidence"

jq -e '
  .format == "tondo-stdlib-owner-contract/1"
  and .owner == "std.async"
  and .implementation.status == "verified"
  and .promotion.implementation_pending == []
' testing/stdlib-async.json >/dev/null || die "std.async implementation is not closed"
jq -e '
  .format == "tondo-async-select-conformance/1"
  and .status == "closed"
  and .task == "ASYNC-SELECT-VM-CONF-001"
' testing/async-select-conformance.json >/dev/null || die "selectable implementation contract is not closed"
jq -e '
  .format == "tondo-stdlib-performance-conformance/1"
  and .status == "promoted"
  and .deferred_dimensions == []
' testing/stdlib-performance-conformance.json >/dev/null || die "performance coordinator is not promoted"
jq -e '
  .format == "tondo-stdlib-conformance/1"
  and .status == "promoted"
  and .summary == {owners:22,signatures:214,requirements:171,rows:385,cases:32}
' testing/stdlib-conformance.json >/dev/null || die "conformance registry is not promoted"

stage="$work/tondo-stdlib-s1a"
mkdir -p "$stage/metadata" "$stage/inputs" "$stage/evidence" "$stage/distribution" "$stage/commands"

copy_input() {
    local path="$1"
    mkdir -p "$stage/inputs/$(dirname "$path")"
    cp -- "$root/$path" "$stage/inputs/$path"
}

while IFS= read -r input; do
    [[ -n "$input" ]] || continue
    copy_input "$input"
done < <(jq -r '.required_inputs[]' "$contract")

copy_evidence() {
    local source="$1" destination="$2"
    mkdir -p "$stage/evidence/$(dirname "$destination")"
    cp -- "$(resolve_path "$source")" "$stage/evidence/$destination"
}

copy_evidence "$evidence_root/stdlib-conformance.json" "stdlib-conformance.json"
copy_evidence "$evidence_root/stdlib-performance-report.json" "stdlib-performance-report.json"
copy_evidence "$evidence_root/async-select-conformance.json" "async-select-conformance.json"
copy_evidence "$evidence_root/async-select-performance.json" "async-select-performance.json"
copy_evidence "$evidence_root/conformance-result.json" "conformance-result.json"
copy_evidence "$evidence_root/layer-evidence.json" "layer-evidence.json"
copy_evidence "$evidence_root/stdlib-distribution/stdlib-distribution.json" "stdlib-distribution.json"
mkdir -p "$stage/evidence/stdlib-conformance-logs"
cp -a -- "$evidence_root/stdlib-conformance-logs/." "$stage/evidence/stdlib-conformance-logs/"
cp -- "$(resolve_path "$evidence_root/stdlib-distribution/tondo-std-0.1.tar")" \
    "$stage/distribution/tondo-std-0.1.tar"

while IFS= read -r log; do
    [[ -n "$log" ]] || continue
    cp -- "$logs/$log" "$stage/commands/$log"
done < <(find "$logs" -maxdepth 1 -type f -printf '%f\n' | LC_ALL=C sort)

jq -s '.' "$commands_ndjson" > "$stage/metadata/commands.json"
jq -S -n \
    --arg revision "$revision" \
    --arg tree_sha256 "$tree_sha256" \
    --arg contract_sha256 "$(sha256sum "$contract" | cut -d' ' -f1)" \
    --argjson commands "$(cat "$stage/metadata/commands.json")" \
    ' {
        format: "tondo-stdlib-s1a-verification/1",
        task: "STD-S1A-SEAL-001",
        edition: "0.1",
        phase: "STD-0.1A",
        revision: $revision,
        source_tree_sha256: $tree_sha256,
        contract_sha256: $contract_sha256,
        checks: $commands,
        claims: {g5:false,native_backend:false,tlf:false,public_release:false}
      }' > "$stage/metadata/verification.json"

entries_ndjson="$work/entries.ndjson"
: > "$entries_ndjson"
while IFS= read -r file; do
    relative="${file#"$stage"/}"
    case "$relative" in
        inputs/*) origin="tracked:${relative#inputs/}" ;;
        evidence/*) origin="evidence:${relative#evidence/}" ;;
        distribution/*) origin="evidence:stdlib-distribution/tondo-std-0.1.tar" ;;
        commands/*) origin="seal-command" ;;
        metadata/*) origin="generated:$relative" ;;
        *) die "unexpected bundle path: $relative" ;;
    esac
    sha256="$(sha256sum "$file" | cut -d' ' -f1)"
    bytes="$(wc -c <"$file" | tr -d '[:space:]')"
    jq -cn --arg path "$relative" --arg sha256 "$sha256" --arg origin "$origin" --argjson bytes "$bytes" \
        '{path:$path,sha256:$sha256,bytes:$bytes,origin:$origin}' >> "$entries_ndjson"
done < <(find "$stage" -type f ! -path "$stage/metadata/manifest.json" -print | LC_ALL=C sort)

entries_json="$(jq -s 'sort_by(.path)' "$entries_ndjson")"
payload_sha256="$(jq -r '.[] | [.path, .sha256, (.bytes | tostring)] | @tsv' <<< "$entries_json" | sha256sum | cut -d' ' -f1)"
jq -S -n \
    --arg payload_sha256 "$payload_sha256" \
    --arg revision "$revision" \
    --arg tree_sha256 "$tree_sha256" \
    --argjson files "$entries_json" \
    ' {
        format: "tondo-stdlib-s1a-bundle/1",
        task: "STD-S1A-SEAL-001",
        edition: "0.1",
        phase: "STD-0.1A",
        root: "tondo-stdlib-s1a",
        revision: $revision,
        source_tree_sha256: $tree_sha256,
        payload_sha256: $payload_sha256,
        claims: {g5:false,native_backend:false,tlf:false,public_release:false},
        files: $files
      }' > "$stage/metadata/manifest.json"

manifest_sha256="$(sha256sum "$stage/metadata/manifest.json" | cut -d' ' -f1)"
bundle_id="tondo-stdlib-s1a-$payload_sha256"
mkdir -p "$output_dir"
rm -rf -- "$output_dir/bundle" "$output_dir/$bundle_id.tar"
mkdir -p "$output_dir/bundle"
cp -a -- "$stage" "$output_dir/bundle/tondo-stdlib-s1a"
archive="$output_dir/$bundle_id.tar"
(
    cd "$output_dir/bundle"
    tar --format=ustar --sort=name --mtime='UTC 1970-01-01' \
        --owner=0 --group=0 --numeric-owner -cf "$archive" tondo-stdlib-s1a
)
archive_sha256="$(sha256sum "$archive" | cut -d' ' -f1)"

jq -S -n \
    --arg revision "$revision" \
    --arg tree_sha256 "$tree_sha256" \
    --arg bundle_id "$bundle_id" \
    --arg manifest "bundle/tondo-stdlib-s1a/metadata/manifest.json" \
    --arg manifest_sha256 "$manifest_sha256" \
    --arg payload_sha256 "$payload_sha256" \
    --arg archive "$(basename "$archive")" \
    --arg archive_sha256 "$archive_sha256" \
    ' {
        format: "tondo-stdlib-s1a-seal/1",
        status: "sealed-draft",
        task: "STD-S1A-SEAL-001",
        edition: "0.1",
        phase: "STD-0.1A",
        revision: $revision,
        source_tree_sha256: $tree_sha256,
        bundle_id: $bundle_id,
        manifest: $manifest,
        manifest_sha256: $manifest_sha256,
        payload_sha256: $payload_sha256,
        archive: $archive,
        archive_sha256: $archive_sha256,
        public_release: false,
        g5_claim: false,
        native_backend_claim: false,
        tlf_claim: false,
        next_block: "DIAG-SPEC-001"
      }' > "$output_dir/seal.json"

TONDO_STDLIB_S1A_SEAL_CONTRACT="$contract" \
TONDO_STDLIB_S1A_SEAL_DIR="$output_dir" \
    "$root/scripts/stdlib-s1a-seal-check.sh"

echo "stdlib S1A seal: OK (draft bundle $bundle_id; archive $archive_sha256)"
