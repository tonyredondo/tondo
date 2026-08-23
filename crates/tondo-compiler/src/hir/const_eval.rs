use std::collections::BTreeMap;

use tondo_vm::bytecode::{ArraySliceError, normalize_array_index, normalize_array_slice_indices};

use crate::source::Span;
use crate::types::{
    Assignability, IntrinsicType, NumericConversion, NumericConversionErrorVariant, ScalarType,
    TypeError, TypeId, TypeKind,
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
                        if signature.generic_arity() != 0 {
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
                    | HirExpressionKind::ArraySequence { .. }
                    | HirExpressionKind::PreludeTraitFunction { .. }
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
                    | HirExpressionKind::Select { .. }
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
        HirExpressionKind::Select { arms, else_body } => arms
            .iter()
            .flat_map(|arm| [arm.operation(), arm.body()])
            .chain(else_body.iter().copied())
            .collect(),
        HirExpressionKind::Map { entries, .. } => entries
            .iter()
            .flat_map(|entry| [entry.key(), entry.value()])
            .collect(),
        HirExpressionKind::Newtype { value, .. }
        | HirExpressionKind::Ref { value }
        | HirExpressionKind::NumericConversion { value, .. }
        | HirExpressionKind::Prefix { operand: value, .. }
        | HirExpressionKind::Field { base: value, .. }
        | HirExpressionKind::TupleField { base: value, .. }
        | HirExpressionKind::RefValue { base: value }
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
        HirExpressionKind::ArraySequence {
            array, argument, ..
        } => vec![*array, *argument],
        HirExpressionKind::MapRemove { map, key } => vec![*map, *key],
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
            ..
        } => std::iter::once(*condition)
            .chain(message_parts.iter().map(|part| part.value()))
            .collect(),
        HirExpressionKind::BootstrapHostCall { arguments, .. } => arguments.clone(),
        HirExpressionKind::Recovery
        | HirExpressionKind::Literal(_)
        | HirExpressionKind::NumericConversionError(_)
        | HirExpressionKind::InterpolatedString { .. }
        | HirExpressionKind::Local(_)
        | HirExpressionKind::Constant(_)
        | HirExpressionKind::Function(_)
        | HirExpressionKind::SyntheticFunction
        | HirExpressionKind::SpecializedFunction { .. }
        | HirExpressionKind::PreludeTraitFunction { .. }
        | HirExpressionKind::Closure(_)
        | HirExpressionKind::Receiver
        | HirExpressionKind::Block { .. }
        | HirExpressionKind::Call { .. }
        | HirExpressionKind::AsyncCall { .. }
        | HirExpressionKind::Await { .. }
        | HirExpressionKind::Spawn { .. }
        | HirExpressionKind::Scope { .. }
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
        HirExpressionKind::Select { .. } => return Err(ConstantEvaluationError::Unavailable),
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
        HirExpressionKind::Map { entries, .. } => HirConstantValueKind::Map(
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
        HirExpressionKind::NumericConversionError(variant) => {
            HirConstantValueKind::NumericConversionError(*variant)
        }
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
                Assignability::Exact | Assignability::EffectWeakening | Assignability::Opaque => {
                    inner.ty = ty;
                    return Ok(inner);
                }
                Assignability::OptionLift => HirConstantValueKind::OptionSome(Box::new(inner)),
                Assignability::UnionInjection | Assignability::UnionWidening => {
                    HirConstantValueKind::Converted(Box::new(inner))
                }
                Assignability::CallableErasure
                | Assignability::CallableOnceErasure
                | Assignability::Diverging => {
                    return Err(ConstantEvaluationError::Unavailable);
                }
            }
        }
        HirExpressionKind::Recovery
        | HirExpressionKind::ArraySequence { .. }
        | HirExpressionKind::MapRemove { .. }
        | HirExpressionKind::Ref { .. }
        | HirExpressionKind::RefValue { .. }
        | HirExpressionKind::Literal(_)
        | HirExpressionKind::InterpolatedString { .. }
        | HirExpressionKind::Local(_)
        | HirExpressionKind::Constant(_)
        | HirExpressionKind::Function(_)
        | HirExpressionKind::SyntheticFunction
        | HirExpressionKind::SpecializedFunction { .. }
        | HirExpressionKind::PreludeTraitFunction { .. }
        | HirExpressionKind::Closure(_)
        | HirExpressionKind::Receiver
        | HirExpressionKind::Block { .. }
        | HirExpressionKind::Call { .. }
        | HirExpressionKind::AsyncCall { .. }
        | HirExpressionKind::Await { .. }
        | HirExpressionKind::Spawn { .. }
        | HirExpressionKind::Scope { .. }
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
    if matches!(
        operator,
        HirBinaryOperator::Add
            | HirBinaryOperator::Subtract
            | HirBinaryOperator::Multiply
            | HirBinaryOperator::Divide
            | HirBinaryOperator::Remainder
    ) {
        validate_lifted_array_shape(&left, &right, span)?;
    }

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

