# Isolated integration-root contract

**Status:** implemented for `UTEST-INTEG-001`

`tondo_compiler::test_integration` closes the semantic boundary for a source
under `tests/`. It produces a distinct synthetic consumer `PackageId` for every
canonical logical path. The tested package name remains in visible test IDs,
but no production module or private declaration is placed in the consumer
scope.

## Consumer identity

`build` derives the consumer identity from the tested `PackageId` and the
logical `tests/*.to` path using length-delimited SHA-256 input. Therefore input
ordering, `FileId`, allocation addresses and physical paths cannot change it.
Two roots in one invocation have distinct identities; `build_many` sorts roots
by logical path and rejects duplicate paths before returning any artifact.

## Imports and visibility

Every import is explicit and carries the public interface declarations supplied
by the already verified package artifact. The importer may name the tested
package or a package in the closed test-dependency graph. The synthetic consumer
itself, undeclared packages, duplicate aliases, and duplicate interface
members are rejected.

An interface member marked private is rejected at the boundary, and references
are resolved only against the public declarations of their named alias. There
is no `friend` flag, companion fallback, implicit module lookup, or private
scope reuse. The resulting references retain the target package and module for
later checking without exposing interface bytes.

## Local declarations

Integration sources may define private helpers, types and constants in their own
consumer scope. Public declarations are rejected, as are duplicate local names
in one namespace. Helpers from another root are never visible implicitly; they
must be supplied through an explicit test dependency or source set.

The boundary is host-free. `build_with_graph` only reads package identities from
the validated `TestDependencyGraph`; it does not open interfaces or mutate the
production `PackageGraph`.
