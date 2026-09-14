# Native AOT lowering contract

`NATIVE-AOT-LOWER-001` remains pending after the 2026-09-07 audit. The historical
evaluation described below tests a bounded normalized-MIR corpus and generated
C runtime. Its `closed` register status describes that prototype boundary;
it does not discharge the tracker's source-driven production lowering task.
Both candidates consume one immutable `tondo-mir-backend/1` program, but
synthetic storage cases do not prove that the frontend can produce those
operations from Tondo source.

## Source-driven local value aggregates

The compiler lowers local tuples and records whose leaves are `Int` and `Bool`
into independent scalar locals. Nested tuples/records and instantiated generic
records use the same route. Construction, field reads and writes, copies,
whole-value and nested-field replacement, `with` updates, branches and
loop-carried values preserve value semantics.
Assignments snapshot the right-hand fields before writing their destinations.
This route does not allocate aggregate handles or define a tuple calling
convention. The native normalization operates on a private copy of MIR blocks;
it leaves verified source MIR and hosted bytecode unchanged. Record layouts
come from instantiated, verified MIR constructors, with declaration-order
member identities. Each generated local belongs to one scalar leaf; a copy
never shares its mutable fields with its source.
Expansion is bounded to 65,536 additional locals per function, including
right-hand snapshots; wider repeated copies are rejected before exceeding
that allocation budget. Layout depth is bounded to 64 aggregate levels.

Record initializers evaluate fields in source order and then place their
evaluated operands in declaration order. A frontend regression previously
rejected reordered fields with an internal MIR invariant error. This is fixed
for both hosted and native compilation, with an independent hosted test using
a mutable counter closure to observe evaluation order.

`tests/native/native-aot-local-tuples.to` and
`tests/native/native-aot-local-records.to` supply real source to the compiler's
`native_mir_probe` and the selected Cranelift adapter. The dedicated
`--source-scalars` mode captures native results and compares them with both the
hosted VM observations and the normalized-MIR interpreter. A minimal C entry
harness calls the generated functions and prints their result; it contains no
expected result or evaluation runtime. The linked functions exercise direct
calls, copies, reassignments, branches, loops, checked overflow and division by
zero. On the admitted x86_64 GNU Linux host, arithmetic traps must be SIGILL;
ordinary nonzero exits or unrelated process signals do not count as agreement.

`scripts/native-source-scalars-test.sh` verifies 45 observations across 20
scalar functions: 24 tuple cases and 21 record/nested-value cases, including
three arithmetic traps. It rejects source-identity
drift, unsupported functions, missing VM observations, changed oracle results
and an empty corpus. Reports are `native-source-scalars.json` and
`native-source-records.json` under `$CARGO_TARGET_DIR/reliability/evidence/` (the default
target directory is `target`). The standard strict gate and native evaluation
workflow run this source test. This is functional evidence, not a performance
campaign or N1 promotion.

Managed fields, other numeric representations, empty records/tuples, whole
aggregate equality, and aggregate values crossing calls/returns remain
unsupported. Scalar fields may be passed to supported scalar functions.
Loan operations, production runtime integration and the public native build/run
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
