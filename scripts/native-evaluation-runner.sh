#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="$root/testing/native-evaluation-runner.json"

die() {
    echo "native evaluation runner: $*" >&2
    exit 1
}

scripts/native-evaluation-runner-check.sh
scripts/native-evaluation-check.sh
scripts/native-evaluation-fast-check.sh

llvm_tool="${TONDO_LLVM_LLC:-/usr/bin/llc}"
cc_tool="${TONDO_NATIVE_CC:-/usr/bin/cc}"
[[ "$llvm_tool" = /* && -x "$llvm_tool" ]] \
    || die "TONDO_LLVM_LLC must be an absolute executable"
[[ "$cc_tool" = /* && -x "$cc_tool" ]] \
    || die "TONDO_NATIVE_CC must be an absolute executable"
llvm_version="$($llvm_tool --version 2>&1 | sed -n '1p')"
grep -Eq 'LLVM version 18\.' <<< "$llvm_version" \
    || die "LLVM llc must be version 18.x"

target_dir="${CARGO_TARGET_DIR:-target}"
if [[ "$target_dir" != /* ]]; then
    target_dir="$root/$target_dir"
fi
evidence_dir="$target_dir/reliability/evidence"
adapter_target="$target_dir/native-evaluation"
tmp_root="$root/.tmp"
mkdir -p "$evidence_dir" "$tmp_root"
tmp="$(mktemp -d "$tmp_root/tondo-native-evaluation-runner.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

mapfile -t fixtures < <(jq -r '.corpus[].path' testing/native-evaluation-fast.json)
[[ "${#fixtures[@]}" -eq 4 ]] || die "expected four hash-pinned fixtures"
probe="$tmp/mir-probe.json"
CARGO_TARGET_DIR="$target_dir" cargo run -p tondo-compiler --example native_mir_probe \
    --locked --quiet -- "${fixtures[@]}" > "$probe"

CARGO_TARGET_DIR="$adapter_target" cargo build \
    --manifest-path tools/native-evaluation/Cargo.toml --locked --quiet
adapter="$adapter_target/debug/tondo-native-evaluation"
[[ -x "$adapter" ]] || die "missing native evaluation adapter binary"

report="$evidence_dir/native-evaluation-runner.json"
"$adapter" \
    --probe "$probe" \
    --output "$report" \
    --llvm "$llvm_tool" \
    --target "$(rustc -vV | sed -n 's/^host: //p')" \
    --temp-dir "$tmp/backend" \
    --cc "$cc_tool" \
    || die "native scalar runner failed"

jq -e '
  .format == "tondo-native-evaluation-candidates/1"
  and .phase == "NATIVE-001"
  and .status == "passed"
  and .adapter.format == "tondo-mir-backend/1"
  and .correctness.native_semantics == "scalar-managed-and-runtime-native-executable-vs-vm-and-contract"
  and ([.native_runs[] | select(.cranelift == "passed" and .llvm == "passed")] | length >= 1)
  and ([.native_runs[] | select(.oracle_status == "trapped")] | length >= 1)
  and all(.native_runs[];
      ((.oracle_status == "returned"
        and .vm_status == "returned"
        and (.oracle_result | type == "number")
        and (.vm_result | type == "number")
        and (.oracle_result == .vm_result))
       or
       (.oracle_status == "trapped"
        and (.vm_status == "panicked" or .vm_status == "error")
        and .oracle_result == null
        and .vm_result == null))
      and ((.oracle_status == "returned" and .vm_diagnostics == [])
           or (.oracle_status == "trapped"
               and (.vm_diagnostics | length >= 1)
               and all(.vm_diagnostics[]; startswith("vm-"))))
  )
  and ([.native_managed_runs[] | select(.cranelift == "passed" and .llvm == "passed")] | length >= 1)
  and all(.native_managed_runs[];
      .oracle_status == "returned"
      and .vm_status == "returned"
      and (.oracle_tag | type == "number")
      and (.vm_tag | type == "number")
      and (.oracle_tag == .vm_tag)
      and (.oracle_payload == null or (.oracle_payload | type == "number"))
      and (.vm_payload == null or (.vm_payload | type == "number"))
  )
  and ([.native_runtime_runs[] | select(.case == "cleanup-exactly-once" and .cranelift == "passed" and .llvm == "passed")] | length == 1)
  and ([.native_runtime_runs[] | select(.case == "cleanup-abort" and .cranelift == "passed" and .llvm == "passed")] | length == 1)
' "$report" >/dev/null || die "runner report did not prove native execution"

! grep -Fq "$root" "$report" || die "runner report leaked a physical workspace path"
echo "native evaluation runner: PASS (Cranelift/LLVM scalar executables; report: ${report#"$root"/})"
