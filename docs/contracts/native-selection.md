# Native backend decision-readiness contract

`NATIVE-001` closes the evidence collection boundary, not the backend choice.
The capture consumes the real-MIR fast report and the physical executable
report for the same `x86_64-unknown-linux-gnu` target. It requires measured
Cranelift and LLVM candidates, exact native/VM counts (118 scalar, 3 managed,
21 runtime, 8 select, 5 thread, 14 `std.core`, 1 lowering and 8 diagnostics),
and the path-free target/oracle identities. It also records aggregate
compile-time and code-size measurements without choosing a winner.

The product scope for the follow-up campaign is fixed by
[`native-aot-scope.md`](native-aot-scope.md): native AOT is the primary 0.1
product, `tondo-vm-hosted` is the reference/oracle target, and JIT is out of
scope. The “ready” status of this contract means that the bounded `NATIVE-001`
evidence can be reviewed; it does not mean that `DEC-013` or Gate N1 is ready
to close.

`DEC-013` is the human decision after the AOT campaign: select the first
backend only after reviewing these reports, repeated AOT performance and
memory data, comparable linked artifacts and the quality gate. Until that
record exists, `selected_backend` remains null and `n1_claim` remains false. A
missing report, count drift, divergence, target drift, path leak or premature
selection is fail-closed.
