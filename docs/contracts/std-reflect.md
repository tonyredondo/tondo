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
