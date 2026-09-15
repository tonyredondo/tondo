//! UInt64 operations over raw 64-bit native carriers. Signed carrier spelling
//! never chooses arithmetic semantics; the compiler supplies unsigned opcodes.

use super::*;

pub(super) fn parse_literal(spelling: &str) -> Result<i64, String> {
    let value = u64::try_from(parse_integer_value(spelling)?)
        .map_err(|_| format!("unsigned scalar integer {spelling} is out of range"))?;
    Ok(i64::from_ne_bytes(value.to_ne_bytes()))
}

pub(super) fn evaluate(operator: &str, left: i64, right: i64) -> Result<i64, String> {
    let left = left as u64;
    let right = right as u64;
    let value = match operator {
        "add" => left.checked_add(right),
        "subtract" => left.checked_sub(right),
        "multiply" => left.checked_mul(right),
        "divide" => left.checked_div(right),
        "remainder" => left.checked_rem(right),
        "shift-right" => (right < 64).then(|| left >> right),
        "less" => Some(u64::from(left < right)),
        "less-equal" => Some(u64::from(left <= right)),
        "greater" => Some(u64::from(left > right)),
        "greater-equal" => Some(u64::from(left >= right)),
        _ => return Err(format!("unknown unsigned scalar operator: {operator}")),
    };
    value
        .map(|value| value as i64)
        .ok_or_else(|| format!("unsigned scalar operation failed: {operator}"))
}

pub(super) fn cranelift(
    builder: &mut FunctionBuilder<'_>,
    operator: &str,
    left: Value,
    right: Value,
) -> Result<Value, String> {
    use cranelift_codegen::ir::types::I64;
    let overflow = match operator {
        "add" => Some(builder.ins().uadd_overflow(left, right)),
        "subtract" => Some(builder.ins().usub_overflow(left, right)),
        "multiply" => Some(builder.ins().umul_overflow(left, right)),
        _ => None,
    };
    if let Some((value, overflow)) = overflow {
        builder.ins().trapnz(overflow, TrapCode::INTEGER_OVERFLOW);
        return Ok(value);
    }
    match operator {
        "divide" | "remainder" => {
            builder
                .ins()
                .trapz(right, TrapCode::INTEGER_DIVISION_BY_ZERO);
            Ok(if operator == "divide" {
                builder.ins().udiv(left, right)
            } else {
                builder.ins().urem(left, right)
            })
        }
        "shift-right" => {
            let invalid = builder
                .ins()
                .icmp_imm(IntCC::UnsignedGreaterThanOrEqual, right, 64);
            builder.ins().trapnz(invalid, TrapCode::unwrap_user(2));
            Ok(builder.ins().ushr(left, right))
        }
        _ => {
            let condition = match operator {
                "less" => IntCC::UnsignedLessThan,
                "less-equal" => IntCC::UnsignedLessThanOrEqual,
                "greater" => IntCC::UnsignedGreaterThan,
                "greater-equal" => IntCC::UnsignedGreaterThanOrEqual,
                _ => return Err(format!("unknown unsigned Cranelift operator: {operator}")),
            };
            let value = builder.ins().icmp(condition, left, right);
            Ok(builder.ins().uextend(I64, value))
        }
    }
}

pub(super) fn llvm_binary(
    operator: &str,
    left: &str,
    right: &str,
    module: &mut String,
    value_index: &mut usize,
) -> Result<String, String> {
    let name = format!("%v{value_index}");
    *value_index += 1;
    let helper = match operator {
        "add" => "uadd",
        "subtract" => "usub",
        "multiply" => "umul",
        "divide" => "udiv",
        "remainder" => "urem",
        "shift-right" => "lshr",
        _ => {
            let predicate = match operator {
                "less" => "ult",
                "less-equal" => "ule",
                "greater" => "ugt",
                "greater-equal" => "uge",
                _ => return Err(format!("unknown unsigned LLVM operator: {operator}")),
            };
            writeln!(module, "  {name} = icmp {predicate} i64 {left}, {right}").unwrap();
            let result = format!("%v{value_index}");
            *value_index += 1;
            writeln!(module, "  {result} = zext i1 {name} to i64").unwrap();
            return Ok(result);
        }
    };
    writeln!(
        module,
        "  {name} = call i64 @tondo_checked_{helper}(i64 {left}, i64 {right})"
    )
    .unwrap();
    Ok(name)
}

