# Native target registry and smoke contract

`NATIVE-TARGET-001` closes one admitted physical target at a time. The registry
contains the canonical triple, object format, profile, capability set, backend
candidates, fixture and artifact kind. The current 0.1 entry is
`x86_64-unknown-linux-gnu`/ELF on Linux. It is deliberately an explicit
registry entry, not an alias for whatever machine happens to run the compiler.

The runner checks the target descriptor, resolves the absolute C driver, links
the pinned native fixture in a fresh workspace and executes the resulting
artifact on the host target. It verifies the link evidence, non-empty product
and target triple. Cross-compilation is never counted as a physical smoke;
adding another architecture or capability set requires another registry entry,
runner evidence and artifact identity. Paths and ambient tool discovery are
forbidden in the published identity.

The portable CI matrix also runs `scripts/native-portability.sh` on each native
runner. Its pinned `cranelift-portability` probe selects the host ISA and emits
one object through the selected Cranelift version, checking the native object
magic (`ELF`, `Mach-O` or `COFF`) and recording a path-free report. This probe is
an early backend-compatibility signal: it does not promote a target into this
registry, prove the Tondo native ABI or replace the physical product smoke.
Those claims still require a target entry, complete lowering and Gate N1.

`NATIVE-TARGET-002` adds a separate draft target record for
`aarch64-unknown-linux-gnu`/ELF. On the native `ubuntu-24.04-arm` runner,
`scripts/native-target-aarch64-test.sh` resolves the absolute C driver, links
the same deterministic fixture and executes it on the ARM64 host. Its report is
kept with the portable CI artifact and is bound to the target and descriptor
hashes. This is a physical target smoke, not a cross-compile claim; it does not
promote the target or close N1 (`promotion: pending-gate-n1`). The selected
Cranelift path is recorded for the candidate, while LLVM remains outside this
target-specific smoke until its complete native product lane is admitted.
