# Native AOT lowering contract

`NATIVE-AOT-LOWER-001` remains pending after the 2026-09-07 audit. The historical
evaluation described below tests a bounded normalized-MIR corpus and generated
C runtime. Its `closed` register status describes that prototype boundary;
it does not discharge the tracker's source-driven production lowering task.
Both candidates consume one immutable `tondo-mir-backend/1` program, but
synthetic storage cases do not prove that the frontend can produce those
operations from Tondo source.

## Source-driven value aggregates, direct calls and equality

The compiler lowers local tuples and records whose leaves are `Int` and `Bool`
into independent scalar locals. Nested tuples/records and instantiated generic
records use the same route. Construction, field reads and writes, copies,
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
Expansion is bounded to 65,536 additional locals per function, including
right-hand snapshots and comparison results; wider repeated copies and
comparisons are rejected before exceeding
that allocation budget. Layout depth is bounded to 64 aggregate levels.

Intrinsic `==` and `!=` compare these tuples and records structurally, including
nested values, projected subaggregates and concrete generic instances. Equality
combines equal leaves with Boolean conjunction; inequality combines unequal
leaves with disjunction. All leaves are `Int` or `Bool`, so these reads and
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
are evaluated in source order, then flattened into `Int`/`Bool` scalar carriers
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

`scripts/native-source-scalars-test.sh` verifies 135 Cranelift observations across
65 scalar entry functions: 24 tuple cases, 21 local record/nested-value cases,
16 aggregate-call cases, 38 generic-call cases and 36 aggregate-equality cases,
including six arithmetic
traps. The call corpus
includes nested and concrete generic records, reordered named arguments,
recursion, mutual recursion, branch results, repeated loop calls, independent
results, discarded results and zero-argument aggregate returns.
It rejects source-identity
drift, unsupported functions, missing VM observations, changed oracle results
and an empty corpus, plus four malformed aggregate-call protocols and five
invalid generic identities/call targets. An equality regression also replaces
conjunction with disjunction in a lowered comparison and must be rejected by
the independent source VM observations. Reports are
`native-source-scalars.json`, `native-source-records.json`,
`native-source-calls.json`, `native-source-generics.json` and
`native-source-equality.json` under
`$CARGO_TARGET_DIR/reliability/evidence/` (the default
target directory is `target`). The standard strict gate and native evaluation
workflow run this source test. This is functional evidence, not a performance
campaign or N1 promotion.

With an explicit `TONDO_LLVM_LLC`, the script also passes `--llvm` to compare
the 90 aggregate-call, generic-call and equality cases through LLVM.
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

The admitted signatures contain `Int`, `Bool`, `Unit` and the existing nonempty
tuple/record value layouts. Aggregate leaves retain the `Int`/`Bool` restriction.
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

Managed fields, other numeric representations, empty records and
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
