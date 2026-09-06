#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-yaml-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.yaml tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.wire.yaml_version = "1.1"' testing/stdlib-yaml.json > "$tmp_dir/yaml-11.json"
expect_failure yaml-11 env TONDO_STDLIB_YAML_CONTRACT="$tmp_dir/yaml-11.json" scripts/stdlib-yaml-check.sh

jq '.wire.merge_key = "merge"' testing/stdlib-yaml.json > "$tmp_dir/merge-key.json"
expect_failure merge-key env TONDO_STDLIB_YAML_CONTRACT="$tmp_dir/merge-key.json" scripts/stdlib-yaml-check.sh

jq '.tags.custom = "accept"' testing/stdlib-yaml.json > "$tmp_dir/custom-tags.json"
expect_failure custom-tags env TONDO_STDLIB_YAML_CONTRACT="$tmp_dir/custom-tags.json" scripts/stdlib-yaml-check.sh

jq '.aliases.cycles = "allow"' testing/stdlib-yaml.json > "$tmp_dir/alias-cycle.json"
expect_failure alias-cycle env TONDO_STDLIB_YAML_CONTRACT="$tmp_dir/alias-cycle.json" scripts/stdlib-yaml-check.sh

jq '.dynamic_model.non_string_keys = "allow"' testing/stdlib-yaml.json > "$tmp_dir/non-string-keys.json"
expect_failure non-string-keys env TONDO_STDLIB_YAML_CONTRACT="$tmp_dir/non-string-keys.json" scripts/stdlib-yaml-check.sh

jq '.surface.selectable_operations = ["YamlReader.next"]' testing/stdlib-yaml.json > "$tmp_dir/selectable.json"
expect_failure selectable env TONDO_STDLIB_YAML_CONTRACT="$tmp_dir/selectable.json" scripts/stdlib-yaml-check.sh

jq '.streaming.stack = "host-recursive"' testing/stdlib-yaml.json > "$tmp_dir/recursive-stack.json"
expect_failure recursive-stack env TONDO_STDLIB_YAML_CONTRACT="$tmp_dir/recursive-stack.json" scripts/stdlib-yaml-check.sh

jq '.implementation.public_api_promoted = true' testing/stdlib-yaml.json > "$tmp_dir/premature-promotion.json"
expect_failure premature-promotion env TONDO_STDLIB_YAML_CONTRACT="$tmp_dir/premature-promotion.json" scripts/stdlib-yaml-check.sh

jq '.corpora += [.corpora[0]]' testing/stdlib-yaml.json > "$tmp_dir/duplicate-corpus.json"
expect_failure duplicate-corpus env TONDO_STDLIB_YAML_CONTRACT="$tmp_dir/duplicate-corpus.json" scripts/stdlib-yaml-check.sh

for marker in \
    'YAML 1.2' \
    'YAML 1.1' \
    '!!binary' \
    'maxExpandedNodes' \
    'AliasCycle' \
    'MergeKeyForbidden' \
    'NonStringKey' \
    'parseAll' \
    'decodeAll' \
    'encodeCanonical' \
    'YamlValueView' \
    'YamlReader' \
    'YamlWriter' \
    'maxCollectionEntries' \
    'NoProgress' \
    'environment interpolation'; do
    grep -Fq "$marker" docs/contracts/stdlib-yaml.md \
        || { echo "std.yaml tests: missing marker $marker" >&2; exit 1; }
done

grep -Fq 'stdlib-yaml-test.json' TONDO_STANDARD_LIBRARY_SPEC.md \
    || { echo "std.yaml tests: missing testing contract link" >&2; exit 1; }
grep -Fq 'stdlib-yaml-test.md' docs/contracts/stdlib-yaml.md \
    || { echo "std.yaml tests: missing testing document link" >&2; exit 1; }

jq -e '
  .task == "STD-YAML-001"
  and .wire.yaml_version == "1.2"
  and .wire.schema == "core"
  and .wire.mapping_keys == "text-only"
  and .wire.merge_key == "reject"
  and .tags.custom == "reject"
  and .aliases.cycles == "reject"
  and .aliases.dynamic_model == "logical-copy"
  and .dynamic_model.shared_identity == false
  and .surface.selectable_operations == []
  and ([.surface.signatures[] | select(.id == "reader-from-reader" or .id == "writer-to-writer") | .effect] | sort) == ["suspends", "suspends"]
  and ([.surface.signatures[] | select(.id == "reader-finish") | .signature] | length) == 1
  and .streaming.chunk_boundary_invariant == true
  and .streaming.stack == "explicit-bounded-frames"
  and .ownership.reader_writer_affine == true
  and .errors.partial_success == false
  and .errors.alias_failure_before_expansion == true
  and .performance.scalar_oracle == true
  and .performance.alias_expansion == "iterative-and-budgeted"
  and .implementation.public_api_promoted == false
  and .implementation.status == "verified-hosted-vm"
  and .implementation.host == "verified-hosted-vm-buffered-yaml-bridge"
  and .implementation.native_aot_lowering == "not-claimed"
  and .testing_contract == "testing/stdlib-yaml-test.json"
  and .testing_document == "docs/contracts/stdlib-yaml-test.md"
  and .implementation.required_follow_ups == ["STD-YAML-CONF-001", "STD-YAML-DOC-001"]
  and .promotion.next_blocks == ["STD-YAML-CONF-001"]
' testing/stdlib-yaml.json >/dev/null

echo "std.yaml tests: OK (schema boundary; tags; aliases; limits; streaming; security; promotion)"
