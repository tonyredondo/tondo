#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_NATIVE_LOWERING_DEBUG_CONTRACT:-$root/testing/native-lowering-debug.json}"

die() {
    echo "native lowering debug: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing contract"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains trailing whitespace"

jq -e '
  def unique_values: length == (unique | length);
  .format == "tondo-native-lowering-debug/1"
  and .owner == "toolchain.native_evaluation"
  and .edition == "0.1"
  and .phase == "NATIVE-LOWER-DEBUG-001"
  and .status == "closed"
  and .contract == "docs/contracts/native-lowering-debug.md"
  and .adapter_format == "tondo-mir-backend/1"
  and .debug_format == "tondo-mir-debug/1"
  and .source_fields == ["ordinal", "module", "logical_path", "content_sha256", "length"]
  and .symbol_fields == ["function", "name", "native", "span"]
  and .source_map_fields == ["id", "kind", "function", "block", "span", "unwind"]
  and .execution_fields == ["id", "kind", "function", "block", "span"]
  and .invariants.logical_paths_only == true
  and .invariants.content_identity == "sha256-of-source-bytes"
  and .invariants.source_ordinals == "deterministic-module-path-content-hash-order"
  and .invariants.symbols_cover_every_function == true
  and .invariants.native_symbol == "tondo_probe_<function-ordinal>"
  and .invariants.regions == ["function", "block", "statement", "terminator"]
  and .invariants.unwind_targets == "declared-block-in-same-function"
  and .invariants.execution_kinds == ["task", "thread"]
  and .invariants.missing_metadata == "reject-before-code-generation"
  and ([.native_consumers[]] | length >= 5 and unique_values)
  and ([.tests[]] | length == 4 and unique_values)
  and ([.evidence[]] | length == 2 and unique_values)
  and .next_blocks == ["NATIVE-THREAD-001"]
' "$contract" >/dev/null || die "invalid machine-readable contract"

for path in \
    docs/contracts/native-lowering-debug.md \
    crates/tondo-compiler/src/mir.rs \
    crates/tondo-compiler/src/driver.rs \
    tools/native-evaluation/src/main.rs \
    scripts/native-lowering-debug-test.sh; do
    [[ -f "$root/$path" ]] || die "missing debug evidence: $path"
done

grep -Fq 'tondo-mir-debug/1' "$root/crates/tondo-compiler/src/mir.rs" \
    || die "compiler does not emit the debug format"
grep -Fq 'backend_program_with_debug' "$root/crates/tondo-compiler/src/driver.rs" \
    || die "driver does not attach resolved source metadata"
grep -Fq 'validate_backend_debug' "$root/tools/native-evaluation/src/main.rs" \
    || die "native adapter does not validate debug metadata"
grep -Fq 'tondo.debug map' "$root/tools/native-evaluation/src/main.rs" \
    || die "LLVM adapter does not preserve source-map records"

echo "native lowering debug: OK (logical source maps, symbols, unwind and task/thread identities)"
