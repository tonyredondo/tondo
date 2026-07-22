use std::collections::BTreeMap;

use crate::hir::{
    HirAssignmentOperator, HirAssignmentTarget, HirAssignmentTargetKind, HirBinaryOperator,
    HirBootstrapHostFunction, HirCallProtocol, HirCallableSignature, HirClosure, HirExpression,
    HirExpressionId, HirExpressionKind, HirForKind, HirIterationProtocol, HirLiteral, HirLoopId,
    HirNominalShape, HirPatternId, HirPatternKind, HirPreludeTraitMethod, HirProgram, HirStatement,
    HirValueCategory, HirVariantPayload, HirVariantValue, verify_typed_hir,
};
use crate::resolve::{LocalId, ResolvedProgram};
use crate::source::Span;
use crate::types::{ScalarType, TypeId, TypeKind};

use super::{
    MirAggregateKind, MirAssertMessagePart, MirBasicBlock, MirBlockId, MirBlockKind,
    MirBootstrapHostFunction, MirCallArgument, MirConstant, MirError, MirFunction, MirFunctionId,
    MirLocal, MirLocalId, MirLocalKind, MirOperand, MirOperandKind, MirOperation, MirOperationKind,
    MirPlace, MirProgram, MirProjection, MirProjectionKind, MirRvalue, MirRvalueKind, MirStatement,
    MirStatementKind, MirTag, MirTerminator, MirTerminatorKind, MirVerificationLimits,
    verify_mir_with_limits,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MirLoweringLimits {
    pub max_functions: u32,
    pub max_blocks_per_function: u32,
    pub max_locals_per_function: u32,
    pub max_statements_per_function: u32,
    pub max_verification_steps: u64,
}

impl Default for MirLoweringLimits {
    fn default() -> Self {
        Self {
            max_functions: 100_000,
            max_blocks_per_function: 1_000_000,
            max_locals_per_function: 1_000_000,
            max_statements_per_function: 4_000_000,
            max_verification_steps: 32_000_000,
        }
    }
}

pub fn lower_to_mir(
    resolved: &ResolvedProgram,
    hir: &HirProgram,
    limits: MirLoweringLimits,
) -> Result<MirProgram, MirError> {
    verify_typed_hir(resolved, hir)?;
    let mut functions = BTreeMap::new();
    for callable in hir.callables() {
        let Some(body) = hir.body(callable.id()) else {
            continue;
        };
        if functions.len() >= limits.max_functions as usize {
            return Err(MirError::NodeLimit {
                span: callable.span(),
                resource: "function",
            });
        }
        let function = FunctionBuilder::new(hir, callable, limits)?.lower(body.root())?;
        functions.insert(MirFunctionId::Callable(callable.id()), function);
    }
    for closure in hir.closures() {
        if functions.len() >= limits.max_functions as usize {
            return Err(MirError::NodeLimit {
                span: closure.span(),
                resource: "function",
            });
        }
        let function =
            FunctionBuilder::new_closure(hir, closure, limits)?.lower(closure.body().root())?;
        functions.insert(MirFunctionId::Closure(closure.id()), function);
    }
    let program = MirProgram { functions };
    if let Err(error) = verify_mir_with_limits(
        resolved,
        hir,
        &program,
        MirVerificationLimits {
            max_dataflow_steps: limits.max_verification_steps,
        },
    ) {
        if error.is_resource_limit() {
            return Err(MirError::VerificationLimit {
                resource: "verification dataflow",
            });
        }
        return Err(error.into());
    }
    Ok(program)
}

struct OpenBlock {
    kind: MirBlockKind,
    statements: Vec<MirStatement>,
    terminator: Option<MirTerminator>,
}

struct FunctionBuilder<'a> {
    hir: &'a HirProgram,
    id: MirFunctionId,
    span: Span,
    outcome: TypeId,
    limits: MirLoweringLimits,
    statement_count: u32,
    locals: Vec<MirLocal>,
    parameters: Vec<MirLocalId>,
    source_locals: BTreeMap<LocalId, MirLocalId>,
    capture_places: BTreeMap<LocalId, MirPlace>,
    loops: BTreeMap<HirLoopId, LoopTargets>,
    receiver: Option<MirLocalId>,
    return_local: MirLocalId,
    entry: MirBlockId,
    unwind: MirBlockId,
    blocks: Vec<OpenBlock>,
}

#[derive(Clone, Copy)]
struct LoopTargets {
    break_target: MirBlockId,
    continue_target: MirBlockId,
}

enum LoweredAssignmentTarget {
    Place {
        place: MirPlace,
        coercion: crate::types::Assignability,
    },
    Discard,
    Tuple {
        ty: TypeId,
        items: Vec<LoweredAssignmentTarget>,
    },
}

fn assignment_target_contains_slice(target: &LoweredAssignmentTarget) -> bool {
    match target {
        LoweredAssignmentTarget::Place { place, .. } => matches!(
            place.projections.last().map(MirProjection::kind),
            Some(MirProjectionKind::Slice { .. })
        ),
        LoweredAssignmentTarget::Discard => false,
        LoweredAssignmentTarget::Tuple { items, .. } => {
            items.iter().any(assignment_target_contains_slice)
        }
    }
}

impl<'a> FunctionBuilder<'a> {
    fn new(
        hir: &'a HirProgram,
        callable: &HirCallableSignature,
        limits: MirLoweringLimits,
    ) -> Result<Self, MirError> {
        let id = MirFunctionId::Callable(callable.id());
        let span = callable.span();
        let mut builder = Self {
            hir,
            id,
            span,
            outcome: callable.outcome(),
            limits,
            statement_count: 0,
            locals: Vec::new(),
            parameters: Vec::new(),
            source_locals: BTreeMap::new(),
            capture_places: BTreeMap::new(),
            loops: BTreeMap::new(),
            receiver: None,
            return_local: MirLocalId(0),
            entry: MirBlockId(0),
            unwind: MirBlockId(0),
            blocks: Vec::new(),
        };
        builder.return_local =
            builder.allocate_local(callable.outcome(), span, MirLocalKind::Return)?;
        for (index, parameter) in callable.parameters().iter().enumerate() {
            let local = builder.allocate_local(
                parameter.ty(),
                parameter.span(),
                MirLocalKind::Parameter {
                    index: index as u32,
                    source: parameter.local(),
                },
            )?;
            builder.parameters.push(local);
            if parameter.is_receiver() {
                builder.receiver = Some(local);
            } else if let Some(source) = parameter.local() {
                builder.source_locals.insert(source, local);
            }
        }
        builder.entry = builder.allocate_block(MirBlockKind::Normal)?;
        builder.unwind = builder.allocate_block(MirBlockKind::Cleanup)?;
        builder.terminate(builder.unwind, span, MirTerminatorKind::ResumePanic)?;
        Ok(builder)
    }

    fn new_closure(
        hir: &'a HirProgram,
        closure: &HirClosure,
        limits: MirLoweringLimits,
    ) -> Result<Self, MirError> {
        let TypeKind::Function(function) =
            hir.interner()
                .kind(closure.function_type())
                .map_err(|error| MirError::Construction {
                    span: closure.span(),
                    message: format!("closure has an invalid call signature: {error}"),
                })?
        else {
            return Err(MirError::Construction {
                span: closure.span(),
                message: "closure call signature is not a function type".into(),
            });
        };
        let span = closure.span();
        let mut builder = Self {
            hir,
            id: MirFunctionId::Closure(closure.id()),
            span,
            outcome: function.outcome(),
            limits,
            statement_count: 0,
            locals: Vec::new(),
            parameters: Vec::new(),
            source_locals: BTreeMap::new(),
            capture_places: BTreeMap::new(),
            loops: BTreeMap::new(),
            receiver: None,
            return_local: MirLocalId(0),
            entry: MirBlockId(0),
            unwind: MirBlockId(0),
            blocks: Vec::new(),
        };
        builder.return_local =
            builder.allocate_local(function.outcome(), span, MirLocalKind::Return)?;
        let environment = builder.allocate_local(
            closure.ty(),
            span,
            MirLocalKind::Parameter {
                index: 0,
                source: None,
            },
        )?;
        builder.parameters.push(environment);
        for (index, capture) in closure.captures().iter().enumerate() {
            let mut place = builder.local_place(environment);
            place.ty = capture.ty();
            place.projections.push(MirProjection {
                ty: capture.ty(),
                kind: MirProjectionKind::ClosureCapture {
                    closure: closure.id(),
                    index: u32::try_from(index).map_err(|_| MirError::NodeLimit {
                        span,
                        resource: "closure capture",
                    })?,
                },
            });
            builder.capture_places.insert(capture.local(), place);
        }
        for (index, parameter) in closure.parameters().iter().enumerate() {
            let parameter_index = u32::try_from(index)
                .ok()
                .and_then(|index| index.checked_add(1))
                .ok_or(MirError::NodeLimit {
                    span,
                    resource: "parameter",
                })?;
            let local = builder.allocate_local(
                parameter.ty(),
                parameter.span(),
                MirLocalKind::Parameter {
                    index: parameter_index,
                    source: parameter.local(),
                },
            )?;
            builder.parameters.push(local);
            if let Some(source) = parameter.local() {
                builder.source_locals.insert(source, local);
            }
        }
        builder.entry = builder.allocate_block(MirBlockKind::Normal)?;
        builder.unwind = builder.allocate_block(MirBlockKind::Cleanup)?;
        builder.terminate(builder.unwind, span, MirTerminatorKind::ResumePanic)?;
        Ok(builder)
    }

    fn lower(mut self, root: HirExpressionId) -> Result<MirFunction, MirError> {
        let return_place = self.local_place(self.return_local);
        if let Some(end) = self.lower_expression(root, return_place, self.entry)? {
            let span = self.expression(root)?.span();
            self.terminate(end, span, MirTerminatorKind::Return)?;
        }
        let mut blocks = Vec::with_capacity(self.blocks.len());
        for block in self.blocks {
            let terminator = block.terminator.ok_or_else(|| MirError::Construction {
                span: self.span,
                message: "a generated basic block has no terminator".into(),
            })?;
            blocks.push(MirBasicBlock {
                kind: block.kind,
                statements: block.statements,
                terminator,
            });
        }
        Ok(MirFunction {
            id: self.id,
            span: self.span,
            outcome: self.outcome,
            locals: self.locals,
            parameters: self.parameters,
            return_local: self.return_local,
            entry: self.entry,
            unwind: self.unwind,
            blocks,
        })
    }

