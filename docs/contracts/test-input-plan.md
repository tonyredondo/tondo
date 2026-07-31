# Value-free test-input plan contract

**Status:** implemented as the pure planning boundary for
`UTEST-INPUTS-PLAN-001`

`tondo_compiler::test_inputs::TestInputPlan` closes the identity and
reproducibility claims for every input referenced by a validated
`TestProjectPlan`. It never reads the host, opens a provider, materializes a
secret, or executes a worker.

## Closed record

The record format is `tondo-test-input-plan-draft`. Every source input name in
the test plan must occur exactly once. Descriptors are sorted by name and carry
the source, `build`/`runtime`/`both` profile, public/secret visibility, and an
optional target capability.

Public inputs carry only a validated `sha256:` value. Secret inputs carry no
content hash; they require opaque non-empty `provider` and `descriptor` strings
and may carry a version. Public descriptors cannot contain secret metadata.
No input value is accepted by the wire schema or emitted by `canonical_bytes()`.

## Identity and reproducibility

`test_plan_sha256` binds the record to the canonical bytes of
`TestProjectPlan`. `public_sha256` fingerprints the ordered public tuples
`(name, source, profile, sha256, capability)`. Secret metadata is isolated in
`secret_profile_sha256`, fingerprinting
`(name, source, profile, provider, descriptor, version, capability)`; it is
`null` when no secret exists. The secret count and one of the following states
must agree with the descriptors:

- `closed`: no secret inputs;
- `secret-dependent-versioned`: every secret has a version;
- `secret-dependent-unversioned`: at least one secret has no version.

The two secondary digests are lowercase hexadecimal without a prefix; the
plan hash retains the `sha256:` prefix used by the project records.

## Boundary and evidence

Parsing rejects missing references, duplicate names, unknown fields, invalid
hashes, capability drift, public/secret field mixing, incorrect digests or
counts, and false reproducibility declarations. The implementation is pure and
deterministic; it does not perform discovery or host I/O. Five compiler tests
cover public closure, canonical round-tripping, versioned/unversioned secret
states, missing/colliding references, hash/capability drift, unknown fields,
and the absence of a secret-value channel.

Materialization, revocation, cache policy, redaction boundaries, and worker
isolation remain the responsibility of `UTEST-INPUTS-001`.
