# Closed package graph contract

**Status:** implemented as the input boundary for M2

## Purpose

Name resolution never discovers packages or modules. Every
`CompilationRequest` carries a target-selected `PackageGraph` that is closed
before source analysis begins. The graph is a build input beside edition,
target, profile, capabilities, resource limits, source snapshots, and the root
file.

Manifest parsing, version selection, source-set selection, fetching, and
lockfile maintenance remain toolchain responsibilities. They cannot be invoked
from the compiler or inferred from a physical path.

## Package nodes

Every node contains:

- an opaque, non-empty `PackageId` without line breaks;
- the unique stable `SourceId` used by diagnostics for that package;
- its local package name for self-qualified imports;
- its exact language edition;
- the closed set of available module paths for the selected target; and
- a map from local dependency aliases to exact `PackageId` values.

Every module path is already canonical at this boundary: it is a non-empty
dot-separated sequence of NFC Tondo identifiers, contains neither keywords nor
the discard `_`, and is therefore representable exactly by source import
syntax.

The graph separately identifies the root package and the exact standard package.
They must be different nodes. `std` is not an ordinary alias: it always resolves
to the selected standard node, and package manifests cannot claim that spelling.

`PackageGraph::new` rejects duplicate package IDs, duplicate source IDs, unknown
dependency targets, alias collisions with the current package, a missing root or
standard node, and package dependency cycles. These are malformed build inputs,
not Tondo source diagnostics.

## Source ownership

A `SourceId` maps to exactly one `PackageId`. Every source snapshot in the
request must use a source ID owned by the graph and a module path declared by
that node. The request root must belong to the graph's root package.

Several files may contribute to one module, and several modules may belong to
one package. Their `FileId` values remain request-local; package, module, and
nominal identity never depend on file insertion order or physical location.

The loose-file CLI creates a small closed graph explicitly: one synthetic root
package for the input plus the bootstrap standard node. This convenience does
not introduce a second resolution algorithm.

## Import lookup

An import path is split into normalized Tondo names. Its first segment resolves
exactly to one of:

1. `std`;
2. the importing package's local name; or
3. one dependency alias declared by that package.

The remaining non-empty segments form the internal `ModulePath`. The module must
exist in the selected node's closed module set. There is no directory search,
relative fallback, version preference, environment lookup, or unqualified
symbol import.

Missing source-level imports are converted by the resolver to `E1008`. Import
cycles are a property of resolved module edges and become `E1006`; they are
separate from a malformed cyclic package dependency graph.

## Semantic identity

`ModuleId` is `PackageId + ModulePath`. A declaration identity is:

~~~text
PackageId + ModulePath + Namespace + DeclarationPath
~~~

Namespaces are the closed values `type`, `value`, and `module`.
`SymbolIdentity` also retains the package's one-to-one `SourceId` so tooling can
emit the canonical atom required by the specification:

~~~text
@<source-id-byte-length>:<source-id>::<module>::<namespace>::<declaration>
~~~

Aliases never appear in this identity. Two aliases targeting one `PackageId`
therefore name the same declaration, while two package versions remain distinct
even if their source spelling and structure are identical.

## Validation

Unit and driver tests cover exact alias/module lookup, standard-package lookup,
missing modules, invalid aliases and module paths, non-closed dependency edges,
source ownership, loose graph construction, and canonical nominal names. A
compilation request whose source set disagrees with its graph is rejected before
lexing.
