# Native reproducible package contract

`NATIVE-REL-001` closes the candidate package boundary for the admitted target.
The package contains an executable, the native runtime identity, STD-0.1A
contract metadata and checksums in a deterministic `tondo-native-package/1`
archive. It records the target, profile and toolchain contract versions, but
never a physical workspace path, timestamp, host name or undeclared
environment value.

The runner builds the same fixture twice in separate clean staging directories
with an absolute C driver and the link contract's no-build-id policy. It sets
archive mtimes to `epoch-zero` and numeric owners to zero, sorts entries, and compares package
bytes and all logical checksums. A changed binary/runtime/stdlib hash, target,
metadata path or partial archive fails closed. A physical workspace path is never
stored in the manifest. This is a reproducible candidate
package, not the final STD 0.1.0 release and not a backend selection; its
selection field remains `pending-NATIVE-001`.
