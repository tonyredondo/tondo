# Native build/run CLI contract

`NATIVE-CLI-001` exposes the closed native build lifecycle without adding a
second language mode. `tondo build` discovers the same TOML project and lock
file as `check`, compiles through the shared frontend/MIR, and atomically emits
the canonical build artifact plus a small native envelope containing the
candidate identities. `DEC-013` records Cranelift as the selected backend for
the admitted 0.1 target, so the envelope is `backend-selected` and names
`promotion: pending-gate-n1`. It is not an executable native product until
Gate N1 passes, and it cannot silently fall back to LLVM or another backend.

`tondo run` keeps the existing source/project invocation and forwards stdout,
stderr, argv and exit classes unchanged. It does not accept `--native` or
`--vm`, and diagnostics use the standard human/JSON envelope. The same command
path will consume the closed link/publish product once Gate N1 promotes it; no
parallel public semantics are introduced while promotion is pending.

Build output is staged beside the destination and atomically renamed. Compile,
manifest, permission and interruption failures remove staging files and leave
no partial product. Product paths are never inserted into artifact identity.
The integration runner builds the fixture twice, compares both canonical
outputs, executes `run` with arguments and checks failure cleanup.
