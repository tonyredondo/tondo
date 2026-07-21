use std::collections::BTreeMap;

use crate::source::Span;
use crate::types::{
    Assignability, IntrinsicType, NumericConversion, ScalarType, TypeError, TypeId, TypeKind,
};

use super::{
    HirBinaryOperator, HirConstantFieldValue, HirConstantValue, HirConstantValueKind,
    HirConstantVariantValue, HirContainmentKind, HirExpressionId, HirExpressionKind,
    HirIndexAccess, HirLiteral, HirPrefixOperator, HirProgram, HirRangeKind, HirVariantValue,
};

#[derive(Debug)]
pub(super) enum ConstantEvaluationError {
    Nonconstant { span: Span, reason: &'static str },
    Panic { span: Span, reason: String },
    Unavailable,
    Type(TypeError),
}

impl From<TypeError> for ConstantEvaluationError {
    fn from(error: TypeError) -> Self {
        Self::Type(error)
    }
}

enum Work {
    Enter(HirExpressionId),
    Finish(HirExpressionId),
    FinishLogical(HirExpressionId),
}

pub(super) fn evaluate(
    program: &HirProgram,
    root: HirExpressionId,
) -> Result<HirConstantValue, ConstantEvaluationError> {
    let mut values = BTreeMap::<HirExpressionId, HirConstantValue>::new();
    let mut pending = vec![Work::Enter(root)];
    while let Some(work) = pending.pop() {
        match work {
            Work::Enter(id) => {
                if values.contains_key(&id) {
                    continue;
                }
                let expression = program
                    .expression(id)
                    .ok_or(ConstantEvaluationError::Unavailable)?;
                match expression.kind() {
                    HirExpressionKind::Recovery => {
                        return Err(ConstantEvaluationError::Unavailable);
                    }
                    HirExpressionKind::Literal(literal) => {
                        values.insert(id, evaluate_literal(program, expression.ty(), literal)?);
                    }
                    HirExpressionKind::Constant(symbol) => {
                        let value = program
                            .constant(*symbol)
                            .and_then(|constant| constant.evaluated())
                            .cloned()
                            .ok_or(ConstantEvaluationError::Unavailable)?;
                        values.insert(id, value);
                    }
                    HirExpressionKind::Function(callable) => {
                        let signature = program
                            .callable(*callable)
                            .ok_or(ConstantEvaluationError::Unavailable)?;
                        if !signature.generics().is_empty() {
                            return Err(ConstantEvaluationError::Nonconstant {
                                span: expression.span(),
                                reason: "a generic function value must be fully specialized",
                            });
                        }
                        values.insert(
                            id,
                            constant_value(
                                expression.ty(),
                                HirConstantValueKind::Function {
                                    callable: *callable,
                                    arguments: Vec::new(),
                                },
                            ),
                        );
                    }
                    HirExpressionKind::SpecializedFunction {
                        callable,
                        arguments,
                    } => {
                        values.insert(
                            id,
                            constant_value(
                                expression.ty(),
                                HirConstantValueKind::Function {
                                    callable: *callable,
                                    arguments: arguments.clone(),
                                },
                            ),
                        );
                    }
                    HirExpressionKind::Binary {
                        operator: HirBinaryOperator::LogicalAnd | HirBinaryOperator::LogicalOr,
                        left,
                        ..
                    } => {
                        pending.push(Work::FinishLogical(id));
                        pending.push(Work::Enter(*left));
                    }
                    HirExpressionKind::Local(_)
                    | HirExpressionKind::Receiver
                    | HirExpressionKind::InterpolatedString { .. }
                    | HirExpressionKind::Block { .. }
                    | HirExpressionKind::Call { .. }
                    | HirExpressionKind::PreludePanic { .. }
                    | HirExpressionKind::PreludeAssert { .. }
                    | HirExpressionKind::BootstrapHostCall { .. }
                    | HirExpressionKind::PropagateOption { .. }
                    | HirExpressionKind::PropagateResult { .. }
                    | HirExpressionKind::If { .. }
                    | HirExpressionKind::Match { .. }
                    | HirExpressionKind::Return { .. }
                    | HirExpressionKind::Fail { .. }
                    | HirExpressionKind::Break { .. }
                    | HirExpressionKind::Continue { .. } => {
                        return Err(ConstantEvaluationError::Nonconstant {
                            span: expression.span(),
                            reason: "this expression requires runtime evaluation",
                        });
                    }
                    _ => {
                        pending.push(Work::Finish(id));
                        let children = constant_children(expression.kind());
                        pending.extend(children.into_iter().rev().map(Work::Enter));
                    }
                }
            }
            Work::FinishLogical(id) => {
                let expression = program
                    .expression(id)
                    .ok_or(ConstantEvaluationError::Unavailable)?;
                let HirExpressionKind::Binary {
                    operator,
                    left,
                    right,
                } = expression.kind()
                else {
                    unreachable!("logical work is created only for logical binary expressions");
                };
                let left = values
                    .get(left)
                    .ok_or(ConstantEvaluationError::Unavailable)?;
                let HirConstantValueKind::Bool(left) = left.kind() else {
                    return Err(ConstantEvaluationError::Unavailable);
                };
                let short_circuited = match operator {
                    HirBinaryOperator::LogicalAnd if !left => Some(false),
                    HirBinaryOperator::LogicalOr if *left => Some(true),
                    HirBinaryOperator::LogicalAnd | HirBinaryOperator::LogicalOr => None,
                    _ => unreachable!("logical work retains a logical operator"),
                };
                if let Some(result) = short_circuited {
                    values.insert(
                        id,
                        constant_value(expression.ty(), HirConstantValueKind::Bool(result)),
                    );
                } else {
                    pending.push(Work::Finish(id));
                    pending.push(Work::Enter(*right));
                }
            }
            Work::Finish(id) => {
                let expression = program
                    .expression(id)
                    .ok_or(ConstantEvaluationError::Unavailable)?;
                let value = evaluate_composite(
                    program,
                    expression.ty(),
                    expression.span(),
                    expression.kind(),
                    &values,
                )?;
                values.insert(id, value);
            }
        }
    }
    values
        .remove(&root)
        .ok_or(ConstantEvaluationError::Unavailable)
}

pub(super) fn has_unavailable_input(program: &HirProgram, root: HirExpressionId) -> bool {
    let mut pending = vec![root];
    let mut visited = BTreeMap::<HirExpressionId, ()>::new();
    while let Some(id) = pending.pop() {
        if visited.insert(id, ()).is_some() {
            continue;
        }
        let Some(expression) = program.expression(id) else {
            return true;
        };
        match expression.kind() {
            HirExpressionKind::Recovery => return true,
            HirExpressionKind::Constant(symbol)
                if program
                    .constant(*symbol)
                    .is_none_or(|constant| constant.evaluated().is_none()) =>
            {
                return true;
            }
            kind => pending.extend(constant_children(kind)),
        }
    }
    false
}

fn constant_children(kind: &HirExpressionKind) -> Vec<HirExpressionId> {
    match kind {
        HirExpressionKind::Tuple(items)
        | HirExpressionKind::Array(items)
        | HirExpressionKind::Set(items) => items.clone(),
        HirExpressionKind::Map(entries) => entries
            .iter()
            .flat_map(|entry| [entry.key(), entry.value()])
            .collect(),
        HirExpressionKind::Newtype { value, .. }
        | HirExpressionKind::NumericConversion { value, .. }
        | HirExpressionKind::Prefix { operand: value, .. }
        | HirExpressionKind::Field { base: value, .. }
        | HirExpressionKind::TupleField { base: value, .. }
        | HirExpressionKind::OptionSome { value }
        | HirExpressionKind::ResultOk { value }
        | HirExpressionKind::ResultErr { error: value }
        | HirExpressionKind::Coerce { value, .. } => vec![*value],
        HirExpressionKind::Record { fields, .. } => {
            fields.iter().map(|field| field.value()).collect()
        }
        HirExpressionKind::Variant { payload, .. } => match payload {
            HirVariantValue::Unit => Vec::new(),
            HirVariantValue::Tuple(values) => values.clone(),
            HirVariantValue::Record(fields) => fields.iter().map(|field| field.value()).collect(),
        },
        HirExpressionKind::RecordUpdate { base, fields } => std::iter::once(*base)
            .chain(fields.iter().map(|field| field.value()))
            .collect(),
        HirExpressionKind::Binary { left, right, .. } => vec![*left, *right],
        HirExpressionKind::Range { start, end, .. } => vec![*start, *end],
        HirExpressionKind::Contains {
            item, container, ..
        } => vec![*item, *container],
        HirExpressionKind::Index { base, index, .. } => vec![*base, *index],
        HirExpressionKind::Slice {
            base,
            start,
            end,
            step,
        } => std::iter::once(*base)
            .chain(start.iter().copied())
            .chain(end.iter().copied())
            .chain(step.iter().copied())
            .collect(),
        HirExpressionKind::PreludePanic { message } => vec![*message],
        HirExpressionKind::PreludeAssert {
            condition,
            message_parts,
        } => std::iter::once(*condition)
            .chain(message_parts.iter().map(|part| part.value()))
            .collect(),
        HirExpressionKind::BootstrapHostCall { arguments, .. } => arguments.clone(),
        HirExpressionKind::Recovery
        | HirExpressionKind::Literal(_)
        | HirExpressionKind::InterpolatedString { .. }
        | HirExpressionKind::Local(_)
        | HirExpressionKind::Constant(_)
        | HirExpressionKind::Function(_)
        | HirExpressionKind::SpecializedFunction { .. }
        | HirExpressionKind::Receiver
        | HirExpressionKind::Block { .. }
        | HirExpressionKind::Call { .. }
        | HirExpressionKind::PropagateOption { .. }
        | HirExpressionKind::PropagateResult { .. }
        | HirExpressionKind::If { .. }
        | HirExpressionKind::Match { .. }
        | HirExpressionKind::Return { .. }
        | HirExpressionKind::Fail { .. }
        | HirExpressionKind::Break { .. }
        | HirExpressionKind::Continue { .. } => Vec::new(),
    }
}

fn evaluate_literal(
    program: &HirProgram,
    ty: TypeId,
    literal: &HirLiteral,
) -> Result<HirConstantValue, ConstantEvaluationError> {
    let kind = match literal {
        HirLiteral::Unit => HirConstantValueKind::Unit,
        HirLiteral::Bool(value) => HirConstantValueKind::Bool(*value),
        HirLiteral::Integer(spelling) => HirConstantValueKind::Integer(
            integer_magnitude(spelling).ok_or(ConstantEvaluationError::Unavailable)? as i128,
        ),
        HirLiteral::Float(spelling) => {
            let normalized = numeric_body(spelling);
            let value = parse_float_literal(&normalized, scalar(program, ty)?)
                .ok_or(ConstantEvaluationError::Unavailable)?;
            HirConstantValueKind::Float(value.to_bits())
        }
        HirLiteral::Char(spelling) => HirConstantValueKind::Char(
            decode_char_literal(spelling).ok_or(ConstantEvaluationError::Unavailable)?,
        ),
        HirLiteral::String(spelling) => HirConstantValueKind::String(
            decode_string_literal(spelling).ok_or(ConstantEvaluationError::Unavailable)?,
        ),
        HirLiteral::None => HirConstantValueKind::OptionNone,
    };
    Ok(constant_value(ty, kind))
}

fn constant_value(ty: TypeId, kind: HirConstantValueKind) -> HirConstantValue {
    HirConstantValue { ty, kind }
}

fn evaluate_composite(
    program: &HirProgram,
    ty: TypeId,
    span: Span,
    kind: &HirExpressionKind,
    values: &BTreeMap<HirExpressionId, HirConstantValue>,
) -> Result<HirConstantValue, ConstantEvaluationError> {
    let value = |id: HirExpressionId| {
        values
            .get(&id)
            .cloned()
            .ok_or(ConstantEvaluationError::Unavailable)
    };
    let result = match kind {
        HirExpressionKind::Tuple(items) => HirConstantValueKind::Tuple(
            items
                .iter()
                .map(|item| value(*item))
                .collect::<Result<_, _>>()?,
        ),
        HirExpressionKind::Array(items) => HirConstantValueKind::Array(
            items
                .iter()
                .map(|item| value(*item))
                .collect::<Result<_, _>>()?,
        ),
        HirExpressionKind::Map(entries) => HirConstantValueKind::Map(
            entries
                .iter()
                .map(|entry| Ok((value(entry.key())?, value(entry.value())?)))
                .collect::<Result<_, ConstantEvaluationError>>()?,
        ),
        HirExpressionKind::Set(items) => {
            let mut unique = Vec::new();
            for item in items {
                let item = value(*item)?;
                let mut duplicate = false;
                for previous in &unique {
                    if values_equal(program, previous, &item)? {
                        duplicate = true;
                        break;
                    }
                }
                if !duplicate {
                    unique.push(item);
                }
            }
            HirConstantValueKind::Set(unique)
        }
        HirExpressionKind::Newtype {
            constructor,
            value: inner,
        } => HirConstantValueKind::Newtype {
            constructor: *constructor,
            value: Box::new(value(*inner)?),
        },
        HirExpressionKind::Record { owner, fields } => HirConstantValueKind::Record {
            owner: *owner,
            fields: fields
                .iter()
                .map(|field| {
                    Ok(HirConstantFieldValue {
                        member: field.member(),
                        value: value(field.value())?,
                    })
                })
                .collect::<Result<_, ConstantEvaluationError>>()?,
        },
        HirExpressionKind::Variant { variant, payload } => HirConstantValueKind::Variant {
            variant: *variant,
            payload: match payload {
                HirVariantValue::Unit => HirConstantVariantValue::Unit,
                HirVariantValue::Tuple(items) => HirConstantVariantValue::Tuple(
                    items
                        .iter()
                        .map(|item| value(*item))
                        .collect::<Result<_, _>>()?,
                ),
                HirVariantValue::Record(fields) => HirConstantVariantValue::Record(
                    fields
                        .iter()
                        .map(|field| {
                            Ok(HirConstantFieldValue {
                                member: field.member(),
                                value: value(field.value())?,
                            })
                        })
                        .collect::<Result<_, ConstantEvaluationError>>()?,
                ),
            },
        },
        HirExpressionKind::RecordUpdate { base, fields } => {
            let base = value(*base)?;
            let HirConstantValueKind::Record {
                owner,
                fields: base_fields,
            } = base.kind
            else {
                return Err(ConstantEvaluationError::Unavailable);
            };
            let mut updated = base_fields;
            for field in fields {
                let replacement = value(field.value())?;
                let Some(existing) = updated
                    .iter_mut()
                    .find(|existing| existing.member == field.member())
                else {
                    return Err(ConstantEvaluationError::Unavailable);
                };
                existing.value = replacement;
            }
            HirConstantValueKind::Record {
                owner,
                fields: updated,
            }
        }
        HirExpressionKind::NumericConversion {
            target,
            conversion,
            value: inner,
        } => {
            return evaluate_numeric_conversion(
                program,
                ty,
                span,
                *target,
                *conversion,
                value(*inner)?,
            );
        }
        HirExpressionKind::Prefix { operator, operand } => {
            return evaluate_prefix(program, ty, span, *operator, value(*operand)?);
        }
        HirExpressionKind::Binary {
            operator,
            left,
            right,
        } => {
            return evaluate_binary(program, ty, span, *operator, value(*left)?, value(*right)?);
        }
        HirExpressionKind::Range { kind, start, end } => HirConstantValueKind::Range {
            kind: *kind,
            start: Box::new(value(*start)?),
            end: Box::new(value(*end)?),
        },
        HirExpressionKind::Contains {
            kind,
            item,
            container,
        } => HirConstantValueKind::Bool(evaluate_contains(
            program,
            *kind,
            &value(*item)?,
            &value(*container)?,
        )?),
        HirExpressionKind::Field { base, member } => {
            let base = value(*base)?;
            match base.kind {
                HirConstantValueKind::Newtype { value, .. } => return Ok(*value),
                HirConstantValueKind::Record { fields, .. } => {
                    return fields
                        .into_iter()
                        .find(|field| field.member == *member)
                        .map(|field| field.value)
                        .ok_or(ConstantEvaluationError::Unavailable);
                }
                _ => return Err(ConstantEvaluationError::Unavailable),
            }
        }
        HirExpressionKind::TupleField { base, index } => {
            let base = value(*base)?;
            let HirConstantValueKind::Tuple(items) = base.kind else {
                return Err(ConstantEvaluationError::Unavailable);
            };
            return items
                .into_iter()
                .nth(*index as usize)
                .ok_or(ConstantEvaluationError::Unavailable);
        }
        HirExpressionKind::Index {
            base,
            index,
            access,
        } => {
            return evaluate_index(program, ty, span, *access, value(*base)?, value(*index)?);
        }
        HirExpressionKind::Slice {
            base,
            start,
            end,
            step,
        } => {
            return evaluate_slice(
                ty,
                span,
                value(*base)?,
                start.map(&value).transpose()?,
                end.map(&value).transpose()?,
                step.map(&value).transpose()?,
            );
        }
        HirExpressionKind::OptionSome { value: inner } => {
            HirConstantValueKind::OptionSome(Box::new(value(*inner)?))
        }
        HirExpressionKind::ResultOk { value: inner } => {
            HirConstantValueKind::ResultOk(Box::new(value(*inner)?))
        }
        HirExpressionKind::ResultErr { error } => {
            HirConstantValueKind::ResultErr(Box::new(value(*error)?))
        }
        HirExpressionKind::Coerce { kind, value: inner } => {
            let mut inner = value(*inner)?;
            match kind {
                Assignability::Exact => {
                    inner.ty = ty;
                    return Ok(inner);
                }
                Assignability::OptionLift => HirConstantValueKind::OptionSome(Box::new(inner)),
                Assignability::UnionInjection | Assignability::UnionWidening => {
                    HirConstantValueKind::Converted(Box::new(inner))
                }
                Assignability::Diverging => {
                    return Err(ConstantEvaluationError::Unavailable);
                }
            }
        }
        HirExpressionKind::Recovery
        | HirExpressionKind::Literal(_)
        | HirExpressionKind::InterpolatedString { .. }
        | HirExpressionKind::Local(_)
        | HirExpressionKind::Constant(_)
        | HirExpressionKind::Function(_)
        | HirExpressionKind::SpecializedFunction { .. }
        | HirExpressionKind::Receiver
        | HirExpressionKind::Block { .. }
        | HirExpressionKind::Call { .. }
        | HirExpressionKind::PreludePanic { .. }
        | HirExpressionKind::PreludeAssert { .. }
        | HirExpressionKind::BootstrapHostCall { .. }
        | HirExpressionKind::PropagateOption { .. }
        | HirExpressionKind::PropagateResult { .. }
        | HirExpressionKind::If { .. }
        | HirExpressionKind::Match { .. }
        | HirExpressionKind::Return { .. }
        | HirExpressionKind::Fail { .. }
        | HirExpressionKind::Break { .. }
        | HirExpressionKind::Continue { .. } => {
            return Err(ConstantEvaluationError::Unavailable);
        }
    };
    Ok(constant_value(ty, result))
}

fn evaluate_prefix(
    program: &HirProgram,
    ty: TypeId,
    span: Span,
    operator: HirPrefixOperator,
    operand: HirConstantValue,
) -> Result<HirConstantValue, ConstantEvaluationError> {
    let kind = match (operator, operand.kind) {
        (HirPrefixOperator::LogicalNot, HirConstantValueKind::Bool(value)) => {
            HirConstantValueKind::Bool(!value)
        }
        (HirPrefixOperator::Negate, HirConstantValueKind::Integer(value)) => {
            let scalar = scalar(program, ty)?;
            let result = value
                .checked_neg()
                .filter(|value| integer_fits(*value, scalar))
                .ok_or_else(|| panic_error(span, "integer negation overflows"))?;
            HirConstantValueKind::Integer(result)
        }
        (HirPrefixOperator::Negate, HirConstantValueKind::Float(bits)) => {
            HirConstantValueKind::Float(
                round_float(-f64::from_bits(bits), scalar(program, ty)?).to_bits(),
            )
        }
        (HirPrefixOperator::BitwiseNot, HirConstantValueKind::Integer(value)) => {
            let scalar = scalar(program, ty)?;
            let (_, bits) = integer_shape(scalar).ok_or(ConstantEvaluationError::Unavailable)?;
            HirConstantValueKind::Integer(integer_from_bits(!integer_to_bits(value, bits), scalar)?)
        }
        _ => return Err(ConstantEvaluationError::Unavailable),
    };
    Ok(constant_value(ty, kind))
}

fn evaluate_binary(
    program: &HirProgram,
    ty: TypeId,
    span: Span,
    operator: HirBinaryOperator,
    left: HirConstantValue,
    right: HirConstantValue,
) -> Result<HirConstantValue, ConstantEvaluationError> {
    enum LiftWork<'a> {
        Apply(&'a HirConstantValue, &'a HirConstantValue, TypeId),
        FinishArray { ty: TypeId, length: usize },
    }

