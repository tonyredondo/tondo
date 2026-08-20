# Test bytecode backend

**Status:** implemented for `UTEST-BACKEND-001`

`tondo_compiler::driver::Operation::Test` is the executable boundary between
the static `suite`/`test` tree and the hosted VM. The selected declaration is
lowered to an ordinary private `fn main()` while preserving imports, normal
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

Discovery derives the source-class segment from the canonical logical path:
files below `tests/` produce `integration` IDs and other test companion files
produce `unit` IDs. The entry also retains that logical path for CODEOWNERS
matching; physical paths and insertion order never participate in identity.

This is the execution bridge used by the test runner. Envelope operations such
as logs, tags, attachments, snapshots, retries and scheduling remain owned by
their sealed runner modules and are not reimplemented in the backend.

## Canonical suspension migration

The backend compiles test bodies through the same inferred-effect path as
ordinary source. A test or setup body is always written with `fn`; a direct
call to a `suspends` operation waits implicitly, while `await call()` is
rejected. `Join` handles retain explicit
consumption, and the runner does not expose an `async` test API or infer an
effect from a body-local compatibility modifier. `@sync` and `@nosuspend`
remain compiler-enforced boundaries and reject a suspendible call with
`E1601`. The independent adapter conformance case fixes compile-pass,
compile-fail, runtime, and interface-hash observations for this contract.
`async fn` is not accepted source syntax.
