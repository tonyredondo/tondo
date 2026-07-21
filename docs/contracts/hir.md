# Semantic and typed HIR contract

**Status:** bootstrap declarations, typed expressions, patterns, assignment,
discard, structured control flow, calls, semantic occurrences, and verified MIR
admission implemented

## Boundary

`hir::lower_types` is the first semantic lowering pass after name resolution;
`hir::check_expressions` adds checked bodies to that same request-owned program.
Both receive immutable source snapshots, parsed CSTs, the complete
`ResolvedProgram`, and explicit resource limits. Neither performs fallback
lookup or reads the filesystem, process environment, or global state.

The output owns:

- one request-local canonical `TypeInterner`;
- semantic type, alias, enum, record, newtype, and trait declarations;
- normalized generic binders and trait bounds;
- callable signatures for functions, methods, trait members, and `impl`
  members;
- receiver and parameter modes, including the body-visible `Array[T]` type of
  a variadic parameter;
- declaration-stable identities for opaque results;
- a source occurrence map from complete written type expressions to their
  lowered `TypeId`;
- typed constant initializers, their normalized compile-time values, and
  callable bodies in request-owned expression and pattern arenas;
- a static type, value category, source span, and resolved identity for every
  expression in the implemented subset;
- a bottom-up normal-completion summary and reachable loop-transfer targets for
  every expression, plus an exact source span for every statement;
- exact member-use occurrences recorded where field and enum-pattern selection
  becomes type-directed; and
- explicit HIR nodes for option/union coercions, `Result` construction,
  propagation, control transfers, `match` arms, every pattern form, field/index
  projections, assignment targets, and standalone explicit discard.

The checker deliberately leaves its completion flag false when it encounters a
surface whose semantics belongs to an unfinished phase. It checks unbounded
generic bodies and their invariant call inference, but does not yet claim
bounded generic, trait or `impl` bodies; constraint proof, closures, string
interpolation through `Display`, `defer`, ownership capabilities, or
trait-provided iteration. Those remain explicit later boundaries rather than
receiving provisional semantics.

## Typed expression invariants

Every successfully checked expression arena entry has one canonical `TypeId`,
one source `Span`, and one category: `Value` or stable `Place`. Every local used
by a checked body has a recorded type. Contextual conversions are represented
as nodes and are never inferred again by MIR.

A complete, error-free program is passed through `verify_typed_hir` before the
driver may accept it or lower it further. The verifier checks aligned metadata
arenas, canonical reachable types, topological child IDs, resolved declaration
kinds, category consistency, constants, callable bodies, loop targets, and all
expression/pattern/member references. A failure is an internal compiler error,
not a Tondo diagnostic. Partial snapshots may retain recovery nodes for tooling,
but they are never MIR input. The complete phase split is recorded in
`docs/contracts/mir.md` and ADR-016.

The implemented bootstrap subset includes:

- constants ordered by their dependency graph, simple `let`/`var` bindings,
  functions, and non-generic inherent methods;
- scalar, string, tuple and `none` literals with bidirectional expected types;
- blocks, `if`, all three `for` forms, `break`, `continue`, and `return`;
- intrinsic iteration for `Array`, `Map`, `Set`, `Range`, and `String`;
- prefix and closed scalar binary operators, discrete ranges, membership,
  direct and inherent calls, named and variadic arguments, and parameter modes;
- inferred or explicit generic function specialization when no unresolved
  bound proof is required;
- `some`, `ok`, `err`, implicit callable-success lifting, `fail`, and postfix
  `?` over both `Option` and `Result`;
- exact error propagation, injection into a union, and closed union-subset
  widening;
- wildcard, binding, borrowed binding, literal, tuple, record, enum, newtype,
  option, result, union-member, and array patterns;
- guarded `match` expressions with explicit arm HIR, irrefutability checks,
  unreachable-arm detection, exhaustiveness, and branch coercions;
- array, map, and set literals; nominal record, newtype, and enum construction;
  record `with` updates; nominal fields, tuple slots, array indices and slices;
  and map lookup/entry projections with instantiated types and value/place
  categories;
- map literals carrying an explicit dynamic-duplicate policy derived from the
  value type's closed `Discard` status;
- dedicated runtime `panic`, variadic `assert`, and provisional typed
  `std.console.print` operations rather than unresolved ordinary calls;
- a nonempty source representation of every `assert` condition, retained for
  the normative default panic message without keeping CST nodes alive;
- explicit numeric conversions classified as identity, total, or checked by
  the closed conversion table;
- scalar and closed element-wise array arithmetic;
- simple, compound, discard, and nested tuple assignment with target resolution
  before RHS evaluation, per-leaf coercions, left-to-right writes, static
  overlap rejection, and explicit replace-versus-preserve-extent requirements;
  and