    let mut work = vec![LiftWork::Apply(&left, &right, ty)];
    let mut results = Vec::<HirConstantValue>::new();
    while let Some(current) = work.pop() {
        match current {
            LiftWork::FinishArray { ty, length } => {
                let start = results
                    .len()
                    .checked_sub(length)
                    .ok_or(ConstantEvaluationError::Unavailable)?;
                let items = results.split_off(start);
                results.push(constant_value(ty, HirConstantValueKind::Array(items)));
            }
            LiftWork::Apply(left, right, result_ty) => {
                let left_array = match left.kind() {
                    HirConstantValueKind::Array(items) => Some(items.as_slice()),
                    _ => None,
                };
                let right_array = match right.kind() {
                    HirConstantValueKind::Array(items) => Some(items.as_slice()),
                    _ => None,
                };
                if left_array.is_some() || right_array.is_some() {
                    let element_ty = intrinsic_element(program, result_ty, IntrinsicType::Array)?;
                    let length = match (left_array, right_array) {
                        (Some(left), Some(right)) if left.len() != right.len() => {
                            return Err(panic_error(span, "array operands have different shapes"));
                        }
                        (Some(left), Some(_)) | (Some(left), None) => left.len(),
                        (None, Some(right)) => right.len(),
                        (None, None) => unreachable!(),
                    };
                    work.push(LiftWork::FinishArray {
                        ty: result_ty,
                        length,
                    });
                    for index in (0..length).rev() {
                        let left = left_array.map_or(left, |items| &items[index]);
                        let right = right_array.map_or(right, |items| &items[index]);
                        work.push(LiftWork::Apply(left, right, element_ty));
                    }
                } else {
                    results.push(evaluate_scalar_binary(
                        program, result_ty, span, operator, left, right,
                    )?);
                }
            }
        }
    }
    if results.len() != 1 {
        return Err(ConstantEvaluationError::Unavailable);
    }
    results.pop().ok_or(ConstantEvaluationError::Unavailable)
}

