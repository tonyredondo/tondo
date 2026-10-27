# Vendored crates

`cranelift-codegen/` is Cranelift's code generator 0.137.0, from crates.io,
with three changes, which the workspace's `[patch.crates-io]` puts in place
of the published crate. Tondo checks every addition, subtraction and
multiplication of integers:

- a branch on the overflow of `sadd_overflow` or `ssub_overflow` is one
  instruction, `jo` on x86-64 and `b.vs` on arm64, as Cranelift already
  does for `uadd_overflow` and the multiplications, where it cost a `seto`,
  a `test` and a jump;
- `smul_overflow` of 32 or 64 bits on x86-64 is the two-operand `imul`,
  which sets the overflow flag the same, in one micro-op and without `rdx`.

And a multiplication by 3, 5, 7 or 9, as a remainder by a constant makes
one, is a shift and an addition or a subtraction, as one by a power of
two is a shift already: a multiplication waits for the one unit that
multiplies.

The rules are marked `Tondo:` in `src/isa/x64/inst.isle`,
`src/isa/x64/lower.isle`, `src/isa/aarch64/inst.isle`,
`src/isa/aarch64/lower.isle`, `src/opts/arithmetic.isle` and
`src/prelude_opt.isle`.

Only the code of x86-64 and arm64 is kept; the build still compiles every
target's ISLE, so the `.isle` files of the others stay.

To move to a newer Cranelift: copy the published crate here, drop
`src/isa/riscv64`, `src/isa/s390x` and `src/isa/pulley_shared` but their
`.isle` files, and add the rules again, unless Cranelift has them by then,
when this directory and the patch go.
