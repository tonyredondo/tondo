# Capability-gated hosted logging sinks

`STD-LOG-HOST-001` verifies the production hosted boundary. It implements ordinary Tondo
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

The measured source is `0f9f7250e969be4e2a965d7550b478b5c7e10fb6e8fd3063a2d7600467649e52`:
1,437 inputs with set digest
`6cf078882cbdcee636aefc15946f0f3ade2df87118bf135bd761a9bf3c07f099`.
It includes the promotion-check fixture correction in signed commit `151fbf9b`;
the Rust implementation is unchanged from `96cd3c62`. The original fresh
implementation campaign remains historical evidence. For the corrected source,
all earlier raw counters are cleared. Cargo reuses current instrumented binaries
and rebuilds artifacts required by the selected target/features. No earlier raw
counters contribute to the renewed measurement.

All 2,893 Rust tests in 77 suites pass across source-bound continuations: 53 CLI
unit tests, nine acceptance tests, 162 CLI integration tests, 2,668 remaining
workspace/all-target tests and the one CLI process-argument test. The initial
CLI integration run reports lost isolation in an interruption case. Both a
focused repeat and the complete 162-test repeat at the original four-thread
setting pass without changing assertions or deadlines. That initial failure is
retained as unreproduced and unclassified. Reconciliation detects the missing
process-argument target before attestation; it is executed and included before
the full report and 196 layer observations are generated. Successful log parts
are retained verbatim with their producer hashes; neither failed run is
rewritten into a pass.

| Metric | Observed |
| --- | --- |
| Lines | 307,393 / 335,625 (91.588231%) |
| Functions | 20,179 / 22,954 (87.910604%) |
| Regions | 452,188 / 502,614 (89.967251%) |
| Critical mutation selection | 6 / 6 caught; none missed, timed out or unviable |

Fresh coverage/mutation bindings, every locked 80% global/risk dimension and
supported ratchet generation/verification pass. No threshold or mutation
selection is weakened. The canonical quality script's final check initially
refuses the previous retained ratchet; supported regeneration then verifies the
new measurement, without repeating the successful test executions. Current
reports and raw profiles are archived and hash-verified before supported cleanup
of completed instrumentation caches. Mutation uses a separate fresh scratch
directory; its baseline passes and all six selected mutants are caught.

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
Publication follow-up must also confirm the metadata closure revision.

HOST is promoted only to `verified-production-hosted`; the next owner is
`STD-LOG-TEST-001`. No timing, native logging execution or full owner promotion
is claimed.
