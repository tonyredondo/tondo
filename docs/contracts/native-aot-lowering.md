# Native AOT lowering contract

`NATIVE-AOT-LOWER-001` remains pending after the 2026-09-07 audit. The historical
evaluation described below tests a bounded normalized-MIR corpus and generated
C runtime. Its `closed` register status describes that prototype boundary;
it does not discharge the tracker's source-driven production lowering task.
Both candidates consume one immutable `tondo-mir-backend/1` program, but
synthetic storage cases do not prove that the frontend can produce those
operations from Tondo source.

## Source-driven value aggregates, enums, unions, sums, direct calls and equality

The compiler lowers local tuples and records whose leaves are `Int`, `Bool`, `Char`, `Unit`,
`Byte`, `Int8`/`Int16`/`Int32`, `UInt8`/`UInt16`/`UInt32`/`UInt64` and `Float32`/`Float64`
into independent scalar locals. Nested tuples/records and instantiated generic
records use the same route. Nominal enums, structural unions, `T?` and `T ! E` recursively admit these value
layouts, including the closed `NumericConversionError` type. Construction, field reads and writes, copies,
whole-value and nested-field replacement, `with` updates, branches and
loop-carried values preserve value semantics.
Assignments snapshot the right-hand fields before writing their destinations.
This route does not allocate aggregate handles. The native normalization operates
on a private copy of MIR blocks;
it leaves verified source MIR and hosted bytecode unchanged. Record layouts
come from verified HIR record declarations and instantiated MIR constructors,
with declaration-order member identities. Each generated local belongs to one
scalar leaf; a copy
never shares its mutable fields with its source.
An empty record uses one canonical `Unit` carrier in this private layout.
It retains its nominal source type and has no added source member. This keeps
the existing nonempty aggregate argument/result protocol; it is not a claim of
zero-byte storage or a public record ABI. Unit fields and empty-record carriers
count toward storage, snapshot and comparison limits like other leaves.
Expansion is bounded to 65,536 additional locals per function, including
right-hand snapshots, comparison results, sum tags and numeric guards; wider repeated copies and
comparisons are rejected before exceeding
that allocation budget. Layout depth is bounded to 64 aggregate levels.

Intrinsic `==` and `!=` compare these tuples and records structurally, including
nested values, projected subaggregates and concrete generic instances. Equality
combines equal leaves with Boolean conjunction; inequality combines unequal
leaves with disjunction. All admitted leaves are scalar values, so these reads and
comparisons have no user code, suspension or side effects. Their order follows
the existing tuple/declaration field order.

The source operands remain immutable intrinsic observations. Native
normalization reads their scalar storage without modifying either value and
writes the result only after all field reads. A result destination may be a
Boolean field of either operand, including a nested field. Comparison scratch
locals count against the same per-function expansion budget, one per leaf;
exhaustion rejects the function and propagates to its callers. This does not
admit general `ref`/`mut`/`var` parameter or loan storage.

Ordinary direct calls accept and return these aggregates by value. Arguments
are evaluated in source order, then flattened into scalar carriers
in parameter declaration order. Nested values keep their field order and
independent storage. The source parameter types remain in the normalized
function metadata; flattened carrier count is not source arity.

An aggregate-returning function takes a final hidden result pointer in the
private adapter ABI. Each result field occupies one eight-byte carrier. The
caller owns a distinct, statically sized buffer at each call site. LLVM places
these buffers in the function entry, and Cranelift uses explicit stack slots,
so a call in a loop reuses its buffer. Recursive frames own separate buffers.
The callee writes all fields on normal return; the caller then copies them into
independent locals. A discarded result still receives private storage. Checked
arithmetic traps do not publish partial results. This is an internal compiler
protocol, without a public layout, FFI or runtime ABI promise.
An empty record return still uses one eight-byte carrier. Calls producing
`Unit` or an empty record remain evaluated, including discarded results; a
checked error before completion must still trap.

Core sums use a private integer tag followed by disjoint payload carriers:
`Option` stores its possible value, and `Result` stores both success and error
layouts. Construction initializes every carrier, including inactive payloads,
to canonical zero, false or Unit values before inserting the active payload.
Replacing a variant clears the previous payload. Nested inactive tags are
padding and are never projected as active values. This invariant makes complete
field copies and structural comparisons independent of previous local contents.
There is no payload overlap, handle table, heap allocation or public ABI claim.

