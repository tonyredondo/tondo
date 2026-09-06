# `std.yaml` testing contract

`STD-YAML-TEST-001` closes the independent model, hosted regression suite and
bounded fuzz boundary for the YAML 1.2 Core subset in the Tondo 0.1 draft. The
machine-readable record is [`testing/stdlib-yaml-test.json`](../../testing/stdlib-yaml-test.json).
The owner and implementation boundary remain in
[`stdlib-yaml.md`](./stdlib-yaml.md); this block does not promote a public API,
native runtime, native AOT lowering or an optimized parser.

## Independent bounded model

`crates/tondo-reliability/src/yaml_model.rs` implements a separate scalar Core
resolver and canonical value renderer. It does not call the production parser,
serializer or compiler bridge. The model covers null and boolean spelling,
signed and unsigned radix integers, finite floats, explicit binary values,
quoted text, UTF-8 key ordering, duplicate-key rejection and canonical
idempotence. Generated values are capped at 128 nodes and 96 bytes per sampled
scalar.

`crates/tondo-reliability/tests/yaml_models.rs` compares that oracle with the
scalar stdlib for values and errors, then exercises parse views, validation,
typed codecs, serialization conversions and the hosted event surface. It also
uses one-byte readers and terminal reader/writer operations so chunk boundaries,
ownership and close semantics are observable rather than inferred.

## Corpus, limits and security cases

The regression matrix covers multi-document streams, empty documents, comments,
Unicode and escapes, block and flow collections, literal and folded scalars,
core tags, padded `!!binary`, anchors and aliases, duplicate and non-string
keys, merge-key rejection, invalid directives, nesting and every declared input,
document, depth, node, scalar and collection limit. Alias bombs, forward
references, cycles, custom tags, includes and YAML 1.1 implicit booleans remain
negative cases; no case grants ambient lookup or code execution.

The persistent seed is `fuzz/corpus/stdlib_yaml/seed`. The fuzz target consumes
at most 4,096 input bytes and 512 deterministic actions. Each input is replayed
twice, compares the independent canonical renderer with production encoding,
parses the result and checks canonical re-encoding. The smoke runner uses 128
runs, seed 4107, `nightly-2026-07-28`, a ten-second per-input timeout and a
4-GiB RSS cap. The exact commands are recorded in the JSON contract and run by
[`scripts/stdlib-yaml-fuzz.sh`](../../scripts/stdlib-yaml-fuzz.sh).

## Promotion boundary

This block promotes only the independent model, scalar/hosted regression
evidence, persistent corpus and bounded fuzz harness. It does not claim native
AOT execution, SIMD equivalence or interoperability beyond the already
implemented hosted surface. The hosted scalar throughput, tail-latency,
allocation and adversarial baseline is closed separately by
`STD-YAML-PERF-001`; VM/native conformance is now closed separately by
`STD-YAML-CONF-001`, while usage documentation remains a separate leaf.
The conformance contract and report are
[`testing/stdlib-yaml-conformance.json`](../../testing/stdlib-yaml-conformance.json) and
[`docs/contracts/stdlib-yaml-conformance.md`](./stdlib-yaml-conformance.md).
The next block is `STD-YAML-DOC-001`.
Its performance contract is
[`testing/stdlib-yaml-performance.json`](../../testing/stdlib-yaml-performance.json),
with the promoted report recorded by its runner.
