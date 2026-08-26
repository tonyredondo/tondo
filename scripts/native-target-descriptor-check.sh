#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
contract="${TONDO_NATIVE_TARGET_CONTRACT:-$root/testing/native-target-descriptor.json}"

[[ -f "$contract" ]] || { echo "missing native target descriptor contract: ${contract#"$root"/}" >&2; exit 1; }
if ! tail -c 1 "$contract" | cmp -s <(printf '\n'); then
    echo "native target descriptor contract must end with one LF" >&2
    exit 1
fi
if grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null; then
    echo "native target descriptor contract contains CR or trailing whitespace" >&2
    exit 1
fi

jq -e '
  .format == "tondo-native-target-descriptor-contract/1"
  and .owner == "toolchain.native_target"
  and .edition == "0.1"
  and .phase == "M11"
  and .status == "closed-contract"
  and .descriptor_format == "tondo-native-target-descriptor-draft"
  and .canonical_encoding == "compact-utf8-json-struct-order"
  and .identity == "sha256(canonical-descriptor-bytes)"
  and .required_fields == [
    "format", "backend", "target", "object_format", "runtime_abi",
    "capability_registry", "capabilities", "features", "flags", "driver",
    "linker", "artifacts"
  ]
  and .backend_fields == ["name", "version", "implementation_hash"]
  and .target_fields == ["name", "triple", "profile"]
  and .tool_fields == ["id", "version", "artifact_id", "arguments"]
  and .artifact_fields == ["id", "kind", "sha256"]
  and .object_formats == ["elf", "macho", "coff"]
  and .artifact_kinds == ["backend", "driver", "linker", "runtime", "stdlib", "sysroot", "support"]
  and .determinism.capabilities == "sorted-unique-closed-registry"
  and .determinism.features == "sorted-unique-kebab"
  and .determinism.flags == "sorted-unique-no-ambient-expansion"
  and .determinism.artifacts == "sorted-unique-by-id"
  and .determinism.tool_arguments == "ordered-no-ambient-expansion"
  and .determinism.paths == "forbidden-in-tool-identity-and-arguments"
  and .selection.path_lookup == "forbidden"
  and .selection.environment_lookup == "forbidden"
  and .selection.shell_expansion == "forbidden"
  and .selection.unhashed_tool_input == "forbidden"
  and (.negative_cases | sort) == [
    "environment-expansion", "invalid-backend-hash", "invalid-target-triple",
    "missing-driver-artifact", "path-bearing-tool-identity", "pretty-or-unsorted-record",
    "unknown-fields", "unsupported-object-format", "wrong-driver-artifact-kind"
  ]
  and .next_blocks == ["NATIVE-ABI-001"]
' "$contract" >/dev/null

source="$root/crates/tondo-compiler/src/toolchain.rs"
for symbol in \
    'pub struct NativeTargetDescriptor' \
    'pub struct NativeBackendIdentity' \
    'pub struct NativeTargetIdentity' \
    'pub struct NativeToolRef' \
    'pub struct NativeToolArtifact' \
    'pub const NATIVE_TARGET_DESCRIPTOR_FORMAT'; do
    grep -Fq "$symbol" "$source" || { echo "missing native target descriptor symbol: $symbol" >&2; exit 1; }
done

echo "native target descriptor contract: OK"
