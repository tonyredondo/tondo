# Unsafe and raw-pointer contract

**Status:** implemented compiler boundary for M9

This document records how the bootstrap compiler represents the `unsafe`
effect. The normative language rules and the closed undefined-behavior list
remain in sections 8.13 and 16.12 of `TONDO_LANGUAGE_SPEC.md`.

## Safety invariant

Safe Tondo cannot initiate an unsafe callable, execute a raw-pointer operation,
or hide a raw pointer in a safe closure. An `unsafe` region grants only those
three permissions. It does not disable ordinary type checking, ownership,
borrow checking, exhaustiveness, overflow checks, resource limits, or panic
semantics.

The compiler proves the visible side of the contract. The programmer or a
privileged adapter remains responsible for preconditions that depend on
provenance, lifetime, layout, alignment, aliasing, foreign state, or concurrent
access.

## Source regions

The four callable effects remain distinct:

~~~tondo
fn safeSync()
unsafe fn unsafeSync()
async fn safeAsync()
async unsafe fn unsafeAsync()
~~~

Closures preserve the same product:

~~~tondo
let safeSync = () { () }
let unsafeSync = unsafe () { () }
let safeAsync = async () { () }
let unsafeAsync = async unsafe () { () }
~~~

The body of an unsafe function or closure is an unsafe region. Safe code creates
a local region with:

~~~tondo
let value = unsafe {
    rawOperation()
}
~~~

The region is lexical and expression-valued. Leaving it removes permission.
Nested ordinary blocks inherit the active permission; a separately declared
safe closure does not.

An async unsafe call keeps both effects explicit:

~~~tondo
let value = unsafe {
    await unsafeAsyncOperation()
}
~~~

`unsafe` proves caller acknowledgement and `await` initiates suspension.
Neither keyword substitutes for the other.

## Diagnostics

`E1701` is emitted when:

- an unsafe named function is called outside an unsafe region;
- an unsafe closure is called outside an unsafe region;
- an async unsafe callable is awaited outside an unsafe region; or
- a raw-pointer operation is used outside an unsafe region.

`E1702` is emitted when a safe or safe-async closure captures a value whose type
can contain `Pointer`. The search is structural through tuples, nominal types,
unions, options, results, arrays, maps, sets, ranges, `Ref`, callables and
closure environments. `defer` uses a safe closure and follows the same rule.

The capture is permitted only by an `unsafe` or `async unsafe` closure. Such a
closure preserves the unsafe bit in its concrete type and any exact uniform
`unsafe fn(...)` coercion.

## Closed raw-pointer surface

The compiler recognizes exactly these operations:

| Operation | Static signature | Meaning |
|---|---|---|
| `pointer.read()` | `Pointer[T] -> T` | read one initialized `T` |
| `pointer.write(value)` | `(Pointer[T], T) -> Unit` | write one `T` |
| `pointer.offset(count)` | `(Pointer[T], Int) -> Pointer[T]` | derive an address offset by `count` elements |
| `pointer.cast[U]()` | `Pointer[T] -> Pointer[U]` | reinterpret the address with target element type `U` |
| `pointer.address()` | `Pointer[T] -> UInt64` | expose the numeric address |
| `address.toPointer[T]()` | `UInt64 -> Pointer[T]` | construct a non-null raw pointer from an address |

`cast` and `toPointer` require exactly one explicit type argument. The other
operations reject type arguments. `write` requires exactly one value of `T`;
`offset` requires exactly one `Int`; the remaining operations take no ordinary
arguments.

The ordinary qualified method spelling is also available and infers `T` from
its first argument:

~~~tondo
Pointer.read(pointer)
Pointer.write(pointer, value)
Pointer.offset(pointer, count)
Pointer.cast[U](pointer)
Pointer.address(pointer)
UInt64.toPointer[T](address)
~~~

These checks happen statically and do not become recoverable runtime failures.
There is no dereference operator, implicit conversion, pointer arithmetic,
pointer comparison, pointer ordering, or pointer hashing.

`Pointer[T]` is `Copy` and `Discard`, but never `Equatable`, `Key`, `Send`, or
`Share`. Copying an address does not prove that any use is valid.

## Preconditions

Before executing `read`, the caller must establish:

- non-nullness;
- provenance for one complete `T`;
- target-defined alignment and size;
- initialized bytes forming a valid representation of `T`;
- a live allocation for the complete operation;
- no incompatible concurrent write.

Before executing `write`, the caller must additionally establish:

- writable storage;
- permission to replace its current contents;
- no incompatible alias or data race;
- any target-specific ownership action required for the old and new value.

Before executing `offset`, the caller must establish:

- that the target has a declared layout for `T`;
- that the mathematical element offset and address calculation are
  representable;
- that the derived pointer remains within the same provenance-granting object
  or its permitted one-past boundary;
- that the result is not used as a `T` pointer unless correctly aligned.

Before executing `cast`, the caller must establish every alignment,
representation, size, provenance and aliasing condition required by later
operations on `U`.

