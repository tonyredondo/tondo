# Semantic and typed HIR contract

**Status:** bootstrap declarations, typed expressions, generic specialization,
uniform named function values, static trait selection, declaration-owned opaque
results, patterns, assignment, discard, structured control flow, calls,
semantic occurrences, and verified MIR admission implemented

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
- a deterministic implementation table containing each normalized trait/target
  header, header binders, module owner, source-ordered method, and instantiated
  method contract;
- receiver and parameter modes, including the body-visible `Array[T]` type of
  a variadic parameter;
- declaration-stable opaque-result identities, normalized public bounds, family
  arguments, and one compiler-private witness per checked declaration;
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
  projections, assignment targets, standalone explicit discard, and exact
  named-function specializations.

The checker deliberately leaves its completion flag false when it encounters a
surface whose semantics belongs to an unfinished phase. It checks bounded and
unbounded generic function bodies, invariant call inference, explicit
specialization, static source/prelude trait selection, declaration-owned opaque
results, trait default bodies, exact implementation bodies, trait-provided
iteration, and the closed structural capabilities `Copy`, `Discard`,
`Equatable`, `Key`, `Send`, and `Share`. Concrete external implementations,
closures and their callable capabilities, string interpolation through
`Display`, `defer`, ownership availability, and executable async bodies remain
explicit later boundaries rather than receiving provisional semantics.

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
  functions, and inherent methods;
- empty and generic traits with required receiver methods, associated
  operations, default bodies, contextual `Self`, and the intrinsic `Self: Send`
  marker on async receiver methods;
- generic and concrete `impl` declarations with normalized coherence headers,
  module-based orphan checks, exact source/prelude contracts, omitted or
  replaced defaults, and checked implementation bodies;
- scalar, string, tuple and `none` literals with bidirectional expected types;
- blocks, `if`, all three `for` forms, `break`, `continue`, and `return`;
- intrinsic iteration for `Array`, `Map`, `Set`, `Range`, and `String`;
- prefix and closed scalar binary operators, discrete ranges, membership,
  direct and inherent calls, named and variadic arguments, and parameter modes;
- inferred or explicit generic function specialization, including closed
  intrinsic-capability obligations and forwarding through an enclosing generic
  binder;
- first-class free and receiver-free associated functions with one exact
  uniform `fn(...)` type, including contextual generic specialization and
  qualified source-trait associated operations;
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
- opaque success results on free, inherent, and associated functions, including
  generic families, an optional exterior error channel, exact witness inference,
  published trait use, and explicit representation-sealing coercions;
- map literals carrying an explicit dynamic-duplicate policy derived from the
  value type's closed `Discard` status;
- formation checks requiring `Map[K, V]` and `Set[K]` keys to satisfy `Key` and
  `Ref[T]` targets to satisfy `Discard`, in declarations and body-local types;
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
- explicit discard, structural equality, collection membership, map lookup, and
  async receiver implementations checked against the common closed-capability
  proof.

A fallible callable is checked against two related expectations: its logical
success type and its complete `Result` type. A success expression receives one
explicit `ResultOk` node. An expression already having the complete result type
is retained unchanged, preventing double wrapping. Error propagation stores its
exact widening class for MIR.

Iteration over `Array`, `Map`, `Set`, `Range`, and `String` records the closed
intrinsic protocol and its exact element type. Every other source must satisfy
one visible `Iterator[T]` assumption or the unique admitted `Iterator[T]`
implementation for its target. The functional target-to-element rule derives
`T`, and HIR stores a `Trait` iteration protocol containing both that element
and the exact `fn(mut Self): T?` type. A source with neither protocol receives
`E1206`; it is no longer accepted through an incomplete bootstrap path.

