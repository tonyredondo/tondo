//! Tondo: the forms of 256 bits, VEX.256, of the instructions that the
//! vectors of `std.simd` of 256 bits are made of (Tondo's change 22b). Each
//! format's name is that of its 128-bit form followed by `Y`. The integer
//! forms need AVX2, the float ones AVX.

use crate::dsl::{Eflags::*, Feature::*, Inst, Length::*, Location::*};
use crate::dsl::{fmt, inst, r, rw, vex, w};

#[rustfmt::skip] // Keeps instructions on a single line.
pub fn list() -> Vec<Inst> {
    // `ymm1 = ymm2 op ymm3/m256`.
    let three = || fmt("BY", [w(ymm1), r(ymm2), r(ymm_m256)]);
    // The same with the operands of the 128-bit form's name "A".
    let three_a = || fmt("AY", [w(ymm1), r(ymm2), r(ymm_m256)]);
    let p66 = || vex(L256)._66()._0f();
    let p66_38 = || vex(L256)._66()._0f38();
    let p66_3a = || vex(L256)._66()._0f3a();
    let ps = || vex(L256)._0f();
    vec![
        // Moves, loads and stores.
        inst("vmovdqu", fmt("AY", [w(ymm1), r(ymm_m256)]), vex(L256)._f3()._0f().op(0x6F).r(), (_64b | compat) & avx),
        inst("vmovdqu", fmt("BY", [w(ymm_m256), r(ymm1)]), vex(L256)._f3()._0f().op(0x7F).r(), (_64b | compat) & avx),
        inst("vmovups", fmt("AY", [w(ymm1), r(ymm_m256)]), ps().op(0x10).r(), (_64b | compat) & avx),
        inst("vmovups", fmt("BY", [w(ymm_m256), r(ymm1)]), ps().op(0x11).r(), (_64b | compat) & avx),
        inst("vmovaps", fmt("AY", [w(ymm1), r(ymm_m256)]), ps().op(0x28).r(), (_64b | compat) & avx),
        inst("vzeroupper", fmt("ZO", []), vex(L128)._0f().op(0x77), (_64b | compat) & avx),

        // Integer arithmetic.
        inst("vpaddb", three(), p66().op(0xFC).r(), (_64b | compat) & avx2),
        inst("vpaddw", three(), p66().op(0xFD).r(), (_64b | compat) & avx2),
        inst("vpaddd", three(), p66().op(0xFE).r(), (_64b | compat) & avx2),
        inst("vpaddq", three(), p66().op(0xD4).r(), (_64b | compat) & avx2),
        inst("vpsubb", three(), p66().op(0xF8).r(), (_64b | compat) & avx2),
        inst("vpsubw", three(), p66().op(0xF9).r(), (_64b | compat) & avx2),
        inst("vpsubd", three(), p66().op(0xFA).r(), (_64b | compat) & avx2),
        inst("vpsubq", three(), p66().op(0xFB).r(), (_64b | compat) & avx2),
        inst("vpaddsb", three(), p66().op(0xEC).r(), (_64b | compat) & avx2),
        inst("vpaddsw", three(), p66().op(0xED).r(), (_64b | compat) & avx2),
        inst("vpaddusb", three(), p66().op(0xDC).r(), (_64b | compat) & avx2),
        inst("vpaddusw", three(), p66().op(0xDD).r(), (_64b | compat) & avx2),
        inst("vpsubsb", three(), p66().op(0xE8).r(), (_64b | compat) & avx2),
        inst("vpsubsw", three(), p66().op(0xE9).r(), (_64b | compat) & avx2),
        inst("vpsubusb", three(), p66().op(0xD8).r(), (_64b | compat) & avx2),
        inst("vpsubusw", three(), p66().op(0xD9).r(), (_64b | compat) & avx2),
        inst("vpmullw", three(), p66().op(0xD5).r(), (_64b | compat) & avx2),
        inst("vpmulld", three(), p66_38().op(0x40).r(), (_64b | compat) & avx2),
        inst("vpmuludq", three(), p66().op(0xF4).r(), (_64b | compat) & avx2),
        inst("vpabsb", fmt("AY", [w(ymm1), r(ymm_m256)]), p66_38().op(0x1C).r(), (_64b | compat) & avx2),
        inst("vpabsw", fmt("AY", [w(ymm1), r(ymm_m256)]), p66_38().op(0x1D).r(), (_64b | compat) & avx2),
        inst("vpabsd", fmt("AY", [w(ymm1), r(ymm_m256)]), p66_38().op(0x1E).r(), (_64b | compat) & avx2),

        // Float arithmetic.
        inst("vaddps", three(), ps().op(0x58).r(), (_64b | compat) & avx),
        inst("vaddpd", three(), p66().op(0x58).r(), (_64b | compat) & avx),
        inst("vsubps", three(), ps().op(0x5C).r(), (_64b | compat) & avx),
        inst("vsubpd", three(), p66().op(0x5C).r(), (_64b | compat) & avx),
        inst("vmulps", three(), ps().op(0x59).r(), (_64b | compat) & avx),
        inst("vmulpd", three(), p66().op(0x59).r(), (_64b | compat) & avx),
        inst("vdivps", three(), ps().op(0x5E).r(), (_64b | compat) & avx),
        inst("vdivpd", three(), p66().op(0x5E).r(), (_64b | compat) & avx),
        inst("vminps", three(), ps().op(0x5D).r(), (_64b | compat) & avx),
        inst("vminpd", three(), p66().op(0x5D).r(), (_64b | compat) & avx),
        inst("vmaxps", three(), ps().op(0x5F).r(), (_64b | compat) & avx),
        inst("vmaxpd", three(), p66().op(0x5F).r(), (_64b | compat) & avx),
        inst("vsqrtps", fmt("BY", [w(ymm1), r(ymm_m256)]), ps().op(0x51).r(), (_64b | compat) & avx),
        inst("vsqrtpd", fmt("BY", [w(ymm1), r(ymm_m256)]), p66().op(0x51).r(), (_64b | compat) & avx),
        inst("vroundps", fmt("RMIY", [w(ymm1), r(ymm_m256), r(imm8)]), p66_3a().op(0x08).r().ib(), (_64b | compat) & avx),
        inst("vroundpd", fmt("RMIY", [w(ymm1), r(ymm_m256), r(imm8)]), p66_3a().op(0x09).r().ib(), (_64b | compat) & avx),
        inst("vcmpps", fmt("BY", [w(ymm1), r(ymm2), r(ymm_m256), r(imm8)]), ps().op(0xC2).r().ib(), (_64b | compat) & avx),
        inst("vcmppd", fmt("BY", [w(ymm1), r(ymm2), r(ymm_m256), r(imm8)]), p66().op(0xC2).r().ib(), (_64b | compat) & avx),
        inst("vfmadd213ps", fmt("AY", [rw(ymm1), r(ymm2), r(ymm_m256)]), p66_38().w0().op(0xA8).r(), (_64b | compat) & fma),
        inst("vfmadd213pd", fmt("AY", [rw(ymm1), r(ymm2), r(ymm_m256)]), p66_38().w1().op(0xA8).r(), (_64b | compat) & fma),
        inst("vandps", three_a(), ps().op(0x54).r(), (_64b | compat) & avx),
        inst("vandnps", three_a(), ps().op(0x55).r(), (_64b | compat) & avx),
        inst("vorps", three_a(), ps().op(0x56).r(), (_64b | compat) & avx),
        inst("vxorps", three_a(), ps().op(0x57).r(), (_64b | compat) & avx),

        // Bits.
        inst("vpand", three(), p66().op(0xDB).r(), (_64b | compat) & avx2),
        inst("vpandn", three(), p66().op(0xDF).r(), (_64b | compat) & avx2),
        inst("vpor", three(), p66().op(0xEB).r(), (_64b | compat) & avx2),
        inst("vpxor", three(), p66().op(0xEF).r(), (_64b | compat) & avx2),

        // Least and greatest.
        inst("vpminsb", three(), p66_38().op(0x38).r(), (_64b | compat) & avx2),
        inst("vpminsw", three(), p66().op(0xEA).r(), (_64b | compat) & avx2),
        inst("vpminsd", three(), p66_38().op(0x39).r(), (_64b | compat) & avx2),
        inst("vpminub", three(), p66().op(0xDA).r(), (_64b | compat) & avx2),
        inst("vpminuw", three(), p66_38().op(0x3A).r(), (_64b | compat) & avx2),
        inst("vpminud", three(), p66_38().op(0x3B).r(), (_64b | compat) & avx2),
        inst("vpmaxsb", three(), p66_38().op(0x3C).r(), (_64b | compat) & avx2),
        inst("vpmaxsw", three(), p66().op(0xEE).r(), (_64b | compat) & avx2),
        inst("vpmaxsd", three(), p66_38().op(0x3D).r(), (_64b | compat) & avx2),
        inst("vpmaxub", three(), p66().op(0xDE).r(), (_64b | compat) & avx2),
        inst("vpmaxuw", three(), p66_38().op(0x3E).r(), (_64b | compat) & avx2),
        inst("vpmaxud", three(), p66_38().op(0x3F).r(), (_64b | compat) & avx2),

        // Comparisons.
        inst("vpcmpeqb", three(), p66().op(0x74).r(), (_64b | compat) & avx2),
        inst("vpcmpeqw", three(), p66().op(0x75).r(), (_64b | compat) & avx2),
        inst("vpcmpeqd", three(), p66().op(0x76).r(), (_64b | compat) & avx2),
        inst("vpcmpeqq", three(), p66_38().op(0x29).r(), (_64b | compat) & avx2),
        inst("vpcmpgtb", three(), p66().op(0x64).r(), (_64b | compat) & avx2),
        inst("vpcmpgtw", three(), p66().op(0x65).r(), (_64b | compat) & avx2),
        inst("vpcmpgtd", three(), p66().op(0x66).r(), (_64b | compat) & avx2),
        inst("vpcmpgtq", three(), p66_38().op(0x37).r(), (_64b | compat) & avx2),

        // Shifts, by the count in an XMM register and by an immediate.
        inst("vpsllw", fmt("CY", [w(ymm1), r(ymm2), r(xmm_m128)]), p66().op(0xF1).r(), (_64b | compat) & avx2),
        inst("vpslld", fmt("CY", [w(ymm1), r(ymm2), r(xmm_m128)]), p66().op(0xF2).r(), (_64b | compat) & avx2),
        inst("vpsllq", fmt("CY", [w(ymm1), r(ymm2), r(xmm_m128)]), p66().op(0xF3).r(), (_64b | compat) & avx2),
        inst("vpsrlw", fmt("CY", [w(ymm1), r(ymm2), r(xmm_m128)]), p66().op(0xD1).r(), (_64b | compat) & avx2),
        inst("vpsrld", fmt("CY", [w(ymm1), r(ymm2), r(xmm_m128)]), p66().op(0xD2).r(), (_64b | compat) & avx2),
        inst("vpsrlq", fmt("CY", [w(ymm1), r(ymm2), r(xmm_m128)]), p66().op(0xD3).r(), (_64b | compat) & avx2),
        inst("vpsraw", fmt("CY", [w(ymm1), r(ymm2), r(xmm_m128)]), p66().op(0xE1).r(), (_64b | compat) & avx2),
        inst("vpsrad", fmt("CY", [w(ymm1), r(ymm2), r(xmm_m128)]), p66().op(0xE2).r(), (_64b | compat) & avx2),
        inst("vpsllw", fmt("DY", [w(ymm1), r(ymm2), r(imm8)]), p66().op(0x71).digit(6).ib(), (_64b | compat) & avx2),
        inst("vpslld", fmt("DY", [w(ymm1), r(ymm2), r(imm8)]), p66().op(0x72).digit(6).ib(), (_64b | compat) & avx2),
        inst("vpsllq", fmt("DY", [w(ymm1), r(ymm2), r(imm8)]), p66().op(0x73).digit(6).ib(), (_64b | compat) & avx2),
        inst("vpsrlw", fmt("DY", [w(ymm1), r(ymm2), r(imm8)]), p66().op(0x71).digit(2).ib(), (_64b | compat) & avx2),
        inst("vpsrld", fmt("DY", [w(ymm1), r(ymm2), r(imm8)]), p66().op(0x72).digit(2).ib(), (_64b | compat) & avx2),
        inst("vpsrlq", fmt("DY", [w(ymm1), r(ymm2), r(imm8)]), p66().op(0x73).digit(2).ib(), (_64b | compat) & avx2),
        inst("vpsraw", fmt("DY", [w(ymm1), r(ymm2), r(imm8)]), p66().op(0x71).digit(4).ib(), (_64b | compat) & avx2),
        inst("vpsrad", fmt("DY", [w(ymm1), r(ymm2), r(imm8)]), p66().op(0x72).digit(4).ib(), (_64b | compat) & avx2),

        // Choosing by a mask, and reading one.
        inst("vpblendvb", fmt("RVMRY", [w(ymm1), r(ymm2), r(ymm_m256), r(ymm3)]), p66_3a().w0().op(0x4C).r().is4(), (_64b | compat) & avx2),
        inst("vpmovmskb", fmt("RMY", [w(r32), r(ymm2)]), p66().op(0xD7).r(), (_64b | compat) & avx2),
        inst("vmovmskps", fmt("RMY", [w(r32), r(ymm2)]), ps().op(0x50).r(), (_64b | compat) & avx),
        inst("vmovmskpd", fmt("RMY", [w(r32), r(ymm2)]), p66().op(0x50).r(), (_64b | compat) & avx),
        inst("vptest", fmt("RMY", [r(ymm1), r(ymm_m256)]).flags(W), p66_38().op(0x17).r(), (_64b | compat) & avx),

        // One value in every lane.
        inst("vpbroadcastb", fmt("AY", [w(ymm1), r(xmm_m8)]), p66_38().w0().op(0x78).r(), (_64b | compat) & avx2),
        inst("vpbroadcastw", fmt("AY", [w(ymm1), r(xmm_m16)]), p66_38().w0().op(0x79).r(), (_64b | compat) & avx2),
        inst("vpbroadcastd", fmt("AY", [w(ymm1), r(xmm_m32)]), p66_38().w0().op(0x58).r(), (_64b | compat) & avx2),
        inst("vpbroadcastq", fmt("AY", [w(ymm1), r(xmm_m64)]), p66_38().w0().op(0x59).r(), (_64b | compat) & avx2),

        // The halves, and the lanes in another order.
        inst("vextracti128", fmt("MRIY", [w(xmm_m128), r(ymm2), r(imm8)]), p66_3a().w0().op(0x39).r().ib(), (_64b | compat) & avx2),
        inst("vinserti128", fmt("RVMIY", [w(ymm1), r(ymm2), r(xmm_m128), r(imm8)]), p66_3a().w0().op(0x38).r().ib(), (_64b | compat) & avx2),
        inst("vpermq", fmt("RMIY", [w(ymm1), r(ymm_m256), r(imm8)]), p66_3a().w1().op(0x00).r().ib(), (_64b | compat) & avx2),
        inst("vpshufb", three(), p66_38().op(0x00).r(), (_64b | compat) & avx2),
    ]
}
