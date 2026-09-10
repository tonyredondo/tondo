# Public meta and reflection conformance

`testing/stdlib-meta-reflect-conformance.json` records the reviewed public
boundary for `STD-META-CONF-001` and `META-CONF-001`: ordinary compiled Tondo
providers on `tondo-meta`, and compiler-retained descriptors queried by the
hosted VM. It enumerates all 25 meta callables and 27 reflection callables,
plus each owner's six requirement rows and their exact observables.

Each requirement identifies positive, rejection, boundary, composition,
oracle and public execution tests. The meta companion case executes every
callable with fresh owned requests; public TOML tests exercise generators,
derives, locks, transitive inputs and sealed production consumers. The
reflection fixture executes every query, while compiler/VM tests check
visibility, reachability, identity, malformed descriptors and memory limits.
Model tests provide additional oracles and retain their component scope.

All referenced tests belong to the live draft meta layer. The normal test gate
runs the workspace, attests actually passed tests against their source hashes,
and runs the conformance adapter with that attestation. The public row checker
consumes that resulting execution report. It verifies the live input tree,
inventory, manifests, exact cases, individual observations and their digests.
A missing test, changed source, omitted signature or changed requirement is an
error. A case plan or source-token match alone cannot satisfy execution.

Run `scripts/stdlib-meta-reflect-conformance-check.sh --plan` to inspect the
declared boundary. Run the command without `--plan` after the test gate to
verify current observations. Its companion `-test.sh` rejects missing rows,
empty traces, changed requirements, skipped execution and stale or modified
observations. The gate runs both commands on each affected source tree.

This boundary does not promote native AOT, benchmark measurements, public
fuzzing of arbitrary programs, unrelated standard-library owners or S1A as a
whole. Their separate evidence cells retain their recorded scope.
