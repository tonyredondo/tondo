#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_NATIVE_EVALUATION_FAST_CONTRACT:-$root/testing/native-evaluation-fast.json}"

die() {
    echo "native evaluation fast: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with one LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains CR or trailing whitespace"

jq -e '
  def unique_values: length == (unique | length);
  .format == "tondo-native-evaluation-fast/1"
  and .owner == "toolchain.native_evaluation"
  and .edition == "0.1"
  and .phase == "NATIVE-001"
  and .status == "evaluation-ready"
  and .parent_contract == "testing/native-evaluation.json"
  and .runner == "scripts/native-evaluation-fast.sh"
  and .report == "target/reliability/evidence/native-evaluation-fast.json"
  and .adapter_manifest == "tools/native-evaluation/Cargo.toml"
  and .adapter == {
      format: "tondo-mir-backend/1",
      supported_subset: "scalar-int-checked-arithmetic-asserts-control-flow-tag-dispatch-direct-calls-and-traps",
      unsupported_policy: "explicit-trap-and-report",
      native_semantics: "pending-executable-runner"
  }
  and .protocol == {
      warmup_iterations: 1,
      measurement_repetitions: 3,
      independent_processes: 1,
      minimum_sample_count: 3,
      seed: "tondo-native-evaluation-fast-0.1",
      clean_workspace: false,
      full_quality_gate: "promotion-only"
  }
  and ([.corpus[].path] | length == 4 and unique_values)
  and all(.corpus[]; (.path | test("^[^/][^\\\\]*\\.to$")) and (.sha256 | test("^[0-9a-f]{64}$")))
  and ([.candidates[].id] == ["cranelift", "llvm", "custom"])
  and ([.candidates[].role] == ["candidate", "candidate", "excluded"])
  and ([.candidates[] | select(.role == "candidate")] | length == 2)
  and all(.candidates[];
      (.status | type == "string" and length > 0)
      and (.adapter == null or (.adapter | type == "string" and length > 0))
      and (.dimensions | type == "array" and length <= 4 and unique_values)
  )
  and ([.dimensions[].id] == ["compile-time", "code-size", "peak-memory", "runtime"])
  and .oracle.input == "hash-pinned-real-mir-probe"
  and .oracle.mir == "same-normalized-module-shape"
  and .oracle["backend-verifier"] == "required"
  and .oracle["runtime-equivalence"] == "required-before-selection"
  and .oracle.mismatch == "fail-closed"
  and .selection.selected_backend == null
  and .selection.selection_status == "pending-measured-evidence"
  and .selection.minimum_candidates == 2
  and ([.negative_cases | length] == [6])
' "$contract" >/dev/null || die "invalid machine-readable fast contract"

for path in \
    scripts/native-evaluation-fast.sh \
    tools/native-evaluation/Cargo.toml \
    tools/native-evaluation/Cargo.lock \
    tools/native-evaluation/src/main.rs \
    docs/contracts/native-evaluation.md; do
    [[ -f "$root/$path" ]] || die "missing fast-lane input: $path"
done

grep -Fq 'cranelift-codegen = { version = "=0.132.3"' \
    tools/native-evaluation/Cargo.toml \
    || die "Cranelift adapter version is not pinned"
grep -Fq 'name = "cranelift-codegen"' tools/native-evaluation/Cargo.lock \
    || die "Cranelift adapter lock entry is missing"
grep -Fq 'tondo-mir-backend/1' crates/tondo-compiler/src/mir.rs \
    || die "normalized MIR adapter format is missing"
grep -Fq 'explicit trap' docs/contracts/native-evaluation.md \
    || die "unsupported MIR policy is not fail-closed"

while IFS=$'\t' read -r fixture_path expected_sha; do
    fixture="$root/$fixture_path"
    [[ -f "$fixture" ]] || die "missing corpus fixture: $fixture_path"
    actual_sha="$(sha256sum "$fixture" | cut -d' ' -f1)"
    [[ "$actual_sha" == "$expected_sha" ]] || die "corpus hash mismatch: $fixture_path"
done < <(jq -r '.corpus[] | [.path, .sha256] | @tsv' "$contract")

grep -Fq 'selected_backend": null' "$contract" || die "fast lane cannot select a backend"
grep -Fq 'full_quality_gate": "promotion-only"' "$contract" || die "fast lane must defer full quality gate"

echo "native evaluation fast: OK (two measured candidates; selection and quality promotion deferred)"
