# Architecture decision records

Accepted records describe the current implementation baseline. A later decision
does not edit history silently: it adds a new ADR that supersedes the old one.

| ADR | Decision | Status |
|---|---|---|
| [001](001-rust-implementation.md) | Rust implementation | Accepted |
| [002](002-handwritten-parser.md) | Handwritten lexer and parser | Accepted |
| [003](003-lossless-cst.md) | Lossless CST | Accepted |
| [004](004-recursive-descent-pratt.md) | Recursive descent plus Pratt parsing | Accepted |
| [005](005-compiler-pipeline.md) | CST to HIR to MIR to bytecode pipeline | Accepted |
| [006](006-slot-bytecode.md) | Slot-based bytecode | Accepted |
| [007](007-bytecode-vm-first.md) | Bytecode VM before native backend | Accepted |
| [008](008-explicit-values.md) | Explicit bootstrap value representation | Accepted |
| [009](009-tracing-gc.md) | Precise tracing GC for the bootstrap VM | Accepted |
| [010](010-cooperative-executor.md) | Single-thread cooperative executor first | Accepted |
| [011](011-eager-logical-copies.md) | Eager logical copies before COW | Accepted |
| [012](012-non-incremental-pipeline.md) | Deterministic non-incremental pipeline first | Accepted |
| [013](013-monomorphization.md) | Monomorphization for initial generics | Accepted |
| [014](014-in-memory-bytecode.md) | No stable serialized bootstrap bytecode | Accepted |
| [015](015-bootstrap-is-not-a-dialect.md) | Bootstrap subset is not a source dialect | Accepted |
| [016](016-verified-hir-mir-contract.md) | Verified HIR and explicit MIR effects | Accepted |

The detailed implemented object and tracing model selected collectively by
ADR-006 through ADR-011 is recorded in
[`docs/contracts/vm-runtime.md`](../contracts/vm-runtime.md).
The provisional standard-library boundary selected for DEC-007 is recorded in
[`docs/contracts/bootstrap-host.md`](../contracts/bootstrap-host.md).
