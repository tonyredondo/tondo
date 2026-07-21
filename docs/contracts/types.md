# Canonical type representation

**Status:** DEC-004 accepted; canonical representation and local algorithms implemented

## Boundary and invariants

`TypeInterner` is the single request-owned store for semantic types. `TypeId` is
a compact request-local index and is never serialized, hashed into public
identity, or compared across requests. Observable output uses the recursive
canonical serialization defined by the language specification.

The interner deduplicates structurally identical nodes. Its current inventory
can represent:

- canonical scalar types, including `Unit` and `Never`;
- nominal applications identified by a complete type-namespace
  `SymbolIdentity`;
- tuples and function types with parameter modes, variadics, async, and unsafe;
- option, result, and normalized structural union types;
- intrinsic applications such as `Array[T]`, `Map[K, V]`, and `Ref[T]`;
- generic parameters by complete binder position;
- request-local inference variables;
- opaque declaration results, generated closure identities, and cursor types;
- one internal recovery type that cannot escape into public diagnostics.

Aliases are deliberately absent from `TypeKind`. Type lowering expands a
transparent alias before interning its target, so aliases cannot acquire nominal
identity or survive into compatibility checks. Records, enums, newtypes, and
opaque results retain declaration identity.

`TypeSubstitution` maps complete binder positions to canonical arguments and
rebuilds the full type graph through the same interner. Rebuilding unions repeats
normalization, so substituting two parameters with one type cannot leave a
duplicate union member. Missing arguments are typed errors rather than implicit
inference variables.

The interner exposes two distinct first-order relations. Declaration-local
overlap keeps one binder scope, as required when checking members of the same
generic union. Coherence compares lists of roots with independent left and right
binder scopes, so `$0` in two `impl` declarations is alpha-renamed rather than
silently identified. Repeated occurrences inside either header remain linked,
occurs checks reject infinite substitutions, normalized unions match as
unordered member sets, and all roots share one substitution. A second operation
can compare an output after computing the most-general input unifier without
adding another equation; `Iterator[T]` uses that distinction for its functional
target-to-element rule. Trait bounds are intentionally outside both relations.

Source syntax is connected to this representation by `hir::lower_types`; the
lowering boundary and its recovery rules are recorded in
`docs/contracts/hir.md`.

## Canonical primitives

`Int64` maps to the same `TypeId` as `Int`, and `Float64` maps to `Float`.
Canonical output always uses the short spelling. Other fixed-width scalar types
remain distinct.

`Option[T]` and `T?` lower to one `Option` node. `Result[T, E]`, `T ! E`, and
`!E` lower to one `Result` node, with the final spelling derived from the node.
Required parentheses follow the type grammar, including nested options and
union-valued result errors.

## Union normalization

Constructing a union performs all normative normalization immediately:

1. recursively flatten union members;
2. remove `Never`;
3. deduplicate identical canonical members; and
4. sort by the UTF-8 bytes of each member's canonical serialization.

An empty result becomes `Never`, and a one-member result becomes that member
directly. Consequently no nested, duplicate, singleton, or `Never`-polluted
union node can enter later phases. Type IDs may depend on interning history;
union order and every public string do not.

## Nominal and generated identity

A nominal type accepts only a `type`-namespace identity and is invariant in its
argument vector. Its canonical constructor is the atom supplied by
`SymbolIdentity`, followed by canonical generic arguments when present.

Opaque results use the value declaration atom followed by `#result`. Generated
closure types use their kind, source ID, module, logical file, and starting byte;
captured generic arguments follow as a canonical application. Cursor types use
the exact `cursor[own,T]` or `cursor[ref,T]` form.

## Inference and errors

Inference variables are explicit interner nodes so local bidirectional inference
can refer to them without inventing a public type. Asking for canonical output
before solving one is an error. The recovery node is likewise non-serializable.
Completed signatures, HIR, diagnostics, and interfaces must contain neither.

`InferenceContext` implements local invariant equality constraints for the
future bidirectional expression checker. It has an occurs check, treats generic
parameters as rigid, rolls back every solution introduced by a failed
constraint, resolves compound types through an explicit worklist, and rejects
unsolved variables when a local inference boundary closes. It deliberately does
not implement global inference, polymorphic generalization, subtyping, or
Hindley-Milner inference. The solver is implemented and unit-tested; its public
source-language integration completes with CHECK-001 and CHECK-010.

The interner enforces the request's type-node budget before allocation and
returns a typed resource error. Compound constructors reject unknown child IDs,
invalid tuple arity, intrinsic arity errors, and nominal identities from the
wrong namespace.

Intrinsic arity is closed: `Array`, `Set`, `Range`, `Ref`, and `Pointer` take
one argument; `Map` and `Join` take two; `Command`, `Pipeline`, and
`NumericConversionError` take none. `Iterator[T]` is a trait bound, not a value
type node.

## Assignment and numeric conversion

The interner exposes the closed top-level assignment relation required by 8.9.
It distinguishes exact identity, injection into a structural union, widening a
union to a normalized superset, contextual lifting from `T` to direct `T?`, and
the diverging `Never` case. `none` is not assigned an invented standalone type;
the expression checker asks whether its direct expected type is an option.

No relation recurses through tuple, option, result, function, nominal, or
intrinsic arguments. Those constructors remain invariant. In particular,
`Array[A]` never widens to `Array[A | B]` merely because `A` can be injected
into the union.

`numeric_conversion` implements the complete intrinsic scalar matrix. It
classifies identity spellings, total conversions, and conversions returning
`NumericConversionError`; nonnumeric pairs are absent. Integer range inclusion,
integer-to-float rounding, `Float32` to `Float64`, narrowing float conversions,
and float-to-integer checks follow section 18.6. These algorithms are
implemented and unit-tested; constructor-expression integration belongs to
CHECK-010.

## Resource-safe algorithms

Canonical rendering, transparent substitution, first-order generic
unification, inference resolution, occurs checks, and recursive HIR analyses use
explicit worklists. They do not recurse in proportion to a user-controlled type
graph. The type-node budget is checked before every new interned node and before
allocating a fresh inference identity.

## Physical representation is not semantics

The interner is an analysis structure, not a runtime layout or ABI. It does not
choose enum tags, field offsets, object headers, calling convention, COW, ARC,
or GC representation. MIR and VM contracts consume these semantic types without
making their request-local indices externally stable.

## Validation

Tests prove scalar synonym identity, resource limits, nominal invariance,
namespace checks, intrinsic arity, function serialization, option/result
equivalence, recursive union normalization, substitution and renormalization,
stable generated identities, exact top-level assignment, the closed numeric
conversion table, local inference rollback and occurs checks, first-order
generic overlap, independently scoped multi-root coherence, unordered
alpha-equivalent unions, functional-output comparison, deep graph handling, and
the prohibition on serializing unresolved inference or recovery types.