Nominal enums retain every variant and its payload fields from verified HIR,
including variants that a function never constructs. Their private integer tag
is the declaration-order variant ordinal. Unit variants contribute no payload
fields; every value still reserves the full layout, including inactive carriers.
Positional and named payloads occupy disjoint carriers in declaration order.
Variant and field identities remain tied to their source declarations. Record
initializers evaluate in textual order before their operands are arranged in
declaration order, for both ordinary records and enum variants.

Concrete generic enums substitute every declared payload type, including nested
records, enums and core sums. Unused type arguments retain nominal identity.
Declaration traversal memoizes completed types and bounds recursive expansion;
finite scalar storage must still satisfy the shared layout and local limits.
An unsupported payload in any variant rejects the whole enum layout, even when
the program constructs only a unit variant. Recursive value layouts remain
outside this finite storage route.

Structural unions retain a tag and disjoint initialized storage for every
normalized member. The private tag is the member's concrete interned type
identity in this native program, not its position in a particular union. It is
not a cross-program identifier or public ABI. Injection selects the exact member
and clears every inactive carrier. Widening preserves the tag, maps existing
members by type identity into destination offsets and initializes added members.
All right-hand carriers are snapshotted before any destination write. There is
no implicit conversion between members or recursive widening of containers.

Generic specialization substitutes union projection and branch identities as
well as operand types. Complete record/enum declarations are traversed even
when a union member is never constructed. Unsupported members reject the whole
layout, including inactive managed, recursive or over-depth members. The
existing layout depth, type expansion and additional-local budgets also cover
union storage, copies, widening and tag comparisons. Source normalization of
member order, duplicates and `Never` remains the frontend's responsibility;
bare type parameters and overlapping generic members remain forbidden.

`some`/`none`, `ok`/`err`, implicit Option lifting, success returns and `fail`
use this layout. Tag tests become integer comparisons and ordinary Boolean
branches; payload projections resolve to their own scalar fields. Source `?`
retains its existing control flow, including returning absence or an error
before subsequent expressions execute. Direct and generic calls use the same
flattened arguments and caller-owned result buffers as records. Numeric error
variants retain the ordinals defined by `NumericConversionErrorVariant`.

Checked conversions between the admitted integer types snapshot their input,
test it against the destination bounds and write a complete success or error
value. Failures produce `OutOfRange`; they do not truncate or trap. A later
checked arithmetic operation can still trap normally. Each conversion charges
four scratch locals and adds three blocks; each tested tag charges one local
and at most one block. Storage, copies, comparisons and these control-flow
expansions share the existing per-function budget. Bounds are intersected with
the source range and compared in the source domain. `UInt64` sources therefore
use unsigned comparisons, while a signed source never needs to represent
`UInt64.max`. Total conversions preserve the value; checked failures still
return `OutOfRange`.

`Float32` and `Float` (`Float64`) use one private eight-byte carrier each:
binary32 bits occupy the low 32 bits with a zero upper half; binary64 uses all
64 bits. Constants retain their format and round directly to that format.
Typed private operators decode these bits into native floating-point values for
addition, subtraction, multiplication, division and IEEE comparisons. Negation
flips the sign bit, preserving signed zero. Both candidates preserve subnormal
values, infinities and NaN comparison behavior. Arithmetic rounds to the source
format, without fast-math flags or automatic multiply-add contraction. The
private carrier convention is not a public floating-point ABI.

Integer/Byte-to-float conversions round directly to the destination format;
UInt64 sources use unsigned conversion instructions. Float32-to-Float64 is
total. Checked Float64-to-Float32 returns `OutOfRange` only when a finite source
becomes infinite; NaNs, infinities and gradual underflow remain admitted.
Float-to-integer/Byte conversions test `NotFinite`, `NotIntegral`, then
`OutOfRange`, in that order. Range checks use exact lower bounds and exclusive
power-of-two upper bounds, avoiding rounded Int64/UInt64 maxima. Integrality
uses an exact integer round trip below the format's fractional range; larger
finite values are already integral.

