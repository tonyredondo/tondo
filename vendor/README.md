# Vendored crates

`cranelift-codegen/` is Cranelift's code generator 0.137.0, from crates.io,
with one change, which the workspace's `[patch.crates-io]` puts in place of
the published crate: a branch on the overflow of `sadd_overflow` or
`ssub_overflow` is one instruction, `jo` on x86-64 and `b.vs` on arm64, as
Cranelift already does for `uadd_overflow` and the multiplications. Tondo
checks every addition and subtraction of integers, which otherwise cost a
`seto`, a `test` and a jump each. The rules are marked `Tondo:` in
`src/isa/x64/inst.isle`, `src/isa/x64/lower.isle`,
`src/isa/aarch64/inst.isle` and `src/isa/aarch64/lower.isle`.

Only the code of x86-64 and arm64 is kept; the build still compiles every
target's ISLE, so the `.isle` files of the others stay.

To move to a newer Cranelift: copy the published crate here, drop
`src/isa/riscv64`, `src/isa/s390x` and `src/isa/pulley_shared` but their
`.isle` files, and add the four rules again, unless Cranelift has them by
then, when this directory and the patch go.
