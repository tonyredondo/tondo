#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_NATIVE_CLI_CONTRACT:-$root/testing/native-cli.json}"
die() { echo "native CLI: $*" >&2; exit 1; }
[[ -f "$contract" ]] || die "missing native CLI contract"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains trailing whitespace"

jq -e '
  .format == "tondo-native-cli/1"
  and .task == "NATIVE-CLI-001"
  and .owner == "toolchain.native_cli"
  and .edition == "0.1"
  and .phase == "M20"
  and .status == "closed"
  and .contract == "docs/contracts/native-cli.md"
  and .runner == "scripts/native-cli-test.sh"
  and .fixture == "tests/native/native-cli-001.to"
  and .commands == ["build", "run"]
  and .project_input == "tondo.toml-and-tondo.lock.toml"
  and .build_output == ["tondo.artifact.json", "tondo.artifact.native.json"]
  and .run_contract == {source:"same-closed-project-input",stdout:"preserved",stderr:"preserved",argv:"preserved-after-separator",exit:"stable",diagnostics:"json-or-human"}
  and (.failure_boundaries | length == 6)
  and (.invariants | length == 6)
  and (.negative_cases | length == 8)
  and .next_blocks == ["NATIVE-CONF-ADAPTER-001"]
' "$contract" >/dev/null || die "invalid native CLI contract"

for path in docs/contracts/native-cli.md scripts/native-cli-test.sh crates/tondo-cli/src/main.rs tests/native/native-cli-001.to; do
    [[ -f "$root/$path" ]] || die "missing native CLI input: $path"
done
for marker in '"build"' 'tondo-native-build/1' 'write_atomic' 'emit_artifact' 'program_arguments'; do
    grep -Fq "$marker" crates/tondo-cli/src/main.rs docs/contracts/native-cli.md scripts/native-cli-test.sh \
        || die "native CLI implementation is missing $marker"
done
if grep -Fq -- '--native' crates/tondo-cli/src/main.rs; then
    die "CLI exposes a forbidden --native switch"
fi
if grep -Fq -- '--vm' crates/tondo-cli/src/main.rs; then
    die "CLI exposes a forbidden --vm switch"
fi

echo "native CLI: OK (closed build envelope, atomic products and one run path)"