- precise `Never` propagation through blocks, contextual coercions, calls,
  `if`, `match`, and loops, including mandatory `W1006` diagnostics; and
- explicit discard with a closed structural `Discard` proof for the implemented
  type subset.

A fallible callable is checked against two related expectations: its logical
success type and its complete `Result` type. A success expression receives one
explicit `ResultOk` node. An expression already having the complete result type
is retained unchanged, preventing double wrapping. Error propagation stores its
exact widening class for MIR.

Unsupported nominal iterator sources are deferred until trait resolution;
intrinsically invalid sources such as an integer receive `E1206`. This avoids
rejecting a future-valid `Iterator[T]` implementation while keeping the
bootstrap boundary observable through the completion flag.

Call arguments remain in source evaluation order while each HIR argument stores
its resolved receiver, fixed-parameter, variadic-element, or variadic-spread
target. Dot calls and qualified inherent calls therefore share one explicit
receiver representation without rewriting or reevaluating source expressions.
Unbounded generic calls use a request-local invariant solver, close every
inference variable before publishing HIR, and materialize a
`SpecializedFunction`; no inference variable crosses the expression boundary.

Range HIR distinguishes exclusive and inclusive ends and accepts only identical
integer or `Char` endpoint types. Membership HIR records whether it observes an
array element, map key, set member, range element, or string character. Both
retain left-to-right runtime evaluation even where bidirectional checking uses
the container type to select an item literal type.

Record construction, update, projection, and inherent calls enforce visibility
against the declaring module. External construction of a record with hidden
representation emits one non-revealing `E1502`; diagnostics for omitted fields
list only fields visible to the caller.

## Closed constant evaluation

Every acyclic constant is checked and evaluated after its dependencies. The
order is derived from complete `SymbolIdentity` values, not request-local
`SymbolId` allocation, so changing file insertion order cannot select a
different cycle primary or evaluation order. Strongly connected components
produce one `E1902` each; a constant downstream of a rejected component does
not receive a redundant cascade.

The evaluator consumes typed HIR with an explicit worklist and never executes a
Tondo function body. It accepts literals, prior constants, tuples, nominal
constructors and updates, options, results, arrays, maps, sets, ranges, named
function values, fully explicit generic function specializations, projections,
indexing, slicing, closed numeric conversions, and pure intrinsic operators.
Logical operators short-circuit. Element-wise array arithmetic checks every
nested length before producing a value. Integer overflow, zero division,
invalid shifts, invalid indices, zero slice steps, shape mismatches, and failed
checked conversions become `E1903`; calls, interpolation through `Display`, and
other runtime-only work become `E1901`.

Evaluated scalars retain their exact semantic payload: integers use a
mathematical signed representation constrained by their `TypeId`, and floats
store the IEEE value after the required `Float32` or `Float64` rounding. Values
for records and variants retain resolved member identities. Sets keep the first
equal value, maps retain source insertion order, and function values retain the
resolved callable plus complete type arguments.

A post-check scan evaluates collection keys and comparison operands on a
best-effort basis without executing dynamic expressions. Repeated known map
keys produce `E1116` with the first occurrence as a related location; repeated
known set values produce `W1011` and still normalize to one member. A scalar
comparison with a compile-time-known NaN produces `W1008`. Dynamic keys remain
a runtime concern and are not guessed equal by the compiler.

## Control flow and reachability

Expression types and control flow are deliberately separate HIR facts. A
contextual conversion may give a diverging expression the expected static type,
but its `HirFlow::Diverges` summary remains unchanged. Each summary is computed
bottom-up when the expression enters the arena and records only loop breaks
reachable in evaluation order.

Every loop receives a stable request-local `HirLoopId`. An infinite loop may
complete only when its body contains a reachable break targeting that exact
loop. Breaks consumed by nested loops, and breaks after a diverging expression,
do not count. Conditional and iterator loops may complete normally after their
header; a diverging header still makes the complete loop statement diverge.

Blocks stop accumulating normal flow at the first diverging statement or tail.
`if` diverges only when its condition diverges or both branches diverge;
`match` uses its scrutinee, guards, and all possible arm bodies. Logical
short-circuit operators retain a normal path when their left operand completes,
even if the right operand diverges.

After bodies are checked, a top-down worklist starts at constant and callable
roots. It follows the language evaluation order through statements, assignment
locations, RHS values, operands, arguments, branches, match arms, and loop
headers. The first unreachable statement or expression boundary receives
`W1006`; its subtree is not traversed, which prevents warning cascades. Invalid
`break` and `continue` without a loop target recover as potentially completing
after `E1205`, so they likewise cannot manufacture downstream warnings.

## Assignment lowering