fn evaluate_scalar_binary(
    program: &HirProgram,
    ty: TypeId,
    span: Span,
    operator: HirBinaryOperator,
    left: &HirConstantValue,
    right: &HirConstantValue,
) -> Result<HirConstantValue, ConstantEvaluationError> {
    use HirBinaryOperator as Op;

    let operand_type = left.ty;
    let result = match (left.kind(), right.kind()) {
        (HirConstantValueKind::Integer(left), HirConstantValueKind::Integer(right)) => {
            let operand_scalar = scalar(program, operand_type)?;
            match operator {
                Op::Multiply | Op::Divide | Op::Remainder | Op::Add | Op::Subtract => {
                    HirConstantValueKind::Integer(checked_integer_arithmetic(
                        operator,
                        *left,
                        *right,
                        operand_scalar,
                        span,
                    )?)
                }
                Op::ShiftLeft | Op::ShiftRight => {
                    let (_, width) = integer_shape(operand_scalar)
                        .ok_or(ConstantEvaluationError::Unavailable)?;
                    let shift = u32::try_from(*right)
                        .ok()
                        .filter(|shift| *shift < width)
                        .ok_or_else(|| {
                            panic_error(span, "shift count is outside the operand width")
                        })?;
                    let bits = integer_to_bits(*left, width);
                    let shifted = if operator == Op::ShiftLeft {
                        bits << shift
                    } else if integer_shape(operand_scalar).is_some_and(|(signed, _)| signed) {
                        (*left >> shift) as u128
                    } else {
                        bits >> shift
                    };
                    HirConstantValueKind::Integer(integer_from_bits(shifted, operand_scalar)?)
                }
                Op::BitwiseAnd | Op::BitwiseXor | Op::BitwiseOr => {
                    let (_, width) = integer_shape(operand_scalar)
                        .ok_or(ConstantEvaluationError::Unavailable)?;
                    let left = integer_to_bits(*left, width);
                    let right = integer_to_bits(*right, width);
                    let bits = match operator {
                        Op::BitwiseAnd => left & right,
                        Op::BitwiseXor => left ^ right,
                        Op::BitwiseOr => left | right,
                        _ => unreachable!(),
                    };
                    HirConstantValueKind::Integer(integer_from_bits(bits, operand_scalar)?)
                }
                Op::Less => HirConstantValueKind::Bool(left < right),
                Op::LessEqual => HirConstantValueKind::Bool(left <= right),
                Op::Greater => HirConstantValueKind::Bool(left > right),
                Op::GreaterEqual => HirConstantValueKind::Bool(left >= right),
                Op::Equal => HirConstantValueKind::Bool(left == right),
                Op::NotEqual => HirConstantValueKind::Bool(left != right),
                Op::LogicalAnd | Op::LogicalOr => {
                    return Err(ConstantEvaluationError::Unavailable);
                }
            }
        }
        (HirConstantValueKind::Float(left), HirConstantValueKind::Float(right)) => {
            let left = f64::from_bits(*left);
            let right = f64::from_bits(*right);
            let operand_scalar = scalar(program, operand_type)?;
            match operator {
                Op::Multiply | Op::Divide | Op::Add | Op::Subtract => HirConstantValueKind::Float(
                    float_binary(operator, left, right, operand_scalar)?.to_bits(),
                ),
                Op::Less => HirConstantValueKind::Bool(left < right),
                Op::LessEqual => HirConstantValueKind::Bool(left <= right),
                Op::Greater => HirConstantValueKind::Bool(left > right),
                Op::GreaterEqual => HirConstantValueKind::Bool(left >= right),
                Op::Equal => HirConstantValueKind::Bool(left == right),
                Op::NotEqual => HirConstantValueKind::Bool(left != right),
                _ => return Err(ConstantEvaluationError::Unavailable),
            }
        }
        (HirConstantValueKind::Bool(left), HirConstantValueKind::Bool(right)) => match operator {
            Op::Equal => HirConstantValueKind::Bool(left == right),
            Op::NotEqual => HirConstantValueKind::Bool(left != right),
            Op::LogicalAnd => HirConstantValueKind::Bool(*left && *right),
            Op::LogicalOr => HirConstantValueKind::Bool(*left || *right),
            _ => return Err(ConstantEvaluationError::Unavailable),
        },
        (HirConstantValueKind::Char(left), HirConstantValueKind::Char(right)) => {
            comparison_kind(operator, left.cmp(right))?
        }
        (HirConstantValueKind::String(left), HirConstantValueKind::String(right)) => {
            comparison_kind(operator, left.cmp(right))?
        }
        (HirConstantValueKind::Unit, HirConstantValueKind::Unit) => match operator {
            Op::Equal => HirConstantValueKind::Bool(true),
            Op::NotEqual => HirConstantValueKind::Bool(false),
            _ => return Err(ConstantEvaluationError::Unavailable),
        },
        _ => return Err(ConstantEvaluationError::Unavailable),
    };
    Ok(constant_value(ty, result))
}

