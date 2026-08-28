# Native backend selection contract

`NATIVE-001` closes the evidence collection boundary and supplies the record
for `DEC-013`. The decision now selects **Cranelift** as Tondo 0.1's native AOT
backend for the admitted `x86_64-unknown-linux-gnu` target. The capture still
consumes the real-MIR fast report and the physical executable report for that
target, requiring measured Cranelift and LLVM candidates, exact native/VM
counts (118 scalar, 3 managed, 21 runtime, 8 select, 5 thread, 14 `std.core`,
1 lowering and 8 diagnostics), and path-free target/oracle identities.
LLVM remains an experimental comparison backend; it is not an automatic
fallback and it is not shipped as the default backend.

The product scope is fixed by [`native-aot-scope.md`](native-aot-scope.md):
native AOT is the primary 0.1 product, `tondo-vm-hosted` is the reference/oracle
target, and JIT is out of scope. The selected backend is Cranelift. The
`selected` status records the human decision only; Gate N1 remains open and the
native product is not yet promoted.

## DEC-013 decision

The decision is deliberately scoped to the current target and product lane:

* `selected_backend` is `cranelift` for native AOT on
  `x86_64-unknown-linux-gnu`.
* Cranelift was chosen for its Rust-native embedded integration and lower
  maintenance/distribution cost. The current AOT campaign also shows runtime
  dimensions within one percent of LLVM and a slightly smaller stripped
  product; LLVM's build-time advantage does not offset the additional
  toolchain/FFI burden for the first backend.
* LLVM remains available for experimental comparison and future reconsideration
  if a target or workload demonstrates a material need. There is no silent
  backend fallback.
* `n1_claim` remains `false`: Gate N1 still has to prove the selected Cranelift
  implementation across its complete conformance, diagnostics, target and
  packaging gates.

A missing report, count drift, divergence, target drift, path leak, unknown
selection or premature N1 claim is fail-closed.
