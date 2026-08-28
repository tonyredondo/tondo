#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="$root/testing/native-evaluation.json"
die() {
    echo "native evaluation runner: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing native evaluation contract"
scripts/native-evaluation-check.sh

if [[ "${TONDO_NATIVE_EVAL_ALLOW_DIRTY:-0}" != "1" ]]; then
    git diff --quiet || die "workspace is dirty; set TONDO_NATIVE_EVAL_ALLOW_DIRTY=1 for local evidence"
    git diff --cached --quiet || die "index is staged; set TONDO_NATIVE_EVAL_ALLOW_DIRTY=1 for local evidence"
fi

target_dir="${CARGO_TARGET_DIR:-target}"
if [[ "$target_dir" != /* ]]; then
    target_dir="$root/$target_dir"
fi
evidence="$target_dir/reliability/evidence"
tmp_root="$root/.tmp"
mkdir -p "$evidence" "$tmp_root"
tmp="$(mktemp -d "$tmp_root/tondo-native-evaluation.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

mapfile -t fixtures < <(jq -r '.mir_probe.fixtures[].path' "$contract")
[[ "${#fixtures[@]}" -eq 4 ]] || die "expected exactly four MIR fixtures"

probe="$tmp/mir-probe.json"
CARGO_TARGET_DIR="$target_dir" cargo run -p tondo-compiler --example native_mir_probe \
    --locked --quiet -- "${fixtures[@]}" > "$probe"

while IFS=$'\t' read -r fixture_path fixture_sha required_features; do
    required_json="$(jq -c --arg path "$fixture_path" \
        '.mir_probe.fixtures[] | select(.path == $path) | .required_features' "$contract")"
    jq -e \
        --arg path "$fixture_path" \
        --arg sha "sha256:$fixture_sha" \
        --argjson required "$required_json" \
        '
        [ .fixtures[] | select(.fixture == $path) ] as $matches
        | ($matches | length == 1)
        and ($matches[0].status == "passed")
        and ($matches[0].fixture_sha256 == $sha)
        and ($matches[0].exit_code == 0)
        and ($matches[0].diagnostic_codes == [])
        and ($matches[0].mir != null)
        and ($matches[0].mir.functions > 0)
        and ($matches[0].mir.blocks > 0)
        and all($required[]; $matches[0].mir.features[.] != null
                and $matches[0].mir.features[.] > 0)
        ' "$probe" >/dev/null || die "MIR probe failed for $fixture_path"
done < <(jq -r '.mir_probe.fixtures[] | [.path, .sha256, (.required_features | join(" "))] | @tsv' "$contract")

version_line() {
    local command="$1"
    if command -v "$command" >/dev/null 2>&1; then
        "$command" --version 2>&1 | sed -n '1p'
    else
        echo "unavailable"
    fi
}

rust_target="$(rustc -vV | sed -n 's/^host: //p')"
git_revision="$(git rev-parse HEAD)"
workspace_state="clean"
if ! git diff --quiet || ! git diff --cached --quiet; then
    workspace_state="dirty-allowed-for-local-evidence"
fi

jq -n \
    --slurpfile contract "$contract" \
    --slurpfile probe "$probe" \
    --arg revision "$git_revision" \
    --arg workspace "$workspace_state" \
    --arg rust_target "$rust_target" \
    --arg rustc "$(version_line rustc)" \
    --arg cargo "$(version_line cargo)" \
    --arg llvm "$(version_line llc)" \
    --arg clang "$(version_line clang)" \
    '{
      format: "tondo-native-evaluation-report/1",
      phase: "NATIVE-001",
      status: "passed",
      git_revision: $revision,
      workspace: $workspace,
      target: $rust_target,
      decision: $contract[0].decision,
      candidates: $contract[0].candidates,
      toolchain: {
        rustc: $rustc,
        cargo: $cargo,
        llvm: $llvm,
        clang: $clang,
        cranelift: "selected-for-0.1-aot; promotion-pending-gate-n1"
      },
      oracle: $contract[0].oracle,
      n1_claim: false,
      native_performance: "deferred-until-complete-native-lowering",
      mir_probe: $probe[0]
    }' > "$evidence/native-evaluation.json"

jq -e '
    .format == "tondo-native-evaluation-report/1"
    and .phase == "NATIVE-001"
    and .status == "passed"
    and .n1_claim == false
    and .native_performance == "deferred-until-complete-native-lowering"
    and .mir_probe.format == "tondo-native-mir-probe/1"
    and ([.mir_probe.fixtures[] | select(.status == "passed")] | length == 4)
' "$evidence/native-evaluation.json" >/dev/null || die "generated report failed validation"

echo "native evaluation: PASS (report: ${evidence#"$root"/}/native-evaluation.json)"
