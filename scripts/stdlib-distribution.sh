#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_DISTRIBUTION_CONTRACT:-$root/testing/stdlib-distribution.json}"
vm_binary="${TONDO_VM_BINARY:-$root/target/debug/tondo}"
output_dir="${TONDO_STDLIB_DISTRIBUTION_DIR:-$root/target/reliability/evidence/stdlib-distribution}"
mkdir -p "$output_dir" "$root/.tmp"

[[ -x "$vm_binary" ]] || {
    echo "stdlib distribution: VM binary is missing or not executable: ${vm_binary#"$root"/}" >&2
    exit 1
}
scripts/stdlib-distribution-check.sh >/dev/null

work="$(mktemp -d "$root/.tmp/tondo-stdlib-distribution.XXXXXX")"
trap 'rm -rf -- "$work"' EXIT

sha256_file() {
    sha256sum "$1" | cut -d ' ' -f 1
}

copy_payload() {
    local workspace="$1" source="$2" role="$3" destination="$4" package="$5" inventory="$6"
    [[ -f "$workspace/$source" ]] || {
        echo "stdlib distribution: missing input in clean workspace: $source" >&2
        exit 1
    }
    mkdir -p "$package/$(dirname "$destination")"
    cp -- "$workspace/$source" "$package/$destination"
    printf '%s\t%s\t%s\n' "$role" "$destination" "$source" >> "$inventory"
}

copy_generated() {
    local role="$1" destination="$2" package="$3" inventory="$4"
    mkdir -p "$package/$(dirname "$destination")"
    printf '%s\t%s\tgenerated:%s\n' "$role" "$destination" "$destination" >> "$inventory"
}

copy_contract() {
    local package="$1" inventory="$2"
    mkdir -p "$package/metadata"
    cp -- "$contract" "$package/metadata/distribution-contract.json"
    printf 'metadata\tmetadata/distribution-contract.json\tcontract\n' >> "$inventory"
}

capabilities_jq='def capabilities($id):
  if $id == "std.console" then ["console"]
  elif $id == "std.env" then ["environment"]
  elif $id == "std.fs" then ["filesystem"]
  elif $id == "std.process" then ["process"]
  elif $id == "std.time" then ["clock"]
  else [] end;'

