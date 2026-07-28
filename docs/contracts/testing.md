# Test harness contract

**Status:** accepted for bootstrap

## Fixture classes

The repository reserves these roots:

~~~text
tests/spec/
tests/compile-pass/
tests/compile-fail/
tests/runtime/
~~~

Cases are discovered in lexicographic logical-path order. A `.to` file is the
source. Optional adjacent files use the same basename:

- `.codes`: one expected diagnostic code per line; mandatory for spec and
  compile-fail fixtures.
- `.jsonl`: exact structured diagnostic snapshot.
- `.stderr`: exact human diagnostic.
- `.stdout`: exact formatter or program stdout.
- `.runtime-stderr`: exact program stderr.
- `.exit`: decimal process exit code.

No test may infer success from a missing expected file. Each fixture class has a
closed default contract documented by its directory.

## Inline fixtures

Unit and integration tests may construct `SourceInput::virtual_file` directly.
They must still provide source ID, module, logical path, edition, target, profile,
capabilities, and limits through the normal driver.

Inline tests are preferred for small algorithmic behavior. Filesystem fixtures
are preferred for stable public output and multi-file behavior.

## Specification fence extraction

The maintained test extractor follows language-spec section 21.6 directly; it
does not delegate scanning to a Markdown library. For the pinned `0.1` edition
it:

- Recognizes only an unindented opening line beginning exactly with `~~~tondo`
  and an exact unindented `~~~` closer.
- Accepts only `tondo`, `fragment`, `script`, `compile-fail`, and `pseudocode`
  header forms defined by the spec.
- Normalizes fence content to LF and appends one final LF.
- Records the opening byte offset and processes fences in that order.
- Carries the explicit fixture name or the normative `spec.0_1` default.
- Carries the exact distinct `Edddd` set for `compile-fail`.
- Runs every non-pseudocode fence through the ordinary edition, target,
  formatter, diagnostic, and fixture paths.
- Emits the complete machine-readable result record and SHA-256 fields required
  by section 21.6.

Extraction failures are document failures, not Tondo source diagnostics. Every
lexically valid, syntactically valid non-pseudocode fence is also formatted,
reparsed, and checked for a second identical formatting result.

## Regression rule

Every compiler bug receives the smallest Tondo input that reproduces it and a
test at the lowest public boundary that would have caught it. A test must not
depend on physical absolute paths, wall time, locale, network, hash iteration,
or scheduling order.

## Value-copy representation equivalence

`tests/runtime/value-copy/` is the stable black-box corpus for logical copy
semantics. Its cases separately cover:

- preservation of every managed `Copy` shape admitted by the bootstrap;
- independence of compound values and closure state after a write;
- deliberate identity sharing through `Ref[T]`;
- slice snapshots separated from their source in both write directions,
  including nested elements, overlapping assignment, borrowed materialization,
  and `mut` region updates;
- iteration over copied arrays, maps, sets, ranges, and strings;
- the exact panic class and exit status after copying; and
- retained values under sustained allocation and GC pressure.

The oracle is the complete `FixtureObservation`: compilation status, process
exit code, ordered diagnostic codes and renderings, stdout, and stderr. VM
statistics are intentionally absent. Cases cannot inspect heap handles,
addresses, allocation or reference counts, collection timing, or eager versus
COW storage.

The ordinary fixture pass checks these observations against sidecars. The
equivalence pass runs the identical sources again with an initial GC threshold
of one, checks the same sidecars, and requires byte-for-byte equal observations.
All capacity limits remain at their ordinary values so the oracle cannot depend
on an object or byte count. A future COW candidate must execute this corpus
unchanged; if eager and COW implementations coexist, their adapters compare
this same observation type directly.

## Array mutation permissions

Array-mutation regressions distinguish four cases: fixed-length mutation of a
complete `mut` owner, fixed-length mutation of a `mut` slice, length-changing
replacement of a complete `var` owner, and structural replacement of a
complete nested element without changing the outer array length. Compile-fail
fixtures reject root replacement through `mut` and every `var` slice.

Adversarial bytecode replaces an otherwise valid in-place result with an
`Array` of another runtime length. The program remains structurally typed, so
execution must reject publication at the dynamic fixed-extent boundary. The
legitimate sliced path also runs with an initial GC threshold of one to prove
that measuring its lender keeps the pending replacement rooted.

## Named array sequences

ARRAY-007 fixtures execute both dot and qualified `concat`/`repeat`, including
a receiver that crosses a `ref Array[T]` parameter. They observe concat order,
zero repetition, an empty receiver with `Int.max`, nested write independence,
and preserved `Ref` identity. Separate runtime cases fix `P0011` for a negative
count and `P0005` for a mathematical result length outside `Int`; compile-fail
cases fix the `T: Copy` obligation and reject a redundant explicit mode on the
qualified `self` receiver or explicit type arguments on the intrinsic owner.

Internal execution repeats the nested case with an initial GC threshold of one
and checks receiver-before-argument panic precedence. Mutated typed HIR, MIR,
and bytecode independently prove that operation kind, argument signature,
shared receiver access, and closed capability cannot be forged past their
admission gates.

## Text interpolation

TEXT-003 fixtures cover scalar and `String` intrinsic display, explicit and
generic user `Display` implementations, preservation of a generic value after
observation, temporary receivers, left-to-right side effects, doubled braces,
escapes, and multiline dedentation. Missing evidence is fixed as `E1105`.
The same runtime fixture is repeated with an initial GC threshold of one.
Mutated bytecode separately rejects an interpolation arity mismatch and a
forged intrinsic Display receiver association.

## Homogeneous variadic packs

VARIADIC-001 has one end-to-end runtime fixture covering empty and populated
packs, a fixed prefix, generic inference, body-visible `Array[T]`, methods,
direct and indirect named functions, explicit and contextual closures,
left-to-right effects, nested managed elements, and affine opaque closure
elements consumed through `CallOnce`. The same fixture runs again with an
initial GC threshold of one and must produce an identical complete
observation.

Compile-fail fixtures fix `E1102` for heterogeneous elements, `E1115` for an
unnamed pack, `E1411` for mutation through its immutable body binding, and
`E1401` for reuse after an affine element move. Unit tests retain the
unique-final value-parameter restrictions and exact function type, HIR, and MIR
associations. Adversarial bytecode changes a valid `VariadicElement` into a
fixed target and must be rejected before execution. Explicit `...array`
transfer behavior is intentionally outside this gate and belongs to
VARIADIC-002.

## Conformance separation

Implementation fixtures may test private invariants and `T` diagnostics. The
future `tondo-conformance-0.1` suite contains only normative behavior and can run
against another implementation through adapters.
