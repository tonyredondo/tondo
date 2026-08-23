# `std.time` monotonic time contract

**Status:** implementation evidence is covered in the hosted VM and the
distribution/conformance identity is promoted for the unpublished Tondo 0.1
draft; the S1A seal remains open. La ABI ejecutable publica `sleep` y `Timer.wait` como
`selectable`: una llamada directa sigue esperando implícitamente y ambas
operaciones pueden registrarse como brazos de `select`.

This contract describes the first `std.time` slice. It contains only a
monotonic time-base: civil dates, wall-clock time, time zones, and calendar
conversion are separate work in STD-0.1B.

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
output. The target capability test proves that importing `std.time` without
`clock` is rejected with `E1008`.

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
`process_host` boundary; `STD-A-FUZZ-001` promotes the owner-aware route while
provider-scoped performance remains explicitly pending rather than inferred from unit-test timing.
The contract and its negative fixtures are checked by
`scripts/stdlib-time-check.sh` and `scripts/stdlib-time-test.sh`.
