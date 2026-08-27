# Native build/run CLI contract

`NATIVE-CLI-001` exposes the closed native build lifecycle without adding a
second language mode. `tondo build` discovers the same TOML project and lock
file as `check`, compiles through the shared frontend/MIR, and atomically emits
the canonical build artifact plus a small native envelope containing the
candidate identities. Until backend selection, that envelope is explicitly
`selection-pending`; it is not an executable product and cannot silently pick
Cranelift or LLVM.

`tondo run` keeps the existing source/project invocation and forwards stdout,
stderr, argv and exit classes unchanged. It does not accept `--native` or
`--vm`, and diagnostics use the standard human/JSON envelope. Once a backend is
selected, the same command path consumes the closed link/publish product; no
parallel public semantics are introduced during the candidate phase.

Build output is staged beside the destination and atomically renamed. Compile,
manifest, permission and interruption failures remove staging files and leave
no partial product. Product paths are never inserted into artifact identity.
The integration runner builds the fixture twice, compares both canonical
outputs, executes `run` with arguments and checks failure cleanup.
