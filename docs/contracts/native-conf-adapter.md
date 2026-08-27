# Native conformance adapter contract

The conformance adapter is the narrow boundary between the common MIR
evaluation and the public conformance suites. It receives a hash-bound probe,
an explicit backend (`cranelift` or `llvm`), an explicit target triple and a
selected capability set. It emits `tondo-native-observation/1`: stable status,
value/tag/payload, diagnostics, testing lifecycle and resource-cleanup fields.

The VM oracle is read from the probe's expected observations and is never
reused as a native result. An unknown backend, target, capability or probe
shape fails before an observation is written; every rejection is fail-closed. Unsupported MIR cases are
reported as `unsupported` and cannot be promoted to `passed`. Reports contain
logical identities only; physical paths are redacted.

`scripts/native-conformance-adapter.sh` is intentionally small and deterministic
so each owner can run it independently. Its output is a protocol observation,
not a backend-selection decision; the generated differential and performance
lanes still compare actual candidate executables before Gate N1.
