#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_NATIVE_EVALUATION_RUNNER_CONTRACT:-$root/testing/native-evaluation-runner.json}"

die() {
    echo "native evaluation runner: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing runner contract"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains trailing whitespace"

jq -e '
  .format == "tondo-native-evaluation-runner/1"
  and .phase == "NATIVE-BACKEND-ADAPTER-001"
  and .status == "evaluation-ready"
  and .runner == "scripts/native-evaluation-runner.sh"
  and .report == "target/reliability/evidence/native-evaluation-runner.json"
  and .parent_contract == "testing/native-evaluation-fast.json"
  and .adapter_format == "tondo-mir-backend/1"
  and .oracle == "bytecode-vm-scalar-and-managed-result-oracle"
  and .candidates == ["cranelift", "llvm"]
  and .toolchain_policy == {
      llvm: "explicit-absolute-llc-18",
      linker: "explicit-absolute-cc",
      ambient_path_lookup: "forbidden",
      physical_paths_in_report: "forbidden"
  }
  and .native_semantics == "scalar-and-managed-result-checked-arithmetic-control-flow-host-calls-and-traps"
  and (.negative_cases | length == 6)
' "$contract" >/dev/null || die "invalid runner contract"

for path in \
    scripts/native-evaluation-runner.sh \
    tools/native-evaluation/src/main.rs \
    testing/native-evaluation-fast.json; do
    [[ -f "$root/$path" ]] || die "missing runner input: $path"
done

grep -Fq -- '--cc' tools/native-evaluation/src/main.rs \
    || die "adapter has no explicit linker argument"
grep -Fq 'vm_scalar' crates/tondo-compiler/examples/native_mir_probe.rs \
    || die "probe has no VM scalar observations"
grep -Fq 'vm_result' tools/native-evaluation/src/main.rs \
    || die "adapter does not report VM scalar results"
grep -Fq 'vm_managed' crates/tondo-compiler/examples/native_mir_probe.rs \
    || die "probe has no VM managed observations"
grep -Fq 'native_managed_runs' tools/native-evaluation/src/main.rs \
    || die "adapter does not report managed native results"
grep -Fq 'scalar-and-managed-native-executable-vs-vm-and-normalized-oracle' \
    tools/native-evaluation/src/main.rs \
    || die "adapter has no executable scalar evidence state"

echo "native evaluation runner: OK (explicit toolchain and scalar oracle contract)"