fn checked_integer_arithmetic(
    operator: HirBinaryOperator,
    left: i128,
    right: i128,
    scalar: ScalarType,
    span: Span,
) -> Result<i128, ConstantEvaluationError> {
    let result = match operator {
        HirBinaryOperator::Multiply => left.checked_mul(right),
        HirBinaryOperator::Add => left.checked_add(right),
        HirBinaryOperator::Subtract => left.checked_sub(right),
        HirBinaryOperator::Divide => {
            if right == 0 {
                return Err(panic_error(span, "integer division by zero"));
            }
            left.checked_div(right)
        }
        HirBinaryOperator::Remainder => {
            if right == 0 {
                return Err(panic_error(span, "integer remainder by zero"));
            }
            if integer_shape(scalar).is_some_and(|(signed, _)| signed)
                && left == integer_minimum(scalar).unwrap_or(i128::MIN)
                && right == -1
            {
                Some(0)
            } else {
                left.checked_rem(right)
            }
        }
        _ => return Err(ConstantEvaluationError::Unavailable),
    }
    .filter(|value| integer_fits(*value, scalar))
    .ok_or_else(|| panic_error(span, "integer arithmetic overflows"))?;
    Ok(result)
}

fn comparison_kind(
    operator: HirBinaryOperator,
    ordering: std::cmp::Ordering,
) -> Result<HirConstantValueKind, ConstantEvaluationError> {
    use HirBinaryOperator as Op;
    let value = match operator {
        Op::Less => ordering.is_lt(),
        Op::LessEqual => !ordering.is_gt(),
        Op::Greater => ordering.is_gt(),
        Op::GreaterEqual => !ordering.is_lt(),
        Op::Equal => ordering.is_eq(),
        Op::NotEqual => !ordering.is_eq(),
        _ => return Err(ConstantEvaluationError::Unavailable),
    };
    Ok(HirConstantValueKind::Bool(value))
}

