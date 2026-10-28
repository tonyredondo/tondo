//! The named operations of the numbers (language 18.9), intrinsics of the
//! compiler: the table the checker resolves their calls with, and the
//! declarations `tondo api` and the editor show for each type.

use crate::types::{FloatTy, IntTy};
use std::sync::OnceLock;

/// A number type with named operations.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Int(IntTy),
    Float(FloatTy),
    Byte,
}

impl Kind {
    pub const ALL: [Kind; 13] = [
        Kind::Int(IntTy::I64),
        Kind::Int(IntTy::I8),
        Kind::Int(IntTy::I16),
        Kind::Int(IntTy::I32),
        Kind::Int(IntTy::I128),
        Kind::Int(IntTy::U64),
        Kind::Int(IntTy::U8),
        Kind::Int(IntTy::U16),
        Kind::Int(IntTy::U32),
        Kind::Int(IntTy::U128),
        Kind::Float(FloatTy::F64),
        Kind::Float(FloatTy::F32),
        Kind::Byte,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Int(int) => int.name(),
            Kind::Float(FloatTy::F64) => "Float",
            Kind::Float(FloatTy::F32) => "Float32",
            Kind::Byte => "Byte",
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum NumOp {
    Parse,
    ToString,
    Compare,
    Min,
    Max,
    Clamp,
    Pow,
    Abs,
    WrappingAdd,
    WrappingSub,
    WrappingMul,
    WrappingNeg,
    WrappingPow,
    CheckedAdd,
    CheckedSub,
    CheckedMul,
    CheckedDiv,
    CheckedRem,
    CheckedNeg,
    CheckedPow,
    SaturatingAdd,
    SaturatingSub,
    SaturatingMul,
    SaturatingPow,
    OverflowingAdd,
    OverflowingSub,
    OverflowingMul,
    CountOnes,
    LeadingZeros,
    TrailingZeros,
    RotateLeft,
    RotateRight,
    ReverseBytes,
    ReverseBits,
    IsPowerOfTwo,
    NextPowerOfTwo,
    ToBytes,
    FromBytes,
    Wrapping,
    Saturating,
    Floor,
    Ceil,
    Round,
    RoundTiesAway,
    Truncate,
    Sqrt,
    IsNaN,
    IsInfinite,
    IsFinite,
    ToFixed,
    Bits,
    FromBits,
    /// What the language's numbers do for `Number.fromInt`: an `Int` as the
    /// number, a panic when it does not fit (18.10).
    FromInt,
    /// `T.min`, `T.max` and `T.bits` of a `T: Integer`.
    Least,
    Greatest,
    Width,
}

impl NumOp {
    /// Whether it can panic: a negative exponent, a result that does not
    /// fit where it panics, a radix out of range, bytes that are not there.
    pub fn may_panic(self) -> bool {
        matches!(
            self,
            NumOp::Pow
                | NumOp::Abs
                | NumOp::WrappingPow
                | NumOp::CheckedPow
                | NumOp::SaturatingPow
                | NumOp::NextPowerOfTwo
                | NumOp::FromBytes
                | NumOp::FromInt
        )
    }
}

/// A parameter's or result's type, in terms of the type that has the
/// operation.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Shape {
    /// The type itself.
    Same,
    Int,
    Bool,
    String,
    /// `Array[Byte]`
    Bytes,
    /// `ByteOrder`
    Order,
    /// Any integer, or a `Byte`.
    Integral,
    /// Any number, or a `Byte`.
    Numeric,
    /// The unsigned integer of a float's width: `UInt` or `UInt32`.
    Pattern,
    /// `T?`
    Optional,
    /// `(T, Bool)`
    Overflowing,
    /// `T ! text.TextError`
    Parsed,
    /// `T ! math.MathError`
    Rooted,
}

/// Which types have an operation.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Owners {
    Integers,
    Signed,
    Unsigned,
    Floats,
    /// The integers and the floats.
    Numbers,
    /// The integers and `Byte`.
    Whole,
    /// Every number and `Byte`.
    All,
}

