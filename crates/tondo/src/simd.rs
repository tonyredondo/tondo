//! The vectors of `std.simd` (standard library 14.46): their operations,
//! intrinsics of the compiler that the checker resolves and the code
//! generator makes lane by lane, on vectors of 128 bits and on the halves of
//! wider ones.

use crate::types::{FloatTy, IntTy, Ty, TyKind, Types, Width};

/// What the lanes of a vector are.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Lanes {
    Signed(IntTy),
    Unsigned(IntTy),
    Byte,
    Float(FloatTy),
}

impl Lanes {
    /// The lanes of a vector of `lane`, or `None` for a type that no vector
    /// has (rule 2).
    pub fn of(types: &Types, lane: Ty) -> Option<Lanes> {
        Some(match types.kind(types.repr(lane)) {
            TyKind::Int(int @ (IntTy::I8 | IntTy::I16 | IntTy::I32 | IntTy::I64)) => Lanes::Signed(*int),
            TyKind::Int(int @ (IntTy::U8 | IntTy::U16 | IntTy::U32 | IntTy::U64)) => Lanes::Unsigned(*int),
            TyKind::Byte => Lanes::Byte,
            TyKind::Float(float) => Lanes::Float(*float),
            _ => return None,
        })
    }

    /// The bits of one lane.
    pub fn bits(self) -> u32 {
        match self {
            Lanes::Signed(int) | Lanes::Unsigned(int) => int.bits(),
            Lanes::Byte => 8,
            Lanes::Float(FloatTy::F32) => 32,
            Lanes::Float(FloatTy::F64) => 64,
        }
    }

    pub fn is_float(self) -> bool {
        matches!(self, Lanes::Float(_))
    }

    /// Whether the lanes are numbers with arithmetic: not a `Byte`.
    pub fn arithmetic(self) -> bool {
        self != Lanes::Byte
    }

    /// Whether the lanes are integers or bytes, with `&`, `|`, `^`, `~`
    /// and the shifts.
    pub fn bitwise(self) -> bool {
        !self.is_float()
    }
}

/// The types of lanes a vector may have (rule 2), as the errors list them.
pub const LANE_TYPES: &str =
    "`Int8`, `Int16`, `Int32`, `Int`, `UInt8`, `UInt16`, `UInt32`, `UInt`, `Byte`, `Float32` and `Float`";

/// Whether a value of `ty` is a vector or a mask wider than 128 bits,
/// which code for a level with wider vectors keeps in another way.
/// Why a place, which `what` says, may not keep `vector`, a `Vector[T]`
/// or a `Mask[T]`, and what to keep instead (`E1120`, language 18.11).
pub fn kept(what: &str, vector: &str) -> String {
    let (kind, lanes) = vector.split_once('[').unwrap_or((vector, "T]"));
    let fixed = |bits: u32| format!("{kind}{bits}[{lanes}");
    format!(
        "{what}, which lives only in locals, parameters and results, as the version of a function for a processor \
         with wider vectors gives it more lanes: keep a `{}` or a `{}` here, or its lanes in an array, with `toArray` \
         or `store`",
        fixed(128),
        fixed(256)
    )
}

pub fn wide(types: &Types, ty: Ty) -> bool {
    matches!(*types.kind(types.repr(ty)), TyKind::Vector(width, _) | TyKind::Mask(width, _) if width.bits() > 128)
}

/// The lanes a vector of `ty` has, as `lanes()` gives them: a
/// `Vector[T]`'s too, as it is as wide in every version of a function.
pub fn lane_count(types: &Types, ty: Ty) -> Option<u32> {
    let TyKind::Vector(width, lane) = *types.kind(types.repr(ty)) else { return None };
    Some(count(width, Lanes::of(types, lane)?))
}

/// How many lanes a vector of `width` with lanes of `lanes` has.
pub fn count(width: Width, lanes: Lanes) -> u32 {
    width.bits() / lanes.bits()
}

/// The functions of the standard library that a program's call reaches in
/// another form when its last type argument is `Float` or `Float32`: one
/// that adds in the order the language fixes (18.5), with vectors where it
/// can, and takes the other type arguments.
pub const FLOAT_FORMS: [(&str, &str, &str); 7] = [
    ("Array.sum", "prelude.sumFloat", "prelude.sumFloat32"),
    ("Array.sumBy", "prelude.sumByFloat", "prelude.sumByFloat32"),
    ("Iterator.sumBy", "prelude.iterSumByFloat", "prelude.iterSumByFloat32"),
    ("math.dot", "math.dotFloat", "math.dotFloat32"),
    ("math.dotAt", "math.dotAtFloat", "math.dotAtFloat32"),
    ("math.applied", "math.appliedFloat", "math.appliedFloat32"),
    ("math.cycled", "math.cycledFloat", "math.cycledFloat32"),
];

/// An operation of `std.simd`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SimdOp {
    // Making and reading (rule 7).
    Zero,
    Splat,
    Of,
    Load,
    /// `load` of lanes that `prove` showed are within the array.
    LoadInBounds,
    Lane,
    WithLane,
    ToArray,
    Store,
    /// `store` of lanes that `prove` showed are within the array.
    StoreInBounds,
    // Lane by lane (rules 8 to 11).
    Add,
    Sub,
    Mul,
    Div,
    Neg,
    And,
    Or,
    Xor,
    Not,
    Shl,
    Shr,
    Min,
    Max,
    Abs,
    SaturatingAdd,
    SaturatingSub,
    Sqrt,
    Floor,
    Ceil,
    Round,
    Truncate,
    Fma,
    // Comparing (rule 12).
    Equal,
    NotEqual,
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
    Choose,
    Any,
    All,
    Count,
    Bits,
    MaskLane,
    // Across the lanes (rule 13).
    Sum,
    MinLane,
    MaxLane,
    // Rearranging and converting (rule 14).
    Reversed,
    Swizzle,
    Joined,
    Lower,
    Upper,
    To,
    Cast,
    /// `==` of two vectors or masks: every lane equal.
    Same,
    /// `accelerated()` of the vectors of a width, which each function's
    /// code knows as it is compiled.
    Accelerated(Width),
}

impl SimdOp {
    /// Whether the operation may panic: an index or a range out of bounds,
    /// or a shift by a count out of range.
    pub fn may_panic(self) -> bool {
        matches!(
            self,
            SimdOp::Of
                | SimdOp::Load
                | SimdOp::Lane
                | SimdOp::WithLane
                | SimdOp::Store
                | SimdOp::Shl
                | SimdOp::Shr
                | SimdOp::MaskLane
        )
    }
}
