# ADR-013: Monomorphize initial generic code

**Status:** accepted

## Context

Tondo traits use static dispatch and do not expose dynamic trait objects.

## Decision

Instantiate generic callables for concrete substitutions between verified MIR
and bytecode lowering. One instance is identified by the pair of its resolved
callable ID and its complete canonical type-argument vector.

The request-local worklist starts with every non-generic callable and every
generic function value retained by a closed constant. It then follows static
function operands in each reached MIR template, applies the enclosing
substitution to nested specializations, and inserts unseen instances in stable
order. Reaching the same recursive instance is a no-op. Recursion that keeps
constructing a different type is stopped by the generic-instantiation budget.

Every reached MIR type is substituted through the request's cloned interner
before bytecode construction. Executable bytecode callables consequently have
zero generic arity, direct calls carry no runtime type-argument pack, and slots,
places, projections, operations, tags, and outcomes refer only to their
concrete forms. Generic nominal declarations remain once as layout templates;
the bytecode verifier substitutes their concrete arguments when checking a
field or variant projection.

Source constraints are checked before an instance is admitted. The closed
bootstrap `Discard` constraint is executable now. Other intrinsic and user
trait obligations remain represented and budgeted, but cannot admit executable
code until CAP-001 and TRAIT-001 through TRAIT-005 provide their proof rules.

## Consequences

Bytecode stays simply typed and calls remain direct. A generic function that is
never reached produces no executable body, while equal substitutions across
multiple call sites share one body. Constants can root an otherwise unreachable
specialization because their function values are executable data.

Compilation cost and code size grow with the number of unique instances. The
compiler therefore bounds both unique generic instances and newly specialized
type nodes, reports exhaustion as `T0002`, and never publishes partial
bytecode. Ordering uses sorted request-local sets, not hash iteration, so equal
inputs produce equal bytecode.

This decision does not freeze a serialized ABI, require an incremental cache,
or preclude later internal code sharing. Any such optimization must preserve
the same static-dispatch and concrete-bytecode behavior.
