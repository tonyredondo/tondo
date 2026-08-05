//! Semantic high-level representation produced after name and type lowering.
//!
//! The typed portion keeps source identity, resolved names, canonical types,
//! value categories, and explicit contextual coercions. Ownership dataflow,
//! control-flow cleanup, and runtime layout remain later MIR/runtime concerns.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use crate::diagnostics::{Diagnostic, DiagnosticError};
use crate::package::{ModuleId, Name, PackageGraphError, SymbolIdentity};
use crate::resolve::{LocalId, MemberId, SymbolId};
use crate::source::{FileId, SourceError, Span, TextRange};
use crate::types::{
    Assignability, FunctionParameter, FunctionType, GeneratedTypeIdentity, GeneratedTypeKind,
    InferenceError, IntrinsicType, NumericConversion, NumericConversionErrorVariant, ParameterMode,
    ScalarType, TypeError, TypeId, TypeInterner, TypeKind, TypeSubstitution,
};

mod availability;
mod capabilities;
mod check;
mod const_eval;
mod lower;
mod regions;
mod terminal;
mod termination;
mod traits;
mod verify;

pub(crate) use availability::{
    AvailabilityFindingKind, analyze_availability, analyze_closure_captures,
};
pub(crate) use capabilities::{CapabilityAnalysis, CapabilityAssumptions};
pub(crate) use check::check_expressions_configured;
pub use check::{ExpressionCheckLimits, HirCheckOutput, check_expressions};
pub use lower::{TypeLoweringLimits, lower_types};
pub(crate) use regions::{
    StaticCollectionRegion, StaticRegionRelation, StaticSlice, parse_nonnegative_integer,
    static_collection_relation, static_nonnegative_integer, static_slice,
};
pub(crate) use terminal::TerminalAnalysis;
pub use terminal::{
    HirTerminalContract, HirTerminalOperation, HirTerminalStatus, HirTerminalUnwindAction,
};

fn bootstrap_process_intrinsic(module: &ModuleId, name: &Name) -> Option<IntrinsicType> {
    if module.package().as_str() != "toolchain:std:0.1-bootstrap" {
        return None;
    }
    match module.path().as_str() {
        "bytes" => Some(match name.as_str() {
            "Bytes" => IntrinsicType::Bytes,
            "BytesBuilder" => IntrinsicType::BytesBuilder,
            "BytesError" => IntrinsicType::BytesError,
            "Utf8Error" => IntrinsicType::Utf8Error,
            _ => return None,
        }),
        "format" => Some(match name.as_str() {
            "Builder" => IntrinsicType::FormatBuilder,
            "FormatError" => IntrinsicType::FormatError,
            _ => return None,
        }),
        "text" => Some(match name.as_str() {
            "TextError" => IntrinsicType::TextError,
            _ => return None,
        }),
        "collections" => Some(match name.as_str() {
            "CollectionError" => IntrinsicType::CollectionError,
            _ => return None,
        }),
        "path" => Some(match name.as_str() {
            "Path" => IntrinsicType::Path,
            "PathError" => IntrinsicType::PathError,
            _ => return None,
        }),
        "fs" => Some(match name.as_str() {
            "FsError" => IntrinsicType::FsError,
            _ => return None,
        }),
        "math" => Some(match name.as_str() {
            "MathError" => IntrinsicType::MathError,
            _ => return None,
        }),
        "process" => Some(match name.as_str() {
            "Command" => IntrinsicType::Command,
            "Pipeline" => IntrinsicType::Pipeline,
            // Kept as a source-compatibility bridge for the bootstrap process
            // contract. The canonical owner is now std.bytes.Bytes.
            "Bytes" => IntrinsicType::Bytes,
            "ExitStatus" => IntrinsicType::ExitStatus,
            "ProcessOutput" => IntrinsicType::ProcessOutput,
            "ProcessHandle" => IntrinsicType::ProcessHandle,
            "ProcessError" => IntrinsicType::ProcessError,
            "ProcessExitError" => IntrinsicType::ProcessExitError,
            "Utf8Error" => IntrinsicType::Utf8Error,
            _ => return None,
        }),
        "time" => Some(match name.as_str() {
            "Duration" => IntrinsicType::Duration,
            "Instant" => IntrinsicType::Instant,
            "Timer" => IntrinsicType::Timer,
            "DurationError" => IntrinsicType::DurationError,
            "ClockError" => IntrinsicType::ClockError,
            _ => return None,
        }),
        "env" => Some(match name.as_str() {
            "Snapshot" => IntrinsicType::EnvSnapshot,
            "Name" => IntrinsicType::EnvName,
            "Value" => IntrinsicType::EnvValue,
            "EnvError" => IntrinsicType::EnvError,
            _ => return None,
        }),
        "testing" => Some(match name.as_str() {
            "VirtualTime" => IntrinsicType::VirtualTime,
            "FloatTolerance" => IntrinsicType::FloatTolerance,
            "FloatToleranceError" => IntrinsicType::FloatToleranceError,
            "TextDiff" => IntrinsicType::TextDiff,
            "TempDirectory" => IntrinsicType::TempDirectory,
            "TempError" => IntrinsicType::TempError,
            "Generator" => IntrinsicType::Generator,
            "GenerationId" => IntrinsicType::GenerationId,
            "GenerationError" => IntrinsicType::GenerationError,
            _ => return None,
        }),
        "io" => Some(match name.as_str() {
            "Reader" => IntrinsicType::Reader,
            "Writer" => IntrinsicType::Writer,
            "IoLimits" => IntrinsicType::IoLimits,
            "IoError" => IntrinsicType::IoError,
            _ => return None,
        }),
        "console" => Some(match name.as_str() {
            "ConsoleError" => IntrinsicType::ConsoleError,
            _ => return None,
        }),
        _ => None,
    }
}
pub(crate) use traits::{TraitQuery, TraitSelectionError, select_implementation};
pub use verify::HirInvariantError;
pub(crate) use verify::verify_typed_hir;

#[derive(Debug)]
pub enum HirError {
    DiagnosticLimit { file: FileId, offset: u32 },
    NodeLimit { file: FileId, offset: u32 },
    PatternAnalysisLimit { file: FileId, offset: u32 },
    TraitObligationLimit { file: FileId, offset: u32 },
    TraitTerminationInvariant { message: String },
    TraitSelectionInvariant { message: String },
    TextInvariant { message: String },
    Invariant(HirInvariantError),
    Diagnostic(DiagnosticError),
    Package(PackageGraphError),
    Source(SourceError),
    Type(TypeError),
    Inference(InferenceError),
}

impl fmt::Display for HirError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DiagnosticLimit { file, offset } => write!(
                formatter,
                "HIR diagnostic limit exceeded in file {file} at byte {offset}"
            ),
            Self::NodeLimit { file, offset } => write!(
                formatter,
                "typed HIR node limit exceeded in file {file} at byte {offset}"
            ),
            Self::PatternAnalysisLimit { file, offset } => write!(
                formatter,
                "pattern analysis limit exceeded in file {file} at byte {offset}"
            ),
            Self::TraitObligationLimit { file, offset } => write!(
                formatter,
                "trait obligation limit exceeded in file {file} at byte {offset}"
            ),
            Self::TraitTerminationInvariant { message } => {
                write!(formatter, "trait termination invariant failed: {message}")
            }
            Self::TraitSelectionInvariant { message } => {
                write!(formatter, "trait selection invariant failed: {message}")
            }
            Self::TextInvariant { message } => {
                write!(formatter, "text invariant failed: {message}")
            }
            Self::Invariant(error) => error.fmt(formatter),
            Self::Diagnostic(error) => error.fmt(formatter),
            Self::Package(error) => error.fmt(formatter),
            Self::Source(error) => error.fmt(formatter),
            Self::Type(error) => error.fmt(formatter),
            Self::Inference(error) => error.fmt(formatter),
        }
    }
}

impl Error for HirError {}

impl From<HirInvariantError> for HirError {
    fn from(error: HirInvariantError) -> Self {
        Self::Invariant(error)
    }
}

impl From<DiagnosticError> for HirError {
    fn from(error: DiagnosticError) -> Self {
        Self::Diagnostic(error)
    }
}

impl From<PackageGraphError> for HirError {
    fn from(error: PackageGraphError) -> Self {
        Self::Package(error)
    }
}

impl From<SourceError> for HirError {
    fn from(error: SourceError) -> Self {
        Self::Source(error)
    }
}

impl From<TypeError> for HirError {
    fn from(error: TypeError) -> Self {
        Self::Type(error)
    }
}

impl From<InferenceError> for HirError {
    fn from(error: InferenceError) -> Self {
        Self::Inference(error)
    }
}

#[derive(Debug)]
pub struct HirOutput {
    program: HirProgram,
    diagnostics: Vec<Diagnostic>,
}

impl HirOutput {
    pub fn program(&self) -> &HirProgram {
        &self.program
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn into_parts(self) -> (HirProgram, Vec<Diagnostic>) {
        (self.program, self.diagnostics)
    }
}

#[derive(Debug)]
pub struct HirProgram {
    interner: TypeInterner,
    declarations: BTreeMap<SymbolId, HirTypeDeclaration>,
    constants: BTreeMap<SymbolId, HirConstant>,
    callables: Vec<HirCallableSignature>,
    implementations: Vec<HirImplementation>,
    derive_requests: Vec<HirDeriveRequest>,
    annotations: BTreeMap<(FileId, u32, u32), TypeId>,
    expressions: Vec<HirExpression>,
    expression_flows: Vec<HirFlow>,
    expression_breaks: Vec<Vec<HirLoopId>>,
    member_references: Vec<HirMemberReference>,
    unsafe_regions: Vec<Span>,
    patterns: Vec<HirPattern>,
    bodies: BTreeMap<HirCallableId, HirBody>,
    closures: Vec<HirClosure>,
    local_types: BTreeMap<LocalId, TypeId>,
    capability_statuses: Vec<[HirCapabilityStatus; HirCapability::COUNT]>,
    terminal_statuses: Vec<HirTerminalStatus>,
    expression_check_complete: bool,
}

impl HirProgram {
    pub fn interner(&self) -> &TypeInterner {
        &self.interner
    }

    pub fn declaration(&self, symbol: SymbolId) -> Option<&HirTypeDeclaration> {
        self.declarations.get(&symbol)
    }