Call arguments remain in source evaluation order while each HIR argument stores
its resolved receiver, fixed-parameter, variadic-element, or variadic-spread
target. Dot calls and qualified inherent calls therefore share one explicit
receiver representation without rewriting or reevaluating source expressions.
Generic calls use a request-local invariant solver, close every inference
variable before publishing HIR, and materialize a `SpecializedFunction`; no
inference variable crosses the expression boundary. Explicit type arguments
may refer to the enclosing binder through composite spellings such as `T?` or
`Array[T]`. An explicit member specialization supplies only method-local
arguments; owner arguments and the trait's hidden `Self` position remain fixed
or are inferred from the receiver. The preliminary bracket remains
contextually resolved until the checker classifies it as an index or a
specialization.

## Uniform named function values

A named free function or associated operation without `self` becomes a value
with its structural function type. Parameter names are erased, while parameter
modes, variadic shape, `async`, `unsafe`, success, and error types remain part
of that type. A receiver method is never converted into a bound function:
`value.method` and `Type.method` without an immediate receiver call are rejected
and a later closure must bind the receiver explicitly.

`Function` represents only a non-generic named callable. A generic callable
must become `SpecializedFunction` before a complete HIR program is admitted.
Explicit syntax such as `identity[Int]` supplies the complete argument vector.
Otherwise the checker creates one invariant inference problem from the complete
expected function type, requires every callable binder to have one solution,
proves all substituted bounds, and writes the resulting monomorphic type and
argument vector into the original expression node. An enclosing rigid generic
binder may be that solution; no inference variable or independently polymorphic
function value may escape.

The same contextual rule applies in local and constant initializers, returns,
arguments, aggregate elements, record fields, and record shorthand. Calls
through a local, parameter, field, or constant of function type use positional
indices only. A direct call whose callee is still resolved by name retains
source parameter labels and performs ordinary generic call inference instead.

Associated function values may infer their nominal owner arguments from the
expected function type or fix owner and method-local arguments explicitly.
Qualified source-trait associated values require the written trait arguments,
explicit `Self`, and method-local arguments, and prove the complete trait query
before publishing the value. Their HIR operand still names the source trait
member; static implementation selection remains the monomorphization boundary.

`verify_typed_hir` rejects a generic `Function`, a specialization with
incomplete arity, and any `SpecializedFunction` whose expression type differs
from the exact substitution of its callable signature. The capability verifier
also rechecks closed generic bounds. MIR therefore receives neither an open
function value nor a type assertion that needs contextual reinterpretation.

## Trait declarations, defaults, and implementations

A trait declaration owns one contextual `Self` type immediately after its
written generic parameters. That hidden position participates in the complete
callable arity but is not exposed as a source binder. Method-local generics
follow it, so a trait `Catalog[T]` with `fn choose[U]` has the complete positions
`T = $0`, `Self = $1`, and `U = $2`.

HIR stores every trait member in strict `MemberId` order together with whether
it has a default body and whether an async receiver imposes `Self: Send`.
Required methods have a signature but no checked body. Associated operations
without `self` use the same representation and may themselves have defaults.
The admission verifier requires the table to match resolution exactly, checks
owner and receiver classification, preserves the trait-generic prefix, and
rejects inconsistent arity, default-body, or async requirement metadata.

Each default body is checked once with rigid trait parameters and contextual
`Self`. A receiver call may select only another receiver method declared by the
same trait; it does not search unrelated traits or concrete implementations.
Both inferred calls such as `self.choose(value)` and explicit calls such as
`self.choose[Int](value)` produce a complete `SpecializedFunction` argument
vector. This declaration-time lookup remains distinct from the static dispatch
performed for an instantiated call. Implementation validation has separate
contract and program-wide coherence admission passes.

Implementation declarations are indexed by stable logical source identity
(`SourceId`, module path, logical path, then start byte), never by request-local
`FileId`. Each method receives a source-ordinal ID under that implementation.
The table retains the declaring module, normalized target, complete trait
reference, header binders, method names and spans, and an optional instantiated
contract while recovering. An error-free implementation must have a contract
for every stored method and a complete-contract flag.

