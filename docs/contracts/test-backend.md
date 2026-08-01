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