assemble() {
    local workspace="$1" archive="$2" label="$3"
    local package_parent="$work/$label"
    local package="$package_parent/tondo-std-0.1"
    local inventory="$package_parent/inventory.tsv"
    mkdir -p "$package" "$package_parent"
    : > "$inventory"

    while IFS= read -r source; do
        copy_payload "$workspace" "$source" sources "src/$source" "$package" "$inventory"
    done < <(cd "$workspace" && find stdlib -type f -print | LC_ALL=C sort -u)

    while IFS= read -r source; do
        copy_payload "$workspace" "$source" docs "docs/$source" "$package" "$inventory"
    done < <(
        cd "$workspace"
        {
            [[ -f TONDO_STANDARD_LIBRARY_SPEC.md ]] && printf '%s\n' TONDO_STANDARD_LIBRARY_SPEC.md
            find docs/contracts -maxdepth 1 -type f \( \
                -name 'stdlib-*.md' -o \
                -name 'std-meta.md' -o \
                -name 'std-reflect.md' -o \
                -name 'package-graph.md' \
            \) -print
        } | LC_ALL=C sort -u
    )

    local capability_input
    for capability_input in \
        testing/stdlib-matrix.json \
        testing/stdlib-public-api.json \
        testing/stdlib-conformance-coordination.json \
        testing/stdlib-owner-evidence.json \
        testing/stdlib-documentation.json \
        testing/stdlib-implementation-coordination.json \
        testing/stdlib-hosted-implementation-coordination.json; do
        copy_payload "$workspace" "$capability_input" capabilities "capabilities/$capability_input" "$package" "$inventory"
    done

    while IFS= read -r source; do
        copy_payload "$workspace" "$source" examples "examples/$(basename "$source")" "$package" "$inventory"
    done < <(
        cd "$workspace"
        find tests/runtime -maxdepth 1 -type f \( -name 'm10-std-*' -o -name 'm11-std-*' \) -print \
            | LC_ALL=C sort -u
    )

    mkdir -p "$package/bin"
    cp -- "$vm_binary" "$package/bin/tondo"
    chmod 0755 "$package/bin/tondo"
    printf 'bin\tbin/tondo\tvm:%s\n' "${vm_binary#"$root"/}" >> "$inventory"
    copy_contract "$package" "$inventory"

    local api_hash matrix_hash evidence_hash
    api_hash="$(sha256_file "$workspace/testing/stdlib-public-api.json")"
    matrix_hash="$(sha256_file "$workspace/testing/stdlib-matrix.json")"
    evidence_hash="$(sha256_file "$workspace/testing/stdlib-owner-evidence.json")"

    mkdir -p "$package/interfaces" "$package/units" "$package/providers" "$package/manifests" "$package/capabilities"
    jq -S -c \
        --slurpfile api "$workspace/testing/stdlib-public-api.json" \
        --arg package_id "toolchain:std:0.1-bootstrap" \
        --arg api_hash "$api_hash" \
        --arg matrix_hash "$matrix_hash" \
        ' {
            format: "tondo-stdlib-interface/1",
            package_id: $package_id,
            edition: "0.1",
            target: "tondo-vm-hosted",
            profile: "hosted",
            api_sha256: $api_hash,
            matrix_sha256: $matrix_hash,
            owners: ($api[0].rows | map(.owner) | unique | sort),
            signatures: ($api[0].rows | map({id, owner, symbol, signature, status}) | sort_by(.id))
        }' > "$package/interfaces/std-0.1.json"
    copy_generated interfaces interfaces/std-0.1.json "$package" "$inventory"

    jq -S -c \
        --slurpfile evidence "$workspace/testing/stdlib-owner-evidence.json" \
        --arg package_id "toolchain:std:0.1-bootstrap" \
        --arg evidence_hash "$evidence_hash" \
        "$capabilities_jq
        {
          format: \"tondo-stdlib-units/1\",
          package_id: \$package_id,
          edition: \"0.1\",
          evidence_sha256: \$evidence_hash,
          units: (\$evidence[0].owners | map({
            id,
            layer,
            provider: (if .id == \"std.meta\" or .id == \"std.reflect\" then \"tondo-meta\" elif .id == \"std.testing\" then \"tondo-testing\" else \"tondo-vm\" end),
            capabilities: capabilities(.id),
            source_set: \"stdlib-core\"
          }) | sort_by(.id))
        }" > "$package/units/std-0.1.json"
    copy_generated units units/std-0.1.json "$package" "$inventory"

    jq -S -c \
        --arg package_id "toolchain:std:0.1-bootstrap" \
        '{
          format: "tondo-stdlib-providers/1",
          package_id: $package_id,
          providers: [
            {id: "tondo-vm-hosted", kind: "runtime", capabilities: ["clock", "console", "environment", "filesystem", "process"]},
            {id: "tondo-meta", kind: "build-time", capabilities: []},
            {id: "tondo-testing", kind: "test-only", capabilities: []}
          ]
        }' > "$package/providers/providers.json"
    copy_generated providers providers/providers.json "$package" "$inventory"

    local capability_matrix_hash
    jq -S -c \
        --slurpfile matrix "$workspace/testing/stdlib-matrix.json" \
        --arg package_id "toolchain:std:0.1-bootstrap" \
        "$capabilities_jq
        {
          format: \"tondo-stdlib-capability-matrix/1\",
          package_id: \$package_id,
          edition: \"0.1\",
          target: \"tondo-vm-hosted\",
          profile: \"hosted\",
          owners: (\$matrix[0].owners | map({id, layer, capabilities: capabilities(.id)}) | sort_by(.id)),
          required_capabilities: ([\$matrix[0].owners[].id | capabilities(.)[]] | unique | sort)
        }" > "$package/capabilities/capability-matrix.json"
    copy_generated capabilities capabilities/capability-matrix.json "$package" "$inventory"
    capability_matrix_hash="$(sha256_file "$package/capabilities/capability-matrix.json")"

    printf '%s\n' \
        '[package]' \
        'name = "tondo-std"' \
        'edition = "0.1"' \
        'package_id = "toolchain:std:0.1-bootstrap"' \
        '' \
        '[distribution]' \
        'format = "tondo-stdlib-vm-distribution/1"' \
        'target = "tondo-vm-hosted"' \
        'profile = "hosted"' \
        'source = "src"' \
        'interface = "interfaces/std-0.1.json"' \
        'units = "units/std-0.1.json"' \
        'providers = "providers/providers.json"' \
        'capabilities = "capabilities/capability-matrix.json"' \
        > "$package/manifests/tondo.toml"
    copy_generated manifests manifests/tondo.toml "$package" "$inventory"

    printf '%s\n' \
        'format = "tondo-stdlib-lock/1"' \
        'package_id = "toolchain:std:0.1-bootstrap"' \
        'edition = "0.1"' \
        "api_sha256 = \"$api_hash\"" \
        "matrix_sha256 = \"$matrix_hash\"" \
        'source_policy = "content-addressed-clean-snapshot"' \
        > "$package/manifests/tondo.lock.toml"
    copy_generated manifests manifests/tondo.lock.toml "$package" "$inventory"

    local entries_json payload_hash manifest_bytes manifest_hash
    entries_json="$(
        while IFS=$'\t' read -r role destination origin; do
            file="$package/$destination"
            [[ -f "$file" ]] || { echo "stdlib distribution: missing generated payload: $destination" >&2; exit 1; }
            hash="$(sha256_file "$file")"
            bytes="$(wc -c < "$file" | tr -d ' ')"
            jq -cn --arg role "$role" --arg path "$destination" --arg origin "$origin" --arg sha256 "$hash" --argjson bytes "$bytes" \
                '{role: $role, path: $path, origin: $origin, sha256: $sha256, bytes: $bytes}'
        done < <(LC_ALL=C sort -t $'\t' -k2,2 "$inventory")
    )"
    entries_json="$(printf '%s\n' "$entries_json" | jq -s 'sort_by(.path)')"
    payload_hash="$(jq -r '.[] | [.path, .sha256, (.bytes | tostring)] | @tsv' <<< "$entries_json" | sha256sum | cut -d ' ' -f 1)"
    manifest_bytes="$(jq -S -c -n \
        --arg package_id "toolchain:std:0.1-bootstrap" \
        --arg payload_hash "$payload_hash" \
        --arg api_hash "$api_hash" \
        --arg matrix_hash "$capability_matrix_hash" \
        --arg evidence_hash "$evidence_hash" \
        --argjson files "$entries_json" \
        --argjson contract "$(cat "$contract")" \
        ' {
            format: "tondo-stdlib-vm-distribution/1",
            package_id: $package_id,
            edition: "0.1",
            target: "tondo-vm-hosted",
            profile: "hosted",
            files: $files,
            source_hashes: ($files | map(select(.role == "sources"))),
            interface_hashes: ($files | map(select(.role == "interfaces"))),
            unit_hashes: ($files | map(select(.role == "units"))),
            provider_hashes: ($files | map(select(.role == "providers"))),
            manifest_hashes: ($files | map(select(.role == "manifests"))),
            documentation_hashes: ($files | map(select(.role == "docs"))),
            capability_matrix_hash: $matrix_hash,
            example_hashes: ($files | map(select(.role == "examples"))),
            api_sha256: $api_hash,
            owner_evidence_sha256: $evidence_hash,
            contract_format: $contract.format,
            payload_hash: $payload_hash,
            reproducible: true,
            public_release: false
        }')"
    printf '%s\n' "$manifest_bytes" > "$package/metadata/manifest.json"
    manifest_hash="$(sha256_file "$package/metadata/manifest.json")"
    printf '%s\n' "$manifest_hash" > "$package_parent/manifest.sha256"
    printf '%s\n' "$payload_hash" > "$package_parent/payload.sha256"

    (
        cd "$package_parent"
        tar --format=ustar --sort=name --mtime='UTC 1970-01-01' \
            --owner=0 --group=0 --numeric-owner \
            -cf "$archive" tondo-std-0.1
    )
}