    fn lower_expression(
        &mut self,
        id: HirExpressionId,
        destination: MirPlace,
        block: MirBlockId,
    ) -> Result<Option<MirBlockId>, MirError> {
        let expression = self.expression(id)?.clone();
        let span = expression.span();
        match expression.kind() {
            HirExpressionKind::Literal(literal) => {
                let value = match literal {
                    HirLiteral::None => MirRvalue {
                        ty: expression.ty(),
                        kind: MirRvalueKind::Aggregate {
                            shape: MirAggregateKind::OptionNone,
                            values: Vec::new(),
                        },
                    },
                    _ => MirRvalue {
                        ty: expression.ty(),
                        kind: MirRvalueKind::Use(self.literal_operand(expression.ty(), literal)),
                    },
                };
                self.assign(block, span, destination, value)?;
                Ok(Some(block))
            }
            HirExpressionKind::Local(local) => {
                let place = self.source_place(*local, span)?;
                self.assign_operand(
                    block,
                    span,
                    destination,
                    MirOperand {
                        ty: place.ty,
                        kind: MirOperandKind::Copy(place),
                    },
                )?;
                Ok(Some(block))
            }
            HirExpressionKind::Receiver => {
                let local = self.receiver.ok_or_else(|| MirError::Construction {
                    span,
                    message: "receiver expression has no receiver local".into(),
                })?;
                self.assign_operand(block, span, destination, self.copy_local(local))?;
                Ok(Some(block))
            }
            HirExpressionKind::Constant(symbol) => {
                self.assign_operand(
                    block,
                    span,
                    destination,
                    MirOperand {
                        ty: expression.ty(),
                        kind: MirOperandKind::Constant(MirConstant::Named(*symbol)),
                    },
                )?;
                Ok(Some(block))
            }
            HirExpressionKind::Function(callable) => {
                self.assign_operand(
                    block,
                    span,
                    destination,
                    MirOperand {
                        ty: expression.ty(),
                        kind: MirOperandKind::Function {
                            callable: *callable,
                            arguments: Vec::new(),
                        },
                    },
                )?;
                Ok(Some(block))
            }
            HirExpressionKind::SpecializedFunction {
                callable,
                arguments,
            } => {
                self.assign_operand(
                    block,
                    span,
                    destination,
                    MirOperand {
                        ty: expression.ty(),
                        kind: MirOperandKind::Function {
                            callable: *callable,
                            arguments: arguments.clone(),
                        },
                    },
                )?;
                Ok(Some(block))
            }
            HirExpressionKind::PreludeTraitFunction { method, arguments } => {
                self.assign_operand(
                    block,
                    span,
                    destination,
                    MirOperand {
                        ty: expression.ty(),
                        kind: MirOperandKind::PreludeTraitFunction {
                            method: *method,
                            arguments: arguments.clone(),
                        },
                    },
                )?;
                Ok(Some(block))
            }
            HirExpressionKind::Closure(closure_id) => {
                let closure =
                    self.hir
                        .closure(*closure_id)
                        .ok_or_else(|| MirError::Construction {
                            span,
                            message: format!("closure#{} has no HIR metadata", closure_id.index()),
                        })?;
                let captures = closure
                    .captures()
                    .iter()
                    .map(|capture| (capture.local(), capture.ty()))
                    .collect::<Vec<_>>();
                let mut values = Vec::with_capacity(captures.len());
                for (local, ty) in captures {
                    let place = self.source_place(local, span)?;
                    let operand = MirOperand {
                        ty: place.ty,
                        kind: MirOperandKind::Copy(place),
                    };
                    if operand.ty != ty {
                        return Err(MirError::Construction {
                            span,
                            message: "closure capture type differs from its source local".into(),
                        });
                    }
                    values.push(operand);
                }
                let arguments =
                    match self.hir.interner().kind(expression.ty()).map_err(|error| {
                        MirError::Construction {
                            span,
                            message: format!("closure has an invalid generated type: {error}"),
                        }
                    })? {
                        TypeKind::Generated { arguments, .. } => arguments.clone(),
                        _ => {
                            return Err(MirError::Construction {
                                span,
                                message: "closure expression has a non-generated type".into(),
                            });
                        }
                    };
                self.assign(
                    block,
                    span,
                    destination,
                    MirRvalue {
                        ty: expression.ty(),
                        kind: MirRvalueKind::Aggregate {
                            shape: MirAggregateKind::Closure {
                                closure: *closure_id,
                                arguments,
                            },
                            values,
                        },
                    },
                )?;
                Ok(Some(block))
            }
            HirExpressionKind::Tuple(values) => self.lower_aggregate(
                values,
                MirAggregateKind::Tuple,
                expression.ty(),
                span,
                destination,
                block,
            ),
            HirExpressionKind::Array(values) => self.lower_aggregate(
                values,
                MirAggregateKind::Array,
                expression.ty(),
                span,
                destination,
                block,
            ),
            HirExpressionKind::Set(values) => self.lower_aggregate(
                values,
                MirAggregateKind::Set,
                expression.ty(),
                span,
                destination,
                block,
            ),
            HirExpressionKind::Map {
                entries,
                reject_dynamic_duplicates,
            } => {
                let mut current = block;
                let mut values = Vec::with_capacity(entries.len());
                for entry in entries {
                    let Some((next, key)) = self.lower_value(entry.key(), current)? else {
                        return Ok(None);
                    };
                    let Some((next, value)) = self.lower_value(entry.value(), next)? else {
                        return Ok(None);
                    };
                    current = next;
                    values.push((key, value));
                }
                self.invoke(
                    current,
                    span,
                    Some(destination),
                    MirOperation {
                        ty: expression.ty(),
                        kind: MirOperationKind::BuildMap {
                            entries: values,
                            reject_dynamic_duplicates: *reject_dynamic_duplicates,
                        },
                    },
                )
            }
            HirExpressionKind::Newtype { constructor, value } => {
                let Some((block, value)) = self.lower_value(*value, block)? else {
                    return Ok(None);
                };
                self.assign(
                    block,
                    span,
                    destination,
                    MirRvalue {
                        ty: expression.ty(),
                        kind: MirRvalueKind::Aggregate {
                            shape: MirAggregateKind::Newtype {
                                owner: *constructor,
                            },
                            values: vec![value],
                        },
                    },
                )?;
                Ok(Some(block))
            }
            HirExpressionKind::Record { owner, fields } => {
                let mut current = block;
                let mut values = Vec::with_capacity(fields.len());
                let mut members = Vec::with_capacity(fields.len());
                for field in fields {
                    let Some((next, value)) = self.lower_value(field.value(), current)? else {
                        return Ok(None);
                    };
                    current = next;
                    values.push(value);
                    members.push(field.member());
                }
                self.assign(
                    current,
                    span,
                    destination,
                    MirRvalue {
                        ty: expression.ty(),
                        kind: MirRvalueKind::Aggregate {
                            shape: MirAggregateKind::Record {
                                owner: *owner,
                                fields: members,
                            },
                            values,
                        },
                    },
                )?;
                Ok(Some(current))
            }
            HirExpressionKind::Variant { variant, payload } => {
                let mut current = block;
                let mut values = Vec::new();
                let fields = match payload {
                    HirVariantValue::Unit => Vec::new(),
                    HirVariantValue::Tuple(items) => {
                        for item in items {
                            let Some((next, value)) = self.lower_value(*item, current)? else {
                                return Ok(None);
                            };
                            current = next;
                            values.push(value);
                        }
                        vec![None; items.len()]
                    }
                    HirVariantValue::Record(items) => {
                        let mut members = Vec::with_capacity(items.len());
                        for item in items {
                            let Some((next, value)) = self.lower_value(item.value(), current)?
                            else {
                                return Ok(None);
                            };
                            current = next;
                            values.push(value);
                            members.push(Some(item.member()));
                        }
                        members
                    }
                };
                self.assign(
                    current,
                    span,
                    destination,
                    MirRvalue {
                        ty: expression.ty(),
                        kind: MirRvalueKind::Aggregate {
                            shape: MirAggregateKind::Variant {
                                variant: *variant,
                                fields,
                            },
                            values,
                        },
                    },
                )?;
                Ok(Some(current))
            }
            HirExpressionKind::RecordUpdate { base, fields } => {
                let Some((mut current, base)) = self.lower_value(*base, block)? else {
                    return Ok(None);
                };
                let mut updates = Vec::with_capacity(fields.len());
                for field in fields {
                    let Some((next, value)) = self.lower_value(field.value(), current)? else {
                        return Ok(None);
                    };
                    current = next;
                    updates.push((field.member(), value));
                }
                self.assign(
                    current,
                    span,
                    destination,
                    MirRvalue {
                        ty: expression.ty(),
                        kind: MirRvalueKind::RecordUpdate {
                            base,
                            fields: updates,
                        },
                    },
                )?;
                Ok(Some(current))
            }
            HirExpressionKind::NumericConversion {
                target,
                conversion,
                value,
            } => {
                let Some((block, value)) = self.lower_value(*value, block)? else {
                    return Ok(None);
                };
                self.assign(
                    block,
                    span,
                    destination,
                    MirRvalue {
                        ty: expression.ty(),
                        kind: MirRvalueKind::NumericConversion {
                            target: *target,
                            conversion: *conversion,
                            value,
                        },
                    },
                )?;
                Ok(Some(block))
            }
            HirExpressionKind::Block { statements, tail } => {
                self.lower_block(statements, *tail, expression.ty(), span, destination, block)
            }
            HirExpressionKind::Prefix { operator, operand } => {
                let Some((block, operand)) = self.lower_value(*operand, block)? else {
                    return Ok(None);
                };
                if self.prefix_may_panic(*operator, operand.ty) {
                    self.invoke(
                        block,
                        span,
                        Some(destination),
                        MirOperation {
                            ty: expression.ty(),
                            kind: MirOperationKind::CheckedPrefix {
                                operator: *operator,
                                operand,
                            },
                        },
                    )
                } else {
                    self.assign(
                        block,
                        span,
                        destination,
                        MirRvalue {
                            ty: expression.ty(),
                            kind: MirRvalueKind::Prefix {
                                operator: *operator,
                                operand,
                            },
                        },
                    )?;
                    Ok(Some(block))
                }
            }
            HirExpressionKind::Binary {
                operator,
                left,
                right,
            } if matches!(
                operator,
                HirBinaryOperator::LogicalAnd | HirBinaryOperator::LogicalOr
            ) =>
            {
                self.lower_logical(*operator, *left, *right, span, destination, block)
            }
            HirExpressionKind::Binary {
                operator,
                left,
                right,
            } => {
                let Some((block, left)) = self.lower_value(*left, block)? else {
                    return Ok(None);
                };
                let Some((block, right)) = self.lower_value(*right, block)? else {
                    return Ok(None);
                };
                if self.binary_may_panic(*operator, left.ty) {
                    self.invoke(
                        block,
                        span,
                        Some(destination),
                        MirOperation {
                            ty: expression.ty(),
                            kind: MirOperationKind::CheckedBinary {
                                operator: *operator,
                                left,
                                right,
                            },
                        },
                    )
                } else {
                    self.assign(
                        block,
                        span,
                        destination,
                        MirRvalue {
                            ty: expression.ty(),
                            kind: MirRvalueKind::Binary {
                                operator: *operator,
                                left,
                                right,
                            },
                        },
                    )?;
                    Ok(Some(block))
                }
            }
            HirExpressionKind::Range { kind, start, end } => {
                let Some((block, start)) = self.lower_value(*start, block)? else {
                    return Ok(None);
                };
                let Some((block, end)) = self.lower_value(*end, block)? else {
                    return Ok(None);
                };
                self.assign(
                    block,
                    span,
                    destination,
                    MirRvalue {
                        ty: expression.ty(),
                        kind: MirRvalueKind::Range {
                            kind: *kind,
                            start,
                            end,
                        },
                    },
                )?;
                Ok(Some(block))
            }
            HirExpressionKind::Contains {
                kind,
                item,
                container,
            } => {
                let Some((block, item)) = self.lower_value(*item, block)? else {
                    return Ok(None);
                };
                let Some((block, container)) = self.lower_value(*container, block)? else {
                    return Ok(None);
                };
                self.assign(
                    block,
                    span,
                    destination,
                    MirRvalue {
                        ty: expression.ty(),
                        kind: MirRvalueKind::Contains {
                            kind: *kind,
                            item,
                            container,
                        },
                    },
                )?;
                Ok(Some(block))
            }
            HirExpressionKind::Field { .. } | HirExpressionKind::TupleField { .. } => {
                let Some((block, place)) = self.lower_place(id, block)? else {
                    return Ok(None);
                };
                self.assign_operand(
                    block,
                    span,
                    destination,
                    MirOperand {
                        ty: expression.ty(),
                        kind: MirOperandKind::Copy(place),
                    },
                )?;
                Ok(Some(block))
            }
            HirExpressionKind::Index {
                base,
                index,
                access,
            } => {
                let Some((block, base)) = self.lower_value(*base, block)? else {
                    return Ok(None);
                };
                let Some((block, index)) = self.lower_value(*index, block)? else {
                    return Ok(None);
                };
                self.invoke(
                    block,
                    span,
                    Some(destination),
                    MirOperation {
                        ty: expression.ty(),
                        kind: MirOperationKind::Index {
                            base,
                            index,
                            access: *access,
                        },
                    },
                )
            }
            HirExpressionKind::Slice {
                base,
                start,
                end,
                step,
            } => {
                let Some((mut current, base)) = self.lower_value(*base, block)? else {
                    return Ok(None);
                };
                let (next, start) = self.lower_optional_value(*start, current)?;
                let Some(next) = next else {
                    return Ok(None);
                };
                current = next;
                let (next, end) = self.lower_optional_value(*end, current)?;
                let Some(next) = next else {
                    return Ok(None);
                };
                current = next;
                let (next, step) = self.lower_optional_value(*step, current)?;
                let Some(current) = next else {
                    return Ok(None);
                };
                self.invoke(
                    current,
                    span,
                    Some(destination),
                    MirOperation {
                        ty: expression.ty(),
                        kind: MirOperationKind::Slice {
                            base,
                            start,
                            end,
                            step,
                        },
                    },
                )
            }
            HirExpressionKind::Call {
                callee,
                arguments,
                signature,
                protocol,
            } => {
                let Some((mut current, callee)) = self.lower_callee(*callee, *protocol, block)?
                else {
                    return Ok(None);
                };
                let mut lowered = Vec::with_capacity(arguments.len());
                for argument in arguments {
                    let Some((next, value)) = self.lower_value(argument.value(), current)? else {
                        return Ok(None);
                    };
                    current = next;
                    lowered.push(MirCallArgument {
                        mode: argument.mode(),
                        target: argument.target(),
                        value,
                    });
                }
                self.invoke(
                    current,
                    span,
                    Some(destination),
                    MirOperation {
                        ty: expression.ty(),
                        kind: MirOperationKind::Call {
                            callee,
                            arguments: lowered,
                            signature: *signature,
                            protocol: *protocol,
                        },
                    },
                )
            }
            HirExpressionKind::PreludePanic { message } => {
                let Some((block, message)) = self.lower_value(*message, block)? else {
                    return Ok(None);
                };
                self.invoke(
                    block,
                    span,
                    None,
                    MirOperation {
                        ty: expression.ty(),
                        kind: MirOperationKind::ExplicitPanic { message },
                    },
                )
            }
            HirExpressionKind::PreludeAssert {
                condition,
                condition_repr,
                message_parts,
            } => {
                let Some((mut current, condition)) = self.lower_value(*condition, block)? else {
                    return Ok(None);
                };
                let mut lowered_parts = Vec::with_capacity(message_parts.len());
                for part in message_parts {
                    let Some((next, value)) = self.lower_value(part.value(), current)? else {
                        return Ok(None);
                    };
                    current = next;
                    lowered_parts.push(MirAssertMessagePart {
                        value,
                        spread: part.is_spread(),
                    });
                }
                self.invoke(
                    current,
                    span,
                    Some(destination),
                    MirOperation {
                        ty: expression.ty(),
                        kind: MirOperationKind::Assert {
                            condition,
                            condition_repr: condition_repr.clone(),
                            message_parts: lowered_parts,
                        },
                    },
                )
            }
            HirExpressionKind::BootstrapHostCall {
                function,
                arguments,
            } => {
                let Some((block, arguments)) = self.lower_values(arguments, block)? else {
                    return Ok(None);
                };
                let function = match function {
                    HirBootstrapHostFunction::ConsolePrint => {
                        MirBootstrapHostFunction::ConsolePrint
                    }
                };
                self.invoke(
                    block,
                    span,
                    Some(destination),
                    MirOperation {
                        ty: expression.ty(),
                        kind: MirOperationKind::BootstrapHostCall {
                            function,
                            arguments,
                        },
                    },
                )
            }
            HirExpressionKind::OptionSome { value } => self.lower_single_aggregate(
                *value,
                MirAggregateKind::OptionSome,
                expression.ty(),
                span,
                destination,
                block,
            ),
            HirExpressionKind::ResultOk { value } => self.lower_single_aggregate(
                *value,
                MirAggregateKind::ResultOk,
                expression.ty(),
                span,
                destination,
                block,
            ),
            HirExpressionKind::ResultErr { error } => self.lower_single_aggregate(
                *error,
                MirAggregateKind::ResultErr,
                expression.ty(),
                span,
                destination,
                block,
            ),
            HirExpressionKind::If {
                condition,
                then_branch,
                else_branch,
            } => self.lower_if(
                *condition,
                *then_branch,
                *else_branch,
                span,
                destination,
                block,
            ),
            HirExpressionKind::PropagateOption { value } => {
                self.lower_propagate_option(*value, expression.ty(), span, destination, block)
            }
            HirExpressionKind::PropagateResult {
                value,
                error_coercion,
            } => self.lower_propagate_result(
                *value,
                *error_coercion,
                expression.ty(),
                span,
                destination,
                block,
            ),
            HirExpressionKind::Match { scrutinee, arms } => {
                self.lower_match(*scrutinee, arms, span, destination, block)
            }
            HirExpressionKind::Return { value } => {
                let return_place = self.local_place(self.return_local);
                let end = if let Some(value) = value {
                    self.lower_expression(*value, return_place, block)?
                } else {
                    self.assign_operand(block, span, return_place, self.unit_operand())?;
                    Some(block)
                };
                if let Some(end) = end {
                    self.terminate(end, span, MirTerminatorKind::Return)?;
                }
                Ok(None)
            }
            HirExpressionKind::Fail { error } => {
                let Some((block, error)) = self.lower_value(*error, block)? else {
                    return Ok(None);
                };
                self.assign(
                    block,
                    span,
                    self.local_place(self.return_local),
                    MirRvalue {
                        ty: self.outcome,
                        kind: MirRvalueKind::Aggregate {
                            shape: MirAggregateKind::ResultErr,
                            values: vec![error],
                        },
                    },
                )?;
                self.terminate(block, span, MirTerminatorKind::Return)?;
                Ok(None)
            }
            HirExpressionKind::Coerce { kind, value } => {
                let Some((block, value)) = self.lower_value(*value, block)? else {
                    return Ok(None);
                };
                self.assign(
                    block,
                    span,
                    destination,
                    MirRvalue {
                        ty: expression.ty(),
                        kind: MirRvalueKind::Coerce { kind: *kind, value },
                    },
                )?;
                Ok(Some(block))
            }
            HirExpressionKind::Break { target } => {
                let target = target.ok_or_else(|| MirError::Construction {
                    span,
                    message: "verified break has no loop target".into(),
                })?;
                let targets =
                    self.loops
                        .get(&target)
                        .copied()
                        .ok_or_else(|| MirError::Construction {
                            span,
                            message: format!("break targets inactive loop#{}", target.index()),
                        })?;
                self.terminate(
                    block,
                    span,
                    MirTerminatorKind::Goto {
                        target: targets.break_target,
                    },
                )?;
                Ok(None)
            }
            HirExpressionKind::Continue { target } => {
                let target = target.ok_or_else(|| MirError::Construction {
                    span,
                    message: "verified continue has no loop target".into(),
                })?;
                let targets =
                    self.loops
                        .get(&target)
                        .copied()
                        .ok_or_else(|| MirError::Construction {
                            span,
                            message: format!("continue targets inactive loop#{}", target.index()),
                        })?;
                self.terminate(
                    block,
                    span,
                    MirTerminatorKind::Goto {
                        target: targets.continue_target,
                    },
                )?;
                Ok(None)
            }
            HirExpressionKind::Recovery | HirExpressionKind::InterpolatedString { .. } => {
                Err(MirError::Construction {
                    span,
                    message: "non-executable expression crossed the verified HIR boundary".into(),
                })
            }
        }
    }

    fn lower_block(
        &mut self,
        statements: &[HirStatement],
        tail: Option<HirExpressionId>,
        ty: TypeId,
        span: Span,
        destination: MirPlace,
        mut block: MirBlockId,
    ) -> Result<Option<MirBlockId>, MirError> {
        for statement in statements {
            let Some(next) = self.lower_statement(statement, block)? else {
                return Ok(None);
            };
            block = next;
        }
        if let Some(tail) = tail {
            self.lower_expression(tail, destination, block)
        } else {
            debug_assert_eq!(ty, self.hir.interner().scalar(ScalarType::Unit));
            self.assign_operand(block, span, destination, self.unit_operand())?;
            Ok(Some(block))
        }
    }

    fn lower_statement(
        &mut self,
        statement: &HirStatement,
        block: MirBlockId,
    ) -> Result<Option<MirBlockId>, MirError> {
        match statement {
            HirStatement::Binding { pattern, value, .. } => {
                let Some((block, value)) = self.lower_value(*value, block)? else {
                    return Ok(None);
                };
                self.bind_irrefutable(*pattern, value, block)
            }
            HirStatement::Expression { value, .. } | HirStatement::Discard { value, .. } => self
                .lower_value(*value, block)
                .map(|result| result.map(|(block, _)| block)),
            HirStatement::Assignment {
                span,
                operator,
                target,
                value,
            } => self.lower_assignment(*span, *operator, target, *value, block),
            HirStatement::For {
                span,
                id,
                kind,
                body,
            } => self.lower_for(*span, *id, kind, *body, block),
        }
    }