impl Owners {
    fn have(self, kind: Kind) -> bool {
        match (self, kind) {
            (Owners::All, _) => true,
            (Owners::Integers | Owners::Whole | Owners::Numbers, Kind::Int(_)) => true,
            (Owners::Signed, Kind::Int(int)) => int.signed(),
            (Owners::Unsigned, Kind::Int(int)) => !int.signed(),
            (Owners::Floats | Owners::Numbers, Kind::Float(_)) => true,
            (Owners::Whole, Kind::Byte) => true,
            _ => false,
        }
    }
}

/// An operation: its name, whether it takes `self`, its other parameters
/// with their defaults, its result and its documentation.
pub struct Entry {
    pub name: &'static str,
    pub op: NumOp,
    owners: Owners,
    pub receiver: bool,
    pub params: &'static [(&'static str, Shape, Option<&'static str>)],
    pub result: Shape,
    doc: &'static str,
}

const fn method(
    name: &'static str,
    op: NumOp,
    owners: Owners,
    params: &'static [(&'static str, Shape, Option<&'static str>)],
    result: Shape,
    doc: &'static str,
) -> Entry {
    Entry { name, op, owners, receiver: true, params, result, doc }
}

const fn function(
    name: &'static str,
    op: NumOp,
    owners: Owners,
    params: &'static [(&'static str, Shape, Option<&'static str>)],
    result: Shape,
    doc: &'static str,
) -> Entry {
    Entry { name, op, owners, receiver: false, params, result, doc }
}

use Owners::*;
use Shape::*;

const OTHER: &[(&str, Shape, Option<&str>)] = &[("other", Same, None)];
const EXPONENT: &[(&str, Shape, Option<&str>)] = &[("exponent", Int, None)];
const COUNT: &[(&str, Shape, Option<&str>)] = &[("count", Int, None)];
const NONE: &[(&str, Shape, Option<&str>)] = &[];

