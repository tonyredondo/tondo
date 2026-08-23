# Async selection VM conformance

This contract closes `ASYNC-SELECT-VM-CONF-001` for the hosted reference VM.
It is deliberately separate from the performance contract: a conformance run
proves language observables and exact ownership outcomes, while the performance
probe measures implementation counters that are not Tondo semantics.

The live draft runner executes the complete `conformance/0.1` suite through the
same parser, formatter, interface, bytecode verifier and VM path. The contract
then checks the three public selection cases explicitly:

- `select-runtime` commits a directly `selectable` operation;
- `select-join-runtime` commits a pending `Join`; and
- `select-join-loser-runtime` consumes the winner and subsequently observes the
  losing `Join` without leaking its owner.

Each case runs 32 times. The adapter returns an exact observation hash for each
run; the contract requires every repetition for a case to be identical and to
match the pinned hash in `testing/async-select-conformance.json`. The report is
written to `target/reliability/evidence/async-select-conformance.json` and is
not a release artifact.

This is a VM-hosted conformance claim only. Native lowering remains pending in
`NATIVE-SELECT-001`; a native backend cannot inherit this result without running
the same corpus and proving semantic equivalence.
