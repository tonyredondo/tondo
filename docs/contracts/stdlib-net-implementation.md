# Private networking implementation

`STD-NET-IMPL-001` advanced from `ready-kernel-private-provider` to
`verified-kernel-private-provider` for the private Rust boundary.
Its selected route is `scalar-rust-kernel-and-private-nonblocking-provider`.
The parent contract is [stdlib-net.md](stdlib-net.md); the owner register is
[stdlib-net.json](../../testing/stdlib-net.json).

The Rust kernel supplies checked ASCII host names, numeric address families,
ports, finite limits and explicit monotonic options. It preserves original
host-name bytes, IPv4 versus IPv6 identity and provider result order after
deduplication. Invalid arguments and whole-result limits never publish prefixes.
TCP admission distinguishes the final data chunk from EOF and reports exact
partial-write progress. UDP admission preserves empty datagrams and rejects
oversize/truncation without exposing a valid partial message.

The compiler's private provider supplies controlled DNS, nonblocking TCP/UDP,
and authenticated client TLS using the adopted pinned dependencies. Its
current-thread readiness context returns `Poll` to the owning executor and
does not create a thread, Tondo task scheduler or public polling API. Every
operation owns a bounded slot. It validates the caller's explicit clock domain
and checks cancellation before timeout before polling provider work. Dropping
or cancelling pending work retires its provider state before publishing the
terminal error. A committed reply is preserved after later cancellation.

Selectable TCP reads and UDP receives prepare with a non-consuming peek and
retain their admitted buffer until commit. Listener preparation can move a
connection into one listener-owned slot, which remains available after a
losing preparation; it does not publish a stream before commit. A single
consumer permit per endpoint prevents replacement or concurrent consumption
of prepared state. Dropping a pending wait or prepared lease releases that
permit. These private leases are not yet integrated into the public VM selector.

Each TCP transport owns its Tokio descriptor and a safe duplicated standard
descriptor for read/both shutdown. They name one transport and close with its
last owner. TLS splits share bounded protocol state; each state transition
holds a mutex only around nonblocking work, never across an await. Writes
report exactly the plaintext accepted into the provider buffer; a transport
failure after that commit is retained for flush or the following operation.
Failed or cancelled TLS setup drops its consumed TCP stream.

The kernel ceilings are 64 MiB per read, 65,535 bytes per datagram and 1,024
resolver results. There are at most 256 pending operations and eight DNS
provider jobs per resolution. Target validation bounds the explicit resolver
list independently. These are finite structural limits. Runtime heap admission,
RSS, allocator calls and native code size are not measured or promoted here.
Dependency/global-allocator OOM recovery is not claimed.

The ordered versioned trust-anchor identity observed by the provider is
`sha256:142ba280f8d4a0090f7dec142503ce47aa6960752374f31147cb84e1dc797f14`.
Its focused test checks that pin against the actual compiled root material.
Graceful TLS EOF requires `close_notify`; an abrupt TCP EOF after a valid
plaintext chunk is a nominal transport reset on the next read.

Focused proof covers fourteen kernel tests, twenty private provider/executor
tests and five CLI/toolchain configuration tests. It includes actual controlled
loopback DNS/TCP/UDP, authenticated TLS 1.2/1.3 both in memory and over TCP,
name/expiry/root rejection, partial and empty I/O, prepared losers, EOF,
operation limits and immediate cancellation cleanup. The no-retry regression
outlasts Hickory's UDP retry floor; DNS cancellation immediately rebinds the
retired query source ports. TLS test certificates and keys are public fixtures
with fixed validation time and never enter the production trust bundle.

The implementation checker verifies those source/test anchors and exact
dependency routes. Its negative tests preserve the distinction between ready
and verified states. Fresh workspace coverage observed 296,733 of 323,815
lines (91.6366%), 87.8228% of functions and 90.0416% of regions. All global
and risk metrics passed the 80% floors; all six selected critical mutants
were caught, with no missed, unviable or timed-out cases. The campaign ran
2,689 Rust tests and attested 196 layer observations. After correcting the
two normative expectation tests, it reused unchanged instrumented binaries
with the same target/features and discarded all previous raw counters before
rerunning every workspace target. No historical counters enter this report.
The source tree is
`04d1514617d51586119b155dcd8702d4dd3e1c3b599225a8d9a3bba16ee8f0b7`;
the generated [ratchet](../../testing/conformance-ratchet.json) binds both
reports to that tree and the exact input set. The complete functional gate
and exact-source publication CI remain required before tracker closure.

Public Tondo compiler registration, static capability/affine checking, VM host
admission, production scope cleanup, sealed VM provider controls and target
activation remain `STD-NET-HOST-001`. Independent model/fuzz, performance,
conformance and executable public usage remain their separate owner leaves.
Native ABI/AOT and portability promotion are not established by these Linux
x86_64 Rust tests. The tracker stays open until the required evidence is current.
