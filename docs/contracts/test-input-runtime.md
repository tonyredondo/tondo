# Test-input materialization and revocation

**Status:** public hosted environment integration, worker revocation and
report identity are verified through the joint testing gate. The public
provider boundary is the explicit environment route described below.

`tondo_compiler::test_inputs::TestInputPlan` remains the value-free planning
boundary. `tondo_compiler::test_input_runtime` is the worker boundary that
materializes descriptors accepted by the selected build/runtime target. The
public CLI captures source and snapshot inputs and accepts explicit runtime
environment declarations in `tondo.toml`. The isolated worker revalidates their
closed input plan before starting any Tondo entry.

## Public configuration

Each `[[test.inputs]]` record uses the value-free descriptor fields from the
toolchain input-plan contract. `profile` must be `runtime`. Environment sources
use `environment:NAME` and require the production target's `environment`
capability. A public record supplies its expected `sha256`. A secret record
uses provider `environment`, a `descriptor` naming the host variable and an
optional opaque `version`. The source names the variable visible to Tondo;
the secret descriptor can name a different host variable.

```toml
[[test.inputs]]
name = "host:token"
source = "environment:APPLICATION_TOKEN"
profile = "runtime"
visibility = "secret"
provider = "environment"
descriptor = "CI_APPLICATION_TOKEN"
version = "deployment-v1"
capability = "environment"
```

Unknown fields, value fields, unregistered providers, unsupported sources,
duplicate input names or environment destinations, invalid environment names,
missing capabilities and secret build inputs are rejected. This hosted adapter
registers the environment provider only. It does not silently substitute a
provider or grant capabilities. The normal build does not materialize test
inputs, and their declarations cannot change production interfaces/artifacts.

## Capture and isolation

Public environment values are captured and hash-checked before selection,
including `--list` and an empty selection. They travel with the compiled
participation, so retries cannot reread a different public value. Secret values
are read only inside the isolated worker; listing validates descriptors without
opening their providers. Transport and report identity contain only their
descriptors, count and versioned/unversioned profile digest.

Each fresh participation/retry/iteration worker materializes its own inputs.
Tondo receives no process arguments and an environment containing only the
declared destinations. The worker starts in the project directory; its physical
path is not part of input or report identity. The worker's private host process
inherits the launcher environment to access explicitly named providers; the
Tondo environment API has no fallback to that ambient environment.

Unavailable inputs or failed public hash checks prevent every entry of that
worker from starting. The invocation exits with an input diagnostic and code
`1`, preserving prior JSON, JUnit and snapshot outputs. Materialized bytes must
fit the declared memory budget before VM execution. Runtime input storage and
the host's environment buffers are revoked when execution ends, including error
and unwind paths, before a worker result is published. A killed worker loses
its process-owned inputs; incomplete isolation remains an infrastructure error.

The shared materializer checks public bytes against the declared `sha256:`;
it does not compute a content hash for secret bytes. Provider errors are reduced
to the logical input name, and a build worker cannot request runtime-only inputs.
Custom Rust providers must also keep their own logging and panic hooks free of
secret values; catching a provider panic cannot retract output it already wrote.

`WorkerInputs` owns every materialized buffer and zeroes it on explicit
revocation and on drop. Revocation is idempotent; after it, access returns a
typed `Revoked` error. The coordinator report contains public counts/digests,
secret count, the already planned secret-profile digest and reproducibility
state, but no secret names, values, hashes, or bytes. The API cannot redact a
secret that the Tondo program explicitly copies into a log, tag, stream,
failure, snapshot, or artifact; that remains the documented caller boundary.

The provider layer does not make external filesystem, process or network effects
reproducible. Its report describes only the closed inputs it actually received.
