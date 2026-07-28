# Tondo

**Released:** Tondo 0.1 language edition, reference toolchain 0.1.0

**Conformant target:** `tondo-vm-hosted` / `hosted` / `[console, process]`

Reference workspace for the Tondo compiler.

The compiler and runtime are organized into three production boundaries:

- `tondo-cli` owns the command-line entry point.
- `tondo-compiler` owns the lossless frontend and compilation pipeline.
- `tondo-vm` owns the verified bytecode contract and its runtime boundary.

`tondo-conformance` is the implementation-independent runner, while
`tondo-reference-adapter` connects that protocol to the public compiler and VM
paths plus the isolated collector observations.

The CLI recognizes `fmt`, `check`, and `run`. Source validation, Unicode 16
lexing, the lossless CST, recoverable parsing, the typed AST facade, and the
canonical formatter are implemented. Syntax diagnostics run before formatting
or semantic work. The closed package graph, deterministic name/member
resolution, visibility checks, public-API validation, and foundational
canonical type interner are also implemented. Source type expressions now lower
to semantic declarations and callable signatures, including aliases, generic
bounds, normalized unions, and recursive-productivity checks. The typed HIR now
checks the bootstrap core, including bounded and unbounded generic bodies,
invariant call inference, explicit specialization, and the six closed
capabilities `Copy`, `Discard`, `Equatable`, `Key`, `Send`, and `Share`. Their
proof is structural and coinductive over recursive nominal types; generic and
opaque values expose only the bounds available to their caller. Named free
functions and associated operations without a receiver are first-class uniform
values. Generic functions specialize explicitly or from one exact expected
`fn(...)` type, while receiver methods remain unbound and calls through stored
values are positional. The HIR, MIR, bytecode verifier, monomorphizer, and VM
all preserve that exact contract, including statically selected source-trait
function values retained by constants. Trait declarations now retain a
contextual `Self`, required and
associated methods, default bodies, and the intrinsic `Self: Send` condition of
async receivers. Defaults are checked once under the trait's binders and may
call other methods of that same trait without opening global method lookup.
Explicit implementations now have deterministic HIR identities, normalized
coherence headers, exact source/prelude method contracts, orphan checks, and
checked bodies. Defaults may be omitted or replaced; missing, extra, or
signature-drifting methods and manual implementations of closed protocols are
rejected with `E1114`. Coherence compares independently scoped generic headers
before resolving bounds, rejects ordinary overlap with `E1111`, and enforces
the unique `Iterator[T]` element for each target with `E1113`. Generic
implementation bounds then pass the normative size-change termination analysis:
canonical query matrices are saturated inside trait SCCs, non-decreasing
idempotent cycles produce `E1112`, and all analysis work is explicitly bounded.
Constraint selection and static dispatch select one coherent implementation
without a runtime witness or vtable. The same HIR covers
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
the structural `Discard` contract; equality, membership, collection formation,
map lookup, and async receiver implementations enforce their corresponding
closed capabilities. Terminal `Join` values fail every closed capability even
through generic nominal containers.

Final homogeneous variadics are executable end to end. One unique final
`...T` is visible to its body as an immutable `Array[T]`; calls accept zero or
more individual values, evaluate them left to right, preserve static
homogeneity, and apply the ordinary Copy-or-Move rule to each element. The same
contract works for named functions, methods, concrete closures, contextual
closures, generics, and uniform indirect function values. The distinct
whole-array spread is also executable: a Copy array remains available and an
affine array moves as one owner, while positional and exact named forms reach
the same final pack without recopying every element.

The VM keeps eager logical copying as an executable reference strategy and uses
measured copy-on-write buffers by default for representation-safe Array, Map,
and Set leaves. The unchanged value-copy corpus runs eager and COW under normal
and allocation-by-allocation GC pressure, so storage sharing cannot alter
values, write independence, identity, iteration, panic, or output.

