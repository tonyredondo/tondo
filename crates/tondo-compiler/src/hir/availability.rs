use std::collections::{BTreeMap, BTreeSet};

use crate::resolve::LocalId;
use crate::source::Span;
use crate::types::{CursorMode, ParameterMode, TypeError, TypeId, TypeKind};

use super::{
    CapabilityAnalysis, CapabilityAssumptions, HirAssignmentTarget, HirAssignmentTargetKind,
    HirBinaryOperator, HirCallProtocol, HirCapability, HirCapabilityStatus, HirExpressionId,
    HirExpressionKind, HirForKind, HirIterationProtocol, HirLoopId, HirPatternId, HirPatternKind,
    HirProgram, HirStatement, HirValueCategory, HirVariantValue,
};

type AvailabilityState = BTreeMap<LocalId, Span>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct AvailabilityFinding {
    local: LocalId,
    use_span: Span,
    move_span: Span,
}

impl AvailabilityFinding {
    pub(crate) fn local(self) -> LocalId {
        self.local
    }

    pub(crate) fn use_span(self) -> Span {
        self.use_span
    }

    pub(crate) fn move_span(self) -> Span {
        self.move_span
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Demand {
    Observe,
    Transfer,
}

#[derive(Clone, Debug, Default)]
struct AvailabilityFlow {
    normal: Option<AvailabilityState>,
    breaks: BTreeMap<HirLoopId, AvailabilityState>,
    continues: BTreeMap<HirLoopId, AvailabilityState>,
}

impl AvailabilityFlow {
    fn normal(state: AvailabilityState) -> Self {
        Self {
            normal: Some(state),
            ..Self::default()
        }
    }

    fn merge(&mut self, other: Self) {
        merge_optional_state(&mut self.normal, other.normal);
        merge_control_states(&mut self.breaks, other.breaks);
        merge_control_states(&mut self.continues, other.continues);
    }

    fn strip_locals(&mut self, locals: &[LocalId]) {
        if let Some(state) = &mut self.normal {
            remove_locals(state, locals);
        }
        for state in self.breaks.values_mut() {
            remove_locals(state, locals);
        }
        for state in self.continues.values_mut() {
            remove_locals(state, locals);
        }
    }
}

pub(crate) fn analyze_availability(
    program: &HirProgram,
    capabilities: &CapabilityAnalysis,
) -> Result<Vec<AvailabilityFinding>, TypeError> {
    let mut findings = BTreeSet::new();
    for callable in program.callables() {
        let Some(body) = program.body(callable.id()) else {
            continue;
        };
        let owners = callable
            .parameters()
            .iter()
            .filter(|parameter| parameter.mode() == ParameterMode::Value)
            .filter_map(|parameter| parameter.local())
            .collect();
        Analyzer::new(
            program,
            capabilities,
            CapabilityAssumptions::from_generics(program, callable.generics()),
            owners,
            &mut findings,
        )
        .analyze_body(body.root())?;
    }
    for closure in program.closures() {
        let owners = closure
            .parameters()
            .iter()
            .filter(|parameter| parameter.mode() == ParameterMode::Value)
            .filter_map(|parameter| parameter.local())
            .collect();
        Analyzer::new(
            program,
            capabilities,
            CapabilityAssumptions::from_generics(program, closure.generics()),
            owners,
            &mut findings,
        )
        .analyze_body(closure.body().root())?;
    }
    Ok(findings.into_iter().collect())
}

struct Analyzer<'a, 'f> {
    program: &'a HirProgram,
    capabilities: &'a CapabilityAnalysis,
    assumptions: CapabilityAssumptions,
    owners: BTreeSet<LocalId>,
    copy_statuses: BTreeMap<TypeId, HirCapabilityStatus>,
    findings: &'f mut BTreeSet<AvailabilityFinding>,
}

impl<'a, 'f> Analyzer<'a, 'f> {
    fn new(
        program: &'a HirProgram,
        capabilities: &'a CapabilityAnalysis,
        assumptions: CapabilityAssumptions,
        owners: BTreeSet<LocalId>,
        findings: &'f mut BTreeSet<AvailabilityFinding>,
    ) -> Self {
        Self {
            program,
            capabilities,
            assumptions,
            owners,
            copy_statuses: BTreeMap::new(),
            findings,
        }
    }

    fn analyze_body(&mut self, root: HirExpressionId) -> Result<(), TypeError> {
        let _ = self.expression(root, AvailabilityState::new(), Demand::Transfer)?;
        Ok(())
    }

    fn expression(
        &mut self,
        id: HirExpressionId,
        state: AvailabilityState,
        demand: Demand,
    ) -> Result<AvailabilityFlow, TypeError> {
        let expression = self
            .program
            .expression(id)
            .expect("availability analysis receives verified expression IDs")
            .clone();
        if expression.category() == HirValueCategory::Place {
            return self.place(id, state, demand);
        }

        let flow = match expression.kind() {
            HirExpressionKind::Recovery
            | HirExpressionKind::Literal(_)
            | HirExpressionKind::Constant(_)
            | HirExpressionKind::Function(_)
            | HirExpressionKind::SpecializedFunction { .. }
            | HirExpressionKind::PreludeTraitFunction { .. }
            | HirExpressionKind::Closure(_)
            | HirExpressionKind::Receiver => AvailabilityFlow::normal(state),
            HirExpressionKind::Local(_) => {
                unreachable!("local expressions are place-category values")
            }
            HirExpressionKind::InterpolatedString { values, .. } => self.sequence(
                state,
                values.iter().copied().map(|value| (value, Demand::Observe)),
            )?,
            HirExpressionKind::Tuple(values)
            | HirExpressionKind::Array(values)
            | HirExpressionKind::Set(values) => self.sequence(
                state,
                values
                    .iter()
                    .copied()
                    .map(|value| (value, Demand::Transfer)),
            )?,
            HirExpressionKind::Map { entries, .. } => {
                let values = entries.iter().flat_map(|entry| {
                    [
                        (entry.key(), Demand::Transfer),
                        (entry.value(), Demand::Transfer),
                    ]
                });
                self.sequence(state, values)?
            }
            HirExpressionKind::Newtype { value, .. }
            | HirExpressionKind::NumericConversion { value, .. }
            | HirExpressionKind::OptionSome { value }
            | HirExpressionKind::ResultOk { value }
            | HirExpressionKind::Coerce { value, .. } => {
                self.expression(*value, state, Demand::Transfer)?
            }
            HirExpressionKind::ResultErr { error } | HirExpressionKind::Fail { error } => {
                let mut flow = self.expression(*error, state, Demand::Transfer)?;
                if matches!(expression.kind(), HirExpressionKind::Fail { .. }) {
                    flow.normal = None;
                }
                flow
            }
            HirExpressionKind::Record { fields, .. } => self.sequence(
                state,
                fields.iter().map(|field| (field.value(), Demand::Transfer)),
            )?,
            HirExpressionKind::Variant { payload, .. } => match payload {
                HirVariantValue::Unit => AvailabilityFlow::normal(state),
                HirVariantValue::Tuple(values) => self.sequence(
                    state,
                    values
                        .iter()
                        .copied()
                        .map(|value| (value, Demand::Transfer)),
                )?,
                HirVariantValue::Record(fields) => self.sequence(
                    state,
                    fields.iter().map(|field| (field.value(), Demand::Transfer)),
                )?,
            },
            HirExpressionKind::RecordUpdate { base, fields } => {
                let mut flow = self.expression(*base, state, Demand::Transfer)?;
                for field in fields {
                    flow = self.then_expression(flow, field.value(), Demand::Transfer)?;
                }
                flow
            }
            HirExpressionKind::Block { statements, tail } => {
                self.block(statements, *tail, state)?
            }
            HirExpressionKind::Prefix { operand, .. } => {
                self.expression(*operand, state, Demand::Transfer)?
            }
            HirExpressionKind::Binary {
                operator: HirBinaryOperator::LogicalAnd | HirBinaryOperator::LogicalOr,
                left,
                right,
            } => {
                let mut left_flow = self.expression(*left, state, Demand::Transfer)?;
                if let Some(right_entry) = left_flow.normal.clone() {
                    let right_flow = self.expression(*right, right_entry, Demand::Transfer)?;
                    merge_optional_state(&mut left_flow.normal, right_flow.normal.clone());
                    let mut controls = right_flow;
                    controls.normal = None;
                    left_flow.merge(controls);
                }
                left_flow
            }
            HirExpressionKind::Binary {
                operator: HirBinaryOperator::Equal | HirBinaryOperator::NotEqual,
                left,
                right,
            } => self.sequence(state, [(*left, Demand::Observe), (*right, Demand::Observe)])?,
            HirExpressionKind::Binary { left, right, .. }
            | HirExpressionKind::Range {
                start: left,
                end: right,
                ..
            } => self.sequence(
                state,
                [(*left, Demand::Transfer), (*right, Demand::Transfer)],
            )?,
            HirExpressionKind::Contains {
                item, container, ..
            } => self.sequence(
                state,
                [(*item, Demand::Observe), (*container, Demand::Observe)],
            )?,
            HirExpressionKind::Field { .. } | HirExpressionKind::TupleField { .. } => {
                self.place(id, state, demand)?
            }
            HirExpressionKind::Index { base, index, .. } => self.sequence(
                state,
                [(*base, Demand::Observe), (*index, Demand::Transfer)],
            )?,
            HirExpressionKind::Slice {
                base,
                start,
                end,
                step,
            } => {
                let mut values = vec![(*base, Demand::Observe)];
                values.extend(start.map(|value| (value, Demand::Transfer)));
                values.extend(end.map(|value| (value, Demand::Transfer)));
                values.extend(step.map(|value| (value, Demand::Transfer)));
                self.sequence(state, values)?
            }
            HirExpressionKind::Call {
                callee,
                arguments,
                protocol,
                ..
            } => {
                let callee_demand = if *protocol == HirCallProtocol::CallOnce {
                    Demand::Transfer
                } else {
                    Demand::Observe
                };
                let mut flow = self.expression(*callee, state, callee_demand)?;
                for argument in arguments {
                    let demand = if argument.mode() == ParameterMode::Value {
                        Demand::Transfer
                    } else {
                        Demand::Observe
                    };
                    flow = self.then_expression(flow, argument.value(), demand)?;
                }
                flow
            }
            HirExpressionKind::PreludePanic { message } => {
                let mut flow = self.expression(*message, state, Demand::Transfer)?;
                flow.normal = None;
                flow
            }
            HirExpressionKind::PreludeAssert {
                condition,
                message_parts,
                ..
            } => {
                let mut flow = self.expression(*condition, state, Demand::Transfer)?;
                for part in message_parts {
                    flow = self.then_expression(flow, part.value(), Demand::Transfer)?;
                }
                flow
            }
            HirExpressionKind::BootstrapHostCall { arguments, .. } => self.sequence(
                state,
                arguments
                    .iter()
                    .copied()
                    .map(|argument| (argument, Demand::Transfer)),
            )?,
            HirExpressionKind::PropagateOption { value }
            | HirExpressionKind::PropagateResult { value, .. } => {
                self.expression(*value, state, Demand::Transfer)?
            }
            HirExpressionKind::If {
                condition,
                then_branch,
                else_branch,
            } => self.if_expression(*condition, *then_branch, *else_branch, state)?,
            HirExpressionKind::Match { scrutinee, arms } => {
                self.match_expression(*scrutinee, arms, state)?
            }
            HirExpressionKind::Return { value } => {
                let mut flow = if let Some(value) = value {
                    self.expression(*value, state, Demand::Transfer)?
                } else {
                    AvailabilityFlow::normal(state)
                };
                flow.normal = None;
                flow
            }
            HirExpressionKind::Break { target } => {
                let mut flow = AvailabilityFlow::default();
                if let Some(target) = target {
                    flow.breaks.insert(*target, state);
                }
                flow
            }
            HirExpressionKind::Continue { target } => {
                let mut flow = AvailabilityFlow::default();
                if let Some(target) = target {
                    flow.continues.insert(*target, state);
                }
                flow
            }
        };
        Ok(self.finish_expression(id, flow))
    }

    fn block(
        &mut self,
        statements: &[HirStatement],
        tail: Option<HirExpressionId>,
        state: AvailabilityState,
    ) -> Result<AvailabilityFlow, TypeError> {
        let mut flow = AvailabilityFlow::normal(state);
        let mut locals = Vec::new();
        for statement in statements {
            let Some(state) = flow.normal.take() else {
                break;
            };
            let next = self.statement(statement, state, &mut locals)?;
            flow.merge(next);
        }
        if let Some(tail) = tail {
            flow = self.then_expression(flow, tail, Demand::Transfer)?;
        }
        flow.strip_locals(&locals);
        Ok(flow)
    }

    fn statement(
        &mut self,
        statement: &HirStatement,
        state: AvailabilityState,
        block_locals: &mut Vec<LocalId>,
    ) -> Result<AvailabilityFlow, TypeError> {
        match statement {
            HirStatement::Binding { pattern, value, .. } => {
                let mut flow = self.expression(*value, state, Demand::Transfer)?;
                if let Some(state) = &mut flow.normal {
                    self.introduce_pattern(*pattern, state, block_locals);
                }
                Ok(flow)
            }
            HirStatement::Expression { value, .. } | HirStatement::Discard { value, .. } => {
                self.expression(*value, state, Demand::Transfer)
            }
            HirStatement::Assignment { target, value, .. } => {
                self.assignment(target, *value, state)
            }
            HirStatement::For { id, kind, body, .. } => self.for_statement(*id, kind, *body, state),
        }
    }

    fn assignment(
        &mut self,
        target: &HirAssignmentTarget,
        value: HirExpressionId,
        state: AvailabilityState,
    ) -> Result<AvailabilityFlow, TypeError> {
        let mut direct = Vec::new();
        let target_flow =
            self.assignment_target(target, AvailabilityFlow::normal(state), &mut direct)?;
        let restorable = target_flow
            .normal
            .as_ref()
            .map(|state| {
                direct
                    .into_iter()
                    .filter(|local| self.owners.contains(local) && !state.contains_key(local))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut flow = self.then_expression(target_flow, value, Demand::Transfer)?;
        if let Some(state) = &mut flow.normal {
            remove_locals(state, &restorable);
        }
        Ok(flow)
    }

    fn assignment_target(
        &mut self,
        target: &HirAssignmentTarget,
        mut flow: AvailabilityFlow,
        direct: &mut Vec<LocalId>,
    ) -> Result<AvailabilityFlow, TypeError> {
        match target.kind() {
            HirAssignmentTargetKind::Place { place, .. } => {
                if let Some(local) = self.direct_local(*place) {
                    direct.push(local);
                }
                self.then_expression(flow, *place, Demand::Observe)
            }
            HirAssignmentTargetKind::Discard => Ok(flow),
            HirAssignmentTargetKind::Tuple(items) => {
                for item in items {
                    flow = self.assignment_target(item, flow, direct)?;
                }
                Ok(flow)
            }
        }
    }

    fn if_expression(
        &mut self,
        condition: HirExpressionId,
        then_branch: HirExpressionId,
        else_branch: Option<HirExpressionId>,
        state: AvailabilityState,
    ) -> Result<AvailabilityFlow, TypeError> {
        let mut condition_flow = self.expression(condition, state, Demand::Transfer)?;
        let Some(branch_entry) = condition_flow.normal.take() else {
            return Ok(condition_flow);
        };
        let then_flow = self.expression(then_branch, branch_entry.clone(), Demand::Transfer)?;
        let else_flow = if let Some(else_branch) = else_branch {
            self.expression(else_branch, branch_entry, Demand::Transfer)?
        } else {
            AvailabilityFlow::normal(branch_entry)
        };
        condition_flow.merge(then_flow);
        condition_flow.merge(else_flow);
        Ok(condition_flow)
    }

    fn match_expression(
        &mut self,
        scrutinee: HirExpressionId,
        arms: &[super::HirMatchArm],
        state: AvailabilityState,
    ) -> Result<AvailabilityFlow, TypeError> {
        let mut scrutinee_flow = self.expression(scrutinee, state, Demand::Transfer)?;
        let Some(mut next_entry) = scrutinee_flow.normal.take() else {
            return Ok(scrutinee_flow);
        };
        let mut result = scrutinee_flow;
        for arm in arms {
            let mut pattern_locals = Vec::new();
            let mut arm_entry = next_entry.clone();
            self.introduce_pattern(arm.pattern(), &mut arm_entry, &mut pattern_locals);
            let guarded_entry = if let Some(guard) = arm.guard() {
                let guard_flow = self.expression(guard, arm_entry, Demand::Transfer)?;
                let body_entry = guard_flow.normal.clone();
                if let Some(mut guard_state) = guard_flow.normal.clone() {
                    remove_locals(&mut guard_state, &pattern_locals);
                    merge_state(&mut next_entry, guard_state);
                }
                let mut controls = guard_flow;
                controls.normal = None;
                controls.strip_locals(&pattern_locals);
                result.merge(controls);
                body_entry
            } else {
                Some(arm_entry)
            };
            let Some(body_entry) = guarded_entry else {
                continue;
            };
            let mut body_flow = self.expression(arm.body(), body_entry, Demand::Transfer)?;
            body_flow.strip_locals(&pattern_locals);
            result.merge(body_flow);
        }
        Ok(result)
    }

    fn for_statement(
        &mut self,
        id: HirLoopId,
        kind: &HirForKind,
        body: HirExpressionId,
        state: AvailabilityState,
    ) -> Result<AvailabilityFlow, TypeError> {
        match kind {
            HirForKind::Infinite => self.loop_fixed_point(id, None, None, body, state),
            HirForKind::Conditional { condition } => {
                self.loop_fixed_point(id, Some(*condition), None, body, state)
            }
            HirForKind::Iterate {
                pattern,
                source,
                protocol,
            } => {
                let source_demand = match protocol {
                    HirIterationProtocol::Intrinsic { cursor } => {
                        match self.program.interner().kind(*cursor)? {
                            TypeKind::Cursor {
                                mode: CursorMode::Ref,
                                ..
                            } => Demand::Observe,
                            TypeKind::Cursor {
                                mode: CursorMode::Own,
                                ..
                            } => Demand::Transfer,
                            _ => Demand::Transfer,
                        }
                    }
                    HirIterationProtocol::Trait { .. } => Demand::Transfer,
                };
                let mut source_flow = self.expression(*source, state, source_demand)?;
                let Some(loop_entry) = source_flow.normal.take() else {
                    return Ok(source_flow);
                };
                let loop_flow =
                    self.loop_fixed_point(id, None, Some(*pattern), body, loop_entry)?;
                source_flow.merge(loop_flow);
                Ok(source_flow)
            }
        }
    }

    fn loop_fixed_point(
        &mut self,
        id: HirLoopId,
        condition: Option<HirExpressionId>,
        pattern: Option<HirPatternId>,
        body: HirExpressionId,
        initial: AvailabilityState,
    ) -> Result<AvailabilityFlow, TypeError> {
        let mut header = initial.clone();
        let limit = self.program.local_types.len().saturating_add(1);
        for _ in 0..=limit {
            let mut iteration = if let Some(condition) = condition {
                self.expression(condition, header.clone(), Demand::Transfer)?
            } else {
                AvailabilityFlow::normal(header.clone())
            };
            let natural_exit = if condition.is_some() || pattern.is_some() {
                iteration.normal.clone()
            } else {
                None
            };
            if let Some(mut body_entry) = iteration.normal.take() {
                let mut pattern_locals = Vec::new();
                if let Some(pattern) = pattern {
                    self.introduce_pattern(pattern, &mut body_entry, &mut pattern_locals);
                }
                let mut body_flow = self.expression(body, body_entry, Demand::Transfer)?;
                body_flow.strip_locals(&pattern_locals);
                iteration.merge(body_flow);
            }
            let break_exit = iteration.breaks.remove(&id);
            let continue_state = iteration.continues.remove(&id);
            let mut backedge = iteration.normal.take();
            merge_optional_state(&mut backedge, continue_state);

            let mut next_header = initial.clone();
            if let Some(backedge) = backedge {
                merge_state(&mut next_header, backedge);
            }
            if state_keys_equal(&header, &next_header) {
                let mut output = iteration;
                output.normal = natural_exit;
                merge_optional_state(&mut output.normal, break_exit);
                return Ok(output);
            }
            header = next_header;
        }
        unreachable!("availability loop lattice converges once per owned local")
    }

    fn place(
        &mut self,
        id: HirExpressionId,
        state: AvailabilityState,
        demand: Demand,
    ) -> Result<AvailabilityFlow, TypeError> {
        let expression = self
            .program
            .expression(id)
            .expect("availability analysis receives verified place IDs")
            .clone();
        let (mut flow, root) = self.place_components(id, state)?;
        if let (Some(state), Some(local)) = (&mut flow.normal, root) {
            self.access_local(state, local, expression.span(), expression.ty(), demand)?;
        }
        Ok(self.finish_expression(id, flow))
    }

    fn place_components(
        &mut self,
        id: HirExpressionId,
        state: AvailabilityState,
    ) -> Result<(AvailabilityFlow, Option<LocalId>), TypeError> {
        let expression = self
            .program
            .expression(id)
            .expect("availability analysis receives verified place IDs")
            .clone();
        match expression.kind() {
            HirExpressionKind::Local(local) => Ok((AvailabilityFlow::normal(state), Some(*local))),
            HirExpressionKind::Receiver => Ok((AvailabilityFlow::normal(state), None)),
            HirExpressionKind::Field { base, .. } | HirExpressionKind::TupleField { base, .. } => {
                self.place_base(*base, state)
            }
            HirExpressionKind::Index { base, index, .. } => {
                let (flow, root) = self.place_base(*base, state)?;
                Ok((self.then_expression(flow, *index, Demand::Transfer)?, root))
            }
            HirExpressionKind::Slice {
                base,
                start,
                end,
                step,
            } => {
                let (mut flow, root) = self.place_base(*base, state)?;
                for value in start.iter().chain(end).chain(step) {
                    flow = self.then_expression(flow, *value, Demand::Transfer)?;
                }
                Ok((flow, root))
            }
            // Expression checking can preserve a recovery value as an invalid
            // assignment target. Earlier diagnostics own that error; the
            // availability pass must remain total over incomplete HIR. The HIR
            // verifier independently rejects a non-place kind marked as a
            // complete place before this analysis is trusted by MIR.
            _ => Ok((AvailabilityFlow::normal(state), None)),
        }
    }

    fn place_base(
        &mut self,
        base: HirExpressionId,
        state: AvailabilityState,
    ) -> Result<(AvailabilityFlow, Option<LocalId>), TypeError> {
        if self
            .program
            .expression(base)
            .expect("place bases reference verified expressions")
            .category()
            == HirValueCategory::Place
        {
            self.place_components(base, state)
        } else {
            Ok((self.expression(base, state, Demand::Transfer)?, None))
        }
    }

    fn access_local(
        &mut self,
        state: &mut AvailabilityState,
        local: LocalId,
        span: Span,
        ty: TypeId,
        demand: Demand,
    ) -> Result<(), TypeError> {
        if !self.owners.contains(&local) {
            return Ok(());
        }
        if let Some(move_span) = state.get(&local).copied() {
            self.findings.insert(AvailabilityFinding {
                local,
                use_span: span,
                move_span,
            });
            return Ok(());
        }
        if demand == Demand::Transfer && !self.is_copy(ty)? {
            state.insert(local, span);
        }
        Ok(())
    }

    fn is_copy(&mut self, ty: TypeId) -> Result<bool, TypeError> {
        let status = if let Some(status) = self.copy_statuses.get(&ty).copied() {
            status
        } else {
            let status = self.capabilities.status(
                self.program,
                ty,
                HirCapability::Copy,
                &self.assumptions,
            )?;
            self.copy_statuses.insert(ty, status);
            status
        };
        Ok(status != HirCapabilityStatus::Unsatisfied)
    }

    fn introduce_pattern(
        &mut self,
        pattern: HirPatternId,
        state: &mut AvailabilityState,
        locals: &mut Vec<LocalId>,
    ) {
        let pattern = self
            .program
            .pattern(pattern)
            .expect("availability analysis receives verified pattern IDs")
            .clone();
        match pattern.kind() {
            HirPatternKind::Recovery
            | HirPatternKind::Wildcard
            | HirPatternKind::Literal(_)
            | HirPatternKind::OptionNone => {}
            HirPatternKind::Binding(local) => {
                self.owners.insert(*local);
                state.remove(local);
                locals.push(*local);
            }
            HirPatternKind::BorrowBinding(local) => {
                state.remove(local);
                locals.push(*local);
            }
            HirPatternKind::Tuple(items) => {
                for item in items {
                    self.introduce_pattern(*item, state, locals);
                }
            }
            HirPatternKind::OptionSome(item)
            | HirPatternKind::ResultOk(item)
            | HirPatternKind::ResultErr(item)
            | HirPatternKind::UnionMember { pattern: item, .. } => {
                self.introduce_pattern(*item, state, locals);
            }
            HirPatternKind::Newtype { value, .. } => {
                self.introduce_pattern(*value, state, locals);
            }
            HirPatternKind::Variant { fields, .. } => {
                for field in fields {
                    self.introduce_pattern(*field, state, locals);
                }
            }
            HirPatternKind::Record { fields, .. } => {
                for field in fields {
                    self.introduce_pattern(field.pattern(), state, locals);
                }
            }
            HirPatternKind::Array { prefix, rest } => {
                for item in prefix {
                    self.introduce_pattern(*item, state, locals);
                }
                if let Some(rest) = rest {
                    self.introduce_pattern(*rest, state, locals);
                }
            }
        }
    }

    fn sequence(
        &mut self,
        state: AvailabilityState,
        values: impl IntoIterator<Item = (HirExpressionId, Demand)>,
    ) -> Result<AvailabilityFlow, TypeError> {
        let mut flow = AvailabilityFlow::normal(state);
        for (value, demand) in values {
            flow = self.then_expression(flow, value, demand)?;
        }
        Ok(flow)
    }

    fn then_expression(
        &mut self,
        mut flow: AvailabilityFlow,
        expression: HirExpressionId,
        demand: Demand,
    ) -> Result<AvailabilityFlow, TypeError> {
        let Some(state) = flow.normal.take() else {
            return Ok(flow);
        };
        flow.merge(self.expression(expression, state, demand)?);
        Ok(flow)
    }

    fn finish_expression(
        &self,
        id: HirExpressionId,
        mut flow: AvailabilityFlow,
    ) -> AvailabilityFlow {
        if self
            .program
            .expression_flow(id)
            .is_some_and(|flow| !flow.may_complete())
        {
            flow.normal = None;
        }
        flow
    }

    fn direct_local(&self, expression: HirExpressionId) -> Option<LocalId> {
        match self.program.expression(expression)?.kind() {
            HirExpressionKind::Local(local) => Some(*local),
            _ => None,
        }
    }
}

fn merge_optional_state(target: &mut Option<AvailabilityState>, source: Option<AvailabilityState>) {
    let Some(source) = source else {
        return;
    };
    if let Some(target) = target {
        merge_state(target, source);
    } else {
        *target = Some(source);
    }
}

fn merge_control_states(
    target: &mut BTreeMap<HirLoopId, AvailabilityState>,
    source: BTreeMap<HirLoopId, AvailabilityState>,
) {
    for (loop_id, state) in source {
        if let Some(target) = target.get_mut(&loop_id) {
            merge_state(target, state);
        } else {
            target.insert(loop_id, state);
        }
    }
}

fn merge_state(target: &mut AvailabilityState, source: AvailabilityState) {
    for (local, origin) in source {
        target.entry(local).or_insert(origin);
    }
}

fn remove_locals(state: &mut AvailabilityState, locals: &[LocalId]) {
    for local in locals {
        state.remove(local);
    }
}

fn state_keys_equal(left: &AvailabilityState, right: &AvailabilityState) -> bool {
    left.keys().eq(right.keys())
}
