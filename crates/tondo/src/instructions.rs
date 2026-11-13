//! The functions of `std.cpu.x64` and `std.cpu.arm64` (standard library
//! 14.47): one instruction of the processor each, which the checker puts
//! where the call is written and the code generator emits as it is.

use crate::project::Arch;

/// An instruction a function of `std.cpu.x64` or `std.cpu.arm64` is.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Instruction {
    /// `crc32` on a byte, or arm64's `crc32cb`.
    Crc32cByte,
    /// `crc32` on 64 bits, or arm64's `crc32cx`.
    Crc32cInt,
    Pdep,
    Pext,
    AesEnc,
    AesEncLast,
    AesDec,
    AesDecLast,
    /// `pclmulqdq`, with the halves as a constant.
    Pclmulqdq,
    Aese,
    Aesd,
    Aesmc,
    Aesimc,
    /// `pmull` of 64 bits.
    Pmull,
    /// `pause` on x64, `isb` on arm64.
    Pause,
}

/// The variants of `cpu.Feature` (standard library 14.44) an instruction
/// needs, by their index.
mod feature {
    pub const SSE42: u32 = 3;
    pub const BMI2: u32 = 9;
    pub const PCLMULQDQ: u32 = 15;
    pub const CRC32: u32 = 19;
    pub const PMULL: u32 = 20;
    pub const AES: u32 = 22;
}

/// Each function of the two modules: its architecture, its name, the
/// instruction and the feature it needs, if any.
pub const INSTRUCTIONS: [(Arch, &str, Instruction, Option<u32>); 18] = {
    use Arch::{Arm64, X64};
    use Instruction::*;
    use feature::*;
    [
        (X64, "crc32cByte", Crc32cByte, Some(SSE42)),
        (X64, "crc32cInt", Crc32cInt, Some(SSE42)),
        (X64, "depositBits", Pdep, Some(BMI2)),
        (X64, "extractBits", Pext, Some(BMI2)),
        (X64, "aesEncrypt", AesEnc, Some(AES)),
        (X64, "aesEncryptLast", AesEncLast, Some(AES)),
        (X64, "aesDecrypt", AesDec, Some(AES)),
        (X64, "aesDecryptLast", AesDecLast, Some(AES)),
        (X64, "carrylessMultiply", Pclmulqdq, Some(PCLMULQDQ)),
        (X64, "pause", Pause, None),
        (Arm64, "crc32cByte", Crc32cByte, Some(CRC32)),
        (Arm64, "crc32cInt", Crc32cInt, Some(CRC32)),
        (Arm64, "aesEncrypt", Aese, Some(AES)),
        (Arm64, "aesDecrypt", Aesd, Some(AES)),
        (Arm64, "aesMixColumns", Aesmc, Some(AES)),
        (Arm64, "aesInverseMixColumns", Aesimc, Some(AES)),
        (Arm64, "polynomialMultiply", Pmull, Some(PMULL)),
        (Arm64, "pause", Pause, None),
    ]
};

/// The values `carrylessMultiply`'s `halves` may be, with what each takes.
pub const HALVES: [(i128, &str); 4] = [
    (0, "the low halves of both"),
    (1, "the high half of `a` and the low half of `b`"),
    (16, "the low half of `a` and the high half of `b`"),
    (17, "the high halves of both"),
];

/// The module of `arch`, as `cpu.x64`.
pub fn module(arch: Arch) -> String {
    format!("cpu.{}", arch.name())
}

/// The instruction a function of a module, as `cpu.x64`, is, and the
/// feature it needs.
pub fn of(module: &str, name: &str) -> Option<(Instruction, Option<u32>)> {
    INSTRUCTIONS
        .iter()
        .find(|(arch, function, ..)| *function == name && module == self::module(*arch))
        .map(|&(_, _, instruction, feature)| (instruction, feature))
}

/// The name a program calls an instruction of `arch` by, as
/// `x64.depositBits`.
pub fn name(instruction: Instruction, arch: Arch) -> String {
    let found = INSTRUCTIONS.iter().find(|(a, _, i, _)| *a == arch && *i == instruction);
    format!("{}.{}", arch.name(), found.map_or("", |(_, name, ..)| *name))
}

/// The feature an instruction needs on `arch`.
pub fn feature(instruction: Instruction, arch: Arch) -> Option<u32> {
    INSTRUCTIONS.iter().find(|(a, _, i, _)| *a == arch && *i == instruction).and_then(|&(.., feature)| feature)
}
