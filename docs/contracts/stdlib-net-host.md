# Public hosted networking boundary

`STD-NET-HOST-001` integrates the locked networking surface through compiler
checking, MIR, verified bytecode and the hosted VM. Promotion remains pending
until the current source passes the joint selection prerequisite, the complete
repository gate, coverage in every locked scope and publication CI.

The registry contains 40 operations and 15 network value types: three address
records and twelve private host carriers. `NetError`, `TlsError`,
`TlsVerification` and `Shutdown` retain their nominal enum schemas. Address
records have content equality and keys. Transport owners are affine and `Send`;
they are neither `Copy` nor `Share`. Option and configuration snapshots are
immutable. Terminal calls, splits and TLS handshake consume their explicit
by-value owners. `TcpListener.localAddress(self)` exposes an ephemeral bind's
assigned address through the same observational contract as the other sockets.

`network` activation requires the ordered explicit resolver endpoints from
`[target.network]`. Compiler requests, project identities, artifact inputs,
isolated test workers and source replacements preserve that configuration.
Installation and import create no reactor, DNS job, socket or certificate I/O.
The provider does not inherit resolver, proxy, locale or root-store settings.

The private Tokio reactor is polled by Tondo's hosted executor. It does not
replace the language scheduler or introduce a hidden blocking fallback.
DNS and TLS keep the pinned implementation contract in
[`stdlib-net-implementation.md`](stdlib-net-implementation.md). Controlled
loopback peers exercise public DNS, TLS 1.2/1.3, exact certificate pinning,
TCP/UDP, partial I/O and shutdown. The public `spawn thread` route is the
cooperative hosted reference. A separate `BlockingPool` regression exercises
an actual OS worker for synchronous calls; suspendible callbacks remain
forbidden at that boundary.

Validation precedes provider effects. An already invalid, cancelled or expired
request admits its fixed error envelope without allocating a success-sized
resolver array, read buffer or TLS workspace. TLS consumes its TCP owner even
when a deadline precludes handshake, and retires that owner before returning.
The VM and host preserve the receiving test account through response admission.
Logical storage charges include input copies, pending jobs, typed responses,
TLS buffers and shared half-stream ownership; they do not measure RSS or OS
allocator calls. Scope cleanup drops pending futures and transport owners
before the scope leaves. Panic and cancellation use the same terminal path.

Accept, TCP read and datagram receive use prepared readiness. Their losing
reservations preserve the original socket and pending data. Source wrappers
and mixed channel/network selections use the shared protocol in
[`async-select-transactions.md`](async-select-transactions.md); that block is
a prerequisite rather than evidence inherited from older bounded campaigns.

The source fixtures and tests are registered in
[`testing/stdlib-net-host.json`](../../testing/stdlib-net-host.json).
The earlier selection source proof is retained in
[the selection contract](async-select-transactions.md). The renewed joint
HOST/TEST measurements and source-bound ratchet are recorded in
[the networking test contract](stdlib-net-test.md#retained-local-quality).
`STD-NET-TEST-001`, performance, portable conformance, native runtime ABI,
Cranelift AOT and public API promotion remain separate owner boundaries.
