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

A value plus a sum with a constant, as a loop's running total plus its
counter and a step, is a `lea` of the inner sum and an `add` of the value
on x86-64, not one `lea` of a base, an index and an offset, which takes two
cycles on AMD's cores and three on older Intel ones: the total waits one
cycle a turn instead of two.

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

The instructions of `std.cpu.x64` and `std.cpu.arm64` are here too, as
Cranelift has none of them. `cranelift-codegen-meta/` 0.137.0, which
defines the generator's instructions and settings, has the CLIF of each in
`define_tondo` of `src/shared/instructions.rs`: `crc32c8`, `crc32c64` and
`spin_hint` for both, `x86_pdep`, `x86_pext`, the four `x86_aes*` and
`x86_pclmulqdq` for x86-64, and the four `aarch64_aes*` and
`aarch64_pmull64` for arm64; and the settings `has_aes` and `has_pclmulqdq`
in `src/isa/x86.rs`. `cranelift-assembler-x64-meta/` 0.137.0, from which
the assembler of x86-64 is made, has their instructions in
`src/instructions/tondo.rs`, with the features `aes` and `pclmulqdq`, and
`cranelift-assembler-x64/` is the published crate, kept beside it so that
it builds from this one. Their lowering is marked `Tondo:` in
`src/isa/x64/lower.isle` and `src/isa/aarch64/lower.isle`, and arm64's
instructions, `TondoRRR`, `TondoRRMod`, `TondoRR` and `Isb`, in
`src/isa/aarch64/inst.isle` and `inst/`, with their encodings from the
Arm Architecture Reference Manual, as `llvm-mc` gives them too.

This copy is where an optimization that Tondo's code needs and Cranelift
lacks is written, as a rule marked `Tondo:`. To move to a newer Cranelift:
copy the published crates here, drop `src/isa/riscv64`, `src/isa/s390x` and
`src/isa/pulley_shared` but their `.isle` files, and add the rules and the
instructions again, but those Cranelift has by then.