Async execution is implemented without a visible future wrapper. Calls with an
`async` signature must be initiated by `await` or, inside `scope`, by `spawn`.
The latter returns one affine, scope-bound `Join[T, E]`; HIR follows that handle
through bindings and containers, requires exactly one terminal consumption, and
keeps every structured `ref` loan active until the handle is awaited or torn
down. `Send` is checked for transferred and suspension-live values, while a
concurrently observed `ref T` also requires `Share`. MIR and bytecode retain
separate `Await`, `Spawn`, task-scope entry, and scope-drain operations. The VM
executes them with a cooperative single-thread scheduler, idempotent wakeups,
suspendible typed frame vectors, structured cancellation, sibling cleanup, and
deterministic child-panic propagation. Parked frames and completed child results
remain precise GC roots.

`CompilationOutput` now retains an immutable semantic snapshot after name
resolution. Embedding tools can query contextual expression types, resolved
entities and references, callable signatures, enum/union members, and closed
call error sets; partial snapshots state exactly which semantic phase completed.
The portable Tondo 0.1 view also exposes stable source-derived IDs, all six
capabilities, terminal origins, closure protocols, opaque results, iterator
proofs, borrow regions, MIR loan/check lifecycles, affine state events,
structured `Join` ownership, `unsafe` regions, sugar expansion, and canonical
formatter bytes without serializing request-local compiler handles.
Record construction/update, inherent method dispatch, closed generic-call inference,
range/membership checking, and compile-time constant evaluation are implemented
for the bootstrap subset. `tondo check` now succeeds when that entire subset is
understood. Executable collections include insertion-ordered maps and unique
sets with content-based equality, plus lazy integer and Unicode-scalar ranges
whose inclusive maximum does not require an overflowing successor. The closed
121-pair numeric conversion matrix also reaches execution. Checked conversions
return the intrinsic, exhaustively matchable `NumericConversionError`, with
identical classification in constants and at runtime. Fixed-width integer
arithmetic, division, remainder, shifts, bitwise operations, and their normative
panic classes are likewise executable. `Float32` and `Float64` preserve their
IEEE precision at every operation, including ties-to-even rounding, gradual
underflow, infinities, NaN, signed zero, and the prohibition on implicit FMA
contraction. Immutable strings retain valid UTF-8, exact scalar equality and
ordering, linear Unicode-scalar iteration, negative indexing to `Char`, and
array-compatible scalar slicing back to `String`; `String`, `Char`, `Byte`, and
`Array[Byte]` remain distinct. Opaque `Bytes` values cross only the
capability-gated process boundary and require an explicit fallible `.text()`
decode. Normal and multiline interpolation now decodes escapes
once, evaluates holes from left to right, and resolves the predeclared
`Display` trait statically. Scalar and `String` display use a closed bootstrap
intrinsic; user values call their selected implementation through a shared
receiver, so interpolation neither moves affine values nor introduces vtables.
Complete HIR lowers through a verified typed MIR and
then to verified in-memory slot bytecode with source maps. Reached generic
functions are monomorphized deterministically; equal concrete substitutions
share one body, direct bytecode calls carry no runtime type pack, and expanding
recursion is stopped by an explicit request limit. `tondo run` executes a safe
sync or async explicit `main`, or an inferred implicit script entry, in an
iterative VM with checked operations, normative panics, precise generational
handles, non-moving mark-and-sweep collection, defensive limits, and
capability-gated `std.console` and `std.process` host bridges. Root scripts
support shebangs, top-level statements, inferred closed error unions, and
automatic async entry when they use `await` or `scope`. `Command` and
`Pipeline` are inert copied
plans; only four closed `|` compositions exist, argv never passes through a
shell, and shell execution is explicitly named. Async terminal operations use
direct OS pipes, concurrent draining, typed status/output/errors, affine
`ProcessHandle` cleanup, and host workers that do not block runnable Tondo
tasks.

Unsafe effects are now explicit end to end. Functions and closures preserve the
four sync/async and safe/unsafe combinations; lexical `unsafe` regions admit
only matching callables and the six closed raw Pointer operations. Safe
closures cannot capture Pointer-containing values. HIR, MIR, bytecode, and
their independent verifiers preserve the effect bit and exact raw-operation
types, while the VM delegates dynamic pointer behavior only to a pinned
privileged target adapter rather than inventing a stable layout or FFI ABI.

