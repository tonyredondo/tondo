//! Semantic high-level representation produced after name and type lowering.
//!
//! The typed portion keeps source identity, resolved names, canonical types,
//! value categories, and explicit contextual coercions. Ownership dataflow,
//! control-flow cleanup, and runtime layout remain later MIR/runtime concerns.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use crate::diagnostics::{Diagnostic, DiagnosticError};
use crate::package::{ModuleId, Name, PackageGraphError, SymbolIdentity};
use crate::resolve::{LocalId, MemberId, SymbolId};
use crate::source::{FileId, SourceError, Span, TextRange};
use crate::types::{
    Assignability, NumericConversion, ParameterMode, ScalarType, TypeError, TypeId, TypeInterner,
};

mod check;
mod const_eval;
mod lower;
mod termination;
mod traits;
mod verify;

pub use check::{ExpressionCheckLimits, HirCheckOutput, check_expressions};
pub use lower::{TypeLoweringLimits, lower_types};
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
    Invariant(HirInvariantError),
    Diagnostic(DiagnosticError),
    Package(PackageGraphError),
    Source(SourceError),
    Type(TypeError),
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
            Self::Invariant(error) => error.fmt(formatter),
            Self::Diagnostic(error) => error.fmt(formatter),
            Self::Package(error) => error.fmt(formatter),
            Self::Source(error) => error.fmt(formatter),
            Self::Type(error) => error.fmt(formatter),
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
    annotations: BTreeMap<(FileId, u32, u32), TypeId>,
    expressions: Vec<HirExpression>,
    expression_flows: Vec<HirFlow>,
    expression_breaks: Vec<Vec<HirLoopId>>,
    member_references: Vec<HirMemberReference>,
    patterns: Vec<HirPattern>,
    bodies: BTreeMap<HirCallableId, HirBody>,
    local_types: BTreeMap<LocalId, TypeId>,
    discard_statuses: Vec<HirDiscardStatus>,
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

    pub fn callable(&self, id: HirCallableId) -> Option<&HirCallableSignature> {
        self.callables.iter().find(|callable| callable.id == id)
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

    pub fn local_type(&self, local: LocalId) -> Option<TypeId> {
        self.local_types.get(&local).copied()
    }

    pub fn discard_status(&self, ty: TypeId) -> Option<HirDiscardStatus> {
        self.discard_statuses.get(ty.index() as usize).copied()
    }

    pub fn expression_check_complete(&self) -> bool {
        self.expression_check_complete
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum HirDiscardStatus {
    Satisfied,
    Deferred,
    Unsatisfied,
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
    body_source: Option<Span>,
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

    pub fn body_source(&self) -> Option<Span> {
        self.body_source
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HirIndexAccess {
    Array,
    MapLookup,
    MapEntry,
}

#[derive(Debug, Clone)]
pub enum HirExpressionKind {
    Recovery,
    Literal(HirLiteral),
    InterpolatedString {
        source: String,
        values: Vec<HirExpressionId>,
    },
    Local(LocalId),
    Constant(SymbolId),
    Function(HirCallableId),
    SpecializedFunction {
        callable: HirCallableId,
        arguments: Vec<TypeId>,
    },
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
    Record {
        owner: SymbolId,
        fields: Vec<HirRecordFieldValue>,
    },
    Variant {
        variant: MemberId,
        payload: HirVariantValue,
    },
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HirBootstrapHostFunction {
    ConsolePrint,
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
    BorrowBinding(LocalId),
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
