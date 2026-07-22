use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::error::Error;
use std::fmt;

use crate::hir::{
    CapabilityAnalysis, CapabilityAssumptions, HirBinaryOperator, HirCallProtocol, HirCallableId,
    HirCapability, HirCapabilityStatus, HirClosureProtocols, HirConstantValueKind,
    HirContainmentKind, HirGenericParameter, HirIndexAccess, HirNominalShape, HirPrefixOperator,
    HirProgram, HirTraitConstructor, HirTypeDeclarationKind, HirVariantPayload,
    StaticCollectionRegion, StaticRegionRelation, StaticSlice, parse_nonnegative_integer,
    static_collection_relation,
};
use crate::resolve::{MemberKind, MemberOwner, ResolvedProgram, SymbolId};
use crate::types::{
    Assignability, CursorMode, IntrinsicType, NumericConversion, ParameterMode, ScalarType, TypeId,
    TypeKind, TypeSubstitution, numeric_conversion,
};

use super::{
    MirAggregateKind, MirBasicBlock, MirBlockId, MirBlockKind, MirConstant, MirFunction,
    MirFunctionId, MirLoanId, MirLoanKind, MirLocalId, MirLocalKind, MirOperand, MirOperandKind,
    MirOperation, MirOperationKind, MirPlace, MirProgram, MirProjection, MirProjectionKind,
    MirRvalue, MirRvalueKind, MirStatementKind, MirTag, MirTerminatorKind,
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
    outcome: TypeId,
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
    let verifier = Verifier {
        resolved,
        hir,
        capability_analysis,
        capability_statuses: RefCell::new(BTreeMap::new()),
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
    capability_statuses:
        RefCell<BTreeMap<(MirFunctionId, TypeId, HirCapability), HirCapabilityStatus>>,
    limits: MirVerificationLimits,
    dataflow_steps: Cell<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LocalEvent {
    Read(LocalAccess),
    Move(LocalAccess),
    Write(LocalAccess),
    WriteAccess(LocalAccess),
    StorageLive(MirLocalId),
    StorageDead(MirLocalId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LoanEvent {
    Local(LocalEvent),
    Reserve(MirLoanId),
    Release(MirLoanId),
    Consume(Vec<MirLoanId>),
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum MovePathComponent {
    ClosureCapture(crate::hir::HirClosureId, u32),
    Field(crate::resolve::MemberId),
    TupleField(u32),
    NewtypeValue,
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
    Index(MirLocalId),
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
            MirProjectionKind::Index { index, .. } => Self::Index(*index),
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
        MirOperationKind::BuildMap { entries, .. } => entries
            .iter()
            .any(|(key, value)| escapes(key) || escapes(value)),
        MirOperationKind::Index { base, index, .. } => mir_operand_is_loan(base) || escapes(index),
        MirOperationKind::Slice {
            base,
            start,
            end,
            step,
        } => mir_operand_is_loan(base) || start.iter().chain(end).chain(step).any(escapes),
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
    }
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
                MirLoanKind::Region if loan.mode == ParameterMode::Ref => {}
                MirLoanKind::Region => {
                    return Err(MirInvariantError::new(
                        &loan_context,
                        "region loan is not a shared `ref` reservation",
                    ));
                }
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
        if let MirFunctionId::Closure(closure) = function.id {
            self.verify_closure_protocols(function, closure, &context)?;
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
                MirStatementKind::Assign { destination, value } => {
                    self.verify_place(function, destination, &context)?;
                    self.verify_rvalue(function, value, &context)?;
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
                self.verify_operation(function, operation, &context)?;
                let never = self.hir.interner().scalar(ScalarType::Never);
                match (destination, target) {
                    (Some(destination), Some(target)) => {
                        self.verify_place(function, destination, &context)?;
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
            MirTerminatorKind::IteratorNext {
                state,
                destination,
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
                self.verify_iterator(state.ty, destination.ty, &context)?;
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
                for_write,
                target,
                unwind,
            } => {
                if block.kind != MirBlockKind::Normal
                    || places.is_empty()
                    || places.len() != replacements.len()
                {
                    return Err(MirInvariantError::new(
                        &context,
                        "place validation must be a non-empty aligned ordinary operation",
                    ));
                }
                let mut unique = Vec::new();
                for (place, replacement) in places.iter().zip(replacements) {
                    self.verify_place(function, place, &context)?;
                    if unique.contains(&place) {
                        return Err(MirInvariantError::new(
                            &context,
                            "place validation repeats the same destination",
                        ));
                    }
                    unique.push(place);
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
            MirRvalueKind::Length(operand) => {
                self.verify_operand(function, operand, context)?;
                if value.ty != self.hir.interner().scalar(ScalarType::Int)
                    || !self.is_array(operand.ty)
                {
                    return Err(MirInvariantError::new(
                        context,
                        "length requires Array and produces Int",
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
                    || (*mode == CursorMode::Ref) != borrows
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

    fn verify_operation(
        &self,
        function: &MirFunction,
        operation: &MirOperation,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        if mir_operation_contains_invalid_borrow(operation) {
            return Err(MirInvariantError::new(
                context,
                "borrow escapes its permitted immediate operation",
            ));
        }
        self.verify_type(operation.ty, context)?;
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
            } => {
                self.verify_operand(function, base, context)?;
                self.verify_operand(function, index, context)?;
                self.verify_index_result(base.ty, index.ty, *access, operation.ty, context)?;
            }
            MirOperationKind::Slice {
                base,
                start,
                end,
                step,
            } => {
                self.verify_operand(function, base, context)?;
                for operand in start.iter().chain(end).chain(step) {
                    self.verify_operand(function, operand, context)?;
                    if operand.ty != self.hir.interner().scalar(ScalarType::Int) {
                        return Err(MirInvariantError::new(context, "slice bound is not Int"));
                    }
                }
                if operation.ty != base.ty || !self.is_array(base.ty) {
                    return Err(MirInvariantError::new(
                        context,
                        "slice operation must preserve its Array type",
                    ));
                }
            }
            MirOperationKind::Call {
                callee,
                arguments,
                signature,
                protocol,
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
                        outcome: operation.ty,
                    },
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
                if !matches!(host_function, super::MirBootstrapHostFunction::ConsolePrint)
                    || arguments.len() != 1
                    || arguments[0].ty != self.hir.interner().scalar(ScalarType::String)
                    || operation.ty != self.hir.interner().scalar(ScalarType::Unit)
                {
                    return Err(MirInvariantError::new(
                        context,
                        "bootstrap console print requires one String and produces Unit",
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
        for projection in &place.projections {
            self.verify_type(projection.ty, context)?;
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
            if source.kind != MirLoanKind::Region || source.mode != ParameterMode::Ref {
                return Err(MirInvariantError::new(
                    context,
                    "place source is not a shared region loan",
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
        let signature = TypeSubstitution::new(arguments.clone())
            .apply(&mut interner, closure.function_type())
            .map_err(|error| MirInvariantError::new(context, error.to_string()))?;
        Ok(signature == expected
            && closure
                .protocols()
                .supports(crate::hir::HirCallProtocol::Call))
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
        _right: TypeId,
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
        ) && !matches!(
            self.hir.interner().kind(left),
            Ok(TypeKind::Scalar(ScalarType::Float | ScalarType::Float32))
        )
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
            MirProjectionKind::Index { index, access } => {
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

    fn verify_call(
        &self,
        function: &MirFunction,
        verification: MirCallVerification<'_>,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        let MirCallVerification {
            callee,
            arguments,
            signature,
            protocol,
            outcome,
        } = verification;
        let TypeKind::Function(call_signature) = self.kind(signature, context)? else {
            return Err(MirInvariantError::new(
                context,
                "call operation signature is not a function",
            ));
        };
        if call_signature.is_async() || call_signature.is_unsafe() {
            return Err(MirInvariantError::new(
                context,
                "effectful call reached the synchronous safe MIR call operation",
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
        let expected_protocol = if available.supports(HirCallProtocol::Call) {
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
        if matches!(callee.kind, MirOperandKind::PreludeTraitFunction { .. }) {
            if call_signature.variadic().is_some() || call_signature.parameters().len() != 1 {
                return Err(MirInvariantError::new(
                    context,
                    "prelude trait callable does not have exactly one fixed receiver",
                ));
            }
            let parameter = &call_signature.parameters()[0];
            receiver = Some((
                crate::hir::HirCallArgumentTarget::Receiver,
                parameter.mode(),
                parameter.ty(),
            ));
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
        let static_integers = static_integer_locals(self.hir, function);
        let mut reservations = vec![0_u32; function.loans.len()];
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
        for index in 0..function.loans.len() {
            let loan = &function.loans[index];
            let valid_consumptions = match loan.kind {
                MirLoanKind::CallLocal => consumptions[index] <= 1,
                MirLoanKind::Region => consumptions[index] == 0,
            };
            if reservations[index] != 1 || !valid_consumptions {
                return Err(MirInvariantError::new(
                    format!("{context} loan#{index}"),
                    format!(
                        "has {} reservations and {} call consumptions, which violates its {:?} contract",
                        reservations[index], consumptions[index], loan.kind
                    ),
                ));
            }
        }

        let mut incoming = vec![None::<BTreeSet<MirLoanId>>; function.blocks.len()];
        incoming[function.entry.index() as usize] = Some(BTreeSet::new());
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
            let mut propagate = |target: MirBlockId,
                                 edge_state: BTreeSet<MirLoanId>|
             -> Result<(), MirInvariantError> {
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
                    destination,
                    target,
                    unwind,
                    ..
                } => {
                    if let Some(target) = target {
                        let normal = state.clone();
                        if let Some(destination) = destination {
                            self.verify_loan_local_access(
                                function,
                                &static_integers,
                                &normal,
                                &LocalEvent::Write(LocalAccess::from_place(destination)),
                                &block_context,
                            )?;
                        }
                        propagate(*target, normal)?;
                    }
                    propagate(*unwind, BTreeSet::new())?;
                }
                MirTerminatorKind::IteratorNext {
                    destination,
                    has_value,
                    exhausted,
                    unwind,
                    ..
                } => {
                    let has_value_state = state.clone();
                    self.verify_loan_local_access(
                        function,
                        &static_integers,
                        &has_value_state,
                        &LocalEvent::Write(LocalAccess::from_place(destination)),
                        &block_context,
                    )?;
                    propagate(*has_value, has_value_state)?;
                    propagate(*exhausted, state)?;
                    propagate(*unwind, BTreeSet::new())?;
                }
                MirTerminatorKind::ValidatePlaces { target, unwind, .. } => {
                    propagate(*target, state)?;
                    propagate(*unwind, BTreeSet::new())?;
                }
                MirTerminatorKind::Return => {
                    if !state.is_empty() {
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
        state: &mut BTreeSet<MirLoanId>,
        event: &LoanEvent,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        match event {
            LoanEvent::Local(event) => {
                self.verify_loan_local_access(function, static_integers, state, event, context)
            }
            LoanEvent::Reserve(id) => {
                let loan = self.loan(function, *id, context)?;
                self.verify_reborrow_mode(function, loan, context)?;
                let access = LocalAccess::from_place(loan.place());
                if state.contains(id) {
                    return Err(MirInvariantError::new(
                        context,
                        format!("reserves already-active loan#{}", id.index()),
                    ));
                }
                for active in state.iter().copied() {
                    let existing = self.loan(function, active, context)?;
                    let existing_access = LocalAccess::from_place(existing.place());
                    if access.local == existing_access.local
                        && loan_paths_overlap(&access.path, &existing_access.path, static_integers)
                        && !(loan.mode() == ParameterMode::Ref
                            && existing.mode() == ParameterMode::Ref)
                    {
                        return Err(MirInvariantError::new(
                            context,
                            format!(
                                "loan#{} overlaps incompatible active loan#{}",
                                id.index(),
                                active.index()
                            ),
                        ));
                    }
                }
                state.insert(*id);
                Ok(())
            }
            LoanEvent::Release(loan) => {
                if !state.contains(loan) {
                    return Err(MirInvariantError::new(
                        context,
                        format!("releases inactive loan#{}", loan.index()),
                    ));
                }
                if let Some(dependent) =
                    self.active_dependent_loan(function, state, *loan, context)?
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
                state.remove(loan);
                Ok(())
            }
            LoanEvent::Consume(loans) => {
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
                        state,
                        &LocalAccess::from_place(&metadata.place),
                        "read",
                        context,
                    )?;
                    if !seen.insert(*loan) || !state.remove(loan) {
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
        context: &str,
    ) -> Result<(), MirInvariantError> {
        let (access, access_kind) = match event {
            LocalEvent::Read(access) => (Some(access), "read"),
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
                    &access,
                    "storage change",
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
            access,
            access_kind,
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
        if access_kind != "read" {
            return Err(MirInvariantError::new(
                context,
                format!("{access_kind} uses a shared region reference"),
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
            if loan.kind != MirLoanKind::Region || loan.mode != ParameterMode::Ref {
                return Err(MirInvariantError::new(
                    context,
                    "place source is not a shared region loan",
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
        access: &LocalAccess,
        access_kind: &str,
        context: &str,
    ) -> Result<(), MirInvariantError> {
        for active in state.iter().copied() {
            let loan = self.loan(function, active, context)?;
            let loan_access = LocalAccess::from_place(loan.place());
            if access.local == loan_access.local
                && loan_paths_overlap(&access.path, &loan_access.path, static_integers)
                && !(access_kind == "read" && loan.mode() == ParameterMode::Ref)
            {
                return Err(MirInvariantError::new(
                    context,
                    format!(
                        "{access_kind} overlaps active loan#{} ({:?})",
                        active.index(),
                        loan.mode()
                    ),
                ));
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
            MirFunctionId::Closure(closure) if index == 0 => Some(ParameterMode::Value),
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
                    LocalEvent::Read(access) if access.local == local => {
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
            }
        }
        match &block.terminator.kind {
            MirTerminatorKind::Goto { .. }
            | MirTerminatorKind::ResumePanic
            | MirTerminatorKind::Unreachable => {}
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
            MirTerminatorKind::IteratorNext {
                state, destination, ..
            } => {
                push_place_events(state, true, &mut events);
                push_destination_reads(destination, true, &mut events);
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
                    push_place_events(loan.place(), true, &mut local);
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
        }
    }
    let mut local = Vec::new();
    match &block.terminator.kind {
        MirTerminatorKind::Goto { .. }
        | MirTerminatorKind::ResumePanic
        | MirTerminatorKind::Unreachable => {}
        MirTerminatorKind::SwitchBool { condition, .. } => {
            push_operand_events(condition, &mut local);
        }
        MirTerminatorKind::SwitchTag { value, .. } => {
            push_operand_events(value, &mut local);
        }
        MirTerminatorKind::Invoke { operation, .. } => {
            push_operation_events(operation, &mut local);
        }
        MirTerminatorKind::IteratorNext { state, .. } => {
            push_destination_reads(state, true, &mut local);
        }
        MirTerminatorKind::ValidatePlaces {
            places,
            replacements,
            for_write,
            ..
        } => {
            for place in places {
                push_destination_reads(place, *for_write, &mut local);
            }
            for replacement in replacements.iter().flatten() {
                push_operand_events(replacement, &mut local);
            }
        }
        MirTerminatorKind::Return => local.push(LocalEvent::Read(LocalAccess {
            local: function.return_local,
            path: Vec::new(),
            source_loan: None,
        })),
    }
    events.extend(local.into_iter().map(LoanEvent::Local));
    if let MirTerminatorKind::Invoke {
        operation:
            MirOperation {
                kind: MirOperationKind::Call { arguments, .. },
                ..
            },
        ..
    } = &block.terminator.kind
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
            | MirStatementKind::ReserveLoan(_)
            | MirStatementKind::ReleaseLoan(_) => {}
            MirStatementKind::Assign { destination, value } => {
                push_tag_rvalue(function, value, &mut events);
                push_tag_place(function, destination, true, &mut events);
            }
        }
    }
    match &block.terminator.kind {
        MirTerminatorKind::Goto { .. }
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
        MirOperationKind::Slice {
            base,
            start,
            end,
            step,
        } => {
            push_tag_operand(function, base, events);
            for value in start.iter().chain(end).chain(step) {
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
            | MirProjectionKind::Field(_)
            | MirProjectionKind::TupleField(_)
            | MirProjectionKind::NewtypeValue
            | MirProjectionKind::ArrayPatternIndex(_)
            | MirProjectionKind::ArrayPatternRest { .. }
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
                source_loan: place.source_loan,
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
                | MirOperandKind::Borrow(place) => Some(place.clone()),
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
        MirTerminatorKind::ValidatePlaces { target, unwind, .. } => {
            vec![edge(*target), edge(*unwind)]
        }
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
        MirTag::Variant(_) | MirTag::Union(_) => None,
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

fn loan_paths_overlap(
    left: &[MovePathComponent],
    right: &[MovePathComponent],
    static_integers: &BTreeMap<MirLocalId, u64>,
) -> bool {
    for (left, right) in left.iter().zip(right) {
        if left == right {
            continue;
        }
        match (
            collection_region(left, static_integers),
            collection_region(right, static_integers),
        ) {
            (CollectionComponent::Static(left), CollectionComponent::Static(right)) => {
                if static_collection_relation(left, right) == StaticRegionRelation::Disjoint {
                    return false;
                }
                return true;
            }
            (CollectionComponent::None, CollectionComponent::None) => {
                if move_path_components_are_disjoint(left, right) {
                    return false;
                }
            }
            (CollectionComponent::Dynamic, _)
            | (_, CollectionComponent::Dynamic)
            | (CollectionComponent::Static(_), CollectionComponent::None)
            | (CollectionComponent::None, CollectionComponent::Static(_)) => return true,
        }
    }
    true
}

#[derive(Clone, Copy)]
enum CollectionComponent {
    None,
    Static(StaticCollectionRegion),
    Dynamic,
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
        MovePathComponent::Index(index) => static_integers
            .get(index)
            .map_or(CollectionComponent::Dynamic, |index| {
                CollectionComponent::Static(StaticCollectionRegion::Index(*index))
            }),
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
        | MovePathComponent::Field(_)
        | MovePathComponent::TupleField(_)
        | MovePathComponent::NewtypeValue
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

fn static_integer_locals(hir: &HirProgram, function: &MirFunction) -> BTreeMap<MirLocalId, u64> {
    let mut candidates = BTreeMap::<MirLocalId, Option<u64>>::new();
    let mut record = |place: &MirPlace, value: Option<u64>| {
        if !place.projections.is_empty()
            || function.locals[place.local.index() as usize].kind != MirLocalKind::Temporary
        {
            return;
        }
        candidates
            .entry(place.local)
            .and_modify(|candidate| *candidate = None)
            .or_insert(value);
    };
    for block in &function.blocks {
        for statement in &block.statements {
            if let MirStatementKind::Assign { destination, value } = &statement.kind {
                record(destination, static_integer_rvalue(hir, value));
            }
        }
        match &block.terminator.kind {
            MirTerminatorKind::Invoke {
                destination: Some(destination),
                ..
            }
            | MirTerminatorKind::IteratorNext { destination, .. } => record(destination, None),
            MirTerminatorKind::Goto { .. }
            | MirTerminatorKind::SwitchBool { .. }
            | MirTerminatorKind::SwitchTag { .. }
            | MirTerminatorKind::Invoke {
                destination: None, ..
            }
            | MirTerminatorKind::ValidatePlaces { .. }
            | MirTerminatorKind::Return
            | MirTerminatorKind::ResumePanic
            | MirTerminatorKind::Unreachable => {}
        }
    }
    candidates
        .into_iter()
        .filter_map(|(local, value)| value.map(|value| (local, value)))
        .collect()
}

fn static_integer_rvalue(hir: &HirProgram, value: &MirRvalue) -> Option<u64> {
    let MirRvalueKind::Use(operand) = &value.kind else {
        return None;
    };
    match &operand.kind {
        MirOperandKind::Constant(MirConstant::Integer(spelling)) => {
            parse_nonnegative_integer(spelling)
        }
        MirOperandKind::Constant(MirConstant::Named(symbol)) => {
            let HirConstantValueKind::Integer(value) = hir.constant(*symbol)?.evaluated()?.kind()
            else {
                return None;
            };
            u64::try_from(*value).ok()
        }
        MirOperandKind::Constant(
            MirConstant::Unit
            | MirConstant::Bool(_)
            | MirConstant::Float(_)
            | MirConstant::Char(_)
            | MirConstant::String(_),
        )
        | MirOperandKind::Copy(_)
        | MirOperandKind::Move(_)
        | MirOperandKind::Borrow(_)
        | MirOperandKind::Loan(_)
        | MirOperandKind::Function { .. }
        | MirOperandKind::PreludeTraitFunction { .. } => None,
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
    }
}

fn push_operation_events(operation: &MirOperation, events: &mut Vec<LocalEvent>) {
    match &operation.kind {
        MirOperationKind::CheckedPrefix { operand, .. } => push_operand_events(operand, events),
        MirOperationKind::CheckedBinary { left, right, .. } => {
            push_operand_events(left, events);
            push_operand_events(right, events);
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
        MirOperationKind::Slice {
            base,
            start,
            end,
            step,
        } => {
            push_operand_events(base, events);
            for value in start.iter().chain(end).chain(step) {
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

fn push_projection_index_events(place: &MirPlace, events: &mut Vec<LocalEvent>) {
    for projection in &place.projections {
        match &projection.kind {
            MirProjectionKind::Index { index, .. } => events.push(LocalEvent::Read(LocalAccess {
                local: *index,
                path: Vec::new(),
                source_loan: None,
            })),
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
            | MirProjectionKind::ArrayPatternRest { .. } => {}
        }
    }
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
    use super::*;
    use crate::types::TypeInterner;

    fn projected_place(kind: MirProjectionKind) -> MirPlace {
        let ty = TypeInterner::default().scalar(ScalarType::Int);
        MirPlace {
            local: MirLocalId(0),
            ty,
            projections: vec![MirProjection { ty, kind }],
            source_loan: None,
        }
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
            &[MovePathComponent::Index(dynamic)],
            &[MovePathComponent::Index(split)],
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
}