    pub fn declarations(&self) -> impl ExactSizeIterator<Item = (&SymbolId, &HirTypeDeclaration)> {
        self.declarations.iter()
    }

    pub fn constant(&self, symbol: SymbolId) -> Option<&HirConstant> {
        self.constants.get(&symbol)
    }

    pub fn constants(&self) -> impl ExactSizeIterator<Item = (&SymbolId, &HirConstant)> {
        self.constants.iter()
    }

    pub fn callables(&self) -> impl ExactSizeIterator<Item = &HirCallableSignature> {
        self.callables.iter()
    }

    /// Parsed `derive` requests in source order. Semantic provider selection
    /// and target validation are performed by the meta-semantic phase; HIR
    /// keeps the lossless, typed request visible without executing it.
    pub fn derive_requests(&self) -> &[HirDeriveRequest] {
        &self.derive_requests
    }

    pub fn callable(&self, id: HirCallableId) -> Option<&HirCallableSignature> {
        self.callables.iter().find(|callable| callable.id == id)
    }

    pub fn opaque_result(&self, identity: &SymbolIdentity) -> Option<&HirOpaqueResult> {
        self.callables
            .iter()
            .filter_map(|callable| callable.opaque_result.as_ref())
            .find(|opaque| opaque.identity == *identity)
    }

    pub(crate) fn opaque_exposes_capability(
        &self,
        identity: &SymbolIdentity,
        capability: HirCapability,
    ) -> bool {
        self.opaque_result(identity).is_some_and(|opaque| {
            capabilities::bounds_imply_capability(self, opaque.bounds(), capability)
        })
    }

    pub(crate) fn opaque_witness_for(
        &self,
        interner: &mut TypeInterner,
        ty: TypeId,
    ) -> Result<Option<TypeId>, TypeError> {
        let TypeKind::OpaqueResult {
            identity,
            arguments,
        } = interner.kind(ty)?.clone()
        else {
            return Ok(None);
        };
        let Some(witness) = self
            .opaque_result(&identity)
            .and_then(HirOpaqueResult::witness)
        else {
            return Ok(None);
        };
        TypeSubstitution::new(arguments)
            .apply(interner, witness)
            .map(Some)
    }

    pub(crate) fn opaque_coercion_matches(
        &self,
        interner: &mut TypeInterner,
        actual: TypeId,
        expected: TypeId,
    ) -> Result<bool, TypeError> {
        if self.opaque_witness_for(interner, expected)? == Some(actual) {
            return Ok(true);
        }
        let actual_kind = interner.kind(actual)?.clone();
        let expected_kind = interner.kind(expected)?.clone();
        let (
            TypeKind::Result {
                success: actual_success,
                error: actual_error,
            },
            TypeKind::Result {
                success: expected_success,
                error: expected_error,
            },
        ) = (actual_kind, expected_kind)
        else {
            return Ok(false);
        };
        Ok(actual_error == expected_error
            && self.opaque_witness_for(interner, expected_success)? == Some(actual_success))
    }

    pub(crate) fn opaque_representation_for(
        &self,
        interner: &mut TypeInterner,
        root: TypeId,
    ) -> Result<TypeId, TypeError> {
        interner.kind(root)?;
        let mut memo = BTreeMap::<TypeId, TypeId>::new();
        let mut active = BTreeSet::new();
        let mut pending = vec![(root, false)];
        while let Some((original, expanded)) = pending.pop() {
            if memo.contains_key(&original) {
                continue;
            }
            if !expanded && !active.insert(original) {
                return Err(TypeError::CyclicOpaqueRepresentation);
            }
            let kind = interner.kind(original)?.clone();
            if !expanded {
                match &kind {
                    TypeKind::Error
                    | TypeKind::Scalar(_)
                    | TypeKind::GenericParameter(_)
                    | TypeKind::Inference(_) => {
                        active.remove(&original);
                        memo.insert(original, original);
                    }
                    TypeKind::OpaqueResult { .. } => {
                        let witness = self
                            .opaque_witness_for(interner, original)?
                            .ok_or(TypeError::CyclicOpaqueRepresentation)?;
                        pending.push((original, true));
                        pending.push((witness, false));
                    }
                    _ => {
                        pending.push((original, true));
                        push_opaque_representation_children(&kind, &mut pending);
                    }
                }
                continue;
            }

            let get = |ty: TypeId| {
                memo.get(&ty)
                    .copied()
                    .expect("representation children are rebuilt before their parent")
            };
            let represented = match kind {
                TypeKind::OpaqueResult { .. } => {
                    let witness = self
                        .opaque_witness_for(interner, original)?
                        .ok_or(TypeError::CyclicOpaqueRepresentation)?;
                    get(witness)
                }
                TypeKind::Nominal {
                    identity,
                    arguments,
                } => interner.nominal(identity, arguments.into_iter().map(get).collect())?,
                TypeKind::Tuple(items) => interner.tuple(items.into_iter().map(get).collect())?,
                TypeKind::Function(function) => interner.function(FunctionType::new(
                    function.is_async(),
                    function.is_unsafe(),
                    function
                        .parameters()
                        .iter()
                        .map(|parameter| {
                            FunctionParameter::new(parameter.mode(), get(parameter.ty()))
                        })
                        .collect(),
                    function.variadic().map(get),
                    get(function.outcome()),
                ))?,
                TypeKind::Option(item) => interner.option(get(item))?,
                TypeKind::Result { success, error } => interner.result(get(success), get(error))?,
                TypeKind::Union(members) => interner.union(members.into_iter().map(get))?,
                TypeKind::Intrinsic {
                    constructor,
                    arguments,
                } => interner.intrinsic(constructor, arguments.into_iter().map(get).collect())?,
                TypeKind::Generated {
                    identity,
                    arguments,
                } => interner.generated(identity, arguments.into_iter().map(get).collect())?,
                TypeKind::Cursor { mode, collection } => interner.cursor(mode, get(collection))?,
                TypeKind::Error
                | TypeKind::Scalar(_)
                | TypeKind::GenericParameter(_)
                | TypeKind::Inference(_) => original,
            };
            active.remove(&original);
            memo.insert(original, represented);
        }
        Ok(memo[&root])
    }

    pub fn implementations(&self) -> impl ExactSizeIterator<Item = &HirImplementation> {
        self.implementations.iter()
    }

    pub fn implementation(&self, id: HirImplementationId) -> Option<&HirImplementation> {
        self.implementations
            .get(id.0 as usize)
            .filter(|implementation| implementation.id == id)
    }

    pub fn implementation_method(
        &self,
        id: HirImplementationMethodId,
    ) -> Option<&HirImplementationMethod> {
        self.implementation(id.implementation)?
            .methods
            .get(id.index as usize)
            .filter(|method| method.id == id)
    }

    pub fn type_at(&self, file: FileId, range: TextRange) -> Option<TypeId> {
        self.annotations
            .get(&(file, range.start(), range.end()))
            .copied()
    }

    pub fn expression(&self, id: HirExpressionId) -> Option<&HirExpression> {
        self.expressions.get(id.0 as usize)
    }

    pub fn expressions(&self) -> impl ExactSizeIterator<Item = &HirExpression> {
        self.expressions.iter()
    }

    pub fn expressions_with_ids(
        &self,
    ) -> impl ExactSizeIterator<Item = (HirExpressionId, &HirExpression)> {
        self.expressions
            .iter()
            .enumerate()
            .map(|(index, expression)| {
                (
                    HirExpressionId(u32::try_from(index).expect("HIR expression IDs fit in u32")),
                    expression,
                )
            })
    }

    pub fn expression_at(
        &self,
        file: FileId,
        range: TextRange,
    ) -> Option<(HirExpressionId, &HirExpression)> {
        self.expressions
            .iter()
            .enumerate()
            .rev()
            .find(|(_, expression)| {
                expression.span.file() == file && expression.span.range() == range
            })
            .map(|(index, expression)| {
                (
                    HirExpressionId(u32::try_from(index).expect("HIR expression IDs fit in u32")),
                    expression,
                )
            })
    }

    pub fn expression_covering(
        &self,
        file: FileId,
        range: TextRange,
    ) -> Option<(HirExpressionId, &HirExpression)> {
        self.expressions
            .iter()
            .enumerate()
            .filter(|(_, expression)| {
                expression.span.file() == file
                    && range_contains_range(expression.span.range(), range)
            })
            .min_by_key(|(index, expression)| {
                (
                    expression
                        .span
                        .range()
                        .end()
                        .saturating_sub(expression.span.range().start()),
                    std::cmp::Reverse(*index),
                )
            })
            .map(|(index, expression)| {
                (
                    HirExpressionId(u32::try_from(index).expect("HIR expression IDs fit in u32")),
                    expression,
                )
            })
    }

    pub fn expression_containing(
        &self,
        file: FileId,
        offset: u32,
    ) -> Option<(HirExpressionId, &HirExpression)> {
        self.expressions
            .iter()
            .enumerate()
            .filter(|(_, expression)| {
                expression.span.file() == file
                    && range_contains_offset(expression.span.range(), offset)
            })
            .min_by_key(|(index, expression)| {
                (
                    expression
                        .span
                        .range()
                        .end()
                        .saturating_sub(expression.span.range().start()),
                    std::cmp::Reverse(*index),
                )
            })
            .map(|(index, expression)| {
                (
                    HirExpressionId(u32::try_from(index).expect("HIR expression IDs fit in u32")),
                    expression,
                )
            })
    }

    pub fn member_references(&self) -> impl ExactSizeIterator<Item = &HirMemberReference> {
        self.member_references.iter()
    }

    /// Source regions whose bodies were checked with raw operations enabled.
    ///
    /// These spans retain the complete `unsafe { ... }` expression rather
    /// than only its lowered block so semantic tooling can make the boundary
    /// visible without reconstructing it from tokens.
    pub fn unsafe_regions(&self) -> impl ExactSizeIterator<Item = Span> + '_ {
        self.unsafe_regions.iter().copied()
    }

    pub fn expression_flow(&self, id: HirExpressionId) -> Option<HirFlow> {
        self.expression_flows.get(id.0 as usize).copied()
    }

    pub fn expression_break_targets(&self, id: HirExpressionId) -> Option<&[HirLoopId]> {
        self.expression_breaks.get(id.0 as usize).map(Vec::as_slice)
    }

