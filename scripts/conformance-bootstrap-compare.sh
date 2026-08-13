#!/usr/bin/env bash
set -euo pipefail

composed="$1"
baseline="$2"
temporary="$(mktemp)"
trap 'rm -f "$temporary"' EXIT

jq -c 'del(
    .lineage,
    .revision,
    .lineage_manifest_sha256,
    .inventory_sha256,
    .tree_sha256,
    .input_set_sha256,
    .case_layers
) | .format = "tondo-conformance-result-draft"' "$composed" > "$temporary"
truncate -s -1 "$temporary"
cmp "$temporary" "$baseline"
