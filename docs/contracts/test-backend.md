# Test bytecode backend

**Status:** executable hosted bytecode boundary; the public testing promotion
remains open under the audit's reopened testing tasks.

`tondo_compiler::driver::Operation::Test` is the executable boundary between
the static `suite`/`test` tree and the hosted VM. The selected declaration is
lowered to an ordinary private `fn __tondoTestEntry()` while preserving imports, normal
declarations and enclosing suite setup. That generated module is then sent
through the existing resolver, HIR checker, MIR lowering, bytecode verifier and
VM; the backend does not interpret Tondo source in Rust or report success from
a host closure.

The operation accepts one visible test ID (or a unique leaf name). Omitting the
selector is only valid when the root contains exactly one test. A test target
with no tests, an ambiguous selector, or a production `main` is rejected before
execution. The VM result and diagnostics are returned through the normal
`CompilationOutput`, so assertion failures, panics and resource limits retain
their existing semantics.

Discovery consumes an explicit source class from the closed project plan.
Conventional discovery assigns `integration` to files below `tests/` and `unit`
to test companions; the backend does not guess the class again from a path.
The entry also retains that logical path for CODEOWNERS
matching; physical paths and insertion order never participate in identity.

The public runner compiles an entire participation, preserving suite scopes
and wrapping each selected child in a compiler-owned VM boundary. Production
semantics are reused from the checked snapshot as described in `test-overlay.md`.
Compilation finishes before selection can omit a test body. Fresh workers
execute the captured verified bytecode through `test_backend::execute_compiled`;
they do not recompile source. The bytecode boundaries notify the host of node
entry and completion, and the host retains each node's envelope and evidence.

Synchronous testing host operations preserve their typed terminal and issue one
VM control unwind. This stops the leaf or setup immediately, executes ordinary
cleanup and permits sibling continuation. Assertion helpers do not merely set
a flag and continue. Output, artifact and snapshot preflight failures retain
`resource-limit`; they are not assertion failures eligible for retry. The VM
distinguishes its private control unwind from actual language panics, including
those observed during cleanup. A skip cannot hide a cleanup panic.

`console.print` and `console.println` append to the active node's stdout and
share its output budget with logs and tags. A println's newline participates in
the same atomic preflight. Suite setup and teardown share their own capture;
leaf capture does not enter the suite buffer or the worker protocol stream.

This evidence does not yet establish independent VM instruction/heap budgets
or OS processes for siblings in a suite participation, per-phase wall-clock
deadlines, generalized async Writer capture, runtime input providers,
dev-dependencies, or native execution. The hosted external interruption route
and its remaining boundaries are recorded in `test-interrupt.md`. Whole-VM resource
failures still abort the participation without a complete per-node report.
The focused CLI regressions exercise assertions, budget rejection without
retry, sibling evidence, stdout attribution, newline preflight and cleanup
panic precedence through actual worker processes.

## Canonical suspension migration

The backend compiles test bodies through the same inferred-effect path as
ordinary source. A test or setup body is always written with `fn`; a direct
call to a `suspends` operation waits implicitly, while `await call()` is
rejected. `Join` handles retain explicit
consumption, and the runner does not expose an `async` test API or infer an
effect from a body-local compatibility modifier. `@sync` and `@nosuspend`
remain compiler-enforced boundaries and reject a suspendible call with
`E1601`. Adapter corpus entries remain separate from executable public/native
conformance promotion; their presence alone is not proof of this pipeline.
`async fn` is not accepted source syntax.

Receiver-selected calls may reveal suspension after the source-name pass.
The checker repeats the unsealed extension with monotonically promoted
callable effects until signatures stabilize, then verifies typed HIR. Earlier
function values, contextual closure types and callers are rechecked against
those signatures; production bodies and their sealed types are retained.
