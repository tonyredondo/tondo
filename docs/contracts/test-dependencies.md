# Test-only dependency graph contract

**Status:** public hosted integration under validation; `UTEST-DEPS-001` and T0
remain open until the integration and promotion checks finish.

`tondo_compiler::test_dependencies` validates the dev-dependency interface
records alongside, but never inside, the production `PackageGraph`. It accepts
already supplied lockfile metadata and performs no fetch, file read, resolution
or source compilation.

The CLI reads a separate `test.packages` array in `tondo.lock.toml`. Each entry
has the ordinary locked package fields (`id`, `content_hash`, `dependencies`,
`sources`, `interface`) plus `local_name` and `edition`. Every package requires
an interface, including a package without transitive dependencies. Without a
sidecar, the generated test plan uses each record's `local_name` as its alias.
An advanced `tondo.test.toml` can choose aliases; its interface pins and package
set must still match the lock exactly. A project containing only tests can use
a lockfile containing only the `test` table. No resolver or network fallback
runs during testing.

`TestDependencySources` projects these records into the existing pure project
validator. It admits exact package fingerprints, source hashes, interface
identities and transitive dependencies. The compiler appends captured sources
to the test compilation after sealing production, then checks the actual
public API against each supplied interface. Integration consumers retain the
production package's original dependency edges; unit overlay imports cannot
change those edges for another consumer. Workers receive the compiled program,
so retries and repetitions do not reread dependency source or interfaces.

Normal project discovery removes the `test` section before deriving production
lock identity and never opens its files. Source/interface bytes and the test
lock section participate in the test invocation's public input identity only.
Malformed graphs and stale inputs fail before selection or report publication.

## Closed records

Every record carries a `PackageId`, canonical interface path, exact
`sha256:`-prefixed interface hash and an ordered dependency map. The records
must match the test plan's aliases, PackageIds, interface paths and hashes
exactly; missing, extra or duplicate records are errors. Every explicit edge
must point to another test record, never a production package or the implicit
toolchain-owned standard package. Ordinary `import std` resolves through the
selected toolchain distribution without a lockfile dependency edge, as required
by toolchain section 3.4. Cycles are rejected before graph materialization.

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
Public CLI regressions additionally execute transitive calls and unit overlays,
reject stale sources/interfaces before an empty selection, reject malformed
graphs, and compare exact production artifact/interface bytes after the test
dependency files disappear.
