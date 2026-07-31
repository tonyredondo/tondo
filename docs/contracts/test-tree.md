# Static test-tree contract

**Status:** implemented for `UTEST-ID-001`

`tondo_compiler::test_tree` is the compile-time boundary between parsed
`suite`/`test` declarations and later capture, lowering, selection and worker
phases. It consumes lossless CSTs plus the already closed package/source
metadata. It never executes setup, evaluates control flow, registers runtime
callbacks or reads a host path.

## Input and ordering

Each `TestSourceInput` supplies:

- the exact `PackageId` and local package name used by visible IDs;
- one of the closed `unit-test` or `integration-test` source classes (the
  production class is accepted only to emit `E2001`);
- the declared `ModulePath` and `LogicalPath`;
- the request-local `FileId`; and
- the parsed CST for that file.

The builder validates that the module and logical path match the source
database. Inputs are sorted by package ID, source class, module path, logical
path and source ID. `FileId`, vector insertion order, allocation address and
physical path never participate in this order. Within a file, declaration
order is retained. Therefore permuting source inputs produces the same tree,
IDs, warnings and source ranges.

## Identity and visible IDs

Every descriptor exposes the exact internal identity:

```text
PackageId + source class + module path + ordered node path + node kind
```

`OrderedNodePath` starts with the canonical source ordinal and then contains
the zero-based ordinal of each direct `suite`/`test` member. `TestNodeKind` is
`suite` or `test`; this keeps the identity explicit even though sibling names
are unique. A descriptor also carries its parent identity, declaration span and
name span.

Visible IDs are built without executing code:

```text
package::unit::module.path::suite::test
package::integration::relative.path::suite::nestedSuite::test
```

Unit sources use their module path. Integration sources use their logical path
relative to `tests/`, remove the final `.to` extension and replace `/` with
`.`. The package segment is the local manifest name; the full `PackageId`
remains in the internal identity.

## Semantic checks

The builder registers every direct member in a single sibling namespace. A
second `suite` or `test` with the same name, including a cross-kind collision or
a repeat in another file of the same module, emits `E2002`. The diagnostic
points at the later canonical declaration and relates the first declaration;
reopening or merging a suite is never implicit. A suite with no direct member
emits `E2004`, even when its setup contains ordinary statements. A node in a
production source emits `E2001` with the complete declaration range.

Names are normalized through the ordinary `Name` validator. `_` is rejected as
an identity, and the ordinary camelCase naming warning `W1004` is preserved for
otherwise valid names. Errors are returned as a deterministic diagnostic list;
warnings are attached to a successful tree in the same canonical order.

The flat `StaticTestTree::nodes()` sequence is pre-order: a suite precedes its
descendants, and sibling order is source order. This is the stable input for
future capture and lowering work; no runtime registration is needed to answer
`--list`.