The private conversion expansion evaluates saturating machine conversion
candidates before branching. These internal operations cannot trap or produce
LLVM poison; source code still receives a checked Result, never a saturating
conversion. Source storage is snapshotted first, and every success/error branch
initializes all Result carriers before publishing its payload. Narrowing adds
three blocks; float-to-integer conversion adds seven. Every predicate and
candidate local counts against the shared expansion budget. Float value
aggregates use the existing copy/call protocol and canonical positive-zero
inactive payloads. General float `ref`/`mut`/`var` parameters remain unadmitted.

`Char` uses one private eight-byte carrier containing a nonnegative Unicode
scalar value. Its domain is `0..=0x10FFFF` excluding `0xD800..=0xDFFF`. Literal
spelling remains typed as `Char` in the private program; the adapter independently
decodes one scalar or a specified escape before native code generation.
Empty/multiple-scalar literals, unescaped ASCII controls, unknown escapes,
invalid hex syntax, surrogates and out-of-range values are rejected. This
validation also visits constants in unused functions and inactive paths.

Scalar equality, inequality and order use the complete scalar value, with no
UTF-16 truncation, locale ordering, normalization or case folding. Literal and
guarded `match` retain the verified source branches. Transparent aliases preserve
the same representation; Char remains distinct from integers, Byte and String.
The source arithmetic and numeric-conversion restrictions remain unchanged.
Records, tuples, enums, unions and Option/Result use the existing copy and call
protocol, including ordinary/generic functions and initialized inactive storage.
Inactive Char fields contain the valid NUL scalar, not an invalid sentinel.
Storage and comparison temporaries use the existing expansion budget. This
value route does not establish general Char loans, managed collections,
string APIs or a public character ABI. Discrete `Range[Char]` values use the
separate private range layout below.

### Discrete range values, membership and owned iteration

`Range[T]` for the intrinsic signed and unsigned integer types and `Char` uses
three private scalar carriers: start, end and an inclusive-end Boolean. `Byte`,
floating-point and managed element types are not admitted. Construction
evaluates both bounds in source order, then snapshots them into independent
storage. Copies, nested value fields, ordinary calls and concrete generic
instances use the existing aggregate protocol. An unused return still
evaluates the bounds and propagates checked arithmetic traps.

The `in` operator compares the item with the stored start and end. It includes
the end only when the stored flag is true. Reversed and equal exclusive bounds
are empty. `UInt64` comparisons remain unsigned across the high bit; `Char`
compares Unicode scalar values across the surrogate gap. The private lowering
uses six Boolean temporaries per membership test and charges them to the same
per-function expansion budget as aggregate copies. The source MIR and hosted
bytecode remain unchanged. The comparison inputs are pure scalar values, so
evaluating both bound predicates does not duplicate source effects.

An owned `for` over one of these ranges creates a private cursor with five
scalar carriers: the three source-range fields, the next element and an active
flag. The source expression is evaluated once; cursor initialization snapshots
its value before the loop. Each `IteratorNext` becomes bounded Boolean tests
and normal control-flow blocks on the private MIR copy. The current element is
emitted before advancing. An exclusive end is not emitted; an inclusive end is
emitted once without calculating a successor past the element type's maximum.
Equal exclusive and reversed ranges yield no elements. `UInt64` order remains
unsigned across the high bit. The private `Char` successor views its native
carrier as an integer only for a guarded increment, jumps from U+D7FF to
U+E000, and never creates a surrogate or a value above U+10FFFF. This does not
introduce source-level Char arithmetic.

Five Boolean temporaries per static `IteratorNext` site, plus one for the Char surrogate
guard, share the existing per-function local limit with cursor storage and
assignment snapshots. The lowering leaves source MIR, its types and hosted
bytecode unchanged. Concrete generic instances containing an owned range loop
use the same route; generic templates remain unadmitted. `break`, `continue`,
nested loops and ordinary calls keep their verified source control flow.
Borrowed/mutable cursors, other intrinsic collections, user `Iterator`
implementations, custom steps, general loans and production runtime storage
remain pending. Neither the three range carriers nor the five cursor carriers
are a public layout or FFI ABI.