Contract admission performs these checks before any body is typechecked:

- every header binder occurs in the normalized target or complete trait
  arguments; occurrence only in a bound produces `E1114`;
- the current module owns either the trait or the outer nominal constructor of
  the target after alias expansion; structural targets acquire no ownership;
- every required method appears exactly once, a default may be omitted or
  replaced, and no extra method is accepted;
- after substituting trait arguments, contextual `Self`, and method-local
  binders, function type, receiver classification, generic arity, unordered
  bound sets, parameter modes and positions, variadic element, `async`,
  `unsafe`, success, and error are exact; and
- `Display` and `Iterator[T]` synthesize their language-owned contracts, while
  `Copy`, `Discard`, `Equatable`, `Key`, `Send`, `Share`, `Call`, `CallMut`, and
  `CallOnce` reject manual implementations.

After every individually complete contract is materialized, coherence groups
implementations by the resolved trait identity and compares pairs in stable
implementation-ID order. Generic positions belong to independent binder scopes
on each side; all trait arguments followed by the target share one first-order
substitution, while positive bounds never select between candidates. Aliases and
shorthands are already absent from the canonical types and structural unions are
matched as normalized unordered sets. A unifiable complete header emits `E1111`
at the later implementation with the earlier declaration as related evidence.
Invalid contracts do not participate, preventing a secondary overlap cascade.

`Iterator[T]` first unifies only the two targets. If they do not unify, both
implementations are independent. If they unify and their element arguments are
already identical under that most-general substitution, ordinary coherence
emits `E1111`; otherwise the functional target-to-element rule emits `E1113`.
This pass runs before any constraint proof, so adding or removing positive
bounds cannot change coherence.

Termination admission runs only after the complete implementation table passes
coherence, preventing overlap errors from cascading into cycle diagnostics. A
generic implementation contributes one edge from its complete normalized
header query —trait arguments followed by target— to every open source or
prelude trait bound on a header binder. The destination query contains the bound
arguments followed by that binder. Closed `Copy`, `Discard`, `Equatable`, `Key`,
`Send`, `Share`, `Call`, `CallMut`, and `CallOnce` bounds create no edge.

Each edge owns a size-change matrix whose rows are destination components and
whose columns are source components. Exact canonical terms are `=`, strict
structural subterms are `<`, and every other relation is `?`. Matrix composition
implements the normative strongest-path algebra, and a deterministic worklist
saturates the finite matrix set inside each SCC of the trait-identity graph.
Every idempotent matrix returning to its source trait must contain `<` on its
diagonal. A failing SCC emits one `E1112` with the complete reconstructed trait
path, the non-decreasing matrix, and related spans for the other contributing
implementations. Acyclic edges need no decrease.

Matrix construction, structural walks, compositions, idempotence checks, and
witness expansion consume an explicit ceiling derived from the request's trait-
obligation limit; exhaustion is `T0002`, never partial admission or a panic.
The algorithm is iterative over type graphs, SCCs, saturation, and witnesses.
The admission verifier rebuilds every edge and repeats the complete termination
proof independently before HIR can cross into MIR.

Parameter and generic-binder spellings are intentionally absent from this
comparison. `Display` requires `fn display(self): String`; `Iterator[T]`
requires `fn next(mut self): T?`. A trait default remains a generic template;
omitting it does not create an implementation callable. A replacement is an
ordinary implementation body and is checked once under the implementation
binders.

The admission verifier independently reconstructs each expected signature and
method-generic bound set from the source or prelude trait. It also rechecks
orphan ownership, deterministic IDs, generic prefixes, required/default
coverage, table/callable correspondence, receiver metadata, and the propagated
`Self: Send` requirement. The common capability engine proves that every
concrete implementation target satisfies `Send`; the same implicit bound is
available while checking a generic caller and when an opaque result publishes
such a trait. The verifier independently repeats that proof and also reruns ordinary and
`Iterator[T]` coherence and size-change termination over the admitted table.

