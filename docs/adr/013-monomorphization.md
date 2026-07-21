# ADR-013: Monomorphize initial generic code

**Status:** accepted

## Context

Tondo traits use static dispatch and do not expose dynamic trait objects.

## Decision

Instantiate generic callables for concrete substitutions between verified MIR
and bytecode lowering. An instance is either a named callable ID or a source
closure ID paired with its complete canonical type-argument vector.

The request-local worklist starts with every non-generic callable and every
generic function value retained anywhere inside a closed constant. It then
follows static function operands and closure aggregates in each reached MIR
template, applies the enclosing substitution to nested specializations, and
inserts unseen named or closure instances in stable order. A source-trait
associated function retained by a constant is resolved to the same concrete
override or default as an operand reached from MIR, and the constant stores only
that selected callable. Reaching the same recursive instance is a no-op.
Recursion that keeps constructing a different type is stopped by the shared
generic-instantiation budget; a unique generic closure body consumes a distinct
entry from that budget.

Every reached MIR type is substituted through the request's cloned interner
before bytecode construction. Executable bytecode callables consequently have
zero generic arity, direct calls carry no runtime type-argument pack, and slots,
places, projections, operations, tags, and outcomes refer only to their
concrete forms. Generic nominal declarations remain once as layout templates;
the bytecode verifier substitutes their concrete arguments when checking a
field or variant projection.

Source constraints are checked before an instance is admitted. All six closed
intrinsic constraints (`Copy`, `Discard`, `Equatable`, `Key`, `Send`, and
`Share`) use the completed structural proof; open source/prelude traits use the
unique coherent static-selection proof. Function, concrete closure, generic,
and opaque callables use the completed closed protocol proof; each call retains
one exact signature, supported protocol, and compatible access form.

## Consequences

Bytecode stays simply typed. Named calls remain direct; closure values dispatch
through a concrete callable identity and body whose hidden first parameter is
the environment. A generic named function or closure that is never reached
produces no executable body, while equal substitutions across multiple call
sites share one body. Constants can root an otherwise unreachable
specialization because their function values are executable data.

Compilation cost and code size grow with the number of unique instances. The
compiler therefore bounds both unique generic instances and newly specialized
type nodes, reports exhaustion as `T0002`, and never publishes partial
bytecode. Ordering uses sorted request-local sets, not hash iteration, so equal
inputs produce equal bytecode.

This decision does not freeze a serialized ABI, require an incremental cache,
or preclude later internal code sharing. Any such optimization must preserve
the same static-dispatch and concrete-bytecode behavior.
