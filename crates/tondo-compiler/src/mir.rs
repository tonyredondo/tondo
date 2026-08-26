//! Typed control-flow representation consumed by bytecode generation.
//!
//! MIR contains no syntax nodes and performs no semantic lookup. Its explicit
//! blocks, places, operands, normal edges, and unwind edges are the shared
//! execution contract for the bootstrap VM and later native backends.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::hir::{
    HirBinaryOperator, HirCallArgumentTarget, HirCallProtocol, HirCallableId, HirClosureId,
    HirContainmentKind, HirPrefixOperator, HirPreludeTraitMethod, HirRangeKind, HirScopeId,
    HirSpawnKind,
};
use crate::resolve::{LocalId, MemberId, SymbolId};
use crate::source::Span;
use crate::types::{
    Assignability, NumericConversion, NumericConversionErrorVariant, ParameterMode, ScalarType,
    TypeId, TypeInterner,
};

mod lower;
mod regions;
mod verify;

pub use lower::{MirLoweringLimits, lower_to_mir};
pub(crate) use verify::verify_mir_with_capability_analysis;
pub use verify::{MirInvariantError, MirVerificationLimits, verify_mir, verify_mir_with_limits};

#[derive(Debug)]
pub enum MirError {
    NodeLimit { span: Span, resource: &'static str },
    VerificationLimit { resource: &'static str },
    Construction { span: Span, message: String },
    InvalidHir(crate::hir::HirInvariantError),
    Invariant(MirInvariantError),
}

impl fmt::Display for MirError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NodeLimit { span, resource } => write!(
                formatter,
                "MIR {resource} limit exceeded in {} at byte {}",
                span.file(),
                span.range().start()
            ),
            Self::VerificationLimit { resource } => {
                write!(formatter, "MIR {resource} limit exceeded")
            }
            Self::Construction { span, message } => write!(
                formatter,
                "MIR construction failed in {} at byte {}: {message}",
                span.file(),
                span.range().start()
            ),
            Self::InvalidHir(error) => error.fmt(formatter),
            Self::Invariant(error) => error.fmt(formatter),
        }
    }
}

impl Error for MirError {}

impl From<crate::hir::HirInvariantError> for MirError {
    fn from(error: crate::hir::HirInvariantError) -> Self {
        Self::InvalidHir(error)
    }
}

impl From<MirInvariantError> for MirError {
    fn from(error: MirInvariantError) -> Self {
        Self::Invariant(error)
    }
}

#[derive(Debug)]
pub struct MirProgram {
    functions: BTreeMap<MirFunctionId, MirFunction>,
}

impl MirProgram {
    /// Test-only mutable access for verifier fixtures that forge malformed
    /// MIR shapes.
    #[cfg(test)]
    pub fn functions_mut_for_tests(&mut self) -> &mut BTreeMap<MirFunctionId, MirFunction> {
        &mut self.functions
    }

    pub fn functions(&self) -> impl ExactSizeIterator<Item = &MirFunction> {
        self.functions.values()
    }

    pub fn function(&self, id: HirCallableId) -> Option<&MirFunction> {
        self.functions.get(&MirFunctionId::Callable(id))
    }

    pub fn closure_function(&self, id: HirClosureId) -> Option<&MirFunction> {
        self.functions.get(&MirFunctionId::Closure(id))
    }

    /// Returns a stable, backend-facing inventory of the verified MIR.
    ///
    /// The summary contains bounded counts/features and, at the executable
    /// lowering boundary, an optional normalized adapter input. Request-local
    /// IDs, addresses, layouts and source paths never cross this boundary.
    pub fn summary(&self) -> MirSummary {
        let mut summary = MirSummary::default();
        for function in self.functions() {
            summary.functions += 1;
            summary.source_spans += 1;
            summary.locals += function.locals().len() as u64;
            summary.loans += function.loans().len() as u64;
            summary.source_spans += function.locals().len() as u64;
            for block in function.blocks() {
                summary.blocks += 1;
                if block.kind() == MirBlockKind::Cleanup {
                    summary.cleanup_blocks += 1;
                    summary.feature("cleanup");
                }
                for statement in block.statements() {
                    summary.statements += 1;
                    summary.source_spans += 1;
                    match statement.kind() {
                        MirStatementKind::RegisterDefer { action, .. } => {
                            summary.defers += 1;
                            summary.feature("defer");
                            observe_operation(action, &mut summary);
                        }
                        MirStatementKind::RegisterFallback { .. } => {
                            summary.fallbacks += 1;
                            summary.feature("fallback");
                        }
                        MirStatementKind::EnterTaskScope { .. } => {
                            summary.task_scopes += 1;
                            summary.feature("task-scope");
                        }
                        MirStatementKind::BeginSelect { .. } => {
                            summary.select_regions += 1;
                            summary.feature("select");
                        }
                        MirStatementKind::RegisterSelectArm { registration, .. } => {
                            summary.select_arms += 1;
                            summary.feature("select");
                            if let MirSelectRegistration::Call(operation) = registration {
                                observe_operation(operation, &mut summary);
                            }
                        }
                        MirStatementKind::StorageLive(_)
                        | MirStatementKind::StorageDead(_)
                        | MirStatementKind::ReserveLoan(_)
                        | MirStatementKind::ReleaseLoan(_)
                        | MirStatementKind::Assign { .. }
                        | MirStatementKind::RetargetCleanup { .. }
                        | MirStatementKind::DisarmCleanup(_) => {}
                    }
                }
                summary.terminators += 1;
                summary.source_spans += 1;
                observe_terminator(block.terminator(), &mut summary);
            }
        }
        summary
    }

    /// Extracts the bounded, backend-neutral input consumed by native
    /// candidate adapters.  The extraction deliberately keeps source-local
    /// handles out of the serialized shape: locals and blocks are ordinals,
    /// and every unsupported construct is named explicitly instead of being
    /// silently approximated.
    pub fn backend_program(&self, interner: &TypeInterner) -> MirBackendProgram {
        let callable_ordinals = self
            .functions
            .keys()
            .enumerate()
            .filter_map(|(ordinal, id)| match id {
                MirFunctionId::Callable(callable) => Some((*callable, ordinal as u32)),
                MirFunctionId::Closure(_) => None,
            })
            .collect::<BTreeMap<_, _>>();
        let mut functions = self
            .functions()
            .enumerate()
            .map(|(ordinal, function)| {
                backend_function(ordinal as u32, function, interner, &callable_ordinals)
            })
            .collect::<Vec<_>>();
        validate_backend_call_targets(&mut functions);
        MirBackendProgram {
            format: "tondo-mir-backend/1".to_owned(),
            functions,
        }
    }
}

/// Backend-neutral facts extracted from one verified MIR program.
///
/// This is an evaluation artifact, not a serialized MIR format and not a
/// promise about object layout or calling convention.  Feature counters are
/// sorted by construction so reports remain deterministic across runs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MirSummary {
    pub functions: u64,
    pub blocks: u64,
    pub cleanup_blocks: u64,
    pub statements: u64,
    pub terminators: u64,
    pub locals: u64,
    pub loans: u64,
    pub operations: u64,
    pub defers: u64,
    pub fallbacks: u64,
    pub task_scopes: u64,
    pub select_regions: u64,
    pub select_arms: u64,
    pub awaits: u64,
    pub spawns: u64,
    pub invokes: u64,
    pub host_calls: u64,
    pub unsafe_calls: u64,
    pub source_spans: u64,
    pub features: BTreeMap<String, u64>,
    /// Detailed input for the bounded native adapter.  Older probe files may
    /// omit it; such a probe is rejected by the adapter rather than treated as
    /// a native lowering.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<MirBackendProgram>,
}

impl MirSummary {
    fn feature(&mut self, name: &'static str) {
        *self.features.entry(name.to_owned()).or_default() += 1;
    }
}

