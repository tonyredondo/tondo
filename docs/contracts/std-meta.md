# `std.meta` companion contract

`std.meta` is the only public API that a provider or manifest generator uses to
consume a meta snapshot and return generated source. The 0.1 API is identified
by `tondo-std-meta-0.1/1` and is deliberately value-oriented: a request owns its
snapshot, inputs, output declarations and finite limits; a response owns its
generated sources.

## Request

`MetaRequest` contains:

- one canonical `MetaSnapshot` (`tondo-meta-model-0.1/1`);
- a sorted, duplicate-free list of named byte inputs, each hashed with
  SHA-256;
- a sorted, duplicate-free list of exact output `(path, module)` declarations;
  and
- positive `steps`, `memory_bytes` and `output_bytes` limits.

The request has no callback, capability, filesystem, environment, process,
clock, entropy, thread, suspension, FFI, unsafe or host-identity field. There is no
ambient lookup. Inputs are the only non-model values visible to the companion.

## Source builder and ownership

`MetaRequest::into_source_builder` consumes the request. The builder accepts a
source only when its path and module exactly match one declaration, the path is
relative, slash-separated and ends in `.to`, and the bytes are valid UTF-8. It
owns the accepted bytes, computes their hash, rejects duplicates and enforces
the aggregate output limit. `finish` consumes the builder and succeeds only if
every declared output appears exactly once. Outputs are returned sorted by
logical path; no partial response is published.

## Errors and determinism

Invalid limits/text/paths, duplicate or undeclared inputs/outputs, module drift,
invalid UTF-8, missing outputs and output-budget exhaustion are closed typed
errors. The caller cannot recover by reading ambient state. The snapshot and
all collections are already canonical before the companion runs, so equivalent
requests have one traversal and one output order.

The implementation lives in `crates/tondo-compiler/src/meta.rs`; it is a pure
contract layer and does not execute a provider. Execution and sandbox admission
belong to `META-VM-001`.

## Evidence and budgets

The owner contract is [`testing/stdlib-meta.json`](../../testing/stdlib-meta.json)
and the current cell-by-cell record is
[`testing/stdlib-owner-evidence.json`](../../testing/stdlib-owner-evidence.json).
They keep `MODEL`, `TEST` and `FUZZ` separate from the implementation proof.
`HOST` is explicitly `not-applicable`: this package is admitted to the
build-only `tondo-meta` graph and has no runtime host adapter. Compile-time and
generated-source-size budgets are declared with the standard performance
regression budgets; capture remains a promotion gate rather than an invented
runtime benchmark.

The executable evidence is bounded by `meta_steps`, `meta_memory_bytes`,
`meta_output_bytes`, renderer indentation, and the protocol probe input limit.
Run `scripts/stdlib-meta-check.sh` and
`scripts/stdlib-owner-evidence-check.sh` before the normative matrix check.