`UInt64` occupies one eight-byte carrier with all bits preserved. The private
`UnsignedInteger` constant retains its source spelling and is validated over
`0..=2^64-1`; signed `Integer` constants retain their signed range validation.
Native consumers may spell the raw bits as a signed host integer, which is not
a source-level numeric conversion. The compiler emits `unsigned-` operators for
addition, subtraction, multiplication, division, remainder, ordering and right
shift. Cranelift uses unsigned instructions and checked overflow; LLVM uses
unsigned overflow intrinsics, `udiv`/`urem`, unsigned comparisons and `lshr`.
Equality, complement, bitwise operations and left shift share the same raw-bit
operations. A count outside `0..63`, including counts above 32 bits, traps before
shifting. Division by zero, overflow and underflow use the existing trap protocol,
including discarded results. No source arithmetic or conversion rule changes.

`UInt64` values use the existing tuple, record, enum, union, Option/Result and
ordinary/generic call layouts, including inactive payload initialization and
independent copies. Checked conversions must be normalized before reaching the
adapter; a residual checked conversion involving `UInt64` is rejected instead
of using the historical evaluation-runtime conversion helper. Neither that
runtime nor the production runtime is required by the source corpus.

Narrow integers retain their mathematical value in an eight-byte signed carrier;
signed values are sign-extended and unsigned values are zero-extended. `Byte`
keeps its nominal source identity. Before type erasure, private MIR normalization
checks arithmetic against the source width. The existing checked `Int` operation
first computes a temporary, then a range guard permits publishing the destination.
Even discarded operations retain these checks. An overflowing `UInt32` product
that exceeds the carrier range is also necessarily outside `UInt32`.
Signed minimum divided by `-1` traps; the corresponding remainder is zero.

Shifts validate the count against the source width before computing a result.
Left shifts retain the low bits and restore signed interpretation when required;
signed right shifts extend the sign, and unsigned/Byte right shifts fill with zero.
Unsigned and Byte complement flips only the source-width bits. Guards and
truncation locals share the existing expansion budget. Each checked operation
adds at most two normal blocks, with allocation charged before those blocks are
published. The source MIR, unwind contracts and numeric conversion rules are
unchanged. This execution lane compares values and terminal traps; it does not
yet prove production native diagnostic codes or cleanup/unwind behavior.

`return_fields` identifies the callee's result locals. `CallAggregate` records
the direct callee, flattened arguments, all destination locals and the normal
successor. Both candidate adapters and both MIR interpreters implement this
protocol. Validation rejects mismatched arity or result width, duplicate result
locals, missing callees or successors, and scalar calls to aggregate-returning
functions. MIR parameter metadata retains the verified source mode: `ref`,
`mut` and `var` aggregate parameters remain unadmitted, even when unused.

Record initializers evaluate fields in source order and then place their
evaluated operands in declaration order. A frontend regression previously
rejected reordered fields with an internal MIR invariant error. This is fixed
for both hosted and native compilation, with an independent hosted test using
a mutable counter closure to observe evaluation order.

`tests/native/native-aot-local-tuples.to` and
`tests/native/native-aot-local-records.to`, together with
`tests/native/native-aot-aggregate-calls.to`, supply real source to the compiler's
`native_mir_probe` and the selected Cranelift adapter. The dedicated
`--source-scalars` mode captures native results and compares them with both the
hosted VM observations and the normalized-MIR interpreter. A minimal C entry
harness calls the generated functions and prints their result; it contains no
expected result or evaluation runtime. The linked functions exercise direct
calls, copies, reassignments, branches, loops, checked overflow and division by
zero. On the admitted x86_64 GNU Linux host, arithmetic traps must be SIGILL;
ordinary nonzero exits or unrelated process signals do not count as agreement.