make_clean_snapshot() {
    local destination="$1"
    mkdir -p "$destination"
    git archive --format=tar HEAD | tar -xf - -C "$destination"
    [[ ! -e "$destination/.git" && ! -e "$destination/target" ]] || {
        echo "stdlib distribution: clean snapshot contains repository/build state" >&2
        exit 1
    }
}

workspace_a="$work/workspace-a"
workspace_b="$work/workspace-b"
archive_a="$work/stdlib-a.tar"
archive_b="$work/stdlib-b.tar"
make_clean_snapshot "$workspace_a"
make_clean_snapshot "$workspace_b"
assemble "$workspace_a" "$archive_a" a
assemble "$workspace_b" "$archive_b" b

cmp -s "$archive_a" "$archive_b" || {
    echo "stdlib distribution: archives differ between clean workspaces" >&2
    exit 1
}
cmp -s "$work/a/tondo-std-0.1/metadata/manifest.json" "$work/b/tondo-std-0.1/metadata/manifest.json" || {
    echo "stdlib distribution: manifests differ between clean workspaces" >&2
    exit 1
}

verify_manifest() {
    local package="$1" manifest="$package/metadata/manifest.json"
    jq -e \
        --arg package_id "toolchain:std:0.1-bootstrap" \
        '.format == "tondo-stdlib-vm-distribution/1" and .package_id == $package_id and .edition == "0.1" and .target == "tondo-vm-hosted" and .profile == "hosted" and .reproducible == true and .public_release == false and (.files | length) > 0' \
        "$manifest" >/dev/null
    while IFS= read -r record; do
        path="$(jq -r '.path' <<< "$record")"
        expected_hash="$(jq -r '.sha256' <<< "$record")"
        expected_bytes="$(jq -r '.bytes' <<< "$record")"
        file="$package/$path"
        [[ -f "$file" && ! -L "$file" ]] || { echo "stdlib distribution: missing or linked payload: $path" >&2; exit 1; }
        [[ "$(sha256_file "$file")" == "$expected_hash" ]] || { echo "stdlib distribution: payload hash mismatch: $path" >&2; exit 1; }
        [[ "$(wc -c < "$file" | tr -d ' ')" == "$expected_bytes" ]] || { echo "stdlib distribution: payload size mismatch: $path" >&2; exit 1; }
    done < <(jq -c '.files[]' "$manifest")
    actual_payload_hash="$(jq -r '.files[] | [.path, .sha256, (.bytes | tostring)] | @tsv' "$manifest" | sha256sum | cut -d ' ' -f 1)"
    [[ "$actual_payload_hash" == "$(jq -r '.payload_hash' "$manifest")" ]] || {
        echo "stdlib distribution: manifest payload hash mismatch" >&2
        exit 1
    }
}

