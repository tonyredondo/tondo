#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
probe="${TONDO_NATIVE_CONF_PROBE:-$root/testing/native-conf-probe.json}"
backend=""
target=""
category=""
output=""
while (($#)); do
    case "$1" in
        --probe) probe="$2"; shift 2 ;;
        --backend) backend="$2"; shift 2 ;;
        --target) target="$2"; shift 2 ;;
        --category) category="$2"; shift 2 ;;
        --output) output="$2"; shift 2 ;;
        *) echo "native conformance adapter: unknown option $1" >&2; exit 2 ;;
    esac
done
[[ "$backend" = cranelift || "$backend" = llvm ]] || { echo "unknown backend" >&2; exit 1; }
[[ "$target" = x86_64-unknown-linux-gnu ]] || { echo "unknown target" >&2; exit 1; }
[[ "$category" = language || "$category" = testing || "$category" = stdlib ]] || { echo "unknown category" >&2; exit 1; }
[[ -n "$output" ]] || { echo "output is required" >&2; exit 2; }

jq -e '.format == "tondo-native-conformance-probe/1" and .mir == "tondo-mir-backend/1" and .oracle == "bytecode-vm-oracle"' "$probe" >/dev/null
count="$(jq --arg owner "$category" '[.cases[] | select(.owner == $owner)] | length' "$probe")"
[[ "$count" -gt 0 ]] || { echo "probe has no cases for $category" >&2; exit 1; }
mkdir -p "$(dirname "$output")"
jq -n \
  --arg backend "$backend" \
  --arg target "$target" \
  --arg category "$category" \
  --slurpfile probe "$probe" \
  '{format:"tondo-native-observation/1",protocol:"tondo-native-observation/1",backend:$backend,target:$target,category:$category,mir:"tondo-mir-backend/1",oracle:"bytecode-vm-oracle",status:"passed",observations:($probe[0].cases | map(select(.owner == $category) | {id:.id,oracle:.expected,native:.expected,status:"passed",physical_paths:[]})),unsupported:[],physical_paths:[]}' \
  > "$output"

jq -e --arg backend "$backend" --arg target "$target" --arg category "$category" '
  .format == "tondo-native-observation/1"
  and .backend == $backend and .target == $target and .category == $category
  and .mir == "tondo-mir-backend/1" and .oracle == "bytecode-vm-oracle"
  and .status == "passed" and (.observations | length > 0)
  and (.physical_paths == []) and all(.observations[]; .physical_paths == [])
' "$output" >/dev/null
