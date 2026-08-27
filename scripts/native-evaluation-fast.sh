#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="$root/testing/native-evaluation-fast.json"

die() {
    echo "native evaluation fast: $*" >&2
    exit 1
}

scripts/native-evaluation-check.sh
scripts/native-evaluation-fast-check.sh

llvm_tool="${TONDO_LLVM_LLC:-/usr/bin/llc}"
[[ "$llvm_tool" = /* ]] || die "TONDO_LLVM_LLC must be an absolute path"
[[ -x "$llvm_tool" ]] || die "LLVM llc is not executable: $llvm_tool"
llvm_version="$("$llvm_tool" --version 2>&1 | sed -n '1p')"
grep -Eq 'LLVM version 18\.' <<< "$llvm_version" \
    || die "LLVM llc must be version 18.x for the pinned fast lane"

target_dir="${CARGO_TARGET_DIR:-target}"
if [[ "$target_dir" != /* ]]; then
    target_dir="$root/$target_dir"
fi
evidence_dir="$target_dir/reliability/evidence"
adapter_target="$target_dir/native-evaluation"
tmp_root="$root/.tmp"
mkdir -p "$evidence_dir" "$tmp_root"
tmp="$(mktemp -d "$tmp_root/tondo-native-evaluation-fast.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

mapfile -t fixtures < <(jq -r '.corpus[].path' "$contract")
[[ "${#fixtures[@]}" -eq 4 ]] || die "expected exactly four fast-lane fixtures"
probe="$tmp/mir-probe.json"
CARGO_TARGET_DIR="$target_dir" cargo run -p tondo-compiler --example native_mir_probe \
    --locked --quiet -- "${fixtures[@]}" > "$probe"

CARGO_TARGET_DIR="$adapter_target" cargo build \
    --manifest-path tools/native-evaluation/Cargo.toml --locked --quiet
adapter="$adapter_target/debug/tondo-native-evaluation"
[[ -x "$adapter" ]] || die "missing native evaluation adapter binary"

candidate_report="$tmp/candidates.json"
"$adapter" \
    --probe "$probe" \
    --output "$candidate_report" \
    --llvm "$llvm_tool" \
    --target "$(rustc -vV | sed -n 's/^host: //p')" \
    --temp-dir "$tmp/backend" \
    || die "candidate adapter failed"

workspace="clean"
if ! git diff --quiet || ! git diff --cached --quiet; then
    workspace="dirty-allowed-for-fast-lane"
fi
revision="$(git rev-parse HEAD)"
target="$(rustc -vV | sed -n 's/^host: //p')"

jq -n \
    --slurpfile contract "$contract" \
    --slurpfile probe "$probe" \
    --slurpfile candidates "$candidate_report" \
    --arg revision "$revision" \
    --arg workspace "$workspace" \
    --arg target "$target" \
    --arg llvm "$llvm_version" \
    ' {
      format: "tondo-native-evaluation-fast-report/1",
      phase: "NATIVE-001",
      status: "passed",
      git_revision: $revision,
      workspace: $workspace,
      target: $target,
      toolchain: {
        llvm: $llvm,
        cranelift: "cranelift-codegen/0.132.3"
      },
      adapter: $contract[0].adapter,
      protocol: $contract[0].protocol,
      corpus: $contract[0].corpus,
      mir_probe: {
        format: $probe[0].format,
        fixtures: ($probe[0].fixtures | map({
          fixture,
          fixture_sha256,
          status,
          exit_code,
          diagnostic_codes,
          stdout_sha256,
          mir
        }))
      },
      candidates: ($candidates[0].candidates | map(
        . as $candidate
        | ($candidate.samples | group_by(.fixture) | map(
            . as $rows
            | ($rows | map(.compile_time_ns) | sort) as $compile
            | ($rows | map(.code_size_bytes) | sort) as $size
            | ($compile | length) as $n
            | {
                fixture: $rows[0].fixture,
                fixture_sha256: $rows[0].fixture_sha256,
                sample_count: $n,
                compile_time_ns: {
                  median: $compile[((($n * 0.50) | ceil) - 1)],
                  p95: $compile[((($n * 0.95) | ceil) - 1)],
                  p99: $compile[((($n * 0.99) | ceil) - 1)]
                },
                code_size_bytes: $size[0]
              }
          )) as $summary
        | . + {summary: $summary}
      )),
      excluded: $candidates[0].excluded,
      correctness: $candidates[0].correctness,
      promotion: {
        selected_backend: null,
        selection_status: "pending-measured-evidence",
        native_semantics: "deferred-to-opt-in-runner",
        full_quality_gate: "promotion-only"
      }
    }' > "$evidence_dir/native-evaluation-fast.json"

jq -e '
    .format == "tondo-native-evaluation-fast-report/1"
    and .phase == "NATIVE-001"
    and .status == "passed"
    and .promotion.selected_backend == null
    and .promotion.selection_status == "pending-measured-evidence"
    and .promotion.native_semantics == "deferred-to-opt-in-runner"
    and .adapter.format == "tondo-mir-backend/1"
    and .adapter.unsupported_policy == "explicit-trap-and-report"
    and .mir_probe.format == "tondo-native-mir-probe/1"
    and ([.mir_probe.fixtures[] | select(.status == "passed")] | length == 4)
    and ([.candidates[] | select(.status == "measured")] | length == 2)
    and all(.candidates[] | select(.status == "measured");
        (.samples | length) == 12
        and all(.samples[]; .compile_time_ns > 0 and .code_size_bytes > 0)
        and (.summary | length == 4)
        and all(.summary[];
            .sample_count == 3
            and .compile_time_ns.median > 0
            and .compile_time_ns.p95 >= .compile_time_ns.median
            and .compile_time_ns.p99 >= .compile_time_ns.p95
            and .code_size_bytes > 0
        )
    )
' "$evidence_dir/native-evaluation-fast.json" >/dev/null \
    || die "fast-lane report failed validation"

! grep -Fq "$root" "$evidence_dir/native-evaluation-fast.json" \
    || die "fast-lane report leaked a physical workspace path"

echo "native evaluation fast: PASS (Cranelift/LLVM measured; report: ${evidence_dir#"$root"/}/native-evaluation-fast.json)"