## Static trait selection and calls

Method syntax never scans the program-wide implementation table by name.
Lookup first prefers an inherent method, then the current trait's own members
while checking a default, and finally methods supplied by exact trait
assumptions visible on the receiver type. Two visible constraints providing the
same name produce `E1004` and require a qualified call. Source traits can be
qualified through their resolved module path; `Display.display(value)` and
`Iterator[T].next(mut value)` are the corresponding prelude forms.

Every qualified call constructs one canonical trait query after closing trait,
hidden `Self`, and method-local arguments. Constraint calls reuse their already
canonical assumption. A query succeeds from an exact visible assumption or by
selecting the unique coherent implementation and recursively proving its
substituted header bounds. Missing evidence produces `E1105`; re-entering an
admitted query or observing two candidates is an internal invariant failure,
because coherence and size-change termination have already been proved.

HIR retains source-trait calls as complete specialized member operands and
uses `PreludeTraitFunction` for the language-owned `Display.display` and
`Iterator.next` contracts. Neither form contains a runtime witness, vtable, or
type pack. The admission verifier checks canonical complete arguments and the
exact closed function type. A `for` using `Iterator[T]` retains the same
contract in its `Trait` iteration protocol, so MIR never repeats lookup or
infers the yielded element.

## Opaque static results

An admitted `impl Bound` annotation belongs to exactly one free, inherent, or
associated function declaration. It is not a general type expression: using it
in a parameter, field, alias, function-value type, closure, trait member, or
trait implementation member is rejected as `E0004`. An async declaration may
retain the same opaque contract even though executable async bodies belong to
M7.

The type interner represents the result as one nominal family keyed by the
callable's stable `SymbolIdentity` plus its invariant generic arguments. The
ordinary outcome is that opaque type; a written `! E` wraps it as
`Result[Opaque, E]` without hiding the error channel. Two calls at the same
specialization therefore share one identity, while distinct concrete generic
arguments remain distinct opaque types.

While checking the declaring body, HIR uses one body-local inference variable
for the concrete success witness. Every reachable normal `return` and final
success expression must equate exactly with that witness after aliases. No
option lift, union widening, function coercion, or other implicit conversion is
allowed to choose a common representation. `Never` paths and fallible `err`
paths do not contribute a witness, and at least one reachable normal success
path must do so. Contextual empty `Array`, `Map`, and `Set` literals may infer
their element types through this same single constraint system. An unsolved,
recursive, divergent-only, error-only, or inconsistent body produces `E1117`.

After inference, the checker proves every normalized published bound against
the concrete witness under the declaration's own generic assumptions. The
closed implication lattice and structural engine apply to all six intrinsic
capabilities; source traits, `Display`, and `Iterator[T]` use the ordinary
static selection proof. A published source trait with an async receiver also
implies `Send`. The contract itself must publish or imply `Discard`. Callers
retain only the opaque
identity, generic arguments, exterior error, and published bounds: the witness
does not participate in ordinary name or inherent-method lookup.

Every successful exit contains an explicit `Assignability::Opaque` coercion
from the witness to the declared opaque success. This is a compile-time seal,
not an ordinary assignability rule. The admission verifier independently checks
declaration identity, family arguments, normalized duplicate-free bounds, the
single finite witness, exact seals, reachable success, and absence of direct or
mutual opaque representation cycles. It then re-proves all published bounds
without trusting the expression checker's result. Public HIR access exposes the
contract but keeps the witness accessor crate-private.

## Generic constraints

Every specialization validates the selected callable's bounds after inference
or explicit type parsing and before publishing the specialized function node.
Bound argument types are fully substituted at that boundary, so a callable-
local generic or inference variable cannot escape as an apparently closed
obligation. Each attempted proof consumes the request's trait-obligation
budget.

