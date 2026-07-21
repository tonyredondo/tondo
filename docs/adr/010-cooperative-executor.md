# ADR-010: Start with a cooperative single-thread executor

**Status:** accepted

## Context

Tondo specifies structured tasks and progress, not one operating-system thread
per task.

## Decision

The first async runtime schedules tasks cooperatively on one thread.

## Consequences

Async lowering, cancellation, joins, and roots can be validated before parallel
scheduling. `Send` and `Share` remain statically enforced even on this target.
