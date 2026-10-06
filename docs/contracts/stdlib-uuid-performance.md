# `std.uuid` hosted scalar performance

`STD-UUID-PERF-001` measures the existing private Rust `BootstrapHost` UUID
bridge with explicit reply admission. The selected route is `hosted-scalar`,
backend `rust-hosted-bridge`, target `x86_64-unknown-linux-gnu`, Rust `test`
profile. The default profile is unoptimized; both Rust flag sources are
recorded. Public compiler/VM registration is inherited from the verified HOST
leaf. These calls execute no bytecode and do not measure the complete VM,
native runtime ABI, native AOT, SIMD, multiversion dispatch or code size.
`native_live_handles` is unmeasured and remains `null`.

The register advances from `measurement-ready` to
`verified-hosted-scalar-baseline` after committed clean-source capture, focused
checks and the complete functional/source-bound quality gates. Probe bytes
from `1838ba5` and corrected conformance pins from `971d431` retain 594 actual
measurements. Quality verifies 295,200 of 322,169 workspace lines (91.6289%),
every global/risk dimension at or above 80% and all six critical mutants
caught. All 2,647 Rust tests and the complete functional gate pass on that
unchanged source. The initially stale standard/suite pins were corrected with
the supported generator and exact dependent-pin update, with all assertions
preserved. Publication and exact-SHA CI closure remain tracker steps.
Development captures cannot close the leaf. Shared conformance and usage
documentation remain separate owner leaves. No speedup is claimed.

## Protocol and expectations

Three independent processes each execute three warmup batches and nine
measured batches per workload. Sixteen complete operations form each batch.
`Instant` provides monotonic timing. All 27 measurements retain process
ordinals 1–3 and repetition ordinals 0–8, including outliers. Median, P95 and
P99 use nearest rank; with 27 samples P99 is the maximum. These bounded
measurements do not guarantee production tail latency.

Fixture construction and independent oracle evaluation precede timing.
Fixtures are freshly rebuilt for every batch, including finite providers;
their retained storage and selected ownership identities remain represented
in the resource model. Host calls, black boxes, exact reply comparisons,
result destruction, output-buffer release and charge checks stay timed.
Remaining-provider checks and final fixture cleanup occur after timing.
Oracle temporaries are not included in latency or retained resources.

Successful fixtures use an 8,192-byte host target limit and a 16,384-byte shared
reply/buffer budget. The selected refusals reduce the name target to 63 bytes,
the v4 provider target to 15 bytes, or the reply budget to 181 bytes. Sealed
envelopes have 16,384-unit work, memory and metadata limits. These are bounded
probe settings rather than production defaults or a complete VM-heap limit.

The 22 cases are the smallest selected set covering these distinct routes:

| Family | Cases | Purpose |
| --- | ---: | --- |
| Conversion | 4 | Dashed parse, mixed-case URN, canonical text, network byte round trip |
| Generation | 8 | Sealed/OS v4 and v7; v5 empty, small, padding boundary and 4,096-byte name |
| Bounded refusal | 10 | Oversized text, invalid character, name limit, entropy failure/shape, clock failure/negative/overflow, provider bytes and reply admission |

Deterministic results and nominal errors use the std-only bounded integer,
name-digest and finite-provider model from `STD-UUID-TEST-001`. Error checks
compare the complete kind and optional byte offset. The oversized text fixture
is an authored constant-time length refusal, rather than a claim that the
reference model validates arbitrary large documents.

The large v5 name is outside the model's 96-byte domain. Its expected UUID
`788dc126-8af8-5603-9087-9ddc041c9750` was separately computed with Python
stdlib `hashlib.sha1` over DNS namespace network bytes followed by
`bytes(index % 251 for index in range(4096))`, then the prescribed v5/variant
bits. Report tests independently reproduce this authored vector. A reference
domain boundary never implies production rejection.

OS v4/v7 results are intentionally nondeterministic. Only their version and
variant laws are checked; no exact byte oracle, statistical uniqueness,
entropy-quality test or monotonic v7 guarantee is claimed. A provider failure
fails capture rather than being hidden in a successful timing sample.

