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

Each leaf and suite phase has an independent instruction counter. Cooperative
children and blocking workers share their owner's counter. Live VM heap charges
follow the allocating node, including across blocking workers; retained suite
objects do not consume a child's budget. GC and heap destruction release those
charges. Compiler entry and structural cleanup have separate finite instruction
allowances. Resource exhaustion terminates the affected node, cancels its work
and preserves completed and unrelated sibling evidence; it is not retried.
Explicit user defers are not guaranteed after a resource terminal.

Blocking host responses carry their own testing control signal back to the
worker. `skip` retains its reason and status through worker and parent cleanup;
ordinary worker failures are delivered when the owning task resumes. Deferred
pool shutdown uses the same pending lifecycle state as an ordinary call.

The public coordinator admits closed test dependencies and runtime input
descriptors before executing its immutable compiled participation; see
`test-dependencies.md` and `test-input-runtime.md`. Phase deadlines and external
interruption are described in `test-limits.md` and `test-interrupt.md`.

Aggregate hosted-value memory accounting, complete scheduler capacity
accounting, generalized async Writer capture and native execution remain
separate open boundaries. An infrastructure failure that prevents isolation
does not become a successful partial report. Current whole-tree promotion
evidence is still required for T0.

## Error and source identity

Hidden leaf and setup callbacks infer `Unit ! E` using ordinary generic closure
checking and require `E: Discard`. The VM consumes an error after cleanup and
reports its concrete type without implicitly serializing its payload. Retries,
repeats and blocked suite attempts preserve that original error record.

Copied source ranges map static diagnostics, related locations and runtime
terminal spans back to the original file. Diagnostic IDs are regenerated from
the mapped locations. A fix is retained only if every edit maps to verbatim
user source; generated helper edits are not presented as user fixes.

Private runner calls carry the exact identifier ranges emitted by the compiler.
The checker requires that provenance for every direct participation operation;
the generated file's origin alone grants no access to copied user bodies,
suite setup, deferred code or ordinary helpers. These operations cannot be
function values. The public runner rejects such calls before listing or
filtering, with diagnostics at the original source location.

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