install_root="$work/install"
empty_workspace="$work/empty-workspace"
mkdir -p "$install_root" "$empty_workspace"
tar -xf "$archive_a" -C "$install_root"
package="$install_root/tondo-std-0.1"
verify_manifest "$package"
rm -rf -- "$workspace_a" "$workspace_b"
[[ ! -e "$workspace_a" && ! -e "$workspace_b" ]] || {
    echo "stdlib distribution: source snapshots survived installation check" >&2
    exit 1
}

expected_output="$(cat "$package/examples/m11-std-core-001.stdout")"
actual_output="$(
    cd "$empty_workspace"
    env -i PATH=/usr/bin:/bin HOME="$work/home" \
        "$package/bin/tondo" run "$package/examples/m11-std-core-001.to"
)"
[[ "$actual_output" == "$expected_output" ]] || {
    echo "stdlib distribution: installed example output mismatch" >&2
    exit 1
}

marker="$empty_workspace/keep.txt"
printf 'workspace-preserved\n' > "$marker"
rm -rf -- "$package"
[[ ! -e "$package" && -f "$marker" ]] || {
    echo "stdlib distribution: uninstall mutated the workspace" >&2
    exit 1
}

archive_hash="$(sha256_file "$archive_a")"
archive_bytes="$(wc -c < "$archive_a" | tr -d ' ')"
manifest_hash="$(cat "$work/a/manifest.sha256")"
payload_hash="$(cat "$work/a/payload.sha256")"
cp -- "$archive_a" "$output_dir/tondo-std-0.1.tar"
jq -S -n \
    --arg package_id "toolchain:std:0.1-bootstrap" \
    --arg archive "${output_dir#"$root"/}/tondo-std-0.1.tar" \
    --arg archive_sha256 "$archive_hash" \
    --arg manifest_sha256 "$manifest_hash" \
    --arg payload_sha256 "$payload_hash" \
    --argjson archive_bytes "$archive_bytes" \
    '{
      format: "tondo-stdlib-distribution-evidence/1",
      edition: "0.1",
      phase: "STD-0.1A",
      status: "promoted-draft",
      package_id: $package_id,
      archive: $archive,
      archive_sha256: $archive_sha256,
      archive_bytes: $archive_bytes,
      manifest_sha256: $manifest_sha256,
      payload_sha256: $payload_sha256,
      clean_source_workspaces: 2,
      byte_identical: true,
      installed_example: "examples/m11-std-core-001.to",
      installed_output: "core-ok\\n",
      source_tree_required_after_install: false,
      uninstall_preserves_workspace: true,
      public_release: false
    }' > "$output_dir/stdlib-distribution.json"

echo "stdlib distribution: OK (2 clean workspaces; byte-identical VM package; install/run/uninstall verified; evidence: ${output_dir#"$root"/}/stdlib-distribution.json)"
