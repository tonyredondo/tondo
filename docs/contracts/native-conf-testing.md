# Native testing-target conformance contract

The testing leaf executes the adapter against pass, fail and isolation cases.
It checks that logs and failure code `P0007` survive the native observation,
cleanup runs exactly once and a fresh attempt does not inherit state from a
previous process. Cranelift and LLVM are checked independently against the VM
oracle; a missing lifecycle field or backend-specific result is a failure.

The leaf covers the public runner protocol only. It does not grant production
entries access to `std.testing` and it does not choose a backend.
