# Convention-first project discovery

**Status:** implemented for the unpublished Tondo 0.1 draft

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

`tondo.toml` is optional and may contain only `[package]`, `[target]` and
`[dependencies]`. It sets package name/edition, target/profile/registry,
capabilities/features and dependency aliases. Sources and modules are never
listed there. Unknown keys are rejected.

External dependencies require the generated `tondo.lock.toml`; a project with
no dependencies has an equivalent lock materialized in memory. There is no
JSON project configuration or compatibility fallback.

## Test-plan sidecar

`tondo test` optionally reads `tondo.test.toml`. There is no JSON sidecar. The
TOML describes the closed `tondo-test-plan-draft` shape and is converted to the
internal value model before validation. The sidecar is optional, and CLI flags
overlay its selection/policy without rewriting it.

## Determinism and boundary

Discovery is deterministic and local: no network, registry lookup, clock,
environment or process is consulted. A malformed TOML file, invalid package
alias, missing dependency lockfile, source collision or missing root source is
a usage error before lexing. After discovery, the compiler sees only the
materialized manifest/lockfile bytes and declared source inputs.
