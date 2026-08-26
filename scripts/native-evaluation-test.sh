#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp_root="$root/.tmp"
mkdir -p "$tmp_root"
tmp="$(mktemp -d "$tmp_root/tondo-native-evaluation-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

expect_failure() {
    local name="$1"
    local candidate="$2"
    if TONDO_NATIVE_EVALUATION_CONTRACT="$candidate" scripts/native-evaluation-check.sh \
        >/dev/null 2>&1; then
        echo "native evaluation tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.decision.selected_backend = "cranelift"' \
    testing/native-evaluation.json > "$tmp/wrong-selection.json"
expect_failure wrong-selection "$tmp/wrong-selection.json"

jq '.candidates[0].role = "excluded"' \
    testing/native-evaluation.json > "$tmp/multiple-comparators.json"
expect_failure multiple-comparators "$tmp/multiple-comparators.json"

jq '.mir_probe.fixtures[2].required_features += ["missing-feature"]' \
    testing/native-evaluation.json > "$tmp/missing-feature.json"
expect_failure missing-feature "$tmp/missing-feature.json"

jq '.mir_probe.fixtures[0].sha256 = ("0" * 64)' \
    testing/native-evaluation.json > "$tmp/hash-mismatch.json"
expect_failure hash-mismatch "$tmp/hash-mismatch.json"

jq '.mir_probe.source = "/physical/path/native_mir_probe.rs"' \
    testing/native-evaluation.json > "$tmp/physical-path.json"
expect_failure physical-path "$tmp/physical-path.json"

jq '.decision.n1_claim = true' \
    testing/native-evaluation.json > "$tmp/premature-n1.json"
expect_failure premature-n1 "$tmp/premature-n1.json"

jq '.decision.native_performance_baseline = "captured"' \
    testing/native-evaluation.json > "$tmp/premature-performance.json"
expect_failure premature-performance "$tmp/premature-performance.json"

jq '.mir_probe.adapter_format = "legacy-summary-only"' \
    testing/native-evaluation.json > "$tmp/legacy-adapter.json"
expect_failure legacy-adapter "$tmp/legacy-adapter.json"

jq '.adapter.vm_equivalence = "available"' \
    testing/native-evaluation.json > "$tmp/premature-adapter-equivalence.json"
expect_failure premature-adapter-equivalence "$tmp/premature-adapter-equivalence.json"

jq '.evaluation_dimensions = [.evaluation_dimensions[] | select(.id != "diagnostic-parity")]' \
    testing/native-evaluation.json > "$tmp/missing-diagnostics.json"
expect_failure missing-diagnostics "$tmp/missing-diagnostics.json"

jq '.next_blocks = ["NATIVE-ABI-001"]' \
    testing/native-evaluation.json > "$tmp/stale-next.json"
expect_failure stale-next "$tmp/stale-next.json"

grep -Fq 'checked before a report can be accepted.' docs/contracts/native-evaluation.md
grep -Fq 'The VM is' docs/contracts/native-evaluation.md
grep -Fq 'NATIVE-BACKEND-ADAPTER-001' docs/contracts/native-evaluation.md
grep -Fq 'N1' docs/adr/019-native-backend-selection.md

echo "native evaluation tests: OK (selection, probe, oracle, safety and frontier negatives rejected)"
