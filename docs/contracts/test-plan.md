# Closed test-plan contract

**Status:** implemented as the pure planning boundary for `UTEST-PLAN-001`

`tondo-compiler::test_plan::TestProjectPlan` validates an optional
`tondo-test-plan-draft` record against a previously validated production
`ProjectPlan`. When no record is supplied, `TestProjectPlan::defaults` derives
the same closed shape from the project graph in memory. It never reads a path,
opens CODEOWNERS, resolves a dependency, materializes an input, consults the
host, or executes a test.

## What the plan closes

- exactly one source class per source: `production`, `unit-test`, or
  `integration-test`;
- explicit physical/logical roots and source paths, with no common-prefix
  inference;
- production source identity equal to the active production project;
- unit/integration package ownership and unique input references;
- development dependencies, interface paths, and hashes;
- CODEOWNERS mode (`auto`, `none`, or explicit `path`);
- one selector (`none`, `filter`, `glob`, or `exact`), optional shard, and
  canonical/random order with normalized seed;
- worker policy, reporters, content-addressed artifact store, and snapshot
  stores;
- target, profile, capabilities, features, and `std.time@monotonic-v1`; and
- positive timeout, memory, output, instruction, artifact, snapshot, and
  virtual-timer budgets.

`repository_root` and a source root that represents the repository root are
represented canonically by the empty string. `.` is only an accepted input
spelling and is normalized before the plan is exposed.

## Identity and normalization

The plan carries the exact manifest and lockfile SHA-256 values. Source and
dependency lists are normalized to deterministic order, duplicate paths and
input names are rejected, and random seeds are rendered as sixteen lowercase
hexadecimal digits. `canonical_bytes()` emits compact JSON from this normalized
record. Source bytes, secret values, host paths, and CODEOWNERS contents are not
part of the plan result.

The source class is part of the identity even when physical paths overlap
between production and a unit-test companion. An integration source may use a
synthetic package ID; production and unit sources must name a package in the
closed project graph.

## Validation evidence

The module tests cover canonical normalization, all three source classes,
explicit-root coverage, production drift, unknown fields, project/target/time
mismatches, duplicate inputs, invalid selectors/shards/policies, invalid
dependency hashes, CODEOWNERS modes, store contracts, zero budgets, source
ordering, and the absence of source bytes from the public canonical plan.

`UTEST-INPUTS-PLAN-001` now closes public/secret input identity and
reproducibility in the separate value-free `TestInputPlan`; its contract is
documented in `test-input-plan.md`. The CLI consumes an explicit or adjacent
`tondo.test.toml` when present and otherwise materializes defaults
before discovery. It loads declared snapshot stores as immutable inputs, uses
the closed timeout as the upper bound for each process-isolated leaf worker,
and publishes snapshot updates only through an all-passing atomic stage.
Invocation flags may overlay selection and bounded campaign policy without
mutating the base plan. Discovery, CODEOWNERS file matching, selectors,
lifecycle, reporters, and input materialization remain separate consumers;
they are intentionally not hidden in this parser.