`scripts/native-source-scalars-test.sh` verifies 590 Cranelift observations across
388 scalar entry functions: 24 tuple cases, 21 local record/nested-value cases,
16 aggregate-call cases, 38 generic-call cases, 36 aggregate-equality cases,
43 Unit/empty-record cases, 67 fixed-width integer cases, 71 sum-value cases and
38 nominal-enum cases, 50 structural-union cases, 54 UInt64 cases, 69 float cases,
33 Char cases, 12 discrete-range value cases and 18 owned-range iteration cases,
including 70 arithmetic
traps. The call corpus
includes nested and concrete generic records, reordered named arguments,
recursion, mutual recursion, branch results, repeated loop calls, independent
results, discarded results and zero-argument aggregate returns.
It rejects source-identity
drift, unsupported functions, missing VM observations, changed oracle results
and an empty corpus, plus four malformed aggregate-call protocols and five
invalid generic identities/call targets. An equality regression also replaces
conjunction with disjunction in a lowered comparison and must be rejected by
the independent source VM observations. Two further regressions remove calls
producing `Unit` or a discarded empty record; both must fail specifically on
disagreement with the source VM, without publishing a partial report. Three integer
regressions remove the range check, corrupt signed shift reconstruction and widen
Byte complement; each must likewise disagree with the VM. Four sum regressions
change a tag, leave an inactive payload nonzero, bypass error propagation and
remove a conversion range decision. Each must disagree with the VM without
publishing a report. Four enum regressions change a variant tag, retain an
inactive payload, alter a named payload field and bypass custom-error propagation;
each must disagree with the VM without publishing a partial report. Five union
regressions cover an altered injection tag, incorrect widened payload mapping,
uncleared inactive storage, an incorrect generic member tag and bypassed error
propagation. Five UInt64 regressions substitute signed addition, ordering,
division or right shift, or bypass the conversion range decision. Each must
disagree with the VM before a report is published. Six float regressions change
the arithmetic width, signed-zero negation, NaN inequality, unsigned conversion,
narrowing range decision or conversion error priority. Six Char regressions
truncate a supplementary scalar, change an escape, reverse ordering, corrupt an
inactive field, omit a discarded call or insert an invalid surrogate literal.
The five semantic changes must disagree with the hosted VM; the invalid literal
must fail scalar validation. Five range regressions reverse exclusive or
inclusive end policy, the lower-bound predicate, unsigned high-bit order or
Unicode scalar order. Three owned-iteration regressions change the exclusive
end predicate, successor step or Char surrogate jump. They must disagree with
the hosted VM. All 58 negative evidence
cases must be rejected without publishing a partial report. Reports are
`native-source-scalars.json`, `native-source-records.json`,
`native-source-calls.json`, `native-source-generics.json`,
`native-source-equality.json`, `native-source-units.json`, `native-source-integers.json`,
`native-source-sums.json`, `native-source-enums.json`, `native-source-unions.json`
`native-source-uint64.json`, `native-source-floats.json`, `native-source-chars.json`
`native-source-ranges.json` and `native-source-range-iteration.json` under
`$CARGO_TARGET_DIR/reliability/evidence/` (the default
target directory is `target`). The standard strict gate and native evaluation
workflow run this source test. This is functional evidence, not a performance
campaign or N1 promotion.

With an explicit `TONDO_LLVM_LLC`, the script also passes `--llvm` to compare
the 545 aggregate-call, generic-call, equality, Unit/empty-record, integer, sum,
enum, union, UInt64, float, Char, range-value and range-iteration cases through LLVM.
`llvm_comparison` retains its actual
version and observations only when requested and successfully executed. Both
candidates use the same source, normalized MIR and hosted observations. LLVM
emits the runtime-dependent checked-conversion helper only when a function
actually needs it, allowing this scalar-only corpus to link without the
evaluation runtime. The native evaluation workflow supplies the pinned LLVM
tool; the standard strict gate requires Cranelift and has no LLVM dependency.

## Concrete generic function instances

Native normalization specializes ordinary generic function bodies before
flattening aggregates. Each key contains the verified callable and its complete,
ordered type argument list. Explicit and inferred uses of the same key share
one instance. The key is registered before visiting the body, so self and mutual
recursion reuse existing instances. Multiple binders and unused binders retain
their identity. This is code generation for existing generic semantics, not
trait implementation specialization or dynamic trait dispatch.

Specialization copies only instantiated bodies and lazily copies the type
interner. It substitutes local, operand, result, projection and call-signature
types with the existing compiler type substitution. Generic record fields are
instantiated from verified declarations with their own binder arguments, so
nested records do not require a concrete source constructor. The source MIR,
source interner and hosted bytecode lowering remain unchanged.

The admitted signatures contain the scalar types listed above and the existing
tuple/record value layouts, including empty nominal records, nominal enums, structural unions, nested Option/Result
and `NumericConversionError`. Generic instances
preserve concrete integer widths before arithmetic normalization and flattening.
Managed values, loans, generic closures, suspension, dynamic trait dispatch and
other MIR protocols outside this ordinary value-call slice remain unadmitted.
A stored named function value can resolve to a direct call only when its local
and every copied source have one static definition. Reassigned or selected
function values require later dynamic dispatch support.

