# `std.net` public hosted VM and native kernel process conformance

`STD-NET-CONF-001` executes normal public Tondo networking programs in the hosted
VM and compares four finite groups with a fresh native Rust kernel process.
The register is [stdlib-net-conformance.json](../../testing/stdlib-net-conformance.json).
Its state is `adapter-ready`: implementation and focused execution do not close
consolidated quality, the full functional gate or exact-SHA publication CI.

The target is `x86_64-unknown-linux-gnu`, Rust 1.93.0, Cargo `dev` profile.
The VM uses the production compiler, public registration and admitted hosted
provider. No bodyless callback substitutes a Rust kernel call for Tondo code.
`BuildTarget::with_network_target` currently accepts only `tondo-vm-hosted`.
The native process executes existing Rust scalar value/admission kernels and
finite models; it opens no socket and supplies no native Tondo network target.
Native runtime network ABI, native provider admission and Cranelift networking
AOT are `not-implemented` or `not-claimed`, never inferred from this comparison.
Portable execution remains unmeasured until it actually runs on another target.

## Exact common observations

Both adapters compile the same fixture module. The independent finite model
checks hostname, limits, stream, datagram and resolver laws before VM execution.
IP spelling/equality and key laws use authored expectations and the production
kernel; the bounded model is not an independent IP parser. Python independently
checks every ordered observation, so equal but altered adapter outputs fail.

| Group | Observations | Executable boundary |
| --- | ---: | --- |
| `values-and-keys` | 28 | Eight hostnames, five limit constructors, eight numeric IP inputs, four ports and three copy/key laws |
| `tcp-transcript` | 7 | Three one-byte reads, final EOF and three one-byte writes |
| `udp-transcript` | 4 | Normal and empty datagrams, oversized refusal, then a valid datagram |
| `dns-transcript` | 2 | Atomic result-limit refusal and first-seen IPv4/IPv6 ordering |

These are 41 common observations. `NetLimits` and address fields are opaque in
the public API; constructor acceptance is compared, while exact internal fields
remain kernel evidence. Public Tondo maps actually retrieve copied `HostName`,
`IpAddress` and `SocketAddress` keys. Generic signatures require `Copy`,
`Discard`, `Equatable`, `Key`, `Send` and `Share`; no cross-thread layout or
transport is inferred. Key retrieval does not publish map iteration order or
the Rust map's randomized seed.

The VM uses bounded real loopback peers. The TCP peer writes `a` and `bc`, shuts
down writing, receives exactly `xyz` and observes EOF. Public code splits the
stream, retains final data before EOF, flushes and shuts down its writer, and
closes both halves. UDP checks each source address against the actual peer and
never exposes partial oversized data; the next valid message remains readable.
DNS receives exactly two explicit questions, A followed by AAAA, with retained
duplicate answers. The complete array refuses at limit two and returns the
three ordered unique addresses at limit three. No system resolver or external
service supplies these results.

The complete forty-case retained kernel corpus is a capture prerequisite.
Malformed provider reports, impossible partial UDP sends and internal deadline
domains cannot be manufactured through the public API. They remain kernel-only
controls, not omitted regressions, mocked VM successes or published provider
behavior. The reference remains bounded to 128 bytes and 32 addresses. It has
no independent OS, DNS packet or TLS record parser.

## VM-only integration and capabilities

Fifteen additional observations qualify actual public hosted integration:

- TLS 1.2 and TLS 1.3 each write `ping`, read `po` and `ng`, then observe EOF
  after `close_notify`. The controlled server checks the negotiated version.
  Each version also rejects `wrong.test` with exactly `CertificateRejected`,
  sends no application data and releases its consumed TCP transport.
- A listener binds port zero, reports its assigned address, accepts a real
  connection and checks both endpoint address identities before shutdown and
  explicit close. A source `selectable` wrapper selects between two UDP
  receivers; whichever loses retains its exact datagram for a subsequent read.
- A public monotonic deadline checks `InvalidPort` before timeout and then
  produces `Timeout` for a valid port. The explicitly configured local DNS
  socket observes no query. The fixture explicitly grants `clock` and `network`.

The TLS oracle is an explicit verdict and affine owner model, combined with a
real existing local certificate fixture. It does not prove a second TLS stack.
These fifteen records are VM-only; the native Rust reference cannot claim them.
Inherited HOST regressions additionally prove cancellation, budget admission,
mixed channel/network selection and production host retirement.

