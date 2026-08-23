#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_S1A_SEAL_CONTRACT:-$root/testing/stdlib-s1a-seal.json}"
seal_dir="${TONDO_STDLIB_S1A_SEAL_DIR:-$root/target/reliability/evidence/stdlib-s1a-seal}"

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
  and .scope == "stdlib-foundation-hosted-draft-only"
  and .bundle.format == "tondo-stdlib-s1a-seal/1"
  and .bundle.archive_format == "tar-ustar"
  and .bundle.source_policy == "clean-git-head"
  and .bundle.content_address == "payload-sha256"
  and .bundle.public_release == false
  and (.bundle.separate_from | sort) == ["G5", "N1", "TLF"]
  and (.required_inputs | type == "array" and length > 0)
  and ((.required_inputs | unique | length) == (.required_inputs | length))
  and (.required_evidence | type == "array" and length == 5)
  and (.required_checks | type == "array" and length > 0)
  and ([.requirements[].id] | sort) == [
    "ASYNC-SELECT-VM-CONF-001", "STD-A-ASYNC-IMPL-001", "STD-A-CONF-001",
    "STD-A-DIST-001", "STD-A-FUZZ-001", "STD-A-PERF-001",
    "STD-A-SELECTABLE-IMPL-001"
  ]
  and .invariants.public_api.signatures == 214
  and .invariants.public_api.gaps == 0
  and .invariants.matrix == {owners:22,requirements:171,rows:385,open_rows:0,status:"verified",applicable_open_cells:0}
  and .invariants.fuzz == {owners:22,verified:22,partial:0}
  and .invariants.performance == {captured_owners:10,not_applicable_owners:12,deferred_dimensions:[]}
  and .invariants.conformance == {owners:22,rows:385,cases:206,passed:true}
  and .invariants.distribution == {clean_source_workspaces:2,byte_identical:true,public_release:false}
  and .invariants.claims == {g5:false,native_backend:false,tlf:false,public_release:false}
  and (.negative_cases | sort) == [
    "archive-payload-mismatch", "bundle-manifest-mismatch", "dirty-workspace",
    "distribution-not-reproducible", "evidence-revision-drift", "g5-claim",
    "matrix-open-cell", "native-backend-claim", "performance-deferred-dimension",
    "public-api-gap", "tlf-claim"
  ]
  and .next_blocks == ["DIAG-SPEC-001"]
  and .public_release == false
' "$contract" >/dev/null || die "invalid seal contract"

seal="$seal_dir/seal.json"
manifest="$seal_dir/bundle/tondo-stdlib-s1a/metadata/manifest.json"
[[ -f "$seal" ]] || die "missing generated seal: ${seal#"$root"/}"
[[ -f "$manifest" ]] || die "missing bundle manifest: ${manifest#"$root"/}"

jq -e '
  .format == "tondo-stdlib-s1a-seal/1"
  and .status == "sealed-draft"
  and .task == "STD-S1A-SEAL-001"
  and .edition == "0.1"
  and .phase == "STD-0.1A"
  and .manifest == "bundle/tondo-stdlib-s1a/metadata/manifest.json"
  and (.revision | test("^[0-9a-f]{40}$"))
  and (.source_tree_sha256 | test("^[0-9a-f]{64}$"))
  and (.bundle_id | test("^tondo-stdlib-s1a-[0-9a-f]{64}$"))
  and (.manifest_sha256 | test("^[0-9a-f]{64}$"))
  and (.payload_sha256 | test("^[0-9a-f]{64}$"))
  and (.archive | test("^tondo-stdlib-s1a-[0-9a-f]{64}\\.tar$"))
  and (.archive_sha256 | test("^[0-9a-f]{64}$"))
  and .public_release == false
  and .g5_claim == false
  and .native_backend_claim == false
  and .tlf_claim == false
  and .next_block == "DIAG-SPEC-001"
' "$seal" >/dev/null || die "invalid generated seal metadata"