The private `generics` metadata distinguishes `Template { arity }` inventory
entries from `Instance { template, arguments }` executable bodies. Original
ordinals remain stable; instances append in deterministic discovery order.
Instance debug names include canonical type arguments and source maps point
back to the declaration. Templates stay unadmitted even when their binders are
unused. Both adapters validate template ownership, argument arity, concrete
names, unique instance keys and the prohibition on calling templates.

Each extraction permits at most 1,024 additional instances and 1,000,000 copied
body nodes (locals, statements and terminators). Concrete type shapes have a
64-level depth limit and at most 4,096 expanded nodes; this also bounds repeated
tuple children whose interned representation is small. Additional interned
types are limited to 65,536, within the existing interner capacity. Recursive
record expansion retains the 64-level limit. Budget or protocol rejection marks
the requesting function unadmitted and propagates to its callers; it never
publishes a truncated admitted body.

`tests/native/native-aot-generic-calls.to` exercises multiple concrete scalar
and aggregate instances, nested generic records, independent copies, named and
inferred arguments, stored named function values, unused binders, recursion,
branches, loops and overflow. The source campaign repeats probe generation in
independent processes and requires byte-identical output. VM observations bind
to the exact callable name, including canonical type arguments, rather than
assuming native and bytecode function ordinals coincide. The separate bytecode
monomorphization orders its own instances and remains the execution oracle.

`tests/native/native-aot-aggregate-equality.to` additionally checks both truth
outcomes, unequal leaves at every position, nested tuples and generic records,
projected comparisons, overlapping result fields, independent copies, ordinary
and generic calls, recursion, loops and checked arithmetic in operands. Repeated
probe generation must remain byte-identical. The compiler regressions retain
source borrow observations and reject excessive comparison expansion.

`tests/native/native-aot-unit-aggregates.to` covers Unit-only and mixed tuples,
empty and generic records, nested copies and projected writes, structural
equality, named arguments, recursion, loops and branch results. Distinct nominal
empty types remain incompatible at the frontend. Compiler tests preserve the
verified empty field lists and source types, retain loan-parameter rejection,
and reject excessive comparison expansion for Unit and empty-record leaves.
Independent probe processes must produce identical output.

`tests/native/native-aot-integer-aggregates.to` exercises all seven added leaf
types, numeric extrema, nested copies and writes, `with` updates, equality,
generic calls and returns, recursion and loops. Its 37 entry functions produce
67 observations, including 38 expected traps. The cases include checked
arithmetic, signed minimum remainder, carrier overflow, discarded overflow,
negative/oversized shift counts, last-bit shifts, complement and total explicit
numeric conversions. Compiler regressions retain rejection of implicit mixed
numeric operations and Byte arithmetic, verify unchanged source MIR/types and
exercise the shared normalization limit. The sum-value corpus below adds
checked integer conversions using private scalar Result storage; historical
adapter conversion tests do not establish production runtime storage.
Cranelift explicitly checks division/remainder by zero and signed carrier
division overflow before issuing the machine operation, so hardware `SIGFPE`
cannot substitute for the required terminal trap. The corpus checks the
minimum signed carrier's division and remainder by `-1` separately.

`tests/native/native-aot-sum-values.to` supplies 26 scalar entry functions and
71 observations, including three traps. It covers nested options and results,
record and Unit payloads, independent copies, variant replacement, structural
equality with overlapping result destinations, generic propagation and matching,
recursive calls and repeated loop calls. Early absence and failure skip a later
division by zero; successful and discarded expressions retain required traps.
Integer conversion cases cover every admitted narrow destination, both bounds
and adjacent failures, signed/unsigned source crossings, carrier extrema,
projected destinations and arithmetic after conversion. Explicit expected
outcomes supplement the VM/native comparison. Compiler tests also check
immutable source MIR and types, shared expansion limits and rejection of
unsupported managed payloads and loan parameters.

