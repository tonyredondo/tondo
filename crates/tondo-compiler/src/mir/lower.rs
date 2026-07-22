use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::hir::{
    CapabilityAnalysis, CapabilityAssumptions, HirAssignmentOperator, HirAssignmentTarget,
    HirAssignmentTargetKind, HirBinaryOperator, HirBootstrapHostFunction, HirCallProtocol,
    HirCallableSignature, HirCapability, HirCapabilityStatus, HirClosure, HirExpression,
    HirExpressionId, HirExpressionKind, HirForKind, HirIterationProtocol, HirLiteral, HirLoopId,
    HirMatchMode, HirNominalShape, HirPatternId, HirPatternKind, HirPreludeTraitMethod, HirProgram,
    HirStatement, HirValueCategory, HirVariantPayload, HirVariantValue, StaticRegionRelation,
    verify_typed_hir,
};
use crate::resolve::{LocalId, MemberKind, ResolvedProgram};
use crate::source::Span;
use crate::types::{ScalarType, TypeId, TypeKind};

use super::{
    MirAggregateKind, MirAssertMessagePart, MirBasicBlock, MirBlockId, MirBlockKind,
    MirBootstrapHostFunction, MirCallArgument, MirConstant, MirError, MirFunction, MirFunctionId,
    MirLoan, MirLoanId, MirLoanKind, MirLocal, MirLocalId, MirLocalKind, MirOperand,
    MirOperandKind, MirOperation, MirOperationKind, MirPlace, MirProgram, MirProjection,
    MirProjectionKind, MirRvalue, MirRvalueKind, MirSliceBounds, MirStatement, MirStatementKind,
    MirTag, MirTerminator, MirTerminatorKind, MirVerificationLimits,
    verify_mir_with_capability_analysis, verify_mir_with_limits,
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
    let first_body_span = hir
        .callables()
        .find(|callable| hir.body(callable.id()).is_some())
        .map(HirCallableSignature::span)
        .or_else(|| hir.closures().next().map(HirClosure::span));
    let capability_analysis = first_body_span
        .map(|span| {
            CapabilityAnalysis::new(hir, resolved).map_err(|error| MirError::Construction {
                span,
                message: format!("cannot derive ownership capabilities: {error}"),
            })
        })
        .transpose()?;
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
        let function = FunctionBuilder::new(
            resolved,
            hir,
            capability_analysis
                .as_ref()
                .expect("a callable body requires capability analysis"),
            callable,
            limits,
        )?
        .lower(body.root())?;
        functions.insert(MirFunctionId::Callable(callable.id()), function);
    }
    for closure in hir.closures() {
        if functions.len() >= limits.max_functions as usize {
            return Err(MirError::NodeLimit {
                span: closure.span(),
                resource: "function",
            });
        }
        let function = FunctionBuilder::new_closure(
            resolved,
            hir,
            capability_analysis
                .as_ref()
                .expect("a closure body requires capability analysis"),
            closure,
            limits,
        )?
        .lower(closure.body().root())?;
        functions.insert(MirFunctionId::Closure(closure.id()), function);
    }
    let program = MirProgram { functions };
    let verification = if let Some(capability_analysis) = capability_analysis.as_ref() {
        verify_mir_with_capability_analysis(
            resolved,
            hir,
            &program,
            MirVerificationLimits {
                max_dataflow_steps: limits.max_verification_steps,
            },
            capability_analysis,
        )
    } else {
        verify_mir_with_limits(
            resolved,
            hir,
            &program,
            MirVerificationLimits {
                max_dataflow_steps: limits.max_verification_steps,
            },
        )
    };
    if let Err(error) = verification {
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
    resolved: &'a ResolvedProgram,
    hir: &'a HirProgram,
    capability_analysis: &'a CapabilityAnalysis,
    capability_assumptions: CapabilityAssumptions,
    copy_statuses: RefCell<BTreeMap<TypeId, HirCapabilityStatus>>,
    id: MirFunctionId,
    span: Span,
    outcome: TypeId,
    limits: MirLoweringLimits,
    statement_count: u32,
    locals: Vec<MirLocal>,
    loans: Vec<MirLoan>,
    active_loans: Vec<MirLoanId>,
    parameters: Vec<MirLocalId>,
    source_locals: BTreeMap<LocalId, MirLocalId>,
    capture_places: BTreeMap<LocalId, MirPlace>,
    borrow_aliases: BTreeMap<LocalId, MirPlace>,
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
    loan_depth: usize,
}

