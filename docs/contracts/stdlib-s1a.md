# STD-0.1A implementation and S1A evidence

Status: `open-draft`. The audit reopened S1A because component models,
hosted fixtures and planned cases had been counted as complete public
implementation and conformance. `STD-S1A-SEAL-001` remains pending.
Historical bundles do not close the reopened prerequisites and do not
publish `STD-0.1.0`.

## Evidence boundaries

The owner contracts retain the implemented kernels, compiler components and
bounded hosted tests. Their scope is distinct from the public API, native ABI,
native AOT execution and full conformance.

`testing/stdlib-owner-evidence.json` is a component registry. Its authored
cells describe the referenced component; they cannot override a missing public
route. Its FUZZ and CONF cells are generated from their current contracts by
`scripts/stdlib-owner-evidence-generate.sh`. The historical
`stdlib-fuzz-promote-evidence.sh` command delegates to that generator and
does not grant promotion.

The [public API audit](./stdlib-public-api-audit.md), [normative
matrix](./stdlib-matrix.md) and generated documentation registry preserve
unimplemented surfaces. There are 22 owners, 216 indexed public signatures
and 171 owner requirements. In particular, `std.bytes`, `std.meta`,
`std.reflect` and `std.serialization` retain explicit public integration
gaps. An owner with missing signature rows is partial, not automatically
`not-applicable`.

The [Core coordinator](./stdlib-implementation-coordination.md) remains open
where required public rows are missing. The [Hosted
coordinator](./stdlib-hosted-implementation-coordination.md) records the
bounded compiler/VM routes for `std.console`, `std.path`, `std.fs` and
`std.process`, including 48 indexed signatures. That evidence does not close
other owners or establish native lowering.

The aggregate owner order and capability rules remain in
[`testing/stdlib-spec.json`](../../testing/stdlib-spec.json). Its checker
validates the integration contract without inferring execution from the graph.

## Owner-aware fuzz evidence

`STD-A-FUZZ-001` remains open. The
[`stdlib_owners` target](../../fuzz/fuzz_targets/stdlib_owners.rs) selects one
of 22 routes using the first input byte, with the remaining bytes as payload.
The [fuzz contract](../../testing/stdlib-fuzz.json) records each route's
actual scope, corpus, seed, limits and incomplete obligations. It pins the
audited target and shared codec oracle sources; changing either requires
reviewing the route descriptions before refreshing their hashes.

| Route kind | Owners | Evidence |
| --- | --- | --- |
| Constant admission | `std.async`, `std.console`, `std.core`, `std.env`, `std.fs`, `std.iter`, `std.process`, `std.time` | A fixed source is accepted by the frontend. Payload does not vary the program; owner APIs do not execute. |
| Rust reference only | `std.bytes`, `std.collections`, `std.text` | Rust standard types execute; these routes do not exercise Tondo's implementation. |
| Bounded kernel invariants | `std.format`, `std.io`, `std.json`, `std.math`, `std.messagepack`, `std.path`, `std.protobuf`, `std.serialization`, `std.testing` | The explicitly asserted component properties execute. Whole-owner coverage remains partial. |
| Bounded model | `std.meta`, `std.reflect` | Rust compiler models execute; public Tondo providers and reflection are not established. |

The kernel routes assert formatting byte limits and append atomicity, JSON
value/error replay and independently encoded documents, rounding and square-root
properties across Float64 bit patterns, exact bounded integer FMA, Protobuf
varint/wire invariants, short-I/O byte equality and flush, MessagePack
canonical replay with collision-aware errors, native-path byte identity,
bounded serialization event/Base64 invariants, and testing diff bounds with a
fixed tolerance-reflexivity check. These are narrow properties:
for example, path normalization operations and general tolerance inputs are
not exhaustively checked by those routes.

All 22 owner FUZZ cells remain partial. The nine `component_status=verified`
records preserve useful bounded assertions without promoting the owner. Smoke
and nightly commands execute the target; minimized failures remain replayable
in the owner corpus. A completed campaign proves only the paths and assertions
it exercised.

`scripts/stdlib-fuzz-check.sh` checks source hashes, route selectors, corpora
and agreement with the component registry. Its negative tests reject missing
owners, stale sources, unsupported component promotions and declared cases
presented as observed conformance. Generation is deterministic and cannot
replace a prior output after invalid input admission.

## Owner contracts and implemented components

The following contracts locate the detailed behavior, fixtures and remaining
boundaries. A row is a navigation entry, not a claim that every public or native
route is complete.

