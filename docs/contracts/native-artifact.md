# Native artifact graph

`NATIVE-ARTIFACT-001` closes the metadata that sits between a selected native
target descriptor and the link plan. It is a graph of immutable logical
identities, not a native ABI or an object-layout description.

The record uses `tondo-native-artifact-draft`, compact UTF-8 JSON in declared
struct order, and rejects unknown fields, unsorted sets and unreachable graph
members. `content_hash` is the SHA-256 of the canonical record bytes. The
record's `artifact_hash` is a semantic identity recomputed from all fields
except itself.

## Shape

```json
{
  "format": "tondo-native-artifact-draft",
  "compiler": "tondo-bootstrap/draft",
  "edition": "0.1",
  "package_id": "workspace:app@1",
  "target_descriptor_hash": "sha256:<64 lowercase hex>",
  "source_artifact_hash": "sha256:<64 lowercase hex>",
  "nodes": [
    {
      "id": "object-main",
      "kind": "object",
      "role": "input",
      "sha256": "sha256:<64 lowercase hex>",
      "producer": null
    },
    {
      "id": "object-prepared",
      "kind": "object",
      "role": "intermediate",
      "sha256": "sha256:<64 lowercase hex>",
      "producer": "prepare"
    },
    {
      "id": "privileged-console",
      "kind": "privileged-unit",
      "role": "input",
      "sha256": "sha256:<64 lowercase hex>",
      "producer": null
    },
    {
      "id": "product",
      "kind": "product",
      "role": "output",
      "sha256": "sha256:<64 lowercase hex>",
      "producer": "link"
    },
    {
      "id": "runtime",
      "kind": "runtime",
      "role": "input",
      "sha256": "sha256:<64 lowercase hex>",
      "producer": null
    },
    {
      "id": "stdlib",
      "kind": "stdlib",
      "role": "input",
      "sha256": "sha256:<64 lowercase hex>",
      "producer": null
    }
  ],
  "producers": [
    {
      "id": "link",
      "kind": "link",
      "inputs": ["object-prepared", "privileged-console", "runtime", "stdlib"],
      "outputs": ["product"],
      "sha256": "sha256:<64 lowercase hex>"
    },
    {
      "id": "prepare",
      "kind": "prepare",
      "inputs": ["object-main"],
      "outputs": ["object-prepared"],
      "sha256": "sha256:<64 lowercase hex>"
    }
  ],
  "product_id": "product",
  "artifact_hash": "sha256:<64 lowercase hex>",
  "reproducible": true
}
```

`nodes` and `producers` are sorted by `id`; producer `inputs` and `outputs`
are sorted sets. Input nodes are immutable objects, exactly one runtime and
exactly one standard library input, plus zero or more privileged units. An
intermediate is currently an object produced by `compile` or `prepare`. The
single output node is the product and its producer is the single `link`
producer. Every node and producer must be reachable backwards from that
product, and the producer dependency graph is acyclic.

`target_descriptor_hash` binds the graph to the complete backend/target
selection. `source_artifact_hash` binds it to the existing
`tondo-artifact-draft` metadata for the Tondo source and generated inputs.
Node and producer hashes are supplied by the closed orchestrator; the pure
reader validates their identity grammar but never opens a path to recompute
them. `NATIVE-LINK-PLAN-001` resolves those hashes into an ordered, closed link
record; it still does not resolve physical paths or execute a process.

No field contains a physical path, linker command, symbol name, object layout,
calling convention or FFI promise. `--output`, staging paths and publication
atomicity are closed by `NATIVE-PUBLISH-SPEC-001`; implementation proceeds
through `NATIVE-001`.

The implementation and negative contract cases are in
`crates/tondo-compiler/src/toolchain.rs`,
`testing/native-artifact.json`, `scripts/native-artifact-check.sh` and
`scripts/native-artifact-test.sh`.
