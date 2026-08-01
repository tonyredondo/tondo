# Deterministic virtual time

`tondo-compiler::test_virtual_time` implements the time domain used by one
Tondo test attempt.  A domain is created inside `withVirtualTime`, is borrowed
for the callback, and is dropped before the envelope can be closed.  The
production clock API is unchanged: the worker selects a monotonic or virtual
provider, while this domain supplies the deterministic provider for the
attempt.

The domain starts at instant `0`.  Durations are signed at the host boundary
so negative values are rejected with `P2005`; the stored instant is an unsigned
64-bit value and addition is checked.  `advance` and `advanceTo` wake every
deadline at or before the target, order equal deadlines by timer creation
sequence, and finish at the exact target.  They never round up or overshoot.

Tasks can be ready, blocked, or completed.  Only timer, join, and local-sync
waits are eligible for automatic advancement.  External waits are surfaced as
`P2003` and continue to be governed by the real worker timeout.  If all tasks
are locally blocked and there is no future wake-up, the domain reports the
same `P2003` deadlock class.  The bounded automatic-step counter detects a
livelock/reschedule loop without sleeping the host thread.

`settle` processes due timers and returns the deterministic ready queue.  It
does not run user code; the worker must explicitly transition each awakened
task to another wait or `completed`.  This makes task order observable and
prevents a hidden callback from introducing wall-clock or host-scheduler
dependence.  A domain owns its timers, task order, and counters, so retry and
repeat workers cannot inherit virtual state from another attempt.

The envelope maps domain failures to the normative control errors:

| Condition | Code |
| --- | --- |
| Deadlock, external wait, or livelock | `P2003` |
| Overlap of two domains in one envelope | `P2004` |
| Negative duration, overflow, or clock regression | `P2005` |