| Owners | Contract | Referenced component |
| --- | --- | --- |
| `std.meta` | [stdlib-meta.json](../../testing/stdlib-meta.json) | Build-time protocol and generation models; general public providers remain open. |
| `std.reflect` | [stdlib-reflect.json](../../testing/stdlib-reflect.json) | Closed metadata catalog, privacy and artifact-local identity; public `typeInfo` remains open. |
| `std.bytes` | [stdlib-bytes.json](../../testing/stdlib-bytes.json) | Byte identity, immutable snapshots, UTF-8 and builder limits; public indexing/integration gaps remain explicit. |
| `std.core`, `std.text`, `std.collections`, `std.iter`, `std.math`, `std.format`, `std.io` | [stdlib-core.json](../../testing/stdlib-core.json) | Intrinsic/compiler/VM and portable kernels, numeric/text/collection fixtures and bounded Reader/Writer I/O tests. |
| `std.time` | [stdlib-time.json](../../testing/stdlib-time.json) | Duration/instant/timer models, real and virtual hosted providers and lifecycle tests. |
| `std.env` | [stdlib-env.json](../../testing/stdlib-env.json) | Hosted snapshots, arguments, raw/text values and checked limits. |
| `std.async` | [stdlib-async.json](../../testing/stdlib-async.json) | Hosted structured scheduling and selectable operations within their recorded contracts. |
| `std.console`, `std.path`, `std.fs`, `std.process` | [stdlib-hosted.json](../../testing/stdlib-hosted.json) | Capability checks, paths represented as native bytes, stream separation, file/process handles and hosted cleanup. |
| `std.serialization`, `std.json`, `std.messagepack`, `std.protobuf` | [stdlib-codec-conformance.json](../../testing/stdlib-codec-conformance.json) | Portable typed/dynamic/streaming codec tests and external interoperability; general provider/public integration remains separate. |
| `std.testing` | [test backend](./test-backend.md) | Typed assertions, hosted execution, reports, selection and cleanup; declared inputs, dependencies and remaining isolation obligations are open. |

Portable kernels live in `tondo-stdlib`; intrinsic lowering and capability
gated adapters live in the compiler and VM. A component's HOST
`not-applicable` entry means it has no separate provider at that boundary.
It cannot waive required compiler integration or native execution.

The [owner evidence registry](../../testing/stdlib-owner-evidence.json) retains
the detailed component leaves: `STD-A-CONSOLE-EVIDENCE-001`,
`STD-A-FS-EVIDENCE-001` and `STD-A-PROC-EVIDENCE-001` for hosted I/O;
`STD-A-SER-EVIDENCE-001`, `STD-A-JSON-EVIDENCE-001`,
`STD-A-MSGPACK-EVIDENCE-001` and `STD-A-PROTOBUF-EVIDENCE-001` for codecs.
These IDs locate component evidence and its open FUZZ/CONF cells; they do not
close whole-owner promotion.

## Test, documentation and conformance coordination

`STD-TEST-001` remains open. The generated
`testing/stdlib-test-coordination.json` associates 216 signatures and 171
requirements with 66 declared model laws, test commands and fuzz references.
Its Rust tests check those associations. A law's name and a reference do not
by themselves execute or prove every mapped public signature. The registry
retains all 22 partial fuzz owners and the nine bounded kernel components.

`testing/stdlib-documentation.json` separates kernel, bridge and public API
evidence, with 18 complete and four partial public documentation entries.
Each owner retains an example and its actual verification route. Runtime
examples use exit/output sidecars; compiler/model examples identify that
limited route. Missing public execution for meta or reflection is not waived
as a documentation-only choice.

`testing/stdlib-conformance.json` declares owner commands and cases.
`scripts/stdlib-conformance.sh` executes those commands, public runtime
fixtures and the 206-case draft suite. The report binds the actual revision,
tree, manifest, contract, command logs and repeated case results.
`scripts/stdlib-conformance-check.sh --plan` validates only the case plan;
the default checker requires current execution evidence. Even a passed
declared-case campaign retains `public_row_coverage=unverified` and
`promotion=pending` until every applicable public boundary is demonstrated.

The meta and reflection CONF cells record the ordinary-provider and hosted
descriptor scopes checked by `stdlib-meta-reflect-conformance-check.sh`.
Their complete callable and requirement traces are described in
[`stdlib-meta-reflect-conformance.md`](stdlib-meta-reflect-conformance.md).
Other CONF cells and aggregate promotion remain pending. Existing codec interoperability
with `serde_json`, `rmpv` and `prost`, one-byte fragmentation tests and
bounded hosted fixtures remain valuable component evidence. They do not
establish public reflection, general metaprogramming, full testing integration
or native AOT conformance.

## Performance, lineage and release

The performance contract and coordinator record six workloads and eight
dimensions for ten portable owners, with three processes and 27 retained
samples per workload. Other owners point to their separate compiler/VM or
hosted measurement planes. Logical owned-buffer counters are not OS allocator
calls or RSS. These target-qualified measurements do not prove public route
completeness or native code size.

The live conformance lineage is `conformance/draft/manifest.json`, backed by
`conformance/0.1/manifest.json`. Both describe the same unpublished draft.
Local reports belong in the selected artifact directory through
`CARGO_TARGET_DIR` and the campaign's documented evidence override.

The current acceptance floor is 8,000 basis points (80%) for global line,
function and region coverage, as recorded in `testing/quality-baseline.json`.
Historical measurements remain unchanged; risk dimensions retain the lower
of 80% and their historical threshold. Mutation requirements remain separate.
Coverage, mutation,
conformance, current provenance and all reopened prerequisites must pass
before sealing S1A again. Publication additionally requires the explicit
human decisions and checklist in section 20 of
`TONDO_STANDARD_LIBRARY_SPEC.md`.
