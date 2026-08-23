#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
contract="${TONDO_STDLIB_DISTRIBUTION_CONTRACT:-$root/testing/stdlib-distribution.json}"

[[ -f "$contract" ]] || {
    echo "stdlib distribution: missing contract: ${contract#"$root"/}" >&2
    exit 1
}
tail -c 1 "$contract" | cmp -s <(printf '\n') || {
    echo "stdlib distribution: contract must end with one LF" >&2
    exit 1
}
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || {
    echo "stdlib distribution: contract contains CR or trailing whitespace" >&2
    exit 1
}

jq -e '
  .format == "tondo-stdlib-distribution-contract/1"
  and .owner == "toolchain.std_distribution"
  and .edition == "0.1"
  and .phase == "STD-0.1A"
  and .status == "closed-contract"
  and .distribution_format == "tondo-stdlib-vm-distribution/1"
  and .package_id == "toolchain:std:0.1-bootstrap"
  and .target == "tondo-vm-hosted"
  and .profile == "hosted"
  and (.required_sections | sort) == ["bin", "capabilities", "docs", "examples", "interfaces", "manifests", "providers", "sources", "units"]
  and (.required_records | sort) == ["capability_matrix_hash", "documentation_hashes", "edition", "example_hashes", "interface_hashes", "manifest_hashes", "package_id", "payload_hash", "profile", "provider_hashes", "reproducible", "source_hashes", "target", "unit_hashes"]
  and .archive == {
    "format": "tar-ustar",
    "ordering": "lexicographic-path",
    "mtime": 0,
    "numeric_owner": true,
    "source_workspaces": 2,
    "byte_identical": true
  }
  and .installation == {
    "workspace": "empty-directory-only",
    "source_tree": "not-consulted-after-install",
    "verification": "manifest-and-file-hash-before-run",
    "example": "examples/m11-std-core-001.to",
    "runner": "bin/tondo"
  }
  and .uninstallation == {
    "scope": "installed-package-root-only",
    "preserves_workspace": true,
    "no_source_tree_mutation": true
  }
  and .capabilities == {
    "std.console": ["console"],
    "std.env": ["environment"],
    "std.fs": ["filesystem"],
    "std.process": ["process"],
    "std.time": ["clock"]
  }
  and (.negative_cases | sort) == [
    "archive-differs-between-clean-workspaces", "binary-missing", "contract-drift",
    "example-missing", "manifest-hash-mismatch", "payload-hash-mismatch",
    "source-tree-required-after-install", "workspace-not-empty", "wrong-package-id"
  ]
  and .next_blocks == ["STD-S1A-SEAL-001"]
  and .public_release == false
' "$contract" >/dev/null || {
    echo "stdlib distribution: invalid contract" >&2
    exit 1
}

for path in \
    "$root/docs/contracts/stdlib-distribution.md" \
    "$root/testing/stdlib-matrix.json" \
    "$root/testing/stdlib-public-api.json" \
    "$root/testing/stdlib-conformance-coordination.json" \
    "$root/testing/stdlib-owner-evidence.json" \
    "$root/testing/stdlib-documentation.json"; do
    [[ -f "$path" ]] || {
        echo "stdlib distribution: missing distribution input: ${path#"$root"/}" >&2
        exit 1
    }
done

for marker in \
    'STD-A-DIST-001' \
    'tondo-stdlib-vm-distribution/1' \
    'source_workspaces' \
    'manifest-and-file-hash-before-run' \
    'not-consulted-after-install' \
    'uninstallation'; do
    grep -Fq "$marker" "$root/docs/contracts/stdlib-distribution.md" || {
        echo "stdlib distribution: missing documentation marker: $marker" >&2
        exit 1
    }
done

[[ -x "$root/scripts/stdlib-distribution.sh" ]] || {
    echo "stdlib distribution: runner is not executable" >&2
    exit 1
}
[[ -x "$root/scripts/stdlib-distribution-test.sh" ]] || {
    echo "stdlib distribution: contract test runner is not executable" >&2
    exit 1
}

echo "stdlib distribution contract: OK (STD-0.1A VM package; draft, not published)"
