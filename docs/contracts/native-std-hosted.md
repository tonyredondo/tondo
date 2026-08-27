# Native `std.hosted` contract

`NATIVE-STD-HOSTED-001` closes the first hosted standard-library boundary for
the native runtime. It is deliberately an ABI contract, not a promise about an
OS descriptor layout: native code exchanges opaque `u64` capabilities and
bounded byte carriers with `tondo-native-runtime`.

## Capability boundary

The capability ids are private and stable for edition 0.1: `console=0`,
`filesystem=1`, `process=2` and `clock=3`. `tondo_rt_host_open` accepts only a
selected id. Unknown ids return zero and set `unsupported`; there is no PATH,
current-directory, environment-variable or ambient provider lookup. A target
without a selected capability must reject the source before lowering, while the
runtime repeats the check for stale or forged native calls.

## Handles, buffers and operations

`Host` handles are affine runtime tokens. They have the states `open`,
`cancelled` and `closed`; cancellation is terminal for I/O and cleanup is
performed exactly once by `close` or by ARC destruction. A second close is an
invalid transition and never releases a resource twice.

`Buffer` is an immutable, bounded byte carrier. Its only bootstrap constructor
is `tondo_rt_buffer_from_byte`; length and indexed reads are scalar operations,
so no pointer, address or host allocation escapes the private ABI. A production
provider may construct larger buffers behind the same limit without changing
the contract.

The native operations are:

* `host_open(capability) -> Host` (zero plus `unsupported` on rejection);
* `host_read(host, max_bytes) -> Result[Buffer, HostError]`;
* `host_write(host, Buffer) -> Result[UInt64, HostError]`;
* `host_output(host) -> Result[Buffer, HostError]`;
* `host_cancel(host) -> Unit` and `host_close(host) -> Unit`;
* `host_status(host) -> HostState`;
* `buffer_len` and `buffer_byte` for bounded observation.

Reads preserve their cursor and may return fewer bytes than requested. EOF is a
successful empty buffer at this ABI edge; the standard-library adapter maps it
to `Option.none` for the public `Reader` contract. A write either appends the
whole buffer or returns `limit`; it never publishes a partial write. Errors are
returned in the same opaque `Result` carrier used by `std.core` and also update
the private status channel for diagnostic probes.

## Evidence

[`testing/native-std-hosted.json`](../../testing/native-std-hosted.json) fixes
the cases and negative boundary. The focused runtime suite in
`crates/tondo-native-runtime` executes the capability, partial-I/O,
cancellation, stale-handle, limit and cleanup cases. The runner writes
`target/reliability/evidence/native-std-hosted.json` only after that suite
passes; it records the exact Rust toolchain and contract hash and does not
claim a backend ranking. `NATIVE-STD-001` is the following coordination block,
where Core and Hosted observations are compared with the VM and both candidate
adapters.
