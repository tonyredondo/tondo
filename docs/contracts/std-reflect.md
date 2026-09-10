# `std.reflect` contract

**Implementation status:** the ordinary frontend and hosted VM execute
`std.reflect.typeInfo[T]()` and the nine `TypeInfo` queries. The compiler retains
concrete public descriptor types and field documentation in an immutable
artifact table. The VM checks query signatures, descriptor shapes and type
capabilities, and admits result storage before construction. Secondary
descriptors use query methods with the signatures below. Complete public
conformance and owner promotion remain unfinished. `REFLECT-IMPL-001` and
`STD-A-REFLECT-EVIDENCE-001` stay open; native
ABI and native AOT execution are not established by this hosted implementation.

`std.reflect` is descriptive, immutable and opt-in. Its only entry point is the
statically instantiated `typeInfo[T]()` function. A live instantiation retains
metadata for `T` and the public descriptor types reachable from it; dead calls
and unreachable types retain nothing under closed-entry lowering. The general
bytecode lowering API retains all concrete declared functions as entry roots;
its callers must not claim that unused functions have been eliminated. The
closed-entry API follows only the selected function, referenced callables and
constant function roots.

`TypeInfo` exposes artifact-local identity, qualified name, a closed kind,
concrete generic arguments, proven capabilities and public structural
descriptors. Secondary descriptors contain names, declaration ordinals, types,
parameter modes and function effects. They contain no value access, private
members, constructors, callable handles, layout, addresses, ABI or GC state.
For a variadic function, the final parameter descriptor contains the variadic
element type and value mode; the function's variadic flag distinguishes it from
an ordinary final parameter. Field documentation retains multiline text across
field attributes. Private field types do not extend the retained closure.

`TypeId` supports equality and hashing only inside the exact artifact that
created it. It has no public bits, parser, stable encoding or cross-artifact
meaning. There is no global registry, enumeration, lookup by name or ID,
dynamic loading, `get`, `set`, `invoke` or value reflection.

The API has no runtime error type. A non-describable static request is a compile
error; kind-specific optional views return `none`, and collection views return
an empty immutable value when inapplicable. JSON, MessagePack and Protobuf use
generated static implementations rather than this module.

The public callable signatures are:

```text
pub fn typeInfo[T](): TypeInfo
pub fn TypeInfo.id(self): TypeId
pub fn TypeInfo.qualifiedName(self): String
pub fn TypeInfo.kind(self): TypeKind
pub fn TypeInfo.genericArguments(self): Array[TypeInfo]
pub fn TypeInfo.capabilities(self): Set[TypeCapability]
pub fn TypeInfo.fields(self): Array[FieldInfo]
pub fn TypeInfo.variants(self): Array[VariantInfo]
pub fn TypeInfo.tupleElements(self): Array[TypeInfo]
pub fn TypeInfo.function(self): FunctionInfo?
pub fn FieldInfo.name(self): String
pub fn FieldInfo.typeInfo(self): TypeInfo
pub fn FieldInfo.ordinal(self): Int
pub fn FieldInfo.docs(self): String?
pub fn VariantInfo.name(self): String
pub fn VariantInfo.ordinal(self): Int
pub fn VariantInfo.payloadKind(self): VariantPayloadKind
pub fn VariantInfo.tupleElements(self): Array[TypeInfo]
pub fn VariantInfo.fields(self): Array[FieldInfo]
pub fn ParameterInfo.position(self): Int
pub fn ParameterInfo.typeInfo(self): TypeInfo
pub fn ParameterInfo.mode(self): ParameterMode
pub fn FunctionInfo.parameters(self): Array[ParameterInfo]
pub fn FunctionInfo.outcome(self): TypeInfo
pub fn FunctionInfo.variadic(self): Bool
pub fn FunctionInfo.suspends(self): Bool
pub fn FunctionInfo.isUnsafe(self): Bool
```

| Descriptor | Query methods |
| --- | --- |
| `FieldInfo` | `name(): String`, `typeInfo(): TypeInfo`, `ordinal(): Int`, `docs(): String?` |
| `VariantInfo` | `name(): String`, `ordinal(): Int`, `payloadKind(): VariantPayloadKind`, `tupleElements(): Array[TypeInfo]`, `fields(): Array[FieldInfo]` |
| `ParameterInfo` | `position(): Int`, `typeInfo(): TypeInfo`, `mode(): ParameterMode` |
| `FunctionInfo` | `parameters(): Array[ParameterInfo]`, `outcome(): TypeInfo`, `variadic(): Bool`, `suspends(): Bool`, `isUnsafe(): Bool` |

`VariantPayloadKind` has `Unit`, `Tuple`, `Record`; `ParameterMode` has `Value`,
`Ref`, `Mut`, `Var`. Ordinals and positions start at zero. Omitted private
fields leave ordinal gaps in record types. Enum record payload fields follow
the public payload contract. Documentation is optional and preserves multiline
text. Secondary descriptors have no public constructors or fields.

## Evidence and budgets

Focused compiler tests named `reflection_public_*` execute ordinary Tondo
queries, including concrete generics, TypeId map/set keys, public field
filtering, declaration documentation, enum payloads, function modes and
variadic tails. `reflection_entry_lowering_drops_queries_in_unreachable_functions`
checks the closed-entry lowering boundary. VM tests cover foreign descriptor
rejection and result admission with complete memory release. The public
conformance checker requires a current draft execution report for the complete
callable and requirement table, as defined in
[`stdlib-meta-reflect-conformance.md`](stdlib-meta-reflect-conformance.md).
Owner promotion remains pending until that report and the joint gate pass.
Performance measurements retain their separate boundary.

The owner contract is [`testing/stdlib-reflect.json`](../../testing/stdlib-reflect.json)
and the current cell record is maintained in
[`testing/stdlib-owner-evidence.json`](../../testing/stdlib-owner-evidence.json)
under `STD-A-REFLECT-EVIDENCE-001`. `HOST` is explicitly
`not-applicable`: reflection metadata is built by the compiler and has no
runtime host adapter, ambient provider or value channel. The record separates
the catalog model, privacy/root tests, bounded boundary corpus and the pending
link-work/descriptor-size performance capture.

Run `scripts/stdlib-reflect-check.sh`, `scripts/stdlib-reflect-test.sh` and
`scripts/stdlib-owner-evidence-check.sh` before the normative matrix check.
