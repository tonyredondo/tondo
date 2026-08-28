# Native AOT memory and ARC contract

`NATIVE-AOT-MEM-001` captures memory behavior from the same complete linked
AOT product recipe used by `NATIVE-AOT-BINARY-001`. It does not turn runtime
internals into Tondo semantics and it does not select a backend. The
machine-readable authority is [`testing/native-aot-memory.json`](../../testing/native-aot-memory.json);
the static and mutation gates are [`scripts/native-aot-memory-check.sh`](../../scripts/native-aot-memory-check.sh)
and [`scripts/native-aot-memory-test.sh`](../../scripts/native-aot-memory-test.sh).

## What is measured

Cranelift and LLVM each receive the same normalized MIR AOT corpus, target,
release profile, runtime harness and hosted stdlib identity. A linked product
then runs all admitted cases before every observation and executes a bounded
memory workload covering:

- allocation count, allocated bytes, live bytes and peak live bytes;
- local and shared/atomic retain and release operations;
- detached cycle reclamation at explicit quiescence;
- weak creation/upgrade, including a dead-target upgrade that cannot resurrect;
- pause time around cycle collection; and
- a real `pthread` worker as the concurrency-pressure observation.

The product also records the process `ru_maxrss` value as a physical RSS
observation. Logical runtime counters and RSS are separate dimensions; neither
is a language guarantee or a substitute for the VM's collector policy.

## Protocol

Each candidate product is executed in three independent fresh processes. Each
process performs three warmups and then nine measured iterations, yielding 27
samples per candidate. Every iteration resets the process-local runtime before
running the complete corpus and memory workload. Samples carry only logical
process/repetition ordinals; no PID, timestamp or physical path is emitted.
Summaries are median, p95 and p99, calculated over all 27 samples. A missing
sample, non-positive required counter, live-byte residue or non-monotonic
summary fails closed; live bytes must return to zero after each workload.

## Semantic boundary and instrumentation

The VM is the oracle for values, errors, ownership, cancellation, traps and
exit status. Native counters are harness-only observations. The instrumented
driver compares every admitted scalar/managed result and trap before it
publishes counters, so a faster or smaller run with a semantic mismatch is
invalid. The counter hooks are process-local and do not expose addresses,
layouts or an FFI promise. The current identity is
`tondo-runtime-draft/1`; production runtime ABI promotion remains a later
decision.

The workload deliberately exercises both ARC representations and the cycle
and weak-reference boundaries closed by `ARC-001`/`ARC-002`. It does not claim
that the temporary harness is the final allocator or scheduler. Any missing
physical capability is reported as a failed dimension rather than silently
treated as zero.

## Scope

This block closes native memory evidence only. It does not run the complete
quality/conformance corpus or publish a performance winner. The quality and
performance reports are now consumed by the independently hash-bound Gate N1
promotion record; this block cannot promote a target by itself.
