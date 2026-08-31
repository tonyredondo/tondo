use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::error::Error;
use std::fmt;

use crate::hir::{
    CapabilityAnalysis, CapabilityAssumptions, HirBinaryOperator, HirBootstrapHostFunction,
    HirCallProtocol, HirCallableId, HirCapability, HirCapabilityStatus, HirClosureProtocols,
    HirContainmentKind, HirGenericParameter, HirIndexAccess, HirNominalShape, HirPrefixOperator,
    HirPreludeTraitMethod, HirProgram, HirTerminalStatus, HirTraitConstructor,
    HirTypeDeclarationKind, HirVariantPayload, StaticCollectionRegion, StaticRegionRelation,
    StaticSlice, TerminalAnalysis, static_collection_relation,
};
use crate::resolve::{MemberKind, MemberOwner, ResolvedProgram, SymbolId};
use crate::types::{
    Assignability, CursorMode, IntrinsicType, NumericConversion, ParameterMode, ScalarType, TypeId,
    TypeKind, TypeSubstitution, numeric_conversion,
};

use super::{
    MirAggregateKind, MirAwaitable, MirBasicBlock, MirBlockId, MirBlockKind, MirFunction,
    MirFunctionId, MirLoanId, MirLoanKind, MirLocalId, MirLocalKind, MirOperand, MirOperandKind,
    MirOperation, MirOperationKind, MirPlace, MirProgram, MirProjection, MirProjectionKind,
    MirRvalue, MirRvalueKind, MirSelectRegistration, MirStatement, MirStatementKind, MirTag,
    MirTerminatorKind,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirInvariantError {
    context: String,
    message: String,
    resource_limit: bool,
}

impl MirInvariantError {
    fn new(context: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            context: context.into(),
            message: message.into(),
            resource_limit: false,
        }
    }

    fn resource_limit(context: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            context: context.into(),
            message: message.into(),
            resource_limit: true,
        }
    }

    pub fn context(&self) -> &str {
        &self.context
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn is_resource_limit(&self) -> bool {
        self.resource_limit
    }
}

impl fmt::Display for MirInvariantError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "MIR invariant failed for {}: {}",
            self.context, self.message
        )
    }
}

impl Error for MirInvariantError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MirVerificationLimits {
    pub max_dataflow_steps: u64,
}

struct MirCallVerification<'a> {
    callee: &'a MirOperand,
    arguments: &'a [super::MirCallArgument],
    signature: TypeId,
    protocol: HirCallProtocol,
    unsafe_call: bool,
    outcome: TypeId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MirOperationContext {
    Immediate,
    Deferred,
    DeferredAsync,
    Await,
    Select,
    Spawn,
}

impl MirOperationContext {
    fn expects_async(self) -> bool {
        matches!(
            self,
            Self::DeferredAsync | Self::Await | Self::Select | Self::Spawn
        )
    }

    fn is_deferred(self) -> bool {
        matches!(self, Self::Deferred | Self::DeferredAsync)
    }
}

impl Default for MirVerificationLimits {
    fn default() -> Self {
        Self {
            max_dataflow_steps: 32_000_000,
        }
    }
}

pub fn verify_mir(
    resolved: &ResolvedProgram,
    hir: &HirProgram,
    program: &MirProgram,
) -> Result<(), MirInvariantError> {
    verify_mir_with_limits(resolved, hir, program, MirVerificationLimits::default())
}

pub fn verify_mir_with_limits(
    resolved: &ResolvedProgram,
    hir: &HirProgram,
    program: &MirProgram,
    limits: MirVerificationLimits,
) -> Result<(), MirInvariantError> {
    let capability_analysis = CapabilityAnalysis::new(hir, resolved).map_err(|error| {
        MirInvariantError::new(
            "MIR ownership capabilities",
            format!("cannot derive the typed HIR capability graph: {error}"),
        )
    })?;
    verify_mir_with_capability_analysis(resolved, hir, program, limits, &capability_analysis)
}

pub(crate) fn verify_mir_with_capability_analysis(
    resolved: &ResolvedProgram,
    hir: &HirProgram,
    program: &MirProgram,
    limits: MirVerificationLimits,
    capability_analysis: &CapabilityAnalysis,
) -> Result<(), MirInvariantError> {
    let expected = hir
        .callables()
        .filter(|callable| hir.body(callable.id()).is_some())
        .map(|callable| MirFunctionId::Callable(callable.id()))
        .chain(
            hir.closures()
                .map(|closure| MirFunctionId::Closure(closure.id())),
        )
        .collect::<BTreeSet<_>>();
    let actual = program.functions.keys().copied().collect::<BTreeSet<_>>();
    if actual != expected {
        return Err(MirInvariantError::new(
            "MIR program",
            "function set does not exactly match the typed HIR bodies",
        ));
    }
    let terminal_analysis = TerminalAnalysis::new(hir, resolved).map_err(|error| {
        MirInvariantError::new(
            "MIR terminal ownership",
            format!("cannot derive the typed HIR terminal graph: {error}"),
        )
    })?;
    let verifier = Verifier {
        resolved,
        hir,
        capability_analysis,
        terminal_analysis,
        capability_statuses: RefCell::new(BTreeMap::new()),
        terminal_statuses: RefCell::new(BTreeMap::new()),
        limits,
        dataflow_steps: Cell::new(0),
    };
    for (key, function) in &program.functions {
        if *key != function.id {
            return Err(MirInvariantError::new(
                function_context(*key),
                format!(
                    "map key differs from stored {}",
                    function_context(function.id)
                ),
            ));
        }
        verifier.verify_function(function)?;
    }
    Ok(())
}

struct Verifier<'a> {
    resolved: &'a ResolvedProgram,
    hir: &'a HirProgram,
    capability_analysis: &'a CapabilityAnalysis,
    terminal_analysis: TerminalAnalysis,
    capability_statuses:
        RefCell<BTreeMap<(MirFunctionId, TypeId, HirCapability), HirCapabilityStatus>>,
    terminal_statuses: RefCell<BTreeMap<(MirFunctionId, TypeId), HirTerminalStatus>>,
    limits: MirVerificationLimits,
    dataflow_steps: Cell<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LocalEvent {
    Read(LocalAccess),
    Resolve(LocalAccess),
    Move(LocalAccess),
    Write(LocalAccess),
    WriteAccess(LocalAccess),
    StorageLive(MirLocalId),
    StorageDead(MirLocalId),
}

#[derive(Debug, Clone, Copy)]
struct ClassifiedLocalAccess<'a> {
    access: &'a LocalAccess,
    kind: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LoanEvent {
    Local(LocalEvent),
    Reserve(MirLoanId),
    Release(MirLoanId),
    Consume(Vec<MirLoanId>),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct LoanFlowState {
    active: BTreeSet<MirLoanId>,
    validated: BTreeMap<MirLoanId, Vec<MirLoanId>>,
    accesses: BTreeMap<ValidatedAccess, Vec<MirLoanId>>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ValidatedAccess {
    access: LocalAccess,
    for_write: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct LocalAccess {
    local: MirLocalId,
    path: Vec<MovePathComponent>,
    source_loan: Option<MirLoanId>,
}

impl LocalAccess {
    fn from_place(place: &MirPlace) -> Self {
        Self {
            local: place.local,
            path: place
                .projections
                .iter()
                .map(MovePathComponent::from_projection)
                .collect(),
            source_loan: place.source_loan,
        }
    }
}

fn is_complete_defer_owner_place(place: &MirPlace) -> bool {
    place.source_loan.is_none()
        && (place.projections.is_empty()
            || matches!(
                place.projections.as_slice(),
                [MirProjection {
                    kind: MirProjectionKind::ClosureCapture { .. },
                    ..
                }]
            ))
}

fn is_iterator_defer_target(place: &MirPlace) -> bool {
    place.source_loan.is_none()
        && matches!(
            place.projections.as_slice(),
            [MirProjection {
                kind: MirProjectionKind::IteratorSource,
                ..
            }]
        )
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Default)]
struct DeferFlowState {
    unguarded_scopes: BTreeSet<crate::hir::HirScopeId>,
    guards: BTreeMap<LocalAccess, ActiveDeferGuard>,
    registrations: BTreeMap<(MirBlockId, usize), ActiveCleanupRegistration>,
    scope_order: Vec<crate::hir::HirScopeId>,
    pending_moves: BTreeMap<LocalAccess, PendingDeferTransition>,
    consumed: BTreeSet<LocalAccess>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct ActiveDeferGuard {
    scope: crate::hir::HirScopeId,
    registration: (MirBlockId, usize),
    kind: CleanupEntryKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct ActiveCleanupRegistration {
    scope: crate::hir::HirScopeId,
    kind: CleanupEntryKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum CleanupEntryKind {
    Explicit,
    Fallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum PendingDeferTransition {
    Retarget,
    Disarm,
}

impl DeferFlowState {
    fn scope_is_active(&self, scope: crate::hir::HirScopeId) -> bool {
        self.unguarded_scopes.contains(&scope)
            || self.guards.values().any(|candidate| {
                candidate.kind == CleanupEntryKind::Explicit && candidate.scope == scope
            })
    }

    fn activate_scope(
        &mut self,
        scope: crate::hir::HirScopeId,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        if self.scope_is_active(scope) {
            if self.scope_order.last() != Some(&scope) {
                return Err(MirInvariantError::new(
                    context,
                    "defer registration re-enters an outer scope beneath active inner entries",
                ));
            }
        } else {
            self.scope_order.push(scope);
        }
        Ok(())
    }

    fn remove_inactive_scope(&mut self, scope: crate::hir::HirScopeId) {
        if !self.scope_is_active(scope) {
            self.scope_order.retain(|candidate| *candidate != scope);
        }
    }

    fn drain(
        &mut self,
        scopes: &[crate::hir::HirScopeId],
        context: &str,
    ) -> Result<(), MirInvariantError> {
        let selected = scopes.iter().copied().collect::<BTreeSet<_>>();
        if let Some(first) = self
            .scope_order
            .iter()
            .position(|scope| selected.contains(scope))
            && self.scope_order[first..]
                .iter()
                .any(|scope| !selected.contains(scope))
        {
            return Err(MirInvariantError::new(
                context,
                "defer drain skips a still-active inner scope",
            ));
        }
        self.consumed
            .extend(self.guards.iter().filter_map(|(place, guard)| {
                (guard.kind == CleanupEntryKind::Explicit && selected.contains(&guard.scope))
                    .then_some(place.clone())
            }));
        self.unguarded_scopes
            .retain(|scope| !selected.contains(scope));
        self.guards.retain(|_, guard| {
            guard.kind != CleanupEntryKind::Explicit || !selected.contains(&guard.scope)
        });
        self.registrations.retain(|_, registration| {
            registration.kind != CleanupEntryKind::Explicit
                || !selected.contains(&registration.scope)
        });
        self.scope_order.retain(|scope| !selected.contains(scope));
        Ok(())
    }

    fn drain_unwind(&mut self) {
        self.consumed.extend(self.guards.keys().cloned());
        self.unguarded_scopes.clear();
        self.guards.clear();
        self.registrations.clear();
        self.scope_order.clear();
        self.pending_moves.clear();
    }

    fn finish_normal(&mut self, context: &str) -> Result<(), MirInvariantError> {
        let has_explicit = !self.unguarded_scopes.is_empty()
            || !self.scope_order.is_empty()
            || self
                .guards
                .values()
                .any(|guard| guard.kind == CleanupEntryKind::Explicit)
            || self
                .registrations
                .values()
                .any(|registration| registration.kind == CleanupEntryKind::Explicit)
            || !self.pending_moves.is_empty();
        if has_explicit {
            return Err(MirInvariantError::new(
                context,
                "normal return abandons an explicit defer entry",
            ));
        }
        self.guards.clear();
        self.registrations.clear();
        Ok(())
    }

    fn is_empty(&self) -> bool {
        self.unguarded_scopes.is_empty()
            && self.guards.is_empty()
            && self.registrations.is_empty()
            && self.scope_order.is_empty()
            && self.pending_moves.is_empty()
    }
}

fn defer_assignment_transition(
    function: &MirFunction,
    block: &MirBasicBlock,
    destination: &MirPlace,
    value: &MirRvalue,
    guard: &LocalAccess,
    scope: crate::hir::HirScopeId,
) -> Option<PendingDeferTransition> {
    if assignment_directly_moves(value, guard) {
        return Some(PendingDeferTransition::Retarget);
    }

    let exits_scope = block_exits_defer_scope(function, block, scope);
    let return_root =
        destination.local == function.return_local && destination.projections.is_empty();
    let confirmed_handoff = matches!(value.kind(), MirRvalueKind::Coerce { .. })
        || (return_root
            && matches!(
                value.kind(),
                MirRvalueKind::Aggregate {
                    shape: MirAggregateKind::ResultOk | MirAggregateKind::ResultErr,
                    ..
                }
            ));
    (exits_scope && confirmed_handoff).then_some(PendingDeferTransition::Disarm)
}

fn assignment_directly_moves(value: &MirRvalue, guard: &LocalAccess) -> bool {
    match value.kind() {
        MirRvalueKind::Use(MirOperand {
            kind: MirOperandKind::Move(place),
            ..
        })
        | MirRvalueKind::IteratorState {
            source:
                MirOperand {
                    kind: MirOperandKind::Move(place),
                    ..
                },
        } => LocalAccess::from_place(place) == *guard,
        _ => false,
    }
}

fn assignment_cleanup_transfer(
    destination: &MirPlace,
    value: &MirRvalue,
) -> Option<(MirPlace, MirPlace)> {
    let (from, to) = match value.kind() {
        MirRvalueKind::Use(MirOperand {
            kind: MirOperandKind::Move(from),
            ..
        }) => (from.clone(), destination.clone()),
        MirRvalueKind::IteratorState {
            source:
                MirOperand {
                    kind: MirOperandKind::Move(from),
                    ..
                },
        } => {
            let mut to = destination.clone();
            to.ty = from.ty();
            to.projections.push(MirProjection {
                ty: from.ty(),
                kind: MirProjectionKind::IteratorSource,
            });
            (from.clone(), to)
        }
        _ => return None,
    };
    (is_complete_defer_owner_place(&from)
        && (is_complete_defer_owner_place(&to) || is_iterator_defer_target(&to)))
    .then_some((from, to))
}

fn defer_disarm_is_confirmed(
    function: &MirFunction,
    block: &MirBasicBlock,
    statement: usize,
    place: &LocalAccess,
    scope: crate::hir::HirScopeId,
    pending: Option<PendingDeferTransition>,
) -> bool {
    let tail = &block.statements[statement + 1..];
    if tail.iter().any(|statement| {
        !matches!(
            &statement.kind,
            MirStatementKind::ReleaseLoan(_)
                | MirStatementKind::DisarmCleanup(_)
                | MirStatementKind::RegisterSelectArm { .. }
        )
    }) {
        return false;
    }
    let select_handoff = tail.iter().any(|statement| {
        matches!(
            &statement.kind,
            MirStatementKind::RegisterSelectArm { registration, .. }
                if select_registration_moves_defer_guard(registration, place)
        )
    });
    match pending {
        Some(PendingDeferTransition::Retarget) => false,
        Some(PendingDeferTransition::Disarm) => block_exits_defer_scope(function, block, scope),
        None => {
            terminator_moves_defer_guard(&block.terminator.kind, place)
                || preceding_assignment_copies_complete_sum_payload(block, statement, place)
                || select_handoff
                || block_exits_defer_scope(function, block, scope)
        }
    }
}

fn select_registration_moves_defer_guard(
    registration: &MirSelectRegistration,
    guard: &LocalAccess,
) -> bool {
    let MirSelectRegistration::Call(operation) = registration else {
        return false;
    };
    operation_operands(operation).into_iter().any(|operand| {
        matches!(
            operand.kind(),
            MirOperandKind::Move(place) if LocalAccess::from_place(place) == *guard
        )
    })
}

fn preceding_assignment_copies_complete_sum_payload(
    block: &MirBasicBlock,
    statement: usize,
    owner: &LocalAccess,
) -> bool {
    let Some(MirStatement {
        kind:
            MirStatementKind::Assign {
                value:
                    MirRvalue {
                        kind:
                            MirRvalueKind::Use(MirOperand {
                                kind: MirOperandKind::Copy(payload),
                                ..
                            }),
                        ..
                    },
                ..
            },
        ..
    }) = statement
        .checked_sub(1)
        .and_then(|index| block.statements.get(index))
    else {
        return false;
    };
    local_access_is_complete_sum_payload(owner, &LocalAccess::from_place(payload))
}

fn terminator_moves_defer_guard(terminator: &MirTerminatorKind, guard: &LocalAccess) -> bool {
    let operation = match terminator {
        MirTerminatorKind::Invoke { operation, .. }
        | MirTerminatorKind::Await {
            awaitable: MirAwaitable::Call(operation),
            ..
        } => operation,
        _ => return false,
    };
    operation_operands(operation).into_iter().any(|operand| {
        matches!(
            operand.kind(),
            MirOperandKind::Move(place) if LocalAccess::from_place(place) == *guard
        )
    })
}

fn block_exits_defer_scope(
    function: &MirFunction,
    block: &MirBasicBlock,
    scope: crate::hir::HirScopeId,
) -> bool {
    let drains = |terminator: &MirTerminatorKind| match terminator {
        MirTerminatorKind::DrainDefers { scopes, .. } => scopes.contains(&scope),
        MirTerminatorKind::DrainScopes {
            task_scopes,
            defer_scopes,
            ..
        } => task_scopes.contains(&scope) || defer_scopes.contains(&scope),
        _ => false,
    };
    if drains(&block.terminator.kind) {
        return true;
    }
    let MirTerminatorKind::Goto { target } = &block.terminator.kind else {
        return false;
    };
    function
        .blocks
        .get(target.0 as usize)
        .is_some_and(|target| drains(&target.terminator.kind))
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum MovePathComponent {
    ClosureCapture(crate::hir::HirClosureId, u32),
    Field(crate::resolve::MemberId),
    TupleField(u32),
    NewtypeValue,
    RefValue,
    VariantTuple(crate::resolve::MemberId, u32),
    VariantField(crate::resolve::MemberId, crate::resolve::MemberId),
    OptionValue,
    ResultOkValue,
    ResultErrValue,
    UnionValue(TypeId),
    ArrayPatternIndex(u32),
    ArrayPatternRest {
        start: u32,
        suffix: u32,
    },
    IteratorElement {
        index: MirLocalId,
    },
    IteratorSource,
    Index {
        index: MirLocalId,
        access: HirIndexAccess,
    },
    Slice {
        start: Option<MirLocalId>,
        end: Option<MirLocalId>,
        step: Option<MirLocalId>,
    },
}

impl MovePathComponent {
    fn from_projection(projection: &MirProjection) -> Self {
        match projection.kind() {
            MirProjectionKind::ClosureCapture { closure, index } => {
                Self::ClosureCapture(*closure, *index)
            }
            MirProjectionKind::Field(member) => Self::Field(*member),
            MirProjectionKind::TupleField(index) => Self::TupleField(*index),
            MirProjectionKind::NewtypeValue => Self::NewtypeValue,
            MirProjectionKind::RefValue => Self::RefValue,
            MirProjectionKind::VariantTuple { variant, index } => {
                Self::VariantTuple(*variant, *index)
            }
            MirProjectionKind::VariantField { variant, field } => {
                Self::VariantField(*variant, *field)
            }
            MirProjectionKind::OptionValue => Self::OptionValue,
            MirProjectionKind::ResultOkValue => Self::ResultOkValue,
            MirProjectionKind::ResultErrValue => Self::ResultErrValue,
            MirProjectionKind::UnionValue(member) => Self::UnionValue(*member),
            MirProjectionKind::ArrayPatternIndex(index) => Self::ArrayPatternIndex(*index),
            MirProjectionKind::ArrayPatternRest { start, suffix } => Self::ArrayPatternRest {
                start: *start,
                suffix: *suffix,
            },
            MirProjectionKind::IteratorElement { index } => Self::IteratorElement { index: *index },
            MirProjectionKind::IteratorSource => Self::IteratorSource,
            MirProjectionKind::Index { index, access } => Self::Index {
                index: *index,
                access: *access,
            },
            MirProjectionKind::Slice { start, end, step } => Self::Slice {
                start: *start,
                end: *end,
                step: *step,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LocalState {
    live: bool,
    unavailable: BTreeSet<Vec<MovePathComponent>>,
}

fn mir_operand_is_borrow(operand: &MirOperand) -> bool {
    matches!(operand.kind, MirOperandKind::Borrow(_))
}

fn mir_operand_is_loan(operand: &MirOperand) -> bool {
    matches!(operand.kind, MirOperandKind::Loan(_))
}

fn operand_place<'a>(function: &'a MirFunction, operand: &'a MirOperand) -> Option<&'a MirPlace> {
    match &operand.kind {
        MirOperandKind::Copy(place)
        | MirOperandKind::Move(place)
        | MirOperandKind::Borrow(place) => Some(place),
        MirOperandKind::Loan(loan) => function.loan(*loan).map(|loan| loan.place()),
        MirOperandKind::Constant(_)
        | MirOperandKind::Function { .. }
        | MirOperandKind::PreludeTraitFunction { .. } => None,
    }
}

fn place_is_closure_capture(
    function: &MirFunction,
    closure: crate::hir::HirClosureId,
    place: &MirPlace,
) -> bool {
    function.parameters.first() == Some(&place.local)
        && matches!(
            place.projections.first().map(|projection| &projection.kind),
            Some(MirProjectionKind::ClosureCapture {
                closure: projected,
                ..
            }) if *projected == closure
        )
}

fn access_is_closure_capture(
    function: &MirFunction,
    closure: crate::hir::HirClosureId,
    access: &LocalAccess,
) -> bool {
    closure_capture_access_index(function, closure, access).is_some()
}

fn closure_capture_access_index(
    function: &MirFunction,
    closure: crate::hir::HirClosureId,
    access: &LocalAccess,
) -> Option<u32> {
    (function.parameters.first() == Some(&access.local))
        .then(|| match access.path.first() {
            Some(MovePathComponent::ClosureCapture(projected, index)) if *projected == closure => {
                Some(*index)
            }
            _ => None,
        })
        .flatten()
}

fn closure_capture_transfer_index(
    function: &MirFunction,
    closure: crate::hir::HirClosureId,
    access: &LocalAccess,
) -> Option<u32> {
    let index = closure_capture_access_index(function, closure, access)?;
    access.path[1..]
        .iter()
        .all(|component| matches!(component, MovePathComponent::NewtypeValue))
        .then_some(index)
}

fn mir_rvalue_contains_invalid_borrow(value: &MirRvalue) -> bool {
    let escapes =
        |operand: &MirOperand| mir_operand_is_borrow(operand) || mir_operand_is_loan(operand);
    match &value.kind {
        MirRvalueKind::Use(value)
        | MirRvalueKind::Prefix { operand: value, .. }
        | MirRvalueKind::Coerce { value, .. }
        | MirRvalueKind::NumericConversion { value, .. } => escapes(value),
        MirRvalueKind::Binary {
            left,
            right,
            operator: HirBinaryOperator::Equal | HirBinaryOperator::NotEqual,
        } => mir_operand_is_loan(left) || mir_operand_is_loan(right),
        MirRvalueKind::Contains {
            item, container, ..
        } => mir_operand_is_loan(item) || mir_operand_is_loan(container),
        MirRvalueKind::MapRemove { key, .. } => escapes(key),
        MirRvalueKind::Interpolate { values, .. } => values.iter().any(escapes),
        MirRvalueKind::Length(operand) => mir_operand_is_loan(operand),
        MirRvalueKind::IteratorState { source } => mir_operand_is_loan(source),
        MirRvalueKind::Binary { left, right, .. }
        | MirRvalueKind::Range {
            start: left,
            end: right,
            ..
        } => escapes(left) || escapes(right),
        MirRvalueKind::Aggregate { values, .. } => values.iter().any(escapes),
        MirRvalueKind::RecordUpdate { base, fields } => {
            escapes(base) || fields.iter().any(|(_, value)| escapes(value))
        }
    }
}

fn mir_operation_contains_invalid_borrow(operation: &MirOperation) -> bool {
    let escapes =
        |operand: &MirOperand| mir_operand_is_borrow(operand) || mir_operand_is_loan(operand);
    match &operation.kind {
        MirOperationKind::CheckedPrefix { operand, .. }
        | MirOperationKind::ExplicitPanic { message: operand } => escapes(operand),
        MirOperationKind::CheckedBinary { left, right, .. } => escapes(left) || escapes(right),
        MirOperationKind::ArraySequence {
            array, argument, ..
        } => mir_operand_is_loan(array) || escapes(argument),
        MirOperationKind::BuildMap { entries, .. } => entries
            .iter()
            .any(|(key, value)| escapes(key) || escapes(value)),
        MirOperationKind::Index { base, index, .. } => mir_operand_is_loan(base) || escapes(index),
        MirOperationKind::Slice { base, bounds, .. } => {
            mir_operand_is_loan(base)
                || bounds
                    .start
                    .iter()
                    .chain(&bounds.end)
                    .chain(&bounds.step)
                    .any(escapes)
        }
        MirOperationKind::Call {
            callee, arguments, ..
        } => {
            mir_operand_is_loan(callee)
                || arguments.iter().any(|argument| {
                    if argument.mode == crate::types::ParameterMode::Value {
                        escapes(&argument.value)
                    } else {
                        !mir_operand_is_loan(&argument.value)
                    }
                })
        }
        MirOperationKind::Assert {
            condition,
            message_parts,
            ..
        } => escapes(condition) || message_parts.iter().any(|part| escapes(&part.value)),
        MirOperationKind::BootstrapHostCall { arguments, .. } => arguments.iter().any(escapes),
        MirOperationKind::Format { value, display } => {
            escapes(value) || display.as_ref().is_some_and(escapes)
        }
        MirOperationKind::JoinFormat {
            values,
            separator,
            display,
        } => escapes(values) || escapes(separator) || display.as_ref().is_some_and(escapes),
    }
}

fn operation_operands(operation: &MirOperation) -> Vec<&MirOperand> {
    let mut operands = Vec::new();
    match operation.kind() {
        MirOperationKind::CheckedPrefix { operand, .. }
        | MirOperationKind::ExplicitPanic { message: operand } => operands.push(operand),
        MirOperationKind::CheckedBinary { left, right, .. }
        | MirOperationKind::ArraySequence {
            array: left,
            argument: right,
            ..
        }
        | MirOperationKind::Index {
            base: left,
            index: right,
            ..
        } => {
            operands.push(left);
            operands.push(right);
        }
        MirOperationKind::BuildMap { entries, .. } => {
            for (key, value) in entries {
                operands.push(key);
                operands.push(value);
            }
        }
        MirOperationKind::Slice { base, bounds, .. } => {
            operands.push(base);
            operands.extend(
                bounds
                    .start()
                    .into_iter()
                    .chain(bounds.end())
                    .chain(bounds.step()),
            );
        }
        MirOperationKind::Call {
            callee, arguments, ..
        } => {
            operands.push(callee);
            operands.extend(arguments.iter().map(super::MirCallArgument::value));
        }
        MirOperationKind::Assert {
            condition,
            message_parts,
            ..
        } => {
            operands.push(condition);
            operands.extend(message_parts.iter().map(super::MirAssertMessagePart::value));
        }
        MirOperationKind::BootstrapHostCall { arguments, .. } => operands.extend(arguments),
        MirOperationKind::Format { value, display } => {
            operands.push(value);
            if let Some(display) = display {
                operands.push(display);
            }
        }
        MirOperationKind::JoinFormat {
            values,
            separator,
            display,
        } => {
            operands.push(values);
            operands.push(separator);
            if let Some(display) = display {
                operands.push(display);
            }
        }
    }
    operands
}

#[derive(Debug, Clone)]
struct SuccessorEdge {
    target: MirBlockId,
    refinement: Option<TagFact>,
    writes: Option<MirPlace>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TagFact {
    place: MirPlace,
    tag: MirTag,
}

#[derive(Debug, Clone)]
enum TagEvent {
    Require(TagFact),
    Write(MirPlace),
}

impl Verifier<'_> {
    fn verify_function(&self, function: &MirFunction) -> Result<(), MirInvariantError> {
        let context = function_context(function.id);
        self.verify_span(function, function.span, &context)?;
        let (expected_outcome, expected_parameters) = match function.id {
            MirFunctionId::Callable(id) => {
                let signature = self.hir.callable(id).ok_or_else(|| {
                    MirInvariantError::new(&context, "function has no typed HIR callable")
                })?;
                (
                    signature.outcome(),
                    signature
                        .parameters()
                        .iter()
                        .map(|parameter| (parameter.ty(), parameter.local()))
                        .collect::<Vec<_>>(),
                )
            }
            MirFunctionId::Closure(id) => {
                let closure = self.hir.closure(id).ok_or_else(|| {
                    MirInvariantError::new(&context, "function has no typed HIR closure")
                })?;
                let TypeKind::Function(signature) = self
                    .hir
                    .interner()
                    .kind(closure.function_type())
                    .map_err(|error| MirInvariantError::new(&context, error.to_string()))?
                else {
                    return Err(MirInvariantError::new(
                        &context,
                        "closure function has a non-function HIR signature",
                    ));
                };
                let mut parameters = Vec::with_capacity(closure.parameters().len() + 1);
                parameters.push((closure.ty(), None));
                parameters.extend(
                    closure
                        .parameters()
                        .iter()
                        .map(|parameter| (parameter.ty(), parameter.local())),
                );
                (signature.outcome(), parameters)
            }
        };
        if expected_outcome != function.outcome {
            return Err(MirInvariantError::new(
                &context,
                format!(
                    "outcome is {}, typed HIR requires {}",
                    function.outcome, expected_outcome
                ),
            ));
        }
        self.verify_type(function.outcome, &context)?;
        if function.locals.is_empty() {
            return Err(MirInvariantError::new(&context, "local table is empty"));
        }
        let return_local = self.local(function, function.return_local, &context)?;
        if return_local.kind != MirLocalKind::Return || return_local.ty != function.outcome {
            return Err(MirInvariantError::new(
                &context,
                "return local kind or type does not match the function outcome",
            ));
        }
        if function.parameters.len() != expected_parameters.len() {
            return Err(MirInvariantError::new(
                &context,
                format!(
                    "{} MIR parameters for {} typed HIR parameters",
                    function.parameters.len(),
                    expected_parameters.len()
                ),
            ));
        }
        let mut parameter_locals = BTreeSet::new();
        for (index, (local_id, (expected_type, expected_source))) in function
            .parameters
            .iter()
            .zip(&expected_parameters)
            .enumerate()
        {
            if !parameter_locals.insert(*local_id) {
                return Err(MirInvariantError::new(
                    &context,
                    format!("parameter local#{} is repeated", local_id.index()),
                ));
            }
            let local = self.local(function, *local_id, &context)?;
            if local.ty != *expected_type
                || local.kind
                    != (MirLocalKind::Parameter {
                        index: index as u32,
                        source: *expected_source,
                    })
            {
                return Err(MirInvariantError::new(
                    &context,
                    format!("parameter {index} local metadata does not match typed HIR"),
                ));
            }
        }
        let mut user_locals = BTreeSet::new();
        let mut return_count = 0_usize;
        for (index, local) in function.locals.iter().enumerate() {
            self.verify_type(local.ty, &format!("{context} local#{index}"))?;
            self.verify_span(function, local.span, &format!("{context} local#{index}"))?;
            match local.kind {
                MirLocalKind::Return => return_count += 1,
                MirLocalKind::Parameter { .. } => {
                    if !parameter_locals.contains(&MirLocalId(index as u32)) {
                        return Err(MirInvariantError::new(
                            &context,
                            format!("local#{index} is an unlisted parameter"),
                        ));
                    }
                }
                MirLocalKind::User(source) => {
                    let expected = self.hir.local_type(source).ok_or_else(|| {
                        MirInvariantError::new(
                            &context,
                            format!("user local#{index} references an untyped HIR local"),
                        )
                    })?;
                    if local.ty != expected
                        || self.resolved.local(source).is_none()
                        || !user_locals.insert(source)
                    {
                        return Err(MirInvariantError::new(
                            &context,
                            format!(
                                "user local#{index} has inconsistent or duplicate source identity"
                            ),
                        ));
                    }
                }
                MirLocalKind::Temporary => {}
            }
        }
        if return_count != 1 {
            return Err(MirInvariantError::new(
                &context,
                format!("function has {return_count} return locals instead of one"),
            ));
        }
        for (index, loan) in function.loans.iter().enumerate() {
            let loan_context = format!("{context} loan#{index}");
            if loan.mode == ParameterMode::Value {
                return Err(MirInvariantError::new(
                    &loan_context,
                    "loan metadata uses the owning value mode",
                ));
            }
            match loan.kind {
                MirLoanKind::CallLocal => {}
                MirLoanKind::Region => {}
            }
            if let Some(source) = loan.place.source_loan
                && source.index() as usize >= index
            {
                return Err(MirInvariantError::new(
                    &loan_context,
                    "loan source region is not an earlier acyclic reservation",
                ));
            }
            self.verify_place(function, &loan.place, &loan_context)?;
            if loan.mode != ParameterMode::Ref && place_contains_ref_value(&loan.place) {
                return Err(MirInvariantError::new(
                    &loan_context,
                    "`Ref[T].value` permits only shared `ref` loans",
                ));
            }
            if loan.kind == MirLoanKind::Region
                && matches!(loan.mode, ParameterMode::Mut | ParameterMode::Var)
            {
                self.verify_exclusive_iterator_loan_path(function, &loan.place, &loan_context)?;
            }
        }
        if function.blocks.is_empty() {
            return Err(MirInvariantError::new(
                &context,
                "basic-block table is empty",
            ));
        }
        let entry = self.block(function, function.entry, &context)?;
        if function.entry == function.unwind {
            return Err(MirInvariantError::new(
                &context,
                "entry and unwind blocks are identical",
            ));
        }
        if entry.kind != MirBlockKind::Normal {
            return Err(MirInvariantError::new(
                &context,
                "entry block is cleanup code",
            ));
        }
        let unwind = self.block(function, function.unwind, &context)?;
        if unwind.kind != MirBlockKind::Cleanup
            || !matches!(unwind.terminator.kind, MirTerminatorKind::ResumePanic)
        {
            return Err(MirInvariantError::new(
                &context,
                "unwind entry is not a cleanup block ending in ResumePanic",
            ));
        }
        for (index, block) in function.blocks.iter().enumerate() {
            self.verify_block(function, MirBlockId(index as u32), block)?;
        }
        self.verify_control_and_dataflow(function)?;
        self.verify_defer_flow(function, &context)?;
        self.verify_select_flow(function, &context)?;
        self.verify_task_scope_flow(function, &context)?;
        self.verify_suspension_liveness(function, &context)?;
        if let MirFunctionId::Closure(closure) = function.id {
            self.verify_closure_protocols(function, closure, &context)?;
        }
        Ok(())
    }

    fn verify_task_scope_flow(
        &self,
        function: &MirFunction,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        let mut incoming = vec![None::<Vec<crate::hir::HirScopeId>>; function.blocks.len()];
        incoming[function.entry.index() as usize] = Some(Vec::new());
        let mut queue = VecDeque::from([function.entry]);
        let mut queued = vec![false; function.blocks.len()];
        queued[function.entry.index() as usize] = true;
        while let Some(block_id) = queue.pop_front() {
            queued[block_id.index() as usize] = false;
            self.consume_dataflow_step(context)?;
            let Some(mut scopes) = incoming[block_id.index() as usize].clone() else {
                continue;
            };
            let block = &function.blocks[block_id.index() as usize];
            if block.kind != MirBlockKind::Normal {
                continue;
            }
            let block_context = format!("{context} block#{}", block_id.index());
            for statement in &block.statements {
                if let MirStatementKind::EnterTaskScope { scope } = statement.kind() {
                    if scopes.contains(scope) {
                        return Err(MirInvariantError::new(
                            &block_context,
                            "task scope is re-entered before its previous extent is drained",
                        ));
                    }
                    scopes.push(*scope);
                }
            }
            if let MirTerminatorKind::Spawn { scope, .. } = block.terminator.kind()
                && scopes.last() != Some(scope)
            {
                return Err(MirInvariantError::new(
                    &block_context,
                    "spawn is not owned by the innermost active task scope",
                ));
            }
            if let MirTerminatorKind::DrainScopes { task_scopes, .. } = block.terminator.kind() {
                let start = scopes.len().checked_sub(task_scopes.len()).ok_or_else(|| {
                    MirInvariantError::new(
                        &block_context,
                        "structured drain removes more task scopes than are active",
                    )
                })?;
                if scopes[start..] != *task_scopes {
                    return Err(MirInvariantError::new(
                        &block_context,
                        "structured drain does not remove the exact active task-scope suffix",
                    ));
                }
                scopes.truncate(start);
            }
            if matches!(block.terminator.kind(), MirTerminatorKind::Return) && !scopes.is_empty() {
                return Err(MirInvariantError::new(
                    &block_context,
                    "normal return abandons active task scopes",
                ));
            }
            for edge in successor_edges(block.terminator.kind()) {
                if function.blocks[edge.target.index() as usize].kind != MirBlockKind::Normal {
                    continue;
                }
                let target = edge.target.index() as usize;
                match &incoming[target] {
                    Some(previous) if previous != &scopes => {
                        return Err(MirInvariantError::new(
                            format!("{context} block#{}", edge.target.index()),
                            "control-flow predecessors disagree about active task scopes",
                        ));
                    }
                    Some(_) => {}
                    None => {
                        incoming[target] = Some(scopes.clone());
                        if !queued[target] {
                            queued[target] = true;
                            queue.push_back(edge.target);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn verify_suspension_liveness(
        &self,
        function: &MirFunction,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        let mut uses = vec![BTreeSet::<MirLocalId>::new(); function.blocks.len()];
        let mut definitions = vec![BTreeSet::<MirLocalId>::new(); function.blocks.len()];
        for (index, block) in function.blocks.iter().enumerate() {
            for event in self.local_events(function, block) {
                match event {
                    LocalEvent::Read(access)
                    | LocalEvent::Resolve(access)
                    | LocalEvent::WriteAccess(access) => {
                        if !definitions[index].contains(&access.local) {
                            uses[index].insert(access.local);
                        }
                    }
                    LocalEvent::Move(access) => {
                        if !definitions[index].contains(&access.local) {
                            uses[index].insert(access.local);
                        }
                        if access.path.is_empty() {
                            definitions[index].insert(access.local);
                        }
                    }
                    LocalEvent::Write(access) => {
                        if !access.path.is_empty() && !definitions[index].contains(&access.local) {
                            uses[index].insert(access.local);
                        }
                        if access.path.is_empty() {
                            definitions[index].insert(access.local);
                        }
                    }
                    LocalEvent::StorageLive(local) | LocalEvent::StorageDead(local) => {
                        definitions[index].insert(local);
                    }
                }
            }
        }
        let mut live_in = vec![BTreeSet::<MirLocalId>::new(); function.blocks.len()];
        let mut live_out = live_in.clone();
        loop {
            self.consume_dataflow_step(context)?;
            let mut changed = false;
            for index in (0..function.blocks.len()).rev() {
                let block = &function.blocks[index];
                let mut outgoing = BTreeSet::new();
                for edge in successor_edges(block.terminator.kind()) {
                    let mut edge_live = live_in[edge.target.index() as usize].clone();
                    if let Some(destination) = edge.writes
                        && destination.projections.is_empty()
                    {
                        edge_live.remove(&destination.local);
                    }
                    outgoing.extend(edge_live);
                }
                let mut incoming = uses[index].clone();
                incoming.extend(
                    outgoing
                        .iter()
                        .filter(|local| !definitions[index].contains(local))
                        .copied(),
                );
                if live_out[index] != outgoing {
                    live_out[index] = outgoing;
                    changed = true;
                }
                if live_in[index] != incoming {
                    live_in[index] = incoming;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        for (index, block) in function.blocks.iter().enumerate() {
            if !matches!(
                block.terminator.kind(),
                MirTerminatorKind::Await { .. } | MirTerminatorKind::DrainScopes { .. }
            ) {
                continue;
            }
            let block_context = format!("{context} block#{index}");
            for local in &live_out[index] {
                let ty = function.locals[local.index() as usize].ty;
                if matches!(
                    self.kind(ty, &block_context)?,
                    TypeKind::Intrinsic {
                        constructor: IntrinsicType::Join,
                        ..
                    }
                ) {
                    continue;
                }
                self.require_capability(function.id, ty, HirCapability::Send, &block_context)?;
            }
        }
        Ok(())
    }

    fn verify_closure_protocols(
        &self,
        function: &MirFunction,
        closure: crate::hir::HirClosureId,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        let metadata = self.hir.closure(closure).ok_or_else(|| {
            MirInvariantError::new(context, "closure body has no typed HIR metadata")
        })?;
        let mut writes_capture = false;
        let mut moves_capture = false;
        for block in &function.blocks {
            for event in self.local_events(function, block) {
                match event {
                    LocalEvent::Move(access)
                        if access_is_closure_capture(function, closure, &access) =>
                    {
                        moves_capture = true;
                    }
                    LocalEvent::Write(access)
                        if access_is_closure_capture(function, closure, &access) =>
                    {
                        writes_capture = true;
                    }
                    LocalEvent::Read(_)
                    | LocalEvent::Resolve(_)
                    | LocalEvent::Move(_)
                    | LocalEvent::Write(_)
                    | LocalEvent::WriteAccess(_)
                    | LocalEvent::StorageLive(_)
                    | LocalEvent::StorageDead(_) => {}
                }
            }
            if let MirTerminatorKind::Invoke {
                operation:
                    MirOperation {
                        kind:
                            MirOperationKind::Call {
                                callee,
                                arguments,
                                protocol,
                                ..
                            },
                        ..
                    },
                ..
            } = &block.terminator.kind
            {
                writes_capture |= *protocol == HirCallProtocol::CallMut
                    && operand_place(function, callee)
                        .is_some_and(|place| place_is_closure_capture(function, closure, place));
                writes_capture |= arguments.iter().any(|argument| {
                    matches!(argument.mode, ParameterMode::Mut | ParameterMode::Var)
                        && operand_place(function, &argument.value)
                            .is_some_and(|place| place_is_closure_capture(function, closure, place))
                });
            }
        }
        let mut required_transfers = BTreeSet::new();
        for (index, capture) in metadata.captures().iter().enumerate() {
            if self.capability_status(function.id, capture.ty(), HirCapability::Discard, context)?
                != HirCapabilityStatus::Satisfied
            {
                required_transfers.insert(u32::try_from(index).map_err(|_| {
                    MirInvariantError::new(context, "closure capture index exceeds MIR limits")
                })?);
            }
        }
        let transferred_on_all_returns =
            self.closure_captures_transferred_on_all_returns(function, closure, context)?;
        let derived = HirClosureProtocols::new(
            !writes_capture && !moves_capture,
            !moves_capture && (!metadata.is_async() || !writes_capture),
            required_transfers.is_subset(&transferred_on_all_returns),
        );
        if metadata.protocols() != derived {
            return Err(MirInvariantError::new(
                context,
                "closure protocols differ from the lowered environment accesses",
            ));
        }
        Ok(())
    }

    fn closure_captures_transferred_on_all_returns(
        &self,
        function: &MirFunction,
        closure: crate::hir::HirClosureId,
        context: &str,
    ) -> Result<BTreeSet<u32>, MirInvariantError> {
        let all = self
            .hir
            .closure(closure)
            .expect("closure protocol verification has HIR metadata")
            .captures()
            .iter()
            .enumerate()
            .map(|(index, _)| {
                u32::try_from(index).map_err(|_| {
                    MirInvariantError::new(context, "closure capture index exceeds MIR limits")
                })
            })
            .collect::<Result<BTreeSet<_>, _>>()?;
        let mut incoming = vec![None::<BTreeSet<u32>>; function.blocks.len()];
        incoming[function.entry.index() as usize] = Some(BTreeSet::new());
        let mut queue = VecDeque::from([function.entry]);
        let mut queued = vec![false; function.blocks.len()];
        queued[function.entry.index() as usize] = true;
        let mut returns = None::<BTreeSet<u32>>;

        while let Some(block_id) = queue.pop_front() {
            queued[block_id.index() as usize] = false;
            self.consume_dataflow_step(context)?;
            let Some(mut state) = incoming[block_id.index() as usize].clone() else {
                continue;
            };
            let block = &function.blocks[block_id.index() as usize];
            if block.kind != MirBlockKind::Normal {
                continue;
            }
            for event in self.local_events(function, block) {
                match event {
                    LocalEvent::Move(access) => {
                        if let Some(index) =
                            closure_capture_transfer_index(function, closure, &access)
                        {
                            state.insert(index);
                        }
                    }
                    LocalEvent::Write(access) => {
                        if let Some(index) =
                            closure_capture_access_index(function, closure, &access)
                        {
                            state.remove(&index);
                        }
                    }
                    LocalEvent::Read(_)
                    | LocalEvent::Resolve(_)
                    | LocalEvent::WriteAccess(_)
                    | LocalEvent::StorageLive(_)
                    | LocalEvent::StorageDead(_) => {}
                }
            }
            if matches!(block.terminator.kind, MirTerminatorKind::Return) {
                intersect_optional_set(&mut returns, state);
                continue;
            }
            for edge in successor_edges(&block.terminator.kind) {
                if function.blocks[edge.target.index() as usize].kind != MirBlockKind::Normal {
                    continue;
                }
                let mut edge_state = state.clone();
                if let Some(index) = edge.writes.as_ref().and_then(|place| {
                    closure_capture_access_index(function, closure, &LocalAccess::from_place(place))
                }) {
                    edge_state.remove(&index);
                }
                let changed =
                    intersect_incoming_set(&mut incoming[edge.target.index() as usize], edge_state);
                if changed && !queued[edge.target.index() as usize] {
                    queued[edge.target.index() as usize] = true;
                    queue.push_back(edge.target);
                }
            }
        }

        Ok(returns.unwrap_or(all))
    }

    fn verify_block(
        &self,
        function: &MirFunction,
        id: MirBlockId,
        block: &MirBasicBlock,
    ) -> Result<(), MirInvariantError> {
        let context = format!("{} block#{}", function_context(function.id), id.index());
        for statement in &block.statements {
            self.verify_span(function, statement.span, &context)?;
            match &statement.kind {
                MirStatementKind::StorageLive(local) | MirStatementKind::StorageDead(local) => {
                    self.local(function, *local, &context)?;
                    if matches!(
                        function.locals[local.0 as usize].kind,
                        MirLocalKind::Return | MirLocalKind::Parameter { .. }
                    ) {
                        return Err(MirInvariantError::new(
                            &context,
                            "return and parameter locals have function-wide storage",
                        ));
                    }
                }
                MirStatementKind::ReserveLoan(loan) | MirStatementKind::ReleaseLoan(loan) => {
                    if block.kind != MirBlockKind::Normal {
                        return Err(MirInvariantError::new(
                            &context,
                            "cleanup block manipulates a loan reservation",
                        ));
                    }
                    self.loan(function, *loan, &context)?;
                }
                MirStatementKind::EnterTaskScope { .. } => {
                    if block.kind != MirBlockKind::Normal
                        || !self.function_is_async(function, &context)?
                    {
                        return Err(MirInvariantError::new(
                            &context,
                            "task scope is entered outside ordinary async code",
                        ));
                    }
                }
                MirStatementKind::Assign { destination, value } => {
                    self.verify_place(function, destination, &context)?;
                    self.verify_rvalue(function, value, &context)?;
                    if place_contains_ref_value(destination) {
                        return Err(MirInvariantError::new(
                            &context,
                            "`Ref[T].value` is a read-only projection",
                        ));
                    }
                    if destination.ty != value.ty {
                        return Err(MirInvariantError::new(
                            &context,
                            format!(
                                "assignment writes {} into destination {}",
                                value.ty, destination.ty
                            ),
                        ));
                    }
                }
                MirStatementKind::RegisterDefer { action, guard, .. } => {
                    if block.kind != MirBlockKind::Normal {
                        return Err(MirInvariantError::new(
                            &context,
                            "cleanup block registers another defer",
                        ));
                    }
                    if let MirOperationKind::Call {
                        callee, protocol, ..
                    } = &action.kind
                        && matches!(callee.kind(), MirOperandKind::Move(_))
                        && *protocol != crate::hir::HirCallProtocol::CallOnce
                    {
                        return Err(MirInvariantError::new(
                            &context,
                            "a non-Copy deferred callee does not use CallOnce",
                        ));
                    }
                    let operation_context = match &action.kind {
                        MirOperationKind::Call { signature, .. }
                            if matches!(
                                self.kind(*signature, &context)?,
                                TypeKind::Function(function) if function.is_async()
                            ) =>
                        {
                            MirOperationContext::DeferredAsync
                        }
                        _ => MirOperationContext::Deferred,
                    };
                    self.verify_operation(function, action, operation_context, &context)?;
                    if action.ty != self.hir.interner().scalar(ScalarType::Unit)
                        || !matches!(
                            action.kind,
                            MirOperationKind::Call { .. }
                                | MirOperationKind::Assert { .. }
                                | MirOperationKind::BootstrapHostCall { .. }
                        )
                    {
                        return Err(MirInvariantError::new(
                            &context,
                            "defer entry is not an infallible Unit invocation",
                        ));
                    }
                    if operation_operands(action).iter().any(|operand| {
                        matches!(
                            operand.kind(),
                            MirOperandKind::Borrow(_) | MirOperandKind::Loan(_)
                        )
                    }) {
                        return Err(MirInvariantError::new(
                            &context,
                            "defer entry retains a borrowed operand",
                        ));
                    }
                    let moved = operation_operands(action)
                        .into_iter()
                        .filter_map(|operand| match operand.kind() {
                            MirOperandKind::Move(place) => Some(place),
                            _ => None,
                        })
                        .collect::<Vec<_>>();
                    if moved.len() > 1 || guard.is_some() != (moved.len() == 1) {
                        return Err(MirInvariantError::new(
                            &context,
                            "defer entry does not have exactly one guard for its affine operand",
                        ));
                    }
                    if let Some(guard) = guard {
                        self.verify_place(function, guard, &context)?;
                        if !is_complete_defer_owner_place(guard) {
                            return Err(MirInvariantError::new(
                                &context,
                                "registered defer guard is not one complete owner place",
                            ));
                        }
                        if moved[0] != guard {
                            return Err(MirInvariantError::new(
                                &context,
                                "defer guard does not match exactly one moved invocation operand",
                            ));
                        }
                    }
                }
                MirStatementKind::RegisterFallback { owner, .. } => {
                    if block.kind != MirBlockKind::Normal {
                        return Err(MirInvariantError::new(
                            &context,
                            "cleanup block registers a terminal fallback",
                        ));
                    }
                    self.verify_place(function, owner, &context)?;
                    if owner.source_loan.is_some()
                        || self.terminal_status(function.id, owner.ty, &context)?
                            == HirTerminalStatus::Absent
                    {
                        return Err(MirInvariantError::new(
                            &context,
                            "terminal fallback owner is borrowed or has no terminal token",
                        ));
                    }
                }
                MirStatementKind::RetargetCleanup { from, to } => {
                    if block.kind != MirBlockKind::Normal {
                        return Err(MirInvariantError::new(
                            &context,
                            "cleanup block retargets a defer guard",
                        ));
                    }
                    self.verify_place(function, from, &context)?;
                    self.verify_place(function, to, &context)?;
                    if from.ty != to.ty
                        || !is_complete_defer_owner_place(from)
                        || !(is_complete_defer_owner_place(to) || is_iterator_defer_target(to))
                    {
                        return Err(MirInvariantError::new(
                            &context,
                            "defer retarget does not preserve one complete owner place",
                        ));
                    }
                }
                MirStatementKind::DisarmCleanup(place) => {
                    if block.kind != MirBlockKind::Normal {
                        return Err(MirInvariantError::new(
                            &context,
                            "cleanup block explicitly disarms a defer guard",
                        ));
                    }
                    self.verify_place(function, place, &context)?;
                }
                MirStatementKind::BeginSelect { capacity } => {
                    if block.kind != MirBlockKind::Normal
                        || !self.function_is_async(function, &context)?
                    {
                        return Err(MirInvariantError::new(
                            &context,
                            "select registration appears outside ordinary async code",
                        ));
                    }
                    if *capacity == 0 || *capacity > crate::mir::MAX_SELECT_ARMS {
                        return Err(MirInvariantError::new(
                            &context,
                            format!(
                                "select region declares {capacity} arms outside the checked bound 1..={}",
                                crate::mir::MAX_SELECT_ARMS
                            ),
                        ));
                    }
                }
                MirStatementKind::RegisterSelectArm { registration, .. } => {
                    if block.kind != MirBlockKind::Normal
                        || !self.function_is_async(function, &context)?
                    {
                        return Err(MirInvariantError::new(
                            &context,
                            "select registration appears outside ordinary async code",
                        ));
                    }
                    match registration {
                        MirSelectRegistration::Call(operation) => {
                            self.verify_operation(
                                function,
                                operation,
                                MirOperationContext::Select,
                                &context,
                            )?;
                        }
                        MirSelectRegistration::Join(place) => {
                            self.verify_place(function, place, &context)?;
                            let kind = self.hir.interner().kind(place.ty).map_err(|error| {
                                MirInvariantError::new(&context, error.to_string())
                            })?;
                            if !matches!(
                                kind,
                                TypeKind::Intrinsic {
                                    constructor: IntrinsicType::Join,
                                    ..
                                }
                            ) {
                                return Err(MirInvariantError::new(
                                    &context,
                                    "select registered a non-Join handle",
                                ));
                            }
                        }
                    }
                }
            }
        }
        self.verify_span(function, block.terminator.span, &context)?;
        match &block.terminator.kind {
            MirTerminatorKind::Goto { target } => {
                let target_block = self.block(function, *target, &context)?;
                if target_block.kind != block.kind {
                    return Err(MirInvariantError::new(
                        &context,
                        "Goto crosses the ordinary/cleanup boundary",
                    ));
                }
            }
            MirTerminatorKind::SwitchBool {
                condition,
                if_true,
                if_false,
            } => {
                if block.kind != MirBlockKind::Normal {
                    return Err(MirInvariantError::new(
                        &context,
                        "cleanup block performs an ordinary boolean branch",
                    ));
                }
                self.verify_operand(function, condition, &context)?;
                if mir_operand_is_borrow(condition)
                    || mir_operand_is_loan(condition)
                    || condition.ty != self.hir.interner().scalar(ScalarType::Bool)
                {
                    return Err(MirInvariantError::new(
                        &context,
                        "SwitchBool condition is not a materialized Bool",
                    ));
                }
                self.normal_block(function, *if_true, &context)?;
                self.normal_block(function, *if_false, &context)?;
            }
            MirTerminatorKind::SwitchTag {
                value,
                cases,
                otherwise,
            } => {
                if block.kind != MirBlockKind::Normal {
                    return Err(MirInvariantError::new(
                        &context,
                        "cleanup block performs an ordinary tag branch",
                    ));
                }
                self.verify_operand(function, value, &context)?;
                if !matches!(
                    value.kind,
                    MirOperandKind::Copy(_) | MirOperandKind::Move(_) | MirOperandKind::Borrow(_)
                ) {
                    return Err(MirInvariantError::new(
                        &context,
                        "SwitchTag value is not materialized in a place",
                    ));
                }
                if cases.is_empty() {
                    return Err(MirInvariantError::new(
                        &context,
                        "SwitchTag has no explicit cases",
                    ));
                }
                let mut tags = BTreeSet::new();
                for (tag, target) in cases {
                    if !tags.insert(tag) {
                        return Err(MirInvariantError::new(
                            &context,
                            format!("switch tag {tag:?} is duplicated"),
                        ));
                    }
                    self.verify_tag(value.ty, tag, &context)?;
                    self.normal_block(function, *target, &context)?;
                }
                self.normal_block(function, *otherwise, &context)?;
            }
            MirTerminatorKind::Invoke {
                operation,
                destination,
                target,
                unwind,
            } => {
                if block.kind != MirBlockKind::Normal {
                    return Err(MirInvariantError::new(
                        &context,
                        "cleanup block invokes an ordinary fallible operation",
                    ));
                }
                self.verify_operation(
                    function,
                    operation,
                    MirOperationContext::Immediate,
                    &context,
                )?;
                let never = self.hir.interner().scalar(ScalarType::Never);
                match (destination, target) {
                    (Some(destination), Some(target)) => {
                        self.verify_place(function, destination, &context)?;
                        if place_contains_ref_value(destination) {
                            return Err(MirInvariantError::new(
                                &context,
                                "`Ref[T].value` is a read-only projection",
                            ));
                        }
                        if destination.ty != operation.ty || operation.ty == never {
                            return Err(MirInvariantError::new(
                                &context,
                                "invoke destination does not match its normal result",
                            ));
                        }
                        self.normal_block(function, *target, &context)?;
                    }
                    (None, None) if operation.ty == never => {}
                    _ => {
                        return Err(MirInvariantError::new(
                            &context,
                            "invoke must have both destination and target, or neither for Never",
                        ));
                    }
                }
                let unwind_block = self.block(function, *unwind, &context)?;
                if unwind_block.kind != MirBlockKind::Cleanup {
                    return Err(MirInvariantError::new(
                        &context,
                        "invoke unwind edge does not enter cleanup code",
                    ));
                }
            }
            MirTerminatorKind::Await {
                awaitable,
                destination,
                target,
                unwind,
            } => {
                if block.kind != MirBlockKind::Normal
                    || !self.function_is_async(function, &context)?
                {
                    return Err(MirInvariantError::new(
                        &context,
                        "await appears outside ordinary async code",
                    ));
                }
                self.verify_place(function, destination, &context)?;
                if place_contains_ref_value(destination) {
                    return Err(MirInvariantError::new(
                        &context,
                        "`Ref[T].value` is a read-only projection",
                    ));
                }
                let expected = match awaitable {
                    MirAwaitable::Call(operation) => {
                        self.verify_operation(
                            function,
                            operation,
                            MirOperationContext::Await,
                            &context,
                        )?;
                        operation.ty
                    }
                    MirAwaitable::Join(join) => {
                        self.verify_operand(function, join, &context)?;
                        if !matches!(join.kind(), MirOperandKind::Move(_)) {
                            return Err(MirInvariantError::new(
                                &context,
                                "await must consume its affine Join operand",
                            ));
                        }
                        self.join_logical_outcome(join.ty(), &context)?
                    }
                };
                if destination.ty != expected {
                    return Err(MirInvariantError::new(
                        &context,
                        "await destination differs from its logical outcome",
                    ));
                }
                self.normal_block(function, *target, &context)?;
                if self.block(function, *unwind, &context)?.kind != MirBlockKind::Cleanup {
                    return Err(MirInvariantError::new(
                        &context,
                        "await unwind edge does not enter cleanup code",
                    ));
                }
            }
            MirTerminatorKind::CommitSelect {
                arms,
                else_target,
                unwind,
            } => {
                if block.kind != MirBlockKind::Normal
                    || !self.function_is_async(function, &context)?
                {
                    return Err(MirInvariantError::new(
                        &context,
                        "select commit appears outside ordinary async code",
                    ));
                }
                if arms.is_empty() || arms.len() > crate::mir::MAX_SELECT_ARMS as usize {
                    return Err(MirInvariantError::new(
                        &context,
                        format!(
                            "commit table has {} arms outside the checked bound 1..={}",
                            arms.len(),
                            crate::mir::MAX_SELECT_ARMS
                        ),
                    ));
                }
                for arm in arms {
                    if let Some(payload) = arm.payload() {
                        self.verify_place(function, payload, &context)?;
                        if place_contains_ref_value(payload) {
                            return Err(MirInvariantError::new(
                                &context,
                                "`Ref[T].value` is a read-only projection",
                            ));
                        }
                    }
                    self.normal_block(function, arm.target(), &context)?;
                }
                if let Some(else_target) = else_target {
                    self.normal_block(function, *else_target, &context)?;
                }
                if self.block(function, *unwind, &context)?.kind != MirBlockKind::Cleanup {
                    return Err(MirInvariantError::new(
                        &context,
                        "select commit unwind edge does not enter cleanup code",
                    ));
                }
            }
            MirTerminatorKind::Spawn {
                operation,
                destination,
                target,
                unwind,
                ..
            } => {
                if block.kind != MirBlockKind::Normal
                    || !self.function_is_async(function, &context)?
                {
                    return Err(MirInvariantError::new(
                        &context,
                        "spawn appears outside ordinary async code",
                    ));
                }
                self.verify_operation(function, operation, MirOperationContext::Spawn, &context)?;
                self.verify_spawn_transfer(function, operation, &context)?;
                self.verify_place(function, destination, &context)?;
                if place_contains_ref_value(destination)
                    || !self.is_join_for_outcome(destination.ty, operation.ty, &context)?
                {
                    return Err(MirInvariantError::new(
                        &context,
                        "spawn destination is not its exact writable Join result",
                    ));
                }
                self.normal_block(function, *target, &context)?;
                if self.block(function, *unwind, &context)?.kind != MirBlockKind::Cleanup {
                    return Err(MirInvariantError::new(
                        &context,
                        "spawn unwind edge does not enter cleanup code",
                    ));
                }
            }
            MirTerminatorKind::IteratorNext {
                state,
                destination,
                borrowed_source,
                exhaustion_guard,
                has_value,
                exhausted,
                unwind,
            } => {
                if block.kind != MirBlockKind::Normal {
                    return Err(MirInvariantError::new(
                        &context,
                        "cleanup block advances an iterator",
                    ));
                }
                self.verify_place(function, state, &context)?;
                self.verify_place(function, destination, &context)?;
                if place_contains_ref_value(destination) {
                    return Err(MirInvariantError::new(
                        &context,
                        "`Ref[T].value` is a read-only projection",
                    ));
                }
                let TypeKind::Cursor { mode, collection } = self.kind(state.ty, &context)? else {
                    return Err(MirInvariantError::new(
                        &context,
                        "iterator state is not a concrete intrinsic cursor",
                    ));
                };
                match mode {
                    CursorMode::Own => {
                        if borrowed_source.is_some() {
                            return Err(MirInvariantError::new(
                                &context,
                                "owning iterator carries a borrowed source",
                            ));
                        }
                        self.verify_iterator(state.ty, destination.ty, &context)?;
                        let terminal = self.terminal_status(function.id, *collection, &context)?
                            != HirTerminalStatus::Absent;
                        if terminal != exhaustion_guard.is_some() {
                            return Err(MirInvariantError::new(
                                &context,
                                "owning iterator exhaustion guard does not match contextual terminal ownership",
                            ));
                        }
                        if let Some(guard) = exhaustion_guard {
                            if has_value == exhausted {
                                return Err(MirInvariantError::new(
                                    &context,
                                    "guarded iterator uses the same value and exhaustion edge",
                                ));
                            }
                            self.verify_place(function, guard, &context)?;
                            let mut expected = state.clone();
                            expected.ty = *collection;
                            expected.projections.push(MirProjection {
                                ty: *collection,
                                kind: MirProjectionKind::IteratorSource,
                            });
                            if guard != &expected {
                                return Err(MirInvariantError::new(
                                    &context,
                                    "iterator exhaustion guard is not its exact owned source",
                                ));
                            }
                        }
                    }
                    CursorMode::Ref | CursorMode::Mut => {
                        if exhaustion_guard.is_some() {
                            return Err(MirInvariantError::new(
                                &context,
                                "borrowed iterator carries an owning exhaustion guard",
                            ));
                        }
                        let source = borrowed_source.as_ref().ok_or_else(|| {
                            MirInvariantError::new(
                                &context,
                                "borrowed iterator has no source place",
                            )
                        })?;
                        self.verify_place(function, source, &context)?;
                        if *mode == CursorMode::Mut
                            && !matches!(
                                self.kind(*collection, &context)?,
                                TypeKind::Intrinsic {
                                    constructor: IntrinsicType::Array | IntrinsicType::Map,
                                    ..
                                }
                            )
                        {
                            return Err(MirInvariantError::new(
                                &context,
                                "exclusive iterator source is not an Array or Map",
                            ));
                        }
                        if source.ty != *collection
                            || destination.ty != self.hir.interner().scalar(ScalarType::Int)
                            || source.source_loan.is_none()
                            || self
                                .iterated_borrowed_item_type(*collection, &context)?
                                .is_none()
                        {
                            return Err(MirInvariantError::new(
                                &context,
                                "borrowed iterator source, position, or collection type is inconsistent",
                            ));
                        }
                        self.verify_borrowed_iterator_origin(
                            function,
                            state,
                            destination,
                            source,
                            *mode,
                            &context,
                        )?;
                    }
                }
                self.normal_block(function, *has_value, &context)?;
                self.normal_block(function, *exhausted, &context)?;
                if self.block(function, *unwind, &context)?.kind != MirBlockKind::Cleanup {
                    return Err(MirInvariantError::new(
                        &context,
                        "iterator unwind edge does not enter cleanup code",
                    ));
                }
            }
            MirTerminatorKind::ValidatePlaces {
                places,
                replacements,
                against,
                for_write,
                target,
                unwind,
            } => {
                if block.kind != MirBlockKind::Normal
                    || places.is_empty()
                    || places.len() != replacements.len()
                    || places.len() != against.len()
                {
                    return Err(MirInvariantError::new(
                        &context,
                        "place validation must be a non-empty aligned ordinary operation",
                    ));
                }
                let mut unique = Vec::new();
                for ((place, replacement), against) in places.iter().zip(replacements).zip(against)
                {
                    self.verify_place(function, place, &context)?;
                    if *for_write && place_contains_ref_value(place) {
                        return Err(MirInvariantError::new(
                            &context,
                            "`Ref[T].value` is a read-only projection",
                        ));
                    }
                    if unique.contains(&place) {
                        return Err(MirInvariantError::new(
                            &context,
                            "place validation repeats the same destination",
                        ));
                    }
                    unique.push(place);
                    let mut previous = None;
                    for loan in against {
                        self.loan(function, *loan, &context)?;
                        if previous.is_some_and(|previous| previous >= *loan) {
                            return Err(MirInvariantError::new(
                                &context,
                                "place validation conflicts are not unique canonical IDs",
                            ));
                        }
                        previous = Some(*loan);
                    }
                    match (*for_write, replacement) {
                        (false, None) => {}
                        (true, Some(replacement)) => {
                            self.verify_operand(function, replacement, &context)?;
                            if replacement.ty() != place.ty()
                                || !matches!(replacement.kind(), MirOperandKind::Borrow(_))
                            {
                                return Err(MirInvariantError::new(
                                    &context,
                                    "write validation requires a borrowed replacement of the place type",
                                ));
                            }
                        }
                        _ => {
                            return Err(MirInvariantError::new(
                                &context,
                                "place validation replacement shape disagrees with its mode",
                            ));
                        }
                    }
                }
                self.normal_block(function, *target, &context)?;
                if self.block(function, *unwind, &context)?.kind != MirBlockKind::Cleanup {
                    return Err(MirInvariantError::new(
                        &context,
                        "place-validation unwind edge does not enter cleanup code",
                    ));
                }
            }
            MirTerminatorKind::ValidateLoan {
                loan,
                against,
                target,
                unwind,
            } => {
                if block.kind != MirBlockKind::Normal {
                    return Err(MirInvariantError::new(
                        &context,
                        "cleanup block validates a loan reservation",
                    ));
                }
                let loan_metadata = self.loan(function, *loan, &context)?;
                if !place_requires_loan_validation(loan_metadata.place()) {
                    return Err(MirInvariantError::new(
                        &context,
                        "loan validation has no index or slice projection",
                    ));
                }
                let mut previous = None;
                for candidate in against {
                    self.loan(function, *candidate, &context)?;
                    if candidate == loan || previous.is_some_and(|previous| previous >= *candidate)
                    {
                        return Err(MirInvariantError::new(
                            &context,
                            "loan validation conflicts are not unique canonical active IDs",
                        ));
                    }
                    previous = Some(*candidate);
                }
                let target_block = self.block(function, *target, &context)?;
                if target_block.kind != MirBlockKind::Normal
                    || !matches!(
                        target_block.statements.first().map(|statement| &statement.kind),
                        Some(MirStatementKind::ReserveLoan(candidate)) if candidate == loan
                    )
                {
                    return Err(MirInvariantError::new(
                        &context,
                        "loan-validation success does not immediately reserve the same loan",
                    ));
                }
                if self.block(function, *unwind, &context)?.kind != MirBlockKind::Cleanup {
                    return Err(MirInvariantError::new(
                        &context,
                        "loan-validation unwind edge does not enter cleanup code",
                    ));
                }
            }
            MirTerminatorKind::DrainDefers {
                scopes,
                target,
                unwind,
            } => {
                if scopes.is_empty()
                    || scopes.iter().copied().collect::<BTreeSet<_>>().len() != scopes.len()
                {
                    return Err(MirInvariantError::new(
                        &context,
                        "defer drain has an empty or duplicate scope set",
                    ));
                }
                let target_block = self.block(function, *target, &context)?;
                let unwind_block = self.block(function, *unwind, &context)?;
                if target_block.kind != block.kind || unwind_block.kind != MirBlockKind::Cleanup {
                    return Err(MirInvariantError::new(
                        &context,
                        "defer drain crosses an invalid normal or unwind boundary",
                    ));
                }
                if !block.statements.is_empty() {
                    return Err(MirInvariantError::new(
                        &context,
                        "defer drain block contains ordinary statements",
                    ));
                }
            }
            MirTerminatorKind::DrainScopes {
                task_scopes,
                defer_scopes,
                target,
                unwind,
            } => {
                if task_scopes.is_empty() && defer_scopes.is_empty() {
                    return Err(MirInvariantError::new(
                        &context,
                        "structured drain has no task or defer scopes",
                    ));
                }
                if task_scopes.iter().copied().collect::<BTreeSet<_>>().len() != task_scopes.len()
                    || defer_scopes.iter().copied().collect::<BTreeSet<_>>().len()
                        != defer_scopes.len()
                {
                    return Err(MirInvariantError::new(
                        &context,
                        "structured drain repeats a task or defer scope",
                    ));
                }
                if !task_scopes.is_empty() && !self.function_is_async(function, &context)? {
                    return Err(MirInvariantError::new(
                        &context,
                        "task scopes are drained by a synchronous function",
                    ));
                }
                let target_block = self.block(function, *target, &context)?;
                let unwind_block = self.block(function, *unwind, &context)?;
                if target_block.kind != block.kind
                    || unwind_block.kind != MirBlockKind::Cleanup
                    || !block.statements.is_empty()
                {
                    return Err(MirInvariantError::new(
                        &context,
                        "structured drain crosses an invalid boundary or contains statements",
                    ));
                }
            }
            MirTerminatorKind::DrainUnwind { target } => {
                if block.kind != MirBlockKind::Cleanup
                    || *target != function.unwind
                    || !block.statements.is_empty()
                {
                    return Err(MirInvariantError::new(
                        &context,
                        "unwind drain is not an empty cleanup block targeting the function unwind",
                    ));
                }
                if self.block(function, *target, &context)?.kind != MirBlockKind::Cleanup {
                    return Err(MirInvariantError::new(
                        &context,
                        "unwind drain target is not cleanup code",
                    ));
                }
            }
            MirTerminatorKind::Return => {
                if block.kind != MirBlockKind::Normal {
                    return Err(MirInvariantError::new(
                        &context,
                        "cleanup block returns normally",
                    ));
                }
                if function.outcome == self.hir.interner().scalar(ScalarType::Never) {
                    return Err(MirInvariantError::new(
                        &context,
                        "Never function has a normal return",
                    ));
                }
            }
            MirTerminatorKind::ResumePanic => {
                if block.kind != MirBlockKind::Cleanup {
                    return Err(MirInvariantError::new(
                        &context,
                        "ordinary block resumes panic unwinding",
                    ));
                }
            }
            MirTerminatorKind::Unreachable => {}
        }
        Ok(())
    }

    fn verify_rvalue(
        &self,
        function: &MirFunction,
        value: &MirRvalue,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        if mir_rvalue_contains_invalid_borrow(value) {
            return Err(MirInvariantError::new(
                context,
                "borrow escapes its permitted immediate observation",
            ));
        }
        self.verify_type(value.ty, context)?;
        match &value.kind {
            MirRvalueKind::Use(operand) => {
                self.verify_operand(function, operand, context)?;
                if operand.ty != value.ty {
                    return Err(MirInvariantError::new(
                        context,
                        "Use rvalue changes its operand type",
                    ));
                }
            }
            MirRvalueKind::Prefix { operator, operand } => {
                self.verify_operand(function, operand, context)?;
                self.verify_prefix(*operator, operand.ty, value.ty, context)?;
                if self.prefix_requires_checked(*operator, operand.ty) {
                    return Err(MirInvariantError::new(
                        context,
                        "potentially panicking prefix operation is not an Invoke",
                    ));
                }
            }
            MirRvalueKind::Binary {
                operator,
                left,
                right,
            } => {
                self.verify_operand(function, left, context)?;
                self.verify_operand(function, right, context)?;
                self.verify_binary(*operator, left.ty, right.ty, value.ty, context)?;
                if self.binary_requires_checked(*operator, left.ty, right.ty) {
                    return Err(MirInvariantError::new(
                        context,
                        "potentially panicking binary operation is not an Invoke",
                    ));
                }
            }
            MirRvalueKind::Aggregate { shape, values } => {
                for operand in values {
                    self.verify_operand(function, operand, context)?;
                }
                self.verify_aggregate(function, shape, values, value.ty, context)?;
            }
            MirRvalueKind::RecordUpdate { base, fields } => {
                self.verify_operand(function, base, context)?;
                if base.ty != value.ty {
                    return Err(MirInvariantError::new(
                        context,
                        "record update changes the nominal base type",
                    ));
                }
                let mut seen = BTreeSet::new();
                for (member, operand) in fields {
                    if self.resolved.member(*member).is_none() || !seen.insert(*member) {
                        return Err(MirInvariantError::new(
                            context,
                            "record update contains an unknown or duplicate field",
                        ));
                    }
                    self.verify_operand(function, operand, context)?;
                    if !self.nominal_field_matches(value.ty, *member, operand.ty, context)? {
                        return Err(MirInvariantError::new(
                            context,
                            "record update value does not match its instantiated field type",
                        ));
                    }
                }
            }
            MirRvalueKind::Coerce {
                kind,
                value: operand,
            } => {
                self.verify_operand(function, operand, context)?;
                let actual = match kind {
                    Assignability::Opaque => {
                        let mut interner = self.hir.interner().clone();
                        self.hir
                            .opaque_coercion_matches(&mut interner, operand.ty, value.ty)
                            .map_err(|error| {
                                MirInvariantError::new(
                                    context,
                                    format!("cannot validate opaque MIR coercion: {error}"),
                                )
                            })?
                            .then_some(Assignability::Opaque)
                    }
                    Assignability::CallableErasure => self
                        .callable_erasure_matches(operand.ty, value.ty, context)?
                        .then_some(Assignability::CallableErasure),
                    Assignability::CallableOnceErasure => self
                        .callable_once_erasure_matches(operand.ty, value.ty, context)?
                        .then_some(Assignability::CallableOnceErasure),
                    _ => self
                        .hir
                        .interner()
                        .assignability(operand.ty, value.ty)
                        .map_err(|error| {
                            MirInvariantError::new(
                                context,
                                format!("cannot validate MIR coercion: {error}"),
                            )
                        })?,
                };
                if actual != Some(*kind) || *kind == Assignability::Exact {
                    return Err(MirInvariantError::new(
                        context,
                        "coercion kind does not match the closed assignability relation",
                    ));
                }
            }
            MirRvalueKind::NumericConversion {
                target,
                conversion,
                value: operand,
            } => {
                self.verify_operand(function, operand, context)?;
                self.verify_numeric_conversion(
                    operand.ty,
                    *target,
                    *conversion,
                    value.ty,
                    context,
                )?;
            }
            MirRvalueKind::Range { start, end, .. } => {
                self.verify_operand(function, start, context)?;
                self.verify_operand(function, end, context)?;
                let element = self.intrinsic_arguments(value.ty, IntrinsicType::Range, context)?;
                if start.ty != end.ty || element != [start.ty] {
                    return Err(MirInvariantError::new(
                        context,
                        "range bounds or result element type are inconsistent",
                    ));
                }
            }
            MirRvalueKind::Contains {
                kind,
                item,
                container,
            } => {
                self.verify_operand(function, item, context)?;
                self.verify_operand(function, container, context)?;
                self.verify_contains(*kind, item.ty, container.ty, value.ty, context)?;
            }
            MirRvalueKind::MapRemove { map, key } => {
                self.verify_place(function, map, context)?;
                self.verify_operand(function, key, context)?;
                let arguments = self.intrinsic_arguments(map.ty, IntrinsicType::Map, context)?;
                let TypeKind::Option(result) = self.kind(value.ty, context)? else {
                    return Err(MirInvariantError::new(
                        context,
                        "Map.remove result is not Option[V]",
                    ));
                };
                let source = map.source_loan.and_then(|source| function.loan(source));
                if key.ty != arguments[0]
                    || *result != arguments[1]
                    || !source.is_some_and(|source| {
                        source.kind() == MirLoanKind::Region
                            && source.mode() == ParameterMode::Var
                            && same_place_path(source.place(), map)
                    })
                {
                    return Err(MirInvariantError::new(
                        context,
                        "Map.remove receiver, key, result, or exclusive region is inconsistent",
                    ));
                }
            }
            MirRvalueKind::Interpolate { segments, values } => {
                let string = self.hir.interner().scalar(ScalarType::String);
                if value.ty != string || segments.len() != values.len() + 1 {
                    return Err(MirInvariantError::new(
                        context,
                        "interpolation must produce String with one more segment than value",
                    ));
                }
                for operand in values {
                    self.verify_operand(function, operand, context)?;
                    if operand.ty != string {
                        return Err(MirInvariantError::new(
                            context,
                            "interpolation received a non-String Display result",
                        ));
                    }
                }
            }
            MirRvalueKind::Length(operand) => {
                self.verify_operand(function, operand, context)?;
                if value.ty != self.hir.interner().scalar(ScalarType::Int)
                    || (!self.is_array(operand.ty)
                        && operand.ty != self.hir.interner().scalar(ScalarType::String))
                {
                    return Err(MirInvariantError::new(
                        context,
                        "length requires Array or String and produces Int",
                    ));
                }
            }
            MirRvalueKind::IteratorState { source } => {
                self.verify_operand(function, source, context)?;
                let TypeKind::Cursor { mode, collection } = self.kind(value.ty, context)? else {
                    return Err(MirInvariantError::new(
                        context,
                        "iterator state result is not a concrete intrinsic cursor",
                    ));
                };
                let borrows = matches!(source.kind, MirOperandKind::Borrow(_));
                if *collection != source.ty
                    || (*mode != CursorMode::Own) != borrows
                    || self.iterated_item_type(source.ty).is_none()
                {
                    return Err(MirInvariantError::new(
                        context,
                        "iterator state does not wrap exactly one iterable source type",
                    ));
                }
            }
        }
        Ok(())
    }

    fn verify_format_operation(
        &self,
        function: &MirFunction,
        value: &MirOperand,
        separator: Option<&MirOperand>,
        display: Option<&MirOperand>,
        operation_ty: TypeId,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        self.verify_operand(function, value, context)?;
        let string = self.hir.interner().scalar(ScalarType::String);
        let format_error = {
            let mut interner = self.hir.interner().clone();
            interner
                .intrinsic(IntrinsicType::FormatError, Vec::new())
                .map_err(|error| MirInvariantError::new(context, error.to_string()))?
        };
        let TypeKind::Result { success, error } = self.kind(operation_ty, context)? else {
            return Err(MirInvariantError::new(
                context,
                "format operation must produce Result[String, FormatError]",
            ));
        };
        if *success != string || *error != format_error {
            return Err(MirInvariantError::new(
                context,
                "format operation result is not Result[String, FormatError]",
            ));
        }

        let display_target = match separator {
            None => value.ty,
            Some(separator) => {
                self.verify_operand(function, separator, context)?;
                if separator.ty != string {
                    return Err(MirInvariantError::new(
                        context,
                        "format.join separator is not String",
                    ));
                }
                let arguments =
                    self.intrinsic_arguments(value.ty, IntrinsicType::Array, context)?;
                arguments[0]
            }
        };
        let mut interner = self.hir.interner().clone();
        let intrinsic = HirPreludeTraitMethod::Display
            .has_intrinsic_implementation(&interner, &[display_target])
            .map_err(|error| MirInvariantError::new(context, error.to_string()))?;
        match (intrinsic, display) {
            (true, None) => {}
            (false, Some(callback)) => {
                self.verify_operand(function, callback, context)?;
                let expected = HirPreludeTraitMethod::Display
                    .function_type(&mut interner, &[display_target])
                    .map_err(|error| MirInvariantError::new(context, error.to_string()))?
                    .ok_or_else(|| {
                        MirInvariantError::new(
                            context,
                            "format Display callback has no closed function type",
                        )
                    })?;
                if callback.ty != expected
                    || !matches!(
                        &callback.kind,
                        MirOperandKind::PreludeTraitFunction {
                            method: HirPreludeTraitMethod::Display,
                            arguments,
                        } if arguments.as_slice() == [display_target]
                    )
                {
                    return Err(MirInvariantError::new(
                        context,
                        "format Display callback does not match its target",
                    ));
                }
            }
            (true, Some(_)) => {
                return Err(MirInvariantError::new(
                    context,
                    "intrinsic Display format operation must not carry a callback",
                ));
            }
            (false, None) => {
                return Err(MirInvariantError::new(
                    context,
                    "non-intrinsic Display format operation is missing its callback",
                ));
            }
        }
        Ok(())
    }

    fn verify_operation(
        &self,
        function: &MirFunction,
        operation: &MirOperation,
        operation_context: MirOperationContext,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        if mir_operation_contains_invalid_borrow(operation) {
            return Err(MirInvariantError::new(
                context,
                "borrow escapes its permitted immediate operation",
            ));
        }
        self.verify_type(operation.ty, context)?;
        if operation_context.expects_async()
            && !matches!(operation.kind(), MirOperationKind::Call { .. })
        {
            return Err(MirInvariantError::new(
                context,
                "async initiation does not contain exactly one call operation",
            ));
        }
        match &operation.kind {
            MirOperationKind::CheckedPrefix { operator, operand } => {
                self.verify_operand(function, operand, context)?;
                self.verify_prefix(*operator, operand.ty, operation.ty, context)?;
                if !self.prefix_requires_checked(*operator, operand.ty) {
                    return Err(MirInvariantError::new(
                        context,
                        "non-panicking prefix operation is encoded as Invoke",
                    ));
                }
            }
            MirOperationKind::CheckedBinary {
                operator,
                left,
                right,
            } => {
                self.verify_operand(function, left, context)?;
                self.verify_operand(function, right, context)?;
                self.verify_binary(*operator, left.ty, right.ty, operation.ty, context)?;
                if !self.binary_requires_checked(*operator, left.ty, right.ty) {
                    return Err(MirInvariantError::new(
                        context,
                        "non-panicking binary operation is encoded as Invoke",
                    ));
                }
            }
            MirOperationKind::ArraySequence {
                kind,
                array,
                argument,
            } => {
                self.verify_operand(function, array, context)?;
                self.verify_operand(function, argument, context)?;
                let elements =
                    self.intrinsic_arguments(operation.ty, IntrinsicType::Array, context)?;
                if array.ty != operation.ty
                    || !matches!(array.kind, MirOperandKind::Borrow(_))
                    || self.capability_status(
                        function.id,
                        elements[0],
                        HirCapability::Copy,
                        context,
                    )? != HirCapabilityStatus::Satisfied
                {
                    return Err(MirInvariantError::new(
                        context,
                        "Array sequence operation requires a borrowed Array[T: Copy] receiver",
                    ));
                }
                let expected = match kind {
                    crate::hir::HirArraySequenceKind::Concat => operation.ty,
                    crate::hir::HirArraySequenceKind::Repeat => {
                        self.hir.interner().scalar(ScalarType::Int)
                    }
                };
                if argument.ty != expected {
                    return Err(MirInvariantError::new(
                        context,
                        "Array sequence argument differs from its closed signature",
                    ));
                }
            }
            MirOperationKind::BuildMap { entries, .. } => {
                let arguments =
                    self.intrinsic_arguments(operation.ty, IntrinsicType::Map, context)?;
                for (key, value) in entries {
                    self.verify_operand(function, key, context)?;
                    self.verify_operand(function, value, context)?;
                    if key.ty != arguments[0] || value.ty != arguments[1] {
                        return Err(MirInvariantError::new(
                            context,
                            "map entry does not match the map key/value types",
                        ));
                    }
                }
            }
            MirOperationKind::Index {
                base,
                index,
                access,
                against,
            } => {
                self.verify_operand(function, base, context)?;
                self.verify_operand(function, index, context)?;
                self.verify_index_result(base.ty, index.ty, *access, operation.ty, context)?;
                if *access == HirIndexAccess::String && !against.is_empty() {
                    return Err(MirInvariantError::new(
                        context,
                        "String indexing cannot carry runtime place conflicts",
                    ));
                }
                self.verify_runtime_conflict_ids(function, against, context)?;
                let _ = operation_access_place(operation, context)?;
            }
            MirOperationKind::Slice {
                base,
                bounds,
                against,
            } => {
                self.verify_operand(function, base, context)?;
                for operand in bounds.start.iter().chain(&bounds.end).chain(&bounds.step) {
                    self.verify_operand(function, operand, context)?;
                    if operand.ty != self.hir.interner().scalar(ScalarType::Int) {
                        return Err(MirInvariantError::new(context, "slice bound is not Int"));
                    }
                }
                let is_array = self.is_array(base.ty);
                let is_string = base.ty == self.hir.interner().scalar(ScalarType::String);
                if operation.ty != base.ty || !(is_array || is_string) {
                    return Err(MirInvariantError::new(
                        context,
                        "slice operation must preserve its Array or String type",
                    ));
                }
                if is_string && !against.is_empty() {
                    return Err(MirInvariantError::new(
                        context,
                        "String slicing cannot carry runtime place conflicts",
                    ));
                }
                if is_array
                    && self.capability_status(
                        function.id,
                        operation.ty,
                        HirCapability::Copy,
                        context,
                    )? != HirCapabilityStatus::Satisfied
                {
                    return Err(MirInvariantError::new(
                        context,
                        "slice operation materializes a non-Copy Array",
                    ));
                }
                self.verify_runtime_conflict_ids(function, against, context)?;
                let _ = operation_access_place(operation, context)?;
            }
            MirOperationKind::Call {
                callee,
                arguments,
                signature,
                protocol,
                unsafe_call,
            } => {
                self.verify_operand(function, callee, context)?;
                for argument in arguments {
                    if argument.target == crate::hir::HirCallArgumentTarget::Invalid {
                        return Err(MirInvariantError::new(
                            context,
                            "call operation retains an invalid argument association",
                        ));
                    }
                    self.verify_operand(function, &argument.value, context)?;
                }
                self.verify_call(
                    function,
                    MirCallVerification {
                        callee,
                        arguments,
                        signature: *signature,
                        protocol: *protocol,
                        unsafe_call: *unsafe_call,
                        outcome: operation.ty,
                    },
                    operation_context,
                    context,
                )?;
                if operation_context == MirOperationContext::DeferredAsync {
                    self.require_capability(function.id, callee.ty, HirCapability::Send, context)?;
                    for argument in arguments {
                        self.require_capability(
                            function.id,
                            argument.value.ty,
                            HirCapability::Send,
                            context,
                        )?;
                    }
                }
            }
            MirOperationKind::Format { value, display } => {
                self.verify_format_operation(
                    function,
                    value,
                    None,
                    display.as_ref(),
                    operation.ty,
                    context,
                )?;
            }
            MirOperationKind::JoinFormat {
                values,
                separator,
                display,
            } => {
                self.verify_format_operation(
                    function,
                    values,
                    Some(separator),
                    display.as_ref(),
                    operation.ty,
                    context,
                )?;
            }
            MirOperationKind::ExplicitPanic { message } => {
                self.verify_operand(function, message, context)?;
                if message.ty != self.hir.interner().scalar(ScalarType::String)
                    || operation.ty != self.hir.interner().scalar(ScalarType::Never)
                {
                    return Err(MirInvariantError::new(
                        context,
                        "panic requires a String message and has outcome Never",
                    ));
                }
            }
            MirOperationKind::Assert {
                condition,
                condition_repr,
                message_parts,
            } => {
                self.verify_operand(function, condition, context)?;
                if condition.ty != self.hir.interner().scalar(ScalarType::Bool) {
                    return Err(MirInvariantError::new(
                        context,
                        "assert operation condition is not Bool",
                    ));
                }
                if condition_repr.is_empty() {
                    return Err(MirInvariantError::new(
                        context,
                        "assert operation has no condition representation",
                    ));
                }
                let string_type = self.hir.interner().scalar(ScalarType::String);
                for part in message_parts {
                    self.verify_operand(function, part.value(), context)?;
                    if part.is_spread() {
                        let arguments = self.intrinsic_arguments(
                            part.value().ty,
                            IntrinsicType::Array,
                            context,
                        )?;
                        if arguments != [string_type] {
                            return Err(MirInvariantError::new(
                                context,
                                "spread assert message part is not Array[String]",
                            ));
                        }
                    } else if part.value().ty != string_type {
                        return Err(MirInvariantError::new(
                            context,
                            "assert message part is not String",
                        ));
                    }
                }
                if operation.ty != self.hir.interner().scalar(ScalarType::Unit) {
                    return Err(MirInvariantError::new(
                        context,
                        "assert operation does not produce Unit",
                    ));
                }
            }
            MirOperationKind::BootstrapHostCall {
                function: host_function,
                arguments,
            } => {
                for argument in arguments {
                    self.verify_operand(function, argument, context)?;
                }
                let intrinsic = |ty, expected| -> Result<bool, MirInvariantError> {
                    Ok(matches!(
                        self.kind(ty, context)?,
                        TypeKind::Intrinsic {
                            constructor,
                            arguments,
                        } if *constructor == expected && arguments.is_empty()
                    ))
                };
                let statuses = |ty| -> Result<bool, MirInvariantError> {
                    Ok(matches!(
                        self.kind(ty, context)?,
                        TypeKind::Intrinsic {
                            constructor: IntrinsicType::Array,
                            arguments,
                        } if arguments.len() == 1
                            && intrinsic(arguments[0], IntrinsicType::ExitStatus)?
                    ))
                };
                let string_map = |ty| -> Result<bool, MirInvariantError> {
                    Ok(matches!(
                        self.kind(ty, context)?,
                        TypeKind::Intrinsic {
                            constructor: IntrinsicType::Map,
                            arguments,
                        } if arguments.len() == 2
                            && arguments.iter().all(|argument| {
                                *argument == self.hir.interner().scalar(ScalarType::String)
                            })
                    ))
                };
                let pointer_element = |ty| -> Result<Option<TypeId>, MirInvariantError> {
                    Ok(match self.kind(ty, context)? {
                        TypeKind::Intrinsic {
                            constructor: IntrinsicType::Pointer,
                            arguments,
                        } if arguments.len() == 1 => Some(arguments[0]),
                        _ => None,
                    })
                };
                let sync_collection = |ty, name: &str, arity| -> Result<bool, MirInvariantError> {
                    Ok(matches!(
                        self.kind(ty, context)?,
                        TypeKind::Nominal { identity, arguments }
                            if identity.package().as_str() == "toolchain:std:0.1-bootstrap"
                                && identity.module().as_str() == "sync"
                                && identity.declaration().names().first().is_some_and(|candidate| candidate.as_str() == name)
                                && arguments.len() == arity
                    ))
                };
                let valid = match host_function {
                    super::MirBootstrapHostFunction::ConsolePrint
                    | super::MirBootstrapHostFunction::ConsolePrintln => {
                        arguments.len() == 1
                            && arguments[0].ty == self.hir.interner().scalar(ScalarType::String)
                            && operation.ty == self.hir.interner().scalar(ScalarType::Unit)
                    }
                    super::MirBootstrapHostFunction::ProcessPipe => {
                        arguments.len() == 2
                            && (intrinsic(arguments[0].ty, IntrinsicType::Command)?
                                || intrinsic(arguments[0].ty, IntrinsicType::Pipeline)?)
                            && (intrinsic(arguments[1].ty, IntrinsicType::Command)?
                                || intrinsic(arguments[1].ty, IntrinsicType::Pipeline)?)
                            && intrinsic(operation.ty, IntrinsicType::Pipeline)?
                    }
                    super::MirBootstrapHostFunction::CommandMergeStderr => {
                        arguments.len() == 1
                            && intrinsic(arguments[0].ty, IntrinsicType::Command)?
                            && intrinsic(operation.ty, IntrinsicType::Command)?
                    }
                    super::MirBootstrapHostFunction::PipelineMergeStderr => {
                        arguments.len() == 1
                            && intrinsic(arguments[0].ty, IntrinsicType::Pipeline)?
                            && intrinsic(operation.ty, IntrinsicType::Pipeline)?
                    }
                    super::MirBootstrapHostFunction::ProcessOutputStdout
                    | super::MirBootstrapHostFunction::ProcessOutputStderr
                    | super::MirBootstrapHostFunction::ProcessOutputCombined => {
                        arguments.len() == 1
                            && intrinsic(arguments[0].ty, IntrinsicType::ProcessOutput)?
                            && intrinsic(operation.ty, IntrinsicType::Bytes)?
                    }
                    super::MirBootstrapHostFunction::ProcessOutputStatuses => {
                        arguments.len() == 1
                            && intrinsic(arguments[0].ty, IntrinsicType::ProcessOutput)?
                            && statuses(operation.ty)?
                    }
                    super::MirBootstrapHostFunction::ExitStatusCode => {
                        arguments.len() == 1
                            && intrinsic(arguments[0].ty, IntrinsicType::ExitStatus)?
                            && matches!(
                                self.kind(operation.ty, context)?,
                                TypeKind::Option(item)
                                    if *item
                                        == self.hir.interner().scalar(ScalarType::Int)
                            )
                    }
                    super::MirBootstrapHostFunction::ExitStatusSuccess => {
                        arguments.len() == 1
                            && intrinsic(arguments[0].ty, IntrinsicType::ExitStatus)?
                            && operation.ty == self.hir.interner().scalar(ScalarType::Bool)
                    }
                    super::MirBootstrapHostFunction::PointerRead => {
                        arguments.len() == 1
                            && pointer_element(arguments[0].ty)? == Some(operation.ty)
                    }
                    super::MirBootstrapHostFunction::PointerWrite => {
                        arguments.len() == 2
                            && pointer_element(arguments[0].ty)? == Some(arguments[1].ty)
                            && operation.ty == self.hir.interner().scalar(ScalarType::Unit)
                    }
                    super::MirBootstrapHostFunction::PointerOffset => {
                        arguments.len() == 2
                            && pointer_element(arguments[0].ty)?.is_some()
                            && arguments[1].ty == self.hir.interner().scalar(ScalarType::Int)
                            && operation.ty == arguments[0].ty
                    }
                    super::MirBootstrapHostFunction::PointerCast => {
                        arguments.len() == 1
                            && pointer_element(arguments[0].ty)?.is_some()
                            && pointer_element(operation.ty)?.is_some()
                    }
                    super::MirBootstrapHostFunction::PointerAddress => {
                        arguments.len() == 1
                            && pointer_element(arguments[0].ty)?.is_some()
                            && operation.ty == self.hir.interner().scalar(ScalarType::UInt64)
                    }
                    super::MirBootstrapHostFunction::PointerFromAddress => {
                        arguments.len() == 1
                            && arguments[0].ty == self.hir.interner().scalar(ScalarType::UInt64)
                            && pointer_element(operation.ty)?.is_some()
                    }
                    super::MirBootstrapHostFunction::SyncArrayLiteral => {
                        arguments.len() <= 128 && sync_collection(operation.ty, "Array", 1)?
                    }
                    super::MirBootstrapHostFunction::SyncMapLiteral => {
                        arguments.len() <= 128
                            && arguments.len().is_multiple_of(2)
                            && sync_collection(operation.ty, "Map", 2)?
                    }
                    super::MirBootstrapHostFunction::SyncSetLiteral => {
                        arguments.len() <= 128 && sync_collection(operation.ty, "Set", 1)?
                    }
                    super::MirBootstrapHostFunction::SyncStackLiteral => {
                        arguments.len() <= 128 && sync_collection(operation.ty, "Stack", 1)?
                    }
                    super::MirBootstrapHostFunction::SyncQueueLiteral => {
                        arguments.len() <= 128 && sync_collection(operation.ty, "Queue", 1)?
                    }
                    super::MirBootstrapHostFunction::TestingLog => {
                        arguments.len() == 1
                            && arguments[0].ty == self.hir.interner().scalar(ScalarType::String)
                            && operation.ty == self.hir.interner().scalar(ScalarType::Unit)
                    }
                    super::MirBootstrapHostFunction::TestingFailNow
                    | super::MirBootstrapHostFunction::TestingSkip => {
                        arguments.len() == 1
                            && arguments[0].ty == self.hir.interner().scalar(ScalarType::String)
                            && operation.ty == self.hir.interner().scalar(ScalarType::Never)
                    }
                    super::MirBootstrapHostFunction::TestingTags => {
                        arguments.len() == 1
                            && string_map(arguments[0].ty)?
                            && operation.ty == self.hir.interner().scalar(ScalarType::Unit)
                    }
                    super::MirBootstrapHostFunction::TestingAttach => {
                        arguments.len() == 3
                            && arguments[0].ty == self.hir.interner().scalar(ScalarType::String)
                            && arguments[1].ty == self.hir.interner().scalar(ScalarType::String)
                            && intrinsic(arguments[2].ty, IntrinsicType::Bytes)?
                            && operation.ty == self.hir.interner().scalar(ScalarType::Unit)
                    }
                    super::MirBootstrapHostFunction::TestingSnapshot => {
                        arguments.len() == 2
                            && arguments.iter().all(|argument| {
                                argument.ty == self.hir.interner().scalar(ScalarType::String)
                            })
                            && operation.ty == self.hir.interner().scalar(ScalarType::Unit)
                    }
                    super::MirBootstrapHostFunction::TestingRunLeaf
                    | super::MirBootstrapHostFunction::TestingRunSuite => {
                        arguments.len() == 2
                            && arguments[0].ty == self.hir.interner().scalar(ScalarType::String)
                            && matches!(
                                self.kind(arguments[1].ty, context)?,
                                TypeKind::Function(function)
                                    if function.is_async()
                                        && function.parameters().is_empty()
                                        && function.outcome()
                                            == self.hir.interner().scalar(ScalarType::Unit)
                            )
                            && operation.ty == self.hir.interner().scalar(ScalarType::Unit)
                    }
                    super::MirBootstrapHostFunction::TestingBeginSuiteCleanup => {
                        arguments.is_empty()
                            && operation.ty == self.hir.interner().scalar(ScalarType::Unit)
                    }
                };
                if !valid {
                    return Err(MirInvariantError::new(
                        context,
                        "bootstrap host operation does not match its closed contract",
                    ));
                }
            }
        }
        Ok(())
    }

    fn verify_aggregate(
        &self,
        function: &MirFunction,
        shape: &MirAggregateKind,
        values: &[MirOperand],
        ty: TypeId,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        match shape {
            MirAggregateKind::Tuple => {
                let TypeKind::Tuple(items) = self.kind(ty, context)? else {
                    return Err(MirInvariantError::new(
                        context,
                        "tuple aggregate has a non-tuple type",
                    ));
                };
                self.verify_operand_types(values, items, context)?;
            }
            MirAggregateKind::Array => {
                let arguments = self.intrinsic_arguments(ty, IntrinsicType::Array, context)?;
                if values.iter().any(|value| value.ty != arguments[0]) {
                    return Err(MirInvariantError::new(
                        context,
                        "array aggregate contains a value of the wrong element type",
                    ));
                }
            }
            MirAggregateKind::Set => {
                let arguments = self.intrinsic_arguments(ty, IntrinsicType::Set, context)?;
                if values.iter().any(|value| value.ty != arguments[0]) {
                    return Err(MirInvariantError::new(
                        context,
                        "set aggregate contains a value of the wrong element type",
                    ));
                }
            }
            MirAggregateKind::Closure { closure, arguments } => {
                let closure = self.hir.closure(*closure).ok_or_else(|| {
                    MirInvariantError::new(context, "closure aggregate has no HIR metadata")
                })?;
                if arguments.len() != closure.generic_arity() as usize {
                    return Err(MirInvariantError::new(
                        context,
                        "closure aggregate has the wrong generic arity",
                    ));
                }
                for argument in arguments {
                    self.verify_type(*argument, context)?;
                }
                let substitution = TypeSubstitution::new(arguments.clone());
                let mut interner = self.hir.interner().clone();
                let expected_type = substitution
                    .apply(&mut interner, closure.ty())
                    .map_err(|error| MirInvariantError::new(context, error.to_string()))?;
                if ty != expected_type
                    || values.len() != closure.captures().len()
                    || values
                        .iter()
                        .zip(closure.captures())
                        .any(|(value, capture)| {
                            let Ok(expected_capture) =
                                substitution.apply(&mut interner, capture.ty())
                            else {
                                return true;
                            };
                            if value.ty != expected_capture {
                                return true;
                            }
                            let (MirOperandKind::Copy(place) | MirOperandKind::Move(place)) =
                                &value.kind
                            else {
                                return true;
                            };
                            !self.place_represents_source_local(function, place, capture.local())
                        })
                {
                    return Err(MirInvariantError::new(
                        context,
                        "closure aggregate type, capture layout, or source binding is inconsistent",
                    ));
                }
            }
            MirAggregateKind::Newtype { owner } => {
                let (actual_owner, arguments, nominal) = self.nominal_instance(ty, context)?;
                let HirNominalShape::Newtype { underlying } = nominal.shape() else {
                    return Err(MirInvariantError::new(
                        context,
                        "newtype aggregate owner does not declare a newtype",
                    ));
                };
                if actual_owner != *owner
                    || values.len() != 1
                    || !self.type_matches_substitution(
                        *underlying,
                        values[0].ty,
                        arguments,
                        context,
                    )?
                {
                    return Err(MirInvariantError::new(
                        context,
                        "newtype aggregate owner or payload type is inconsistent",
                    ));
                }
            }
            MirAggregateKind::Ref => {
                let arguments = self.intrinsic_arguments(ty, IntrinsicType::Ref, context)?;
                self.verify_operand_types(values, &[arguments[0]], context)?;
            }
            MirAggregateKind::Record { owner, fields } => {
                let (actual_owner, arguments, nominal) = self.nominal_instance(ty, context)?;
                let HirNominalShape::Record { fields: declared } = nominal.shape() else {
                    return Err(MirInvariantError::new(
                        context,
                        "record aggregate owner does not declare a record",
                    ));
                };
                let mut seen = BTreeSet::new();
                if actual_owner != *owner
                    || fields.len() != declared.len()
                    || fields.len() != values.len()
                    || fields.iter().any(|field| !seen.insert(*field))
                {
                    return Err(MirInvariantError::new(
                        context,
                        "record aggregate owner, arity, or field set is inconsistent",
                    ));
                }
                for ((member, value), declared) in fields.iter().zip(values).zip(declared.iter()) {
                    if *member != declared.member()
                        || !self.type_matches_substitution(
                            declared.ty(),
                            value.ty,
                            arguments,
                            context,
                        )?
                    {
                        return Err(MirInvariantError::new(
                            context,
                            "record aggregate field order or type is inconsistent",
                        ));
                    }
                }
            }
            MirAggregateKind::Variant { variant, fields } => {
                let (owner, arguments, nominal) = self.nominal_instance(ty, context)?;
                let HirNominalShape::Enum { variants } = nominal.shape() else {
                    return Err(MirInvariantError::new(
                        context,
                        "variant aggregate has a non-enum type",
                    ));
                };
                let declaration = variants
                    .iter()
                    .find(|candidate| candidate.member() == *variant)
                    .ok_or_else(|| {
                        MirInvariantError::new(context, "variant does not belong to its enum type")
                    })?;
                self.verify_variant_payload(
                    owner,
                    *variant,
                    declaration.payload(),
                    fields,
                    values,
                    arguments,
                    context,
                )?;
            }
            MirAggregateKind::NumericConversionError(_) => {
                if !values.is_empty()
                    || !matches!(
                        self.kind(ty, context)?,
                        TypeKind::Intrinsic {
                            constructor: IntrinsicType::NumericConversionError,
                            arguments,
                        } if arguments.is_empty()
                    )
                {
                    return Err(MirInvariantError::new(
                        context,
                        "numeric conversion error aggregate has an invalid type or payload",
                    ));
                }
            }
            MirAggregateKind::OptionNone => {
                if !values.is_empty() || !matches!(self.kind(ty, context)?, TypeKind::Option(_)) {
                    return Err(MirInvariantError::new(
                        context,
                        "none aggregate shape or arity is inconsistent",
                    ));
                }
            }
            MirAggregateKind::OptionSome => {
                let TypeKind::Option(item) = self.kind(ty, context)? else {
                    return Err(MirInvariantError::new(
                        context,
                        "some aggregate has a non-option type",
                    ));
                };
                self.verify_operand_types(values, &[*item], context)?;
            }
            MirAggregateKind::ResultOk => {
                let TypeKind::Result { success, .. } = self.kind(ty, context)? else {
                    return Err(MirInvariantError::new(
                        context,
                        "ok aggregate has a non-result type",
                    ));
                };
                self.verify_operand_types(values, &[*success], context)?;
            }
            MirAggregateKind::ResultErr => {
                let TypeKind::Result { error, .. } = self.kind(ty, context)? else {
                    return Err(MirInvariantError::new(
                        context,
                        "err aggregate has a non-result type",
                    ));
                };
                self.verify_operand_types(values, &[*error], context)?;
            }
        }
        Ok(())
    }

    fn verify_operand(
        &self,
        function: &MirFunction,
        operand: &MirOperand,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        self.verify_type(operand.ty, context)?;
        match &operand.kind {
            MirOperandKind::Constant(super::MirConstant::Named(symbol)) => {
                let constant = self.hir.constant(*symbol).ok_or_else(|| {
                    MirInvariantError::new(
                        context,
                        format!(
                            "operand references unknown constant symbol#{}",
                            symbol.index()
                        ),
                    )
                })?;
                if constant.ty() != Some(operand.ty) || constant.evaluated().is_none() {
                    return Err(MirInvariantError::new(
                        context,
                        "named constant operand lacks a matching normalized value",
                    ));
                }
            }
            MirOperandKind::Constant(constant) => {
                self.verify_constant(constant, operand.ty, context)?;
            }
            MirOperandKind::Copy(place) | MirOperandKind::Move(place) => {
                self.verify_place(function, place, context)?;
                if matches!(operand.kind, MirOperandKind::Move(_))
                    && place_contains_ref_value(place)
                {
                    return Err(MirInvariantError::new(
                        context,
                        "`Ref[T].value` cannot be moved out of its identity cell",
                    ));
                }
                if place.ty != operand.ty {
                    return Err(MirInvariantError::new(
                        context,
                        "place operand changes its place type",
                    ));
                }
                let status =
                    self.capability_status(function.id, operand.ty, HirCapability::Copy, context)?;
                let valid = matches!(
                    (&operand.kind, status),
                    (MirOperandKind::Copy(_), HirCapabilityStatus::Satisfied)
                        | (MirOperandKind::Move(_), HirCapabilityStatus::Unsatisfied)
                );
                if !valid {
                    return Err(MirInvariantError::new(
                        context,
                        format!(
                            "{:?} operand does not match the type's contextual Copy status {status:?}",
                            operand.kind
                        ),
                    ));
                }
            }
            MirOperandKind::Borrow(place) => {
                self.verify_place(function, place, context)?;
                if place.ty != operand.ty {
                    return Err(MirInvariantError::new(
                        context,
                        "borrow operand changes its place type",
                    ));
                }
            }
            MirOperandKind::Loan(loan) => {
                let loan = self.loan(function, *loan, context)?;
                if loan.kind != MirLoanKind::CallLocal {
                    return Err(MirInvariantError::new(
                        context,
                        "region loan cannot be consumed as a call argument",
                    ));
                }
                if loan.place.ty != operand.ty {
                    return Err(MirInvariantError::new(
                        context,
                        "loan operand changes its reserved place type",
                    ));
                }
            }
            MirOperandKind::Function {
                callable,
                arguments,
            } => {
                let signature = self.hir.callable(*callable).ok_or_else(|| {
                    MirInvariantError::new(context, "function operand has no HIR signature")
                })?;
                if arguments.len() != signature.generic_arity() as usize {
                    return Err(MirInvariantError::new(
                        context,
                        "function operand specialization arity is invalid",
                    ));
                }
                for argument in arguments {
                    self.verify_type(*argument, context)?;
                }
                if !self.type_matches_substitution(
                    signature.function_type(),
                    operand.ty,
                    arguments,
                    context,
                )? {
                    return Err(MirInvariantError::new(
                        context,
                        "function operand type does not match its specialization",
                    ));
                }
            }
            MirOperandKind::PreludeTraitFunction { method, arguments } => {
                if arguments.len() != method.generic_arity() as usize {
                    return Err(MirInvariantError::new(
                        context,
                        "prelude trait function operand specialization arity is invalid",
                    ));
                }
                for argument in arguments {
                    self.verify_type(*argument, context)?;
                }
                let mut interner = self.hir.interner().clone();
                let expected = method
                    .function_type(&mut interner, arguments)
                    .map_err(|error| MirInvariantError::new(context, error.to_string()))?
                    .ok_or_else(|| {
                        MirInvariantError::new(
                            context,
                            "prelude trait function operand has an invalid specialization",
                        )
                    })?;
                if expected != operand.ty {
                    return Err(MirInvariantError::new(
                        context,
                        "prelude trait function operand type does not match its closed contract",
                    ));
                }
            }
        }
        Ok(())
    }

    fn capability_status(
        &self,
        function: MirFunctionId,
        ty: TypeId,
        capability: HirCapability,
        context: &str,
    ) -> Result<HirCapabilityStatus, MirInvariantError> {
        let key = (function, ty, capability);
        if let Some(status) = self.capability_statuses.borrow().get(&key).copied() {
            return Ok(status);
        }
        let generics = match function {
            MirFunctionId::Callable(callable) => self
                .hir
                .callable(callable)
                .map(|callable| callable.generics()),
            MirFunctionId::Closure(closure) => {
                self.hir.closure(closure).map(|closure| closure.generics())
            }
        }
        .ok_or_else(|| MirInvariantError::new(context, "function has no typed HIR generics"))?;
        let assumptions = CapabilityAssumptions::from_generics(self.hir, generics);
        let status = self
            .capability_analysis
            .status(self.hir, ty, capability, &assumptions)
            .map_err(|error| MirInvariantError::new(context, error.to_string()))?;
        if status == HirCapabilityStatus::Deferred {
            return Err(MirInvariantError::new(
                context,
                format!(
                    "{} capability remains unresolved for MIR type {}",
                    capability.as_str(),
                    self.hir
                        .interner()
                        .canonical(ty)
                        .unwrap_or_else(|_| ty.to_string())
                ),
            ));
        }
        self.capability_statuses.borrow_mut().insert(key, status);
        Ok(status)
    }

    fn terminal_status(
        &self,
        function: MirFunctionId,
        ty: TypeId,
        context: &str,
    ) -> Result<HirTerminalStatus, MirInvariantError> {
        let key = (function, ty);
        if let Some(status) = self.terminal_statuses.borrow().get(&key).copied() {
            return Ok(status);
        }
        let generics = match function {
            MirFunctionId::Callable(callable) => self
                .hir
                .callable(callable)
                .map(|callable| callable.generics()),
            MirFunctionId::Closure(closure) => {
                self.hir.closure(closure).map(|closure| closure.generics())
            }
        }
        .ok_or_else(|| MirInvariantError::new(context, "function has no typed HIR generics"))?;
        let assumptions = CapabilityAssumptions::from_generics(self.hir, generics);
        let status = self
            .terminal_analysis
            .status(self.hir, ty, &assumptions)
            .map_err(|error| MirInvariantError::new(context, error.to_string()))?;
        self.terminal_statuses.borrow_mut().insert(key, status);
        Ok(status)
    }

    fn verify_place(
        &self,
        function: &MirFunction,
        place: &MirPlace,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        let local = self.local(function, place.local, context)?;
        if let Some((position, projection)) =
            place
                .projections
                .iter()
                .enumerate()
                .find(|(_, projection)| {
                    matches!(projection.kind, MirProjectionKind::ClosureCapture { .. })
                })
        {
            let valid_root = position == 0
                && function.parameters.first() == Some(&place.local)
                && matches!(
                    (function.id, &projection.kind),
                    (
                        MirFunctionId::Closure(function_closure),
                        MirProjectionKind::ClosureCapture { closure, .. }
                    ) if function_closure == *closure
                );
            if !valid_root {
                return Err(MirInvariantError::new(
                    context,
                    "closure capture projection is not rooted in its hidden environment parameter",
                ));
            }
        }
        self.verify_type(place.ty, context)?;
        let mut current = local.ty;
        for (position, projection) in place.projections.iter().enumerate() {
            self.verify_type(projection.ty, context)?;
            if let MirProjectionKind::IteratorElement { index } = projection.kind {
                let base = MirPlace {
                    local: place.local,
                    ty: current,
                    projections: place.projections[..position].to_vec(),
                    source_loan: place.source_loan,
                };
                self.verify_iterator_element_origin(function, &base, index, context)?;
            }
            let expected = self.projection_result(function, current, projection, context)?;
            if expected != projection.ty {
                return Err(MirInvariantError::new(
                    context,
                    format!(
                        "projection declares {}, but its base shape produces {expected}",
                        projection.ty
                    ),
                ));
            }
            current = projection.ty;
        }
        if current != place.ty {
            return Err(MirInvariantError::new(
                context,
                format!(
                    "place projection ends in {current}, but place declares {}",
                    place.ty
                ),
            ));
        }
        if let Some(source) = place.source_loan {
            let source = self.loan(function, source, context)?;
            if source.kind != MirLoanKind::Region {
                return Err(MirInvariantError::new(
                    context,
                    "place source is not a region loan",
                ));
            }
            let source = LocalAccess::from_place(&source.place);
            let access = LocalAccess::from_place(place);
            if source.local != access.local || !move_path_is_prefix(&source.path, &access.path) {
                return Err(MirInvariantError::new(
                    context,
                    "place escapes the source region's reserved path",
                ));
            }
        }
        Ok(())
    }

    fn kind<'a>(&'a self, ty: TypeId, context: &str) -> Result<&'a TypeKind, MirInvariantError> {
        self.hir.interner().kind(ty).map_err(|error| {
            MirInvariantError::new(context, format!("type {ty} is not interned: {error}"))
        })
    }

    fn intrinsic_arguments<'a>(
        &'a self,
        ty: TypeId,
        constructor: IntrinsicType,
        context: &str,
    ) -> Result<&'a [TypeId], MirInvariantError> {
        let TypeKind::Intrinsic {
            constructor: actual,
            arguments,
        } = self.kind(ty, context)?
        else {
            return Err(MirInvariantError::new(
                context,
                format!("expected {constructor}, found non-intrinsic {ty}"),
            ));
        };
        if *actual != constructor {
            return Err(MirInvariantError::new(
                context,
                format!("expected {constructor}, found {actual}"),
            ));
        }
        Ok(arguments)
    }

    fn nominal_instance<'a>(
        &'a self,
        ty: TypeId,
        context: &str,
    ) -> Result<(SymbolId, &'a [TypeId], &'a crate::hir::HirNominalDefinition), MirInvariantError>
    {
        let TypeKind::Nominal {
            identity,
            arguments,
        } = self.kind(ty, context)?
        else {
            return Err(MirInvariantError::new(
                context,
                format!("{ty} is not a nominal type"),
            ));
        };
        let symbol = self
            .resolved
            .symbols()
            .find(|symbol| symbol.identity() == identity)
            .map(|symbol| symbol.id())
            .ok_or_else(|| {
                MirInvariantError::new(context, "nominal type identity is not resolved")
            })?;
        let declaration = self.hir.declaration(symbol).ok_or_else(|| {
            MirInvariantError::new(context, "nominal type has no typed HIR declaration")
        })?;
        let HirTypeDeclarationKind::Nominal(nominal) = declaration.kind() else {
            return Err(MirInvariantError::new(
                context,
                "nominal TypeId points to a non-nominal HIR declaration",
            ));
        };
        if arguments.len() != declaration.parameters().len() {
            return Err(MirInvariantError::new(
                context,
                "nominal instance has the wrong generic arity",
            ));
        }
        Ok((symbol, arguments, nominal))
    }

    fn type_matches_substitution(
        &self,
        template: TypeId,
        actual: TypeId,
        arguments: &[TypeId],
        context: &str,
    ) -> Result<bool, MirInvariantError> {
        let mut pending = vec![(template, actual)];
        while let Some((template, actual)) = pending.pop() {
            if template == actual {
                continue;
            }
            let template_kind = self.kind(template, context)?;
            if let TypeKind::GenericParameter(position) = template_kind {
                if arguments.get(*position as usize) != Some(&actual) {
                    return Ok(false);
                }
                continue;
            }
            let actual_kind = self.kind(actual, context)?;
            match (template_kind, actual_kind) {
                (TypeKind::Scalar(left), TypeKind::Scalar(right)) if left == right => {}
                (
                    TypeKind::Nominal {
                        identity: left_identity,
                        arguments: left,
                    },
                    TypeKind::Nominal {
                        identity: right_identity,
                        arguments: right,
                    },
                ) if left_identity == right_identity && left.len() == right.len() => {
                    pending.extend(left.iter().copied().zip(right.iter().copied()));
                }
                (TypeKind::Tuple(left), TypeKind::Tuple(right))
                | (TypeKind::Union(left), TypeKind::Union(right))
                    if left.len() == right.len() =>
                {
                    pending.extend(left.iter().copied().zip(right.iter().copied()));
                }
                (TypeKind::Option(left), TypeKind::Option(right)) => {
                    pending.push((*left, *right));
                }
                (
                    TypeKind::Result {
                        success: left_success,
                        error: left_error,
                    },
                    TypeKind::Result {
                        success: right_success,
                        error: right_error,
                    },
                ) => {
                    pending.push((*left_success, *right_success));
                    pending.push((*left_error, *right_error));
                }
                (
                    TypeKind::Intrinsic {
                        constructor: left_constructor,
                        arguments: left,
                    },
                    TypeKind::Intrinsic {
                        constructor: right_constructor,
                        arguments: right,
                    },
                ) if left_constructor == right_constructor && left.len() == right.len() => {
                    pending.extend(left.iter().copied().zip(right.iter().copied()));
                }
                (TypeKind::Function(left), TypeKind::Function(right))
                    if left.is_async() == right.is_async()
                        && left.is_unsafe() == right.is_unsafe()
                        && left.parameters().len() == right.parameters().len()
                        && left.variadic().is_some() == right.variadic().is_some() =>
                {
                    for (left, right) in left.parameters().iter().zip(right.parameters()) {
                        if left.mode() != right.mode() {
                            return Ok(false);
                        }
                        pending.push((left.ty(), right.ty()));
                    }
                    if let (Some(left), Some(right)) = (left.variadic(), right.variadic()) {
                        pending.push((left, right));
                    }
                    pending.push((left.outcome(), right.outcome()));
                }
                (
                    TypeKind::OpaqueResult {
                        identity: left_identity,
                        arguments: left,
                    },
                    TypeKind::OpaqueResult {
                        identity: right_identity,
                        arguments: right,
                    },
                ) if left_identity == right_identity && left.len() == right.len() => {
                    pending.extend(left.iter().copied().zip(right.iter().copied()));
                }
                (
                    TypeKind::Generated {
                        identity: left_identity,
                        arguments: left,
                    },
                    TypeKind::Generated {
                        identity: right_identity,
                        arguments: right,
                    },
                ) if left_identity == right_identity && left.len() == right.len() => {
                    pending.extend(left.iter().copied().zip(right.iter().copied()));
                }
                (
                    TypeKind::Cursor {
                        mode: left_mode,
                        collection: left,
                    },
                    TypeKind::Cursor {
                        mode: right_mode,
                        collection: right,
                    },
                ) if left_mode == right_mode => pending.push((*left, *right)),
                _ => return Ok(false),
            }
        }
        Ok(true)
    }

    fn callable_erasure_matches(
        &self,
        actual: TypeId,
        expected: TypeId,
        context: &str,
    ) -> Result<bool, MirInvariantError> {
        if !matches!(self.kind(expected, context)?, TypeKind::Function(_)) {
            return Ok(false);
        }
        let TypeKind::Generated {
            identity,
            arguments,
        } = self.kind(actual, context)?
        else {
            return Ok(false);
        };
        let Some(closure) = self.hir.closure_by_identity(identity) else {
            return Ok(false);
        };
        let mut interner = self.hir.interner().clone();
        let signature = match TypeSubstitution::new(arguments.clone())
            .apply(&mut interner, closure.function_type())
        {
            Ok(signature) => signature,
            Err(error) => return Err(MirInvariantError::new(context, error.to_string())),
        };
        let assignability = self
            .hir
            .interner()
            .assignability(signature, expected)
            .expect("verified callable signatures must be valid interner entries");
        Ok(matches!(
            assignability,
            Some(crate::types::Assignability::Exact | crate::types::Assignability::EffectWeakening)
        ) && closure
            .protocols()
            .supports(crate::hir::HirCallProtocol::Call))
    }

    fn callable_once_erasure_matches(
        &self,
        actual: TypeId,
        expected: TypeId,
        context: &str,
    ) -> Result<bool, MirInvariantError> {
        if !matches!(self.kind(expected, context)?, TypeKind::Function(_)) {
            return Ok(false);
        }
        let TypeKind::Generated {
            identity,
            arguments,
        } = self.kind(actual, context)?
        else {
            return Ok(false);
        };
        let Some(closure) = self.hir.closure_by_identity(identity) else {
            return Ok(false);
        };
        let mut interner = self.hir.interner().clone();
        let signature = TypeSubstitution::new(arguments.clone())
            .apply(&mut interner, closure.function_type())
            .map_err(|error| MirInvariantError::new(context, error.to_string()))?;
        let assignability = self
            .hir
            .interner()
            .assignability(signature, expected)
            .expect("verified callable signatures must be valid interner entries");
        Ok(matches!(
            assignability,
            Some(crate::types::Assignability::Exact | crate::types::Assignability::EffectWeakening)
        ) && closure
            .protocols()
            .supports(crate::hir::HirCallProtocol::CallOnce))
    }

    fn nominal_field_matches(
        &self,
        ty: TypeId,
        member: crate::resolve::MemberId,
        actual: TypeId,
        context: &str,
    ) -> Result<bool, MirInvariantError> {
        let (owner, arguments, nominal) = self.nominal_instance(ty, context)?;
        let declaration = self
            .resolved
            .member(member)
            .ok_or_else(|| MirInvariantError::new(context, "field references an unknown member"))?;
        if declaration.owner() != MemberOwner::Type(owner) || !declaration.kind().is_field() {
            return Ok(false);
        }
        let template = match nominal.shape() {
            HirNominalShape::Newtype { underlying }
                if declaration.kind() == MemberKind::NewtypeValue =>
            {
                *underlying
            }
            HirNominalShape::Record { fields } => fields
                .iter()
                .find(|field| field.member() == member)
                .map(|field| field.ty())
                .ok_or_else(|| {
                    MirInvariantError::new(context, "field is absent from its nominal HIR shape")
                })?,
            _ => return Ok(false),
        };
        self.type_matches_substitution(template, actual, arguments, context)
    }

    fn verify_prefix(
        &self,
        operator: HirPrefixOperator,
        operand: TypeId,
        result: TypeId,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        let TypeKind::Scalar(scalar) = self.kind(operand, context)? else {
            return Err(MirInvariantError::new(
                context,
                "prefix operator has a non-scalar operand",
            ));
        };
        let valid = match operator {
            HirPrefixOperator::LogicalNot => *scalar == ScalarType::Bool,
            HirPrefixOperator::Negate => is_signed_integer(*scalar) || is_float(*scalar),
            HirPrefixOperator::BitwiseNot => is_integer(*scalar) || *scalar == ScalarType::Byte,
        };
        let expected = if operator == HirPrefixOperator::LogicalNot {
            self.hir.interner().scalar(ScalarType::Bool)
        } else {
            operand
        };
        if !valid || result != expected {
            return Err(MirInvariantError::new(
                context,
                "prefix operand or result type is invalid",
            ));
        }
        Ok(())
    }

    fn verify_binary(
        &self,
        operator: HirBinaryOperator,
        left: TypeId,
        right: TypeId,
        result: TypeId,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        if !self.binary_result_matches(operator, left, right, result, context)? {
            return Err(MirInvariantError::new(
                context,
                "binary operand or result type is invalid",
            ));
        }
        Ok(())
    }

    fn binary_result_matches(
        &self,
        operator: HirBinaryOperator,
        left: TypeId,
        right: TypeId,
        result: TypeId,
        context: &str,
    ) -> Result<bool, MirInvariantError> {
        let arithmetic = matches!(
            operator,
            HirBinaryOperator::Add
                | HirBinaryOperator::Subtract
                | HirBinaryOperator::Multiply
                | HirBinaryOperator::Divide
                | HirBinaryOperator::Remainder
        );
        let left_array = self.array_element(left);
        let right_array = self.array_element(right);
        if arithmetic && (left_array.is_some() || right_array.is_some()) {
            let Some(result_element) = self.array_element(result) else {
                return Ok(false);
            };
            return self.binary_result_matches(
                operator,
                left_array.unwrap_or(left),
                right_array.unwrap_or(right),
                result_element,
                context,
            );
        }
        let left_scalar = match self.kind(left, context)? {
            TypeKind::Scalar(scalar) => Some(*scalar),
            _ => None,
        };
        let right_scalar = match self.kind(right, context)? {
            TypeKind::Scalar(scalar) => Some(*scalar),
            _ => None,
        };
        if left != right
            && !matches!(
                operator,
                HirBinaryOperator::ShiftLeft | HirBinaryOperator::ShiftRight
            )
        {
            return Ok(false);
        }
        let valid = match operator {
            HirBinaryOperator::Multiply
            | HirBinaryOperator::Divide
            | HirBinaryOperator::Add
            | HirBinaryOperator::Subtract => left_scalar.is_some_and(is_arithmetic),
            HirBinaryOperator::Remainder => left_scalar.is_some_and(is_integer),
            HirBinaryOperator::ShiftLeft | HirBinaryOperator::ShiftRight => {
                left_scalar.is_some_and(|scalar| is_integer(scalar) || scalar == ScalarType::Byte)
                    && right_scalar.is_some_and(is_integer)
            }
            HirBinaryOperator::BitwiseAnd
            | HirBinaryOperator::BitwiseXor
            | HirBinaryOperator::BitwiseOr => {
                left_scalar.is_some_and(|scalar| is_integer(scalar) || scalar == ScalarType::Byte)
            }
            HirBinaryOperator::Less
            | HirBinaryOperator::LessEqual
            | HirBinaryOperator::Greater
            | HirBinaryOperator::GreaterEqual => left_scalar.is_some_and(is_relational),
            HirBinaryOperator::Equal | HirBinaryOperator::NotEqual => !matches!(
                self.hir.capability_status(left, HirCapability::Equatable),
                None | Some(HirCapabilityStatus::Unsatisfied)
            ),
            HirBinaryOperator::LogicalAnd | HirBinaryOperator::LogicalOr => {
                left_scalar == Some(ScalarType::Bool)
            }
        };
        if !valid {
            return Ok(false);
        }
        let expected = if matches!(
            operator,
            HirBinaryOperator::Less
                | HirBinaryOperator::LessEqual
                | HirBinaryOperator::Greater
                | HirBinaryOperator::GreaterEqual
                | HirBinaryOperator::Equal
                | HirBinaryOperator::NotEqual
                | HirBinaryOperator::LogicalAnd
                | HirBinaryOperator::LogicalOr
        ) {
            self.hir.interner().scalar(ScalarType::Bool)
        } else {
            left
        };
        Ok(result == expected)
    }

    fn prefix_requires_checked(&self, operator: HirPrefixOperator, operand: TypeId) -> bool {
        operator == HirPrefixOperator::Negate
            && matches!(
                self.hir.interner().kind(operand),
                Ok(TypeKind::Scalar(
                    ScalarType::Int | ScalarType::Int8 | ScalarType::Int16 | ScalarType::Int32
                ))
            )
    }

    fn binary_requires_checked(
        &self,
        operator: HirBinaryOperator,
        left: TypeId,
        right: TypeId,
    ) -> bool {
        matches!(
            operator,
            HirBinaryOperator::Multiply
                | HirBinaryOperator::Divide
                | HirBinaryOperator::Remainder
                | HirBinaryOperator::Add
                | HirBinaryOperator::Subtract
                | HirBinaryOperator::ShiftLeft
                | HirBinaryOperator::ShiftRight
        ) && (self.is_array(left)
            || self.is_array(right)
            || !matches!(
                self.hir.interner().kind(left),
                Ok(TypeKind::Scalar(ScalarType::Float | ScalarType::Float32))
            ))
    }

    fn verify_numeric_conversion(
        &self,
        source: TypeId,
        target: ScalarType,
        conversion: NumericConversion,
        result: TypeId,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        let TypeKind::Scalar(source_scalar) = self.kind(source, context)? else {
            return Err(MirInvariantError::new(
                context,
                "numeric conversion source is not scalar",
            ));
        };
        if numeric_conversion(*source_scalar, target) != Some(conversion) {
            return Err(MirInvariantError::new(
                context,
                "numeric conversion class does not match the closed conversion table",
            ));
        }
        let target_type = self.hir.interner().scalar(target);
        let valid_result = if conversion == NumericConversion::Checked {
            matches!(
                self.kind(result, context)?,
                TypeKind::Result { success, error }
                    if *success == target_type
                        && matches!(
                            self.hir.interner().kind(*error),
                            Ok(TypeKind::Intrinsic {
                                constructor: IntrinsicType::NumericConversionError,
                                arguments,
                            }) if arguments.is_empty()
                        )
            )
        } else {
            result == target_type
        };
        if !valid_result {
            return Err(MirInvariantError::new(
                context,
                "numeric conversion result type is inconsistent",
            ));
        }
        Ok(())
    }

    fn verify_contains(
        &self,
        kind: HirContainmentKind,
        item: TypeId,
        container: TypeId,
        result: TypeId,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        let expected = match kind {
            HirContainmentKind::Array => {
                self.intrinsic_arguments(container, IntrinsicType::Array, context)?[0]
            }
            HirContainmentKind::MapKey => {
                self.intrinsic_arguments(container, IntrinsicType::Map, context)?[0]
            }
            HirContainmentKind::Set => {
                self.intrinsic_arguments(container, IntrinsicType::Set, context)?[0]
            }
            HirContainmentKind::Range => {
                self.intrinsic_arguments(container, IntrinsicType::Range, context)?[0]
            }
            HirContainmentKind::StringChar => {
                if container != self.hir.interner().scalar(ScalarType::String) {
                    return Err(MirInvariantError::new(
                        context,
                        "StringChar containment has a non-String container",
                    ));
                }
                self.hir.interner().scalar(ScalarType::Char)
            }
        };
        if item != expected || result != self.hir.interner().scalar(ScalarType::Bool) {
            return Err(MirInvariantError::new(
                context,
                "containment item or result type is inconsistent",
            ));
        }
        let capability = match kind {
            HirContainmentKind::Array => Some(HirCapability::Equatable),
            HirContainmentKind::MapKey | HirContainmentKind::Set => Some(HirCapability::Key),
            HirContainmentKind::Range | HirContainmentKind::StringChar => None,
        };
        if let Some(capability) = capability
            && matches!(
                self.hir.capability_status(expected, capability),
                None | Some(HirCapabilityStatus::Unsatisfied)
            )
        {
            return Err(MirInvariantError::new(
                context,
                "containment item lacks its closed capability",
            ));
        }
        Ok(())
    }

    fn projection_result(
        &self,
        function: &MirFunction,
        current: TypeId,
        projection: &MirProjection,
        context: &str,
    ) -> Result<TypeId, MirInvariantError> {
        let declared = projection.ty;
        match &projection.kind {
            MirProjectionKind::ClosureCapture { closure, index } => {
                let metadata = self.hir.closure(*closure).ok_or_else(|| {
                    MirInvariantError::new(
                        context,
                        "closure capture projection has no HIR metadata",
                    )
                })?;
                if function.id != MirFunctionId::Closure(*closure) || current != metadata.ty() {
                    return Err(MirInvariantError::new(
                        context,
                        "closure capture projection has the wrong function or environment type",
                    ));
                }
                let capture = metadata.captures().get(*index as usize).ok_or_else(|| {
                    MirInvariantError::new(
                        context,
                        "closure capture projection index is out of range",
                    )
                })?;
                if declared != capture.ty() {
                    return Err(MirInvariantError::new(
                        context,
                        "closure capture projection has the wrong capture type",
                    ));
                }
                Ok(declared)
            }
            MirProjectionKind::Field(member) => {
                if !self.nominal_field_matches(current, *member, declared, context)? {
                    return Err(MirInvariantError::new(
                        context,
                        "field projection does not belong to its nominal base or has wrong type",
                    ));
                }
                Ok(declared)
            }
            MirProjectionKind::TupleField(index) => {
                let TypeKind::Tuple(items) = self.kind(current, context)? else {
                    return Err(MirInvariantError::new(
                        context,
                        "tuple projection has a non-tuple base",
                    ));
                };
                items.get(*index as usize).copied().ok_or_else(|| {
                    MirInvariantError::new(context, "tuple projection index is out of range")
                })
            }
            MirProjectionKind::NewtypeValue => {
                let (_, arguments, nominal) = self.nominal_instance(current, context)?;
                let HirNominalShape::Newtype { underlying } = nominal.shape() else {
                    return Err(MirInvariantError::new(
                        context,
                        "newtype projection has a non-newtype base",
                    ));
                };
                if !self.type_matches_substitution(*underlying, declared, arguments, context)? {
                    return Err(MirInvariantError::new(
                        context,
                        "newtype projection has the wrong instantiated payload type",
                    ));
                }
                Ok(declared)
            }
            MirProjectionKind::RefValue => {
                let arguments = self.intrinsic_arguments(current, IntrinsicType::Ref, context)?;
                if declared != arguments[0] {
                    return Err(MirInvariantError::new(
                        context,
                        "Ref value projection has the wrong target type",
                    ));
                }
                Ok(declared)
            }
            MirProjectionKind::VariantTuple { variant, index } => {
                let (owner, arguments, nominal) = self.nominal_instance(current, context)?;
                let HirNominalShape::Enum { variants } = nominal.shape() else {
                    return Err(MirInvariantError::new(
                        context,
                        "variant tuple projection has a non-enum base",
                    ));
                };
                self.verify_variant_owner(owner, *variant, context)?;
                let payload = variants
                    .iter()
                    .find(|candidate| candidate.member() == *variant)
                    .map(|variant| variant.payload())
                    .ok_or_else(|| {
                        MirInvariantError::new(context, "variant is absent from its enum HIR shape")
                    })?;
                let HirVariantPayload::Tuple(items) = payload else {
                    return Err(MirInvariantError::new(
                        context,
                        "tuple payload projection targets a non-tuple variant",
                    ));
                };
                let template = items.get(*index as usize).copied().ok_or_else(|| {
                    MirInvariantError::new(context, "variant tuple index is out of range")
                })?;
                if !self.type_matches_substitution(template, declared, arguments, context)? {
                    return Err(MirInvariantError::new(
                        context,
                        "variant tuple projection payload type is inconsistent",
                    ));
                }
                Ok(declared)
            }
            MirProjectionKind::VariantField { variant, field } => {
                let (owner, arguments, nominal) = self.nominal_instance(current, context)?;
                let HirNominalShape::Enum { variants } = nominal.shape() else {
                    return Err(MirInvariantError::new(
                        context,
                        "variant field projection has a non-enum base",
                    ));
                };
                self.verify_variant_owner(owner, *variant, context)?;
                let payload = variants
                    .iter()
                    .find(|candidate| candidate.member() == *variant)
                    .map(|variant| variant.payload())
                    .ok_or_else(|| {
                        MirInvariantError::new(context, "variant is absent from its enum HIR shape")
                    })?;
                let HirVariantPayload::Record(fields) = payload else {
                    return Err(MirInvariantError::new(
                        context,
                        "record payload projection targets a non-record variant",
                    ));
                };
                let declaration = self.resolved.member(*field).ok_or_else(|| {
                    MirInvariantError::new(context, "variant field is not resolved")
                })?;
                if declaration.owner() != MemberOwner::Variant(*variant)
                    || declaration.kind() != MemberKind::VariantField
                {
                    return Err(MirInvariantError::new(
                        context,
                        "variant field has the wrong owner or member kind",
                    ));
                }
                let template = fields
                    .iter()
                    .find(|candidate| candidate.member() == *field)
                    .map(|field| field.ty())
                    .ok_or_else(|| {
                        MirInvariantError::new(context, "field is absent from the variant payload")
                    })?;
                if !self.type_matches_substitution(template, declared, arguments, context)? {
                    return Err(MirInvariantError::new(
                        context,
                        "variant field projection payload type is inconsistent",
                    ));
                }
                Ok(declared)
            }
            MirProjectionKind::OptionValue => {
                let TypeKind::Option(item) = self.kind(current, context)? else {
                    return Err(MirInvariantError::new(
                        context,
                        "option payload projection has a non-option base",
                    ));
                };
                Ok(*item)
            }
            MirProjectionKind::ResultOkValue => {
                let TypeKind::Result { success, .. } = self.kind(current, context)? else {
                    return Err(MirInvariantError::new(
                        context,
                        "ok payload projection has a non-result base",
                    ));
                };
                Ok(*success)
            }
            MirProjectionKind::ResultErrValue => {
                let TypeKind::Result { error, .. } = self.kind(current, context)? else {
                    return Err(MirInvariantError::new(
                        context,
                        "err payload projection has a non-result base",
                    ));
                };
                Ok(*error)
            }
            MirProjectionKind::UnionValue(member) => {
                self.verify_type(*member, context)?;
                let TypeKind::Union(members) = self.kind(current, context)? else {
                    return Err(MirInvariantError::new(
                        context,
                        "union payload projection has a non-union base",
                    ));
                };
                if !members.contains(member) {
                    return Err(MirInvariantError::new(
                        context,
                        "union projection member is absent from the union",
                    ));
                }
                Ok(*member)
            }
            MirProjectionKind::ArrayPatternIndex(_) => {
                Ok(self.intrinsic_arguments(current, IntrinsicType::Array, context)?[0])
            }
            MirProjectionKind::ArrayPatternRest { start, suffix } => {
                let _ = self.intrinsic_arguments(current, IntrinsicType::Array, context)?;
                start.checked_add(*suffix).ok_or_else(|| {
                    MirInvariantError::new(context, "array rest projection offsets overflow")
                })?;
                Ok(current)
            }
            MirProjectionKind::IteratorElement { index } => {
                if self.local(function, *index, context)?.ty
                    != self.hir.interner().scalar(ScalarType::Int)
                {
                    return Err(MirInvariantError::new(
                        context,
                        "borrowed iterator element position is not Int",
                    ));
                }
                let expected = self
                    .iterated_borrowed_item_type(current, context)?
                    .ok_or_else(|| {
                        MirInvariantError::new(
                            context,
                            "borrowed iterator element has a non-borrowable collection base",
                        )
                    })?;
                if expected != declared {
                    return Err(MirInvariantError::new(
                        context,
                        "borrowed iterator element declares the wrong item type",
                    ));
                }
                Ok(expected)
            }
            MirProjectionKind::IteratorSource => {
                let TypeKind::Cursor { collection, .. } = self.kind(current, context)? else {
                    return Err(MirInvariantError::new(
                        context,
                        "iterator source projection has a non-cursor base",
                    ));
                };
                if *collection != declared {
                    return Err(MirInvariantError::new(
                        context,
                        "iterator source projection declares the wrong collection type",
                    ));
                }
                Ok(*collection)
            }
            MirProjectionKind::Index { index, access } => {
                if *access == HirIndexAccess::String {
                    return Err(MirInvariantError::new(
                        context,
                        "String indexing cannot form a place projection",
                    ));
                }
                let index_type = self.local(function, *index, context)?.ty;
                self.verify_index_result(current, index_type, *access, declared, context)?;
                Ok(declared)
            }
            MirProjectionKind::Slice { start, end, step } => {
                let _ = self.intrinsic_arguments(current, IntrinsicType::Array, context)?;
                for local in start.iter().chain(end).chain(step) {
                    if self.local(function, *local, context)?.ty
                        != self.hir.interner().scalar(ScalarType::Int)
                    {
                        return Err(MirInvariantError::new(
                            context,
                            "slice projection bound local is not Int",
                        ));
                    }
                }
                Ok(current)
            }
        }
    }

    fn verify_index_result(
        &self,
        base: TypeId,
        index: TypeId,
        access: HirIndexAccess,
        result: TypeId,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        let valid = match access {
            HirIndexAccess::Array => {
                let arguments = self.intrinsic_arguments(base, IntrinsicType::Array, context)?;
                index == self.hir.interner().scalar(ScalarType::Int) && result == arguments[0]
            }
            HirIndexAccess::String => {
                base == self.hir.interner().scalar(ScalarType::String)
                    && index == self.hir.interner().scalar(ScalarType::Int)
                    && result == self.hir.interner().scalar(ScalarType::Char)
            }
            HirIndexAccess::MapLookup | HirIndexAccess::MapEntry => {
                let arguments = self.intrinsic_arguments(base, IntrinsicType::Map, context)?;
                if index != arguments[0] {
                    false
                } else if access == HirIndexAccess::MapEntry {
                    result == arguments[1]
                } else {
                    !matches!(
                        self.hir
                            .capability_status(arguments[1], HirCapability::Copy),
                        None | Some(HirCapabilityStatus::Unsatisfied)
                    ) && matches!(self.kind(result, context)?, TypeKind::Option(item) if *item == arguments[1])
                }
            }
        };
        if !valid {
            return Err(MirInvariantError::new(
                context,
                "index base, key, access kind, or result type is inconsistent",
            ));
        }
        Ok(())
    }

    fn verify_variant_owner(
        &self,
        owner: SymbolId,
        variant: crate::resolve::MemberId,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        let declaration = self.resolved.member(variant).ok_or_else(|| {
            MirInvariantError::new(context, "variant references an unknown member")
        })?;
        if declaration.owner() != MemberOwner::Type(owner)
            || declaration.kind() != MemberKind::EnumVariant
        {
            return Err(MirInvariantError::new(
                context,
                "variant has the wrong enum owner or member kind",
            ));
        }
        Ok(())
    }

    fn verify_operand_types(
        &self,
        values: &[MirOperand],
        expected: &[TypeId],
        context: &str,
    ) -> Result<(), MirInvariantError> {
        if values.len() != expected.len()
            || values
                .iter()
                .zip(expected)
                .any(|(value, expected)| value.ty != *expected)
        {
            return Err(MirInvariantError::new(
                context,
                "aggregate operand arity or type is inconsistent",
            ));
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn verify_variant_payload(
        &self,
        owner: SymbolId,
        variant: crate::resolve::MemberId,
        payload: &HirVariantPayload,
        fields: &[Option<crate::resolve::MemberId>],
        values: &[MirOperand],
        arguments: &[TypeId],
        context: &str,
    ) -> Result<(), MirInvariantError> {
        self.verify_variant_owner(owner, variant, context)?;
        match payload {
            HirVariantPayload::Unit if fields.is_empty() && values.is_empty() => Ok(()),
            HirVariantPayload::Tuple(types)
                if types.len() == values.len()
                    && fields.len() == values.len()
                    && fields.iter().all(Option::is_none) =>
            {
                for (template, value) in types.iter().zip(values) {
                    if !self.type_matches_substitution(*template, value.ty, arguments, context)? {
                        return Err(MirInvariantError::new(
                            context,
                            "variant tuple payload type is inconsistent",
                        ));
                    }
                }
                Ok(())
            }
            HirVariantPayload::Record(declared)
                if declared.len() == values.len() && fields.len() == values.len() =>
            {
                for ((field, value), declaration) in fields.iter().zip(values).zip(declared.iter())
                {
                    if *field != Some(declaration.member())
                        || self
                            .resolved
                            .member(declaration.member())
                            .is_none_or(|member| {
                                member.owner() != MemberOwner::Variant(variant)
                                    || member.kind() != MemberKind::VariantField
                            })
                        || !self.type_matches_substitution(
                            declaration.ty(),
                            value.ty,
                            arguments,
                            context,
                        )?
                    {
                        return Err(MirInvariantError::new(
                            context,
                            "variant record field identity or type is inconsistent",
                        ));
                    }
                }
                Ok(())
            }
            _ => Err(MirInvariantError::new(
                context,
                "variant payload shape or arity is inconsistent",
            )),
        }
    }

    fn verify_constant(
        &self,
        constant: &super::MirConstant,
        ty: TypeId,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        let valid = match constant {
            super::MirConstant::Unit => ty == self.hir.interner().scalar(ScalarType::Unit),
            super::MirConstant::Bool(_) => ty == self.hir.interner().scalar(ScalarType::Bool),
            super::MirConstant::Integer(_) => {
                matches!(self.kind(ty, context)?, TypeKind::Scalar(scalar) if is_integer(*scalar))
            }
            super::MirConstant::Float(_) => {
                matches!(self.kind(ty, context)?, TypeKind::Scalar(scalar) if is_float(*scalar))
            }
            super::MirConstant::Char(_) => ty == self.hir.interner().scalar(ScalarType::Char),
            super::MirConstant::String(_) => ty == self.hir.interner().scalar(ScalarType::String),
            super::MirConstant::Named(_) => {
                return Err(MirInvariantError::new(
                    context,
                    "named constant reached literal constant validation",
                ));
            }
        };
        if !valid {
            return Err(MirInvariantError::new(
                context,
                "literal constant payload does not match its type",
            ));
        }
        Ok(())
    }

    fn verify_tag(
        &self,
        value: TypeId,
        tag: &MirTag,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        let valid = match (self.kind(value, context)?, tag) {
            (TypeKind::Option(_), MirTag::OptionNone | MirTag::OptionSome) => true,
            (TypeKind::Result { .. }, MirTag::ResultOk | MirTag::ResultErr) => true,
            (
                TypeKind::Intrinsic {
                    constructor: IntrinsicType::NumericConversionError,
                    arguments,
                },
                MirTag::NumericConversionError(_),
            ) => arguments.is_empty(),
            (TypeKind::Union(members), MirTag::Union(member)) => {
                self.verify_type(*member, context)?;
                members.contains(member)
            }
            (TypeKind::Nominal { .. }, MirTag::Variant(variant)) => {
                let (owner, _, nominal) = self.nominal_instance(value, context)?;
                matches!(
                    nominal.shape(),
                    HirNominalShape::Enum { variants }
                        if variants.iter().any(|candidate| candidate.member() == *variant)
                            && self.verify_variant_owner(owner, *variant, context).is_ok()
                )
            }
            _ => false,
        };
        if !valid {
            return Err(MirInvariantError::new(
                context,
                "switch tag is incompatible with its value type",
            ));
        }
        Ok(())
    }

    fn verify_iterator(
        &self,
        state: TypeId,
        destination: TypeId,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        let TypeKind::Cursor { collection, .. } = self.kind(state, context)? else {
            return Err(MirInvariantError::new(
                context,
                "iterator state is not a concrete intrinsic cursor",
            ));
        };
        let valid = match self.kind(*collection, context)? {
            TypeKind::Intrinsic {
                constructor: IntrinsicType::Array | IntrinsicType::Set | IntrinsicType::Range,
                arguments,
            } => destination == arguments[0],
            TypeKind::Intrinsic {
                constructor: IntrinsicType::Map,
                arguments,
            } => matches!(
                self.kind(destination, context)?,
                TypeKind::Tuple(items) if items == arguments
            ),
            TypeKind::Scalar(ScalarType::String) => {
                destination == self.hir.interner().scalar(ScalarType::Char)
            }
            _ => false,
        };
        if !valid {
            return Err(MirInvariantError::new(
                context,
                "iterator state and yielded destination types are inconsistent",
            ));
        }
        Ok(())
    }

    fn iterated_borrowed_item_type(
        &self,
        collection: TypeId,
        context: &str,
    ) -> Result<Option<TypeId>, MirInvariantError> {
        let item = match self.kind(collection, context)? {
            TypeKind::Intrinsic {
                constructor: IntrinsicType::Array | IntrinsicType::Set,
                arguments,
            } => arguments.first().copied(),
            TypeKind::Intrinsic {
                constructor: IntrinsicType::Map,
                arguments,
            } => {
                let mut interner = self.hir.interner().clone();
                Some(
                    interner
                        .tuple(arguments.clone())
                        .map_err(|error| MirInvariantError::new(context, error.to_string()))?,
                )
            }
            _ => None,
        };
        Ok(item)
    }

    fn verify_exclusive_iterator_loan_path(
        &self,
        function: &MirFunction,
        place: &MirPlace,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        let mut current = self.local(function, place.local, context)?.ty;
        for (index, projection) in place.projections.iter().enumerate() {
            if matches!(projection.kind, MirProjectionKind::IteratorElement { .. }) {
                match self.kind(current, context)? {
                    TypeKind::Intrinsic {
                        constructor: IntrinsicType::Array,
                        ..
                    } => {}
                    TypeKind::Intrinsic {
                        constructor: IntrinsicType::Map,
                        ..
                    } if matches!(
                        place.projections.get(index + 1).map(|next| &next.kind),
                        Some(MirProjectionKind::TupleField(1))
                    ) => {}
                    TypeKind::Intrinsic {
                        constructor: IntrinsicType::Map,
                        ..
                    } => {
                        return Err(MirInvariantError::new(
                            context,
                            "exclusive Map iterator loan does not project through its value",
                        ));
                    }
                    _ => {
                        return Err(MirInvariantError::new(
                            context,
                            "exclusive iterator loan has a non-mutable collection source",
                        ));
                    }
                }
                return Ok(());
            }
            current = projection.ty;
        }
        Ok(())
    }

    fn verify_borrowed_iterator_origin(
        &self,
        function: &MirFunction,
        state: &MirPlace,
        destination: &MirPlace,
        source: &MirPlace,
        mode: CursorMode,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        if !state.projections.is_empty()
            || state.source_loan.is_some()
            || !destination.projections.is_empty()
            || destination.source_loan.is_some()
            || self.local(function, state.local, context)?.kind() != MirLocalKind::Temporary
            || self.local(function, destination.local, context)?.kind() != MirLocalKind::Temporary
        {
            return Err(MirInvariantError::new(
                context,
                "borrowed iterator state and position must be direct temporaries",
            ));
        }
        let source_loan = source.source_loan.ok_or_else(|| {
            MirInvariantError::new(context, "borrowed iterator source has no region loan")
        })?;
        let loan = self.loan(function, source_loan, context)?;
        let expected_mode = match mode {
            CursorMode::Ref => ParameterMode::Ref,
            CursorMode::Mut => ParameterMode::Mut,
            CursorMode::Own => {
                return Err(MirInvariantError::new(
                    context,
                    "owning cursor requested borrowed-origin verification",
                ));
            }
        };
        if loan.kind != MirLoanKind::Region
            || loan.mode != expected_mode
            || !same_place_path(&loan.place, source)
        {
            return Err(MirInvariantError::new(
                context,
                "borrowed iterator source is not backed by its exact region loan",
            ));
        }

        let mut state_definitions = 0_u32;
        let mut position_definitions = 0_u32;
        for block in &function.blocks {
            self.consume_dataflow_step(context)?;
            for statement in &block.statements {
                self.consume_dataflow_step(context)?;
                let MirStatementKind::Assign {
                    destination: assigned,
                    value,
                } = &statement.kind
                else {
                    continue;
                };
                if assigned.local == destination.local {
                    return Err(MirInvariantError::new(
                        context,
                        "borrowed iterator position has a non-canonical definition",
                    ));
                }
                if assigned.local == state.local {
                    let matches_origin = assigned.projections.is_empty()
                        && assigned.source_loan.is_none()
                        && matches!(
                            &value.kind,
                            MirRvalueKind::IteratorState {
                                source: MirOperand {
                                    kind: MirOperandKind::Borrow(origin),
                                    ..
                                }
                            } if origin == source
                        );
                    if !matches_origin {
                        return Err(MirInvariantError::new(
                            context,
                            "borrowed iterator state has a non-canonical definition",
                        ));
                    }
                    state_definitions = state_definitions.saturating_add(1);
                }
            }
            match &block.terminator.kind {
                MirTerminatorKind::IteratorNext {
                    state: candidate_state,
                    destination: assigned,
                    borrowed_source: Some(candidate_source),
                    ..
                } if assigned.local == destination.local => {
                    if candidate_state != state
                        || assigned != destination
                        || candidate_source != source
                    {
                        return Err(MirInvariantError::new(
                            context,
                            "borrowed iterator position has a non-canonical producer",
                        ));
                    }
                    position_definitions = position_definitions.saturating_add(1);
                }
                MirTerminatorKind::IteratorNext {
                    destination: assigned,
                    ..
                }
                | MirTerminatorKind::Invoke {
                    destination: Some(assigned),
                    ..
                } if assigned.local == destination.local => {
                    return Err(MirInvariantError::new(
                        context,
                        "borrowed iterator position has a non-canonical producer",
                    ));
                }
                MirTerminatorKind::IteratorNext {
                    destination: assigned,
                    ..
                }
                | MirTerminatorKind::Invoke {
                    destination: Some(assigned),
                    ..
                } if assigned.local == state.local => {
                    return Err(MirInvariantError::new(
                        context,
                        "borrowed iterator state has a non-canonical definition",
                    ));
                }
                _ => {}
            }
        }
        if state_definitions != 1 {
            return Err(MirInvariantError::new(
                context,
                "borrowed iterator state must have exactly one canonical definition",
            ));
        }
        if position_definitions != 1 {
            return Err(MirInvariantError::new(
                context,
                "borrowed iterator position must have exactly one canonical producer",
            ));
        }
        Ok(())
    }

    fn verify_iterator_element_origin(
        &self,
        function: &MirFunction,
        base: &MirPlace,
        index: MirLocalId,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        let Some(origin_loan) = base.source_loan else {
            return Err(MirInvariantError::new(
                context,
                "borrowed iterator element has no source region",
            ));
        };
        let mut producers = 0_u32;
        for block in &function.blocks {
            self.consume_dataflow_step(context)?;
            let MirTerminatorKind::IteratorNext {
                destination,
                borrowed_source: Some(source),
                ..
            } = &block.terminator.kind
            else {
                continue;
            };
            if destination.local == index
                && destination.projections.is_empty()
                && destination.source_loan.is_none()
                && same_place_path(base, source)
            {
                let expected_loan = source.source_loan.ok_or_else(|| {
                    MirInvariantError::new(context, "borrowed iterator source has no region loan")
                })?;
                if !self.region_loan_descends_from(function, origin_loan, expected_loan, context)? {
                    return Err(MirInvariantError::new(
                        context,
                        "borrowed iterator element does not derive from its source region",
                    ));
                }
                producers = producers.saturating_add(1);
            }
        }
        if producers != 1 {
            return Err(MirInvariantError::new(
                context,
                "borrowed iterator element does not have one matching iterator position source",
            ));
        }
        Ok(())
    }

    fn region_loan_descends_from(
        &self,
        function: &MirFunction,
        mut candidate: MirLoanId,
        ancestor: MirLoanId,
        context: &str,
    ) -> Result<bool, MirInvariantError> {
        let mut seen = BTreeSet::new();
        loop {
            self.consume_dataflow_step(context)?;
            if candidate == ancestor {
                return Ok(true);
            }
            if !seen.insert(candidate) {
                return Err(MirInvariantError::new(
                    context,
                    "borrowed iterator source region chain contains a cycle",
                ));
            }
            let loan = self.loan(function, candidate, context)?;
            if loan.kind != MirLoanKind::Region {
                return Ok(false);
            }
            let Some(parent) = loan.place.source_loan else {
                return Ok(false);
            };
            candidate = parent;
        }
    }

    fn iterated_item_type(&self, source: TypeId) -> Option<TypeId> {
        match self.hir.interner().kind(source).ok()? {
            TypeKind::Intrinsic {
                constructor: IntrinsicType::Array | IntrinsicType::Set | IntrinsicType::Range,
                arguments,
            } => Some(arguments[0]),
            TypeKind::Scalar(ScalarType::String) => {
                Some(self.hir.interner().scalar(ScalarType::Char))
            }
            TypeKind::Intrinsic {
                constructor: IntrinsicType::Map,
                ..
            } => Some(source),
            _ => None,
        }
    }

    fn array_element(&self, ty: TypeId) -> Option<TypeId> {
        match self.hir.interner().kind(ty).ok()? {
            TypeKind::Intrinsic {
                constructor: IntrinsicType::Array,
                arguments,
            } => Some(arguments[0]),
            _ => None,
        }
    }

    fn is_array(&self, ty: TypeId) -> bool {
        self.array_element(ty).is_some()
    }

    fn place_represents_source_local(
        &self,
        function: &MirFunction,
        place: &MirPlace,
        source: crate::resolve::LocalId,
    ) -> bool {
        if place.projections.is_empty() {
            return matches!(
                function.locals.get(place.local.0 as usize).map(|local| local.kind),
                Some(MirLocalKind::User(candidate))
                    | Some(MirLocalKind::Parameter {
                        source: Some(candidate),
                        ..
                    }) if candidate == source
            );
        }
        let (
            MirFunctionId::Closure(function_closure),
            [
                MirProjection {
                    kind: MirProjectionKind::ClosureCapture { closure, index },
                    ..
                },
            ],
        ) = (function.id, place.projections.as_slice())
        else {
            return false;
        };
        function_closure == *closure
            && function.parameters.first() == Some(&place.local)
            && self
                .hir
                .closure(*closure)
                .and_then(|metadata| metadata.captures().get(*index as usize))
                .is_some_and(|capture| capture.local() == source)
    }

    fn function_is_async(
        &self,
        function: &MirFunction,
        context: &str,
    ) -> Result<bool, MirInvariantError> {
        let function_type = match function.id {
            MirFunctionId::Callable(id) => self
                .hir
                .callable(id)
                .ok_or_else(|| MirInvariantError::new(context, "missing HIR callable metadata"))?
                .function_type(),
            MirFunctionId::Closure(id) => self
                .hir
                .closure(id)
                .ok_or_else(|| MirInvariantError::new(context, "missing HIR closure metadata"))?
                .function_type(),
        };
        let TypeKind::Function(signature) = self.kind(function_type, context)? else {
            return Err(MirInvariantError::new(
                context,
                "MIR function has a non-function HIR signature",
            ));
        };
        Ok(signature.is_async())
    }

    fn join_logical_outcome(
        &self,
        join: TypeId,
        context: &str,
    ) -> Result<TypeId, MirInvariantError> {
        let arguments = self.intrinsic_arguments(join, IntrinsicType::Join, context)?;
        let [success, error] = arguments else {
            return Err(MirInvariantError::new(
                context,
                "Join has the wrong intrinsic arity",
            ));
        };
        if *error == self.hir.interner().scalar(ScalarType::Never) {
            return Ok(*success);
        }
        let mut interner = self.hir.interner().clone();
        interner
            .result(*success, *error)
            .map_err(|error| MirInvariantError::new(context, error.to_string()))
    }

    fn is_join_for_outcome(
        &self,
        join: TypeId,
        outcome: TypeId,
        context: &str,
    ) -> Result<bool, MirInvariantError> {
        let TypeKind::Intrinsic {
            constructor: IntrinsicType::Join,
            arguments,
        } = self.kind(join, context)?
        else {
            return Ok(false);
        };
        let (success, error) = match self.kind(outcome, context)? {
            TypeKind::Result { success, error } => (*success, *error),
            _ => (outcome, self.hir.interner().scalar(ScalarType::Never)),
        };
        Ok(arguments.as_slice() == [success, error])
    }

    fn verify_spawn_transfer(
        &self,
        function: &MirFunction,
        operation: &MirOperation,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        let MirOperationKind::Call {
            callee, arguments, ..
        } = operation.kind()
        else {
            return Err(MirInvariantError::new(
                context,
                "spawn does not contain a call operation",
            ));
        };
        let affine_host_wait = matches!(
            callee.kind(),
            MirOperandKind::Function {
                callable: HirCallableId::Host(HirBootstrapHostFunction::AsyncWaiterWait),
                ..
            }
        );
        self.require_capability(function.id, callee.ty(), HirCapability::Send, context)?;
        for argument in arguments {
            match argument.mode() {
                ParameterMode::Value => {
                    self.require_capability(
                        function.id,
                        argument.value().ty(),
                        HirCapability::Send,
                        context,
                    )?;
                }
                ParameterMode::Ref => {
                    self.require_capability(
                        function.id,
                        argument.value().ty(),
                        HirCapability::Send,
                        context,
                    )?;
                    self.require_capability(
                        function.id,
                        argument.value().ty(),
                        HirCapability::Share,
                        context,
                    )?;
                }
                ParameterMode::Mut | ParameterMode::Var => {
                    if !affine_host_wait {
                        return Err(MirInvariantError::new(
                            context,
                            "spawn carries an exclusive argument loan",
                        ));
                    }
                    self.require_capability(
                        function.id,
                        argument.value().ty(),
                        HirCapability::Send,
                        context,
                    )?;
                }
            }
        }
        let (success, error) = match self.kind(operation.ty(), context)? {
            TypeKind::Result { success, error } => (*success, *error),
            _ => (
                operation.ty(),
                self.hir.interner().scalar(ScalarType::Never),
            ),
        };
        self.require_capability(function.id, success, HirCapability::Send, context)?;
        self.require_capability(function.id, error, HirCapability::Send, context)
    }

    fn require_capability(
        &self,
        function: MirFunctionId,
        ty: TypeId,
        capability: HirCapability,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        if self.capability_status(function, ty, capability, context)?
            != HirCapabilityStatus::Satisfied
        {
            return Err(MirInvariantError::new(
                context,
                format!("{ty} does not satisfy {capability:?}"),
            ));
        }
        Ok(())
    }

    fn verify_call(
        &self,
        function: &MirFunction,
        verification: MirCallVerification<'_>,
        operation_context: MirOperationContext,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        let MirCallVerification {
            callee,
            arguments,
            signature,
            protocol,
            unsafe_call,
            outcome,
        } = verification;
        let TypeKind::Function(call_signature) = self.kind(signature, context)? else {
            return Err(MirInvariantError::new(
                context,
                "call operation signature is not a function",
            ));
        };
        if call_signature.is_async() != operation_context.expects_async()
            || call_signature.is_unsafe() != unsafe_call
        {
            return Err(MirInvariantError::new(
                context,
                "call effects differ from their MIR initiation context",
            ));
        }
        if operation_context == MirOperationContext::Select && !call_signature.is_selectable() {
            return Err(MirInvariantError::new(
                context,
                "select registration call is async but not selectable",
            ));
        }
        if call_signature.outcome() != outcome {
            return Err(MirInvariantError::new(
                context,
                "call operation outcome differs from its function type",
            ));
        }
        let available = match self.kind(callee.ty, context)? {
            TypeKind::Function(_) => {
                if callee.ty == signature {
                    HirClosureProtocols::new(true, true, true)
                } else {
                    HirClosureProtocols::new(false, false, false)
                }
            }
            TypeKind::Generated {
                identity,
                arguments,
            } => {
                if let Some(closure) = self.hir.closure_by_identity(identity) {
                    let mut interner = self.hir.interner().clone();
                    let actual = TypeSubstitution::new(arguments.clone())
                        .apply(&mut interner, closure.function_type())
                        .map_err(|error| MirInvariantError::new(context, error.to_string()))?;
                    if actual == signature {
                        closure.protocols()
                    } else {
                        HirClosureProtocols::new(false, false, false)
                    }
                } else {
                    HirClosureProtocols::new(false, false, false)
                }
            }
            TypeKind::GenericParameter(position) => {
                self.generic_call_protocols(function, *position, signature, context)?
            }
            TypeKind::OpaqueResult {
                identity,
                arguments,
            } => self.opaque_call_protocols(identity, arguments, signature, context)?,
            _ => HirClosureProtocols::new(false, false, false),
        };
        let expected_protocol = if operation_context == MirOperationContext::Spawn {
            available
                .supports(HirCallProtocol::CallOnce)
                .then_some(HirCallProtocol::CallOnce)
        } else if operation_context.is_deferred()
            && matches!(callee.kind, MirOperandKind::Move(_))
            && available.supports(HirCallProtocol::CallOnce)
        {
            Some(HirCallProtocol::CallOnce)
        } else if available.supports(HirCallProtocol::Call) {
            Some(HirCallProtocol::Call)
        } else if available.supports(HirCallProtocol::CallMut)
            && matches!(callee.kind, MirOperandKind::Borrow(_))
        {
            Some(HirCallProtocol::CallMut)
        } else if available.supports(HirCallProtocol::CallOnce)
            && !matches!(callee.kind, MirOperandKind::Borrow(_))
        {
            Some(HirCallProtocol::CallOnce)
        } else {
            None
        };
        if expected_protocol != Some(protocol) {
            return Err(MirInvariantError::new(
                context,
                format!(
                    "call operation records {protocol:?}, expected {expected_protocol:?} from its closed callee contract"
                ),
            ));
        }
        match protocol {
            crate::hir::HirCallProtocol::CallMut
                if !matches!(callee.kind, MirOperandKind::Borrow(_)) =>
            {
                return Err(MirInvariantError::new(
                    context,
                    "CallMut callee is not an exclusive environment borrow",
                ));
            }
            crate::hir::HirCallProtocol::CallOnce
                if matches!(callee.kind, MirOperandKind::Borrow(_)) =>
            {
                return Err(MirInvariantError::new(
                    context,
                    "CallOnce callee cannot be an environment borrow",
                ));
            }
            _ => {}
        }

        let callable = match &callee.kind {
            MirOperandKind::Function { callable, .. } => self.hir.callable(*callable),
            _ => None,
        };
        let mut fixed = Vec::new();
        let mut receiver = None;
        if let MirOperandKind::PreludeTraitFunction { method, .. } = callee.kind {
            if call_signature.variadic().is_some() || call_signature.parameters().is_empty() {
                return Err(MirInvariantError::new(
                    context,
                    "prelude trait callable does not have a fixed protocol target",
                ));
            }
            let has_receiver = method.has_receiver();
            for (index, parameter) in call_signature.parameters().iter().enumerate() {
                let association = if has_receiver && index == 0 {
                    crate::hir::HirCallArgumentTarget::Receiver
                } else {
                    crate::hir::HirCallArgumentTarget::Fixed(index as u32)
                };
                let item = (association, parameter.mode(), parameter.ty());
                if has_receiver && index == 0 {
                    receiver = Some(item);
                } else {
                    fixed.push(item);
                }
            }
        } else if let Some(callable) = callable {
            let mut concrete = call_signature.parameters().iter();
            for (source_index, parameter) in callable.parameters().iter().enumerate() {
                if parameter.variadic_element().is_some() {
                    continue;
                }
                let concrete = concrete.next().ok_or_else(|| {
                    MirInvariantError::new(
                        context,
                        "callable HIR has more fixed parameters than its function type",
                    )
                })?;
                let association = if parameter.is_receiver() {
                    crate::hir::HirCallArgumentTarget::Receiver
                } else {
                    crate::hir::HirCallArgumentTarget::Fixed(source_index as u32)
                };
                let item = (association, concrete.mode(), concrete.ty());
                if parameter.is_receiver() {
                    if receiver.replace(item).is_some() {
                        return Err(MirInvariantError::new(
                            context,
                            "callable has more than one receiver parameter",
                        ));
                    }
                } else {
                    fixed.push(item);
                }
            }
            if concrete.next().is_some() {
                return Err(MirInvariantError::new(
                    context,
                    "function type has excess fixed parameters",
                ));
            }
        } else {
            fixed.extend(call_signature.parameters().iter().enumerate().map(
                |(index, parameter)| {
                    (
                        crate::hir::HirCallArgumentTarget::Fixed(index as u32),
                        parameter.mode(),
                        parameter.ty(),
                    )
                },
            ));
        }

        let mut provided = Vec::new();
        let mut spread = false;
        for (position, argument) in arguments.iter().enumerate() {
            let expected = match argument.target {
                crate::hir::HirCallArgumentTarget::Receiver => receiver,
                crate::hir::HirCallArgumentTarget::Fixed(index) => fixed
                    .iter()
                    .find(|(target, _, _)| {
                        *target == crate::hir::HirCallArgumentTarget::Fixed(index)
                    })
                    .copied(),
                crate::hir::HirCallArgumentTarget::VariadicElement => call_signature
                    .variadic()
                    .map(|ty| (argument.target, crate::types::ParameterMode::Value, ty)),
                crate::hir::HirCallArgumentTarget::VariadicSpread => {
                    if spread || position + 1 != arguments.len() {
                        return Err(MirInvariantError::new(
                            context,
                            "variadic spread is repeated or is not the final argument",
                        ));
                    }
                    spread = true;
                    let element = call_signature.variadic().ok_or_else(|| {
                        MirInvariantError::new(
                            context,
                            "variadic spread targets a non-variadic function",
                        )
                    })?;
                    let valid = matches!(
                        self.kind(argument.value.ty, context)?,
                        TypeKind::Intrinsic {
                            constructor: IntrinsicType::Array,
                            arguments,
                        } if arguments == &[element]
                    );
                    if !valid || argument.mode != crate::types::ParameterMode::Value {
                        return Err(MirInvariantError::new(
                            context,
                            "variadic spread must pass Array[element] by value",
                        ));
                    }
                    continue;
                }
                crate::hir::HirCallArgumentTarget::Invalid => None,
            }
            .ok_or_else(|| {
                MirInvariantError::new(
                    context,
                    format!(
                        "call argument association {:?} has no parameter",
                        argument.target
                    ),
                )
            })?;
            if matches!(
                argument.target,
                crate::hir::HirCallArgumentTarget::Receiver
                    | crate::hir::HirCallArgumentTarget::Fixed(_)
            ) && provided.contains(&argument.target)
            {
                return Err(MirInvariantError::new(
                    context,
                    "fixed call parameter is provided more than once",
                ));
            }
            if matches!(
                argument.target,
                crate::hir::HirCallArgumentTarget::Receiver
                    | crate::hir::HirCallArgumentTarget::Fixed(_)
            ) {
                provided.push(argument.target);
            }
            if argument.mode != expected.1 || argument.value.ty != expected.2 {
                return Err(MirInvariantError::new(
                    context,
                    "call argument mode or type differs from its parameter",
                ));
            }
            let loans = matches!(argument.value.kind, MirOperandKind::Loan(_));
            if (argument.mode == crate::types::ParameterMode::Value) == loans {
                return Err(MirInvariantError::new(
                    context,
                    "call argument loan access does not match its parameter mode",
                ));
            }
            if let MirOperandKind::Loan(loan) = argument.value.kind {
                let loan = self.loan(function, loan, context)?;
                if loan.mode != argument.mode || loan.place.ty != argument.value.ty {
                    return Err(MirInvariantError::new(
                        context,
                        "call argument differs from its reserved loan metadata",
                    ));
                }
            }
        }
        let expected_fixed = fixed.len() + usize::from(receiver.is_some());
        if provided.len() != expected_fixed {
            return Err(MirInvariantError::new(
                context,
                "call omits one or more fixed parameters",
            ));
        }
        Ok(())
    }

    fn generic_call_protocols(
        &self,
        function: &MirFunction,
        position: u32,
        signature: TypeId,
        context: &str,
    ) -> Result<HirClosureProtocols, MirInvariantError> {
        let generics: &[HirGenericParameter] = match function.id {
            MirFunctionId::Callable(callable) => self
                .hir
                .callable(callable)
                .map(|callable| callable.generics())
                .ok_or_else(|| {
                    MirInvariantError::new(context, "MIR function has no HIR callable metadata")
                })?,
            MirFunctionId::Closure(closure) => self
                .hir
                .closure(closure)
                .map(|closure| closure.generics())
                .ok_or_else(|| {
                    MirInvariantError::new(context, "MIR closure has no HIR metadata")
                })?,
        };
        let parameter = generics
            .iter()
            .find(|parameter| parameter.position() == position)
            .ok_or_else(|| {
                MirInvariantError::new(
                    context,
                    format!("generic call target ${position} has no function binder"),
                )
            })?;
        self.call_protocols_from_bounds(
            parameter
                .bounds()
                .iter()
                .map(|bound| (bound.constructor().clone(), bound.arguments().to_vec())),
            signature,
            context,
        )
    }

    fn opaque_call_protocols(
        &self,
        identity: &crate::package::SymbolIdentity,
        arguments: &[TypeId],
        signature: TypeId,
        context: &str,
    ) -> Result<HirClosureProtocols, MirInvariantError> {
        let opaque = self.hir.opaque_result(identity).ok_or_else(|| {
            MirInvariantError::new(context, "opaque call target has no published contract")
        })?;
        let substitution = TypeSubstitution::new(arguments.to_vec());
        let mut interner = self.hir.interner().clone();
        let bounds = opaque
            .bounds()
            .iter()
            .map(|bound| {
                Ok((
                    bound.constructor().clone(),
                    bound
                        .arguments()
                        .iter()
                        .map(|argument| {
                            substitution
                                .apply(&mut interner, *argument)
                                .map_err(|error| MirInvariantError::new(context, error.to_string()))
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                ))
            })
            .collect::<Result<Vec<_>, MirInvariantError>>()?;
        self.call_protocols_from_bounds(bounds, signature, context)
    }

    fn call_protocols_from_bounds(
        &self,
        bounds: impl IntoIterator<Item = (HirTraitConstructor, Vec<TypeId>)>,
        signature: TypeId,
        context: &str,
    ) -> Result<HirClosureProtocols, MirInvariantError> {
        let mut call = false;
        let mut call_mut = false;
        let mut call_once = false;
        let mut discard = false;
        for (constructor, arguments) in bounds {
            let HirTraitConstructor::Prelude(name) = constructor else {
                continue;
            };
            match (name.as_str(), arguments.as_slice()) {
                ("Call", [actual]) if *actual == signature => call = true,
                ("CallMut", [actual]) if *actual == signature => call_mut = true,
                ("CallOnce", [actual]) if *actual == signature => call_once = true,
                ("Discard", []) => discard = true,
                ("Call" | "CallMut" | "CallOnce", [_]) => {}
                ("Call" | "CallMut" | "CallOnce", _) => {
                    return Err(MirInvariantError::new(
                        context,
                        "call protocol bound has an invalid signature arity",
                    ));
                }
                _ => {}
            }
        }
        call_mut |= call;
        call_once |= discard && call_mut;
        Ok(HirClosureProtocols::new(call, call_mut, call_once))
    }

    fn verify_span(
        &self,
        function: &MirFunction,
        span: crate::source::Span,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        if span.file() != function.span.file() {
            return Err(MirInvariantError::new(
                context,
                "source span belongs to a different file than its MIR function",
            ));
        }
        Ok(())
    }

    fn verify_control_and_dataflow(&self, function: &MirFunction) -> Result<(), MirInvariantError> {
        let context = function_context(function.id);
        let events = function
            .blocks
            .iter()
            .map(|block| self.local_events(function, block))
            .collect::<Vec<_>>();
        let successors = function
            .blocks
            .iter()
            .map(|block| successor_edges(&block.terminator.kind))
            .collect::<Vec<_>>();
        let mut predecessors =
            vec![Vec::<(MirBlockId, SuccessorEdge)>::new(); function.blocks.len()];
        for (source, edges) in successors.iter().enumerate() {
            for edge in edges {
                predecessors[edge.target.0 as usize]
                    .push((MirBlockId(source as u32), edge.clone()));
            }
        }
        if !predecessors[function.entry.0 as usize].is_empty() {
            return Err(MirInvariantError::new(
                &context,
                "entry block has an incoming control-flow edge",
            ));
        }

        let mut reachable = vec![false; function.blocks.len()];
        let mut queue = VecDeque::from([function.entry]);
        reachable[function.entry.0 as usize] = true;
        while let Some(block) = queue.pop_front() {
            for edge in &successors[block.0 as usize] {
                let index = edge.target.0 as usize;
                if !reachable[index] {
                    reachable[index] = true;
                    queue.push_back(edge.target);
                }
            }
        }
        for (index, block) in function.blocks.iter().enumerate() {
            if reachable[index] || MirBlockId(index as u32) == function.unwind {
                continue;
            }
            if !block.statements.is_empty()
                || !matches!(block.terminator.kind, MirTerminatorKind::Unreachable)
            {
                return Err(MirInvariantError::new(
                    &context,
                    format!("block#{index} is unreachable but contains executable MIR"),
                ));
            }
        }

        let managed = events
            .iter()
            .flatten()
            .filter_map(|event| match event {
                LocalEvent::StorageLive(local) | LocalEvent::StorageDead(local) => Some(*local),
                LocalEvent::Read(_)
                | LocalEvent::Resolve(_)
                | LocalEvent::Move(_)
                | LocalEvent::Write(_)
                | LocalEvent::WriteAccess(_) => None,
            })
            .collect::<BTreeSet<_>>();
        let mut relevant = events
            .iter()
            .flatten()
            .map(|event| match event {
                LocalEvent::Read(access)
                | LocalEvent::Resolve(access)
                | LocalEvent::Move(access)
                | LocalEvent::Write(access)
                | LocalEvent::WriteAccess(access) => access.local,
                LocalEvent::StorageLive(local) | LocalEvent::StorageDead(local) => *local,
            })
            .collect::<BTreeSet<_>>();
        relevant.insert(function.return_local);
        for edges in &successors {
            relevant.extend(
                edges
                    .iter()
                    .filter_map(|edge| edge.writes.as_ref().map(|place| place.local)),
            );
        }
        for local in relevant {
            self.verify_local_flow(
                function,
                local,
                &events,
                &successors,
                &predecessors,
                &reachable,
                managed.contains(&local),
                &context,
            )?;
        }
        self.verify_loan_flow(function, &reachable, &context)?;
        self.verify_tag_refinements(function, &successors, &reachable, &context)?;
        Ok(())
    }

    /// Selection phase protocol: within a block, `BeginSelect` opens a
    /// region, exactly `capacity` `RegisterSelectArm` steps follow in order,
    /// and only `CommitSelect` may close it.  Loan reservations/releases are
    /// permitted as compiler-generated arm preparation; no user-visible
    /// operation or terminator may run while a region is open, so phases
    /// cannot be skipped, arms cannot commit twice, the payload cannot be
    /// observed before commit, and arm tables stay inside their checked bound.
    fn verify_select_flow(
        &self,
        function: &MirFunction,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        for (block_index, block) in function.blocks.iter().enumerate() {
            let block_context = format!("{context} block#{block_index}");
            let mut open: Option<(u32, u32)> = None;
            let mut registered_types = Vec::new();
            for (sequence, statement) in block.statements.iter().enumerate() {
                let statement_context = format!("{block_context} statement#{sequence}");
                let arm_preparation_allowed =
                    matches!(open, Some((capacity, registered)) if registered < capacity);
                match &statement.kind {
                    MirStatementKind::BeginSelect { capacity } => {
                        if open.is_some() {
                            return Err(MirInvariantError::new(
                                &statement_context,
                                "selection region is re-entered before its commit",
                            ));
                        }
                        open = Some((*capacity, 0));
                        registered_types.clear();
                    }
                    MirStatementKind::RegisterSelectArm {
                        index,
                        registration,
                    } => {
                        let Some((capacity, registered)) = &mut open else {
                            return Err(MirInvariantError::new(
                                &statement_context,
                                "select arm registration outside a selection region",
                            ));
                        };
                        if *index != *registered {
                            return Err(MirInvariantError::new(
                                &statement_context,
                                "select arm registration skips or duplicates a phase",
                            ));
                        }
                        *registered += 1;
                        if *registered > *capacity {
                            return Err(MirInvariantError::new(
                                &statement_context,
                                "select registrations exceed the declared arm bound",
                            ));
                        }
                        registered_types.push(match registration {
                            MirSelectRegistration::Call(operation) => operation.ty,
                            MirSelectRegistration::Join(place) => {
                                self.join_logical_outcome(place.ty(), &statement_context)?
                            }
                        });
                    }
                    MirStatementKind::ReserveLoan(_) | MirStatementKind::ReleaseLoan(_)
                        if arm_preparation_allowed => {}
                    MirStatementKind::StorageLive(_)
                    | MirStatementKind::StorageDead(_)
                    | MirStatementKind::Assign { .. }
                    | MirStatementKind::RetargetCleanup { .. }
                    | MirStatementKind::DisarmCleanup(_)
                        if arm_preparation_allowed => {}
                    _ if open.is_some() => {
                        return Err(MirInvariantError::new(
                            &statement_context,
                            "only arm registration may appear inside a selection region",
                        ));
                    }
                    _ => {}
                }
            }
            match &block.terminator.kind {
                MirTerminatorKind::CommitSelect { arms, .. } => {
                    let Some((capacity, registered)) = open else {
                        return Err(MirInvariantError::new(
                            &block_context,
                            "select commit has no open selection region",
                        ));
                    };
                    if arms.len() as u32 != registered || registered != capacity {
                        return Err(MirInvariantError::new(
                            &block_context,
                            "select commit table does not match its registered arms",
                        ));
                    }
                    for (index, arm) in arms.iter().enumerate() {
                        let Some(payload) = arm.payload() else {
                            continue;
                        };
                        let expected = registered_types.get(index).ok_or_else(|| {
                            MirInvariantError::new(
                                &block_context,
                                "select commit payload has no matching registration",
                            )
                        })?;
                        if payload.ty() != *expected {
                            return Err(MirInvariantError::new(
                                &block_context,
                                format!(
                                    "select payload type {} does not match registration {} type {}",
                                    payload.ty(),
                                    index,
                                    expected
                                ),
                            ));
                        }
                    }
                }
                _ if open.is_some() => {
                    return Err(MirInvariantError::new(
                        &block_context,
                        "selection region reaches a terminator before its commit",
                    ));
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn verify_defer_flow(
        &self,
        function: &MirFunction,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        self.verify_fallback_coverage(function, context)?;
        let entry = self.block(function, function.entry, context)?;
        let registered_entry_owners = entry
            .statements
            .iter()
            .take_while(|statement| {
                matches!(statement.kind, MirStatementKind::RegisterFallback { .. })
            })
            .filter_map(|statement| match &statement.kind {
                MirStatementKind::RegisterFallback { owner, .. } => {
                    Some(LocalAccess::from_place(owner))
                }
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        for owner in self.terminal_entry_owners(function, context)? {
            if !registered_entry_owners.contains(&LocalAccess::from_place(&owner)) {
                return Err(MirInvariantError::new(
                    context,
                    format!(
                        "terminal entry owner rooted at local#{} has no entry fallback registration",
                        owner.local.index()
                    ),
                ));
            }
        }
        let registered_scopes = function
            .blocks
            .iter()
            .flat_map(|block| &block.statements)
            .filter_map(|statement| match &statement.kind {
                MirStatementKind::RegisterDefer { scope, .. } => Some(*scope),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        for (index, block) in function.blocks.iter().enumerate() {
            let scopes = match &block.terminator.kind {
                MirTerminatorKind::DrainDefers { scopes, .. } => scopes,
                MirTerminatorKind::DrainScopes { defer_scopes, .. } => defer_scopes,
                _ => continue,
            };
            if scopes
                .iter()
                .any(|scope| !registered_scopes.contains(scope))
            {
                return Err(MirInvariantError::new(
                    format!("{context} block#{index}"),
                    "defer drain references a scope with no registration",
                ));
            }
        }
        let successors = function
            .blocks
            .iter()
            .map(|block| successor_edges(&block.terminator.kind))
            .collect::<Vec<_>>();
        let mut incoming = vec![BTreeSet::<DeferFlowState>::new(); function.blocks.len()];
        incoming[function.entry.0 as usize].insert(DeferFlowState::default());
        let mut queue = VecDeque::from([function.entry]);
        let mut queued = vec![false; function.blocks.len()];
        queued[function.entry.0 as usize] = true;

        while let Some(block_id) = queue.pop_front() {
            queued[block_id.0 as usize] = false;
            let block = &function.blocks[block_id.0 as usize];
            let block_context = format!("{context} block#{}", block_id.0);
            let local_events = self.local_events(function, block);
            let mut outgoing = BTreeSet::new();
            for mut state in incoming[block_id.0 as usize].clone() {
                self.consume_dataflow_step(context)?;
                apply_consumed_defer_events(&mut state, &local_events, &block_context)?;
                for (index, statement) in block.statements.iter().enumerate() {
                    let statement_context = format!("{block_context} statement#{index}");
                    let advances_pending = match &statement.kind {
                        MirStatementKind::RetargetCleanup { from, .. } => state
                            .pending_moves
                            .contains_key(&LocalAccess::from_place(from)),
                        MirStatementKind::DisarmCleanup(place) => state
                            .pending_moves
                            .contains_key(&LocalAccess::from_place(place)),
                        _ => false,
                    };
                    if !state.pending_moves.is_empty() && !advances_pending {
                        return Err(MirInvariantError::new(
                            statement_context,
                            "guarded move is not immediately followed by its defer transition",
                        ));
                    }
                    match &statement.kind {
                        MirStatementKind::BeginSelect { .. }
                        | MirStatementKind::RegisterSelectArm { .. } => {}
                        MirStatementKind::RegisterDefer { scope, guard, .. } => {
                            state.activate_scope(*scope, &statement_context)?;
                            let registration = (block_id, index);
                            if state
                                .registrations
                                .insert(
                                    registration,
                                    ActiveCleanupRegistration {
                                        scope: *scope,
                                        kind: CleanupEntryKind::Explicit,
                                    },
                                )
                                .is_some()
                            {
                                return Err(MirInvariantError::new(
                                    statement_context,
                                    "defer registration is re-executed before it is drained or disarmed",
                                ));
                            }
                            if let Some(guard) = guard {
                                let terminal = self.terminal_status(
                                    function.id,
                                    guard.ty,
                                    &statement_context,
                                )? != HirTerminalStatus::Absent;
                                let guard = LocalAccess::from_place(guard);
                                let replaced = state
                                    .guards
                                    .iter()
                                    .filter_map(|(existing, active)| {
                                        (active.kind == CleanupEntryKind::Fallback
                                            && local_access_contains(&guard, existing))
                                        .then_some((existing.clone(), *active))
                                    })
                                    .collect::<Vec<_>>();
                                if terminal && replaced.len() != 1 {
                                    return Err(MirInvariantError::new(
                                        statement_context,
                                        "terminal explicit cleanup guard does not replace exactly one fallback",
                                    ));
                                }
                                for (existing, active) in replaced {
                                    state.guards.remove(&existing);
                                    state.registrations.remove(&active.registration);
                                }
                                if state
                                    .guards
                                    .keys()
                                    .any(|existing| local_accesses_overlap(existing, &guard))
                                {
                                    return Err(MirInvariantError::new(
                                        statement_context,
                                        "defer registration overlaps an already-active guard",
                                    ));
                                }
                                state.guards.insert(
                                    guard,
                                    ActiveDeferGuard {
                                        scope: *scope,
                                        registration,
                                        kind: CleanupEntryKind::Explicit,
                                    },
                                );
                            } else {
                                state.unguarded_scopes.insert(*scope);
                            }
                        }
                        MirStatementKind::RegisterFallback { scope, owner } => {
                            let owner = LocalAccess::from_place(owner);
                            if state.guards.iter().any(|(existing, active)| {
                                active.kind == CleanupEntryKind::Explicit
                                    && local_accesses_overlap(existing, &owner)
                            }) {
                                return Err(MirInvariantError::new(
                                    statement_context,
                                    "terminal fallback overlaps an active explicit cleanup guard",
                                ));
                            }
                            if state.guards.iter().any(|(existing, active)| {
                                active.kind == CleanupEntryKind::Fallback
                                    && local_access_contains(existing, &owner)
                            }) {
                                continue;
                            }
                            let folded = state
                                .guards
                                .iter()
                                .filter_map(|(existing, active)| {
                                    (active.kind == CleanupEntryKind::Fallback
                                        && local_access_contains(&owner, existing))
                                    .then_some((existing.clone(), *active))
                                })
                                .collect::<Vec<_>>();
                            for (existing, active) in folded {
                                state.guards.remove(&existing);
                                state.registrations.remove(&active.registration);
                            }
                            let registration = (block_id, index);
                            if state
                                .registrations
                                .insert(
                                    registration,
                                    ActiveCleanupRegistration {
                                        scope: *scope,
                                        kind: CleanupEntryKind::Fallback,
                                    },
                                )
                                .is_some()
                            {
                                return Err(MirInvariantError::new(
                                    statement_context,
                                    "terminal fallback registration is re-executed while active",
                                ));
                            }
                            state.guards.insert(
                                owner,
                                ActiveDeferGuard {
                                    scope: *scope,
                                    registration,
                                    kind: CleanupEntryKind::Fallback,
                                },
                            );
                        }
                        MirStatementKind::RetargetCleanup { from, to } => {
                            let from = LocalAccess::from_place(from);
                            let to = LocalAccess::from_place(to);
                            if let Some(guard) = state.guards.remove(&from) {
                                if state.pending_moves.remove(&from)
                                    != Some(PendingDeferTransition::Retarget)
                                {
                                    return Err(MirInvariantError::new(
                                        statement_context,
                                        "defer guard retarget is not backed by an immediate move",
                                    ));
                                }
                                if state
                                    .guards
                                    .keys()
                                    .any(|existing| local_accesses_overlap(existing, &to))
                                {
                                    return Err(MirInvariantError::new(
                                        statement_context,
                                        "defer guard retarget overlaps another active guard",
                                    ));
                                }
                                state.guards.insert(to, guard);
                            }
                        }
                        MirStatementKind::DisarmCleanup(place) => {
                            let place = LocalAccess::from_place(place);
                            let pending = state.pending_moves.remove(&place);
                            if let Some(guard) = state.guards.remove(&place) {
                                let confirmed = match guard.kind {
                                    CleanupEntryKind::Explicit => defer_disarm_is_confirmed(
                                        function,
                                        block,
                                        index,
                                        &place,
                                        guard.scope,
                                        pending,
                                    ),
                                    CleanupEntryKind::Fallback => {
                                        pending == Some(PendingDeferTransition::Disarm)
                                            || (place.local == function.return_local
                                                && place.path.is_empty()
                                                && matches!(
                                                    block.terminator.kind,
                                                    MirTerminatorKind::Return
                                                ))
                                            || (pending.is_none()
                                                && defer_disarm_is_confirmed(
                                                    function,
                                                    block,
                                                    index,
                                                    &place,
                                                    guard.scope,
                                                    None,
                                                ))
                                    }
                                };
                                if !confirmed {
                                    return Err(MirInvariantError::new(
                                        statement_context,
                                        format!(
                                            "{:?} cleanup guard is disarmed without an immediate consuming handoff or scope exit (pending {pending:?}, terminator {:?})",
                                            guard.kind, block.terminator.kind
                                        ),
                                    ));
                                }
                                state.registrations.remove(&guard.registration);
                                state.remove_inactive_scope(guard.scope);
                            }
                        }
                        MirStatementKind::Assign { destination, value } => {
                            let destination_place = destination;
                            let destination = LocalAccess::from_place(destination_place);
                            if state.guards.iter().any(|(guard, active)| {
                                local_accesses_overlap(guard, &destination)
                                    && (active.kind == CleanupEntryKind::Explicit
                                        || !local_access_contains(guard, &destination)
                                        || guard == &destination)
                            }) {
                                return Err(MirInvariantError::new(
                                    statement_context,
                                    "assignment overwrites an active cleanup guard",
                                ));
                            }
                            let mut events = Vec::new();
                            push_rvalue_events(value, &mut events);
                            for access in events.into_iter().filter_map(|event| match event {
                                LocalEvent::Move(access) => Some(access),
                                _ => None,
                            }) {
                                let overlapping = state
                                    .guards
                                    .iter()
                                    .filter(|(guard, _)| local_accesses_overlap(guard, &access))
                                    .map(|(guard, active)| (guard.clone(), *active))
                                    .collect::<Vec<_>>();
                                for (guard_place, guard) in overlapping {
                                    if guard_place != access {
                                        if guard.kind == CleanupEntryKind::Fallback
                                            && local_access_contains(&guard_place, &access)
                                        {
                                            if local_access_is_complete_sum_payload(
                                                &guard_place,
                                                &access,
                                            ) && state
                                                .pending_moves
                                                .insert(
                                                    guard_place.clone(),
                                                    PendingDeferTransition::Disarm,
                                                )
                                                .is_some()
                                            {
                                                return Err(MirInvariantError::new(
                                                    &statement_context,
                                                    "one cleanup owner is moved more than once by one assignment",
                                                ));
                                            }
                                            continue;
                                        }
                                        return Err(MirInvariantError::new(
                                            &statement_context,
                                            "assignment partially moves an explicit guard or embeds a fallback owner",
                                        ));
                                    }
                                    let transition = match guard.kind {
                                        CleanupEntryKind::Explicit => defer_assignment_transition(
                                            function,
                                            block,
                                            destination_place,
                                            value,
                                            &access,
                                            guard.scope,
                                        )
                                        .ok_or_else(|| {
                                            MirInvariantError::new(
                                                &statement_context,
                                                "assignment embeds an active defer guard instead of retargeting or handing it off",
                                            )
                                        })?,
                                        CleanupEntryKind::Fallback => {
                                            if assignment_directly_moves(value, &access) {
                                                PendingDeferTransition::Retarget
                                            } else {
                                                PendingDeferTransition::Disarm
                                            }
                                        }
                                    };
                                    if state
                                        .pending_moves
                                        .insert(access.clone(), transition)
                                        .is_some()
                                    {
                                        return Err(MirInvariantError::new(
                                            &statement_context,
                                            "one cleanup owner is moved more than once by one assignment",
                                        ));
                                    }
                                }
                            }
                        }
                        MirStatementKind::StorageLive(local)
                        | MirStatementKind::StorageDead(local) => {
                            if state.guards.keys().any(|guard| guard.local == *local) {
                                return Err(MirInvariantError::new(
                                    statement_context,
                                    "storage lifetime crosses an active defer guard",
                                ));
                            }
                        }
                        MirStatementKind::ReserveLoan(_)
                        | MirStatementKind::ReleaseLoan(_)
                        | MirStatementKind::EnterTaskScope { .. } => {}
                    }
                }
                if !state.pending_moves.is_empty() {
                    return Err(MirInvariantError::new(
                        &block_context,
                        "guarded move reaches a terminator without retarget or disarm",
                    ));
                }

                let mut terminator_events = Vec::new();
                let await_join_owner = match &block.terminator.kind {
                    MirTerminatorKind::Await {
                        awaitable:
                            MirAwaitable::Join(MirOperand {
                                kind: MirOperandKind::Move(place),
                                ..
                            }),
                        ..
                    } => Some(LocalAccess::from_place(place)),
                    _ => None,
                };
                match &block.terminator.kind {
                    MirTerminatorKind::CommitSelect { arms, .. } => {
                        for arm in arms {
                            if let Some(payload) = arm.payload() {
                                terminator_events
                                    .push(LocalEvent::Write(LocalAccess::from_place(payload)));
                            }
                        }
                    }
                    MirTerminatorKind::SwitchBool { condition, .. }
                    | MirTerminatorKind::SwitchTag {
                        value: condition, ..
                    } => push_operand_events(condition, &mut terminator_events),
                    MirTerminatorKind::Invoke { operation, .. }
                    | MirTerminatorKind::Spawn { operation, .. } => {
                        push_operation_events(operation, &mut terminator_events);
                    }
                    MirTerminatorKind::Await { awaitable, .. } => match awaitable {
                        MirAwaitable::Call(operation) => {
                            push_operation_events(operation, &mut terminator_events);
                        }
                        MirAwaitable::Join(join) => {
                            push_operand_events(join, &mut terminator_events);
                        }
                    },
                    MirTerminatorKind::ValidatePlaces { replacements, .. } => {
                        for replacement in replacements.iter().flatten() {
                            push_operand_events(replacement, &mut terminator_events);
                        }
                    }
                    MirTerminatorKind::Goto { .. }
                    | MirTerminatorKind::IteratorNext { .. }
                    | MirTerminatorKind::ValidateLoan { .. }
                    | MirTerminatorKind::DrainDefers { .. }
                    | MirTerminatorKind::DrainScopes { .. }
                    | MirTerminatorKind::DrainUnwind { .. }
                    | MirTerminatorKind::Return
                    | MirTerminatorKind::ResumePanic
                    | MirTerminatorKind::Unreachable => {}
                }
                for access in terminator_events
                    .into_iter()
                    .filter_map(|event| match event {
                        LocalEvent::Move(access) => Some(access),
                        _ => None,
                    })
                {
                    let overlaps = state
                        .guards
                        .iter()
                        .filter(|(guard, _)| local_accesses_overlap(guard, &access))
                        .map(|(guard, entry)| (guard.clone(), *entry))
                        .collect::<Vec<_>>();
                    if overlaps.len() == 1
                        && await_join_owner.as_ref() == Some(&access)
                        && overlaps[0].0 == access
                        && overlaps[0].1.kind == CleanupEntryKind::Fallback
                    {
                        let guard = state
                            .guards
                            .remove(&access)
                            .expect("the exact await Join guard was just observed");
                        state.registrations.remove(&guard.registration);
                        state.remove_inactive_scope(guard.scope);
                    } else if !overlaps.is_empty() {
                        return Err(MirInvariantError::new(
                            &block_context,
                            "terminator moves an active defer guard without disarming it",
                        ));
                    }
                }

                if let MirTerminatorKind::DrainDefers { scopes, .. } = &block.terminator.kind {
                    state.drain(scopes, &block_context)?;
                }
                if let MirTerminatorKind::DrainScopes { defer_scopes, .. } = &block.terminator.kind
                {
                    state.drain(defer_scopes, &block_context)?;
                }
                if matches!(block.terminator.kind, MirTerminatorKind::DrainUnwind { .. }) {
                    state.drain_unwind();
                }
                match block.terminator.kind {
                    MirTerminatorKind::Return => {
                        state.finish_normal(&block_context)?;
                    }
                    MirTerminatorKind::ResumePanic if !state.is_empty() => {
                        return Err(MirInvariantError::new(
                            &block_context,
                            "panic resume abandons registered cleanup entries",
                        ));
                    }
                    _ => {}
                }
                outgoing.insert(state);
            }

            for edge in &successors[block_id.0 as usize] {
                let target = edge.target.0 as usize;
                let mut changed = false;
                for state in &outgoing {
                    let mut edge_state = state.clone();
                    if let Some(destination) = &edge.writes {
                        let destination = LocalAccess::from_place(destination);
                        if edge_state
                            .guards
                            .keys()
                            .any(|guard| local_accesses_overlap(guard, &destination))
                        {
                            return Err(MirInvariantError::new(
                                &block_context,
                                format!(
                                    "terminator overwrites an active defer guard at {destination:?}; active guards: {:?}; terminator: {:?}",
                                    edge_state.guards, block.terminator.kind,
                                ),
                            ));
                        }
                        apply_consumed_defer_events(
                            &mut edge_state,
                            &[LocalEvent::Write(destination)],
                            &block_context,
                        )?;
                    }
                    let mut edge_states = vec![edge_state.clone()];
                    if let MirTerminatorKind::IteratorNext {
                        exhaustion_guard: Some(place),
                        exhausted,
                        ..
                    } = &block.terminator.kind
                        && edge.target == *exhausted
                    {
                        let terminal =
                            self.terminal_status(function.id, place.ty, &block_context)?;
                        if terminal == HirTerminalStatus::Present {
                            let place = LocalAccess::from_place(place);
                            let mut disarmed = edge_state;
                            if let Some(guard) = disarmed.guards.remove(&place) {
                                disarmed.registrations.remove(&guard.registration);
                                disarmed.remove_inactive_scope(guard.scope);
                            }
                            edge_states = vec![disarmed];
                        }
                    }
                    for state in edge_states {
                        changed |= incoming[target].insert(state);
                    }
                }
                if changed && !queued[target] {
                    queued[target] = true;
                    queue.push_back(edge.target);
                }
            }
        }
        Ok(())
    }

    fn verify_fallback_coverage(
        &self,
        function: &MirFunction,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        for (block_index, block) in function.blocks.iter().enumerate() {
            let block_context = format!("{context} block#{block_index}");
            for (index, statement) in block.statements.iter().enumerate() {
                let MirStatementKind::Assign { destination, value } = &statement.kind else {
                    continue;
                };
                if self.terminal_status(function.id, destination.ty, &block_context)?
                    == HirTerminalStatus::Absent
                {
                    continue;
                }
                if let Some((from, to)) = assignment_cleanup_transfer(destination, value) {
                    if !matches!(
                        block
                            .statements
                            .get(index + 1)
                            .map(|statement| statement.kind()),
                        Some(MirStatementKind::RetargetCleanup {
                            from: actual_from,
                            to: actual_to,
                        }) if actual_from == &from && actual_to == &to
                    ) {
                        return Err(MirInvariantError::new(
                            format!("{block_context} statement#{index}"),
                            "terminal assignment has no immediate cleanup retarget",
                        ));
                    }
                    continue;
                }
                let mut next = index + 1;
                while matches!(
                    block.statements.get(next).map(|statement| statement.kind()),
                    Some(MirStatementKind::DisarmCleanup(_))
                ) {
                    next += 1;
                }
                if !matches!(
                    block
                        .statements
                        .get(next)
                        .map(|statement| statement.kind()),
                    Some(MirStatementKind::RegisterFallback { owner, .. })
                        if owner == destination
                ) {
                    return Err(MirInvariantError::new(
                        format!("{block_context} statement#{index}"),
                        "terminal assignment result has no immediate fallback registration",
                    ));
                }
            }
            match &block.terminator.kind {
                MirTerminatorKind::Invoke {
                    destination: Some(destination),
                    target: Some(target),
                    ..
                } if self.terminal_status(function.id, destination.ty, &block_context)?
                    != HirTerminalStatus::Absent =>
                {
                    let target = self.block(function, *target, &block_context)?;
                    if !matches!(
                        target.statements.first().map(|statement| statement.kind()),
                        Some(MirStatementKind::RegisterFallback { owner, .. })
                            if owner == destination
                    ) {
                        return Err(MirInvariantError::new(
                            &block_context,
                            "terminal invocation result edge has no fallback registration",
                        ));
                    }
                }
                MirTerminatorKind::IteratorNext {
                    destination,
                    has_value,
                    ..
                } if self.terminal_status(function.id, destination.ty, &block_context)?
                    != HirTerminalStatus::Absent =>
                {
                    let target = self.block(function, *has_value, &block_context)?;
                    if !matches!(
                        target.statements.first().map(|statement| statement.kind()),
                        Some(MirStatementKind::RegisterFallback { owner, .. })
                            if owner == destination
                    ) {
                        return Err(MirInvariantError::new(
                            &block_context,
                            "terminal iterator value edge has no fallback registration",
                        ));
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn terminal_entry_owners(
        &self,
        function: &MirFunction,
        context: &str,
    ) -> Result<Vec<MirPlace>, MirInvariantError> {
        let mut candidates = Vec::new();
        match function.id {
            MirFunctionId::Callable(callable) => {
                let callable = self.hir.callable(callable).ok_or_else(|| {
                    MirInvariantError::new(context, "function has no typed HIR signature")
                })?;
                for (index, local) in function.parameters.iter().copied().enumerate() {
                    let parameter = callable.parameters().get(index).ok_or_else(|| {
                        MirInvariantError::new(
                            context,
                            "MIR parameter index exceeds its HIR signature",
                        )
                    })?;
                    if parameter.mode() == ParameterMode::Value && !parameter.is_receiver() {
                        candidates.push(MirPlace {
                            local,
                            ty: self.local(function, local, context)?.ty,
                            projections: Vec::new(),
                            source_loan: None,
                        });
                    }
                }
            }
            MirFunctionId::Closure(closure_id) => {
                let closure = self.hir.closure(closure_id).ok_or_else(|| {
                    MirInvariantError::new(context, "closure function has no typed HIR body")
                })?;
                let environment = function.parameters.first().copied().ok_or_else(|| {
                    MirInvariantError::new(
                        context,
                        "closure function has no hidden environment parameter",
                    )
                })?;
                for (index, capture) in closure.captures().iter().enumerate() {
                    candidates.push(MirPlace {
                        local: environment,
                        ty: capture.ty(),
                        projections: vec![MirProjection {
                            ty: capture.ty(),
                            kind: MirProjectionKind::ClosureCapture {
                                closure: closure_id,
                                index: index as u32,
                            },
                        }],
                        source_loan: None,
                    });
                }
                for (index, local) in function.parameters.iter().copied().skip(1).enumerate() {
                    let parameter = closure.parameters().get(index).ok_or_else(|| {
                        MirInvariantError::new(
                            context,
                            "MIR closure parameter index exceeds its HIR signature",
                        )
                    })?;
                    if parameter.mode() == ParameterMode::Value {
                        candidates.push(MirPlace {
                            local,
                            ty: self.local(function, local, context)?.ty,
                            projections: Vec::new(),
                            source_loan: None,
                        });
                    }
                }
            }
        }
        candidates
            .into_iter()
            .filter_map(|owner| {
                self.terminal_status(function.id, owner.ty, context)
                    .map(|status| (status != HirTerminalStatus::Absent).then_some(owner))
                    .transpose()
            })
            .collect()
    }

    fn verify_loan_flow(
        &self,
        function: &MirFunction,
        reachable: &[bool],
        context: &str,
    ) -> Result<(), MirInvariantError> {
        let events = function
            .blocks
            .iter()
            .map(|block| mir_loan_events(function, block))
            .collect::<Vec<_>>();
        let static_integers = super::regions::static_integer_locals(self.hir, function);
        let mut reservations = vec![0_u32; function.loans.len()];
        let mut validations = vec![0_u32; function.loans.len()];
        let mut consumptions = vec![0_u32; function.loans.len()];
        for block_events in &events {
            for event in block_events {
                match event {
                    LoanEvent::Reserve(loan) => {
                        let count =
                            reservations.get_mut(loan.index() as usize).ok_or_else(|| {
                                MirInvariantError::new(context, "reserves an unknown loan")
                            })?;
                        *count = count.saturating_add(1);
                    }
                    LoanEvent::Consume(loans) => {
                        for loan in loans {
                            let count =
                                consumptions.get_mut(loan.index() as usize).ok_or_else(|| {
                                    MirInvariantError::new(context, "consumes an unknown loan")
                                })?;
                            *count = count.saturating_add(1);
                        }
                    }
                    LoanEvent::Local(_) | LoanEvent::Release(_) => {}
                }
            }
        }
        for block in &function.blocks {
            if let MirTerminatorKind::ValidateLoan { loan, .. } = &block.terminator.kind {
                let count = validations
                    .get_mut(loan.index() as usize)
                    .ok_or_else(|| MirInvariantError::new(context, "validates an unknown loan"))?;
                *count = count.saturating_add(1);
            }
        }
        for index in 0..function.loans.len() {
            let loan = &function.loans[index];
            let valid_consumptions = match loan.kind {
                MirLoanKind::CallLocal => consumptions[index] <= 1,
                MirLoanKind::Region => consumptions[index] == 0,
            };
            let expected_validations = u32::from(place_requires_loan_validation(&loan.place));
            if reservations[index] != 1
                || validations[index] != expected_validations
                || !valid_consumptions
            {
                return Err(MirInvariantError::new(
                    format!("{context} loan#{index}"),
                    format!(
                        "has {} validations, {} reservations, and {} call consumptions, which violates its {:?} contract",
                        validations[index], reservations[index], consumptions[index], loan.kind
                    ),
                ));
            }
        }

        let mut incoming = vec![None::<LoanFlowState>; function.blocks.len()];
        incoming[function.entry.index() as usize] = Some(LoanFlowState::default());
        let mut queue = VecDeque::from([function.entry]);
        let mut queued = vec![false; function.blocks.len()];
        queued[function.entry.index() as usize] = true;
        while let Some(block_id) = queue.pop_front() {
            queued[block_id.index() as usize] = false;
            self.consume_dataflow_step(context)?;
            let mut state = incoming[block_id.index() as usize]
                .clone()
                .expect("queued loan-flow blocks have an incoming state");
            let block_context = format!("{context} block#{}", block_id.index());
            for event in &events[block_id.index() as usize] {
                self.apply_loan_event(
                    function,
                    &static_integers,
                    &mut state,
                    event,
                    &block_context,
                )?;
            }
            let block = &function.blocks[block_id.index() as usize];
            let mut propagate =
                |target: MirBlockId, edge_state: LoanFlowState| -> Result<(), MirInvariantError> {
                    let target_index = target.index() as usize;
                    if !reachable[target_index] {
                        return Ok(());
                    }
                    match &incoming[target_index] {
                        Some(existing) if existing != &edge_state => {
                            return Err(MirInvariantError::new(
                                format!("{context} block#{}", target.index()),
                                "control-flow predecessors disagree about active loans",
                            ));
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
            if !state.accesses.is_empty() {
                return Err(MirInvariantError::new(
                    &block_context,
                    "runtime place proof is not consumed by the immediate access",
                ));
            }
            match &block.terminator.kind {
                MirTerminatorKind::Goto { target } => propagate(*target, state)?,
                MirTerminatorKind::SwitchBool {
                    if_true, if_false, ..
                } => {
                    propagate(*if_true, state.clone())?;
                    propagate(*if_false, state)?;
                }
                MirTerminatorKind::SwitchTag {
                    cases, otherwise, ..
                } => {
                    for (_, target) in cases {
                        propagate(*target, state.clone())?;
                    }
                    propagate(*otherwise, state)?;
                }
                MirTerminatorKind::Invoke {
                    operation,
                    destination,
                    target,
                    unwind,
                    ..
                } => {
                    if let Some((place, against)) =
                        operation_access_place(operation, &block_context)?
                    {
                        let expected = self.runtime_place_conflicts(
                            function,
                            &static_integers,
                            &state.active,
                            &place,
                            false,
                            &block_context,
                        )?;
                        if against != expected {
                            return Err(MirInvariantError::new(
                                &block_context,
                                "indexed operation runtime proof differs from active dynamic conflicts",
                            ));
                        }
                    }
                    if let Some(target) = target {
                        let normal = state.clone();
                        if let Some(destination) = destination {
                            self.verify_loan_local_access(
                                function,
                                &static_integers,
                                &normal.active,
                                &LocalEvent::Write(LocalAccess::from_place(destination)),
                                None,
                                &block_context,
                            )?;
                        }
                        propagate(*target, normal)?;
                    }
                    propagate(*unwind, LoanFlowState::default())?;
                }
                MirTerminatorKind::Await {
                    destination,
                    target,
                    unwind,
                    awaitable,
                } => {
                    let suspendible_receiver = match awaitable {
                        MirAwaitable::Call(operation) => async_iterator_receiver_loan(operation),
                        MirAwaitable::Join(_) => None,
                    };
                    for loan in &state.active {
                        if self.loan(function, *loan, &block_context)?.mode != ParameterMode::Ref
                            && suspendible_receiver != Some(*loan)
                        {
                            return Err(MirInvariantError::new(
                                &block_context,
                                "exclusive loan crosses an await suspension",
                            ));
                        }
                    }
                    self.verify_loan_local_access(
                        function,
                        &static_integers,
                        &state.active,
                        &LocalEvent::Write(LocalAccess::from_place(destination)),
                        None,
                        &block_context,
                    )?;
                    propagate(*target, state)?;
                    propagate(*unwind, LoanFlowState::default())?;
                }
                MirTerminatorKind::CommitSelect {
                    arms,
                    else_target,
                    unwind,
                } => {
                    for loan in &state.active {
                        if self.loan(function, *loan, &block_context)?.mode != ParameterMode::Ref {
                            return Err(MirInvariantError::new(
                                &block_context,
                                "exclusive loan crosses a select suspension",
                            ));
                        }
                    }
                    for arm in arms {
                        if let Some(payload) = arm.payload() {
                            self.verify_loan_local_access(
                                function,
                                &static_integers,
                                &state.active,
                                &LocalEvent::Write(LocalAccess::from_place(payload)),
                                None,
                                &block_context,
                            )?;
                        }
                        propagate(arm.target(), state.clone())?;
                    }
                    if let Some(else_target) = else_target {
                        propagate(*else_target, state.clone())?;
                    }
                    propagate(*unwind, LoanFlowState::default())?;
                }
                MirTerminatorKind::Spawn {
                    destination,
                    target,
                    unwind,
                    ..
                } => {
                    self.verify_loan_local_access(
                        function,
                        &static_integers,
                        &state.active,
                        &LocalEvent::Write(LocalAccess::from_place(destination)),
                        None,
                        &block_context,
                    )?;
                    propagate(*target, state)?;
                    propagate(*unwind, LoanFlowState::default())?;
                }
                MirTerminatorKind::IteratorNext {
                    destination,
                    borrowed_source,
                    has_value,
                    exhausted,
                    unwind,
                    ..
                } => {
                    let source_chain = borrowed_source
                        .as_ref()
                        .map(|source| self.place_source_chain(function, source, &block_context))
                        .transpose()?
                        .unwrap_or_default();
                    for id in &state.active {
                        let loan = self.loan(function, *id, &block_context)?;
                        if loan.kind != MirLoanKind::Region
                            || (loan.mode != ParameterMode::Ref && !source_chain.contains(id))
                        {
                            return Err(MirInvariantError::new(
                                &block_context,
                                "only shared regions or the iterator source chain may cross an iterator boundary",
                            ));
                        }
                    }
                    let has_value_state = state.clone();
                    self.verify_loan_local_access(
                        function,
                        &static_integers,
                        &has_value_state.active,
                        &LocalEvent::Write(LocalAccess::from_place(destination)),
                        None,
                        &block_context,
                    )?;
                    propagate(*has_value, has_value_state)?;
                    propagate(*exhausted, state)?;
                    propagate(*unwind, LoanFlowState::default())?;
                }
                MirTerminatorKind::ValidatePlaces {
                    places,
                    against,
                    for_write,
                    target,
                    unwind,
                    ..
                } => {
                    let expected = places
                        .iter()
                        .map(|place| {
                            self.runtime_place_conflicts(
                                function,
                                &static_integers,
                                &state.active,
                                place,
                                *for_write,
                                &block_context,
                            )
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    if against != &expected {
                        return Err(MirInvariantError::new(
                            &block_context,
                            "place validation runtime proof differs from active dynamic conflicts",
                        ));
                    }
                    for (place, loans) in places.iter().zip(expected) {
                        if loans.is_empty() {
                            continue;
                        }
                        let key = ValidatedAccess {
                            access: LocalAccess::from_place(place),
                            for_write: *for_write,
                        };
                        if state.accesses.insert(key, loans).is_some() {
                            return Err(MirInvariantError::new(
                                &block_context,
                                "place validation duplicates a pending runtime access proof",
                            ));
                        }
                    }
                    propagate(*target, state)?;
                    propagate(*unwind, LoanFlowState::default())?;
                }
                MirTerminatorKind::ValidateLoan {
                    loan,
                    against,
                    target,
                    unwind,
                } => {
                    let expected = self.runtime_loan_conflicts(
                        function,
                        &static_integers,
                        &state.active,
                        *loan,
                        &block_context,
                    )?;
                    if against != &expected {
                        return Err(MirInvariantError::new(
                            &block_context,
                            format!(
                                "loan#{} runtime proof lists {:?}, expected {:?}",
                                loan.index(),
                                against.iter().map(|loan| loan.index()).collect::<Vec<_>>(),
                                expected.iter().map(|loan| loan.index()).collect::<Vec<_>>()
                            ),
                        ));
                    }
                    if state.active.contains(loan)
                        || state.validated.insert(*loan, expected).is_some()
                    {
                        return Err(MirInvariantError::new(
                            &block_context,
                            format!("validates already-active or pending loan#{}", loan.index()),
                        ));
                    }
                    propagate(*target, state)?;
                    propagate(*unwind, LoanFlowState::default())?;
                }
                MirTerminatorKind::DrainDefers { target, unwind, .. } => {
                    propagate(*target, state)?;
                    propagate(*unwind, LoanFlowState::default())?;
                }
                MirTerminatorKind::DrainScopes { target, unwind, .. } => {
                    for loan in &state.active {
                        if self.loan(function, *loan, &block_context)?.mode != ParameterMode::Ref {
                            return Err(MirInvariantError::new(
                                &block_context,
                                "exclusive loan crosses structured scope suspension",
                            ));
                        }
                    }
                    propagate(*target, state)?;
                    propagate(*unwind, LoanFlowState::default())?;
                }
                MirTerminatorKind::DrainUnwind { target } => {
                    propagate(*target, LoanFlowState::default())?;
                }
                MirTerminatorKind::Return => {
                    if !state.active.is_empty()
                        || !state.validated.is_empty()
                        || !state.accesses.is_empty()
                    {
                        return Err(MirInvariantError::new(
                            block_context,
                            "return abandons active loans without explicit release",
                        ));
                    }
                }
                MirTerminatorKind::ResumePanic | MirTerminatorKind::Unreachable => {}
            }
        }
        Ok(())
    }

    fn apply_loan_event(
        &self,
        function: &MirFunction,
        static_integers: &BTreeMap<MirLocalId, u64>,
        state: &mut LoanFlowState,
        event: &LoanEvent,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        match event {
            LoanEvent::Local(event) => {
                self.verify_runtime_proof_inputs_stable(function, state, event, context)?;
                let key = match event {
                    LocalEvent::Read(access) => Some(ValidatedAccess {
                        access: access.clone(),
                        for_write: false,
                    }),
                    LocalEvent::Move(access) | LocalEvent::Write(access) => Some(ValidatedAccess {
                        access: access.clone(),
                        for_write: true,
                    }),
                    LocalEvent::Resolve(_)
                    | LocalEvent::WriteAccess(_)
                    | LocalEvent::StorageLive(_)
                    | LocalEvent::StorageDead(_) => None,
                };
                let proof = key.as_ref().and_then(|key| state.accesses.remove(key));
                self.verify_loan_local_access(
                    function,
                    static_integers,
                    &state.active,
                    event,
                    proof.as_deref(),
                    context,
                )
            }
            LoanEvent::Reserve(id) => {
                if !state.accesses.is_empty() {
                    return Err(MirInvariantError::new(
                        context,
                        "reserves a loan while a runtime access proof is pending",
                    ));
                }
                let loan = self.loan(function, *id, context)?;
                self.verify_reborrow_mode(function, loan, context)?;
                if state.active.contains(id) {
                    return Err(MirInvariantError::new(
                        context,
                        format!("reserves already-active loan#{}", id.index()),
                    ));
                }
                let proof = state.validated.remove(id);
                if place_requires_loan_validation(loan.place()) != proof.is_some() {
                    return Err(MirInvariantError::new(
                        context,
                        format!(
                            "loan#{} reservation disagrees with its required validation",
                            id.index()
                        ),
                    ));
                }
                let source_chain = self.place_source_chain(function, loan.place(), context)?;
                for active in state.active.iter().copied() {
                    self.consume_dataflow_step(context)?;
                    if source_chain.contains(&active) {
                        continue;
                    }
                    let existing = self.loan(function, active, context)?;
                    if loan.mode() == ParameterMode::Ref && existing.mode() == ParameterMode::Ref {
                        continue;
                    }
                    match super::regions::loan_place_relation(
                        loan.place(),
                        existing.place(),
                        static_integers,
                    ) {
                        StaticRegionRelation::Disjoint => {}
                        StaticRegionRelation::Runtime
                            if proof
                                .as_ref()
                                .is_some_and(|against| against.contains(&active)) => {}
                        StaticRegionRelation::Runtime | StaticRegionRelation::Overlap => {
                            return Err(MirInvariantError::new(
                                context,
                                format!(
                                    "loan#{} lacks a valid proof against incompatible active loan#{}",
                                    id.index(),
                                    active.index()
                                ),
                            ));
                        }
                    }
                }
                state.active.insert(*id);
                Ok(())
            }
            LoanEvent::Release(loan) => {
                if !state.validated.is_empty() || !state.accesses.is_empty() {
                    return Err(MirInvariantError::new(
                        context,
                        "releases a loan while another reservation proof is pending",
                    ));
                }
                if !state.active.contains(loan) {
                    return Err(MirInvariantError::new(
                        context,
                        format!("releases inactive loan#{}", loan.index()),
                    ));
                }
                if let Some(dependent) =
                    self.active_dependent_loan(function, &state.active, *loan, context)?
                {
                    return Err(MirInvariantError::new(
                        context,
                        format!(
                            "releases source region loan#{} while dependent loan#{} remains active",
                            loan.index(),
                            dependent.index()
                        ),
                    ));
                }
                state.active.remove(loan);
                Ok(())
            }
            LoanEvent::Consume(loans) => {
                if !state.validated.is_empty() || !state.accesses.is_empty() {
                    return Err(MirInvariantError::new(
                        context,
                        "consumes loans while a runtime proof is pending",
                    ));
                }
                let mut seen = BTreeSet::new();
                for loan in loans {
                    let metadata = self.loan(function, *loan, context)?;
                    if metadata.kind != MirLoanKind::CallLocal {
                        return Err(MirInvariantError::new(
                            context,
                            format!("call consumes region loan#{}", loan.index()),
                        ));
                    }
                    self.verify_source_loan_access(
                        function,
                        &state.active,
                        &LocalAccess::from_place(&metadata.place),
                        "read",
                        context,
                    )?;
                    if !seen.insert(*loan) || !state.active.remove(loan) {
                        return Err(MirInvariantError::new(
                            context,
                            format!("consumes inactive loan#{}", loan.index()),
                        ));
                    }
                }
                Ok(())
            }
        }
    }

    fn verify_runtime_proof_inputs_stable(
        &self,
        function: &MirFunction,
        state: &LoanFlowState,
        event: &LocalEvent,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        let changed = match event {
            LocalEvent::Move(access)
            | LocalEvent::Write(access)
            | LocalEvent::WriteAccess(access) => Some(access.local),
            LocalEvent::StorageLive(local) | LocalEvent::StorageDead(local) => Some(*local),
            LocalEvent::Read(_) | LocalEvent::Resolve(_) => None,
        };
        let Some(changed) = changed else {
            return Ok(());
        };
        let access_input_changed = state.accesses.keys().any(|validated| {
            move_path_runtime_inputs(&validated.access.path).any(|local| local == changed)
        });
        let loan_input_changed =
            state
                .validated
                .keys()
                .try_fold(false, |changed_input, loan| {
                    if changed_input {
                        return Ok(true);
                    }
                    let loan = self.loan(function, *loan, context)?;
                    Ok(
                        move_path_runtime_inputs(&LocalAccess::from_place(loan.place()).path)
                            .any(|local| local == changed),
                    )
                })?;
        if access_input_changed || loan_input_changed {
            return Err(MirInvariantError::new(
                context,
                format!(
                    "changes local#{} while it is an input to a pending runtime-overlap proof",
                    changed.index()
                ),
            ));
        }
        Ok(())
    }

    fn runtime_loan_conflicts(
        &self,
        function: &MirFunction,
        static_integers: &BTreeMap<MirLocalId, u64>,
        active: &BTreeSet<MirLoanId>,
        candidate: MirLoanId,
        context: &str,
    ) -> Result<Vec<MirLoanId>, MirInvariantError> {
        let loan = self.loan(function, candidate, context)?;
        if !place_requires_loan_validation(loan.place()) {
            return Err(MirInvariantError::new(
                context,
                format!(
                    "loan#{} has no runtime-resolvable collection projection",
                    candidate.index()
                ),
            ));
        }
        let mut against = Vec::new();
        let source_chain = self.place_source_chain(function, loan.place(), context)?;
        for active in active.iter().copied() {
            self.consume_dataflow_step(context)?;
            if source_chain.contains(&active) {
                continue;
            }
            let existing = self.loan(function, active, context)?;
            if loan.mode() == ParameterMode::Ref && existing.mode() == ParameterMode::Ref {
                continue;
            }
            match super::regions::loan_place_relation(
                loan.place(),
                existing.place(),
                static_integers,
            ) {
                StaticRegionRelation::Disjoint => {}
                StaticRegionRelation::Runtime => against.push(active),
                StaticRegionRelation::Overlap => {
                    return Err(MirInvariantError::new(
                        context,
                        format!(
                            "loan#{} statically overlaps incompatible active loan#{}",
                            candidate.index(),
                            active.index()
                        ),
                    ));
                }
            }
        }
        Ok(against)
    }

    fn runtime_place_conflicts(
        &self,
        function: &MirFunction,
        static_integers: &BTreeMap<MirLocalId, u64>,
        active: &BTreeSet<MirLoanId>,
        place: &MirPlace,
        for_write: bool,
        context: &str,
    ) -> Result<Vec<MirLoanId>, MirInvariantError> {
        let mut against = Vec::new();
        let source_chain = self.place_source_chain(function, place, context)?;
        for active in active.iter().copied() {
            self.consume_dataflow_step(context)?;
            if source_chain.contains(&active) {
                continue;
            }
            let existing = self.loan(function, active, context)?;
            if !for_write && existing.mode() == ParameterMode::Ref {
                continue;
            }
            match super::regions::loan_place_relation(place, existing.place(), static_integers) {
                StaticRegionRelation::Disjoint => {}
                StaticRegionRelation::Runtime => against.push(active),
                StaticRegionRelation::Overlap => {
                    return Err(MirInvariantError::new(
                        context,
                        format!(
                            "place validation statically overlaps active loan#{}",
                            active.index()
                        ),
                    ));
                }
            }
        }
        Ok(against)
    }

    fn place_source_chain(
        &self,
        function: &MirFunction,
        place: &MirPlace,
        context: &str,
    ) -> Result<BTreeSet<MirLoanId>, MirInvariantError> {
        let mut chain = BTreeSet::new();
        let mut source = place.source_loan;
        while let Some(id) = source {
            if !chain.insert(id) {
                return Err(MirInvariantError::new(
                    context,
                    "place source region chain contains a cycle",
                ));
            }
            source = self.loan(function, id, context)?.place.source_loan;
        }
        Ok(chain)
    }

    fn active_dependent_loan(
        &self,
        function: &MirFunction,
        state: &BTreeSet<MirLoanId>,
        source: MirLoanId,
        context: &str,
    ) -> Result<Option<MirLoanId>, MirInvariantError> {
        for candidate in state
            .iter()
            .copied()
            .filter(|candidate| *candidate != source)
        {
            let mut parent = self.loan(function, candidate, context)?.place.source_loan;
            let mut seen = BTreeSet::new();
            while let Some(id) = parent {
                if id == source {
                    return Ok(Some(candidate));
                }
                if !seen.insert(id) {
                    return Err(MirInvariantError::new(
                        context,
                        "loan source region chain contains a cycle",
                    ));
                }
                parent = self.loan(function, id, context)?.place.source_loan;
            }
        }
        Ok(None)
    }

    fn verify_loan_local_access(
        &self,
        function: &MirFunction,
        static_integers: &BTreeMap<MirLocalId, u64>,
        state: &BTreeSet<MirLoanId>,
        event: &LocalEvent,
        proof: Option<&[MirLoanId]>,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        let (access, access_kind) = match event {
            LocalEvent::Read(access) => (Some(access), "read"),
            LocalEvent::Resolve(access) => {
                self.verify_source_loan_access(function, state, access, "read", context)?;
                return Ok(());
            }
            LocalEvent::Move(access) => (Some(access), "move"),
            LocalEvent::Write(access) | LocalEvent::WriteAccess(access) => (Some(access), "write"),
            LocalEvent::StorageLive(local) | LocalEvent::StorageDead(local) => {
                let access = LocalAccess {
                    local: *local,
                    path: Vec::new(),
                    source_loan: None,
                };
                return self.verify_active_loan_access(
                    function,
                    static_integers,
                    state,
                    ClassifiedLocalAccess {
                        access: &access,
                        kind: "storage change",
                    },
                    None,
                    context,
                );
            }
        };
        let access = access.expect("access events carry a place");
        self.verify_source_loan_access(function, state, access, access_kind, context)?;
        if let Some(mode) = self.parameter_mode(function, access.local, context)? {
            if access_kind == "move" && mode != ParameterMode::Value {
                return Err(MirInvariantError::new(
                    context,
                    "moves content out of a borrowed parameter",
                ));
            }
            if access_kind == "write" && mode == ParameterMode::Ref {
                return Err(MirInvariantError::new(
                    context,
                    "writes through a shared `ref` parameter",
                ));
            }
        }
        self.verify_active_loan_access(
            function,
            static_integers,
            state,
            ClassifiedLocalAccess {
                access,
                kind: access_kind,
            },
            proof,
            context,
        )
    }

    fn verify_source_loan_access(
        &self,
        function: &MirFunction,
        state: &BTreeSet<MirLoanId>,
        access: &LocalAccess,
        access_kind: &str,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        let Some(mut source) = access.source_loan else {
            return Ok(());
        };
        if access_kind == "move" {
            return Err(MirInvariantError::new(
                context,
                "move transfers content out of a region reference",
            ));
        }
        let mut seen = BTreeSet::new();
        loop {
            if !seen.insert(source) {
                return Err(MirInvariantError::new(
                    context,
                    "place source region chain contains a cycle",
                ));
            }
            if !state.contains(&source) {
                return Err(MirInvariantError::new(
                    context,
                    format!("read uses inactive source region loan#{}", source.index()),
                ));
            }
            let loan = self.loan(function, source, context)?;
            if loan.kind != MirLoanKind::Region {
                return Err(MirInvariantError::new(
                    context,
                    "place source is not a region loan",
                ));
            }
            if access_kind == "write" && loan.mode == ParameterMode::Ref {
                return Err(MirInvariantError::new(
                    context,
                    "write uses a shared region reference",
                ));
            }
            let Some(parent) = loan.place.source_loan else {
                return Ok(());
            };
            source = parent;
        }
    }

    fn verify_active_loan_access(
        &self,
        function: &MirFunction,
        static_integers: &BTreeMap<MirLocalId, u64>,
        state: &BTreeSet<MirLoanId>,
        access: ClassifiedLocalAccess<'_>,
        proof: Option<&[MirLoanId]>,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        let ClassifiedLocalAccess { access, kind } = access;
        let mut source_chain = BTreeSet::new();
        let mut source = access.source_loan;
        while let Some(id) = source {
            if !source_chain.insert(id) {
                return Err(MirInvariantError::new(
                    context,
                    "place source region chain contains a cycle",
                ));
            }
            source = self.loan(function, id, context)?.place().source_loan();
        }
        for active in state.iter().copied() {
            self.consume_dataflow_step(context)?;
            let loan = self.loan(function, active, context)?;
            let loan_access = LocalAccess::from_place(loan.place());
            if source_chain.contains(&active)
                || access.local != loan_access.local
                || kind == "read" && loan.mode() == ParameterMode::Ref
            {
                continue;
            }
            match loan_paths_relation(&access.path, &loan_access.path, static_integers) {
                StaticRegionRelation::Disjoint => {}
                StaticRegionRelation::Runtime
                    if proof.is_some_and(|proof| proof.contains(&active)) => {}
                StaticRegionRelation::Runtime | StaticRegionRelation::Overlap => {
                    return Err(MirInvariantError::new(
                        context,
                        format!(
                            "{kind} {:?} overlaps active loan#{} ({:?}) at {:?}",
                            access.path,
                            active.index(),
                            loan.mode(),
                            loan_access.path,
                        ),
                    ));
                }
            }
        }
        Ok(())
    }

    fn verify_reborrow_mode(
        &self,
        function: &MirFunction,
        loan: &super::MirLoan,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        let Some(source) = self.loan_source_mode(function, loan, context)? else {
            return Ok(());
        };
        let compatible = match loan.mode() {
            ParameterMode::Value => false,
            ParameterMode::Ref => true,
            ParameterMode::Mut => matches!(source, ParameterMode::Mut | ParameterMode::Var),
            ParameterMode::Var => {
                source == ParameterMode::Var
                    || source == ParameterMode::Mut
                        && place_is_structurally_replaceable(loan.place())
            }
        };
        if compatible {
            Ok(())
        } else {
            Err(MirInvariantError::new(
                context,
                "loan requests stronger permissions than its borrowed parameter source",
            ))
        }
    }

    fn loan_source_mode(
        &self,
        function: &MirFunction,
        loan: &super::MirLoan,
        context: &str,
    ) -> Result<Option<ParameterMode>, MirInvariantError> {
        if let Some(source) = loan.place().source_loan() {
            let source = self.loan(function, source, context)?;
            if source.kind() != MirLoanKind::Region {
                return Err(MirInvariantError::new(
                    context,
                    "reborrow source is not a region loan",
                ));
            }
            return Ok(Some(source.mode()));
        }
        if let MirFunctionId::Closure(closure_id) = function.id
            && function.parameters.first() == Some(&loan.place().local())
            && let Some(MirProjectionKind::ClosureCapture { closure, index }) =
                loan.place().projections().first().map(MirProjection::kind)
        {
            if *closure != closure_id {
                return Err(MirInvariantError::new(
                    context,
                    "loan capture projection belongs to a different closure",
                ));
            }
            let capture = self
                .hir
                .closure(closure_id)
                .and_then(|closure| closure.captures().get(*index as usize))
                .ok_or_else(|| {
                    MirInvariantError::new(context, "loan references an unknown closure capture")
                })?;
            return Ok(Some(if capture.is_mutable() {
                ParameterMode::Var
            } else {
                ParameterMode::Ref
            }));
        }
        self.parameter_mode(function, loan.place().local(), context)
    }

    fn parameter_mode(
        &self,
        function: &MirFunction,
        local: MirLocalId,
        context: &str,
    ) -> Result<Option<ParameterMode>, MirInvariantError> {
        let MirLocalKind::Parameter { index, .. } = self.local(function, local, context)?.kind
        else {
            return Ok(None);
        };
        let mode = match function.id {
            MirFunctionId::Callable(callable) => self
                .hir
                .callable(callable)
                .and_then(|callable| callable.parameters().get(index as usize))
                .map(|parameter| parameter.mode()),
            MirFunctionId::Closure(_closure) if index == 0 => Some(ParameterMode::Value),
            MirFunctionId::Closure(closure) => self
                .hir
                .closure(closure)
                .and_then(|closure| closure.parameters().get(index as usize - 1))
                .map(|parameter| parameter.mode()),
        };
        mode.map(Some).ok_or_else(|| {
            MirInvariantError::new(
                context,
                "parameter local has no matching HIR parameter mode",
            )
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn verify_local_flow(
        &self,
        function: &MirFunction,
        local: MirLocalId,
        events: &[Vec<LocalEvent>],
        successors: &[Vec<SuccessorEdge>],
        predecessors: &[Vec<(MirBlockId, SuccessorEdge)>],
        reachable: &[bool],
        managed_storage: bool,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        let local_kind = self.local(function, local, context)?.kind;
        if managed_storage
            && matches!(
                local_kind,
                MirLocalKind::Return | MirLocalKind::Parameter { .. }
            )
        {
            return Err(MirInvariantError::new(
                context,
                format!(
                    "local#{} has function-wide storage but uses StorageLive/StorageDead",
                    local.index()
                ),
            ));
        }
        let root = Vec::new();
        let mut initial_unavailable = BTreeSet::new();
        if !matches!(local_kind, MirLocalKind::Parameter { .. }) {
            initial_unavailable.insert(root.clone());
        }
        let initial = LocalState {
            live: !managed_storage,
            unavailable: initial_unavailable,
        };
        let top = LocalState {
            live: true,
            unavailable: BTreeSet::new(),
        };
        let mut incoming = vec![top.clone(); function.blocks.len()];
        incoming[function.entry.0 as usize] = initial;
        let mut queue = (0..function.blocks.len())
            .filter(|index| reachable[*index] && *index != function.entry.0 as usize)
            .map(|index| MirBlockId(index as u32))
            .collect::<VecDeque<_>>();
        let mut queued = reachable.to_vec();
        queued[function.entry.0 as usize] = false;
        while let Some(block) = queue.pop_front() {
            queued[block.0 as usize] = false;
            self.consume_dataflow_step(context)?;
            let mut state = top.clone();
            let mut found = false;
            for (predecessor, edge) in &predecessors[block.0 as usize] {
                if !reachable[predecessor.0 as usize] {
                    continue;
                }
                let mut edge_state = transfer_local(
                    incoming[predecessor.0 as usize].clone(),
                    &events[predecessor.0 as usize],
                    local,
                );
                if let Some(write) = edge.writes.as_ref().filter(|place| place.local == local)
                    && edge_state.live
                {
                    write_path_unchecked(
                        &mut edge_state.unavailable,
                        &LocalAccess::from_place(write).path,
                    );
                }
                state.live &= edge_state.live;
                state.unavailable.extend(edge_state.unavailable);
                found = true;
            }
            if !found {
                continue;
            }
            let index = block.0 as usize;
            if incoming[index] != state {
                incoming[index] = state;
                for edge in &successors[index] {
                    let next = edge.target.0 as usize;
                    if reachable[next] && edge.target != function.entry && !queued[next] {
                        queued[next] = true;
                        queue.push_back(edge.target);
                    }
                }
            }
        }

        for (block_index, block_events) in events.iter().enumerate() {
            if !reachable[block_index] {
                continue;
            }
            let mut state = incoming[block_index].clone();
            for event in block_events {
                match event {
                    LocalEvent::Read(access) | LocalEvent::Resolve(access)
                        if access.local == local =>
                    {
                        if !state.live || !path_is_available(&state.unavailable, &access.path) {
                            return Err(MirInvariantError::new(
                                format!("{context} block#{block_index}"),
                                unavailable_read_message(local, &access.path),
                            ));
                        }
                    }
                    LocalEvent::Move(access) if access.local == local => {
                        if !state.live || !path_is_available(&state.unavailable, &access.path) {
                            return Err(MirInvariantError::new(
                                format!("{context} block#{block_index}"),
                                unavailable_move_message(local, &access.path),
                            ));
                        }
                        move_path_unchecked(&mut state.unavailable, access.path.clone());
                    }
                    LocalEvent::WriteAccess(access) if access.local == local => {
                        if !state.live
                            || !path_parent_is_available(&state.unavailable, &access.path)
                        {
                            return Err(MirInvariantError::new(
                                format!("{context} block#{block_index}"),
                                format!(
                                    "resolves a write through unavailable local#{}",
                                    local.index()
                                ),
                            ));
                        }
                    }
                    LocalEvent::Write(access) if access.local == local => {
                        if !state.live {
                            return Err(MirInvariantError::new(
                                format!("{context} block#{block_index}"),
                                format!(
                                    "writes local#{} outside its storage lifetime",
                                    local.index()
                                ),
                            ));
                        }
                        if !path_parent_is_available(&state.unavailable, &access.path) {
                            return Err(MirInvariantError::new(
                                format!("{context} block#{block_index}"),
                                format!("writes through unavailable local#{}", local.index()),
                            ));
                        }
                        write_path_unchecked(&mut state.unavailable, &access.path);
                    }
                    LocalEvent::StorageLive(event_local) if *event_local == local => {
                        state.live = true;
                        state.unavailable.clear();
                        state.unavailable.insert(Vec::new());
                    }
                    LocalEvent::StorageDead(event_local) if *event_local == local => {
                        if !state.live {
                            return Err(MirInvariantError::new(
                                format!("{context} block#{block_index}"),
                                format!("ends dead storage for local#{}", local.index()),
                            ));
                        }
                        state.live = false;
                        state.unavailable.clear();
                        state.unavailable.insert(Vec::new());
                    }
                    LocalEvent::Read(_)
                    | LocalEvent::Resolve(_)
                    | LocalEvent::Move(_)
                    | LocalEvent::Write(_)
                    | LocalEvent::WriteAccess(_)
                    | LocalEvent::StorageLive(_)
                    | LocalEvent::StorageDead(_) => {}
                }
            }
        }
        Ok(())
    }

    fn verify_tag_refinements(
        &self,
        function: &MirFunction,
        successors: &[Vec<SuccessorEdge>],
        reachable: &[bool],
        context: &str,
    ) -> Result<(), MirInvariantError> {
        let events = function
            .blocks
            .iter()
            .map(|block| tag_events(function, block))
            .collect::<Vec<_>>();
        let mut facts = Vec::<TagFact>::new();
        for fact in events.iter().flatten().filter_map(|event| match event {
            TagEvent::Require(fact) => Some(fact),
            TagEvent::Write(_) => None,
        }) {
            if !facts.contains(fact) {
                facts.push(fact.clone());
            }
        }
        if facts.is_empty() {
            return Ok(());
        }
        let mut predecessors =
            vec![Vec::<(MirBlockId, SuccessorEdge)>::new(); function.blocks.len()];
        for (source, edges) in successors.iter().enumerate() {
            for edge in edges {
                predecessors[edge.target.0 as usize]
                    .push((MirBlockId(source as u32), edge.clone()));
            }
        }
        for fact in facts {
            let mut incoming = vec![true; function.blocks.len()];
            incoming[function.entry.0 as usize] = false;
            let mut queue = (0..function.blocks.len())
                .filter(|index| reachable[*index] && *index != function.entry.0 as usize)
                .map(|index| MirBlockId(index as u32))
                .collect::<VecDeque<_>>();
            let mut queued = reachable.to_vec();
            queued[function.entry.0 as usize] = false;
            while let Some(block) = queue.pop_front() {
                queued[block.0 as usize] = false;
                self.consume_dataflow_step(context)?;
                let mut state = true;
                let mut found = false;
                for (predecessor, edge) in &predecessors[block.0 as usize] {
                    if !reachable[predecessor.0 as usize] {
                        continue;
                    }
                    let mut edge_state = transfer_tag(
                        incoming[predecessor.0 as usize],
                        &events[predecessor.0 as usize],
                        &fact,
                    );
                    if edge
                        .writes
                        .as_ref()
                        .is_some_and(|write| places_may_overlap(write, &fact.place))
                    {
                        edge_state = false;
                    }
                    if edge.refinement.as_ref() == Some(&fact) {
                        edge_state = true;
                    }
                    state &= edge_state;
                    found = true;
                }
                if !found {
                    continue;
                }
                let index = block.0 as usize;
                if incoming[index] != state {
                    incoming[index] = state;
                    for edge in &successors[index] {
                        let next = edge.target.0 as usize;
                        if reachable[next] && edge.target != function.entry && !queued[next] {
                            queued[next] = true;
                            queue.push_back(edge.target);
                        }
                    }
                }
            }
            for (block_index, block_events) in events.iter().enumerate() {
                if !reachable[block_index] {
                    continue;
                }
                let mut state = incoming[block_index];
                for event in block_events {
                    match event {
                        TagEvent::Require(required) if required == &fact => {
                            if !state {
                                return Err(MirInvariantError::new(
                                    format!("{context} block#{block_index}"),
                                    format!(
                                        "projects {:?} without a dominating matching SwitchTag",
                                        fact.tag
                                    ),
                                ));
                            }
                        }
                        TagEvent::Write(write) if places_may_overlap(write, &fact.place) => {
                            state = false;
                        }
                        TagEvent::Require(_) | TagEvent::Write(_) => {}
                    }
                }
            }
        }
        Ok(())
    }

    fn consume_dataflow_step(&self, context: &str) -> Result<(), MirInvariantError> {
        let next = self.dataflow_steps.get().saturating_add(1);
        if next > self.limits.max_dataflow_steps {
            return Err(MirInvariantError::resource_limit(
                context,
                format!(
                    "MIR verification exceeded its {}-step dataflow budget",
                    self.limits.max_dataflow_steps
                ),
            ));
        }
        self.dataflow_steps.set(next);
        Ok(())
    }

    fn local_events(&self, function: &MirFunction, block: &MirBasicBlock) -> Vec<LocalEvent> {
        let mut events = Vec::new();
        for statement in &block.statements {
            match &statement.kind {
                MirStatementKind::StorageLive(local) => {
                    events.push(LocalEvent::StorageLive(*local));
                }
                MirStatementKind::StorageDead(local) => {
                    events.push(LocalEvent::StorageDead(*local));
                }
                MirStatementKind::ReserveLoan(loan) => {
                    if let Some(loan) = function.loan(*loan) {
                        push_place_events(loan.place(), true, &mut events);
                    }
                }
                MirStatementKind::ReleaseLoan(_) => {}
                MirStatementKind::Assign { destination, value } => {
                    push_rvalue_events(value, &mut events);
                    push_destination_events(destination, &mut events);
                }
                MirStatementKind::RegisterDefer { action, guard, .. } => {
                    push_defer_operation_events(action, guard.as_ref(), &mut events);
                }
                MirStatementKind::RegisterFallback { owner, .. } => {
                    push_place_events(owner, true, &mut events);
                }
                MirStatementKind::EnterTaskScope { .. }
                | MirStatementKind::RetargetCleanup { .. }
                | MirStatementKind::DisarmCleanup(_)
                | MirStatementKind::BeginSelect { .. } => {}
                MirStatementKind::RegisterSelectArm { registration, .. } => match registration {
                    MirSelectRegistration::Call(operation) => {
                        push_operation_events(operation, &mut events);
                    }
                    MirSelectRegistration::Join(place) => {
                        push_destination_reads(place, false, &mut events);
                    }
                },
            }
        }
        match &block.terminator.kind {
            MirTerminatorKind::Goto { .. }
            | MirTerminatorKind::DrainDefers { .. }
            | MirTerminatorKind::DrainScopes { .. }
            | MirTerminatorKind::DrainUnwind { .. }
            | MirTerminatorKind::ResumePanic
            | MirTerminatorKind::Unreachable => {}
            MirTerminatorKind::CommitSelect { arms, .. } => {
                for arm in arms {
                    if let Some(payload) = arm.payload() {
                        push_destination_events(payload, &mut events);
                    }
                }
            }
            MirTerminatorKind::SwitchBool { condition, .. } => {
                push_operand_events(condition, &mut events);
            }
            MirTerminatorKind::SwitchTag { value, .. } => {
                push_operand_events(value, &mut events);
            }
            MirTerminatorKind::Invoke {
                operation,
                destination,
                ..
            } => {
                push_operation_events(operation, &mut events);
                if let Some(destination) = destination {
                    push_destination_reads(destination, true, &mut events);
                }
            }
            MirTerminatorKind::Await {
                awaitable,
                destination,
                ..
            } => {
                match awaitable {
                    MirAwaitable::Call(operation) => {
                        push_operation_events(operation, &mut events);
                    }
                    MirAwaitable::Join(join) => push_operand_events(join, &mut events),
                }
                push_destination_reads(destination, true, &mut events);
            }
            MirTerminatorKind::Spawn {
                operation,
                destination,
                ..
            } => {
                push_operation_events(operation, &mut events);
                push_destination_reads(destination, true, &mut events);
            }
            MirTerminatorKind::IteratorNext {
                state,
                destination,
                borrowed_source,
                ..
            } => {
                push_place_events(state, true, &mut events);
                push_destination_reads(destination, true, &mut events);
                if let Some(source) = borrowed_source {
                    push_place_events(source, true, &mut events);
                }
            }
            MirTerminatorKind::ValidatePlaces {
                places,
                replacements,
                for_write,
                ..
            } => {
                for place in places {
                    push_destination_reads(place, *for_write, &mut events);
                }
                for replacement in replacements.iter().flatten() {
                    push_operand_events(replacement, &mut events);
                }
            }
            MirTerminatorKind::ValidateLoan { loan, .. } => {
                if let Some(loan) = function.loan(*loan) {
                    push_resolve_place_events(loan.place(), &mut events);
                }
            }
            MirTerminatorKind::Return => events.push(LocalEvent::Read(LocalAccess {
                local: function.return_local,
                path: Vec::new(),
                source_loan: None,
            })),
        }
        events
    }

    fn verify_type(
        &self,
        ty: crate::types::TypeId,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        self.hir
            .interner()
            .canonical(ty)
            .map(|_| ())
            .map_err(|error| {
                MirInvariantError::new(context, format!("type {ty} is not canonical: {error}"))
            })
    }

    fn local<'a>(
        &self,
        function: &'a MirFunction,
        id: MirLocalId,
        context: &str,
    ) -> Result<&'a super::MirLocal, MirInvariantError> {
        function.locals.get(id.0 as usize).ok_or_else(|| {
            MirInvariantError::new(
                context,
                format!("references unknown MIR local#{}", id.index()),
            )
        })
    }

    fn loan<'a>(
        &self,
        function: &'a MirFunction,
        id: MirLoanId,
        context: &str,
    ) -> Result<&'a super::MirLoan, MirInvariantError> {
        function.loans.get(id.0 as usize).ok_or_else(|| {
            MirInvariantError::new(
                context,
                format!("references unknown MIR loan#{}", id.index()),
            )
        })
    }

    fn verify_runtime_conflict_ids(
        &self,
        function: &MirFunction,
        loans: &[MirLoanId],
        context: &str,
    ) -> Result<(), MirInvariantError> {
        let mut previous = None;
        for loan in loans {
            self.loan(function, *loan, context)?;
            if previous.is_some_and(|previous| previous >= *loan) {
                return Err(MirInvariantError::new(
                    context,
                    "runtime conflict IDs are not unique and canonical",
                ));
            }
            previous = Some(*loan);
        }
        Ok(())
    }

    fn block<'a>(
        &self,
        function: &'a MirFunction,
        id: MirBlockId,
        context: &str,
    ) -> Result<&'a MirBasicBlock, MirInvariantError> {
        function.blocks.get(id.0 as usize).ok_or_else(|| {
            MirInvariantError::new(
                context,
                format!("references unknown MIR block#{}", id.index()),
            )
        })
    }

    fn normal_block(
        &self,
        function: &MirFunction,
        id: MirBlockId,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        if self.block(function, id, context)?.kind != MirBlockKind::Normal {
            return Err(MirInvariantError::new(
                context,
                format!("ordinary edge enters cleanup block#{}", id.index()),
            ));
        }
        Ok(())
    }
}

fn operation_access_place<'a>(
    operation: &'a MirOperation,
    context: &str,
) -> Result<Option<(MirPlace, &'a [MirLoanId])>, MirInvariantError> {
    let (base, projection, against) = match &operation.kind {
        MirOperationKind::Index {
            base,
            index,
            access,
            against,
        } => (
            base,
            MirProjectionKind::Index {
                index: operand_materialized_local(index, context)?,
                access: *access,
            },
            against.as_slice(),
        ),
        MirOperationKind::Slice {
            base,
            bounds,
            against,
        } => (
            base,
            MirProjectionKind::Slice {
                start: bounds
                    .start
                    .as_ref()
                    .map(|operand| operand_materialized_local(operand, context))
                    .transpose()?,
                end: bounds
                    .end
                    .as_ref()
                    .map(|operand| operand_materialized_local(operand, context))
                    .transpose()?,
                step: bounds
                    .step
                    .as_ref()
                    .map(|operand| operand_materialized_local(operand, context))
                    .transpose()?,
            },
            against.as_slice(),
        ),
        _ => return Ok(None),
    };
    let MirOperandKind::Borrow(base) = &base.kind else {
        return Err(MirInvariantError::new(
            context,
            "indexed operation has no borrowed base place",
        ));
    };
    let mut place = base.clone();
    place.ty = operation.ty;
    place.projections.push(MirProjection {
        ty: operation.ty,
        kind: projection,
    });
    Ok(Some((place, against)))
}

/// A small closed set of compiler-owned operations may retain their exclusive
/// receiver over suspension. Each operation consumes/replaces that receiver
/// atomically at the runtime boundary.
fn async_iterator_receiver_loan(operation: &MirOperation) -> Option<MirLoanId> {
    let MirOperationKind::Call {
        callee, arguments, ..
    } = &operation.kind
    else {
        return None;
    };
    let allowed = match &callee.kind {
        MirOperandKind::PreludeTraitFunction { method, .. } => {
            *method == HirPreludeTraitMethod::AsyncIteratorNext
        }
        MirOperandKind::Function {
            callable: HirCallableId::Host(function),
            ..
        } => *function == HirBootstrapHostFunction::AsyncWaiterWait,
        _ => false,
    };
    if !allowed {
        return None;
    }
    arguments
        .first()
        .and_then(|argument| match argument.value.kind {
            MirOperandKind::Loan(loan) if argument.mode == ParameterMode::Mut => Some(loan),
            _ => None,
        })
}

fn operand_materialized_local(
    operand: &MirOperand,
    context: &str,
) -> Result<MirLocalId, MirInvariantError> {
    match &operand.kind {
        MirOperandKind::Copy(place)
        | MirOperandKind::Move(place)
        | MirOperandKind::Borrow(place)
            if place.projections.is_empty() && place.source_loan.is_none() =>
        {
            Ok(place.local)
        }
        _ => Err(MirInvariantError::new(
            context,
            "index or slice input is not a materialized local",
        )),
    }
}

fn mir_loan_events(function: &MirFunction, block: &MirBasicBlock) -> Vec<LoanEvent> {
    let mut events = Vec::new();
    for statement in &block.statements {
        match &statement.kind {
            MirStatementKind::StorageLive(local) => {
                events.push(LoanEvent::Local(LocalEvent::StorageLive(*local)));
            }
            MirStatementKind::StorageDead(local) => {
                events.push(LoanEvent::Local(LocalEvent::StorageDead(*local)));
            }
            MirStatementKind::ReserveLoan(id) => {
                if let Some(loan) = function.loan(*id) {
                    let mut local = Vec::new();
                    if place_requires_loan_validation(loan.place()) {
                        push_resolve_place_events(loan.place(), &mut local);
                    } else {
                        push_place_events(loan.place(), true, &mut local);
                    }
                    events.extend(local.into_iter().map(LoanEvent::Local));
                }
                events.push(LoanEvent::Reserve(*id));
            }
            MirStatementKind::ReleaseLoan(id) => {
                events.push(LoanEvent::Release(*id));
            }
            MirStatementKind::Assign { destination, value } => {
                let mut local = Vec::new();
                push_rvalue_events(value, &mut local);
                push_destination_events(destination, &mut local);
                events.extend(local.into_iter().map(LoanEvent::Local));
            }
            MirStatementKind::RegisterDefer { action, guard, .. } => {
                let mut local = Vec::new();
                push_defer_operation_events(action, guard.as_ref(), &mut local);
                events.extend(local.into_iter().map(LoanEvent::Local));
            }
            MirStatementKind::RegisterFallback { owner, .. } => {
                let mut local = Vec::new();
                push_place_events(owner, true, &mut local);
                events.extend(local.into_iter().map(LoanEvent::Local));
            }
            MirStatementKind::EnterTaskScope { .. }
            | MirStatementKind::RetargetCleanup { .. }
            | MirStatementKind::DisarmCleanup(_)
            | MirStatementKind::BeginSelect { .. } => {}
            MirStatementKind::RegisterSelectArm { registration, .. } => {
                let mut local = Vec::new();
                match registration {
                    MirSelectRegistration::Call(operation) => {
                        push_operation_events(operation, &mut local);
                    }
                    MirSelectRegistration::Join(_) => {}
                }
                events.extend(local.into_iter().map(LoanEvent::Local));
                if let MirSelectRegistration::Call(MirOperation {
                    kind: MirOperationKind::Call { arguments, .. },
                    ..
                }) = registration
                {
                    events.push(LoanEvent::Consume(
                        arguments
                            .iter()
                            .filter_map(|argument| match &argument.value.kind {
                                MirOperandKind::Loan(loan) => Some(*loan),
                                _ => None,
                            })
                            .collect(),
                    ));
                }
            }
        }
    }
    let mut local = Vec::new();
    match &block.terminator.kind {
        MirTerminatorKind::Goto { .. }
        | MirTerminatorKind::DrainDefers { .. }
        | MirTerminatorKind::DrainScopes { .. }
        | MirTerminatorKind::DrainUnwind { .. }
        | MirTerminatorKind::ResumePanic
        | MirTerminatorKind::Unreachable => {}
        MirTerminatorKind::CommitSelect { arms, .. } => {
            for arm in arms {
                if let Some(payload) = arm.payload() {
                    push_destination_events(payload, &mut local);
                }
            }
        }
        MirTerminatorKind::SwitchBool { condition, .. } => {
            push_operand_events(condition, &mut local);
        }
        MirTerminatorKind::SwitchTag { value, .. } => {
            push_operand_events(value, &mut local);
        }
        MirTerminatorKind::Invoke { operation, .. } => {
            if let Some((place, _)) = operation_access_place(operation, "loan events")
                .expect("verified indexed operations retain materialized places")
            {
                push_resolve_place_events(&place, &mut local);
            } else {
                push_operation_events(operation, &mut local);
            }
        }
        MirTerminatorKind::Await { awaitable, .. } => match awaitable {
            MirAwaitable::Call(operation) => push_operation_events(operation, &mut local),
            MirAwaitable::Join(join) => push_operand_events(join, &mut local),
        },
        MirTerminatorKind::Spawn { operation, .. } => {
            push_operation_events(operation, &mut local);
        }
        MirTerminatorKind::IteratorNext {
            state,
            borrowed_source,
            ..
        } => {
            push_destination_reads(state, true, &mut local);
            if let Some(source) = borrowed_source {
                push_place_events(source, true, &mut local);
            }
        }
        MirTerminatorKind::ValidatePlaces {
            places,
            replacements,
            ..
        } => {
            for place in places {
                push_resolve_place_events(place, &mut local);
            }
            for replacement in replacements.iter().flatten() {
                push_operand_events(replacement, &mut local);
            }
        }
        MirTerminatorKind::ValidateLoan { loan, .. } => {
            if let Some(loan) = function.loan(*loan) {
                push_resolve_place_events(loan.place(), &mut local);
            }
        }
        MirTerminatorKind::Return => local.push(LocalEvent::Read(LocalAccess {
            local: function.return_local,
            path: Vec::new(),
            source_loan: None,
        })),
    }
    events.extend(local.into_iter().map(LoanEvent::Local));
    let operation = match &block.terminator.kind {
        MirTerminatorKind::Invoke { operation, .. }
        | MirTerminatorKind::Spawn { operation, .. } => Some(operation),
        MirTerminatorKind::Await {
            awaitable: MirAwaitable::Call(operation),
            ..
        } => Some(operation),
        _ => None,
    };
    if let Some(MirOperation {
        kind: MirOperationKind::Call { arguments, .. },
        ..
    }) = operation
    {
        events.push(LoanEvent::Consume(
            arguments
                .iter()
                .filter_map(|argument| match &argument.value.kind {
                    MirOperandKind::Loan(loan) => Some(*loan),
                    _ => None,
                })
                .collect(),
        ));
    }
    events
}

fn function_context(id: MirFunctionId) -> String {
    match id {
        MirFunctionId::Callable(HirCallableId::Symbol(symbol)) => {
            format!("MIR function symbol#{}", symbol.index())
        }
        MirFunctionId::Callable(HirCallableId::Member(member)) => {
            format!("MIR function member#{}", member.index())
        }
        MirFunctionId::Callable(HirCallableId::Implementation(method)) => format!(
            "MIR function implementation#{}.method#{}",
            method.implementation().index(),
            method.index()
        ),
        MirFunctionId::Callable(HirCallableId::Host(function)) => {
            format!("MIR host function {}", function.name())
        }
        MirFunctionId::Closure(closure) => {
            format!("MIR closure function#{}", closure.index())
        }
    }
}

fn tag_events(function: &MirFunction, block: &MirBasicBlock) -> Vec<TagEvent> {
    let mut events = Vec::new();
    for statement in &block.statements {
        match &statement.kind {
            MirStatementKind::StorageLive(_)
            | MirStatementKind::StorageDead(_)
            | MirStatementKind::EnterTaskScope { .. }
            | MirStatementKind::ReserveLoan(_)
            | MirStatementKind::ReleaseLoan(_)
            | MirStatementKind::RegisterFallback { .. }
            | MirStatementKind::RetargetCleanup { .. }
            | MirStatementKind::DisarmCleanup(_) => {}
            MirStatementKind::Assign { destination, value } => {
                push_tag_rvalue(function, value, &mut events);
                push_tag_place(function, destination, true, &mut events);
            }
            MirStatementKind::RegisterDefer { action, .. } => {
                push_tag_operation(function, action, &mut events);
            }
            MirStatementKind::BeginSelect { .. } | MirStatementKind::RegisterSelectArm { .. } => {}
        }
    }
    match &block.terminator.kind {
        MirTerminatorKind::Goto { .. }
        | MirTerminatorKind::DrainDefers { .. }
        | MirTerminatorKind::DrainScopes { .. }
        | MirTerminatorKind::DrainUnwind { .. }
        | MirTerminatorKind::Return
        | MirTerminatorKind::ResumePanic
        | MirTerminatorKind::Unreachable => {}
        MirTerminatorKind::SwitchBool { condition, .. } => {
            push_tag_operand(function, condition, &mut events);
        }
        MirTerminatorKind::SwitchTag { value, .. } => {
            push_tag_operand(function, value, &mut events);
        }
        MirTerminatorKind::Invoke {
            operation,
            destination,
            ..
        } => {
            push_tag_operation(function, operation, &mut events);
            if let Some(destination) = destination {
                push_tag_place(function, destination, false, &mut events);
            }
        }
        MirTerminatorKind::Await {
            awaitable,
            destination,
            ..
        } => {
            match awaitable {
                MirAwaitable::Call(operation) => {
                    push_tag_operation(function, operation, &mut events);
                }
                MirAwaitable::Join(join) => push_tag_operand(function, join, &mut events),
            }
            push_tag_place(function, destination, false, &mut events);
        }
        MirTerminatorKind::Spawn {
            operation,
            destination,
            ..
        } => {
            push_tag_operation(function, operation, &mut events);
            push_tag_place(function, destination, false, &mut events);
        }
        MirTerminatorKind::IteratorNext {
            state, destination, ..
        } => {
            push_tag_place(function, state, false, &mut events);
            push_tag_place(function, destination, false, &mut events);
        }
        MirTerminatorKind::ValidatePlaces {
            places,
            replacements,
            ..
        } => {
            for place in places {
                push_tag_place(function, place, false, &mut events);
            }
            for replacement in replacements.iter().flatten() {
                push_tag_operand(function, replacement, &mut events);
            }
        }
        MirTerminatorKind::ValidateLoan { loan, .. } => {
            if let Some(loan) = function.loan(*loan) {
                push_tag_place(function, loan.place(), false, &mut events);
            }
        }
        MirTerminatorKind::CommitSelect { arms, .. } => {
            for arm in arms {
                if let Some(payload) = arm.payload() {
                    push_tag_place(function, payload, true, &mut events);
                }
            }
        }
    }
    events
}

fn push_tag_rvalue(function: &MirFunction, value: &MirRvalue, events: &mut Vec<TagEvent>) {
    match &value.kind {
        MirRvalueKind::Use(operand)
        | MirRvalueKind::Prefix { operand, .. }
        | MirRvalueKind::Coerce { value: operand, .. }
        | MirRvalueKind::NumericConversion { value: operand, .. }
        | MirRvalueKind::Length(operand)
        | MirRvalueKind::IteratorState { source: operand } => {
            push_tag_operand(function, operand, events);
        }
        MirRvalueKind::Binary { left, right, .. } => {
            push_tag_operand(function, left, events);
            push_tag_operand(function, right, events);
        }
        MirRvalueKind::Aggregate { values, .. } => {
            for value in values {
                push_tag_operand(function, value, events);
            }
        }
        MirRvalueKind::Interpolate { values, .. } => {
            for value in values {
                push_tag_operand(function, value, events);
            }
        }
        MirRvalueKind::RecordUpdate { base, fields } => {
            push_tag_operand(function, base, events);
            for (_, value) in fields {
                push_tag_operand(function, value, events);
            }
        }
        MirRvalueKind::Range { start, end, .. } => {
            push_tag_operand(function, start, events);
            push_tag_operand(function, end, events);
        }
        MirRvalueKind::Contains {
            item, container, ..
        } => {
            push_tag_operand(function, item, events);
            push_tag_operand(function, container, events);
        }
        MirRvalueKind::MapRemove { map, key } => {
            push_tag_place(function, map, true, events);
            push_tag_operand(function, key, events);
        }
    }
}

fn push_tag_operation(
    function: &MirFunction,
    operation: &MirOperation,
    events: &mut Vec<TagEvent>,
) {
    match &operation.kind {
        MirOperationKind::CheckedPrefix { operand, .. } => {
            push_tag_operand(function, operand, events);
        }
        MirOperationKind::CheckedBinary { left, right, .. } => {
            push_tag_operand(function, left, events);
            push_tag_operand(function, right, events);
        }
        MirOperationKind::ArraySequence {
            array, argument, ..
        } => {
            push_tag_operand(function, array, events);
            push_tag_operand(function, argument, events);
        }
        MirOperationKind::BuildMap { entries, .. } => {
            for (key, value) in entries {
                push_tag_operand(function, key, events);
                push_tag_operand(function, value, events);
            }
        }
        MirOperationKind::Index { base, index, .. } => {
            push_tag_operand(function, base, events);
            push_tag_operand(function, index, events);
        }
        MirOperationKind::Slice { base, bounds, .. } => {
            push_tag_operand(function, base, events);
            for value in bounds.start.iter().chain(&bounds.end).chain(&bounds.step) {
                push_tag_operand(function, value, events);
            }
        }
        MirOperationKind::Call {
            callee, arguments, ..
        } => {
            push_tag_operand(function, callee, events);
            for argument in arguments {
                push_tag_operand(function, &argument.value, events);
            }
        }
        MirOperationKind::ExplicitPanic { message } => {
            push_tag_operand(function, message, events);
        }
        MirOperationKind::Assert {
            condition,
            message_parts,
            ..
        } => {
            push_tag_operand(function, condition, events);
            for part in message_parts {
                push_tag_operand(function, part.value(), events);
            }
        }
        MirOperationKind::BootstrapHostCall { arguments, .. } => {
            for argument in arguments {
                push_tag_operand(function, argument, events);
            }
        }
        MirOperationKind::Format { value, display } => {
            push_tag_operand(function, value, events);
            if let Some(display) = display {
                push_tag_operand(function, display, events);
            }
        }
        MirOperationKind::JoinFormat {
            values,
            separator,
            display,
        } => {
            push_tag_operand(function, values, events);
            push_tag_operand(function, separator, events);
            if let Some(display) = display {
                push_tag_operand(function, display, events);
            }
        }
    }
}

fn push_tag_operand(function: &MirFunction, operand: &MirOperand, events: &mut Vec<TagEvent>) {
    if let MirOperandKind::Copy(place)
    | MirOperandKind::Move(place)
    | MirOperandKind::Borrow(place) = &operand.kind
    {
        push_tag_place(function, place, false, events);
    }
}

fn push_tag_place(
    function: &MirFunction,
    place: &MirPlace,
    write: bool,
    events: &mut Vec<TagEvent>,
) {
    let root_type = function.locals[place.local.0 as usize].ty;
    for (index, projection) in place.projections.iter().enumerate() {
        let tag = match &projection.kind {
            MirProjectionKind::OptionValue => Some(MirTag::OptionSome),
            MirProjectionKind::ResultOkValue => Some(MirTag::ResultOk),
            MirProjectionKind::ResultErrValue => Some(MirTag::ResultErr),
            MirProjectionKind::VariantTuple { variant, .. }
            | MirProjectionKind::VariantField { variant, .. } => Some(MirTag::Variant(*variant)),
            MirProjectionKind::UnionValue(member) => Some(MirTag::Union(*member)),
            MirProjectionKind::ClosureCapture { .. }
            | MirProjectionKind::IteratorSource
            | MirProjectionKind::Field(_)
            | MirProjectionKind::TupleField(_)
            | MirProjectionKind::NewtypeValue
            | MirProjectionKind::RefValue
            | MirProjectionKind::ArrayPatternIndex(_)
            | MirProjectionKind::ArrayPatternRest { .. }
            | MirProjectionKind::IteratorElement { .. }
            | MirProjectionKind::Index { .. }
            | MirProjectionKind::Slice { .. } => None,
        };
        if let Some(tag) = tag {
            let base = MirPlace {
                local: place.local,
                ty: if index == 0 {
                    root_type
                } else {
                    place.projections[index - 1].ty
                },
                projections: place.projections[..index].to_vec(),
                // Region loans authorize access to the same underlying
                // discriminant; they do not create a distinct tagged place.
                source_loan: None,
            };
            events.push(TagEvent::Require(TagFact { place: base, tag }));
        }
    }
    if write {
        events.push(TagEvent::Write(place.clone()));
    }
}

fn transfer_tag(state: bool, events: &[TagEvent], fact: &TagFact) -> bool {
    let mut state = state;
    for event in events {
        if let TagEvent::Write(write) = event
            && places_may_overlap(write, &fact.place)
        {
            state = false;
        }
    }
    state
}

fn places_may_overlap(left: &MirPlace, right: &MirPlace) -> bool {
    if left.local != right.local {
        return false;
    }
    for (left, right) in left.projections.iter().zip(&right.projections) {
        if left == right {
            continue;
        }
        return match (&left.kind, &right.kind) {
            (MirProjectionKind::Field(left), MirProjectionKind::Field(right)) => left == right,
            (MirProjectionKind::TupleField(left), MirProjectionKind::TupleField(right)) => {
                left == right
            }
            (
                MirProjectionKind::ArrayPatternIndex(left),
                MirProjectionKind::ArrayPatternIndex(right),
            ) => left == right,
            (
                MirProjectionKind::VariantTuple { variant: left, .. }
                | MirProjectionKind::VariantField { variant: left, .. },
                MirProjectionKind::VariantTuple { variant: right, .. }
                | MirProjectionKind::VariantField { variant: right, .. },
            ) => left == right,
            _ => true,
        };
    }
    true
}

fn same_place_path(left: &MirPlace, right: &MirPlace) -> bool {
    left.local == right.local && left.ty == right.ty && left.projections == right.projections
}

fn successor_edges(terminator: &MirTerminatorKind) -> Vec<SuccessorEdge> {
    let edge = |target| SuccessorEdge {
        target,
        refinement: None,
        writes: None,
    };
    match terminator {
        MirTerminatorKind::Goto { target } => vec![edge(*target)],
        MirTerminatorKind::SwitchBool {
            if_true, if_false, ..
        } => vec![edge(*if_true), edge(*if_false)],
        MirTerminatorKind::SwitchTag {
            value,
            cases,
            otherwise,
        } => {
            let place = match &value.kind {
                MirOperandKind::Copy(place)
                | MirOperandKind::Move(place)
                | MirOperandKind::Borrow(place) => {
                    let mut place = place.clone();
                    // Match refinement follows the semantic place path across
                    // aliases borrowed from that path. The loan handle is
                    // request-local access metadata, not tag identity.
                    place.source_loan = None;
                    Some(place)
                }
                MirOperandKind::Constant(_)
                | MirOperandKind::Function { .. }
                | MirOperandKind::PreludeTraitFunction { .. }
                | MirOperandKind::Loan(_) => None,
            };
            cases
                .iter()
                .map(|(tag, target)| SuccessorEdge {
                    target: *target,
                    refinement: place.clone().map(|place| TagFact { place, tag: *tag }),
                    writes: None,
                })
                .chain(std::iter::once(SuccessorEdge {
                    target: *otherwise,
                    refinement: (cases.len() == 1)
                        .then(|| complementary_tag(cases[0].0))
                        .flatten()
                        .and_then(|tag| place.clone().map(|place| TagFact { place, tag })),
                    writes: None,
                }))
                .collect()
        }
        MirTerminatorKind::Invoke {
            destination,
            target,
            unwind,
            ..
        } => target
            .iter()
            .map(|target| SuccessorEdge {
                target: *target,
                refinement: None,
                writes: destination.clone(),
            })
            .chain(std::iter::once(edge(*unwind)))
            .collect(),
        MirTerminatorKind::Await {
            destination,
            target,
            unwind,
            ..
        }
        | MirTerminatorKind::Spawn {
            destination,
            target,
            unwind,
            ..
        } => vec![
            SuccessorEdge {
                target: *target,
                refinement: None,
                writes: Some(destination.clone()),
            },
            edge(*unwind),
        ],
        MirTerminatorKind::CommitSelect {
            arms,
            else_target,
            unwind,
        } => {
            let mut successors = Vec::new();
            for arm in arms {
                successors.push(SuccessorEdge {
                    target: arm.target(),
                    refinement: None,
                    writes: arm.payload().cloned(),
                });
            }
            for target in else_target.iter().copied() {
                successors.push(edge(target));
            }
            successors.push(edge(*unwind));
            successors
        }
        MirTerminatorKind::IteratorNext {
            destination,
            has_value,
            exhausted,
            unwind,
            ..
        } => vec![
            SuccessorEdge {
                target: *has_value,
                refinement: None,
                writes: Some(destination.clone()),
            },
            edge(*exhausted),
            edge(*unwind),
        ],
        MirTerminatorKind::ValidatePlaces { target, unwind, .. }
        | MirTerminatorKind::ValidateLoan { target, unwind, .. }
        | MirTerminatorKind::DrainDefers { target, unwind, .. }
        | MirTerminatorKind::DrainScopes { target, unwind, .. } => {
            vec![edge(*target), edge(*unwind)]
        }
        MirTerminatorKind::DrainUnwind { target } => vec![edge(*target)],
        MirTerminatorKind::Return
        | MirTerminatorKind::ResumePanic
        | MirTerminatorKind::Unreachable => Vec::new(),
    }
}

fn intersect_optional_set(target: &mut Option<BTreeSet<u32>>, source: BTreeSet<u32>) {
    let _ = intersect_incoming_set(target, source);
}

fn intersect_incoming_set(target: &mut Option<BTreeSet<u32>>, source: BTreeSet<u32>) -> bool {
    let Some(target) = target else {
        *target = Some(source);
        return true;
    };
    let previous = target.len();
    target.retain(|value| source.contains(value));
    target.len() != previous
}

fn complementary_tag(tag: MirTag) -> Option<MirTag> {
    match tag {
        MirTag::OptionNone => Some(MirTag::OptionSome),
        MirTag::OptionSome => Some(MirTag::OptionNone),
        MirTag::ResultOk => Some(MirTag::ResultErr),
        MirTag::ResultErr => Some(MirTag::ResultOk),
        MirTag::Variant(_) | MirTag::NumericConversionError(_) | MirTag::Union(_) => None,
    }
}

fn transfer_local(state: LocalState, events: &[LocalEvent], local: MirLocalId) -> LocalState {
    let mut state = state;
    for event in events {
        match event {
            LocalEvent::Write(access) if access.local == local => {
                if state.live {
                    write_path_unchecked(&mut state.unavailable, &access.path);
                }
            }
            LocalEvent::Move(access) if access.local == local => {
                if state.live {
                    move_path_unchecked(&mut state.unavailable, access.path.clone());
                }
            }
            LocalEvent::StorageLive(event_local) if *event_local == local => {
                state.live = true;
                state.unavailable.clear();
                state.unavailable.insert(Vec::new());
            }
            LocalEvent::StorageDead(event_local) if *event_local == local => {
                state.live = false;
                state.unavailable.clear();
                state.unavailable.insert(Vec::new());
            }
            LocalEvent::Read(_)
            | LocalEvent::Resolve(_)
            | LocalEvent::Move(_)
            | LocalEvent::Write(_)
            | LocalEvent::WriteAccess(_)
            | LocalEvent::StorageLive(_)
            | LocalEvent::StorageDead(_) => {}
        }
    }
    state
}

fn path_is_available(
    unavailable: &BTreeSet<Vec<MovePathComponent>>,
    path: &[MovePathComponent],
) -> bool {
    unavailable
        .iter()
        .all(|moved| !move_paths_overlap(moved, path))
}

fn path_parent_is_available(
    unavailable: &BTreeSet<Vec<MovePathComponent>>,
    path: &[MovePathComponent],
) -> bool {
    unavailable
        .iter()
        .all(|moved| !(moved.len() < path.len() && move_path_is_prefix(moved, path)))
}

fn move_path_unchecked(
    unavailable: &mut BTreeSet<Vec<MovePathComponent>>,
    path: Vec<MovePathComponent>,
) {
    if path.is_empty() {
        unavailable.clear();
    } else if unavailable
        .iter()
        .any(|moved| move_path_is_prefix(moved, &path))
    {
        return;
    } else {
        unavailable.retain(|moved| !move_path_is_prefix(&path, moved));
    }
    unavailable.insert(path);
}

fn write_path_unchecked(
    unavailable: &mut BTreeSet<Vec<MovePathComponent>>,
    path: &[MovePathComponent],
) {
    unavailable.retain(|moved| !move_path_is_prefix(path, moved));
}

fn move_paths_overlap(left: &[MovePathComponent], right: &[MovePathComponent]) -> bool {
    left.iter()
        .zip(right)
        .all(|(left, right)| !move_path_components_are_disjoint(left, right))
}

fn local_accesses_overlap(left: &LocalAccess, right: &LocalAccess) -> bool {
    left.local == right.local && move_paths_overlap(&left.path, &right.path)
}

fn local_access_contains(outer: &LocalAccess, inner: &LocalAccess) -> bool {
    outer.local == inner.local
        && outer.source_loan == inner.source_loan
        && outer.path.len() <= inner.path.len()
        && outer
            .path
            .iter()
            .zip(&inner.path)
            .all(|(left, right)| left == right)
}

fn local_access_is_complete_sum_payload(owner: &LocalAccess, access: &LocalAccess) -> bool {
    owner.local == access.local
        && owner.source_loan == access.source_loan
        && access.path.len() == owner.path.len() + 1
        && access.path.starts_with(&owner.path)
        && matches!(
            access.path.last(),
            Some(
                MovePathComponent::OptionValue
                    | MovePathComponent::ResultOkValue
                    | MovePathComponent::ResultErrValue
                    | MovePathComponent::UnionValue(_)
            )
        )
}

fn apply_consumed_defer_events(
    state: &mut DeferFlowState,
    events: &[LocalEvent],
    context: &str,
) -> Result<(), MirInvariantError> {
    for event in events {
        match event {
            LocalEvent::Read(access) | LocalEvent::Resolve(access) | LocalEvent::Move(access) => {
                if state
                    .consumed
                    .iter()
                    .any(|consumed| local_accesses_overlap(consumed, access))
                {
                    return Err(MirInvariantError::new(
                        context,
                        "accesses an owner already consumed by a deferred action",
                    ));
                }
            }
            LocalEvent::WriteAccess(access) => {
                if state.consumed.iter().any(|consumed| {
                    local_accesses_overlap(consumed, access)
                        && !local_access_contains(access, consumed)
                }) {
                    return Err(MirInvariantError::new(
                        context,
                        "resolves a partial write through an owner consumed by a deferred action",
                    ));
                }
            }
            LocalEvent::Write(access) => {
                let overlapping = state
                    .consumed
                    .iter()
                    .filter(|consumed| local_accesses_overlap(consumed, access))
                    .cloned()
                    .collect::<Vec<_>>();
                if overlapping
                    .iter()
                    .any(|consumed| !local_access_contains(access, consumed))
                {
                    return Err(MirInvariantError::new(
                        context,
                        "partially reinitializes an owner consumed by a deferred action",
                    ));
                }
                state
                    .consumed
                    .retain(|consumed| !local_access_contains(access, consumed));
            }
            LocalEvent::StorageLive(local) | LocalEvent::StorageDead(local) => {
                state.consumed.retain(|consumed| consumed.local != *local);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
fn loan_paths_overlap(
    left: &[MovePathComponent],
    right: &[MovePathComponent],
    static_integers: &BTreeMap<MirLocalId, u64>,
) -> bool {
    loan_paths_relation(left, right, static_integers) != StaticRegionRelation::Disjoint
}

fn loan_paths_relation(
    left: &[MovePathComponent],
    right: &[MovePathComponent],
    static_integers: &BTreeMap<MirLocalId, u64>,
) -> StaticRegionRelation {
    let mut relation = StaticRegionRelation::Overlap;
    for (left, right) in left.iter().zip(right) {
        if matches!(
            (left, right),
            (
                MovePathComponent::IteratorElement { index: left },
                MovePathComponent::IteratorElement { index: right }
            ) if left == right
        ) {
            continue;
        }
        match (
            collection_region(left, static_integers),
            collection_region(right, static_integers),
        ) {
            (CollectionComponent::Static(left), CollectionComponent::Static(right)) => {
                let current = static_collection_relation(left, right);
                if current == StaticRegionRelation::Disjoint {
                    return current;
                }
                if static_regions_are_identical(left, right) {
                    relation = current;
                    continue;
                }
                return current;
            }
            (CollectionComponent::None, CollectionComponent::None) => {
                if left == right {
                    continue;
                }
                if move_path_components_are_disjoint(left, right) {
                    return StaticRegionRelation::Disjoint;
                }
                return StaticRegionRelation::Overlap;
            }
            (CollectionComponent::Dynamic, _)
            | (_, CollectionComponent::Dynamic)
            | (CollectionComponent::Static(_), CollectionComponent::None)
            | (CollectionComponent::None, CollectionComponent::Static(_)) => {
                return StaticRegionRelation::Runtime;
            }
        }
    }
    relation
}

fn move_path_runtime_inputs(path: &[MovePathComponent]) -> impl Iterator<Item = MirLocalId> + '_ {
    path.iter().flat_map(|component| {
        let inputs = match component {
            MovePathComponent::Index { index, .. } => [Some(*index), None, None],
            MovePathComponent::IteratorElement { index } => [Some(*index), None, None],
            MovePathComponent::Slice { start, end, step } => [*start, *end, *step],
            MovePathComponent::ClosureCapture(_, _)
            | MovePathComponent::IteratorSource
            | MovePathComponent::Field(_)
            | MovePathComponent::TupleField(_)
            | MovePathComponent::NewtypeValue
            | MovePathComponent::RefValue
            | MovePathComponent::VariantTuple(_, _)
            | MovePathComponent::VariantField(_, _)
            | MovePathComponent::OptionValue
            | MovePathComponent::ResultOkValue
            | MovePathComponent::ResultErrValue
            | MovePathComponent::UnionValue(_)
            | MovePathComponent::ArrayPatternIndex(_)
            | MovePathComponent::ArrayPatternRest { .. } => [None, None, None],
        };
        inputs.into_iter().flatten()
    })
}

#[derive(Clone, Copy)]
enum CollectionComponent {
    None,
    Static(StaticCollectionRegion),
    Dynamic,
}

fn static_regions_are_identical(
    left: StaticCollectionRegion,
    right: StaticCollectionRegion,
) -> bool {
    if left == right {
        return true;
    }
    matches!(
        (left, right),
        (
            StaticCollectionRegion::Index(left),
            StaticCollectionRegion::PatternIndex(right)
        ) | (
            StaticCollectionRegion::PatternIndex(right),
            StaticCollectionRegion::Index(left)
        ) if left == u64::from(right)
    )
}

fn collection_region(
    component: &MovePathComponent,
    static_integers: &BTreeMap<MirLocalId, u64>,
) -> CollectionComponent {
    match component {
        MovePathComponent::ArrayPatternIndex(index) => {
            CollectionComponent::Static(StaticCollectionRegion::PatternIndex(*index))
        }
        MovePathComponent::ArrayPatternRest { start, suffix } => {
            CollectionComponent::Static(StaticCollectionRegion::PatternRest {
                start: *start,
                suffix: *suffix,
            })
        }
        MovePathComponent::Index {
            index,
            access: HirIndexAccess::Array,
        } => static_integers
            .get(index)
            .map_or(CollectionComponent::Dynamic, |index| {
                CollectionComponent::Static(StaticCollectionRegion::Index(*index))
            }),
        MovePathComponent::Index {
            access: HirIndexAccess::String | HirIndexAccess::MapLookup | HirIndexAccess::MapEntry,
            ..
        }
        | MovePathComponent::IteratorElement { .. } => CollectionComponent::Dynamic,
        MovePathComponent::Slice { start, end, step } => {
            let Some(start) = static_optional_bound(*start, static_integers) else {
                return CollectionComponent::Dynamic;
            };
            let Some(end) = static_optional_bound(*end, static_integers) else {
                return CollectionComponent::Dynamic;
            };
            let Some(step) = static_optional_bound(*step, static_integers) else {
                return CollectionComponent::Dynamic;
            };
            CollectionComponent::Static(StaticCollectionRegion::Slice(StaticSlice {
                start,
                end,
                step,
            }))
        }
        MovePathComponent::ClosureCapture(_, _)
        | MovePathComponent::IteratorSource
        | MovePathComponent::Field(_)
        | MovePathComponent::TupleField(_)
        | MovePathComponent::NewtypeValue
        | MovePathComponent::RefValue
        | MovePathComponent::VariantTuple(_, _)
        | MovePathComponent::VariantField(_, _)
        | MovePathComponent::OptionValue
        | MovePathComponent::ResultOkValue
        | MovePathComponent::ResultErrValue
        | MovePathComponent::UnionValue(_) => CollectionComponent::None,
    }
}

fn static_optional_bound(
    local: Option<MirLocalId>,
    static_integers: &BTreeMap<MirLocalId, u64>,
) -> Option<Option<u64>> {
    match local {
        Some(local) => Some(Some(*static_integers.get(&local)?)),
        None => Some(None),
    }
}

fn move_path_is_prefix(prefix: &[MovePathComponent], path: &[MovePathComponent]) -> bool {
    prefix.len() <= path.len() && prefix.iter().zip(path).all(|(left, right)| left == right)
}

fn move_path_components_are_disjoint(left: &MovePathComponent, right: &MovePathComponent) -> bool {
    match (left, right) {
        (
            MovePathComponent::ClosureCapture(_, left),
            MovePathComponent::ClosureCapture(_, right),
        )
        | (MovePathComponent::TupleField(left), MovePathComponent::TupleField(right))
        | (
            MovePathComponent::ArrayPatternIndex(left),
            MovePathComponent::ArrayPatternIndex(right),
        ) => left != right,
        (MovePathComponent::Field(left), MovePathComponent::Field(right)) => left != right,
        (
            MovePathComponent::VariantTuple(left_variant, left),
            MovePathComponent::VariantTuple(right_variant, right),
        ) => left_variant != right_variant || left != right,
        (
            MovePathComponent::VariantField(left_variant, left),
            MovePathComponent::VariantField(right_variant, right),
        ) => left_variant != right_variant || left != right,
        (
            MovePathComponent::VariantTuple(left, _) | MovePathComponent::VariantField(left, _),
            MovePathComponent::VariantTuple(right, _) | MovePathComponent::VariantField(right, _),
        ) => left != right,
        (MovePathComponent::OptionValue, MovePathComponent::ResultOkValue)
        | (MovePathComponent::OptionValue, MovePathComponent::ResultErrValue)
        | (MovePathComponent::ResultOkValue, MovePathComponent::OptionValue)
        | (MovePathComponent::ResultErrValue, MovePathComponent::OptionValue)
        | (MovePathComponent::ResultOkValue, MovePathComponent::ResultErrValue)
        | (MovePathComponent::ResultErrValue, MovePathComponent::ResultOkValue) => true,
        (MovePathComponent::UnionValue(left), MovePathComponent::UnionValue(right)) => {
            left != right
        }
        (
            MovePathComponent::ArrayPatternIndex(index),
            MovePathComponent::ArrayPatternRest { start, suffix: 0 },
        )
        | (
            MovePathComponent::ArrayPatternRest { start, suffix: 0 },
            MovePathComponent::ArrayPatternIndex(index),
        ) => index < start,
        _ => false,
    }
}

fn unavailable_read_message(local: MirLocalId, path: &[MovePathComponent]) -> String {
    if path.is_empty() {
        format!(
            "reads local#{} before a dominating live definition",
            local.index()
        )
    } else {
        format!("reads an unavailable move path of local#{}", local.index())
    }
}

fn unavailable_move_message(local: MirLocalId, path: &[MovePathComponent]) -> String {
    if path.is_empty() {
        format!(
            "moves local#{} after its value became unavailable",
            local.index()
        )
    } else {
        format!("moves an unavailable move path of local#{}", local.index())
    }
}

fn push_rvalue_events(value: &MirRvalue, events: &mut Vec<LocalEvent>) {
    match &value.kind {
        MirRvalueKind::Use(operand)
        | MirRvalueKind::Prefix { operand, .. }
        | MirRvalueKind::Coerce { value: operand, .. }
        | MirRvalueKind::NumericConversion { value: operand, .. }
        | MirRvalueKind::Length(operand)
        | MirRvalueKind::IteratorState { source: operand } => {
            push_operand_events(operand, events);
        }
        MirRvalueKind::Binary { left, right, .. } => {
            push_operand_events(left, events);
            push_operand_events(right, events);
        }
        MirRvalueKind::Aggregate { values, .. } => {
            for value in values {
                push_operand_events(value, events);
            }
        }
        MirRvalueKind::Interpolate { values, .. } => {
            for value in values {
                push_operand_events(value, events);
            }
        }
        MirRvalueKind::RecordUpdate { base, fields } => {
            push_operand_events(base, events);
            for (_, value) in fields {
                push_operand_events(value, events);
            }
        }
        MirRvalueKind::Range { start, end, .. } => {
            push_operand_events(start, events);
            push_operand_events(end, events);
        }
        MirRvalueKind::Contains {
            item, container, ..
        } => {
            push_operand_events(item, events);
            push_operand_events(container, events);
        }
        MirRvalueKind::MapRemove { map, key } => {
            push_destination_reads(map, true, events);
            push_operand_events(key, events);
        }
    }
}

fn push_operation_events(operation: &MirOperation, events: &mut Vec<LocalEvent>) {
    match &operation.kind {
        MirOperationKind::CheckedPrefix { operand, .. } => push_operand_events(operand, events),
        MirOperationKind::CheckedBinary { left, right, .. } => {
            push_operand_events(left, events);
            push_operand_events(right, events);
        }
        MirOperationKind::ArraySequence {
            array, argument, ..
        } => {
            push_operand_events(array, events);
            push_operand_events(argument, events);
        }
        MirOperationKind::BuildMap { entries, .. } => {
            for (key, value) in entries {
                push_operand_events(key, events);
                push_operand_events(value, events);
            }
        }
        MirOperationKind::Index { base, index, .. } => {
            push_operand_events(base, events);
            push_operand_events(index, events);
        }
        MirOperationKind::Slice { base, bounds, .. } => {
            push_operand_events(base, events);
            for value in bounds.start.iter().chain(&bounds.end).chain(&bounds.step) {
                push_operand_events(value, events);
            }
        }
        MirOperationKind::Call {
            callee, arguments, ..
        } => {
            push_operand_events(callee, events);
            for argument in arguments {
                push_operand_events(&argument.value, events);
            }
        }
        MirOperationKind::ExplicitPanic { message } => push_operand_events(message, events),
        MirOperationKind::Assert {
            condition,
            message_parts,
            ..
        } => {
            push_operand_events(condition, events);
            for part in message_parts {
                push_operand_events(part.value(), events);
            }
        }
        MirOperationKind::BootstrapHostCall { arguments, .. } => {
            for argument in arguments {
                push_operand_events(argument, events);
            }
        }
        MirOperationKind::Format { value, display } => {
            push_operand_events(value, events);
            if let Some(display) = display {
                push_operand_events(display, events);
            }
        }
        MirOperationKind::JoinFormat {
            values,
            separator,
            display,
        } => {
            push_operand_events(values, events);
            push_operand_events(separator, events);
            if let Some(display) = display {
                push_operand_events(display, events);
            }
        }
    }
}

fn push_defer_operation_events(
    operation: &MirOperation,
    guard: Option<&MirPlace>,
    events: &mut Vec<LocalEvent>,
) {
    for operand in operation_operands(operation) {
        if let (Some(guard), MirOperandKind::Move(place)) = (guard, operand.kind())
            && place == guard
        {
            push_resolve_place_events(place, events);
        } else {
            push_operand_events(operand, events);
        }
    }
}

fn push_operand_events(operand: &MirOperand, events: &mut Vec<LocalEvent>) {
    match &operand.kind {
        MirOperandKind::Move(place) => {
            push_projection_index_events(place, events);
            events.push(LocalEvent::Move(LocalAccess::from_place(place)));
        }
        MirOperandKind::Copy(place) | MirOperandKind::Borrow(place) => {
            push_projection_index_events(place, events);
            events.push(LocalEvent::Read(LocalAccess::from_place(place)));
        }
        MirOperandKind::Constant(_)
        | MirOperandKind::Loan(_)
        | MirOperandKind::Function { .. }
        | MirOperandKind::PreludeTraitFunction { .. } => {}
    }
}

fn push_destination_events(place: &MirPlace, events: &mut Vec<LocalEvent>) {
    push_projection_index_events(place, events);
    events.push(LocalEvent::Write(LocalAccess::from_place(place)));
}

fn push_destination_reads(place: &MirPlace, for_write: bool, events: &mut Vec<LocalEvent>) {
    push_projection_index_events(place, events);
    let access = LocalAccess::from_place(place);
    if for_write {
        events.push(LocalEvent::WriteAccess(access));
    } else {
        events.push(LocalEvent::Read(access));
    }
}

fn push_place_events(place: &MirPlace, read_root: bool, events: &mut Vec<LocalEvent>) {
    push_projection_index_events(place, events);
    if read_root {
        events.push(LocalEvent::Read(LocalAccess::from_place(place)));
    }
}

fn push_resolve_place_events(place: &MirPlace, events: &mut Vec<LocalEvent>) {
    push_projection_index_events(place, events);
    events.push(LocalEvent::Resolve(LocalAccess::from_place(place)));
}

fn push_projection_index_events(place: &MirPlace, events: &mut Vec<LocalEvent>) {
    for projection in &place.projections {
        match &projection.kind {
            MirProjectionKind::Index { index, .. } => events.push(LocalEvent::Read(LocalAccess {
                local: *index,
                path: Vec::new(),
                source_loan: None,
            })),
            MirProjectionKind::IteratorElement { index } => {
                events.push(LocalEvent::Read(LocalAccess {
                    local: *index,
                    path: Vec::new(),
                    source_loan: None,
                }));
            }
            MirProjectionKind::Slice { start, end, step } => {
                events.extend(start.iter().chain(end).chain(step).copied().map(|local| {
                    LocalEvent::Read(LocalAccess {
                        local,
                        path: Vec::new(),
                        source_loan: None,
                    })
                }));
            }
            MirProjectionKind::ClosureCapture { .. }
            | MirProjectionKind::IteratorSource
            | MirProjectionKind::Field(_)
            | MirProjectionKind::TupleField(_)
            | MirProjectionKind::NewtypeValue
            | MirProjectionKind::RefValue
            | MirProjectionKind::VariantTuple { .. }
            | MirProjectionKind::VariantField { .. }
            | MirProjectionKind::OptionValue
            | MirProjectionKind::ResultOkValue
            | MirProjectionKind::ResultErrValue
            | MirProjectionKind::UnionValue(_)
            | MirProjectionKind::ArrayPatternIndex(_)
            | MirProjectionKind::ArrayPatternRest { .. } => {}
        }
    }
}

fn place_requires_loan_validation(place: &MirPlace) -> bool {
    place.projections.iter().any(|projection| {
        matches!(
            projection.kind,
            MirProjectionKind::Index { .. } | MirProjectionKind::Slice { .. }
        )
    })
}

fn place_contains_ref_value(place: &MirPlace) -> bool {
    place
        .projections
        .iter()
        .any(|projection| matches!(projection.kind, MirProjectionKind::RefValue))
}

fn place_is_structurally_replaceable(place: &MirPlace) -> bool {
    matches!(
        place.projections.last().map(|projection| &projection.kind),
        Some(
            MirProjectionKind::ClosureCapture { .. }
                | MirProjectionKind::Field(_)
                | MirProjectionKind::TupleField(_)
                | MirProjectionKind::NewtypeValue
                | MirProjectionKind::VariantTuple { .. }
                | MirProjectionKind::VariantField { .. }
                | MirProjectionKind::OptionValue
                | MirProjectionKind::ResultOkValue
                | MirProjectionKind::ResultErrValue
                | MirProjectionKind::UnionValue(_)
                | MirProjectionKind::ArrayPatternIndex(_)
                | MirProjectionKind::IteratorElement { .. }
                | MirProjectionKind::Index {
                    access: crate::hir::HirIndexAccess::Array,
                    ..
                }
        )
    )
}

fn is_integer(scalar: ScalarType) -> bool {
    matches!(
        scalar,
        ScalarType::Int
            | ScalarType::Int8
            | ScalarType::Int16
            | ScalarType::Int32
            | ScalarType::UInt8
            | ScalarType::UInt16
            | ScalarType::UInt32
            | ScalarType::UInt64
    )
}

fn is_signed_integer(scalar: ScalarType) -> bool {
    matches!(
        scalar,
        ScalarType::Int | ScalarType::Int8 | ScalarType::Int16 | ScalarType::Int32
    )
}

fn is_float(scalar: ScalarType) -> bool {
    matches!(scalar, ScalarType::Float | ScalarType::Float32)
}

fn is_arithmetic(scalar: ScalarType) -> bool {
    is_integer(scalar) || is_float(scalar)
}

fn is_relational(scalar: ScalarType) -> bool {
    is_arithmetic(scalar)
        || matches!(
            scalar,
            ScalarType::Byte | ScalarType::Char | ScalarType::String
        )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::hir::{
        ExpressionCheckLimits, HirCallArgumentTarget, HirRangeKind, TypeLoweringLimits,
        check_expressions, lower_types,
    };
    use crate::mir::{
        MirAssertMessagePart, MirBootstrapHostFunction, MirCallArgument, MirConstant,
        MirLoweringLimits, MirSliceBounds, MirStatement, MirTerminator, lower_to_mir,
    };
    use crate::package::PackageGraph;
    use crate::resolve::{ResolvedProgram, SymbolKind, resolve};
    use crate::source::{LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput};
    use crate::syntax::{LexMode, ParseLimits, ParseMode, lex, parse};
    use crate::types::TypeInterner;

    fn checked_mir(source: &str) -> (ResolvedProgram, HirProgram, MirProgram) {
        let mut sources = SourceDatabase::new();
        let file = sources
            .add(SourceInput::virtual_file(
                SourceId::new("root:mir-verifier").unwrap(),
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
        assert!(
            parsed.diagnostics().is_empty(),
            "{:#?}",
            parsed.diagnostics()
        );
        let packages = PackageGraph::loose(&sources, file).unwrap();
        let (resolved, diagnostics) = resolve(&packages, &sources, [(file, &parsed)], 100)
            .unwrap()
            .into_parts();
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
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
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
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
        let mir = lower_to_mir(&resolved, &hir, MirLoweringLimits::default()).unwrap();
        (resolved, hir, mir)
    }

    fn callable_named(resolved: &ResolvedProgram, name: &str) -> HirCallableId {
        HirCallableId::Symbol(
            resolved
                .symbols()
                .find(|symbol| {
                    symbol.kind() == SymbolKind::Function && symbol.name().as_str() == name
                })
                .unwrap_or_else(|| panic!("fixture has no function named {name}"))
                .id(),
        )
    }

    fn member_named(resolved: &ResolvedProgram, name: &str) -> crate::resolve::MemberId {
        resolved
            .members()
            .find(|member| member.name().as_str() == name)
            .unwrap_or_else(|| panic!("fixture has no member named {name}"))
            .id()
    }

    fn corrupted_mir(
        source: &str,
        mutate: impl FnOnce(&ResolvedProgram, &HirProgram, &mut MirProgram),
    ) -> MirInvariantError {
        let (resolved, hir, mut mir) = checked_mir(source);
        verify_mir(&resolved, &hir, &mir).unwrap();
        mutate(&resolved, &hir, &mut mir);
        verify_mir(&resolved, &hir, &mir).unwrap_err()
    }

    const SELECT_MIR_SOURCE: &str = "fn ready(): Int selectable { 1 }\n\
         fn run(): Int suspends {\n\
             let selected = select {\n\
                 ready() => 1\n\
                 else => 0\n\
             }\n\
             selected\n\
         }\n";

    fn select_run_function_mut(program: &mut MirProgram) -> &mut crate::mir::MirFunction {
        program
            .functions_mut_for_tests()
            .values_mut()
            .find(|function| {
                let blocks: Vec<_> = function.blocks().collect();
                blocks.iter().any(|block| {
                    block.statements().iter().any(|statement| {
                        matches!(statement.kind(), MirStatementKind::BeginSelect { .. })
                    })
                })
            })
            .expect("the select source lowers to a region")
    }

    fn region_block(function: &crate::mir::MirFunction) -> usize {
        let blocks: Vec<_> = function.blocks().collect();
        blocks
            .iter()
            .position(|block| {
                block.statements().iter().any(|statement| {
                    matches!(statement.kind(), MirStatementKind::BeginSelect { .. })
                })
            })
            .expect("the region block exists")
    }

    fn assert_select_flow_error(source_mutation: impl FnOnce(&mut MirProgram), needle: &str) {
        let (resolved, hir, mut mir) = checked_mir(SELECT_MIR_SOURCE);
        source_mutation(&mut mir);
        let error =
            verify_mir(&resolved, &hir, &mir).expect_err("the forged selection must be rejected");
        let message = error.to_string();
        assert!(
            message.contains(needle),
            "expected `{needle}` in: {message}"
        );
    }

    #[test]
    fn select_verifier_rejects_registration_without_a_region() {
        assert_select_flow_error(
            |program| {
                let function = select_run_function_mut(program);
                let index = region_block(function);
                function.blocks_mut_for_tests()[index]
                    .statements_mut_for_tests()
                    .retain(|statement| {
                        !matches!(statement.kind(), MirStatementKind::BeginSelect { .. })
                    });
            },
            "outside a selection region",
        );
    }

    #[test]
    fn select_verifier_rejects_duplicate_arm_phases() {
        assert_select_flow_error(
            |program| {
                let function = select_run_function_mut(program);
                let index = region_block(function);
                let statements = function.blocks_mut_for_tests()[index].statements_mut_for_tests();
                let position = statements
                    .iter()
                    .position(|statement| {
                        matches!(statement.kind(), MirStatementKind::RegisterSelectArm { .. })
                    })
                    .expect("a registration exists");
                let duplicate = statements[position].clone();
                statements.insert(position + 1, duplicate);
            },
            "skips or duplicates",
        );
    }

    #[test]
    fn select_verifier_rejects_interleaved_statements_before_commit() {
        assert_select_flow_error(
            |program| {
                let function = select_run_function_mut(program);
                let index = region_block(function);
                let locals: Vec<_> = function.locals().cloned().collect();
                assert!(locals.len() > 1, "fixture has temporaries");
                let existing_span = function
                    .blocks()
                    .flat_map(|block| block.statements())
                    .map(|statement| statement.span())
                    .next()
                    .expect("the fixture has statements");
                let smuggled = MirStatement {
                    span: existing_span,
                    kind: MirStatementKind::StorageLive(crate::mir::MirLocalId(
                        (locals.len() - 1) as u32,
                    )),
                };
                let statements = function.blocks_mut_for_tests()[index].statements_mut_for_tests();
                let position = statements
                    .iter()
                    .position(|statement| {
                        matches!(statement.kind(), MirStatementKind::RegisterSelectArm { .. })
                    })
                    .expect("a registration exists");
                statements.insert(position + 1, smuggled);
            },
            "only arm registration may appear",
        );
    }

    #[test]
    fn select_verifier_rejects_an_orphan_commit() {
        assert_select_flow_error(
            |program| {
                let function = select_run_function_mut(program);
                let index = region_block(function);
                function.blocks_mut_for_tests()[index]
                    .statements_mut_for_tests()
                    .retain(|statement| {
                        !matches!(
                            statement.kind(),
                            MirStatementKind::BeginSelect { .. }
                                | MirStatementKind::RegisterSelectArm { .. }
                        )
                    });
            },
            "no open selection region",
        );
    }

    #[test]
    fn select_verifier_rejects_region_reentry_before_commit() {
        assert_select_flow_error(
            |program| {
                let function = select_run_function_mut(program);
                let index = region_block(function);
                let statements = function.blocks_mut_for_tests()[index].statements_mut_for_tests();
                let begin = statements
                    .iter()
                    .position(|statement| {
                        matches!(statement.kind(), MirStatementKind::BeginSelect { .. })
                    })
                    .expect("begin exists");
                let reopened = statements[begin].clone();
                statements.insert(begin + 1, reopened);
            },
            "re-entered before its commit",
        );
    }

    #[test]
    fn select_verifier_rejects_unbounded_regions() {
        assert_select_flow_error(
            |program| {
                let function = select_run_function_mut(program);
                let index = region_block(function);
                let statements = function.blocks_mut_for_tests()[index].statements_mut_for_tests();
                let position = statements
                    .iter()
                    .position(|statement| {
                        matches!(statement.kind(), MirStatementKind::BeginSelect { .. })
                    })
                    .expect("begin exists");
                statements[position] = MirStatement {
                    span: statements[position].span,
                    kind: MirStatementKind::BeginSelect { capacity: 0 },
                };
            },
            "outside the checked bound",
        );
    }

    #[test]
    fn select_verifier_rejects_non_join_handles() {
        let (resolved, hir, mut mir) = checked_mir(SELECT_MIR_SOURCE);
        {
            let (span, position, outcome, return_local) = {
                let function = select_run_function_mut(&mut mir);
                let index = region_block(function);
                let statements = function.blocks_mut_for_tests()[index].statements_mut_for_tests();
                let position = statements
                    .iter()
                    .position(|statement| {
                        matches!(statement.kind(), MirStatementKind::RegisterSelectArm { .. })
                    })
                    .expect("a registration exists");
                (
                    statements[position].span,
                    position,
                    function.outcome(),
                    function.return_local(),
                )
            };
            let function = select_run_function_mut(&mut mir);
            let index = region_block(function);
            let statements = function.blocks_mut_for_tests()[index].statements_mut_for_tests();
            statements[position] = MirStatement {
                span,
                kind: MirStatementKind::RegisterSelectArm {
                    index: 0,
                    registration: MirSelectRegistration::Join(crate::mir::MirPlace {
                        local: return_local,
                        ty: outcome,
                        projections: Vec::new(),
                        source_loan: None,
                    }),
                },
            };
        }
        let error =
            verify_mir(&resolved, &hir, &mir).expect_err("a non-Join handle must be rejected");
        assert!(format!("{error}").contains("non-Join handle"), "{error}");
    }

    #[test]
    fn select_verifier_rejects_commit_table_mismatches() {
        assert_select_flow_error(
            |program| {
                let function = select_run_function_mut(program);
                let index = region_block(function);
                let statements = function.blocks_mut_for_tests()[index].statements_mut_for_tests();
                let position = statements
                    .iter()
                    .position(|statement| {
                        matches!(
                            statement.kind(),
                            MirStatementKind::RegisterSelectArm { index: 0, .. }
                        )
                    })
                    .expect("arm 0 registration exists");
                statements.remove(position);
            },
            "does not match its registered arms",
        );
    }

    #[test]
    fn select_verifier_rejects_a_payload_type_mismatch() {
        let (resolved, hir, mut mir) = checked_mir(SELECT_MIR_SOURCE);
        let wrong_type = hir.interner().scalar(ScalarType::String);
        let (block_index, terminator) = {
            let function = select_run_function_mut(&mut mir);
            function
                .blocks()
                .enumerate()
                .find_map(|(index, block)| {
                    matches!(
                        block.terminator().kind(),
                        MirTerminatorKind::CommitSelect { .. }
                    )
                    .then(|| (index, block.terminator().clone()))
                })
                .expect("the fixture contains a select commit")
        };
        let MirTerminatorKind::CommitSelect {
            mut arms,
            else_target,
            unwind,
        } = terminator.kind
        else {
            unreachable!("the fixture contains a select commit");
        };
        let original = arms[0].payload().cloned().expect("the arm binds a payload");
        let target = arms[0].target();
        arms[0] = crate::mir::MirSelectArm::new(
            Some(crate::mir::MirPlace {
                ty: wrong_type,
                ..original
            }),
            target,
        );
        let function = select_run_function_mut(&mut mir);
        function.blocks_mut_for_tests()[block_index].set_terminator_for_tests(MirTerminator {
            span: terminator.span,
            kind: MirTerminatorKind::CommitSelect {
                arms,
                else_target,
                unwind,
            },
        });
        let capabilities = CapabilityAnalysis::new(&hir, &resolved).unwrap();
        let terminal_analysis = TerminalAnalysis::new(&hir, &resolved).unwrap();
        let verifier = Verifier {
            resolved: &resolved,
            hir: &hir,
            capability_analysis: &capabilities,
            terminal_analysis,
            capability_statuses: RefCell::new(BTreeMap::new()),
            terminal_statuses: RefCell::new(BTreeMap::new()),
            limits: MirVerificationLimits::default(),
            dataflow_steps: Cell::new(0),
        };
        let function = select_run_function_mut(&mut mir);
        let error = verifier
            .verify_select_flow(function, "select payload test")
            .expect_err("a mismatched select payload must be rejected");
        assert!(error.message().contains("payload type"), "{error}");
    }

    #[test]
    fn selectable_closures_lower_and_verify_as_weakened_suspending_values() {
        let source = "fn expose(): fn(): Int suspends {\n\
             let operation: fn(): Int suspends = (): Int selectable { 1 }\n\
             operation\n\
         }\n";
        let (resolved, hir, mir) = checked_mir(source);
        verify_mir(&resolved, &hir, &mir).unwrap();
    }

    #[test]
    fn selectable_closures_lower_through_call_once_erasure() {
        let source = "fn target(): Int suspends { 1 }\n\
             fn build(input: Int) {\n\
                 let operation = (): Int selectable { input }\n\
                 _ = operation\n\
             }\n";
        let (resolved, hir, mir) = checked_mir(source);
        verify_mir(&resolved, &hir, &mir).unwrap();
        let actual = hir
            .expressions()
            .find_map(|expression| match expression.kind() {
                crate::hir::HirExpressionKind::Closure(_) => Some(expression.ty()),
                _ => None,
            })
            .expect("the fixture contains a selectable closure");
        let expected = hir
            .callables()
            .map(|callable| callable.function_type())
            .find(|ty| {
                matches!(
                    hir.interner().kind(*ty),
                    Ok(TypeKind::Function(function))
                        if function.is_async()
                            && !function.is_selectable()
                            && function.outcome() == hir.interner().scalar(ScalarType::Int)
                )
            })
            .expect("the fixture contains a suspending target signature");
        let capabilities = CapabilityAnalysis::new(&hir, &resolved).unwrap();
        let terminal_analysis = TerminalAnalysis::new(&hir, &resolved).unwrap();
        let verifier = Verifier {
            resolved: &resolved,
            hir: &hir,
            capability_analysis: &capabilities,
            terminal_analysis,
            capability_statuses: RefCell::new(BTreeMap::new()),
            terminal_statuses: RefCell::new(BTreeMap::new()),
            limits: MirVerificationLimits::default(),
            dataflow_steps: Cell::new(0),
        };
        assert!(
            verifier
                .callable_once_erasure_matches(actual, expected, "selectable call once")
                .unwrap()
        );
    }

    fn projected_place(kind: MirProjectionKind) -> MirPlace {
        let ty = TypeInterner::default().scalar(ScalarType::Int);
        MirPlace {
            local: MirLocalId(0),
            ty,
            projections: vec![MirProjection { ty, kind }],
            source_loan: None,
        }
    }

    fn aggregate_rvalue_mut(
        function: &mut MirFunction,
        predicate: impl Fn(&MirAggregateKind) -> bool,
    ) -> &mut MirRvalue {
        function
            .blocks
            .iter_mut()
            .flat_map(|block| &mut block.statements)
            .find_map(|statement| match &mut statement.kind {
                MirStatementKind::Assign { value, .. }
                    if matches!(
                        &value.kind,
                        MirRvalueKind::Aggregate { shape, .. } if predicate(shape)
                    ) =>
                {
                    Some(value)
                }
                _ => None,
            })
            .expect("fixture contains the requested aggregate")
    }

    fn rvalue_mut(
        function: &mut MirFunction,
        predicate: impl Fn(&MirRvalueKind) -> bool,
    ) -> &mut MirRvalue {
        function
            .blocks
            .iter_mut()
            .flat_map(|block| &mut block.statements)
            .find_map(|statement| match &mut statement.kind {
                MirStatementKind::Assign { value, .. } if predicate(&value.kind) => Some(value),
                _ => None,
            })
            .expect("fixture contains the requested rvalue")
    }

    fn operation_mut(
        function: &mut MirFunction,
        predicate: impl Fn(&MirOperationKind) -> bool,
    ) -> &mut MirOperation {
        function
            .blocks
            .iter_mut()
            .find_map(|block| match &mut block.terminator.kind {
                MirTerminatorKind::Invoke { operation, .. }
                | MirTerminatorKind::Spawn { operation, .. }
                    if predicate(&operation.kind) =>
                {
                    Some(operation)
                }
                MirTerminatorKind::Await {
                    awaitable: MirAwaitable::Call(operation),
                    ..
                } if predicate(&operation.kind) => Some(operation),
                _ => None,
            })
            .expect("fixture contains the requested MIR operation")
    }

    #[test]
    fn structural_reborrows_require_a_complete_strict_subplace() {
        assert!(place_is_structurally_replaceable(&projected_place(
            MirProjectionKind::TupleField(0)
        )));
        assert!(place_is_structurally_replaceable(&projected_place(
            MirProjectionKind::Index {
                index: MirLocalId(1),
                access: crate::hir::HirIndexAccess::Array,
            }
        )));
        assert!(!place_is_structurally_replaceable(&projected_place(
            MirProjectionKind::Slice {
                start: None,
                end: None,
                step: None,
            }
        )));
        assert!(!place_is_structurally_replaceable(&projected_place(
            MirProjectionKind::ArrayPatternRest {
                start: 0,
                suffix: 0,
            }
        )));
        assert!(!place_is_structurally_replaceable(&projected_place(
            MirProjectionKind::Index {
                index: MirLocalId(1),
                access: crate::hir::HirIndexAccess::MapEntry,
            }
        )));
    }

    #[test]
    fn invariant_errors_and_verification_limits_remain_observable() {
        let ordinary = MirInvariantError::new("function", "broken contract");
        assert_eq!(ordinary.context(), "function");
        assert_eq!(ordinary.message(), "broken contract");
        assert!(!ordinary.is_resource_limit());
        assert_eq!(
            ordinary.to_string(),
            "MIR invariant failed for function: broken contract"
        );

        let limited = MirInvariantError::resource_limit("dataflow", "budget exhausted");
        assert_eq!(limited.context(), "dataflow");
        assert_eq!(limited.message(), "budget exhausted");
        assert!(limited.is_resource_limit());

        let (resolved, hir, mir) = checked_mir("fn identity(value: Int): Int { value }\n");
        let error = verify_mir_with_limits(
            &resolved,
            &hir,
            &mir,
            MirVerificationLimits {
                max_dataflow_steps: 0,
            },
        )
        .unwrap_err();
        assert!(error.is_resource_limit());
    }

    #[test]
    fn collection_loan_paths_rederive_static_disjunction() {
        let split = MirLocalId(1);
        let dynamic = MirLocalId(2);
        let static_integers = BTreeMap::from([(split, 2)]);
        let left = vec![MovePathComponent::Slice {
            start: None,
            end: Some(split),
            step: None,
        }];
        let right = vec![MovePathComponent::Slice {
            start: Some(split),
            end: None,
            step: None,
        }];
        assert!(!loan_paths_overlap(&left, &right, &static_integers));
        assert!(loan_paths_overlap(
            &left,
            &[MovePathComponent::Slice {
                start: None,
                end: None,
                step: None,
            }],
            &static_integers,
        ));
        assert!(loan_paths_overlap(
            &[MovePathComponent::Index {
                index: dynamic,
                access: HirIndexAccess::Array,
            }],
            &[MovePathComponent::Index {
                index: split,
                access: HirIndexAccess::Array,
            }],
            &static_integers,
        ));
        assert!(!loan_paths_overlap(
            &[MovePathComponent::ArrayPatternRest {
                start: 1,
                suffix: 0,
            }],
            &[MovePathComponent::ArrayPatternIndex(0)],
            &static_integers,
        ));
    }

    #[test]
    fn move_path_relations_cover_fixed_dynamic_and_runtime_regions() {
        let static_index = MirLocalId(1);
        let dynamic_index = MirLocalId(2);
        let slice_start = MirLocalId(3);
        let slice_end = MirLocalId(4);
        let slice_step = MirLocalId(5);
        let static_integers = BTreeMap::from([
            (static_index, 2),
            (slice_start, 1),
            (slice_end, 7),
            (slice_step, 2),
        ]);

        let fixed_cases = [
            (
                MovePathComponent::TupleField(0),
                MovePathComponent::TupleField(1),
            ),
            (
                MovePathComponent::OptionValue,
                MovePathComponent::ResultOkValue,
            ),
            (
                MovePathComponent::ResultOkValue,
                MovePathComponent::ResultErrValue,
            ),
            (
                MovePathComponent::ArrayPatternIndex(0),
                MovePathComponent::ArrayPatternIndex(1),
            ),
        ];
        for (left, right) in fixed_cases {
            assert_eq!(
                loan_paths_relation(&[left], &[right], &static_integers),
                StaticRegionRelation::Disjoint
            );
        }

        assert_eq!(
            loan_paths_relation(
                &[MovePathComponent::Index {
                    index: static_index,
                    access: HirIndexAccess::Array,
                }],
                &[MovePathComponent::ArrayPatternIndex(2)],
                &static_integers,
            ),
            StaticRegionRelation::Overlap
        );
        assert_eq!(
            loan_paths_relation(
                &[MovePathComponent::Index {
                    index: static_index,
                    access: HirIndexAccess::Array,
                }],
                &[MovePathComponent::ArrayPatternIndex(3)],
                &static_integers,
            ),
            StaticRegionRelation::Disjoint
        );
        assert_eq!(
            loan_paths_relation(
                &[MovePathComponent::Slice {
                    start: Some(slice_start),
                    end: Some(slice_end),
                    step: Some(slice_step),
                }],
                &[MovePathComponent::ArrayPatternIndex(3)],
                &static_integers,
            ),
            StaticRegionRelation::Overlap
        );
        assert_eq!(
            loan_paths_relation(
                &[MovePathComponent::Index {
                    index: dynamic_index,
                    access: HirIndexAccess::Array,
                }],
                &[MovePathComponent::TupleField(0)],
                &static_integers,
            ),
            StaticRegionRelation::Runtime
        );
        assert_eq!(
            loan_paths_relation(
                &[MovePathComponent::Index {
                    index: static_index,
                    access: HirIndexAccess::String,
                }],
                &[MovePathComponent::Index {
                    index: static_index,
                    access: HirIndexAccess::MapLookup,
                }],
                &static_integers,
            ),
            StaticRegionRelation::Runtime
        );

        let iterator_index = MirLocalId(6);
        assert_eq!(
            loan_paths_relation(
                &[MovePathComponent::IteratorElement {
                    index: iterator_index,
                }],
                &[MovePathComponent::IteratorElement {
                    index: iterator_index,
                }],
                &static_integers,
            ),
            StaticRegionRelation::Overlap
        );
        assert_eq!(
            move_path_runtime_inputs(&[
                MovePathComponent::Index {
                    index: dynamic_index,
                    access: HirIndexAccess::Array,
                },
                MovePathComponent::IteratorElement {
                    index: iterator_index,
                },
                MovePathComponent::Slice {
                    start: Some(slice_start),
                    end: Some(slice_end),
                    step: Some(slice_step),
                },
                MovePathComponent::TupleField(0),
            ])
            .collect::<Vec<_>>(),
            [
                dynamic_index,
                iterator_index,
                slice_start,
                slice_end,
                slice_step,
            ]
        );
    }

    #[test]
    fn local_flow_helpers_preserve_partial_move_and_defer_invariants() {
        let local = MirLocalId(7);
        let field_zero = vec![MovePathComponent::TupleField(0)];
        let field_one = vec![MovePathComponent::TupleField(1)];
        let nested = vec![
            MovePathComponent::TupleField(0),
            MovePathComponent::TupleField(1),
        ];

        let mut unavailable = BTreeSet::new();
        move_path_unchecked(&mut unavailable, nested.clone());
        move_path_unchecked(&mut unavailable, field_zero.clone());
        assert_eq!(unavailable, BTreeSet::from([field_zero.clone()]));
        assert!(!path_is_available(&unavailable, &nested));
        assert!(path_is_available(&unavailable, &field_one));
        assert!(!path_parent_is_available(&unavailable, &nested));
        assert!(path_parent_is_available(&unavailable, &field_zero));
        write_path_unchecked(&mut unavailable, &field_zero);
        assert!(unavailable.is_empty());

        move_path_unchecked(&mut unavailable, Vec::new());
        assert_eq!(unavailable, BTreeSet::from([Vec::new()]));
        let live = transfer_local(
            LocalState {
                live: false,
                unavailable: BTreeSet::new(),
            },
            &[LocalEvent::StorageLive(local)],
            local,
        );
        assert!(live.live);
        assert_eq!(live.unavailable, BTreeSet::from([Vec::new()]));
        let initialized = transfer_local(
            live,
            &[LocalEvent::Write(LocalAccess {
                local,
                path: Vec::new(),
                source_loan: None,
            })],
            local,
        );
        assert!(initialized.unavailable.is_empty());
        let moved = transfer_local(
            initialized,
            &[LocalEvent::Move(LocalAccess {
                local,
                path: field_zero.clone(),
                source_loan: None,
            })],
            local,
        );
        assert_eq!(moved.unavailable, BTreeSet::from([field_zero.clone()]));
        let dead = transfer_local(moved, &[LocalEvent::StorageDead(local)], local);
        assert!(!dead.live);

        let whole = LocalAccess {
            local,
            path: Vec::new(),
            source_loan: None,
        };
        let partial = LocalAccess {
            local,
            path: field_zero.clone(),
            source_loan: None,
        };
        let sibling = LocalAccess {
            local,
            path: field_one,
            source_loan: None,
        };
        assert!(local_accesses_overlap(&whole, &partial));
        assert!(!local_accesses_overlap(&partial, &sibling));
        assert!(local_access_contains(&whole, &partial));
        assert!(!local_access_contains(&partial, &whole));

        for event in [
            LocalEvent::Read(partial.clone()),
            LocalEvent::Resolve(partial.clone()),
            LocalEvent::Move(partial.clone()),
        ] {
            let mut state = DeferFlowState {
                consumed: BTreeSet::from([whole.clone()]),
                ..DeferFlowState::default()
            };
            let error = apply_consumed_defer_events(&mut state, &[event], "test").unwrap_err();
            assert!(
                error
                    .message()
                    .contains("owner already consumed by a deferred action")
            );
        }

        let mut state = DeferFlowState {
            consumed: BTreeSet::from([whole.clone()]),
            ..DeferFlowState::default()
        };
        let error = apply_consumed_defer_events(
            &mut state,
            &[LocalEvent::WriteAccess(partial.clone())],
            "test",
        )
        .unwrap_err();
        assert!(error.message().contains("partial write"));

        let mut state = DeferFlowState {
            consumed: BTreeSet::from([whole.clone()]),
            ..DeferFlowState::default()
        };
        let error = apply_consumed_defer_events(&mut state, &[LocalEvent::Write(partial)], "test")
            .unwrap_err();
        assert!(error.message().contains("partially reinitializes"));

        let mut state = DeferFlowState {
            consumed: BTreeSet::from([whole]),
            ..DeferFlowState::default()
        };
        apply_consumed_defer_events(
            &mut state,
            &[LocalEvent::Write(LocalAccess {
                local,
                path: Vec::new(),
                source_loan: None,
            })],
            "test",
        )
        .unwrap();
        assert!(state.consumed.is_empty());
    }

    #[test]
    fn defer_flow_state_rejects_invalid_nesting_and_drains_explicit_entries() {
        let (_, hir, _) = checked_mir(
            "fn note(value: Int) {}\n\
             fn main() {\n\
                 defer note(1)\n\
                 {\n\
                     defer note(2)\n\
                 }\n\
             }\n",
        );
        let mut scopes = hir
            .expressions()
            .filter_map(|expression| match expression.kind() {
                crate::hir::HirExpressionKind::Block { scope, .. } => Some(*scope),
                _ => None,
            })
            .collect::<Vec<_>>();
        scopes.sort();
        scopes.dedup();
        assert!(
            scopes.len() >= 2,
            "the fixture retains nested cleanup scopes"
        );
        let outer = scopes[0];
        let inner = scopes[1];

        let place = LocalAccess {
            local: MirLocalId(0),
            path: Vec::new(),
            source_loan: None,
        };
        let mut active = DeferFlowState::default();
        active.activate_scope(outer, "test").unwrap();
        active.unguarded_scopes.insert(outer);
        active.activate_scope(inner, "test").unwrap();
        active.unguarded_scopes.insert(inner);
        let error = active.activate_scope(outer, "test").unwrap_err();
        assert!(error.message().contains("re-enters an outer scope"));
        active.unguarded_scopes.clear();
        active.remove_inactive_scope(outer);
        assert_eq!(active.scope_order, vec![inner]);

        let mut skipped = DeferFlowState {
            scope_order: vec![outer, inner],
            ..DeferFlowState::default()
        };
        let error = skipped.drain(&[outer], "test").unwrap_err();
        assert!(error.message().contains("skips a still-active inner scope"));

        let mut drained = DeferFlowState {
            unguarded_scopes: BTreeSet::from([outer, inner]),
            scope_order: vec![outer, inner],
            guards: BTreeMap::from([(
                place.clone(),
                ActiveDeferGuard {
                    scope: outer,
                    registration: (MirBlockId(0), 0),
                    kind: CleanupEntryKind::Explicit,
                },
            )]),
            registrations: BTreeMap::from([(
                (MirBlockId(0), 0),
                ActiveCleanupRegistration {
                    scope: outer,
                    kind: CleanupEntryKind::Explicit,
                },
            )]),
            ..DeferFlowState::default()
        };
        drained.drain(&[outer, inner], "test").unwrap();
        assert!(drained.consumed.contains(&place));
        assert!(drained.is_empty());

        let mut unfinished = DeferFlowState {
            pending_moves: BTreeMap::from([(place, PendingDeferTransition::Retarget)]),
            ..DeferFlowState::default()
        };
        let error = unfinished.finish_normal("test").unwrap_err();
        assert!(error.message().contains("abandons an explicit defer entry"));
        unfinished.pending_moves.clear();
        unfinished.finish_normal("test").unwrap();
        unfinished.drain_unwind();
        assert!(unfinished.is_empty());
    }

    #[test]
    fn scalar_and_unavailable_messages_cover_the_closed_type_catalog() {
        for scalar in [
            ScalarType::Int,
            ScalarType::Int8,
            ScalarType::Int16,
            ScalarType::Int32,
            ScalarType::UInt8,
            ScalarType::UInt16,
            ScalarType::UInt32,
            ScalarType::UInt64,
        ] {
            assert!(is_integer(scalar), "{scalar:?}");
            assert!(is_arithmetic(scalar), "{scalar:?}");
            assert!(is_relational(scalar), "{scalar:?}");
        }
        for scalar in [
            ScalarType::Int,
            ScalarType::Int8,
            ScalarType::Int16,
            ScalarType::Int32,
        ] {
            assert!(is_signed_integer(scalar), "{scalar:?}");
        }
        for scalar in [ScalarType::Float, ScalarType::Float32] {
            assert!(is_float(scalar), "{scalar:?}");
            assert!(is_arithmetic(scalar), "{scalar:?}");
            assert!(is_relational(scalar), "{scalar:?}");
        }
        for scalar in [ScalarType::Byte, ScalarType::Char, ScalarType::String] {
            assert!(is_relational(scalar), "{scalar:?}");
            assert!(!is_arithmetic(scalar), "{scalar:?}");
        }
        for scalar in [ScalarType::Bool, ScalarType::Unit, ScalarType::Never] {
            assert!(!is_integer(scalar), "{scalar:?}");
            assert!(!is_float(scalar), "{scalar:?}");
            assert!(!is_relational(scalar), "{scalar:?}");
        }

        let local = MirLocalId(9);
        assert_eq!(
            unavailable_read_message(local, &[]),
            "reads local#9 before a dominating live definition"
        );
        assert_eq!(
            unavailable_read_message(local, &[MovePathComponent::TupleField(0)]),
            "reads an unavailable move path of local#9"
        );
        assert_eq!(
            unavailable_move_message(local, &[]),
            "moves local#9 after its value became unavailable"
        );
        assert_eq!(
            unavailable_move_message(local, &[MovePathComponent::TupleField(0)]),
            "moves an unavailable move path of local#9"
        );
    }

    #[test]
    fn event_extractors_cover_every_closed_rvalue_operation_and_projection_shape() {
        let (resolved, hir, mir) = checked_mir(
            "type Record = { field: Int }\n\
             enum Choice { Item(Int) }\n\
             fn fixture(value: Int): Int {\n\
                 let closure = () { value }\n\
                 _ = closure\n\
                 value\n\
             }\n",
        );
        let fixture_id = MirFunctionId::Callable(callable_named(&resolved, "fixture"));
        let function = mir.functions.get(&fixture_id).unwrap();
        let integer = hir.interner().scalar(ScalarType::Int);
        let boolean = hir.interner().scalar(ScalarType::Bool);
        let parameter = function.parameters[0];
        let field = member_named(&resolved, "field");
        let variant = member_named(&resolved, "Item");
        let closure = mir
            .functions
            .keys()
            .find_map(|id| match id {
                MirFunctionId::Closure(id) => Some(*id),
                MirFunctionId::Callable(_) => None,
            })
            .expect("fixture lowers one closure");

        let place = MirPlace {
            local: parameter,
            ty: integer,
            projections: Vec::new(),
            source_loan: None,
        };
        let copy = MirOperand {
            ty: integer,
            kind: MirOperandKind::Copy(place.clone()),
        };
        let moved = MirOperand {
            ty: integer,
            kind: MirOperandKind::Move(place.clone()),
        };
        let borrowed = MirOperand {
            ty: integer,
            kind: MirOperandKind::Borrow(place.clone()),
        };
        let loaned = MirOperand {
            ty: integer,
            kind: MirOperandKind::Loan(MirLoanId(0)),
        };
        let constant = MirOperand {
            ty: integer,
            kind: MirOperandKind::Constant(MirConstant::Integer("1".into())),
        };
        let callable = MirOperand {
            ty: integer,
            kind: MirOperandKind::Function {
                callable: callable_named(&resolved, "fixture"),
                arguments: Vec::new(),
            },
        };
        let prelude = MirOperand {
            ty: integer,
            kind: MirOperandKind::PreludeTraitFunction {
                method: crate::hir::HirPreludeTraitMethod::Display,
                arguments: vec![integer],
            },
        };

        for (operand, expected_events) in [
            (&moved, 1),
            (&copy, 1),
            (&borrowed, 1),
            (&constant, 0),
            (&loaned, 0),
            (&callable, 0),
            (&prelude, 0),
        ] {
            let mut events = Vec::new();
            push_operand_events(operand, &mut events);
            assert_eq!(events.len(), expected_events);
        }

        let rvalues = vec![
            MirRvalue {
                ty: integer,
                kind: MirRvalueKind::Use(borrowed.clone()),
            },
            MirRvalue {
                ty: integer,
                kind: MirRvalueKind::Prefix {
                    operator: HirPrefixOperator::Negate,
                    operand: borrowed.clone(),
                },
            },
            MirRvalue {
                ty: integer,
                kind: MirRvalueKind::Binary {
                    operator: HirBinaryOperator::Equal,
                    left: loaned.clone(),
                    right: copy.clone(),
                },
            },
            MirRvalue {
                ty: integer,
                kind: MirRvalueKind::Aggregate {
                    shape: MirAggregateKind::Tuple,
                    values: vec![borrowed.clone()],
                },
            },
            MirRvalue {
                ty: integer,
                kind: MirRvalueKind::RecordUpdate {
                    base: borrowed.clone(),
                    fields: vec![(field, copy.clone())],
                },
            },
            MirRvalue {
                ty: integer,
                kind: MirRvalueKind::Coerce {
                    kind: Assignability::Exact,
                    value: borrowed.clone(),
                },
            },
            MirRvalue {
                ty: integer,
                kind: MirRvalueKind::NumericConversion {
                    target: ScalarType::Int,
                    conversion: NumericConversion::Identity,
                    value: borrowed.clone(),
                },
            },
            MirRvalue {
                ty: integer,
                kind: MirRvalueKind::Range {
                    kind: HirRangeKind::Exclusive,
                    start: borrowed.clone(),
                    end: copy.clone(),
                },
            },
            MirRvalue {
                ty: boolean,
                kind: MirRvalueKind::Contains {
                    kind: HirContainmentKind::Array,
                    item: loaned.clone(),
                    container: copy.clone(),
                },
            },
            MirRvalue {
                ty: integer,
                kind: MirRvalueKind::MapRemove {
                    map: place.clone(),
                    key: borrowed.clone(),
                },
            },
            MirRvalue {
                ty: integer,
                kind: MirRvalueKind::Interpolate {
                    segments: vec!["".into(), "".into()],
                    values: vec![borrowed.clone()],
                },
            },
            MirRvalue {
                ty: integer,
                kind: MirRvalueKind::Length(loaned.clone()),
            },
            MirRvalue {
                ty: integer,
                kind: MirRvalueKind::IteratorState {
                    source: loaned.clone(),
                },
            },
        ];
        let mut rvalue_events = 0;
        for value in &rvalues {
            assert!(mir_rvalue_contains_invalid_borrow(value));
            let mut local = Vec::new();
            push_rvalue_events(value, &mut local);
            rvalue_events += local.len();
            let mut tags = Vec::new();
            push_tag_rvalue(function, value, &mut tags);
        }
        assert_eq!(rvalue_events, 14);

        let bounds = || MirSliceBounds {
            start: Some(copy.clone()),
            end: Some(copy.clone()),
            step: Some(copy.clone()),
        };
        let operations = vec![
            (
                MirOperation {
                    ty: integer,
                    kind: MirOperationKind::CheckedPrefix {
                        operator: HirPrefixOperator::Negate,
                        operand: borrowed.clone(),
                    },
                },
                1,
            ),
            (
                MirOperation {
                    ty: integer,
                    kind: MirOperationKind::CheckedBinary {
                        operator: HirBinaryOperator::Add,
                        left: borrowed.clone(),
                        right: copy.clone(),
                    },
                },
                2,
            ),
            (
                MirOperation {
                    ty: integer,
                    kind: MirOperationKind::ArraySequence {
                        kind: crate::hir::HirArraySequenceKind::Concat,
                        array: loaned.clone(),
                        argument: borrowed.clone(),
                    },
                },
                2,
            ),
            (
                MirOperation {
                    ty: integer,
                    kind: MirOperationKind::BuildMap {
                        entries: vec![(borrowed.clone(), copy.clone())],
                        reject_dynamic_duplicates: true,
                    },
                },
                2,
            ),
            (
                MirOperation {
                    ty: integer,
                    kind: MirOperationKind::Index {
                        base: loaned.clone(),
                        index: borrowed.clone(),
                        access: HirIndexAccess::Array,
                        against: Vec::new(),
                    },
                },
                2,
            ),
            (
                MirOperation {
                    ty: integer,
                    kind: MirOperationKind::Slice {
                        base: loaned.clone(),
                        bounds: Box::new(bounds()),
                        against: Vec::new(),
                    },
                },
                4,
            ),
            (
                MirOperation {
                    ty: integer,
                    kind: MirOperationKind::Call {
                        callee: loaned.clone(),
                        arguments: vec![
                            MirCallArgument {
                                mode: ParameterMode::Value,
                                target: HirCallArgumentTarget::Fixed(0),
                                value: borrowed.clone(),
                            },
                            MirCallArgument {
                                mode: ParameterMode::Ref,
                                target: HirCallArgumentTarget::Fixed(1),
                                value: copy.clone(),
                            },
                        ],
                        signature: integer,
                        protocol: crate::hir::HirCallProtocol::Call,
                        unsafe_call: false,
                    },
                },
                3,
            ),
            (
                MirOperation {
                    ty: integer,
                    kind: MirOperationKind::ExplicitPanic {
                        message: borrowed.clone(),
                    },
                },
                1,
            ),
            (
                MirOperation {
                    ty: integer,
                    kind: MirOperationKind::Assert {
                        condition: borrowed.clone(),
                        condition_repr: "condition".into(),
                        message_parts: vec![MirAssertMessagePart {
                            value: copy.clone(),
                            spread: false,
                        }],
                    },
                },
                2,
            ),
            (
                MirOperation {
                    ty: integer,
                    kind: MirOperationKind::BootstrapHostCall {
                        function: MirBootstrapHostFunction::ConsolePrint,
                        arguments: vec![borrowed.clone()],
                    },
                },
                1,
            ),
            (
                MirOperation {
                    ty: integer,
                    kind: MirOperationKind::Format {
                        value: borrowed.clone(),
                        display: Some(copy.clone()),
                    },
                },
                2,
            ),
            (
                MirOperation {
                    ty: integer,
                    kind: MirOperationKind::JoinFormat {
                        values: loaned.clone(),
                        separator: borrowed.clone(),
                        display: Some(copy.clone()),
                    },
                },
                3,
            ),
        ];
        for (operation, expected_operands) in &operations {
            assert!(mir_operation_contains_invalid_borrow(operation));
            assert_eq!(operation_operands(operation).len(), *expected_operands);
            let mut local = Vec::new();
            push_operation_events(operation, &mut local);
            assert!(!local.is_empty());
            let mut tags = Vec::new();
            push_tag_operation(function, operation, &mut tags);
        }

        let indexed = MirPlace {
            local: parameter,
            ty: integer,
            projections: vec![
                MirProjection {
                    ty: integer,
                    kind: MirProjectionKind::ClosureCapture { closure, index: 0 },
                },
                MirProjection {
                    ty: integer,
                    kind: MirProjectionKind::IteratorSource,
                },
                MirProjection {
                    ty: integer,
                    kind: MirProjectionKind::Field(field),
                },
                MirProjection {
                    ty: integer,
                    kind: MirProjectionKind::TupleField(0),
                },
                MirProjection {
                    ty: integer,
                    kind: MirProjectionKind::NewtypeValue,
                },
                MirProjection {
                    ty: integer,
                    kind: MirProjectionKind::RefValue,
                },
                MirProjection {
                    ty: integer,
                    kind: MirProjectionKind::VariantTuple { variant, index: 0 },
                },
                MirProjection {
                    ty: integer,
                    kind: MirProjectionKind::VariantField { variant, field },
                },
                MirProjection {
                    ty: integer,
                    kind: MirProjectionKind::OptionValue,
                },
                MirProjection {
                    ty: integer,
                    kind: MirProjectionKind::ResultOkValue,
                },
                MirProjection {
                    ty: integer,
                    kind: MirProjectionKind::ResultErrValue,
                },
                MirProjection {
                    ty: integer,
                    kind: MirProjectionKind::UnionValue(integer),
                },
                MirProjection {
                    ty: integer,
                    kind: MirProjectionKind::ArrayPatternIndex(0),
                },
                MirProjection {
                    ty: integer,
                    kind: MirProjectionKind::ArrayPatternRest {
                        start: 1,
                        suffix: 0,
                    },
                },
                MirProjection {
                    ty: integer,
                    kind: MirProjectionKind::IteratorElement { index: parameter },
                },
                MirProjection {
                    ty: integer,
                    kind: MirProjectionKind::Index {
                        index: parameter,
                        access: HirIndexAccess::Array,
                    },
                },
                MirProjection {
                    ty: integer,
                    kind: MirProjectionKind::Slice {
                        start: Some(parameter),
                        end: Some(parameter),
                        step: Some(parameter),
                    },
                },
            ],
            source_loan: None,
        };
        let mut projection_events = Vec::new();
        push_projection_index_events(&indexed, &mut projection_events);
        assert_eq!(projection_events.len(), 5);
        assert!(place_requires_loan_validation(&indexed));
        assert!(place_contains_ref_value(&indexed));
        let mut tag_events = Vec::new();
        push_tag_place(function, &indexed, true, &mut tag_events);
        assert_eq!(
            tag_events
                .iter()
                .filter(|event| matches!(event, TagEvent::Require(_)))
                .count(),
            6
        );
        assert!(matches!(tag_events.last(), Some(TagEvent::Write(_))));

        let sibling_local = function.return_local;
        let sibling = MirPlace {
            local: sibling_local,
            ty: integer,
            projections: Vec::new(),
            source_loan: None,
        };
        assert!(!places_may_overlap(&place, &sibling));
        assert!(places_may_overlap(&place, &indexed));
        assert!(same_place_path(&place, &place));
        assert!(!same_place_path(&place, &indexed));

        let block = |kind| MirTerminator {
            span: function.span,
            kind,
        };
        let destination = place.clone();
        let operation = operations[0].0.clone();
        let terminators = vec![
            (
                block(MirTerminatorKind::Goto {
                    target: MirBlockId(1),
                }),
                1,
            ),
            (
                block(MirTerminatorKind::SwitchBool {
                    condition: copy.clone(),
                    if_true: MirBlockId(1),
                    if_false: MirBlockId(2),
                }),
                2,
            ),
            (
                block(MirTerminatorKind::SwitchTag {
                    value: copy.clone(),
                    cases: vec![(MirTag::OptionSome, MirBlockId(1))],
                    otherwise: MirBlockId(2),
                }),
                2,
            ),
            (
                block(MirTerminatorKind::Invoke {
                    operation: operation.clone(),
                    destination: Some(destination.clone()),
                    target: Some(MirBlockId(1)),
                    unwind: MirBlockId(2),
                }),
                2,
            ),
            (
                block(MirTerminatorKind::Await {
                    awaitable: MirAwaitable::Call(operation.clone()),
                    destination: destination.clone(),
                    target: MirBlockId(1),
                    unwind: MirBlockId(2),
                }),
                2,
            ),
            (
                block(MirTerminatorKind::IteratorNext {
                    state: destination.clone(),
                    destination: destination.clone(),
                    borrowed_source: Some(destination.clone()),
                    exhaustion_guard: Some(destination.clone()),
                    has_value: MirBlockId(1),
                    exhausted: MirBlockId(2),
                    unwind: MirBlockId(3),
                }),
                3,
            ),
            (
                block(MirTerminatorKind::ValidatePlaces {
                    places: vec![destination.clone()],
                    replacements: vec![Some(copy.clone())],
                    against: vec![Vec::new()],
                    for_write: true,
                    target: MirBlockId(1),
                    unwind: MirBlockId(2),
                }),
                2,
            ),
            (
                block(MirTerminatorKind::ValidateLoan {
                    loan: MirLoanId(0),
                    against: Vec::new(),
                    target: MirBlockId(1),
                    unwind: MirBlockId(2),
                }),
                2,
            ),
            (
                block(MirTerminatorKind::DrainDefers {
                    scopes: Vec::new(),
                    target: MirBlockId(1),
                    unwind: MirBlockId(2),
                }),
                2,
            ),
            (
                block(MirTerminatorKind::DrainScopes {
                    task_scopes: Vec::new(),
                    defer_scopes: Vec::new(),
                    target: MirBlockId(1),
                    unwind: MirBlockId(2),
                }),
                2,
            ),
            (
                block(MirTerminatorKind::DrainUnwind {
                    target: MirBlockId(1),
                }),
                1,
            ),
            (block(MirTerminatorKind::Return), 0),
            (block(MirTerminatorKind::ResumePanic), 0),
            (block(MirTerminatorKind::Unreachable), 0),
        ];
        for (terminator, expected_edges) in terminators {
            assert_eq!(successor_edges(&terminator.kind).len(), expected_edges);
        }
    }

    #[test]
    fn projection_corruption_matrix_rejects_every_closed_place_shape() {
        const SOURCE: &str = "type UserId = Int\n\
             type Record = { value: Int, text: String }\n\
             enum Choice {\n\
                 Empty\n\
                 Pair(Int)\n\
                 Named { value: Int }\n\
             }\n\
             fn inspect(\n\
                 scalar: Int,\n\
                 text: String,\n\
                 tuple: (Int, String),\n\
                 identifier: UserId,\n\
                 reference: Ref[Int],\n\
                 record: Record,\n\
                 choice: Choice,\n\
                 optional: Int?,\n\
                 result: Int ! String,\n\
                 either: Int | String,\n\
                 array: Array[Int],\n\
             ) {\n\
                 let offset = 1\n\
                 let closure = (): Int { offset }\n\
                 for ref item in array {\n\
                     _ = item\n\
                 }\n\
                 _ = scalar\n\
                 _ = text\n\
                 _ = tuple\n\
                 _ = identifier\n\
                 _ = reference\n\
                 _ = record\n\
                 _ = choice\n\
                 _ = optional\n\
                 _ = result\n\
                 _ = either\n\
                 _ = closure\n\
             }\n";

        let (resolved, hir, mir) = checked_mir(SOURCE);
        verify_mir(&resolved, &hir, &mir).unwrap();
        let capabilities = CapabilityAnalysis::new(&hir, &resolved).unwrap();
        let terminal_analysis = TerminalAnalysis::new(&hir, &resolved).unwrap();
        let verifier = Verifier {
            resolved: &resolved,
            hir: &hir,
            capability_analysis: &capabilities,
            terminal_analysis,
            capability_statuses: RefCell::new(BTreeMap::new()),
            terminal_statuses: RefCell::new(BTreeMap::new()),
            limits: MirVerificationLimits::default(),
            dataflow_steps: Cell::new(0),
        };
        let inspect_id = MirFunctionId::Callable(callable_named(&resolved, "inspect"));
        let inspect = mir.functions.get(&inspect_id).unwrap();
        let int = hir.interner().scalar(ScalarType::Int);
        let string = hir.interner().scalar(ScalarType::String);
        let boolean = hir.interner().scalar(ScalarType::Bool);

        let nominal_named = |name: &str| {
            let identity = resolved
                .symbols()
                .find(|symbol| symbol.name().as_str() == name)
                .unwrap()
                .identity();
            hir.interner()
                .ids()
                .find(|ty| {
                    matches!(
                        hir.interner().kind(*ty),
                        Ok(TypeKind::Nominal {
                            identity: actual,
                            ..
                        }) if actual == identity
                    )
                })
                .unwrap_or_else(|| panic!("fixture has no nominal type named {name}"))
        };
        let type_matching = |predicate: &dyn Fn(&TypeKind) -> bool| {
            hir.interner()
                .ids()
                .find(|ty| hir.interner().kind(*ty).is_ok_and(predicate))
                .expect("fixture omitted a required projection type")
        };
        let tuple = type_matching(
            &|kind| matches!(kind, TypeKind::Tuple(items) if items.as_slice() == [int, string]),
        );
        let user_id = nominal_named("UserId");
        let record = nominal_named("Record");
        let choice = nominal_named("Choice");
        let reference = type_matching(&|kind| {
            matches!(
                kind,
                TypeKind::Intrinsic {
                    constructor: IntrinsicType::Ref,
                    arguments,
                } if arguments.as_slice() == [int]
            )
        });
        let option = type_matching(&|kind| matches!(kind, TypeKind::Option(item) if *item == int));
        let result = type_matching(&|kind| {
            matches!(
                kind,
                TypeKind::Result { success, error } if *success == int && *error == string
            )
        });
        let union = type_matching(
            &|kind| matches!(kind, TypeKind::Union(members) if members.contains(&int) && members.contains(&string)),
        );
        let array = type_matching(&|kind| {
            matches!(
                kind,
                TypeKind::Intrinsic {
                    constructor: IntrinsicType::Array,
                    arguments,
                } if arguments.as_slice() == [int]
            )
        });
        let cursor = type_matching(
            &|kind| matches!(kind, TypeKind::Cursor { collection, .. } if *collection == array),
        );
        let local_of_type = |ty| {
            inspect
                .locals
                .iter()
                .position(|local| local.ty == ty)
                .map(|index| MirLocalId(index as u32))
                .expect("fixture omitted a local of the requested type")
        };
        let int_local = local_of_type(int);
        let string_local = local_of_type(string);
        let record_field = resolved
            .members()
            .find(|member| {
                member.name().as_str() == "text"
                    && member.kind() == MemberKind::RecordField
                    && matches!(member.owner(), MemberOwner::Type(_))
            })
            .unwrap()
            .id();
        let pair = resolved
            .members()
            .find(|member| {
                member.name().as_str() == "Pair" && member.kind() == MemberKind::EnumVariant
            })
            .unwrap()
            .id();
        let named = resolved
            .members()
            .find(|member| {
                member.name().as_str() == "Named" && member.kind() == MemberKind::EnumVariant
            })
            .unwrap()
            .id();
        let named_field = resolved
            .members()
            .find(|member| member.owner() == MemberOwner::Variant(named))
            .unwrap()
            .id();

        macro_rules! rejects {
            ($function:expr, $current:expr, $declared:expr, $kind:expr, $message:literal) => {{
                let error = verifier
                    .projection_result(
                        $function,
                        $current,
                        &MirProjection {
                            ty: $declared,
                            kind: $kind,
                        },
                        "projection corruption matrix",
                    )
                    .unwrap_err();
                assert!(error.message().contains($message), "{error}");
            }};
        }

        rejects!(
            inspect,
            int,
            int,
            MirProjectionKind::TupleField(0),
            "non-tuple base"
        );
        rejects!(
            inspect,
            tuple,
            int,
            MirProjectionKind::TupleField(9),
            "index is out of range"
        );
        rejects!(
            inspect,
            record,
            string,
            MirProjectionKind::NewtypeValue,
            "non-newtype base"
        );
        rejects!(
            inspect,
            user_id,
            string,
            MirProjectionKind::NewtypeValue,
            "wrong instantiated payload type"
        );
        rejects!(
            inspect,
            reference,
            string,
            MirProjectionKind::RefValue,
            "wrong target type"
        );
        rejects!(
            inspect,
            record,
            int,
            MirProjectionKind::Field(record_field),
            "wrong type"
        );
        rejects!(
            inspect,
            record,
            int,
            MirProjectionKind::VariantTuple {
                variant: pair,
                index: 0,
            },
            "non-enum base"
        );
        rejects!(
            inspect,
            choice,
            int,
            MirProjectionKind::VariantTuple {
                variant: named,
                index: 0,
            },
            "non-tuple variant"
        );
        rejects!(
            inspect,
            choice,
            int,
            MirProjectionKind::VariantTuple {
                variant: pair,
                index: 9,
            },
            "index is out of range"
        );
        rejects!(
            inspect,
            choice,
            string,
            MirProjectionKind::VariantTuple {
                variant: pair,
                index: 0,
            },
            "payload type is inconsistent"
        );
        rejects!(
            inspect,
            record,
            int,
            MirProjectionKind::VariantField {
                variant: named,
                field: named_field,
            },
            "non-enum base"
        );
        rejects!(
            inspect,
            choice,
            int,
            MirProjectionKind::VariantField {
                variant: pair,
                field: named_field,
            },
            "non-record variant"
        );
        rejects!(
            inspect,
            choice,
            int,
            MirProjectionKind::VariantField {
                variant: named,
                field: record_field,
            },
            "wrong owner or member kind"
        );
        rejects!(
            inspect,
            int,
            int,
            MirProjectionKind::OptionValue,
            "non-option base"
        );
        rejects!(
            inspect,
            int,
            int,
            MirProjectionKind::ResultOkValue,
            "non-result base"
        );
        rejects!(
            inspect,
            int,
            string,
            MirProjectionKind::ResultErrValue,
            "non-result base"
        );
        rejects!(
            inspect,
            int,
            int,
            MirProjectionKind::UnionValue(int),
            "non-union base"
        );
        rejects!(
            inspect,
            union,
            boolean,
            MirProjectionKind::UnionValue(boolean),
            "member is absent"
        );
        rejects!(
            inspect,
            array,
            array,
            MirProjectionKind::ArrayPatternRest {
                start: u32::MAX,
                suffix: 1,
            },
            "offsets overflow"
        );
        rejects!(
            inspect,
            array,
            int,
            MirProjectionKind::IteratorElement {
                index: string_local,
            },
            "position is not Int"
        );
        rejects!(
            inspect,
            int,
            int,
            MirProjectionKind::IteratorElement { index: int_local },
            "non-borrowable collection base"
        );
        rejects!(
            inspect,
            array,
            string,
            MirProjectionKind::IteratorElement { index: int_local },
            "wrong item type"
        );
        rejects!(
            inspect,
            int,
            array,
            MirProjectionKind::IteratorSource,
            "non-cursor base"
        );
        rejects!(
            inspect,
            cursor,
            string,
            MirProjectionKind::IteratorSource,
            "wrong collection type"
        );
        rejects!(
            inspect,
            string,
            string,
            MirProjectionKind::Index {
                index: int_local,
                access: HirIndexAccess::String,
            },
            "String indexing cannot form a place"
        );
        rejects!(
            inspect,
            array,
            array,
            MirProjectionKind::Slice {
                start: Some(string_local),
                end: None,
                step: None,
            },
            "bound local is not Int"
        );

        let closure = hir.closures().next().unwrap();
        let closure_function = mir
            .functions
            .get(&MirFunctionId::Closure(closure.id()))
            .unwrap();
        let capture_ty = closure.captures()[0].ty();
        rejects!(
            inspect,
            closure.ty(),
            capture_ty,
            MirProjectionKind::ClosureCapture {
                closure: closure.id(),
                index: 0,
            },
            "wrong function or environment type"
        );
        rejects!(
            closure_function,
            closure.ty(),
            capture_ty,
            MirProjectionKind::ClosureCapture {
                closure: closure.id(),
                index: 9,
            },
            "index is out of range"
        );
        rejects!(
            closure_function,
            closure.ty(),
            string,
            MirProjectionKind::ClosureCapture {
                closure: closure.id(),
                index: 0,
            },
            "wrong capture type"
        );

        assert_eq!(
            verifier
                .projection_result(
                    inspect,
                    option,
                    &MirProjection {
                        ty: int,
                        kind: MirProjectionKind::OptionValue,
                    },
                    "projection success matrix",
                )
                .unwrap(),
            int
        );
        assert_eq!(
            verifier
                .projection_result(
                    inspect,
                    result,
                    &MirProjection {
                        ty: int,
                        kind: MirProjectionKind::ResultOkValue,
                    },
                    "projection success matrix",
                )
                .unwrap(),
            int
        );
    }

    #[test]
    fn block_guard_matrix_rejects_wrong_effect_storage_and_cleanup_contexts() {
        const SOURCE: &str = "fn inspect(value: Int, text: String): Int { value }\n\
             fn sum(left: Int, right: Int, text: String): Int { left + right }\n\
             fn load(value: Int): Int suspends { value }\n\
             fn effects(text: String): Int {\n\
                 let direct = load(1)\n\
                 scope {\n\
                     let task = spawn load(direct)\n\
                     await task\n\
                 }\n\
             }\n\
             fn borrowed(items: Array[Int]) {\n\
                 for ref item in items {\n\
                     _ = item\n\
                 }\n\
             }\n\
             fn owning(items: Array[Int]) {\n\
                 for item in items {\n\
                     _ = item\n\
                 }\n\
             }\n";

        let (resolved, hir, mir) = checked_mir(SOURCE);
        verify_mir(&resolved, &hir, &mir).unwrap();
        let capabilities = CapabilityAnalysis::new(&hir, &resolved).unwrap();
        let terminal_analysis = TerminalAnalysis::new(&hir, &resolved).unwrap();
        let verifier = Verifier {
            resolved: &resolved,
            hir: &hir,
            capability_analysis: &capabilities,
            terminal_analysis,
            capability_statuses: RefCell::new(BTreeMap::new()),
            terminal_statuses: RefCell::new(BTreeMap::new()),
            limits: MirVerificationLimits::default(),
            dataflow_steps: Cell::new(0),
        };
        let function = |name| {
            mir.functions
                .get(&MirFunctionId::Callable(callable_named(&resolved, name)))
                .unwrap()
        };
        let inspect = function("inspect");
        let sum = function("sum");
        let effects = function("effects");
        let borrowed = function("borrowed");
        let owning = function("owning");
        let int = hir.interner().scalar(ScalarType::Int);
        let string = hir.interner().scalar(ScalarType::String);
        let bool_ty = hir.interner().scalar(ScalarType::Bool);
        let place_of_type = |function: &MirFunction, ty| {
            let local = function
                .parameters
                .iter()
                .copied()
                .find(|local| function.locals[local.index() as usize].ty == ty)
                .expect("fixture omitted a parameter of the requested type");
            MirPlace {
                local,
                ty,
                projections: Vec::new(),
                source_loan: None,
            }
        };
        let int_place = place_of_type(inspect, int);
        let string_place = place_of_type(inspect, string);
        let task_scope = effects
            .blocks
            .iter()
            .flat_map(|block| &block.statements)
            .find_map(|statement| match statement.kind {
                MirStatementKind::EnterTaskScope { scope } => Some(scope),
                _ => None,
            })
            .expect("async fixture has a structured task scope");
        let spawn = effects
            .blocks
            .iter()
            .find_map(|block| {
                matches!(block.terminator.kind, MirTerminatorKind::Spawn { .. })
                    .then(|| block.terminator.kind.clone())
            })
            .expect("async fixture has a Spawn terminator");
        let await_join = effects
            .blocks
            .iter()
            .find_map(|block| match &block.terminator.kind {
                MirTerminatorKind::Await {
                    awaitable: MirAwaitable::Join(_),
                    ..
                } => Some(block.terminator.kind.clone()),
                _ => None,
            })
            .expect("async fixture has an Await Join terminator");
        let invoke = sum
            .blocks
            .iter()
            .find_map(|block| {
                matches!(block.terminator.kind, MirTerminatorKind::Invoke { .. })
                    .then(|| block.terminator.kind.clone())
            })
            .expect("checked addition has an Invoke terminator");
        let borrowed_next = borrowed
            .blocks
            .iter()
            .find_map(|block| {
                matches!(
                    block.terminator.kind,
                    MirTerminatorKind::IteratorNext {
                        borrowed_source: Some(_),
                        ..
                    }
                )
                .then(|| block.terminator.kind.clone())
            })
            .expect("borrowed loop has an IteratorNext terminator");
        let owning_next = owning
            .blocks
            .iter()
            .find_map(|block| {
                matches!(
                    block.terminator.kind,
                    MirTerminatorKind::IteratorNext {
                        borrowed_source: None,
                        ..
                    }
                )
                .then(|| block.terminator.kind.clone())
            })
            .expect("owning loop has an IteratorNext terminator");
        let unreachable = |function: &MirFunction| MirTerminator {
            span: function.span,
            kind: MirTerminatorKind::Unreachable,
        };
        let block = |function: &MirFunction,
                     kind: MirBlockKind,
                     statements: Vec<MirStatement>,
                     terminator: MirTerminatorKind| MirBasicBlock {
            kind,
            statements,
            terminator: MirTerminator {
                span: function.span,
                kind: terminator,
            },
        };
        macro_rules! rejects {
            ($function:expr, $block:expr, $message:literal) => {{
                let error = verifier
                    .verify_block($function, MirBlockId(0), &$block)
                    .unwrap_err();
                assert!(error.message().contains($message), "{error}");
            }};
        }

        rejects!(
            inspect,
            MirBasicBlock {
                kind: MirBlockKind::Normal,
                statements: vec![MirStatement {
                    span: inspect.span,
                    kind: MirStatementKind::EnterTaskScope { scope: task_scope },
                }],
                terminator: unreachable(inspect),
            },
            "task scope is entered outside ordinary async code"
        );
        rejects!(
            inspect,
            MirBasicBlock {
                kind: MirBlockKind::Cleanup,
                statements: vec![MirStatement {
                    span: inspect.span,
                    kind: MirStatementKind::ReserveLoan(MirLoanId(u32::MAX)),
                }],
                terminator: unreachable(inspect),
            },
            "cleanup block manipulates a loan reservation"
        );
        let spawn_operation = match &spawn {
            MirTerminatorKind::Spawn { operation, .. } => operation.clone(),
            _ => unreachable!(),
        };
        rejects!(
            inspect,
            MirBasicBlock {
                kind: MirBlockKind::Cleanup,
                statements: vec![MirStatement {
                    span: inspect.span,
                    kind: MirStatementKind::RegisterDefer {
                        scope: task_scope,
                        action: spawn_operation,
                        guard: None,
                    },
                }],
                terminator: unreachable(inspect),
            },
            "cleanup block registers another defer"
        );
        rejects!(
            inspect,
            MirBasicBlock {
                kind: MirBlockKind::Cleanup,
                statements: vec![MirStatement {
                    span: inspect.span,
                    kind: MirStatementKind::RegisterFallback {
                        scope: task_scope,
                        owner: int_place.clone(),
                    },
                }],
                terminator: unreachable(inspect),
            },
            "cleanup block registers a terminal fallback"
        );
        rejects!(
            inspect,
            MirBasicBlock {
                kind: MirBlockKind::Normal,
                statements: vec![MirStatement {
                    span: inspect.span,
                    kind: MirStatementKind::RegisterFallback {
                        scope: task_scope,
                        owner: int_place.clone(),
                    },
                }],
                terminator: unreachable(inspect),
            },
            "has no terminal token"
        );
        rejects!(
            inspect,
            MirBasicBlock {
                kind: MirBlockKind::Cleanup,
                statements: vec![MirStatement {
                    span: inspect.span,
                    kind: MirStatementKind::RetargetCleanup {
                        from: int_place.clone(),
                        to: int_place.clone(),
                    },
                }],
                terminator: unreachable(inspect),
            },
            "cleanup block retargets a defer guard"
        );
        rejects!(
            inspect,
            MirBasicBlock {
                kind: MirBlockKind::Normal,
                statements: vec![MirStatement {
                    span: inspect.span,
                    kind: MirStatementKind::RetargetCleanup {
                        from: int_place.clone(),
                        to: string_place.clone(),
                    },
                }],
                terminator: unreachable(inspect),
            },
            "does not preserve one complete owner place"
        );
        rejects!(
            inspect,
            MirBasicBlock {
                kind: MirBlockKind::Cleanup,
                statements: vec![MirStatement {
                    span: inspect.span,
                    kind: MirStatementKind::DisarmCleanup(int_place.clone()),
                }],
                terminator: unreachable(inspect),
            },
            "cleanup block explicitly disarms"
        );

        let bool_condition = MirOperand {
            ty: bool_ty,
            kind: MirOperandKind::Constant(MirConstant::Bool(true)),
        };
        rejects!(
            inspect,
            block(
                inspect,
                MirBlockKind::Cleanup,
                Vec::new(),
                MirTerminatorKind::SwitchBool {
                    condition: bool_condition,
                    if_true: inspect.entry,
                    if_false: inspect.entry,
                },
            ),
            "cleanup block performs an ordinary boolean branch"
        );
        rejects!(
            inspect,
            block(
                inspect,
                MirBlockKind::Cleanup,
                Vec::new(),
                MirTerminatorKind::SwitchTag {
                    value: MirOperand {
                        ty: int,
                        kind: MirOperandKind::Copy(int_place.clone()),
                    },
                    cases: vec![(MirTag::OptionSome, inspect.entry)],
                    otherwise: inspect.entry,
                },
            ),
            "cleanup block performs an ordinary tag branch"
        );
        rejects!(
            sum,
            block(sum, MirBlockKind::Cleanup, Vec::new(), invoke.clone()),
            "cleanup block invokes an ordinary fallible operation"
        );
        rejects!(
            inspect,
            block(
                inspect,
                MirBlockKind::Normal,
                Vec::new(),
                await_join.clone()
            ),
            "await appears outside ordinary async code"
        );
        rejects!(
            inspect,
            block(inspect, MirBlockKind::Normal, Vec::new(), spawn.clone()),
            "spawn appears outside ordinary async code"
        );
        rejects!(
            borrowed,
            block(
                borrowed,
                MirBlockKind::Cleanup,
                Vec::new(),
                borrowed_next.clone(),
            ),
            "cleanup block advances an iterator"
        );
        rejects!(
            inspect,
            block(
                inspect,
                MirBlockKind::Cleanup,
                Vec::new(),
                MirTerminatorKind::ValidatePlaces {
                    places: vec![int_place.clone()],
                    replacements: vec![None],
                    against: vec![Vec::new()],
                    for_write: false,
                    target: inspect.entry,
                    unwind: inspect.unwind,
                },
            ),
            "non-empty aligned ordinary operation"
        );
        rejects!(
            inspect,
            block(
                inspect,
                MirBlockKind::Cleanup,
                Vec::new(),
                MirTerminatorKind::ValidateLoan {
                    loan: MirLoanId(u32::MAX),
                    against: Vec::new(),
                    target: inspect.entry,
                    unwind: inspect.unwind,
                },
            ),
            "cleanup block validates a loan reservation"
        );

        let mut invalid_join = await_join.clone();
        let MirTerminatorKind::Await {
            awaitable: MirAwaitable::Join(join),
            ..
        } = &mut invalid_join
        else {
            unreachable!()
        };
        let MirOperandKind::Move(place) = join.kind.clone() else {
            unreachable!()
        };
        join.kind = MirOperandKind::Borrow(place);
        rejects!(
            effects,
            block(effects, MirBlockKind::Normal, Vec::new(), invalid_join),
            "await must consume its affine Join operand"
        );

        let effects_string = place_of_type(effects, string);
        let mut wrong_await_destination = await_join.clone();
        let MirTerminatorKind::Await { destination, .. } = &mut wrong_await_destination else {
            unreachable!()
        };
        *destination = effects_string.clone();
        rejects!(
            effects,
            block(
                effects,
                MirBlockKind::Normal,
                Vec::new(),
                wrong_await_destination,
            ),
            "await destination differs from its logical outcome"
        );
        let mut wrong_await_unwind = await_join;
        let MirTerminatorKind::Await { unwind, target, .. } = &mut wrong_await_unwind else {
            unreachable!()
        };
        *unwind = *target;
        rejects!(
            effects,
            block(
                effects,
                MirBlockKind::Normal,
                Vec::new(),
                wrong_await_unwind
            ),
            "await unwind edge does not enter cleanup code"
        );

        let mut wrong_spawn_destination = spawn.clone();
        let MirTerminatorKind::Spawn { destination, .. } = &mut wrong_spawn_destination else {
            unreachable!()
        };
        *destination = effects_string;
        rejects!(
            effects,
            block(
                effects,
                MirBlockKind::Normal,
                Vec::new(),
                wrong_spawn_destination,
            ),
            "spawn destination is not its exact writable Join result"
        );
        let mut wrong_spawn_unwind = spawn;
        let MirTerminatorKind::Spawn { unwind, target, .. } = &mut wrong_spawn_unwind else {
            unreachable!()
        };
        *unwind = *target;
        rejects!(
            effects,
            block(
                effects,
                MirBlockKind::Normal,
                Vec::new(),
                wrong_spawn_unwind
            ),
            "spawn unwind edge does not enter cleanup code"
        );

        let mut borrowed_guard = borrowed_next.clone();
        let MirTerminatorKind::IteratorNext {
            borrowed_source,
            exhaustion_guard,
            ..
        } = &mut borrowed_guard
        else {
            unreachable!()
        };
        *exhaustion_guard = borrowed_source.clone();
        rejects!(
            borrowed,
            block(borrowed, MirBlockKind::Normal, Vec::new(), borrowed_guard),
            "borrowed iterator carries an owning exhaustion guard"
        );
        let mut missing_borrowed_source = borrowed_next;
        let MirTerminatorKind::IteratorNext {
            borrowed_source, ..
        } = &mut missing_borrowed_source
        else {
            unreachable!()
        };
        *borrowed_source = None;
        rejects!(
            borrowed,
            block(
                borrowed,
                MirBlockKind::Normal,
                Vec::new(),
                missing_borrowed_source,
            ),
            "borrowed iterator has no source place"
        );
        let mut owning_with_source = owning_next;
        let MirTerminatorKind::IteratorNext {
            state,
            borrowed_source,
            ..
        } = &mut owning_with_source
        else {
            unreachable!()
        };
        *borrowed_source = Some(state.clone());
        rejects!(
            owning,
            block(owning, MirBlockKind::Normal, Vec::new(), owning_with_source),
            "owning iterator carries a borrowed source"
        );

        rejects!(
            inspect,
            block(
                inspect,
                MirBlockKind::Normal,
                Vec::new(),
                MirTerminatorKind::DrainDefers {
                    scopes: vec![task_scope],
                    target: inspect.unwind,
                    unwind: inspect.unwind,
                },
            ),
            "defer drain crosses an invalid normal or unwind boundary"
        );
        rejects!(
            inspect,
            MirBasicBlock {
                kind: MirBlockKind::Normal,
                statements: vec![MirStatement {
                    span: inspect.span,
                    kind: MirStatementKind::Assign {
                        destination: int_place.clone(),
                        value: MirRvalue {
                            ty: int,
                            kind: MirRvalueKind::Use(MirOperand {
                                ty: int,
                                kind: MirOperandKind::Copy(int_place.clone()),
                            }),
                        },
                    },
                }],
                terminator: MirTerminator {
                    span: inspect.span,
                    kind: MirTerminatorKind::DrainDefers {
                        scopes: vec![task_scope],
                        target: inspect.entry,
                        unwind: inspect.unwind,
                    },
                },
            },
            "defer drain block contains ordinary statements"
        );
        rejects!(
            inspect,
            block(
                inspect,
                MirBlockKind::Normal,
                Vec::new(),
                MirTerminatorKind::DrainScopes {
                    task_scopes: vec![task_scope, task_scope],
                    defer_scopes: Vec::new(),
                    target: inspect.entry,
                    unwind: inspect.unwind,
                },
            ),
            "structured drain repeats a task or defer scope"
        );
        rejects!(
            inspect,
            block(
                inspect,
                MirBlockKind::Normal,
                Vec::new(),
                MirTerminatorKind::DrainScopes {
                    task_scopes: vec![task_scope],
                    defer_scopes: Vec::new(),
                    target: inspect.entry,
                    unwind: inspect.unwind,
                },
            ),
            "task scopes are drained by a synchronous function"
        );
    }

    #[test]
    fn block_shape_corruption_matrix_rejects_every_closed_control_boundary() {
        const SIMPLE: &str = "fn inspect(value: Int): Int { value }\n";
        const SUM: &str = "fn sum(left: Int, right: Int): Int { left + right }\n";

        let error = corrupted_mir(SIMPLE, |resolved, _hir, mir| {
            let inspect = MirFunctionId::Callable(callable_named(resolved, "inspect"));
            let function = mir.functions.get_mut(&inspect).unwrap();
            let entry = function.entry.index() as usize;
            function.blocks[entry].statements.insert(
                0,
                MirStatement {
                    span: function.span,
                    kind: MirStatementKind::StorageLive(function.parameters[0]),
                },
            );
        });
        assert!(
            error
                .message()
                .contains("locals have function-wide storage"),
            "{error}"
        );

        let error = corrupted_mir(SIMPLE, |resolved, hir, mir| {
            let inspect = MirFunctionId::Callable(callable_named(resolved, "inspect"));
            let function = mir.functions.get_mut(&inspect).unwrap();
            let entry = function.entry.index() as usize;
            let parameter = function.parameters[0];
            let destination = MirPlace {
                local: parameter,
                ty: function.locals[parameter.index() as usize].ty,
                projections: Vec::new(),
                source_loan: None,
            };
            function.blocks[entry].statements.insert(
                0,
                MirStatement {
                    span: function.span,
                    kind: MirStatementKind::Assign {
                        destination,
                        value: MirRvalue {
                            ty: hir.interner().scalar(ScalarType::Bool),
                            kind: MirRvalueKind::Use(MirOperand {
                                ty: hir.interner().scalar(ScalarType::Bool),
                                kind: MirOperandKind::Constant(MirConstant::Bool(true)),
                            }),
                        },
                    },
                },
            );
        });
        assert!(
            error.message().contains("assignment writes type#"),
            "{error}"
        );

        let error = corrupted_mir(SIMPLE, |resolved, hir, mir| {
            let inspect = MirFunctionId::Callable(callable_named(resolved, "inspect"));
            let function = mir.functions.get_mut(&inspect).unwrap();
            let entry = function.entry;
            let span = function.blocks[entry.index() as usize].terminator.span;
            let integer = hir.interner().scalar(ScalarType::Int);
            function.blocks[entry.index() as usize].terminator = MirTerminator {
                span,
                kind: MirTerminatorKind::SwitchBool {
                    condition: MirOperand {
                        ty: integer,
                        kind: MirOperandKind::Constant(MirConstant::Integer("1".into())),
                    },
                    if_true: entry,
                    if_false: entry,
                },
            };
        });
        assert!(
            error
                .message()
                .contains("condition is not a materialized Bool"),
            "{error}"
        );

        let error = corrupted_mir(SIMPLE, |resolved, hir, mir| {
            let inspect = MirFunctionId::Callable(callable_named(resolved, "inspect"));
            let function = mir.functions.get_mut(&inspect).unwrap();
            let entry = function.entry;
            let span = function.blocks[entry.index() as usize].terminator.span;
            let integer = hir.interner().scalar(ScalarType::Int);
            function.blocks[entry.index() as usize].terminator = MirTerminator {
                span,
                kind: MirTerminatorKind::SwitchTag {
                    value: MirOperand {
                        ty: integer,
                        kind: MirOperandKind::Constant(MirConstant::Integer("1".into())),
                    },
                    cases: Vec::new(),
                    otherwise: entry,
                },
            };
        });
        assert!(
            error
                .message()
                .contains("value is not materialized in a place"),
            "{error}"
        );

        let error = corrupted_mir(SIMPLE, |resolved, _hir, mir| {
            let inspect = MirFunctionId::Callable(callable_named(resolved, "inspect"));
            let function = mir.functions.get_mut(&inspect).unwrap();
            let entry = function.entry;
            let span = function.blocks[entry.index() as usize].terminator.span;
            let parameter = function.parameters[0];
            let ty = function.locals[parameter.index() as usize].ty;
            function.blocks[entry.index() as usize].terminator = MirTerminator {
                span,
                kind: MirTerminatorKind::SwitchTag {
                    value: MirOperand {
                        ty,
                        kind: MirOperandKind::Copy(MirPlace {
                            local: parameter,
                            ty,
                            projections: Vec::new(),
                            source_loan: None,
                        }),
                    },
                    cases: Vec::new(),
                    otherwise: entry,
                },
            };
        });
        assert!(error.message().contains("has no explicit cases"), "{error}");

        let error = corrupted_mir(
            "fn inspect(value: Int?): Int { 0 }\n",
            |resolved, _hir, mir| {
                let inspect = MirFunctionId::Callable(callable_named(resolved, "inspect"));
                let function = mir.functions.get_mut(&inspect).unwrap();
                let entry = function.entry;
                let span = function.blocks[entry.index() as usize].terminator.span;
                let parameter = function.parameters[0];
                let ty = function.locals[parameter.index() as usize].ty;
                function.blocks[entry.index() as usize].terminator = MirTerminator {
                    span,
                    kind: MirTerminatorKind::SwitchTag {
                        value: MirOperand {
                            ty,
                            kind: MirOperandKind::Copy(MirPlace {
                                local: parameter,
                                ty,
                                projections: Vec::new(),
                                source_loan: None,
                            }),
                        },
                        cases: vec![(MirTag::OptionSome, entry), (MirTag::OptionSome, entry)],
                        otherwise: entry,
                    },
                };
            },
        );
        assert!(error.message().contains("is duplicated"), "{error}");

        let error = corrupted_mir(SUM, |resolved, _hir, mir| {
            let sum = MirFunctionId::Callable(callable_named(resolved, "sum"));
            let function = mir.functions.get_mut(&sum).unwrap();
            let terminator = function
                .blocks
                .iter_mut()
                .find_map(|block| match &mut block.terminator.kind {
                    MirTerminatorKind::Invoke {
                        destination,
                        target,
                        ..
                    } if destination.is_some() && target.is_some() => Some((destination, target)),
                    _ => None,
                })
                .expect("checked addition lowers through Invoke");
            *terminator.0 = None;
        });
        assert!(
            error.message().contains("must have both destination"),
            "{error}"
        );

        let error = corrupted_mir(SUM, |resolved, _hir, mir| {
            let sum = MirFunctionId::Callable(callable_named(resolved, "sum"));
            let function = mir.functions.get_mut(&sum).unwrap();
            let entry = function.entry;
            let unwind = function
                .blocks
                .iter_mut()
                .find_map(|block| match &mut block.terminator.kind {
                    MirTerminatorKind::Invoke { unwind, .. } => Some(unwind),
                    _ => None,
                })
                .expect("checked addition lowers through Invoke");
            *unwind = entry;
        });
        assert!(
            error
                .message()
                .contains("unwind edge does not enter cleanup"),
            "{error}"
        );

        let error = corrupted_mir(SIMPLE, |resolved, _hir, mir| {
            let inspect = MirFunctionId::Callable(callable_named(resolved, "inspect"));
            let function = mir.functions.get_mut(&inspect).unwrap();
            let entry = function.entry;
            let unwind = function.unwind;
            let span = function.blocks[entry.index() as usize].terminator.span;
            let parameter = function.parameters[0];
            let ty = function.locals[parameter.index() as usize].ty;
            let place = MirPlace {
                local: parameter,
                ty,
                projections: Vec::new(),
                source_loan: None,
            };
            function.blocks[entry.index() as usize].terminator = MirTerminator {
                span,
                kind: MirTerminatorKind::IteratorNext {
                    state: place.clone(),
                    destination: place,
                    borrowed_source: None,
                    exhaustion_guard: None,
                    has_value: entry,
                    exhausted: entry,
                    unwind,
                },
            };
        });
        assert!(
            error
                .message()
                .contains("state is not a concrete intrinsic cursor"),
            "{error}"
        );

        let error = corrupted_mir(SIMPLE, |resolved, _hir, mir| {
            let inspect = MirFunctionId::Callable(callable_named(resolved, "inspect"));
            let function = mir.functions.get_mut(&inspect).unwrap();
            let entry = function.entry;
            let span = function.blocks[entry.index() as usize].terminator.span;
            function.blocks[entry.index() as usize].terminator = MirTerminator {
                span,
                kind: MirTerminatorKind::ValidatePlaces {
                    places: Vec::new(),
                    replacements: Vec::new(),
                    against: Vec::new(),
                    for_write: false,
                    target: entry,
                    unwind: function.unwind,
                },
            };
        });
        assert!(
            error
                .message()
                .contains("non-empty aligned ordinary operation"),
            "{error}"
        );

        let error = corrupted_mir(SIMPLE, |resolved, _hir, mir| {
            let inspect = MirFunctionId::Callable(callable_named(resolved, "inspect"));
            let function = mir.functions.get_mut(&inspect).unwrap();
            let entry = function.entry;
            let span = function.blocks[entry.index() as usize].terminator.span;
            let parameter = function.parameters[0];
            let ty = function.locals[parameter.index() as usize].ty;
            let place = MirPlace {
                local: parameter,
                ty,
                projections: Vec::new(),
                source_loan: None,
            };
            function.blocks[entry.index() as usize].terminator = MirTerminator {
                span,
                kind: MirTerminatorKind::ValidatePlaces {
                    places: vec![place.clone(), place],
                    replacements: vec![None, None],
                    against: vec![Vec::new(), Vec::new()],
                    for_write: false,
                    target: entry,
                    unwind: function.unwind,
                },
            };
        });
        assert!(
            error.message().contains("repeats the same destination"),
            "{error}"
        );

        let error = corrupted_mir(SIMPLE, |resolved, _hir, mir| {
            let inspect = MirFunctionId::Callable(callable_named(resolved, "inspect"));
            let function = mir.functions.get_mut(&inspect).unwrap();
            let entry = function.entry;
            let span = function.blocks[entry.index() as usize].terminator.span;
            let parameter = function.parameters[0];
            let ty = function.locals[parameter.index() as usize].ty;
            let place = MirPlace {
                local: parameter,
                ty,
                projections: Vec::new(),
                source_loan: None,
            };
            function.blocks[entry.index() as usize].terminator = MirTerminator {
                span,
                kind: MirTerminatorKind::ValidatePlaces {
                    places: vec![place],
                    replacements: vec![Some(MirOperand {
                        ty,
                        kind: MirOperandKind::Constant(MirConstant::Integer("1".into())),
                    })],
                    against: vec![Vec::new()],
                    for_write: true,
                    target: entry,
                    unwind: function.unwind,
                },
            };
        });
        assert!(
            error.message().contains("requires a borrowed replacement"),
            "{error}"
        );

        let error = corrupted_mir(SIMPLE, |resolved, _hir, mir| {
            let inspect = MirFunctionId::Callable(callable_named(resolved, "inspect"));
            let function = mir.functions.get_mut(&inspect).unwrap();
            let entry = function.entry;
            let span = function.blocks[entry.index() as usize].terminator.span;
            function.blocks[entry.index() as usize].terminator = MirTerminator {
                span,
                kind: MirTerminatorKind::DrainDefers {
                    scopes: Vec::new(),
                    target: entry,
                    unwind: function.unwind,
                },
            };
        });
        assert!(
            error.message().contains("empty or duplicate scope set"),
            "{error}"
        );

        let error = corrupted_mir(SIMPLE, |resolved, _hir, mir| {
            let inspect = MirFunctionId::Callable(callable_named(resolved, "inspect"));
            let function = mir.functions.get_mut(&inspect).unwrap();
            let entry = function.entry;
            let span = function.blocks[entry.index() as usize].terminator.span;
            function.blocks[entry.index() as usize].terminator = MirTerminator {
                span,
                kind: MirTerminatorKind::DrainScopes {
                    task_scopes: Vec::new(),
                    defer_scopes: Vec::new(),
                    target: entry,
                    unwind: function.unwind,
                },
            };
        });
        assert!(
            error.message().contains("has no task or defer scopes"),
            "{error}"
        );

        let error = corrupted_mir(SIMPLE, |resolved, _hir, mir| {
            let inspect = MirFunctionId::Callable(callable_named(resolved, "inspect"));
            let function = mir.functions.get_mut(&inspect).unwrap();
            let entry = function.entry;
            let span = function.blocks[entry.index() as usize].terminator.span;
            function.blocks[entry.index() as usize].terminator = MirTerminator {
                span,
                kind: MirTerminatorKind::DrainUnwind {
                    target: function.unwind,
                },
            };
        });
        assert!(
            error.message().contains("not an empty cleanup block"),
            "{error}"
        );

        let error = corrupted_mir(SIMPLE, |resolved, _hir, mir| {
            let inspect = MirFunctionId::Callable(callable_named(resolved, "inspect"));
            let function = mir.functions.get_mut(&inspect).unwrap();
            let entry = function.entry.index() as usize;
            function.blocks[entry].terminator.kind = MirTerminatorKind::ResumePanic;
        });
        assert!(
            error.message().contains("ordinary block resumes panic"),
            "{error}"
        );

        let error = corrupted_mir(SIMPLE, |resolved, _hir, mir| {
            let inspect = MirFunctionId::Callable(callable_named(resolved, "inspect"));
            let function = mir.functions.get_mut(&inspect).unwrap();
            function.blocks.push(MirBasicBlock {
                kind: MirBlockKind::Cleanup,
                statements: Vec::new(),
                terminator: MirTerminator {
                    span: function.span,
                    kind: MirTerminatorKind::Return,
                },
            });
        });
        assert!(
            error.message().contains("cleanup block returns normally"),
            "{error}"
        );

        let error = corrupted_mir(SIMPLE, |resolved, _hir, mir| {
            let inspect = MirFunctionId::Callable(callable_named(resolved, "inspect"));
            let function = mir.functions.get_mut(&inspect).unwrap();
            function.blocks.push(MirBasicBlock {
                kind: MirBlockKind::Cleanup,
                statements: Vec::new(),
                terminator: MirTerminator {
                    span: function.span,
                    kind: MirTerminatorKind::Goto {
                        target: function.entry,
                    },
                },
            });
        });
        assert!(error.message().contains("Goto crosses"), "{error}");

        let error = corrupted_mir(
            "fn stop(): Never { panic(\"stop\") }\n",
            |resolved, _hir, mir| {
                let stop = MirFunctionId::Callable(callable_named(resolved, "stop"));
                let function = mir.functions.get_mut(&stop).unwrap();
                let entry = function.entry.index() as usize;
                function.blocks[entry].terminator.kind = MirTerminatorKind::Return;
            },
        );
        assert!(
            error
                .message()
                .contains("Never function has a normal return"),
            "{error}"
        );
    }

    #[test]
    fn rvalue_corruption_matrix_rejects_every_closed_value_contract() {
        const SOURCE: &str = "const Answer: Int = 42\n\
             type Record = { value: Int, text: String }\n\
             fn values(\n\
                 value: Int,\n\
                 flag: Bool,\n\
                 text: String,\n\
                 record: Record,\n\
                 items: Array[Int],\n\
                 entries: var Map[String, Int],\n\
             ): Int? {\n\
                 let used = value\n\
                 let named = Answer\n\
                 let prefixed = not flag\n\
                 let compared = value == Answer\n\
                 let updated = record with { value: value }\n\
                 let optional: Int? = value\n\
                 let converted = Int32(value)\n\
                 let range = 0..10\n\
                 let contained = value in items\n\
                 let removed = entries.remove(text)\n\
                 let interpolated = \"value {value}\"\n\
                 for item in items {\n\
                     _ = item\n\
                 }\n\
                 let first = match items {\n\
                     [item, ..] => item\n\
                     [] => 0\n\
                 }\n\
                 _ = used\n\
                 _ = named\n\
                 _ = prefixed\n\
                 _ = compared\n\
                 _ = updated\n\
                 _ = optional\n\
                 _ = converted\n\
                 _ = range\n\
                 _ = contained\n\
                 _ = interpolated\n\
                 _ = first\n\
                 removed\n\
             }\n";

        let error = corrupted_mir(SOURCE, |resolved, _hir, mir| {
            let values = MirFunctionId::Callable(callable_named(resolved, "values"));
            let value = rvalue_mut(mir.functions.get_mut(&values).unwrap(), |kind| {
                matches!(
                    kind,
                    MirRvalueKind::Use(MirOperand {
                        kind: MirOperandKind::Copy(_),
                        ..
                    })
                )
            });
            let MirRvalueKind::Use(operand) = &mut value.kind else {
                unreachable!()
            };
            let MirOperandKind::Copy(place) = &operand.kind else {
                unreachable!()
            };
            operand.kind = MirOperandKind::Borrow(place.clone());
        });
        assert!(
            error
                .message()
                .contains("borrow escapes its permitted immediate observation"),
            "{error}"
        );

        let error = corrupted_mir(SOURCE, |resolved, hir, mir| {
            let values = MirFunctionId::Callable(callable_named(resolved, "values"));
            let value = rvalue_mut(mir.functions.get_mut(&values).unwrap(), |kind| {
                matches!(kind, MirRvalueKind::Use(_))
            });
            value.ty = hir.interner().scalar(ScalarType::String);
        });
        assert!(error.message().contains("Use rvalue changes"), "{error}");

        let error = corrupted_mir(SOURCE, |resolved, hir, mir| {
            let values = MirFunctionId::Callable(callable_named(resolved, "values"));
            let value = rvalue_mut(mir.functions.get_mut(&values).unwrap(), |kind| {
                matches!(kind, MirRvalueKind::Prefix { .. })
            });
            let integer = hir.interner().scalar(ScalarType::Int);
            value.ty = integer;
            value.kind = MirRvalueKind::Prefix {
                operator: HirPrefixOperator::Negate,
                operand: MirOperand {
                    ty: integer,
                    kind: MirOperandKind::Constant(MirConstant::Integer("1".into())),
                },
            };
        });
        assert!(
            error
                .message()
                .contains("potentially panicking prefix operation is not an Invoke"),
            "{error}"
        );

        let error = corrupted_mir(SOURCE, |resolved, hir, mir| {
            let values = MirFunctionId::Callable(callable_named(resolved, "values"));
            let value = rvalue_mut(mir.functions.get_mut(&values).unwrap(), |kind| {
                matches!(kind, MirRvalueKind::Binary { .. })
            });
            let integer = hir.interner().scalar(ScalarType::Int);
            let operand = MirOperand {
                ty: integer,
                kind: MirOperandKind::Constant(MirConstant::Integer("1".into())),
            };
            value.ty = integer;
            value.kind = MirRvalueKind::Binary {
                operator: HirBinaryOperator::Divide,
                left: operand.clone(),
                right: operand,
            };
        });
        assert!(
            error
                .message()
                .contains("potentially panicking binary operation is not an Invoke"),
            "{error}"
        );

        let error = corrupted_mir(SOURCE, |resolved, hir, mir| {
            let values = MirFunctionId::Callable(callable_named(resolved, "values"));
            let value = rvalue_mut(mir.functions.get_mut(&values).unwrap(), |kind| {
                matches!(kind, MirRvalueKind::RecordUpdate { .. })
            });
            value.ty = hir.interner().scalar(ScalarType::Int);
        });
        assert!(
            error
                .message()
                .contains("record update changes the nominal base type"),
            "{error}"
        );

        let error = corrupted_mir(SOURCE, |resolved, _hir, mir| {
            let values = MirFunctionId::Callable(callable_named(resolved, "values"));
            let value = rvalue_mut(mir.functions.get_mut(&values).unwrap(), |kind| {
                matches!(kind, MirRvalueKind::RecordUpdate { .. })
            });
            let MirRvalueKind::RecordUpdate { fields, .. } = &mut value.kind else {
                unreachable!()
            };
            fields.push(fields[0].clone());
        });
        assert!(
            error.message().contains("unknown or duplicate field"),
            "{error}"
        );

        let error = corrupted_mir(SOURCE, |resolved, hir, mir| {
            let values = MirFunctionId::Callable(callable_named(resolved, "values"));
            let value = rvalue_mut(mir.functions.get_mut(&values).unwrap(), |kind| {
                matches!(kind, MirRvalueKind::RecordUpdate { .. })
            });
            let MirRvalueKind::RecordUpdate { fields, .. } = &mut value.kind else {
                unreachable!()
            };
            fields[0].1 = MirOperand {
                ty: hir.interner().scalar(ScalarType::String),
                kind: MirOperandKind::Constant(MirConstant::String("wrong".into())),
            };
        });
        assert!(
            error
                .message()
                .contains("does not match its instantiated field type"),
            "{error}"
        );

        let error = corrupted_mir(SOURCE, |resolved, _hir, mir| {
            let values = MirFunctionId::Callable(callable_named(resolved, "values"));
            let value = rvalue_mut(mir.functions.get_mut(&values).unwrap(), |kind| {
                matches!(kind, MirRvalueKind::Coerce { .. })
            });
            let MirRvalueKind::Coerce { kind, .. } = &mut value.kind else {
                unreachable!()
            };
            *kind = Assignability::Exact;
        });
        assert!(
            error.message().contains("coercion kind does not match"),
            "{error}"
        );

        let error = corrupted_mir(SOURCE, |resolved, _hir, mir| {
            let values = MirFunctionId::Callable(callable_named(resolved, "values"));
            let value = rvalue_mut(mir.functions.get_mut(&values).unwrap(), |kind| {
                matches!(kind, MirRvalueKind::NumericConversion { .. })
            });
            let MirRvalueKind::NumericConversion { conversion, .. } = &mut value.kind else {
                unreachable!()
            };
            *conversion = NumericConversion::Identity;
        });
        assert!(error.message().contains("numeric conversion"), "{error}");

        let error = corrupted_mir(SOURCE, |resolved, hir, mir| {
            let values = MirFunctionId::Callable(callable_named(resolved, "values"));
            let value = rvalue_mut(mir.functions.get_mut(&values).unwrap(), |kind| {
                matches!(kind, MirRvalueKind::Range { .. })
            });
            let MirRvalueKind::Range { start, .. } = &mut value.kind else {
                unreachable!()
            };
            *start = MirOperand {
                ty: hir.interner().scalar(ScalarType::String),
                kind: MirOperandKind::Constant(MirConstant::String("wrong".into())),
            };
        });
        assert!(
            error
                .message()
                .contains("range bounds or result element type"),
            "{error}"
        );

        let error = corrupted_mir(SOURCE, |resolved, _hir, mir| {
            let values = MirFunctionId::Callable(callable_named(resolved, "values"));
            let value = rvalue_mut(mir.functions.get_mut(&values).unwrap(), |kind| {
                matches!(kind, MirRvalueKind::Contains { .. })
            });
            let MirRvalueKind::Contains { kind, .. } = &mut value.kind else {
                unreachable!()
            };
            *kind = HirContainmentKind::StringChar;
        });
        assert!(error.message().contains("containment"), "{error}");

        let error = corrupted_mir(SOURCE, |resolved, hir, mir| {
            let values = MirFunctionId::Callable(callable_named(resolved, "values"));
            let value = rvalue_mut(mir.functions.get_mut(&values).unwrap(), |kind| {
                matches!(kind, MirRvalueKind::MapRemove { .. })
            });
            value.ty = hir.interner().scalar(ScalarType::Int);
        });
        assert!(
            error.message().contains("result is not Option[V]"),
            "{error}"
        );

        let error = corrupted_mir(SOURCE, |resolved, hir, mir| {
            let values = MirFunctionId::Callable(callable_named(resolved, "values"));
            let value = rvalue_mut(mir.functions.get_mut(&values).unwrap(), |kind| {
                matches!(kind, MirRvalueKind::Interpolate { .. })
            });
            let MirRvalueKind::Interpolate { values, .. } = &mut value.kind else {
                unreachable!()
            };
            values[0] = MirOperand {
                ty: hir.interner().scalar(ScalarType::Int),
                kind: MirOperandKind::Constant(MirConstant::Integer("1".into())),
            };
        });
        assert!(
            error
                .message()
                .contains("interpolation received a non-String Display result"),
            "{error}"
        );

        let error = corrupted_mir(SOURCE, |resolved, hir, mir| {
            let values = MirFunctionId::Callable(callable_named(resolved, "values"));
            let value = rvalue_mut(mir.functions.get_mut(&values).unwrap(), |kind| {
                matches!(kind, MirRvalueKind::Length(_))
            });
            value.ty = hir.interner().scalar(ScalarType::String);
        });
        assert!(error.message().contains("length requires"), "{error}");

        let error = corrupted_mir(SOURCE, |resolved, hir, mir| {
            let values = MirFunctionId::Callable(callable_named(resolved, "values"));
            let value = rvalue_mut(mir.functions.get_mut(&values).unwrap(), |kind| {
                matches!(kind, MirRvalueKind::IteratorState { .. })
            });
            value.ty = hir.interner().scalar(ScalarType::Int);
        });
        assert!(
            error.message().contains("iterator state result is not"),
            "{error}"
        );
    }

    #[test]
    fn operation_corruption_matrix_rejects_every_closed_operation_contract() {
        const SOURCE: &str = "import std.console\n\
             fn identity(value: Int): Int { value }\n\
             fn operations(\n\
                 value: Int,\n\
                 text: String,\n\
                 values: Array[Int],\n\
             ) {\n\
                 let negated = -value\n\
                 let sum = value + 1\n\
                 let concatenated = values.concat(values)\n\
                 let repeated = values.repeat(2)\n\
                 let entries = [\"value\": value]\n\
                 let indexed = values[value]\n\
                 let sliced = values[value:]\n\
                 let called = identity(value)\n\
                 assert(value == called, text)\n\
                 console.print(text)\n\
                 _ = negated\n\
                 _ = sum\n\
                 _ = concatenated\n\
                 _ = repeated\n\
                 _ = entries\n\
                 _ = indexed\n\
                 _ = sliced\n\
             }\n\
             fn stop(): Never { panic(\"stop\") }\n";

        let error = corrupted_mir(SOURCE, |resolved, hir, mir| {
            let operations = MirFunctionId::Callable(callable_named(resolved, "operations"));
            let operation = operation_mut(mir.functions.get_mut(&operations).unwrap(), |kind| {
                matches!(kind, MirOperationKind::CheckedPrefix { .. })
            });
            let boolean = hir.interner().scalar(ScalarType::Bool);
            operation.ty = boolean;
            operation.kind = MirOperationKind::CheckedPrefix {
                operator: HirPrefixOperator::LogicalNot,
                operand: MirOperand {
                    ty: boolean,
                    kind: MirOperandKind::Constant(MirConstant::Bool(true)),
                },
            };
        });
        assert!(
            error.message().contains("non-panicking prefix operation"),
            "{error}"
        );

        let error = corrupted_mir(SOURCE, |resolved, hir, mir| {
            let operations = MirFunctionId::Callable(callable_named(resolved, "operations"));
            let operation = operation_mut(mir.functions.get_mut(&operations).unwrap(), |kind| {
                matches!(kind, MirOperationKind::CheckedBinary { .. })
            });
            let integer = hir.interner().scalar(ScalarType::Int);
            let boolean = hir.interner().scalar(ScalarType::Bool);
            let constant = MirOperand {
                ty: integer,
                kind: MirOperandKind::Constant(MirConstant::Integer("1".into())),
            };
            operation.ty = boolean;
            operation.kind = MirOperationKind::CheckedBinary {
                operator: HirBinaryOperator::Equal,
                left: constant.clone(),
                right: constant,
            };
        });
        assert!(
            error.message().contains("non-panicking binary operation"),
            "{error}"
        );

        let error = corrupted_mir(SOURCE, |resolved, _hir, mir| {
            let operations = MirFunctionId::Callable(callable_named(resolved, "operations"));
            let operation = operation_mut(mir.functions.get_mut(&operations).unwrap(), |kind| {
                matches!(kind, MirOperationKind::ArraySequence { .. })
            });
            let MirOperationKind::ArraySequence { array, .. } = &mut operation.kind else {
                unreachable!()
            };
            let MirOperandKind::Borrow(place) = &array.kind else {
                unreachable!()
            };
            array.kind = MirOperandKind::Copy(place.clone());
        });
        assert!(
            error
                .message()
                .contains("requires a borrowed Array[T: Copy] receiver"),
            "{error}"
        );

        let error = corrupted_mir(SOURCE, |resolved, hir, mir| {
            let operations = MirFunctionId::Callable(callable_named(resolved, "operations"));
            let operation = operation_mut(mir.functions.get_mut(&operations).unwrap(), |kind| {
                matches!(kind, MirOperationKind::BuildMap { .. })
            });
            let MirOperationKind::BuildMap { entries, .. } = &mut operation.kind else {
                unreachable!()
            };
            entries[0].0 = MirOperand {
                ty: hir.interner().scalar(ScalarType::Int),
                kind: MirOperandKind::Constant(MirConstant::Integer("1".into())),
            };
        });
        assert!(
            error
                .message()
                .contains("map entry does not match the map key/value types"),
            "{error}"
        );

        let error = corrupted_mir(SOURCE, |resolved, hir, mir| {
            let operations = MirFunctionId::Callable(callable_named(resolved, "operations"));
            let operation = operation_mut(mir.functions.get_mut(&operations).unwrap(), |kind| {
                matches!(kind, MirOperationKind::Index { .. })
            });
            let string = hir.interner().scalar(ScalarType::String);
            let character = hir.interner().scalar(ScalarType::Char);
            let integer = hir.interner().scalar(ScalarType::Int);
            operation.ty = character;
            operation.kind = MirOperationKind::Index {
                base: MirOperand {
                    ty: string,
                    kind: MirOperandKind::Constant(MirConstant::String("text".into())),
                },
                index: MirOperand {
                    ty: integer,
                    kind: MirOperandKind::Constant(MirConstant::Integer("0".into())),
                },
                access: HirIndexAccess::String,
                against: vec![MirLoanId(0)],
            };
        });
        assert!(
            error
                .message()
                .contains("String indexing cannot carry runtime place conflicts"),
            "{error}"
        );

        let error = corrupted_mir(SOURCE, |resolved, hir, mir| {
            let operations = MirFunctionId::Callable(callable_named(resolved, "operations"));
            let operation = operation_mut(mir.functions.get_mut(&operations).unwrap(), |kind| {
                matches!(kind, MirOperationKind::Slice { .. })
            });
            let string = hir.interner().scalar(ScalarType::String);
            operation.ty = string;
            operation.kind = MirOperationKind::Slice {
                base: MirOperand {
                    ty: string,
                    kind: MirOperandKind::Constant(MirConstant::String("text".into())),
                },
                bounds: Box::new(MirSliceBounds {
                    start: None,
                    end: None,
                    step: None,
                }),
                against: vec![MirLoanId(0)],
            };
        });
        assert!(
            error
                .message()
                .contains("String slicing cannot carry runtime place conflicts"),
            "{error}"
        );

        let error = corrupted_mir(SOURCE, |resolved, _hir, mir| {
            let operations = MirFunctionId::Callable(callable_named(resolved, "operations"));
            let operation = operation_mut(mir.functions.get_mut(&operations).unwrap(), |kind| {
                matches!(kind, MirOperationKind::Call { .. })
            });
            let MirOperationKind::Call { arguments, .. } = &mut operation.kind else {
                unreachable!()
            };
            arguments[0].target = HirCallArgumentTarget::Invalid;
        });
        assert!(
            error
                .message()
                .contains("retains an invalid argument association"),
            "{error}"
        );

        let error = corrupted_mir(SOURCE, |resolved, hir, mir| {
            let stop = MirFunctionId::Callable(callable_named(resolved, "stop"));
            let operation = operation_mut(mir.functions.get_mut(&stop).unwrap(), |kind| {
                matches!(kind, MirOperationKind::ExplicitPanic { .. })
            });
            let MirOperationKind::ExplicitPanic { message } = &mut operation.kind else {
                unreachable!()
            };
            *message = MirOperand {
                ty: hir.interner().scalar(ScalarType::Int),
                kind: MirOperandKind::Constant(MirConstant::Integer("1".into())),
            };
        });
        assert!(
            error.message().contains("panic requires a String message"),
            "{error}"
        );

        let error = corrupted_mir(SOURCE, |resolved, hir, mir| {
            let operations = MirFunctionId::Callable(callable_named(resolved, "operations"));
            let operation = operation_mut(mir.functions.get_mut(&operations).unwrap(), |kind| {
                matches!(kind, MirOperationKind::Assert { .. })
            });
            let MirOperationKind::Assert { condition, .. } = &mut operation.kind else {
                unreachable!()
            };
            *condition = MirOperand {
                ty: hir.interner().scalar(ScalarType::Int),
                kind: MirOperandKind::Constant(MirConstant::Integer("1".into())),
            };
        });
        assert!(error.message().contains("condition is not Bool"), "{error}");

        let error = corrupted_mir(SOURCE, |resolved, hir, mir| {
            let operations = MirFunctionId::Callable(callable_named(resolved, "operations"));
            let operation = operation_mut(mir.functions.get_mut(&operations).unwrap(), |kind| {
                matches!(kind, MirOperationKind::Assert { .. })
            });
            let MirOperationKind::Assert { message_parts, .. } = &mut operation.kind else {
                unreachable!()
            };
            message_parts[0].value = MirOperand {
                ty: hir.interner().scalar(ScalarType::Int),
                kind: MirOperandKind::Constant(MirConstant::Integer("1".into())),
            };
        });
        assert!(
            error.message().contains("message part is not String"),
            "{error}"
        );

        let error = corrupted_mir(SOURCE, |resolved, _hir, mir| {
            let operations = MirFunctionId::Callable(callable_named(resolved, "operations"));
            let operation = operation_mut(mir.functions.get_mut(&operations).unwrap(), |kind| {
                matches!(kind, MirOperationKind::BootstrapHostCall { .. })
            });
            let MirOperationKind::BootstrapHostCall { function, .. } = &mut operation.kind else {
                unreachable!()
            };
            *function = MirBootstrapHostFunction::ExitStatusSuccess;
        });
        assert!(
            error
                .message()
                .contains("bootstrap host operation does not match its closed contract"),
            "{error}"
        );
    }

    #[test]
    fn format_operations_are_explicitly_verified_and_reject_corruption() {
        const SOURCE: &str = "import std.format\n\
             type Label = { text: String }\n\
             impl Display for Label {\n\
                 fn display(self): String { self.text }\n\
             }\n\
             fn operations(label: Label, values: Array[Int]): !format.FormatError {\n\
                 let custom = format.format(label)?\n\
                 let intrinsic = format.format(42)?\n\
                 let joined = format.join(values, \",\")?\n\
                 _ = custom\n\
                 _ = intrinsic\n\
                 _ = joined\n\
             }\n";

        let (resolved, hir, mir) = checked_mir(SOURCE);
        let operations = MirFunctionId::Callable(callable_named(&resolved, "operations"));
        let function = mir.functions.get(&operations).unwrap();
        assert!(function.blocks.iter().any(|block| {
            matches!(
                block.terminator.kind,
                MirTerminatorKind::Invoke {
                    operation: MirOperation {
                        kind: MirOperationKind::Format {
                            display: Some(_),
                            ..
                        },
                        ..
                    },
                    ..
                }
            )
        }));
        assert!(function.blocks.iter().any(|block| {
            matches!(
                block.terminator.kind,
                MirTerminatorKind::Invoke {
                    operation: MirOperation {
                        kind: MirOperationKind::Format { display: None, .. },
                        ..
                    },
                    ..
                }
            )
        }));
        assert!(function.blocks.iter().any(|block| {
            matches!(
                block.terminator.kind,
                MirTerminatorKind::Invoke {
                    operation: MirOperation {
                        kind: MirOperationKind::JoinFormat { display: None, .. },
                        ..
                    },
                    ..
                }
            )
        }));
        verify_mir(&resolved, &hir, &mir).unwrap();

        let error = corrupted_mir(SOURCE, |resolved, hir, mir| {
            let operations = MirFunctionId::Callable(callable_named(resolved, "operations"));
            let operation = operation_mut(mir.functions.get_mut(&operations).unwrap(), |kind| {
                matches!(kind, MirOperationKind::Format { display: None, .. })
            });
            operation.ty = hir.interner().scalar(ScalarType::Int);
        });
        assert!(
            error
                .message()
                .contains("format operation must produce Result[String, FormatError]"),
            "{error}"
        );

        let error = corrupted_mir(SOURCE, |resolved, hir, mir| {
            let operations = MirFunctionId::Callable(callable_named(resolved, "operations"));
            let operation = operation_mut(mir.functions.get_mut(&operations).unwrap(), |kind| {
                matches!(kind, MirOperationKind::JoinFormat { .. })
            });
            let MirOperationKind::JoinFormat { separator, .. } = &mut operation.kind else {
                unreachable!()
            };
            *separator = MirOperand {
                ty: hir.interner().scalar(ScalarType::Int),
                kind: MirOperandKind::Constant(MirConstant::Integer("1".into())),
            };
        });
        assert!(
            error.message().contains("separator is not String"),
            "{error}"
        );

        let error = corrupted_mir(SOURCE, |resolved, _hir, mir| {
            let operations = MirFunctionId::Callable(callable_named(resolved, "operations"));
            let callback = mir
                .functions
                .get(&operations)
                .unwrap()
                .blocks
                .iter()
                .find_map(|block| match &block.terminator.kind {
                    MirTerminatorKind::Invoke {
                        operation:
                            MirOperation {
                                kind:
                                    MirOperationKind::Format {
                                        display: Some(callback),
                                        ..
                                    },
                                ..
                            },
                        ..
                    } => Some(callback.clone()),
                    _ => None,
                })
                .unwrap();
            let operation = operation_mut(mir.functions.get_mut(&operations).unwrap(), |kind| {
                matches!(kind, MirOperationKind::Format { display: None, .. })
            });
            let MirOperationKind::Format { display, .. } = &mut operation.kind else {
                unreachable!()
            };
            *display = Some(callback);
        });
        assert!(
            error
                .message()
                .contains("intrinsic Display format operation must not carry a callback"),
            "{error}"
        );

        let error = corrupted_mir(SOURCE, |resolved, _hir, mir| {
            let operations = MirFunctionId::Callable(callable_named(resolved, "operations"));
            let operation = operation_mut(mir.functions.get_mut(&operations).unwrap(), |kind| {
                matches!(
                    kind,
                    MirOperationKind::Format {
                        display: Some(_),
                        ..
                    }
                )
            });
            let MirOperationKind::Format { display, .. } = &mut operation.kind else {
                unreachable!()
            };
            *display = None;
        });
        assert!(
            error
                .message()
                .contains("non-intrinsic Display format operation is missing its callback"),
            "{error}"
        );

        let error = corrupted_mir(SOURCE, |resolved, hir, mir| {
            let operations = MirFunctionId::Callable(callable_named(resolved, "operations"));
            let operation = operation_mut(mir.functions.get_mut(&operations).unwrap(), |kind| {
                matches!(
                    kind,
                    MirOperationKind::Format {
                        display: Some(_),
                        ..
                    }
                )
            });
            let MirOperationKind::Format { display, .. } = &mut operation.kind else {
                unreachable!()
            };
            let callback = display.as_mut().unwrap();
            callback.ty = hir.interner().scalar(ScalarType::String);
            callback.kind = MirOperandKind::Constant(MirConstant::String("wrong".into()));
        });
        assert!(
            error
                .message()
                .contains("format Display callback does not match its target"),
            "{error}"
        );
    }

    #[test]
    fn testing_host_operation_contracts_accept_every_typed_shape() {
        const SOURCE: &str = "import std.console\n\
             import std.bytes\n\
             fn operations(\n\
                 text: String,\n\
                 tags: Map[String, String],\n\
                 payload: bytes.Bytes,\n\
             ) { console.print(text) }\n";
        for function in [
            MirBootstrapHostFunction::TestingLog,
            MirBootstrapHostFunction::TestingFailNow,
            MirBootstrapHostFunction::TestingSkip,
            MirBootstrapHostFunction::TestingTags,
            MirBootstrapHostFunction::TestingAttach,
            MirBootstrapHostFunction::TestingSnapshot,
        ] {
            let (resolved, hir, mut mir) = checked_mir(SOURCE);
            let operations = MirFunctionId::Callable(callable_named(&resolved, "operations"));
            let parameters = mir.functions[&operations].parameters.clone();
            let string = hir.interner().scalar(ScalarType::String);
            let outcome = if matches!(
                function,
                MirBootstrapHostFunction::TestingFailNow | MirBootstrapHostFunction::TestingSkip
            ) {
                hir.interner().scalar(ScalarType::Never)
            } else {
                hir.interner().scalar(ScalarType::Unit)
            };
            let string_operand = || MirOperand {
                ty: string,
                kind: MirOperandKind::Constant(MirConstant::String("text".into())),
            };
            let copied_parameter = |index: usize| {
                let local = parameters[index];
                let ty = mir.functions[&operations].locals[local.0 as usize].ty;
                MirOperand {
                    ty,
                    kind: MirOperandKind::Copy(MirPlace {
                        local,
                        ty,
                        projections: Vec::new(),
                        source_loan: None,
                    }),
                }
            };
            let arguments = match function {
                MirBootstrapHostFunction::TestingLog
                | MirBootstrapHostFunction::TestingFailNow
                | MirBootstrapHostFunction::TestingSkip => vec![string_operand()],
                MirBootstrapHostFunction::TestingTags => vec![copied_parameter(1)],
                MirBootstrapHostFunction::TestingAttach => {
                    vec![string_operand(), string_operand(), copied_parameter(2)]
                }
                MirBootstrapHostFunction::TestingSnapshot => {
                    vec![string_operand(), string_operand()]
                }
                _ => unreachable!(),
            };
            let operation = operation_mut(mir.functions.get_mut(&operations).unwrap(), |kind| {
                matches!(kind, MirOperationKind::BootstrapHostCall { .. })
            });
            operation.ty = outcome;
            operation.kind = MirOperationKind::BootstrapHostCall {
                function,
                arguments,
            };
            if outcome == hir.interner().scalar(ScalarType::Never) {
                let block = mir
                    .functions
                    .get_mut(&operations)
                    .unwrap()
                    .blocks
                    .iter_mut()
                    .find(|block| {
                        matches!(block.terminator.kind, MirTerminatorKind::Invoke {
                        operation: MirOperation {
                            kind: MirOperationKind::BootstrapHostCall { function: actual, .. },
                            ..
                        },
                        ..
                    } if actual == function)
                    })
                    .unwrap();
                let MirTerminatorKind::Invoke {
                    destination,
                    target,
                    ..
                } = &mut block.terminator.kind
                else {
                    unreachable!()
                };
                let abandoned = *target;
                *destination = None;
                *target = None;
                if let Some(abandoned) = abandoned {
                    let block = &mut mir.functions.get_mut(&operations).unwrap().blocks
                        [abandoned.0 as usize];
                    block.statements.clear();
                    block.terminator.kind = MirTerminatorKind::Unreachable;
                }
            }
            verify_mir(&resolved, &hir, &mir).unwrap();
        }
    }

    #[test]
    fn process_output_redirections_are_verified_at_the_mir_boundary() {
        const SOURCE: &str = "import std.process\n\
             fn main(): !(process.ProcessError) {\n\
                 let command = process.command(\"/usr/bin/true\")\n\
                 let merged_command = command.mergeStderr()\n\
                 let pipeline = process.command(\"/usr/bin/printf\", \"x\") | process.command(\"/bin/cat\")\n\
                 let merged_pipeline = pipeline.mergeStderr()\n\
                 let output = merged_command.output()?\n\
                 let combined = output.combined\n\
                 _ = merged_pipeline\n\
                 _ = combined\n\
             }\n";

        let (resolved, hir, mir) = checked_mir(SOURCE);
        verify_mir(&resolved, &hir, &mir).unwrap();
    }

    #[test]
    fn testing_suite_boundary_contracts_reject_every_invalid_shape() {
        const SOURCE: &str = "import std.console\n\
             fn operations(\n\
                 text: String,\n\
                 body: fn() suspends,\n\
                 syncBody: fn(),\n\
                 argumentBody: fn(Int) suspends,\n\
                 valueBody: fn(): Int suspends,\n\
                 number: Int,\n\
             ) { console.print(text) }\n";
        let cases = [
            (
                MirBootstrapHostFunction::TestingRunLeaf,
                &[0, 1][..],
                ScalarType::Unit,
                true,
            ),
            (
                MirBootstrapHostFunction::TestingRunSuite,
                &[0, 1][..],
                ScalarType::Unit,
                true,
            ),
            (
                MirBootstrapHostFunction::TestingBeginSuiteCleanup,
                &[][..],
                ScalarType::Unit,
                true,
            ),
            (
                MirBootstrapHostFunction::TestingRunLeaf,
                &[0][..],
                ScalarType::Unit,
                false,
            ),
            (
                MirBootstrapHostFunction::TestingRunLeaf,
                &[5, 1][..],
                ScalarType::Unit,
                false,
            ),
            (
                MirBootstrapHostFunction::TestingRunLeaf,
                &[0, 2][..],
                ScalarType::Unit,
                false,
            ),
            (
                MirBootstrapHostFunction::TestingRunLeaf,
                &[0, 3][..],
                ScalarType::Unit,
                false,
            ),
            (
                MirBootstrapHostFunction::TestingRunLeaf,
                &[0, 4][..],
                ScalarType::Unit,
                false,
            ),
            (
                MirBootstrapHostFunction::TestingRunLeaf,
                &[0, 1][..],
                ScalarType::Int,
                false,
            ),
            (
                MirBootstrapHostFunction::TestingBeginSuiteCleanup,
                &[0][..],
                ScalarType::Unit,
                false,
            ),
            (
                MirBootstrapHostFunction::TestingBeginSuiteCleanup,
                &[][..],
                ScalarType::Int,
                false,
            ),
        ];

        for (index, (host_function, argument_parameters, outcome, valid)) in
            cases.into_iter().enumerate()
        {
            let (resolved, hir, mut mir) = checked_mir(SOURCE);
            let operations = MirFunctionId::Callable(callable_named(&resolved, "operations"));
            let parameters = mir.functions[&operations].parameters.clone();
            let arguments = argument_parameters
                .iter()
                .map(|parameter| {
                    let local = parameters[*parameter];
                    let ty = mir.functions[&operations].locals[local.0 as usize].ty;
                    MirOperand {
                        ty,
                        kind: MirOperandKind::Copy(MirPlace {
                            local,
                            ty,
                            projections: Vec::new(),
                            source_loan: None,
                        }),
                    }
                })
                .collect();
            let operation = operation_mut(mir.functions.get_mut(&operations).unwrap(), |kind| {
                matches!(kind, MirOperationKind::BootstrapHostCall { .. })
            });
            operation.ty = hir.interner().scalar(outcome);
            operation.kind = MirOperationKind::BootstrapHostCall {
                function: host_function,
                arguments,
            };

            let result = verify_mir(&resolved, &hir, &mir);
            if valid {
                result.unwrap_or_else(|error| panic!("valid suite boundary case {index}: {error}"));
            } else {
                let error = result.unwrap_err();
                assert!(
                    error
                        .message()
                        .contains("bootstrap host operation does not match its closed contract"),
                    "invalid suite boundary case {index}: {error}"
                );
            }
        }
    }

    #[test]
    fn aggregate_corruption_matrix_rejects_every_closed_shape_contract() {
        const SOURCE: &str = "type UserId = Int\n\
             type Record = { value: Int, text: String }\n\
             enum Choice {\n\
                 Empty\n\
                 Item(Int)\n\
                 Named { value: Int }\n\
             }\n\
             fn catalog(): Int8 ! NumericConversionError {\n\
                 let tuple = (1, \"text\")\n\
                 let array = [1, 2]\n\
                 let set = Set[1, 2]\n\
                 let identifier = UserId(1)\n\
                 let reference = Ref(1)\n\
                 let record = Record { value: 1, text: \"text\" }\n\
                 let empty = Choice.Empty\n\
                 let item = Choice.Item(1)\n\
                 let named = Choice.Named { value: 1 }\n\
                 let missing: Int? = none\n\
                 let present: Int? = some(1)\n\
                 let success: Int ! String = ok(1)\n\
                 let failure: Int ! String = err(\"error\")\n\
                 let conversionError = NumericConversionError.OutOfRange\n\
                 let closure = (value: Int): Int { value }\n\
                 _ = tuple\n\
                 _ = array\n\
                 _ = set\n\
                 _ = identifier\n\
                 _ = reference\n\
                 _ = record\n\
                 _ = empty\n\
                 _ = item\n\
                 _ = named\n\
                 _ = missing\n\
                 _ = present\n\
                 _ = success\n\
                 _ = failure\n\
                 _ = conversionError\n\
                 _ = closure\n\
                 Int8(128)\n\
             }\n";

        let error = corrupted_mir(SOURCE, |resolved, hir, mir| {
            let catalog = MirFunctionId::Callable(callable_named(resolved, "catalog"));
            aggregate_rvalue_mut(mir.functions.get_mut(&catalog).unwrap(), |shape| {
                matches!(shape, MirAggregateKind::Tuple)
            })
            .ty = hir.interner().scalar(ScalarType::Int);
        });
        assert!(error.message().contains("non-tuple type"), "{error}");

        for is_set in [false, true] {
            let error = corrupted_mir(SOURCE, |resolved, hir, mir| {
                let catalog = MirFunctionId::Callable(callable_named(resolved, "catalog"));
                let value =
                    aggregate_rvalue_mut(mir.functions.get_mut(&catalog).unwrap(), |shape| {
                        matches!(shape, MirAggregateKind::Set) == is_set
                            && matches!(shape, MirAggregateKind::Array) != is_set
                    });
                let MirRvalueKind::Aggregate { values, .. } = &mut value.kind else {
                    unreachable!()
                };
                values[0] = MirOperand {
                    ty: hir.interner().scalar(ScalarType::String),
                    kind: MirOperandKind::Constant(MirConstant::String("\"wrong\"".into())),
                };
            });
            assert!(error.message().contains("wrong element type"), "{error}");
        }

        let error = corrupted_mir(SOURCE, |resolved, _hir, mir| {
            let catalog = MirFunctionId::Callable(callable_named(resolved, "catalog"));
            let value = aggregate_rvalue_mut(mir.functions.get_mut(&catalog).unwrap(), |shape| {
                matches!(shape, MirAggregateKind::Newtype { .. })
            });
            let MirRvalueKind::Aggregate { values, .. } = &mut value.kind else {
                unreachable!()
            };
            values.clear();
        });
        assert!(
            error.message().contains("newtype aggregate owner"),
            "{error}"
        );

        let error = corrupted_mir(SOURCE, |resolved, _hir, mir| {
            let catalog = MirFunctionId::Callable(callable_named(resolved, "catalog"));
            let value = aggregate_rvalue_mut(mir.functions.get_mut(&catalog).unwrap(), |shape| {
                matches!(shape, MirAggregateKind::Ref)
            });
            let MirRvalueKind::Aggregate { values, .. } = &mut value.kind else {
                unreachable!()
            };
            values.clear();
        });
        assert!(error.message().contains("operand arity"), "{error}");

        let error = corrupted_mir(SOURCE, |resolved, _hir, mir| {
            let catalog = MirFunctionId::Callable(callable_named(resolved, "catalog"));
            let value = aggregate_rvalue_mut(mir.functions.get_mut(&catalog).unwrap(), |shape| {
                matches!(shape, MirAggregateKind::Record { .. })
            });
            let MirRvalueKind::Aggregate { shape, .. } = &mut value.kind else {
                unreachable!()
            };
            let MirAggregateKind::Record { fields, .. } = shape else {
                unreachable!()
            };
            fields.pop();
        });
        assert!(
            error.message().contains("record aggregate owner"),
            "{error}"
        );

        for (unit, record) in [(true, false), (false, false), (false, true)] {
            let error = corrupted_mir(SOURCE, |resolved, hir, mir| {
                let catalog = MirFunctionId::Callable(callable_named(resolved, "catalog"));
                let value =
                    aggregate_rvalue_mut(mir.functions.get_mut(&catalog).unwrap(), |shape| {
                        matches!(
                            shape,
                            MirAggregateKind::Variant { fields, .. }
                                if fields.is_empty() == unit
                                    && fields.first().is_some_and(Option::is_some) == record
                        )
                    });
                let MirRvalueKind::Aggregate { values, .. } = &mut value.kind else {
                    unreachable!()
                };
                if unit {
                    values.push(MirOperand {
                        ty: hir.interner().scalar(ScalarType::Int),
                        kind: MirOperandKind::Constant(MirConstant::Integer("1".into())),
                    });
                } else {
                    values.clear();
                }
            });
            assert!(error.message().contains("variant"), "{error}");
        }

        for shape_name in ["none", "some", "ok", "err"] {
            let error = corrupted_mir(SOURCE, |resolved, hir, mir| {
                let catalog = MirFunctionId::Callable(callable_named(resolved, "catalog"));
                let value =
                    aggregate_rvalue_mut(mir.functions.get_mut(&catalog).unwrap(), |shape| {
                        match shape_name {
                            "none" => matches!(shape, MirAggregateKind::OptionNone),
                            "some" => matches!(shape, MirAggregateKind::OptionSome),
                            "ok" => matches!(shape, MirAggregateKind::ResultOk),
                            "err" => matches!(shape, MirAggregateKind::ResultErr),
                            _ => unreachable!(),
                        }
                    });
                let MirRvalueKind::Aggregate { values, .. } = &mut value.kind else {
                    unreachable!()
                };
                if values.is_empty() {
                    values.push(MirOperand {
                        ty: hir.interner().scalar(ScalarType::Int),
                        kind: MirOperandKind::Constant(MirConstant::Integer("1".into())),
                    });
                } else {
                    values.clear();
                }
            });
            assert!(
                error.message().contains("aggregate") || error.message().contains("operand arity"),
                "{shape_name}: {error}"
            );
        }

        let error = corrupted_mir(SOURCE, |resolved, hir, mir| {
            let catalog = MirFunctionId::Callable(callable_named(resolved, "catalog"));
            let value = aggregate_rvalue_mut(mir.functions.get_mut(&catalog).unwrap(), |shape| {
                matches!(shape, MirAggregateKind::NumericConversionError(_))
            });
            let MirRvalueKind::Aggregate { values, .. } = &mut value.kind else {
                unreachable!()
            };
            values.push(MirOperand {
                ty: hir.interner().scalar(ScalarType::Int),
                kind: MirOperandKind::Constant(MirConstant::Integer("1".into())),
            });
        });
        assert!(
            error
                .message()
                .contains("numeric conversion error aggregate"),
            "{error}"
        );

        let error = corrupted_mir(SOURCE, |resolved, hir, mir| {
            let catalog = MirFunctionId::Callable(callable_named(resolved, "catalog"));
            let value = aggregate_rvalue_mut(mir.functions.get_mut(&catalog).unwrap(), |shape| {
                matches!(shape, MirAggregateKind::Closure { .. })
            });
            let MirRvalueKind::Aggregate { shape, .. } = &mut value.kind else {
                unreachable!()
            };
            let MirAggregateKind::Closure { arguments, .. } = shape else {
                unreachable!()
            };
            arguments.push(hir.interner().scalar(ScalarType::Int));
        });
        assert!(error.message().contains("wrong generic arity"), "{error}");
    }

    #[test]
    fn function_metadata_corruption_matrix_is_rejected_before_bytecode() {
        const SOURCE: &str = "fn combine(left: Int, right: Int): Int {\n\
                 let total = left + right\n\
                 total\n\
             }\n\
             fn main() {}\n";

        let error = corrupted_mir(SOURCE, |resolved, _hir, mir| {
            mir.functions
                .remove(&MirFunctionId::Callable(callable_named(
                    resolved, "combine",
                )));
        });
        assert!(error.message().contains("function set"), "{error}");

        let error = corrupted_mir(SOURCE, |resolved, _hir, mir| {
            let combine = MirFunctionId::Callable(callable_named(resolved, "combine"));
            let main = MirFunctionId::Callable(callable_named(resolved, "main"));
            mir.functions.get_mut(&combine).unwrap().id = main;
        });
        assert!(error.message().contains("map key differs"), "{error}");

        let error = corrupted_mir(SOURCE, |resolved, hir, mir| {
            let combine = MirFunctionId::Callable(callable_named(resolved, "combine"));
            mir.functions.get_mut(&combine).unwrap().outcome =
                hir.interner().scalar(ScalarType::String);
        });
        assert!(error.message().contains("typed HIR requires"), "{error}");

        let error = corrupted_mir(SOURCE, |resolved, _hir, mir| {
            let combine = MirFunctionId::Callable(callable_named(resolved, "combine"));
            mir.functions.get_mut(&combine).unwrap().locals.clear();
        });
        assert!(error.message().contains("local table is empty"), "{error}");

        let error = corrupted_mir(SOURCE, |resolved, _hir, mir| {
            let combine = MirFunctionId::Callable(callable_named(resolved, "combine"));
            let function = mir.functions.get_mut(&combine).unwrap();
            function.return_local = MirLocalId(u32::MAX);
        });
        assert!(error.message().contains("unknown MIR local"), "{error}");

        let error = corrupted_mir(SOURCE, |resolved, _hir, mir| {
            let combine = MirFunctionId::Callable(callable_named(resolved, "combine"));
            let function = mir.functions.get_mut(&combine).unwrap();
            function.locals[function.return_local.index() as usize].kind = MirLocalKind::Temporary;
        });
        assert!(
            error.message().contains("return local kind or type"),
            "{error}"
        );

        let error = corrupted_mir(SOURCE, |resolved, _hir, mir| {
            let combine = MirFunctionId::Callable(callable_named(resolved, "combine"));
            mir.functions.get_mut(&combine).unwrap().parameters.pop();
        });
        assert!(error.message().contains("MIR parameters"), "{error}");

        let error = corrupted_mir(SOURCE, |resolved, _hir, mir| {
            let combine = MirFunctionId::Callable(callable_named(resolved, "combine"));
            let function = mir.functions.get_mut(&combine).unwrap();
            function.parameters[1] = function.parameters[0];
        });
        assert!(error.message().contains("is repeated"), "{error}");

        let error = corrupted_mir(SOURCE, |resolved, _hir, mir| {
            let combine = MirFunctionId::Callable(callable_named(resolved, "combine"));
            let function = mir.functions.get_mut(&combine).unwrap();
            let parameter = function.parameters[0];
            function.locals[parameter.index() as usize].kind = MirLocalKind::Temporary;
        });
        assert!(
            error.message().contains("parameter 0 local metadata"),
            "{error}"
        );

        let error = corrupted_mir(SOURCE, |resolved, _hir, mir| {
            let combine = MirFunctionId::Callable(callable_named(resolved, "combine"));
            let function = mir.functions.get_mut(&combine).unwrap();
            let source = function
                .locals
                .iter()
                .find_map(|local| match local.kind {
                    MirLocalKind::User(source) => Some(source),
                    _ => None,
                })
                .expect("combine has one user binding");
            let temporary = function
                .locals
                .iter()
                .position(|local| local.kind == MirLocalKind::Temporary)
                .expect("combine has one temporary");
            function.locals[temporary].kind = MirLocalKind::User(source);
        });
        assert!(
            error
                .message()
                .contains("inconsistent or duplicate source identity"),
            "{error}"
        );

        let error = corrupted_mir(SOURCE, |resolved, _hir, mir| {
            let combine = MirFunctionId::Callable(callable_named(resolved, "combine"));
            let function = mir.functions.get_mut(&combine).unwrap();
            let temporary = function
                .locals
                .iter()
                .position(|local| local.kind == MirLocalKind::Temporary)
                .expect("combine has one temporary");
            function.locals[temporary].kind = MirLocalKind::Return;
        });
        assert!(
            error.message().contains("return locals instead of one"),
            "{error}"
        );

        let error = corrupted_mir(SOURCE, |resolved, _hir, mir| {
            let combine = MirFunctionId::Callable(callable_named(resolved, "combine"));
            mir.functions.get_mut(&combine).unwrap().blocks.clear();
        });
        assert!(
            error.message().contains("basic-block table is empty"),
            "{error}"
        );

        let error = corrupted_mir(SOURCE, |resolved, _hir, mir| {
            let combine = MirFunctionId::Callable(callable_named(resolved, "combine"));
            let function = mir.functions.get_mut(&combine).unwrap();
            function.entry = function.unwind;
        });
        assert!(error.message().contains("entry and unwind"), "{error}");

        let error = corrupted_mir(SOURCE, |resolved, _hir, mir| {
            let combine = MirFunctionId::Callable(callable_named(resolved, "combine"));
            let function = mir.functions.get_mut(&combine).unwrap();
            function.blocks[function.entry.index() as usize].kind = MirBlockKind::Cleanup;
        });
        assert!(
            error.message().contains("entry block is cleanup"),
            "{error}"
        );

        let error = corrupted_mir(SOURCE, |resolved, _hir, mir| {
            let combine = MirFunctionId::Callable(callable_named(resolved, "combine"));
            let function = mir.functions.get_mut(&combine).unwrap();
            function.blocks[function.unwind.index() as usize].kind = MirBlockKind::Normal;
        });
        assert!(error.message().contains("unwind entry"), "{error}");

        let error = corrupted_mir(SOURCE, |resolved, _hir, mir| {
            let combine = MirFunctionId::Callable(callable_named(resolved, "combine"));
            let function = mir.functions.get_mut(&combine).unwrap();
            let temporary = function
                .locals
                .iter()
                .position(|local| local.kind == MirLocalKind::Temporary)
                .expect("combine has one temporary");
            function.locals[temporary].kind = MirLocalKind::Parameter {
                index: 99,
                source: None,
            };
        });
        assert!(
            error.message().contains("is an unlisted parameter"),
            "{error}"
        );

        const LOAN_SOURCE: &str = "fn observe(value: ref Int): Int { value }\n\
             fn use(value: Int): Int { observe(ref value) }\n";
        let error = corrupted_mir(LOAN_SOURCE, |resolved, _hir, mir| {
            let use_function = MirFunctionId::Callable(callable_named(resolved, "use"));
            mir.functions.get_mut(&use_function).unwrap().loans[0].mode = ParameterMode::Value;
        });
        assert!(
            error.message().contains("uses the owning value mode"),
            "{error}"
        );

        let error = corrupted_mir(LOAN_SOURCE, |resolved, _hir, mir| {
            let use_function = MirFunctionId::Callable(callable_named(resolved, "use"));
            let function = mir.functions.get_mut(&use_function).unwrap();
            function.loans[0].place.source_loan = Some(MirLoanId(0));
        });
        assert!(
            error
                .message()
                .contains("not an earlier acyclic reservation"),
            "{error}"
        );
    }
}