An assignment statement owns one target tree, one RHS expression, and one
operator. A target leaf is either a resolved place or an explicit discard; tuple
targets preserve their written nesting and order. Every place retains its
field/index/slice expression, so receivers, keys, bounds, and indices are arena
nodes allocated before the RHS and cannot be regenerated by MIR. Compound
assignment retains the operator instead of lowering to a duplicated read and
write.

A map expression records whether dynamic duplicate keys use ordered
last-write-wins replacement or must produce `P0009`. MIR never recomputes this
choice from a runtime representation.

Each writable leaf records the conversion applied after tuple destructuring and
whether the write may replace the logical value or must preserve structural
extent. `mut` roots and slices require preservation; `var` roots and complete
strict subplaces may replace. A direct map entry is typed as `V`, distinct from
the ordinary lookup result `V?`; insertion requires `var`, and compound map
index assignment is rejected. Array arithmetic is lifted only for the five
normative numeric operators.

Statically inevitable overlap is rejected before HIR leaves the checker. Place
keys normalize constant integer, character, and string operands, and root/path
prefix overlap is included. Runtime-dependent disjointness for affine or
terminal transfers remains an ownership/MIR responsibility: the retained place
tree is the input to that later proof or runtime check. Likewise, a discard leaf
inside a multiple assignment is represented at its exact tuple position.

## Explicit discard

A standalone `_ = expression` is `HirStatement::Discard`, not an assignment to
a fabricated location. A `_` inside multiple assignment remains a discard leaf
because it participates in tuple destructuring. Both forms evaluate their value
in the ordinary written order and require `Discard`; a non-`Unit` expression
statement without either form receives `E1303`.

The bootstrap checker derives `Discard` structurally for scalars, functions,
tuples, options, results, unions, intrinsic collections, newtypes, records, and
enums. `Join[T, E]` is terminal and therefore makes every containing value fail
the proof. `Ref`, `Pointer`, `Command`, `Pipeline`, and the intrinsic numeric
conversion error are directly discardable under their well-formedness
contracts.

Nominal summaries are symbolic formulas over their generic parameter positions.
They are solved coinductively with a deterministic worklist before concrete
arguments are inspected. This handles mutual recursion and recursive argument
transformations without expanding an infinite family of type instances. A
fixed discard parameter passed by value uses the same proof; `ref`, `mut`, and
`var` discard parameters retain only their borrow contract. Generic bounds
`Discard`, `Copy`, and `Key` prove the requirement, while an unbounded generic
parameter produces `E1105`.

The resulting status for every interned type is stored in an arena aligned with
the type interner. Later target validation consumes this semantic fact rather
than duplicating the structural algorithm; in particular, a hosted fallible
`main` admits its error type only when the status is `Satisfied`.

Opaque results, generated closure environments, cursors, and source-less
nominals remain deferred until their published capability contracts exist.
General move tracking, implicit scope-end obligations, and terminal operations
remain the ownership phase's responsibility; the checker does not infer them
from successful explicit discard.

## Declaration ordering

Lowering is independent of textual declaration order and file insertion order.
It first indexes every resolved declaration, then analyzes transparent alias
dependencies, declares all generic binders, and only afterwards lowers bounds
and declaration bodies. `Self` is therefore available in trait, inherent, and
`impl` generic bounds without depending on the order in which syntax happened
to be visited.

Request-local `SymbolId`, `MemberId`, `LocalId`, and `TypeId` values are not
observable identities. Public comparisons and diagnostics use complete symbol
identity and canonical type serialization.

The request-owned arenas also expose deterministic ID-plus-node iteration and
exact, covering, and offset-based expression lookup. The public snapshot and
the distinction between request-local handles and serialized identities are
specified in `docs/contracts/semantic-queries.md`.

## Source type lowering

Every accepted spelling reaches the same canonical type graph:

- `Int64`/`Int` and `Float64`/`Float` share scalar nodes;
- `Option[T]` and `T?` share an option node;
- `Result[T, E]`, `T ! E`, and `!E` share a result node;
- tuple, function, mode, variadic, async, and unsafe information is preserved;
- intrinsic constructors have a closed arity table;
- records, enums, and newtypes retain nominal identity; and
- aliases are substituted completely and never enter `TypeKind`.

Generic parameters receive complete positions in their enclosing callable or
declaration binder. Bounds are deduplicated and sorted after lowering. A value
type used as a trait, a trait used as a value, an invalid arity, and malformed
declaration structure produce semantic diagnostics instead of recovery types
with invented meaning.

For a source-less dependency module, resolution can currently provide only an
external symbol identity. Such a type remains an opaque nominal application and
such a bound remains an external trait reference. Generic arity and declaration
kind for compiled dependencies will become checkable when the versioned module
interface of M9 exists; source modules present in the request are always checked
against their real declaration now.

