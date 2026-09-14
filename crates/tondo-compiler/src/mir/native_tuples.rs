//! Lower local scalar tuples to independent native locals. This does not
//! introduce a tuple calling convention or a heap/storage ABI. Copies snapshot
//! every field before writing the destination, preserving Tondo value semantics.

use super::*;
use crate::types::TypeKind;

// Bound expansion even when a small source copies one wide tuple repeatedly.
const MAX_ADDITIONAL_LOCALS: u32 = 65_536;

pub(super) fn scalar_tuple_fields(interner: &TypeInterner, ty: TypeId) -> Option<&[TypeId]> {
    let TypeKind::Tuple(fields) = interner.kind(ty).ok()? else {
        return None;
    };
    fields
        .iter()
        .all(|field| {
            matches!(
                interner.kind(*field),
                Ok(TypeKind::Scalar(ScalarType::Bool | ScalarType::Int))
            )
        })
        .then_some(fields)
}

pub(super) fn lower_local_tuples(
    function: &MirFunction,
    interner: &TypeInterner,
    blocks: &mut [MirBackendBlock],
    unsupported: &mut Vec<String>,
) {
    let Ok(mut next_local) = u32::try_from(function.locals.len()) else {
        unsupported.push("tuple:local-limit".to_owned());
        return;
    };
    let local_limit = next_local.saturating_add(MAX_ADDITIONAL_LOCALS);
    let mut tuples = BTreeMap::new();
    for (index, local) in function.locals.iter().enumerate() {
        let Some(fields) = scalar_tuple_fields(interner, local.ty()) else {
            continue;
        };
        let Some(end) = u32::try_from(fields.len())
            .ok()
            .and_then(|length| next_local.checked_add(length))
            .filter(|end| *end <= local_limit)
        else {
            unsupported.push("tuple:local-limit".to_owned());
            return;
        };
        tuples.insert(index as u32, (next_local..end).collect::<Vec<_>>());
        next_local = end;
    }
    if tuples.is_empty() {
        return;
    }
    for block in blocks {
        let mut statements = Vec::with_capacity(block.statements.len());
        for mut statement in std::mem::take(&mut block.statements) {
            if let MirBackendStatement::Assign { destination, value } = &statement
                && let Some(fields) = tuples.get(destination)
            {
                let values = match value {
                    MirBackendRvalue::Aggregate { kind, values } if kind == "tuple" => {
                        Some(values.clone())
                    }
                    MirBackendRvalue::Use(MirBackendOperand::Local { index }) => {
                        tuples.get(index).map(|source| {
                            source
                                .iter()
                                .map(|index| MirBackendOperand::Local { index: *index })
                                .collect()
                        })
                    }
                    _ => None,
                };
                if let Some(mut values) = values.filter(|values| values.len() == fields.len()) {
                    // Parallel assignment must also work when a tuple's own
                    // fields occur in the RHS in a different order.
                    let Some(end) = next_local
                        .checked_add(fields.len() as u32)
                        .filter(|end| *end <= local_limit)
                    else {
                        unsupported.push("tuple:local-limit".to_owned());
                        return;
                    };
                    for (temporary, operand) in (next_local..end).zip(&mut values) {
                        lower_operand(operand, &tuples, unsupported);
                        statements.push(MirBackendStatement::Assign {
                            destination: temporary,
                            value: MirBackendRvalue::Use(operand.clone()),
                        });
                    }
                    for (field, temporary) in fields.iter().zip(next_local..end) {
                        statements.push(MirBackendStatement::Assign {
                            destination: *field,
                            value: MirBackendRvalue::Use(MirBackendOperand::Local {
                                index: temporary,
                            }),
                        });
                    }
                    next_local = end;
                    continue;
                }
                unsupported.push("tuple:assignment-storage".to_owned());
            }
            match &mut statement {
                MirBackendStatement::Assign { value, .. } => {
                    lower_rvalue(value, &tuples, unsupported);
                }
                MirBackendStatement::Runtime { arguments, .. } => {
                    for argument in arguments {
                        lower_operand(argument, &tuples, unsupported);
                    }
                }
                MirBackendStatement::Marker { .. } => {}
            }
            statements.push(statement);
        }
        block.statements = statements;
        match &mut block.terminator {
            MirBackendTerminator::SwitchBool { condition, .. } => {
                lower_operand(condition, &tuples, unsupported);
            }
            MirBackendTerminator::SwitchTag { value, .. } => {
                lower_operand(value, &tuples, unsupported);
            }
            MirBackendTerminator::Invoke { operation, .. } => {
                lower_operation(operation, &tuples, unsupported);
            }
            MirBackendTerminator::Return
            | MirBackendTerminator::Goto { .. }
            | MirBackendTerminator::Marker { .. } => {}
        }
    }
}