`tests/native/native-aot-enum-values.to` supplies 28 scalar entry functions and
38 observations, including three traps. Unit, positional and named variants
cover nested records/enums/options/results, independent copies, variant changes,
inactive-carrier clearing, equality and overlapping comparison destinations.
Ordinary and generic calls include multiple and unused type arguments, custom
error propagation, recursion, loops and discarded results. Named fields appear
in a different order from their declaration. Explicit results supplement the
VM comparison; repeated probe processes must emit identical bytes. Compiler
tests preserve complete declarations and source MIR, enforce the shared storage
budget and reject inactive unsupported payloads and loan parameters. A hosted
regression with a stateful closure verifies textual evaluation order and
declaration-order field storage independently of native admission.

`tests/native/native-aot-union-values.to` supplies 40 scalar entry functions and
50 observations, including three arithmetic traps. The corpus covers exact
member injection, canonical type order and aliases, member widening across
different storage offsets, scalar/record/enum/Option/Result/numeric-error members,
narrow integer identities, generic members and nested unions. Copies,
replacements, inactive clearing, projected widening, overlapping equality,
recursion, loops, value calls and discarded results retain their existing
contracts. Union errors widen through `?` and preserve early return. Explicit
expected results supplement the VM oracle, and separate probe processes must
produce identical bytes. Focused compiler tests preserve source MIR and type
identity, charge storage and tag tests to the shared expansion budget, resolve
unconstructed generic record members and reject inactive unsupported or
over-depth members and loan parameters. These are source-driven scalar-value
observations on the admitted x86_64 GNU Linux target.

`tests/native/native-aot-uint64-values.to` supplies 49 scalar entry functions and
54 observations, including twelve arithmetic traps. Int-returning entry functions
observe source UInt64 values and calls through explicit value checks; no harness
reinterpretation substitutes for the source oracle. Decimal and radix literals,
`Int.max`, the high bit and `UInt64.max` exercise arithmetic, comparisons,
logical shifts, copies, replacement, recursion and generic nested values.
Every smaller signed/unsigned integer and Byte participates in conversion checks;
negative sources, out-of-range unsigned values and early error propagation retain
their required Result behavior. Compiler tests preserve immutable source MIR and
bounded storage, admit previously rejected inactive UInt64 members and keep
source numeric restrictions. Adapter tests retain literal range rejection and
large-count shift checks. Repeated compiler probes must have identical bytes.

`tests/native/native-aot-float-values.to` supplies 69 scalar entry functions and
69 observations. Both formats exercise arithmetic, ties-to-even rounding,
separate multiply/add, signed zero, subnormals, infinities and all NaN comparison
operators through source calls. Every integer/Byte type participates in numeric
conversions, including full-width boundaries, fractional error priority and
Float64-to-Float32 narrowing. Nested tuples, records, enums, unions and sums
cover independent copies, equality, inactive clearing, propagation, generic
calls, recursion and loops. Int-returning entry functions check exact source
outcomes against 42; raw carrier bits are not substituted for the hosted oracle.
Compiler regressions preserve immutable source MIR/types, deterministic probes,
bounded normalization and source restrictions on mixed formats and Byte
arithmetic. Named `std.math` operations and managed float collections remain
outside this increment.

`tests/native/native-aot-char-values.to` supplies 33 scalar entry functions and
33 observations, including one required arithmetic trap in a discarded
Char-returning call. ASCII, NUL, escapes, combining scalars, supplementary-plane
values and both sides of the surrogate gap retain exact Unicode identity.
Comparisons, literal/guarded matches, copies, projected writes, `with` updates,
overlapping equality destinations and inactive fields use actual source code.
Ordinary/generic calls, named function values, recursion, loops, Option/Result
propagation and union widening use the admitted value protocol. The 32 normal
observations return 42 after explicit source checks. The hosted VM and both
native candidates must agree; independent compiler probes must be identical.
Compiler tests retain source MIR/types, enforce storage limits and reject invalid
literals, numeric operations, managed Char collections and loan protocols.

`tests/native/native-aot-range-values.to` supplies twelve Int-returning entry
functions and twelve source-driven observations, including one arithmetic trap.
Exclusive/inclusive, equal/reversed and signed/unsigned extrema cover both
bound predicates. The Unicode cases cross the surrogate gap and reach the
maximum scalar. Calls, generic copies, nested fields, reassignment and a
discarded range result exercise independent value storage and evaluation.
The VM, Cranelift and explicitly selected LLVM candidate compare exact
observations; separate compiler processes must produce identical probes.
Compiler regressions retain the source MIR and types, charge membership to
the shared expansion budget and keep loan modes unsupported.

