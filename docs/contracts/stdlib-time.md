# `std.time` monotonic time contract

**Status:** implementation evidence is covered in the hosted VM; the
distribution/conformance identity remains pending for the unpublished Tondo
0.1 draft

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
pub fn sleep(delay: Duration): Unit ! ClockError

pub fn Timer.after(delay: Duration): Timer ! ClockError
pub fn Timer.at(deadline: Instant): Timer ! ClockError
pub fn Timer.wait(self): Unit ! ClockError
pub fn Timer.cancel(self): Unit
~~~

`deadline` accepts signed durations so an already-expired deadline can be
represented. `sleep` and timer creation reject negative delays with
`ClockError.InvalidDelay`; zero remains a real suspension point. A timer
is one-shot and has no reset or repeat operation.

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
