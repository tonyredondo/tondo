# Common test-lowering contract

**Status:** implemented for `UTEST-LOWER-001`

`tondo_compiler::test_lower` is the only bridge from the checked test-body
contract to executable test artifacts. It does not parse source, reopen a
production body, or introduce a test-specific frontend. The caller supplies
the static tree, source spans, checked contracts, suite environment snapshots,
input/output domains and cleanup metadata.

## One identity, three representations

Every entry has a target, node path and `suite`/`test` kind. Its identity is a
length-independent SHA-256 digest of those canonical fields. HIR, MIR and
bytecode contain the same closed operation stream: `TestLog`, `TestTags`,
`TestFailNow`, `TestSkip`, `TestAttach`, `TestSnapshot`, `WithVirtualTime`,
`VirtualTimeSettle` and `VirtualTimeAdvance`. The admission verifier rejects
any divergence, identity drift or artifact hash mismatch.

Entries are ordered by logical source span, not insertion order. Parent links
must refer to an earlier entry, so cycles and hidden second roots cannot be
introduced by lowering. Checked error members, async status, input/output
domain, deferred cleanup and cleanup hooks remain attached to the entry.

## Environment and host boundaries

Environment captures are opaque, named snapshots with a type and digest. They
are sorted and duplicate bindings are rejected; no heap, `FileId`, address or
host handle is serialized. `main` is an explicit negative input: a target that
contains it is rejected with `E2011` and has no executable `main` entry.

The resulting artifact has canonical bytes and an artifact hash suitable for
cache identity. `verify` is intentionally callable again after a later pass,
so a corrupted or forged test-only stream fails before runtime admission.