`tests/native/native-aot-range-iteration.to` supplies 18 observations from 16
scalar entry functions, including one checked-division trap before the loop
starts. Exclusive/inclusive and empty ranges, signed/unsigned and narrow
integer maxima, the Unicode surrogate gap and maximum scalar, independent
source copies, nested loops, `break`/`continue`, ordinary calls and one concrete
generic instance return the VM's exact outcomes. The 17 normal observations
return 42 after source assertions. The VM, Cranelift and explicitly selected
LLVM candidate compare the same source-driven probe; a repeated compiler
process produces identical bytes. Separate compiler tests confirm that cursor
normalization does not mutate verified MIR/types, that generated cursor work
consumes the shared local budget, and that borrowed parameter modes remain
unadmitted.

Managed fields, recursive value layouts and
aggregate calls through suspension/spawn protocols
remain unsupported. Loan operations, production
runtime integration and the public native build/run
path remain separate work. Admission rejection propagates through direct-call
chains, including recursive components, so an admitted entry cannot reach an
unsupported function through an intermediate caller.

## One MIR, two candidates, one oracle

The runner builds one normalized MIR corpus and validates its debug metadata
before code generation. The same program is lowered to a Cranelift object and
to LLVM IR; no candidate receives a rewritten graph or a backend-specific
source fixture. Each case runs in a fresh native process and its scalar result
is checked against the deterministic normalized-MIR reference oracle. The
seven synthetic storage rows are evaluated by that interpreter before native
execution; the existing cleanup/async/select/thread rows retain their already
validated runtime-contract expectations and are replayed from the same physical
lane. The report records only logical case IDs, function ordinals and statuses,
never paths, addresses, process IDs or host payloads.

The private handle ABI now has concrete aggregate storage for the bounded
evaluation lane:

* `aggregate-new`, `aggregate-set`, `aggregate-get`, `aggregate-len` and
  `aggregate-tag` store bounded arrays, sets, records, tuples and closures
  without exposing pointers or object layout;
* `aggregate:<index>` is a checked one-level projection used by both adapters;
* a closure carries its callable as `function:<ordinal>` plus mutable capture
  values; `aggregate-set` changes the capture before invocation;
* `indirect-call` accepts exactly a verified function ordinal, capture and
  argument. Arbitrary symbols and raw function pointers are rejected;
* the existing runtime transitions cover cleanup, ownership/COW,
  task/select/thread state, cancellation and traps. They are part of the same
  AOT corpus rather than a separate smoke implementation.

The aggregate table is deliberately bounded in this proof lane. Exceeding its
capacity, using an invalid field, or naming an unadmitted storage shape returns
an error/trap and is reported as a fail-closed capability. A production runtime
may replace the table with an optimized representation without changing the
normalized MIR contract.

## Admitted corpus

The report contains seven storage/ABI cases (`array-storage`,
`record-projection`, `closure-mutable-capture`, `direct-call`,
`aggregate-metadata`, `set-storage` and `ownership-cow`) plus the existing
cleanup, async, select and thread cases. The feature matrix is emitted in the
`native_aot_lowering` report field and every row must be `passed` for Cranelift,
LLVM and the reference oracle.

An explicit unsupported function is compiled as a trap by Cranelift and as
`unreachable` by LLVM. Its two inventory rows explain why the capability is not
admitted; this prevents a green function count from hiding a missing lowering.

## Machine-readable authority and checks

[`testing/native-aot-lowering.json`](../../testing/native-aot-lowering.json) is
the machine-readable authority. Static and mutation checks are
`scripts/native-aot-lowering-{check,test}.sh`. The executable evidence is the
`native_aot_lowering` field in
`target/reliability/evidence/native-evaluation-runner.json`, generated by
`scripts/native-evaluation-runner.sh`.

This contract does not choose a backend, define the public ABI, or close Gate
N1. Source-driven lowering, production runtime linking, memory, quality and
repeated-performance campaigns remain pending. Historical prototype reports
retain their measured scope and cannot promote the production pipeline.
