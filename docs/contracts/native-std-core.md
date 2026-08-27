# Native `std.core` contract

`NATIVE-STD-CORE-001` closes the first native standard-library boundary. It
proves that the portable `Option` and `Result` core operations have the same
observable behavior in the VM, Cranelift candidate and LLVM candidate. The
machine-readable contract is [`testing/native-std-core.json`](../../testing/native-std-core.json).

## Boundary

The core surface is intrinsic and representation-independent:

* `Option.some`, `Option.none`, `Option.unwrapOr` and `Option.map`;
* `Result.ok`, `Result.err`, `Result.unwrapOr`, `Result.map` and `Result.mapErr`.

`Option` and `Result` values cross the native boundary as private runtime
carriers. Their tags and payloads are observed through `tondo_rt_result_tag`,
`tondo_rt_result_payload` and `tondo_rt_result_new`; no user-visible layout,
address or pointer is exposed.

The normalized MIR labels the only executable reads as `option-value`,
`result-ok-value` and `result-err-value`. A projection is executable only when
it has depth one and one of those three kinds. Field, index, nested and storage
projections remain explicit unsupported MIR instead of being approximated as a
read of the aggregate handle.

Named mapper functions are resolved while normalizing MIR. A callback stored in
a local becomes a direct ordinal call, so the native adapters do not need a
function-pointer ABI. Unknown or dynamic function values remain rejected by
the existing fail-closed policy.

`map` invokes its callback only on `Some`/`Ok`, while `mapErr` invokes it only on
`Err`. `unwrapOr` returns the payload or fallback and never panics. Construction,
projection, branch dispatch and cleanup continue to use the same normalized
MIR for both candidates.

## Evidence

`tests/native/native-std-core-001.to` is compiled through the ordinary compiler
and VM probe. The opt-in native runner receives that probe with
`--std-core-probe`, evaluates all fourteen cases in fresh subprocesses, and
records VM/oracle observables beside Cranelift and LLVM status in
`native_std_core_runs`. Managed cases release their returned carrier in the C
entry point, so a passing case includes the terminal ownership operation.

Static and negative contract checks are provided by
[`scripts/native-std-core-check.sh`](../../scripts/native-std-core-check.sh) and
[`scripts/native-std-core-test.sh`](../../scripts/native-std-core-test.sh).
The native runner remains opt-in because it compiles and executes a real
cross-backend corpus; the normal test gate still checks the contract and its
failure modes without requiring LLVM or a C linker.

This block does not claim hosted filesystem/network/time APIs, collection
storage, iterator lowering or a selected backend. Those are explicit follow-up
boundaries (`NATIVE-STD-HOSTED-001` and later).
