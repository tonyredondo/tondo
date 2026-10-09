# Capability-gated hosted logging sinks

`STD-LOG-HOST-001` defines the verified production hosted boundary. Current
local quality passes; publication requires exact-SHA CI follow-up.
It implements ordinary Tondo
`ConsoleSink` and `FileSink` declarations and their `LogSink` implementations,
the scalar formatting bridge and existing hosted I/O providers. It does not
promote native ABI/AOT, the independent model/fuzz boundary, performance, common
conformance, usage documentation or the full owner. The register is
[`testing/stdlib-log-host.json`](../../testing/stdlib-log-host.json).

Importing `std.log` constructs no provider. The compiler installs the console
and filesystem source sets only with their declared target capabilities.
Missing constructors and references produce `E1008`; private delivery helpers
remain inaccessible with `E1004`. Neither constructor chooses an ambient
provider. `ConsoleSink.create` is non-suspendible. `FileSink.create` is
`suspends` and opens its explicit path immediately. Append atomically creates
a missing file or appends to an existing file; Truncate creates or truncates.
Neither mode creates parent directories. The public filesystem Append mode
retains its existing-only contract.

Each builtin sink owns a single guarded writer state through channel endpoints.
Its structural capabilities are affine, `Send` and `Share`; it is neither
`Copy` nor `Discard`. The logger owns and consumes one sink value. Source loans
prevent consuming a logger while a spawned call still borrows it (`E1403`).
Calls that share a logger serialize through its existing owner slot.

`write` validates and formats the entire event, then admits one record into a
queue bounded by `SinkOptions.capacity`. `Accepted` denotes queue admission.
`flush` and consuming `close` deliver accepted records in commit order. On a
full queue, Block drains its oldest record; Reject and Drop preserve accepted
records. Delivery does not group records or create a worker. A short write
advances the retained offset. Cooperative cancellation restores the writer
lease and resumes from that offset. An Io error is visible, may leave a physical
prefix, and makes the sink terminal. Closing drains and flushes accepted data,
then retires the writer even if flushing fails; errors remain visible.

Hosted command results retain stdout and stderr separately. The CLI delivers
runtime stderr before compiler diagnostics, including an unhandled main error.
Wire conformance, reliability observations and runtime fixture sidecars retain
both streams through the public compiler output. A shared public fixture checks
their exact bytes, including serialization of the reliability observation.
VM resource failure retains bytes already committed by the writer and preserves
`T0002`; it does not execute or manufacture user cleanup. The public CLI
regressions and bounded VM exhaustion regression verify these paths. The
previous CLI silently discarded the hosted stderr collector, despite the
provider test proving its contents. The correction is included in current proof.

The formatter uses the existing iterative scalar Text/JsonLines oracle, including
supplied UTC timestamps, byte-ordered JSON fields, explicit redaction, Base64 and
one final LF. Decoding retains the core's fixed depth bound. Logical workspace
is reserved before decoding/formatting; typed Bytes admission precedes a new
host identity. File/path storage and the typed file result are admitted before
the physical open. This is an open-operation guarantee, not transactional
rollback of filesystem effects after later caller failure. Retained payloads
have separate logical charges when the receiving test owner supplies an account.
These charges are not RSS or OS allocator counts. Bytes, paths, writers and
files participate in liveness even during ordinary execution without that
test account; returned values remain rooted.

A caller can implement `LogSink` over an explicit network transport. The public
loopback fixture sends the supplied message in its own declared wire protocol;
it does not claim builtin Text/JsonLines formatting. Networking owns resolver
configuration, sockets, timeouts and TLS. Logging does not resolve, reconnect,
rotate or retry underneath this protocol.

The public console project and file/concurrent/network fixtures exercise actual
compiler/VM calls. A Rust-only sealed standard fixture supplies short, zero,
oversized and failing writes, flush failure, nominal interruptions and actual
`Group.cancel` after a written prefix. It is absent from the shipped source
sets. Public and sealed paths verify whole records, visible errors, consumed
owners and terminal zero handles. The associated checks are
[`stdlib-log-host-check.sh`](../../scripts/stdlib-log-host-check.sh) and
[`stdlib-log-host-test.sh`](../../scripts/stdlib-log-host-test.sh).