pub const ENTRIES: &[Entry] = &[
    function(
        "parse",
        NumOp::Parse,
        Whole,
        &[("text", String, None), ("radix", Int, Some("10"))],
        Parsed,
        "The integer the text writes in `radix` (2 to 36), with an optional sign and `_` between digits; \
         `InvalidData` when it is not a value of the type.",
    ),
    function(
        "parse",
        NumOp::Parse,
        Floats,
        &[("text", String, None)],
        Parsed,
        "The float the text writes, such as `1.5`, `-2e3`, `inf` or `NaN`.",
    ),
    method(
        "toString",
        NumOp::ToString,
        Whole,
        &[("radix", Int, Some("10"))],
        String,
        "The value in `radix` (2 to 36), with lowercase digits and `-` before a negative one.",
    ),
    method(
        "compare",
        NumOp::Compare,
        Whole,
        OTHER,
        Int,
        "Negative, zero or positive as the value goes before, with or after `other`.",
    ),
    method(
        "compare",
        NumOp::Compare,
        Floats,
        OTHER,
        Int,
        "Negative, zero or positive as the value goes before, with or after `other`, with `NaN` after every \
         other value and equal to itself.",
    ),
    method(
        "min",
        NumOp::Min,
        All,
        OTHER,
        Same,
        "The smaller of the two; for a float, the value itself when either is NaN.",
    ),
    method(
        "max",
        NumOp::Max,
        All,
        OTHER,
        Same,
        "The larger of the two; for a float, the value itself when either is NaN.",
    ),
    method(
        "clamp",
        NumOp::Clamp,
        Numbers,
        &[("low", Same, None), ("high", Same, None)],
        Same,
        "The value inside `low` to `high`.",
    ),
    method(
        "pow",
        NumOp::Pow,
        Integers,
        EXPONENT,
        Same,
        "The value raised to a power of zero or more; overflow panics as any arithmetic, and so does a \
         negative exponent.",
    ),
    method("pow", NumOp::Pow, Floats, &[("exponent", Same, None)], Same, "The value raised to `exponent`."),
    method(
        "abs",
        NumOp::Abs,
        Signed,
        NONE,
        Same,
        "The value without its sign; the least value has none in its type, and panics.",
    ),
    method("abs", NumOp::Abs, Floats, NONE, Same, "The value without its sign."),
    method(
        "wrappingAdd",
        NumOp::WrappingAdd,
        Integers,
        OTHER,
        Same,
        "`self + other`, keeping the low bits of a result that does not fit.",
    ),
    method(
        "wrappingSub",
        NumOp::WrappingSub,
        Integers,
        OTHER,
        Same,
        "`self - other`, keeping the low bits of a result that does not fit.",
    ),
    method(
        "wrappingMul",
        NumOp::WrappingMul,
        Integers,
        OTHER,
        Same,
        "`self * other`, keeping the low bits of a result that does not fit.",
    ),
    method(
        "wrappingNeg",
        NumOp::WrappingNeg,
        Integers,
        NONE,
        Same,
        "`-self`, keeping the low bits: the two's complement of an unsigned value.",
    ),
    method(
        "wrappingPow",
        NumOp::WrappingPow,
        Integers,
        EXPONENT,
        Same,
        "The value raised to `exponent`, keeping the low bits of a result that does not fit.",
    ),
    method(
        "checkedAdd",
        NumOp::CheckedAdd,
        Integers,
        OTHER,
        Optional,
        "`self + other`, or `none` when it does not fit.",
    ),
    method(
        "checkedSub",
        NumOp::CheckedSub,
        Integers,
        OTHER,
        Optional,
        "`self - other`, or `none` when it does not fit.",
    ),
    method(
        "checkedMul",
        NumOp::CheckedMul,
        Integers,
        OTHER,
        Optional,
        "`self * other`, or `none` when it does not fit.",
    ),
    method(
        "checkedDiv",
        NumOp::CheckedDiv,
        Integers,
        OTHER,
        Optional,
        "`self / other`, or `none` when `other` is zero or the quotient does not fit.",
    ),
    method(
        "checkedRem",
        NumOp::CheckedRem,
        Integers,
        OTHER,
        Optional,
        "`self % other`, or `none` when `other` is zero.",
    ),
    method("checkedNeg", NumOp::CheckedNeg, Integers, NONE, Optional, "`-self`, or `none` when it does not fit."),
    method(
        "checkedPow",
        NumOp::CheckedPow,
        Integers,
        EXPONENT,
        Optional,
        "The value raised to `exponent`, or `none` when it does not fit.",
    ),
    method(
        "saturatingAdd",
        NumOp::SaturatingAdd,
        Integers,
        OTHER,
        Same,
        "`self + other`, or the least or greatest value when it does not fit.",
    ),
    method(
        "saturatingSub",
        NumOp::SaturatingSub,
        Integers,
        OTHER,
        Same,
        "`self - other`, or the least or greatest value when it does not fit.",
    ),
    method(
        "saturatingMul",
        NumOp::SaturatingMul,
        Integers,
        OTHER,
        Same,
        "`self * other`, or the least or greatest value when it does not fit.",
    ),
    method(
        "saturatingPow",
        NumOp::SaturatingPow,
        Integers,
        EXPONENT,
        Same,
        "The value raised to `exponent`, or the least or greatest value when it does not fit.",
    ),
    method(
        "overflowingAdd",
        NumOp::OverflowingAdd,
        Integers,
        OTHER,
        Overflowing,
        "`wrappingAdd`'s result, and whether `self + other` did not fit.",
    ),
    method(
        "overflowingSub",
        NumOp::OverflowingSub,
        Integers,
        OTHER,
        Overflowing,
        "`wrappingSub`'s result, and whether `self - other` did not fit.",
    ),
    method(
        "overflowingMul",
        NumOp::OverflowingMul,
        Integers,
        OTHER,
        Overflowing,
        "`wrappingMul`'s result, and whether `self * other` did not fit.",
    ),
    method("countOnes", NumOp::CountOnes, Whole, NONE, Int, "The bits at 1."),
    method(
        "leadingZeros",
        NumOp::LeadingZeros,
        Whole,
        NONE,
        Int,
        "The bits at 0 above the highest 1: all of them for 0.",
    ),
    method(
        "trailingZeros",
        NumOp::TrailingZeros,
        Whole,
        NONE,
        Int,
        "The bits at 0 below the lowest 1: all of them for 0.",
    ),
    method(
        "rotateLeft",
        NumOp::RotateLeft,
        Whole,
        COUNT,
        Same,
        "The bits moved `count` places toward the highest, those that leave coming back at the lowest; \
         `count` is taken modulo the width.",
    ),
    method(
        "rotateRight",
        NumOp::RotateRight,
        Whole,
        COUNT,
        Same,
        "The bits moved `count` places toward the lowest, those that leave coming back at the highest; \
         `count` is taken modulo the width.",
    ),
    method(
        "reverseBytes",
        NumOp::ReverseBytes,
        Integers,
        NONE,
        Same,
        "The bytes in the opposite order, which turns big-endian into little-endian and back.",
    ),
    method("reverseBits", NumOp::ReverseBits, Whole, NONE, Same, "The bits in the opposite order."),
    method("isPowerOfTwo", NumOp::IsPowerOfTwo, Unsigned, NONE, Bool, "Whether exactly one bit is 1."),
    method(
        "nextPowerOfTwo",
        NumOp::NextPowerOfTwo,
        Unsigned,
        NONE,
        Same,
        "The least power of two not below the value, 1 for 0; panics when it does not fit.",
    ),
    method(
        "toBytes",
        NumOp::ToBytes,
        Integers,
        &[("order", Order, Some("ByteOrder.Big"))],
        Bytes,
        "The value's bytes, as many as its bits over 8, the highest first unless `order` is `Little`.",
    ),
    function(
        "fromBytes",
        NumOp::FromBytes,
        Integers,
        &[("bytes", Bytes, None), ("at", Int, Some("0")), ("order", Order, Some("ByteOrder.Big"))],
        Same,
        "The value the bytes from `at` write, as many as its bits over 8, the highest first unless `order` \
         is `Little`; panics when they are not there.",
    ),
    function(
        "wrapping",
        NumOp::Wrapping,
        Integers,
        &[("value", Integral, None)],
        Same,
        "The low bits of any integer or `Byte`, in two's complement: `UInt8.wrapping(300)` is 44 and \
         `UInt.wrapping(-1)` is `UInt.max`.",
    ),
    function(
        "saturating",
        NumOp::Saturating,
        Integers,
        &[("value", Numeric, None)],
        Same,
        "The nearest value to any number or `Byte`: `UInt8.saturating(-5)` is 0; a float is truncated toward \
         zero, and NaN gives 0.",
    ),
    method("floor", NumOp::Floor, Floats, NONE, Same, "The greatest integer not above the value: `-1.0` for `-0.5`."),
    method("ceil", NumOp::Ceil, Floats, NONE, Same, "The least integer not below the value: `1.0` for `0.5`."),
    method(
        "round",
        NumOp::Round,
        Floats,
        NONE,
        Same,
        "The nearest integer; a value halfway between goes to the even one.",
    ),
    method(
        "roundTiesAway",
        NumOp::RoundTiesAway,
        Floats,
        NONE,
        Same,
        "The nearest integer; a value halfway between goes away from zero.",
    ),
    method("truncate", NumOp::Truncate, Floats, NONE, Same, "The integer part, toward zero: `-1.0` for `-1.7`."),
    method("sqrt", NumOp::Sqrt, Floats, NONE, Rooted, "The square root; an error for a negative value."),
    method("isNaN", NumOp::IsNaN, Floats, NONE, Bool, "Whether the value is not a number, as `0.0 / 0.0`."),
    method("isInfinite", NumOp::IsInfinite, Floats, NONE, Bool, "Whether the value is infinite, of either sign."),
    method("isFinite", NumOp::IsFinite, Floats, NONE, Bool, "Whether the value is neither infinite nor NaN."),
    method(
        "toFixed",
        NumOp::ToFixed,
        Floats,
        &[("decimals", Int, None)],
        String,
        "The value with exactly `decimals` digits after the point: `2.50`.",
    ),
    method(
        "bits",
        NumOp::Bits,
        Floats,
        NONE,
        Pattern,
        "The value's IEEE 754 bits, as they are: `UInt` for a `Float`, `UInt32` for a `Float32`.",
    ),
    function(
        "fromBits",
        NumOp::FromBits,
        Floats,
        &[("bits", Pattern, None)],
        Same,
        "The float of these IEEE 754 bits, as they are.",
    ),
];