fn lower_operand(
    operand: &mut MirBackendOperand,
    tuples: &BTreeMap<u32, Vec<u32>>,
    unsupported: &mut Vec<String>,
) {
    match operand {
        MirBackendOperand::Projection { index, depth, kind } if kind.starts_with("tuple:") => {
            let field = kind
                .strip_prefix("tuple:")
                .and_then(|field| field.parse::<usize>().ok());
            if *depth == 1
                && let Some(local) = tuples
                    .get(index)
                    .and_then(|fields| field.and_then(|field| fields.get(field)))
            {
                *operand = MirBackendOperand::Local { index: *local };
            } else {
                unsupported.push("operand:tuple-storage".to_owned());
            }
        }
        MirBackendOperand::Local { index } | MirBackendOperand::Borrow { index }
            if tuples.contains_key(index) =>
        {
            // Whole tuple equality, calls and other consumers require their
            // own lowering. Never compare fabricated carrier identities.
            unsupported.push("operand:whole-tuple".to_owned());
        }
        _ => {}
    }
}

fn lower_rvalue(
    value: &mut MirBackendRvalue,
    tuples: &BTreeMap<u32, Vec<u32>>,
    unsupported: &mut Vec<String>,
) {
    match value {
        MirBackendRvalue::Use(operand)
        | MirBackendRvalue::Prefix { operand, .. }
        | MirBackendRvalue::NumericConversion { operand, .. }
        | MirBackendRvalue::Coerce { operand, .. } => lower_operand(operand, tuples, unsupported),
        MirBackendRvalue::Binary { left, right, .. } => {
            lower_operand(left, tuples, unsupported);
            lower_operand(right, tuples, unsupported);
        }
        MirBackendRvalue::Aggregate { values, .. }
        | MirBackendRvalue::HostCall {
            arguments: values, ..
        } => {
            for operand in values {
                lower_operand(operand, tuples, unsupported);
            }
        }
        MirBackendRvalue::Tag { .. } | MirBackendRvalue::Unsupported { .. } => {}
    }
}

fn lower_operation(
    operation: &mut MirBackendOperation,
    tuples: &BTreeMap<u32, Vec<u32>>,
    unsupported: &mut Vec<String>,
) {
    match operation {
        MirBackendOperation::CheckedPrefix { operand, .. }
        | MirBackendOperation::JoinValue { operand } => lower_operand(operand, tuples, unsupported),
        MirBackendOperation::CheckedBinary { left, right, .. } => {
            lower_operand(left, tuples, unsupported);
            lower_operand(right, tuples, unsupported);
        }
        MirBackendOperation::BoundsCheck { index, length } => {
            lower_operand(index, tuples, unsupported);
            lower_operand(length, tuples, unsupported);
        }
        MirBackendOperation::Call { arguments, .. }
        | MirBackendOperation::HostCall { arguments, .. }
        | MirBackendOperation::Runtime { arguments, .. } => {
            for argument in arguments {
                lower_operand(argument, tuples, unsupported);
            }
        }
        MirBackendOperation::Spawn { operation, .. } => {
            lower_operation(operation, tuples, unsupported);
        }
        MirBackendOperation::Assert { condition } => lower_operand(condition, tuples, unsupported),
        MirBackendOperation::Trap { .. } | MirBackendOperation::Marker { .. } => {}
    }
}
