//! Tondo: the instructions of `std.cpu.x64`, one for each of its functions
//! (Tondo's change 23): CRC32C of a byte and of 64 bits, BMI2's bit deposit
//! and extract, the rounds of AES, carry-less multiplication and `pause`.

use crate::dsl::{Feature::*, Inst, Length::*, Location::*};
use crate::dsl::{align, fmt, inst, r, rex, rw, vex, w};

#[rustfmt::skip] // Keeps instructions on a single line.
pub fn list() -> Vec<Inst> {
    vec![
        inst("crc32b", fmt("RM", [rw(r32), r(rm8)]), rex([0xF2, 0x0F, 0x38, 0xF0]).r(), (_64b | compat) & sse42),
        inst("crc32q", fmt("RM", [rw(r64), r(rm64)]), rex([0xF2, 0x0F, 0x38, 0xF1]).w().r(), _64b & sse42),
        inst("pdepq", fmt("RVM", [w(r64a), r(r64b), r(rm64)]), vex(LZ)._f2()._0f38().w1().op(0xF5), _64b & bmi2),
        inst("pextq", fmt("RVM", [w(r64a), r(r64b), r(rm64)]), vex(LZ)._f3()._0f38().w1().op(0xF5), _64b & bmi2),
        inst("aesenc", fmt("A", [rw(xmm1), r(align(xmm_m128))]), rex([0x66, 0x0F, 0x38, 0xDC]).r(), (_64b | compat) & aes),
        inst("aesenclast", fmt("A", [rw(xmm1), r(align(xmm_m128))]), rex([0x66, 0x0F, 0x38, 0xDD]).r(), (_64b | compat) & aes),
        inst("aesdec", fmt("A", [rw(xmm1), r(align(xmm_m128))]), rex([0x66, 0x0F, 0x38, 0xDE]).r(), (_64b | compat) & aes),
        inst("aesdeclast", fmt("A", [rw(xmm1), r(align(xmm_m128))]), rex([0x66, 0x0F, 0x38, 0xDF]).r(), (_64b | compat) & aes),
        inst("pclmulqdq", fmt("A", [rw(xmm1), r(align(xmm_m128)), r(imm8)]), rex([0x66, 0x0F, 0x3A, 0x44]).r().ib(), (_64b | compat) & pclmulqdq),
        inst("pause", fmt("ZO", []), rex([0xF3, 0x90]), _64b | compat),
    ]
}
