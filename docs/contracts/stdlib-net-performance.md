# Networking performance contract

`STD-NET-PERF-001` records a target-qualified hosted baseline. The register is
[stdlib-net-performance.json](../../testing/stdlib-net-performance.json).
The protocol is measurement-ready; development samples do not promote it.

The selected route is `hosted-scalar`, target
`x86_64-unknown-linux-gnu`, backend `rust-hosted-bridge`, Cargo `test` profile
with the repository's pinned Rust 1.93.0 settings. The private probe calls
`BootstrapHost` synchronous admission and owner-polled network jobs directly.
It measures no bytecode interpreter, native runtime ABI or Cranelift AOT route.
Private TLS dependency acceleration is not a Tondo SIMD promotion. No code-size,
multiversion dispatch or portable performance claim follows from this campaign.

## Workload table

Each sample executes one complete logical workload. Host calls, polls and
partial replies are retained separately and may vary with legal scheduling.

| Route | Exact payload | Purpose |
| --- | --- | --- |
| tcp-connect-accept | 0 | Listen, ephemeral address, connect, accept, address laws and close |
| tcp-read-small | 64 bytes | Complete small read and final EOF |
| tcp-read-chunked | 65,536 bytes, requests up to 4,096 | Multiple completions and exact byte conservation |
| tcp-read-rollback | 64 bytes | Prepared losing readiness preserves the whole subsequent payload |
| tcp-write-small | 64 bytes | Timed argument/provider copies, count and shutdown |
| tcp-write-large | 65,536 bytes | Partial writes, checked ordered delivery and shutdown |
| tcp-write-backpressure | 8,388,608 bytes | Withheld consumer, partial progress, 16 pending producer polls after partial progress and exact drain |
| udp-send-empty | 0 bytes | Empty datagram is a successful whole message |
| udp-send-small | 64 bytes | Exact whole send and actual bound-source equality |
| udp-receive-empty | 0 bytes | Empty message differs from stream EOF |
| udp-receive-small | 64 bytes | Whole receive and explicit datagram accessors |
| udp-reject-oversized | Reject 65, then retain exact 64 bytes | No partial published handle; next datagram preserved |
| dns-ordered | Three authored addresses | A before AAAA, duplicate removal and first-seen order |
| dns-delay | Same addresses; 5 ms fixture delay per request | Complete bridge latency under a controlled delayed provider |
| dns-result-limit | No result; exact ResourceLimit | Whole-result refusal without partial publication |
| tls12-echo | 32 sent, 32 received | TLS 1.2 handshake, exact echo, close_notify and half shutdown |
| tls13-echo | 32 sent, 32 received | Same laws under TLS 1.3 |
| tls-reject-name | No application payload | Exact CertificateRejected and consumed TCP teardown |
| pending-accept-cancel | No connection | Pending registration retired before Cancelled reply |
| reject-expired-deadline | No DNS traffic | Exact Timeout before reactor, resolver or socket effects |
| reject-host-memory | No DNS traffic | VM memory admission refusal before job publication |

The 21 routes distinguish materially different public provider paths
and outcomes. They do not multiply equivalent small fixtures or TLS cipher
combinations. IPv6 returned DNS addresses are authored outputs; this is not an
IPv6 transport benchmark. Existing IPv6/kernel tests remain separate evidence.

## Oracle and timing

Validate independent finite admission, DNS and TLS verdict models before
starting the timer. Small TCP rollback also uses the independent stream ledger.
The model's 128-byte domain is not a production input limit.
Larger payloads use an authored `index % 251` byte vector and independently
checked peer chunks; `OutsideDomain` is never converted into a target error.

Use monotonic `Instant` timing. Exclude fixture/oracle construction, peer
spawn, preloaded inputs and setup connection from latency except where the
connection workload explicitly measures them. Include admitted calls,
argument copies, polling, exact return validation, result disposal, explicit
transport close and shutdown. After timing, every sample must join its peer,
collect private host roots, observe empty registry/charge tables/jobs/slots
and zero shared budget, then drop the private reactor. No provider error may
be replaced by a successful sample or discarded outlier.

Use three warmups and nine retained repetitions for each of three independent
processes: 27 samples per route, no filtering. Median/P95/P99 use nearest rank;
with 27 samples P99 equals the maximum. Throughput is complete logical
workloads or successful application payload bytes per median elapsed second.
It excludes TLS record bytes, DNS packet bytes and rejected UDP bytes.

## Resource observations

Resource counters cover setup, timed work and final cleanup together. Excluding
setup latency does not erase setup allocations or live charge observations.
`host_handles_created` is the actual registry ID increment for that whole
sample. `polls`, pending replies, partial replies, rollbacks, cancellations
and exact expected refusals describe actual bridge actions. Two explicit DNS
requests are checked against the fixture peer; no system resolver is used.