#[derive(Clone, Copy)]
struct IntrinsicIteration {
    pattern: HirPatternId,
    source: HirExpressionId,
    cursor: TypeId,
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum MatchBindingPhase {
    Guard,
    Body,
}

fn assignment_target_contains_place(target: &LoweredAssignmentTarget) -> bool {
    match target {
        LoweredAssignmentTarget::Place { .. } => true,
        LoweredAssignmentTarget::Discard => false,
        LoweredAssignmentTarget::Tuple { items, .. } => {
            items.iter().any(assignment_target_contains_place)
        }
    }
}

impl<'a> FunctionBuilder<'a> {
    fn new(
        resolved: &'a ResolvedProgram,
        hir: &'a HirProgram,
        capability_analysis: &'a CapabilityAnalysis,
        callable: &HirCallableSignature,
        limits: MirLoweringLimits,
    ) -> Result<Self, MirError> {
        let id = MirFunctionId::Callable(callable.id());
        let span = callable.span();
        let mut builder = Self {
            resolved,
            hir,
            capability_analysis,
            capability_assumptions: CapabilityAssumptions::from_generics(hir, callable.generics()),
            copy_statuses: RefCell::new(BTreeMap::new()),
            id,
            span,
            outcome: callable.outcome(),
            limits,
            statement_count: 0,
            locals: Vec::new(),
            loans: Vec::new(),
            active_loans: Vec::new(),
            parameters: Vec::new(),
            source_locals: BTreeMap::new(),
            capture_places: BTreeMap::new(),
            borrow_aliases: BTreeMap::new(),
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
        resolved: &'a ResolvedProgram,
        hir: &'a HirProgram,
        capability_analysis: &'a CapabilityAnalysis,
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
            resolved,
            hir,
            capability_analysis,
            capability_assumptions: CapabilityAssumptions::from_generics(hir, closure.generics()),
            copy_statuses: RefCell::new(BTreeMap::new()),
            id: MirFunctionId::Closure(closure.id()),
            span,
            outcome: function.outcome(),
            limits,
            statement_count: 0,
            locals: Vec::new(),
            loans: Vec::new(),
            active_loans: Vec::new(),
            parameters: Vec::new(),
            source_locals: BTreeMap::new(),
            capture_places: BTreeMap::new(),
            borrow_aliases: BTreeMap::new(),
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
        self.infer_region_releases()?;
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
        let mut function = MirFunction {
            id: self.id,
            span: self.span,
            outcome: self.outcome,
            locals: self.locals,
            loans: self.loans,
            parameters: self.parameters,
            return_local: self.return_local,
            entry: self.entry,
            unwind: self.unwind,
            blocks,
        };
        populate_runtime_loan_checks(self.hir, &mut function, self.limits)?;
        Ok(function)
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
                let value = self.transfer_place(place, span)?;
                self.assign_operand(block, span, destination, value)?;
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
                    let operand = self.transfer_place(place, span)?;
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
                let observes = matches!(
                    operator,
                    HirBinaryOperator::Equal | HirBinaryOperator::NotEqual
                );
                let left = if observes {
                    self.lower_borrowed_value(*left, block)?
                } else {
                    self.lower_value(*left, block)?
                };
                let Some((block, left)) = left else {
                    return Ok(None);
                };
                let right = if observes {
                    self.lower_borrowed_value(*right, block)?
                } else {
                    self.lower_value(*right, block)?
                };
                let Some((block, right)) = right else {
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
                let Some((block, item)) = self.lower_borrowed_value(*item, block)? else {
                    return Ok(None);
                };
                let Some((block, container)) = self.lower_borrowed_value(*container, block)? else {
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
                let value = self.transfer_place(place, span)?;
                self.assign_operand(block, span, destination, value)?;
                Ok(Some(block))
            }
            HirExpressionKind::Index {
                base,
                index,
                access,
            } => {
                let Some((block, base)) = self.lower_borrowed_value(*base, block)? else {
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
                            against: Vec::new(),
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
                let Some((mut current, base)) = self.lower_borrowed_value(*base, block)? else {
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
                            bounds: Box::new(MirSliceBounds { start, end, step }),
                            against: Vec::new(),
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
                let loan_depth = self.active_loans.len();
                let mut lowered = Vec::with_capacity(arguments.len());
                for argument in arguments {
                    let result = if argument.mode() == crate::types::ParameterMode::Value {
                        self.lower_value(argument.value(), current)?
                    } else {
                        self.lower_loan_value(argument.value(), argument.mode(), current)?
                    };
                    let Some((next, value)) = result else {
                        self.active_loans.truncate(loan_depth);
                        return Ok(None);
                    };
                    current = next;
                    lowered.push(MirCallArgument {
                        mode: argument.mode(),
                        target: argument.target(),
                        value,
                    });
                }
                self.consume_call_loans(&lowered, loan_depth, span)?;
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
            HirExpressionKind::Match {
                scrutinee,
                mode,
                arms,
            } => self.lower_match(*scrutinee, *mode, arms, span, destination, block),
            HirExpressionKind::Return { value } => {
                let return_place = self.local_place(self.return_local);
                let end = if let Some(value) = value {
                    self.lower_expression(*value, return_place, block)?
                } else {
                    self.assign_operand(block, span, return_place, self.unit_operand())?;
                    Some(block)
                };
                if let Some(end) = end {
                    self.release_loans_from(end, span, 0)?;
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
                self.release_loans_from(block, span, 0)?;
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
                self.release_loans_from(block, span, targets.loan_depth)?;
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
                self.release_loans_from(block, span, targets.loan_depth)?;
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
        let result = self.transfer_local(result, span)?;
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
        let against = vec![Vec::new(); places.len()];
        let target_block = self.allocate_block(MirBlockKind::Normal)?;
        self.terminate(
            block,
            span,
            MirTerminatorKind::ValidatePlaces {
                places,
                replacements,
                against,
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
                let replacement = if for_write {
                    if *coercion != crate::types::Assignability::Exact {
                        return Err(MirError::Construction {
                            span,
                            message: "validated assignment cannot defer a contextual coercion"
                                .into(),
                        });
                    }
                    Some(self.borrow_operand(
                        replacement.ok_or_else(|| MirError::Construction {
                            span,
                            message: "write validation has no replacement value".into(),
                        })?,
                        span,
                    )?)
                } else {
                    None
                };
                places.push(place.clone());
                replacements.push(replacement);
            }
            LoweredAssignmentTarget::Discard => {}
            LoweredAssignmentTarget::Tuple { ty, items } => {
                for (index, item) in items.iter().enumerate() {
                    let projected = if for_write && assignment_target_contains_place(item) {
                        let value = replacement.ok_or_else(|| MirError::Construction {
                            span,
                            message: "tuple write validation has no replacement value".into(),
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
                HirIterationProtocol::Intrinsic { cursor } => self.lower_intrinsic_iterating_for(
                    span,
                    id,
                    IntrinsicIteration {
                        pattern: *pattern,
                        source: *source,
                        cursor: *cursor,
                    },
                    body,
                    block,
                ),
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
                loan_depth: self.active_loans.len(),
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
                loan_depth: self.active_loans.len(),
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
        iteration: IntrinsicIteration,
        body: HirExpressionId,
        block: MirBlockId,
    ) -> Result<Option<MirBlockId>, MirError> {
        let Some((block, source)) = self.lower_value(iteration.source, block)? else {
            return Ok(None);
        };
        let state = self.allocate_temporary(iteration.cursor, span, block)?;
        self.assign(
            block,
            span,
            self.local_place(state),
            MirRvalue {
                ty: iteration.cursor,
                kind: MirRvalueKind::IteratorState { source },
            },
        )?;
        let item = self.allocate_temporary(self.pattern_type(iteration.pattern)?, span, block)?;
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
            iteration.pattern,
            self.transfer_local(item, span)?,
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
        let loan_depth = self.active_loans.len();
        let state_loan = self.reserve_loan(
            header,
            span,
            crate::types::ParameterMode::Mut,
            self.local_place(state),
        )?;
        let arguments = vec![MirCallArgument {
            mode: crate::types::ParameterMode::Mut,
            target: crate::hir::HirCallArgumentTarget::Receiver,
            value: state_loan,
        }];
        self.consume_call_loans(&arguments, loan_depth, span)?;
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
                        arguments,
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
                value: self.borrow_local(next),
                cases: vec![(MirTag::OptionSome, body_start)],
                otherwise: exit,
            },
        )?;
        let item = self.project_operand(
            &self.transfer_local(next, span)?,
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
                loan_depth: self.active_loans.len(),
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
            self.transfer_place(place, expression_node.span())?,
        )))
    }

    fn lower_borrowed_value(
        &mut self,
        expression: HirExpressionId,
        block: MirBlockId,
    ) -> Result<Option<(MirBlockId, MirOperand)>, MirError> {
        let expression_node = self.expression(expression)?.clone();
        if expression_node.category() == HirValueCategory::Place {
            let Some((block, place)) = self.lower_place(expression, block)? else {
                return Ok(None);
            };
            return Ok(Some((block, self.borrow_place(place))));
        }
        let local = self.allocate_temporary(expression_node.ty(), expression_node.span(), block)?;
        let place = self.local_place(local);
        let Some(block) = self.lower_expression(expression, place.clone(), block)? else {
            return Ok(None);
        };
        Ok(Some((block, self.borrow_place(place))))
    }

    fn lower_loan_value(
        &mut self,
        expression: HirExpressionId,
        mode: crate::types::ParameterMode,
        block: MirBlockId,
    ) -> Result<Option<(MirBlockId, MirOperand)>, MirError> {
        let expression_node = self.expression(expression)?.clone();
        let span = expression_node.span();
        let place_like = expression_node.category() == HirValueCategory::Place
            || matches!(expression_node.kind(), HirExpressionKind::Slice { .. });
        let (block, place) = if place_like {
            let Some((block, place)) = self.lower_place(expression, block)? else {
                return Ok(None);
            };
            (block, place)
        } else {
            if mode != crate::types::ParameterMode::Ref {
                return Err(MirError::Construction {
                    span,
                    message: "an exclusive loan was not formed from a place".into(),
                });
            }
            let local = self.allocate_temporary(expression_node.ty(), span, block)?;
            let place = self.local_place(local);
            let Some(block) = self.lower_expression(expression, place.clone(), block)? else {
                return Ok(None);
            };
            (block, place)
        };
        if mode == crate::types::ParameterMode::Value {
            return Err(MirError::Construction {
                span,
                message: "a value parameter cannot reserve a loan".into(),
            });
        }
        let ty = place.ty;
        let loan = self.allocate_loan(span, MirLoanKind::CallLocal, mode, place)?;
        let block = self.validate_loan_place(loan, block, span)?;
        self.push_statement(block, span, MirStatementKind::ReserveLoan(loan))?;
        self.active_loans.push(loan);
        Ok(Some((
            block,
            MirOperand {
                ty,
                kind: MirOperandKind::Loan(loan),
            },
        )))
    }

    fn validate_loan_place(
        &mut self,
        loan: MirLoanId,
        block: MirBlockId,
        span: Span,
    ) -> Result<MirBlockId, MirError> {
        let place = &self.loans[loan.index() as usize].place;
        if !place.projections.iter().any(|projection| {
            matches!(
                projection.kind(),
                MirProjectionKind::Index { .. } | MirProjectionKind::Slice { .. }
            )
        }) {
            return Ok(block);
        }
        let target = self.allocate_block(MirBlockKind::Normal)?;
        self.terminate(
            block,
            span,
            MirTerminatorKind::ValidateLoan {
                loan,
                against: Vec::new(),
                target,
                unwind: self.unwind,
            },
        )?;
        Ok(target)
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
                value: self.borrow_operand(&option, span)?,
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
                            values: vec![self.transfer_local(option_local, span)?],
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
        self.release_loans_from(none, span, 0)?;
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
                value: self.borrow_operand(&result, span)?,
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
            self.transfer_local(local, span)?
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
        self.release_loans_from(err, span, 0)?;
        self.terminate(err, span, MirTerminatorKind::Return)?;
        Ok(Some(join))
    }

    fn lower_match(
        &mut self,
        scrutinee: HirExpressionId,
        mode: HirMatchMode,
        arms: &[crate::hir::HirMatchArm],
        span: Span,
        destination: MirPlace,
        block: MirBlockId,
    ) -> Result<Option<MirBlockId>, MirError> {
        let (block, scrutinee) = if mode == HirMatchMode::Observe {
            let Some((block, place)) = self.lower_place(scrutinee, block)? else {
                return Ok(None);
            };
            (block, self.borrow_place(place))
        } else {
            let Some((block, value)) = self.lower_value(scrutinee, block)? else {
                return Ok(None);
            };
            (block, self.borrow_operand(&value, span)?)
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
                self.bind_match_pattern(
                    arm.pattern(),
                    &scrutinee,
                    mode,
                    MatchBindingPhase::Guard,
                    matched,
                )?;
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
                self.bind_match_pattern(
                    arm.pattern(),
                    &scrutinee,
                    mode,
                    MatchBindingPhase::Body,
                    body,
                )?;
                body
            } else {
                self.bind_match_pattern(
                    arm.pattern(),
                    &scrutinee,
                    mode,
                    MatchBindingPhase::Body,
                    matched,
                )?;
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

    fn bind_match_pattern(
        &mut self,
        pattern: HirPatternId,
        source: &MirOperand,
        mode: HirMatchMode,
        phase: MatchBindingPhase,
        block: MirBlockId,
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
            HirPatternKind::Binding(source_local) => {
                if self.source_locals.contains_key(source_local) {
                    return Ok(());
                }
                let place = self.operand_place(source, span)?;
                let transfer = self.transfer_kind(place.clone(), span)?;
                let affine = matches!(transfer, MirOperandKind::Move(_));
                if phase == MatchBindingPhase::Guard && affine {
                    return Ok(());
                }
                if mode != HirMatchMode::Consume && affine {
                    return Err(MirError::Construction {
                        span,
                        message: "non-consuming match mode contains an affine value binding".into(),
                    });
                }
                let local = self.allocate_user_local(*source_local, pattern.ty(), span, block)?;
                self.assign_operand(
                    block,
                    span,
                    self.local_place(local),
                    MirOperand {
                        ty: place.ty,
                        kind: transfer,
                    },
                )?;
            }
            HirPatternKind::BorrowBinding(local) => {
                let place = self.operand_place(source, span)?;
                if let Some(previous) = self.borrow_aliases.get(local) {
                    let loan = previous
                        .source_loan
                        .and_then(|loan| self.loans.get(loan.index() as usize))
                        .ok_or_else(|| MirError::Construction {
                            span,
                            message: "borrow pattern local lost its inferred region".into(),
                        })?;
                    if loan.kind != MirLoanKind::Region || loan.place != place {
                        return Err(MirError::Construction {
                            span,
                            message: "borrow pattern local changed its projected source".into(),
                        });
                    }
                } else {
                    let borrowed = self.reserve_region_loan(block, span, place)?;
                    self.borrow_aliases.insert(*local, borrowed);
                }
            }
            HirPatternKind::Tuple(items) => {
                for (index, item) in items.iter().enumerate() {
                    let projected = self.project_operand(
                        source,
                        MirProjection {
                            ty: self.pattern_type(*item)?,
                            kind: MirProjectionKind::TupleField(index as u32),
                        },
                        span,
                    )?;
                    self.bind_match_pattern(*item, &projected, mode, phase, block)?;
                }
            }
            HirPatternKind::OptionSome(item) => {
                let projected = self.project_operand(
                    source,
                    MirProjection {
                        ty: self.pattern_type(*item)?,
                        kind: MirProjectionKind::OptionValue,
                    },
                    span,
                )?;
                self.bind_match_pattern(*item, &projected, mode, phase, block)?;
            }
            HirPatternKind::ResultOk(item) | HirPatternKind::ResultErr(item) => {
                let kind = if matches!(pattern.kind(), HirPatternKind::ResultOk(_)) {
                    MirProjectionKind::ResultOkValue
                } else {
                    MirProjectionKind::ResultErrValue
                };
                let projected = self.project_operand(
                    source,
                    MirProjection {
                        ty: self.pattern_type(*item)?,
                        kind,
                    },
                    span,
                )?;
                self.bind_match_pattern(*item, &projected, mode, phase, block)?;
            }
            HirPatternKind::Newtype { value, .. } => {
                let projected = self.project_operand(
                    source,
                    MirProjection {
                        ty: self.pattern_type(*value)?,
                        kind: MirProjectionKind::NewtypeValue,
                    },
                    span,
                )?;
                self.bind_match_pattern(*value, &projected, mode, phase, block)?;
            }
            HirPatternKind::Variant { variant, fields } => {
                let projections = self.variant_pattern_projections(*variant, fields, span)?;
                for (field, kind) in fields.iter().zip(projections) {
                    let projected = self.project_operand(
                        source,
                        MirProjection {
                            ty: self.pattern_type(*field)?,
                            kind,
                        },
                        span,
                    )?;
                    self.bind_match_pattern(*field, &projected, mode, phase, block)?;
                }
            }
            HirPatternKind::Record { fields, .. } => {
                for field in fields {
                    let projected = self.project_operand(
                        source,
                        MirProjection {
                            ty: self.pattern_type(field.pattern())?,
                            kind: MirProjectionKind::Field(field.member()),
                        },
                        span,
                    )?;
                    self.bind_match_pattern(field.pattern(), &projected, mode, phase, block)?;
                }
            }
            HirPatternKind::UnionMember { member, pattern } => {
                let projected = self.project_operand(
                    source,
                    MirProjection {
                        ty: self.pattern_type(*pattern)?,
                        kind: MirProjectionKind::UnionValue(*member),
                    },
                    span,
                )?;
                self.bind_match_pattern(*pattern, &projected, mode, phase, block)?;
            }
            HirPatternKind::Array { prefix, rest } => {
                for (index, pattern) in prefix.iter().enumerate() {
                    let projected = self.project_operand(
                        source,
                        MirProjection {
                            ty: self.pattern_type(*pattern)?,
                            kind: MirProjectionKind::ArrayPatternIndex(index as u32),
                        },
                        span,
                    )?;
                    self.bind_match_pattern(*pattern, &projected, mode, phase, block)?;
                }
                if let Some(rest) = rest {
                    let projected = self.project_operand(
                        source,
                        MirProjection {
                            ty: self.pattern_type(*rest)?,
                            kind: MirProjectionKind::ArrayPatternRest {
                                start: prefix.len() as u32,
                                suffix: 0,
                            },
                        },
                        span,
                    )?;
                    self.bind_match_pattern(*rest, &projected, mode, phase, block)?;
                }
            }
            HirPatternKind::Recovery
            | HirPatternKind::Wildcard
            | HirPatternKind::Literal(_)
            | HirPatternKind::OptionNone => {}
        }
        Ok(())
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
            HirPatternKind::Binding(_) | HirPatternKind::BorrowBinding(_) => {
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
                        value: self.borrow_operand(&value, span)?,
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
                    value: self.borrow_operand(&value, span)?,
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
                        value: self.borrow_operand(&value, span)?,
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
                        value: self.borrow_operand(&value, span)?,
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
                        value: self.borrow_operand(&value, span)?,
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
                        kind: MirRvalueKind::Length(self.borrow_operand(&value, span)?),
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
                let member_kind =
                    self.resolved
                        .member(*member)
                        .ok_or_else(|| MirError::Construction {
                            span,
                            message: format!("missing verified member#{}", member.index()),
                        })?;
                place.projections.push(MirProjection {
                    ty: expression.ty(),
                    kind: if member_kind.kind() == MemberKind::NewtypeValue {
                        MirProjectionKind::NewtypeValue
                    } else {
                        MirProjectionKind::Field(*member)
                    },
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
        let borrowed = matches!(operand.kind, MirOperandKind::Borrow(_));
        Ok(MirOperand {
            ty: place.ty,
            kind: if borrowed {
                MirOperandKind::Borrow(place)
            } else {
                self.transfer_kind(place, span)?
            },
        })
    }

    fn operand_place(&self, operand: &MirOperand, span: Span) -> Result<MirPlace, MirError> {
        match &operand.kind {
            MirOperandKind::Copy(place)
            | MirOperandKind::Move(place)
            | MirOperandKind::Borrow(place) => Ok(place.clone()),
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
        if let Some(place) = self.borrow_aliases.get(&local) {
            return Ok(place.clone());
        }
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

    fn reserve_loan(
        &mut self,
        block: MirBlockId,
        span: Span,
        mode: crate::types::ParameterMode,
        place: MirPlace,
    ) -> Result<MirOperand, MirError> {
        if mode == crate::types::ParameterMode::Value {
            return Err(MirError::Construction {
                span,
                message: "a value parameter cannot reserve a loan".into(),
            });
        }
        let ty = place.ty;
        let id = self.allocate_loan(span, MirLoanKind::CallLocal, mode, place)?;
        self.push_statement(block, span, MirStatementKind::ReserveLoan(id))?;
        self.active_loans.push(id);
        Ok(MirOperand {
            ty,
            kind: MirOperandKind::Loan(id),
        })
    }

    fn reserve_region_loan(
        &mut self,
        block: MirBlockId,
        span: Span,
        place: MirPlace,
    ) -> Result<MirPlace, MirError> {
        let id = self.allocate_loan(
            span,
            MirLoanKind::Region,
            crate::types::ParameterMode::Ref,
            place.clone(),
        )?;
        self.push_statement(block, span, MirStatementKind::ReserveLoan(id))?;
        let mut borrowed = place;
        borrowed.source_loan = Some(id);
        Ok(borrowed)
    }

    fn allocate_loan(
        &mut self,
        span: Span,
        kind: MirLoanKind,
        mode: crate::types::ParameterMode,
        place: MirPlace,
    ) -> Result<MirLoanId, MirError> {
        if self.loans.len() >= self.limits.max_locals_per_function as usize {
            return Err(MirError::NodeLimit {
                span,
                resource: "loan",
            });
        }
        let id = MirLoanId(
            u32::try_from(self.loans.len()).map_err(|_| MirError::NodeLimit {
                span,
                resource: "loan",
            })?,
        );
        self.loans.push(MirLoan { kind, mode, place });
        Ok(id)
    }

    fn release_loans_from(
        &mut self,
        block: MirBlockId,
        span: Span,
        depth: usize,
    ) -> Result<(), MirError> {
        if depth > self.active_loans.len() {
            return Err(MirError::Construction {
                span,
                message: "loan release depth exceeds the active reservation stack".into(),
            });
        }
        let released = self.active_loans[depth..]
            .iter()
            .rev()
            .copied()
            .collect::<Vec<_>>();
        for loan in released {
            self.push_statement(block, span, MirStatementKind::ReleaseLoan(loan))?;
        }
        Ok(())
    }

    fn consume_call_loans(
        &mut self,
        arguments: &[MirCallArgument],
        expected_depth: usize,
        span: Span,
    ) -> Result<(), MirError> {
        for loan in arguments
            .iter()
            .filter_map(|argument| match &argument.value.kind {
                MirOperandKind::Loan(loan) => Some(*loan),
                _ => None,
            })
        {
            let position = self
                .active_loans
                .iter()
                .rposition(|active| *active == loan)
                .ok_or_else(|| MirError::Construction {
                    span,
                    message: format!("call consumes inactive loan#{}", loan.index()),
                })?;
            self.active_loans.remove(position);
        }
        if self.active_loans.len() != expected_depth {
            return Err(MirError::Construction {
                span,
                message: "call did not consume exactly its own loan reservations".into(),
            });
        }
        Ok(())
    }

    fn infer_region_releases(&mut self) -> Result<(), MirError> {
        let regions = self
            .loans
            .iter()
            .enumerate()
            .filter_map(|(index, loan)| {
                (loan.kind == MirLoanKind::Region).then_some(MirLoanId(index as u32))
            })
            .collect::<BTreeSet<_>>();
        if regions.is_empty() {
            return Ok(());
        }

        let original_blocks = self.blocks.len();
        let successors = (0..original_blocks)
            .map(|index| {
                normal_successors(
                    &self.blocks[index]
                        .terminator
                        .as_ref()
                        .expect("region inference runs after MIR termination")
                        .kind,
                )
            })
            .collect::<Vec<_>>();
        let mut predecessors = vec![Vec::new(); original_blocks];
        for (source, targets) in successors.iter().enumerate() {
            for target in targets {
                predecessors[target.index() as usize].push(source);
            }
        }

        let mut live_in = vec![BTreeSet::<MirLoanId>::new(); original_blocks];
        let mut queue = VecDeque::from_iter(0..original_blocks);
        let mut queued = vec![true; original_blocks];
        while let Some(index) = queue.pop_front() {
            queued[index] = false;
            let mut live = successors[index]
                .iter()
                .flat_map(|target| live_in[target.index() as usize].iter().copied())
                .collect::<BTreeSet<_>>();
            let block = &self.blocks[index];
            collect_terminator_region_uses(
                &block
                    .terminator
                    .as_ref()
                    .expect("region inference runs after MIR termination")
                    .kind,
                &self.loans,
                &mut live,
            );
            for statement in block.statements.iter().rev() {
                transfer_region_liveness(statement, &self.loans, &regions, &mut live);
            }
            if live != live_in[index] {
                live_in[index] = live;
                for predecessor in &predecessors[index] {
                    if !queued[*predecessor] {
                        queued[*predecessor] = true;
                        queue.push_back(*predecessor);
                    }
                }
            }
        }

        for (index, block_successors) in successors.iter().enumerate().take(original_blocks) {
            let (terminator_span, terminator_kind) = {
                let terminator = self.blocks[index]
                    .terminator
                    .as_ref()
                    .expect("region inference runs after MIR termination");
                (terminator.span, terminator.kind.clone())
            };
            let mut live_after_terminator = block_successors
                .iter()
                .flat_map(|target| live_in[target.index() as usize].iter().copied())
                .collect::<BTreeSet<_>>();
            let mut terminator_uses = BTreeSet::new();
            collect_terminator_region_uses(&terminator_kind, &self.loans, &mut terminator_uses);
            let mut live_before_terminator = live_after_terminator.clone();
            live_before_terminator.extend(terminator_uses);

            let statements = &self.blocks[index].statements;
            let mut live_after_statements = vec![BTreeSet::new(); statements.len()];
            let mut live = std::mem::take(&mut live_after_terminator);
            live.extend(collect_terminator_region_use_set(
                &terminator_kind,
                &self.loans,
            ));
            for (statement_index, statement) in statements.iter().enumerate().rev() {
                live_after_statements[statement_index] = live.clone();
                transfer_region_liveness(statement, &self.loans, &regions, &mut live);
            }

            let old = std::mem::take(&mut self.blocks[index].statements);
            let mut rebuilt = Vec::with_capacity(old.len());
            for (statement_index, statement) in old.into_iter().enumerate() {
                let mut uses = BTreeSet::new();
                collect_statement_region_uses(&statement.kind, &self.loans, &mut uses);
                let mut releases = uses
                    .difference(&live_after_statements[statement_index])
                    .copied()
                    .collect::<BTreeSet<_>>();
                if let MirStatementKind::ReserveLoan(id) = &statement.kind
                    && regions.contains(id)
                    && !live_after_statements[statement_index].contains(id)
                {
                    releases.insert(*id);
                }
                let span = statement.span;
                rebuilt.push(statement);
                for loan in releases.iter().rev().copied() {
                    rebuilt.push(MirStatement {
                        span,
                        kind: MirStatementKind::ReleaseLoan(loan),
                    });
                    self.statement_count =
                        self.statement_count
                            .checked_add(1)
                            .ok_or(MirError::NodeLimit {
                                span,
                                resource: "statement",
                            })?;
                    if self.statement_count > self.limits.max_statements_per_function {
                        return Err(MirError::NodeLimit {
                            span,
                            resource: "statement",
                        });
                    }
                }
            }
            self.blocks[index].statements = rebuilt;

            let rewritten = self.rewrite_region_release_edges(
                terminator_kind,
                terminator_span,
                &live_before_terminator,
                &live_in,
            )?;
            self.blocks[index]
                .terminator
                .as_mut()
                .expect("region inference retains every terminator")
                .kind = rewritten;
        }
        Ok(())
    }

    fn rewrite_region_release_edges(
        &mut self,
        kind: MirTerminatorKind,
        span: Span,
        live: &BTreeSet<MirLoanId>,
        live_in: &[BTreeSet<MirLoanId>],
    ) -> Result<MirTerminatorKind, MirError> {
        let target = |builder: &mut Self, target: MirBlockId| {
            let releases = live
                .difference(&live_in[target.index() as usize])
                .copied()
                .collect::<BTreeSet<_>>();
            builder.region_release_edge(target, &releases, span)
        };
        Ok(match kind {
            MirTerminatorKind::Goto { target: next } => MirTerminatorKind::Goto {
                target: target(self, next)?,
            },
            MirTerminatorKind::SwitchBool {
                condition,
                if_true,
                if_false,
            } => MirTerminatorKind::SwitchBool {
                condition,
                if_true: target(self, if_true)?,
                if_false: target(self, if_false)?,
            },
            MirTerminatorKind::SwitchTag {
                value,
                cases,
                otherwise,
            } => MirTerminatorKind::SwitchTag {
                value,
                cases: cases
                    .into_iter()
                    .map(|(tag, next)| Ok((tag, target(self, next)?)))
                    .collect::<Result<_, MirError>>()?,
                otherwise: target(self, otherwise)?,
            },
            MirTerminatorKind::Invoke {
                operation,
                destination,
                target: next,
                unwind,
            } => MirTerminatorKind::Invoke {
                operation,
                destination,
                target: next.map(|next| target(self, next)).transpose()?,
                unwind,
            },
            MirTerminatorKind::IteratorNext {
                state,
                destination,
                has_value,
                exhausted,
                unwind,
            } => MirTerminatorKind::IteratorNext {
                state,
                destination,
                has_value: target(self, has_value)?,
                exhausted: target(self, exhausted)?,
                unwind,
            },
            MirTerminatorKind::ValidatePlaces {
                places,
                replacements,
                against,
                for_write,
                target: next,
                unwind,
            } => MirTerminatorKind::ValidatePlaces {
                places,
                replacements,
                against,
                for_write,
                target: target(self, next)?,
                unwind,
            },
            MirTerminatorKind::ValidateLoan {
                loan,
                against,
                target: next,
                unwind,
            } => MirTerminatorKind::ValidateLoan {
                loan,
                against,
                target: target(self, next)?,
                unwind,
            },
            MirTerminatorKind::Return => {
                if !live.is_empty() {
                    return Err(MirError::Construction {
                        span,
                        message: "a region loan reached return without a last-use release".into(),
                    });
                }
                MirTerminatorKind::Return
            }
            MirTerminatorKind::ResumePanic => MirTerminatorKind::ResumePanic,
            MirTerminatorKind::Unreachable => MirTerminatorKind::Unreachable,
        })
    }

    fn region_release_edge(
        &mut self,
        target: MirBlockId,
        releases: &BTreeSet<MirLoanId>,
        span: Span,
    ) -> Result<MirBlockId, MirError> {
        if releases.is_empty() {
            return Ok(target);
        }
        let bridge = self.allocate_block(MirBlockKind::Normal)?;
        for loan in releases.iter().rev().copied() {
            self.push_statement(bridge, span, MirStatementKind::ReleaseLoan(loan))?;
        }
        self.terminate(bridge, span, MirTerminatorKind::Goto { target })?;
        Ok(bridge)
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
            source_loan: None,
        }
    }

    fn copy_local(&self, local: MirLocalId) -> MirOperand {
        let place = self.local_place(local);
        MirOperand {
            ty: place.ty,
            kind: MirOperandKind::Copy(place),
        }
    }

    fn borrow_local(&self, local: MirLocalId) -> MirOperand {
        self.borrow_place(self.local_place(local))
    }

    fn borrow_place(&self, place: MirPlace) -> MirOperand {
        MirOperand {
            ty: place.ty,
            kind: MirOperandKind::Borrow(place),
        }
    }

    fn borrow_operand(&self, operand: &MirOperand, span: Span) -> Result<MirOperand, MirError> {
        Ok(self.borrow_place(self.operand_place(operand, span)?))
    }

    fn transfer_local(&self, local: MirLocalId, span: Span) -> Result<MirOperand, MirError> {
        self.transfer_place(self.local_place(local), span)
    }

    fn transfer_place(&self, place: MirPlace, span: Span) -> Result<MirOperand, MirError> {
        Ok(MirOperand {
            ty: place.ty,
            kind: self.transfer_kind(place, span)?,
        })
    }

    fn transfer_kind(&self, place: MirPlace, span: Span) -> Result<MirOperandKind, MirError> {
        let status = if let Some(status) = self.copy_statuses.borrow().get(&place.ty).copied() {
            status
        } else {
            let status = self
                .capability_analysis
                .status(
                    self.hir,
                    place.ty,
                    HirCapability::Copy,
                    &self.capability_assumptions,
                )
                .map_err(|error| MirError::Construction {
                    span,
                    message: format!("cannot classify ownership transfer: {error}"),
                })?;
            self.copy_statuses.borrow_mut().insert(place.ty, status);
            status
        };
        match status {
            HirCapabilityStatus::Satisfied => Ok(MirOperandKind::Copy(place)),
            HirCapabilityStatus::Unsatisfied => Ok(MirOperandKind::Move(place)),
            HirCapabilityStatus::Deferred => Err(MirError::Construction {
                span,
                message: format!(
                    "ownership transfer for `{}` has an unresolved Copy capability",
                    self.hir
                        .interner()
                        .canonical(place.ty)
                        .unwrap_or_else(|_| place.ty.to_string())
                ),
            }),
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

fn populate_runtime_loan_checks(
    hir: &HirProgram,
    function: &mut MirFunction,
    limits: MirLoweringLimits,
) -> Result<(), MirError> {
    let static_integers = super::regions::static_integer_locals(hir, function);
    let mut incoming = vec![None::<BTreeSet<MirLoanId>>; function.blocks.len()];
    incoming[function.entry.index() as usize] = Some(BTreeSet::new());
    let mut queue = VecDeque::from([function.entry]);
    let mut queued = vec![false; function.blocks.len()];
    queued[function.entry.index() as usize] = true;
    let mut checks = vec![None::<Vec<MirLoanId>>; function.blocks.len()];
    let mut place_checks = vec![None::<Vec<Vec<MirLoanId>>>; function.blocks.len()];
    let mut operation_checks = vec![None::<Vec<MirLoanId>>; function.blocks.len()];
    let mut steps = 0_u64;

    while let Some(block_id) = queue.pop_front() {
        queued[block_id.index() as usize] = false;
        consume_runtime_loan_analysis_step(&mut steps, limits)?;
        let index = block_id.index() as usize;
        let mut active = incoming[index]
            .clone()
            .expect("queued MIR blocks have an incoming loan state");
        let block = &function.blocks[index];
        for statement in &block.statements {
            consume_runtime_loan_analysis_step(&mut steps, limits)?;
            match statement.kind {
                MirStatementKind::ReserveLoan(loan) => {
                    if !active.insert(loan) {
                        return Err(MirError::Construction {
                            span: statement.span,
                            message: format!(
                                "generated MIR reserves already-active loan#{}",
                                loan.index()
                            ),
                        });
                    }
                }
                MirStatementKind::ReleaseLoan(loan) => {
                    if !active.remove(&loan) {
                        return Err(MirError::Construction {
                            span: statement.span,
                            message: format!(
                                "generated MIR releases inactive loan#{}",
                                loan.index()
                            ),
                        });
                    }
                }
                MirStatementKind::StorageLive(_)
                | MirStatementKind::StorageDead(_)
                | MirStatementKind::Assign { .. } => {}
            }
        }

        let span = block.terminator.span;
        let mut propagate = |target: MirBlockId,
                             edge_state: BTreeSet<MirLoanId>|
         -> Result<(), MirError> {
            let target_index = target.index() as usize;
            match &incoming[target_index] {
                Some(previous) if previous != &edge_state => {
                    return Err(MirError::Construction {
                        span,
                        message: format!(
                            "generated MIR predecessors disagree about active loans at block#{}",
                            target.index()
                        ),
                    });
                }
                Some(_) => {}
                None => {
                    incoming[target_index] = Some(edge_state);
                    if !queued[target_index] {
                        queued[target_index] = true;
                        queue.push_back(target);
                    }
                }
            }
            Ok(())
        };

        match &block.terminator.kind {
            MirTerminatorKind::Goto { target } => propagate(*target, active)?,
            MirTerminatorKind::SwitchBool {
                if_true, if_false, ..
            } => {
                propagate(*if_true, active.clone())?;
                propagate(*if_false, active)?;
            }
            MirTerminatorKind::SwitchTag {
                cases, otherwise, ..
            } => {
                for (_, target) in cases {
                    propagate(*target, active.clone())?;
                }
                propagate(*otherwise, active)?;
            }
            MirTerminatorKind::Invoke {
                operation,
                target,
                unwind,
                ..
            } => {
                if let Some(place) = mir_operation_access_place(operation, span)? {
                    let mut against = Vec::new();
                    for active_id in active.iter().copied() {
                        consume_runtime_loan_analysis_step(&mut steps, limits)?;
                        let existing = &function.loans[active_id.index() as usize];
                        if existing.mode == crate::types::ParameterMode::Ref {
                            continue;
                        }
                        match super::regions::loan_place_relation(
                            &place,
                            &existing.place,
                            &static_integers,
                        ) {
                            StaticRegionRelation::Disjoint => {}
                            StaticRegionRelation::Runtime => against.push(active_id),
                            StaticRegionRelation::Overlap => {
                                return Err(MirError::Construction {
                                    span,
                                    message: format!(
                                        "indexed read statically overlaps active loan#{}",
                                        active_id.index()
                                    ),
                                });
                            }
                        }
                    }
                    operation_checks[index] = Some(against);
                }
                consume_operation_loans(operation, &mut active, span)?;
                if let Some(target) = target {
                    propagate(*target, active)?;
                }
                propagate(*unwind, BTreeSet::new())?;
            }
            MirTerminatorKind::IteratorNext {
                has_value,
                exhausted,
                unwind,
                ..
            } => {
                propagate(*has_value, active.clone())?;
                propagate(*exhausted, active)?;
                propagate(*unwind, BTreeSet::new())?;
            }
            MirTerminatorKind::ValidatePlaces {
                places,
                for_write,
                target,
                unwind,
                ..
            } => {
                let mut validations = Vec::with_capacity(places.len());
                for place in places {
                    let mut against = Vec::new();
                    for active_id in active.iter().copied() {
                        consume_runtime_loan_analysis_step(&mut steps, limits)?;
                        let existing = &function.loans[active_id.index() as usize];
                        if !*for_write && existing.mode == crate::types::ParameterMode::Ref {
                            continue;
                        }
                        match super::regions::loan_place_relation(
                            place,
                            &existing.place,
                            &static_integers,
                        ) {
                            StaticRegionRelation::Disjoint => {}
                            StaticRegionRelation::Runtime => against.push(active_id),
                            StaticRegionRelation::Overlap => {
                                return Err(MirError::Construction {
                                    span,
                                    message: format!(
                                        "place validation statically overlaps active loan#{}",
                                        active_id.index()
                                    ),
                                });
                            }
                        }
                    }
                    validations.push(against);
                }
                place_checks[index] = Some(validations);
                propagate(*target, active)?;
                propagate(*unwind, BTreeSet::new())?;
            }
            MirTerminatorKind::ValidateLoan {
                loan,
                target,
                unwind,
                ..
            } => {
                let candidate = function.loans.get(loan.index() as usize).ok_or_else(|| {
                    MirError::Construction {
                        span,
                        message: format!("generated MIR validates unknown loan#{}", loan.index()),
                    }
                })?;
                let mut against = Vec::new();
                for active_id in active.iter().copied() {
                    consume_runtime_loan_analysis_step(&mut steps, limits)?;
                    let existing = &function.loans[active_id.index() as usize];
                    if candidate.mode == crate::types::ParameterMode::Ref
                        && existing.mode == crate::types::ParameterMode::Ref
                    {
                        continue;
                    }
                    match super::regions::loan_place_relation(
                        &candidate.place,
                        &existing.place,
                        &static_integers,
                    ) {
                        StaticRegionRelation::Disjoint => {}
                        StaticRegionRelation::Runtime => against.push(active_id),
                        StaticRegionRelation::Overlap => {
                            return Err(MirError::Construction {
                                span,
                                message: format!(
                                    "loan#{} statically overlaps incompatible active loan#{}",
                                    loan.index(),
                                    active_id.index()
                                ),
                            });
                        }
                    }
                }
                checks[index] = Some(against);
                propagate(*target, active)?;
                propagate(*unwind, BTreeSet::new())?;
            }
            MirTerminatorKind::Return
            | MirTerminatorKind::ResumePanic
            | MirTerminatorKind::Unreachable => {}
        }
    }

    for (index, against) in checks.into_iter().enumerate() {
        let Some(against) = against else {
            continue;
        };
        let MirTerminatorKind::ValidateLoan {
            against: stored, ..
        } = &mut function.blocks[index].terminator.kind
        else {
            unreachable!("runtime-loan checks retain their terminator kind");
        };
        *stored = against;
    }
    for (index, against) in place_checks.into_iter().enumerate() {
        let Some(against) = against else {
            continue;
        };
        let MirTerminatorKind::ValidatePlaces {
            against: stored, ..
        } = &mut function.blocks[index].terminator.kind
        else {
            unreachable!("place checks retain their terminator kind");
        };
        *stored = against;
    }
    for (index, against) in operation_checks.into_iter().enumerate() {
        let Some(against) = against else {
            continue;
        };
        let MirTerminatorKind::Invoke { operation, .. } =
            &mut function.blocks[index].terminator.kind
        else {
            unreachable!("operation checks retain their invoke terminator");
        };
        match &mut operation.kind {
            MirOperationKind::Index {
                against: stored, ..
            }
            | MirOperationKind::Slice {
                against: stored, ..
            } => *stored = against,
            _ => unreachable!("only indexed operations receive access checks"),
        }
    }
    Ok(())
}

fn consume_runtime_loan_analysis_step(
    steps: &mut u64,
    limits: MirLoweringLimits,
) -> Result<(), MirError> {
    *steps = steps.saturating_add(1);
    if *steps > limits.max_verification_steps {
        return Err(MirError::VerificationLimit {
            resource: "runtime-loan analysis",
        });
    }
    Ok(())
}

fn mir_operation_access_place(
    operation: &MirOperation,
    span: Span,
) -> Result<Option<MirPlace>, MirError> {
    let (base, projection) = match &operation.kind {
        MirOperationKind::Index {
            base,
            index,
            access,
            ..
        } => (
            base,
            MirProjectionKind::Index {
                index: materialized_operand_local(index, span)?,
                access: *access,
            },
        ),
        MirOperationKind::Slice { base, bounds, .. } => (
            base,
            MirProjectionKind::Slice {
                start: bounds
                    .start
                    .as_ref()
                    .map(|operand| materialized_operand_local(operand, span))
                    .transpose()?,
                end: bounds
                    .end
                    .as_ref()
                    .map(|operand| materialized_operand_local(operand, span))
                    .transpose()?,
                step: bounds
                    .step
                    .as_ref()
                    .map(|operand| materialized_operand_local(operand, span))
                    .transpose()?,
            },
        ),
        _ => return Ok(None),
    };
    let MirOperandKind::Borrow(base) = &base.kind else {
        return Err(MirError::Construction {
            span,
            message: "generated indexed operation has no borrowed base place".into(),
        });
    };
    let mut place = base.clone();
    place.ty = operation.ty;
    place.projections.push(MirProjection {
        ty: operation.ty,
        kind: projection,
    });
    Ok(Some(place))
}

fn materialized_operand_local(operand: &MirOperand, span: Span) -> Result<MirLocalId, MirError> {
    let place = match &operand.kind {
        MirOperandKind::Copy(place)
        | MirOperandKind::Move(place)
        | MirOperandKind::Borrow(place)
            if place.projections.is_empty() && place.source_loan.is_none() =>
        {
            place
        }
        _ => {
            return Err(MirError::Construction {
                span,
                message: "generated index or slice input is not a materialized local".into(),
            });
        }
    };
    Ok(place.local)
}

fn consume_operation_loans(
    operation: &MirOperation,
    active: &mut BTreeSet<MirLoanId>,
    span: Span,
) -> Result<(), MirError> {
    let MirOperationKind::Call { arguments, .. } = &operation.kind else {
        return Ok(());
    };
    for loan in arguments
        .iter()
        .filter_map(|argument| match argument.value.kind {
            MirOperandKind::Loan(loan) => Some(loan),
            _ => None,
        })
    {
        if !active.remove(&loan) {
            return Err(MirError::Construction {
                span,
                message: format!("generated MIR call consumes inactive loan#{}", loan.index()),
            });
        }
    }
    Ok(())
}

fn normal_successors(terminator: &MirTerminatorKind) -> Vec<MirBlockId> {
    match terminator {
        MirTerminatorKind::Goto { target } => vec![*target],
        MirTerminatorKind::SwitchBool {
            if_true, if_false, ..
        } => vec![*if_true, *if_false],
        MirTerminatorKind::SwitchTag {
            cases, otherwise, ..
        } => cases
            .iter()
            .map(|(_, target)| *target)
            .chain([*otherwise])
            .collect(),
        MirTerminatorKind::Invoke { target, .. } => target.iter().copied().collect(),
        MirTerminatorKind::IteratorNext {
            has_value,
            exhausted,
            ..
        } => vec![*has_value, *exhausted],
        MirTerminatorKind::ValidatePlaces { target, .. }
        | MirTerminatorKind::ValidateLoan { target, .. } => vec![*target],
        MirTerminatorKind::Return
        | MirTerminatorKind::ResumePanic
        | MirTerminatorKind::Unreachable => Vec::new(),
    }
}

fn transfer_region_liveness(
    statement: &MirStatement,
    loans: &[MirLoan],
    regions: &BTreeSet<MirLoanId>,
    live: &mut BTreeSet<MirLoanId>,
) {
    if let MirStatementKind::ReserveLoan(id) = statement.kind
        && regions.contains(&id)
    {
        live.remove(&id);
    }
    collect_statement_region_uses(&statement.kind, loans, live);
}

fn collect_statement_region_uses(
    statement: &MirStatementKind,
    loans: &[MirLoan],
    output: &mut BTreeSet<MirLoanId>,
) {
    match statement {
        MirStatementKind::StorageLive(_) | MirStatementKind::StorageDead(_) => {}
        MirStatementKind::ReserveLoan(id) => {
            if let Some(loan) = loans.get(id.index() as usize) {
                collect_place_region_uses(&loan.place, loans, output);
            }
        }
        MirStatementKind::ReleaseLoan(id) => collect_loan_region_uses(*id, loans, output),
        MirStatementKind::Assign { destination, value } => {
            collect_rvalue_region_uses(value, loans, output);
            collect_place_region_uses(destination, loans, output);
        }
    }
}

fn collect_terminator_region_use_set(
    terminator: &MirTerminatorKind,
    loans: &[MirLoan],
) -> BTreeSet<MirLoanId> {
    let mut output = BTreeSet::new();
    collect_terminator_region_uses(terminator, loans, &mut output);
    output
}

fn collect_terminator_region_uses(
    terminator: &MirTerminatorKind,
    loans: &[MirLoan],
    output: &mut BTreeSet<MirLoanId>,
) {
    match terminator {
        MirTerminatorKind::Goto { .. }
        | MirTerminatorKind::Return
        | MirTerminatorKind::ResumePanic
        | MirTerminatorKind::Unreachable => {}
        MirTerminatorKind::SwitchBool { condition, .. } => {
            collect_operand_region_uses(condition, loans, output);
        }
        MirTerminatorKind::SwitchTag { value, .. } => {
            collect_operand_region_uses(value, loans, output);
        }
        MirTerminatorKind::Invoke {
            operation,
            destination,
            ..
        } => {
            collect_operation_region_uses(operation, loans, output);
            if let Some(destination) = destination {
                collect_place_region_uses(destination, loans, output);
            }
        }
        MirTerminatorKind::IteratorNext {
            state, destination, ..
        } => {
            collect_place_region_uses(state, loans, output);
            collect_place_region_uses(destination, loans, output);
        }
        MirTerminatorKind::ValidatePlaces {
            places,
            replacements,
            ..
        } => {
            for place in places {
                collect_place_region_uses(place, loans, output);
            }
            for replacement in replacements.iter().flatten() {
                collect_operand_region_uses(replacement, loans, output);
            }
        }
        MirTerminatorKind::ValidateLoan { loan, against, .. } => {
            collect_loan_region_uses(*loan, loans, output);
            for loan in against {
                collect_loan_region_uses(*loan, loans, output);
            }
        }
    }
}

fn collect_rvalue_region_uses(
    value: &MirRvalue,
    loans: &[MirLoan],
    output: &mut BTreeSet<MirLoanId>,
) {
    match &value.kind {
        MirRvalueKind::Use(operand)
        | MirRvalueKind::Prefix { operand, .. }
        | MirRvalueKind::Coerce { value: operand, .. }
        | MirRvalueKind::NumericConversion { value: operand, .. }
        | MirRvalueKind::Length(operand)
        | MirRvalueKind::IteratorState { source: operand } => {
            collect_operand_region_uses(operand, loans, output);
        }
        MirRvalueKind::Binary { left, right, .. }
        | MirRvalueKind::Range {
            start: left,
            end: right,
            ..
        }
        | MirRvalueKind::Contains {
            item: left,
            container: right,
            ..
        } => {
            collect_operand_region_uses(left, loans, output);
            collect_operand_region_uses(right, loans, output);
        }
        MirRvalueKind::Aggregate { values, .. } => {
            for operand in values {
                collect_operand_region_uses(operand, loans, output);
            }
        }
        MirRvalueKind::RecordUpdate { base, fields } => {
            collect_operand_region_uses(base, loans, output);
            for (_, operand) in fields {
                collect_operand_region_uses(operand, loans, output);
            }
        }
    }
}

fn collect_operation_region_uses(
    operation: &MirOperation,
    loans: &[MirLoan],
    output: &mut BTreeSet<MirLoanId>,
) {
    match &operation.kind {
        MirOperationKind::CheckedPrefix { operand, .. }
        | MirOperationKind::ExplicitPanic { message: operand } => {
            collect_operand_region_uses(operand, loans, output);
        }
        MirOperationKind::CheckedBinary { left, right, .. } => {
            collect_operand_region_uses(left, loans, output);
            collect_operand_region_uses(right, loans, output);
        }
        MirOperationKind::BuildMap { entries, .. } => {
            for (key, value) in entries {
                collect_operand_region_uses(key, loans, output);
                collect_operand_region_uses(value, loans, output);
            }
        }
        MirOperationKind::Index { base, index, .. } => {
            collect_operand_region_uses(base, loans, output);
            collect_operand_region_uses(index, loans, output);
        }
        MirOperationKind::Slice { base, bounds, .. } => {
            collect_operand_region_uses(base, loans, output);
            for bound in bounds.start.iter().chain(&bounds.end).chain(&bounds.step) {
                collect_operand_region_uses(bound, loans, output);
            }
        }
        MirOperationKind::Call {
            callee, arguments, ..
        } => {
            collect_operand_region_uses(callee, loans, output);
            for argument in arguments {
                collect_operand_region_uses(&argument.value, loans, output);
            }
        }
        MirOperationKind::Assert {
            condition,
            message_parts,
            ..
        } => {
            collect_operand_region_uses(condition, loans, output);
            for part in message_parts {
                collect_operand_region_uses(&part.value, loans, output);
            }
        }
        MirOperationKind::BootstrapHostCall { arguments, .. } => {
            for argument in arguments {
                collect_operand_region_uses(argument, loans, output);
            }
        }
    }
}

fn collect_operand_region_uses(
    operand: &MirOperand,
    loans: &[MirLoan],
    output: &mut BTreeSet<MirLoanId>,
) {
    match &operand.kind {
        MirOperandKind::Copy(place)
        | MirOperandKind::Move(place)
        | MirOperandKind::Borrow(place) => collect_place_region_uses(place, loans, output),
        MirOperandKind::Loan(loan) => collect_loan_region_uses(*loan, loans, output),
        MirOperandKind::Constant(_)
        | MirOperandKind::Function { .. }
        | MirOperandKind::PreludeTraitFunction { .. } => {}
    }
}

fn collect_place_region_uses(
    place: &MirPlace,
    loans: &[MirLoan],
    output: &mut BTreeSet<MirLoanId>,
) {
    if let Some(loan) = place.source_loan {
        collect_loan_region_uses(loan, loans, output);
    }
}

fn collect_loan_region_uses(
    mut loan: MirLoanId,
    loans: &[MirLoan],
    output: &mut BTreeSet<MirLoanId>,
) {
    let mut visited = BTreeSet::new();
    while visited.insert(loan) {
        let Some(metadata) = loans.get(loan.index() as usize) else {
            return;
        };
        if metadata.kind == MirLoanKind::Region {
            output.insert(loan);
        }
        let Some(source) = metadata.place.source_loan else {
            return;
        };
        loan = source;
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
    fn affine_transfers_are_moves_and_copy_bounded_transfers_remain_copies() {
        let source = "fn copied[T: Copy](input: T): T {\n\
                          let local = input\n\
                          local\n\
                      }\n\
                      fn moved[T: Discard](input: T): T {\n\
                          let local = input\n\
                          local\n\
                      }\n\
                      fn unbounded[T](input: T): T { input }\n\
                      fn accept[T: Discard](input: T): T { input }\n\
                      fn forward[T: Discard](input: T): T { accept(input) }\n\
                      fn pack[T: Discard](input: T): Array[T] { [input] }\n\
                      fn drain[T: Discard](values: Array[T]) {\n\
                          for value in values {\n\
                              _ = value\n\
                          }\n\
                      }\n\
                      fn once[F: Discard + CallOnce[fn(Int): Int]](operation: F): Int {\n\
                          operation(42)\n\
                      }\n\
                      fn replace_slice[T: Discard](\n\
                          target: var Array[T],\n\
                          replacement: Array[T],\n\
                      ) {\n\
                          target[:] = replacement\n\
                      }\n";
        let (resolved, hir) = checked(source);
        let mut mir = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();

        let local_accesses = |function: &MirFunction| {
            function
                .blocks()
                .flat_map(MirBasicBlock::statements)
                .filter_map(|statement| match statement.kind() {
                    MirStatementKind::Assign {
                        value:
                            MirRvalue {
                                kind: MirRvalueKind::Use(operand),
                                ..
                            },
                        ..
                    } => match operand.kind() {
                        MirOperandKind::Copy(place) => Some((true, place.local())),
                        MirOperandKind::Move(place) => Some((false, place.local())),
                        _ => None,
                    },
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        let copied = mir.function(function_id(&resolved, "copied")).unwrap();
        let copied_accesses = local_accesses(copied);
        assert!(copied_accesses.iter().all(|(copies, _)| *copies));
        assert!(copied_accesses.iter().any(|(_, local)| {
            matches!(
                copied.local(*local).unwrap().kind(),
                MirLocalKind::Parameter { .. }
            )
        }));
        assert!(copied_accesses.iter().any(|(_, local)| {
            matches!(copied.local(*local).unwrap().kind(), MirLocalKind::User(_))
        }));

        let moved = mir.function(function_id(&resolved, "moved")).unwrap();
        let moved_accesses = local_accesses(moved);
        assert!(moved_accesses.iter().all(|(copies, _)| !copies));
        assert!(moved_accesses.iter().any(|(_, local)| {
            matches!(
                moved.local(*local).unwrap().kind(),
                MirLocalKind::Parameter { .. }
            )
        }));
        assert!(moved_accesses.iter().any(|(_, local)| {
            matches!(moved.local(*local).unwrap().kind(), MirLocalKind::User(_))
        }));

        let unbounded = mir.function(function_id(&resolved, "unbounded")).unwrap();
        assert!(local_accesses(unbounded).iter().all(|(copies, _)| !copies));

        let forward = mir.function(function_id(&resolved, "forward")).unwrap();
        assert!(forward.blocks().any(|block| matches!(
            block.terminator().kind(),
            MirTerminatorKind::Invoke {
                operation: MirOperation {
                    kind: MirOperationKind::Call { arguments, .. },
                    ..
                },
                ..
            } if matches!(arguments[0].value().kind(), MirOperandKind::Move(_))
        )));

        let pack = mir.function(function_id(&resolved, "pack")).unwrap();
        assert!(
            pack.blocks()
                .flat_map(MirBasicBlock::statements)
                .any(|statement| matches!(
                    statement.kind(),
                    MirStatementKind::Assign {
                        value: MirRvalue {
                            kind: MirRvalueKind::Aggregate { values, .. },
                            ..
                        },
                        ..
                    } if matches!(values[0].kind(), MirOperandKind::Move(_))
                ))
        );

        let drain = mir.function(function_id(&resolved, "drain")).unwrap();
        assert!(
            drain
                .blocks()
                .flat_map(MirBasicBlock::statements)
                .any(|statement| matches!(
                    statement.kind(),
                    MirStatementKind::Assign {
                        value: MirRvalue {
                            kind: MirRvalueKind::IteratorState { source },
                            ..
                        },
                        ..
                    } if matches!(source.kind(), MirOperandKind::Move(_))
                ))
        );

        let once = mir.function(function_id(&resolved, "once")).unwrap();
        assert!(once.blocks().any(|block| matches!(
            block.terminator().kind(),
            MirTerminatorKind::Invoke {
                operation: MirOperation {
                    kind: MirOperationKind::Call {
                        callee,
                        protocol: HirCallProtocol::CallOnce,
                        ..
                    },
                    ..
                },
                ..
            } if matches!(callee.kind(), MirOperandKind::Move(_))
        )));

        let replace_slice = mir
            .function(function_id(&resolved, "replace_slice"))
            .unwrap();
        assert!(replace_slice.blocks().any(|block| matches!(
            block.terminator().kind(),
            MirTerminatorKind::ValidatePlaces {
                replacements,
                for_write: true,
                ..
            } if matches!(
                replacements.as_slice(),
                [Some(MirOperand {
                    kind: MirOperandKind::Borrow(_),
                    ..
                })]
            )
        )));

        let moved = mir
            .functions
            .get_mut(&MirFunctionId::Callable(function_id(&resolved, "moved")))
            .unwrap();
        let operand = moved
            .blocks
            .iter_mut()
            .flat_map(|block| &mut block.statements)
            .find_map(|statement| match &mut statement.kind {
                MirStatementKind::Assign {
                    value:
                        MirRvalue {
                            kind: MirRvalueKind::Use(operand),
                            ..
                        },
                    ..
                } if matches!(operand.kind, MirOperandKind::Move(_)) => Some(operand),
                _ => None,
            })
            .unwrap();
        let MirOperandKind::Move(place) = &operand.kind else {
            unreachable!()
        };
        operand.kind = MirOperandKind::Copy(place.clone());
        let error = verify_mir(&resolved, &hir, &mir).unwrap_err();
        assert!(
            error
                .message()
                .contains("contextual Copy status Unsatisfied")
        );
    }

    #[test]
    fn mir_verifier_rejects_repeated_and_joined_affine_moves() {
        let (resolved, hir) = checked("fn consume[T: Discard](input: T) {\n    _ = input\n}\n");
        let mut mir = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        let function = mir
            .functions
            .get_mut(&MirFunctionId::Callable(function_id(&resolved, "consume")))
            .unwrap();
        let (block, index) = function
            .blocks
            .iter()
            .enumerate()
            .find_map(|(block, body)| {
                body.statements
                    .iter()
                    .position(|statement| {
                        matches!(
                            statement.kind(),
                            MirStatementKind::Assign {
                                value: MirRvalue {
                                    kind: MirRvalueKind::Use(MirOperand {
                                        kind: MirOperandKind::Move(place),
                                        ..
                                    }),
                                    ..
                                },
                                ..
                            } if place.projections().is_empty()
                        )
                    })
                    .map(|index| (block, index))
            })
            .expect("affine input is transferred by one direct move");
        let duplicate = function.blocks[block].statements[index].clone();
        function.blocks[block]
            .statements
            .insert(index + 1, duplicate);

        let error = verify_mir(&resolved, &hir, &mir).unwrap_err();
        assert!(
            error
                .message()
                .contains("after its value became unavailable")
        );

        let (resolved, hir) = checked(
            "fn consume[T: Discard](input: T, flag: Bool) {\n\
                 if flag {\n\
                     _ = input\n\
                     return\n\
                 }\n\
                 _ = input\n\
             }\n",
        );
        let mut mir = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        let function = mir
            .functions
            .get_mut(&MirFunctionId::Callable(function_id(&resolved, "consume")))
            .unwrap();
        let input = function
            .locals
            .iter()
            .position(|local| {
                matches!(
                    local.kind,
                    MirLocalKind::Parameter {
                        index: 0,
                        source: Some(_)
                    }
                )
            })
            .map(|index| MirLocalId(index as u32))
            .expect("input is the first source parameter");
        let move_blocks = function
            .blocks
            .iter()
            .enumerate()
            .filter_map(|(index, block)| {
                block
                    .statements
                    .iter()
                    .any(|statement| {
                        matches!(
                            &statement.kind,
                            MirStatementKind::Assign {
                                value: MirRvalue {
                                    kind: MirRvalueKind::Use(MirOperand {
                                        kind: MirOperandKind::Move(place),
                                        ..
                                    }),
                                    ..
                                },
                                ..
                            } if place.local == input && place.projections.is_empty()
                        )
                    })
                    .then_some(index)
            })
            .collect::<Vec<_>>();
        assert_eq!(move_blocks.len(), 2);
        let returning = move_blocks
            .iter()
            .copied()
            .find(|index| {
                matches!(
                    function.blocks[*index].terminator.kind,
                    MirTerminatorKind::Return
                )
            })
            .expect("the first move originally diverges");
        let joined = move_blocks
            .into_iter()
            .find(|index| *index != returning)
            .unwrap();
        function.blocks[returning].terminator.kind = MirTerminatorKind::Goto {
            target: MirBlockId(joined as u32),
        };

        let error = verify_mir(&resolved, &hir, &mir).unwrap_err();
        assert!(
            error
                .message()
                .contains("after its value became unavailable")
        );
    }

    #[test]
    fn mir_move_paths_allow_siblings_restore_children_and_reject_root_reuse() {
        let source = "fn identity[T](value: T): T { value }\n\
                      fn rebuild[T: Discard](input: (T, T)): (T, T) {\n\
                          let (left, right) = input\n\
                          identity((left, right))\n\
                      }\n";
        let (resolved, hir) = checked(source);

        let mut duplicate = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        let function = duplicate
            .functions
            .get_mut(&MirFunctionId::Callable(function_id(&resolved, "rebuild")))
            .unwrap();
        let (block, index) = function
            .blocks
            .iter()
            .enumerate()
            .find_map(|(block, body)| {
                body.statements
                    .iter()
                    .position(|statement| {
                        matches!(
                            statement.kind(),
                            MirStatementKind::Assign {
                                value: MirRvalue {
                                    kind: MirRvalueKind::Use(MirOperand {
                                        kind: MirOperandKind::Move(place),
                                        ..
                                    }),
                                    ..
                                },
                                ..
                            } if matches!(
                                place.projections(),
                                [MirProjection {
                                    kind: MirProjectionKind::TupleField(_),
                                    ..
                                }]
                            )
                        )
                    })
                    .map(|index| (block, index))
            })
            .expect("tuple destructuring moves one projected child");
        let repeated = function.blocks[block].statements[index].clone();
        function.blocks[block]
            .statements
            .insert(index + 1, repeated);
        let error = verify_mir(&resolved, &hir, &duplicate).unwrap_err();
        assert!(error.message().contains("unavailable move path"), "{error}");

        let mut restored = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        let function = restored
            .functions
            .get_mut(&MirFunctionId::Callable(function_id(&resolved, "rebuild")))
            .unwrap();
        let (block, index) = function
            .blocks
            .iter()
            .enumerate()
            .find_map(|(block, body)| {
                body.statements
                    .iter()
                    .position(|statement| {
                        matches!(
                            statement.kind(),
                            MirStatementKind::Assign {
                                value: MirRvalue {
                                    kind: MirRvalueKind::Use(MirOperand {
                                        kind: MirOperandKind::Move(place),
                                        ..
                                    }),
                                    ..
                                },
                                ..
                            } if matches!(
                                place.projections(),
                                [MirProjection {
                                    kind: MirProjectionKind::TupleField(_),
                                    ..
                                }]
                            )
                        )
                    })
                    .map(|index| (block, index))
            })
            .unwrap();
        let projected_move = function.blocks[block].statements[index].clone();
        let MirStatementKind::Assign {
            destination: child_owner,
            value:
                MirRvalue {
                    kind:
                        MirRvalueKind::Use(MirOperand {
                            kind: MirOperandKind::Move(child),
                            ..
                        }),
                    ..
                },
        } = projected_move.kind()
        else {
            unreachable!()
        };
        let restore = MirStatement {
            span: projected_move.span(),
            kind: MirStatementKind::Assign {
                destination: child.clone(),
                value: MirRvalue {
                    ty: child_owner.ty(),
                    kind: MirRvalueKind::Use(MirOperand {
                        ty: child_owner.ty(),
                        kind: MirOperandKind::Move(child_owner.clone()),
                    }),
                },
            },
        };
        function.blocks[block].statements.insert(index + 1, restore);
        function.blocks[block]
            .statements
            .insert(index + 2, projected_move);
        verify_mir(&resolved, &hir, &restored).unwrap();

        let mut root_reuse = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        let function = root_reuse
            .functions
            .get_mut(&MirFunctionId::Callable(function_id(&resolved, "rebuild")))
            .unwrap();
        let (root, root_type) = function
            .blocks
            .iter()
            .flat_map(|block| &block.statements)
            .find_map(|statement| match statement.kind() {
                MirStatementKind::Assign {
                    value:
                        MirRvalue {
                            kind:
                                MirRvalueKind::Use(MirOperand {
                                    kind: MirOperandKind::Move(place),
                                    ..
                                }),
                            ..
                        },
                    ..
                } if matches!(
                    place.projections(),
                    [MirProjection {
                        kind: MirProjectionKind::TupleField(_),
                        ..
                    }]
                ) =>
                {
                    Some((place.local(), function.local(place.local()).unwrap().ty()))
                }
                _ => None,
            })
            .unwrap();
        let call_place = function
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
                } => arguments
                    .iter_mut()
                    .find_map(|argument| match &mut argument.value.kind {
                        MirOperandKind::Move(place)
                            if place.projections.is_empty() && place.ty == root_type =>
                        {
                            Some(place)
                        }
                        _ => None,
                    }),
                _ => None,
            })
            .expect("the rebuilt tuple is passed by value");
        call_place.local = root;

        let error = verify_mir(&resolved, &hir, &root_reuse).unwrap_err();
        assert!(
            error
                .message()
                .contains("after its value became unavailable"),
            "{error}"
        );
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
        for state in loops.blocks().filter_map(|block| {
            let MirTerminatorKind::IteratorNext { state, .. } = block.terminator().kind() else {
                return None;
            };
            Some(state)
        }) {
            assert!(matches!(
                hir.interner().kind(state.ty()).unwrap(),
                TypeKind::Cursor {
                    mode: crate::types::CursorMode::Own,
                    ..
                }
            ));
        }
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
        let MirOperandKind::Borrow(place) = &replacement.as_ref().unwrap().kind else {
            panic!("slice assignment validation observes its replacement")
        };
        replacement.as_mut().unwrap().kind = MirOperandKind::Copy(place.clone());
        let error = verify_mir(&resolved, &hir, &invalid_slice).unwrap_err();
        assert!(error.message().contains("borrowed replacement"));

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
    fn affine_closure_environments_move_in_and_out_with_verified_protocols() {
        let source = "fn consume[T](input: T): T {\n\
                          let operation = (): T { input }\n\
                          operation()\n\
                      }\n";
        let (resolved, hir) = checked(source);
        let mut mir = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        verify_mir(&resolved, &hir, &mir).unwrap();
        let closure = hir.closures().next().unwrap();

        let construction = mir
            .functions()
            .flat_map(|function| function.blocks())
            .flat_map(|block| block.statements())
            .find_map(|statement| match statement.kind() {
                MirStatementKind::Assign {
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
                } => values.first(),
                _ => None,
            })
            .expect("closure construction has one capture");
        assert!(matches!(construction.kind(), MirOperandKind::Move(_)));

        let body = mir.closure_function(closure.id()).unwrap();
        assert!(
            body.blocks()
                .flat_map(MirBasicBlock::statements)
                .any(|statement| matches!(
                    statement.kind(),
                    MirStatementKind::Assign {
                        value:
                            MirRvalue {
                                kind:
                                    MirRvalueKind::Use(MirOperand {
                                        kind: MirOperandKind::Move(MirPlace { projections, .. }),
                                        ..
                                    }),
                                ..
                            },
                        ..
                    } if matches!(
                        projections.first().map(MirProjection::kind),
                        Some(MirProjectionKind::ClosureCapture { .. })
                    )
                ))
        );

        let captured = mir
            .functions
            .values_mut()
            .flat_map(|function| &mut function.blocks)
            .flat_map(|block| &mut block.statements)
            .find_map(|statement| match &mut statement.kind {
                MirStatementKind::Assign {
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
                } => values.first_mut(),
                _ => None,
            })
            .unwrap();
        let MirOperandKind::Move(place) = &captured.kind else {
            unreachable!()
        };
        captured.kind = MirOperandKind::Copy(place.clone());
        let error = verify_mir(&resolved, &hir, &mir).unwrap_err();
        assert!(
            error.message().contains("contextual Copy status"),
            "{error}"
        );
    }

    #[test]
    fn mir_rederives_capture_moves_when_validating_closure_protocols() {
        let source = "fn compare[T: Equatable + Discard](input: T, other: T): Bool {\n\
                          let operation = (candidate: T): Bool { input == candidate }\n\
                          operation(other)\n\
                      }\n";
        let (resolved, hir) = checked(source);
        let closure = hir.closures().next().unwrap().id();
        let mut mir = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        verify_mir(&resolved, &hir, &mir).unwrap();

        let body = mir
            .functions
            .get_mut(&MirFunctionId::Closure(closure))
            .unwrap();
        let captured = body
            .blocks
            .iter_mut()
            .flat_map(|block| &mut block.statements)
            .find_map(|statement| match &mut statement.kind {
                MirStatementKind::Assign {
                    value:
                        MirRvalue {
                            kind: MirRvalueKind::Binary { left, .. },
                            ..
                        },
                    ..
                } if matches!(
                    &left.kind,
                    MirOperandKind::Borrow(MirPlace { projections, .. })
                        if matches!(
                            projections.first().map(MirProjection::kind),
                            Some(MirProjectionKind::ClosureCapture { .. })
                        )
                ) =>
                {
                    Some(left)
                }
                _ => None,
            })
            .expect("comparison observes its captured environment slot");
        let MirOperandKind::Borrow(place) = &captured.kind else {
            unreachable!()
        };
        captured.kind = MirOperandKind::Move(place.clone());

        let error = verify_mir(&resolved, &hir, &mir).unwrap_err();
        assert!(error.message().contains("protocols differ"), "{error}");
    }

    #[test]
    fn mir_rederives_call_once_from_capture_transfers_on_every_return() {
        let complete = "fn consume[T](input: T, choose: Bool): T {\n\
                            let operation = (): T {\n\
                                if choose {\n\
                                    return input\n\
                                }\n\
                                input\n\
                            }\n\
                            operation()\n\
                        }\n";
        let (resolved, hir) = checked(complete);
        assert_eq!(
            hir.closures().next().unwrap().protocols(),
            crate::hir::HirClosureProtocols::new(false, false, true)
        );
        let mir = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        verify_mir(&resolved, &hir, &mir).unwrap();

        let partial = "fn build[T](input: T, choose: Bool) {\n\
                           let operation = (): T? {\n\
                               if choose {\n\
                                   return some(input)\n\
                               }\n\
                               none\n\
                           }\n\
                       }\n";
        let (resolved, hir) = checked(partial);
        assert_eq!(
            hir.closures().next().unwrap().protocols(),
            crate::hir::HirClosureProtocols::new(false, false, false)
        );
        let mir = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        verify_mir(&resolved, &hir, &mir).unwrap();

        let newtype = "type Wrapped = Join[Int, String]\n\
                       fn build(input: Wrapped) {\n\
                           let operation = (): Join[Int, String] { input.value }\n\
                       }\n";
        let (resolved, hir) = checked(newtype);
        assert_eq!(
            hir.closures().next().unwrap().protocols(),
            crate::hir::HirClosureProtocols::new(false, false, true)
        );
        let mir = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        verify_mir(&resolved, &hir, &mir).unwrap();
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
    fn mir_verifier_rejects_borrows_in_value_arguments() {
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

        let (resolved, hir) = checked(
            "fn inspect(value: ref Int): Int { value }\n\
             fn execute(): Int {\n\
                 let value = 42\n\
                 inspect(ref value)\n\
             }\n",
        );
        let mut mir = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        let (function, loan) = mir
            .functions
            .iter()
            .find_map(|(function, body)| {
                body.blocks
                    .iter()
                    .find_map(|block| match &block.terminator.kind {
                        MirTerminatorKind::Invoke {
                            operation:
                                MirOperation {
                                    kind: MirOperationKind::Call { arguments, .. },
                                    ..
                                },
                            ..
                        } => arguments.iter().find_map(|argument| {
                            if argument.mode == crate::types::ParameterMode::Ref {
                                match argument.value.kind {
                                    MirOperandKind::Loan(loan) => Some((*function, loan)),
                                    _ => None,
                                }
                            } else {
                                None
                            }
                        }),
                        _ => None,
                    })
            })
            .expect("ref argument consumes an explicit loan");
        let place = mir.functions[&function].loans[loan.index() as usize]
            .place
            .clone();
        let argument = mir
            .functions
            .get_mut(&function)
            .unwrap()
            .blocks
            .iter_mut()
            .find_map(|block| {
                match &mut block.terminator.kind {
                MirTerminatorKind::Invoke {
                    operation:
                        MirOperation {
                            kind: MirOperationKind::Call { arguments, .. },
                            ..
                        },
                    ..
                } => arguments.iter_mut().find(|argument| {
                    matches!(argument.value.kind, MirOperandKind::Loan(id) if id == loan)
                }),
                _ => None,
            }
            })
            .unwrap();
        argument.value.kind = MirOperandKind::Borrow(place);
        let error = verify_mir(&resolved, &hir, &mir).unwrap_err();
        assert!(error.message().contains("borrow escapes"));
    }

    #[test]
    fn mir_loans_have_one_explicit_reservation_and_call_consumption() {
        let source = "fn inspect(value: ref Int): Int { value }\n\
                      fn execute(): Int {\n\
                          let value = 42\n\
                          inspect(ref value)\n\
                      }\n";
        let (resolved, hir) = checked(source);
        let mir = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        let execute = mir.function(function_id(&resolved, "execute")).unwrap();
        assert_eq!(execute.loans().len(), 1);
        let loan = MirLoanId(0);
        assert!(execute.blocks().any(|block| {
            block.statements().iter().any(|statement| {
                matches!(statement.kind(), MirStatementKind::ReserveLoan(id) if *id == loan)
            })
        }));
        assert!(execute.blocks().any(|block| {
            matches!(
                block.terminator().kind(),
                MirTerminatorKind::Invoke {
                    operation: MirOperation {
                        kind: MirOperationKind::Call { arguments, .. },
                        ..
                    },
                    ..
                } if arguments.iter().any(|argument| {
                    matches!(argument.value.kind, MirOperandKind::Loan(id) if id == loan)
                })
            )
        }));
        verify_mir(&resolved, &hir, &mir).unwrap();

        let mut forged = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        let function = forged
            .functions
            .get_mut(&MirFunctionId::Callable(function_id(&resolved, "execute")))
            .unwrap();
        let (block, index) = function
            .blocks
            .iter()
            .enumerate()
            .find_map(|(block, body)| {
                body.statements
                    .iter()
                    .enumerate()
                    .find_map(|(index, statement)| {
                        matches!(statement.kind, MirStatementKind::ReserveLoan(_))
                            .then_some((block, index))
                    })
            })
            .unwrap();
        let duplicate = function.blocks[block].statements[index].clone();
        function.blocks[block]
            .statements
            .insert(index + 1, duplicate);
        let error = verify_mir(&resolved, &hir, &forged).unwrap_err();
        assert!(error.message().contains("reservations"), "{error}");

        let mut forged = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        let function = forged
            .functions
            .get_mut(&MirFunctionId::Callable(function_id(&resolved, "execute")))
            .unwrap();
        let (block, index) = function
            .blocks
            .iter()
            .enumerate()
            .find_map(|(block, body)| {
                body.statements
                    .iter()
                    .position(|statement| {
                        matches!(statement.kind, MirStatementKind::ReserveLoan(_))
                    })
                    .map(|index| (block, index))
            })
            .unwrap();
        let reservation = function.blocks[block].statements.remove(index);
        function.blocks[function.unwind.index() as usize]
            .statements
            .push(reservation);
        let error = verify_mir(&resolved, &hir, &forged).unwrap_err();
        assert!(
            error
                .message()
                .contains("cleanup block manipulates a loan reservation"),
            "{error}"
        );

        let mut forged = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        let function = forged
            .functions
            .get_mut(&MirFunctionId::Callable(function_id(&resolved, "execute")))
            .unwrap();
        let (block, index) = function
            .blocks
            .iter()
            .enumerate()
            .find_map(|(block, body)| {
                body.statements
                    .iter()
                    .position(|statement| {
                        matches!(statement.kind, MirStatementKind::ReserveLoan(_))
                    })
                    .map(|index| (block, index))
            })
            .unwrap();
        let mut release = function.blocks[block].statements[index].clone();
        release.kind = MirStatementKind::ReleaseLoan(MirLoanId(0));
        function.blocks[block].statements.insert(index, release);
        let error = verify_mir(&resolved, &hir, &forged).unwrap_err();
        assert!(
            error.message().contains("releases inactive loan"),
            "{error}"
        );

        let mut forged = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        let function = forged
            .functions
            .get_mut(&MirFunctionId::Callable(function_id(&resolved, "execute")))
            .unwrap();
        let loan_local = function.loans[0].place.local;
        let write = function
            .blocks
            .iter()
            .flat_map(|block| &block.statements)
            .find(|statement| {
                matches!(
                    &statement.kind,
                    MirStatementKind::Assign { destination, .. }
                        if destination.local == loan_local
                )
            })
            .cloned()
            .unwrap();
        let (block, index) = function
            .blocks
            .iter()
            .enumerate()
            .find_map(|(block, body)| {
                body.statements
                    .iter()
                    .enumerate()
                    .find_map(|(index, statement)| {
                        matches!(statement.kind, MirStatementKind::ReserveLoan(_))
                            .then_some((block, index))
                    })
            })
            .unwrap();
        function.blocks[block].statements.insert(index + 1, write);
        let error = verify_mir(&resolved, &hir, &forged).unwrap_err();
        assert!(
            error.message().contains("write overlaps active loan"),
            "{error}"
        );

        let branch_source = "fn inspect(value: ref Bool): Bool { value }\n\
                             fn execute(): Bool {\n\
                                 let value = true\n\
                                 if inspect(ref value) { true } else { false }\n\
                             }\n";
        let (branch_resolved, branch_hir) = checked(branch_source);
        let mut forged =
            lower_to_mir(&branch_resolved, &branch_hir, MirLoweringLimits::default()).unwrap();
        let function = forged
            .functions
            .get_mut(&MirFunctionId::Callable(function_id(
                &branch_resolved,
                "execute",
            )))
            .unwrap();
        let condition = function
            .blocks
            .iter_mut()
            .find_map(|block| match &mut block.terminator.kind {
                MirTerminatorKind::SwitchBool { condition, .. } => Some(condition),
                _ => None,
            })
            .unwrap();
        condition.kind = MirOperandKind::Loan(MirLoanId(0));
        let error = verify_mir(&branch_resolved, &branch_hir, &forged).unwrap_err();
        assert!(error.message().contains("materialized Bool"), "{error}");
    }

    #[test]
    fn runtime_collection_proofs_are_explicit_and_reverified_in_mir() {
        let source = "fn update(left: mut Int, right: mut Int) {}\n\
                      fn hold(value: mut Int, token: Int) {}\n\
                      fn execute() {\n\
                          var values = [1, 2]\n\
                          let left = 0\n\
                          let right = 1\n\
                          update(mut values[left], mut values[right])\n\
                          hold(mut values[left], values[right])\n\
                          hold(mut values[left], {\n\
                              values[right] = 9\n\
                              0\n\
                          })\n\
                      }\n";
        let (resolved, hir) = checked(source);
        let mir = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        verify_mir(&resolved, &hir, &mir).unwrap();
        let execute = mir.function(function_id(&resolved, "execute")).unwrap();
        assert!(execute.blocks().any(|block| matches!(
            block.terminator().kind(),
            MirTerminatorKind::ValidateLoan { against, .. } if !against.is_empty()
        )));
        assert!(execute.blocks().any(|block| matches!(
            block.terminator().kind(),
            MirTerminatorKind::Invoke {
                operation: MirOperation {
                    kind: MirOperationKind::Index { against, .. },
                    ..
                },
                ..
            } if !against.is_empty()
        )));
        assert!(execute.blocks().any(|block| matches!(
            block.terminator().kind(),
            MirTerminatorKind::ValidatePlaces { against, .. }
                if against.iter().any(|loans| !loans.is_empty())
        )));

        let mut missing_loan_proof =
            lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        let function = missing_loan_proof
            .functions
            .get_mut(&MirFunctionId::Callable(function_id(&resolved, "execute")))
            .unwrap();
        let against = function
            .blocks
            .iter_mut()
            .find_map(|block| match &mut block.terminator.kind {
                MirTerminatorKind::ValidateLoan { against, .. } if !against.is_empty() => {
                    Some(against)
                }
                _ => None,
            })
            .expect("the second dynamic loan has one runtime conflict proof");
        against.clear();
        let error = verify_mir(&resolved, &hir, &missing_loan_proof).unwrap_err();
        assert!(error.message().contains("runtime proof lists"), "{error}");

        let mut missing_read_proof =
            lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        let function = missing_read_proof
            .functions
            .get_mut(&MirFunctionId::Callable(function_id(&resolved, "execute")))
            .unwrap();
        let against = function
            .blocks
            .iter_mut()
            .find_map(|block| match &mut block.terminator.kind {
                MirTerminatorKind::Invoke {
                    operation:
                        MirOperation {
                            kind: MirOperationKind::Index { against, .. },
                            ..
                        },
                    ..
                } if !against.is_empty() => Some(against),
                _ => None,
            })
            .expect("the later indexed read has one runtime conflict proof");
        against.clear();
        let error = verify_mir(&resolved, &hir, &missing_read_proof).unwrap_err();
        assert!(
            error.message().contains("indexed operation runtime proof"),
            "{error}"
        );

        let mut missing_write_proof =
            lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        let function = missing_write_proof
            .functions
            .get_mut(&MirFunctionId::Callable(function_id(&resolved, "execute")))
            .unwrap();
        let against = function
            .blocks
            .iter_mut()
            .find_map(|block| match &mut block.terminator.kind {
                MirTerminatorKind::ValidatePlaces { against, .. }
                    if against.iter().any(|loans| !loans.is_empty()) =>
                {
                    Some(against)
                }
                _ => None,
            })
            .expect("the later indexed write has one runtime conflict proof");
        against.iter_mut().for_each(Vec::clear);
        let error = verify_mir(&resolved, &hir, &missing_write_proof).unwrap_err();
        assert!(
            error.message().contains("place validation runtime proof"),
            "{error}"
        );

        let mut detached_validation =
            lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        let function = detached_validation
            .functions
            .get_mut(&MirFunctionId::Callable(function_id(&resolved, "execute")))
            .unwrap();
        let target = function
            .blocks
            .iter()
            .find_map(|block| match block.terminator.kind {
                MirTerminatorKind::ValidateLoan { target, .. } => Some(target),
                _ => None,
            })
            .expect("dynamic loan has an explicit validation target");
        let first = function.blocks[target.index() as usize]
            .statements
            .remove(0);
        assert!(matches!(first.kind, MirStatementKind::ReserveLoan(_)));
        let error = verify_mir(&resolved, &hir, &detached_validation).unwrap_err();
        assert!(
            error
                .message()
                .contains("success does not immediately reserve the same loan"),
            "{error}"
        );
    }

    #[test]
    fn borrow_pattern_regions_are_explicit_and_end_before_later_writes() {
        let source = "type Pair = { left: Int, right: Int }\n\
                      fn inspect(value: ref Int) {}\n\
                      fn abandonBorrow(value: ref Int, after: Unit) {}\n\
                      fn abandon(pair: var Pair) {\n\
                          match pair {\n\
                              Pair { ref left, right: _ } => {\n\
                                  abandonBorrow(ref left, {\n\
                                      return\n\
                                  })\n\
                              }\n\
                          }\n\
                      }\n\
                      fn execute(): Int {\n\
                          var pair = Pair { left: 1, right: 2 }\n\
                          match pair {\n\
                              Pair { ref left, right: _ } => {\n\
                                  if pair.right == 2 {\n\
                                      match left {\n\
                                          ref nested => inspect(ref nested)\n\
                                      }\n\
                                  }\n\
                                  pair.left = 7\n\
                              }\n\
                          }\n\
                          pair.left\n\
                      }\n";
        let (resolved, hir) = checked(source);
        let mir = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        let execute = mir.function(function_id(&resolved, "execute")).unwrap();
        let regions = execute
            .loans()
            .enumerate()
            .filter(|(_, loan)| loan.kind() == MirLoanKind::Region)
            .map(|(index, loan)| (MirLoanId(index as u32), loan))
            .collect::<Vec<_>>();
        assert_eq!(regions.len(), 2);
        let (region, region_loan) = regions[0];
        let (nested_region, nested_loan) = regions[1];
        assert_eq!(region_loan.mode(), crate::types::ParameterMode::Ref);
        assert!(region_loan.place().source_loan().is_none());
        assert_eq!(nested_loan.place().source_loan(), Some(region));
        assert!(execute.loans().any(|loan| {
            loan.kind() == MirLoanKind::CallLocal
                && loan.place().source_loan() == Some(nested_region)
        }));
        assert!(execute.blocks().any(|block| {
            block.statements().iter().any(|statement| {
                matches!(statement.kind(), MirStatementKind::ReserveLoan(id) if *id == region)
            })
        }));
        assert!(execute.blocks().any(|block| {
            block.statements().iter().any(|statement| {
                matches!(statement.kind(), MirStatementKind::ReleaseLoan(id) if *id == region)
            })
        }));

        let abandon = mir.function(function_id(&resolved, "abandon")).unwrap();
        let abandon_region = abandon
            .loans()
            .enumerate()
            .find(|(_, loan)| loan.kind() == MirLoanKind::Region)
            .map(|(index, _)| MirLoanId(index as u32))
            .unwrap();
        let abandoned_call = abandon
            .loans()
            .enumerate()
            .find(|(_, loan)| {
                loan.kind() == MirLoanKind::CallLocal
                    && loan.place().source_loan() == Some(abandon_region)
            })
            .map(|(index, _)| MirLoanId(index as u32))
            .unwrap();
        assert!(abandon.blocks().any(|block| {
            block.statements().windows(2).any(|statements| {
                matches!(
                    statements[0].kind(),
                    MirStatementKind::ReleaseLoan(id) if *id == abandoned_call
                ) && matches!(
                    statements[1].kind(),
                    MirStatementKind::ReleaseLoan(id) if *id == abandon_region
                )
            })
        }));
        verify_mir(&resolved, &hir, &mir).unwrap();

        let mut forged = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        let function = forged
            .functions
            .get_mut(&MirFunctionId::Callable(function_id(&resolved, "execute")))
            .unwrap();
        for block in &mut function.blocks {
            block.statements.retain(|statement| {
                !matches!(statement.kind, MirStatementKind::ReleaseLoan(id) if id == region)
            });
        }
        let (block, index) = function
            .blocks
            .iter()
            .enumerate()
            .find_map(|(block, body)| {
                body.statements
                    .iter()
                    .position(|statement| {
                        matches!(statement.kind, MirStatementKind::ReserveLoan(id) if id == nested_region)
                    })
                    .map(|index| (block, index))
            })
            .unwrap();
        let span = function.blocks[block].statements[index].span;
        function.blocks[block].statements.insert(
            index + 1,
            MirStatement {
                span,
                kind: MirStatementKind::ReleaseLoan(region),
            },
        );
        let error = verify_mir(&resolved, &hir, &forged).unwrap_err();
        assert!(error.message().contains("dependent loan"), "{error}");
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
