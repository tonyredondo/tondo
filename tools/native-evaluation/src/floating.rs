//! IEEE operations over private raw-bit carriers. Float32 uses the low 32 bits;
//! no integer arithmetic, fast-math flags or evaluation runtime interprets them.

use super::*;
use cranelift_codegen::ir::{MemFlags, condcodes::FloatCC, types};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Width {
    Single,
    Double,
}

impl Width {
    fn ir(self) -> cranelift_codegen::ir::Type {
        match self {
            Self::Single => types::F32,
            Self::Double => types::F64,
        }
    }
    fn llvm(self) -> &'static str {
        match self {
            Self::Single => "float",
            Self::Double => "double",
        }
    }
}

pub(super) fn width(name: &str) -> Option<Width> {
    match name {
        "Float32" => Some(Width::Single),
        "Float" => Some(Width::Double),
        _ => None,
    }
}

pub(super) fn operation(name: &str) -> Option<(Width, &str)> {
    name.strip_prefix("float32-")
        .map(|op| (Width::Single, op))
        .or_else(|| name.strip_prefix("float64-").map(|op| (Width::Double, op)))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum UnaryMath {
    Floor,
    Ceil,
    Round,
    RoundTiesAway,
    Truncate,
    Abs,
}

pub(super) fn unary_math(kind: &str) -> Option<UnaryMath> {
    Some(match kind {
        "host:std.math.floor" => UnaryMath::Floor,
        "host:std.math.ceil" => UnaryMath::Ceil,
        "host:std.math.round" => UnaryMath::Round,
        "host:std.math.roundTiesAway" => UnaryMath::RoundTiesAway,
        "host:std.math.truncate" => UnaryMath::Truncate,
        "host:std.math.abs" => UnaryMath::Abs,
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ScalarMath {
    Unary(UnaryMath),
    SqrtUnchecked,
    Fma,
    Min,
    Max,
}

pub(super) fn math_arguments<'a, T>(
    kind: &str,
    arguments: &'a [T],
) -> Result<Option<(ScalarMath, &'a [T])>, String> {
    let operation = match kind {
        "native-math-sqrt-unchecked" => ScalarMath::SqrtUnchecked,
        "host:std.math.fma" => ScalarMath::Fma,
        "host:std.math.min" => ScalarMath::Min,
        "host:std.math.max" => ScalarMath::Max,
        _ => match unary_math(kind) {
            Some(operation) => ScalarMath::Unary(operation),
            None => return Ok(None),
        },
    };
    let (arity, description) = match operation {
        ScalarMath::Unary(_) | ScalarMath::SqrtUnchecked => (1, "one argument"),
        ScalarMath::Fma => (3, "three arguments"),
        ScalarMath::Min | ScalarMath::Max => (2, "two arguments"),
    };
    if arguments.len() != arity {
        return Err(format!("scalar math call `{kind}` requires {description}"));
    }
    Ok(Some((operation, arguments)))
}

pub(super) fn evaluate_math(operation: ScalarMath, bits: &[i64]) -> i64 {
    if let ScalarMath::Unary(operation) = operation {
        return evaluate_unary_math(operation, bits[0]);
    }
    let first = f64::from_bits(bits[0] as u64);
    if operation == ScalarMath::SqrtUnchecked {
        return first.sqrt().to_bits() as i64;
    }
    let second = f64::from_bits(bits[1] as u64);
    match operation {
        ScalarMath::Fma => first
            .mul_add(second, f64::from_bits(bits[2] as u64))
            .to_bits() as i64,
        ScalarMath::Min | ScalarMath::Max if first == 0.0 && second == 0.0 => {
            if operation == ScalarMath::Min {
                bits[0] | bits[1]
            } else {
                bits[0] & bits[1]
            }
        }
        ScalarMath::Min => first.min(second).to_bits() as i64,
        ScalarMath::Max => first.max(second).to_bits() as i64,
        ScalarMath::Unary(_) | ScalarMath::SqrtUnchecked => unreachable!(),
    }
}

pub(super) fn cranelift_math(
    builder: &mut FunctionBuilder<'_>,
    operation: ScalarMath,
    bits: &[Value],
) -> Value {
    if let ScalarMath::Unary(operation) = operation {
        return cranelift_unary_math(builder, operation, bits[0]);
    }
    let first = decode(builder, Width::Double, bits[0]);
    if operation == ScalarMath::SqrtUnchecked {
        let result = builder.ins().sqrt(first);
        return encode(builder, Width::Double, result);
    }
    let second = decode(builder, Width::Double, bits[1]);
    if operation == ScalarMath::Fma {
        let third = decode(builder, Width::Double, bits[2]);
        let result = builder.ins().fma(first, second, third);
        return encode(builder, Width::Double, result);
    }
    // Cranelift fmin/fmax propagate NaNs. Tondo chooses the numeric operand
    // and specifies the sign of zero independently of the host instruction.
    let first_nan = builder.ins().fcmp(FloatCC::Unordered, first, first);
    let second_nan = builder.ins().fcmp(FloatCC::Unordered, second, second);
    let preference = if operation == ScalarMath::Min {
        FloatCC::LessThan
    } else {
        FloatCC::GreaterThan
    };
    let first_preferred = builder.ins().fcmp(preference, first, second);
    let second_preferred = builder.ins().fcmp(preference, second, first);
    let equal_bits = if operation == ScalarMath::Min {
        builder.ins().bor(bits[0], bits[1])
    } else {
        builder.ins().band(bits[0], bits[1])
    };
    let second_or_equal = builder.ins().select(second_preferred, bits[1], equal_bits);
    let ordered = builder
        .ins()
        .select(first_preferred, bits[0], second_or_equal);
    let second_checked = builder.ins().select(second_nan, bits[0], ordered);
    builder.ins().select(first_nan, bits[1], second_checked)
}

pub(super) fn llvm_math(
    operation: ScalarMath,
    bits: &[String],
    module: &mut String,
    index: &mut usize,
) -> String {
    if let ScalarMath::Unary(operation) = operation {
        return llvm_unary_math(operation, &bits[0], module, index);
    }
    let mut ir = Llvm { module, index };
    let first = ir.decode(Width::Double, &bits[0]);
    if operation == ScalarMath::SqrtUnchecked {
        let result = ir.instruction(format!("call double @llvm.sqrt.f64(double {first})"));
        return ir.encode(Width::Double, &result);
    }
    let second = ir.decode(Width::Double, &bits[1]);
    if operation == ScalarMath::Fma {
        let third = ir.decode(Width::Double, &bits[2]);
        let result = ir.instruction(format!(
            "call double @llvm.fma.f64(double {first}, double {second}, double {third})"
        ));
        return ir.encode(Width::Double, &result);
    }
    let first_nan = ir.instruction(format!("fcmp uno double {first}, {first}"));
    let second_nan = ir.instruction(format!("fcmp uno double {second}, {second}"));
    let predicate = if operation == ScalarMath::Min {
        "olt"
    } else {
        "ogt"
    };
    let first_preferred = ir.instruction(format!("fcmp {predicate} double {first}, {second}"));
    let second_preferred = ir.instruction(format!("fcmp {predicate} double {second}, {first}"));
    let bit_operator = if operation == ScalarMath::Min {
        "or"
    } else {
        "and"
    };
    let equal_bits = ir.instruction(format!("{bit_operator} i64 {}, {}", bits[0], bits[1]));
    let second_or_equal = ir.instruction(format!(
        "select i1 {second_preferred}, i64 {}, i64 {equal_bits}",
        bits[1]
    ));
    let ordered = ir.instruction(format!(
        "select i1 {first_preferred}, i64 {}, i64 {second_or_equal}",
        bits[0]
    ));
    let second_checked = ir.instruction(format!(
        "select i1 {second_nan}, i64 {}, i64 {ordered}",
        bits[0]
    ));
    ir.instruction(format!(
        "select i1 {first_nan}, i64 {}, i64 {second_checked}",
        bits[1]
    ))
}

pub(super) fn evaluate_unary_math(operation: UnaryMath, bits: i64) -> i64 {
    let value = f64::from_bits(bits as u64);
    let result = match operation {
        UnaryMath::Floor => value.floor(),
        UnaryMath::Ceil => value.ceil(),
        UnaryMath::Round => value.round_ties_even(),
        UnaryMath::RoundTiesAway => value.round(),
        UnaryMath::Truncate => value.trunc(),
        UnaryMath::Abs => value.abs(),
    };
    result.to_bits() as i64
}

pub(super) fn cranelift_unary_math(
    builder: &mut FunctionBuilder<'_>,
    operation: UnaryMath,
    bits: Value,
) -> Value {
    let value = decode(builder, Width::Double, bits);
    let result = match operation {
        UnaryMath::Floor => builder.ins().floor(value),
        UnaryMath::Ceil => builder.ins().ceil(value),
        UnaryMath::Round => builder.ins().nearest(value),
        UnaryMath::Truncate => builder.ins().trunc(value),
        UnaryMath::Abs => builder.ins().fabs(value),
        UnaryMath::RoundTiesAway => {
            let nearest = builder.ins().nearest(value);
            let truncated = builder.ins().trunc(value);
            let difference = builder.ins().fsub(value, truncated);
            let magnitude = builder.ins().fabs(difference);
            let half = builder
                .ins()
                .f64const(cranelift_codegen::ir::immediates::Ieee64::with_float(0.5));
            let tie = builder.ins().fcmp(FloatCC::Equal, magnitude, half);
            let one = builder
                .ins()
                .f64const(cranelift_codegen::ir::immediates::Ieee64::with_float(1.0));
            let direction = builder.ins().fcopysign(one, value);
            let away = builder.ins().fadd(truncated, direction);
            builder.ins().select(tie, away, nearest)
        }
    };
    encode(builder, Width::Double, result)
}

pub(super) fn llvm_unary_math(
    operation: UnaryMath,
    bits: &str,
    module: &mut String,
    index: &mut usize,
) -> String {
    let mut ir = Llvm { module, index };
    let value = ir.decode(Width::Double, bits);
    let function = match operation {
        UnaryMath::Floor => "floor",
        UnaryMath::Ceil => "ceil",
        UnaryMath::Round => "roundeven",
        UnaryMath::RoundTiesAway => "round",
        UnaryMath::Truncate => "trunc",
        UnaryMath::Abs => "fabs",
    };
    let result = ir.instruction(format!("call double @llvm.{function}.f64(double {value})"));
    ir.encode(Width::Double, &result)
}

pub(super) fn involved(source: &str, target: &str) -> bool {
    width(source).is_some() || width(target).is_some()
}

pub(super) fn parse_literal(width: Width, spelling: &str) -> Result<i64, String> {
    let spelling = spelling.replace('_', "");
    let text = spelling
        .strip_suffix("f32")
        .or_else(|| spelling.strip_suffix("f64"))
        .unwrap_or(&spelling);
    let invalid = || format!("invalid or non-finite native float literal: {spelling}");
    match width {
        Width::Single => {
            let value = text.parse::<f32>().map_err(|_| invalid())?;
            if !value.is_finite() {
                return Err(invalid());
            }
            Ok(i64::from(value.to_bits()))
        }
        Width::Double => {
            let value = text.parse::<f64>().map_err(|_| invalid())?;
            if !value.is_finite() {
                return Err(invalid());
            }
            Ok(value.to_bits() as i64)
        }
    }
}

pub(super) fn negate(width: Width, operator: &str, value: i64) -> Result<i64, String> {
    if operator != "negate" {
        return Err(format!("unknown float prefix: {operator}"));
    }
    Ok(value ^ sign_mask(width))
}

fn sign_mask(width: Width) -> i64 {
    match width {
        Width::Single => 1_i64 << 31,
        Width::Double => i64::MIN,
    }
}

pub(super) fn evaluate(width: Width, operator: &str, left: i64, right: i64) -> Result<i64, String> {
    macro_rules! calculate {
        ($ty:ty, $bits:ty) => {{
            let left = <$ty>::from_bits(left as $bits);
            let right = <$ty>::from_bits(right as $bits);
            let result = match operator {
                "add" => left + right,
                "subtract" => left - right,
                "multiply" => left * right,
                "divide" => left / right,
                "equal" => return Ok(i64::from(left == right)),
                "not-equal" => return Ok(i64::from(left != right)),
                "less" => return Ok(i64::from(left < right)),
                "less-equal" => return Ok(i64::from(left <= right)),
                "greater" => return Ok(i64::from(left > right)),
                "greater-equal" => return Ok(i64::from(left >= right)),
                _ => return Err(format!("unknown float binary operator: {operator}")),
            };
            Ok(result.to_bits() as i64)
        }};
    }
    match width {
        Width::Single => calculate!(f32, u32),
        Width::Double => calculate!(f64, u64),
    }
}

/// Total machine conversion used by the compiler's checked Result expansion.
/// Float-to-integer saturation is never an implicit source-language conversion.
pub(super) fn convert(source: &str, target: &str, mode: &str, value: i64) -> Result<i64, String> {
    validate_conversion(source, target, mode)?;
    let source_width = width(source);
    let decoded = match source_width {
        Some(Width::Single) => f64::from(f32::from_bits(value as u32)),
        Some(Width::Double) => f64::from_bits(value as u64),
        None => 0.0,
    };
    match width(target) {
        Some(Width::Single) => Ok(i64::from(match source_width {
            Some(_) => (decoded as f32).to_bits(),
            None if source == "UInt64" => (value as u64 as f32).to_bits(),
            None => (value as f32).to_bits(),
        })),
        Some(Width::Double) => Ok(match source_width {
            Some(_) => decoded.to_bits(),
            None if source == "UInt64" => (value as u64 as f64).to_bits(),
            None => (value as f64).to_bits(),
        } as i64),
        None if unsigned_target(target) => Ok(decoded as u64 as i64),
        None => Ok(decoded as i64),
    }
}

fn unsigned_target(target: &str) -> bool {
    target.starts_with("UInt") || target == "Byte"
}

fn validate_conversion(source: &str, target: &str, mode: &str) -> Result<(), String> {
    let numeric = |name| width(name).is_some() || is_native_integer_scalar(name);
    if !numeric(source)
        || !numeric(target)
        || !involved(source, target)
        || !matches!(mode, "identity" | "total")
        || (mode == "identity" && source != target)
    {
        return Err(format!(
            "float conversion requires normalized scalar mode: {source}->{target} ({mode})"
        ));
    }
    Ok(())
}

fn decode(builder: &mut FunctionBuilder<'_>, width: Width, value: Value) -> Value {
    let bits = if width == Width::Single {
        builder.ins().ireduce(types::I32, value)
    } else {
        value
    };
    builder.ins().bitcast(width.ir(), MemFlags::new(), bits)
}

fn encode(builder: &mut FunctionBuilder<'_>, width: Width, value: Value) -> Value {
    let integer = if width == Width::Single {
        types::I32
    } else {
        types::I64
    };
    let bits = builder.ins().bitcast(integer, MemFlags::new(), value);
    if width == Width::Single {
        builder.ins().uextend(types::I64, bits)
    } else {
        bits
    }
}

pub(super) fn cranelift_prefix(
    builder: &mut FunctionBuilder<'_>,
    width: Width,
    op: &str,
    value: Value,
) -> Result<Value, String> {
    if op != "negate" {
        return Err(format!("unknown float prefix: {op}"));
    }
    Ok(builder.ins().bxor_imm(value, sign_mask(width)))
}

pub(super) fn cranelift_binary(
    builder: &mut FunctionBuilder<'_>,
    width: Width,
    op: &str,
    left: Value,
    right: Value,
) -> Result<Value, String> {
    let left = decode(builder, width, left);
    let right = decode(builder, width, right);
    let result = match op {
        "add" => builder.ins().fadd(left, right),
        "subtract" => builder.ins().fsub(left, right),
        "multiply" => builder.ins().fmul(left, right),
        "divide" => builder.ins().fdiv(left, right),
        _ => {
            let condition = match op {
                "equal" => FloatCC::Equal,
                "not-equal" => FloatCC::NotEqual,
                "less" => FloatCC::LessThan,
                "less-equal" => FloatCC::LessThanOrEqual,
                "greater" => FloatCC::GreaterThan,
                "greater-equal" => FloatCC::GreaterThanOrEqual,
                _ => return Err(format!("unknown float binary operator: {op}")),
            };
            let result = builder.ins().fcmp(condition, left, right);
            return Ok(builder.ins().uextend(types::I64, result));
        }
    };
    Ok(encode(builder, width, result))
}

pub(super) fn cranelift_convert(
    builder: &mut FunctionBuilder<'_>,
    source: &str,
    target: &str,
    mode: &str,
    value: Value,
) -> Result<Value, String> {
    validate_conversion(source, target, mode)?;
    let source_width = width(source);
    let target_width = width(target);
    let value = source_width
        .map(|width| decode(builder, width, value))
        .unwrap_or(value);
    let converted = match (source_width, target_width) {
        (Some(a), Some(b)) if a == b => value,
        (Some(Width::Single), Some(Width::Double)) => builder.ins().fpromote(types::F64, value),
        (Some(_), Some(_)) => builder.ins().fdemote(types::F32, value),
        (None, Some(b)) if source == "UInt64" => builder.ins().fcvt_from_uint(b.ir(), value),
        (None, Some(b)) => builder.ins().fcvt_from_sint(b.ir(), value),
        (Some(_), None) if unsigned_target(target) => {
            builder.ins().fcvt_to_uint_sat(types::I64, value)
        }
        (Some(_), None) => builder.ins().fcvt_to_sint_sat(types::I64, value),
        (None, None) => return Err("float conversion has no float operand".to_owned()),
    };
    Ok(target_width
        .map(|width| encode(builder, width, converted))
        .unwrap_or(converted))
}

struct Llvm<'a> {
    module: &'a mut String,
    index: &'a mut usize,
}
impl Llvm<'_> {
    fn instruction(&mut self, operation: String) -> String {
        let name = format!("%v{}", self.index);
        *self.index += 1;
        writeln!(self.module, "  {name} = {operation}").unwrap();
        name
    }
    fn decode(&mut self, width: Width, value: &str) -> String {
        if width == Width::Single {
            let bits = self.instruction(format!("trunc i64 {value} to i32"));
            self.instruction(format!("bitcast i32 {bits} to float"))
        } else {
            self.instruction(format!("bitcast i64 {value} to double"))
        }
    }
    fn encode(&mut self, width: Width, value: &str) -> String {
        if width == Width::Single {
            let bits = self.instruction(format!("bitcast float {value} to i32"));
            self.instruction(format!("zext i32 {bits} to i64"))
        } else {
            self.instruction(format!("bitcast double {value} to i64"))
        }
    }
}

pub(super) fn llvm_prefix(
    width: Width,
    op: &str,
    value: &str,
    module: &mut String,
    index: &mut usize,
) -> Result<String, String> {
    if op != "negate" {
        return Err(format!("unknown float prefix: {op}"));
    }
    Ok(Llvm { module, index }.instruction(format!("xor i64 {value}, {}", sign_mask(width))))
}

pub(super) fn llvm_binary(
    width: Width,
    op: &str,
    left: &str,
    right: &str,
    module: &mut String,
    index: &mut usize,
) -> Result<String, String> {
    let mut ir = Llvm { module, index };
    let left = ir.decode(width, left);
    let right = ir.decode(width, right);
    let ty = width.llvm();
    let instruction = match op {
        "add" => "fadd",
        "subtract" => "fsub",
        "multiply" => "fmul",
        "divide" => "fdiv",
        _ => {
            let predicate = match op {
                "equal" => "oeq",
                "not-equal" => "une",
                "less" => "olt",
                "less-equal" => "ole",
                "greater" => "ogt",
                "greater-equal" => "oge",
                _ => return Err(format!("unknown float binary operator: {op}")),
            };
            let result = ir.instruction(format!("fcmp {predicate} {ty} {left}, {right}"));
            return Ok(ir.instruction(format!("zext i1 {result} to i64")));
        }
    };
    let result = ir.instruction(format!("{instruction} {ty} {left}, {right}"));
    Ok(ir.encode(width, &result))
}

pub(super) fn llvm_convert(
    source: &str,
    target: &str,
    mode: &str,
    value: &str,
    module: &mut String,
    index: &mut usize,
) -> Result<String, String> {
    validate_conversion(source, target, mode)?;
    let mut ir = Llvm { module, index };
    let a = width(source);
    let b = width(target);
    let value = a
        .map(|width| ir.decode(width, value))
        .unwrap_or_else(|| value.to_owned());
    let converted = match (a, b) {
        (Some(a), Some(b)) if a == b => value,
        (Some(a), Some(b)) => ir.instruction(format!(
            "{} {} {value} to {}",
            if a == Width::Single {
                "fpext"
            } else {
                "fptrunc"
            },
            a.llvm(),
            b.llvm()
        )),
        (None, Some(b)) => ir.instruction(format!(
            "{} i64 {value} to {}",
            if source == "UInt64" {
                "uitofp"
            } else {
                "sitofp"
            },
            b.llvm()
        )),
        (Some(a), None) => ir.instruction(format!(
            "call i64 @llvm.fpto{}.sat.i64.f{}({} {value})",
            if unsigned_target(target) { "ui" } else { "si" },
            if a == Width::Single { 32 } else { 64 },
            a.llvm()
        )),
        (None, None) => return Err("float conversion has no float operand".to_owned()),
    };
    Ok(b.map(|width| ir.encode(width, &converted))
        .unwrap_or(converted))
}

pub(super) fn llvm_helpers(module: &mut String) {
    writeln!(module, "declare double @llvm.sqrt.f64(double)").unwrap();
    writeln!(
        module,
        "declare double @llvm.fma.f64(double, double, double)"
    )
    .unwrap();
    for name in ["floor", "ceil", "roundeven", "round", "trunc", "fabs"] {
        writeln!(module, "declare double @llvm.{name}.f64(double)").unwrap();
    }
    for (bits, ty) in [(32, "float"), (64, "double")] {
        for sign in ["si", "ui"] {
            writeln!(module, "declare i64 @llvm.fpto{sign}.sat.i64.f{bits}({ty})").unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_round_directly_to_their_format_and_keep_signed_zero() {
        assert_eq!(
            parse_literal(Width::Single, "1.0000000596046448"),
            Ok(0x3f80_0001)
        );
        assert_eq!(parse_literal(Width::Single, "-0.0f32"), Ok(1_i64 << 31));
        assert_eq!(parse_literal(Width::Double, "-0.0f64"), Ok(i64::MIN));
        assert_eq!(
            parse_literal(Width::Double, "1_000.5"),
            Ok(1000.5_f64.to_bits() as i64)
        );
        for (width, text) in [
            (Width::Single, "3.5e38"),
            (Width::Double, "1e309"),
            (Width::Single, "NaN"),
            (Width::Double, "inf"),
            (Width::Single, "1.0unknown"),
        ] {
            assert!(parse_literal(width, text).is_err(), "{text}");
        }
    }

    #[test]
    fn ieee_operations_preserve_nan_order_subnormals_and_separate_rounding() {
        for width in [Width::Single, Width::Double] {
            let one = parse_literal(width, "1.0").unwrap();
            let negative_zero = negate(width, "negate", 0).unwrap();
            let inf = evaluate(width, "divide", one, negative_zero).unwrap();
            assert_eq!(evaluate(width, "less", inf, 0), Ok(1));
            let nan = evaluate(width, "divide", 0, 0).unwrap();
            for op in ["equal", "less", "less-equal", "greater", "greater-equal"] {
                assert_eq!(evaluate(width, op, nan, nan), Ok(0), "{width:?}: {op}");
            }
            assert_eq!(evaluate(width, "not-equal", nan, nan), Ok(1));
            assert_eq!(evaluate(width, "add", 1, 1), Ok(2));
            assert!(evaluate(width, "remainder", one, one).is_err());
            assert!(negate(width, "logical-not", one).is_err());
        }
        let left = i64::from((1.0_f32 + f32::EPSILON).to_bits());
        let right = i64::from((1.0_f32 - f32::EPSILON).to_bits());
        let product = evaluate(Width::Single, "multiply", left, right).unwrap();
        assert_eq!(
            evaluate(
                Width::Single,
                "add",
                product,
                i64::from((-1.0_f32).to_bits())
            ),
            Ok(0)
        );
    }

    #[test]
    fn normalized_conversions_preserve_unsigned_ranges_and_reject_opaque_modes() {
        assert_eq!(
            convert("UInt64", "Float", "total", -1),
            Ok((u64::MAX as f64).to_bits() as i64)
        );
        assert_eq!(
            convert("UInt64", "Float32", "total", -1),
            Ok(i64::from((u64::MAX as f32).to_bits()))
        );
        assert_eq!(
            convert("Float32", "Float", "total", 1_i64 << 31),
            Ok(i64::MIN)
        );
        assert_eq!(
            convert("Float", "Float32", "total", i64::MIN),
            Ok(1_i64 << 31)
        );
        for target in [
            "Int", "Int8", "Int16", "Int32", "UInt64", "UInt32", "UInt16", "UInt8", "Byte",
        ] {
            assert_eq!(
                convert("Float", target, "total", 42.0_f64.to_bits() as i64),
                Ok(42)
            );
        }
        // The classifier may safely evaluate these candidates before deciding
        // NotFinite/NotIntegral/OutOfRange. They never trap or produce poison.
        assert_eq!(
            convert("Float", "Int", "total", f64::NAN.to_bits() as i64),
            Ok(0)
        );
        assert_eq!(
            convert("Float", "UInt64", "total", f64::INFINITY.to_bits() as i64),
            Ok(-1)
        );
        for (source, target, mode) in [
            ("Float", "Int", "checked"),
            ("Float", "Float32", "identity"),
            ("Char", "Float", "total"),
            ("Int", "Int", "total"),
            ("Float", "Int", "unknown"),
        ] {
            assert!(
                convert(source, target, mode, 0).is_err(),
                "{source}->{target} {mode}"
            );
        }
    }
}