fn evaluate_numeric_conversion(
    program: &HirProgram,
    expression_ty: TypeId,
    span: Span,
    target: ScalarType,
    conversion: NumericConversion,
    source: HirConstantValue,
) -> Result<HirConstantValue, ConstantEvaluationError> {
    let source_scalar = scalar(program, source.ty)?;
    let target_ty = program.interner.scalar(target);
    let converted = match source.kind {
        HirConstantValueKind::Integer(value) => match numeric_class(target) {
            NumericClass::Integer => {
                if !integer_fits(value, target) {
                    return Err(panic_error(
                        span,
                        "constant numeric conversion is out of range",
                    ));
                }
                constant_value(target_ty, HirConstantValueKind::Integer(value))
            }
            NumericClass::Float => constant_value(
                target_ty,
                HirConstantValueKind::Float(integer_to_float(value, target).to_bits()),
            ),
        },
        HirConstantValueKind::Float(bits) => {
            let value = f64::from_bits(bits);
            match numeric_class(target) {
                NumericClass::Integer => {
                    if !value.is_finite() {
                        return Err(panic_error(
                            span,
                            "constant float-to-integer conversion is not finite",
                        ));
                    }
                    if value.fract() != 0.0 {
                        return Err(panic_error(
                            span,
                            "constant float-to-integer conversion is not integral",
                        ));
                    }
                    if !float_fits_integer(value, target) {
                        return Err(panic_error(
                            span,
                            "constant numeric conversion is out of range",
                        ));
                    }
                    constant_value(target_ty, HirConstantValueKind::Integer(value as i128))
                }
                NumericClass::Float => {
                    let rounded = round_float(value, target);
                    if value.is_finite() && rounded.is_infinite() {
                        return Err(panic_error(
                            span,
                            "constant numeric conversion is out of range",
                        ));
                    }
                    constant_value(target_ty, HirConstantValueKind::Float(rounded.to_bits()))
                }
            }
        }
        _ => return Err(ConstantEvaluationError::Unavailable),
    };
    let expected = crate::types::numeric_conversion(source_scalar, target)
        .ok_or(ConstantEvaluationError::Unavailable)?;
    if expected != conversion {
        return Err(ConstantEvaluationError::Unavailable);
    }
    if conversion == NumericConversion::Checked {
        Ok(constant_value(
            expression_ty,
            HirConstantValueKind::ResultOk(Box::new(converted)),
        ))
    } else {
        Ok(converted)
    }
}

