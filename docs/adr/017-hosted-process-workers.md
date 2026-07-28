# ADR-017: Isolate blocking process work from the cooperative executor

**Status:** accepted

## Context

Child waits and pipe reads can block for an unbounded time. Performing either
inside the single-thread Tondo executor would suspend unrelated runnable tasks.
Buffering a complete pipeline between stages would avoid one blocking edge but
would remove kernel backpressure and make memory use proportional to producer
output.

## Decision

`std.process` connects adjacent stages with direct operating-system pipes and
drains final stdout plus every stderr concurrently. Waiting and stream I/O run
on host workers, not on the Tondo executor.

An async bodyless host callable starts one typed run-local call and returns its
identity to the VM. The scheduler polls host completions whenever it rotates
runnable tasks. It may block waiting for one host completion only when no Tondo
task is runnable. `spawn` wraps the same host call in an ordinary
scope-owned `Join`; direct `await` parks its current task.

Cancellation sets a host-owned request flag. The worker terminates and reaps
all stages before publishing completion. A live `ProcessHandle` also retains a
defensive host cleanup record for panic, VM failure, and host destruction.

## Consequences

The language keeps one cooperative scheduler and does not expose threads,
futures, or a `Task` return wrapper. Host workers are an implementation detail
of blocking I/O. Pipelines retain bounded kernel backpressure, and unrelated
Tondo tasks continue to make progress. The VM host contract must support
start, poll, idle wait, cancellation, and defensive terminal cleanup in
addition to immediate calls.
