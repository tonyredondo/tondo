# Native AOT quality contract

`NATIVE-AOT-QUALITY-001` is the last correctness gate before the repeated
performance campaign. It does not choose Cranelift or LLVM and it does not
promote Gate N1 by itself. Its purpose is narrower and stricter: every
observable in the admitted native AOT product must agree with the same MIR,
the VM oracle and the already-closed runtime/diagnostic contracts.

The machine-readable authority is
[`testing/native-aot-quality.json`](../../testing/native-aot-quality.json).
The static and mutation checks are
[`scripts/native-aot-quality-check.sh`](../../scripts/native-aot-quality-check.sh)
and [`scripts/native-aot-quality-test.sh`](../../scripts/native-aot-quality-test.sh).
The executable campaign is
[`scripts/native-aot-quality.sh`](../../scripts/native-aot-quality.sh), and its
path-free summary is written to
`target/reliability/evidence/native-aot-quality.json`.

## One input, two candidates, one oracle

The campaign consumes the exact linked-product recipe from
`NATIVE-AOT-LOWER-001`, `NATIVE-AOT-BINARY-001` and `NATIVE-AOT-MEM-001`:

* the same hash-pinned normalized MIR and target descriptor;
* the same release profile, runtime ABI, hosted `STD-0.1A` identity and link
  flags; and
* one independent Cranelift product and one independent LLVM product.

The VM and the normalized MIR interpreter are the semantic oracles. Native
counters, RSS and sanitizer diagnostics are implementation observations; they
never replace a value, error, trap, ordering, ownership or cancellation
comparison. A mismatch, missing observation, unexpected `unsupported` result or
an incomplete product fails closed.

The executable runner already checks the complete admitted AOT inventory:
27 lowering cases, 118 scalar calls, three managed-result calls, 21 runtime
state cases, 14 `std.core` cases, eight selection cases, five physical worker
cases, one deferred-task coordinator, eight diagnostic profiles and the
instrumented 27-sample memory lane for each candidate. This quality gate
revalidates those report fields together, so a later lane cannot silently
replace an earlier oracle with a counter-only result.

## Quality corpus

The campaign has four complementary parts:

1. **Executable AOT differential.** `scripts/native-evaluation-runner.sh`
   builds and executes both linked products in fresh processes and validates
   every admitted case against the VM and normalized-MIR results. The quality
   checker requires both candidates to be `passed`, zero divergences and zero
   unsupported admitted functions. Unsupported functions outside the admitted
   inventory must remain explicit traps with a reason.
2. **Conformance and generated differential.**
   `scripts/native-conf-test.sh` covers the language, testing and stdlib
   owner leaves; `scripts/native-diff-test.sh` checks deterministic generated
   observations, cross-backend equality, stable IDs and the fail-closed oracle
   mutation. These are coordinated evidence, not a second frontend or a
   backend-specific semantic path.
3. **Properties and fuzzing.** The five owner-aware stdlib targets and the
   diagnostics target run with fixed seeds, bounded inputs, timeouts and RSS
   limits. Existing minimized regressions are replayed; a crash, timeout,
   nondeterministic output or missing corpus entry is fatal. The campaign uses
   the smoke budget for routine reproducibility and leaves the longer nightly
   budget to the dedicated fuzz workflow.
4. **Sanitizer and workspace quality.** The native products are linked once
   with the checked-in ASan/UBSan compiler wrapper and run through the same
   case driver. The normal workspace quality gate is then checked for a stable
   coverage/mutation baseline. Sanitizer failures, a changed baseline or a
   surviving critical-oracle mutation fail the campaign.

Each part writes a bounded log below `target/reliability/evidence/`; the final
summary contains logical names, hashes, counts and statuses only. Physical
paths, addresses, process IDs, payloads and ambient environment values are
forbidden in the report.

## Isolation and determinism

Every native case and every fuzz/diagnostic input uses a fresh process or a
fresh isolated runner state. The campaign never shares a heap, ARC table,
cycle collector, scheduler, task scope, retry state, fuzz artifact or sanitizer
state between cases. The report binds the source revision, contract hash,
runner-report hash, target and logical toolchain identities. The normal quality
baseline is read before and after the run and must be byte-for-byte unchanged.

The smoke fuzz protocol is intentionally bounded: 128 runs per owner-aware
target and 128 diagnostic runs, fixed seeds from the existing contracts,
64 KiB maximum input, ten-second per-input timeout and 4 GiB RSS limit. The
campaign does not promote fuzz output or mutate the reviewed corpus; minimized
regressions are retained by the existing fuzz contracts.

The sanitizer wrapper uses `/usr/bin/cc` with `-fsanitize=address,undefined`
and `-fno-omit-frame-pointer`. The wrapper is an explicit absolute executable,
so no compiler or linker is discovered through `PATH`. ASan/UBSan is an
additional memory-safety signal, not a replacement for the ARC/cycle counters
or the hosted leak detector.

## Fail-closed mutation checks

`native-aot-quality-test.sh` mutates each critical report boundary and proves
that the checker rejects it: candidate status, VM observable, cross-backend
equality, panic/trap, cancellation, cleanup, ARC/cycle recovery, sanitizer
status, corpus completeness, report redaction and the next-block frontier.
The test also rejects a non-zero unsupported count and a changed quality
baseline. The executable workspace lane adds a deterministic six-mutant sample:
one function-value mutation at each of the six reviewed frontiers
(`PrivilegedUnit::validate`, `ProjectPlan::parse`, `validate_line_endings`,
`normalize_array_index`, `Heap::ensure_capacity` and `Heap::has_capacity`).
All six must be caught with no timeout, survivor or unviable result. The full
30-mutant corpus remains an explicit performance-lane workload; it is not
silently represented by this bounded quality gate.

## Promotion boundary

The summary is promotable only when all parts are `passed`, both candidates
have the same admitted case IDs, the VM and native observations match, fuzz and
sanitizer lanes are clean, the normal baseline is unchanged and the report has
no divergence, unsupported or physical data. This closes quality evidence for
the current target. It does not claim a final backend, public ABI or measured
performance winner. `NATIVE-AOT-PERF-001` remains responsible for the repeated
compile/link/startup/throughput/latency/size/memory measurements and is the
last input to `DEC-013`.
