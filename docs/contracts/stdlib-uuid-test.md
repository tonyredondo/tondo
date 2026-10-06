# std.uuid independent model and testing boundary

`STD-UUID-TEST-001` owns the independent reference, retained vectors, generated
comparisons and bounded fuzz in [`testing/stdlib-uuid-test.json`](../../testing/stdlib-uuid-test.json).
It extends the verified [Rust kernel](stdlib-uuid.md) and
[hosted provider registration](stdlib-uuid-host.md). Public compiler/production
VM registration already exists through HOST; TEST does not promote native ABI,
native AOT, SIMD, performance, common conformance or usage documentation.

## Independent finite reference

`crates/tondo-reliability/src/uuid_model.rs` imports only `std`. Its value is a
single unsigned `u128`, with its own lexical parser, integer-based text
formatting, network-byte conversion, variant tree, version extraction and
unsigned comparison. Nil and Max preserve all bits. Parsing accepts 36-byte
dashed text and the case-insensitive nine-byte URN prefix, validates length
before prefix/body and reports the first absolute UTF-8 byte offset. External
version and variant bits are preserved rather than regenerated.

The v4/v7 reference applies integer masks to supplied entropy and a checked
48-bit millisecond prefix. It checks timestamp range before entropy length.
The v5 reference implements its own fixed-buffer, one/two-block name digest
over sixteen namespace bytes and at most 96 opaque name bytes. This uses the
rounds and padding of [FIPS 180-4](https://nvlpubs.nist.gov/nistpubs/FIPS/NIST.FIPS.180-4.pdf)
to check [RFC 9562 UUIDv5](https://www.rfc-editor.org/rfc/rfc9562.html#section-5.5).
It is a bounded test oracle, not a new production hash API or dependency. Names
longer than 96 return `OutsideDomain`, even when a supplied target name limit
would reject them. That reference result never establishes a kernel rejection.
Within the domain, explicit target limits produce `NameLimitExceeded`.
Production's 16 MiB name limit remains unchanged. Names are bytes, with no
Unicode normalization or string terminator.

`ReferenceProviders` models a sequential finite transcript: at most sixteen
clock rows and sixteen entropy rows, with at most sixteen bytes per entropy
row. It records consumed rows, target byte-limit refusal before consumption,
clock failure/range before entropy, exhausted streams and terminal close. It
has no OS fallback. These are millisecond-level provider laws, not a model of
sub-millisecond clock conversion, OS entropy quality, VM admission/accounting,
shared-clone synchronization or host allocator behavior. Those concerns retain
the separate executable HOST tests and contract.

## Retained and generated proof

The authored corpus contains seventeen valid and thirty-seven invalid vectors,
each with a unique stable identity. Valid vectors include the exact RFC 9562
Appendix A.3/A.4/A.6 v4/v5/v7 examples, Nil/Max, network order, URN/hex case,
external variants, opaque/empty/non-normalized v5 names and v7's epoch/maximum.
Invalid vectors cover lexical forms and precedence, absolute offsets including
UTF-8 and NUL, byte/provider lengths, name bounds and timestamp precedence.
This small retained set records exact regressions; exhaustive position/bit
comparisons and generated cases cover the larger finite domains separately.

Eleven integration tests compare 4,096 deterministic seeds over all pure
operations and generation. Every ASCII mutation at every canonical/URN byte
position is compared, alongside UTF-8 mutations, all version/variant values,
every unsigned network-byte position and v5 lengths 0 through 96 including
padding boundaries. Tests enumerate all sixty-four ignored-bit combinations
for v4/v7, prove that flipping any retained entropy bit changes its result,
and preserve repeated values, equal timestamps and clock regressions. This
models collisions without a statistical uniqueness or monotonicity claim.
Copy, key and immutable concurrent observation tests preserve value semantics.

A finite provider transcript is independently evaluated, then replayed through
the actual compiler and production VM using the sealed Rust test envelope.
It checks nominal error display, stream consumption, failure ordering,
exhaustion and valid results. It never reads the OS as a fallback. The eighteen
kernel and twenty-three focused compiler/VM HOST tests remain additional proof.

## Fuzz protocol and promotion

`scripts/stdlib-uuid-fuzz.sh` runs `stdlib_uuid` with `nightly-2026-07-28`,
128 runs, seed 4113, at most 4,096 input bytes and 512 reference replay steps,
ten seconds per input and 4,096 MiB RSS. Each run starts from all fifty-four
retained vectors plus the checked-in seed in a fresh directory. Generated
discoveries remain separate; they do not silently extend the next smoke's
starting corpus. Sanitizers remain enabled.

The envelope starts with one selector byte modulo five: parse UTF-8, raw bytes,
v4 entropy, v5 or v7. v5 adds a two-byte unsigned name limit and sixteen network
namespace bytes before the name; v7 adds signed big-endian `i128` milliseconds
before entropy. Incomplete structural fields have explicit zero defaults.
Arbitrary text uses lossy UTF-8 conversion before both parsers. Arbitrary input
and deterministic generated seeds compare exact values/errors; v5 names
outside the reference domain check production limit refusal or repeated success
without fabricating a digest oracle. Replays also exercise bounded provider
transcripts. No partial UUID is exposed on failure.

The fuzz target includes the std-only model directly. The minimal dependency
graph links stdlib without compiler, VM, conformance runner or reliability CLI;
both this graph and the normal full fuzz graph must build. This finite smoke
does not claim exhaustive testing or OS entropy certification.

`ready` requires the focused models, retained corpus and observed fuzz smoke,
with consolidated quality pending. `verified` also requires the full functional
gate, source-bound coverage at or above 80% for every global/risk dimension and
all six selected critical mutants caught. Negative contract tests exercise
both states, parent progression, corpus corruption and utility availability.
The next owner is `STD-UUID-PERF-001`; CONF and DOC follow separately.

The boundary is locally `verified` on the source from `aa0aae4`. Focused tests,
85 invalid testing records and the observed 128-run smoke pass. The consolidated
quality campaign verifies 294,814 of 321,733 lines (91.6331%), all global/risk
80% floors and six selected mutants caught. The full functional gate passes
on that same frozen source. The initial unscoped coverage invocation refused
the required process-test isolation; recovery reset only counters and reused
the current instrumented binaries in the documented delegated scope. No source,
test or threshold changed. Publication/CI closure remains a tracker step.
