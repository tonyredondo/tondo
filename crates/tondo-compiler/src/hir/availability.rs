use std::collections::{BTreeMap, BTreeSet};

use crate::resolve::LocalId;
use crate::source::Span;
use crate::types::{CursorMode, ParameterMode, TypeError, TypeId, TypeKind};

use super::{
    CapabilityAnalysis, CapabilityAssumptions, HirAssignmentOperator, HirAssignmentTarget,
    HirAssignmentTargetKind, HirBinaryOperator, HirCallProtocol, HirCapability,
    HirCapabilityStatus, HirClosureCapture, HirExpressionId, HirExpressionKind, HirForKind,
    HirIterationProtocol, HirLoopId, HirMatchMode, HirPatternId, HirPatternKind, HirProgram,
    HirStatement, HirValueCategory, HirVariantValue, HirWriteKind,
};

#[derive(Clone, Debug, Default)]
struct AvailabilityState {
    unavailable: BTreeMap<LocalId, Span>,
    definitely_transferred: BTreeSet<LocalId>,
    loans: Vec<LoanReservation>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum AvailabilityFindingKind {
    UseAfterMove,
    InvalidPartialTransfer,
    InvalidBorrowedTransfer,
    InvalidGuardAccess,
    InvalidMatchMode,
    ConflictingLoan,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct AvailabilityFinding {
    kind: AvailabilityFindingKind,
    local: Option<LocalId>,
    use_span: Span,
    move_span: Option<Span>,
}

impl AvailabilityFinding {
    pub(crate) fn kind(self) -> AvailabilityFindingKind {
        self.kind
    }

    pub(crate) fn local(self) -> Option<LocalId> {
        self.local
    }

    pub(crate) fn use_span(self) -> Span {
        self.use_span
    }

    pub(crate) fn move_span(self) -> Option<Span> {
        self.move_span
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PlaceRoot {
    Local(LocalId),
    Receiver,
    Temporary(HirExpressionId),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum PlaceProjection {
    Field(crate::resolve::MemberId),
    TupleField(u32),
    Collection,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PlaceInfo {
    root: PlaceRoot,
    projections: Vec<PlaceProjection>,
    complete_transfer: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LoanReservation {
    mode: ParameterMode,
    place: PlaceInfo,
    span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LoanAccess {
    Read,
    Move,
    Write,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Demand {
    Observe,
    Transfer,
}

#[derive(Clone, Debug, Default)]
struct AvailabilityFlow {
    normal: Option<AvailabilityState>,
    exits: Option<AvailabilityState>,
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
        merge_optional_state(&mut self.exits, other.exits);
        merge_control_states(&mut self.breaks, other.breaks);
        merge_control_states(&mut self.continues, other.continues);
    }

    fn strip_locals(&mut self, locals: &[LocalId]) {
        if let Some(state) = &mut self.normal {
            remove_locals(state, locals);
        }
        if let Some(state) = &mut self.exits {
            remove_locals(state, locals);
        }
        for state in self.breaks.values_mut() {
            remove_locals(state, locals);
        }
        for state in self.continues.values_mut() {
            remove_locals(state, locals);
        }
    }

    fn truncate_loans(&mut self, count: usize) {
        if let Some(state) = &mut self.normal {
            state.loans.truncate(count);
        }
        if let Some(state) = &mut self.exits {
            state.loans.truncate(count);
        }
        for state in self.breaks.values_mut() {
            state.loans.truncate(count);
        }
        for state in self.continues.values_mut() {
            state.loans.truncate(count);
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
        let borrowed = callable
            .parameters()
            .iter()
            .filter(|parameter| parameter.mode() != ParameterMode::Value)
            .filter_map(|parameter| parameter.local())
            .collect();
        Analyzer::new(
            program,
            capabilities,
            CapabilityAssumptions::from_generics(program, callable.generics()),
            owners,
            borrowed,
            &mut findings,
        )
        .analyze_body(body.root())?;
    }
    for closure in program.closures() {
        let mut owners = closure
            .parameters()
            .iter()
            .filter(|parameter| parameter.mode() == ParameterMode::Value)
            .filter_map(|parameter| parameter.local())
            .collect::<BTreeSet<_>>();
        owners.extend(closure.captures().iter().map(HirClosureCapture::local));
        let borrowed = closure
            .parameters()
            .iter()
            .filter(|parameter| parameter.mode() != ParameterMode::Value)
            .filter_map(|parameter| parameter.local())
            .collect();
        let mut analyzer = Analyzer::new(
            program,
            capabilities,
            CapabilityAssumptions::from_generics(program, closure.generics()),
            owners,
            borrowed,
            &mut findings,
        );
        analyzer.reinitializable.extend(
            closure
                .captures()
                .iter()
                .filter(|capture| capture.is_mutable())
                .map(HirClosureCapture::local),
        );
        analyzer.analyze_body(closure.body().root())?;
    }
    Ok(findings.into_iter().collect())
}

/// Derives which environment slots a closure body transfers by value.
///
/// This uses the same contextual `Copy` proof and evaluation demands as the
/// whole-program availability pass. It intentionally treats only captures as
/// tracked owners: moving a parameter or a body-local value does not weaken the
/// protocols of the closure itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClosureCaptureAnalysis {
    transferred: BTreeSet<LocalId>,
    transferred_on_all_exits: BTreeSet<LocalId>,
}

impl ClosureCaptureAnalysis {
    pub(crate) fn transferred(&self) -> &BTreeSet<LocalId> {
        &self.transferred
    }

    pub(crate) fn transferred_on_all_exits(&self) -> &BTreeSet<LocalId> {
        &self.transferred_on_all_exits
    }
}

pub(crate) fn analyze_closure_captures(
    program: &HirProgram,
    capabilities: &CapabilityAnalysis,
    assumptions: CapabilityAssumptions,
    captures: &[HirClosureCapture],
    root: HirExpressionId,
) -> Result<ClosureCaptureAnalysis, TypeError> {
    let tracked = captures
        .iter()
        .map(HirClosureCapture::local)
        .collect::<BTreeSet<_>>();
    let mut findings = BTreeSet::new();
    let mut analyzer = Analyzer::new(
        program,
        capabilities,
        assumptions,
        tracked.clone(),
        BTreeSet::new(),
        &mut findings,
    );
    analyzer.tracked_transfers = tracked.clone();
    analyzer.reinitializable.extend(
        captures
            .iter()
            .filter(|capture| capture.is_mutable())
            .map(HirClosureCapture::local),
    );
    let flow = analyzer.analyze_body_flow(root)?;
    let transferred_on_all_exits = flow
        .exits
        .map(|state| state.definitely_transferred)
        .unwrap_or(tracked);
    Ok(ClosureCaptureAnalysis {
        transferred: std::mem::take(&mut analyzer.transferred),
        transferred_on_all_exits,
    })
}

struct Analyzer<'a, 'f> {
    program: &'a HirProgram,
    capabilities: &'a CapabilityAnalysis,
    assumptions: CapabilityAssumptions,
    owners: BTreeSet<LocalId>,
    borrowed: BTreeSet<LocalId>,
    pattern_borrowed: BTreeSet<LocalId>,
    reinitializable: BTreeSet<LocalId>,
    guard_forbidden: BTreeSet<LocalId>,
    tracked_transfers: BTreeSet<LocalId>,
    transferred: BTreeSet<LocalId>,
    copy_statuses: BTreeMap<TypeId, HirCapabilityStatus>,
    findings: &'f mut BTreeSet<AvailabilityFinding>,
}

impl<'a, 'f> Analyzer<'a, 'f> {
    fn new(
        program: &'a HirProgram,
        capabilities: &'a CapabilityAnalysis,
        assumptions: CapabilityAssumptions,
        owners: BTreeSet<LocalId>,
        borrowed: BTreeSet<LocalId>,
        findings: &'f mut BTreeSet<AvailabilityFinding>,
    ) -> Self {
        Self {
            program,
            capabilities,
            assumptions,
            owners,
            borrowed,
            pattern_borrowed: BTreeSet::new(),
            reinitializable: BTreeSet::new(),
            guard_forbidden: BTreeSet::new(),
            tracked_transfers: BTreeSet::new(),
            transferred: BTreeSet::new(),
            copy_statuses: BTreeMap::new(),
            findings,
        }
    }

    fn analyze_body(&mut self, root: HirExpressionId) -> Result<(), TypeError> {
        let _ = self.analyze_body_flow(root)?;
        Ok(())
    }

    fn analyze_body_flow(&mut self, root: HirExpressionId) -> Result<AvailabilityFlow, TypeError> {
        let mut flow = self.expression(root, AvailabilityState::default(), Demand::Transfer)?;
        let normal = flow.normal.take();
        merge_optional_state(&mut flow.exits, normal);
        Ok(flow)
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
            | HirExpressionKind::Receiver => AvailabilityFlow::normal(state),
            HirExpressionKind::Closure(closure) => {
                self.closure_construction(*closure, expression.span(), state)?
            }
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
            HirExpressionKind::ResultErr { error } => {
                self.expression(*error, state, Demand::Transfer)?
            }
            HirExpressionKind::Fail { error } => {
                let mut flow = self.expression(*error, state, Demand::Transfer)?;
                let exit = flow.normal.take();
                merge_optional_state(&mut flow.exits, exit);
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
                let loan_depth = flow.normal.as_ref().map_or(0, |state| state.loans.len());
                for argument in arguments {
                    let demand = if argument.mode() == ParameterMode::Value {
                        Demand::Transfer
                    } else {
                        Demand::Observe
                    };
                    flow = self.then_expression(flow, argument.value(), demand)?;
                    if argument.mode() != ParameterMode::Value
                        && let Some(state) = &mut flow.normal
                    {
                        let place = self.loan_place(argument.value());
                        self.reserve_loan(
                            state,
                            place,
                            argument.mode(),
                            self.program
                                .expression(argument.value())
                                .expect("verified call arguments remain indexed")
                                .span(),
                        );
                    }
                }
                flow.truncate_loans(loan_depth);
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
                let mut flow = self.expression(*value, state, Demand::Transfer)?;
                let exit = flow.normal.clone();
                merge_optional_state(&mut flow.exits, exit);
                flow
            }
            HirExpressionKind::If {
                condition,
                then_branch,
                else_branch,
            } => self.if_expression(*condition, *then_branch, *else_branch, state)?,
            HirExpressionKind::Match {
                scrutinee,
                mode,
                arms,
            } => self.match_expression(*scrutinee, *mode, arms, state)?,
            HirExpressionKind::Return { value } => {
                let mut flow = if let Some(value) = value {
                    self.expression(*value, state, Demand::Transfer)?
                } else {
                    AvailabilityFlow::normal(state)
                };
                let exit = flow.normal.take();
                merge_optional_state(&mut flow.exits, exit);
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
            HirStatement::Binding {
                mutable,
                pattern,
                value,
                ..
            } => {
                let mut flow = self.expression(*value, state, Demand::Transfer)?;
                if let Some(state) = &mut flow.normal {
                    self.introduce_pattern(*pattern, state, block_locals, *mutable);
                }
                Ok(flow)
            }
            HirStatement::Expression { value, .. } | HirStatement::Discard { value, .. } => {
                self.expression(*value, state, Demand::Transfer)
            }
            HirStatement::Assignment {
                operator,
                target,
                value,
                ..
            } => self.assignment(*operator, target, *value, state),
            HirStatement::For { id, kind, body, .. } => self.for_statement(*id, kind, *body, state),
        }
    }

    fn assignment(
        &mut self,
        operator: HirAssignmentOperator,
        target: &HirAssignmentTarget,
        value: HirExpressionId,
        state: AvailabilityState,
    ) -> Result<AvailabilityFlow, TypeError> {
        let mut written = Vec::new();
        collect_written_places(target, &mut written);
        let mut direct = Vec::new();
        let target_flow = self.assignment_target(
            target,
            AvailabilityFlow::normal(state),
            &mut direct,
            operator == HirAssignmentOperator::Assign,
        )?;
        let restorable = if target_flow.normal.is_some() {
            direct
        } else {
            Vec::new()
        };
        let mut flow = self.then_expression(target_flow, value, Demand::Transfer)?;
        if let Some(state) = &mut flow.normal {
            for (place, span) in written {
                let place = self.loan_place(place);
                self.check_loan_access(state, &place, LoanAccess::Write, span);
            }
            remove_locals(state, &restorable);
        }
        Ok(flow)
    }

    fn assignment_target(
        &mut self,
        target: &HirAssignmentTarget,
        mut flow: AvailabilityFlow,
        direct: &mut Vec<LocalId>,
        may_reinitialize: bool,
    ) -> Result<AvailabilityFlow, TypeError> {
        match target.kind() {
            HirAssignmentTargetKind::Place { place, write, .. } => {
                if let Some(local) = self.direct_local(*place)
                    && may_reinitialize
                    && *write == HirWriteKind::Replace
                    && self.reinitializable.contains(&local)
                {
                    direct.push(local);
                    return Ok(flow);
                }
                self.then_expression(flow, *place, Demand::Observe)
            }
            HirAssignmentTargetKind::Discard => Ok(flow),
            HirAssignmentTargetKind::Tuple(items) => {
                for item in items {
                    flow = self.assignment_target(item, flow, direct, may_reinitialize)?;
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
        mode: HirMatchMode,
        arms: &[super::HirMatchArm],
        state: AvailabilityState,
    ) -> Result<AvailabilityFlow, TypeError> {
        let has_borrow = arms
            .iter()
            .any(|arm| self.pattern_contains_borrow(arm.pattern()));
        let mut requires_affine_ownership = false;
        for arm in arms {
            requires_affine_ownership |= !self.affine_pattern_bindings(arm.pattern())?.is_empty();
        }
        let scrutinee_expression = self
            .program
            .expression(scrutinee)
            .expect("availability match scrutinees exist");
        let scrutinee_is_copy = self.is_copy(scrutinee_expression.ty())?;
        let stable = self.match_scrutinee_is_stable(scrutinee);
        let expected = if scrutinee_is_copy {
            if has_borrow && stable {
                HirMatchMode::Observe
            } else {
                HirMatchMode::Copy
            }
        } else if stable && !requires_affine_ownership {
            HirMatchMode::Observe
        } else {
            HirMatchMode::Consume
        };
        if mode != expected {
            self.findings.insert(AvailabilityFinding {
                kind: AvailabilityFindingKind::InvalidMatchMode,
                local: None,
                use_span: scrutinee_expression.span(),
                move_span: None,
            });
        }
        let demand = if mode == HirMatchMode::Consume {
            Demand::Transfer
        } else {
            Demand::Observe
        };
        let mut scrutinee_flow = self.expression(scrutinee, state, demand)?;
        let Some(mut next_entry) = scrutinee_flow.normal.take() else {
            return Ok(scrutinee_flow);
        };
        let mut result = scrutinee_flow;
        for arm in arms {
            let mut pattern_locals = Vec::new();
            let mut arm_entry = next_entry.clone();
            self.introduce_pattern(arm.pattern(), &mut arm_entry, &mut pattern_locals, false);
            let guarded_entry = if let Some(guard) = arm.guard() {
                let forbidden = self.affine_pattern_bindings(arm.pattern())?;
                self.guard_forbidden.extend(forbidden.iter().copied());
                let guard_flow = self.expression(guard, arm_entry, Demand::Transfer);
                for local in &forbidden {
                    self.guard_forbidden.remove(local);
                }
                let guard_flow = guard_flow?;
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

    fn pattern_contains_borrow(&self, root: HirPatternId) -> bool {
        let mut pending = vec![root];
        while let Some(id) = pending.pop() {
            let pattern = self
                .program
                .pattern(id)
                .expect("availability patterns retain their children");
            match pattern.kind() {
                HirPatternKind::BorrowBinding(_) => return true,
                HirPatternKind::Tuple(items) | HirPatternKind::Variant { fields: items, .. } => {
                    pending.extend(items.iter().copied());
                }
                HirPatternKind::OptionSome(item)
                | HirPatternKind::ResultOk(item)
                | HirPatternKind::ResultErr(item)
                | HirPatternKind::Newtype { value: item, .. }
                | HirPatternKind::UnionMember { pattern: item, .. } => pending.push(*item),
                HirPatternKind::Record { fields, .. } => {
                    pending.extend(fields.iter().map(super::HirPatternField::pattern));
                }
                HirPatternKind::Array { prefix, rest } => {
                    pending.extend(prefix.iter().copied());
                    pending.extend(*rest);
                }
                HirPatternKind::Recovery
                | HirPatternKind::Wildcard
                | HirPatternKind::Binding(_)
                | HirPatternKind::Literal(_)
                | HirPatternKind::OptionNone => {}
            }
        }
        false
    }

    fn match_scrutinee_is_stable(&self, id: HirExpressionId) -> bool {
        let Some(expression) = self.program.expression(id) else {
            return false;
        };
        if expression.category() != HirValueCategory::Place {
            return false;
        }
        match expression.kind() {
            HirExpressionKind::Local(_) | HirExpressionKind::Receiver => true,
            HirExpressionKind::Field { base, .. }
            | HirExpressionKind::TupleField { base, .. }
            | HirExpressionKind::Index { base, .. } => self.match_scrutinee_is_stable(*base),
            _ => false,
        }
    }

    fn affine_pattern_bindings(&mut self, root: HirPatternId) -> Result<Vec<LocalId>, TypeError> {
        let mut output = Vec::new();
        let mut pending = vec![root];
        while let Some(id) = pending.pop() {
            let pattern = self
                .program
                .pattern(id)
                .expect("availability patterns retain their children")
                .clone();
            match pattern.kind() {
                HirPatternKind::Binding(local) => {
                    if !self.is_copy(pattern.ty())? {
                        output.push(*local);
                    }
                }
                HirPatternKind::Tuple(items) | HirPatternKind::Variant { fields: items, .. } => {
                    pending.extend(items.iter().copied());
                }
                HirPatternKind::OptionSome(item)
                | HirPatternKind::ResultOk(item)
                | HirPatternKind::ResultErr(item)
                | HirPatternKind::Newtype { value: item, .. }
                | HirPatternKind::UnionMember { pattern: item, .. } => pending.push(*item),
                HirPatternKind::Record { fields, .. } => {
                    pending.extend(fields.iter().map(super::HirPatternField::pattern));
                }
                HirPatternKind::Array { prefix, rest } => {
                    pending.extend(prefix.iter().copied());
                    pending.extend(*rest);
                }
                HirPatternKind::Recovery
                | HirPatternKind::Wildcard
                | HirPatternKind::BorrowBinding(_)
                | HirPatternKind::Literal(_)
                | HirPatternKind::OptionNone => {}
            }
        }
        Ok(output)
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
                    self.introduce_pattern(pattern, &mut body_entry, &mut pattern_locals, false);
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
        let (mut flow, place) = self.place_components(id, state)?;
        if let Some(state) = &mut flow.normal {
            self.access_place(state, place, expression.span(), expression.ty(), demand)?;
        }
        Ok(self.finish_expression(id, flow))
    }

    fn place_components(
        &mut self,
        id: HirExpressionId,
        state: AvailabilityState,
    ) -> Result<(AvailabilityFlow, PlaceInfo), TypeError> {
        let expression = self
            .program
            .expression(id)
            .expect("availability analysis receives verified place IDs")
            .clone();
        match expression.kind() {
            HirExpressionKind::Local(local) => Ok((
                AvailabilityFlow::normal(state),
                PlaceInfo {
                    root: PlaceRoot::Local(*local),
                    projections: Vec::new(),
                    complete_transfer: true,
                },
            )),
            HirExpressionKind::Receiver => Ok((
                AvailabilityFlow::normal(state),
                PlaceInfo {
                    root: PlaceRoot::Receiver,
                    projections: Vec::new(),
                    complete_transfer: true,
                },
            )),
            HirExpressionKind::Field { base, member } => {
                let (flow, mut place) = self.place_base(*base, state)?;
                place.complete_transfer &= self.is_newtype(self.expression_type(*base))?;
                place.projections.push(PlaceProjection::Field(*member));
                Ok((flow, place))
            }
            HirExpressionKind::TupleField { base, index } => {
                let (flow, mut place) = self.place_base(*base, state)?;
                place.complete_transfer = false;
                place.projections.push(PlaceProjection::TupleField(*index));
                Ok((flow, place))
            }
            HirExpressionKind::Index { base, index, .. } => {
                let (flow, mut place) = self.place_base(*base, state)?;
                place.complete_transfer = false;
                place.projections.push(PlaceProjection::Collection);
                Ok((self.then_expression(flow, *index, Demand::Transfer)?, place))
            }
            HirExpressionKind::Slice {
                base,
                start,
                end,
                step,
            } => {
                let (mut flow, mut place) = self.place_base(*base, state)?;
                place.complete_transfer = false;
                place.projections.push(PlaceProjection::Collection);
                for value in start.iter().chain(end).chain(step) {
                    flow = self.then_expression(flow, *value, Demand::Transfer)?;
                }
                Ok((flow, place))
            }
            // Expression checking can preserve a recovery value as an invalid
            // assignment target. Earlier diagnostics own that error; the
            // availability pass must remain total over incomplete HIR. The HIR
            // verifier independently rejects a non-place kind marked as a
            // complete place before this analysis is trusted by MIR.
            _ => Ok((
                AvailabilityFlow::normal(state),
                PlaceInfo {
                    root: PlaceRoot::Temporary(id),
                    projections: Vec::new(),
                    complete_transfer: false,
                },
            )),
        }
    }

    fn place_base(
        &mut self,
        base: HirExpressionId,
        state: AvailabilityState,
    ) -> Result<(AvailabilityFlow, PlaceInfo), TypeError> {
        if self
            .program
            .expression(base)
            .expect("place bases reference verified expressions")
            .category()
            == HirValueCategory::Place
        {
            self.place_components(base, state)
        } else {
            Ok((
                self.expression(base, state, Demand::Transfer)?,
                PlaceInfo {
                    root: PlaceRoot::Temporary(base),
                    projections: Vec::new(),
                    complete_transfer: true,
                },
            ))
        }
    }

    fn access_place(
        &mut self,
        state: &mut AvailabilityState,
        place: PlaceInfo,
        span: Span,
        ty: TypeId,
        demand: Demand,
    ) -> Result<(), TypeError> {
        let local = match place.root {
            PlaceRoot::Local(local) => Some(local),
            PlaceRoot::Receiver | PlaceRoot::Temporary(_) => None,
        };
        if let Some(local) = local.filter(|local| self.owners.contains(local))
            && let Some(move_span) = state.unavailable.get(&local).copied()
        {
            self.findings.insert(AvailabilityFinding {
                kind: AvailabilityFindingKind::UseAfterMove,
                local: Some(local),
                use_span: span,
                move_span: Some(move_span),
            });
            return Ok(());
        }
        if let Some(local) = local.filter(|local| self.guard_forbidden.contains(local)) {
            self.findings.insert(AvailabilityFinding {
                kind: AvailabilityFindingKind::InvalidGuardAccess,
                local: Some(local),
                use_span: span,
                move_span: None,
            });
            return Ok(());
        }
        if demand == Demand::Transfer
            && local.is_some_and(|local| self.pattern_borrowed.contains(&local))
        {
            self.findings.insert(AvailabilityFinding {
                kind: AvailabilityFindingKind::InvalidBorrowedTransfer,
                local,
                use_span: span,
                move_span: None,
            });
            return Ok(());
        }
        let transfers = demand == Demand::Transfer && !self.is_copy(ty)?;
        self.check_loan_access(
            state,
            &place,
            if transfers {
                LoanAccess::Move
            } else {
                LoanAccess::Read
            },
            span,
        );
        if !transfers {
            return Ok(());
        }
        match place.root {
            PlaceRoot::Local(local) if self.owners.contains(&local) => {
                if place.complete_transfer {
                    state.unavailable.insert(local, span);
                    if self.tracked_transfers.contains(&local) {
                        self.transferred.insert(local);
                        state.definitely_transferred.insert(local);
                    }
                } else {
                    self.findings.insert(AvailabilityFinding {
                        kind: AvailabilityFindingKind::InvalidPartialTransfer,
                        local: Some(local),
                        use_span: span,
                        move_span: None,
                    });
                }
            }
            PlaceRoot::Local(local) if self.borrowed.contains(&local) => {
                self.findings.insert(AvailabilityFinding {
                    kind: AvailabilityFindingKind::InvalidBorrowedTransfer,
                    local: Some(local),
                    use_span: span,
                    move_span: None,
                });
            }
            PlaceRoot::Receiver => {
                self.findings.insert(AvailabilityFinding {
                    kind: AvailabilityFindingKind::InvalidBorrowedTransfer,
                    local: None,
                    use_span: span,
                    move_span: None,
                });
            }
            PlaceRoot::Temporary(_) if !place.complete_transfer => {
                self.findings.insert(AvailabilityFinding {
                    kind: AvailabilityFindingKind::InvalidPartialTransfer,
                    local: None,
                    use_span: span,
                    move_span: None,
                });
            }
            PlaceRoot::Local(_) | PlaceRoot::Temporary(_) => {}
        }
        Ok(())
    }

    fn loan_place(&self, id: HirExpressionId) -> PlaceInfo {
        let Some(expression) = self.program.expression(id) else {
            return PlaceInfo {
                root: PlaceRoot::Temporary(id),
                projections: Vec::new(),
                complete_transfer: false,
            };
        };
        if expression.category() != HirValueCategory::Place {
            return PlaceInfo {
                root: PlaceRoot::Temporary(id),
                projections: Vec::new(),
                complete_transfer: true,
            };
        }
        match expression.kind() {
            HirExpressionKind::Local(local) => PlaceInfo {
                root: PlaceRoot::Local(*local),
                projections: Vec::new(),
                complete_transfer: true,
            },
            HirExpressionKind::Receiver => PlaceInfo {
                root: PlaceRoot::Receiver,
                projections: Vec::new(),
                complete_transfer: true,
            },
            HirExpressionKind::Field { base, member } => {
                let mut place = self.loan_place(*base);
                place.projections.push(PlaceProjection::Field(*member));
                place
            }
            HirExpressionKind::TupleField { base, index } => {
                let mut place = self.loan_place(*base);
                place.projections.push(PlaceProjection::TupleField(*index));
                place
            }
            HirExpressionKind::Index { base, .. } | HirExpressionKind::Slice { base, .. } => {
                let mut place = self.loan_place(*base);
                place.projections.push(PlaceProjection::Collection);
                place
            }
            _ => PlaceInfo {
                root: PlaceRoot::Temporary(id),
                projections: Vec::new(),
                complete_transfer: false,
            },
        }
    }

    fn reserve_loan(
        &mut self,
        state: &mut AvailabilityState,
        place: PlaceInfo,
        mode: ParameterMode,
        span: Span,
    ) {
        if place.projections.contains(&PlaceProjection::Collection) {
            return;
        }
        for active in &state.loans {
            if places_overlap(&active.place, &place)
                && !(active.mode == ParameterMode::Ref && mode == ParameterMode::Ref)
            {
                self.findings.insert(AvailabilityFinding {
                    kind: AvailabilityFindingKind::ConflictingLoan,
                    local: place_local(&place),
                    use_span: span,
                    move_span: Some(active.span),
                });
            }
        }
        state.loans.push(LoanReservation { mode, place, span });
    }

    fn check_loan_access(
        &mut self,
        state: &AvailabilityState,
        place: &PlaceInfo,
        access: LoanAccess,
        span: Span,
    ) {
        for active in &state.loans {
            if !places_overlap(&active.place, place) {
                continue;
            }
            let compatible = access == LoanAccess::Read && active.mode == ParameterMode::Ref;
            if !compatible {
                self.findings.insert(AvailabilityFinding {
                    kind: AvailabilityFindingKind::ConflictingLoan,
                    local: place_local(place),
                    use_span: span,
                    move_span: Some(active.span),
                });
            }
        }
    }

    fn closure_construction(
        &mut self,
        closure: super::HirClosureId,
        span: Span,
        mut state: AvailabilityState,
    ) -> Result<AvailabilityFlow, TypeError> {
        let captures = self
            .program
            .closure(closure)
            .expect("availability receives verified closure metadata")
            .captures()
            .iter()
            .map(|capture| (capture.local(), capture.ty()))
            .collect::<Vec<_>>();
        for (local, ty) in captures {
            self.access_place(
                &mut state,
                PlaceInfo {
                    root: PlaceRoot::Local(local),
                    projections: Vec::new(),
                    complete_transfer: true,
                },
                span,
                ty,
                Demand::Transfer,
            )?;
        }
        Ok(AvailabilityFlow::normal(state))
    }

    fn expression_type(&self, id: HirExpressionId) -> TypeId {
        self.program
            .expression(id)
            .expect("availability expression IDs retain their types")
            .ty()
    }

    fn is_newtype(&self, ty: TypeId) -> Result<bool, TypeError> {
        let TypeKind::Nominal { identity, .. } = self.program.interner().kind(ty)? else {
            return Ok(false);
        };
        for (_, declaration) in self.program.declarations() {
            let super::HirTypeDeclarationKind::Nominal(nominal) = declaration.kind() else {
                continue;
            };
            let TypeKind::Nominal {
                identity: candidate,
                ..
            } = self.program.interner().kind(nominal.self_type())?
            else {
                continue;
            };
            if candidate == identity
                && matches!(nominal.shape(), super::HirNominalShape::Newtype { .. })
            {
                return Ok(true);
            }
        }
        Ok(false)
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
        mutable: bool,
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
                if mutable {
                    self.reinitializable.insert(*local);
                }
                remove_local(state, *local);
                locals.push(*local);
            }
            HirPatternKind::BorrowBinding(local) => {
                self.borrowed.insert(*local);
                self.pattern_borrowed.insert(*local);
                remove_local(state, *local);
                locals.push(*local);
            }
            HirPatternKind::Tuple(items) => {
                for item in items {
                    self.introduce_pattern(*item, state, locals, mutable);
                }
            }
            HirPatternKind::OptionSome(item)
            | HirPatternKind::ResultOk(item)
            | HirPatternKind::ResultErr(item)
            | HirPatternKind::UnionMember { pattern: item, .. } => {
                self.introduce_pattern(*item, state, locals, mutable);
            }
            HirPatternKind::Newtype { value, .. } => {
                self.introduce_pattern(*value, state, locals, mutable);
            }
            HirPatternKind::Variant { fields, .. } => {
                for field in fields {
                    self.introduce_pattern(*field, state, locals, mutable);
                }
            }
            HirPatternKind::Record { fields, .. } => {
                for field in fields {
                    self.introduce_pattern(field.pattern(), state, locals, mutable);
                }
            }
            HirPatternKind::Array { prefix, rest } => {
                for item in prefix {
                    self.introduce_pattern(*item, state, locals, mutable);
                }
                if let Some(rest) = rest {
                    self.introduce_pattern(*rest, state, locals, mutable);
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
    let common_loans = target
        .loans
        .iter()
        .zip(&source.loans)
        .take_while(|(left, right)| left == right)
        .count();
    target.loans.truncate(common_loans);
    for (local, origin) in source.unavailable {
        target.unavailable.entry(local).or_insert(origin);
    }
    target
        .definitely_transferred
        .retain(|local| source.definitely_transferred.contains(local));
}

fn remove_locals(state: &mut AvailabilityState, locals: &[LocalId]) {
    for local in locals {
        remove_local(state, *local);
    }
}

fn state_keys_equal(left: &AvailabilityState, right: &AvailabilityState) -> bool {
    left.unavailable.keys().eq(right.unavailable.keys())
        && left.definitely_transferred == right.definitely_transferred
        && left.loans == right.loans
}

fn remove_local(state: &mut AvailabilityState, local: LocalId) {
    state.unavailable.remove(&local);
    state.definitely_transferred.remove(&local);
}

fn place_local(place: &PlaceInfo) -> Option<LocalId> {
    match place.root {
        PlaceRoot::Local(local) => Some(local),
        PlaceRoot::Receiver | PlaceRoot::Temporary(_) => None,
    }
}

fn places_overlap(left: &PlaceInfo, right: &PlaceInfo) -> bool {
    if left.root != right.root {
        return false;
    }
    left.projections
        .iter()
        .zip(&right.projections)
        .all(|(left, right)| match (left, right) {
            (PlaceProjection::Field(left), PlaceProjection::Field(right)) => left == right,
            (PlaceProjection::TupleField(left), PlaceProjection::TupleField(right)) => {
                left == right
            }
            (PlaceProjection::Collection, _) | (_, PlaceProjection::Collection) => true,
            _ => true,
        })
}

fn collect_written_places(target: &HirAssignmentTarget, output: &mut Vec<(HirExpressionId, Span)>) {
    match target.kind() {
        HirAssignmentTargetKind::Place { place, .. } => output.push((*place, target.span())),
        HirAssignmentTargetKind::Discard => {}
        HirAssignmentTargetKind::Tuple(items) => {
            for item in items {
                collect_written_places(item, output);
            }
        }
    }
}
