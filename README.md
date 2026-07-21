# Tondo

Bootstrap workspace for the Tondo compiler.

The workspace is organized into three boundaries:

- `tondo-cli` owns the command-line entry point.
- `tondo-compiler` owns the lossless frontend and compilation pipeline.
- `tondo-vm` owns the verified bytecode contract and its runtime boundary.

The CLI recognizes `fmt`, `check`, and `run`. Source validation, Unicode 16
lexing, the lossless CST, recoverable parsing, the typed AST facade, and the
canonical formatter are implemented. Syntax diagnostics run before formatting
or semantic work. The closed package graph, deterministic name/member
resolution, visibility checks, public-API validation, and foundational
canonical type interner are also implemented. Source type expressions now lower
to semantic declarations and callable signatures, including aliases, generic
bounds, normalized unions, and recursive-productivity checks. The typed HIR now
checks the bootstrap core, including bounded and unbounded generic bodies,
invariant call inference, explicit specialization, and closed `Discard`
constraints. Trait declarations now retain a contextual `Self`, required and
associated methods, default bodies, and the intrinsic `Self: Send` condition of
async receivers. Defaults are checked once under the trait's binders and may
call other methods of that same trait without opening global method lookup.
Explicit implementations now have deterministic HIR identities, normalized
coherence headers, exact source/prelude method contracts, orphan checks, and
checked bodies. Defaults may be omitted or replaced; missing, extra, or
signature-drifting methods and manual implementations of closed protocols are
rejected with `E1114`. Coherence compares independently scoped generic headers
before resolving bounds, rejects ordinary overlap with `E1111`, and enforces
the unique `Iterator[T]` element for each target with `E1113`. Termination of
trait obligations and static dispatch remain later M4 work. The same HIR covers
constants, bindings, functions, inherent methods, blocks, conditionals, loops,
scalar operators, calls, `Option`,
`Result`, `fail`, `?`, every pattern form, and exhaustive guarded `match`, with
explicit coercions and structured diagnostics. Field and tuple-slot access,
array indexing/slicing, map lookup, array arithmetic, and simple, compound, and
multiple assignment are also typed with their evaluation order retained in
HIR. Reachability is explicit: `Never` propagates through structured control
flow, infinite loops distinguish reachable breaks by loop identity, and
unreachable statements or operands produce `W1006` without warning cascades.
Explicit `_ = value`, tuple discard leaves, and fixed discard parameters enforce
the structural `Discard` contract; terminal `Join` values produce `E1105` even
through generic nominal containers.

`CompilationOutput` now retains an immutable semantic snapshot after name
resolution. Embedding tools can query contextual expression types, resolved
entities and references, callable signatures, enum/union members, and closed
call error sets; partial snapshots state exactly which semantic phase completed.
Record construction/update, inherent method dispatch, closed generic-call inference,
range/membership checking, and compile-time constant evaluation are implemented
for the bootstrap subset. `tondo check` now succeeds when that entire subset is
understood. Complete HIR lowers through a verified typed MIR and then to
verified in-memory slot bytecode with source maps. Reached generic functions
are monomorphized deterministically; equal concrete substitutions share one
body, direct bytecode calls carry no runtime type pack, and expanding recursion
is stopped by an explicit request limit. `tondo run` executes a synchronous
explicit `main` in an iterative VM with checked operations,
normative panics, precise generational mark-and-sweep collection, defensive
limits, and a provisional capability-gated `std.console.print` host shim. Async
entry points, implicit script bodies, ownership analysis, and later language
milestones remain under construction, so the workspace identifies itself as a
bootstrap and does not claim full Tondo conformance.

## Project documentation

- `docs/architecture.md` describes the compiler pipeline and phase invariants.
- `docs/adr/` records accepted architectural decisions.
- `docs/contracts/` records bootstrap interfaces that later milestones build on.
- `docs/contracts/formatter.md` records the implemented formatting boundary.
- `docs/contracts/package-graph.md` records the closed M2 build input.
- `docs/contracts/resolution.md` records name, scope, member, and visibility
  resolution.
- `docs/contracts/hir.md` records declaration lowering and the typed-HIR subset.
- `docs/contracts/mir.md` records typed CFG lowering, unwind edges, and MIR
  admission invariants.
- `docs/contracts/bytecode.md` records the in-memory slot format, source maps,
  verifier, and tooling-only disassembler.
- `docs/contracts/vm-runtime.md` records the executable object, GC, panic, and
  admission model.
- `docs/contracts/bootstrap-host.md` records the provisional console shim and
  capability boundary.
- `docs/contracts/semantic-queries.md` records the request-owned tooling
  snapshot and CHECK-009 query boundary.
- `docs/contracts/types.md` records the canonical semantic type representation.

## Local validation

~~~text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps --locked
~~~
