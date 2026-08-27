# Native conformance coordination contract

`NATIVE-CONF-001` is the coordinator for the native 0.1A corpus. It invokes
the adapter and the language, testing and STD-0.1A leaves independently for
both Cranelift and LLVM. Every observation is normalized through the same
`tondo-mir-backend/1` probe and compared with the independent VM oracle before
the coordinator can publish evidence.

The coordinator is deliberately fail-closed: a missing leaf, category,
backend, target, MIR version, duplicate observation, divergence or physical
path aborts the run. It coordinates the existing contracts; it does not add
a second frontend, a backend-specific public API, or a claim that the final
backend has already been selected. Generated evidence is path-free and records
the exact contract hash and the three categories covered. The next boundary is
the executable differential campaign in `NATIVE-DIFF-001`.
