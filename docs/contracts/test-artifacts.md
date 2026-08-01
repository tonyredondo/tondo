# Content-addressed test artifacts

`tondo-compiler::test_artifacts` is the host-side store for
`testing.attach`.  An attempt supplies a closed name, an RFC-style
`major/minor` media type, and exact bytes.  The store enforces both the byte
and item budgets before publishing a descriptor.  Names are unique within an
attempt and failures use `P2006`; the original bytes are never Base64 encoded
into a test report.

Every blob is written to an immutable SHA-256 object.  Equal bytes from two
attachments share one object, while a different payload under an existing
digest is treated as an object collision.  Object creation is write-to-temp,
`sync_all`, and rename; a failed manifest publish may leave an unreferenced
blob, which is intentional and safe to reclaim later.

The manifest format is `tondo-test-artifacts-0.1/1`.  Descriptors are sorted by
UTF-8 name and expose only the logical `sha256/<digest>` object identity,
media type, size, and digest.  It contains no physical paths, timestamps,
uploads, or host-specific state.  The manifest is serialized canonically and
published atomically under a digest-derived filename; an existing attempt
manifest is immutable and causes a collision rather than an overwrite.

The store rejects `..` path components, absolute escapes, symlinked roots,
object prefixes, manifests, and files.  `orphan_objects` reports unreferenced
regular blobs in bytewise order and `reclaim_orphans` deletes only those
validated objects.  It never follows a symlink or removes a referenced blob.

