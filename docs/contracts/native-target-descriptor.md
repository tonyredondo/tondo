# Native target descriptor

`NATIVE-TARGET-DESC-001` closes the first machine-readable contract of the
native backend lane. It does not choose Cranelift, LLVM or a code generator,
and it does not define the runtime ABI. It records the identities that a later
backend evaluation and link plan must consume.

The descriptor is `tondo-native-target-descriptor-draft`. It is compact UTF-8
JSON in declared struct order, rejects unknown fields, and is identified by
`sha256(canonical-descriptor-bytes)`. Arrays with set semantics are sorted and
unique; ordered tool arguments retain their declared order.

## Shape

```json
{
  "format": "tondo-native-target-descriptor-draft",
  "backend": {
    "name": "cranelift",
    "version": "draft",
    "implementation_hash": "sha256:<64 lowercase hex>"
  },
  "target": {
    "name": "tondo-native-linux-x86-64",
    "triple": "x86_64-unknown-linux-gnu",
    "profile": "release"
  },
  "object_format": "elf",
  "runtime_abi": "tondo-runtime-draft/1",
  "capability_registry": "tondo-capabilities-draft",
  "capabilities": ["console", "process"],
  "features": ["fast"],
  "flags": ["-Ctarget-cpu=generic"],
  "driver": {
    "id": "tondo-driver",
    "version": "draft",
    "artifact_id": "driver",
    "arguments": ["--target=tondo-native-linux-x86-64"]
  },
  "linker": {
    "id": "tondo-linker",
    "version": "draft",
    "artifact_id": "linker",
    "arguments": ["--build-id=none"]
  },
  "artifacts": [
    {"id": "driver", "kind": "driver", "sha256": "sha256:<64 lowercase hex>"},
    {"id": "linker", "kind": "linker", "sha256": "sha256:<64 lowercase hex>"},
    {"id": "runtime", "kind": "runtime", "sha256": "sha256:<64 lowercase hex>"},
    {"id": "stdlib", "kind": "stdlib", "sha256": "sha256:<64 lowercase hex>"}
  ]
}
```

`backend.implementation_hash` fixes the backend implementation. `driver` and
`linker` reference artifacts of the matching kind; every other toolchain input
is represented by a logical artifact ID and SHA-256. There is no physical
executable path, shell command, `PATH lookup`, environment map or unhashable
input in the record.

Target triples are lowercase canonical tokens. Object formats are currently
the closed set `elf`, `macho` and `coff`. Capabilities use the existing
`tondo-capabilities-draft` registry. Features and flags are sorted sets;
arguments are ordered tokens because their order can affect a driver.

The pure reader validates all references before a build can consume the
descriptor. It rejects pretty/non-canonical bytes, unknown fields, duplicate
or unsorted identities, invalid hashes, unsupported object formats, path-like
tool identities and `$`/`%`/backtick environment expansion. No selection may
consult `PATH` or the ambient environment.

This contract is deliberately narrower than the next records:

1. `NATIVE-ARTIFACT-001` closes the artifact graph and product identities.
2. `NATIVE-LINK-PLAN-001` closes the ordered link invocation.
3. `NATIVE-PUBLISH-SPEC-001` closes staging and atomic publication.

None of these records promises a public FFI ABI, object layout, dynamic
linking or a stable name mangling scheme.