fn validate_lifted_array_shape(
    left: &HirConstantValue,
    right: &HirConstantValue,
    span: Span,
) -> Result<(), ConstantEvaluationError> {
    let mut pending = vec![(left, right)];
    while let Some((left, right)) = pending.pop() {
        let left_array = match left.kind() {
            HirConstantValueKind::Array(items) => Some(items.as_slice()),
            _ => None,
        };
        let right_array = match right.kind() {
            HirConstantValueKind::Array(items) => Some(items.as_slice()),
            _ => None,
        };
        let length = match (left_array, right_array) {
            (Some(left), Some(right)) if left.len() != right.len() => {
                return Err(panic_error(span, "array operands have different shapes"));
            }
            (Some(left), Some(_)) | (Some(left), None) => left.len(),
            (None, Some(right)) => right.len(),
            (None, None) => continue,
        };
        for index in (0..length).rev() {
            pending.push((
                left_array.map_or(left, |items| &items[index]),
                right_array.map_or(right, |items| &items[index]),
            ));
        }
    }
    Ok(())
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
    _span: Span,
    target: ScalarType,
    conversion: NumericConversion,
    source: HirConstantValue,
) -> Result<HirConstantValue, ConstantEvaluationError> {
    let source_scalar = scalar(program, source.ty)?;
    let target_ty = program.interner.scalar(target);
    let expected = crate::types::numeric_conversion(source_scalar, target)
        .ok_or(ConstantEvaluationError::Unavailable)?;
    if expected != conversion {
        return Err(ConstantEvaluationError::Unavailable);
    }
    let converted = match source.kind {
        HirConstantValueKind::Integer(value) => match numeric_class(target) {
            NumericClass::Integer => {
                if !integer_fits(value, target) {
                    Err(NumericConversionErrorVariant::OutOfRange)
                } else {
                    Ok(constant_value(
                        target_ty,
                        HirConstantValueKind::Integer(value),
                    ))
                }
            }
            NumericClass::Float => Ok(constant_value(
                target_ty,
                HirConstantValueKind::Float(integer_to_float(value, target).to_bits()),
            )),
        },
        HirConstantValueKind::Float(bits) => {
            let value = f64::from_bits(bits);
            match numeric_class(target) {
                NumericClass::Integer => {
                    if !value.is_finite() {
                        Err(NumericConversionErrorVariant::NotFinite)
                    } else if value.fract() != 0.0 {
                        Err(NumericConversionErrorVariant::NotIntegral)
                    } else if !float_fits_integer(value, target) {
                        Err(NumericConversionErrorVariant::OutOfRange)
                    } else {
                        Ok(constant_value(
                            target_ty,
                            HirConstantValueKind::Integer(value as i128),
                        ))
                    }
                }
                NumericClass::Float => {
                    let rounded = round_float(value, target);
                    if value.is_finite() && rounded.is_infinite() {
                        Err(NumericConversionErrorVariant::OutOfRange)
                    } else {
                        Ok(constant_value(
                            target_ty,
                            HirConstantValueKind::Float(rounded.to_bits()),
                        ))
                    }
                }
            }
        }
        _ => return Err(ConstantEvaluationError::Unavailable),
    };
    match (conversion, converted) {
        (NumericConversion::Checked, Ok(converted)) => Ok(constant_value(
            expression_ty,
            HirConstantValueKind::ResultOk(Box::new(converted)),
        )),
        (NumericConversion::Checked, Err(error)) => {
            let error_ty = match program.interner.kind(expression_ty)? {
                TypeKind::Result { success, error } if *success == target_ty => *error,
                _ => return Err(ConstantEvaluationError::Unavailable),
            };
            if !matches!(
                program.interner.kind(error_ty)?,
                TypeKind::Intrinsic {
                    constructor: IntrinsicType::NumericConversionError,
                    arguments,
                } if arguments.is_empty()
            ) {
                return Err(ConstantEvaluationError::Unavailable);
            }
            Ok(constant_value(
                expression_ty,
                HirConstantValueKind::ResultErr(Box::new(constant_value(
                    error_ty,
                    HirConstantValueKind::NumericConversionError(error),
                ))),
            ))
        }
        (NumericConversion::Identity | NumericConversion::Total, Ok(converted)) => Ok(converted),
        (NumericConversion::Identity | NumericConversion::Total, Err(_)) => {
            Err(ConstantEvaluationError::Unavailable)
        }
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
            let normalized = normalize_array_index(index, items.len())
                .ok_or_else(|| panic_error(span, "constant array index is out of bounds"))?;
            items
                .into_iter()
                .nth(normalized)
                .ok_or(ConstantEvaluationError::Unavailable)
        }
        (HirIndexAccess::String, HirConstantValueKind::String(text)) => {
            let HirConstantValueKind::Integer(index) = index.kind else {
                return Err(ConstantEvaluationError::Unavailable);
            };
            let length = text.chars().count();
            let normalized = normalize_array_index(index, length)
                .ok_or_else(|| panic_error(span, "constant String index is out of bounds"))?;
            let character = text
                .chars()
                .nth(normalized)
                .ok_or(ConstantEvaluationError::Unavailable)?;
            Ok(constant_value(ty, HirConstantValueKind::Char(character)))
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
    let start = optional_integer(start)?;
    let end = optional_integer(end)?;
    let step = optional_integer(step)?;
    match base.kind {
        HirConstantValueKind::Array(items) => {
            let indices = constant_slice_indices(span, start, end, step, items.len())?;
            let output = indices
                .into_iter()
                .map(|index| items[index].clone())
                .collect();
            Ok(constant_value(ty, HirConstantValueKind::Array(output)))
        }
        HirConstantValueKind::String(text) => {
            let characters = text.chars().collect::<Vec<_>>();
            let indices = constant_slice_indices(span, start, end, step, characters.len())?;
            let output = indices.into_iter().map(|index| characters[index]).collect();
            Ok(constant_value(ty, HirConstantValueKind::String(output)))
        }
        _ => Err(ConstantEvaluationError::Unavailable),
    }
}

fn constant_slice_indices(
    span: Span,
    start: Option<i128>,
    end: Option<i128>,
    step: Option<i128>,
    length: usize,
) -> Result<Vec<usize>, ConstantEvaluationError> {
    normalize_array_slice_indices(start, end, step, length).map_err(|error| match error {
        ArraySliceError::ZeroStep => panic_error(span, "constant slice step is zero"),
        ArraySliceError::LengthNotRepresentable => ConstantEvaluationError::Unavailable,
    })
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
            (
                HirConstantValueKind::NumericConversionError(left),
                HirConstantValueKind::NumericConversionError(right),
            ) if left == right => {}
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::hir::{ExpressionCheckLimits, TypeLoweringLimits, check_expressions, lower_types};
    use crate::package::PackageGraph;
    use crate::resolve::{ResolvedProgram, resolve};
    use crate::source::{LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput};
    use crate::syntax::{LexMode, ParseLimits, ParseMode, lex, parse};

    use super::*;

    fn checked_program(source: &str) -> (ResolvedProgram, HirProgram) {
        let mut sources = SourceDatabase::new();
        let file = sources
            .add(SourceInput::virtual_file(
                SourceId::new("root:const-eval").unwrap(),
                ModulePath::new("main").unwrap(),
                LogicalPath::new("main.to").unwrap(),
                Arc::<[u8]>::from(source.as_bytes().to_vec()),
            ))
            .unwrap();
        let lexed = lex(&sources, file, LexMode::Module).unwrap();
        assert!(lexed.diagnostics().is_empty());
        let parsed = parse(
            &sources,
            file,
            lexed,
            ParseMode::Module,
            ParseLimits::default(),
        )
        .unwrap();
        assert!(parsed.diagnostics().is_empty());
        let packages = PackageGraph::loose(&sources, file).unwrap();
        let (resolved, diagnostics) = resolve(&packages, &sources, [(file, &parsed)], 100)
            .unwrap()
            .into_parts();
        assert!(diagnostics.is_empty());
        let (program, diagnostics) = lower_types(
            &packages,
            &sources,
            [(file, &parsed)],
            &resolved,
            TypeLoweringLimits {
                max_type_nodes: 10_000,
                max_trait_obligations: 10_000,
                max_diagnostics: 100,
            },
        )
        .unwrap()
        .into_parts();
        assert!(diagnostics.is_empty());
        let (program, diagnostics, complete) = check_expressions(
            &sources,
            [(file, &parsed)],
            &resolved,
            program,
            ExpressionCheckLimits {
                max_nodes: 10_000,
                max_pattern_steps: 10_000,
                max_trait_obligations: 10_000,
                max_diagnostics: 100,
            },
        )
        .unwrap()
        .into_parts();
        assert!(complete);
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        (resolved, program)
    }

    fn evaluated<'a>(
        resolved: &ResolvedProgram,
        program: &'a HirProgram,
        name: &str,
    ) -> &'a HirConstantValue {
        resolved
            .symbols()
            .find(|symbol| symbol.name().as_str() == name)
            .and_then(|symbol| program.constant(symbol.id()))
            .and_then(super::super::HirConstant::evaluated)
            .unwrap_or_else(|| panic!("{name} must have a normalized constant value"))
    }

    #[test]
    fn structural_constant_equality_covers_every_closed_value_shape() {
        const SOURCE: &str = "const UnitValue: Unit = ()\n\
             const Flag: Bool = true\n\
             const Answer: Int = 42\n\
             const Ratio: Float = 2.5\n\
             const Letter: Char = 'x'\n\
             const Text: String = \"value\"\n\
             const Pair: (Int, String) = (Answer, Text)\n\
             const OtherPair: (Int, String) = (41, Text)\n\
             const Numbers: Array[Int] = [1, 2]\n\
             const OtherNumbers: Array[Int] = [2, 1]\n\
             const Entries: Map[String, Int] = [\"one\": 1]\n\
             const OtherEntries: Map[String, Int] = [\"two\": 1]\n\
             const Permissions: Set[String] = Set[\"read\"]\n\
             const OtherPermissions: Set[String] = Set[\"write\"]\n\
             const Missing: Int? = none\n\
             const Present: Int? = some(Answer)\n\
             const OtherPresent: Int? = some(41)\n\
             const Success: Int ! String = ok(Answer)\n\
             const OtherSuccess: Int ! String = ok(41)\n\
             const Failure: Int ! String = err(Text)\n\
             const OtherFailure: Int ! String = err(\"other\")\n\
             const Span: Range[Int] = 1..=3\n\
             const OtherSpan: Range[Int] = 2..=3\n\
             const Converted: Int8 ! NumericConversionError = Int8(127)\n\
             const ConversionFailure: Int8 ! NumericConversionError = Int8(128)\n\
             type UserId = Int\n\
             type Person = { id: UserId, name: String }\n\
             enum Choice {\n\
                 Empty\n\
                 Pair(Int)\n\
                 Named { value: Int }\n\
             }\n\
             const Id: UserId = UserId(9)\n\
             const OtherId: UserId = UserId(10)\n\
             const User: Person = Person { id: Id, name: \"Ada\" }\n\
             const OtherUser: Person = Person { id: Id, name: \"Grace\" }\n\
             const UnitChoice: Choice = Choice.Empty\n\
             const TupleChoice: Choice = Choice.Pair(1)\n\
             const OtherTupleChoice: Choice = Choice.Pair(2)\n\
             const RecordChoice: Choice = Choice.Named { value: 2 }\n\
             const OtherRecordChoice: Choice = Choice.Named { value: 3 }\n\
             fn identity(value: Int): Int { value }\n\
             const Handler: fn(Int): Int = identity\n";
        let (resolved, program) = checked_program(SOURCE);

        let values = program
            .constants()
            .filter_map(|(_, constant)| constant.evaluated())
            .collect::<Vec<_>>();
        assert!(values.len() >= 30);
        for value in values {
            assert!(values_equal(&program, value, value).unwrap(), "{value:?}");
        }

        for (left, right) in [
            ("Pair", "OtherPair"),
            ("Numbers", "OtherNumbers"),
            ("Entries", "OtherEntries"),
            ("Permissions", "OtherPermissions"),
            ("Present", "OtherPresent"),
            ("Success", "OtherSuccess"),
            ("Failure", "OtherFailure"),
            ("Span", "OtherSpan"),
            ("Id", "OtherId"),
            ("User", "OtherUser"),
            ("TupleChoice", "OtherTupleChoice"),
            ("RecordChoice", "OtherRecordChoice"),
        ] {
            assert!(
                !values_equal(
                    &program,
                    evaluated(&resolved, &program, left),
                    evaluated(&resolved, &program, right),
                )
                .unwrap(),
                "{left} must differ from {right}"
            );
        }
        assert!(
            !values_equal(
                &program,
                evaluated(&resolved, &program, "Answer"),
                evaluated(&resolved, &program, "Text"),
            )
            .unwrap()
        );
        assert!(is_nan(&constant_value(
            program.interner.scalar(ScalarType::Float),
            HirConstantValueKind::Float(f64::NAN.to_bits()),
        )));
    }

    #[test]
    fn literal_numeric_and_text_helpers_close_their_edge_tables() {
        for (scalar, expected) in [
            (ScalarType::Byte, Some((false, 8))),
            (ScalarType::UInt8, Some((false, 8))),
            (ScalarType::UInt16, Some((false, 16))),
            (ScalarType::UInt32, Some((false, 32))),
            (ScalarType::UInt64, Some((false, 64))),
            (ScalarType::Int8, Some((true, 8))),
            (ScalarType::Int16, Some((true, 16))),
            (ScalarType::Int32, Some((true, 32))),
            (ScalarType::Int, Some((true, 64))),
            (ScalarType::Float, None),
        ] {
            assert_eq!(integer_shape(scalar), expected, "{scalar}");
        }
        assert_eq!(integer_minimum(ScalarType::Int8), Some(-128));
        assert_eq!(integer_minimum(ScalarType::UInt8), None);
        assert!(integer_fits(-128, ScalarType::Int8));
        assert!(integer_fits(255, ScalarType::UInt8));
        assert!(!integer_fits(128, ScalarType::Int8));
        assert!(!integer_fits(-1, ScalarType::UInt8));
        assert!(!integer_fits(0, ScalarType::Bool));
        assert!(float_fits_integer(-128.0, ScalarType::Int8));
        assert!(!float_fits_integer(128.0, ScalarType::Int8));
        assert!(float_fits_integer(255.0, ScalarType::UInt8));
        assert!(!float_fits_integer(-1.0, ScalarType::UInt8));
        assert!(!float_fits_integer(0.0, ScalarType::Float));

        assert_eq!(integer_to_bits(-1, 8), 255);
        assert_eq!(integer_from_bits(255, ScalarType::Int8).unwrap(), -1);
        assert_eq!(integer_from_bits(255, ScalarType::UInt8).unwrap(), 255);
        assert!(integer_from_bits(0, ScalarType::Bool).is_err());
        assert_eq!(
            round_float(1.0 / 3.0, ScalarType::Float32),
            (1_f32 / 3.0) as f64
        );
        assert_eq!(round_float(1.0 / 3.0, ScalarType::Float), 1.0 / 3.0);
        assert_eq!(parse_float_literal("1.5", ScalarType::Float32), Some(1.5));
        assert_eq!(parse_float_literal("1.5", ScalarType::Float), Some(1.5));
        assert_eq!(parse_float_literal("1.5", ScalarType::Int), None);

        for operator in [
            HirBinaryOperator::Multiply,
            HirBinaryOperator::Divide,
            HirBinaryOperator::Add,
            HirBinaryOperator::Subtract,
        ] {
            assert!(float_binary(operator, 6.0, 2.0, ScalarType::Float).is_ok());
            assert!(float_binary(operator, 6.0, 2.0, ScalarType::Float32).is_ok());
        }
        assert!(float_binary(HirBinaryOperator::Equal, 1.0, 1.0, ScalarType::Float).is_err());
        assert!(float_binary(HirBinaryOperator::Equal, 1.0, 1.0, ScalarType::Float32).is_err());
        assert_eq!(
            integer_to_float(16_777_217, ScalarType::Float32),
            16_777_216.0
        );
        assert_eq!(integer_to_float(42, ScalarType::Float), 42.0);
        assert_eq!(numeric_body("1_2.5f32"), "12.5");

        for (spelling, expected) in [
            ("0b1010u8", Some(10)),
            ("0o17i16", Some(15)),
            ("0xffu32", Some(255)),
            ("1_000i64", Some(1_000)),
            ("42", Some(42)),
            ("nope", None),
        ] {
            assert_eq!(integer_magnitude(spelling), expected, "{spelling}");
        }
        assert_eq!(decode_char_literal("'x'"), Some('x'));
        assert_eq!(decode_char_literal("'\\n'"), Some('\n'));
        assert_eq!(decode_char_literal("'xy'"), None);
        assert_eq!(decode_char_literal("x"), None);
        assert_eq!(
            decode_string_literal("\"a\\n{{b}}\""),
            Some("a\n{b}".into())
        );
        assert_eq!(decode_string_literal("r\"a\\n\""), Some("a\\n".into()));
        assert_eq!(
            decode_string_literal("\"\"\"\n    one\n      two\n    \"\"\""),
            Some("one\n  two".into())
        );
        assert_eq!(
            decode_string_literal("r\"\"\"\n  a\\n\n  \"\"\""),
            Some("a\\n".into())
        );
        assert_eq!(decode_string_literal("not-a-string"), None);
        assert_eq!(
            decode_escaped_text("\\r\\t\\\\\\'\\\"\\0", false),
            Some("\r\t\\'\"\0".into())
        );
        assert_eq!(decode_escaped_text("\\u{1f642}", false), Some("🙂".into()));
        for malformed in [
            "\\",
            "\\x",
            "\\u0",
            "\\u{}",
            "\\u{1234567}",
            "\\u{110000}",
            "{",
        ] {
            assert_eq!(decode_escaped_text(malformed, true), None, "{malformed:?}");
        }
        assert_eq!(normalize_multiline_string("plain"), "plain");
        assert_eq!(normalize_multiline_string("\r\n\tline\r\n\t"), "line");
        assert_eq!(normalize_multiline_string("\nleft\n  tail"), "left\n  tail");
    }
}
