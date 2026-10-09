# Capability-gated hosted logging sinks

`STD-LOG-HOST-001` defines the production hosted boundary. Promotion is reopened
for command stream delivery until renewed quality and publication checks pass.
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
provider test proving its contents. This omission reopens HOST promotion.

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

The last retained measured source is `76dc5dbb39a2485ea1e98162ebeb8bd5ec22d4ab1a450530a398fcacdc2b0b51`:
1,437 inputs with set digest
`6cf078882cbdcee636aefc15946f0f3ade2df87118bf135bd761a9bf3c07f099`.
It includes the promotion-check fixture correction in signed commit `151fbf9b`
and the explicit nightly quality target in `89130e3f`. The Rust implementation
is unchanged from `96cd3c62`. Fresh canonical workspace/all-target coverage
clears previous instrumented workspace binaries and raw counters. No earlier
counters contribute to the current measurement.

All 2,893 Rust tests in 77 suites pass in the fresh coverage execution, including
the CLI process-argument target; the report attests 196 layer observations.
An earlier campaign reports lost isolation in a CLI interruption case. Both a
focused repeat and the complete CLI repeat pass unchanged. That failure remains
unreproduced and unclassified; an omitted process-argument target in that
earlier continuation was restored before attestation. Historical logs and
profiles remain retained, and failed commands are not rewritten into passes.

| Metric | Observed |
| --- | --- |
| Lines | 307,531 / 335,625 (91.629348%) |
| Functions | 20,179 / 22,954 (87.910604%) |
| Regions | 452,454 / 502,614 (90.020175%) |
| Critical mutation selection | 6 / 6 caught; none missed, timed out or unviable |

Fresh coverage/mutation bindings, every locked 80% global/risk dimension and
supported ratchet generation/verification pass. No threshold or mutation
selection is weakened. Mutation resumes the unchanged canonical selection in
a fresh isolated directory after restricted source-copy traversal fails before
its baseline. A continuation helper's relative runner lookup is corrected
before the remaining checks execute. Both failures remain recorded; the
baseline and all six scored mutants then pass their required classification.
The final retained-ratchet check initially refuses the old source identity;
supported regeneration verifies the new measurement without repeating the
successful tests. Current reports and raw profiles are archived and
hash-verified before supported cleanup of completed instrumentation caches.

All 364 local functional checks pass across 238 unchanged prefix checks and
126 canonical continuation checks. The first native scalar check times out;
two complete subsequent runs pass its original two-second bound, including
630 Cranelift cases, 70 arithmetic traps and 75 evidence refusals. The initial
timeout is retained as unreproduced; it is not classified as an implementation
repair or infrastructure fault. No native source or test limit changes.

Exact implementation source `96cd3c62` passes all 364 named functional checks
and 2,893 Rust tests in strict Linux CI run `37897332476`, job `113711568377`,
attempt 1. The actual checkout and logs are verified, with an 86-second final
quiet confirmation and no failure annotations or open main PR. Portable and
fuzz jobs are expected normal-push skips; this is not global portable proof.
Metadata closure `040a93ca` also passes all 364 checks and 2,893 Rust tests in
strict Linux run `37907917225`, job `113745771559`, attempt 1, with actual
checkout/log proof. Its scheduled nightly run `37908629083` passes the complete
test gate and extended fuzz. Coverage and all six critical mutants pass, but
the final quality ratchet fails because that job omits `TONDO_TEST_TARGET`.
The failure is retained and is not called global green CI. Commit `89130e3f`
declares `linux-x86_64` on the existing quality step, matching the full gate and
the retained provenance. Exact publication `97eaa29a` passes strict run
`37918455600` and all three jobs in manual nightly run `37918521272`. Actual
checkout, the complete quality artifact provenance and final quiet confirmation
are verified. This qualifies the unchanged workflow; it does not measure the
later command stream correction.

The command output correction invalidates that quality identity for the new
source. Current promotion awaits renewed quality and exact publication CI.
The next owner after HOST verification is `STD-LOG-TEST-001`. No timing, native
logging execution or full owner promotion is claimed.