    pub fn pattern(&self, id: HirPatternId) -> Option<&HirPattern> {
        self.patterns.get(id.0 as usize)
    }

    pub fn body(&self, callable: HirCallableId) -> Option<&HirBody> {
        self.bodies.get(&callable)
    }

    pub fn closure(&self, id: HirClosureId) -> Option<&HirClosure> {
        self.closures
            .get(id.0 as usize)
            .filter(|closure| closure.id == id)
    }

    pub fn closures(&self) -> impl ExactSizeIterator<Item = &HirClosure> {
        self.closures.iter()
    }

    pub(crate) fn closure_by_identity(
        &self,
        identity: &GeneratedTypeIdentity,
    ) -> Option<&HirClosure> {
        self.closures
            .iter()
            .find(|closure| closure.identity == *identity)
    }

    pub fn local_type(&self, local: LocalId) -> Option<TypeId> {
        self.local_types.get(&local).copied()
    }

    pub fn capability_status(
        &self,
        ty: TypeId,
        capability: HirCapability,
    ) -> Option<HirCapabilityStatus> {
        self.capability_statuses
            .get(ty.index() as usize)
            .map(|statuses| statuses[capability.index()])
    }

    pub fn discard_status(&self, ty: TypeId) -> Option<HirCapabilityStatus> {
        self.capability_status(ty, HirCapability::Discard)
    }

    pub fn terminal_status(&self, ty: TypeId) -> Option<HirTerminalStatus> {
        self.terminal_statuses.get(ty.index() as usize).copied()
    }

    pub fn direct_terminal_contract(
        &self,
        ty: TypeId,
    ) -> Result<Option<HirTerminalContract>, TypeError> {
        let TypeKind::Intrinsic { constructor, .. } = self.interner.kind(ty)? else {
            return Ok(None);
        };
        Ok(terminal::intrinsic_terminal_contract(*constructor))
    }

    pub fn expression_check_complete(&self) -> bool {
        self.expression_check_complete
    }
}

fn push_opaque_representation_children(kind: &TypeKind, pending: &mut Vec<(TypeId, bool)>) {
    let mut push = |ty| pending.push((ty, false));
    match kind {
        TypeKind::Nominal { arguments, .. }
        | TypeKind::Tuple(arguments)
        | TypeKind::Union(arguments)
        | TypeKind::Intrinsic { arguments, .. }
        | TypeKind::Generated { arguments, .. } => {
            for argument in arguments.iter().rev() {
                push(*argument);
            }
        }
        TypeKind::Function(function) => {
            push(function.outcome());
            if let Some(variadic) = function.variadic() {
                push(variadic);
            }
            for parameter in function.parameters().iter().rev() {
                push(parameter.ty());
            }
        }
        TypeKind::Option(item) => push(*item),
        TypeKind::Result { success, error } => {
            push(*error);
            push(*success);
        }
        TypeKind::Cursor { collection, .. } => push(*collection),
        TypeKind::Error
        | TypeKind::Scalar(_)
        | TypeKind::GenericParameter(_)
        | TypeKind::Inference(_)
        | TypeKind::OpaqueResult { .. } => {}
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum HirCapabilityStatus {
    Satisfied,
    Deferred,
    Unsatisfied,
}

pub type HirDiscardStatus = HirCapabilityStatus;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HirCapability {
    Copy,
    Discard,
    Equatable,
    Key,
    Send,
    Share,
}

impl HirCapability {
    pub const ALL: [Self; 6] = [
        Self::Copy,
        Self::Discard,
        Self::Equatable,
        Self::Key,
        Self::Send,
        Self::Share,
    ];
    pub const COUNT: usize = Self::ALL.len();

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Copy => "Copy",
            Self::Discard => "Discard",
            Self::Equatable => "Equatable",
            Self::Key => "Key",
            Self::Send => "Send",
            Self::Share => "Share",
        }
    }

    const fn index(self) -> usize {
        self as usize
    }

    pub(crate) fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "Copy" => Self::Copy,
            "Discard" => Self::Discard,
            "Equatable" => Self::Equatable,
            "Key" => Self::Key,
            "Send" => Self::Send,
            "Share" => Self::Share,
            _ => return None,
        })
    }
}

fn range_contains_offset(range: TextRange, offset: u32) -> bool {
    if range.start() == range.end() {
        offset == range.start()
    } else {
        range.start() <= offset && offset < range.end()
    }
}

fn range_contains_range(container: TextRange, query: TextRange) -> bool {
    container.start() <= query.start() && query.end() <= container.end()
}

#[derive(Debug, Clone)]
pub struct HirConstant {
    symbol: SymbolId,
    span: Span,
    declared_type: Option<TypeId>,
    initializer: Span,
    ty: Option<TypeId>,
    value: Option<HirExpressionId>,
    evaluated: Option<HirConstantValue>,
}

impl HirConstant {
    pub fn symbol(&self) -> SymbolId {
        self.symbol
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn declared_type(&self) -> Option<TypeId> {
        self.declared_type
    }

    pub fn initializer(&self) -> Span {
        self.initializer
    }

    pub fn ty(&self) -> Option<TypeId> {
        self.ty
    }

    pub fn value(&self) -> Option<HirExpressionId> {
        self.value
    }

    pub fn evaluated(&self) -> Option<&HirConstantValue> {
        self.evaluated.as_ref()
    }
}

#[derive(Debug, Clone)]
pub struct HirConstantValue {
    ty: TypeId,
    kind: HirConstantValueKind,
}

impl HirConstantValue {
    pub fn ty(&self) -> TypeId {
        self.ty
    }

    pub fn kind(&self) -> &HirConstantValueKind {
        &self.kind
    }
}

#[derive(Debug, Clone)]
pub enum HirConstantValueKind {
    Unit,
    Bool(bool),
    Integer(i128),
    Float(u64),
    Char(char),
    String(String),
    Function {
        callable: HirCallableId,
        arguments: Vec<TypeId>,
    },
    Tuple(Vec<HirConstantValue>),
    Array(Vec<HirConstantValue>),
    Map(Vec<(HirConstantValue, HirConstantValue)>),
    Set(Vec<HirConstantValue>),
    Newtype {
        constructor: SymbolId,
        value: Box<HirConstantValue>,
    },
    Record {
        owner: SymbolId,
        fields: Vec<HirConstantFieldValue>,
    },
    Variant {
        variant: MemberId,
        payload: HirConstantVariantValue,
    },
    NumericConversionError(NumericConversionErrorVariant),
    OptionNone,
    OptionSome(Box<HirConstantValue>),
    ResultOk(Box<HirConstantValue>),
    ResultErr(Box<HirConstantValue>),
    Range {
        kind: HirRangeKind,
        start: Box<HirConstantValue>,
        end: Box<HirConstantValue>,
    },
    Converted(Box<HirConstantValue>),
}

#[derive(Debug, Clone)]
pub struct HirConstantFieldValue {
    member: MemberId,
    value: HirConstantValue,
}

impl HirConstantFieldValue {
    pub fn member(&self) -> MemberId {
        self.member
    }

    pub fn value(&self) -> &HirConstantValue {
        &self.value
    }
}

#[derive(Debug, Clone)]
pub enum HirConstantVariantValue {
    Unit,
    Tuple(Vec<HirConstantValue>),
    Record(Vec<HirConstantFieldValue>),
}

#[derive(Debug, Clone)]
pub struct HirTypeDeclaration {
    symbol: SymbolId,
    span: Span,
    parameters: Vec<HirGenericParameter>,
    kind: HirTypeDeclarationKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HirDeriveRequest {
    span: Span,
    generic_parameters: Vec<String>,
    traits: Vec<String>,
    target: String,
}

impl HirDeriveRequest {
    pub fn span(&self) -> Span {
        self.span
    }

    pub fn generic_parameters(&self) -> &[String] {
        &self.generic_parameters
    }

    pub fn traits(&self) -> &[String] {
        &self.traits
    }

    pub fn target(&self) -> &str {
        &self.target
    }
}

impl HirTypeDeclaration {
    pub fn symbol(&self) -> SymbolId {
        self.symbol
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn parameters(&self) -> &[HirGenericParameter] {
        &self.parameters
    }

    pub fn kind(&self) -> &HirTypeDeclarationKind {
        &self.kind
    }
}

#[derive(Debug, Clone)]
pub enum HirTypeDeclarationKind {
    Alias { target: TypeId },
    Nominal(HirNominalDefinition),
    Trait(HirTraitDefinition),
}

#[derive(Debug, Clone)]
pub struct HirTraitDefinition {
    self_type: TypeId,
    methods: Vec<HirTraitMethod>,
}

impl HirTraitDefinition {
    pub fn self_type(&self) -> TypeId {
        self.self_type
    }

    pub fn methods(&self) -> &[HirTraitMethod] {
        &self.methods
    }
}

#[derive(Debug, Clone)]
pub struct HirTraitMethod {
    member: MemberId,
    has_default: bool,
    requires_self_send: bool,
}

impl HirTraitMethod {
    pub fn member(&self) -> MemberId {
        self.member
    }

    pub fn has_default(&self) -> bool {
        self.has_default
    }

    pub fn requires_self_send(&self) -> bool {
        self.requires_self_send
    }
}

#[derive(Debug, Clone)]
pub struct HirNominalDefinition {
    self_type: TypeId,
    shape: HirNominalShape,
}

impl HirNominalDefinition {
    pub fn self_type(&self) -> TypeId {
        self.self_type
    }

    pub fn shape(&self) -> &HirNominalShape {
        &self.shape
    }
}

#[derive(Debug, Clone)]
pub enum HirNominalShape {
    Newtype { underlying: TypeId },
    Record { fields: Vec<HirField> },
    Enum { variants: Vec<HirVariant> },
}

#[derive(Debug, Clone)]
pub struct HirField {
    member: MemberId,
    ty: TypeId,
}

impl HirField {
    pub fn member(&self) -> MemberId {
        self.member
    }

    pub fn ty(&self) -> TypeId {
        self.ty
    }
}

#[derive(Debug, Clone)]
pub struct HirVariant {
    member: MemberId,
    payload: HirVariantPayload,
}

impl HirVariant {
    pub fn member(&self) -> MemberId {
        self.member
    }