## Recovery

The internal recovery type suppresses dependent cascades but has no canonical
name. If any child of a tuple, union, option, result, application, or function
signature is recovery, the containing public type occurrence also becomes
recovery. Internal type errors are propagated as `HirError`; they are not
silently converted into source diagnostics.

HIR semantic errors preempt the driver's `T0001` marker. A complete module-mode
`Operation::Check` succeeds and retains warnings in its report; script and
fragment checks, or an incomplete semantic surface, advance to `T0001` until
their later milestones are implemented. Exhausting the
type-node, combined typed-expression/pattern-node, pattern-analysis-work, or
diagnostic budget becomes `T0002`, using the same public driver as ordinary
`tondo check` requests.

## Pattern analysis

Pattern paths resolve through the type namespace, including imported nominal
types, contextual generic arguments, and transparent generic aliases used as
union discriminators. Explicit arguments must instantiate exactly the
scrutinee member; omitted nominal arguments are recovered from the scrutinee.
Literal coverage compares decoded scalar values, so alternate escape, raw, and
numeric spellings cannot evade overlap detection.

Usefulness is a deterministic matrix algorithm over constructor domains.
Finite domains include `Unit`, `Bool`, options, results, tuples, nominal
records/newtypes/enums, structural unions, and the empty/cons decomposition of
arrays. Open scalar domains require a wildcard. Guards are typechecked but do
not contribute coverage. Array prefixes remain flat and shared in the internal
pattern shape, and the matrix algorithm uses an explicit worklist, so wide
patterns do not recurse on the Rust process stack.

## Recursive declarations

Transparent aliases form a separate dependency graph. One `E1106` is emitted
per cyclic strongly connected component and every alias in that component
lowers to recovery.

Nominal recursion follows the specification's least-fixed-point rule. Only
cyclic nominal components require the productivity test. The evaluator:

- treats tuple fields, record fields, newtype payloads, enum payloads, and
  `Ref[T]` as requiring their children;
- accepts an enum when at least one variant payload is finite;
- recognizes `none`, empty collections, functions, and other non-immediate
  constructors as finite bases;
- substitutes actual generic arguments before deciding productivity; and
- reports one `E1107` for each cyclic component lacking a finite value.

Dependency discovery, SCC construction, substitution, canonical rendering, and
productivity evaluation use explicit worklists. Deep valid or invalid type
graphs therefore consume the request's declared node budget instead of the Rust
host stack.

## Validation

Tests cover canonical spelling equivalence, complete generic substitution,
alias SCCs, arity and trait/value misuse, discriminated-union overlap,
receiver/variadic semantics, bounds and contextual `Self`, opaque results,
recovery propagation, productive and nonproductive mutual recursion, generic
instantiations that remove a finite base, deep nominal graphs, and independence
from file insertion order. Expression tests cover contextual literal types,
explicit coercions, value categories, calls and modes, all loop forms, control
transfers, error constructors, both propagation channels, union widening,
constant cycles, invalid discards, and non-cascading recovery. Pattern tests
cover all constructors, nested finite domains, decoded literals, arrays,
guards, imported/generic paths, union discrimination, refutability,
exhaustiveness, unreachable arms, direct control transfers, and a 4,096-element
array prefix. Member-occurrence tests cover value projections, assignment
places, enum variants, and record-pattern shorthand. Assignment tests cover
every compound operator, partial tuple context, swaps, nested targets,
normalized static overlap, fields, tuple slots, arrays, slices, maps,
mutability modes, and target-before-RHS ordering. Call tests cover named
association, both variadic spread forms, receiver lowering, method permissions,
explicit and inferred generic specialization, inference conflicts, and unsolved
variables. Construction tests cover every nominal shape, contextual generic
instances, `with`, numeric conversions, ranges, membership, and cross-module
visibility without leaking omitted private field names. Driver
tests prove that semantic diagnostics and all HIR resource limits are observable
through the public compilation path. Reachability tests cover infinite,
conditional, iterator, and nested loops; reachable and dead breaks; divergent
headers; all-diverging and partially completing `if`/`match` joins; contextual
`Never` conversions; ordered nested operands; warning de-cascading; and warning
retention across the public driver boundary. Discard tests cover dedicated HIR,
implicit-result rejection, direct and nested `Join`, generic nominal
substitution, recursive and argument-transforming declarations, 512 nominal
layers, multiple-assignment leaves, borrow-only discard parameters, generic
bounds, and public-driver `E1105` propagation. Dedicated admission tests mutate
otherwise valid HIR to prove rejection of incomplete/recovery state,
noncanonical types, non-topological edges, misaligned flow metadata, missing
local types, and invalid value categories.
