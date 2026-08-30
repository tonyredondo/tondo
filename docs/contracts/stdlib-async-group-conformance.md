# std.async.Group VM/native conformance

This contract closes STD-ASYNC-GROUP-CONF-001. It compares the hosted VM
fixture with a fresh-process execution of the native runtime ABI using the
same eight logical cases. The machine-readable authority is
testing/stdlib-async-group-conformance.json.

The hosted oracle is the executable
tests/runtime/m11-std-async-group-001.to, which covers all, settle, next,
selectable commit/rollback, error selection, panic/cancellation cleanup and
affine terminal use. The native probe
crates/tondo-native-runtime/examples/async_group_conformance.rs exercises the
opaque ABI directly. It is independent from the hosted VM implementation,
uses a fresh process for every run, and emits one normalized JSON observation
per case. For all and settle, the probe also reads private scalar-outcome
diagnostics so insertion order, payloads and error tags are observed rather
than inferred from a hard-coded case count.

The native runtime owns one strong edge for each child transferred by
group_add. Terminal notifications queue a stable insertion index exactly
once. next consumes completion order and returns none without consuming the
still-affine group; all selects the lowest insertion-index error after
draining live siblings; settle preserves every declared outcome without
turning cancellation into E; and cancel drains every child before returning.
A panic marker is separate from the declared Result channel and propagates
only after sibling cleanup. Invalid handles, joined children, post-terminal
operations and duplicate cleanup fail closed.

The probe is ABI evidence, not a claim that source-level AOT lowering already
emits Group calls. native_status therefore names the verified runtime
boundary (verified-native-runtime-abi); native compiler lowering and
cross-target scheduler work remain future leaves. The report never contains
physical paths, addresses, process IDs, timestamps or user payloads beyond
the bounded logical values needed by the corpus.

Run the conformance lane with:

    scripts/stdlib-async-group-conformance.sh

The runner writes
target/reliability/evidence/stdlib-async-group-conformance.json with the
fixture/probe hashes, source revision, normalized observations and cleanup
invariants. Contract negatives are exercised by
scripts/stdlib-async-group-conformance-test.sh.