/// The operation `name` of `kind`, a method when `receiver`.
pub fn find(kind: Kind, name: &str, receiver: bool) -> Option<&'static Entry> {
    ENTRIES.iter().find(|e| e.name == name && e.receiver == receiver && e.owners.have(kind))
}

/// The operation `name` every integer has, which a `T: Integer` has too
/// (18.10).
pub fn find_integer(name: &str, receiver: bool) -> Option<&'static Entry> {
    ENTRIES
        .iter()
        .find(|e| e.name == name && e.receiver == receiver && matches!(e.owners, Integers | Whole | Numbers | All))
}

/// The names of the operations of `kind`, as methods or as functions of
/// the type.
pub fn names(kind: Kind) -> impl Iterator<Item = &'static str> {
    ENTRIES.iter().filter(move |e| e.owners.have(kind)).map(|e| e.name)
}

/// The type a shape is for `kind`, as Tondo writes it.
fn written(shape: Shape, kind: Kind) -> std::string::String {
    let own = kind.name();
    match shape {
        Same => own.into(),
        Int => "Int".into(),
        Bool => "Bool".into(),
        String => "String".into(),
        Bytes => "Array[Byte]".into(),
        Order => "ByteOrder".into(),
        Integral => "N".into(),
        Numeric => "N".into(),
        Pattern if kind == Kind::Float(FloatTy::F32) => "UInt32".into(),
        Pattern => "UInt".into(),
        Optional => format!("{own}?"),
        Overflowing => format!("({own}, Bool)"),
        Parsed => format!("{own} ! text.TextError"),
        Rooted => format!("{own} ! math.MathError"),
    }
}