pub(super) fn llvm_helpers(module: &mut String) {
    for intrinsic in ["uadd", "usub", "umul"] {
        writeln!(
            module,
            "declare {{ i64, i1 }} @llvm.{intrinsic}.with.overflow.i64(i64, i64)"
        )
        .unwrap();
        llvm_checked_overflow_helper(module, intrinsic, intrinsic);
    }
    for instruction in ["udiv", "urem"] {
        writeln!(
            module,
            "define internal i64 @tondo_checked_{instruction}(i64 %left, i64 %right) {{"
        )
        .unwrap();
        writeln!(module, "entry:").unwrap();
        writeln!(module, "  %zero = icmp eq i64 %right, 0").unwrap();
        llvm_trap_branch(module, "%zero", "zero_trap", "nonzero");
        writeln!(
            module,
            "zero_trap:\n  call void @llvm.trap()\n  unreachable"
        )
        .unwrap();
        writeln!(
            module,
            "nonzero:\n  %result = {instruction} i64 %left, %right\n  ret i64 %result\n}}"
        )
        .unwrap();
    }
    llvm_checked_shift_helper(module, "lshr");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsigned_literals_preserve_all_bits_and_reject_values_outside_the_domain() {
        for spelling in [
            "18446744073709551615",
            "0xffff_ffff_ffff_ffffu64",
            "0o1777777777777777777777u64",
        ] {
            assert_eq!(parse_literal(spelling), Ok(-1), "{spelling}");
            assert!(
                parse_integer_literal(spelling).is_err(),
                "signed literal admitted {spelling}"
            );
        }
        assert_eq!(parse_literal("9223372036854775808u64"), Ok(i64::MIN));
        for spelling in [
            "-1u64",
            "18446744073709551616u64",
            "0x",
            "1unknown",
            "340282366920938463463374607431768211456",
        ] {
            assert!(parse_literal(spelling).is_err(), "{spelling}");
        }
    }

    #[test]
    fn unsigned_operations_distinguish_value_order_overflow_and_logical_shifts() {
        assert_eq!(evaluate("add", i64::MAX, 1), Ok(i64::MIN));
        assert_eq!(evaluate("subtract", i64::MIN, 1), Ok(i64::MAX));
        assert_eq!(evaluate("multiply", -1, 0), Ok(0));
        assert_eq!(evaluate("divide", -1, i64::MIN), Ok(1));
        assert_eq!(evaluate("remainder", i64::MIN, -1), Ok(i64::MIN));
        assert_eq!(evaluate("shift-right", -1, 1), Ok(i64::MAX));
        assert_eq!(evaluate("greater", -1, i64::MAX), Ok(1));
        for (operator, left, right) in [
            ("add", -1, 1),
            ("subtract", 0, 1),
            ("multiply", i64::MIN, 2),
            ("divide", -1, 0),
            ("remainder", -1, 0),
            ("shift-right", -1, -1),
            ("shift-right", -1, 64),
            ("shift-right", -1, 1_i64 << 32),
            ("unknown", 1, 1),
        ] {
            assert!(
                evaluate(operator, left, right).is_err(),
                "{operator}: {left}, {right}"
            );
        }
        let operand = |value: i64| {
            MirBackendOperand::Constant(MirBackendConstant::Integer(value.to_string()))
        };
        assert!(
            evaluate_binary(
                "shift-left",
                &operand(-1),
                &operand(1_i64 << 32),
                &BTreeMap::new()
            )
            .is_err()
        );
    }
}
