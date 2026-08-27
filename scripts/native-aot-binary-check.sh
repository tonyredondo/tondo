#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_NATIVE_AOT_BINARY_CONTRACT:-$root/testing/native-aot-binary.json}"

die() {
    echo "native AOT binary: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with one LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains CR or trailing whitespace"

jq -e '
  def unique_values: length == (unique | length);
  .format == "tondo-native-aot-binary/1"
  and .owner == "toolchain.native_evaluation"
  and .edition == "0.1"
  and .task == "NATIVE-AOT-BINARY-001"
  and .status == "closed"
  and .implementation.adapter == "tools/native-evaluation/src/main.rs"
  and .implementation.runtime == "tools/native-evaluation/src/main.rs:native_runtime_c_source"
  and .implementation.runner == "scripts/native-evaluation-runner.sh"
  and .implementation.evidence == "target/reliability/evidence/native-evaluation-runner.json"
  and .implementation.report_field == "native_aot_binary"
  and .implementation.receipt == "tondo-native-aot-binary-receipt/1"
  and .input == {
      mir_format: "tondo-mir-backend/1",
      target: "x86_64-unknown-linux-gnu",
      profile: "release",
      runtime_abi: "tondo-runtime-draft/1",
      stdlib: "STD-0.1A",
      candidates: ["cranelift", "llvm"],
      same_target_runtime_stdlib_linker_profile: true,
      fresh_builds_per_candidate: 2,
      same_mir: true,
      linker: "explicit-absolute-cc",
      strip: "explicit-absolute-strip",
      section_reader: "explicit-absolute-readelf"
  }
  and .product.kind == "complete-linked-aot-executable"
  and .product.debug_bytes == "linked-product-before-strip-debug"
  and .product.stripped_bytes == "same-product-after-strip-debug"
  and .product.sections == "readelf-wide-section-sizes"
  and .product.startup == {
      fresh_processes: 3,
      unit: "nanoseconds",
      product: "stripped",
      quantiles: ["median", "p95", "p99"]
  }
  and .product.required_nonempty_section == ".text"
  and .identity.hash == "sha256"
  and ([.identity.inputs[]] | length == 9 and unique_values)
  and .identity.physical_paths == "forbidden"
  and .identity.timestamps == "forbidden"
  and .identity.process_ids == "forbidden"
  and .identity.addresses == "forbidden"
  and .identity.workspace_prefix == "mapped-to-dot-before-debug-build"
  and (.invariants | length == 12 and unique_values)
  and (.negative_cases | length == 13 and unique_values)
  and .next_blocks == ["NATIVE-AOT-MEM-001", "NATIVE-AOT-QUALITY-001"]
' "$contract" >/dev/null || die "invalid machine-readable contract"

for path in \
    docs/contracts/native-aot-binary.md \
    TONDO_IMPLEMENTATION_TRACKER.md \
    testing/tracker-graph.json \
    tools/native-evaluation/src/main.rs \
    scripts/native-evaluation-runner.sh \
    testing/native-evaluation-runner.json; do
    [[ -f "$root/$path" ]] || die "missing AOT binary evidence: $path"
done

grep -Fq 'NATIVE-AOT-BINARY-001' "$root/TONDO_IMPLEMENTATION_TRACKER.md" \
    || die "tracker does not reference the AOT binary block"
grep -Fq 'run_native_aot_binary_probe' "$root/tools/native-evaluation/src/main.rs" \
    || die "adapter has no linked-product probe"
grep -Fq 'tondo-native-aot-binary-receipt/1' "$root/tools/native-evaluation/src/main.rs" \
    || die "adapter has no product receipt"
grep -Fq 'readelf_sections' "$root/tools/native-evaluation/src/main.rs" \
    || die "adapter does not capture section sizes"
grep -Fq 'strip_binary' "$root/tools/native-evaluation/src/main.rs" \
    || die "adapter does not produce a stripped product"
grep -Fq 'same_target_runtime_stdlib_linker_profile' \
    "$root/tools/native-evaluation/src/main.rs" \
    || die "adapter does not record common product inputs"
grep -Fq 'native_aot_binary' "$root/scripts/native-evaluation-runner.sh" \
    || die "runner does not validate linked-product evidence"

jq -e '
  (.task_dependencies["NATIVE-AOT-BINARY-001"] | index("NATIVE-AOT-LOWER-001")) != null
  and (.task_dependencies["NATIVE-AOT-MEM-001"] | index("NATIVE-AOT-LOWER-001")) != null
  and (.task_dependencies["NATIVE-AOT-QUALITY-001"] | index("NATIVE-AOT-LOWER-001")) != null
  and (.task_dependencies["NATIVE-AOT-PERF-001"] | index("NATIVE-AOT-BINARY-001")) != null
' testing/tracker-graph.json >/dev/null || die "tracker graph does not preserve AOT dependency order"

echo "native AOT binary: OK (complete linked products, sections, startup and receipts)"
