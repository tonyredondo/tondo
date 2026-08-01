# Deterministic test scheduling

**Status:** implemented for `UTEST-SCHED-001`

`tondo_compiler::test_schedule` validates a selected suite tree and exposes two
stable order modes. `id-byte-order-v1` is the default and sorts every set of
siblings by UTF-8 bytes. `sha256-tree-v1` uses a normalized sixteen-digit seed
and compares the complete digest:

```text
SHA-256("tondo-test-order-v1\\0" || seed_u64_be || 00 ||
        UTF8(parent_id) || 00 || UTF8(child_id))
```

The public `execution_plan` contains leaves only. `dispatch_plan` keeps suite
enter/children/exit events contiguous, so setup and teardown remain
structurally atomic while a separate `JobLimiter` enforces the same explicit
maximum across setup, body and teardown admission. Results can therefore be
presented canonically without depending on completion timing.

`VirtualQueue` models only the deterministic test-domain queue: ready work is
ordered by creation/wake sequence and timers are ordered by deadline followed
by creation sequence. It rejects clock regressions and empty identities. It is
not the production scheduler and never virtualizes real I/O or runner
timeouts.
