# Test-input materialization and revocation

**Status:** bounded Rust materialization model; public integration remains open
under `UTEST-INPUTS-001`.

`tondo_compiler::test_inputs::TestInputPlan` remains the value-free planning
boundary. `tondo_compiler::test_input_runtime` is the worker boundary that
models materialization of descriptors accepted by the selected build/runtime
target. The public CLI captures source and snapshot inputs, but has no route
yet for registering these runtime providers or transporting their descriptors
to the worker. The following properties apply to the Rust model.

Public bytes are checked against the declared `sha256:` before a worker is
returned. Provider errors are reduced to the logical input name and provider
panics are contained, so a secret value cannot become a diagnostic through the
materializer. A build worker cannot request runtime-only inputs.

`WorkerInputs` owns every materialized buffer and zeroes it on explicit
revocation and on drop. Revocation is idempotent; after it, access returns a
typed `Revoked` error. The coordinator report contains public counts/digests,
secret count, the already planned secret-profile digest and reproducibility
state, but no secret names, values, hashes, or bytes. The API cannot redact a
secret that the Tondo program explicitly copies into a log, tag, stream,
failure, snapshot, or artifact; that remains the documented caller boundary.