`selected_allocations` models selected result boxes/containers/names, provider
future/job identities and owned payload buffers. Separate fixture identities
include explicitly retained authored vectors and certificate DER. These are
neither OS allocator calls nor exhaustive Rust allocations. Map nodes, Arc
control blocks, provider/dependency internals, capacity slack, oracle
temporaries, stack buffers and the private reactor are outside this model.

`bytes_copied` counts selected timed delivered read bytes, UDP accessor copies
and explicit argument/provider input copies; fixture construction is excluded.
A write copies the submitted remainder, including bytes not committed by a
partial result, rather than only useful payload bytes. The metric excludes TLS/DNS encoding and kernel-internal
copies. It is a declared payload-transfer model, not an allocator profiler.

`sampled_budget_peak_bytes` observes the actual shared logical admission
account before/after selected bridge actions. It does not observe every
transient overlap or represent RSS. Retained fixture bytes plus that observed
maximum form a selected logical-storage estimate, not a full resident-memory
or allocator peak. The memory-refusal fixture deliberately holds an admission
charge; this reserves logical budget without allocating that many OS bytes.
`reply_retained_peak_bytes` observes actual typed reply charges. Native live
handles remain unmeasured `null`; measured terminal host fields must be zero.

## Capture and acceptance

Stable report identity binds suite/workload, probe hash, exact source tree,
clean Git revision, target, backend, profile, Rust/Cargo versions and flags.
Paths, PIDs, timestamps, ephemeral ports, ambient resolver settings and CPU
frequency never enter identity. Host descriptions are contextual metadata.
Actual source/peer-address equality is still checked against the bound sockets.
Source, flags, toolchain and HEAD must be identical before/after capture.
Development reports remain explicitly unpromoted. Promoted reports require
the exact committed probe and clean source.

Acceptance requires every complete route, positive/negative lifecycle and
report/provenance tests, preserved raw samples, supported generated evidence,
the repository functional gate, one consolidated current-source quality
campaign with every 80% floor and the unchanged six-mutant selection, signed
normal main publication and independently confirmed exact-SHA CI. Performance
closure does not establish conformance, usage documentation or native networking.

Run `scripts/stdlib-net-performance-check.sh` for the pinned contract,
`scripts/stdlib-net-performance-test.sh` for schemas, lifecycle and phase
transitions, and `scripts/stdlib-net-performance.sh` for complete capture.
These scripts require the caller's delegated process scope for executable
process checks. Normal capture requires a clean committed probe;
`TONDO_STDLIB_NET_PERF_ALLOW_DIRTY=1` only marks local reports as development.

## Retained local evidence

Clean source `62baf842e5b95b2ee95df4ac6f70eb0234410309` supplies all 567
measurements, probe SHA-256
`24cd509e391ab68aa8ce4941454fdf8953421883b8f54ca60c37ea3dbac64ae4`
and quality input tree
`06bee215915baf8caead882b81d449c5046625a9dad4b5e91ca30a8097a5c408`.
The complete retained report has SHA-256
`046f18734a7674067ca249ef55f2fb761006f2da14510cfc1a413858a1f3d9cb`.
These selected observations describe this controlled host and unoptimized
profile; they guarantee neither production latency nor a performance speedup.

| Route | Median ms | P95 ms | P99 ms |
| --- | ---: | ---: | ---: |
| tcp-connect-accept | 0.198950 | 0.216310 | 0.377700 |
| tcp-read-chunked | 0.588690 | 0.607700 | 0.612581 |
| tcp-write-backpressure | 56.850729 | 57.488329 | 57.541659 |
| dns-ordered | 0.542690 | 0.593800 | 0.594051 |
| dns-delay | 10.575755 | 10.731835 | 11.225106 |
| tls12-echo | 43.039372 | 43.073231 | 43.080991 |
| tls13-echo | 43.036672 | 43.061792 | 43.116891 |
| pending-accept-cancel | 0.042450 | 0.053150 | 0.116530 |

Backpressure retains seven median partial replies and 57,917,990 selected
copy bytes for 8,388,608 useful bytes. Submitted remainders explain the extra
copies; this is baseline evidence, not a corrected or optimized write route.
Every sample ends with zero host handles, jobs, pending slots and budget bytes.

Twenty Python report tests and eighteen focused Rust tests pass. The complete
instrumented workspace executes 2,767 tests in 74 suites and attests 196 layers.
Current-source quality measures 302,082/330,048 lines (91.526687%), preserves
all global/risk line, function and region floors and catches the same six
critical mutants, with no misses, timeouts or unviable mutations. The 25-minute
capture budget ended after joint quality verification and before ratchet
generation completed. A separate bounded metadata-only run generated and
verified the ratchet from those unchanged reports; no coverage or mutation
capture was repeated for that step. All 351 named functional-gate checks pass
in one uninterrupted clean-source invocation on `e11af1b5`, including 206
draft cases and 96 exact async/select observations. The existing scalar native
route passes 630 Cranelift cases, 70 arithmetic traps and 75 negative evidence
records; these establish no native networking. Publication and exact-SHA CI
closure remain pending acceptance steps.
