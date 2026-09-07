# STD-0.1A VM distribution contract

`STD-A-DIST-001` turns the current unpublished standard-library draft into a
relocatable VM package. It is deliberately a distribution of the standard
library and its metadata, not a language release and not the native publisher
described by `NATIVE-PUBLISH-SPEC-001`.

Promotion remains pending. The executable boundary verifies archive
reproducibility and the installed Core example. It does not establish complete
public owner coverage or that an explicitly supplied VM binary was built from
the captured sources. The report records `binary_source_provenance=not-claimed`.

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
and example inputs come from one immutable capture. The binary and distribution
contract are captured once as explicit inputs. Both assemblies use those exact
bytes. A missing input or a changed live or captured input rejects publication
of the final archive and evidence.

## Build and reproducibility

`scripts/stdlib-distribution.sh` captures the current Git file set with
`scripts/stdlib_distribution_inputs.py`, including untracked nonignored inputs
only during explicit local iteration. Canonical relative paths, regular files,
file modes, byte lengths and SHA-256 hashes define the source snapshot.
Symlinks and paths escaping the repository are rejected. Git revision and
dirty state are recorded separately from the content hash.

Two source directories are materialized from that capture; both assemblies
must produce identical archive bytes and package manifests
(`source_workspaces: 2`, `byte_identical: true`). The original inputs and the
capture are rechecked before final output is written. The output evidence is
ignored under `target/reliability/evidence/stdlib-distribution/` and contains
archive, manifest, payload, source, binary and contract hashes, together with
the exact observed installed stdout. All payload modes and archive metadata
are normalized independently of the caller's umask.

The default requires a clean checkout. `TONDO_STDLIB_DIST_ALLOW_DIRTY=1`
permits local iteration and records `clean_source_workspaces: 0`; it never
describes current working bytes as a clean `HEAD` snapshot. The report status
is `verified-vm-bundle`, with `promotion: pending`, on either route. S1A must
still establish its own public implementation and promotion prerequisites.

The VM binary is an explicit input (`TONDO_VM_BINARY`, defaulting to the
already-built `$CARGO_TARGET_DIR/debug/tondo`, or `target/debug/tondo` when
unset). It is copied into the package and hashed;
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
differences, source/revision/binary/contract drift, manifest/payload hash
mismatches and any attempt to execute an example after removing the installed
package. Capture tests also reject file membership changes, executable-mode
changes and corruption of the frozen inputs. A successful bundle round trip
is bounded component evidence; it does not promote S1A or publish Tondo.
