//! Typed control-flow representation consumed by bytecode generation.
//!
//! MIR contains no syntax nodes and performs no semantic lookup. Its explicit
//! blocks, places, operands, normal edges, and unwind edges are the shared
//! execution contract for the bootstrap VM and later native backends.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use crate::hir::{
    HirBinaryOperator, HirCallArgumentTarget, HirCallableId, HirContainmentKind, HirPrefixOperator,
    HirRangeKind,
};
use crate::resolve::{LocalId, MemberId, SymbolId};
use crate::source::Span;
use crate::types::{Assignability, NumericConversion, ParameterMode, ScalarType, TypeId};

mod lower;
mod verify;

pub use lower::{MirLoweringLimits, lower_to_mir};
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
    functions: BTreeMap<HirCallableId, MirFunction>,
}

impl MirProgram {
    pub fn functions(&self) -> impl ExactSizeIterator<Item = &MirFunction> {
        self.functions.values()
    }

    pub fn function(&self, id: HirCallableId) -> Option<&MirFunction> {
        self.functions.get(&id)
    }
}

#[derive(Debug)]
pub struct MirFunction {
    id: HirCallableId,
    span: Span,
    outcome: TypeId,
    locals: Vec<MirLocal>,
    parameters: Vec<MirLocalId>,
    return_local: MirLocalId,
    entry: MirBlockId,
    unwind: MirBlockId,
    blocks: Vec<MirBasicBlock>,
}

impl MirFunction {
    pub fn id(&self) -> HirCallableId {
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

    pub fn local(&self, id: MirLocalId) -> Option<&MirLocal> {
        self.locals.get(id.0 as usize)
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
    Assign {
        destination: MirPlace,
        value: MirRvalue,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirPlace {
    local: MirLocalId,
    ty: TypeId,
    projections: Vec<MirProjection>,
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
    Field(MemberId),
    TupleField(u32),
    NewtypeValue,
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
    Function {
        callable: HirCallableId,
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
    Newtype {
        owner: SymbolId,
    },
    Record {
        owner: SymbolId,
        fields: Vec<MemberId>,
    },
    Variant {
        variant: MemberId,
        fields: Vec<Option<MemberId>>,
    },
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
    BuildMap {
        entries: Vec<(MirOperand, MirOperand)>,
        reject_dynamic_duplicates: bool,
    },
    Index {
        base: MirOperand,
        index: MirOperand,
        access: crate::hir::HirIndexAccess,
    },
    Slice {
        base: MirOperand,
        start: Option<MirOperand>,
        end: Option<MirOperand>,
        step: Option<MirOperand>,
    },
    Call {
        callee: MirOperand,
        arguments: Vec<MirCallArgument>,
    },
    ExplicitPanic {
        message: MirOperand,
    },
    Assert {
        condition: MirOperand,
        condition_repr: String,
        message_parts: Vec<MirAssertMessagePart>,
    },
    BootstrapHostCall {
        function: MirBootstrapHostFunction,
        arguments: Vec<MirOperand>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirBootstrapHostFunction {
    ConsolePrint,
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
    IteratorNext {
        state: MirPlace,
        destination: MirPlace,
        has_value: MirBlockId,
        exhausted: MirBlockId,
        unwind: MirBlockId,
    },
    ValidatePlaces {
        places: Vec<MirPlace>,
        replacements: Vec<Option<MirOperand>>,
        for_write: bool,
        target: MirBlockId,
        unwind: MirBlockId,
    },
    Return,
    ResumePanic,
    Unreachable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MirTag {
    OptionNone,
    OptionSome,
    ResultOk,
    ResultErr,
    Variant(MemberId),
    Union(TypeId),
}
