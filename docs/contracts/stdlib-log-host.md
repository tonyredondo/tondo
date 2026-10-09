# Capability-gated hosted logging sinks

`STD-LOG-HOST-001` is in progress. This boundary implements ordinary Tondo
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

Current-source quality and exact pushed-SHA CI are still pending. No timing,
native execution, full owner promotion or globally green portable CI is claimed.
