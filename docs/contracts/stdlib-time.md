# `std.time` monotonic time contract

**Status:** the hosted monotonic implementation and bounded provider corpus are
covered for the unpublished Tondo 0.1 draft. The full distribution identity,
owner conformance promotion and S1A seal remain open. `sleep` and `Timer.wait`
are `selectable`: ordinary calls wait implicitly and both operations can
register as `select` arms.

This contract describes the monotonic `std.time` slice (STD-0.1A). Civil dates,
wall-clock time, time zones, and calendar conversion are defined separately in
the locked STD-0.1B contract
[`stdlib-civil-time.md`](./stdlib-civil-time.md); they never add a second
`Duration`, `Instant`, timer, or deadline API.

## Values and representation

`Duration` is an immutable signed nanosecond count represented by the language
`Int`. Its operations are pure, allocation-free, and checked. Unit conversion
and arithmetic return a typed `DurationError` on overflow; no operation wraps,
saturates, truncates, or panics.

`Instant` is an opaque host token. It carries the identity of the provider and
its clock domain inside the host registry, never exposes an epoch or numeric
layout, and is `Copy`, `Discard`, `Send`, and `Share`. Operations between
different domains return a typed `ClockError` rather than comparing unrelated
values.

`Timer` is an affine opaque token. It is `Send` only and must be consumed by
`wait` or `cancel`; it is not copyable, shareable, equatable, or a map key.
Terminal cleanup also removes an abandoned timer from the host registry.

## Public surface

Importing `std.time` and naming its value types requires no clock capability.
All twelve `Duration` constructors, conversions, arithmetic and predicates are
pure and usable without `clock`; constructors support references and pure calls
may be deferred. Civil values and pure calendar conversions follow the rule in the
separate civil contract; this does not establish their implementation.

The operations below and all five `Instant` methods require `clock`, because
they read or use the monotonic provider and its opaque registry. So do
`testing.withVirtualTime`, `VirtualTime.settle` and `VirtualTime.advance`:
selecting a virtual provider does not grant a capability. The compiler checks
the resolved host operation before execution, including references, aliases,
deferred calls and intermediate function bodies. Missing `clock` produces
`E1008`. Names alone never confer or require a capability.

The civil provider separately requires `civil-clock`; an anchor sampling both
providers requires `civil-clock` and `clock`. Neither is needed to import the
module. This rule does not enable an ambient provider or add a second time API.

~~~tondo
pub enum DurationError { Overflow }
pub enum ClockError {
    Unavailable
    DomainMismatch
    InvalidDelay
    OutOfRange
    ResourceLimit
}

pub fn now(): Instant ! ClockError
pub fn resolution(): Duration ! ClockError
pub fn deadline(after: Duration): Instant ! ClockError
pub fn sleep(delay: Duration): Unit ! ClockError selectable

pub fn Timer.after(delay: Duration): Timer ! ClockError
pub fn Timer.at(deadline: Instant): Timer ! ClockError
pub fn Timer.wait(self): Unit ! ClockError selectable
pub fn Timer.cancel(self): Unit
~~~

`deadline` accepts signed durations so an already-expired deadline can be
represented. `sleep` and timer creation reject negative delays with
`ClockError.InvalidDelay`; zero remains a real suspension point. A timer
is one-shot and has no reset or repeat operation.

La migración `selectable` conserva estas mismas operaciones y resultados. En un
brazo perdedor, `sleep` desregistra su evento y `Timer.wait` conserva el timer
afín para esa rama; no aparecen `sleepAsync`, `afterCase` ni un selector de
librería.

## Provider boundary

The compiler lowers the calls above to typed host operations. The VM sees only
the operation identity and verified values; user bytecode cannot select or
inspect the provider. The hosted implementation uses one provider boundary:

- the real provider is based on `std::time::Instant`, never the wall clock;
- `now` is non-blocking and non-decreasing within one domain;
- the current hosted resolution reports one nanosecond;
- `sleep` and `Timer.wait` register one-shot suspendible jobs and are polled by the
  existing cooperative executor; and
- cancellation is idempotent and cleanup is completed before a cancelled
  operation leaves the VM.

Active timers and pending time jobs share a bounded host resource pool. The
current hosted default is 1,048,576 resources. Exceeding it returns
`ClockError.ResourceLimit` atomically, without leaving a partial timer or job.
Cancellation, completion, and terminal cleanup release the reservation.

## Virtual provider hook

