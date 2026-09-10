# Convention-first project discovery

**Status:** conventional TOML discovery is implemented for the unpublished
Tondo 0.1 draft. Public test compilation now validates production before the
combined target and validates the full target before selection. Production
semantics are retained for unit overlays and isolated integration consumers;
workers execute captured bytecode. Development dependencies, declared runtime
inputs and ordinary meta packages have public CLI integration. Their owner
promotion still requires the consolidated evidence and acceptance gate.

The user-facing CLI accepts a project directory and never requires a JSON
manifest. `tondo` uses the current directory by default; `--project <dir>`
selects another root. Discovery is a CLI concern and ends by producing the same
closed `ProjectPlan` consumed by the compiler's pure boundary.

## Layout

```text
app/
  src/main.to
  src/models/user.to
  tests/user_test.to
  tondo.toml
  tondo.lock.toml
```

- `src/` is the production source root. `src/main.to` is the preferred root.
- `tests/` is included when `src/` exists; without `src/`, it is included only
  for a project marked by `tondo.toml` or `main.to`.
- A directory without `src/`, `main.to` or `tondo.toml` is not guessed to be a
  project, even if unrelated `.to` files exist below it.
- Symlinks, hidden directories, `target/` and `vendor/` are ignored.
- Physical and logical paths use relative `/`-separated spelling. Source files
  are sorted by UTF-8 path bytes before the internal graph is generated.

## Human configuration

`tondo.toml` is optional and admits `[package]`, `[target]`, `[dependencies]`,
`[test]` and `[meta]`. It sets package name/edition, target/profile/registry,
capabilities/features and dependency aliases. Sources and modules are never
listed there. Unknown keys are rejected.

External dependencies require the generated `tondo.lock.toml`; a project with
no dependencies has an equivalent lock materialized in memory. There is no
JSON project configuration or compatibility fallback.

## Meta packages and local lock resolution

Human meta declarations use `[meta.dependencies]`, `[[meta.inputs]]`,
`[[meta.generators]]` and `[[meta.derive_providers]]`. Provider package names
are aliases from `meta.dependencies`; trait and model-root package names are
runtime aliases, including the root package's own name and `std`.

```toml
[meta.dependencies]
builder = { package = "workspace:builder@1", path = "tools/builder" }

[[meta.inputs]]
name = "schema"
path = "inputs/schema.txt"

[[meta.generators]]
id = "build-values"
provider = { package = "builder", entry = "main.generate" }
inputs = ["schema"]
model_roots = [{ package = "app", module = "model" }]
outputs = [{ logical_path = "generated/values.to", module = "generated" }]
limits = { steps = 100000, memory_bytes = 1048576, output_bytes = 8192 }

[[meta.derive_providers]]
trait = { package = "app", module = "model", name = "ValueOf" }
provider = { package = "builder", entry = "main.expand" }
limits = { steps = 100000, memory_bytes = 1048576, output_bytes = 8192 }
```

Each local meta package uses the same `src/` and directory-to-module discovery
rules. Its optional `tondo.toml` contains `[package]` and `[dependencies]`;
dependency entries contain an exact `package` identity and a `path` relative to
that package. The complete graph must remain inside the selected project.
Cycles, conflicting package locations and unknown keys are rejected. Runtime
sources and meta sources remain separate graphs.

`tondo lock [--project <dir>]` resolves these explicitly named local sources,
compiles provider entries for the closed meta target without executing them,
and writes `tondo.lock.toml` after validating the complete candidate. It fixes
source, input, package, companion and executable hashes. It preserves an
existing `[test]` section and leaves the old lock unchanged if resolution
fails. This command does not resolve external runtime dependencies or access a
package registry. Normal `check`, `run` and `test` require the existing lock for
meta declarations and reject stale inputs without refreshing it.

Generators and derives observe the same authored semantic base. Only root
module closures are resolved before generation; ordinary consumers can import
an output that will be produced in this round. A model dependency on such an
output is `E2109`. Generated files remain in the compiler's source database,
and successful generator outputs carry request-derived source identities and
generation records in the interface and artifact. Tests retain the sealed
generated production graph and do not rerun its generators.

## Test-plan sidecar

`tondo test` optionally reads `tondo.test.toml`. There is no JSON sidecar. The
TOML describes the closed `tondo-test-plan-draft` shape and is converted to the
internal value model before validation. The sidecar is optional, and CLI flags
overlay its selection/policy without rewriting it.

Before selection and worker creation, an explicit sidecar is reconciled with
conventional discovery by source class, physical path, logical path, module,
input name and owning package. Missing or additional sources and changed
identities are usage errors even for a list or an empty selection. The host
currently reads sources while constructing internal lock hashes, before this
reconciliation; this is not a claim of metadata-only discovery before source I/O.

## Determinism and boundary

Discovery is deterministic and local: no network, registry lookup, clock,
environment or process is consulted. A malformed TOML file, invalid package
alias, missing dependency lockfile, source collision or missing root source is
a usage error before lexing. After discovery, the compiler sees only the
materialized manifest/lockfile bytes and declared source inputs.
