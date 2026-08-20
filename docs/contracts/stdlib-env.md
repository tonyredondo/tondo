# `std.env` runtime snapshot contract

**Status:** specification and hosted implementation closed for the current
Tondo 0.1 draft; distribution conformance remains pending.

`std.env` is the single owner for process arguments and environment inputs. It
is capability-gated by `environment`, reads only at runtime, and never turns a
host value into an implicit compiler input.

## Public surface

~~~tondo
pub enum EnvError {
    Unavailable
    InvalidName
    ResourceLimit
}

pub type Name

pub enum Value {
    Text(String)
    Bytes(bytes.Bytes)
}

pub type Snapshot

pub fn snapshot(): Snapshot ! EnvError
pub fn Name.fromText(value: String): Name ! EnvError
pub fn Name.fromBytes(value: bytes.Bytes): Name ! EnvError
pub fn Snapshot.arguments(ref self): Array[Value]
pub fn Snapshot.get(ref self, name: Name): Option[Value] ! EnvError
pub fn Value.asText(ref self): Option[String]
pub fn Value.asBytes(ref self): bytes.Bytes
~~~

The snapshot is immutable, sealed at the invocation boundary, and contains the
ordered argument vector plus the environment entries supplied by the target
adapter. Missing entries are `none`; they are not empty strings and do not
raise an error. There are no `set`, `remove`, `clear`, or ambient fallback
operations.

`Name.fromText` uses the exact UTF-8 bytes without Unicode normalization.
`Name.fromBytes` preserves non-UTF-8 names. Empty names and names containing
NUL or `=` return `EnvError.InvalidName`. Values with valid UTF-8 are `Value.Text`;
other bytes are `Value.Bytes`. `asText` never repairs or replaces invalid
encoding, while `asBytes` returns an independent logical copy.

The provider validates entry and byte budgets before publishing a snapshot. A
host that cannot provide the sealed snapshot returns `Unavailable`; a budget
failure returns `ResourceLimit` and never exposes a partial result. Repeated
calls in one invocation observe the same snapshot.

The current `BootstrapHost` evidence covers the empty adapter snapshot, injected
public entries, ordered arguments, raw bytes, invalid names, unavailable hosts,
and atomic byte limits. It never reads the ambient process environment.

## Testing and secrets

The testing runner supplies an empty snapshot unless the closed test plan
declares public or secret inputs. Public values participate in the plan hash;
secret values exist only inside a worker and are represented outside it by
descriptor and version. The runner cannot redact a value that the program
explicitly copies into logs, tags, snapshots, artifacts, or output.

Importing `std.env` without `environment` is rejected at the module boundary
with `E1008`. A test target does not gain a missing production capability merely
because the code is test code.

The executable owner contract is
[`testing/stdlib-env.json`](../../testing/stdlib-env.json), and its nine-cell
record is in [`testing/stdlib-owner-evidence.json`](../../testing/stdlib-owner-evidence.json)
under `STD-A-ENV-EVIDENCE-001`. The six requirements separate capability and
availability, sealed snapshot identity, ordered arguments, strict names and
raw/text values, missing-entry and copy semantics, and atomic limits. The
owner corpora cover invalid names, injected inputs and budget failures without
consulting the ambient process environment. `HOST` is verified at the single
`process_host` boundary; `STD-A-FUZZ-001` now covers the owner-aware route;
capability-scoped performance promotion remains explicitly pending. The contract and its negative fixtures
are checked by `scripts/stdlib-env-check.sh` and `scripts/stdlib-env-test.sh`.
