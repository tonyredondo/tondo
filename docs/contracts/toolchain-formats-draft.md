# Current draft toolchain format contract

`tondo-compiler::toolchain` is the pure reader for the one unpublished draft
contract: `tondo-manifest-draft`, `tondo-lock-draft`,
`tondo-interface-draft`, `tondo-artifact-draft`,
`tondo-standard-descriptor-draft`, and the matching privileged-unit and
meta-model records.

The bootstrap regression corpus is kept as evidence for already implemented
behavior. It is not a second parser, language edition, or compatibility lane;
the current reader accepts only the draft markers above.

## Boundary

All APIs in this module are pure. They consume caller-supplied UTF-8 JSON
bytes, reject unknown fields and return validated records. They do not read
the filesystem, environment, clock, network or process state and they never
execute generators or derive providers.

`ProjectPlanDraft::parse` validates the manifest, lockfile and canonical standard
descriptor together, checks exact manifest and package hashes, checks runtime
and meta graphs, verifies providers and generator declarations, and returns
the deterministic list of source, meta-source, generator-input and
privileged-unit bytes that an orchestrator may supply next.

## Canonicalization

Manifest and lockfile bytes are user-authored and therefore need not already be
canonical. `Manifest::canonicalize` and `Lockfile::canonicalize` return sorted,
duplicate-free copies; `canonical_bytes` serializes those copies. Their exact
input bytes still remain the value used for `manifest_hash` and the build
identity.

Interfaces, artifacts and standard descriptors require compact canonical JSON
on input. `decode` rejects whitespace, reordered fields, unknown fields and
non-canonical identity lists. `encode` always emits the canonical field order.

## Validation highlights

- SHA-256 values are exactly `sha256:` followed by 64 lower-case hexadecimal
  digits.
- Paths are relative, slash-separated, NFC-normalized and free of `.`/`..`;
  source paths and generated source paths end in `.to`.
- Module paths are validated with the compiler's `ModulePath` rules.
- Runtime and meta PackageIds are disjoint and both dependency graphs are
  closed and acyclic.
- Generator and derive-provider identities, provider packages, roots, outputs
  and positive step/memory/output limits are checked before source execution.
- Active source paths and generated outputs cannot collide.
- The standard descriptor's companion package and standard derive providers
  must agree byte-for-byte with the lockfile.
- Artifact `build_hash` is recomputed from all identity, generation and source
  fields before admission.

The focused tests live beside the implementation in
`crates/tondo-compiler/src/toolchain.rs` and cover canonical round-trips,
unknown-field rejection, graph cycles, hash failures and rejection of
non-draft records.
