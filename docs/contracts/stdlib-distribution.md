# STD-0.1A VM distribution contract

`STD-A-DIST-001` turns the current unpublished standard-library draft into a
relocatable VM package. It is deliberately a distribution of the standard
library and its metadata, not a language release and not the native publisher
described by `NATIVE-PUBLISH-SPEC-001`.

## Package identity

The package uses the compiler-owned `PackageId`
`toolchain:std:0.1-bootstrap`, edition `0.1`, target `tondo-vm-hosted` and
profile `hosted`. The distribution record is
`tondo-stdlib-vm-distribution/1`. A package contains no timestamps, host paths,
environment values, or network-resolved inputs.

The archive is a normalized USTAR tar stream: paths are bytewise sorted,
numeric owner/group IDs are zero, modification time is zero, and file modes
are limited to the reproducible read/execute contract. Two assemblies from
independent clean source snapshots must be byte-identical. The package
manifest records every payload path, role, byte count and SHA-256, plus a
content hash over the payload entries (the manifest itself is excluded from
that self-referential hash).

## Required payload

The package root is `tondo-std-0.1/` and has these sections:

| Section | Contents |
| --- | --- |
| `bin/` | The VM CLI used to execute installed examples. Its bytes are hashed like every other payload. |
| `src/` | Standard-library source snapshots, including the build-time `std.meta` source. |
| `interfaces/` | One static interface record containing the PackageId, API hash, owner set and signature identities. |
| `units/` | Owner-aware compiler/VM unit records, layer, source-set and evidence identity. |
| `providers/` | Hosted VM and build-time provider records with their exact capability requirements. |
| `manifests/` | Canonical TOML package and lock manifests. They are data in the distribution; users do not maintain JSON project files. |
| `docs/` | The standard-library specification and owner contracts needed to understand the installed surface. |
| `capabilities/` | The normative owner matrix, public API/conformance records and a derived capability matrix. |
| `examples/` | Relocatable `.to` examples and their expected sidecars. |
| `metadata/` | The distribution contract and generated package manifest. |

All source, interface, unit, provider, manifest, documentation, capability
and example inputs are copied from the clean snapshot. The generated records
bind their hashes to the same inputs; a missing or changed input fails before
an archive is produced.

## Build and reproducibility

`scripts/stdlib-distribution.sh` creates two clean snapshots with
`git archive`, assembles both packages, and compares the complete archive
bytes and package manifests (`source_workspaces: 2`, `byte_identical: true`). It does not inspect the working-tree source
after the snapshots are made. The output evidence is ignored under
`target/reliability/evidence/stdlib-distribution.json` and contains the
archive hash, manifest hash, payload hash, input snapshot identities and
installation observations.

The VM binary is an explicit input (`TONDO_VM_BINARY`, defaulting to the
already-built `target/debug/tondo`). It is copied into the package and hashed;
the distribution runner never searches `PATH`, executes a shell, or consults
the source tree to resolve an installed module.

## Installation, execution and removal

The executable test extracts the archive into a fresh installation directory
and leaves a separate empty workspace beside it. It verifies the package
manifest and file hashes (`manifest-and-file-hash-before-run`), then runs `examples/m11-std-core-001.to` through the
installed `bin/tondo`. The example is read from the installation, with no
source-tree path present in the workspace (`not-consulted-after-install`). Finally, uninstallation removes
only the package root and proves that the workspace remains untouched and
empty (`uninstall_preserves_workspace: true`).

There is no in-place upgrade protocol in this block. Atomic replacement of a
native product remains the responsibility of `NATIVE-001`; a later release
workflow may add signed or registry-backed distribution without changing the
content-addressed draft contract.

## Failure boundaries

The checker and executable test reject missing binaries, wrong PackageIds,
contract drift, missing examples, non-empty installation workspaces, archive
differences, manifest/payload hash mismatches and any attempt to execute an
example after removing the installed package. A successful draft distribution
is evidence for S1A; it does not publish Tondo or claim a public release.
