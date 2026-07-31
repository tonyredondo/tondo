# Test-only dependency graph contract

**Status:** implemented for `UTEST-DEPS-001`

`tondo_compiler::test_dependencies` validates the dev-dependency interface
records alongside, but never inside, the production `PackageGraph`. It accepts
already supplied lockfile metadata and performs no fetch, file read, resolution
or source compilation.

## Closed records

Every record carries a `PackageId`, canonical interface path, exact
`sha256:`-prefixed interface hash and an ordered dependency map. The records
must match the test plan's aliases, PackageIds, interface paths and hashes
exactly; missing, extra or duplicate records are errors. A record may depend on
another test record or the compiler-owned
`toolchain:std:0.1-bootstrap`, but not on a production package. Cycles are
rejected before graph materialization.

The resulting graph has no production nodes. Its aliases are visible to
`unit-test` and `integration-test` sources through `resolve_alias`; a
`production` source receives an explicit `DevDependencyNotVisible` error before
alias lookup. Transitive lookup uses only the closed test graph and never falls
back to the production graph.

`production_identity(project)` fingerprints only production manifest/lockfile,
target, package, source, capability and feature inputs. Test plans and test
dependency records are not parameters, so constructing or changing this graph
cannot alter the public production identity.

Nine compiler tests cover record validation, deterministic order, exact plan
metadata, missing/extra records, production overlap, transitive closure,
cycles, alias visibility and the production-boundary fingerprint.