jq -e '
  .format == "tondo-stdlib-s1a-bundle/1"
  and .task == "STD-S1A-SEAL-001"
  and .edition == "0.1"
  and .phase == "STD-0.1A"
  and .root == "tondo-stdlib-s1a"
  and (.files | type == "array" and length > 0)
  and ([.files[].path] | sort) == [.files[].path]
  and ([.files[].path] | unique | length) == (.files | length)
  and all(.files[];
    (.path | type == "string" and length > 0 and (startswith("/") | not) and (contains("..") | not))
    and (.sha256 | test("^[0-9a-f]{64}$"))
    and (.bytes | type == "number" and . >= 0)
    and (.origin | type == "string" and length > 0)
  )
  and .claims == {g5:false,native_backend:false,tlf:false,public_release:false}
' "$manifest" >/dev/null || die "invalid bundle manifest"

archive_name="$(jq -r '.archive' "$seal")"
archive="$seal_dir/$archive_name"
[[ -f "$archive" ]] || die "missing bundle archive: ${archive#"$root"/}"
archive_sha256="$(sha256sum "$archive" | cut -d' ' -f1)"
expected_archive_sha256="$(jq -r '.archive_sha256' "$seal")"
[[ "$archive_sha256" == "$expected_archive_sha256" ]] || die "bundle archive hash mismatch"

manifest_sha256="$(sha256sum "$manifest" | cut -d' ' -f1)"
expected_manifest_sha256="$(jq -r '.manifest_sha256' "$seal")"
[[ "$manifest_sha256" == "$expected_manifest_sha256" ]] || die "bundle manifest hash mismatch"

payload_hash="$(jq -r '.files[] | [.path, .sha256, (.bytes | tostring)] | @tsv' "$manifest" | sha256sum | cut -d' ' -f1)"
expected_payload_hash="$(jq -r '.payload_sha256' "$seal")"
[[ "$payload_hash" == "$expected_payload_hash" ]] || die "bundle payload hash mismatch"

bundle_id="$(jq -r '.bundle_id' "$seal")"
[[ "$bundle_id" == "tondo-stdlib-s1a-$payload_hash" ]] || die "bundle ID is not content-addressed"

mapfile -t archive_files < <(tar --format=ustar -tf "$archive" | awk 'index($0, "tondo-stdlib-s1a/") == 1 && $0 !~ /\/$/')
mapfile -t manifest_files < <(jq -r '.files[].path' "$manifest" | sed 's#^#tondo-stdlib-s1a/#')
mapfile -t expected_archive_files < <(
    {
        printf '%s\n' "${manifest_files[@]}"
        printf '%s\n' "tondo-stdlib-s1a/metadata/manifest.json"
    } | LC_ALL=C sort
)
[[ "${archive_files[*]}" == "${expected_archive_files[*]}" ]] || die "archive file set differs from the canonical manifest"

bundle_root="$seal_dir/bundle/tondo-stdlib-s1a"
while IFS=$'\t' read -r path expected_sha expected_bytes; do
    file="$bundle_root/$path"
    [[ -f "$file" ]] || die "bundle payload is missing: $path"
    actual_sha="$(sha256sum "$file" | cut -d' ' -f1)"
    [[ "$actual_sha" == "$expected_sha" ]] || die "bundle payload hash mismatch: $path"
    actual_bytes="$(wc -c <"$file" | tr -d '[:space:]')"
    [[ "$actual_bytes" == "$expected_bytes" ]] || die "bundle payload size mismatch: $path"
    archive_sha="$(tar --format=ustar -xOf "$archive" "tondo-stdlib-s1a/$path" | sha256sum | cut -d' ' -f1)"
    [[ "$archive_sha" == "$expected_sha" ]] || die "archive payload hash mismatch: $path"
done < <(jq -r '.files[] | [.path, .sha256, (.bytes | tostring)] | @tsv' "$manifest")

echo "stdlib S1A seal: verified (content-addressed draft bundle; G5/N1/TLF/public-release claims disabled)"
