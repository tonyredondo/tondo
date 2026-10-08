# Pure civil value core

`STD-CIVIL-TIME-CORE-001` supplies the shared values needed by logging. The
authoritative register is [stdlib-civil-time-core.json](../../testing/stdlib-civil-time-core.json),
with exactly 24 signatures inherited from the locked full
[civil contract](stdlib-civil-time.md): eleven Date, seven Time and six
UtcDateTime operations. No clock capability is required. Import, parsing,
formatting, components and checked arithmetic perform no provider reads.

The selected route is the hosted scalar value kernel and production compiler
bridge. Date, Time and UtcDateTime have private fields and satisfy Copy,
Discard, Send, Share and Equatable. They are ordinary immutable nominal values;
they contain no host handles. Constructors and parsing validate before
publication. The six associated constructors/parsers also work as function
values through aliased imports. Value equality compares their date/time data.

Date uses the proleptic Gregorian calendar with years 1..9999. Parsing accepts
exact ASCII `YYYY-MM-DD`. Time accepts `HH:MM:SS` and an optional one-to-nine
digit fraction, with seconds 0..59 and no 24:00 or leap seconds. Canonical
formatting removes trailing fractional zeros. UtcDateTime requires `T` and
the exact suffix `Z`; offsets, locale, normalization and trailing input are
refused. Invalid outer UTC syntax or a bad date returns InvalidDate; a valid
outer shape with an invalid time returns InvalidTime.

Date.addDays checks the resulting year range. Date.addMonths and Date.addYears
require MonthPolicy.Reject or MonthPolicy.Clamp: a missing destination day is
InvalidDate under Reject, or the month's final day under Clamp. A destination
outside the year range is OutOfRange before either policy publishes a value.
UtcDateTime.add consumes the existing Duration and uses checked wide internal
arithmetic to carry or borrow nanoseconds across days. It introduces no epoch,
second Duration, Instant conversion or clock read.

The bridge reserves a conservative logical construction envelope of eight
detached descriptors plus thirty bytes, then shrinks transport to the actual
retained value. The exact result must also fit the receiving VM before
publication. Refusal drops all temporary reservations. These bounds are not
RSS measurements or OS allocator counts. Parsing allocates no variable input
buffer; canonical strings are at most 10, 18 and 30 bytes respectively.
Throughput, latency, SIMD and code size remain unmeasured.

An independent Python datetime/calendar generator produces 1,272 deterministic
bounded reference cases, including century rules, explicit month/year policies,
Int64 extrema and exact UTC nanosecond carry/borrow. Its checked-in corpus is
verified against the production Rust kernel by the reliability integration test.
A complete 400-year cycle, strict grammar failures and finite endpoints have
additional kernel coverage. Compiler tests verify every public operation,
constructor references, aliases, value capabilities and refusals of generic
arguments, private fields, missing policies, wrong Duration types and pending
zone/clock APIs. Host tests verify typed carrier refusal and complete release
of transport charges. The ordinary no-capability project also exercises an
optional timestamp, structured task copying and handled deferred parsing.

Run `scripts/stdlib-civil-time-core-check.sh` for this boundary. Regenerate the
independent corpus with `python3 -B scripts/civil_core_oracle.py`; `--check`
rejects documentary or generator drift rather than blessing old expectations.

The consolidated local campaign on source checkpoint `818407c4` passes all
2,817 workspace Rust tests in 77 suites. Its immutable source tree is
`92d77723f66c954140f7c91d90be91f438fa20bd9c39567ab34b183f5a7a07c5`
(1,408 inputs). Coverage measures 303,385 of 331,124 lines (91.622776%),
19,963 of 22,718 functions (87.873052%) and 446,139 of 495,532 regions
(90.032329%). The formal gate passes every 80% global/risk floor. All six
selected critical mutants are caught, with no missed, timed-out or unviable
mutants and a passing unmutated baseline. Supported ratchet generation and
verification bind both fresh reports to that same tree. This is local proof;
the complete functional gate and exact-checkout publication CI remain pending.

The implementation is in progress until those remaining checks pass. The full
civil owner remains contract-locked. DateTime, UtcOffset, zones, the versioned timezone
bundle, civil clock and anchors remain pending. This is not native Tondo civil
ABI/AOT promotion, a full civil test/fuzz/conformance promotion or a release.