Six static records check missing and granted `network` for imports, aliases and
`defer`. Refusals require the actual `E1008` capability diagnostic, without a
syntax-error substitute. Accepted forms compile against the same public API.
Static checking itself performs no network I/O.

Every executable VM fixture receives a fresh test envelope. Exact detached logs,
successful status, absence of a terminal testing failure and the actual closed
phase are checked. Every peer has a five-second wait bound and is joined. The
native runtime table contains zero objects after each common group; kernel/model
vectors own no objects in that table. Zero does not measure their allocation,
VM heap storage, ARC, cycle collection, full retained memory or RSS.

## Source-bound capture and promotion

Each adapter starts in a fresh process. Capture retains actual stdout/stderr
and checks source, fixture, probe, contract, Git revision/tree and repository
quality provenance before and after all commands. Ephemeral ports are inserted
into real compilation requests and therefore their build identities, but stay
outside report identity only after exact address laws have passed. The report
binds the source templates; it does not claim identical compiled artifact hashes
across ephemeral requests. No PID, artifact path or timestamp enters identity.

The complete capture has a monotonic eight-minute budget, with at most three
minutes per child command and an explicit delegated process scope. Unrecorded
compiler wrappers, flags, linker, target and profile overrides refuse before
execution. Partial or failed captures retain logs and never replace a complete
report. Development capture requires an explicit dirty-tree override and cannot
promote the owner. Reuse after later Markdown/testing-register changes requires
unchanged executable/build inputs and declared contract bytes.

Capture executes focused stdlib kernels, independent models, all retained corpus
rows, hosted model tests, both adapter tests and compiler networking regressions.
Promotion additionally requires one consolidated quality campaign preserving
every 80% global/risk line/function/region floor and catching all six critical
mutants, the complete functional gate, clean capture and exact-SHA publication
CI. `verified-public-hosted-vm-and-native-kernel-process` records only that
qualified boundary. SIMD, dispatch variants, code size, portable networking and
native Tondo ABI/AOT are not promoted. The next owner is `STD-NET-DOC-001`.

## Local source-bound proof

Clean source `85b57677` captures all 41 common, 15 VM-only and six static
observations with complete prerequisite execution. After synchronizing the
toolchain contract and its audit expectation, source `6e9bef79` has quality tree
`495a66b86a6d3b2e47fb35c07d0a25498577f7784826cfe75bdb21c0c76fa7fa`, with 1,393
inputs. One complete fresh-counter workspace campaign runs 2,799 Rust tests in
76 suites using `--test-threads=4`. It measures 302,112/330,178 lines (91.499737%),
19,878/22,667 functions (87.695769%) and 444,184/494,148 regions (89.888859%).
Every global/risk floor passes and all six critical mutants are caught, with
zero missed, timed-out or unviable mutants and a successful unmutated baseline.
Before/after source bindings match; the supported ratchet verifies the joint
evidence. The LLVM report contains no example/support fixture paths: adapter
execution is established separately, without an example line-coverage claim.

The first parallel instrumented attempt stopped in the existing CLI interrupt
test with exit 3 instead of 4. The unchanged instrumented binary then passes
that exact test three times and all 162 CLI tests with four harness threads.
The successful campaign preserves every case, assertion and real grace period;
only outer test scheduling is qualified. The cause of the first failure remains
unconfirmed and no production fix is claimed. Failed profiles/logs and focused
diagnostics are retained separately from the successful fresh counters.

The toolchain prose correction changes the pinned draft manifest and therefore
invalidates the earlier quality identity. The interrupted capture and the
stale audit-list refusal are retained as failed or partial evidence. The narrow
audit fix passes all 101 reliability library tests and explicitly preserves
the network requirement's `toolchain-limit` trace status. The corrected-source
coverage completes before the combined phase reaches its time cap during the
mutation baseline build. A subsequent bounded mutation-only phase catches all
six mutants on the same source; joint binding and ratchet verification pass.
No coverage counters are repeated or rebound for that narrower phase.

All 354 named functional-gate steps pass in one clean-source invocation on
`59351222`, including 2,799 Rust tests in 76 suites, 206 draft cases and 96
exact async/select observations. Publication and exact-SHA CI remain pending.
This local proof does not change the register's `adapter-ready` promotion state.