fn evaluate_contains(
    program: &HirProgram,
    kind: HirContainmentKind,
    item: &HirConstantValue,
    container: &HirConstantValue,
) -> Result<bool, ConstantEvaluationError> {
    match (kind, container.kind()) {
        (HirContainmentKind::Array, HirConstantValueKind::Array(items))
        | (HirContainmentKind::Set, HirConstantValueKind::Set(items)) => {
            for candidate in items {
                if values_equal(program, item, candidate)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        (HirContainmentKind::MapKey, HirConstantValueKind::Map(entries)) => {
            for (key, _) in entries {
                if values_equal(program, item, key)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        (HirContainmentKind::Range, HirConstantValueKind::Range { kind, start, end }) => {
            range_contains(*kind, item, start, end)
        }
        (HirContainmentKind::StringChar, HirConstantValueKind::String(text)) => {
            let HirConstantValueKind::Char(character) = item.kind() else {
                return Err(ConstantEvaluationError::Unavailable);
            };
            Ok(text.contains(*character))
        }
        _ => Err(ConstantEvaluationError::Unavailable),
    }
}

fn range_contains(
    kind: HirRangeKind,
    item: &HirConstantValue,
    start: &HirConstantValue,
    end: &HirConstantValue,
) -> Result<bool, ConstantEvaluationError> {
    let (after_start, before_end) = match (item.kind(), start.kind(), end.kind()) {
        (
            HirConstantValueKind::Integer(item),
            HirConstantValueKind::Integer(start),
            HirConstantValueKind::Integer(end),
        ) => (item >= start, item < end),
        (
            HirConstantValueKind::Char(item),
            HirConstantValueKind::Char(start),
            HirConstantValueKind::Char(end),
        ) => (item >= start, item < end),
        _ => return Err(ConstantEvaluationError::Unavailable),
    };
    if !after_start {
        return Ok(false);
    }
    if kind == HirRangeKind::Exclusive {
        Ok(before_end)
    } else {
        Ok(before_end || values_equal_scalar(item, end)?)
    }
}

fn evaluate_index(
    program: &HirProgram,
    ty: TypeId,
    span: Span,
    access: HirIndexAccess,
    base: HirConstantValue,
    index: HirConstantValue,
) -> Result<HirConstantValue, ConstantEvaluationError> {
    match (access, base.kind) {
        (HirIndexAccess::Array, HirConstantValueKind::Array(items)) => {
            let HirConstantValueKind::Integer(index) = index.kind else {
                return Err(ConstantEvaluationError::Unavailable);
            };
            let normalized = normalize_index(index, items.len())
                .ok_or_else(|| panic_error(span, "constant array index is out of bounds"))?;
            items
                .into_iter()
                .nth(normalized)
                .ok_or(ConstantEvaluationError::Unavailable)
        }
        (HirIndexAccess::MapLookup, HirConstantValueKind::Map(entries)) => {
            for (key, value) in entries {
                if values_equal(program, &index, &key)? {
                    return Ok(constant_value(
                        ty,
                        HirConstantValueKind::OptionSome(Box::new(value)),
                    ));
                }
            }
            Ok(constant_value(ty, HirConstantValueKind::OptionNone))
        }
        (HirIndexAccess::MapEntry, _) => Err(ConstantEvaluationError::Nonconstant {
            span,
            reason: "an assignable map entry is not a constant value",
        }),
        _ => Err(ConstantEvaluationError::Unavailable),
    }
}

fn evaluate_slice(
    ty: TypeId,
    span: Span,
    base: HirConstantValue,
    start: Option<HirConstantValue>,
    end: Option<HirConstantValue>,
    step: Option<HirConstantValue>,
) -> Result<HirConstantValue, ConstantEvaluationError> {
    let HirConstantValueKind::Array(items) = base.kind else {
        return Err(ConstantEvaluationError::Unavailable);
    };
    let start = optional_integer(start)?;
    let end = optional_integer(end)?;
    let step = optional_integer(step)?.unwrap_or(1);
    if step == 0 {
        return Err(panic_error(span, "constant slice step is zero"));
    }
    let length = i128::try_from(items.len()).map_err(|_| ConstantEvaluationError::Unavailable)?;
    let mut output = Vec::new();
    if step > 0 {
        let start = normalize_slice_bound(start.unwrap_or(0), length, true);
        let end = normalize_slice_bound(end.unwrap_or(length), length, true);
        let mut index = start;
        while index < end {
            output.push(items[index as usize].clone());
            if step >= end - index {
                break;
            }
            index += step;
        }
    } else {
        let start = start
            .map(|value| normalize_slice_bound(value, length, false))
            .unwrap_or(length - 1);
        let end = end
            .map(|value| normalize_slice_bound(value, length, false))
            .unwrap_or(-1);
        let mut index = start;
        while index > end {
            output.push(items[index as usize].clone());
            if step.unsigned_abs() >= (index - end) as u128 {
                break;
            }
            index += step;
        }
    }
    Ok(constant_value(ty, HirConstantValueKind::Array(output)))
}

fn optional_integer(
    value: Option<HirConstantValue>,
) -> Result<Option<i128>, ConstantEvaluationError> {
    value
        .map(|value| match value.kind {
            HirConstantValueKind::Integer(value) => Ok(value),
            _ => Err(ConstantEvaluationError::Unavailable),
        })
        .transpose()
}

fn normalize_slice_bound(mut value: i128, length: i128, positive: bool) -> i128 {
    if value < 0 {
        value = value.saturating_add(length);
    }
    if positive {
        value.clamp(0, length)
    } else {
        value.clamp(-1, length - 1)
    }
}

fn normalize_index(index: i128, length: usize) -> Option<usize> {
    let length = i128::try_from(length).ok()?;
    let normalized = if index < 0 {
        length.checked_add(index)?
    } else {
        index
    };
    (0..length)
        .contains(&normalized)
        .then(|| usize::try_from(normalized).ok())
        .flatten()
}

pub(super) fn values_equal(
    program: &HirProgram,
    left: &HirConstantValue,
    right: &HirConstantValue,
) -> Result<bool, ConstantEvaluationError> {
    if left.ty != right.ty {
        return Ok(false);
    }
    let mut pending = vec![(left, right)];
    while let Some((left, right)) = pending.pop() {
        if left.ty != right.ty {
            return Ok(false);
        }
        match (left.kind(), right.kind()) {
            (HirConstantValueKind::Unit, HirConstantValueKind::Unit)
            | (HirConstantValueKind::OptionNone, HirConstantValueKind::OptionNone) => {}
            (HirConstantValueKind::Bool(left), HirConstantValueKind::Bool(right))
                if left == right => {}
            (HirConstantValueKind::Integer(left), HirConstantValueKind::Integer(right))
                if left == right => {}
            (HirConstantValueKind::Float(left), HirConstantValueKind::Float(right))
                if f64::from_bits(*left) == f64::from_bits(*right) => {}
            (HirConstantValueKind::Char(left), HirConstantValueKind::Char(right))
                if left == right => {}
            (HirConstantValueKind::String(left), HirConstantValueKind::String(right))
                if left == right => {}
            (
                HirConstantValueKind::Function {
                    callable: left_callable,
                    arguments: left_arguments,
                },
                HirConstantValueKind::Function {
                    callable: right_callable,
                    arguments: right_arguments,
                },
            ) if left_callable == right_callable && left_arguments == right_arguments => {}
            (HirConstantValueKind::Tuple(left), HirConstantValueKind::Tuple(right))
            | (HirConstantValueKind::Array(left), HirConstantValueKind::Array(right))
                if left.len() == right.len() =>
            {
                pending.extend(left.iter().zip(right));
            }
            (HirConstantValueKind::Set(left), HirConstantValueKind::Set(right))
                if left.len() == right.len() =>
            {
                for item in left {
                    let mut found = false;
                    for candidate in right {
                        if values_equal(program, item, candidate)? {
                            found = true;
                            break;
                        }
                    }
                    if !found {
                        return Ok(false);
                    }
                }
            }
            (HirConstantValueKind::Map(left), HirConstantValueKind::Map(right))
                if left.len() == right.len() =>
            {
                for (key, value) in left {
                    let mut found = None;
                    for (candidate, candidate_value) in right {
                        if values_equal(program, key, candidate)? {
                            found = Some(candidate_value);
                            break;
                        }
                    }
                    let Some(candidate) = found else {
                        return Ok(false);
                    };
                    pending.push((value, candidate));
                }
            }
            (
                HirConstantValueKind::Newtype {
                    constructor: left_constructor,
                    value: left,
                },
                HirConstantValueKind::Newtype {
                    constructor: right_constructor,
                    value: right,
                },
            ) if left_constructor == right_constructor => pending.push((left, right)),
            (
                HirConstantValueKind::Record {
                    owner: left_owner,
                    fields: left,
                },
                HirConstantValueKind::Record {
                    owner: right_owner,
                    fields: right,
                },
            ) if left_owner == right_owner && left.len() == right.len() => {
                for field in left {
                    let Some(other) = right.iter().find(|other| other.member == field.member)
                    else {
                        return Ok(false);
                    };
                    pending.push((&field.value, &other.value));
                }
            }
            (
                HirConstantValueKind::Variant {
                    variant: left_variant,
                    payload: left,
                },
                HirConstantValueKind::Variant {
                    variant: right_variant,
                    payload: right,
                },
            ) if left_variant == right_variant => match (left, right) {
                (HirConstantVariantValue::Unit, HirConstantVariantValue::Unit) => {}
                (HirConstantVariantValue::Tuple(left), HirConstantVariantValue::Tuple(right))
                    if left.len() == right.len() =>
                {
                    pending.extend(left.iter().zip(right));
                }
                (HirConstantVariantValue::Record(left), HirConstantVariantValue::Record(right))
                    if left.len() == right.len() =>
                {
                    for field in left {
                        let Some(other) = right.iter().find(|other| other.member == field.member)
                        else {
                            return Ok(false);
                        };
                        pending.push((&field.value, &other.value));
                    }
                }
                _ => return Ok(false),
            },
            (HirConstantValueKind::OptionSome(left), HirConstantValueKind::OptionSome(right))
            | (HirConstantValueKind::ResultOk(left), HirConstantValueKind::ResultOk(right))
            | (HirConstantValueKind::ResultErr(left), HirConstantValueKind::ResultErr(right))
            | (HirConstantValueKind::Converted(left), HirConstantValueKind::Converted(right)) => {
                pending.push((left, right));
            }
            (
                HirConstantValueKind::Range {
                    kind: left_kind,
                    start: left_start,
                    end: left_end,
                },
                HirConstantValueKind::Range {
                    kind: right_kind,
                    start: right_start,
                    end: right_end,
                },
            ) if left_kind == right_kind => {
                pending.push((left_start, right_start));
                pending.push((left_end, right_end));
            }
            _ => return Ok(false),
        }
    }
    let _ = program;
    Ok(true)
}

pub(super) fn is_nan(value: &HirConstantValue) -> bool {
    matches!(value.kind(), HirConstantValueKind::Float(bits) if f64::from_bits(*bits).is_nan())
}

fn values_equal_scalar(
    left: &HirConstantValue,
    right: &HirConstantValue,
) -> Result<bool, ConstantEvaluationError> {
    Ok(match (left.kind(), right.kind()) {
        (HirConstantValueKind::Integer(left), HirConstantValueKind::Integer(right)) => {
            left == right
        }
        (HirConstantValueKind::Char(left), HirConstantValueKind::Char(right)) => left == right,
        _ => return Err(ConstantEvaluationError::Unavailable),
    })
}

fn scalar(program: &HirProgram, ty: TypeId) -> Result<ScalarType, ConstantEvaluationError> {
    match program.interner.kind(ty)? {
        TypeKind::Scalar(scalar) => Ok(*scalar),
        _ => Err(ConstantEvaluationError::Unavailable),
    }
}

fn intrinsic_element(
    program: &HirProgram,
    ty: TypeId,
    expected: IntrinsicType,
) -> Result<TypeId, ConstantEvaluationError> {
    match program.interner.kind(ty)? {
        TypeKind::Intrinsic {
            constructor,
            arguments,
        } if *constructor == expected => arguments
            .first()
            .copied()
            .ok_or(ConstantEvaluationError::Unavailable),
        _ => Err(ConstantEvaluationError::Unavailable),
    }
}

fn panic_error(span: Span, reason: impl Into<String>) -> ConstantEvaluationError {
    ConstantEvaluationError::Panic {
        span,
        reason: reason.into(),
    }
}

#[derive(Clone, Copy)]
enum NumericClass {
    Integer,
    Float,
}

fn numeric_class(scalar: ScalarType) -> NumericClass {
    if matches!(scalar, ScalarType::Float | ScalarType::Float32) {
        NumericClass::Float
    } else {
        NumericClass::Integer
    }
}

fn integer_shape(scalar: ScalarType) -> Option<(bool, u32)> {
    Some(match scalar {
        ScalarType::Byte | ScalarType::UInt8 => (false, 8),
        ScalarType::UInt16 => (false, 16),
        ScalarType::UInt32 => (false, 32),
        ScalarType::UInt64 => (false, 64),
        ScalarType::Int8 => (true, 8),
        ScalarType::Int16 => (true, 16),
        ScalarType::Int32 => (true, 32),
        ScalarType::Int => (true, 64),
        _ => return None,
    })
}

fn integer_minimum(scalar: ScalarType) -> Option<i128> {
    let (signed, bits) = integer_shape(scalar)?;
    signed.then(|| -(1_i128 << (bits - 1)))
}

fn integer_fits(value: i128, scalar: ScalarType) -> bool {
    let Some((signed, bits)) = integer_shape(scalar) else {
        return false;
    };
    if signed {
        let minimum = -(1_i128 << (bits - 1));
        let maximum = (1_i128 << (bits - 1)) - 1;
        (minimum..=maximum).contains(&value)
    } else {
        let maximum = (1_u128 << bits) - 1;
        value >= 0 && (value as u128) <= maximum
    }
}

fn float_fits_integer(value: f64, scalar: ScalarType) -> bool {
    let Some((signed, bits)) = integer_shape(scalar) else {
        return false;
    };
    if signed {
        let minimum = -(2_f64.powi(bits as i32 - 1));
        let exclusive_maximum = 2_f64.powi(bits as i32 - 1);
        value >= minimum && value < exclusive_maximum
    } else {
        value >= 0.0 && value < 2_f64.powi(bits as i32)
    }
}

fn integer_to_bits(value: i128, width: u32) -> u128 {
    let mask = (1_u128 << width) - 1;
    (value as u128) & mask
}

fn integer_from_bits(bits: u128, scalar: ScalarType) -> Result<i128, ConstantEvaluationError> {
    let (signed, width) = integer_shape(scalar).ok_or(ConstantEvaluationError::Unavailable)?;
    let mask = (1_u128 << width) - 1;
    let masked = bits & mask;
    if signed && masked & (1_u128 << (width - 1)) != 0 {
        Ok((masked as i128) - (1_i128 << width))
    } else {
        i128::try_from(masked).map_err(|_| ConstantEvaluationError::Unavailable)
    }
}

fn round_float(value: f64, scalar: ScalarType) -> f64 {
    if scalar == ScalarType::Float32 {
        (value as f32) as f64
    } else {
        value
    }
}

fn parse_float_literal(spelling: &str, scalar: ScalarType) -> Option<f64> {
    match scalar {
        ScalarType::Float32 => spelling.parse::<f32>().ok().map(|value| value as f64),
        ScalarType::Float => spelling.parse::<f64>().ok(),
        _ => None,
    }
}

fn float_binary(
    operator: HirBinaryOperator,
    left: f64,
    right: f64,
    scalar: ScalarType,
) -> Result<f64, ConstantEvaluationError> {
    let operation_f64 = |left: f64, right: f64| match operator {
        HirBinaryOperator::Multiply => Some(left * right),
        HirBinaryOperator::Divide => Some(left / right),
        HirBinaryOperator::Add => Some(left + right),
        HirBinaryOperator::Subtract => Some(left - right),
        _ => None,
    };
    if scalar == ScalarType::Float32 {
        let left = left as f32;
        let right = right as f32;
        let value = match operator {
            HirBinaryOperator::Multiply => left * right,
            HirBinaryOperator::Divide => left / right,
            HirBinaryOperator::Add => left + right,
            HirBinaryOperator::Subtract => left - right,
            _ => return Err(ConstantEvaluationError::Unavailable),
        };
        Ok(value as f64)
    } else {
        operation_f64(left, right).ok_or(ConstantEvaluationError::Unavailable)
    }
}

fn integer_to_float(value: i128, scalar: ScalarType) -> f64 {
    if scalar == ScalarType::Float32 {
        (value as f32) as f64
    } else {
        value as f64
    }
}

fn numeric_body(spelling: &str) -> String {
    let body = spelling
        .strip_suffix("f32")
        .or_else(|| spelling.strip_suffix("f64"))
        .unwrap_or(spelling);
    body.replace('_', "")
}

fn integer_magnitude(spelling: &str) -> Option<u128> {
    let suffix_length = ["i16", "i32", "i64", "u16", "u32", "u64"]
        .into_iter()
        .find(|suffix| spelling.ends_with(suffix))
        .map_or_else(
            || {
                ["i8", "u8"]
                    .into_iter()
                    .find(|suffix| spelling.ends_with(suffix))
                    .map_or(0, str::len)
            },
            str::len,
        );
    let body = &spelling[..spelling.len().checked_sub(suffix_length)?];
    let (radix, digits) = if let Some(digits) = body.strip_prefix("0b") {
        (2, digits)
    } else if let Some(digits) = body.strip_prefix("0o") {
        (8, digits)
    } else if let Some(digits) = body.strip_prefix("0x") {
        (16, digits)
    } else {
        (10, body)
    };
    u128::from_str_radix(&digits.replace('_', ""), radix).ok()
}

fn decode_char_literal(spelling: &str) -> Option<char> {
    let body = spelling.strip_prefix('\'')?.strip_suffix('\'')?;
    let decoded = decode_escaped_text(body, false)?;
    let mut characters = decoded.chars();
    let character = characters.next()?;
    characters.next().is_none().then_some(character)
}

fn decode_string_literal(spelling: &str) -> Option<String> {
    let (raw, multiline, opening, closing) = if spelling.starts_with("r\"\"\"") {
        (true, true, "r\"\"\"", "\"\"\"")
    } else if spelling.starts_with("r\"") {
        (true, false, "r\"", "\"")
    } else if spelling.starts_with("\"\"\"") {
        (false, true, "\"\"\"", "\"\"\"")
    } else if spelling.starts_with('\"') {
        (false, false, "\"", "\"")
    } else {
        return None;
    };
    let body = spelling.strip_prefix(opening)?.strip_suffix(closing)?;
    let body = if multiline {
        normalize_multiline_string(body)
    } else {
        body.to_owned()
    };
    if raw {
        Some(body)
    } else {
        decode_escaped_text(&body, true)
    }
}

fn normalize_multiline_string(body: &str) -> String {
    let mut normalized = body.replace("\r\n", "\n");
    if normalized.starts_with('\n') {
        normalized.remove(0);
    }
    let line_start = normalized.rfind('\n').map_or(0, |index| index + 1);
    if !normalized[line_start..]
        .bytes()
        .all(|byte| matches!(byte, b' ' | b'\t'))
    {
        return normalized;
    }
    let prefix = normalized[line_start..].to_owned();
    normalized.truncate(if line_start == 0 { 0 } else { line_start - 1 });
    normalized
        .split('\n')
        .map(|line| {
            if line.bytes().all(|byte| matches!(byte, b' ' | b'\t')) {
                let common = line
                    .bytes()
                    .zip(prefix.bytes())
                    .take_while(|(left, right)| left == right)
                    .count();
                &line[common..]
            } else {
                line.strip_prefix(&prefix).unwrap_or(line)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn decode_escaped_text(body: &str, decode_braces: bool) -> Option<String> {
    let mut output = String::with_capacity(body.len());
    let mut characters = body.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\\' => match characters.next()? {
                'n' => output.push('\n'),
                'r' => output.push('\r'),
                't' => output.push('\t'),
                '\\' => output.push('\\'),
                '\'' => output.push('\''),
                '"' => output.push('"'),
                '0' => output.push('\0'),
                'u' => {
                    if characters.next()? != '{' {
                        return None;
                    }
                    let mut digits = String::new();
                    loop {
                        let digit = characters.next()?;
                        if digit == '}' {
                            break;
                        }
                        digits.push(digit);
                    }
                    if !(1..=6).contains(&digits.len()) {
                        return None;
                    }
                    output.push(char::from_u32(u32::from_str_radix(&digits, 16).ok()?)?);
                }
                _ => return None,
            },
            '{' | '}' if decode_braces => {
                characters.next_if_eq(&character)?;
                output.push(character);
            }
            _ => output.push(character),
        }
    }
    Some(output)
}
