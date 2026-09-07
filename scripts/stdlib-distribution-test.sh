#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-distribution-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "stdlib distribution negative case passed unexpectedly: $name" >&2
        exit 1
    fi
}

jq '.package_id = "wrong:std"' "$root/testing/stdlib-distribution.json" > "$tmp/wrong-package.json"
expect_failure wrong-package env TONDO_STDLIB_DISTRIBUTION_CONTRACT="$tmp/wrong-package.json" \
    "$root/scripts/stdlib-distribution-check.sh"

jq '.archive.byte_identical = false' "$root/testing/stdlib-distribution.json" > "$tmp/non-reproducible.json"
expect_failure non-reproducible env TONDO_STDLIB_DISTRIBUTION_CONTRACT="$tmp/non-reproducible.json" \
    "$root/scripts/stdlib-distribution-check.sh"

jq '.installation.workspace = "ambient-directory"' "$root/testing/stdlib-distribution.json" > "$tmp/ambient-workspace.json"
expect_failure ambient-workspace env TONDO_STDLIB_DISTRIBUTION_CONTRACT="$tmp/ambient-workspace.json" \
    "$root/scripts/stdlib-distribution-check.sh"

jq '.negative_cases = ["binary-missing"]' "$root/testing/stdlib-distribution.json" > "$tmp/incomplete-negatives.json"
expect_failure incomplete-negatives env TONDO_STDLIB_DISTRIBUTION_CONTRACT="$tmp/incomplete-negatives.json" \
    "$root/scripts/stdlib-distribution-check.sh"

jq '.promotion = "promoted"' "$root/testing/stdlib-distribution.json" > "$tmp/unsupported-promotion.json"
expect_failure unsupported-promotion env TONDO_STDLIB_DISTRIBUTION_CONTRACT="$tmp/unsupported-promotion.json" \
    "$root/scripts/stdlib-distribution-check.sh"

jq '.input_policy.binary_source_provenance = "verified"' "$root/testing/stdlib-distribution.json" > "$tmp/unproved-binary.json"
expect_failure unproved-binary env TONDO_STDLIB_DISTRIBUTION_CONTRACT="$tmp/unproved-binary.json" \
    "$root/scripts/stdlib-distribution-check.sh"

python3 -B - "$root/scripts" <<'PY'
import copy
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, sys.argv.pop())
import stdlib_distribution_inputs as inputs


class DistributionInputs(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name)
        self.root = self.base / "source"
        self.root.mkdir()
        (self.root / "main.to").write_text("fn main() {}\n")
        self.binary = self.base / "tondo"
        self.binary.write_bytes(b"explicit runtime input")
        self.binary.chmod(0o755)
        self.contract = self.base / "contract.json"
        self.contract.write_text('{}\n')
        self.state = {"revision": "1" * 40, "dirty": False, "paths": ["main.to"]}
        self.destination = self.base / "captured"
        inputs.capture(self.root, self.destination, self.binary, self.contract, self.state)

    def verify(self, state=None):
        inputs.verify(self.root, self.destination, self.binary, self.contract,
                      self.state if state is None else state)

    def test_capture_is_identical_at_another_destination(self):
        self.verify()
        other = self.base / "other"
        inputs.capture(self.root, other, self.binary, self.contract, self.state)
        self.assertEqual((other / "inputs.json").read_bytes(),
                         (self.destination / "inputs.json").read_bytes())

    def test_changed_source_binary_contract_and_captured_payload_are_rejected(self):
        for path in [self.root / "main.to", self.binary, self.contract,
                     self.destination / "source/main.to", self.destination / "runtime/tondo",
                     self.destination / "contract.json"]:
            with self.subTest(path=path.name):
                original = path.read_bytes()
                path.write_bytes(original + b"changed")
                with self.assertRaises(ValueError):
                    self.verify()
                path.write_bytes(original)
                self.verify()

    def test_revision_dirty_state_and_file_membership_are_bound(self):
        (self.root / "extra.to").write_text("fn added() {}\n")
        for key, value in [("revision", "2" * 40), ("dirty", True),
                           ("paths", ["main.to", "extra.to"])]:
            with self.subTest(field=key):
                state = copy.deepcopy(self.state)
                state[key] = value
                with self.assertRaises(ValueError):
                    self.verify(state)

    def test_input_modes_are_bound(self):
        self.binary.chmod(0o644)
        with self.assertRaises(ValueError):
            self.verify()

    def test_missing_source_has_no_completed_capture(self):
        state = {**self.state, "paths": ["missing.to"]}
        other = self.base / "missing"
        with self.assertRaises(FileNotFoundError):
            inputs.capture(self.root, other, self.binary, self.contract, state)
        self.assertFalse((other / "inputs.json").exists())

    def test_source_paths_cannot_escape_or_alias_the_capture(self):
        for name in ["../tondo", str(self.binary), "./main.to", "a/../main.to"]:
            with self.subTest(name=name), self.assertRaises(ValueError):
                inputs.source_path(self.root, name)
        (self.root / "alias.to").symlink_to(self.binary)
        with self.assertRaises(ValueError):
            inputs.source_path(self.root, "alias.to")


unittest.main()
PY

for marker in \
    'source_hashes' \
    'interface_hashes' \
    'unit_hashes' \
    'provider_hashes' \
    'manifest_hashes' \
    'capability_matrix_hash' \
    'manifest-and-file-hash-before-run' \
    'source-tree-required-after-install' \
    'uninstall_preserves_workspace'; do
    grep -Fq "$marker" "$root/docs/contracts/stdlib-distribution.md" "$root/testing/stdlib-distribution.json" || {
        echo "stdlib distribution: missing test marker: $marker" >&2
        exit 1
    }
done

echo "stdlib distribution contract tests: OK"