All six intrinsic constraints use one closed structural proof. A concrete
argument must satisfy the requested capability; a rigid generic argument must
publish a bound that directly or transitively implies it. `Copy` implies
`Discard`, while `Key` implies `Copy`, `Equatable`, and therefore `Discard`.
There are no other intrinsic implications. Missing forwarded bounds and
terminal `Join` values produce `E1105` for explicit calls, inferred calls, and
specialized function values alike.

Source traits and the open prelude traits `Display` and `Iterator[T]` use the
static proof and selection rules above. An exact enclosing bound is sufficient
for a rigid generic parameter; concrete types require an admitted
implementation, including all recursively substituted header bounds. External
trait assumptions can likewise be forwarded, but no concrete external
implementation is fabricated locally. A source trait containing an async
receiver method implies `Self: Send`; that implication is available to generic
callers and opaque contracts and is required of every concrete implementation
target.

`Call`, `CallMut`, and `CallOnce` are not part of this structural engine. They
remain deferred until closure bodies and capture modes can determine them; the
driver never treats a deferred callable capability as satisfied.

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

## Closed intrinsic capabilities

One engine derives `Copy`, `Discard`, `Equatable`, `Key`, `Send`, and `Share`.
Its source-level matrix is:

| Type | Closed capability conditions |
| --- | --- |
| Scalars, `Unit`, `Never` | All six, except `Float` and `Float32` are not `Key` |
| `fn(...)` | `Copy`, `Discard`, `Send`, and `Share`; not `Equatable` or `Key` |
| Tuple, union, `Option`, `Result`, nominal value | Componentwise for the requested capability |
| `Array[T]` | Componentwise except that an array is never `Key` |
| `Map[K, V]` | `Copy` when `K: Key` and `V: Copy`; `Discard`, `Equatable`, `Send`, and `Share` componentwise; never `Key` |
| `Set[K]` | `Copy` when `K: Key`; other non-`Key` capabilities componentwise; never `Key` |
| `Range[T]` | Componentwise `Copy`, `Discard`, `Send`, and `Share`; not `Equatable` or `Key` |
| `Ref[T]` | Always `Copy`, `Discard`, `Equatable`, and `Key` once well formed; `Send` and `Share` both require `T: Send + Share` |
| `Pointer[T]` | `Copy` and `Discard` only |
| `Join[T, E]` | None of the six |
| `Command`, `Pipeline` | `Copy`, `Discard`, `Send`, and `Share` |
| `NumericConversionError` | All six |

Nominal summaries are symbolic formulas over their generic parameter positions.
They are solved coinductively with a deterministic worklist before concrete
arguments are inspected. This handles mutual recursion and recursive argument
transformations without expanding an infinite family of type instances. An
opaque result exposes only capabilities published or implied by its bounds; its
private witness cannot leak extra facts to callers. Generated closure
environments and cursors remain `Deferred` until their owning tasks publish
their concrete contracts. A source-less nominal is likewise deferred.

The checker stores `Satisfied`, `Unsatisfied`, or `Deferred` for all six columns
of every interned type in an arena aligned with the type interner. The HIR
admission verifier reconstructs the summaries, recomputes the complete table,
and independently rechecks every operation that consumes it. MIR receives the
verified decision; the bytecode verifier derives the capabilities needed by
closed executable operations again from its own type and nominal catalogs.

Type formation requires `K: Key` for every `Map[K, V]` and `Set[K]`, and
`T: Discard` for every `Ref[T]`, including declaration signatures, nominal
fields, generic definitions, and inferred body-local types. Equality requires
identical `Equatable` operand types. Array membership requires an `Equatable`
element; map and set membership require a `Key`; map lookup additionally
requires `V: Copy`. Dynamic duplicate replacement in map literals remains
available only when the displaced value is `Discard`.