/// Version-independent, path-free MIR shape for native backend adapters.
///
/// This is intentionally not a public ABI or object-layout description. It is
/// the smallest lossless-enough boundary for the first adapter slice: scalar
/// assignments, comparisons, checked arithmetic, verified direct scalar calls,
/// explicit traps and normal control flow, including loop-carried locals, can
/// be lowered, while aggregates, cleanup and async operations are rejected
/// explicitly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MirBackendProgram {
    pub format: String,
    pub functions: Vec<MirBackendFunction>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MirBackendFunction {
    pub ordinal: u32,
    pub parameters: Vec<u32>,
    /// Canonical source types for parameters.  Native lowering uses these
    /// only to select the private ABI carrier; they are not a public layout
    /// promise.
    #[serde(default)]
    pub parameter_types: Vec<String>,
    pub return_local: u32,
    pub return_type: String,
    pub blocks: Vec<MirBackendBlock>,
    pub supported: bool,
    pub unsupported: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MirBackendBlock {
    pub ordinal: u32,
    pub kind: String,
    pub statements: Vec<MirBackendStatement>,
    pub terminator: MirBackendTerminator,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum MirBackendStatement {
    Assign {
        destination: u32,
        value: MirBackendRvalue,
    },
    Marker {
        kind: String,
    },
    /// A side-effecting runtime edge emitted by MIR (retain/release,
    /// defer/loan registration or task-scope entry).  Arguments are still
    /// normalized operands so adapters cannot smuggle source pointers across
    /// the boundary.
    Runtime {
        kind: String,
        arguments: Vec<MirBackendOperand>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum MirBackendRvalue {
    Use(MirBackendOperand),
    /// The discriminant of a payload-bearing core aggregate. Payloads are
    /// intentionally not represented by the scalar adapter.
    Tag {
        value: u32,
    },
    /// A private managed aggregate.  The first native implementation keeps
    /// the payload in the runtime table and passes only its opaque handle.
    Aggregate {
        kind: String,
        values: Vec<MirBackendOperand>,
    },
    Prefix {
        operator: String,
        operand: MirBackendOperand,
    },
    Binary {
        operator: String,
        left: MirBackendOperand,
        right: MirBackendOperand,
    },
    Unsupported {
        kind: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum MirBackendOperand {
    Constant(MirBackendConstant),
    Local {
        index: u32,
    },
    /// A verified read-only borrow of a direct scalar local. The adapter
    /// lowers this as a value read and never materializes its address.
    Borrow {
        index: u32,
    },
    Unsupported {
        kind: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum MirBackendConstant {
    Unit,
    Bool(bool),
    Integer(String),
    Float(String),
    Char(String),
    String(String),
    Named,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum MirBackendTerminator {
    Return,
    Goto {
        target: u32,
    },
    SwitchBool {
        condition: MirBackendOperand,
        if_true: u32,
        if_false: u32,
    },
    /// Dispatches on a core `Option`/`Result` discriminant. Other enum and
    /// union tags remain outside the scalar adapter until their payload ABI
    /// is lowered.
    SwitchTag {
        value: MirBackendOperand,
        cases: Vec<(u32, u32)>,
        otherwise: u32,
    },
    Invoke {
        operation: MirBackendOperation,
        destination: Option<u32>,
        target: Option<u32>,
    },
    Marker {
        kind: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum MirBackendOperation {
    CheckedPrefix {
        operator: String,
        operand: MirBackendOperand,
    },
    CheckedBinary {
        operator: String,
        left: MirBackendOperand,
        right: MirBackendOperand,
    },
    /// Checked zero-based span access. The native adapter receives the
    /// already-normalized length so bounds policy is identical across
    /// backends and never depends on a user-visible array layout.
    BoundsCheck {
        index: MirBackendOperand,
        length: MirBackendOperand,
    },
    Call {
        function: u32,
        arguments: Vec<MirBackendOperand>,
    },
    /// A compiler-owned host operation.  The kind is a stable logical
    /// operation name; host state remains behind the runtime handle table.
    HostCall {
        kind: String,
        arguments: Vec<MirBackendOperand>,
    },
    /// Runtime operation used by cleanup, ownership and structured async
    /// lowering.  Its arguments are opaque ABI carriers, never addresses.
    Runtime {
        kind: String,
        arguments: Vec<MirBackendOperand>,
    },
    Assert {
        condition: MirBackendOperand,
    },
    Trap {
        kind: String,
    },
    Marker {
        kind: String,
    },
}

fn backend_function(
    ordinal: u32,
    function: &MirFunction,
    interner: &TypeInterner,
    callable_ordinals: &BTreeMap<HirCallableId, u32>,
) -> MirBackendFunction {
    let mut unsupported = Vec::new();
    let blocks = function
        .blocks_with_ids()
        .map(|(block_id, block)| {
            backend_block(block_id.index(), block, &mut unsupported, callable_ordinals)
        })
        .collect::<Vec<_>>();
    validate_backend_control_flow(&blocks, &mut unsupported);
    let return_type = backend_type_name(interner, function.outcome());
    if !is_native_carrier_type(&return_type) {
        unsupported.push(format!("return-type:{return_type}"));
    }
    let parameter_types = function
        .parameters()
        .iter()
        .map(|parameter| {
            function
                .local(*parameter)
                .map(|local| backend_type_name(interner, local.ty()))
                .unwrap_or_else(|| "missing".to_owned())
        })
        .collect::<Vec<_>>();
    for parameter_type in &parameter_types {
        if !is_native_carrier_type(parameter_type) {
            unsupported.push(format!("parameter-type:{parameter_type}"));
        }
    }
    unsupported.sort();
    unsupported.dedup();
    MirBackendFunction {
        ordinal,
        parameters: function.parameters().iter().map(|id| id.index()).collect(),
        parameter_types,
        return_local: function.return_local().index(),
        return_type,
        blocks,
        supported: unsupported.is_empty(),
        unsupported,
    }
}

fn validate_backend_call_targets(functions: &mut [MirBackendFunction]) {
    let targets = functions
        .iter()
        .map(|function| {
            (
                function.ordinal,
                (function.parameters.len(), function.supported),
            )
        })
        .collect::<BTreeMap<_, _>>();
    for function in functions {
        let mut reasons = Vec::new();
        for block in &function.blocks {
            let MirBackendTerminator::Invoke { operation, .. } = &block.terminator else {
                continue;
            };
            let MirBackendOperation::Call {
                function: target,
                arguments,
            } = operation
            else {
                continue;
            };
            let Some((arity, supported)) = targets.get(target).copied() else {
                reasons.push(format!("call-target-missing:{target}"));
                continue;
            };
            if arguments.len() != arity {
                reasons.push(format!("call-arity:{target}:{}:{arity}", arguments.len()));
            }
            if !supported {
                reasons.push(format!("call-target-unsupported:{target}"));
            }
        }
        function.unsupported.extend(reasons);
        function.unsupported.sort();
        function.unsupported.dedup();
        function.supported = function.unsupported.is_empty();
    }
}

fn backend_block(
    ordinal: u32,
    block: &MirBasicBlock,
    unsupported: &mut Vec<String>,
    callable_ordinals: &BTreeMap<HirCallableId, u32>,
) -> MirBackendBlock {
    let kind = match block.kind() {
        MirBlockKind::Normal => "normal",
        MirBlockKind::Cleanup => "cleanup",
    }
    .to_owned();
    let statements = block
        .statements()
        .iter()
        .map(|statement| match statement.kind() {
            MirStatementKind::Assign {
                destination: _,
                value,
            } if matches!(
                value.kind(),
                MirRvalueKind::Use(MirOperand {
                    kind: MirOperandKind::Constant(MirConstant::Unit),
                    ..
                })
            ) =>
            {
                MirBackendStatement::Marker {
                    kind: "unit-assignment".to_owned(),
                }
            }
            MirStatementKind::Assign { destination, value } => MirBackendStatement::Assign {
                destination: backend_place(destination, unsupported),
                value: backend_rvalue(value, unsupported),
            },
            MirStatementKind::StorageLive(_) | MirStatementKind::StorageDead(_) => {
                MirBackendStatement::Marker {
                    kind: statement_kind_name(statement.kind()).to_owned(),
                }
            }
            MirStatementKind::ReserveLoan(loan) => MirBackendStatement::Runtime {
                kind: format!("reserve-loan:{}", loan.index()),
                arguments: Vec::new(),
            },
            MirStatementKind::ReleaseLoan(loan) => MirBackendStatement::Runtime {
                kind: format!("release-loan:{}", loan.index()),
                arguments: Vec::new(),
            },
            MirStatementKind::RegisterDefer { scope, guard, .. } => {
                let mut arguments = Vec::new();
                if let Some(guard) = guard {
                    arguments.push(backend_operand(
                        &MirOperand {
                            ty: guard.ty(),
                            kind: MirOperandKind::Copy(guard.clone()),
                        },
                        unsupported,
                    ));
                }
                MirBackendStatement::Runtime {
                    kind: format!("register-defer:{}", scope.index()),
                    arguments,
                }
            }
            MirStatementKind::RegisterFallback { scope, owner } => {
                let arguments = vec![backend_operand(
                    &MirOperand {
                        ty: owner.ty(),
                        kind: MirOperandKind::Move(owner.clone()),
                    },
                    unsupported,
                )];
                MirBackendStatement::Runtime {
                    kind: format!("register-fallback:{}", scope.index()),
                    arguments,
                }
            }
            MirStatementKind::EnterTaskScope { scope } => MirBackendStatement::Runtime {
                kind: format!("enter-task-scope:{}", scope.index()),
                arguments: Vec::new(),
            },
            MirStatementKind::RetargetCleanup { from, to } => MirBackendStatement::Runtime {
                kind: format!(
                    "retarget-cleanup:{}:{}",
                    from.local().index(),
                    to.local().index()
                ),
                arguments: Vec::new(),
            },
            MirStatementKind::DisarmCleanup(place) if kind == "cleanup" => {
                MirBackendStatement::Runtime {
                    kind: format!("disarm-cleanup:{}", place.local().index()),
                    arguments: Vec::new(),
                }
            }
            MirStatementKind::BeginSelect { capacity } => MirBackendStatement::Runtime {
                kind: format!("begin-select:{capacity}"),
                arguments: Vec::new(),
            },
            MirStatementKind::RegisterSelectArm { index, .. } => MirBackendStatement::Runtime {
                kind: format!("register-select-arm:{index}"),
                arguments: Vec::new(),
            },
            MirStatementKind::DisarmCleanup(_) if kind == "normal" => MirBackendStatement::Marker {
                kind: "disarm-cleanup".to_owned(),
            },
            other => {
                let kind = statement_kind_name(other).to_owned();
                if !kind.starts_with("release-") && !kind.starts_with("reserve-") {
                    unsupported.push(format!("statement:{kind}"));
                }
                MirBackendStatement::Marker { kind }
            }
        })
        .collect();
    let terminator = match block.terminator().kind() {
        MirTerminatorKind::Return => MirBackendTerminator::Return,
        MirTerminatorKind::Goto { target } => MirBackendTerminator::Goto {
            target: target.index(),
        },
        MirTerminatorKind::SwitchBool {
            condition,
            if_true,
            if_false,
        } => MirBackendTerminator::SwitchBool {
            condition: backend_operand(condition, unsupported),
            if_true: if_true.index(),
            if_false: if_false.index(),
        },
        MirTerminatorKind::SwitchTag {
            value,
            cases,
            otherwise,
        } => {
            let mut normalized = Vec::with_capacity(cases.len());
            let mut valid = true;
            for (tag, target) in cases {
                let Some(tag) = backend_tag_discriminant(*tag) else {
                    unsupported.push("switch-tag:unsupported-tag".to_owned());
                    valid = false;
                    continue;
                };
                normalized.push((tag, target.index()));
            }
            if valid {
                normalized.sort_unstable_by_key(|(tag, _)| *tag);
                if normalized.windows(2).any(|pair| pair[0].0 == pair[1].0) {
                    unsupported.push("switch-tag:duplicate-tag".to_owned());
                }
                MirBackendTerminator::SwitchTag {
                    value: backend_operand(value, unsupported),
                    cases: normalized,
                    otherwise: otherwise.index(),
                }
            } else {
                MirBackendTerminator::Marker {
                    kind: "switch-tag".to_owned(),
                }
            }
        }
        MirTerminatorKind::ValidatePlaces {
            places,
            against,
            target,
            ..
        } if kind == "normal" => {
            if places.iter().any(|place| !place.projections().is_empty()) {
                unsupported.push("validate-places:projection".to_owned());
            }
            if against.iter().any(|loans| !loans.is_empty()) {
                unsupported.push("validate-places:loan".to_owned());
            }
            MirBackendTerminator::Goto {
                target: target.index(),
            }
        }
        MirTerminatorKind::Invoke {
            operation,
            destination,
            target,
            ..
        } => MirBackendTerminator::Invoke {
            operation: backend_operation(operation, unsupported, callable_ordinals),
            destination: destination
                .as_ref()
                .map(|place| backend_place(place, unsupported)),
            target: target.map(MirBlockId::index),
        },
        MirTerminatorKind::ResumePanic | MirTerminatorKind::DrainUnwind { .. }
            if kind == "cleanup" =>
        {
            MirBackendTerminator::Marker {
                kind: terminator_kind_name(block.terminator().kind()).to_owned(),
            }
        }
        MirTerminatorKind::Unreachable => MirBackendTerminator::Marker {
            kind: "unreachable".to_owned(),
        },
        other => {
            let kind = terminator_kind_name(other).to_owned();
            unsupported.push(format!("terminator:{kind}"));
            MirBackendTerminator::Marker { kind }
        }
    };
    MirBackendBlock {
        ordinal,
        kind,
        statements,
        terminator,
    }
}

/// Validate the control-flow shape accepted by the first scalar adapter.
///
/// The native evaluator lowers scalar CFGs, including cycles with
/// loop-carried locals.  The normalized boundary must still reject missing
/// targets and any edge into cleanup/unwind regions so a backend cannot
/// silently turn unsupported control flow into straight-line code.
fn validate_backend_control_flow(blocks: &[MirBackendBlock], unsupported: &mut Vec<String>) {
    for block in blocks.iter().filter(|block| block.kind == "cleanup") {
        for statement in &block.statements {
            let MirBackendStatement::Marker { kind } = statement else {
                unsupported.push(format!("cleanup:statement-instruction:{}", block.ordinal));
                continue;
            };
            if kind.starts_with("release-")
                || kind.starts_with("reserve-")
                || matches!(
                    kind.as_str(),
                    "register-defer"
                        | "register-fallback"
                        | "enter-task-scope"
                        | "retarget-cleanup"
                        | "begin-select"
                        | "register-select-arm"
                )
            {
                unsupported.push(format!("cleanup:resource-action:{kind}"));
            }
        }
    }
    let normal = blocks
        .iter()
        .filter(|block| block.kind == "normal")
        .map(|block| (block.ordinal, block))
        .collect::<BTreeMap<_, _>>();
    if normal.is_empty() {
        unsupported.push("control-flow:no-normal-block".to_owned());
        return;
    }

    let mut successors = BTreeMap::<u32, Vec<u32>>::new();
    for block in normal.values() {
        let targets = match &block.terminator {
            MirBackendTerminator::Return => Vec::new(),
            MirBackendTerminator::Goto { target } => vec![*target],
            MirBackendTerminator::SwitchBool {
                if_true, if_false, ..
            } => vec![*if_true, *if_false],
            MirBackendTerminator::SwitchTag {
                cases, otherwise, ..
            } => cases
                .iter()
                .map(|(_, target)| *target)
                .chain(std::iter::once(*otherwise))
                .collect(),
            MirBackendTerminator::Invoke { target, .. } => {
                if target.is_none() {
                    unsupported.push("control-flow:invoke-without-target".to_owned());
                }
                target.iter().copied().collect()
            }
            MirBackendTerminator::Marker { kind } => {
                if kind != "unreachable" {
                    unsupported.push(format!("terminator:{kind}"));
                }
                Vec::new()
            }
        };
        for target in &targets {
            if !normal.contains_key(target) {
                unsupported.push(format!("control-flow:target:{target}"));
            }
        }
        successors.insert(block.ordinal, targets);
    }

    // A scalar adapter must not route execution into cleanup/unwind blocks.
    // The normalized representation only exposes normal blocks as executable
    // targets, so this set also makes that invariant explicit for reviewers.
    let normal_ordinals = normal.keys().copied().collect::<BTreeSet<_>>();
    for target in successors.values().flatten() {
        if !normal_ordinals.contains(target) {
            unsupported.push(format!("control-flow:non-normal-target:{target}"));
        }
    }
}

fn backend_operation(
    operation: &MirOperation,
    unsupported: &mut Vec<String>,
    callable_ordinals: &BTreeMap<HirCallableId, u32>,
) -> MirBackendOperation {
    match operation.kind() {
        MirOperationKind::CheckedPrefix { operator, operand } => {
            if !is_supported_prefix_operator(*operator) {
                unsupported.push(format!("operator:{}", prefix_operator_name(*operator)));
            }
            MirBackendOperation::CheckedPrefix {
                operator: prefix_operator_name(*operator).to_owned(),
                operand: backend_operand(operand, unsupported),
            }
        }
        MirOperationKind::CheckedBinary {
            operator,
            left,
            right,
        } => {
            if !is_supported_binary_operator(*operator) {
                unsupported.push(format!("operator:{}", binary_operator_name(*operator)));
            }
            MirBackendOperation::CheckedBinary {
                operator: binary_operator_name(*operator).to_owned(),
                left: backend_operand(left, unsupported),
                right: backend_operand(right, unsupported),
            }
        }
        MirOperationKind::Call {
            callee,
            arguments,
            protocol,
            unsafe_call,
            ..
        } => {
            if *protocol != HirCallProtocol::Call {
                unsupported.push(format!("call-protocol:{protocol:?}"));
                return MirBackendOperation::Marker {
                    kind: "call".to_owned(),
                };
            }
            if *unsafe_call {
                unsupported.push("call:unsafe".to_owned());
                return MirBackendOperation::Marker {
                    kind: "call".to_owned(),
                };
            }
            let callable = match callee.kind() {
                MirOperandKind::Function { callable, .. } => callable,
                _ => {
                    unsupported.push("call:indirect".to_owned());
                    return MirBackendOperation::Marker {
                        kind: "call".to_owned(),
                    };
                }
            };
            let Some(function) = callable_ordinals.get(callable).copied() else {
                unsupported.push("call:unknown-target".to_owned());
                return MirBackendOperation::Marker {
                    kind: "call".to_owned(),
                };
            };
            let mut positional = BTreeMap::<u32, MirBackendOperand>::new();
            for argument in arguments {
                if argument.mode() != ParameterMode::Value {
                    unsupported.push(format!("call-argument-mode:{:?}", argument.mode()));
                    return MirBackendOperation::Marker {
                        kind: "call".to_owned(),
                    };
                }
                let HirCallArgumentTarget::Fixed(index) = argument.target() else {
                    unsupported.push(format!("call-argument-target:{:?}", argument.target()));
                    return MirBackendOperation::Marker {
                        kind: "call".to_owned(),
                    };
                };
                if positional
                    .insert(index, backend_operand(argument.value(), unsupported))
                    .is_some()
                {
                    unsupported.push(format!("call-argument-duplicate:{index}"));
                    return MirBackendOperation::Marker {
                        kind: "call".to_owned(),
                    };
                }
            }
            let arguments = positional
                .into_iter()
                .enumerate()
                .map(|(expected, (index, operand))| {
                    if expected as u32 != index {
                        unsupported.push("call-argument-noncontiguous".to_owned());
                    }
                    operand
                })
                .collect::<Vec<_>>();
            if unsupported
                .last()
                .is_some_and(|reason| reason == "call-argument-noncontiguous")
            {
                return MirBackendOperation::Marker {
                    kind: "call".to_owned(),
                };
            }
            MirBackendOperation::Call {
                function,
                arguments,
            }
        }
        MirOperationKind::BootstrapHostCall {
            function,
            arguments,
        } => {
            let kind = backend_host_function_name(*function).to_owned();
            let arguments = arguments
                .iter()
                .map(|argument| backend_operand(argument, unsupported))
                .collect::<Vec<_>>();
            MirBackendOperation::HostCall { kind, arguments }
        }
        MirOperationKind::ExplicitPanic { .. } => MirBackendOperation::Trap {
            kind: "explicit-panic".to_owned(),
        },
        MirOperationKind::Assert { condition, .. } => MirBackendOperation::Assert {
            condition: backend_operand(condition, unsupported),
        },
        other => {
            let kind = operation_kind_name(other).to_owned();
            unsupported.push(format!("operation:{kind}"));
            MirBackendOperation::Marker { kind }
        }
    }
}

fn backend_place(place: &MirPlace, unsupported: &mut Vec<String>) -> u32 {
    if !place.projections().is_empty() {
        unsupported.push("place:projection".to_owned());
    }
    place.local().index()
}

fn backend_operand(operand: &MirOperand, unsupported: &mut Vec<String>) -> MirBackendOperand {
    match operand.kind() {
        MirOperandKind::Constant(constant) => MirBackendOperand::Constant(match constant {
            MirConstant::Unit => MirBackendConstant::Unit,
            MirConstant::Bool(value) => MirBackendConstant::Bool(*value),
            MirConstant::Integer(value) => MirBackendConstant::Integer(value.clone()),
            MirConstant::Float(value) => MirBackendConstant::Float(value.clone()),
            MirConstant::Char(value) => MirBackendConstant::Char(value.clone()),
            MirConstant::String(value) => MirBackendConstant::String(value.clone()),
            MirConstant::Named(_) => MirBackendConstant::Named,
        }),
        MirOperandKind::Copy(place) | MirOperandKind::Move(place) => {
            if !place.projections().is_empty() {
                unsupported.push("operand:projection".to_owned());
            }
            MirBackendOperand::Local {
                index: place.local().index(),
            }
        }
        MirOperandKind::Borrow(place) => {
            if !place.projections().is_empty() {
                unsupported.push("operand:borrow-projection".to_owned());
                MirBackendOperand::Unsupported {
                    kind: "borrow-projection".to_owned(),
                }
            } else {
                MirBackendOperand::Borrow {
                    index: place.local().index(),
                }
            }
        }
        other => {
            let kind = operand_kind_name(other).to_owned();
            unsupported.push(format!("operand:{kind}"));
            MirBackendOperand::Unsupported { kind }
        }
    }
}

fn backend_rvalue(value: &MirRvalue, unsupported: &mut Vec<String>) -> MirBackendRvalue {
    match value.kind() {
        MirRvalueKind::Use(operand) => MirBackendRvalue::Use(backend_operand(operand, unsupported)),
        MirRvalueKind::Aggregate { shape, values } => {
            if backend_aggregate_discriminant(shape).is_some() {
                let values = values
                    .iter()
                    .map(|value| backend_operand(value, unsupported))
                    .collect::<Vec<_>>();
                MirBackendRvalue::Aggregate {
                    kind: backend_aggregate_name(shape).to_owned(),
                    values,
                }
            } else {
                unsupported.push("rvalue:aggregate".to_owned());
                MirBackendRvalue::Unsupported {
                    kind: "aggregate".to_owned(),
                }
            }
        }
        MirRvalueKind::Prefix { operator, operand } => {
            if !is_supported_prefix_operator(*operator) {
                unsupported.push(format!("operator:{}", prefix_operator_name(*operator)));
            }
            MirBackendRvalue::Prefix {
                operator: prefix_operator_name(*operator).to_owned(),
                operand: backend_operand(operand, unsupported),
            }
        }
        MirRvalueKind::Binary {
            operator,
            left,
            right,
        } => {
            if !is_supported_binary_operator(*operator) {
                unsupported.push(format!("operator:{}", binary_operator_name(*operator)));
            }
            MirBackendRvalue::Binary {
                operator: binary_operator_name(*operator).to_owned(),
                left: backend_operand(left, unsupported),
                right: backend_operand(right, unsupported),
            }
        }
        other => {
            let kind = rvalue_kind_name(other).to_owned();
            unsupported.push(format!("rvalue:{kind}"));
            MirBackendRvalue::Unsupported { kind }
        }
    }
}

fn backend_type_name(interner: &TypeInterner, ty: TypeId) -> String {
    interner
        .canonical(ty)
        .unwrap_or_else(|_| "unknown".to_owned())
}

fn is_native_carrier_type(name: &str) -> bool {
    matches!(name, "Int" | "Bool" | "Unit")
        || name.starts_with("Option[")
        || name.starts_with("Result[")
        || name.starts_with("Int?")
        || name.contains(" ! ")
}

fn backend_aggregate_name(shape: &MirAggregateKind) -> &'static str {
    match shape {
        MirAggregateKind::OptionNone => "option-none",
        MirAggregateKind::OptionSome => "option-some",
        MirAggregateKind::ResultOk => "result-ok",
        MirAggregateKind::ResultErr => "result-err",
        _ => "aggregate",
    }
}

fn backend_host_function_name(function: MirBootstrapHostFunction) -> &'static str {
    match function {
        MirBootstrapHostFunction::ConsolePrint => "console-print",
        MirBootstrapHostFunction::ConsolePrintln => "console-println",
        MirBootstrapHostFunction::ProcessPipe => "process-pipe",
        MirBootstrapHostFunction::CommandMergeStderr => "command-merge-stderr",
        MirBootstrapHostFunction::PipelineMergeStderr => "pipeline-merge-stderr",
        MirBootstrapHostFunction::ProcessOutputStdout => "process-output-stdout",
        MirBootstrapHostFunction::ProcessOutputStderr => "process-output-stderr",
        MirBootstrapHostFunction::ProcessOutputCombined => "process-output-combined",
        MirBootstrapHostFunction::ProcessOutputStatuses => "process-output-statuses",
        MirBootstrapHostFunction::ExitStatusCode => "exit-status-code",
        MirBootstrapHostFunction::ExitStatusSuccess => "exit-status-success",
        MirBootstrapHostFunction::PointerRead => "pointer-read",
        MirBootstrapHostFunction::PointerWrite => "pointer-write",
        MirBootstrapHostFunction::PointerOffset => "pointer-offset",
        MirBootstrapHostFunction::PointerCast => "pointer-cast",
        MirBootstrapHostFunction::PointerAddress => "pointer-address",
        MirBootstrapHostFunction::PointerFromAddress => "pointer-from-address",
        MirBootstrapHostFunction::TestingLog => "testing-log",
        MirBootstrapHostFunction::TestingTags => "testing-tags",
        MirBootstrapHostFunction::TestingFailNow => "testing-fail-now",
        MirBootstrapHostFunction::TestingSkip => "testing-skip",
        MirBootstrapHostFunction::TestingAttach => "testing-attach",
        MirBootstrapHostFunction::TestingSnapshot => "testing-snapshot",
        MirBootstrapHostFunction::TestingRunLeaf => "testing-run-leaf",
        MirBootstrapHostFunction::TestingRunSuite => "testing-run-suite",
        MirBootstrapHostFunction::TestingBeginSuiteCleanup => "testing-begin-suite-cleanup",
    }
}

fn backend_aggregate_discriminant(shape: &MirAggregateKind) -> Option<u32> {
    match shape {
        MirAggregateKind::OptionNone => Some(0),
        MirAggregateKind::OptionSome => Some(1),
        MirAggregateKind::ResultOk => Some(2),
        MirAggregateKind::ResultErr => Some(3),
        _ => None,
    }
}

fn backend_tag_discriminant(tag: MirTag) -> Option<u32> {
    match tag {
        MirTag::OptionNone => Some(0),
        MirTag::OptionSome => Some(1),
        MirTag::ResultOk => Some(2),
        MirTag::ResultErr => Some(3),
        MirTag::Variant(_) | MirTag::NumericConversionError(_) | MirTag::Union(_) => None,
    }
}

fn prefix_operator_name(operator: HirPrefixOperator) -> &'static str {
    match operator {
        HirPrefixOperator::Negate => "negate",
        HirPrefixOperator::LogicalNot => "logical-not",
        HirPrefixOperator::BitwiseNot => "bitwise-not",
    }
}

fn is_supported_prefix_operator(operator: HirPrefixOperator) -> bool {
    matches!(
        operator,
        HirPrefixOperator::Negate | HirPrefixOperator::BitwiseNot
    )
}

fn is_supported_binary_operator(operator: HirBinaryOperator) -> bool {
    matches!(
        operator,
        HirBinaryOperator::Multiply
            | HirBinaryOperator::Divide
            | HirBinaryOperator::Remainder
            | HirBinaryOperator::Add
            | HirBinaryOperator::Subtract
            | HirBinaryOperator::ShiftLeft
            | HirBinaryOperator::ShiftRight
            | HirBinaryOperator::BitwiseAnd
            | HirBinaryOperator::BitwiseXor
            | HirBinaryOperator::BitwiseOr
            | HirBinaryOperator::Less
            | HirBinaryOperator::LessEqual
            | HirBinaryOperator::Greater
            | HirBinaryOperator::GreaterEqual
            | HirBinaryOperator::Equal
            | HirBinaryOperator::NotEqual
    )
}

fn binary_operator_name(operator: HirBinaryOperator) -> &'static str {
    match operator {
        HirBinaryOperator::Multiply => "multiply",
        HirBinaryOperator::Divide => "divide",
        HirBinaryOperator::Remainder => "remainder",
        HirBinaryOperator::Add => "add",
        HirBinaryOperator::Subtract => "subtract",
        HirBinaryOperator::ShiftLeft => "shift-left",
        HirBinaryOperator::ShiftRight => "shift-right",
        HirBinaryOperator::BitwiseAnd => "bitwise-and",
        HirBinaryOperator::BitwiseXor => "bitwise-xor",
        HirBinaryOperator::BitwiseOr => "bitwise-or",
        HirBinaryOperator::Less => "less",
        HirBinaryOperator::LessEqual => "less-equal",
        HirBinaryOperator::Greater => "greater",
        HirBinaryOperator::GreaterEqual => "greater-equal",
        HirBinaryOperator::Equal => "equal",
        HirBinaryOperator::NotEqual => "not-equal",
        HirBinaryOperator::LogicalAnd => "logical-and",
        HirBinaryOperator::LogicalOr => "logical-or",
    }
}

fn statement_kind_name(statement: &MirStatementKind) -> &'static str {
    match statement {
        MirStatementKind::StorageLive(_) => "storage-live",
        MirStatementKind::StorageDead(_) => "storage-dead",
        MirStatementKind::ReserveLoan(_) => "reserve-loan",
        MirStatementKind::ReleaseLoan(_) => "release-loan",
        MirStatementKind::Assign { .. } => "assign",
        MirStatementKind::RegisterDefer { .. } => "register-defer",
        MirStatementKind::RegisterFallback { .. } => "register-fallback",
        MirStatementKind::EnterTaskScope { .. } => "enter-task-scope",
        MirStatementKind::RetargetCleanup { .. } => "retarget-cleanup",
        MirStatementKind::DisarmCleanup(_) => "disarm-cleanup",
        MirStatementKind::BeginSelect { .. } => "begin-select",
        MirStatementKind::RegisterSelectArm { .. } => "register-select-arm",
    }
}

fn operand_kind_name(operand: &MirOperandKind) -> &'static str {
    match operand {
        MirOperandKind::Constant(_) => "constant",
        MirOperandKind::Copy(_) => "copy",
        MirOperandKind::Move(_) => "move",
        MirOperandKind::Borrow(_) => "borrow",
        MirOperandKind::Loan(_) => "loan",
        MirOperandKind::Function { .. } => "function",
        MirOperandKind::PreludeTraitFunction { .. } => "prelude-trait-function",
    }
}

fn rvalue_kind_name(value: &MirRvalueKind) -> &'static str {
    match value {
        MirRvalueKind::Use(_) => "use",
        MirRvalueKind::Prefix { .. } => "prefix",
        MirRvalueKind::Binary { .. } => "binary",
        MirRvalueKind::Aggregate { .. } => "aggregate",
        MirRvalueKind::RecordUpdate { .. } => "record-update",
        MirRvalueKind::Coerce { .. } => "coerce",
        MirRvalueKind::NumericConversion { .. } => "numeric-conversion",
        MirRvalueKind::Range { .. } => "range",
        MirRvalueKind::Contains { .. } => "contains",
        MirRvalueKind::MapRemove { .. } => "map-remove",
        MirRvalueKind::Interpolate { .. } => "interpolate",
        MirRvalueKind::Length(_) => "length",
        MirRvalueKind::IteratorState { .. } => "iterator-state",
    }
}

fn terminator_kind_name(terminator: &MirTerminatorKind) -> &'static str {
    match terminator {
        MirTerminatorKind::Goto { .. } => "goto",
        MirTerminatorKind::SwitchBool { .. } => "switch-bool",
        MirTerminatorKind::SwitchTag { .. } => "switch-tag",
        MirTerminatorKind::Invoke { .. } => "invoke",
        MirTerminatorKind::Await { .. } => "await",
        MirTerminatorKind::Spawn { .. } => "spawn",
        MirTerminatorKind::IteratorNext { .. } => "iterator-next",
        MirTerminatorKind::ValidatePlaces { .. } => "validate-places",
        MirTerminatorKind::ValidateLoan { .. } => "validate-loan",
        MirTerminatorKind::DrainDefers { .. } => "drain-defers",
        MirTerminatorKind::DrainScopes { .. } => "drain-scopes",
        MirTerminatorKind::DrainUnwind { .. } => "drain-unwind",
        MirTerminatorKind::CommitSelect { .. } => "commit-select",
        MirTerminatorKind::Return => "return",
        MirTerminatorKind::ResumePanic => "resume-panic",
        MirTerminatorKind::Unreachable => "unreachable",
    }
}

fn operation_kind_name(operation: &MirOperationKind) -> &'static str {
    match operation {
        MirOperationKind::CheckedPrefix { .. } => "checked-prefix",
        MirOperationKind::CheckedBinary { .. } => "checked-binary",
        MirOperationKind::ArraySequence { .. } => "array-sequence",
        MirOperationKind::BuildMap { .. } => "build-map",
        MirOperationKind::Index { .. } => "index",
        MirOperationKind::Slice { .. } => "slice",
        MirOperationKind::Call { .. } => "call",
        MirOperationKind::ExplicitPanic { .. } => "explicit-panic",
        MirOperationKind::Assert { .. } => "assert",
        MirOperationKind::Format { .. } => "format",
        MirOperationKind::JoinFormat { .. } => "join-format",
        MirOperationKind::BootstrapHostCall { .. } => "bootstrap-host-call",
    }
}

fn observe_operation(operation: &MirOperation, summary: &mut MirSummary) {
    summary.operations += 1;
    match operation.kind() {
        MirOperationKind::CheckedPrefix { .. } => summary.feature("checked-prefix"),
        MirOperationKind::CheckedBinary { .. } => summary.feature("checked-binary"),
        MirOperationKind::ArraySequence { .. } => summary.feature("array-sequence"),
        MirOperationKind::BuildMap { .. } => summary.feature("map-build"),
        MirOperationKind::Index { .. } => summary.feature("index"),
        MirOperationKind::Slice { .. } => summary.feature("slice"),
        MirOperationKind::Call { unsafe_call, .. } => {
            summary.feature("call");
            if *unsafe_call {
                summary.unsafe_calls += 1;
                summary.feature("unsafe-call");
            }
        }
        MirOperationKind::ExplicitPanic { .. } => summary.feature("panic"),
        MirOperationKind::Assert { .. } => summary.feature("assert"),
        MirOperationKind::Format { .. } => summary.feature("format"),
        MirOperationKind::JoinFormat { .. } => summary.feature("join-format"),
        MirOperationKind::BootstrapHostCall { .. } => {
            summary.host_calls += 1;
            summary.feature("host-call");
        }
    }
}

fn observe_terminator(terminator: &MirTerminator, summary: &mut MirSummary) {
    match terminator.kind() {
        MirTerminatorKind::Goto { .. } => summary.feature("goto"),
        MirTerminatorKind::SwitchBool { .. } => summary.feature("switch-bool"),
        MirTerminatorKind::SwitchTag { .. } => summary.feature("switch-tag"),
        MirTerminatorKind::Invoke { operation, .. } => {
            summary.invokes += 1;
            summary.feature("invoke");
            observe_operation(operation, summary);
        }
        MirTerminatorKind::Await { awaitable, .. } => {
            summary.awaits += 1;
            summary.feature("await");
            if let MirAwaitable::Call(operation) = awaitable {
                observe_operation(operation, summary);
            }
        }
        MirTerminatorKind::Spawn { operation, .. } => {
            summary.spawns += 1;
            summary.feature("spawn");
            observe_operation(operation, summary);
        }
        MirTerminatorKind::IteratorNext { .. } => summary.feature("iterator-next"),
        MirTerminatorKind::ValidatePlaces { .. } => summary.feature("validate-places"),
        MirTerminatorKind::ValidateLoan { .. } => summary.feature("validate-loan"),
        MirTerminatorKind::DrainDefers { .. } => summary.feature("drain-defers"),
        MirTerminatorKind::DrainScopes { .. } => summary.feature("drain-scopes"),
        MirTerminatorKind::DrainUnwind { .. } => summary.feature("drain-unwind"),
        MirTerminatorKind::CommitSelect { .. } => {
            summary.feature("select-commit");
        }
        MirTerminatorKind::Return => summary.feature("return"),
        MirTerminatorKind::ResumePanic => summary.feature("resume-panic"),
        MirTerminatorKind::Unreachable => summary.feature("unreachable"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MirFunctionId {
    Callable(HirCallableId),
    Closure(HirClosureId),
}

#[derive(Debug)]
pub struct MirFunction {
    id: MirFunctionId,
    span: Span,
    outcome: TypeId,
    locals: Vec<MirLocal>,
    loans: Vec<MirLoan>,
    parameters: Vec<MirLocalId>,
    return_local: MirLocalId,
    entry: MirBlockId,
    unwind: MirBlockId,
    blocks: Vec<MirBasicBlock>,
}

impl MirFunction {
    /// Test-only mutable access for verifier fixtures.
    #[cfg(test)]
    pub fn blocks_mut_for_tests(&mut self) -> &mut Vec<MirBasicBlock> {
        &mut self.blocks
    }

    pub fn id(&self) -> MirFunctionId {
        self.id
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn outcome(&self) -> TypeId {
        self.outcome
    }

    pub fn locals(&self) -> impl ExactSizeIterator<Item = &MirLocal> {
        self.locals.iter()
    }

    /// Locals paired with their request-scoped MIR handles.
    ///
    /// Serialized tooling must derive a stable identity from the source and
    /// semantic owner instead of exposing these handles directly.
    pub fn locals_with_ids(&self) -> impl ExactSizeIterator<Item = (MirLocalId, &MirLocal)> {
        self.locals
            .iter()
            .enumerate()
            .map(|(index, local)| (MirLocalId(index as u32), local))
    }

    pub fn local(&self, id: MirLocalId) -> Option<&MirLocal> {
        self.locals.get(id.0 as usize)
    }

    pub fn loans(&self) -> impl ExactSizeIterator<Item = &MirLoan> {
        self.loans.iter()
    }

    /// Loans paired with their request-scoped MIR handles.
    ///
    /// This is the lossless inspection surface used by semantic tooling; the
    /// numeric handles are not stable serialized identities.
    pub fn loans_with_ids(&self) -> impl ExactSizeIterator<Item = (MirLoanId, &MirLoan)> {
        self.loans
            .iter()
            .enumerate()
            .map(|(index, loan)| (MirLoanId(index as u32), loan))
    }

    pub fn loan(&self, id: MirLoanId) -> Option<&MirLoan> {
        self.loans.get(id.0 as usize)
    }

    pub fn parameters(&self) -> &[MirLocalId] {
        &self.parameters
    }

    pub fn return_local(&self) -> MirLocalId {
        self.return_local
    }

    pub fn entry(&self) -> MirBlockId {
        self.entry
    }

    pub fn unwind(&self) -> MirBlockId {
        self.unwind
    }

    pub fn blocks(&self) -> impl ExactSizeIterator<Item = &MirBasicBlock> {
        self.blocks.iter()
    }

    /// Basic blocks paired with their request-scoped MIR handles.
    pub fn blocks_with_ids(&self) -> impl ExactSizeIterator<Item = (MirBlockId, &MirBasicBlock)> {
        self.blocks
            .iter()
            .enumerate()
            .map(|(index, block)| (MirBlockId(index as u32), block))
    }

    pub fn block(&self, id: MirBlockId) -> Option<&MirBasicBlock> {
        self.blocks.get(id.0 as usize)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MirLocalId(u32);

impl MirLocalId {
    pub fn index(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MirLoanId(u32);

impl MirLoanId {
    pub fn index(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone)]
pub struct MirLocal {
    ty: TypeId,
    span: Span,
    kind: MirLocalKind,
}

impl MirLocal {
    pub fn ty(&self) -> TypeId {
        self.ty
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn kind(&self) -> MirLocalKind {
        self.kind
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirLocalKind {
    Return,
    Parameter { index: u32, source: Option<LocalId> },
    User(LocalId),
    Temporary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MirBlockId(u32);

impl MirBlockId {
    pub fn index(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirBlockKind {
    Normal,
    Cleanup,
}

#[derive(Debug, Clone)]
pub struct MirBasicBlock {
    kind: MirBlockKind,
    statements: Vec<MirStatement>,
    terminator: MirTerminator,
}

impl MirBasicBlock {
    /// Test-only mutable access for verifier fixtures.
    #[cfg(test)]
    pub fn statements_mut_for_tests(&mut self) -> &mut Vec<MirStatement> {
        &mut self.statements
    }

    /// Test-only mutable access for verifier fixtures.
    #[cfg(test)]
    pub fn set_terminator_for_tests(&mut self, terminator: MirTerminator) {
        self.terminator = terminator;
    }

    pub fn kind(&self) -> MirBlockKind {
        self.kind
    }

    pub fn statements(&self) -> &[MirStatement] {
        &self.statements
    }

    pub fn terminator(&self) -> &MirTerminator {
        &self.terminator
    }
}

#[derive(Debug, Clone)]
pub struct MirStatement {
    span: Span,
    kind: MirStatementKind,
}

impl MirStatement {
    pub fn span(&self) -> Span {
        self.span
    }

    pub fn kind(&self) -> &MirStatementKind {
        &self.kind
    }
}

#[derive(Debug, Clone)]
pub enum MirStatementKind {
    StorageLive(MirLocalId),
    StorageDead(MirLocalId),
    ReserveLoan(MirLoanId),
    ReleaseLoan(MirLoanId),
    Assign {
        destination: MirPlace,
        value: MirRvalue,
    },
    RegisterDefer {
        scope: HirScopeId,
        action: MirOperation,
        guard: Option<MirPlace>,
    },
    RegisterFallback {
        scope: HirScopeId,
        owner: MirPlace,
    },
    EnterTaskScope {
        scope: HirScopeId,
    },
    RetargetCleanup {
        from: MirPlace,
        to: MirPlace,
    },
    DisarmCleanup(MirPlace),
    BeginSelect {
        capacity: u32,
    },
    RegisterSelectArm {
        index: u32,
        registration: MirSelectRegistration,
    },
}

/// One arm registration of a selection region.  A selectable call enters
/// its prepare phase; a pending `Join` observes its owner place without
/// consuming it — Join losers remain owned by the caller and runtime-owned
/// call losers are cancelled during rollback, so branch-sensitive moves stay
/// explicit in the winner body.
#[derive(Debug, Clone)]
pub enum MirSelectRegistration {
    Call(MirOperation),
    Join(MirPlace),
}

/// Upper bound on selectable arms per selection region.  The lowering emits
/// one registration per source arm; both MIR and bytecode verifiers reject
/// regions whose arm table exceeds this checked bound.
pub const MAX_SELECT_ARMS: u32 = 64;

/// One committed winner of a selection region: where the operation payload
/// lands (when the arm binds it) and which block runs the arm body.
#[derive(Debug, Clone)]
pub struct MirSelectArm {
    payload: Option<MirPlace>,
    target: MirBlockId,
}

impl MirSelectArm {
    pub fn new(payload: Option<MirPlace>, target: MirBlockId) -> Self {
        Self { payload, target }
    }

    pub fn payload(&self) -> Option<&MirPlace> {
        self.payload.as_ref()
    }

    pub fn target(&self) -> MirBlockId {
        self.target
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirPlace {
    local: MirLocalId,
    ty: TypeId,
    projections: Vec<MirProjection>,
    source_loan: Option<MirLoanId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirLoanKind {
    CallLocal,
    Region,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirLoan {
    kind: MirLoanKind,
    mode: ParameterMode,
    place: MirPlace,
}

impl MirLoan {
    pub fn kind(&self) -> MirLoanKind {
        self.kind
    }

    pub fn mode(&self) -> ParameterMode {
        self.mode
    }

    pub fn place(&self) -> &MirPlace {
        &self.place
    }
}

impl MirPlace {
    pub fn local(&self) -> MirLocalId {
        self.local
    }

    pub fn ty(&self) -> TypeId {
        self.ty
    }

    pub fn projections(&self) -> &[MirProjection] {
        &self.projections
    }

    pub fn source_loan(&self) -> Option<MirLoanId> {
        self.source_loan
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirProjection {
    ty: TypeId,
    kind: MirProjectionKind,
}

impl MirProjection {
    pub fn ty(&self) -> TypeId {
        self.ty
    }

    pub fn kind(&self) -> &MirProjectionKind {
        &self.kind
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MirProjectionKind {
    ClosureCapture {
        closure: HirClosureId,
        index: u32,
    },
    Field(MemberId),
    TupleField(u32),
    NewtypeValue,
    RefValue,
    VariantTuple {
        variant: MemberId,
        index: u32,
    },
    VariantField {
        variant: MemberId,
        field: MemberId,
    },
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
        access: crate::hir::HirIndexAccess,
    },
    Slice {
        start: Option<MirLocalId>,
        end: Option<MirLocalId>,
        step: Option<MirLocalId>,
    },
}

#[derive(Debug, Clone)]
pub struct MirOperand {
    ty: TypeId,
    kind: MirOperandKind,
}

impl MirOperand {
    pub fn ty(&self) -> TypeId {
        self.ty
    }

    pub fn kind(&self) -> &MirOperandKind {
        &self.kind
    }
}

#[derive(Debug, Clone)]
pub enum MirOperandKind {
    Constant(MirConstant),
    Copy(MirPlace),
    Move(MirPlace),
    Borrow(MirPlace),
    Loan(MirLoanId),
    Function {
        callable: HirCallableId,
        arguments: Vec<TypeId>,
    },
    PreludeTraitFunction {
        method: HirPreludeTraitMethod,
        arguments: Vec<TypeId>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MirConstant {
    Unit,
    Bool(bool),
    Integer(String),
    Float(String),
    Char(String),
    String(String),
    Named(SymbolId),
}

#[derive(Debug, Clone)]
pub struct MirRvalue {
    ty: TypeId,
    kind: MirRvalueKind,
}

impl MirRvalue {
    pub fn ty(&self) -> TypeId {
        self.ty
    }

    pub fn kind(&self) -> &MirRvalueKind {
        &self.kind
    }
}

#[derive(Debug, Clone)]
pub enum MirRvalueKind {
    Use(MirOperand),
    Prefix {
        operator: HirPrefixOperator,
        operand: MirOperand,
    },
    Binary {
        operator: HirBinaryOperator,
        left: MirOperand,
        right: MirOperand,
    },
    Aggregate {
        shape: MirAggregateKind,
        values: Vec<MirOperand>,
    },
    RecordUpdate {
        base: MirOperand,
        fields: Vec<(MemberId, MirOperand)>,
    },
    Coerce {
        kind: Assignability,
        value: MirOperand,
    },
    NumericConversion {
        target: ScalarType,
        conversion: NumericConversion,
        value: MirOperand,
    },
    Range {
        kind: HirRangeKind,
        start: MirOperand,
        end: MirOperand,
    },
    Contains {
        kind: HirContainmentKind,
        item: MirOperand,
        container: MirOperand,
    },
    MapRemove {
        map: MirPlace,
        key: MirOperand,
    },
    Interpolate {
        segments: Vec<String>,
        values: Vec<MirOperand>,
    },
    Length(MirOperand),
    IteratorState {
        source: MirOperand,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MirAggregateKind {
    Tuple,
    Array,
    Set,
    Closure {
        closure: HirClosureId,
        arguments: Vec<TypeId>,
    },
    Newtype {
        owner: SymbolId,
    },
    Ref,
    Record {
        owner: SymbolId,
        fields: Vec<MemberId>,
    },
    Variant {
        variant: MemberId,
        fields: Vec<Option<MemberId>>,
    },
    NumericConversionError(NumericConversionErrorVariant),
    OptionNone,
    OptionSome,
    ResultOk,
    ResultErr,
}

#[derive(Debug, Clone)]
pub struct MirOperation {
    ty: TypeId,
    kind: MirOperationKind,
}

impl MirOperation {
    pub fn ty(&self) -> TypeId {
        self.ty
    }

    pub fn kind(&self) -> &MirOperationKind {
        &self.kind
    }
}

#[derive(Debug, Clone)]
pub enum MirOperationKind {
    CheckedPrefix {
        operator: HirPrefixOperator,
        operand: MirOperand,
    },
    CheckedBinary {
        operator: HirBinaryOperator,
        left: MirOperand,
        right: MirOperand,
    },
    ArraySequence {
        kind: crate::hir::HirArraySequenceKind,
        array: MirOperand,
        argument: MirOperand,
    },
    BuildMap {
        entries: Vec<(MirOperand, MirOperand)>,
        reject_dynamic_duplicates: bool,
    },
    Index {
        base: MirOperand,
        index: MirOperand,
        access: crate::hir::HirIndexAccess,
        against: Vec<MirLoanId>,
    },
    Slice {
        base: MirOperand,
        bounds: Box<MirSliceBounds>,
        against: Vec<MirLoanId>,
    },
    Call {
        callee: MirOperand,
        arguments: Vec<MirCallArgument>,
        signature: TypeId,
        protocol: crate::hir::HirCallProtocol,
        unsafe_call: bool,
    },
    ExplicitPanic {
        message: MirOperand,
    },
    Assert {
        condition: MirOperand,
        condition_repr: String,
        message_parts: Vec<MirAssertMessagePart>,
    },
    Format {
        value: MirOperand,
        display: Option<MirOperand>,
    },
    JoinFormat {
        values: MirOperand,
        separator: MirOperand,
        display: Option<MirOperand>,
    },
    BootstrapHostCall {
        function: MirBootstrapHostFunction,
        arguments: Vec<MirOperand>,
    },
}

#[derive(Debug, Clone)]
pub struct MirSliceBounds {
    pub(crate) start: Option<MirOperand>,
    pub(crate) end: Option<MirOperand>,
    pub(crate) step: Option<MirOperand>,
}

impl MirSliceBounds {
    pub fn start(&self) -> Option<&MirOperand> {
        self.start.as_ref()
    }

    pub fn end(&self) -> Option<&MirOperand> {
        self.end.as_ref()
    }

    pub fn step(&self) -> Option<&MirOperand> {
        self.step.as_ref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirBootstrapHostFunction {
    ConsolePrint,
    ConsolePrintln,
    ProcessPipe,
    CommandMergeStderr,
    PipelineMergeStderr,
    ProcessOutputStdout,
    ProcessOutputStderr,
    ProcessOutputCombined,
    ProcessOutputStatuses,
    ExitStatusCode,
    ExitStatusSuccess,
    PointerRead,
    PointerWrite,
    PointerOffset,
    PointerCast,
    PointerAddress,
    PointerFromAddress,
    TestingLog,
    TestingTags,
    TestingFailNow,
    TestingSkip,
    TestingAttach,
    TestingSnapshot,
    TestingRunLeaf,
    TestingRunSuite,
    TestingBeginSuiteCleanup,
}

#[derive(Debug, Clone)]
pub struct MirAssertMessagePart {
    value: MirOperand,
    spread: bool,
}

impl MirAssertMessagePart {
    pub fn value(&self) -> &MirOperand {
        &self.value
    }

    pub fn is_spread(&self) -> bool {
        self.spread
    }
}

#[derive(Debug, Clone)]
pub struct MirCallArgument {
    mode: ParameterMode,
    target: HirCallArgumentTarget,
    value: MirOperand,
}

impl MirCallArgument {
    pub fn mode(&self) -> ParameterMode {
        self.mode
    }

    pub fn target(&self) -> HirCallArgumentTarget {
        self.target
    }

    pub fn value(&self) -> &MirOperand {
        &self.value
    }
}

#[derive(Debug, Clone)]
pub struct MirTerminator {
    span: Span,
    kind: MirTerminatorKind,
}

impl MirTerminator {
    pub fn span(&self) -> Span {
        self.span
    }

    pub fn kind(&self) -> &MirTerminatorKind {
        &self.kind
    }
}

#[derive(Debug, Clone)]
pub enum MirTerminatorKind {
    Goto {
        target: MirBlockId,
    },
    SwitchBool {
        condition: MirOperand,
        if_true: MirBlockId,
        if_false: MirBlockId,
    },
    SwitchTag {
        value: MirOperand,
        cases: Vec<(MirTag, MirBlockId)>,
        otherwise: MirBlockId,
    },
    Invoke {
        operation: MirOperation,
        destination: Option<MirPlace>,
        target: Option<MirBlockId>,
        unwind: MirBlockId,
    },
    Await {
        awaitable: MirAwaitable,
        destination: MirPlace,
        target: MirBlockId,
        unwind: MirBlockId,
    },
    Spawn {
        operation: MirOperation,
        scope: HirScopeId,
        kind: HirSpawnKind,
        destination: MirPlace,
        target: MirBlockId,
        unwind: MirBlockId,
    },
    IteratorNext {
        state: MirPlace,
        destination: MirPlace,
        borrowed_source: Option<MirPlace>,
        exhaustion_guard: Option<MirPlace>,
        has_value: MirBlockId,
        exhausted: MirBlockId,
        unwind: MirBlockId,
    },
    ValidatePlaces {
        places: Vec<MirPlace>,
        replacements: Vec<Option<MirOperand>>,
        against: Vec<Vec<MirLoanId>>,
        for_write: bool,
        target: MirBlockId,
        unwind: MirBlockId,
    },
    ValidateLoan {
        loan: MirLoanId,
        against: Vec<MirLoanId>,
        target: MirBlockId,
        unwind: MirBlockId,
    },
    DrainDefers {
        scopes: Vec<HirScopeId>,
        target: MirBlockId,
        unwind: MirBlockId,
    },
    DrainScopes {
        task_scopes: Vec<HirScopeId>,
        defer_scopes: Vec<HirScopeId>,
        target: MirBlockId,
        unwind: MirBlockId,
    },
    DrainUnwind {
        target: MirBlockId,
    },
    CommitSelect {
        arms: Vec<MirSelectArm>,
        else_target: Option<MirBlockId>,
        unwind: MirBlockId,
    },
    Return,
    ResumePanic,
    Unreachable,
}

#[derive(Debug, Clone)]
pub enum MirAwaitable {
    Call(MirOperation),
    Join(MirOperand),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MirTag {
    OptionNone,
    OptionSome,
    ResultOk,
    ResultErr,
    Variant(MemberId),
    NumericConversionError(NumericConversionErrorVariant),
    Union(TypeId),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_mir_summary_is_deterministic_and_serializable() {
        let program = MirProgram {
            functions: BTreeMap::new(),
        };
        let summary = program.summary();
        assert_eq!(summary, MirSummary::default());
        let bytes = serde_json::to_vec(&summary).unwrap();
        assert_eq!(
            serde_json::from_slice::<MirSummary>(&bytes).unwrap(),
            summary
        );
    }

    #[test]
    fn backend_control_flow_accepts_branch_joins() {
        let blocks = vec![
            MirBackendBlock {
                ordinal: 0,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::SwitchBool {
                    condition: MirBackendOperand::Constant(MirBackendConstant::Bool(true)),
                    if_true: 1,
                    if_false: 2,
                },
            },
            MirBackendBlock {
                ordinal: 1,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Goto { target: 3 },
            },
            MirBackendBlock {
                ordinal: 2,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Goto { target: 3 },
            },
            MirBackendBlock {
                ordinal: 3,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Return,
            },
        ];
        let mut unsupported = Vec::new();
        validate_backend_control_flow(&blocks, &mut unsupported);
        assert!(
            unsupported.is_empty(),
            "unexpected unsupported: {unsupported:?}"
        );
    }

    #[test]
    fn backend_control_flow_accepts_loop_cycles_for_scalar_lowering() {
        let blocks = vec![
            MirBackendBlock {
                ordinal: 0,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Goto { target: 1 },
            },
            MirBackendBlock {
                ordinal: 1,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Goto { target: 0 },
            },
        ];
        let mut unsupported = Vec::new();
        validate_backend_control_flow(&blocks, &mut unsupported);
        assert!(
            unsupported.is_empty(),
            "unexpected unsupported: {unsupported:?}"
        );
    }

    #[test]
    fn backend_control_flow_rejects_unlowered_cleanup_actions() {
        let blocks = vec![
            MirBackendBlock {
                ordinal: 0,
                kind: "normal".to_owned(),
                statements: Vec::new(),
                terminator: MirBackendTerminator::Return,
            },
            MirBackendBlock {
                ordinal: 1,
                kind: "cleanup".to_owned(),
                statements: vec![MirBackendStatement::Marker {
                    kind: "release-loan".to_owned(),
                }],
                terminator: MirBackendTerminator::Marker {
                    kind: "resume-panic".to_owned(),
                },
            },
        ];
        let mut unsupported = Vec::new();
        validate_backend_control_flow(&blocks, &mut unsupported);
        assert!(
            unsupported
                .iter()
                .any(|reason| { reason == "cleanup:resource-action:release-loan" })
        );
    }
}