Closed project builds are also implemented. A strict manifest and lockfile
select target, profile, capabilities, features, exact PackageIds, aliases,
source sets, sources, dependency interfaces, generator inputs, and privileged
units before lexing. The pure project planner accepts only declared
hash-matching bytes and performs no filesystem, environment, network, process,
or clock access. Versioned canonical interfaces reject incompatible compiler,
edition, package, target, capability, feature, module, source-set, or transitive
dependency identities before frontend work. Successful builds can emit
canonical interface and artifact metadata, including public API and complete
build-input hashes; identical declared inputs produce identical bytes.

Ownership already distinguishes contextual copies, affine moves, immediate
observations,
typed internal move paths, whole-binding availability across branches and
loops, complete reinitialization of moved `var` bindings, and affine closure
captures with all-exit `CallOnce` obligations. Terminal owners are also followed
through confirmed handoffs and rejected with `E1404` on every unconsumed normal
exit or `E1408` before overwrite. Synchronous `ref`, `mut`, and `var` arguments
use verified call-local loans with ordered reservation,
alias rejection, reborrowing, VM write-through, and explicit release on
abandoned argument paths. General last-use regions, collection-view loans,
fixed-versus-structural mutation enforcement, runtime overlap proofs, and all
four iteration forms are also verified through the VM. `for ref` observes
stable `Array`, `Map`, and `Set` places; `for mut` and `for var` update stable
writable `Array` and `Map` elements without exposing mutable keys or changing
the collection traversed by the cursor. User-defined `Iterator[T]` targets
retain one statically coherent element type. Synchronous `defer` now captures
its operands at registration,
drains lexical scopes in LIFO order on normal and panic exits, and follows or
disarms a unique affine guard through verified ownership transfers. Async
cleanup cannot itself suspend; structured task teardown runs before the defers
of the abandoned task scope and cannot leak cancellation into a recoverable
error type. This exact toolchain and target are covered by the portable Tondo
0.1 conformance suite; the claim does not extend to another backend, profile,
or capability set.

## Project documentation

- `TONDO_LANGUAGE_SPEC.md` is the normative language definition.
- `TONDO_TOOLCHAIN_SPEC.md` defines the implemented manifest, lockfile,
  interface, artifact, and privileged-unit formats.
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
- `docs/contracts/process-host.md` records the closed process API, scheduling,
  pipe, and cleanup boundary.
- `docs/contracts/unsafe.md` records unsafe-region proofs, raw Pointer
  operations, undefined behavior, and the privileged host boundary.
- `docs/contracts/targets.md` records the exact bootstrap target registry and
  capability set.
- `docs/contracts/semantic-queries.md` records the request-owned tooling
  snapshot and the complete Tondo 0.1 semantic serialization.
- `docs/contracts/diagnostics-json.md` freezes the machine-readable diagnostics
  format for Tondo 0.1.
- `docs/contracts/conformance.md` defines the portable suite, adapter boundary,
  and Gate G5 release checks.
- `docs/contracts/types.md` records the canonical semantic type representation.
- `docs/releases/0.1.0.md` records the exact release matrix, limitations, and
  reproducible conformance evidence.

## Prebuilt test binaries

The
[Build binaries](https://github.com/tonyredondo/tondo/actions/workflows/build-binaries.yml)
workflow builds and smoke-tests the public `tondo` CLI on native runners for:

- Linux x86_64 and ARM64;
- macOS Intel and Apple Silicon;
- Windows x86_64.

It runs for version tags and can also be started manually from GitHub Actions.
Each platform artifact is retained for 14 days and contains a native archive
plus its SHA-256 checksum. The archive includes the CLI, this README, and both
language and toolchain specifications. It also contains the same hello-world
program exercised by the workflow:

~~~text
./tondo --version
./tondo run examples/hello.to
~~~

On Windows, use `.\tondo.exe` in place of `./tondo`.

## Local validation

~~~text
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps --locked
cargo build -p tondo-conformance -p tondo-reference-adapter --bins --locked
cargo run -p tondo-conformance --locked -- validate \
  --root . \
  --manifest conformance/0.1/manifest.json
cargo run -p tondo-conformance --locked -- run \
  --root . \
  --manifest conformance/0.1/manifest.json \
  --adapter target/debug/tondo-reference-adapter \
  --output /tmp/tondo-reference-0.1.0.json
~~~