The host has a sealed internal virtual provider used by future
`std.testing.withVirtualTime`. It starts at zero, has a positive configured
resolution, and advances only when the test domain explicitly advances it.
Production source cannot construct or select this provider and it grants no
additional capability. Real and virtual values use the same bytecode and host
operation identities; only the provider selected at the sealed testing boundary
differs.

## Validation

The direct host corpus runs the same checked arithmetic, non-decreasing instant,
zero-delay suspension, deadline, one-shot timer and cancellation assertions
against both the real provider and a virtual provider (with exact virtual
resolution). Dedicated cases cover foreign clock domains, equal-deadline timer
ties, negative delays, virtual deadline boundaries and atomic resource-limit
release. The runtime fixture `tests/runtime/m10-std-time-001.to` exercises
resolution, `now`, a deadline, zero-duration suspension, and a one-shot timer
end to end through parser, type-checker, bytecode verifier, VM, and console
output. Target capability tests execute all twelve pure operations and aliased
constructors without `clock`, including a handled deferred conversion. They
accept all thirteen monotonic provider operations only with `clock`, reject
their use without it with `E1008`, and cover function references, intermediate
bodies, virtual-provider selection without importing `std.time`, and explicit
type-argument refusals.

The final conformance gate still requires the reproducible source-set,
interface, privileged-unit and virtual-provider hashes described in the
standard-library specification; these tests do not weaken that distribution
requirement.

The executable owner contract is
[`testing/stdlib-time.json`](../../testing/stdlib-time.json), and its nine-cell
record is in [`testing/stdlib-owner-evidence.json`](../../testing/stdlib-owner-evidence.json)
under `STD-A-TIME-EVIDENCE-001`. The six requirements separate the portable
model from the real and virtual providers, checked limits and errors, timer
lifecycle, and capability/conformance identity. The owner corpus is split into
arithmetic boundaries, provider equivalence and timer lifecycle. `HOST` is
verified because both providers are implemented at the single hosted
`process_host` boundary. The `STD-A-FUZZ-001` time route currently checks a fixed
source without executing a provider; owner fuzz coverage remains partial while
provider-scoped performance remains explicitly pending rather than inferred from unit-test timing.
The contract and its negative fixtures are checked by
`scripts/stdlib-time-check.sh` and `scripts/stdlib-time-test.sh`.

## Per-operation capability implementation proof

The separate [pure civil core](stdlib-civil-time-core.md) extends the same
operation policy to the 24 specified Date, Time and UtcDateTime operations.
Its own register and gates retain the full civil owner as pending. The twelve
pure operations in this A0 register describe the Duration base; the additional
pure owner is linked through `capabilities.pure_extensions`.

`STD-TIME-CAP-001` implements the operation boundary in the compiler; it does
not implement civil calendar values, zones, a civil provider or native time
lowering. Source `66bb7fad` adds the boundary and six associated function
references. Metadata checkpoint `7c22532f` synchronizes the moved signature
IDs in both conformance coordination and documentation. Seventy-three focused
time tests, the UUID capability regression, the ordinary pure project without
`clock`, and the six-path public networking guide pass. Workspace formatting,
checking and Clippy also pass.

A consolidated campaign uses 1,399 source inputs with quality tree
`d7f424e6b4a74fbf0bb88917ed36579bf351a842af840070ec2e192323b556d1`.
It executes all 2,804 Rust tests in 76 suites and records 196 layer observations.
Coverage measures 302,358/330,070 lines (91.604205%), 19,891/22,643 functions
(87.846133%) and 444,463/493,751 regions (90.017640%). Every global and risk
scope's 80% line/function/region floor passes. All six selected critical mutants
are caught; none are missed, timed out or unviable, and the unmutated baseline
passes. Before/after bindings match and joint quality/ratchet verification
passes. The initial capture correctly refused stale generated signature IDs;
its partial profiles remain separate from this successful fresh capture.

Clean checkpoint `33d9223b` passes all 356 named functional steps, 2,804 Rust
tests in 76 suites and all 206 draft conformance cases. The selection corpus
retains its three selected cases with 32 exact observations each. Its
source publication `1509fb77` passes exact-checkout CI run `37744715050`,
strict job `113203358076`. The actual checkout executes all 356 named steps
and 2,804 Rust tests in 76 suites without error annotations. All paginated
checks/statuses/runs/PRs and the remote main ref remain unchanged after a
109-second quiet interval. Portable and deterministic fuzz jobs are expected
normal-push skips, not newly executed portable evidence.
These Rust percentages do not claim Python, shell or Tondo source coverage,
nor do they promote the full civil, distribution or S1A boundaries.
