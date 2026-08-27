#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_NATIVE_AOT_LOWERING_CONTRACT:-$root/testing/native-aot-lowering.json}"

die() {
    echo "native AOT lowering: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with one LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains CR or trailing whitespace"

jq -e '
  def unique_values: length == (unique | length);
  .format == "tondo-native-aot-lowering/1"
  and .owner == "toolchain.native_evaluation"
  and .edition == "0.1"
  and .task == "NATIVE-AOT-LOWER-001"
  and .status == "closed"
  and .implementation.mir == "crates/tondo-compiler/src/mir.rs"
  and .implementation.adapter == "tools/native-evaluation/src/main.rs"
  and .implementation.runtime == "tools/native-evaluation/src/main.rs:native_runtime_c_source"
  and .implementation.runner == "scripts/native-evaluation-runner.sh"
  and .implementation.evidence == "target/reliability/evidence/native-evaluation-runner.json"
  and .implementation.report_field == "native_aot_lowering"
  and .input == {
      mir_format: "tondo-mir-backend/1",
      debug_format: "tondo-mir-debug/1",
      oracle: "normalized-MIR-reference-interpreter",
      candidates: ["cranelift", "llvm"],
      same_input: true,
      fresh_process_per_case: true
  }
  and .admitted.value_storage == ["opaque-handle", "bounded-aggregate"]
  and .admitted.collections == ["array", "set", "record", "tuple"]
  and .admitted.projections == ["aggregate:index", "option-value", "result-ok-value", "result-err-value"]
  and .admitted.closures == ["function-ordinal", "mutable-capture"]
  and .admitted.calls == ["direct", "verified-indirect"]
  and .admitted.runtime == ["cleanup", "ownership", "async", "select", "thread"]
  and ([.corpus.storage_cases[]] | length == 7 and unique_values)
  and .corpus.runtime_cases == 20
  and .corpus.minimum_cases == 27
  and .corpus.same_mir_identity == true
  and ([.inventory.lowered_families[]] | length == 10 and unique_values)
  and .inventory.trap_policy == "unsupported-functions-emit-explicit-trap-and-reason"
  and .inventory.report_requires_function_inventory == true
  and .inventory.report_requires_candidate_status == true
  and (.invariants | length == 10 and unique_values)
  and (.negative_cases | length == 10 and unique_values)
  and .next_blocks == ["NATIVE-AOT-BINARY-001"]
' "$contract" >/dev/null || die "invalid machine-readable contract"

for path in \
    docs/contracts/native-aot-lowering.md \
    TONDO_IMPLEMENTATION_TRACKER.md \
    testing/tracker-graph.json \
    tools/native-evaluation/src/main.rs \
    scripts/native-evaluation-runner.sh; do
    [[ -f "$root/$path" ]] || die "missing AOT lowering evidence: $path"
done

grep -Fq 'NATIVE-AOT-LOWER-001' "$root/TONDO_IMPLEMENTATION_TRACKER.md" \
    || die "tracker does not reference the AOT lowering block"
grep -Fq 'NATIVE-AOT-BINARY-001' "$root/TONDO_IMPLEMENTATION_TRACKER.md" \
    || die "tracker does not expose the next AOT block"
jq -e '
  (.task_dependencies["NATIVE-AOT-LOWER-001"] | index("NATIVE-AOT-SCOPE-001")) != null
  and (.task_dependencies["NATIVE-AOT-BINARY-001"] | index("NATIVE-AOT-LOWER-001")) != null
' testing/tracker-graph.json >/dev/null \
    || die "tracker graph does not gate binary work on lowering"

for needle in \
    'tondo_rt_aggregate_new' \
    'tondo_rt_aggregate_set' \
    'tondo_rt_aggregate_get' \
    'parse_aggregate_projection' \
    'parse_verified_function_ordinal' \
    'tondo_rt_indirect_call' \
    'run_native_aot_lowering_probe' \
    'native_aot_lowering'; do
    grep -Fq "$needle" "$root/tools/native-evaluation/src/main.rs" \
        || die "adapter is missing AOT lowering path: $needle"
done

echo "native AOT lowering: OK (concrete storage, projections, verified indirect calls, runtime slices and fail-closed inventory)"
