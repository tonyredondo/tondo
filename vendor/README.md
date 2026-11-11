# Vendored crates

`cranelift-codegen/` is Cranelift's code generator 0.137.0, from crates.io,
with these changes, which the workspace's `[patch.crates-io]` puts in place
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

A multiplication of a float by 2.0 is its sum with itself, which needs no
constant. A float constant is not made again in every block that uses it,
as an integer is, which costs nothing where an instruction takes it as an
immediate: making a float costs two instructions on x86-64, in each turn
of a loop, where computing it once keeps it in a register. And equal
constants that instructions read from memory, as x86-64 reads a float's,
are emitted once, however many read them.

And a pass of its own, `src/tondo_select.rs`, which `src/context.rs` runs
after the e-graph's: a branch whose two ways compute a few integers,
without failing, touching memory or calling anything, chooses between
them with `select`, `cmov` on x86-64 and `csel` on arm64, rather than
jumping. A way's checks stay, and fail when that way would have run.

The rules are marked `Tondo:` in `src/isa/x64/inst.isle`,
`src/isa/x64/lower.isle`, `src/isa/aarch64/inst.isle`,
`src/isa/aarch64/lower.isle`, `src/opts/arithmetic.isle`,
`src/opts/remat.isle` and `src/prelude_opt.isle`, and the constants'
merging in `src/machinst/vcode.rs`.

And a feature, `isle-rules`, marked `Tondo:` in `Cargo.toml` and
`build.rs`, has each ISLE rule that matches log itself, as `trace-log`
does but without regalloc2's logs: `tondo asm --rules`, in a `tondo` built
with its feature `rules`, counts them by function.

Only the code of x86-64 and arm64 is kept; the build still compiles every
target's ISLE, so the `.isle` files of the others stay.

This copy is where an optimization that Tondo's code needs and Cranelift
lacks is written, as a rule marked `Tondo:`. To move to a newer Cranelift:
copy the published crate here, drop `src/isa/riscv64`, `src/isa/s390x` and
`src/isa/pulley_shared` but their `.isle` files, and add the rules again,
but those Cranelift has by then.