/// The declarations of every operation of every number, as `tondo api`
/// shows the prelude: `pub fn UInt8.countOnes(self): Int`, each with its
/// documentation.
pub fn declarations() -> &'static str {
    static TEXT: OnceLock<std::string::String> = OnceLock::new();
    TEXT.get_or_init(|| {
        let mut text = std::string::String::from(
            "// The operations of the numbers, the same in every width, which the compiler gives (language 18.9).\n",
        );
        for kind in Kind::ALL {
            for entry in ENTRIES.iter().filter(|e| e.owners.have(kind)) {
                let mut params: Vec<std::string::String> = Vec::new();
                if entry.receiver {
                    params.push("self".into());
                }
                for (name, shape, default) in entry.params {
                    let default = default.map(|d| format!(" = {d}")).unwrap_or_default();
                    params.push(format!("{name}: {}{default}", written(*shape, kind)));
                }
                let generic = match entry.params.iter().any(|(_, s, _)| matches!(s, Integral | Numeric)) {
                    true => "[N]",
                    false => "",
                };
                text.push('\n');
                // The documentation in lines of at most 80 characters.
                let mut line = std::string::String::from("///");
                for word in entry.doc.split(' ') {
                    if line.len() + 1 + word.len() > 80 {
                        text.push_str(&line);
                        text.push('\n');
                        line = "///".into();
                    }
                    line.push(' ');
                    line.push_str(word);
                }
                text.push_str(&line);
                text.push_str(&format!(
                    "\npub fn {}.{}{generic}({}): {}\n",
                    kind.name(),
                    entry.name,
                    params.join(", "),
                    written(entry.result, kind)
                ));
            }
        }
        // The arithmetic the compiler gives them (18.10).
        text.push_str("\n// Every number is `Additive` and `Number`, and every integer `Integer` (language 18.10).\n");
        for kind in Kind::ALL.iter().filter(|k| !matches!(k, Kind::Byte)) {
            let traits: &[&str] = match kind {
                Kind::Int(_) => &["Additive", "Number", "Integer"],
                _ => &["Additive", "Number"],
            };
            for name in traits {
                text.push_str(&format!("impl {name} for {}\n", kind.name()));
            }
        }
        text
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_type_has_its_operations() {
        let unsigned = Kind::Int(IntTy::U32);
        assert!(find(unsigned, "nextPowerOfTwo", true).is_some());
        assert!(find(unsigned, "abs", true).is_none());
        assert!(find(Kind::Int(IntTy::I8), "abs", true).is_some());
        assert!(find(Kind::Byte, "wrappingAdd", true).is_none());
        assert!(find(Kind::Float(FloatTy::F32), "bits", true).is_some());
        assert!(find(Kind::Int(IntTy::U128), "parse", false).is_some());
        let text = declarations();
        assert!(text.contains("pub fn UInt.countOnes(self): Int\n"), "{text}");
        assert!(text.contains("pub fn Float32.bits(self): UInt32\n"), "{text}");
        assert!(text.contains("pub fn Int128.checkedMul(self, other: Int128): Int128?\n"), "{text}");
    }
}
