# Isolated retry rounds

`tondo-compiler::test_retry` owns the retry policy and pure unit planner.
`tondo-cli::test_campaign` connects that planner to actual process workers for
`--retry N`. `N` is the number of additional rounds,
defaults to zero, and is bounded by `MAX_RETRY_ROUNDS`.  The campaign always
executes the complete selected plan once before it evaluates retry causes.

Only `failed-error`, `failed-panic`, and `timeout` are eligible.  Skips,
resource limits, infrastructure failures and compile failures never generate a
retry. A blocked leaf can participate through its eligible causal suite, but
never generates an independent retry unit. Suite units contain
the original selected subtree and absorb descendant causes under the outer
eligible suite.  Units are sorted by the first leaf in the original
`execution_plan`; a retry never crosses a shard.

Each public retry unit invokes an independent process with the same compiled
bytecode and captured inputs. Runtime selection admits only that unit's leaves
and ancestors; it does not repeat parsing or skip full-target validation. The
runner creates a new worker, heap, roots, executor, task set, handles,
envelope, output buffers and resource registry.  The compiled program and
immutable snapshot input may be reused, but no attempt evidence, virtual
domain, timer, counter, or budget is inherited.  All attempts remain in the
report with their round, unit, cause, status, and worker identity.

If a later attempt passes after an eligible failure, the leaf is marked
`flaky-pass`.  That remains a failing exit by default; `allow-flaky` changes
only the exit policy and never erases the earlier failure.  Retry is rejected
with repeat or snapshot update so those operations cannot share mutable state.
