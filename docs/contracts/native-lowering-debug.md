# Native lowering debug contract

`NATIVE-LOWER-DEBUG-001` closes the identity boundary between verified MIR,
native candidate code and the diagnostics that consume it. The machine-readable
authority is [`testing/native-lowering-debug.json`](../../testing/native-lowering-debug.json).

## Canonical metadata

Every compiler-produced `tondo-mir-backend/1` program carries
`tondo-mir-debug/1` metadata. It has five deliberately small pieces:

- `sources` contains only a deterministic ordinal, module, logical path,
  source byte length and a `sha256:` content identity. Physical paths,
  addresses, process IDs and timestamps never cross the boundary.
- `symbols` covers every normalized function. Its `native` field is the exact
  exported candidate symbol (`tondo_probe_<ordinal>`), while `name` is the
  source-level logical identity with source IDs and physical paths removed.
- `source_maps` maps function, block, statement and terminator regions to
  logical byte ranges. Generated sources are first mapped through the source
  database's authorized diagnostic origin.
- `unwind` on a terminator region preserves the original MIR unwind successor,
  including cleanup blocks. The adapter validates that it names a block in the
  same function before code generation.
- `executions` gives every source `spawn` a deterministic `task` or `thread`
  identity. The identity is derived from the function/block region and is the
  join point used by race and dump tooling; it is not a runtime address.

The legacy `MirProgram::backend_program` API remains available for bounded
tooling and emits the same shape with synthetic `file/<ordinal>` sources. The
driver uses `backend_program_with_debug`, which supplies resolved logical
symbols and the real source inventory.

## Native boundary

The Cranelift and LLVM adapters validate the complete metadata before lowering.
Missing metadata, duplicate IDs, malformed logical paths or hashes, out-of-range
spans, missing blocks/unwind successors and unknown execution kinds fail closed.
LLVM preserves the relation in path-free debug records next to each exported
function; both candidate adapters retain the canonical native symbol. A trap is
therefore attributable to a MIR terminator and source range without depending
on the machine's checkout path.

The runner reports metadata counts per fixture and refuses to produce an
evaluation report when a fixture has no debug metadata. Runtime contract probes
use the same metadata shape, so task/thread identities cannot bypass the
diagnostic boundary merely because a function was created by the adapter.

## Reproducibility and privacy

Source ordinals are assigned by `(module, logical_path, content_sha256)` order,
not insertion order; exact duplicate logical sources share one ordinal. Source
spans are byte offsets and are checked against the recorded source length. A
generated source may point at its authorized origin,
but no physical origin is serialized. Symbol names omit `SourceId`; only module,
namespace and declaration identity remain.

The metadata is a transport contract, not a public object-layout or FFI ABI.
It carries enough identity for panic diagnostics, source maps, unwind, race
task/thread context and dump analysis while leaving storage and scheduler
implementation private.

## Verification

The compiler tests cover deterministic path-free inventories and metadata
serialization. The native adapter tests cover missing metadata, invalid unwind
targets and task/thread identity validation. Static and negative contract checks
are in `scripts/native-lowering-debug-{check,test}.sh`; the native evaluation
and runner reports provide the generated evidence.
