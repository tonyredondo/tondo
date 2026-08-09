# `std.reflect` contract

`std.reflect` is descriptive, immutable and opt-in. Its only entry point is the
statically instantiated `typeInfo[T]()` function. A live instantiation retains
metadata for `T` and the public descriptor types reachable from it; dead calls
and unreachable types retain nothing.

`TypeInfo` exposes artifact-local identity, qualified name, a closed kind,
concrete generic arguments, proven capabilities and public structural
descriptors. Secondary descriptors contain names, declaration ordinals, types,
parameter modes and function effects. They contain no value access, private
members, constructors, callable handles, layout, addresses, ABI or GC state.

`TypeId` supports equality and hashing only inside the exact artifact that
created it. It has no public bits, parser, stable encoding or cross-artifact
meaning. There is no global registry, enumeration, lookup by name or ID,
dynamic loading, `get`, `set`, `invoke` or value reflection.

The API has no runtime error type. A non-describable static request is a compile
error; kind-specific optional views return `none`, and collection views return
an empty immutable value when inapplicable. JSON, MessagePack and Protobuf use
generated static implementations rather than this module.

## Evidence and budgets

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
