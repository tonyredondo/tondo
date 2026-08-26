# Native publish and run contract

`NATIVE-PUBLISH-SPEC-001` closes the boundary between a completed native link
and the product that a user can execute. It is a pure, deterministic contract:
the compiler emits a `tondo-native-publish-plan-draft`, while the host
orchestrator is responsible for applying its filesystem policy. No backend or
platform-specific publisher is claimed by this contract.

## Records and identity

The publish plan is compact UTF-8 JSON in declared struct order. Its complete
shape is recorded in `testing/native-publish.json`; unknown fields, pretty JSON,
invalid hashes and non-reproducible values are rejected. The plan contains:

- the compiler, edition and package identity;
- the target descriptor, native artifact and link-plan hashes;
- a logical output (`product_id`, object format and expected product SHA-256);
- the closed publication policy, consumer boundary and finite byte limits; and
- `plan_hash = sha256(canonical-publish-plan-fingerprint)`.

`content_hash` remains the SHA-256 of the complete canonical plan bytes. A
physical output path, timestamp, host name, environment variable, `PATH`
result or temporary name is never part of either identity.

The publisher creates a `tondo-native-published-product-draft` receipt after
the exact product bytes are available. The receipt repeats the three upstream
hashes and records the logical output, product SHA-256, product byte count and
`receipt_hash = sha256(canonical-published-product-fingerprint)`. The receipt
has its own canonical `content_hash`. `NativePublishedProduct::validate_bytes`
must be called with the bytes that will actually be executed; a valid sidecar
alone is not sufficient.

## Publication transaction

The orchestrator applies this fixed sequence. The product and receipt are a
single logical publication pair and must never be exposed as a partially
updated pair to `tondo run`.

1. **Validate records.** Validate the target descriptor, native artifact, link
   plan and publish plan together. Reject a mixed target, stale plan hash,
   output mismatch or limit violation before touching the destination.
2. **Resolve the physical destination.** Resolve `--output` or the target's
   default path outside the semantic records. The destination must be a
   regular file or absent; directories and symlinks are rejected before
   staging. The destination's parent must already exist and be the intended
   output directory.
3. **Stage beside the destination.** Create a unique sibling staging bundle
   with create-new semantics. Write the product and receipt without following
   symlinks or using a shell. A failed write removes only this staging bundle.
4. **Synchronize staged bytes.** Flush both files and call file `fsync` (or the
   platform's equivalent) before commit. If the platform cannot provide the
   requested durability, the orchestrator reports the limitation and does not
   claim stronger durability than the host supplied.
5. **Commit the pair atomically.** Publish the complete product/receipt pair
   through one atomic boundary (for example, a versioned sibling bundle plus a
   single atomic pointer/rename). Never overwrite the old product in place.
   A same-receipt publication is a no-op; a different valid receipt replaces a
   regular existing product only after the new pair is complete.
6. **Synchronize the parent.** Call directory sync after the commit when the
   host supports it. A crash may leave an old complete pair or a new complete
   pair, but never a pair that passed validation with only one updated member.
7. **Clean up.** Remove staging remnants on every pre-commit error and after a
   successful commit. Cleanup is best effort but must not hide the original
   publication error.

The old complete product remains the visible product until the commit boundary.
An interruption before commit therefore preserves it. An interruption after
commit may leave the new complete pair; `tondo run` still verifies both records
and the product bytes before execution.

## `tondo run` consumer

`tondo run` consumes the product selected by the same build plan; it does not
search `PATH` and does not accept a separate native/VM mode. Before executing,
it must:

1. read and size-check the receipt;
2. decode it canonically and bind it to the publish plan's target, artifact,
   link plan, package, product ID and object format;
3. read the product bytes and check both SHA-256 and exact byte count; and
4. reject before execution if either member is absent, stale, a directory, a
   symlink, over the declared limit or otherwise mismatched.

The consumer policy is intentionally closed to
`receipt-and-product-hash-before-exec` and `reject-before-exec`. There is no
fallback to an older product after a mismatch because doing so would hide a
broken publication transaction.

## Failure and collision matrix

| Boundary | Required result |
| --- | --- |
| Invalid descriptor/artifact/link/plan | Reject before destination access |
| Missing or non-positive limit | Reject before staging |
| Directory or symlink at output | Reject before staging |
| Stage/write/fsync failure | Remove staging; preserve old complete pair |
| Same receipt already published | No-op |
| Different valid receipt and regular product | Replace only at atomic commit |
| Receipt/product hash or size mismatch | Reject before execution |
| Unknown or non-canonical record field | Reject before execution/publication |

The pure implementation and executable negative coverage are in
`crates/tondo-compiler/src/toolchain.rs`,
`testing/native-publish.json`, `scripts/native-publish-check.sh` and
`scripts/native-publish-test.sh`. The physical orchestrator is deliberately a
later native lowering implementation concern; this contract is the invariant it
must satisfy.