Before executing `address`, the caller must accept that numeric addresses are
process- and lifetime-local and do not provide stable identity.

Before executing `toPointer`, the caller must establish that the non-zero
numeric address was obtained under a contract that permits reconstruction and
that all preconditions of every subsequent use still hold.

The language type checker can prove arity, source/target types, callable
effects, ownership and static capabilities. It cannot prove the conditions in
this section merely from an integer or `Pointer[T]`.

## Closed undefined-behavior list

Undefined behavior can begin only when an executed raw or privileged operation
violates one of its declared preconditions. The closed classes are:

1. constructing, deriving, reading, or writing through a pointer without the
   provenance, lifetime, alignment, size, initialization, or value
   representation required by that operation;
2. overflowing an offset calculation, deriving a pointer outside its
   provenance-granting object or region, or accessing outside it, even when the
   numeric address happens to refer to mapped memory;
3. writing immutable storage or violating an aliasing obligation;
4. a data race introduced by unsafe or native code;
5. a wrong native signature or calling convention, or use of an expired
   callback;
6. violation of runtime root, pin, retain/release, or foreign-thread attachment
   obligations;
7. allowing a Tondo panic or foreign exception to cross an ABI boundary that
   does not explicitly admit it.

An unexecuted unsafe expression creates no undefined behavior. A statically
unreachable raw call still passes through typed lowering and verification but
has no dynamic precondition to satisfy.

Once undefined behavior begins, no Tondo result, panic code or continuation is
specified. The optimizer may rely only on documented preconditions of
operations that actually execute.

## Safe wrappers

A safe wrapper may contain a private unsafe region only when it:

1. validates every caller-controlled precondition;
2. establishes the remaining invariant itself;
3. prevents invalid pointer state from escaping through safe types;
4. keeps roots, pins, locks and native lifetimes valid across the whole
   operation, including suspension;
5. converts foreign errors to explicit Tondo values or aborts at the adapter;
6. never unwinds a Tondo panic through an undeclared foreign ABI.

The privileged-unit descriptor records the wrapper as `safe-wrapper`, plus
hashes of its Tondo signature, safety contract and implementation. A raw
binding is recorded as `unsafe-function`. The descriptor is a pinned build
input, not source-level authority.

## HIR proof

Body checking carries one explicit `in_unsafe_region` fact:

- it starts true only for an unsafe callable body;
- `unsafe { ... }` sets it for the nested expression and restores the previous
  fact afterward;
- every unsafe call records `unsafe_call = true`;
- each raw operation becomes one closed typed bootstrap-host callable;
- closure capture validation recursively rejects Pointer-containing captures
  when the closure effect is safe.

The HIR verifier independently rederives callable effects, result types,
arguments and raw-operation signatures. Recovery HIR never crosses into MIR.

## MIR proof

MIR preserves `unsafe_call` on every direct or indirect call. Its verifier
rederives the selected callable signature and rejects any forged effect bit.

Raw operations lower to distinct `MirBootstrapHostFunction` values. The MIR
verifier checks their complete receiver, argument and result types. They do not
reuse arithmetic, casts, loads, stores or ordinary safe host calls.

The source region itself needs no runtime token: admission is complete before
MIR exists, while the per-operation effect remains explicit and verifiable.

## Bytecode proof

Bytecode keeps:

- `unsafe_call` on typed call instructions;
- one distinct host-function enum value for each raw operation;
- concrete Pointer element types on operands and outcomes.

The bytecode verifier independently rejects:

- an unsafe bit that disagrees with the callable;
- a safe bit on an unsafe callable;
- wrong arity;
- wrong receiver, value, offset, integer-address or result type;
- erased or unresolved generic target types.

The VM receives only bytecode that passed that proof. The in-memory bytecode is
not a stable ABI or persistent FFI format.

## Bootstrap target boundary

The VM target lowers raw operations to typed privileged-host operations. Their
dynamic implementation belongs to a target adapter whose descriptor and hashes
are declared by `TONDO_TOOLCHAIN_SPEC.md`.

The bootstrap distribution does not expose a safe allocator, native address
source, stable layout, or general FFI ABI. Consequently ordinary safe programs
cannot obtain a usable `Pointer`, and the repository's end-to-end raw-operation
test keeps its synthetic integer address on an unexecuted branch. This is a
deliberate honesty boundary: the compiler and all verifiers implement the
language surface, while no target-specific native behavior is invented before
its ABI contract exists.

Console and process host bridges cannot manufacture Pointer values. A future
privileged adapter must pin compiler, target, profile, capabilities, signature,
safety contract and implementation before any raw operation can execute
conformingly.

## Validation

Tests cover:

- all four closure effect combinations;
- direct and indirect unsafe calls;
- lexical unsafe regions;
- async unsafe initiation;
- `E1701` for missing regions;
- `E1702` for direct and recursively contained Pointer captures;
- the exact six raw operations and their arities/type arguments;
- HIR, MIR and bytecode verifier agreement;
- successful VM execution of ordinary unsafe callables;
- lowering of every raw operation through verified bytecode without executing
  an invalid synthetic address.
