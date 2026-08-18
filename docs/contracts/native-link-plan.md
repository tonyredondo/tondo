# Native link plan

`NATIVE-LINK-PLAN-001` closes the pure hand-off between the native artifact
graph and a future linker invocation. The compiler emits this record; an
orchestrator may consume it, but the record itself performs no filesystem,
process or environment lookup.

The format is `tondo-native-link-plan-draft`, compact UTF-8 JSON in declared
struct order. `content_hash` is the SHA-256 of those canonical bytes and
`plan_hash` is the semantic identity of all fields except itself. Input order
is meaningful and is never sorted by canonicalization.

## Shape

```json
{
  "format": "tondo-native-link-plan-draft",
  "compiler": "tondo-bootstrap/draft",
  "edition": "0.1",
  "package_id": "workspace:app@1",
  "target_descriptor_hash": "sha256:<64 lowercase hex>",
  "artifact_hash": "sha256:<64 lowercase hex>",
  "artifact_target_descriptor_hash": "sha256:<64 lowercase hex>",
  "inputs": [
    {
      "id": "object-prepared",
      "kind": "object",
      "sha256": "sha256:<64 lowercase hex>"
    },
    {
      "id": "privileged-console",
      "kind": "privileged-unit",
      "sha256": "sha256:<64 lowercase hex>"
    },
    {
      "id": "runtime",
      "kind": "runtime",
      "sha256": "sha256:<64 lowercase hex>"
    },
    {
      "id": "stdlib",
      "kind": "stdlib",
      "sha256": "sha256:<64 lowercase hex>"
    }
  ],
  "driver": {
    "id": "tondo-driver",
    "version": "draft",
    "artifact_id": "driver",
    "artifact_sha256": "sha256:<64 lowercase hex>",
    "arguments": ["--target=tondo-native-linux-x86-64"]
  },
  "output": {
    "product_id": "product",
    "object_format": "elf",
    "expected_sha256": "sha256:<64 lowercase hex>"
  },
  "limits": {
    "max_inputs": 64,
    "max_arguments": 64,
    "max_output_bytes": 1073741824
  },
  "plan_hash": "sha256:<64 lowercase hex>",
  "reproducible": true
}
```

`inputs` preserve the compiler's link order: all object inputs first, then
privileged units in their declared order, followed by exactly one runtime and
one standard library input. IDs are unique, every hash is a SHA-256 identity,
and the limits must be positive and cover the declared input and argument
counts.

The target descriptor hash is repeated as
`artifact_target_descriptor_hash` so a pure reader can reject a plan that
mixes target selections before resolving any bytes. `validate_against` then
checks the actual descriptor and `NativeArtifact`: exact driver identity and
ordered arguments, driver artifact hash, link-input IDs/kinds/hashes, package,
object format, product ID and expected product hash must all agree.

The driver is a logical, hash-pinned identity. Its arguments are ordered
tokens; physical paths, shell expansion, environment expansion, `PATH` lookup
and shell execution are forbidden. Output is also logical: `product_id`, object
format and expected hash are recorded, while `--output`, staging and atomic
publication belong only to `NATIVE-PUBLISH-SPEC-001`.

The implementation, machine-readable contract and negative tests are in
`crates/tondo-compiler/src/toolchain.rs`,
`testing/native-link-plan.json`, `scripts/native-link-plan-check.sh` and
`scripts/native-link-plan-test.sh`.