    pub fn payload(&self) -> &HirVariantPayload {
        &self.payload
    }
}

#[derive(Debug, Clone)]
pub enum HirVariantPayload {
    Unit,
    Tuple(Vec<TypeId>),
    Record(Vec<HirField>),
}

#[derive(Debug, Clone)]
pub struct HirGenericParameter {
    local: LocalId,
    position: u32,
    bounds: Vec<HirTraitReference>,
}

impl HirGenericParameter {
    pub fn local(&self) -> LocalId {
        self.local
    }

    pub fn position(&self) -> u32 {
        self.position
    }

    pub fn bounds(&self) -> &[HirTraitReference] {
        &self.bounds
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum HirTraitConstructor {
    Symbol(SymbolId),
    Prelude(Name),
    External(SymbolIdentity),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum HirTraitIdentity {
    Symbol(SymbolIdentity),
    Prelude(Name),
}

impl HirTraitIdentity {
    pub(crate) fn canonical_name(&self) -> String {
        match self {
            Self::Symbol(identity) => identity.canonical_name(),
            Self::Prelude(name) => name.as_str().to_owned(),
        }
    }

    pub(crate) fn is_closed_prelude(&self) -> bool {
        matches!(
            self,
            Self::Prelude(name)
                if matches!(
                    name.as_str(),
                    "Copy"
                        | "Discard"
                        | "Equatable"
                        | "Key"
                        | "Send"
                        | "Share"
                        | "Call"
                        | "CallMut"
                        | "CallOnce"
                )
        )
    }
}

#[derive(Debug, Clone)]
pub struct HirTraitReference {
    constructor: HirTraitConstructor,
    arguments: Vec<TypeId>,
}

impl HirTraitReference {
    pub fn constructor(&self) -> &HirTraitConstructor {
        &self.constructor
    }

    pub fn arguments(&self) -> &[TypeId] {
        &self.arguments
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HirImplementationId(u32);

impl HirImplementationId {
    pub fn index(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HirImplementationMethodId {
    implementation: HirImplementationId,
    index: u32,
}

impl HirImplementationMethodId {
    pub fn implementation(self) -> HirImplementationId {
        self.implementation
    }

    pub fn index(self) -> u32 {
        self.index
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum HirPreludeTraitMethod {
    Display,
    IteratorNext,
}

impl HirPreludeTraitMethod {
    pub(crate) fn trait_name(self) -> &'static str {
        match self {
            Self::Display => "Display",
            Self::IteratorNext => "Iterator",
        }
    }

    pub(crate) fn method_name(self) -> &'static str {
        match self {
            Self::Display => "display",
            Self::IteratorNext => "next",
        }
    }

    pub(crate) fn generic_arity(self) -> u32 {
        match self {
            Self::Display => 1,
            Self::IteratorNext => 2,
        }
    }

    pub(crate) fn query(self, arguments: &[TypeId]) -> Option<TraitQuery> {
        let (trait_arguments, target) = match (self, arguments) {
            (Self::Display, [target]) => (Vec::new(), *target),
            (Self::IteratorNext, [element, target]) => (vec![*element], *target),
            (Self::Display, _) | (Self::IteratorNext, _) => return None,
        };
        Some(TraitQuery::from_parts(
            HirTraitConstructor::Prelude(
                Name::new(self.trait_name()).expect("prelude trait names are valid"),
            ),
            trait_arguments,
            target,
        ))
    }

    pub(crate) fn function_type(
        self,
        interner: &mut TypeInterner,
        arguments: &[TypeId],
    ) -> Result<Option<TypeId>, TypeError> {
        let (mode, receiver, outcome) = match (self, arguments) {
            (Self::Display, [target]) => (
                ParameterMode::Ref,
                *target,
                interner.scalar(ScalarType::String),
            ),
            (Self::IteratorNext, [element, target]) => {
                (ParameterMode::Mut, *target, interner.option(*element)?)
            }
            (Self::Display, _) | (Self::IteratorNext, _) => return Ok(None),
        };
        interner
            .function(FunctionType::new(
                false,
                false,
                vec![FunctionParameter::new(mode, receiver)],
                None,
                outcome,
            ))
            .map(Some)
    }

    pub(crate) fn has_intrinsic_implementation(
        self,
        interner: &TypeInterner,
        arguments: &[TypeId],
    ) -> Result<bool, TypeError> {
        Ok(match (self, arguments) {
            (Self::Display, [target]) => intrinsic_display_type(interner, *target)?,
            (Self::Display, _) | (Self::IteratorNext, _) => false,
        })
    }
}

fn intrinsic_display_type(interner: &TypeInterner, root: TypeId) -> Result<bool, TypeError> {
    let mut pending = vec![root];
    let mut visited = BTreeSet::new();
    while let Some(ty) = pending.pop() {
        if !visited.insert(ty) {
            continue;
        }
        match interner.kind(ty)? {
            TypeKind::Scalar(
                ScalarType::Bool
                | ScalarType::Int
                | ScalarType::Float
                | ScalarType::Byte
                | ScalarType::Char
                | ScalarType::String
                | ScalarType::Unit
                | ScalarType::Int8
                | ScalarType::Int16
                | ScalarType::Int32
                | ScalarType::UInt8
                | ScalarType::UInt16
                | ScalarType::UInt32
                | ScalarType::UInt64
                | ScalarType::Float32,
            ) => {}
            TypeKind::Intrinsic {
                constructor: IntrinsicType::Array,
                arguments,
            } if arguments.len() == 1 => pending.push(arguments[0]),
            _ => return Ok(false),
        }
    }
    Ok(true)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum HirTraitMethodKey {
    Source(MemberId),
    Prelude(HirPreludeTraitMethod),
}

#[derive(Debug, Clone)]
pub struct HirImplementationMethodContract {
    method: HirTraitMethodKey,
    has_default: bool,
    requires_self_send: bool,
    function_type: TypeId,
    has_receiver: bool,
    generic_bounds: Vec<Vec<HirTraitReference>>,
}

impl HirImplementationMethodContract {
    pub fn method(&self) -> HirTraitMethodKey {
        self.method
    }

    pub fn has_default(&self) -> bool {
        self.has_default
    }

    pub fn requires_self_send(&self) -> bool {
        self.requires_self_send
    }

    pub fn function_type(&self) -> TypeId {
        self.function_type
    }

    pub fn has_receiver(&self) -> bool {
        self.has_receiver
    }

    pub fn generic_bounds(&self) -> &[Vec<HirTraitReference>] {
        &self.generic_bounds
    }
}

#[derive(Debug, Clone)]
pub struct HirImplementationMethod {
    id: HirImplementationMethodId,
    span: Span,
    name: Name,
    contract: Option<HirImplementationMethodContract>,
}

impl HirImplementationMethod {
    pub fn id(&self) -> HirImplementationMethodId {
        self.id
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn name(&self) -> &Name {
        &self.name
    }

    pub fn contract(&self) -> Option<&HirImplementationMethodContract> {
        self.contract.as_ref()
    }
}

#[derive(Debug, Clone)]
pub struct HirImplementation {
    id: HirImplementationId,
    span: Span,
    module: ModuleId,
    parameters: Vec<HirGenericParameter>,
    trait_reference: HirTraitReference,
    target: TypeId,
    methods: Vec<HirImplementationMethod>,
    contract_complete: bool,
    requires_self_send: bool,
}

impl HirImplementation {
    pub fn id(&self) -> HirImplementationId {
        self.id
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn module(&self) -> &ModuleId {
        &self.module
    }

    pub fn parameters(&self) -> &[HirGenericParameter] {
        &self.parameters
    }

    pub fn trait_reference(&self) -> &HirTraitReference {
        &self.trait_reference
    }

    pub fn target(&self) -> TypeId {
        self.target
    }

    pub fn methods(&self) -> &[HirImplementationMethod] {
        &self.methods
    }

    pub fn contract_complete(&self) -> bool {
        self.contract_complete
    }

    pub fn requires_self_send(&self) -> bool {
        self.requires_self_send
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum HirCallableId {
    Symbol(SymbolId),
    Member(MemberId),
    Implementation(HirImplementationMethodId),
    Host(HirBootstrapHostFunction),
}

#[derive(Debug, Clone)]
pub struct HirCallableSignature {
    id: HirCallableId,
    span: Span,
    parameters: Vec<HirParameter>,
    generics: Vec<HirGenericParameter>,
    generic_arity: u32,
    outcome: TypeId,
    function_type: TypeId,
    opaque_result: Option<HirOpaqueResult>,
    body_source: Option<Span>,
    implicit_script: bool,
}

impl HirCallableSignature {
    pub fn id(&self) -> HirCallableId {
        self.id
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn parameters(&self) -> &[HirParameter] {
        &self.parameters
    }

    pub fn generics(&self) -> &[HirGenericParameter] {
        &self.generics
    }

    pub fn generic_arity(&self) -> u32 {
        self.generic_arity
    }

    pub fn outcome(&self) -> TypeId {
        self.outcome
    }

    pub fn function_type(&self) -> TypeId {
        self.function_type
    }

    pub fn opaque_result(&self) -> Option<&HirOpaqueResult> {
        self.opaque_result.as_ref()
    }

    pub fn body_source(&self) -> Option<Span> {
        self.body_source
    }

    pub fn is_implicit_script(&self) -> bool {
        self.implicit_script
    }
}

/// The source-visible contract and compiler-private representation of one
/// declaration-owned opaque result family.
#[derive(Debug, Clone)]
pub struct HirOpaqueResult {
    identity: SymbolIdentity,
    span: Span,
    bounds: Vec<HirTraitReference>,
    witness: Option<TypeId>,
}

impl HirOpaqueResult {
    pub fn identity(&self) -> &SymbolIdentity {
        &self.identity
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn bounds(&self) -> &[HirTraitReference] {
        &self.bounds
    }

    pub(crate) fn witness(&self) -> Option<TypeId> {
        self.witness
    }
}

#[derive(Debug, Clone)]
pub struct HirParameter {
    span: Span,
    local: Option<LocalId>,
    mode: ParameterMode,
    ty: TypeId,
    variadic_element: Option<TypeId>,
    receiver: bool,
    discard: bool,
}

impl HirParameter {
    pub fn span(&self) -> Span {
        self.span
    }

    pub fn local(&self) -> Option<LocalId> {
        self.local
    }

    pub fn mode(&self) -> ParameterMode {
        self.mode
    }

    /// Type visible to the callable body. For a variadic parameter this is
    /// `Array[element]`.
    pub fn ty(&self) -> TypeId {
        self.ty
    }

    pub fn variadic_element(&self) -> Option<TypeId> {
        self.variadic_element
    }

    pub fn is_receiver(&self) -> bool {
        self.receiver
    }

    pub fn is_discard(&self) -> bool {
        self.discard
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HirClosureId(u32);

impl HirClosureId {
    pub fn index(self) -> u32 {
        self.0
    }
}

/// The static environment and callable signature of one source closure.
///
/// The body remains a separate semantic root: constructing a closure evaluates
/// its captures, but never executes the body. Its generated identity and exact
/// function signature retain synchronous/asynchronous and safe/unsafe effects.
#[derive(Debug, Clone)]
pub struct HirClosure {
    id: HirClosureId,
    identity: GeneratedTypeIdentity,
    span: Span,
    ty: TypeId,
    generic_arity: u32,
    function_type: TypeId,
    protocols: HirClosureProtocols,
    generics: Vec<HirGenericParameter>,
    parameters: Vec<HirParameter>,
    captures: Vec<HirClosureCapture>,
    body: HirBody,
}

impl HirClosure {
    pub fn id(&self) -> HirClosureId {
        self.id
    }

    pub fn identity(&self) -> &GeneratedTypeIdentity {
        &self.identity
    }

    pub fn kind(&self) -> GeneratedTypeKind {
        self.identity.kind()
    }

    pub fn is_async(&self) -> bool {
        self.kind().is_async()
    }

    pub fn is_unsafe(&self) -> bool {
        self.kind().is_unsafe()
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn ty(&self) -> TypeId {
        self.ty
    }

    pub fn generic_arity(&self) -> u32 {
        self.generic_arity
    }

    pub fn function_type(&self) -> TypeId {
        self.function_type
    }

    pub fn protocols(&self) -> HirClosureProtocols {
        self.protocols
    }

    pub fn generics(&self) -> &[HirGenericParameter] {
        &self.generics
    }

    pub fn parameters(&self) -> &[HirParameter] {
        &self.parameters
    }

    pub fn captures(&self) -> &[HirClosureCapture] {
        &self.captures
    }

    pub fn body(&self) -> &HirBody {
        &self.body
    }
}

/// The three closed invocation contracts derived for a concrete closure.
///
/// The implications from the language specification are materialized here so
/// every later compiler layer can validate and consume one canonical answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HirClosureProtocols {
    call: bool,
    call_mut: bool,
    call_once: bool,
}

impl HirClosureProtocols {
    pub const fn new(call: bool, call_mut: bool, call_once: bool) -> Self {
        Self {
            call,
            call_mut,
            call_once,
        }
    }

    pub const fn supports(self, protocol: HirCallProtocol) -> bool {
        match protocol {
            HirCallProtocol::Call => self.call,
            HirCallProtocol::CallMut => self.call_mut,
            HirCallProtocol::CallOnce => self.call_once,
        }
    }

    pub const fn call(self) -> bool {
        self.call
    }

    pub const fn call_mut(self) -> bool {
        self.call_mut
    }

    pub const fn call_once(self) -> bool {
        self.call_once
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HirCallProtocol {
    Call,
    CallMut,
    CallOnce,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HirClosureCapture {
    local: LocalId,
    ty: TypeId,
    mutable: bool,
}

impl HirClosureCapture {
    pub fn local(&self) -> LocalId {
        self.local
    }

    pub fn ty(&self) -> TypeId {
        self.ty
    }

    pub fn is_mutable(&self) -> bool {
        self.mutable
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HirExpressionId(u32);

impl HirExpressionId {
    pub fn index(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HirValueCategory {
    Value,
    Place,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HirFlow {
    MayComplete,
    Diverges,
}

impl HirFlow {
    pub fn may_complete(self) -> bool {
        self == Self::MayComplete
    }
}

#[derive(Debug, Clone)]
pub struct HirExpression {
    span: Span,
    ty: TypeId,
    category: HirValueCategory,
    kind: HirExpressionKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HirMemberReference {
    member: MemberId,
    span: Span,
}

impl HirMemberReference {
    pub fn member(&self) -> MemberId {
        self.member
    }

    pub fn span(&self) -> Span {
        self.span
    }
}

impl HirExpression {
    pub fn span(&self) -> Span {
        self.span
    }

    pub fn ty(&self) -> TypeId {
        self.ty
    }

    pub fn category(&self) -> HirValueCategory {
        self.category
    }

    pub fn kind(&self) -> &HirExpressionKind {
        &self.kind
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HirLiteral {
    Unit,
    Bool(bool),
    Integer(String),
    Float(String),
    Char(String),
    String(String),
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HirPrefixOperator {
    Negate,
    LogicalNot,
    BitwiseNot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HirBinaryOperator {
    Multiply,
    Divide,
    Remainder,
    Add,
    Subtract,
    ShiftLeft,
    ShiftRight,
    BitwiseAnd,
    BitwiseXor,
    BitwiseOr,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Equal,
    NotEqual,
    LogicalAnd,
    LogicalOr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HirRangeKind {
    Exclusive,
    Inclusive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HirContainmentKind {
    Array,
    MapKey,
    Set,
    Range,
    StringChar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HirArraySequenceKind {
    Concat,
    Repeat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HirMatchMode {
    Copy,
    Observe,
    Consume,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HirAssignmentOperator {
    Assign,
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    BitwiseAnd,
    BitwiseXor,
    BitwiseOr,
    ShiftLeft,
    ShiftRight,
}

impl HirAssignmentOperator {
    pub fn binary_operator(self) -> Option<HirBinaryOperator> {
        Some(match self {
            Self::Assign => return None,
            Self::Add => HirBinaryOperator::Add,
            Self::Subtract => HirBinaryOperator::Subtract,
            Self::Multiply => HirBinaryOperator::Multiply,
            Self::Divide => HirBinaryOperator::Divide,
            Self::Remainder => HirBinaryOperator::Remainder,
            Self::BitwiseAnd => HirBinaryOperator::BitwiseAnd,
            Self::BitwiseXor => HirBinaryOperator::BitwiseXor,
            Self::BitwiseOr => HirBinaryOperator::BitwiseOr,
            Self::ShiftLeft => HirBinaryOperator::ShiftLeft,
            Self::ShiftRight => HirBinaryOperator::ShiftRight,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum HirIndexAccess {
    Array,
    String,
    MapLookup,
    MapEntry,
}

#[derive(Debug, Clone)]
pub enum HirExpressionKind {
    Recovery,
    Literal(HirLiteral),
    InterpolatedString {
        segments: Vec<String>,
        values: Vec<HirExpressionId>,
    },
    Local(LocalId),
    Constant(SymbolId),
    Function(HirCallableId),
    /// Compiler-owned callable used by isolated, bodyless interface catalogs.
    /// It may survive semantic `check`, but executable lowering must reject it.
    SyntheticFunction,
    SpecializedFunction {
        callable: HirCallableId,
        arguments: Vec<TypeId>,
    },
    PreludeTraitFunction {
        method: HirPreludeTraitMethod,
        arguments: Vec<TypeId>,
    },
    Closure(HirClosureId),
    Receiver,
    Tuple(Vec<HirExpressionId>),
    Array(Vec<HirExpressionId>),
    Map {
        entries: Vec<HirMapEntry>,
        reject_dynamic_duplicates: bool,
    },
    Set(Vec<HirExpressionId>),
    Newtype {
        constructor: SymbolId,
        value: HirExpressionId,
    },
    Ref {
        value: HirExpressionId,
    },
    Record {
        owner: SymbolId,
        fields: Vec<HirRecordFieldValue>,
    },
    Variant {
        variant: MemberId,
        payload: HirVariantValue,
    },
    NumericConversionError(NumericConversionErrorVariant),
    RecordUpdate {
        base: HirExpressionId,
        fields: Vec<HirRecordFieldValue>,
    },
    NumericConversion {
        target: ScalarType,
        conversion: NumericConversion,
        value: HirExpressionId,
    },
    Block {
        scope: HirScopeId,
        statements: Vec<HirStatement>,
        tail: Option<HirExpressionId>,
    },
    Prefix {
        operator: HirPrefixOperator,
        operand: HirExpressionId,
    },
    Binary {
        operator: HirBinaryOperator,
        left: HirExpressionId,
        right: HirExpressionId,
    },
    ArraySequence {
        kind: HirArraySequenceKind,
        array: HirExpressionId,
        argument: HirExpressionId,
    },
    MapRemove {
        map: HirExpressionId,
        key: HirExpressionId,
    },
    Range {
        kind: HirRangeKind,
        start: HirExpressionId,
        end: HirExpressionId,
    },
    Contains {
        kind: HirContainmentKind,
        item: HirExpressionId,
        container: HirExpressionId,
    },
    Field {
        base: HirExpressionId,
        member: MemberId,
    },
    TupleField {
        base: HirExpressionId,
        index: u32,
    },
    RefValue {
        base: HirExpressionId,
    },
    Index {
        base: HirExpressionId,
        index: HirExpressionId,
        access: HirIndexAccess,
    },
    Slice {
        base: HirExpressionId,
        start: Option<HirExpressionId>,
        end: Option<HirExpressionId>,
        step: Option<HirExpressionId>,
    },
    Call {
        callee: HirExpressionId,
        arguments: Vec<HirCallArgument>,
        signature: TypeId,
        protocol: HirCallProtocol,
        unsafe_call: bool,
    },
    AsyncCall {
        callee: HirExpressionId,
        arguments: Vec<HirCallArgument>,
        signature: TypeId,
        protocol: HirCallProtocol,
        unsafe_call: bool,
    },
    Await {
        operation: HirExpressionId,
    },
    Spawn {
        operation: HirExpressionId,
    },
    Scope {
        body: HirExpressionId,
    },
    PreludePanic {
        message: HirExpressionId,
    },
    PreludeAssert {
        condition: HirExpressionId,
        condition_repr: String,
        message_parts: Vec<HirAssertMessagePart>,
    },
    BootstrapHostCall {
        function: HirBootstrapHostFunction,
        arguments: Vec<HirExpressionId>,
    },
    OptionSome {
        value: HirExpressionId,
    },
    ResultOk {
        value: HirExpressionId,
    },
    ResultErr {
        error: HirExpressionId,
    },
    PropagateOption {
        value: HirExpressionId,
    },
    PropagateResult {
        value: HirExpressionId,
        error_coercion: Assignability,
    },
    If {
        condition: HirExpressionId,
        then_branch: HirExpressionId,
        else_branch: Option<HirExpressionId>,
    },
    Match {
        scrutinee: HirExpressionId,
        mode: HirMatchMode,
        arms: Vec<HirMatchArm>,
    },
    Return {
        value: Option<HirExpressionId>,
    },
    Fail {
        error: HirExpressionId,
    },
    Break {
        target: Option<HirLoopId>,
    },
    Continue {
        target: Option<HirLoopId>,
    },
    Coerce {
        kind: Assignability,
        value: HirExpressionId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HirBootstrapHostFunction {
    ConsolePrint,
    ConsolePrintln,
    ConsoleFlush,
    ConsoleStdin,
    ConsoleStdout,
    ConsoleStderr,
    ConsoleReadLine,
    ReaderRead,
    WriterWrite,
    WriterFlush,
    IoLimitsDefault,
    IoLimitsNew,
    IoReadAll,
    IoWriteAll,
    ProcessArgs,
    ProcessCmd,
    ProcessShell,
    CommandStart,
    CommandStatus,
    CommandOutput,
    CommandRun,
    CommandCheck,
    PipelineStart,
    PipelineStatus,
    PipelineOutput,
    PipelineRun,
    PipelineCheck,
    ProcessHandleStatus,
    ProcessHandleOutput,
    ProcessHandleRun,
    ProcessHandleCheck,
    ProcessHandleCancel,
    BytesFromString,
    BytesToString,
    BytesEmpty,
    BytesFromArray,
    BytesBuilder,
    BytesLen,
    BytesGet,
    BytesSlice,
    BytesToArray,
    BytesEqual,
    BytesHash,
    BytesBuilderLen,
    BytesBuilderAppendByte,
    BytesBuilderAppend,
    BytesBuilderAppendArray,
    BytesBuilderFinish,
    FormatBuilder,
    FormatBuilderAppend,
    FormatBuilderFinish,
    FormatFormat,
    FormatJoin,
    ProcessOutputStdout,
    ProcessOutputStderr,
    ProcessOutputStatuses,
    ExitStatusCode,
    ExitStatusSuccess,
    ProcessPipe,
    PointerRead,
    PointerWrite,
    PointerOffset,
    PointerCast,
    PointerAddress,
    PointerFromAddress,
    TimeNow,
    TimeResolution,
    TimeDeadline,
    TimeSleep,
    DurationFromNanoseconds,
    DurationFromMicroseconds,
    DurationFromMilliseconds,
    DurationFromSeconds,
    DurationToNanoseconds,
    DurationAdd,
    DurationSubtract,
    DurationMultiply,
    DurationNegate,
    DurationIsZero,
    DurationIsNegative,
    DurationIsLessThan,
    InstantAdd,
    InstantSubtract,
    InstantDurationSince,
    InstantIsBefore,
    InstantIsAfter,
    TimerAfter,
    TimerAt,
    TimerWait,
    TimerCancel,
    EnvSnapshot,
    EnvNameFromText,
    EnvNameFromBytes,
    EnvSnapshotArguments,
    EnvSnapshotGet,
    EnvValueAsText,
    EnvValueAsBytes,
    MathFloor,
    MathCeil,
    MathRound,
    MathTruncate,
    MathSqrt,
    MathFma,
    MathAbs,
    MathMin,
    MathMax,
    PathFromString,
    PathFromBytes,
    PathJoin,
    PathParent,
    PathFileName,
    PathExtension,
    PathKind,
    PathIsEmpty,
    PathToString,
    PathToBytes,
    FsReadAll,
    FsWriteAll,
    FsCreateDirectory,
    FsRemove,
    FsList,
    FsRename,
    FsAtomicWrite,
    JsonValidate,
    JsonCanonicalize,
    MessagePackValidate,
    MessagePackCanonicalize,
    ProtobufValidate,
    TextEmpty,
    TextFromChars,
    TextLength,
    TextByteLength,
    TextGet,
    TextSlice,
    TextChars,
    TextContains,
    TextStartsWith,
    TextEndsWith,
    TextFind,
    TextReplace,
    TextTrim,
    TextLowerAscii,
    TextUpperAscii,
    CollectionArrayNew,
    CollectionArrayWithCapacity,
    CollectionArrayLength,
    CollectionArrayGet,
    CollectionArraySlice,
    CollectionArrayPush,
    CollectionArrayPop,
    CollectionMapNew,
    CollectionMapGet,
    CollectionMapInsert,
    CollectionMapRemove,
    CollectionMapContains,
    CollectionMapEntries,
    CollectionSetNew,
    CollectionSetInsert,
    CollectionSetRemove,
    CollectionSetContains,
    CollectionSetValues,
    IterMap,
    IterFilter,
    IterTake,
    IterCollect,
    /// Compiler-owned Core operation. It is represented as a host callable
    /// during generic call checking and lowered to value-level control flow;
    /// it never reaches the VM host.
    CoreOptionMap,
    CoreOptionUnwrapOr,
    CoreResultMap,
    CoreResultMapErr,
    CoreResultUnwrapOr,
    TestingLog,
    TestingAssertEqual,
    TestingAssertNotEqual,
    TestingAssertTextEqual,
    TestingAssertFloatNear,
    TestingFloatToleranceFrom,
    TestingAssertFloat32Near,
    TestingDiffText,
    TestingTextDiffRender,
    TestingTempDirectory,
    TestingTempDirectoryPath,
    TestingTempDirectoryCleanup,
    TestingGeneratorNew,
    TestingGeneratorForCase,
    TestingGeneratorId,
    TestingGeneratorNextUInt,
    TestingGeneratorNextBool,
    TestingGeneratorNextInt,
    TestingGeneratorNextBytes,
    TestingGeneratorNextText,
    TestingGeneratorDrawCount,
    TestingAssertSome,
    TestingAssertNone,
    TestingAssertOk,
    TestingAssertErr,
    TestingTags,
    TestingFailNow,
    TestingSkip,
    TestingAttach,
    TestingSnapshot,
    TestingWithVirtualTime,
    VirtualTimeSettle,
    VirtualTimeAdvance,
    TestingRunLeaf,
    TestingRunSuite,
    TestingBeginSuiteCleanup,
}

impl HirBootstrapHostFunction {
    pub const fn name(self) -> &'static str {
        match self {
            Self::ConsolePrint => "std.console.print",
            Self::ConsolePrintln => "std.console.println",
            Self::ConsoleFlush => "std.console.flush",
            Self::ConsoleStdin => "std.console.stdin",
            Self::ConsoleStdout => "std.console.stdout",
            Self::ConsoleStderr => "std.console.stderr",
            Self::ConsoleReadLine => "std.console.readLine",
            Self::ReaderRead => "std.io.Reader.read",
            Self::WriterWrite => "std.io.Writer.write",
            Self::WriterFlush => "std.io.Writer.flush",
            Self::IoLimitsDefault => "std.io.defaultLimits",
            Self::IoLimitsNew => "std.io.limits",
            Self::IoReadAll => "std.io.readAll",
            Self::IoWriteAll => "std.io.writeAll",
            Self::ProcessArgs => "std.process.args",
            Self::ProcessCmd => "std.process.cmd",
            Self::ProcessShell => "std.process.shell",
            Self::CommandStart => "std.process.Command.start",
            Self::CommandStatus => "std.process.Command.status",
            Self::CommandOutput => "std.process.Command.output",
            Self::CommandRun => "std.process.Command.run",
            Self::CommandCheck => "std.process.Command.check",
            Self::PipelineStart => "std.process.Pipeline.start",
            Self::PipelineStatus => "std.process.Pipeline.status",
            Self::PipelineOutput => "std.process.Pipeline.output",
            Self::PipelineRun => "std.process.Pipeline.run",
            Self::PipelineCheck => "std.process.Pipeline.check",
            Self::ProcessHandleStatus => "std.process.ProcessHandle.status",
            Self::ProcessHandleOutput => "std.process.ProcessHandle.output",
            Self::ProcessHandleRun => "std.process.ProcessHandle.run",
            Self::ProcessHandleCheck => "std.process.ProcessHandle.check",
            Self::ProcessHandleCancel => "std.process.ProcessHandle.cancel",
            Self::BytesFromString => "intrinsic.Bytes.fromString",
            Self::BytesToString => "intrinsic.String.fromBytes",
            Self::BytesEmpty => "std.bytes.empty",
            Self::BytesFromArray => "std.bytes.fromArray",
            Self::BytesBuilder => "std.bytes.builder",
            Self::BytesLen => "std.bytes.Bytes.length",
            Self::BytesGet => "std.bytes.Bytes.get",
            Self::BytesSlice => "std.bytes.Bytes.slice",
            Self::BytesToArray => "std.bytes.Bytes.toArray",
            Self::BytesEqual => "std.bytes.Bytes.equal",
            Self::BytesHash => "std.bytes.Bytes.hash",
            Self::BytesBuilderLen => "std.bytes.BytesBuilder.length",
            Self::BytesBuilderAppendByte => "std.bytes.BytesBuilder.appendByte",
            Self::BytesBuilderAppend => "std.bytes.BytesBuilder.append",
            Self::BytesBuilderAppendArray => "std.bytes.BytesBuilder.appendArray",
            Self::BytesBuilderFinish => "std.bytes.BytesBuilder.finish",
            Self::FormatBuilder => "std.format.Builder.new",
            Self::FormatBuilderAppend => "std.format.Builder.append",
            Self::FormatBuilderFinish => "std.format.Builder.finish",
            Self::FormatFormat => "std.format.format",
            Self::FormatJoin => "std.format.join",
            Self::ProcessOutputStdout => "std.process.ProcessOutput.stdout",
            Self::ProcessOutputStderr => "std.process.ProcessOutput.stderr",
            Self::ProcessOutputStatuses => "std.process.ProcessOutput.statuses",
            Self::ExitStatusCode => "std.process.ExitStatus.code",
            Self::ExitStatusSuccess => "std.process.ExitStatus.success",
            Self::ProcessPipe => "std.process.pipe",
            Self::PointerRead => "intrinsic.Pointer.read",
            Self::PointerWrite => "intrinsic.Pointer.write",
            Self::PointerOffset => "intrinsic.Pointer.offset",
            Self::PointerCast => "intrinsic.Pointer.cast",
            Self::PointerAddress => "intrinsic.Pointer.address",
            Self::PointerFromAddress => "intrinsic.UInt64.toPointer",
            Self::TimeNow => "std.time.now",
            Self::TimeResolution => "std.time.resolution",
            Self::TimeDeadline => "std.time.deadline",
            Self::TimeSleep => "std.time.sleep",
            Self::DurationFromNanoseconds => "std.time.Duration.fromNanoseconds",
            Self::DurationFromMicroseconds => "std.time.Duration.fromMicroseconds",
            Self::DurationFromMilliseconds => "std.time.Duration.fromMilliseconds",
            Self::DurationFromSeconds => "std.time.Duration.fromSeconds",
            Self::DurationToNanoseconds => "std.time.Duration.toNanoseconds",
            Self::DurationAdd => "std.time.Duration.add",
            Self::DurationSubtract => "std.time.Duration.subtract",
            Self::DurationMultiply => "std.time.Duration.multiply",
            Self::DurationNegate => "std.time.Duration.negate",
            Self::DurationIsZero => "std.time.Duration.isZero",
            Self::DurationIsNegative => "std.time.Duration.isNegative",
            Self::DurationIsLessThan => "std.time.Duration.isLessThan",
            Self::InstantAdd => "std.time.Instant.add",
            Self::InstantSubtract => "std.time.Instant.subtract",
            Self::InstantDurationSince => "std.time.Instant.durationSince",
            Self::InstantIsBefore => "std.time.Instant.isBefore",
            Self::InstantIsAfter => "std.time.Instant.isAfter",
            Self::TimerAfter => "std.time.Timer.after",
            Self::TimerAt => "std.time.Timer.at",
            Self::TimerWait => "std.time.Timer.wait",
            Self::TimerCancel => "std.time.Timer.cancel",
            Self::EnvSnapshot => "std.env.snapshot",
            Self::EnvNameFromText => "std.env.Name.fromText",
            Self::EnvNameFromBytes => "std.env.Name.fromBytes",
            Self::EnvSnapshotArguments => "std.env.Snapshot.arguments",
            Self::EnvSnapshotGet => "std.env.Snapshot.get",
            Self::EnvValueAsText => "std.env.Value.asText",
            Self::EnvValueAsBytes => "std.env.Value.asBytes",
            Self::MathFloor => "std.math.floor",
            Self::MathCeil => "std.math.ceil",
            Self::MathRound => "std.math.round",
            Self::MathTruncate => "std.math.truncate",
            Self::MathSqrt => "std.math.sqrt",
            Self::MathFma => "std.math.fma",
            Self::MathAbs => "std.math.abs",
            Self::MathMin => "std.math.min",
            Self::MathMax => "std.math.max",
            Self::PathFromString => "std.path.Path.fromString",
            Self::PathFromBytes => "std.path.Path.fromBytes",
            Self::PathJoin => "std.path.Path.join",
            Self::PathParent => "std.path.Path.parent",
            Self::PathFileName => "std.path.Path.fileName",
            Self::PathExtension => "std.path.Path.extension",
            Self::PathKind => "std.path.Path.kind",
            Self::PathIsEmpty => "std.path.Path.isEmpty",
            Self::PathToString => "std.path.Path.toString",
            Self::PathToBytes => "std.path.Path.toBytes",
            Self::FsReadAll => "std.fs.readAll",
            Self::FsWriteAll => "std.fs.writeAll",
            Self::FsCreateDirectory => "std.fs.createDirectory",
            Self::FsRemove => "std.fs.remove",
            Self::FsList => "std.fs.list",
            Self::FsRename => "std.fs.rename",
            Self::FsAtomicWrite => "std.fs.atomicWrite",
            Self::JsonValidate => "std.json.validate",
            Self::JsonCanonicalize => "std.json.canonicalize",
            Self::MessagePackValidate => "std.messagepack.validate",
            Self::MessagePackCanonicalize => "std.messagepack.canonicalize",
            Self::ProtobufValidate => "std.protobuf.validate",
            Self::TextEmpty => "std.text.String.empty",
            Self::TextFromChars => "std.text.String.fromChars",
            Self::TextLength => "std.text.String.length",
            Self::TextByteLength => "std.text.String.byteLength",
            Self::TextGet => "std.text.String.get",
            Self::TextSlice => "std.text.String.slice",
            Self::TextChars => "std.text.String.chars",
            Self::TextContains => "std.text.String.contains",
            Self::TextStartsWith => "std.text.String.startsWith",
            Self::TextEndsWith => "std.text.String.endsWith",
            Self::TextFind => "std.text.String.find",
            Self::TextReplace => "std.text.String.replace",
            Self::TextTrim => "std.text.String.trim",
            Self::TextLowerAscii => "std.text.String.toLowerAscii",
            Self::TextUpperAscii => "std.text.String.toUpperAscii",
            Self::CollectionArrayNew => "std.collections.Array.new",
            Self::CollectionArrayWithCapacity => "std.collections.Array.withCapacity",
            Self::CollectionArrayLength => "std.collections.Array.length",
            Self::CollectionArrayGet => "std.collections.Array.get",
            Self::CollectionArraySlice => "std.collections.Array.slice",
            Self::CollectionArrayPush => "std.collections.Array.push",
            Self::CollectionArrayPop => "std.collections.Array.pop",
            Self::CollectionMapNew => "std.collections.Map.new",
            Self::CollectionMapGet => "std.collections.Map.get",
            Self::CollectionMapInsert => "std.collections.Map.insert",
            Self::CollectionMapRemove => "std.collections.Map.remove",
            Self::CollectionMapContains => "std.collections.Map.contains",
            Self::CollectionMapEntries => "std.collections.Map.entries",
            Self::CollectionSetNew => "std.collections.Set.new",
            Self::CollectionSetInsert => "std.collections.Set.insert",
            Self::CollectionSetRemove => "std.collections.Set.remove",
            Self::CollectionSetContains => "std.collections.Set.contains",
            Self::CollectionSetValues => "std.collections.Set.values",
            Self::IterMap => "std.iter.map",
            Self::IterFilter => "std.iter.filter",
            Self::IterTake => "std.iter.take",
            Self::IterCollect => "std.iter.collect",
            Self::CoreOptionMap => "std.core.Option.map",
            Self::CoreOptionUnwrapOr => "std.core.Option.unwrapOr",
            Self::CoreResultMap => "std.core.Result.map",
            Self::CoreResultMapErr => "std.core.Result.mapErr",
            Self::CoreResultUnwrapOr => "std.core.Result.unwrapOr",
            Self::TestingLog => "std.testing.log",
            Self::TestingAssertEqual => "std.testing.assertEqual",
            Self::TestingAssertNotEqual => "std.testing.assertNotEqual",
            Self::TestingAssertTextEqual => "std.testing.assertTextEqual",
            Self::TestingAssertFloatNear => "std.testing.assertFloatNear",
            Self::TestingFloatToleranceFrom => "std.testing.FloatTolerance.from",
            Self::TestingAssertFloat32Near => "std.testing.assertFloat32Near",
            Self::TestingDiffText => "std.testing.diffText",
            Self::TestingTextDiffRender => "std.testing.TextDiff.render",
            Self::TestingTempDirectory => "std.testing.tempDirectory",
            Self::TestingTempDirectoryPath => "std.testing.TempDirectory.path",
            Self::TestingTempDirectoryCleanup => "std.testing.TempDirectory.cleanup",
            Self::TestingGeneratorNew => "std.testing.Generator.new",
            Self::TestingGeneratorForCase => "std.testing.Generator.forCase",
            Self::TestingGeneratorId => "std.testing.Generator.id",
            Self::TestingGeneratorNextUInt => "std.testing.Generator.nextUInt",
            Self::TestingGeneratorNextBool => "std.testing.Generator.nextBool",
            Self::TestingGeneratorNextInt => "std.testing.Generator.nextInt",
            Self::TestingGeneratorNextBytes => "std.testing.Generator.nextBytes",
            Self::TestingGeneratorNextText => "std.testing.Generator.nextText",
            Self::TestingGeneratorDrawCount => "std.testing.Generator.drawCount",
            Self::TestingAssertSome => "std.testing.assertSome",
            Self::TestingAssertNone => "std.testing.assertNone",
            Self::TestingAssertOk => "std.testing.assertOk",
            Self::TestingAssertErr => "std.testing.assertErr",
            Self::TestingTags => "std.testing.tags",
            Self::TestingFailNow => "std.testing.failNow",
            Self::TestingSkip => "std.testing.skip",
            Self::TestingAttach => "std.testing.attach",
            Self::TestingSnapshot => "std.testing.snapshot",
            Self::TestingWithVirtualTime => "std.testing.withVirtualTime",
            Self::VirtualTimeSettle => "std.testing.VirtualTime.settle",
            Self::VirtualTimeAdvance => "std.testing.VirtualTime.advance",
            Self::TestingRunLeaf => "std.testing.__runLeaf",
            Self::TestingRunSuite => "std.testing.__runSuite",
            Self::TestingBeginSuiteCleanup => "std.testing.__beginSuiteCleanup",
        }
    }

    pub const fn is_async(self) -> bool {
        matches!(
            self,
            Self::ReaderRead
                | Self::WriterWrite
                | Self::WriterFlush
                | Self::IoReadAll
                | Self::IoWriteAll
                | Self::CommandStatus
                | Self::CommandOutput
                | Self::CommandRun
                | Self::CommandCheck
                | Self::PipelineStatus
                | Self::PipelineOutput
                | Self::PipelineRun
                | Self::PipelineCheck
                | Self::ProcessHandleStatus
                | Self::ProcessHandleOutput
                | Self::ProcessHandleRun
                | Self::ProcessHandleCheck
                | Self::ProcessHandleCancel
                | Self::TimeSleep
                | Self::TimerWait
                | Self::TestingWithVirtualTime
                | Self::VirtualTimeSettle
                | Self::VirtualTimeAdvance
        )
    }
}

#[derive(Debug, Clone, Copy)]
pub struct HirAssertMessagePart {
    value: HirExpressionId,
    spread: bool,
}

impl HirAssertMessagePart {
    pub fn value(self) -> HirExpressionId {
        self.value
    }

    pub fn is_spread(self) -> bool {
        self.spread
    }
}

#[derive(Debug, Clone)]
pub struct HirMapEntry {
    key: HirExpressionId,
    value: HirExpressionId,
}

impl HirMapEntry {
    pub fn key(&self) -> HirExpressionId {
        self.key
    }

    pub fn value(&self) -> HirExpressionId {
        self.value
    }
}

#[derive(Debug, Clone)]
pub struct HirRecordFieldValue {
    member: MemberId,
    value: HirExpressionId,
}

impl HirRecordFieldValue {
    pub fn member(&self) -> MemberId {
        self.member
    }

    pub fn value(&self) -> HirExpressionId {
        self.value
    }
}

#[derive(Debug, Clone)]
pub enum HirVariantValue {
    Unit,
    Tuple(Vec<HirExpressionId>),
    Record(Vec<HirRecordFieldValue>),
}

#[derive(Debug, Clone)]
pub struct HirMatchArm {
    pattern: HirPatternId,
    guard: Option<HirExpressionId>,
    body: HirExpressionId,
}

impl HirMatchArm {
    pub fn pattern(&self) -> HirPatternId {
        self.pattern
    }

    pub fn guard(&self) -> Option<HirExpressionId> {
        self.guard
    }

    pub fn body(&self) -> HirExpressionId {
        self.body
    }
}

#[derive(Debug, Clone)]
pub struct HirCallArgument {
    label: Option<Name>,
    mode: ParameterMode,
    spread: bool,
    target: HirCallArgumentTarget,
    value: HirExpressionId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HirCallArgumentTarget {
    Receiver,
    Fixed(u32),
    VariadicElement,
    VariadicSpread,
    Invalid,
}

impl HirCallArgument {
    pub fn label(&self) -> Option<&Name> {
        self.label.as_ref()
    }

    pub fn mode(&self) -> ParameterMode {
        self.mode
    }

    pub fn is_spread(&self) -> bool {
        self.spread
    }

    pub fn target(&self) -> HirCallArgumentTarget {
        self.target
    }

    pub fn value(&self) -> HirExpressionId {
        self.value
    }
}

/// One lexical cleanup scope retained after syntax checking.
///
/// IDs are request-local and exist so HIR, MIR, bytecode, and the VM agree on
/// exactly which dynamic `defer` entries are drained by each control edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HirScopeId(u32);

impl HirScopeId {
    pub fn index(self) -> u32 {
        self.0
    }
}

/// A fully checked deferred invocation.
///
/// `expression` is an invocation-shaped HIR node whose call itself is delayed.
/// Its direct operands are evaluated and captured when the statement runs.
/// `guarded` identifies the unique non-`Copy` operand, when present; that value
/// remains in its owner place until cleanup and may only follow verified
/// whole-value moves.
#[derive(Debug, Clone)]
pub struct HirDeferAction {
    expression: HirExpressionId,
    guarded: Option<HirExpressionId>,
}

impl HirDeferAction {
    pub fn expression(&self) -> HirExpressionId {
        self.expression
    }

    pub fn guarded(&self) -> Option<HirExpressionId> {
        self.guarded
    }
}

#[derive(Debug, Clone)]
pub enum HirStatement {
    Binding {
        span: Span,
        mutable: bool,
        pattern: HirPatternId,
        declared_type: Option<TypeId>,
        value: HirExpressionId,
    },
    Expression {
        span: Span,
        value: HirExpressionId,
    },
    Discard {
        span: Span,
        value: HirExpressionId,
    },
    Assignment {
        span: Span,
        operator: HirAssignmentOperator,
        target: HirAssignmentTarget,
        value: HirExpressionId,
    },
    Defer {
        span: Span,
        scope: HirScopeId,
        action: HirDeferAction,
    },
    For {
        span: Span,
        id: HirLoopId,
        kind: HirForKind,
        body: HirExpressionId,
    },
}

impl HirStatement {
    pub fn span(&self) -> Span {
        match self {
            Self::Binding { span, .. }
            | Self::Expression { span, .. }
            | Self::Discard { span, .. }
            | Self::Assignment { span, .. }
            | Self::Defer { span, .. }
            | Self::For { span, .. } => *span,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HirLoopId(u32);

impl HirLoopId {
    pub fn index(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HirWriteKind {
    Replace,
    PreserveExtent,
}

#[derive(Debug, Clone)]
pub struct HirAssignmentTarget {
    span: Span,
    ty: TypeId,
    kind: HirAssignmentTargetKind,
}

impl HirAssignmentTarget {
    pub fn span(&self) -> Span {
        self.span
    }

    pub fn ty(&self) -> TypeId {
        self.ty
    }

    pub fn kind(&self) -> &HirAssignmentTargetKind {
        &self.kind
    }
}

#[derive(Debug, Clone)]
pub enum HirAssignmentTargetKind {
    Place {
        place: HirExpressionId,
        coercion: Assignability,
        write: HirWriteKind,
    },
    Discard,
    Tuple(Vec<HirAssignmentTarget>),
}

#[derive(Debug, Clone)]
pub enum HirForKind {
    Infinite,
    Conditional {
        condition: HirExpressionId,
    },
    Iterate {
        pattern: HirPatternId,
        source: HirExpressionId,
        protocol: HirIterationProtocol,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HirIterationProtocol {
    Intrinsic {
        cursor: TypeId,
    },
    Trait {
        element: TypeId,
        function_type: TypeId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HirPatternId(u32);

impl HirPatternId {
    pub fn index(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone)]
pub struct HirPattern {
    span: Span,
    ty: TypeId,
    kind: HirPatternKind,
}

impl HirPattern {
    pub fn span(&self) -> Span {
        self.span
    }

    pub fn ty(&self) -> TypeId {
        self.ty
    }

    pub fn kind(&self) -> &HirPatternKind {
        &self.kind
    }
}

#[derive(Debug, Clone)]
pub enum HirPatternKind {
    Recovery,
    Wildcard,
    Binding(LocalId),
    BorrowBinding {
        local: LocalId,
        mode: ParameterMode,
    },
    Literal(HirLiteral),
    Tuple(Vec<HirPatternId>),
    OptionSome(HirPatternId),
    OptionNone,
    ResultOk(HirPatternId),
    ResultErr(HirPatternId),
    Newtype {
        constructor: SymbolId,
        value: HirPatternId,
    },
    Variant {
        variant: MemberId,
        fields: Vec<HirPatternId>,
    },
    NumericConversionError(NumericConversionErrorVariant),
    Record {
        owner: SymbolId,
        fields: Vec<HirPatternField>,
        has_rest: bool,
    },
    UnionMember {
        member: TypeId,
        pattern: HirPatternId,
    },
    Array {
        prefix: Vec<HirPatternId>,
        rest: Option<HirPatternId>,
    },
}

#[derive(Debug, Clone)]
pub struct HirPatternField {
    member: MemberId,
    pattern: HirPatternId,
}

impl HirPatternField {
    pub fn member(&self) -> MemberId {
        self.member
    }

    pub fn pattern(&self) -> HirPatternId {
        self.pattern
    }
}

#[derive(Debug, Clone)]
pub struct HirBody {
    root: HirExpressionId,
}

impl HirBody {
    pub fn root(&self) -> HirExpressionId {
        self.root
    }
}

#[cfg(test)]
mod error_tests {
    use super::*;

    #[test]
    fn hir_error_vocabulary_and_conversion_edges_are_observable() {
        let file = FileId::from_index(3).unwrap();
        for (error, expected) in [
            (
                HirError::DiagnosticLimit { file, offset: 5 },
                "HIR diagnostic limit exceeded in file 3 at byte 5".to_owned(),
            ),
            (
                HirError::NodeLimit { file, offset: 6 },
                "typed HIR node limit exceeded in file 3 at byte 6".to_owned(),
            ),
            (
                HirError::PatternAnalysisLimit { file, offset: 7 },
                "pattern analysis limit exceeded in file 3 at byte 7".to_owned(),
            ),
            (
                HirError::TraitObligationLimit { file, offset: 8 },
                "trait obligation limit exceeded in file 3 at byte 8".to_owned(),
            ),
            (
                HirError::TraitTerminationInvariant {
                    message: "cycle".into(),
                },
                "trait termination invariant failed: cycle".to_owned(),
            ),
            (
                HirError::TraitSelectionInvariant {
                    message: "ambiguous".into(),
                },
                "trait selection invariant failed: ambiguous".to_owned(),
            ),
            (
                HirError::TextInvariant {
                    message: "invalid text".into(),
                },
                "text invariant failed: invalid text".to_owned(),
            ),
            (
                HirError::Diagnostic(DiagnosticError::EmptyMessage),
                "diagnostic message cannot be empty".to_owned(),
            ),
            (
                HirError::Package(PackageGraphError::EmptyPackageId),
                "a package ID cannot be empty".to_owned(),
            ),
            (
                HirError::Source(SourceError::TooManyFiles),
                "source database contains too many files".to_owned(),
            ),
            (
                HirError::Type(TypeError::ResourceLimit { limit: 9 }),
                "interned type node limit exceeded (9)".to_owned(),
            ),
            (
                HirError::Inference(InferenceError::Type(TypeError::ResourceLimit { limit: 10 })),
                "interned type node limit exceeded (10)".to_owned(),
            ),
        ] {
            assert_eq!(error.to_string(), expected);
        }

        assert!(matches!(
            HirError::from(DiagnosticError::EmptyMessage),
            HirError::Diagnostic(DiagnosticError::EmptyMessage)
        ));
        assert!(matches!(
            HirError::from(PackageGraphError::EmptyPackageId),
            HirError::Package(PackageGraphError::EmptyPackageId)
        ));
        assert!(matches!(
            HirError::from(SourceError::TooManyFiles),
            HirError::Source(SourceError::TooManyFiles)
        ));
        assert!(matches!(
            HirError::from(TypeError::CyclicOpaqueRepresentation),
            HirError::Type(TypeError::CyclicOpaqueRepresentation)
        ));
        assert!(matches!(
            HirError::from(InferenceError::Type(TypeError::CyclicOpaqueRepresentation)),
            HirError::Inference(InferenceError::Type(TypeError::CyclicOpaqueRepresentation))
        ));
    }
}