## Retained local verification

The current measured source is
`226baed477deb081f04fc6cffd4128cc46366226a3743b982869226ae347724d`,
with 1,441 inputs and set digest
`4243ffecf056d9d254c4748e6130dc0c63b3120a0a9b8bdf4f0efe95cd537fe4`.
Signed source checkpoints `1fd025c0`, `8e59ed8c` and `4e7a5bb0` complete
command stream delivery, refresh the standard package identity and preserve
both streams in every execution observation adapter.

Fresh canonical workspace/all-target coverage clears instrumented workspace
binaries and raw counters before execution. All 2,898 Rust tests in 77 suites
pass without ignored cases, including the CLI process-argument target and the
shared runtime stream fixture. The report attests 196 layer observations.

| Metric | Observed |
| --- | --- |
| Lines | 307,658 / 335,744 (91.634698%) |
| Functions | 20,184 / 22,959 (87.913237%) |
| Regions | 452,624 / 502,775 (90.025160%) |
| Critical mutation selection | 6 / 6 caught; none missed, timed out or unviable |

The unchanged six-frontier mutation selection runs in a fresh isolated
directory with cargo-mutants 27.1.0. Its baseline passes; all six changes are
caught. Coverage and mutation provenance match the current source, flags and
toolchain. Every locked global/risk 80% dimension and the joint quality gate
pass. Reports and raw profiles are archived and hash-verified before supported
cleanup of completed instrumentation caches.

The incomplete command stream campaigns remain retained. The first refuses a
missing explicit process cgroup; the second refuses a stale standard package
hash in the determinism project. Supported generation changes only that hash,
the still-identical permutation artifact hashes and their dependent pins.
Another campaign is deliberately interrupted when inspection finds remaining
observation consumers discarding stderr. All three consumers are corrected
before the complete current campaign; no partial campaign is treated as proof.
The new runtime fixture initially lacks its required exit sidecar; the complete
fixture and conformance checks pass after that setup omission is corrected.

Earlier source verification retains the initial native scalar timeout and CLI
interruption failure as unreproduced after unchanged focused and complete
repeats pass at their original limits. Those logs and profiles remain available.
No test, deadline, coverage floor or mutation selection is weakened.

Exact earlier implementation `96cd3c62` passes all 364 strict Linux checks in
run `37897332476`. Its metadata closure `040a93ca` also passes strict run
`37907917225`, but scheduled nightly `37908629083` fails the final quality
ratchet because that job omits `TONDO_TEST_TARGET`; its full test gate, fuzz,
coverage and six mutants pass. Commit `89130e3f` declares the target explicitly.
Exact publication `97eaa29a` passes strict run `37918455600` and all three
jobs in manual nightly `37918521272`, with actual checkout, complete artifact
provenance and final quiet confirmation verified. This qualifies the unchanged
nightly workflow; it does not measure the later command output correction.
Normal portable/fuzz skips do not establish global portable success.

Exact publication `1891031ab59f303a15a893c2feea0acab1895ef3` passes
[CI run 37937941039](https://github.com/tonyredondo/tondo/actions/runs/37937941039),
strict Linux job `113844629271`/attempt 1, with all 364 named checks and 2,898 Rust
tests in 77 suites. Its log proves the actual checkout. All paginated checks,
statuses, main references and PR observations remain satisfactory after 74
seconds of quiet confirmation, with no failure annotations. Two conformance
manifest pins are synchronized after the earlier stale-plan refusal; no cases,
observations or criteria change. Expected normal portable/fuzz skips are not
portable proof. The next owner is `STD-LOG-TEST-001`, described in
[stdlib-log-test.md](stdlib-log-test.md). No timing, native logging execution
or full owner promotion is claimed.
