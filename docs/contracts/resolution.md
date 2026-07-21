# Name and member resolution contract

**Status:** implemented for the M2 source-resolution boundary

## Inputs and phase order

Resolution consumes only the closed `PackageGraph`, immutable
`SourceDatabase`, and successfully parsed CSTs from one compilation request. It
never searches directories, loads manifests, queries a registry, or reads an
interface implicitly.

The phase runs in deterministic passes:

1. collect file-local imports and all module declarations;
2. build shared module type/value tables and stable `SymbolId` values;
3. diagnose import/declaration collisions and complete module-import cycles;
4. collect fields, variants, methods, and other members;
5. resolve lexical, module, imported, prelude, and contextual references; and
6. validate the type positions exposed by public APIs.

Collecting declarations before resolving any body makes forward references,
mutual recursion, distributed modules, and file insertion order semantically
irrelevant.

## Identities and namespaces

`SymbolId`, `MemberId`, and `LocalId` are compact request-local handles. A
module symbol additionally carries its complete stable `SymbolIdentity`:

~~~text
PackageId + SourceId + ModulePath + Namespace + DeclarationPath
~~~

The resolver keeps three top-level lookup namespaces:

- types, traits, aliases, enums, and nominal declarations;
- functions, constants, and synthetic newtype constructors; and
- file-local module import aliases.

The member namespace is explicit rather than encoded in strings. A
`MemberOwner` is either a type/trait symbol or an enum variant. Member records
cover record and variant fields, the intrinsic newtype field `value`, enum
variants, inherent receiver methods, associated functions, trait receiver
methods, and trait associated functions.

Fields use `MemberName`, which accepts the specification's contextual keyword
field names while still rejecting `_`. Ordinary declarations continue to use
`Name`, which rejects keywords and the discard spelling.

## Declarations and conflicts

Every module table is assembled across all files before references are walked.
Duplicate declarations in one namespace produce `E1002`; a type and a
non-conflicting value may share spelling. A newtype also creates its constructor
in the value namespace, so a function or constant cannot take that spelling.

Imports remain local to one file. Duplicate aliases produce `E1002`; an alias
that collides with either a type or value declaration of its module produces
`E1003`. Imports after another top-level item produce `E1007`, unavailable exact
module paths produce `E1008`, and strongly connected components of resolved
module edges produce one deterministic `E1006` with the complete cycle.

Member conflicts produce `E1505`:

- a member cannot be declared twice for one owner;
- no inherent operation can share a name with an enum variant; and
- no receiver method can share a name with a field.

An associated function may share a field name because one is type-qualified and
the other is instance-qualified. Inherent method owners must be a nominal type
or enum defined by the current module; violations produce `E1504`. Duplicate
method declarations inside one `impl` are rejected before contract checking.
Resolution deliberately does not install implementation methods into an open
member namespace and does not decide trait satisfaction. Typed HIR owns the
coherence header, orphan rule, exact contract match, program-wide overlap and
`Iterator[T]` functional checks, and implementation body.

## Lexical scopes

Each function, parameter list, block, closure, `for` body, and `match` arm has an
explicit lexical scope. Initializers resolve before their binding pattern is
installed. Consequently a local is unavailable in its own initializer and
available through the remainder of its block.

Parameters, generic parameters, destructuring patterns, loop patterns, match
patterns, and closure parameters all create bindings. Redeclaration in one
scope produces `E1002`. A binding that would hide a visible outer binding,
module declaration, or import alias produces `E1003`. Sibling scopes may reuse
spelling because neither binding is visible from the other. Type and value
bindings remain separate except where the source form itself is ambiguous.

Assignment lvalues and record-initializer shorthand participate in ordinary
value lookup. A keyword field has no possible shorthand binding and therefore
requires the explicit `field: value` form.

## Paths and contextual names

Type positions search the type namespace. Plain expression names search the
value namespace. A qualified expression path that has distinct visible type and
value candidates produces `E1004`; the compiler never chooses using
capitalization or an expected result type. The synthetic value constructor and
type side of one newtype are recognized as a single intentional pair rather
than an ambiguity.

The CST deliberately preserves preliminary brackets. A name inside such a
bracket may be an index expression or a generic type argument. Resolution stores
one resolved name when only one namespace matches, or both contextual candidates
when each matches. Typed lowering must classify the bracket and either select
the matching candidate or emit `E1004`; it must not perform a fresh fallback
lookup.

The same contextual treatment applies recursively to source shapes that can be
types but are parsed with expression productions inside a preliminary bracket:
options such as `T?`, nested applications such as `Array[T]`, tuples, groups,
and structural unions. This lets an explicit specialization inside a generic
body refer to its type binder without making ordinary `values[index]`
resolution type-directed. Once HIR classifies the outer bracket, every selected
type name still comes from the reference recorded here.

The same rule covers `value.method[T](...)`. Resolution records `T` without
deciding whether the bracket is indexing; HIR may classify it as an explicit
member specialization only after resolving `method` to a callable member. It
then applies the written arguments to the method-local suffix of the callable
binder, leaving owner parameters and contextual `Self` to receiver inference.

`Self` resolves as a contextual type only inside traits, implementations, and
inherent methods. The value `self` resolves only when the current callable
actually declares a receiver. These are explicit resolved entities, not hidden
prelude declarations or ordinary locals.

## Visibility and public APIs

Module declarations are private unless marked `pub`. A qualified reference to
a private declaration from another module produces `E1501` and retains the
resolved symbol for recovery.

Record fields inherit the owner's visibility unless a public record marks a
field `priv`. Enum variants and their fields inherit enum visibility. Trait
members inherit trait visibility; inherent methods use their own `pub` marker.
The synthetic newtype field `value` inherits its type's visibility. Redundant
`priv` on a private record is rejected as a malformed declaration.

After reference resolution, the API validator walks only externally observable
type positions:

- public function, method, and trait signatures and bounds;
- public constants' declared types;
- public alias and newtype targets;
- public record fields, excluding fields marked `priv`; and
- public enum variant payloads.

A private type from the same module in one of those positions produces `E1503`.
Function bodies, implementation bodies, and private record fields are excluded.
An inaccessible type from another module keeps the earlier and more specific
`E1501` rather than receiving a duplicate API diagnostic.

## Recovery and resource limits

Unknown names produce `E1001`, but resolution continues with other independent
declarations and scopes. References are retained by `(FileId, TextRange)` and
point to symbols, locals, modules, prelude names, external interface
placeholders, contextual `Self`/`self`, or the two candidates of a preliminary
bracket.

The request-wide primary diagnostic budget applies to every resolution pass.
Reaching it returns a typed limit to the driver, which emits `T0002`; it never
panics, truncates into apparent success, or begins type checking with a partial
resolved program.

## Determinism and validation

Files, declarations, import edges, symbols, and members are explicitly sorted
by logical identities and byte positions. Ordered maps back every observable
table. Symbol, member, and local IDs therefore do not depend on the order in
which source files entered the database.

Tests cover closed import lookup, distributed modules, stable identities,
complete cycles, all `E1001`–`E1008` paths, lexical lifetimes, forbidden
shadowing, sibling scopes, patterns, closures, loops, assignment lvalues,
record shorthand, contextual receivers, type/value ambiguity, keyword fields,
member inventory and conflicts, method owners, cross-module visibility, public
API leakage, diagnostic limits, and file-order permutation.