Finite snapshots validate exact provider consumption. Reply and byte-budget
refusals consume none; clock failures/range errors consume the clock but no
entropy. Untouched rows retain their exact values. Exhaustion publishes the
nominal unavailable error without falling back to the OS. Every envelope is
closed, and the private host registry and buffer-charge table finish empty.

## Metrics and resource boundaries

Latency is observed batch latency. Throughput reports complete operations per
second. Byte throughput uses attempted input for parse/v5, and selected output
for formatting, byte transport and successful v4/v7. Provider refusals with
no attempted payload report zero byte throughput. Rejecting input throughput
is descriptive and does not count successful emitted bytes.

`input_bytes`/`output_bytes` are semantic payload widths per operation, including
the sixteen-byte namespace for v5. Sixteen calls constitute one batch; the
byte-roundtrip case contains both `fromBytes` and `toBytes` in each operation.
`bytes_copied` counts selected payload transport per batch: 36 emitted format
bytes, 32 byte-roundtrip bytes, or 16/10 copied sealed entropy bytes per
operation. Internal fixed-width copies, fixture setup, hash-state transport
and OS writes are excluded; an OS write is not represented as a copy.

`allocations` counts selected owned fixture/result identities: argument/result
containers, nominal names, strings, boxed wrappers, the input/output byte
buffers, an installed envelope, provider row containers and nonempty entropy
snapshots. Empty nominal vectors count no backing identity. This does not
count allocator calls, capacity growth, unpublished admission previews or
all host implementation allocations.

`logical_memory_bytes` is modeled logical memory, not RSS or an observed
allocator peak. It includes retained arguments and exact expected replies,
selected host input buffers, the provider's explicit logical fixture storage,
and one admitted 182-byte reply envelope. Byte round trips additionally
overlap their first 132-byte reply with a 48-byte output buffer while admitting
the second response. The 181-byte refused reply charges zero admitted bytes.
Detached values use the existing 32-byte logical descriptor convention.
Stack frames/hash state, capacity slack, map/Arc control blocks, oracle
temporaries, previews/staging, other workloads and OS/native storage are
explicitly excluded. This model is not an OOM guarantee.

`reply_retained_bytes` observes the maximum simultaneous response charges
after host return, including both roundtrip replies but excluding registry
buffers. `reply_admission_bytes` is the actual tested fixed reservation per
call, or zero for its refusal. Neither is a sampled allocator peak.
`host_handles_created` observes the registry ID increment during timing;
input setup is excluded. `terminal_live_handles` follows actual registry and
buffer-table emptiness after private Rust cleanup. `terminal_budget_bytes`
reads the shared host/VM admission budget after cleanup; this does not measure
the separately owned envelope or a complete VM heap.

`entropy_requests` and `clock_requests` count logical bridge provider requests
per batch, verified against sealed row consumption. OS counts describe the
bridge's explicit provider invocations, not OS syscalls or retry counts inside
the dependency. Sealed versus OS timings include their respective full bridge
paths; their difference is not an isolated provider-cost subtraction.
`sha1_blocks` models compression blocks per successful v5 operation from the
namespace/name/padding length: the selected fixtures exercise 1, 2 and 65
blocks. It is not instrumented dependency state. Refusals record zero blocks.
`adversarial_rejections` counts all sixteen expected refusals in a batch;
no partial UUID/text/bytes output is published.

## Provenance and execution

Each workload identity binds suite/id, exact probe SHA-256, source tree,
target, backend, profile, toolchain, both Rust flag sources, incremental setting
and Git revision. CPU/OS descriptors stay outside identity. Paths, PIDs,
timestamps and ambient environment state never enter it. Unsupported compiler,
profile and target overrides are refused. Sources and revision are checked
before and after capture; a clean capture verifies that the committed revision
contains the exact probe. A dirty override is development-only.

Run `scripts/stdlib-uuid-performance-check.sh`,
`scripts/stdlib-uuid-performance-test.sh` and
`scripts/stdlib-uuid-performance.sh`. The selected target directory retains
`reliability/evidence/stdlib-uuid-performance.json`. The campaign runs the
independent model suite and private fixture/lifecycle tests before timing.
Positive, negative, retained-outlier, determinism, source/identity and owner
transition tests precede the consolidated quality and functional gates.