A standalone `_ = expression` is `HirStatement::Discard`, not an assignment to
a fabricated location. A `_` inside multiple assignment remains a discard leaf
because it participates in tuple destructuring. Both forms evaluate their value
in the ordinary written order and require `Discard`; a non-`Unit` expression
statement without either form receives `E1303`. A fixed discard parameter passed
by value uses the same proof; `ref`, `mut`, and `var` parameters retain only
their borrow contract. In particular, a hosted fallible `main` admits its error
type only when its `Discard` status is `Satisfied`.

General move tracking, implicit scope-end obligations, callable capabilities,
and terminal operations remain their owning phases' responsibility; successful
closed-capability proof does not fabricate ownership availability or cleanup.

## Declaration ordering

Lowering is independent of textual declaration order and file insertion order.
It first indexes every resolved declaration, then analyzes transparent alias
dependencies, declares all generic binders, and only afterwards lowers bounds
and declaration bodies. `Self` is therefore available in trait, inherent, and
`impl` generic bounds without depending on the order in which syntax happened
to be visited. All trait signatures are materialized before implementations are
matched, including when the trait lives in a later logical file. Implementation
IDs then follow stable logical-source order and method IDs follow source order
inside their owner, so changing source insertion order cannot change a callable
identity.

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
such a bound remains an external trait reference. A source-less external trait
cannot yet admit an `impl`: exact checking produces `E1114` instead of guessing
its methods. Generic arity, declaration kind, and contract data for compiled
dependencies will become checkable when the versioned module interface of M9
exists; source modules present in the request are always checked against their
real declaration now.

## Recovery

The internal recovery type suppresses dependent cascades but has no canonical
name. If any child of a tuple, union, option, result, application, or function
signature is recovery, the containing public type occurrence also becomes
recovery. Internal type errors are propagated as `HirError`; they are not
silently converted into source diagnostics.

HIR semantic errors preempt the driver's `T0001` marker. A complete module-mode
`Operation::Check` succeeds and retains warnings in its report; script and
fragment checks, or an incomplete semantic surface, advance to `T0001` until
their later milestones are implemented. Exhausting the type-node, combined
typed-expression/pattern-node, pattern-analysis-work, trait-obligation, or
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
variables. Trait tests cover empty and generic declarations, contextual `Self`,
required and associated operations, defaults under bounds, inferred and
explicit same-trait calls, async receiver requirements, invalid bodies, and
unknown members. Implementation tests cover deterministic IDs, generic header
occurrence, local-trait structural targets, cross-module orphan rejection,
source and prelude contracts, closed protocols, method generics and bound sets,
required/default/extra membership, signature drift, checked bodies,
independently scoped generic overlap, ignored positive bounds, alias-normalized
duplicates, distinct trait instantiations, deterministic source ordering,
`E1111` versus `E1113`, verifier mutation, and the public diagnostic and
MIR/bytecode/VM paths. Construction tests cover every nominal shape, contextual
generic instances, `with`, numeric conversions, ranges, membership, and cross-module
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
bounds, constraint forwarding, obligation budgets, and public-driver `E1105`
propagation. Capability regressions cover the complete intrinsic matrix,
implication forwarding, recursive nominal equality and keys, opaque bounds,
async-trait `Self: Send`, collection and reference formation, equality,
membership, map lookup, and order-insensitive map/set runtime equality.
Dedicated admission tests mutate
otherwise valid HIR to prove rejection of incomplete/recovery state,
noncanonical types, non-topological edges, misaligned flow metadata, missing
local types, invalid value categories, incomplete trait tables, shifted generic
arities, inconsistent default-body or `Self: Send` metadata, broken
implementation IDs, incomplete implementation contracts, forged method keys,
signatures not derivable from their trait, corrupted capability rows, and
capability-invalid operations.
Opaque-result regressions additionally cover forbidden syntax positions,
free/inherent/associated identity, generic family specialization, fallible
success and error channels, contextual empty containers, strict witness
equality, unreachable and error-only paths, closed `Discard` derivation,
source/prelude and intrinsic bounds, private representation, async contract retention,
representation cycles, and HIR mutations of bounds and seals.