    fn lower_assignment(
        &mut self,
        span: Span,
        operator: HirAssignmentOperator,
        target: &HirAssignmentTarget,
        value: HirExpressionId,
        block: MirBlockId,
    ) -> Result<Option<MirBlockId>, MirError> {
        let Some((block, target)) = self.lower_assignment_target(target, block)? else {
            return Ok(None);
        };
        if operator == HirAssignmentOperator::Assign {
            let Some((block, value)) = self.lower_value(value, block)? else {
                return Ok(None);
            };
            let block =
                self.validate_assignment_places(&target, Some(&value), true, block, span)?;
            self.write_assignment_target(&target, value, block, span)?;
            return Ok(Some(block));
        }

        let LoweredAssignmentTarget::Place { place, coercion } = target else {
            return Err(MirError::Construction {
                span,
                message: "compound assignment has a non-place target tree".into(),
            });
        };
        if coercion != crate::types::Assignability::Exact {
            return Err(MirError::Construction {
                span,
                message: "compound assignment target has a contextual coercion".into(),
            });
        }
        let access_target = LoweredAssignmentTarget::Place {
            place: place.clone(),
            coercion,
        };
        let block = self.validate_assignment_places(&access_target, None, false, block, span)?;
        let previous = self.allocate_temporary(place.ty, span, block)?;
        self.assign_operand(
            block,
            span,
            self.local_place(previous),
            MirOperand {
                ty: place.ty,
                kind: MirOperandKind::Copy(place.clone()),
            },
        )?;
        let Some((block, right)) = self.lower_value(value, block)? else {
            return Ok(None);
        };
        let binary = operator
            .binary_operator()
            .ok_or_else(|| MirError::Construction {
                span,
                message: "compound assignment has no corresponding binary operator".into(),
            })?;
        let left = self.copy_local(previous);
        let result = self.allocate_temporary(left.ty, span, block)?;
        let result_place = self.local_place(result);
        let block = if self.binary_may_panic(binary, left.ty) {
            let Some(block) = self.invoke(
                block,
                span,
                Some(result_place),
                MirOperation {
                    ty: left.ty,
                    kind: MirOperationKind::CheckedBinary {
                        operator: binary,
                        left,
                        right,
                    },
                },
            )?
            else {
                return Ok(None);
            };
            block
        } else {
            self.assign(
                block,
                span,
                result_place,
                MirRvalue {
                    ty: left.ty,
                    kind: MirRvalueKind::Binary {
                        operator: binary,
                        left,
                        right,
                    },
                },
            )?;
            block
        };
        let result = self.copy_local(result);
        let block =
            self.validate_assignment_places(&access_target, Some(&result), true, block, span)?;
        self.write_assignment_target(&access_target, result, block, span)?;
        Ok(Some(block))
    }

    fn lower_assignment_target(
        &mut self,
        target: &HirAssignmentTarget,
        block: MirBlockId,
    ) -> Result<Option<(MirBlockId, LoweredAssignmentTarget)>, MirError> {
        match target.kind() {
            HirAssignmentTargetKind::Place {
                place, coercion, ..
            } => {
                let Some((block, place)) = self.lower_place(*place, block)? else {
                    return Ok(None);
                };
                Ok(Some((
                    block,
                    LoweredAssignmentTarget::Place {
                        place,
                        coercion: *coercion,
                    },
                )))
            }
            HirAssignmentTargetKind::Discard => Ok(Some((block, LoweredAssignmentTarget::Discard))),
            HirAssignmentTargetKind::Tuple(items) => {
                let mut block = block;
                let mut lowered = Vec::with_capacity(items.len());
                for item in items {
                    let Some((next, item)) = self.lower_assignment_target(item, block)? else {
                        return Ok(None);
                    };
                    block = next;
                    lowered.push(item);
                }
                Ok(Some((
                    block,
                    LoweredAssignmentTarget::Tuple {
                        ty: target.ty(),
                        items: lowered,
                    },
                )))
            }
        }
    }

    fn validate_assignment_places(
        &mut self,
        target: &LoweredAssignmentTarget,
        replacement: Option<&MirOperand>,
        for_write: bool,
        block: MirBlockId,
        span: Span,
    ) -> Result<MirBlockId, MirError> {
        let mut places = Vec::new();
        let mut replacements = Vec::new();
        self.collect_assignment_validations(
            target,
            replacement,
            for_write,
            span,
            &mut places,
            &mut replacements,
        )?;
        if places.is_empty() {
            return Ok(block);
        }
        let target_block = self.allocate_block(MirBlockKind::Normal)?;
        self.terminate(
            block,
            span,
            MirTerminatorKind::ValidatePlaces {
                places,
                replacements,
                for_write,
                target: target_block,
                unwind: self.unwind,
            },
        )?;
        Ok(target_block)
    }

    fn collect_assignment_validations(
        &mut self,
        target: &LoweredAssignmentTarget,
        replacement: Option<&MirOperand>,
        for_write: bool,
        span: Span,
        places: &mut Vec<MirPlace>,
        replacements: &mut Vec<Option<MirOperand>>,
    ) -> Result<(), MirError> {
        match target {
            LoweredAssignmentTarget::Place { place, coercion } => {
                let slice = matches!(
                    place.projections.last().map(MirProjection::kind),
                    Some(MirProjectionKind::Slice { .. })
                );
                let replacement = if for_write && slice {
                    if *coercion != crate::types::Assignability::Exact {
                        return Err(MirError::Construction {
                            span,
                            message: "slice assignment cannot defer a contextual coercion".into(),
                        });
                    }
                    Some(replacement.cloned().ok_or_else(|| MirError::Construction {
                        span,
                        message: "slice write validation has no replacement value".into(),
                    })?)
                } else {
                    None
                };
                places.push(place.clone());
                replacements.push(replacement);
            }
            LoweredAssignmentTarget::Discard => {}
            LoweredAssignmentTarget::Tuple { ty, items } => {
                for (index, item) in items.iter().enumerate() {
                    let projected = if for_write && assignment_target_contains_slice(item) {
                        let value = replacement.ok_or_else(|| MirError::Construction {
                            span,
                            message: "tuple slice write validation has no replacement value".into(),
                        })?;
                        Some(self.project_operand(
                            value,
                            MirProjection {
                                ty: self.assignment_target_item_type(*ty, index, item, span)?,
                                kind: MirProjectionKind::TupleField(index as u32),
                            },
                            span,
                        )?)
                    } else {
                        None
                    };
                    self.collect_assignment_validations(
                        item,
                        projected.as_ref(),
                        for_write,
                        span,
                        places,
                        replacements,
                    )?;
                }
            }
        }
        Ok(())
    }

    fn assignment_target_item_type(
        &self,
        tuple: TypeId,
        index: usize,
        item: &LoweredAssignmentTarget,
        span: Span,
    ) -> Result<TypeId, MirError> {
        match item {
            LoweredAssignmentTarget::Place { place, .. } => Ok(place.ty),
            LoweredAssignmentTarget::Tuple { ty, .. } => Ok(*ty),
            LoweredAssignmentTarget::Discard => {
                let TypeKind::Tuple(types) =
                    self.hir
                        .interner()
                        .kind(tuple)
                        .map_err(|_| MirError::Construction {
                            span,
                            message: "tuple assignment target has an invalid type".into(),
                        })?
                else {
                    return Err(MirError::Construction {
                        span,
                        message: "multiple-assignment target is not a tuple".into(),
                    });
                };
                types
                    .get(index)
                    .copied()
                    .ok_or_else(|| MirError::Construction {
                        span,
                        message: "multiple-assignment target index is out of range".into(),
                    })
            }
        }
    }

    fn write_assignment_target(
        &mut self,
        target: &LoweredAssignmentTarget,
        value: MirOperand,
        block: MirBlockId,
        span: Span,
    ) -> Result<(), MirError> {
        match target {
            LoweredAssignmentTarget::Place { place, coercion } => {
                if *coercion == crate::types::Assignability::Exact {
                    self.assign_operand(block, span, place.clone(), value)
                } else {
                    self.assign(
                        block,
                        span,
                        place.clone(),
                        MirRvalue {
                            ty: place.ty,
                            kind: MirRvalueKind::Coerce {
                                kind: *coercion,
                                value,
                            },
                        },
                    )
                }
            }
            LoweredAssignmentTarget::Discard => Ok(()),
            LoweredAssignmentTarget::Tuple { ty, items } => {
                if value.ty != *ty {
                    return Err(MirError::Construction {
                        span,
                        message: format!(
                            "multiple-assignment value has {}, target tree has {ty}",
                            value.ty
                        ),
                    });
                }
                for (index, item) in items.iter().enumerate() {
                    let item_ty = self.assignment_target_item_type(*ty, index, item, span)?;
                    let projected = self.project_operand(
                        &value,
                        MirProjection {
                            ty: item_ty,
                            kind: MirProjectionKind::TupleField(index as u32),
                        },
                        span,
                    )?;
                    self.write_assignment_target(item, projected, block, span)?;
                }
                Ok(())
            }
        }
    }

    fn lower_for(
        &mut self,
        span: Span,
        id: HirLoopId,
        kind: &HirForKind,
        body: HirExpressionId,
        block: MirBlockId,
    ) -> Result<Option<MirBlockId>, MirError> {
        match kind {
            HirForKind::Infinite => self.lower_infinite_for(span, id, body, block),
            HirForKind::Conditional { condition } => {
                self.lower_conditional_for(span, id, *condition, body, block)
            }
            HirForKind::Iterate {
                pattern,
                source,
                protocol,
            } => match protocol {
                HirIterationProtocol::Intrinsic => {
                    self.lower_intrinsic_iterating_for(span, id, *pattern, *source, body, block)
                }
                HirIterationProtocol::Trait {
                    element,
                    function_type,
                } => self.lower_trait_iterating_for(
                    span,
                    id,
                    *pattern,
                    *source,
                    *element,
                    *function_type,
                    body,
                    block,
                ),
            },
        }
    }

    fn lower_infinite_for(
        &mut self,
        span: Span,
        id: HirLoopId,
        body: HirExpressionId,
        block: MirBlockId,
    ) -> Result<Option<MirBlockId>, MirError> {
        let body_start = self.allocate_block(MirBlockKind::Normal)?;
        let exit = self.allocate_block(MirBlockKind::Normal)?;
        self.terminate(block, span, MirTerminatorKind::Goto { target: body_start })?;
        self.loops.insert(
            id,
            LoopTargets {
                break_target: exit,
                continue_target: body_start,
            },
        );
        let body_end = self.lower_value(body, body_start)?.map(|(block, _)| block);
        self.loops.remove(&id);
        if let Some(body_end) = body_end {
            self.terminate(
                body_end,
                span,
                MirTerminatorKind::Goto { target: body_start },
            )?;
        }
        let can_break = self
            .hir
            .expression_break_targets(body)
            .is_some_and(|targets| targets.contains(&id));
        if can_break {
            Ok(Some(exit))
        } else {
            self.terminate(exit, span, MirTerminatorKind::Unreachable)?;
            Ok(None)
        }
    }

    fn lower_conditional_for(
        &mut self,
        span: Span,
        id: HirLoopId,
        condition: HirExpressionId,
        body: HirExpressionId,
        block: MirBlockId,
    ) -> Result<Option<MirBlockId>, MirError> {
        let header = self.allocate_block(MirBlockKind::Normal)?;
        self.terminate(block, span, MirTerminatorKind::Goto { target: header })?;
        let Some((header_end, condition)) = self.lower_value(condition, header)? else {
            return Ok(None);
        };
        let body_start = self.allocate_block(MirBlockKind::Normal)?;
        let exit = self.allocate_block(MirBlockKind::Normal)?;
        self.terminate(
            header_end,
            span,
            MirTerminatorKind::SwitchBool {
                condition,
                if_true: body_start,
                if_false: exit,
            },
        )?;
        self.loops.insert(
            id,
            LoopTargets {
                break_target: exit,
                continue_target: header,
            },
        );
        let body_end = self.lower_value(body, body_start)?.map(|(block, _)| block);
        self.loops.remove(&id);
        if let Some(body_end) = body_end {
            self.terminate(body_end, span, MirTerminatorKind::Goto { target: header })?;
        }
        Ok(Some(exit))
    }

