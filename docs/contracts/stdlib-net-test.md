# Networking model, regression and bounded fuzz contract

`STD-NET-TEST-001` covers an independent finite reference, retained exact
kernel regressions and controlled public hosted execution. The register is
[stdlib-net-test.json](../../testing/stdlib-net-test.json). The implementation
is ready for joint current-source quality and publication validation; this
document does not close the tracker owner or promote native networking.

## Independent domain

The model imports only Rust's standard collections. It has no production
kernel, provider, compiler, VM, clock, socket, DNS or TLS parser dependency.
Its admission scanner retains ASCII hostname spelling, treats punycode labels
as opaque and refuses implicit IDNA. Address identities are supplied IPv4/IPv6
bytes and ports; IPv6 text parsing is outside this model. Numeric limit and
deadline checks accept signed `i128` inputs and preserve validation, cancellation
and timeout priority. Clock domain and ticks are explicit inputs.

Reference payloads and buffers are at most 128 bytes; resolver transcripts are
at most 32 addresses. Those bounds constrain the oracle, not Tondo's production
limits. `OutsideDomain` means that the model cannot decide the observation;
it must never be compared with a production rejection. DNS deduplicates supplied
address identities in first-seen order, preserving IPv4/mapped-IPv6 distinction,
and refuses the whole result when its caller limit is exceeded. It does not
predict OS resolver behavior, caching or packet arrival order.

## Stream and TLS ownership

The stream ledger has two affine half owners and one abstract transport.
Peer bytes and write credit are explicit. TCP fragments have no message
boundaries: reads deliver admitted prefixes, final bytes precede EOF and writes
may report a partial prefix. Zero peer credit remains pending. The byte law is
`fed = observed + discarded + queued`; repeated readiness is nondestructive.
Only winning commit consumes bytes. Rollback, cancellation and timeout retire
the registration while retaining unread bytes. Scope exit retires both halves,
all queued storage and every prepared registration. Shutdown keeps the writer
owner until close; the last half releases the transport.

TLS models admission and ownership around an explicit peer verdict. It neither
validates certificates nor parses records. A by-value TCP transfer is consumed
even when TLS admission fails. No TLS owner may be published or split before a
verified verdict; certificate rejection, handshake failure, cancellation and
timeout release the consumed owner. The fixed reference clock domain is 7.
Both half-close orders and duplicate/terminal model refusals are covered.

## Executable comparisons

Four kernel differential tests exercise ASCII mutations, all numeric limit and
clock boundaries, every small TCP/UDP completion report and 4,096 deterministic
ordered DNS transcripts. Forty authored corpus rows retain exact success values
and nominal errors across hostname, limits, TCP, UDP, deadline and DNS admission.
The corpus's own expected values are checked against both reference and kernel.

Four public hosted tests compile and run real Tondo programs with declared
`network` capability and explicit resolver endpoints. Controlled loopback peers
exercise four TCP fragment layouts, final data then EOF, split read/write and
shutdown, empty/full/oversized UDP datagrams, ordered A/AAAA DNS and atomic
result-limit refusal. TLS tests exercise real TLS 1.2/1.3 exchanges, close_notify,
wrong-name certificate rejection and peer-observed terminal transport EOF.
The certificate and test key are the repository's existing local fixtures;
no external DNS service or certificate service is contacted.

The existing production provider and public host suites additionally prove
pending-read cancellation, socket consumer exclusion, provider job budgets,
real TLS handshake cancellation, wrong/expired certificate refusal, forced
selector ownership transfer, mixed-arm rollback and host heap/scope retirement.
Their private counters describe logical resources, not RSS or OS allocation
calls. The test script runs these suites together with the independent model.
Tests requiring processes run inside an explicit delegated scope.

Fourteen independent unit tests include 4,096 generated stream schedules replayed
twice, plus replay limits, invalid actions, failure priority and terminal owners.
Nine integration tests comprise four kernel, one retained corpus and four public
hosted tests. These counts name actual Rust tests, not seeds or network packets.

## Fuzz protocol and verification

`stdlib_net` includes the same std-only model source directly, so its minimal
Cargo graph needs only the scalar stdlib and libFuzzer dependencies. It replays
at most 4,096 input bytes and 512 steps, checks determinism, byte conservation
and zero terminal owners/registrations, and compares seven bounded kernel routes.
TLS fuzz verdicts remain model inputs; real TLS belongs to the hosted tests.
Both the minimal graph and the normal compiler-enabled fuzz graph must build.

The bounded smoke uses `nightly-2026-07-28`, 128 runs, seed 4,113, maximum input
4,096 bytes, per-input timeout 10 seconds and RSS limit 4,096 MiB. The smoke
retains active sanitizers; a sandbox's LeakSanitizer refusal is an environment
failure, never a passing result. Corpus output is task-owned temporary storage;
diagnostic crash artifacts are preserved separately.

Run `scripts/stdlib-net-test-check.sh` for metadata and source boundaries,
`scripts/stdlib-net-test-test.sh` for invalid-state/schema/progression tests and
focused execution, and `scripts/stdlib-net-fuzz.sh` for the real bounded smoke.
The full gate includes all three. Closure additionally requires current-source
coverage at every 80% global/risk floor, the unchanged mutation selection,
generated inventories/matrix/ratchet, inspected signed publication and exact-SHA
CI. Hosted execution does not establish native ABI, native AOT, performance or
the separate portable conformance and usage owners.
