# Sealed test execution envelope

**Status:** implemented for `UTEST-CONTROL-001`

`tondo_compiler::test_control` models the private runtime boundary for one
test or suite attempt. It is an `Arc<Mutex<...>>` link owned by the runner,
not a Tondo value, parameter, capability or thread-local. Cloning the link is
how helpers and structured tasks inherit the current envelope; the node ID,
sinks and policies remain inaccessible to source code.

## Atomic evidence and limits

Logs preserve local observation order. Tags are merged in one linearization:
equal repeats are idempotent, conflicting values leave the old map untouched
and report the smallest conflicting UTF-8 key as `P2002`. New bytes are charged
before publication, so a failed budget check never leaves a partial merge.
Stdout and stderr are separate buffers and share the output budget.

Attachments and snapshots use separate per-attempt registries. Attachment names
and media types use the closed grammar and duplicate names produce `P2006`.
Snapshots record `matched`, `missing` or `mismatched` hashes; each name can be
checked once and missing/different values produce `P2007`, while duplicate or
invalid names produce `P2008`. The envelope never exposes a physical path or
store handle.

## Terminals and lifecycle

`fail_now` records `P0007`; `skip` records a cooperative skip except during
cleanup, where `P2001` is returned. A cleanup failure replaces a prior skip.
Structured child handles share the sinks but only their hidden ordinal is used
to choose the first child skip deterministically. Once closed, no operation can
append evidence.

The phase is monotonic (`Setup`, `Body`, `Cleanup`, `Closed`). Virtual time is
one borrowed domain per envelope. Its controller exposes only `settle` and
non-negative `advance`; it is revoked on closure and its observations remain in
the attempt report. A second active domain is rejected with `P2004`.

`admit_operation` rejects test intrinsics in production with `E2003`, providing
the final runtime-facing check in addition to the static checker and lowering
verifiers.