    fn lower_intrinsic_iterating_for(
        &mut self,
        span: Span,
        id: HirLoopId,
        pattern: HirPatternId,
        source: HirExpressionId,
        body: HirExpressionId,
        block: MirBlockId,
    ) -> Result<Option<MirBlockId>, MirError> {
        let Some((block, source)) = self.lower_value(source, block)? else {
            return Ok(None);
        };
        let source_type = source.ty;
        let state = self.allocate_temporary(source_type, span, block)?;
        self.assign(
            block,
            span,
            self.local_place(state),
            MirRvalue {
                ty: source.ty,
                kind: MirRvalueKind::IteratorState { source },
            },
        )?;
        let item = self.allocate_temporary(self.pattern_type(pattern)?, span, block)?;
        let header = self.allocate_block(MirBlockKind::Normal)?;
        let body_start = self.allocate_block(MirBlockKind::Normal)?;
        let exit = self.allocate_block(MirBlockKind::Normal)?;
        self.terminate(block, span, MirTerminatorKind::Goto { target: header })?;
        self.terminate(
            header,
            span,
            MirTerminatorKind::IteratorNext {
                state: self.local_place(state),
                destination: self.local_place(item),
                has_value: body_start,
                exhausted: exit,
                unwind: self.unwind,
            },
        )?;
        self.finish_iterating_for(
            span,
            id,
            pattern,
            self.copy_local(item),
            body,
            header,
            body_start,
            exit,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_trait_iterating_for(
        &mut self,
        span: Span,
        id: HirLoopId,
        pattern: HirPatternId,
        source: HirExpressionId,
        element: TypeId,
        function_type: TypeId,
        body: HirExpressionId,
        block: MirBlockId,
    ) -> Result<Option<MirBlockId>, MirError> {
        let Some((block, source)) = self.lower_value(source, block)? else {
            return Ok(None);
        };
        let source_type = source.ty;
        let state = self.allocate_temporary(source_type, span, block)?;
        self.assign_operand(block, span, self.local_place(state), source)?;

        let outcome = match self.hir.interner().kind(function_type).map_err(|error| {
            MirError::Construction {
                span,
                message: format!("Iterator.next has an invalid function type: {error}"),
            }
        })? {
            TypeKind::Function(function) => function.outcome(),
            _ => {
                return Err(MirError::Construction {
                    span,
                    message: "Iterator.next protocol has a non-function type".into(),
                });
            }
        };
        let next = self.allocate_temporary(outcome, span, block)?;
        let header = self.allocate_block(MirBlockKind::Normal)?;
        let inspect = self.allocate_block(MirBlockKind::Normal)?;
        let body_start = self.allocate_block(MirBlockKind::Normal)?;
        let exit = self.allocate_block(MirBlockKind::Normal)?;
        self.terminate(block, span, MirTerminatorKind::Goto { target: header })?;
        self.terminate(
            header,
            span,
            MirTerminatorKind::Invoke {
                operation: MirOperation {
                    ty: outcome,
                    kind: MirOperationKind::Call {
                        callee: MirOperand {
                            ty: function_type,
                            kind: MirOperandKind::PreludeTraitFunction {
                                method: HirPreludeTraitMethod::IteratorNext,
                                arguments: vec![element, source_type],
                            },
                        },
                        arguments: vec![MirCallArgument {
                            mode: crate::types::ParameterMode::Mut,
                            target: crate::hir::HirCallArgumentTarget::Receiver,
                            value: self.copy_local(state),
                        }],
                        signature: function_type,
                        protocol: HirCallProtocol::Call,
                    },
                },
                destination: Some(self.local_place(next)),
                target: Some(inspect),
                unwind: self.unwind,
            },
        )?;
        self.terminate(
            inspect,
            span,
            MirTerminatorKind::SwitchTag {
                value: self.copy_local(next),
                cases: vec![(MirTag::OptionSome, body_start)],
                otherwise: exit,
            },
        )?;
        let item = self.project_operand(
            &self.copy_local(next),
            MirProjection {
                ty: element,
                kind: MirProjectionKind::OptionValue,
            },
            span,
        )?;
        self.finish_iterating_for(span, id, pattern, item, body, header, body_start, exit)
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_iterating_for(
        &mut self,
        span: Span,
        id: HirLoopId,
        pattern: HirPatternId,
        item: MirOperand,
        body: HirExpressionId,
        header: MirBlockId,
        body_start: MirBlockId,
        exit: MirBlockId,
    ) -> Result<Option<MirBlockId>, MirError> {
        self.loops.insert(
            id,
            LoopTargets {
                break_target: exit,
                continue_target: header,
            },
        );
        let Some(body_start) = self.bind_irrefutable(pattern, item, body_start)? else {
            return Err(MirError::Construction {
                span,
                message: "irrefutable iterator pattern diverged while binding".into(),
            });
        };
        let body_end = self.lower_value(body, body_start)?.map(|(block, _)| block);
        self.loops.remove(&id);
        if let Some(body_end) = body_end {
            self.terminate(body_end, span, MirTerminatorKind::Goto { target: header })?;
        }
        Ok(Some(exit))
    }

    fn bind_irrefutable(
        &mut self,
        pattern: HirPatternId,
        value: MirOperand,
        block: MirBlockId,
    ) -> Result<Option<MirBlockId>, MirError> {
        let pattern = self
            .hir
            .pattern(pattern)
            .ok_or_else(|| MirError::Construction {
                span: self.span,
                message: format!("missing verified pattern#{}", pattern.index()),
            })?
            .clone();
        let span = pattern.span();
        match pattern.kind() {
            HirPatternKind::Wildcard => Ok(Some(block)),
            HirPatternKind::Binding(local) | HirPatternKind::BorrowBinding(local) => {
                let local = self.allocate_user_local(*local, pattern.ty(), span, block)?;
                self.assign_operand(block, span, self.local_place(local), value)?;
                Ok(Some(block))
            }
            HirPatternKind::Tuple(items) => {
                for (index, item) in items.iter().enumerate() {
                    let ty = self.pattern_type(*item)?;
                    let projected = self.project_operand(
                        &value,
                        MirProjection {
                            ty,
                            kind: MirProjectionKind::TupleField(index as u32),
                        },
                        span,
                    )?;
                    self.bind_irrefutable(*item, projected, block)?;
                }
                Ok(Some(block))
            }
            HirPatternKind::Newtype { value: item, .. } => {
                let projected = self.project_operand(
                    &value,
                    MirProjection {
                        ty: self.pattern_type(*item)?,
                        kind: MirProjectionKind::NewtypeValue,
                    },
                    span,
                )?;
                self.bind_irrefutable(*item, projected, block)
            }
            HirPatternKind::Record { fields, .. } => {
                for field in fields {
                    let projected = self.project_operand(
                        &value,
                        MirProjection {
                            ty: self.pattern_type(field.pattern())?,
                            kind: MirProjectionKind::Field(field.member()),
                        },
                        span,
                    )?;
                    self.bind_irrefutable(field.pattern(), projected, block)?;
                }
                Ok(Some(block))
            }
            HirPatternKind::Variant { variant, fields } => {
                for (index, field) in fields.iter().enumerate() {
                    let projected = self.project_operand(
                        &value,
                        MirProjection {
                            ty: self.pattern_type(*field)?,
                            kind: MirProjectionKind::VariantTuple {
                                variant: *variant,
                                index: index as u32,
                            },
                        },
                        span,
                    )?;
                    self.bind_irrefutable(*field, projected, block)?;
                }
                Ok(Some(block))
            }
            HirPatternKind::OptionSome(item) => {
                let projected = self.project_operand(
                    &value,
                    MirProjection {
                        ty: self.pattern_type(*item)?,
                        kind: MirProjectionKind::OptionValue,
                    },
                    span,
                )?;
                self.bind_irrefutable(*item, projected, block)
            }
            HirPatternKind::ResultOk(item) => {
                let projected = self.project_operand(
                    &value,
                    MirProjection {
                        ty: self.pattern_type(*item)?,
                        kind: MirProjectionKind::ResultOkValue,
                    },
                    span,
                )?;
                self.bind_irrefutable(*item, projected, block)
            }
            HirPatternKind::ResultErr(item) => {
                let projected = self.project_operand(
                    &value,
                    MirProjection {
                        ty: self.pattern_type(*item)?,
                        kind: MirProjectionKind::ResultErrValue,
                    },
                    span,
                )?;
                self.bind_irrefutable(*item, projected, block)
            }
            HirPatternKind::UnionMember { member, pattern } => {
                let projected = self.project_operand(
                    &value,
                    MirProjection {
                        ty: self.pattern_type(*pattern)?,
                        kind: MirProjectionKind::UnionValue(*member),
                    },
                    span,
                )?;
                self.bind_irrefutable(*pattern, projected, block)
            }
            HirPatternKind::Array {
                prefix,
                rest: Some(rest),
            } if prefix.is_empty() => self.bind_irrefutable(*rest, value, block),
            HirPatternKind::OptionNone
            | HirPatternKind::Recovery
            | HirPatternKind::Literal(_)
            | HirPatternKind::Array { .. } => Err(MirError::Construction {
                span,
                message: "a refutable pattern reached irrefutable binding lowering".into(),
            }),
        }
    }

    fn lower_aggregate(
        &mut self,
        values: &[HirExpressionId],
        shape: MirAggregateKind,
        ty: TypeId,
        span: Span,
        destination: MirPlace,
        block: MirBlockId,
    ) -> Result<Option<MirBlockId>, MirError> {
        let Some((block, values)) = self.lower_values(values, block)? else {
            return Ok(None);
        };
        self.assign(
            block,
            span,
            destination,
            MirRvalue {
                ty,
                kind: MirRvalueKind::Aggregate { shape, values },
            },
        )?;
        Ok(Some(block))
    }

    fn lower_single_aggregate(
        &mut self,
        value: HirExpressionId,
        shape: MirAggregateKind,
        ty: TypeId,
        span: Span,
        destination: MirPlace,
        block: MirBlockId,
    ) -> Result<Option<MirBlockId>, MirError> {
        let Some((block, value)) = self.lower_value(value, block)? else {
            return Ok(None);
        };
        self.assign(
            block,
            span,
            destination,
            MirRvalue {
                ty,
                kind: MirRvalueKind::Aggregate {
                    shape,
                    values: vec![value],
                },
            },
        )?;
        Ok(Some(block))
    }

    fn lower_values(
        &mut self,
        values: &[HirExpressionId],
        mut block: MirBlockId,
    ) -> Result<Option<(MirBlockId, Vec<MirOperand>)>, MirError> {
        let mut operands = Vec::with_capacity(values.len());
        for value in values {
            let Some((next, operand)) = self.lower_value(*value, block)? else {
                return Ok(None);
            };
            block = next;
            operands.push(operand);
        }
        Ok(Some((block, operands)))
    }

    fn lower_value(
        &mut self,
        expression: HirExpressionId,
        block: MirBlockId,
    ) -> Result<Option<(MirBlockId, MirOperand)>, MirError> {
        let expression_node = self.expression(expression)?.clone();
        let local = self.allocate_temporary(expression_node.ty(), expression_node.span(), block)?;
        let place = self.local_place(local);
        let Some(block) = self.lower_expression(expression, place.clone(), block)? else {
            return Ok(None);
        };
        Ok(Some((
            block,
            MirOperand {
                ty: expression_node.ty(),
                kind: MirOperandKind::Copy(place),
            },
        )))
    }

    fn lower_callee(
        &mut self,
        id: HirExpressionId,
        protocol: HirCallProtocol,
        block: MirBlockId,
    ) -> Result<Option<(MirBlockId, MirOperand)>, MirError> {
        let expression = self.expression(id)?;
        let operand = match expression.kind() {
            HirExpressionKind::Function(callable) => Some(MirOperand {
                ty: expression.ty(),
                kind: MirOperandKind::Function {
                    callable: *callable,
                    arguments: Vec::new(),
                },
            }),
            HirExpressionKind::SpecializedFunction {
                callable,
                arguments,
            } => Some(MirOperand {
                ty: expression.ty(),
                kind: MirOperandKind::Function {
                    callable: *callable,
                    arguments: arguments.clone(),
                },
            }),
            HirExpressionKind::PreludeTraitFunction { method, arguments } => Some(MirOperand {
                ty: expression.ty(),
                kind: MirOperandKind::PreludeTraitFunction {
                    method: *method,
                    arguments: arguments.clone(),
                },
            }),
            _ => None,
        };
        if let Some(operand) = operand {
            Ok(Some((block, operand)))
        } else if matches!(protocol, HirCallProtocol::Call | HirCallProtocol::CallMut)
            && self.expression(id)?.category() == HirValueCategory::Place
        {
            let Some((block, place)) = self.lower_place(id, block)? else {
                return Ok(None);
            };
            Ok(Some((
                block,
                MirOperand {
                    ty: place.ty,
                    kind: MirOperandKind::Borrow(place),
                },
            )))
        } else {
            self.lower_value(id, block)
        }
    }

    fn lower_optional_value(
        &mut self,
        expression: Option<HirExpressionId>,
        block: MirBlockId,
    ) -> Result<(Option<MirBlockId>, Option<MirOperand>), MirError> {
        let Some(expression) = expression else {
            return Ok((Some(block), None));
        };
        Ok(match self.lower_value(expression, block)? {
            Some((block, value)) => (Some(block), Some(value)),
            None => (None, None),
        })
    }

    fn lower_logical(
        &mut self,
        operator: HirBinaryOperator,
        left: HirExpressionId,
        right: HirExpressionId,
        span: Span,
        destination: MirPlace,
        block: MirBlockId,
    ) -> Result<Option<MirBlockId>, MirError> {
        let Some((block, condition)) = self.lower_value(left, block)? else {
            return Ok(None);
        };
        let short = self.allocate_block(MirBlockKind::Normal)?;
        let evaluate = self.allocate_block(MirBlockKind::Normal)?;
        let (if_true, if_false, short_value) = match operator {
            HirBinaryOperator::LogicalAnd => (evaluate, short, false),
            HirBinaryOperator::LogicalOr => (short, evaluate, true),
            _ => unreachable!("logical lowering is selected by a closed operator match"),
        };
        self.terminate(
            block,
            span,
            MirTerminatorKind::SwitchBool {
                condition,
                if_true,
                if_false,
            },
        )?;
        self.assign_operand(
            short,
            span,
            destination.clone(),
            MirOperand {
                ty: self.hir.interner().scalar(ScalarType::Bool),
                kind: MirOperandKind::Constant(MirConstant::Bool(short_value)),
            },
        )?;
        let right_end = self.lower_expression(right, destination, evaluate)?;
        let join = self.allocate_block(MirBlockKind::Normal)?;
        self.terminate(short, span, MirTerminatorKind::Goto { target: join })?;
        if let Some(right_end) = right_end {
            self.terminate(right_end, span, MirTerminatorKind::Goto { target: join })?;
        }
        Ok(Some(join))
    }

    fn lower_if(
        &mut self,
        condition: HirExpressionId,
        then_branch: HirExpressionId,
        else_branch: Option<HirExpressionId>,
        span: Span,
        destination: MirPlace,
        block: MirBlockId,
    ) -> Result<Option<MirBlockId>, MirError> {
        let Some((block, condition)) = self.lower_value(condition, block)? else {
            return Ok(None);
        };
        let then_start = self.allocate_block(MirBlockKind::Normal)?;
        let else_start = self.allocate_block(MirBlockKind::Normal)?;
        self.terminate(
            block,
            span,
            MirTerminatorKind::SwitchBool {
                condition,
                if_true: then_start,
                if_false: else_start,
            },
        )?;
        let then_end = self.lower_expression(then_branch, destination.clone(), then_start)?;
        let else_end = if let Some(else_branch) = else_branch {
            self.lower_expression(else_branch, destination, else_start)?
        } else {
            self.assign_operand(else_start, span, destination, self.unit_operand())?;
            Some(else_start)
        };
        if then_end.is_none() && else_end.is_none() {
            return Ok(None);
        }
        let join = self.allocate_block(MirBlockKind::Normal)?;
        for end in [then_end, else_end].into_iter().flatten() {
            self.terminate(end, span, MirTerminatorKind::Goto { target: join })?;
        }
        Ok(Some(join))
    }

    fn lower_propagate_option(
        &mut self,
        value: HirExpressionId,
        item_ty: TypeId,
        span: Span,
        destination: MirPlace,
        block: MirBlockId,
    ) -> Result<Option<MirBlockId>, MirError> {
        let Some((block, option)) = self.lower_value(value, block)? else {
            return Ok(None);
        };
        let some = self.allocate_block(MirBlockKind::Normal)?;
        let none = self.allocate_block(MirBlockKind::Normal)?;
        self.terminate(
            block,
            span,
            MirTerminatorKind::SwitchTag {
                value: option.clone(),
                cases: vec![(super::MirTag::OptionSome, some)],
                otherwise: none,
            },
        )?;
        let projected = self.project_operand(
            &option,
            MirProjection {
                ty: item_ty,
                kind: MirProjectionKind::OptionValue,
            },
            span,
        )?;
        self.assign_operand(some, span, destination, projected)?;
        let join = self.allocate_block(MirBlockKind::Normal)?;
        self.terminate(some, span, MirTerminatorKind::Goto { target: join })?;

        let return_place = self.local_place(self.return_local);
        match self
            .hir
            .interner()
            .kind(self.outcome)
            .map_err(|_| MirError::Construction {
                span,
                message: "callable outcome is absent from the verified type interner".into(),
            })? {
            TypeKind::Option(_) => {
                self.assign(
                    none,
                    span,
                    return_place,
                    MirRvalue {
                        ty: self.outcome,
                        kind: MirRvalueKind::Aggregate {
                            shape: MirAggregateKind::OptionNone,
                            values: Vec::new(),
                        },
                    },
                )?;
            }
            TypeKind::Result { success, .. }
                if matches!(self.hir.interner().kind(*success), Ok(TypeKind::Option(_))) =>
            {
                let option_local = self.allocate_temporary(*success, span, none)?;
                self.assign(
                    none,
                    span,
                    self.local_place(option_local),
                    MirRvalue {
                        ty: *success,
                        kind: MirRvalueKind::Aggregate {
                            shape: MirAggregateKind::OptionNone,
                            values: Vec::new(),
                        },
                    },
                )?;
                self.assign(
                    none,
                    span,
                    return_place,
                    MirRvalue {
                        ty: self.outcome,
                        kind: MirRvalueKind::Aggregate {
                            shape: MirAggregateKind::ResultOk,
                            values: vec![self.copy_local(option_local)],
                        },
                    },
                )?;
            }
            _ => {
                return Err(MirError::Construction {
                    span,
                    message: "option propagation has no direct callable option channel".into(),
                });
            }
        }
        self.terminate(none, span, MirTerminatorKind::Return)?;
        Ok(Some(join))
    }

    fn lower_propagate_result(
        &mut self,
        value: HirExpressionId,
        error_coercion: crate::types::Assignability,
        success_ty: TypeId,
        span: Span,
        destination: MirPlace,
        block: MirBlockId,
    ) -> Result<Option<MirBlockId>, MirError> {
        let Some((block, result)) = self.lower_value(value, block)? else {
            return Ok(None);
        };
        let TypeKind::Result {
            success: source_success,
            error: source_error,
        } = self
            .hir
            .interner()
            .kind(result.ty)
            .map_err(|_| MirError::Construction {
                span,
                message: "propagated result has an invalid type".into(),
            })?
        else {
            return Err(MirError::Construction {
                span,
                message: "result propagation operand is not Result".into(),
            });
        };
        if *source_success != success_ty {
            return Err(MirError::Construction {
                span,
                message: "result propagation success projection has the wrong type".into(),
            });
        }
        let TypeKind::Result {
            error: target_error,
            ..
        } = self
            .hir
            .interner()
            .kind(self.outcome)
            .map_err(|_| MirError::Construction {
                span,
                message: "callable result type is invalid".into(),
            })?
        else {
            return Err(MirError::Construction {
                span,
                message: "result propagation has no callable result channel".into(),
            });
        };
        let source_error = *source_error;
        let target_error = *target_error;
        let ok = self.allocate_block(MirBlockKind::Normal)?;
        let err = self.allocate_block(MirBlockKind::Normal)?;
        self.terminate(
            block,
            span,
            MirTerminatorKind::SwitchTag {
                value: result.clone(),
                cases: vec![(super::MirTag::ResultOk, ok)],
                otherwise: err,
            },
        )?;
        let success = self.project_operand(
            &result,
            MirProjection {
                ty: success_ty,
                kind: MirProjectionKind::ResultOkValue,
            },
            span,
        )?;
        self.assign_operand(ok, span, destination, success)?;
        let join = self.allocate_block(MirBlockKind::Normal)?;
        self.terminate(ok, span, MirTerminatorKind::Goto { target: join })?;

        let error = self.project_operand(
            &result,
            MirProjection {
                ty: source_error,
                kind: MirProjectionKind::ResultErrValue,
            },
            span,
        )?;
        let error = if error_coercion == crate::types::Assignability::Exact {
            error
        } else {
            let local = self.allocate_temporary(target_error, span, err)?;
            self.assign(
                err,
                span,
                self.local_place(local),
                MirRvalue {
                    ty: target_error,
                    kind: MirRvalueKind::Coerce {
                        kind: error_coercion,
                        value: error,
                    },
                },
            )?;
            self.copy_local(local)
        };
        self.assign(
            err,
            span,
            self.local_place(self.return_local),
            MirRvalue {
                ty: self.outcome,
                kind: MirRvalueKind::Aggregate {
                    shape: MirAggregateKind::ResultErr,
                    values: vec![error],
                },
            },
        )?;
        self.terminate(err, span, MirTerminatorKind::Return)?;
        Ok(Some(join))
    }

    fn lower_match(
        &mut self,
        scrutinee: HirExpressionId,
        arms: &[crate::hir::HirMatchArm],
        span: Span,
        destination: MirPlace,
        block: MirBlockId,
    ) -> Result<Option<MirBlockId>, MirError> {
        let Some((block, scrutinee)) = self.lower_value(scrutinee, block)? else {
            return Ok(None);
        };
        let failure = self.allocate_block(MirBlockKind::Normal)?;
        let mut next_pattern = block;
        let mut completing = Vec::new();
        for (index, arm) in arms.iter().enumerate() {
            let matched = self.allocate_block(MirBlockKind::Normal)?;
            let next = if index + 1 == arms.len() {
                failure
            } else {
                self.allocate_block(MirBlockKind::Normal)?
            };
            self.lower_pattern_test(
                arm.pattern(),
                scrutinee.clone(),
                next_pattern,
                matched,
                next,
            )?;
            let body_start = if let Some(guard) = arm.guard() {
                let Some((guard_end, condition)) = self.lower_value(guard, matched)? else {
                    next_pattern = next;
                    continue;
                };
                let body = self.allocate_block(MirBlockKind::Normal)?;
                self.terminate(
                    guard_end,
                    self.expression(guard)?.span(),
                    MirTerminatorKind::SwitchBool {
                        condition,
                        if_true: body,
                        if_false: next,
                    },
                )?;
                body
            } else {
                matched
            };
            if let Some(end) = self.lower_expression(arm.body(), destination.clone(), body_start)? {
                completing.push(end);
            }
            next_pattern = next;
        }
        self.terminate(failure, span, MirTerminatorKind::Unreachable)?;
        if completing.is_empty() {
            return Ok(None);
        }
        let join = self.allocate_block(MirBlockKind::Normal)?;
        for end in completing {
            self.terminate(end, span, MirTerminatorKind::Goto { target: join })?;
        }
        Ok(Some(join))
    }

    fn lower_pattern_test(
        &mut self,
        pattern: HirPatternId,
        value: MirOperand,
        block: MirBlockId,
        matched: MirBlockId,
        failed: MirBlockId,
    ) -> Result<(), MirError> {
        let pattern = self
            .hir
            .pattern(pattern)
            .ok_or_else(|| MirError::Construction {
                span: self.span,
                message: format!("missing verified pattern#{}", pattern.index()),
            })?
            .clone();
        let span = pattern.span();
        match pattern.kind() {
            HirPatternKind::Wildcard => {
                self.terminate(block, span, MirTerminatorKind::Goto { target: matched })
            }
            HirPatternKind::Binding(local) | HirPatternKind::BorrowBinding(local) => {
                let local = self.allocate_user_local(*local, pattern.ty(), span, block)?;
                self.assign_operand(block, span, self.local_place(local), value)?;
                self.terminate(block, span, MirTerminatorKind::Goto { target: matched })
            }
            HirPatternKind::Literal(literal) => {
                let condition = self.allocate_temporary(
                    self.hir.interner().scalar(ScalarType::Bool),
                    span,
                    block,
                )?;
                self.assign(
                    block,
                    span,
                    self.local_place(condition),
                    MirRvalue {
                        ty: self.hir.interner().scalar(ScalarType::Bool),
                        kind: MirRvalueKind::Binary {
                            operator: HirBinaryOperator::Equal,
                            left: value,
                            right: self.literal_operand(pattern.ty(), literal),
                        },
                    },
                )?;
                self.terminate(
                    block,
                    span,
                    MirTerminatorKind::SwitchBool {
                        condition: self.copy_local(condition),
                        if_true: matched,
                        if_false: failed,
                    },
                )
            }
            HirPatternKind::Tuple(items) => {
                let mut tests = Vec::with_capacity(items.len());
                for (index, item) in items.iter().enumerate() {
                    tests.push((
                        *item,
                        self.project_operand(
                            &value,
                            MirProjection {
                                ty: self.pattern_type(*item)?,
                                kind: MirProjectionKind::TupleField(index as u32),
                            },
                            span,
                        )?,
                    ));
                }
                self.lower_pattern_sequence(&tests, block, matched, failed, span)
            }
            HirPatternKind::OptionSome(item) => {
                let payload = self.allocate_block(MirBlockKind::Normal)?;
                self.terminate(
                    block,
                    span,
                    MirTerminatorKind::SwitchTag {
                        value: value.clone(),
                        cases: vec![(super::MirTag::OptionSome, payload)],
                        otherwise: failed,
                    },
                )?;
                let value = self.project_operand(
                    &value,
                    MirProjection {
                        ty: self.pattern_type(*item)?,
                        kind: MirProjectionKind::OptionValue,
                    },
                    span,
                )?;
                self.lower_pattern_test(*item, value, payload, matched, failed)
            }
            HirPatternKind::OptionNone => self.terminate(
                block,
                span,
                MirTerminatorKind::SwitchTag {
                    value,
                    cases: vec![(super::MirTag::OptionNone, matched)],
                    otherwise: failed,
                },
            ),
            HirPatternKind::ResultOk(item) | HirPatternKind::ResultErr(item) => {
                let payload = self.allocate_block(MirBlockKind::Normal)?;
                let (tag, projection) = match pattern.kind() {
                    HirPatternKind::ResultOk(_) => {
                        (super::MirTag::ResultOk, MirProjectionKind::ResultOkValue)
                    }
                    HirPatternKind::ResultErr(_) => {
                        (super::MirTag::ResultErr, MirProjectionKind::ResultErrValue)
                    }
                    _ => unreachable!("result pattern branch is closed"),
                };
                self.terminate(
                    block,
                    span,
                    MirTerminatorKind::SwitchTag {
                        value: value.clone(),
                        cases: vec![(tag, payload)],
                        otherwise: failed,
                    },
                )?;
                let value = self.project_operand(
                    &value,
                    MirProjection {
                        ty: self.pattern_type(*item)?,
                        kind: projection,
                    },
                    span,
                )?;
                self.lower_pattern_test(*item, value, payload, matched, failed)
            }
            HirPatternKind::Newtype { value: item, .. } => {
                let value = self.project_operand(
                    &value,
                    MirProjection {
                        ty: self.pattern_type(*item)?,
                        kind: MirProjectionKind::NewtypeValue,
                    },
                    span,
                )?;
                self.lower_pattern_test(*item, value, block, matched, failed)
            }
            HirPatternKind::Variant { variant, fields } => {
                let payload = self.allocate_block(MirBlockKind::Normal)?;
                self.terminate(
                    block,
                    span,
                    MirTerminatorKind::SwitchTag {
                        value: value.clone(),
                        cases: vec![(super::MirTag::Variant(*variant), payload)],
                        otherwise: failed,
                    },
                )?;
                let projections = self.variant_pattern_projections(*variant, fields, span)?;
                let mut tests = Vec::with_capacity(fields.len());
                for (field, projection) in fields.iter().zip(projections) {
                    tests.push((
                        *field,
                        self.project_operand(
                            &value,
                            MirProjection {
                                ty: self.pattern_type(*field)?,
                                kind: projection,
                            },
                            span,
                        )?,
                    ));
                }
                self.lower_pattern_sequence(&tests, payload, matched, failed, span)
            }
            HirPatternKind::Record { fields, .. } => {
                let mut tests = Vec::with_capacity(fields.len());
                for field in fields {
                    tests.push((
                        field.pattern(),
                        self.project_operand(
                            &value,
                            MirProjection {
                                ty: self.pattern_type(field.pattern())?,
                                kind: MirProjectionKind::Field(field.member()),
                            },
                            span,
                        )?,
                    ));
                }
                self.lower_pattern_sequence(&tests, block, matched, failed, span)
            }
            HirPatternKind::UnionMember { member, pattern } => {
                let payload = self.allocate_block(MirBlockKind::Normal)?;
                self.terminate(
                    block,
                    span,
                    MirTerminatorKind::SwitchTag {
                        value: value.clone(),
                        cases: vec![(super::MirTag::Union(*member), payload)],
                        otherwise: failed,
                    },
                )?;
                let value = self.project_operand(
                    &value,
                    MirProjection {
                        ty: self.pattern_type(*pattern)?,
                        kind: MirProjectionKind::UnionValue(*member),
                    },
                    span,
                )?;
                self.lower_pattern_test(*pattern, value, payload, matched, failed)
            }
            HirPatternKind::Array { prefix, rest } => {
                let length = self.allocate_temporary(
                    self.hir.interner().scalar(ScalarType::Int),
                    span,
                    block,
                )?;
                self.assign(
                    block,
                    span,
                    self.local_place(length),
                    MirRvalue {
                        ty: self.hir.interner().scalar(ScalarType::Int),
                        kind: MirRvalueKind::Length(value.clone()),
                    },
                )?;
                let condition = self.allocate_temporary(
                    self.hir.interner().scalar(ScalarType::Bool),
                    span,
                    block,
                )?;
                self.assign(
                    block,
                    span,
                    self.local_place(condition),
                    MirRvalue {
                        ty: self.hir.interner().scalar(ScalarType::Bool),
                        kind: MirRvalueKind::Binary {
                            operator: if rest.is_some() {
                                HirBinaryOperator::GreaterEqual
                            } else {
                                HirBinaryOperator::Equal
                            },
                            left: self.copy_local(length),
                            right: MirOperand {
                                ty: self.hir.interner().scalar(ScalarType::Int),
                                kind: MirOperandKind::Constant(MirConstant::Integer(
                                    prefix.len().to_string(),
                                )),
                            },
                        },
                    },
                )?;
                let elements = self.allocate_block(MirBlockKind::Normal)?;
                self.terminate(
                    block,
                    span,
                    MirTerminatorKind::SwitchBool {
                        condition: self.copy_local(condition),
                        if_true: elements,
                        if_false: failed,
                    },
                )?;
                let mut tests = Vec::with_capacity(prefix.len() + usize::from(rest.is_some()));
                for (index, pattern) in prefix.iter().enumerate() {
                    tests.push((
                        *pattern,
                        self.project_operand(
                            &value,
                            MirProjection {
                                ty: self.pattern_type(*pattern)?,
                                kind: MirProjectionKind::ArrayPatternIndex(index as u32),
                            },
                            span,
                        )?,
                    ));
                }
                if let Some(rest) = rest {
                    tests.push((
                        *rest,
                        self.project_operand(
                            &value,
                            MirProjection {
                                ty: self.pattern_type(*rest)?,
                                kind: MirProjectionKind::ArrayPatternRest {
                                    start: prefix.len() as u32,
                                    suffix: 0,
                                },
                            },
                            span,
                        )?,
                    ));
                }
                self.lower_pattern_sequence(&tests, elements, matched, failed, span)
            }
            HirPatternKind::Recovery => Err(MirError::Construction {
                span,
                message: "recovery pattern crossed the verified HIR boundary".into(),
            }),
        }
    }

    fn lower_pattern_sequence(
        &mut self,
        tests: &[(HirPatternId, MirOperand)],
        block: MirBlockId,
        matched: MirBlockId,
        failed: MirBlockId,
        span: Span,
    ) -> Result<(), MirError> {
        if tests.is_empty() {
            return self.terminate(block, span, MirTerminatorKind::Goto { target: matched });
        }
        let mut current = block;
        for (index, (pattern, value)) in tests.iter().enumerate() {
            let next = if index + 1 == tests.len() {
                matched
            } else {
                self.allocate_block(MirBlockKind::Normal)?
            };
            self.lower_pattern_test(*pattern, value.clone(), current, next, failed)?;
            current = next;
        }
        Ok(())
    }

    fn variant_pattern_projections(
        &self,
        member: crate::resolve::MemberId,
        fields: &[HirPatternId],
        span: Span,
    ) -> Result<Vec<MirProjectionKind>, MirError> {
        let payload = self
            .hir
            .declarations()
            .find_map(|(_, declaration)| {
                let crate::hir::HirTypeDeclarationKind::Nominal(nominal) = declaration.kind()
                else {
                    return None;
                };
                let HirNominalShape::Enum { variants } = nominal.shape() else {
                    return None;
                };
                variants
                    .iter()
                    .find(|variant| variant.member() == member)
                    .map(|variant| variant.payload())
            })
            .ok_or_else(|| MirError::Construction {
                span,
                message: format!("variant member#{} has no HIR payload", member.index()),
            })?;
        match payload {
            HirVariantPayload::Unit if fields.is_empty() => Ok(Vec::new()),
            HirVariantPayload::Tuple(types) if types.len() == fields.len() => Ok((0..fields.len())
                .map(|index| MirProjectionKind::VariantTuple {
                    variant: member,
                    index: index as u32,
                })
                .collect()),
            HirVariantPayload::Record(declared) if declared.len() == fields.len() => Ok(declared
                .iter()
                .map(|field| MirProjectionKind::VariantField {
                    variant: member,
                    field: field.member(),
                })
                .collect()),
            _ => Err(MirError::Construction {
                span,
                message: "variant pattern payload shape differs from its declaration".into(),
            }),
        }
    }

    fn lower_place(
        &mut self,
        id: HirExpressionId,
        block: MirBlockId,
    ) -> Result<Option<(MirBlockId, MirPlace)>, MirError> {
        let expression = self.expression(id)?.clone();
        let span = expression.span();
        match expression.kind() {
            HirExpressionKind::Local(local) => {
                let place = self.source_place(*local, span)?;
                Ok(Some((block, place)))
            }
            HirExpressionKind::Receiver => {
                let local = self.receiver.ok_or_else(|| MirError::Construction {
                    span,
                    message: "receiver place has no receiver local".into(),
                })?;
                Ok(Some((block, self.local_place(local))))
            }
            HirExpressionKind::Field { base, member } => {
                let Some((block, mut place)) = self.lower_place_base(*base, block)? else {
                    return Ok(None);
                };
                place.projections.push(MirProjection {
                    ty: expression.ty(),
                    kind: MirProjectionKind::Field(*member),
                });
                place.ty = expression.ty();
                Ok(Some((block, place)))
            }
            HirExpressionKind::TupleField { base, index } => {
                let Some((block, mut place)) = self.lower_place_base(*base, block)? else {
                    return Ok(None);
                };
                place.projections.push(MirProjection {
                    ty: expression.ty(),
                    kind: MirProjectionKind::TupleField(*index),
                });
                place.ty = expression.ty();
                Ok(Some((block, place)))
            }
            HirExpressionKind::Index {
                base,
                index,
                access,
            } => {
                let Some((block, mut place)) = self.lower_place_base(*base, block)? else {
                    return Ok(None);
                };
                let Some((block, index)) = self.lower_value(*index, block)? else {
                    return Ok(None);
                };
                let index = self.operand_local(&index, span)?;
                place.projections.push(MirProjection {
                    ty: expression.ty(),
                    kind: MirProjectionKind::Index {
                        index,
                        access: *access,
                    },
                });
                place.ty = expression.ty();
                Ok(Some((block, place)))
            }
            HirExpressionKind::Slice {
                base,
                start,
                end,
                step,
            } => {
                let Some((mut current, mut place)) = self.lower_place_base(*base, block)? else {
                    return Ok(None);
                };
                let (next, start) = self.lower_optional_local(*start, current, span)?;
                let Some(next) = next else {
                    return Ok(None);
                };
                current = next;
                let (next, end) = self.lower_optional_local(*end, current, span)?;
                let Some(next) = next else {
                    return Ok(None);
                };
                current = next;
                let (next, step) = self.lower_optional_local(*step, current, span)?;
                let Some(current) = next else {
                    return Ok(None);
                };
                place.projections.push(MirProjection {
                    ty: expression.ty(),
                    kind: MirProjectionKind::Slice { start, end, step },
                });
                place.ty = expression.ty();
                Ok(Some((current, place)))
            }
            _ => Err(MirError::Construction {
                span,
                message: "value-category Place has no place lowering".into(),
            }),
        }
    }

    fn lower_place_base(
        &mut self,
        id: HirExpressionId,
        block: MirBlockId,
    ) -> Result<Option<(MirBlockId, MirPlace)>, MirError> {
        if self.expression(id)?.category() == HirValueCategory::Place {
            self.lower_place(id, block)
        } else {
            let Some((block, operand)) = self.lower_value(id, block)? else {
                return Ok(None);
            };
            Ok(Some((
                block,
                self.operand_place(&operand, self.expression(id)?.span())?,
            )))
        }
    }

    fn lower_optional_local(
        &mut self,
        expression: Option<HirExpressionId>,
        block: MirBlockId,
        span: Span,
    ) -> Result<(Option<MirBlockId>, Option<MirLocalId>), MirError> {
        let (block, value) = self.lower_optional_value(expression, block)?;
        Ok((
            block,
            value
                .map(|value| self.operand_local(&value, span))
                .transpose()?,
        ))
    }

    fn project_operand(
        &self,
        operand: &MirOperand,
        projection: MirProjection,
        span: Span,
    ) -> Result<MirOperand, MirError> {
        let mut place = self.operand_place(operand, span)?;
        place.ty = projection.ty;
        place.projections.push(projection);
        Ok(MirOperand {
            ty: place.ty,
            kind: MirOperandKind::Copy(place),
        })
    }

    fn operand_place(&self, operand: &MirOperand, span: Span) -> Result<MirPlace, MirError> {
        match &operand.kind {
            MirOperandKind::Copy(place) | MirOperandKind::Move(place) => Ok(place.clone()),
            _ => Err(MirError::Construction {
                span,
                message: "aggregate projection operand was not materialized in a local".into(),
            }),
        }
    }

    fn operand_local(&self, operand: &MirOperand, span: Span) -> Result<MirLocalId, MirError> {
        let place = self.operand_place(operand, span)?;
        if !place.projections.is_empty() {
            return Err(MirError::Construction {
                span,
                message: "dynamic projection operand is not a direct temporary".into(),
            });
        }
        Ok(place.local)
    }

    fn pattern_type(&self, pattern: HirPatternId) -> Result<TypeId, MirError> {
        self.hir
            .pattern(pattern)
            .map(|pattern| pattern.ty())
            .ok_or_else(|| MirError::Construction {
                span: self.span,
                message: format!("missing verified pattern#{}", pattern.index()),
            })
    }

    fn source_local(&self, local: LocalId, span: Span) -> Result<MirLocalId, MirError> {
        self.source_locals
            .get(&local)
            .copied()
            .ok_or_else(|| MirError::Construction {
                span,
                message: format!("HIR local#{} has no MIR local", local.index()),
            })
    }

    fn source_place(&self, local: LocalId, span: Span) -> Result<MirPlace, MirError> {
        if let Some(place) = self.capture_places.get(&local) {
            return Ok(place.clone());
        }
        self.source_local(local, span)
            .map(|local| self.local_place(local))
    }

    fn allocate_user_local(
        &mut self,
        source: LocalId,
        ty: TypeId,
        span: Span,
        _block: MirBlockId,
    ) -> Result<MirLocalId, MirError> {
        if let Some(local) = self.source_locals.get(&source) {
            return Ok(*local);
        }
        let local = self.allocate_local(ty, span, MirLocalKind::User(source))?;
        self.source_locals.insert(source, local);
        Ok(local)
    }

    fn allocate_temporary(
        &mut self,
        ty: TypeId,
        span: Span,
        _block: MirBlockId,
    ) -> Result<MirLocalId, MirError> {
        self.allocate_local(ty, span, MirLocalKind::Temporary)
    }

    fn allocate_local(
        &mut self,
        ty: TypeId,
        span: Span,
        kind: MirLocalKind,
    ) -> Result<MirLocalId, MirError> {
        if self.locals.len() >= self.limits.max_locals_per_function as usize {
            return Err(MirError::NodeLimit {
                span,
                resource: "local",
            });
        }
        let id = MirLocalId(
            u32::try_from(self.locals.len()).map_err(|_| MirError::NodeLimit {
                span,
                resource: "local",
            })?,
        );
        self.locals.push(MirLocal { ty, span, kind });
        Ok(id)
    }

    fn allocate_block(&mut self, kind: MirBlockKind) -> Result<MirBlockId, MirError> {
        if self.blocks.len() >= self.limits.max_blocks_per_function as usize {
            return Err(MirError::NodeLimit {
                span: self.span,
                resource: "basic block",
            });
        }
        let id = MirBlockId(
            u32::try_from(self.blocks.len()).map_err(|_| MirError::NodeLimit {
                span: self.span,
                resource: "basic block",
            })?,
        );
        self.blocks.push(OpenBlock {
            kind,
            statements: Vec::new(),
            terminator: None,
        });
        Ok(id)
    }

    fn push_statement(
        &mut self,
        block: MirBlockId,
        span: Span,
        kind: MirStatementKind,
    ) -> Result<(), MirError> {
        if self.statement_count >= self.limits.max_statements_per_function {
            return Err(MirError::NodeLimit {
                span,
                resource: "statement",
            });
        }
        let target =
            self.blocks
                .get_mut(block.0 as usize)
                .ok_or_else(|| MirError::Construction {
                    span,
                    message: format!("missing generated block#{}", block.index()),
                })?;
        if target.terminator.is_some() {
            return Err(MirError::Construction {
                span,
                message: format!(
                    "statement appended after block#{} terminator",
                    block.index()
                ),
            });
        }
        target.statements.push(MirStatement { span, kind });
        self.statement_count += 1;
        Ok(())
    }

    fn assign(
        &mut self,
        block: MirBlockId,
        span: Span,
        destination: MirPlace,
        value: MirRvalue,
    ) -> Result<(), MirError> {
        self.push_statement(block, span, MirStatementKind::Assign { destination, value })
    }

    fn assign_operand(
        &mut self,
        block: MirBlockId,
        span: Span,
        destination: MirPlace,
        value: MirOperand,
    ) -> Result<(), MirError> {
        self.assign(
            block,
            span,
            destination,
            MirRvalue {
                ty: value.ty,
                kind: MirRvalueKind::Use(value),
            },
        )
    }

    fn terminate(
        &mut self,
        block: MirBlockId,
        span: Span,
        kind: MirTerminatorKind,
    ) -> Result<(), MirError> {
        let target =
            self.blocks
                .get_mut(block.0 as usize)
                .ok_or_else(|| MirError::Construction {
                    span,
                    message: format!("missing generated block#{}", block.index()),
                })?;
        if target
            .terminator
            .replace(MirTerminator { span, kind })
            .is_some()
        {
            return Err(MirError::Construction {
                span,
                message: format!("block#{} receives two terminators", block.index()),
            });
        }
        Ok(())
    }

    fn invoke(
        &mut self,
        block: MirBlockId,
        span: Span,
        destination: Option<MirPlace>,
        operation: MirOperation,
    ) -> Result<Option<MirBlockId>, MirError> {
        let never = self.hir.interner().scalar(ScalarType::Never);
        let target = if operation.ty == never {
            None
        } else {
            Some(self.allocate_block(MirBlockKind::Normal)?)
        };
        let destination = if target.is_some() { destination } else { None };
        self.terminate(
            block,
            span,
            MirTerminatorKind::Invoke {
                operation,
                destination,
                target,
                unwind: self.unwind,
            },
        )?;
        Ok(target)
    }

    fn local_place(&self, local: MirLocalId) -> MirPlace {
        MirPlace {
            local,
            ty: self.locals[local.0 as usize].ty,
            projections: Vec::new(),
        }
    }

    fn copy_local(&self, local: MirLocalId) -> MirOperand {
        let place = self.local_place(local);
        MirOperand {
            ty: place.ty,
            kind: MirOperandKind::Copy(place),
        }
    }

    fn unit_operand(&self) -> MirOperand {
        MirOperand {
            ty: self.hir.interner().scalar(ScalarType::Unit),
            kind: MirOperandKind::Constant(MirConstant::Unit),
        }
    }

    fn literal_operand(&self, ty: TypeId, literal: &HirLiteral) -> MirOperand {
        let constant = match literal {
            HirLiteral::Unit => MirConstant::Unit,
            HirLiteral::Bool(value) => MirConstant::Bool(*value),
            HirLiteral::Integer(value) => MirConstant::Integer(value.clone()),
            HirLiteral::Float(value) => MirConstant::Float(value.clone()),
            HirLiteral::Char(value) => MirConstant::Char(value.clone()),
            HirLiteral::String(value) => MirConstant::String(value.clone()),
            HirLiteral::None => unreachable!("none is lowered as an aggregate"),
        };
        MirOperand {
            ty,
            kind: MirOperandKind::Constant(constant),
        }
    }

    fn prefix_may_panic(&self, operator: crate::hir::HirPrefixOperator, ty: TypeId) -> bool {
        if operator != crate::hir::HirPrefixOperator::Negate {
            return false;
        }
        matches!(
            self.hir.interner().kind(ty),
            Ok(TypeKind::Scalar(
                ScalarType::Int | ScalarType::Int8 | ScalarType::Int16 | ScalarType::Int32
            ))
        )
    }

    fn binary_may_panic(&self, operator: HirBinaryOperator, ty: TypeId) -> bool {
        match operator {
            HirBinaryOperator::Multiply
            | HirBinaryOperator::Divide
            | HirBinaryOperator::Remainder
            | HirBinaryOperator::Add
            | HirBinaryOperator::Subtract
            | HirBinaryOperator::ShiftLeft
            | HirBinaryOperator::ShiftRight => !matches!(
                self.hir.interner().kind(ty),
                Ok(TypeKind::Scalar(ScalarType::Float | ScalarType::Float32))
            ),
            HirBinaryOperator::BitwiseAnd
            | HirBinaryOperator::BitwiseXor
            | HirBinaryOperator::BitwiseOr
            | HirBinaryOperator::Less
            | HirBinaryOperator::LessEqual
            | HirBinaryOperator::Greater
            | HirBinaryOperator::GreaterEqual
            | HirBinaryOperator::Equal
            | HirBinaryOperator::NotEqual
            | HirBinaryOperator::LogicalAnd
            | HirBinaryOperator::LogicalOr => false,
        }
    }

    fn expression(&self, id: HirExpressionId) -> Result<&HirExpression, MirError> {
        self.hir
            .expression(id)
            .ok_or_else(|| MirError::Construction {
                span: self.span,
                message: format!("missing verified expression#{}", id.index()),
            })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::hir::{
        ExpressionCheckLimits, HirCallableId, TypeLoweringLimits, check_expressions, lower_types,
    };
    use crate::mir::{MirTag, MirVerificationLimits, verify_mir, verify_mir_with_limits};
    use crate::package::PackageGraph;
    use crate::resolve::{ResolvedProgram, SymbolKind, resolve};
    use crate::source::{LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput};
    use crate::syntax::{LexMode, ParseLimits, ParseMode, lex, parse};

    use super::*;

    fn checked(source: &str) -> (ResolvedProgram, HirProgram) {
        let mut sources = SourceDatabase::new();
        let file = sources
            .add(SourceInput::virtual_file(
                SourceId::new("root:mir-lowering").unwrap(),
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
        let (hir, diagnostics) = lower_types(
            &packages,
            &sources,
            [(file, &parsed)],
            &resolved,
            TypeLoweringLimits {
                max_type_nodes: 100_000,
                max_trait_obligations: 100_000,
                max_diagnostics: 100,
            },
        )
        .unwrap()
        .into_parts();
        assert!(diagnostics.is_empty());
        let (hir, diagnostics, complete) = check_expressions(
            &sources,
            [(file, &parsed)],
            &resolved,
            hir,
            ExpressionCheckLimits {
                max_nodes: 100_000,
                max_pattern_steps: 100_000,
                max_trait_obligations: 100_000,
                max_diagnostics: 100,
            },
        )
        .unwrap()
        .into_parts();
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        assert!(complete);
        (resolved, hir)
    }

    fn function_id(resolved: &ResolvedProgram, name: &str) -> HirCallableId {
        HirCallableId::Symbol(
            resolved
                .symbols()
                .find(|symbol| {
                    symbol.kind() == SymbolKind::Function && symbol.name().as_str() == name
                })
                .unwrap()
                .id(),
        )
    }

    #[test]
    fn straight_line_functions_lower_to_typed_blocks_slots_and_unwind_edges() {
        let source = "fn add(left: Int, right: Int): Int {\n    left + right\n}\n\nfn main() {\n    let answer = add(20, 22)\n    _ = answer\n}\n";
        let (resolved, hir) = checked(source);
        let mir = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        assert_eq!(mir.functions().len(), 2);

        let add = mir.function(function_id(&resolved, "add")).unwrap();
        assert_eq!(add.parameters().len(), 2);
        assert_eq!(add.local(add.return_local()).unwrap().ty(), add.outcome());
        assert_eq!(add.block(add.entry()).unwrap().kind(), MirBlockKind::Normal);
        assert!(matches!(
            add.block(add.unwind()).unwrap().terminator().kind(),
            MirTerminatorKind::ResumePanic
        ));
        assert!(add.blocks().any(|block| matches!(
            block.terminator().kind(),
            MirTerminatorKind::Invoke {
                operation: MirOperation {
                    kind: MirOperationKind::CheckedBinary { .. },
                    ..
                },
                unwind,
                ..
            } if *unwind == add.unwind()
        )));
        assert!(
            add.blocks()
                .any(|block| { matches!(block.terminator().kind(), MirTerminatorKind::Return) })
        );
        verify_mir(&resolved, &hir, &mir).unwrap();
    }

    #[test]
    fn if_and_short_circuit_lower_to_explicit_deterministic_cfg() {
        let source = "fn choose(flag: Bool): Int {\n    if flag and true {\n        1\n    } else {\n        2\n    }\n}\n";
        let (resolved, hir) = checked(source);
        let first = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        let second = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        assert_eq!(format!("{first:#?}"), format!("{second:#?}"));
        let function = first.function(function_id(&resolved, "choose")).unwrap();
        assert!(
            function
                .blocks()
                .filter(|block| matches!(
                    block.terminator().kind(),
                    MirTerminatorKind::SwitchBool { .. }
                ))
                .count()
                >= 2
        );
        assert!(
            function
                .blocks()
                .all(|block| block.terminator().span().file() == function.span().file())
        );
    }

    #[test]
    fn all_loop_forms_and_never_loops_have_explicit_control_flow() {
        let source = "fn loops(\n\
                          values: Array[Int],\n\
                          entries: Map[String, Int],\n\
                          unique: Set[Int],\n\
                          numbers: Range[Int],\n\
                          text: String,\n\
                      ) {\n\
                          for {\n\
                              break\n\
                          }\n\
                          for false {\n\
                              continue\n\
                          }\n\
                          for value in values {\n\
                              _ = value\n\
                          }\n\
                          for entry in entries {\n\
                              _ = entry\n\
                          }\n\
                          for value in unique {\n\
                              _ = value\n\
                          }\n\
                          for value in numbers {\n\
                              _ = value\n\
                          }\n\
                          for character in text {\n\
                              _ = character\n\
                          }\n\
                      }\n\
                      fn forever(): Never {\n\
                          for {}\n\
                      }\n";
        let (resolved, hir) = checked(source);
        let mir = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();

        let loops = mir.function(function_id(&resolved, "loops")).unwrap();
        assert_eq!(
            loops
                .blocks()
                .filter(|block| matches!(
                    block.terminator().kind(),
                    MirTerminatorKind::IteratorNext { unwind, .. } if *unwind == loops.unwind()
                ))
                .count(),
            5
        );
        assert!(loops.blocks().any(|block| matches!(
            block.terminator().kind(),
            MirTerminatorKind::SwitchBool { .. }
        )));

        let forever = mir.function(function_id(&resolved, "forever")).unwrap();
        assert!(
            forever
                .blocks()
                .any(|block| matches!(block.terminator().kind(), MirTerminatorKind::Unreachable))
        );
        assert!(
            !forever
                .blocks()
                .any(|block| matches!(block.terminator().kind(), MirTerminatorKind::Return))
        );
        verify_mir(&resolved, &hir, &mir).unwrap();
    }

    #[test]
    fn assignments_resolve_validate_then_write_and_multiple_assignment_is_atomic() {
        let source = "fn index(): Int { 0 }\n\
                      fn replacement(): Int { 3 }\n\
                      fn update(values: var Array[Int]) {\n\
                          var left = 1\n\
                          var right = 2\n\
                          values[index()] = replacement()\n\
                          left += right\n\
                          (left, right) = (right, left)\n\
                      }\n";
        let (resolved, hir) = checked(source);
        let mir = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        let update = mir.function(function_id(&resolved, "update")).unwrap();

        let validations = update
            .blocks()
            .filter_map(|block| match block.terminator().kind() {
                MirTerminatorKind::ValidatePlaces {
                    for_write, unwind, ..
                } if *unwind == update.unwind() => Some(*for_write),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            validations.iter().filter(|for_write| **for_write).count(),
            3
        );
        assert_eq!(
            validations.iter().filter(|for_write| !**for_write).count(),
            1
        );
        assert!(update.blocks().any(|block| matches!(
            block.terminator().kind(),
            MirTerminatorKind::Invoke {
                operation: MirOperation {
                    kind: MirOperationKind::CheckedBinary { .. },
                    ..
                },
                ..
            }
        )));
        assert!(
            update
                .blocks()
                .flat_map(MirBasicBlock::statements)
                .any(|statement| {
                    matches!(
                        statement.kind(),
                        MirStatementKind::Assign {
                            value: MirRvalue {
                                kind: MirRvalueKind::Aggregate {
                                    shape: MirAggregateKind::Tuple,
                                    ..
                                },
                                ..
                            },
                            ..
                        }
                    )
                })
        );
        verify_mir(&resolved, &hir, &mir).unwrap();
    }

    #[test]
    fn indexed_and_sliced_reads_are_checked_operations_with_unwind_edges() {
        let source = "fn read(values: Array[Int], index: Int): Int {\n\
                          values[index]\n\
                      }\n\
                      fn view(values: Array[Int]): Array[Int] {\n\
                          values[1:]\n\
                      }\n";
        let (resolved, hir) = checked(source);
        let mir = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        let read = mir.function(function_id(&resolved, "read")).unwrap();
        assert!(read.blocks().any(|block| matches!(
            block.terminator().kind(),
            MirTerminatorKind::Invoke {
                operation: MirOperation {
                    kind: MirOperationKind::Index { .. },
                    ..
                },
                unwind,
                ..
            } if *unwind == read.unwind()
        )));
        let view = mir.function(function_id(&resolved, "view")).unwrap();
        assert!(view.blocks().any(|block| matches!(
            block.terminator().kind(),
            MirTerminatorKind::Invoke {
                operation: MirOperation {
                    kind: MirOperationKind::Slice { .. },
                    ..
                },
                unwind,
                ..
            } if *unwind == view.unwind()
        )));
        verify_mir(&resolved, &hir, &mir).unwrap();
    }

    #[test]
    fn collections_ranges_membership_and_constants_cover_the_remaining_value_forms() {
        let source = "const Answer: Int = 42\n\
                      fn collections(): (Array[Int], Map[String, Int?], Set[String]) {\n\
                          ([1, Answer], [\"one\": 1, \"none\": none], Set[\"read\", \"write\"])\n\
                      }\n\
                      fn inspect(): Bool {\n\
                          let numbers = 0..10\n\
                          let ages = [\"Ada\": 37]\n\
                          let permissions = Set[\"read\", \"write\"]\n\
                          5 in numbers and \"Ada\" in ages and\n\
                              \"read\" in permissions and 'x' in \"text\"\n\
                      }\n";
        let (resolved, hir) = checked(source);
        let mir = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        assert!(
            mir.functions()
                .flat_map(MirFunction::blocks)
                .any(|block| matches!(
                    block.terminator().kind(),
                    MirTerminatorKind::Invoke {
                        operation: MirOperation {
                            kind: MirOperationKind::BuildMap { .. },
                            ..
                        },
                        ..
                    }
                ))
        );
        assert!(
            mir.functions()
                .flat_map(MirFunction::blocks)
                .flat_map(MirBasicBlock::statements)
                .any(|statement| matches!(
                    statement.kind(),
                    MirStatementKind::Assign {
                        value: MirRvalue {
                            kind: MirRvalueKind::Contains { .. } | MirRvalueKind::Range { .. },
                            ..
                        },
                        ..
                    }
                ))
        );
        verify_mir(&resolved, &hir, &mir).unwrap();
    }

    #[test]
    fn nominal_construction_updates_and_every_variant_payload_lower_with_instantiated_types() {
        let source = "type UserId = Int\n\
                      type User = {\n\
                          id: UserId\n\
                          name: String\n\
                          email: String?\n\
                      }\n\
                      enum Shape {\n\
                          Point\n\
                          Circle(Float)\n\
                          Rectangle { width: Float, height: Float }\n\
                      }\n\
                      fn make(id: UserId, name: String): (User, Shape, Shape, Shape) {\n\
                          (\n\
                              User { id, name, email: none },\n\
                              Shape.Point,\n\
                              Shape.Circle(2.5),\n\
                              Shape.Rectangle { width: 3.0, height: 4.0 },\n\
                          )\n\
                      }\n\
                      fn rename(user: User): User {\n\
                          user with { name: \"Grace\", email: none }\n\
                      }\n\
                      fn make_id(): UserId { UserId(42) }\n";
        let (resolved, hir) = checked(source);
        let mir = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        let shapes = mir
            .functions()
            .flat_map(MirFunction::blocks)
            .flat_map(MirBasicBlock::statements)
            .filter_map(|statement| match statement.kind() {
                MirStatementKind::Assign {
                    value:
                        MirRvalue {
                            kind: MirRvalueKind::Aggregate { shape, .. },
                            ..
                        },
                    ..
                } => Some(shape),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            shapes
                .iter()
                .any(|shape| matches!(shape, MirAggregateKind::Newtype { .. }))
        );
        assert!(
            shapes
                .iter()
                .any(|shape| matches!(shape, MirAggregateKind::Record { .. }))
        );
        assert_eq!(
            shapes
                .iter()
                .filter(|shape| matches!(shape, MirAggregateKind::Variant { .. }))
                .count(),
            3
        );
        assert!(
            mir.functions()
                .flat_map(MirFunction::blocks)
                .flat_map(MirBasicBlock::statements)
                .any(|statement| matches!(
                    statement.kind(),
                    MirStatementKind::Assign {
                        value: MirRvalue {
                            kind: MirRvalueKind::RecordUpdate { .. },
                            ..
                        },
                        ..
                    }
                ))
        );
        verify_mir(&resolved, &hir, &mir).unwrap();
    }

    #[test]
    fn named_variadic_and_method_calls_retain_modes_and_argument_associations() {
        let source = "type Counter = { value: Int }\n\
                      fn Counter.add(self, amount: Int): Int { self.value + amount }\n\
                      fn connect(host: String, port: Int): String { host }\n\
                      fn log(prefix: String, parts: ...String): Array[String] { parts }\n\
                      fn use(counter: Counter): Int {\n\
                          _ = connect(port: 8080, host: \"localhost\")\n\
                          let parts = [\"server\", \" started\"]\n\
                          _ = log(\"Info: \", ...parts)\n\
                          counter.add(amount: 3)\n\
                      }\n";
        let (resolved, hir) = checked(source);
        let mir = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        let calls = mir
            .functions()
            .flat_map(MirFunction::blocks)
            .filter_map(|block| match block.terminator().kind() {
                MirTerminatorKind::Invoke {
                    operation:
                        MirOperation {
                            kind: MirOperationKind::Call { arguments, .. },
                            ..
                        },
                    ..
                } => Some(arguments),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            calls
                .iter()
                .any(|arguments| arguments.iter().any(|argument| {
                    argument.target() == crate::hir::HirCallArgumentTarget::VariadicSpread
                }))
        );
        assert!(calls.iter().any(|arguments| {
            arguments
                .iter()
                .any(|argument| argument.target() == crate::hir::HirCallArgumentTarget::Receiver)
        }));
        verify_mir(&resolved, &hir, &mir).unwrap();
    }

    #[test]
    fn total_and_checked_numeric_conversions_preserve_the_closed_result_shape() {
        let source = "fn widen(value: Int32): Int { Int(value) }\n\
                      fn narrow(value: Int): Int8 ! NumericConversionError { Int8(value) }\n\
                      fn propagated(value: Int): Int8 ! NumericConversionError { Int8(value)? }\n";
        let (resolved, hir) = checked(source);
        let mir = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        assert_eq!(
            mir.functions()
                .flat_map(MirFunction::blocks)
                .flat_map(MirBasicBlock::statements)
                .filter(|statement| matches!(
                    statement.kind(),
                    MirStatementKind::Assign {
                        value: MirRvalue {
                            kind: MirRvalueKind::NumericConversion { .. },
                            ..
                        },
                        ..
                    }
                ))
                .count(),
            3
        );
        verify_mir(&resolved, &hir, &mir).unwrap();
    }

    #[test]
    fn option_and_result_propagation_branch_to_payload_or_early_return() {
        let source = "fn source(): Int ! String { 1 }\n\
                      fn optional(): Int? { some(1) }\n\
                      fn widen(): Int ! (Bool | String) { source()? }\n\
                      fn unwrap_optional(): Int? { optional()? }\n\
                      fn nested(): Int? ! String { optional()? }\n";
        let (resolved, hir) = checked(source);
        let mir = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();

        let widen = mir.function(function_id(&resolved, "widen")).unwrap();
        assert!(widen.blocks().any(|block| matches!(
            block.terminator().kind(),
            MirTerminatorKind::SwitchTag { cases, .. }
                if cases.iter().any(|(tag, _)| *tag == MirTag::ResultOk)
        )));
        assert!(
            widen
                .blocks()
                .filter(|block| matches!(block.terminator().kind(), MirTerminatorKind::Return))
                .count()
                >= 2
        );

        for name in ["unwrap_optional", "nested"] {
            let function = mir.function(function_id(&resolved, name)).unwrap();
            assert!(function.blocks().any(|block| matches!(
                block.terminator().kind(),
                MirTerminatorKind::SwitchTag { cases, .. }
                    if cases.iter().any(|(tag, _)| *tag == MirTag::OptionSome)
            )));
            assert!(
                function
                    .blocks()
                    .filter(|block| matches!(block.terminator().kind(), MirTerminatorKind::Return))
                    .count()
                    >= 2
            );
        }
        verify_mir(&resolved, &hir, &mir).unwrap();
    }

    #[test]
    fn match_patterns_guards_and_diverging_arms_lower_to_tag_and_value_branches() {
        let source = "enum Choice {\n\
                          Empty\n\
                          Item(Int)\n\
                      }\n\
                      fn choose(value: Choice, enabled: Bool): Int {\n\
                          match value {\n\
                              Choice.Item(number) if enabled => number\n\
                              Choice.Item(_) => 1\n\
                              Choice.Empty => 0\n\
                          }\n\
                      }\n\
                      fn inspect(value: Int ! String): Int {\n\
                          match value {\n\
                              ok(number) => number\n\
                              err(_) => return 0\n\
                          }\n\
                      }\n\
                      fn first(values: Array[Int]): Int {\n\
                          match values {\n\
                              [value, ..] => value\n\
                              [] => 0\n\
                          }\n\
                      }\n";
        let (resolved, hir) = checked(source);
        let mir = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();

        let choose = mir.function(function_id(&resolved, "choose")).unwrap();
        assert!(choose.blocks().any(|block| matches!(
            block.terminator().kind(),
            MirTerminatorKind::SwitchTag { cases, .. }
                if cases.iter().any(|(tag, _)| matches!(tag, MirTag::Variant(_)))
        )));
        assert!(choose.blocks().any(|block| matches!(
            block.terminator().kind(),
            MirTerminatorKind::SwitchBool { .. }
        )));

        let inspect = mir.function(function_id(&resolved, "inspect")).unwrap();
        assert!(inspect.blocks().any(|block| matches!(
            block.terminator().kind(),
            MirTerminatorKind::SwitchTag { cases, .. }
                if cases.iter().any(|(tag, _)| *tag == MirTag::ResultOk)
        )));

        let first = mir.function(function_id(&resolved, "first")).unwrap();
        assert!(
            first
                .blocks()
                .flat_map(MirBasicBlock::statements)
                .any(|statement| matches!(
                    statement.kind(),
                    MirStatementKind::Assign {
                        value: MirRvalue {
                            kind: MirRvalueKind::Length(_),
                            ..
                        },
                        ..
                    }
                ))
        );
        verify_mir(&resolved, &hir, &mir).unwrap();
    }

    #[test]
    fn mir_verifier_rejects_unknown_successors_and_broken_cleanup_edges() {
        let source = "fn add(left: Int, right: Int): Int {\n\
                          left + right\n\
                      }\n\
                      fn main() {\n\
                          _ = add(1, 2)\n\
                      }\n";
        let (resolved, hir) = checked(source);

        let mut invalid_successor =
            lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        let main = invalid_successor
            .functions
            .get_mut(&MirFunctionId::Callable(function_id(&resolved, "main")))
            .unwrap();
        let target = main
            .blocks
            .iter_mut()
            .find_map(|block| match &mut block.terminator.kind {
                MirTerminatorKind::Invoke {
                    target: Some(target),
                    ..
                } => Some(target),
                _ => None,
            })
            .unwrap();
        *target = MirBlockId(u32::MAX);
        let error = verify_mir(&resolved, &hir, &invalid_successor).unwrap_err();
        assert!(error.message().contains("unknown MIR block"));

        let mut invalid_cleanup =
            lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        let main = invalid_cleanup
            .functions
            .get_mut(&MirFunctionId::Callable(function_id(&resolved, "main")))
            .unwrap();
        let entry = main.entry;
        let unwind = main
            .blocks
            .iter_mut()
            .find_map(|block| match &mut block.terminator.kind {
                MirTerminatorKind::Invoke { unwind, .. } => Some(unwind),
                _ => None,
            })
            .unwrap();
        *unwind = entry;
        let error = verify_mir(&resolved, &hir, &invalid_cleanup).unwrap_err();
        assert!(error.message().contains("cleanup"));
    }

    #[test]
    fn mir_verifier_rejects_use_before_definition_and_invalid_projection() {
        let (resolved, hir) = checked("fn value(): Int { 1 }\n");
        let mut invalid_use = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        let function = invalid_use
            .functions
            .get_mut(&MirFunctionId::Callable(function_id(&resolved, "value")))
            .unwrap();
        let entry = function.entry.0 as usize;
        let MirStatementKind::Assign { destination, value } =
            &mut function.blocks[entry].statements[0].kind
        else {
            panic!("literal function starts with an assignment");
        };
        *value = MirRvalue {
            ty: destination.ty,
            kind: MirRvalueKind::Use(MirOperand {
                ty: destination.ty,
                kind: MirOperandKind::Copy(destination.clone()),
            }),
        };
        let error = verify_mir(&resolved, &hir, &invalid_use).unwrap_err();
        assert!(error.message().contains("before a dominating"));

        let (resolved, hir) = checked("fn first(pair: (Int, String)): Int { pair.0 }\n");
        let mut invalid_projection =
            lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        let function = invalid_projection
            .functions
            .get_mut(&MirFunctionId::Callable(function_id(&resolved, "first")))
            .unwrap();
        let projection = function
            .blocks
            .iter_mut()
            .flat_map(|block| &mut block.statements)
            .find_map(|statement| match &mut statement.kind {
                MirStatementKind::Assign {
                    value:
                        MirRvalue {
                            kind:
                                MirRvalueKind::Use(MirOperand {
                                    kind: MirOperandKind::Copy(place),
                                    ..
                                }),
                            ..
                        },
                    ..
                } => place.projections.first_mut(),
                _ => None,
            })
            .unwrap();
        projection.kind = MirProjectionKind::TupleField(99);
        let error = verify_mir(&resolved, &hir, &invalid_projection).unwrap_err();
        assert!(error.message().contains("out of range"));

        let (resolved, hir) = checked(
            "fn replace() {\n\
                 var values = [1, 2]\n\
                 values[:] = [3, 4]\n\
             }\n",
        );
        let mut invalid_slice =
            lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        let function = invalid_slice
            .functions
            .get_mut(&MirFunctionId::Callable(function_id(&resolved, "replace")))
            .unwrap();
        let replacement = function
            .blocks
            .iter_mut()
            .find_map(|block| match &mut block.terminator.kind {
                MirTerminatorKind::ValidatePlaces {
                    replacements,
                    for_write: true,
                    ..
                } => replacements
                    .iter_mut()
                    .find(|replacement| replacement.is_some()),
                _ => None,
            })
            .expect("slice assignment has a checked replacement");
        *replacement = None;
        let error = verify_mir(&resolved, &hir, &invalid_slice).unwrap_err();
        assert!(error.message().contains("replacement shape"));
    }

    #[test]
    fn mir_verifier_rejects_a_forged_closure_capture_layout() {
        fn closure_values(mir: &mut MirProgram) -> &mut Vec<MirOperand> {
            mir.functions
                .values_mut()
                .flat_map(|function| &mut function.blocks)
                .flat_map(|block| &mut block.statements)
                .find_map(|statement| {
                    let MirStatementKind::Assign {
                        value:
                            MirRvalue {
                                kind:
                                    MirRvalueKind::Aggregate {
                                        shape: MirAggregateKind::Closure { .. },
                                        values,
                                    },
                                ..
                            },
                        ..
                    } = &mut statement.kind
                    else {
                        return None;
                    };
                    Some(values)
                })
                .expect("closure construction lowers to a MIR aggregate")
        }

        let source = "fn build() {\n\
                          let seed = 41\n\
                          let closure = (): Int { seed + 1 }\n\
                          _ = closure\n\
                      }\n";
        let (resolved, hir) = checked(source);
        let mut mir = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        verify_mir(&resolved, &hir, &mir).unwrap();

        closure_values(&mut mir).clear();

        let error = verify_mir(&resolved, &hir, &mir).unwrap_err();
        assert!(error.message().contains("capture layout"));

        let mut wrong_source = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        closure_values(&mut wrong_source)[0].kind =
            MirOperandKind::Constant(MirConstant::Integer("41".into()));
        let error = verify_mir(&resolved, &hir, &wrong_source).unwrap_err();
        assert!(error.message().contains("source binding"));
    }

    #[test]
    fn all_closure_effect_kinds_keep_their_hidden_environment_in_mir() {
        let source = "fn build() {\n\
                          let sync: fn(): Int = () { 1 }\n\
                          let raw: unsafe fn(): Int = unsafe () { 2 }\n\
                          let later: async fn(): Int = async () { 3 }\n\
                          let both: async unsafe fn(): Int = async unsafe () { 4 }\n\
                          _ = sync()\n\
                          _ = sync\n\
                          _ = raw\n\
                          _ = later\n\
                          _ = both\n\
                      }\n";
        let (resolved, hir) = checked(source);
        let mir = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        verify_mir(&resolved, &hir, &mir).unwrap();

        assert_eq!(hir.closures().count(), 4);
        assert_eq!(mir.functions().len(), 5);
        for closure in hir.closures() {
            let body = mir
                .closure_function(closure.id())
                .expect("each effectful closure keeps a MIR body");
            assert_eq!(body.parameters().len(), 1);
            assert_eq!(body.local(body.parameters()[0]).unwrap().ty(), closure.ty());
        }
        let aggregates = mir
            .functions()
            .flat_map(|function| function.blocks())
            .flat_map(|block| block.statements())
            .filter(|statement| {
                matches!(
                    statement.kind(),
                    MirStatementKind::Assign {
                        value: MirRvalue {
                            kind: MirRvalueKind::Aggregate {
                                shape: MirAggregateKind::Closure { .. },
                                ..
                            },
                            ..
                        },
                        ..
                    }
                )
            })
            .count();
        assert_eq!(aggregates, 4);

        let async_signature = hir
            .closures()
            .find(|closure| closure.is_async() && !closure.is_unsafe())
            .unwrap()
            .function_type();
        let mut forged_call = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        let signature = forged_call
            .functions
            .values_mut()
            .flat_map(|function| &mut function.blocks)
            .find_map(|block| match &mut block.terminator.kind {
                MirTerminatorKind::Invoke {
                    operation:
                        MirOperation {
                            kind: MirOperationKind::Call { signature, .. },
                            ..
                        },
                    ..
                } => Some(signature),
                _ => None,
            })
            .unwrap();
        *signature = async_signature;
        let error = verify_mir(&resolved, &hir, &forged_call).unwrap_err();
        assert!(error.message().contains("effectful call"), "{error}");
    }

    #[test]
    fn mir_verifier_rejects_an_assert_without_its_condition_representation() {
        let (resolved, hir) = checked("fn check() { assert(true) }\n");
        let mut mir = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        let function = mir
            .functions
            .get_mut(&MirFunctionId::Callable(function_id(&resolved, "check")))
            .unwrap();
        let condition_repr = function
            .blocks
            .iter_mut()
            .find_map(|block| match &mut block.terminator.kind {
                MirTerminatorKind::Invoke {
                    operation:
                        MirOperation {
                            kind: MirOperationKind::Assert { condition_repr, .. },
                            ..
                        },
                    ..
                } => Some(condition_repr),
                _ => None,
            })
            .expect("assert lowers to a checked MIR operation");
        condition_repr.clear();
        let error = verify_mir(&resolved, &hir, &mir).unwrap_err();
        assert!(error.message().contains("condition representation"));
    }

    #[test]
    fn mir_verifier_rejects_call_arity_tag_type_and_exhausted_budget() {
        let source = "fn add(left: Int, right: Int): Int {\n\
                          left + right\n\
                      }\n\
                      fn main() {\n\
                          _ = add(1, 2)\n\
                      }\n";
        let (resolved, hir) = checked(source);
        let mut invalid_call = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        let main = invalid_call
            .functions
            .get_mut(&MirFunctionId::Callable(function_id(&resolved, "main")))
            .unwrap();
        let arguments = main
            .blocks
            .iter_mut()
            .find_map(|block| match &mut block.terminator.kind {
                MirTerminatorKind::Invoke {
                    operation:
                        MirOperation {
                            kind: MirOperationKind::Call { arguments, .. },
                            ..
                        },
                    ..
                } => Some(arguments),
                _ => None,
            })
            .unwrap();
        arguments.pop();
        let error = verify_mir(&resolved, &hir, &invalid_call).unwrap_err();
        assert!(error.message().contains("omits"));

        let (resolved, hir) = checked(
            "fn inspect(value: Int?): Int {\n\
                 match value {\n\
                     some(number) => number\n\
                     none => 0\n\
                 }\n\
             }\n",
        );
        let mut invalid_tag = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        let inspect = invalid_tag
            .functions
            .get_mut(&MirFunctionId::Callable(function_id(&resolved, "inspect")))
            .unwrap();
        let tag = inspect
            .blocks
            .iter_mut()
            .find_map(|block| match &mut block.terminator.kind {
                MirTerminatorKind::SwitchTag { cases, .. } => cases.first_mut().map(|(tag, _)| tag),
                _ => None,
            })
            .unwrap();
        *tag = MirTag::ResultOk;
        let error = verify_mir(&resolved, &hir, &invalid_tag).unwrap_err();
        assert!(error.message().contains("incompatible"));

        let mut unguarded_payload =
            lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        let inspect = unguarded_payload
            .functions
            .get_mut(&MirFunctionId::Callable(function_id(&resolved, "inspect")))
            .unwrap();
        let (case, otherwise) = inspect
            .blocks
            .iter_mut()
            .find_map(|block| match &mut block.terminator.kind {
                MirTerminatorKind::SwitchTag {
                    cases, otherwise, ..
                } => cases.first_mut().map(|(_, case)| (case, otherwise)),
                _ => None,
            })
            .unwrap();
        std::mem::swap(case, otherwise);
        let error = verify_mir(&resolved, &hir, &unguarded_payload).unwrap_err();
        assert!(error.message().contains("without a dominating"));

        let (resolved, hir) = checked(
            "fn choose(flag: Bool): Int {\n\
                 if flag { 1 } else { 2 }\n\
             }\n",
        );
        let mir = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        let error = verify_mir_with_limits(
            &resolved,
            &hir,
            &mir,
            MirVerificationLimits {
                max_dataflow_steps: 0,
            },
        )
        .unwrap_err();
        assert!(error.message().contains("dataflow budget"));
    }

    #[test]
    fn mir_verifier_rederives_generic_and_opaque_call_protocols() {
        fn first_call_protocol(function: &mut MirFunction) -> &mut HirCallProtocol {
            function
                .blocks
                .iter_mut()
                .find_map(|block| match &mut block.terminator.kind {
                    MirTerminatorKind::Invoke {
                        operation:
                            MirOperation {
                                kind: MirOperationKind::Call { protocol, .. },
                                ..
                            },
                        ..
                    } => Some(protocol),
                    _ => None,
                })
                .expect("function contains a call")
        }

        let (resolved, hir) = checked(
            "fn apply[F: Call[fn(Int): Int]](operation: F, value: Int): Int {\n\
                 operation(value)\n\
             }\n",
        );
        let mut generic = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        *first_call_protocol(
            generic
                .functions
                .get_mut(&MirFunctionId::Callable(function_id(&resolved, "apply")))
                .unwrap(),
        ) = HirCallProtocol::CallMut;
        let error = verify_mir(&resolved, &hir, &generic).unwrap_err();
        assert!(error.message().contains("closed callee contract"));

        let (resolved, hir) = checked(
            "fn make(offset: Int): impl Call[fn(Int): Int] + Discard {\n\
                 (value: Int): Int { value + offset }\n\
             }\n\
             fn execute(): Int {\n\
                 let operation = make(40)\n\
                 operation(2)\n\
             }\n",
        );
        let mut opaque = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        *first_call_protocol(
            opaque
                .functions
                .get_mut(&MirFunctionId::Callable(function_id(&resolved, "execute")))
                .unwrap(),
        ) = HirCallProtocol::CallOnce;
        let error = verify_mir(&resolved, &hir, &opaque).unwrap_err();
        assert!(error.message().contains("closed callee contract"));
    }

    #[test]
    fn mir_verifier_confines_environment_borrows_to_indirect_callees() {
        let (resolved, hir) = checked(
            "fn execute(): Int {\n\
                 let operation = (value: Int): Int { value + 1 }\n\
                 operation(41)\n\
             }\n",
        );
        let mut mir = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        let execute = mir
            .functions
            .get_mut(&MirFunctionId::Callable(function_id(&resolved, "execute")))
            .unwrap();
        let (callee, arguments) = execute
            .blocks
            .iter_mut()
            .find_map(|block| match &mut block.terminator.kind {
                MirTerminatorKind::Invoke {
                    operation:
                        MirOperation {
                            kind:
                                MirOperationKind::Call {
                                    callee, arguments, ..
                                },
                            ..
                        },
                    ..
                } if matches!(callee.kind, MirOperandKind::Borrow(_)) => Some((callee, arguments)),
                _ => None,
            })
            .expect("closure place call borrows its environment");
        let MirOperandKind::Borrow(environment) = &callee.kind else {
            unreachable!()
        };
        arguments[0].value.kind = MirOperandKind::Borrow(environment.clone());

        let error = verify_mir(&resolved, &hir, &mir).unwrap_err();
        assert!(error.message().contains("borrow escapes"));
    }

    #[test]
    fn mir_verifier_rejects_invalid_prelude_trait_function_operands() {
        let source = "type Label = { text: String }\n\
                      impl Display for Label {\n\
                          fn display(self): String { self.text }\n\
                      }\n\
                      fn render(value: Label): String { Display.display(value) }\n";
        let (resolved, hir) = checked(source);
        let mut mir = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        verify_mir(&resolved, &hir, &mir).unwrap();

        let render = mir
            .functions
            .get_mut(&MirFunctionId::Callable(function_id(&resolved, "render")))
            .unwrap();
        let callee = render
            .blocks
            .iter_mut()
            .find_map(|block| match &mut block.terminator.kind {
                MirTerminatorKind::Invoke {
                    operation:
                        MirOperation {
                            kind: MirOperationKind::Call { callee, .. },
                            ..
                        },
                    ..
                } => Some(callee),
                _ => None,
            })
            .expect("qualified Display call lowers to a MIR call");
        let MirOperandKind::PreludeTraitFunction { arguments, .. } = &mut callee.kind else {
            panic!("qualified Display call retains its prelude trait identity")
        };
        arguments.push(hir.interner().scalar(ScalarType::Unit));

        let error = verify_mir(&resolved, &hir, &mir).unwrap_err();
        assert!(error.message().contains("specialization arity"));
    }

    #[test]
    fn mir_verifier_rejects_mutated_opaque_seals() {
        let source = "fn hidden(): impl Discard { 42 }\n";
        let (resolved, hir) = checked(source);
        let mut wrong_kind = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        verify_mir(&resolved, &hir, &wrong_kind).unwrap();
        let value = wrong_kind
            .functions
            .values_mut()
            .flat_map(|function| &mut function.blocks)
            .flat_map(|block| &mut block.statements)
            .find_map(|statement| {
                let MirStatementKind::Assign { value, .. } = &mut statement.kind else {
                    return None;
                };
                matches!(
                    &value.kind,
                    MirRvalueKind::Coerce {
                        kind: crate::types::Assignability::Opaque,
                        ..
                    }
                )
                .then_some(value)
            })
            .expect("opaque return lowers to a MIR seal");
        let MirRvalueKind::Coerce { kind, .. } = &mut value.kind else {
            unreachable!()
        };
        *kind = crate::types::Assignability::OptionLift;
        let error = verify_mir(&resolved, &hir, &wrong_kind).unwrap_err();
        assert!(error.message().contains("closed assignability relation"));

        let mut wrong_type = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        let value = wrong_type
            .functions
            .values_mut()
            .flat_map(|function| &mut function.blocks)
            .flat_map(|block| &mut block.statements)
            .find_map(|statement| {
                let MirStatementKind::Assign { value, .. } = &mut statement.kind else {
                    return None;
                };
                matches!(
                    &value.kind,
                    MirRvalueKind::Coerce {
                        kind: crate::types::Assignability::Opaque,
                        ..
                    }
                )
                .then_some(value)
            })
            .expect("opaque return lowers to a MIR seal");
        let MirRvalueKind::Coerce { value: witness, .. } = &value.kind else {
            unreachable!()
        };
        let witness = witness.ty;
        value.ty = witness;
        let error = verify_mir(&resolved, &hir, &wrong_type).unwrap_err();
        assert!(
            error.message().contains("closed assignability relation")
                || error.message().contains("rvalue type")
        );
    }

    #[test]
    fn mir_construction_limits_fail_before_unbounded_growth() {
        let (resolved, hir) = checked("fn main() {}\n");
        let error = lower_to_mir(
            &resolved,
            &hir,
            MirLoweringLimits {
                max_blocks_per_function: 1,
                ..MirLoweringLimits::default()
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            MirError::NodeLimit {
                resource: "basic block",
                ..
            }
        ));
    }
}
