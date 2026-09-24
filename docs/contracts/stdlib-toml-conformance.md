# `std.toml` VM/native conformance boundary

`STD-TOML-CONF-001` compares six TOML 1.1 cases on the verified hosted
bytecode VM and in a separate native process. The machine-readable contract is
[`testing/stdlib-toml-conformance.json`](../../testing/stdlib-toml-conformance.json).
Both probes compile the same byte fixtures from
[`testing/stdlib-toml-conformance-fixtures`](../../testing/stdlib-toml-conformance-fixtures).

Run `scripts/stdlib-toml-conformance.sh` for the two-process comparison and
reproducible report. Run `scripts/stdlib-toml-conformance-test.sh` for focused
contract mutations. The probe commands in the contract can be run separately.

The VM probe builds verified bytecode with one bodyless, **test-only** host
callable per case. Its host callback runs the portable Rust TOML kernel and
returns a detached normalized string through the VM. The native probe runs the
same kernel in a fresh process, reports structured JSON, and checks that the
native runtime has zero live table objects after each case. The kernel does not
allocate native-runtime table objects, so that counter is a terminal-state
observation, not a TOML memory-management test. This verifies VM
host dispatch, result admission, shared fixture behavior, and the native
standard-library process. It does **not** execute a source-level Tondo
`std.toml` call. The compiler TOML API and hosted production registration are
still unimplemented; this block does not promote them.

The shared corpus covers dynamic root tables and typed map round trips;
Unicode, radix integers, offset date/time, inline tables, insertion order and
canonical order; arrays of tables, one-byte reader fragments and terminal
`Closed`; duplicate-key kind, path and byte location; atomic input-limit
rejection; and the scalar/SIMD/AOT route boundary. Native JSON must match the
exact expected fields, while its `line` values must match the VM's returned
lines in case order. The VM's lines are calculated from parsed values and
errors, not accepted merely because the host callback ran.

The report at `target/reliability/evidence/stdlib-toml-conformance.json`
records the Git revision, probe and fixture hashes, normalized observations,
and test outcomes. Raw test logs remain beside the report but do not affect its
identity because build and test timing can vary. The report omits physical
paths, timestamps, PIDs and addresses.
The selected route is the portable scalar kernel. No SIMD route, native TOML
ABI, Cranelift TOML lowering, or native AOT execution is measured or claimed.
The toolchain's `tondo.toml` project manifest remains a separate owner and is
never routed through this codec by this conformance.
