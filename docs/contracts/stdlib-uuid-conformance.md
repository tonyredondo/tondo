# `std.uuid` public hosted VM and native kernel process conformance

`STD-UUID-CONF-001` compares five common case groups using actual public Tondo
UUID programs in the hosted VM and a fresh native Rust-kernel process. The
register is [`testing/stdlib-uuid-conformance.json`](../../testing/stdlib-uuid-conformance.json).
The local state is `adapter-ready`: focused execution exists, while consolidated
quality, the full functional gate, clean-source capture and publication CI remain
pending. `verified-public-hosted-vm-and-native-kernel-process` additionally requires
those local gates; exact-SHA publication closure belongs to the tracker.

The VM compiles normal `std.uuid` calls with the production compiler and executes
them through the production hosted bridge. No bodyless test-only callback
substitutes a Rust kernel invocation for a public Tondo program. Public compiler
registration and provider admission are inherited from
[`STD-UUID-HOST-001`](stdlib-uuid-host.md). This conformance comparison does not
add a native UUID runtime ABI, native Tondo provider adapter or Cranelift UUID
lowering. The native process calls the existing Rust scalar kernel with finite
test provider replay. Native capability checking and native AOT are not claimed.

## Exact common cases and independent expectations

Both adapters compile the same retained corpus bytes and common fixture module.
The std-only independent UUID model checks all seventeen valid and thirty-seven
invalid vectors before public VM execution. The full kernel/model regression
suites remain capture prerequisites, including the 4,096-seed model comparison.
The common executable corpus has seventeen valid and thirty-five invalid vectors.
Two kernel-only controls retain their separate prerequisite proof:

- `v4-length-17` supplies seventeen entropy bytes directly to the kernel. The
  sealed hosted fixture refuses a row longer than sixteen at installation;
  it cannot represent this direct generator invocation.
- `v5-name-limit` sets a three-byte Rust `UuidLimits` name bound. The public
  Tondo API has no per-call limit setter. Its bound belongs to the host target
  and ordinary result/input admission; this fixture cannot assert the same
  public call with a different limit.

Neither omission becomes a public VM success, a silent fallback or an ignored
regression. The existing HOST tests retain target/result admission and refusal
before provider consumption. The source-bound report qualifies each route.

The five groups have seventy-seven exact, ordered observations:

- `retained-values` has seventeen observations, including RFC v4/v5/v7,
  Nil/Max, uppercase and URN text, every authored external variant, opaque v5
  names without normalization, epoch and maximum v7 timestamps. Each output
  includes computed canonical text, all sixteen network bytes, version,
  variant and sentinel predicates.
- `retained-refusals` has thirty-five nominal errors, with exact absolute byte
  offsets where defined. It covers text/URN/UTF-8 precedence, byte lengths,
  representable bad provider lengths and timestamp-before-entropy precedence.
- `core-value-laws` has three observations. Public Tondo code invokes Nil/Max,
  byte round trips, unsigned comparisons, equality and immutable copies. A
  generic signature requires `Copy`, `Discard`, `Equatable`, `Key`, `Send` and
  `Share`; a Tondo `Map` actually retrieves the copied round-trip key. The Rust
  reference uses a `BTreeMap`, avoiding an ambient randomized hash seed. This
  is not a new cross-thread transport or native layout claim.
- `provider-transcript` has sixteen calls and observations. Supplied failures,
  successful recovery, repeated values, malformed rows and exhaustion remain
  ordered and finite. The independent/native replay consumes eight clock and
  nine entropy rows. VM consumption is established by the exact transcript,
  not by an exposed counter. No exhausted fixture falls back to the OS.
- `clock-laws` has six observations: equal timestamps with equal entropy,
  a regression from 100 to 99 milliseconds, the maximum 48-bit timestamp,
  epoch, and four actual unsigned comparisons. It preserves duplicates and
  regressions rather than manufacturing strict monotonicity. These common
  fixtures use integer milliseconds; sub-millisecond pre-epoch conversion
  remains separately exercised by the inherited HOST regression.

The Python comparison also checks authored corpus values/errors and explicit
v4/v7 bit-layout expectations. Equal but altered VM/native outputs fail. This
bounded checker is not a new production parser, hash, entropy or clock provider.

## Static capabilities and lifecycle

The VM emits fifteen additional, independently checked capability records:
nine missing-capability `E1008` refusals and six accepted signatures across
direct calls, function aliases and `defer`. v4 requires `entropy`; v7 requires
both `civil-clock` and `entropy`. Each static check receives a sealed fixture.
A real public generator call then replays that same fixture and verifies its
first value is still present. Installing test inputs grants no capability and
static checking consumes no provider row. These are VM-only records, not
native provider or native Tondo capability evidence.

Every executable fixture has a fresh envelope. It closes after capture;
the actual closed phase and refused replacement are checked while detached
logs remain valid. UUID result/error schema and provider admission retain the
compiler/VM HOST regressions. The native runtime table reports zero objects
after each group. UUID kernels and Rust fixture/output allocations do not own
objects in that table; zero does not measure their allocations, ARC/cycle
collection, peak allocator storage or RSS. VM heap metrics are not measured.
SIMD, multiversion dispatch and code size are not measured or promoted.

## Reproducible report and promotion

The capture starts each adapter in a fresh process and retains its actual
stdout/stderr. It also executes the eighteen kernel tests, eleven independent
model tests, three public VM adapter tests and focused compiler/VM UUID HOST
regressions. The observed target is `x86_64-unknown-linux-gnu`, with pinned Rust
1.93.0 and the unoptimized Cargo `dev` profile. The incremental setting is
recorded. A monotonic eight-minute budget covers the whole capture, with at most
three minutes per child command. Unsupported compiler wrappers, flags, profile or target/linker
overrides refuse before execution. No portable-target result is inferred.

Clean evidence binds the exact source revision, Git tree, committed probe,
fixture, runtime/compiler/kernel/model/build inputs and report protocol hashes
before and after capture. Reuse across later Markdown and testing-register JSON
changes requires unchanged declared sources and contract bytes. Other tracked
or untracked executable/build inputs invalidate reuse. Explicit dirty-tree
development capture cannot promote the owner. Artifact paths, PIDs, addresses
and timestamps stay outside report identity.

The report checks exact order, every observation, nominal errors, all static
capability/replay records, lifecycle types, source/build/prerequisite bindings
and canonical identity. Failed or partial processes retain diagnostic logs and
never replace an existing complete report. Source or contract changes during
capture fail before atomic publication of the local artifact.

The consolidated quality gate must preserve every global/risk line, function
and region floor of 80%, and catch all six selected critical mutants. The
conformance register remains `adapter-ready` until those gates and the full
functional campaign actually pass on the same frozen source. Public hosted
conformance, this native Rust reference and native Tondo execution remain
distinct evidence layers. The next owner leaf is `STD-UUID-DOC-001`.

The local register is now `verified-public-hosted-vm-and-native-kernel-process`.
Clean committed capture, all three Rust adapter tests, seven report-law tests,
174 negative records and all 2,650 workspace Rust tests pass. Source-bound
quality measures 295,206 of 322,169 lines (91.6308%), preserves every global/risk
80% line/function/region floor and catches all six selected critical mutants.
Every full functional gate step passes, including 206 draft cases and three
async/select cases repeated exactly 32 times each. The existing scalar native
route passes 630 Cranelift cases, 70 arithmetic traps and 75 rejected evidence
changes; these do not establish UUID AOT. Publication and exact-SHA CI closure
remain pending in the live tracker.
