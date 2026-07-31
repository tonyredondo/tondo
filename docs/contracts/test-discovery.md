# Deterministic test discovery contract

**Status:** implemented for `UTEST-DISC-001`

`tondo_compiler::test_discovery` is the pure boundary between host enumeration
and the closed `TestProjectPlan`. The host may enumerate directory entries and
perform filesystem checks, but this module never opens a path, follows a
symlink, reads source bytes, or infers a root from a common prefix.

## Host input

Each `DiscoveryEntry` carries a repository-relative physical path, a
repository-relative logical path, the module identity, and two file-state
assertions:

- the entry is a regular file;
- resolving the entry did not escape its declared root through a symlink.

Paths are slash-separated, relative, non-empty and canonical: `.`/`..`, empty
components, backslashes, absolute paths and line breaks are rejected. The host
adapter is responsible for populating the file-state assertions before calling
the pure classifier. The classifier fails closed on a non-regular entry or a
symlink escape and never performs a second, implicit filesystem lookup.

## Classification and order

`DiscoveryConfig` contains the repository root and the explicit physical and
logical roots from the test plan. Classification is deterministic and
case-sensitive:

1. a path under the conventional physical `tests/` directory is an
   `integration-test` only when it is covered by an integration root;
2. otherwise a path ending in the exact `_test.to` suffix is a `unit-test` when
   it is covered by a production root;
3. otherwise the first declared root covering both the physical and logical
   path supplies the class.

An entry that is not covered by a permitted root is rejected. The first rule
therefore wins for `tests/foo_test.to`, while an explicitly declared
integration root can cover a different physical directory. No case folding,
extension guessing or common-prefix root inference is performed.

Candidates are sorted by canonical physical-path UTF-8 bytes before
classification. Duplicate physical paths and duplicate `(class, logical path,
module)` nodes are errors. A successful entry receives the stable input name
`source:<class>:<physical-path>` and exposes its class, paths and module to the
next phase.

## Plan reconciliation

Discovery is not allowed to silently change a closed build. `reconcile_plan`
and `reconcile_expected` compare the complete source identity
`(class, physical path, logical path, module, input)` as sets. Any missing or
additional source produces `PlanDrift` before compilation or worker creation;
the diagnostic lists both differences in canonical order.

The result is consequently host-independent: two enumerations containing the
same validated entries produce byte-for-byte equivalent ordered source records,
and a plan can be rejected before any source bytes are requested.

Eight compiler tests cover conventional precedence, order independence,
canonical root ordering, unclassified/root-escape rejection, symlink and
regular-file guards, invalid module identities, duplicate records, exact
reconciliation and canonical path rejection.
