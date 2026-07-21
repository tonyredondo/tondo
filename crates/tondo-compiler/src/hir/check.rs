use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::diagnostics::{Diagnostic, DiagnosticCode, PrimaryLocation, Related, Severity};
use crate::package::{Name, Namespace, SymbolIdentity};
use crate::resolve::{
    LocalId, LocalKind, MemberId, MemberKind, MemberName, MemberOwner, ResolvedEntity,
    ResolvedName, ResolvedProgram, SymbolId, SymbolKind, Visibility,
};
use crate::source::{FileId, SourceDatabase, Span, TextRange};
use crate::syntax::ast::{Expression as AstExpression, Pattern as AstPattern};
use crate::syntax::{Parsed, SyntaxElement, SyntaxKind, SyntaxNodeRef, SyntaxTokenRef, TokenKind};
use crate::types::{
    Assignability, InferenceContext, InferenceError, IntrinsicType, NumericConversion,
    ParameterMode, ScalarType, TypeId, TypeKind, TypeSubstitution, numeric_conversion,
};

use super::const_eval::{
    ConstantEvaluationError, evaluate, has_unavailable_input, is_nan, values_equal,
};

use super::{
    HirAssertMessagePart, HirAssignmentOperator, HirAssignmentTarget, HirAssignmentTargetKind,
    HirBinaryOperator, HirBody, HirBootstrapHostFunction, HirCallArgument, HirCallArgumentTarget,
    HirCallableId, HirCallableSignature, HirContainmentKind, HirError, HirExpression,
    HirExpressionId, HirExpressionKind, HirField, HirFlow, HirForKind, HirIndexAccess, HirLiteral,
    HirLoopId, HirMapEntry, HirMatchArm, HirMemberReference, HirNominalShape, HirPattern,
    HirPatternField, HirPatternId, HirPatternKind, HirPrefixOperator, HirProgram, HirRangeKind,
    HirRecordFieldValue, HirStatement, HirTraitConstructor, HirTypeDeclarationKind,
    HirValueCategory, HirVariantPayload, HirVariantValue, HirWriteKind,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpressionCheckLimits {
    pub max_nodes: u32,
    pub max_pattern_steps: u32,
    pub max_diagnostics: usize,
}

#[derive(Debug)]
pub struct HirCheckOutput {
    program: HirProgram,
    diagnostics: Vec<Diagnostic>,
    complete: bool,
}

#[derive(Debug, Clone, Copy)]
enum AssertArgument {
    Condition,
    Message,
}

impl HirCheckOutput {
    pub fn program(&self) -> &HirProgram {
        &self.program
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn is_complete(&self) -> bool {
        self.complete
    }

    pub fn into_parts(self) -> (HirProgram, Vec<Diagnostic>, bool) {
        (self.program, self.diagnostics, self.complete)
    }
}

pub fn check_expressions<'a>(
    sources: &'a SourceDatabase,
    parsed: impl IntoIterator<Item = (FileId, &'a Parsed)>,
    resolved: &'a ResolvedProgram,
    program: HirProgram,
    limits: ExpressionCheckLimits,
) -> Result<HirCheckOutput, HirError> {
    let mut checker = ExpressionChecker {
        sources,
        parsed: parsed.into_iter().collect(),
        resolved,
        program,
        diagnostics: Vec::new(),
        max_nodes: limits.max_nodes,
        pattern_steps_remaining: u64::from(limits.max_pattern_steps),
        max_diagnostics: limits.max_diagnostics,
        complete: true,
        next_loop_id: 0,
        discard_summaries: None,
    };
    checker.check_discard_parameters()?;
    checker.check_constants()?;
    checker.check_callables()?;
    checker.check_constant_collection_diagnostics()?;
    checker.check_reachability_warnings()?;
    checker.program.expression_check_complete = checker.complete;
    if checker.complete
        && !checker
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity() == Severity::Error)
    {
        super::verify_typed_hir(resolved, &checker.program)?;
    }
    Ok(HirCheckOutput {
        program: checker.program,
        diagnostics: checker.diagnostics,
        complete: checker.complete,
    })
}

#[derive(Clone, Copy)]
enum ExpressionExpectation {
    Direct(TypeId),
    CallableOutcome { full: TypeId, success: TypeId },
}

impl ExpressionExpectation {
    fn contextual_type(self) -> TypeId {
        match self {
            Self::Direct(ty) => ty,
            Self::CallableOutcome { success, .. } => success,
        }
    }

    fn resulting_type(self) -> TypeId {
        match self {
            Self::Direct(ty) | Self::CallableOutcome { full: ty, .. } => ty,
        }
    }
}

#[derive(Clone, Copy)]
struct CallableContext {
    full: TypeId,
    success: TypeId,
    error: Option<TypeId>,
    signature: Span,
}

impl CallableContext {
    fn expectation(self) -> ExpressionExpectation {
        if self.error.is_some() {
            ExpressionExpectation::CallableOutcome {
                full: self.full,
                success: self.success,
            }
        } else {
            ExpressionExpectation::Direct(self.full)
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PatternContext {
    Binding,
    For,
    Match,
}

#[derive(Clone)]
struct CheckedPattern {
    id: HirPatternId,
    shape: PatternShape,
    valid: bool,
}

#[derive(Clone)]
struct PatternPathInfo {
    resolved: ResolvedName,
    suffix: Vec<PatternPathSegment>,
    applied: Option<TypeId>,
}

#[derive(Clone)]
struct PatternPathSegment {
    name: Name,
    span: Span,
}

struct CheckedRecordFields {
    fields: Vec<HirPatternField>,
    ordered_patterns: Vec<HirPatternId>,
    shapes: Vec<PatternShape>,
    valid: bool,
    has_rest: bool,
}

struct UsefulnessState {
    matrix: Vec<Vec<PatternShape>>,
    candidate: Vec<PatternShape>,
    types: Vec<TypeId>,
}

#[derive(Clone, Debug)]
enum PatternShape {
    Wildcard,
    Constructor {
        key: PatternConstructor,
        arguments: Vec<PatternShape>,
    },
    Array {
        elements: Arc<[PatternShape]>,
        offset: usize,
        has_rest: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum PatternConstructor {
    Unit,
    Bool(bool),
    OptionSome,
    OptionNone,
    ResultOk,
    ResultErr,
    Tuple(usize),
    Newtype(SymbolId),
    Record(SymbolId),
    Variant(MemberId),
    Union(TypeId),
    ArrayEmpty,
    ArrayCons,
    Literal { ty: TypeId, value: String },
}

type CompletePatternConstructors = Vec<(PatternConstructor, Vec<TypeId>)>;
type HirSliceOperands = (
    Option<HirExpressionId>,
    Option<HirExpressionId>,
    Option<HirExpressionId>,
);

#[derive(Clone, Default)]
struct BodyContext {
    locals: BTreeMap<LocalId, TypeId>,
    local_permissions: BTreeMap<LocalId, PlacePermission>,
    receiver: Option<TypeId>,
    receiver_permission: PlacePermission,
    callable: Option<CallableContext>,
    loops: Vec<HirLoopId>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum PlacePermission {
    Invalid,
    #[default]
    Immutable,
    MutRoot,
    Replace,
}

impl PlacePermission {
    fn projected(self) -> Self {
        match self {
            Self::Invalid => Self::Invalid,
            Self::Immutable => Self::Immutable,
            Self::MutRoot | Self::Replace => Self::Replace,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum StaticPlaceRoot {
    Local(LocalId),
    Receiver,
    Symbol(SymbolId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum StaticPlaceOperand {
    Local(LocalId),
    Constant(SymbolId),
    Literal { ty: TypeId, value: String },
    Tuple(Vec<StaticPlaceOperand>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum StaticPlaceProjection {
    Field(MemberId),
    TupleField(u32),
    Index(StaticPlaceOperand),
    DynamicIndex(TextRange),
    Slice {
        start: Option<StaticPlaceOperand>,
        end: Option<StaticPlaceOperand>,
        step: Option<StaticPlaceOperand>,
    },
    DynamicSlice(TextRange),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StaticPlace {
    root: StaticPlaceRoot,
    projections: Vec<StaticPlaceProjection>,
}

struct CheckedAssignmentTarget {
    span: Span,
    kind: CheckedAssignmentTargetKind,
}

#[derive(Clone)]
struct CallParameterInfo {
    index: u32,
    name: Option<Name>,
    mode: ParameterMode,
    ty: TypeId,
    receiver: bool,
}

#[derive(Clone)]
struct CallShape {
    fixed: Vec<CallParameterInfo>,
    variadic: Option<(Option<Name>, TypeId)>,
    outcome: TypeId,
}

#[derive(Clone, Copy)]
struct CallSite<'a> {
    file: FileId,
    range: TextRange,
    suffix: SyntaxNodeRef<'a>,
    expected: Option<ExpressionExpectation>,
}

enum ConstantDiagnosticKind {
    Map(Vec<HirMapEntry>),
    Set(Vec<HirExpressionId>),
    Comparison(HirExpressionId, HirExpressionId),
}

struct GenericCallInference {
    callable: HirCallableId,
    variables: Vec<TypeId>,
    solver: InferenceContext,
    contradiction: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InferenceAssignment {
    Applied,
    Ambiguous,
    Mismatch,
}

enum CheckedAssignmentTargetKind {
    Place(CheckedPlace),
    Discard,
    Tuple(Vec<CheckedAssignmentTarget>),
}

struct CheckedPlace {
    expression: HirExpressionId,
    ty: TypeId,
    permission: PlacePermission,
    key: StaticPlace,
    map_entry: bool,
    slice: bool,
}

#[derive(Clone, Debug)]
struct FlowSummary {
    flow: HirFlow,
    breaks: BTreeSet<HirLoopId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum DiscardStatus {
    Satisfied,
    Deferred,
    Unsatisfied,
}

struct DiscardNode {
    floor: DiscardStatus,
    dependencies: Vec<TypeId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DiscardRequirement {
    floor: DiscardStatus,
    parameters: BTreeSet<u32>,
}

impl Default for DiscardRequirement {
    fn default() -> Self {
        Self {
            floor: DiscardStatus::Satisfied,
            parameters: BTreeSet::new(),
        }
    }
}

impl FlowSummary {
    fn completes() -> Self {
        Self {
            flow: HirFlow::MayComplete,
            breaks: BTreeSet::new(),
        }
    }

    fn diverges() -> Self {
        Self {
            flow: HirFlow::Diverges,
            breaks: BTreeSet::new(),
        }
    }

    fn then(mut self, next: Self) -> Self {
        if !self.flow.may_complete() {
            return self;
        }
        self.breaks.extend(next.breaks);
        self.flow = next.flow;
        self
    }
}

struct ExpressionChecker<'a> {
    sources: &'a SourceDatabase,
    parsed: BTreeMap<FileId, &'a Parsed>,
    resolved: &'a ResolvedProgram,
    program: HirProgram,
    diagnostics: Vec<Diagnostic>,
    max_nodes: u32,
    pattern_steps_remaining: u64,
    max_diagnostics: usize,
    complete: bool,
    next_loop_id: u32,
    discard_summaries: Option<BTreeMap<SymbolId, DiscardRequirement>>,
}

impl<'a> ExpressionChecker<'a> {
    fn check_discard_parameters(&mut self) -> Result<(), HirError> {
        let callables = self.program.callables.clone();
        for callable in callables {
            let generic_statuses = callable
                .generics
                .iter()
                .map(|parameter| {
                    let satisfied = parameter.bounds.iter().any(|bound| {
                        matches!(
                            bound.constructor(),
                            HirTraitConstructor::Prelude(name)
                                if matches!(name.as_str(), "Copy" | "Discard" | "Key")
                        )
                    });
                    (
                        parameter.position,
                        if satisfied {
                            DiscardStatus::Satisfied
                        } else {
                            DiscardStatus::Unsatisfied
                        },
                    )
                })
                .collect::<BTreeMap<_, _>>();
            for parameter in callable
                .parameters
                .iter()
                .filter(|parameter| parameter.discard && parameter.mode == ParameterMode::Value)
            {
                self.require_discard_with_generics(
                    parameter.span,
                    parameter.ty,
                    &generic_statuses,
                    "discard parameter",
                )?;
            }
        }
        Ok(())
    }

    fn check_constants(&mut self) -> Result<(), HirError> {
        let mut symbols = self.program.constants.keys().copied().collect::<Vec<_>>();
        symbols.sort_by(|left, right| {
            self.resolved
                .symbol(*left)
                .expect("HIR constants remain resolved")
                .identity()
                .cmp(
                    self.resolved
                        .symbol(*right)
                        .expect("HIR constants remain resolved")
                        .identity(),
                )
        });
        let stable_rank = symbols
            .iter()
            .enumerate()
            .map(|(rank, symbol)| (*symbol, rank))
            .collect::<BTreeMap<_, _>>();
        for symbol in &symbols {
            let declared = self.program.constants[symbol].declared_type;
            self.program
                .constants
                .get_mut(symbol)
                .expect("known constant")
                .ty = declared;
        }

        let symbol_set = symbols.iter().copied().collect::<BTreeSet<_>>();
        let mut dependencies = BTreeMap::<SymbolId, Vec<SymbolId>>::new();
        for symbol in &symbols {
            let initializer = self.program.constants[symbol].initializer;
            let Some(node) = self.find_node(initializer, None) else {
                self.complete = false;
                continue;
            };
            let mut direct = node
                .descendant_tokens()
                .filter_map(|token| self.resolved.reference(initializer.file(), token.range()))
                .filter_map(|reference| match reference.entity() {
                    ResolvedEntity::Name(ResolvedName::Symbol(symbol))
                        if symbol_set.contains(symbol) =>
                    {
                        Some(*symbol)
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            direct.sort_by_key(|dependency| stable_rank[dependency]);
            direct.dedup();
            dependencies.insert(*symbol, direct);
        }

        let mut cyclic = BTreeSet::new();
        let mut components = strongly_connected_components(&symbols, &dependencies);
        for component in &mut components {
            component.sort_by_key(|symbol| stable_rank[symbol]);
        }
        components.sort_by_key(|component| stable_rank[&component[0]]);
        for component in components {
            let is_cycle = component.len() > 1
                || component.first().is_some_and(|symbol| {
                    dependencies
                        .get(symbol)
                        .is_some_and(|items| items.contains(symbol))
                });
            if !is_cycle {
                continue;
            }
            cyclic.extend(component.iter().copied());
            let primary = self.program.constants[&component[0]].span;
            let related = component
                .iter()
                .skip(1)
                .map(|symbol| {
                    (
                        "constant in this dependency cycle",
                        self.program.constants[symbol].span,
                    )
                })
                .collect::<Vec<_>>();
            self.emit(
                primary,
                "E1902",
                "constant dependencies form an evaluation cycle",
                related,
                None,
            )?;
            for symbol in component {
                if self.program.constants[&symbol].ty.is_none() {
                    self.program
                        .constants
                        .get_mut(&symbol)
                        .expect("known constant")
                        .ty = Some(self.program.interner.error());
                }
            }
        }

        let acyclic = symbols
            .iter()
            .copied()
            .filter(|symbol| !cyclic.contains(symbol))
            .collect::<BTreeSet<_>>();
        let mut remaining = BTreeMap::<SymbolId, usize>::new();
        let mut users = BTreeMap::<SymbolId, Vec<SymbolId>>::new();
        for symbol in &acyclic {
            let direct = dependencies
                .get(symbol)
                .into_iter()
                .flatten()
                .copied()
                .filter(|dependency| acyclic.contains(dependency))
                .collect::<Vec<_>>();
            remaining.insert(*symbol, direct.len());
            for dependency in direct {
                users.entry(dependency).or_default().push(*symbol);
            }
        }
        let mut ready = remaining
            .iter()
            .filter_map(|(symbol, count)| (*count == 0).then_some((stable_rank[symbol], *symbol)))
            .collect::<BTreeSet<_>>();
        while let Some((_, symbol)) = ready.pop_first() {
            self.check_constant(symbol)?;
            for user in users.get(&symbol).into_iter().flatten() {
                let count = remaining
                    .get_mut(user)
                    .expect("all constant users have a dependency count");
                *count -= 1;
                if *count == 0 {
                    ready.insert((stable_rank[user], *user));
                }
            }
        }
        Ok(())
    }

    fn check_constant(&mut self, symbol: SymbolId) -> Result<(), HirError> {
        let constant = self.program.constants[&symbol].clone();
        let Some(node) = self.find_node(constant.initializer, None) else {
            self.complete = false;
            return Ok(());
        };
        let Some(expression) = AstExpression::cast(node) else {
            self.complete = false;
            return Ok(());
        };
        let mut context = BodyContext::default();
        let value = self.check_expression(
            constant.initializer.file(),
            expression.syntax(),
            constant.declared_type.map(ExpressionExpectation::Direct),
            &mut context,
        )?;
        let ty = self.expression_type(value);
        {
            let constant = self
                .program
                .constants
                .get_mut(&symbol)
                .expect("the checked constant is indexed");
            constant.ty = Some(ty);
            constant.value = Some(value);
        }
        let evaluated = match evaluate(&self.program, value) {
            Ok(value) => Some(value),
            Err(ConstantEvaluationError::Nonconstant { span, reason }) => {
                self.emit(
                    span,
                    "E1901",
                    format!("constant initializer is not compile-time evaluable: {reason}"),
                    Vec::new(),
                    None,
                )?;
                None
            }
            Err(ConstantEvaluationError::Panic { span, reason }) => {
                self.emit(
                    span,
                    "E1903",
                    format!("constant evaluation would fail: {reason}"),
                    Vec::new(),
                    None,
                )?;
                None
            }
            Err(ConstantEvaluationError::Unavailable) => {
                if !has_unavailable_input(&self.program, value) {
                    self.emit(
                        constant.initializer,
                        "E1901",
                        "constant initializer is not supported by closed compile-time evaluation",
                        Vec::new(),
                        None,
                    )?;
                }
                None
            }
            Err(ConstantEvaluationError::Type(error)) => return Err(error.into()),
        };
        self.program
            .constants
            .get_mut(&symbol)
            .expect("the checked constant is indexed")
            .evaluated = evaluated;
        Ok(())
    }

    fn check_callables(&mut self) -> Result<(), HirError> {
        let callables = self.program.callables.clone();
        for callable in callables {
            let Some(body_source) = callable.body_source else {
                continue;
            };
            if !callable.generics.is_empty() || !self.is_bootstrap_callable(&callable) {
                self.complete = false;
                continue;
            }
            let Some(node) = self.find_node(body_source, Some(SyntaxKind::Block)) else {
                self.complete = false;
                continue;
            };
            let mut context = BodyContext::default();
            let (success, error) = match self.program.interner.kind(callable.outcome)? {
                TypeKind::Result { success, error } => (*success, Some(*error)),
                _ => (callable.outcome, None),
            };
            context.callable = Some(CallableContext {
                full: callable.outcome,
                success,
                error,
                signature: callable.span,
            });
            for parameter in &callable.parameters {
                let permission = match parameter.mode() {
                    ParameterMode::Mut => PlacePermission::MutRoot,
                    ParameterMode::Var => PlacePermission::Replace,
                    ParameterMode::Value | ParameterMode::Ref => PlacePermission::Immutable,
                };
                if parameter.is_receiver() {
                    context.receiver = Some(parameter.ty());
                    context.receiver_permission = permission;
                } else if let Some(local) = parameter.local() {
                    context.locals.insert(local, parameter.ty());
                    context.local_permissions.insert(local, permission);
                }
            }
            let root = self.check_expression(
                body_source.file(),
                node,
                Some(context.callable.expect("just initialized").expectation()),
                &mut context,
            )?;
            self.program.local_types.extend(context.locals);
            self.program.bodies.insert(callable.id, HirBody { root });
        }
        Ok(())
    }

    fn check_constant_collection_diagnostics(&mut self) -> Result<(), HirError> {
        let candidates = self
            .program
            .expressions_with_ids()
            .filter_map(|(id, expression)| match expression.kind() {
                HirExpressionKind::Map(entries) => Some((
                    id,
                    expression.span(),
                    ConstantDiagnosticKind::Map(entries.clone()),
                )),
                HirExpressionKind::Set(items) => Some((
                    id,
                    expression.span(),
                    ConstantDiagnosticKind::Set(items.clone()),
                )),
                HirExpressionKind::Binary {
                    operator:
                        HirBinaryOperator::Less
                        | HirBinaryOperator::LessEqual
                        | HirBinaryOperator::Greater
                        | HirBinaryOperator::GreaterEqual
                        | HirBinaryOperator::Equal
                        | HirBinaryOperator::NotEqual,
                    left,
                    right,
                } => Some((
                    id,
                    expression.span(),
                    ConstantDiagnosticKind::Comparison(*left, *right),
                )),
                _ => None,
            })
            .collect::<Vec<_>>();

        for (_, expression_span, candidate) in candidates {
            match candidate {
                ConstantDiagnosticKind::Map(entries) => {
                    let mut previous = Vec::new();
                    for entry in entries {
                        let key_id = entry.key();
                        let Ok(key) = evaluate(&self.program, key_id) else {
                            continue;
                        };
                        let key_span = self.program.expressions[key_id.0 as usize].span;
                        let mut duplicate = None;
                        for (previous_value, previous_span) in &previous {
                            match values_equal(&self.program, previous_value, &key) {
                                Ok(true) => {
                                    duplicate = Some(*previous_span);
                                    break;
                                }
                                Ok(false) | Err(ConstantEvaluationError::Unavailable) => {}
                                Err(ConstantEvaluationError::Type(error)) => {
                                    return Err(error.into());
                                }
                                Err(
                                    ConstantEvaluationError::Nonconstant { .. }
                                    | ConstantEvaluationError::Panic { .. },
                                ) => {}
                            }
                        }
                        if let Some(first) = duplicate {
                            self.emit(
                                key_span,
                                "E1116",
                                "map literal repeats a compile-time-known key",
                                vec![("the same key first appears here", first)],
                                None,
                            )?;
                        } else {
                            previous.push((key, key_span));
                        }
                    }
                }
                ConstantDiagnosticKind::Set(items) => {
                    let mut previous = Vec::new();
                    for item_id in items {
                        let Ok(item) = evaluate(&self.program, item_id) else {
                            continue;
                        };
                        let item_span = self.program.expressions[item_id.0 as usize].span;
                        let mut duplicate = None;
                        for (previous_value, previous_span) in &previous {
                            match values_equal(&self.program, previous_value, &item) {
                                Ok(true) => {
                                    duplicate = Some(*previous_span);
                                    break;
                                }
                                Ok(false) | Err(ConstantEvaluationError::Unavailable) => {}
                                Err(ConstantEvaluationError::Type(error)) => {
                                    return Err(error.into());
                                }
                                Err(
                                    ConstantEvaluationError::Nonconstant { .. }
                                    | ConstantEvaluationError::Panic { .. },
                                ) => {}
                            }
                        }
                        if let Some(first) = duplicate {
                            self.emit_with_severity(
                                Severity::Warning,
                                item_span,
                                "W1011",
                                "set literal repeats a compile-time-known value",
                                vec![("the same value first appears here", first)],
                                None,
                            )?;
                        } else {
                            previous.push((item, item_span));
                        }
                    }
                }
                ConstantDiagnosticKind::Comparison(left, right) => {
                    let left = evaluate(&self.program, left).ok();
                    let right = evaluate(&self.program, right).ok();
                    if left.as_ref().is_some_and(is_nan) || right.as_ref().is_some_and(is_nan) {
                        self.emit_warning(
                            expression_span,
                            "W1008",
                            "comparison has a compile-time-known NaN operand",
                        )?;
                    }
                }
            }
        }
        Ok(())
    }

    fn is_bootstrap_callable(&self, callable: &HirCallableSignature) -> bool {
        match callable.id {
            HirCallableId::Symbol(_) => true,
            HirCallableId::Member(member) => self.resolved.member(member).is_some_and(|member| {
                matches!(
                    member.kind(),
                    MemberKind::InherentMethod | MemberKind::AssociatedFunction
                )
            }),
            HirCallableId::Implementation(_) => false,
        }
    }

    fn check_reachability_warnings(&mut self) -> Result<(), HirError> {
        let mut pending = self
            .program
            .constants
            .values()
            .filter_map(|constant| constant.value())
            .chain(self.program.bodies.values().map(HirBody::root))
            .collect::<Vec<_>>();
        let mut visited = BTreeSet::new();
        let mut warnings = Vec::new();

        while let Some(id) = pending.pop() {
            if !visited.insert(id) {
                continue;
            }
            let expression = &self.program.expressions[id.0 as usize];
            match &expression.kind {
                HirExpressionKind::Recovery
                | HirExpressionKind::Literal(_)
                | HirExpressionKind::Local(_)
                | HirExpressionKind::Constant(_)
                | HirExpressionKind::Function(_)
                | HirExpressionKind::SpecializedFunction { .. }
                | HirExpressionKind::Receiver
                | HirExpressionKind::Break { .. }
                | HirExpressionKind::Continue { .. } => {}
                HirExpressionKind::Tuple(items)
                | HirExpressionKind::Array(items)
                | HirExpressionKind::Set(items)
                | HirExpressionKind::InterpolatedString { values: items, .. } => self
                    .queue_reachable_sequence(items.iter().copied(), &mut pending, &mut warnings),
                HirExpressionKind::Map(entries) => self.queue_reachable_sequence(
                    entries
                        .iter()
                        .flat_map(|entry| [entry.key(), entry.value()]),
                    &mut pending,
                    &mut warnings,
                ),
                HirExpressionKind::Newtype { value, .. } => pending.push(*value),
                HirExpressionKind::NumericConversion { value, .. } => pending.push(*value),
                HirExpressionKind::Record { fields, .. } => self.queue_reachable_sequence(
                    fields.iter().map(HirRecordFieldValue::value),
                    &mut pending,
                    &mut warnings,
                ),
                HirExpressionKind::Variant { payload, .. } => match payload {
                    HirVariantValue::Unit => {}
                    HirVariantValue::Tuple(values) => self.queue_reachable_sequence(
                        values.iter().copied(),
                        &mut pending,
                        &mut warnings,
                    ),
                    HirVariantValue::Record(fields) => self.queue_reachable_sequence(
                        fields.iter().map(HirRecordFieldValue::value),
                        &mut pending,
                        &mut warnings,
                    ),
                },
                HirExpressionKind::RecordUpdate { base, fields } => self.queue_reachable_sequence(
                    std::iter::once(*base).chain(fields.iter().map(HirRecordFieldValue::value)),
                    &mut pending,
                    &mut warnings,
                ),
                HirExpressionKind::Block { statements, tail } => {
                    let mut reachable = true;
                    for statement in statements {
                        if reachable {
                            self.queue_reachable_statement(statement, &mut pending, &mut warnings);
                            reachable = self.statement_summary(statement).flow.may_complete();
                        } else {
                            warnings.push(statement.span());
                        }
                    }
                    if let Some(tail) = tail {
                        if reachable {
                            pending.push(*tail);
                        } else {
                            warnings.push(self.program.expressions[tail.0 as usize].span);
                        }
                    }
                }
                HirExpressionKind::Prefix { operand, .. }
                | HirExpressionKind::Field { base: operand, .. }
                | HirExpressionKind::TupleField { base: operand, .. }
                | HirExpressionKind::OptionSome { value: operand }
                | HirExpressionKind::ResultOk { value: operand }
                | HirExpressionKind::ResultErr { error: operand }
                | HirExpressionKind::PropagateOption { value: operand }
                | HirExpressionKind::PropagateResult { value: operand, .. }
                | HirExpressionKind::Coerce { value: operand, .. } => pending.push(*operand),
                HirExpressionKind::Binary { left, right, .. }
                | HirExpressionKind::Range {
                    start: left,
                    end: right,
                    ..
                }
                | HirExpressionKind::Contains {
                    item: left,
                    container: right,
                    ..
                }
                | HirExpressionKind::Index {
                    base: left,
                    index: right,
                    ..
                } => self.queue_reachable_sequence([*left, *right], &mut pending, &mut warnings),
                HirExpressionKind::Slice {
                    base,
                    start,
                    end,
                    step,
                } => self.queue_reachable_sequence(
                    std::iter::once(*base)
                        .chain(start.iter().copied())
                        .chain(end.iter().copied())
                        .chain(step.iter().copied()),
                    &mut pending,
                    &mut warnings,
                ),
                HirExpressionKind::Call { callee, arguments } => self.queue_reachable_sequence(
                    std::iter::once(*callee).chain(arguments.iter().map(HirCallArgument::value)),
                    &mut pending,
                    &mut warnings,
                ),
                HirExpressionKind::PreludePanic { message } => pending.push(*message),
                HirExpressionKind::PreludeAssert {
                    condition,
                    message_parts,
                } => self.queue_reachable_sequence(
                    std::iter::once(*condition)
                        .chain(message_parts.iter().map(|part| part.value())),
                    &mut pending,
                    &mut warnings,
                ),
                HirExpressionKind::BootstrapHostCall { arguments, .. } => self
                    .queue_reachable_sequence(
                        arguments.iter().copied(),
                        &mut pending,
                        &mut warnings,
                    ),
                HirExpressionKind::If {
                    condition,
                    then_branch,
                    else_branch,
                } => {
                    pending.push(*condition);
                    if self.expression_flow(*condition).may_complete() {
                        pending.push(*then_branch);
                        pending.extend(else_branch.iter().copied());
                    } else {
                        warnings.push(self.program.expressions[then_branch.0 as usize].span);
                        if let Some(else_branch) = else_branch {
                            warnings.push(self.program.expressions[else_branch.0 as usize].span);
                        }
                    }
                }
                HirExpressionKind::Match { scrutinee, arms } => {
                    pending.push(*scrutinee);
                    if self.expression_flow(*scrutinee).may_complete() {
                        for arm in arms {
                            if let Some(guard) = arm.guard {
                                pending.push(guard);
                                if self.expression_flow(guard).may_complete() {
                                    pending.push(arm.body);
                                } else {
                                    warnings
                                        .push(self.program.expressions[arm.body.0 as usize].span);
                                }
                            } else {
                                pending.push(arm.body);
                            }
                        }
                    } else {
                        for arm in arms {
                            let first = arm.guard.unwrap_or(arm.body);
                            warnings.push(self.program.expressions[first.0 as usize].span);
                        }
                    }
                }
                HirExpressionKind::Return { value } => pending.extend(value.iter().copied()),
                HirExpressionKind::Fail { error } => pending.push(*error),
            }
        }

        warnings.sort_by_key(|span| (span.file(), span.range().start(), span.range().end()));
        warnings.dedup();
        for span in warnings {
            self.emit_warning(span, "W1006", "unreachable code")?;
        }
        Ok(())
    }

    fn queue_reachable_sequence(
        &self,
        expressions: impl IntoIterator<Item = HirExpressionId>,
        pending: &mut Vec<HirExpressionId>,
        warnings: &mut Vec<Span>,
    ) {
        let mut reachable = true;
        for expression in expressions {
            if reachable {
                pending.push(expression);
                reachable = self.expression_flow(expression).may_complete();
            } else {
                warnings.push(self.program.expressions[expression.0 as usize].span);
            }
        }
    }

    fn queue_reachable_statement(
        &self,
        statement: &HirStatement,
        pending: &mut Vec<HirExpressionId>,
        warnings: &mut Vec<Span>,
    ) {
        match statement {
            HirStatement::Binding { value, .. }
            | HirStatement::Expression { value, .. }
            | HirStatement::Discard { value, .. } => pending.push(*value),
            HirStatement::Assignment { target, value, .. } => {
                let mut expressions = Vec::new();
                collect_assignment_target_expressions(target, &mut expressions);
                expressions.push(*value);
                self.queue_reachable_sequence(expressions, pending, warnings);
            }
            HirStatement::For { kind, body, .. } => {
                let header = match kind {
                    HirForKind::Infinite => None,
                    HirForKind::Conditional { condition } => Some(*condition),
                    HirForKind::Iterate { source, .. } => Some(*source),
                };
                if let Some(header) = header {
                    pending.push(header);
                    if self.expression_flow(header).may_complete() {
                        pending.push(*body);
                    } else {
                        warnings.push(self.program.expressions[body.0 as usize].span);
                    }
                } else {
                    pending.push(*body);
                }
            }
        }
    }

    fn check_expression(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        expected: Option<ExpressionExpectation>,
        context: &mut BodyContext,
    ) -> Result<HirExpressionId, HirError> {
        let value = self.check_expression_raw(file, node, expected, context)?;
        let Some(expectation) = expected else {
            return Ok(value);
        };
        let actual = self.expression_type(value);
        if actual == self.program.interner.error() {
            return Ok(value);
        }
        if let ExpressionExpectation::CallableOutcome { full, success } = expectation {
            if actual == full {
                return Ok(value);
            }
            if actual == self.program.interner.scalar(ScalarType::Never) {
                return self.coerce_existing(value, full);
            }
            let Some(assignability) = self.program.interner.assignability(actual, success)? else {
                let expected_name = self.program.interner.canonical(success)?;
                let actual_name = self.program.interner.canonical(actual)?;
                self.emit(
                    self.sources.span(file, node.range())?,
                    "E1102",
                    format!(
                        "expected success `{expected_name}` or the complete result, found `{actual_name}`"
                    ),
                    Vec::new(),
                    Some((expected_name, actual_name)),
                )?;
                return self.recovery_expression(file, node.range());
            };
            let value = if assignability == Assignability::Exact {
                value
            } else {
                self.coerce_existing(value, success)?
            };
            return self.allocate_expression(HirExpression {
                span: self.sources.span(file, node.range())?,
                ty: full,
                category: HirValueCategory::Value,
                kind: HirExpressionKind::ResultOk { value },
            });
        }
        let expected = expectation.contextual_type();
        let Some(assignability) = self.program.interner.assignability(actual, expected)? else {
            let expected_name = self.program.interner.canonical(expected)?;
            let actual_name = self.program.interner.canonical(actual)?;
            self.emit(
                self.sources.span(file, node.range())?,
                "E1102",
                format!("expected `{expected_name}`, found `{actual_name}`"),
                Vec::new(),
                Some((expected_name, actual_name)),
            )?;
            return self.recovery_expression(file, node.range());
        };
        if assignability == Assignability::Exact {
            return Ok(value);
        }
        self.allocate_expression(HirExpression {
            span: self.sources.span(file, node.range())?,
            ty: expected,
            category: HirValueCategory::Value,
            kind: HirExpressionKind::Coerce {
                kind: assignability,
                value,
            },
        })
    }

    fn check_expression_raw(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        expected: Option<ExpressionExpectation>,
        context: &mut BodyContext,
    ) -> Result<HirExpressionId, HirError> {
        let Some(expression) = AstExpression::cast(node) else {
            self.complete = false;
            return self.recovery_expression(file, node.range());
        };
        match expression {
            AstExpression::Literal(_) => self.check_literal(file, node, expected),
            AstExpression::String(_) => self.check_string(file, node, context),
            AstExpression::Path(_) => self.check_path(file, node, expected, context),
            AstExpression::SelfValue(_) => self.check_receiver(file, node, context),
            AstExpression::Tuple(_) => self.check_tuple(file, node, expected, context),
            AstExpression::BracketLiteral(_) => {
                self.check_bracket_literal(file, node, expected, context)
            }
            AstExpression::SetLiteral(_) => self.check_set_literal(file, node, expected, context),
            AstExpression::RecordLike(_) => self.check_record_like(file, node, expected, context),
            AstExpression::Group(_) => {
                let Some(inner) = node
                    .child_nodes()
                    .find(|child| AstExpression::cast(*child).is_some())
                else {
                    return self.recovery_expression(file, node.range());
                };
                self.check_expression(file, inner, expected, context)
            }
            AstExpression::Block(_) => self.check_block(file, node, expected, context),
            AstExpression::If(_) => self.check_if(file, node, expected, context),
            AstExpression::Match(_) => self.check_match(file, node, expected, context),
            AstExpression::Prefix(_) => self.check_prefix(file, node, expected, context),
            AstExpression::Binary(_) => self.check_binary(file, node, expected, context),
            AstExpression::Postfix(_) => self.check_postfix(file, node, expected, context),
            AstExpression::OptionResult(_) => {
                self.check_option_result(file, node, expected, context)
            }
            AstExpression::Closure(_)
            | AstExpression::Await(_)
            | AstExpression::Spawn(_)
            | AstExpression::Scope(_)
            | AstExpression::Unsafe(_) => {
                self.complete = false;
                self.recovery_expression(file, node.range())
            }
        }
    }

    fn check_literal(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        expected: Option<ExpressionExpectation>,
    ) -> Result<HirExpressionId, HirError> {
        let expected = expected.map(ExpressionExpectation::contextual_type);
        let Some(token) = node
            .descendant_tokens()
            .find(|token| !token.kind().is_trivia() && token.kind() != TokenKind::Nl)
        else {
            return self.recovery_expression(file, node.range());
        };
        let (literal, ty) = match token.kind() {
            TokenKind::LParen => (
                HirLiteral::Unit,
                self.program.interner.scalar(ScalarType::Unit),
            ),
            TokenKind::True => (
                HirLiteral::Bool(true),
                self.program.interner.scalar(ScalarType::Bool),
            ),
            TokenKind::False => (
                HirLiteral::Bool(false),
                self.program.interner.scalar(ScalarType::Bool),
            ),
            TokenKind::None => {
                let Some(expected) = expected else {
                    self.emit(
                        self.sources.span(file, node.range())?,
                        "E1304",
                        "`none` requires a direct option type",
                        Vec::new(),
                        None,
                    )?;
                    return self.recovery_expression(file, node.range());
                };
                if !self.program.interner.accepts_none(expected)? {
                    self.emit(
                        self.sources.span(file, node.range())?,
                        "E1304",
                        "`none` requires a direct option type, not a containing union",
                        Vec::new(),
                        None,
                    )?;
                    return self.recovery_expression(file, node.range());
                }
                (HirLiteral::None, expected)
            }
            TokenKind::IntegerLiteral => {
                let spelling = self.token_text(file, token)?.to_owned();
                let Some(magnitude) = integer_magnitude(&spelling) else {
                    self.emit(
                        self.sources.span(file, token.range())?,
                        "E1102",
                        "integer literal exceeds the intrinsic numeric domain",
                        Vec::new(),
                        None,
                    )?;
                    return self.recovery_expression(file, node.range());
                };
                let scalar = if let Some(suffix) = integer_suffix(&spelling) {
                    suffix
                } else {
                    let Some(scalar) = self.contextual_numeric_scalar(
                        expected,
                        ScalarType::Int,
                        is_integer_scalar,
                        self.sources.span(file, token.range())?,
                    )?
                    else {
                        return self.recovery_expression(file, node.range());
                    };
                    scalar
                };
                if !integer_fits_positive(magnitude, scalar) {
                    self.emit(
                        self.sources.span(file, token.range())?,
                        "E1102",
                        format!("integer literal is not representable as `{scalar}`"),
                        Vec::new(),
                        Some((scalar.to_string(), "integer literal".into())),
                    )?;
                    return self.recovery_expression(file, node.range());
                }
                (
                    HirLiteral::Integer(spelling),
                    self.program.interner.scalar(scalar),
                )
            }
            TokenKind::FloatLiteral => {
                let spelling = self.token_text(file, token)?.to_owned();
                let scalar = if let Some(suffix) = float_suffix(&spelling) {
                    suffix
                } else {
                    let Some(scalar) = self.contextual_numeric_scalar(
                        expected,
                        ScalarType::Float,
                        is_float_scalar,
                        self.sources.span(file, token.range())?,
                    )?
                    else {
                        return self.recovery_expression(file, node.range());
                    };
                    scalar
                };
                if !float_is_representable(&spelling, scalar) {
                    self.emit(
                        self.sources.span(file, token.range())?,
                        "E1102",
                        format!("floating literal is not representable as `{scalar}`"),
                        Vec::new(),
                        Some((scalar.to_string(), "floating literal".into())),
                    )?;
                    return self.recovery_expression(file, node.range());
                }
                (
                    HirLiteral::Float(spelling),
                    self.program.interner.scalar(scalar),
                )
            }
            TokenKind::CharLiteral => (
                HirLiteral::Char(self.token_text(file, token)?.to_owned()),
                self.program.interner.scalar(ScalarType::Char),
            ),
            TokenKind::RawStringLiteral | TokenKind::RawMultilineStringLiteral => (
                HirLiteral::String(self.token_text(file, token)?.to_owned()),
                self.program.interner.scalar(ScalarType::String),
            ),
            _ => return self.recovery_expression(file, node.range()),
        };
        self.allocate_expression(HirExpression {
            span: self.sources.span(file, node.range())?,
            ty,
            category: HirValueCategory::Value,
            kind: HirExpressionKind::Literal(literal),
        })
    }

    fn check_option_result(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        expected: Option<ExpressionExpectation>,
        context: &mut BodyContext,
    ) -> Result<HirExpressionId, HirError> {
        let Some(constructor) = node.child_tokens().find(|token| {
            matches!(
                token.kind(),
                TokenKind::Some | TokenKind::Ok | TokenKind::Err
            )
        }) else {
            return self.recovery_expression(file, node.range());
        };
        let Some(payload) = node
            .child_nodes()
            .find(|child| AstExpression::cast(*child).is_some())
        else {
            return self.recovery_expression(file, node.range());
        };

        match constructor.kind() {
            TokenKind::Some => {
                let contextual = expected.map(ExpressionExpectation::contextual_type);
                let option_type = contextual
                    .map(|ty| self.unique_constructor_member(ty, is_option_type))
                    .transpose()?
                    .flatten();
                let value = if let Some(option_type) = option_type {
                    let TypeKind::Option(item) = self.program.interner.kind(option_type)? else {
                        unreachable!("the option constructor predicate is exact");
                    };
                    self.check_with_expected_diagnostic(
                        file,
                        payload,
                        *item,
                        context,
                        "E1304",
                        "`some` payload",
                    )?
                } else {
                    self.check_expression(file, payload, None, context)?
                };
                let value_type = self.expression_type(value);
                if value_type == self.program.interner.error() {
                    return self.recovery_expression(file, node.range());
                }
                let option_type = match option_type {
                    Some(option_type) => option_type,
                    None => self.program.interner.option(value_type)?,
                };
                self.allocate_expression(HirExpression {
                    span: self.sources.span(file, node.range())?,
                    ty: option_type,
                    category: HirValueCategory::Value,
                    kind: HirExpressionKind::OptionSome { value },
                })
            }
            TokenKind::Ok | TokenKind::Err => {
                let result_type = match expected {
                    Some(ExpressionExpectation::CallableOutcome { full, .. })
                        if is_result_type(self.program.interner.kind(full)?) =>
                    {
                        Some(full)
                    }
                    Some(ExpressionExpectation::Direct(ty)) => {
                        self.unique_constructor_member(ty, is_result_type)?
                    }
                    _ => None,
                };
                let Some(result_type) = result_type else {
                    let _ = self.check_expression(file, payload, None, context)?;
                    self.emit(
                        self.sources.span(file, node.range())?,
                        "E1304",
                        "`ok` and `err` require one direct contextual `Result` type",
                        Vec::new(),
                        None,
                    )?;
                    return self.recovery_expression(file, node.range());
                };
                let TypeKind::Result { success, error } =
                    self.program.interner.kind(result_type)?.clone()
                else {
                    unreachable!("the result constructor predicate is exact");
                };
                let (payload_type, label) = if constructor.kind() == TokenKind::Ok {
                    (success, "`ok` payload")
                } else {
                    (error, "`err` payload")
                };
                let value = self.check_with_expected_diagnostic(
                    file,
                    payload,
                    payload_type,
                    context,
                    "E1304",
                    label,
                )?;
                let kind = if constructor.kind() == TokenKind::Ok {
                    HirExpressionKind::ResultOk { value }
                } else {
                    HirExpressionKind::ResultErr { error: value }
                };
                self.allocate_expression(HirExpression {
                    span: self.sources.span(file, node.range())?,
                    ty: result_type,
                    category: HirValueCategory::Value,
                    kind,
                })
            }
            _ => unreachable!("constructor token selection is closed"),
        }
    }

    fn unique_constructor_member(
        &self,
        expected: TypeId,
        predicate: fn(&TypeKind) -> bool,
    ) -> Result<Option<TypeId>, HirError> {
        if predicate(self.program.interner.kind(expected)?) {
            return Ok(Some(expected));
        }
        let TypeKind::Union(members) = self.program.interner.kind(expected)? else {
            return Ok(None);
        };
        let mut candidates = members.iter().copied().filter(|member| {
            self.program
                .interner
                .kind(*member)
                .ok()
                .is_some_and(predicate)
        });
        let first = candidates.next();
        Ok(first.filter(|_| candidates.next().is_none()))
    }

    fn check_with_expected_diagnostic(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        expected: TypeId,
        context: &mut BodyContext,
        code: &str,
        subject: &str,
    ) -> Result<HirExpressionId, HirError> {
        let value = self.check_expression_raw(
            file,
            node,
            Some(ExpressionExpectation::Direct(expected)),
            context,
        )?;
        let actual = self.expression_type(value);
        if actual == self.program.interner.error() {
            return Ok(value);
        }
        let Some(assignability) = self.program.interner.assignability(actual, expected)? else {
            let expected_name = self.program.interner.canonical(expected)?;
            let actual_name = self.program.interner.canonical(actual)?;
            self.emit(
                self.sources.span(file, node.range())?,
                code,
                format!("{subject} expected `{expected_name}`, found `{actual_name}`"),
                Vec::new(),
                Some((expected_name, actual_name)),
            )?;
            return self.recovery_expression(file, node.range());
        };
        if assignability == Assignability::Exact {
            Ok(value)
        } else {
            self.coerce_existing(value, expected)
        }
    }

    fn check_error_with_expected_diagnostic(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        expected: TypeId,
        context: &mut BodyContext,
        code: &str,
        subject: &str,
    ) -> Result<HirExpressionId, HirError> {
        let value = self.check_expression_raw(
            file,
            node,
            Some(ExpressionExpectation::Direct(expected)),
            context,
        )?;
        let actual = self.expression_type(value);
        if actual == self.program.interner.error() {
            return Ok(value);
        }
        let Some(assignability) = self.error_assignability(actual, expected)? else {
            let expected_name = self.program.interner.canonical(expected)?;
            let actual_name = self.program.interner.canonical(actual)?;
            self.emit(
                self.sources.span(file, node.range())?,
                code,
                format!("{subject} expected `{expected_name}`, found `{actual_name}`"),
                Vec::new(),
                Some((expected_name, actual_name)),
            )?;
            return self.recovery_expression(file, node.range());
        };
        if assignability == Assignability::Exact {
            Ok(value)
        } else {
            self.coerce_existing(value, expected)
        }
    }

    fn error_assignability(
        &self,
        actual: TypeId,
        expected: TypeId,
    ) -> Result<Option<Assignability>, HirError> {
        Ok(self
            .program
            .interner
            .assignability(actual, expected)?
            .filter(|assignability| {
                matches!(
                    assignability,
                    Assignability::Exact
                        | Assignability::UnionInjection
                        | Assignability::UnionWidening
                        | Assignability::Diverging
                )
            }))
    }

    fn emit_incompatible_propagation(
        &mut self,
        span: Span,
        operand: TypeId,
        callable: Option<CallableContext>,
        reason: &str,
    ) -> Result<(), HirError> {
        let operand = self.program.interner.canonical(operand)?;
        self.emit(
            span,
            "E1301",
            format!("{reason}; the operand has type `{operand}`"),
            callable
                .map(|callable| {
                    vec![(
                        "the enclosing callable is declared here",
                        callable.signature,
                    )]
                })
                .unwrap_or_default(),
            None,
        )
    }

    fn emit_error_propagation_mismatch(
        &mut self,
        span: Span,
        produced: TypeId,
        expected: TypeId,
        signature: Span,
    ) -> Result<(), HirError> {
        let produced_members = self.top_level_union_members(produced)?;
        let expected_members = self.top_level_union_members(expected)?;
        let missing = produced_members
            .into_iter()
            .filter(|member| !expected_members.contains(member))
            .map(|member| self.program.interner.canonical(member))
            .collect::<Result<Vec<_>, _>>()?;
        let produced_name = self.program.interner.canonical(produced)?;
        let expected_name = self.program.interner.canonical(expected)?;
        self.emit(
            span,
            "E1301",
            format!(
                "cannot propagate error `{produced_name}` into `{expected_name}`; missing members: {}",
                missing.join(", ")
            ),
            vec![("the enclosing error channel is declared here", signature)],
            Some((expected_name, produced_name)),
        )
    }

    fn top_level_union_members(&self, ty: TypeId) -> Result<Vec<TypeId>, HirError> {
        Ok(match self.program.interner.kind(ty)? {
            TypeKind::Union(members) => members.clone(),
            _ => vec![ty],
        })
    }

    fn check_string(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        context: &mut BodyContext,
    ) -> Result<HirExpressionId, HirError> {
        let interpolations = node
            .child_nodes()
            .filter(|child| child.kind() == SyntaxKind::Interpolation)
            .collect::<Vec<_>>();
        if !interpolations.is_empty() {
            self.complete = false;
        }
        let mut values = Vec::with_capacity(interpolations.len());
        for interpolation in interpolations {
            if let Some(expression) = interpolation
                .child_nodes()
                .find(|child| AstExpression::cast(*child).is_some())
            {
                values.push(self.check_expression(file, expression, None, context)?);
            }
        }
        let mut significant = node
            .descendant_tokens()
            .filter(|token| !token.kind().is_trivia());
        let Some(first) = significant.next() else {
            return self.recovery_expression(file, node.range());
        };
        let last = significant.last().unwrap_or(first);
        let literal_range = TextRange::new(first.range().start(), last.range().end())?;
        let source = self.source_text(file, literal_range)?.to_owned();
        self.allocate_expression(HirExpression {
            span: self.sources.span(file, node.range())?,
            ty: self.program.interner.scalar(ScalarType::String),
            category: HirValueCategory::Value,
            kind: if values.is_empty() {
                HirExpressionKind::Literal(HirLiteral::String(source))
            } else {
                HirExpressionKind::InterpolatedString { source, values }
            },
        })
    }

    fn check_path(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        expected: Option<ExpressionExpectation>,
        context: &mut BodyContext,
    ) -> Result<HirExpressionId, HirError> {
        if let Some(variant) = self.check_unit_variant_path(file, node, expected)? {
            return Ok(variant);
        }
        self.check_value_path(file, node, context, None)
    }

    fn check_value_path(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        context: &mut BodyContext,
        stop_before: Option<TextRange>,
    ) -> Result<HirExpressionId, HirError> {
        let Some(token) = node
            .child_tokens()
            .filter(|token| {
                self.resolved
                    .reference(file, token.range())
                    .is_some_and(|reference| matches!(reference.entity(), ResolvedEntity::Name(_)))
            })
            .last()
        else {
            self.complete = false;
            return self.recovery_expression(file, node.range());
        };
        let Some(reference) = self.resolved.reference(file, token.range()) else {
            self.complete = false;
            return self.recovery_expression(file, node.range());
        };
        let ResolvedEntity::Name(name) = reference.entity() else {
            self.complete = false;
            return self.recovery_expression(file, node.range());
        };
        let (ty, category, kind) = match name {
            ResolvedName::Local(local) => {
                let Some(ty) = context.locals.get(local).copied() else {
                    self.complete = false;
                    return self.recovery_expression(file, node.range());
                };
                (
                    ty,
                    HirValueCategory::Place,
                    HirExpressionKind::Local(*local),
                )
            }
            ResolvedName::Receiver => {
                let Some(ty) = context.receiver else {
                    self.complete = false;
                    return self.recovery_expression(file, node.range());
                };
                (ty, HirValueCategory::Place, HirExpressionKind::Receiver)
            }
            ResolvedName::Symbol(symbol) => {
                let declaration = self
                    .resolved
                    .symbol(*symbol)
                    .expect("resolved references contain valid symbols");
                match declaration.kind() {
                    SymbolKind::Constant => {
                        let ty = self
                            .program
                            .constant(*symbol)
                            .and_then(|constant| constant.ty)
                            .unwrap_or_else(|| self.program.interner.error());
                        (
                            ty,
                            HirValueCategory::Value,
                            HirExpressionKind::Constant(*symbol),
                        )
                    }
                    SymbolKind::Function => {
                        let id = HirCallableId::Symbol(*symbol);
                        let Some(callable) = self.callable(id) else {
                            self.complete = false;
                            return self.recovery_expression(file, node.range());
                        };
                        (
                            callable.function_type,
                            HirValueCategory::Value,
                            HirExpressionKind::Function(id),
                        )
                    }
                    SymbolKind::Type
                    | SymbolKind::Alias
                    | SymbolKind::Enum
                    | SymbolKind::Trait
                    | SymbolKind::NewtypeConstructor => {
                        self.complete = false;
                        return self.recovery_expression(file, node.range());
                    }
                }
            }
            ResolvedName::Prelude { .. }
            | ResolvedName::External { .. }
            | ResolvedName::ContextualSelf => {
                self.complete = false;
                return self.recovery_expression(file, node.range());
            }
        };
        let mut value = self.allocate_expression(HirExpression {
            span: self.sources.span(file, token.range())?,
            ty,
            category,
            kind,
        })?;

        let mut after_base = false;
        let mut expects_member = false;
        for element in node.elements() {
            match *element {
                SyntaxElement::Token(id) => {
                    let suffix = node.cst().token_ref(id);
                    if suffix.range() == token.range() {
                        after_base = true;
                        continue;
                    }
                    if !after_base || suffix.kind().is_trivia() {
                        continue;
                    }
                    if suffix.kind() == TokenKind::Dot {
                        expects_member = true;
                        continue;
                    }
                    if expects_member {
                        if stop_before == Some(suffix.range()) {
                            break;
                        }
                        value =
                            self.project_member_expression(file, node.range(), value, suffix)?;
                        expects_member = false;
                    }
                }
                SyntaxElement::Node(id) if after_base => {
                    let suffix = node.cst().node_ref(id);
                    if suffix.kind() == SyntaxKind::BracketPostfix {
                        let callable = match &self.program.expressions[value.0 as usize].kind {
                            HirExpressionKind::Function(callable) => Some(*callable),
                            _ => None,
                        };
                        value = if let Some(callable) = callable {
                            self.specialize_function_value(file, node.range(), suffix, callable)?
                        } else {
                            self.project_bracket_expression(
                                file,
                                node.range(),
                                value,
                                suffix,
                                context,
                            )?
                        };
                    }
                }
                SyntaxElement::Node(_) => {}
            }
        }
        Ok(value)
    }

    fn specialize_function_value(
        &mut self,
        file: FileId,
        range: TextRange,
        bracket: SyntaxNodeRef<'_>,
        id: HirCallableId,
    ) -> Result<HirExpressionId, HirError> {
        let Some(callable) = self.callable(id).cloned() else {
            self.complete = false;
            return self.recovery_expression(file, range);
        };
        if callable.generics.is_empty() {
            self.emit(
                self.sources.span(file, bracket.range())?,
                "E1104",
                "this function does not declare generic parameters",
                Vec::new(),
                None,
            )?;
            return self.recovery_expression(file, range);
        }
        let Some(arguments) = self.expression_generic_arguments(file, bracket)? else {
            return self.recovery_expression(file, range);
        };
        if arguments.len() != callable.generics.len() {
            self.emit(
                self.sources.span(file, bracket.range())?,
                "E1104",
                format!(
                    "generic function value expects {} type arguments, found {}",
                    callable.generics.len(),
                    arguments.len()
                ),
                Vec::new(),
                None,
            )?;
            return self.recovery_expression(file, range);
        }
        if callable
            .generics
            .iter()
            .any(|parameter| !parameter.bounds.is_empty())
        {
            self.complete = false;
        }
        let function_type = TypeSubstitution::new(arguments.clone())
            .apply(&mut self.program.interner, callable.function_type)?;
        self.allocate_expression(HirExpression {
            span: self.sources.span(file, range)?,
            ty: function_type,
            category: HirValueCategory::Value,
            kind: HirExpressionKind::SpecializedFunction {
                callable: id,
                arguments,
            },
        })
    }

    fn check_receiver(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        context: &BodyContext,
    ) -> Result<HirExpressionId, HirError> {
        let Some(ty) = context.receiver else {
            self.complete = false;
            return self.recovery_expression(file, node.range());
        };
        self.allocate_expression(HirExpression {
            span: self.sources.span(file, node.range())?,
            ty,
            category: HirValueCategory::Place,
            kind: HirExpressionKind::Receiver,
        })
    }

    fn project_member_expression(
        &mut self,
        file: FileId,
        range: TextRange,
        base: HirExpressionId,
        token: SyntaxTokenRef<'_>,
    ) -> Result<HirExpressionId, HirError> {
        let base_type = self.expression_type(base);
        let category = self
            .program
            .expression(base)
            .expect("allocated expression IDs remain valid")
            .category();
        if token.kind() == TokenKind::IntegerLiteral {
            let spelling = self.token_text(file, token)?;
            let Ok(index) = spelling.replace('_', "").parse::<u32>() else {
                self.emit(
                    self.sources.span(file, token.range())?,
                    "E1102",
                    "tuple slot is not representable as an index",
                    Vec::new(),
                    None,
                )?;
                return self.recovery_expression(file, range);
            };
            let TypeKind::Tuple(items) = self.program.interner.kind(base_type)? else {
                self.emit(
                    self.sources.span(file, token.range())?,
                    "E1102",
                    "numeric member access requires a tuple value",
                    Vec::new(),
                    None,
                )?;
                return self.recovery_expression(file, range);
            };
            let Some(ty) = items.get(index as usize).copied() else {
                self.emit(
                    self.sources.span(file, token.range())?,
                    "E1102",
                    "tuple slot is outside this tuple type",
                    Vec::new(),
                    None,
                )?;
                return self.recovery_expression(file, range);
            };
            return self.allocate_expression(HirExpression {
                span: self.sources.span(file, range)?,
                ty,
                category,
                kind: HirExpressionKind::TupleField { base, index },
            });
        }

        if self.is_inherent_method_member(file, base_type, token)? {
            self.emit(
                self.sources.span(file, token.range())?,
                "E1102",
                "methods are not values; invoke this method with `(...)`",
                Vec::new(),
                None,
            )?;
            return self.recovery_expression(file, range);
        }

        let Some((member, ty)) = self.resolve_field(file, base_type, token, "E1102")? else {
            return self.recovery_expression(file, range);
        };
        self.allocate_expression(HirExpression {
            span: self.sources.span(file, range)?,
            ty,
            category,
            kind: HirExpressionKind::Field { base, member },
        })
    }

    fn is_inherent_method_member(
        &self,
        file: FileId,
        base: TypeId,
        token: SyntaxTokenRef<'_>,
    ) -> Result<bool, HirError> {
        let spelling = token
            .token()
            .normalized_identifier()
            .map(str::to_owned)
            .unwrap_or(self.token_text(file, token)?.to_owned());
        let Ok(name) = MemberName::new(spelling) else {
            return Ok(false);
        };
        let Some((symbol, _, _)) = self.nominal_instance(base)? else {
            return Ok(false);
        };
        Ok(self
            .resolved
            .lookup_members(MemberOwner::Type(symbol), &name)
            .into_iter()
            .flatten()
            .any(|member| {
                self.resolved
                    .member(*member)
                    .is_some_and(|member| member.kind() == MemberKind::InherentMethod)
            }))
    }

    fn project_bracket_expression(
        &mut self,
        file: FileId,
        range: TextRange,
        base: HirExpressionId,
        bracket: SyntaxNodeRef<'_>,
        context: &mut BodyContext,
    ) -> Result<HirExpressionId, HirError> {
        let base_type = self.expression_type(base);
        let kind = self.program.interner.kind(base_type)?.clone();
        match kind {
            TypeKind::Intrinsic {
                constructor: IntrinsicType::Array,
                arguments,
            } => {
                if bracket
                    .child_nodes()
                    .any(|child| child.kind() == SyntaxKind::SliceSpec)
                {
                    let (start, end, step) = self.check_slice_operands(file, bracket, context)?;
                    return self.allocate_expression(HirExpression {
                        span: self.sources.span(file, range)?,
                        ty: base_type,
                        category: HirValueCategory::Value,
                        kind: HirExpressionKind::Slice {
                            base,
                            start,
                            end,
                            step,
                        },
                    });
                }
                let Some(index_node) = single_bracket_expression(bracket) else {
                    self.emit(
                        self.sources.span(file, bracket.range())?,
                        "E1102",
                        "array access requires exactly one index",
                        Vec::new(),
                        None,
                    )?;
                    return self.recovery_expression(file, range);
                };
                let index = self.check_expression(
                    file,
                    index_node,
                    Some(ExpressionExpectation::Direct(
                        self.program.interner.scalar(ScalarType::Int),
                    )),
                    context,
                )?;
                self.allocate_expression(HirExpression {
                    span: self.sources.span(file, range)?,
                    ty: arguments[0],
                    category: HirValueCategory::Place,
                    kind: HirExpressionKind::Index {
                        base,
                        index,
                        access: HirIndexAccess::Array,
                    },
                })
            }
            TypeKind::Intrinsic {
                constructor: IntrinsicType::Map,
                arguments,
            } => {
                if bracket
                    .child_nodes()
                    .any(|child| child.kind() == SyntaxKind::SliceSpec)
                {
                    let _ = self.check_slice_operands(file, bracket, context)?;
                    self.emit(
                        self.sources.span(file, bracket.range())?,
                        "E1102",
                        "maps do not support slicing",
                        Vec::new(),
                        None,
                    )?;
                    return self.recovery_expression(file, range);
                }
                let Some(key_node) = single_bracket_expression(bracket) else {
                    self.emit(
                        self.sources.span(file, bracket.range())?,
                        "E1102",
                        "map lookup requires exactly one key",
                        Vec::new(),
                        None,
                    )?;
                    return self.recovery_expression(file, range);
                };
                let key = self.check_expression(
                    file,
                    key_node,
                    Some(ExpressionExpectation::Direct(arguments[0])),
                    context,
                )?;
                let ty = self.program.interner.option(arguments[1])?;
                self.allocate_expression(HirExpression {
                    span: self.sources.span(file, range)?,
                    ty,
                    category: HirValueCategory::Value,
                    kind: HirExpressionKind::Index {
                        base,
                        index: key,
                        access: HirIndexAccess::MapLookup,
                    },
                })
            }
            TypeKind::Error => self.recovery_expression(file, range),
            TypeKind::Function(_) if self.bracket_looks_like_type_arguments(file, bracket) => {
                self.complete = false;
                self.recovery_expression(file, range)
            }
            _ => {
                let _ = self.check_bracket_expressions_without_context(file, bracket, context)?;
                self.emit(
                    self.sources.span(file, bracket.range())?,
                    "E1102",
                    "this value cannot be indexed or sliced",
                    Vec::new(),
                    None,
                )?;
                self.recovery_expression(file, range)
            }
        }
    }

    fn bracket_looks_like_type_arguments(&self, file: FileId, bracket: SyntaxNodeRef<'_>) -> bool {
        let items = bracket
            .child_nodes()
            .filter(|child| child.kind() == SyntaxKind::BracketItem)
            .collect::<Vec<_>>();
        !items.is_empty()
            && items.iter().all(|item| {
                item.descendant_tokens()
                    .find_map(|token| self.resolved.reference(file, token.range()))
                    .is_some_and(|reference| match reference.entity() {
                        ResolvedEntity::ContextualCandidates { .. } => true,
                        ResolvedEntity::Name(ResolvedName::Symbol(symbol)) => {
                            self.resolved.symbol(*symbol).is_some_and(|symbol| {
                                matches!(
                                    symbol.kind(),
                                    SymbolKind::Type
                                        | SymbolKind::Alias
                                        | SymbolKind::Enum
                                        | SymbolKind::Trait
                                )
                            })
                        }
                        ResolvedEntity::Name(ResolvedName::Local(local)) => self
                            .resolved
                            .local(*local)
                            .is_some_and(|local| local.kind() == LocalKind::GenericParameter),
                        ResolvedEntity::Name(
                            ResolvedName::Prelude { namespace, .. }
                            | ResolvedName::External { namespace, .. },
                        ) => *namespace == Namespace::Type,
                        ResolvedEntity::Name(ResolvedName::ContextualSelf) => true,
                        ResolvedEntity::Name(ResolvedName::Receiver)
                        | ResolvedEntity::Module(_) => false,
                    })
            })
    }

    fn check_tuple(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        expected: Option<ExpressionExpectation>,
        context: &mut BodyContext,
    ) -> Result<HirExpressionId, HirError> {
        let contextual = expected.map(ExpressionExpectation::contextual_type);
        let items = node
            .child_nodes()
            .filter(|child| AstExpression::cast(*child).is_some())
            .collect::<Vec<_>>();
        let item_count = items.len();
        let expected_items =
            contextual.and_then(
                |expected| match self.program.interner.kind(expected).ok()? {
                    TypeKind::Tuple(items) if items.len() == item_count => Some(items.clone()),
                    _ => None,
                },
            );
        let mut values = Vec::with_capacity(items.len());
        let mut types = Vec::with_capacity(items.len());
        for (index, item) in items.into_iter().enumerate() {
            let expected = expected_items
                .as_ref()
                .and_then(|items| items.get(index).copied())
                .map(ExpressionExpectation::Direct);
            let value = self.check_expression(file, item, expected, context)?;
            types.push(self.expression_type(value));
            values.push(value);
        }
        if types.iter().any(|ty| *ty == self.program.interner.error()) {
            return self.recovery_expression(file, node.range());
        }
        let ty = self.program.interner.tuple(types)?;
        self.allocate_expression(HirExpression {
            span: self.sources.span(file, node.range())?,
            ty,
            category: HirValueCategory::Value,
            kind: HirExpressionKind::Tuple(values),
        })
    }

    fn check_bracket_literal(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        expected: Option<ExpressionExpectation>,
        context: &mut BodyContext,
    ) -> Result<HirExpressionId, HirError> {
        let is_map = node
            .child_tokens()
            .any(|token| token.kind() == TokenKind::Colon);
        if is_map {
            self.check_map_literal(file, node, expected, context)
        } else {
            self.check_array_literal(file, node, expected, context)
        }
    }

    fn check_array_literal(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        expected: Option<ExpressionExpectation>,
        context: &mut BodyContext,
    ) -> Result<HirExpressionId, HirError> {
        let items = node
            .child_nodes()
            .filter(|child| AstExpression::cast(*child).is_some())
            .collect::<Vec<_>>();
        let contextual = expected
            .map(ExpressionExpectation::contextual_type)
            .map(|ty| self.unique_intrinsic_member(ty, IntrinsicType::Array))
            .transpose()?
            .flatten();
        let contextual_item = contextual
            .map(|ty| self.intrinsic_arguments(ty, IntrinsicType::Array))
            .transpose()?
            .flatten()
            .map(|arguments| arguments[0]);
        if items.is_empty() && contextual_item.is_none() {
            self.emit_collection_context_required(file, node.range(), "array", "Array[T]")?;
            return self.recovery_expression(file, node.range());
        }

        let mut values = Vec::with_capacity(items.len());
        let mut item_type = contextual_item;
        let mut invalid = false;
        for item in items {
            let value = self.check_expression(
                file,
                item,
                item_type.map(ExpressionExpectation::Direct),
                context,
            )?;
            let actual = self.expression_type(value);
            invalid |= actual == self.program.interner.error();
            if item_type.is_none() && actual != self.program.interner.error() {
                item_type = Some(actual);
            }
            values.push(value);
        }
        let Some(item_type) = item_type else {
            return self.recovery_expression(file, node.range());
        };
        if invalid {
            return self.recovery_expression(file, node.range());
        }
        let ty = contextual.unwrap_or(
            self.program
                .interner
                .intrinsic(IntrinsicType::Array, vec![item_type])?,
        );
        self.allocate_expression(HirExpression {
            span: self.sources.span(file, node.range())?,
            ty,
            category: HirValueCategory::Value,
            kind: HirExpressionKind::Array(values),
        })
    }

    fn check_map_literal(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        expected: Option<ExpressionExpectation>,
        context: &mut BodyContext,
    ) -> Result<HirExpressionId, HirError> {
        let items = node
            .child_nodes()
            .filter(|child| AstExpression::cast(*child).is_some())
            .collect::<Vec<_>>();
        let contextual = expected
            .map(ExpressionExpectation::contextual_type)
            .map(|ty| self.unique_intrinsic_member(ty, IntrinsicType::Map))
            .transpose()?
            .flatten();
        let contextual_arguments = contextual
            .map(|ty| self.intrinsic_arguments(ty, IntrinsicType::Map))
            .transpose()?
            .flatten();
        let (mut key_type, mut value_type) = contextual_arguments
            .map(|arguments| (Some(arguments[0]), Some(arguments[1])))
            .unwrap_or((None, None));
        if items.is_empty() && contextual.is_none() {
            self.emit_collection_context_required(file, node.range(), "map", "Map[K, V]")?;
            return self.recovery_expression(file, node.range());
        }

        let mut entries = Vec::with_capacity(items.len() / 2);
        let mut invalid = false;
        for pair in items.chunks_exact(2) {
            let key = self.check_expression(
                file,
                pair[0],
                key_type.map(ExpressionExpectation::Direct),
                context,
            )?;
            let actual_key = self.expression_type(key);
            invalid |= actual_key == self.program.interner.error();
            if key_type.is_none() && actual_key != self.program.interner.error() {
                key_type = Some(actual_key);
            }

            let value = self.check_expression(
                file,
                pair[1],
                value_type.map(ExpressionExpectation::Direct),
                context,
            )?;
            let actual_value = self.expression_type(value);
            invalid |= actual_value == self.program.interner.error();
            if value_type.is_none() && actual_value != self.program.interner.error() {
                value_type = Some(actual_value);
            }
            entries.push(HirMapEntry { key, value });
        }
        if !items.len().is_multiple_of(2) {
            self.complete = false;
            invalid = true;
        }
        let (Some(key_type), Some(value_type)) = (key_type, value_type) else {
            return self.recovery_expression(file, node.range());
        };
        if invalid {
            return self.recovery_expression(file, node.range());
        }
        let ty = contextual.unwrap_or(
            self.program
                .interner
                .intrinsic(IntrinsicType::Map, vec![key_type, value_type])?,
        );
        self.allocate_expression(HirExpression {
            span: self.sources.span(file, node.range())?,
            ty,
            category: HirValueCategory::Value,
            kind: HirExpressionKind::Map(entries),
        })
    }

    fn check_set_literal(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        expected: Option<ExpressionExpectation>,
        context: &mut BodyContext,
    ) -> Result<HirExpressionId, HirError> {
        let items = node
            .child_nodes()
            .filter(|child| AstExpression::cast(*child).is_some())
            .collect::<Vec<_>>();
        let contextual = expected
            .map(ExpressionExpectation::contextual_type)
            .map(|ty| self.unique_intrinsic_member(ty, IntrinsicType::Set))
            .transpose()?
            .flatten();
        let contextual_item = contextual
            .map(|ty| self.intrinsic_arguments(ty, IntrinsicType::Set))
            .transpose()?
            .flatten()
            .map(|arguments| arguments[0]);
        if items.is_empty() && contextual_item.is_none() {
            self.emit_collection_context_required(file, node.range(), "set", "Set[K]")?;
            return self.recovery_expression(file, node.range());
        }

        let mut values = Vec::with_capacity(items.len());
        let mut item_type = contextual_item;
        let mut invalid = false;
        for item in items {
            let value = self.check_expression(
                file,
                item,
                item_type.map(ExpressionExpectation::Direct),
                context,
            )?;
            let actual = self.expression_type(value);
            invalid |= actual == self.program.interner.error();
            if item_type.is_none() && actual != self.program.interner.error() {
                item_type = Some(actual);
            }
            values.push(value);
        }
        let Some(item_type) = item_type else {
            return self.recovery_expression(file, node.range());
        };
        if invalid {
            return self.recovery_expression(file, node.range());
        }
        let ty = contextual.unwrap_or(
            self.program
                .interner
                .intrinsic(IntrinsicType::Set, vec![item_type])?,
        );
        self.allocate_expression(HirExpression {
            span: self.sources.span(file, node.range())?,
            ty,
            category: HirValueCategory::Value,
            kind: HirExpressionKind::Set(values),
        })
    }

    fn unique_intrinsic_member(
        &self,
        expected: TypeId,
        constructor: IntrinsicType,
    ) -> Result<Option<TypeId>, HirError> {
        let matches_constructor = |ty| {
            matches!(
                self.program.interner.kind(ty),
                Ok(TypeKind::Intrinsic {
                    constructor: actual,
                    ..
                }) if *actual == constructor
            )
        };
        if matches_constructor(expected) {
            return Ok(Some(expected));
        }
        let TypeKind::Union(members) = self.program.interner.kind(expected)? else {
            return Ok(None);
        };
        let mut candidates = members
            .iter()
            .copied()
            .filter(|member| matches_constructor(*member));
        let first = candidates.next();
        Ok(first.filter(|_| candidates.next().is_none()))
    }

    fn intrinsic_arguments(
        &self,
        ty: TypeId,
        constructor: IntrinsicType,
    ) -> Result<Option<Vec<TypeId>>, HirError> {
        Ok(match self.program.interner.kind(ty)? {
            TypeKind::Intrinsic {
                constructor: actual,
                arguments,
            } if *actual == constructor => Some(arguments.clone()),
            _ => None,
        })
    }

    fn emit_collection_context_required(
        &mut self,
        file: FileId,
        range: TextRange,
        literal: &str,
        expected: &str,
    ) -> Result<(), HirError> {
        self.emit(
            self.sources.span(file, range)?,
            "E1101",
            format!("empty {literal} literal requires one contextual `{expected}` type"),
            Vec::new(),
            Some((expected.to_owned(), format!("empty {literal} literal"))),
        )
    }

    fn expression_path_info(
        &mut self,
        file: FileId,
        path: SyntaxNodeRef<'_>,
    ) -> Result<Option<PatternPathInfo>, HirError> {
        if path.kind() != SyntaxKind::PathExpr {
            return Ok(None);
        }
        let tokens = path
            .child_tokens()
            .filter(|token| token.kind() == TokenKind::Identifier)
            .collect::<Vec<_>>();
        let Some((resolved_index, mut resolved)) =
            tokens.iter().enumerate().find_map(|(index, token)| {
                let reference = self.resolved.reference(file, token.range())?;
                let resolved = match reference.entity() {
                    ResolvedEntity::Name(name) => Some(name.clone()),
                    ResolvedEntity::ContextualCandidates { type_name, .. } => {
                        Some(type_name.clone())
                    }
                    ResolvedEntity::Module(_) => None,
                }?;
                Some((index, resolved))
            })
        else {
            return Ok(None);
        };
        if let ResolvedName::Symbol(symbol) = resolved
            && self
                .resolved
                .symbol(symbol)
                .is_some_and(|symbol| symbol.kind() == SymbolKind::NewtypeConstructor)
        {
            let constructor = self
                .resolved
                .symbol(symbol)
                .expect("resolved constructor references retain their symbol");
            if let Some(ty) = self.resolved.symbols().find(|candidate| {
                candidate.kind() == SymbolKind::Type
                    && candidate.name() == constructor.name()
                    && candidate.identity().source_id() == constructor.identity().source_id()
                    && candidate.identity().module() == constructor.identity().module()
            }) {
                resolved = ResolvedName::Symbol(ty.id());
            }
        }
        let names_type = match &resolved {
            ResolvedName::Symbol(symbol) => self.resolved.symbol(*symbol).is_some_and(|symbol| {
                matches!(
                    symbol.kind(),
                    SymbolKind::Type | SymbolKind::Alias | SymbolKind::Enum
                )
            }),
            ResolvedName::Prelude { namespace, .. } | ResolvedName::External { namespace, .. } => {
                *namespace == Namespace::Type
            }
            ResolvedName::ContextualSelf => true,
            ResolvedName::Local(_) | ResolvedName::Receiver => false,
        };
        if !names_type {
            return Ok(None);
        }

        let mut suffix = Vec::new();
        for token in tokens.iter().skip(resolved_index + 1) {
            let Some(name) = token.token().normalized_identifier() else {
                continue;
            };
            suffix.push(PatternPathSegment {
                name: Name::new(name).expect("constructor paths contain ordinary identifiers"),
                span: self.sources.span(file, token.range())?,
            });
        }
        let brackets = path
            .child_nodes()
            .filter(|child| child.kind() == SyntaxKind::BracketPostfix)
            .collect::<Vec<_>>();
        if brackets.len() > 1 {
            self.emit(
                self.sources.span(file, path.range())?,
                "E1104",
                "a constructor type path has at most one generic argument list",
                Vec::new(),
                None,
            )?;
            return Ok(None);
        }
        let applied = if let Some(bracket) = brackets.first().copied() {
            let Some(arguments) = self.expression_generic_arguments(file, bracket)? else {
                return Ok(None);
            };
            let Some(applied) =
                self.instantiate_pattern_type(file, path.range(), &resolved, arguments)?
            else {
                self.emit(
                    self.sources.span(file, path.range())?,
                    "E1104",
                    "constructor type arguments do not match the declared arity",
                    Vec::new(),
                    None,
                )?;
                return Ok(None);
            };
            Some(applied)
        } else {
            None
        };
        Ok(Some(PatternPathInfo {
            resolved,
            suffix,
            applied,
        }))
    }

    fn expression_generic_arguments(
        &mut self,
        file: FileId,
        bracket: SyntaxNodeRef<'_>,
    ) -> Result<Option<Vec<TypeId>>, HirError> {
        let mut arguments = Vec::new();
        for item in bracket
            .child_nodes()
            .filter(|child| child.kind() == SyntaxKind::BracketItem)
        {
            let ty = if let Some(type_node) = item
                .child_nodes()
                .find(|child| child.kind() == SyntaxKind::TypeExpr)
            {
                self.program.type_at(file, type_node.range())
            } else if let Some(expression) = item
                .child_nodes()
                .find(|child| AstExpression::cast(*child).is_some())
            {
                self.lower_pattern_type_expression(file, expression)?
            } else {
                None
            };
            let Some(ty) = ty else {
                self.emit(
                    self.sources.span(file, item.range())?,
                    "E1104",
                    "generic constructor arguments must be types",
                    Vec::new(),
                    None,
                )?;
                return Ok(None);
            };
            arguments.push(ty);
        }
        Ok(Some(arguments))
    }

    fn construction_type(
        &mut self,
        file: FileId,
        range: TextRange,
        path: &PatternPathInfo,
        expected: Option<ExpressionExpectation>,
    ) -> Result<Option<TypeId>, HirError> {
        if let Some(applied) = path.applied {
            return Ok(Some(applied));
        }
        if let Some(applied) =
            self.instantiate_pattern_type(file, range, &path.resolved, Vec::new())?
        {
            return Ok(Some(applied));
        }
        let contextual = expected.map(ExpressionExpectation::contextual_type);
        let contextual = contextual
            .map(|expected| {
                self.select_pattern_member_checked(expected, |candidate| {
                    self.pattern_path_matches_type(path, candidate)
                })
            })
            .transpose()?
            .flatten();
        if contextual.is_none() {
            self.emit(
                self.sources.span(file, range)?,
                "E1101",
                "generic constructor requires explicit type arguments or one contextual instance",
                Vec::new(),
                None,
            )?;
        }
        Ok(contextual)
    }

    fn check_unit_variant_path(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        expected: Option<ExpressionExpectation>,
    ) -> Result<Option<HirExpressionId>, HirError> {
        let Some(path) = self.expression_path_info(file, node)? else {
            return Ok(None);
        };
        if path.suffix.is_empty() {
            return Ok(None);
        }
        let Some(ty) = self.construction_type(file, node.range(), &path, expected)? else {
            return Ok(Some(self.recovery_expression(file, node.range())?));
        };
        let Some((_, _, HirNominalShape::Enum { variants })) = self.nominal_instance(ty)? else {
            return Ok(None);
        };
        if path.suffix.len() != 1 {
            self.emit(
                self.sources.span(file, node.range())?,
                "E1102",
                "an enum variant path has exactly one segment after its enum type",
                Vec::new(),
                None,
            )?;
            return Ok(Some(self.recovery_expression(file, node.range())?));
        }
        let segment = &path.suffix[0];
        let Some(variant) = variants.iter().find(|variant| {
            self.resolved
                .member(variant.member())
                .is_some_and(|member| member.name().as_str() == segment.name.as_str())
        }) else {
            self.emit(
                segment.span,
                "E1102",
                "unknown enum variant",
                Vec::new(),
                None,
            )?;
            return Ok(Some(self.recovery_expression(file, node.range())?));
        };
        self.record_member_reference(segment.span, variant.member());
        if !matches!(variant.payload(), HirVariantPayload::Unit) {
            self.emit(
                segment.span,
                "E1102",
                "this enum variant requires its declared payload",
                Vec::new(),
                None,
            )?;
            return Ok(Some(self.recovery_expression(file, node.range())?));
        }
        Ok(Some(self.allocate_expression(HirExpression {
            span: self.sources.span(file, node.range())?,
            ty,
            category: HirValueCategory::Value,
            kind: HirExpressionKind::Variant {
                variant: variant.member(),
                payload: HirVariantValue::Unit,
            },
        })?))
    }

    fn check_record_like(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        expected: Option<ExpressionExpectation>,
        context: &mut BodyContext,
    ) -> Result<HirExpressionId, HirError> {
        let Some(path_node) = node
            .child_nodes()
            .find(|child| child.kind() == SyntaxKind::PathExpr)
        else {
            return self.recovery_expression(file, node.range());
        };
        let Some(path) = self.expression_path_info(file, path_node)? else {
            return self.recovery_expression(file, node.range());
        };
        let Some(ty) = self.construction_type(file, path_node.range(), &path, expected)? else {
            return self.recovery_expression(file, node.range());
        };
        let Some((symbol, arguments, shape)) = self.nominal_instance(ty)? else {
            self.emit(
                self.sources.span(file, path_node.range())?,
                "E1102",
                "record construction requires a nominal record or record-payload variant",
                Vec::new(),
                None,
            )?;
            return self.recovery_expression(file, node.range());
        };
        match shape {
            HirNominalShape::Record { fields } if path.suffix.is_empty() => {
                let (fields, valid) = self.check_record_field_values(
                    file,
                    node,
                    SyntaxKind::RecordInitializer,
                    symbol,
                    &arguments,
                    &fields,
                    true,
                    context,
                )?;
                if !valid {
                    return self.recovery_expression(file, node.range());
                }
                self.allocate_expression(HirExpression {
                    span: self.sources.span(file, node.range())?,
                    ty,
                    category: HirValueCategory::Value,
                    kind: HirExpressionKind::Record {
                        owner: symbol,
                        fields,
                    },
                })
            }
            HirNominalShape::Enum { variants } if path.suffix.len() == 1 => {
                let segment = &path.suffix[0];
                let Some(variant) = variants.iter().find(|variant| {
                    self.resolved
                        .member(variant.member())
                        .is_some_and(|member| member.name().as_str() == segment.name.as_str())
                }) else {
                    self.emit(
                        segment.span,
                        "E1102",
                        "unknown enum variant",
                        Vec::new(),
                        None,
                    )?;
                    return self.recovery_expression(file, node.range());
                };
                self.record_member_reference(segment.span, variant.member());
                let HirVariantPayload::Record(declarations) = variant.payload() else {
                    self.emit(
                        segment.span,
                        "E1102",
                        "this enum variant does not have a record payload",
                        Vec::new(),
                        None,
                    )?;
                    return self.recovery_expression(file, node.range());
                };
                let (fields, valid) = self.check_record_field_values(
                    file,
                    node,
                    SyntaxKind::RecordInitializer,
                    symbol,
                    &arguments,
                    declarations,
                    true,
                    context,
                )?;
                if !valid {
                    return self.recovery_expression(file, node.range());
                }
                self.allocate_expression(HirExpression {
                    span: self.sources.span(file, node.range())?,
                    ty,
                    category: HirValueCategory::Value,
                    kind: HirExpressionKind::Variant {
                        variant: variant.member(),
                        payload: HirVariantValue::Record(fields),
                    },
                })
            }
            HirNominalShape::Record { .. } => {
                self.emit(
                    self.sources.span(file, path_node.range())?,
                    "E1102",
                    "a record constructor cannot have a member suffix",
                    Vec::new(),
                    None,
                )?;
                self.recovery_expression(file, node.range())
            }
            HirNominalShape::Enum { .. } => {
                self.emit(
                    self.sources.span(file, path_node.range())?,
                    "E1102",
                    "enum record construction requires exactly one record-payload variant",
                    Vec::new(),
                    None,
                )?;
                self.recovery_expression(file, node.range())
            }
            HirNominalShape::Newtype { .. } => {
                self.emit(
                    self.sources.span(file, path_node.range())?,
                    "E1102",
                    "newtypes use the `Name(value)` constructor form",
                    Vec::new(),
                    None,
                )?;
                self.recovery_expression(file, node.range())
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn check_record_field_values(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        item_kind: SyntaxKind,
        owner: SymbolId,
        arguments: &[TypeId],
        declarations: &[HirField],
        require_all: bool,
        context: &mut BodyContext,
    ) -> Result<(Vec<HirRecordFieldValue>, bool), HirError> {
        let mut values = Vec::new();
        let mut seen = BTreeSet::new();
        let mut valid = true;
        for initializer in node.child_nodes().filter(|child| child.kind() == item_kind) {
            let Some(name_token) = initializer
                .child_tokens()
                .find(|token| token.kind() == TokenKind::Identifier || token.kind().is_keyword())
            else {
                valid = false;
                continue;
            };
            let spelling = name_token
                .token()
                .normalized_identifier()
                .unwrap_or(self.token_text(file, name_token)?);
            let Ok(name) = MemberName::new(spelling) else {
                valid = false;
                continue;
            };
            let Some(declaration) = declarations.iter().find(|field| {
                self.resolved
                    .member(field.member())
                    .is_some_and(|member| member.name() == &name)
            }) else {
                if let Some(expression) = initializer
                    .child_nodes()
                    .find(|child| AstExpression::cast(*child).is_some())
                {
                    let _ = self.check_expression(file, expression, None, context)?;
                }
                self.emit(
                    self.sources.span(file, name_token.range())?,
                    "E1102",
                    format!("unknown record field `{name}`"),
                    Vec::new(),
                    None,
                )?;
                valid = false;
                continue;
            };
            let member = declaration.member();
            if !seen.insert(member) {
                self.emit(
                    self.sources.span(file, name_token.range())?,
                    "E1102",
                    format!("record field `{name}` is initialized more than once"),
                    Vec::new(),
                    None,
                )?;
                valid = false;
            }
            let visible = self.field_visible_for_construction(
                file,
                owner,
                member,
                self.sources.span(file, name_token.range())?,
            )?;
            if visible {
                self.record_member_reference(self.sources.span(file, name_token.range())?, member);
            } else {
                valid = false;
            }
            let ty = self.instantiate_type(arguments, declaration.ty())?;
            let value = if let Some(expression) = initializer
                .child_nodes()
                .find(|child| AstExpression::cast(*child).is_some())
            {
                self.check_expression(
                    file,
                    expression,
                    Some(ExpressionExpectation::Direct(ty)),
                    context,
                )?
            } else {
                self.check_shorthand_value(file, name_token, ty, context)?
            };
            valid &= self.expression_type(value) != self.program.interner.error();
            if seen.contains(&member)
                && !values
                    .iter()
                    .any(|field: &HirRecordFieldValue| field.member() == member)
            {
                values.push(HirRecordFieldValue { member, value });
            }
        }

        if require_all && seen.len() != declarations.len() {
            let mut hidden_missing = false;
            let mut missing = Vec::new();
            for field in declarations
                .iter()
                .filter(|field| !seen.contains(&field.member()))
            {
                if self.field_is_visible_from(file, owner, field.member())? {
                    if let Some(member) = self.resolved.member(field.member()) {
                        missing.push(member.name().as_str());
                    }
                } else {
                    hidden_missing = true;
                }
            }
            if hidden_missing {
                let owner_span = self
                    .resolved
                    .symbol(owner)
                    .expect("record owners remain indexed")
                    .span();
                self.emit(
                    self.sources.span(file, node.range())?,
                    "E1502",
                    "this record cannot be constructed outside its module because its representation is private",
                    vec![("the record is declared here", owner_span)],
                    None,
                )?;
                valid = false;
            }
            if !missing.is_empty() {
                self.emit(
                    self.sources.span(file, node.range())?,
                    "E1102",
                    format!(
                        "record construction is missing fields: {}",
                        missing.join(", ")
                    ),
                    Vec::new(),
                    None,
                )?;
                valid = false;
            }
        }
        Ok((values, valid))
    }

    fn field_visible_for_construction(
        &mut self,
        file: FileId,
        owner: SymbolId,
        member: MemberId,
        use_span: Span,
    ) -> Result<bool, HirError> {
        if self.field_is_visible_from(file, owner, member)? {
            return Ok(true);
        }
        let member = self
            .resolved
            .member(member)
            .expect("HIR fields retain valid resolved members");
        self.emit(
            use_span,
            "E1502",
            "record construction or update cannot set a private field from another module",
            vec![("the private field is declared here", member.span())],
            None,
        )?;
        Ok(false)
    }

    fn field_is_visible_from(
        &self,
        file: FileId,
        owner: SymbolId,
        member: MemberId,
    ) -> Result<bool, HirError> {
        let member = self
            .resolved
            .member(member)
            .expect("HIR fields retain valid resolved members");
        if member.visibility() == Visibility::Public {
            return Ok(true);
        }
        let owner = self
            .resolved
            .symbol(owner)
            .expect("nominal HIR declarations retain valid symbols");
        let source = self.sources.get(file)?;
        Ok(owner.identity().source_id() == source.source_id()
            && owner.identity().module() == source.module())
    }

    fn check_shorthand_value(
        &mut self,
        file: FileId,
        token: SyntaxTokenRef<'_>,
        expected: TypeId,
        context: &BodyContext,
    ) -> Result<HirExpressionId, HirError> {
        let name = self.token_text(file, token)?.to_owned();
        let resolved = self
            .resolved
            .reference(file, token.range())
            .and_then(|reference| match reference.entity() {
                ResolvedEntity::Name(name) => Some(name.clone()),
                ResolvedEntity::ContextualCandidates { value_name, .. } => Some(value_name.clone()),
                ResolvedEntity::Module(_) => None,
            });
        let Some(resolved) = resolved else {
            self.complete = false;
            return self.recovery_expression(file, token.range());
        };
        let (ty, category, kind) = match resolved {
            ResolvedName::Local(local) => {
                let Some(ty) = context.locals.get(&local).copied() else {
                    self.complete = false;
                    return self.recovery_expression(file, token.range());
                };
                (ty, HirValueCategory::Place, HirExpressionKind::Local(local))
            }
            ResolvedName::Symbol(symbol) => {
                let symbol_info = self
                    .resolved
                    .symbol(symbol)
                    .expect("resolved shorthand symbols remain indexed");
                match symbol_info.kind() {
                    SymbolKind::Constant => (
                        self.program
                            .constant(symbol)
                            .and_then(|constant| constant.ty())
                            .unwrap_or_else(|| self.program.interner.error()),
                        HirValueCategory::Value,
                        HirExpressionKind::Constant(symbol),
                    ),
                    SymbolKind::Function => {
                        let id = HirCallableId::Symbol(symbol);
                        let Some(callable) = self.callable(id) else {
                            self.complete = false;
                            return self.recovery_expression(file, token.range());
                        };
                        (
                            callable.function_type,
                            HirValueCategory::Value,
                            HirExpressionKind::Function(id),
                        )
                    }
                    _ => {
                        self.emit(
                            self.sources.span(file, token.range())?,
                            "E1102",
                            format!("`{name}` is not a value usable by record shorthand"),
                            Vec::new(),
                            None,
                        )?;
                        return self.recovery_expression(file, token.range());
                    }
                }
            }
            ResolvedName::Receiver => {
                let Some(ty) = context.receiver else {
                    self.complete = false;
                    return self.recovery_expression(file, token.range());
                };
                (ty, HirValueCategory::Place, HirExpressionKind::Receiver)
            }
            ResolvedName::Prelude { .. }
            | ResolvedName::External { .. }
            | ResolvedName::ContextualSelf => {
                self.complete = false;
                return self.recovery_expression(file, token.range());
            }
        };
        let value = self.allocate_expression(HirExpression {
            span: self.sources.span(file, token.range())?,
            ty,
            category,
            kind,
        })?;
        if ty == self.program.interner.error() {
            return Ok(value);
        }
        let Some(assignability) = self.program.interner.assignability(ty, expected)? else {
            let expected_name = self.program.interner.canonical(expected)?;
            let actual_name = self.program.interner.canonical(ty)?;
            self.emit(
                self.sources.span(file, token.range())?,
                "E1102",
                format!(
                    "record shorthand `{name}` expected `{expected_name}`, found `{actual_name}`"
                ),
                Vec::new(),
                Some((expected_name, actual_name)),
            )?;
            return self.recovery_expression(file, token.range());
        };
        if assignability == Assignability::Exact {
            Ok(value)
        } else {
            self.coerce_existing(value, expected)
        }
    }

    fn check_block(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        expected: Option<ExpressionExpectation>,
        context: &mut BodyContext,
    ) -> Result<HirExpressionId, HirError> {
        let mut statements = Vec::new();
        let mut tail = None;
        let mut reachable = true;
        for item in node.child_nodes() {
            let mut checked_statement = None;
            match item.kind() {
                SyntaxKind::BindingDecl => {
                    checked_statement = self.check_binding(file, item, context)?;
                }
                SyntaxKind::ExpressionStmt => {
                    let Some(expression) = item
                        .child_nodes()
                        .find(|child| AstExpression::cast(*child).is_some())
                    else {
                        continue;
                    };
                    let value = self.check_expression(file, expression, None, context)?;
                    let ty = self.expression_type(value);
                    if self.expression_flow(value).may_complete()
                        && ty != self.program.interner.error()
                        && ty != self.program.interner.scalar(ScalarType::Unit)
                    {
                        self.emit(
                            self.sources.span(file, expression.range())?,
                            "E1303",
                            "a non-`Unit` expression result cannot be discarded implicitly",
                            Vec::new(),
                            None,
                        )?;
                    }
                    checked_statement = Some(HirStatement::Expression {
                        span: self.sources.span(file, item.range())?,
                        value,
                    });
                }
                SyntaxKind::TailExpression => {
                    let Some(expression) = item
                        .child_nodes()
                        .find(|child| AstExpression::cast(*child).is_some())
                    else {
                        continue;
                    };
                    let value = self.check_expression(file, expression, expected, context)?;
                    if reachable && !self.expression_flow(value).may_complete() {
                        reachable = false;
                    }
                    tail = Some(value);
                }
                SyntaxKind::ReturnStmt | SyntaxKind::BreakStmt | SyntaxKind::ContinueStmt => {
                    let value = self.check_control_transfer(file, item, context)?;
                    checked_statement = Some(HirStatement::Expression {
                        span: self.sources.span(file, item.range())?,
                        value,
                    });
                }
                SyntaxKind::FailStmt => {
                    let value = self.check_fail(file, item, context)?;
                    checked_statement = Some(HirStatement::Expression {
                        span: self.sources.span(file, item.range())?,
                        value,
                    });
                }
                SyntaxKind::ForStmt => {
                    checked_statement = Some(self.check_for(file, item, context)?);
                }
                SyntaxKind::Assignment => {
                    checked_statement = Some(self.check_assignment(file, item, context)?);
                }
                SyntaxKind::DeferStmt => {
                    self.complete = false;
                }
                _ => {}
            }
            if let Some(statement) = checked_statement {
                if reachable && !self.statement_summary(&statement).flow.may_complete() {
                    reachable = false;
                }
                statements.push(statement);
            }
        }
        let ty = if reachable {
            tail.map(|tail| self.expression_type(tail))
                .unwrap_or_else(|| self.program.interner.scalar(ScalarType::Unit))
        } else {
            self.program.interner.scalar(ScalarType::Never)
        };
        self.allocate_expression(HirExpression {
            span: self.sources.span(file, node.range())?,
            ty,
            category: HirValueCategory::Value,
            kind: HirExpressionKind::Block { statements, tail },
        })
    }

    fn check_assignment(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        context: &mut BodyContext,
    ) -> Result<HirStatement, HirError> {
        let operator_token = node
            .child_tokens()
            .find(|token| assignment_operator(token.kind()).is_some())
            .expect("parsed assignments contain an assignment operator");
        let operator = assignment_operator(operator_token.kind())
            .expect("the selected token is an assignment operator");
        let target_node = node
            .child_nodes()
            .find(|child| {
                matches!(
                    child.kind(),
                    SyntaxKind::Lvalue
                        | SyntaxKind::TupleAssignmentPattern
                        | SyntaxKind::WildcardPattern
                )
            })
            .expect("parsed assignments contain a target");
        let value_node = node
            .child_nodes()
            .find(|child| AstExpression::cast(*child).is_some())
            .expect("parsed assignments contain a value");

        let checked_target = self.check_assignment_target(file, target_node, context)?;
        self.check_duplicate_assignment_destinations(&checked_target)?;

        if operator != HirAssignmentOperator::Assign
            && !matches!(checked_target.kind, CheckedAssignmentTargetKind::Place(_))
        {
            self.emit(
                self.sources.span(file, target_node.range())?,
                "E1411",
                "compound assignment requires one writable place",
                Vec::new(),
                None,
            )?;
        }

        let value = if operator == HirAssignmentOperator::Assign {
            self.check_assignment_rhs(file, value_node, &checked_target, context)?
        } else {
            self.check_compound_assignment_rhs(
                file,
                value_node,
                operator,
                &checked_target,
                context,
            )?
        };
        let value_type = self.expression_type(value);
        let target = if operator == HirAssignmentOperator::Assign {
            self.finalize_assignment_target(&checked_target, value_type)?
        } else {
            self.finalize_compound_assignment_target(
                file,
                operator_token.range(),
                operator,
                &checked_target,
                value_type,
            )?
        };
        let span = self.sources.span(file, node.range())?;
        if operator == HirAssignmentOperator::Assign
            && matches!(target.kind(), HirAssignmentTargetKind::Discard)
        {
            return Ok(HirStatement::Discard { span, value });
        }
        Ok(HirStatement::Assignment {
            span,
            operator,
            target,
            value,
        })
    }

    fn check_assignment_target(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        context: &mut BodyContext,
    ) -> Result<CheckedAssignmentTarget, HirError> {
        let span = self.sources.span(file, node.range())?;
        match node.kind() {
            SyntaxKind::Lvalue => {
                let place = self.check_lvalue(file, node, context)?;
                if place.permission == PlacePermission::Immutable {
                    self.emit(
                        span,
                        "E1411",
                        "assignment requires a `var`, `mut`, or `var`-borrowed destination",
                        Vec::new(),
                        None,
                    )?;
                } else if place.map_entry
                    && place.permission != PlacePermission::Replace
                    && place.permission != PlacePermission::Invalid
                {
                    self.emit(
                        span,
                        "E1411",
                        "map index assignment can insert a key and therefore requires `var` access",
                        Vec::new(),
                        None,
                    )?;
                }
                Ok(CheckedAssignmentTarget {
                    span,
                    kind: CheckedAssignmentTargetKind::Place(place),
                })
            }
            SyntaxKind::WildcardPattern => Ok(CheckedAssignmentTarget {
                span,
                kind: CheckedAssignmentTargetKind::Discard,
            }),
            SyntaxKind::TupleAssignmentPattern => {
                let mut items = Vec::new();
                for child in node.child_nodes().filter(|child| {
                    matches!(
                        child.kind(),
                        SyntaxKind::Lvalue
                            | SyntaxKind::TupleAssignmentPattern
                            | SyntaxKind::WildcardPattern
                    )
                }) {
                    items.push(self.check_assignment_target(file, child, context)?);
                }
                Ok(CheckedAssignmentTarget {
                    span,
                    kind: CheckedAssignmentTargetKind::Tuple(items),
                })
            }
            _ => unreachable!("assignment target grammar is closed"),
        }
    }

    fn check_assignment_rhs(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        target: &CheckedAssignmentTarget,
        context: &mut BodyContext,
    ) -> Result<HirExpressionId, HirError> {
        match &target.kind {
            CheckedAssignmentTargetKind::Place(place) => self.check_expression(
                file,
                node,
                (place.ty != self.program.interner.error())
                    .then_some(ExpressionExpectation::Direct(place.ty)),
                context,
            ),
            CheckedAssignmentTargetKind::Discard => {
                self.check_expression(file, node, None, context)
            }
            CheckedAssignmentTargetKind::Tuple(targets) if node.kind() == SyntaxKind::TupleExpr => {
                let values = node
                    .child_nodes()
                    .filter(|child| AstExpression::cast(*child).is_some())
                    .collect::<Vec<_>>();
                let mut expressions = Vec::with_capacity(values.len());
                let mut types = Vec::with_capacity(values.len());
                for (index, value_node) in values.into_iter().enumerate() {
                    let value = if let Some(target) = targets.get(index) {
                        self.check_assignment_rhs(file, value_node, target, context)?
                    } else {
                        self.check_expression(file, value_node, None, context)?
                    };
                    types.push(self.expression_type(value));
                    expressions.push(value);
                }
                if types.iter().any(|ty| *ty == self.program.interner.error()) {
                    return self.recovery_expression(file, node.range());
                }
                let ty = self.program.interner.tuple(types)?;
                self.allocate_expression(HirExpression {
                    span: self.sources.span(file, node.range())?,
                    ty,
                    category: HirValueCategory::Value,
                    kind: HirExpressionKind::Tuple(expressions),
                })
            }
            CheckedAssignmentTargetKind::Tuple(_) => {
                self.check_expression(file, node, None, context)
            }
        }
    }

    fn check_compound_assignment_rhs(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        operator: HirAssignmentOperator,
        target: &CheckedAssignmentTarget,
        context: &mut BodyContext,
    ) -> Result<HirExpressionId, HirError> {
        let CheckedAssignmentTargetKind::Place(place) = &target.kind else {
            return self.check_expression(file, node, None, context);
        };
        if place.ty == self.program.interner.error() {
            return self.check_expression(file, node, None, context);
        }
        if place.map_entry {
            self.emit(
                target.span,
                "E1411",
                "compound assignment is not defined for a map index; use an explicit lookup policy",
                Vec::new(),
                None,
            )?;
        }
        let binary = operator
            .binary_operator()
            .expect("compound assignment operators have a binary counterpart");
        let scalar_target = matches!(self.program.interner.kind(place.ty)?, TypeKind::Scalar(_));
        let expected = (scalar_target
            && !matches!(
                binary,
                HirBinaryOperator::ShiftLeft | HirBinaryOperator::ShiftRight
            ))
        .then_some(ExpressionExpectation::Direct(place.ty));
        self.check_expression(file, node, expected, context)
    }

    fn finalize_assignment_target(
        &mut self,
        target: &CheckedAssignmentTarget,
        actual: TypeId,
    ) -> Result<HirAssignmentTarget, HirError> {
        match &target.kind {
            CheckedAssignmentTargetKind::Place(place) => {
                let coercion = self.assignment_coercion(target.span, actual, place.ty)?;
                Ok(HirAssignmentTarget {
                    span: target.span,
                    ty: place.ty,
                    kind: HirAssignmentTargetKind::Place {
                        place: place.expression,
                        coercion,
                        write: assignment_write_kind(place),
                    },
                })
            }
            CheckedAssignmentTargetKind::Discard => {
                self.require_discard(target.span, actual)?;
                Ok(HirAssignmentTarget {
                    span: target.span,
                    ty: actual,
                    kind: HirAssignmentTargetKind::Discard,
                })
            }
            CheckedAssignmentTargetKind::Tuple(targets) => {
                let actual_items = match self.program.interner.kind(actual)?.clone() {
                    TypeKind::Tuple(items) if items.len() == targets.len() => items,
                    TypeKind::Error => vec![self.program.interner.error(); targets.len()],
                    _ => {
                        self.emit(
                            target.span,
                            "E1102",
                            format!(
                                "multiple assignment expects a {}-item tuple, found `{}`",
                                targets.len(),
                                self.program.interner.canonical(actual)?
                            ),
                            Vec::new(),
                            None,
                        )?;
                        vec![self.program.interner.error(); targets.len()]
                    }
                };
                let mut items = Vec::with_capacity(targets.len());
                let mut types = Vec::with_capacity(targets.len());
                for (target, actual) in targets.iter().zip(actual_items) {
                    let item = self.finalize_assignment_target(target, actual)?;
                    types.push(item.ty);
                    items.push(item);
                }
                let ty = if types.iter().any(|ty| *ty == self.program.interner.error()) {
                    self.program.interner.error()
                } else {
                    self.program.interner.tuple(types)?
                };
                Ok(HirAssignmentTarget {
                    span: target.span,
                    ty,
                    kind: HirAssignmentTargetKind::Tuple(items),
                })
            }
        }
    }

    fn finalize_compound_assignment_target(
        &mut self,
        file: FileId,
        operator_range: TextRange,
        operator: HirAssignmentOperator,
        target: &CheckedAssignmentTarget,
        right: TypeId,
    ) -> Result<HirAssignmentTarget, HirError> {
        let CheckedAssignmentTargetKind::Place(place) = &target.kind else {
            return self.finalize_assignment_target(target, self.program.interner.error());
        };
        if place.map_entry || place.permission == PlacePermission::Invalid {
            return Ok(HirAssignmentTarget {
                span: target.span,
                ty: place.ty,
                kind: HirAssignmentTargetKind::Place {
                    place: place.expression,
                    coercion: Assignability::Exact,
                    write: assignment_write_kind(place),
                },
            });
        }
        let binary = operator
            .binary_operator()
            .expect("compound assignment operators have a binary counterpart");
        let result = self.binary_result(binary, place.ty, right)?;
        let Some(result) = result else {
            self.emit_invalid_operator(file, operator_range, place.ty, Some(right))?;
            return Ok(HirAssignmentTarget {
                span: target.span,
                ty: place.ty,
                kind: HirAssignmentTargetKind::Place {
                    place: place.expression,
                    coercion: Assignability::Exact,
                    write: assignment_write_kind(place),
                },
            });
        };
        let coercion = self.assignment_coercion(target.span, result, place.ty)?;
        Ok(HirAssignmentTarget {
            span: target.span,
            ty: place.ty,
            kind: HirAssignmentTargetKind::Place {
                place: place.expression,
                coercion,
                write: assignment_write_kind(place),
            },
        })
    }

    fn assignment_coercion(
        &mut self,
        span: Span,
        actual: TypeId,
        expected: TypeId,
    ) -> Result<Assignability, HirError> {
        if actual == self.program.interner.error() || expected == self.program.interner.error() {
            return Ok(Assignability::Exact);
        }
        let Some(coercion) = self.program.interner.assignability(actual, expected)? else {
            let expected_name = self.program.interner.canonical(expected)?;
            let actual_name = self.program.interner.canonical(actual)?;
            self.emit(
                span,
                "E1102",
                format!("assignment expected `{expected_name}`, found `{actual_name}`"),
                Vec::new(),
                Some((expected_name, actual_name)),
            )?;
            return Ok(Assignability::Exact);
        };
        Ok(coercion)
    }

    fn check_duplicate_assignment_destinations(
        &mut self,
        target: &CheckedAssignmentTarget,
    ) -> Result<(), HirError> {
        let mut places = Vec::new();
        collect_assignment_places(target, &mut places);
        for right in 0..places.len() {
            for left in 0..right {
                if static_places_overlap(places[left].0, places[right].0) {
                    self.emit(
                        places[right].1,
                        "E1405",
                        "this assignment destination inevitably overlaps an earlier destination",
                        vec![("the earlier destination is here", places[left].1)],
                        None,
                    )?;
                }
            }
        }
        Ok(())
    }

    fn check_lvalue(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        context: &mut BodyContext,
    ) -> Result<CheckedPlace, HirError> {
        let root = node
            .child_tokens()
            .find(|token| matches!(token.kind(), TokenKind::Identifier | TokenKind::SelfKw))
            .expect("parsed lvalues contain a root");
        let reference = self.resolved.reference(file, root.range());
        let resolved = reference.and_then(|reference| match reference.entity() {
            ResolvedEntity::Name(name) => Some(name),
            _ => None,
        });
        let (ty, permission, key_root, kind) = match resolved {
            Some(ResolvedName::Local(local)) => {
                let Some(ty) = context.locals.get(local).copied() else {
                    self.complete = false;
                    return self.recovery_place(file, node.range());
                };
                (
                    ty,
                    context
                        .local_permissions
                        .get(local)
                        .copied()
                        .unwrap_or(PlacePermission::Immutable),
                    StaticPlaceRoot::Local(*local),
                    HirExpressionKind::Local(*local),
                )
            }
            Some(ResolvedName::Receiver) => {
                let Some(ty) = context.receiver else {
                    self.complete = false;
                    return self.recovery_place(file, node.range());
                };
                (
                    ty,
                    context.receiver_permission,
                    StaticPlaceRoot::Receiver,
                    HirExpressionKind::Receiver,
                )
            }
            Some(ResolvedName::Symbol(symbol)) => {
                let declaration = self
                    .resolved
                    .symbol(*symbol)
                    .expect("resolved references contain valid symbols");
                match declaration.kind() {
                    SymbolKind::Constant => {
                        let ty = self
                            .program
                            .constant(*symbol)
                            .and_then(|constant| constant.ty)
                            .unwrap_or_else(|| self.program.interner.error());
                        (
                            ty,
                            PlacePermission::Immutable,
                            StaticPlaceRoot::Symbol(*symbol),
                            HirExpressionKind::Constant(*symbol),
                        )
                    }
                    _ => {
                        self.emit_invalid_assignment_target(
                            self.sources.span(file, root.range())?,
                            "a declaration name is not a writable place",
                        )?;
                        return self.recovery_place(file, node.range());
                    }
                }
            }
            Some(ResolvedName::Prelude { .. }) => {
                self.emit_invalid_assignment_target(
                    self.sources.span(file, root.range())?,
                    "a prelude declaration is not a writable place",
                )?;
                return self.recovery_place(file, node.range());
            }
            _ => {
                self.complete = false;
                return self.recovery_place(file, node.range());
            }
        };
        let expression = self.allocate_expression(HirExpression {
            span: self.sources.span(file, root.range())?,
            ty,
            category: HirValueCategory::Place,
            kind,
        })?;
        let mut place = CheckedPlace {
            expression,
            ty,
            permission,
            key: StaticPlace {
                root: key_root,
                projections: Vec::new(),
            },
            map_entry: false,
            slice: false,
        };

        let mut after_root = false;
        let mut expects_member = false;
        for element in node.elements() {
            match *element {
                SyntaxElement::Token(id) => {
                    let token = node.cst().token_ref(id);
                    if token.range() == root.range() {
                        after_root = true;
                        continue;
                    }
                    if !after_root || token.kind().is_trivia() {
                        continue;
                    }
                    if token.kind() == TokenKind::Dot {
                        expects_member = true;
                        continue;
                    }
                    if expects_member {
                        place = self.project_field_place(file, node.range(), place, token)?;
                        expects_member = false;
                    }
                }
                SyntaxElement::Node(id) if after_root => {
                    let child = node.cst().node_ref(id);
                    if child.kind() == SyntaxKind::BracketPostfix {
                        place =
                            self.project_bracket_place(file, node.range(), place, child, context)?;
                    }
                }
                SyntaxElement::Node(_) => {}
            }
        }
        Ok(place)
    }

    fn recovery_place(&mut self, file: FileId, range: TextRange) -> Result<CheckedPlace, HirError> {
        let expression = self.allocate_expression(HirExpression {
            span: self.sources.span(file, range)?,
            ty: self.program.interner.error(),
            category: HirValueCategory::Place,
            kind: HirExpressionKind::Recovery,
        })?;
        Ok(CheckedPlace {
            expression,
            ty: self.program.interner.error(),
            permission: PlacePermission::Invalid,
            key: StaticPlace {
                root: StaticPlaceRoot::Receiver,
                projections: vec![StaticPlaceProjection::DynamicIndex(range)],
            },
            map_entry: false,
            slice: false,
        })
    }

    fn project_field_place(
        &mut self,
        file: FileId,
        range: TextRange,
        mut place: CheckedPlace,
        token: SyntaxTokenRef<'_>,
    ) -> Result<CheckedPlace, HirError> {
        if place.map_entry {
            self.emit_invalid_assignment_target(
                self.sources.span(file, token.range())?,
                "a potentially absent map entry cannot be projected as a place",
            )?;
            return self.recovery_place(file, range);
        }
        if token.kind() == TokenKind::IntegerLiteral {
            let spelling = self.token_text(file, token)?;
            let Ok(index) = spelling.replace('_', "").parse::<u32>() else {
                self.emit_invalid_assignment_target(
                    self.sources.span(file, token.range())?,
                    "tuple slot is not representable as an index",
                )?;
                return self.recovery_place(file, range);
            };
            let TypeKind::Tuple(items) = self.program.interner.kind(place.ty)? else {
                self.emit_invalid_assignment_target(
                    self.sources.span(file, token.range())?,
                    "numeric member access requires a tuple place",
                )?;
                return self.recovery_place(file, range);
            };
            let Some(ty) = items.get(index as usize).copied() else {
                self.emit_invalid_assignment_target(
                    self.sources.span(file, token.range())?,
                    "tuple slot is outside this tuple type",
                )?;
                return self.recovery_place(file, range);
            };
            place.expression = self.allocate_expression(HirExpression {
                span: self.sources.span(file, range)?,
                ty,
                category: HirValueCategory::Place,
                kind: HirExpressionKind::TupleField {
                    base: place.expression,
                    index,
                },
            })?;
            place.ty = ty;
            place.permission = place.permission.projected();
            place
                .key
                .projections
                .push(StaticPlaceProjection::TupleField(index));
            place.map_entry = false;
            place.slice = false;
            return Ok(place);
        }

        let Some((member, ty)) = self.resolve_field(file, place.ty, token, "E1411")? else {
            return self.recovery_place(file, range);
        };
        place.expression = self.allocate_expression(HirExpression {
            span: self.sources.span(file, range)?,
            ty,
            category: HirValueCategory::Place,
            kind: HirExpressionKind::Field {
                base: place.expression,
                member,
            },
        })?;
        place.ty = ty;
        place.permission = place.permission.projected();
        place
            .key
            .projections
            .push(StaticPlaceProjection::Field(member));
        place.map_entry = false;
        place.slice = false;
        Ok(place)
    }

    fn project_bracket_place(
        &mut self,
        file: FileId,
        range: TextRange,
        mut place: CheckedPlace,
        bracket: SyntaxNodeRef<'_>,
        context: &mut BodyContext,
    ) -> Result<CheckedPlace, HirError> {
        if place.map_entry {
            let _ = self.check_bracket_expressions_without_context(file, bracket, context)?;
            self.emit_invalid_assignment_target(
                self.sources.span(file, bracket.range())?,
                "a potentially absent map entry cannot be projected as a place",
            )?;
            return self.recovery_place(file, range);
        }
        let kind = self.program.interner.kind(place.ty)?.clone();
        match kind {
            TypeKind::Intrinsic {
                constructor: IntrinsicType::Array,
                arguments,
            } => {
                let element = arguments[0];
                if bracket
                    .child_nodes()
                    .any(|child| child.kind() == SyntaxKind::SliceSpec)
                {
                    let (start, end, step) = self.check_slice_operands(file, bracket, context)?;
                    place.expression = self.allocate_expression(HirExpression {
                        span: self.sources.span(file, range)?,
                        ty: place.ty,
                        category: HirValueCategory::Place,
                        kind: HirExpressionKind::Slice {
                            base: place.expression,
                            start,
                            end,
                            step,
                        },
                    })?;
                    let projection =
                        self.static_slice_projection(bracket.range(), start, end, step);
                    place.key.projections.push(projection);
                    place.slice = true;
                    place.map_entry = false;
                } else {
                    let Some(index_node) = single_bracket_expression(bracket) else {
                        self.emit_invalid_assignment_target(
                            self.sources.span(file, bracket.range())?,
                            "array assignment requires exactly one index",
                        )?;
                        return self.recovery_place(file, range);
                    };
                    let index = self.check_expression(
                        file,
                        index_node,
                        Some(ExpressionExpectation::Direct(
                            self.program.interner.scalar(ScalarType::Int),
                        )),
                        context,
                    )?;
                    place.expression = self.allocate_expression(HirExpression {
                        span: self.sources.span(file, range)?,
                        ty: element,
                        category: HirValueCategory::Place,
                        kind: HirExpressionKind::Index {
                            base: place.expression,
                            index,
                            access: HirIndexAccess::Array,
                        },
                    })?;
                    place.ty = element;
                    place.permission = place.permission.projected();
                    place.key.projections.push(
                        self.static_place_operand(index)
                            .map(StaticPlaceProjection::Index)
                            .unwrap_or(StaticPlaceProjection::DynamicIndex(index_node.range())),
                    );
                    place.slice = false;
                    place.map_entry = false;
                }
            }
            TypeKind::Intrinsic {
                constructor: IntrinsicType::Map,
                arguments,
            } => {
                if bracket
                    .child_nodes()
                    .any(|child| child.kind() == SyntaxKind::SliceSpec)
                {
                    let _ = self.check_slice_operands(file, bracket, context)?;
                    self.emit_invalid_assignment_target(
                        self.sources.span(file, bracket.range())?,
                        "maps do not support slice assignment",
                    )?;
                    return self.recovery_place(file, range);
                }
                let Some(key_node) = single_bracket_expression(bracket) else {
                    self.emit_invalid_assignment_target(
                        self.sources.span(file, bracket.range())?,
                        "map assignment requires exactly one key",
                    )?;
                    return self.recovery_place(file, range);
                };
                let key = self.check_expression(
                    file,
                    key_node,
                    Some(ExpressionExpectation::Direct(arguments[0])),
                    context,
                )?;
                place.expression = self.allocate_expression(HirExpression {
                    span: self.sources.span(file, range)?,
                    ty: arguments[1],
                    category: HirValueCategory::Place,
                    kind: HirExpressionKind::Index {
                        base: place.expression,
                        index: key,
                        access: HirIndexAccess::MapEntry,
                    },
                })?;
                place.ty = arguments[1];
                place.key.projections.push(
                    self.static_place_operand(key)
                        .map(StaticPlaceProjection::Index)
                        .unwrap_or(StaticPlaceProjection::DynamicIndex(key_node.range())),
                );
                place.map_entry = true;
                place.slice = false;
            }
            TypeKind::Error => return self.recovery_place(file, range),
            _ => {
                let _ = self.check_bracket_expressions_without_context(file, bracket, context)?;
                self.emit_invalid_assignment_target(
                    self.sources.span(file, bracket.range())?,
                    "this type does not provide an assignable index or slice",
                )?;
                return self.recovery_place(file, range);
            }
        }
        Ok(place)
    }

    fn check_slice_operands(
        &mut self,
        file: FileId,
        bracket: SyntaxNodeRef<'_>,
        context: &mut BodyContext,
    ) -> Result<HirSliceOperands, HirError> {
        let int = self.program.interner.scalar(ScalarType::Int);
        let start_node = bracket
            .child_nodes()
            .find(|child| child.kind() == SyntaxKind::BracketItem)
            .and_then(direct_expression_child);
        let slice = bracket
            .child_nodes()
            .find(|child| child.kind() == SyntaxKind::SliceSpec)
            .expect("slice brackets contain a slice specification");
        let mut colons = 0_u8;
        let mut end_node = None;
        let mut step_node = None;
        for element in slice.elements() {
            match *element {
                SyntaxElement::Token(id)
                    if slice.cst().token_ref(id).kind() == TokenKind::Colon =>
                {
                    colons = colons.saturating_add(1);
                }
                SyntaxElement::Node(id) => {
                    let expression = slice.cst().node_ref(id);
                    if AstExpression::cast(expression).is_some() {
                        if colons == 1 {
                            end_node = Some(expression);
                        } else if colons == 2 {
                            step_node = Some(expression);
                        }
                    }
                }
                SyntaxElement::Token(_) => {}
            }
        }
        let mut check = |node: Option<SyntaxNodeRef<'_>>| {
            node.map(|node| {
                self.check_expression(
                    file,
                    node,
                    Some(ExpressionExpectation::Direct(int)),
                    context,
                )
            })
            .transpose()
        };
        let start = check(start_node)?;
        let end = check(end_node)?;
        let step = check(step_node)?;
        Ok((start, end, step))
    }

    fn check_bracket_expressions_without_context(
        &mut self,
        file: FileId,
        bracket: SyntaxNodeRef<'_>,
        context: &mut BodyContext,
    ) -> Result<Vec<HirExpressionId>, HirError> {
        let mut values = Vec::new();
        let mut pending = bracket.child_nodes().collect::<Vec<_>>();
        pending.reverse();
        while let Some(node) = pending.pop() {
            if AstExpression::cast(node).is_some() {
                values.push(self.check_expression(file, node, None, context)?);
            } else {
                let mut children = node.child_nodes().collect::<Vec<_>>();
                children.reverse();
                pending.extend(children);
            }
        }
        Ok(values)
    }

    fn static_slice_projection(
        &self,
        range: TextRange,
        start: Option<HirExpressionId>,
        end: Option<HirExpressionId>,
        step: Option<HirExpressionId>,
    ) -> StaticPlaceProjection {
        let start = start.map(|value| self.static_place_operand(value));
        let end = end.map(|value| self.static_place_operand(value));
        let step = step.map(|value| self.static_place_operand(value));
        if start.as_ref().is_some_and(Option::is_none)
            || end.as_ref().is_some_and(Option::is_none)
            || step.as_ref().is_some_and(Option::is_none)
        {
            StaticPlaceProjection::DynamicSlice(range)
        } else {
            StaticPlaceProjection::Slice {
                start: start.flatten(),
                end: end.flatten(),
                step: step.flatten(),
            }
        }
    }

    fn static_place_operand(&self, expression: HirExpressionId) -> Option<StaticPlaceOperand> {
        let expression = self.program.expression(expression)?;
        match expression.kind() {
            HirExpressionKind::Local(local) => Some(StaticPlaceOperand::Local(*local)),
            HirExpressionKind::Constant(symbol) => Some(StaticPlaceOperand::Constant(*symbol)),
            HirExpressionKind::Literal(literal) => {
                let scalar = match self.program.interner.kind(expression.ty()).ok()? {
                    TypeKind::Scalar(scalar) => Some(*scalar),
                    _ => None,
                };
                normalize_static_literal(literal, expression.ty(), scalar).map(|value| {
                    StaticPlaceOperand::Literal {
                        ty: expression.ty(),
                        value,
                    }
                })
            }
            HirExpressionKind::Tuple(items) => items
                .iter()
                .map(|item| self.static_place_operand(*item))
                .collect::<Option<Vec<_>>>()
                .map(StaticPlaceOperand::Tuple),
            HirExpressionKind::Prefix {
                operator: HirPrefixOperator::Negate,
                operand,
            } => match self.program.expression(*operand)?.kind() {
                HirExpressionKind::Literal(HirLiteral::Integer(value)) => integer_magnitude(value)
                    .map(|value| StaticPlaceOperand::Literal {
                        ty: expression.ty(),
                        value: format!("-{value}"),
                    }),
                HirExpressionKind::Literal(HirLiteral::Float(value)) => {
                    let TypeKind::Scalar(scalar) =
                        self.program.interner.kind(expression.ty()).ok()?
                    else {
                        return None;
                    };
                    normalize_float_pattern(value, true, *scalar).map(|value| {
                        StaticPlaceOperand::Literal {
                            ty: expression.ty(),
                            value,
                        }
                    })
                }
                _ => None,
            },
            HirExpressionKind::Coerce { value, .. } => self.static_place_operand(*value),
            _ => None,
        }
    }

    fn resolve_field(
        &mut self,
        file: FileId,
        base: TypeId,
        token: SyntaxTokenRef<'_>,
        invalid_code: &str,
    ) -> Result<Option<(MemberId, TypeId)>, HirError> {
        let spelling = token
            .token()
            .normalized_identifier()
            .map(str::to_owned)
            .unwrap_or(self.token_text(file, token)?.to_owned());
        let Ok(name) = MemberName::new(spelling) else {
            self.emit(
                self.sources.span(file, token.range())?,
                invalid_code,
                "invalid field name",
                Vec::new(),
                None,
            )?;
            return Ok(None);
        };
        let Some((symbol, arguments, shape)) = self.nominal_instance(base)? else {
            self.emit(
                self.sources.span(file, token.range())?,
                invalid_code,
                "field access requires a record or newtype value",
                Vec::new(),
                None,
            )?;
            return Ok(None);
        };
        let declarations = match shape {
            HirNominalShape::Record { fields } => fields,
            HirNominalShape::Newtype { underlying } => {
                let member = self
                    .resolved
                    .lookup_members(
                        MemberOwner::Type(symbol),
                        &MemberName::new("value").expect("value is a valid member name"),
                    )
                    .and_then(|members| members.first())
                    .copied();
                let Some(member) = member.filter(|_| name.as_str() == "value") else {
                    self.emit(
                        self.sources.span(file, token.range())?,
                        invalid_code,
                        "unknown newtype field",
                        Vec::new(),
                        None,
                    )?;
                    return Ok(None);
                };
                vec![HirField {
                    member,
                    ty: underlying,
                }]
            }
            HirNominalShape::Enum { .. } => {
                self.emit(
                    self.sources.span(file, token.range())?,
                    invalid_code,
                    "enum payloads must be selected by pattern matching",
                    Vec::new(),
                    None,
                )?;
                return Ok(None);
            }
        };
        let Some(field) = declarations.iter().find(|field| {
            self.resolved
                .member(field.member())
                .is_some_and(|member| member.name() == &name)
        }) else {
            self.emit(
                self.sources.span(file, token.range())?,
                invalid_code,
                "unknown field for this value type",
                Vec::new(),
                None,
            )?;
            return Ok(None);
        };
        let member_id = field.member();
        let field_type = field.ty();
        let member = self
            .resolved
            .member(member_id)
            .expect("HIR fields retain valid resolved members");
        if member.visibility() == Visibility::Private {
            let owner = self
                .resolved
                .symbol(symbol)
                .expect("nominal HIR declarations retain valid symbols");
            let source = self.sources.get(file)?;
            if owner.identity().source_id() != source.source_id()
                || owner.identity().module() != source.module()
            {
                self.emit(
                    self.sources.span(file, token.range())?,
                    "E1501",
                    "this field is private to its declaring module",
                    vec![("the private field is declared here", member.span())],
                    None,
                )?;
            }
        }
        let ty = self.instantiate_type(&arguments, field_type)?;
        self.record_member_reference(self.sources.span(file, token.range())?, member_id);
        Ok(Some((member_id, ty)))
    }

    fn emit_invalid_assignment_target(
        &mut self,
        span: Span,
        message: &str,
    ) -> Result<(), HirError> {
        self.emit(span, "E1411", message, Vec::new(), None)
    }

    fn check_binding(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        context: &mut BodyContext,
    ) -> Result<Option<HirStatement>, HirError> {
        let declared_type = node
            .child_nodes()
            .find(|child| child.kind() == SyntaxKind::TypeExpr)
            .and_then(|annotation| self.program.type_at(file, annotation.range()));
        let initializer = node
            .child_nodes()
            .find(|child| AstExpression::cast(*child).is_some());
        let Some(initializer) = initializer else {
            self.emit(
                self.sources.span(file, node.range())?,
                "E1109",
                "`let` and `var` bindings require an initializer",
                Vec::new(),
                None,
            )?;
            return Ok(None);
        };
        let value = self.check_expression(
            file,
            initializer,
            declared_type.map(ExpressionExpectation::Direct),
            context,
        )?;
        let ty = self.expression_type(value);
        let Some(pattern_node) = node
            .child_nodes()
            .find(|child| AstPattern::cast(*child).is_some())
        else {
            self.complete = false;
            return Ok(None);
        };
        let existing_locals = context.locals.keys().copied().collect::<BTreeSet<_>>();
        let pattern =
            self.check_binding_pattern(file, pattern_node, ty, context, PatternContext::Binding)?;
        let mutable = node
            .child_tokens()
            .any(|token| token.kind() == TokenKind::Var);
        if mutable {
            for local in context.locals.keys().copied() {
                if !existing_locals.contains(&local) {
                    context
                        .local_permissions
                        .insert(local, PlacePermission::Replace);
                }
            }
        }
        Ok(Some(HirStatement::Binding {
            span: self.sources.span(file, node.range())?,
            mutable,
            pattern,
            declared_type,
            value,
        }))
    }

    fn check_binding_pattern(
        &mut self,
        file: FileId,
        pattern_node: SyntaxNodeRef<'_>,
        ty: TypeId,
        context: &mut BodyContext,
        pattern_context: PatternContext,
    ) -> Result<HirPatternId, HirError> {
        let checked = self.check_pattern(file, pattern_node, ty, context, pattern_context)?;
        let span = self.sources.span(file, pattern_node.range())?;
        if checked.valid && !self.pattern_is_irrefutable(&checked.shape, ty, span)? {
            self.emit(
                span,
                "E1201",
                "this binding pattern is refutable for its initializer type",
                Vec::new(),
                None,
            )?;
        }
        Ok(checked.id)
    }

    fn check_pattern(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        expected: TypeId,
        context: &mut BodyContext,
        pattern_context: PatternContext,
    ) -> Result<CheckedPattern, HirError> {
        let Some(pattern) = AstPattern::cast(node) else {
            return self.recovery_pattern(file, node.range());
        };
        match pattern {
            AstPattern::Wildcard(_) => {
                let id = self.allocate_pattern(HirPattern {
                    span: self.sources.span(file, node.range())?,
                    ty: expected,
                    kind: HirPatternKind::Wildcard,
                })?;
                Ok(CheckedPattern {
                    id,
                    shape: PatternShape::Wildcard,
                    valid: true,
                })
            }
            AstPattern::Binding(_) => {
                self.check_pattern_binding(file, node, expected, context, false)
            }
            AstPattern::BorrowBinding(_) => {
                if pattern_context == PatternContext::Binding {
                    self.emit_invalid_pattern(
                        file,
                        node.range(),
                        "borrow bindings are allowed only in `for` headers and `match` arms",
                    )?;
                    return self.recovery_pattern(file, node.range());
                }
                self.check_pattern_binding(file, node, expected, context, true)
            }
            AstPattern::Unit(_) => self.check_unit_pattern(file, node, expected),
            AstPattern::Literal(_) => self.check_literal_pattern(file, node, expected),
            AstPattern::OptionResult(_) => {
                self.check_option_result_pattern(file, node, expected, context, pattern_context)
            }
            AstPattern::Tuple(_) => {
                self.check_tuple_pattern(file, node, expected, context, pattern_context)
            }
            AstPattern::Constructor(_) => {
                self.check_constructor_pattern(file, node, expected, context, pattern_context)
            }
            AstPattern::Record(_) => {
                self.check_record_pattern(file, node, expected, context, pattern_context)
            }
            AstPattern::QualifiedValue(_) => {
                self.check_qualified_value_pattern(file, node, expected)
            }
            AstPattern::Array(_) => {
                self.check_array_pattern(file, node, expected, context, pattern_context)
            }
        }
    }

    fn check_pattern_binding(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        ty: TypeId,
        context: &mut BodyContext,
        borrowed: bool,
    ) -> Result<CheckedPattern, HirError> {
        let Some(token) = node
            .child_tokens()
            .find(|token| token.kind() == TokenKind::Identifier)
        else {
            return self.recovery_pattern(file, node.range());
        };
        if token.token().normalized_identifier() == Some("_") {
            self.emit_invalid_pattern(file, node.range(), "`ref _` is redundant; use `_`")?;
            return self.recovery_pattern(file, node.range());
        }
        let Some(local) = self.resolved.local_at(file, token.range()) else {
            self.complete = false;
            return self.recovery_pattern(file, node.range());
        };
        context.locals.insert(local.id(), ty);
        context
            .local_permissions
            .entry(local.id())
            .or_insert(PlacePermission::Immutable);
        self.program.local_types.insert(local.id(), ty);
        let id = self.allocate_pattern(HirPattern {
            span: self.sources.span(file, node.range())?,
            ty,
            kind: if borrowed {
                HirPatternKind::BorrowBinding(local.id())
            } else {
                HirPatternKind::Binding(local.id())
            },
        })?;
        Ok(CheckedPattern {
            id,
            shape: PatternShape::Wildcard,
            valid: true,
        })
    }

    fn check_unit_pattern(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        expected: TypeId,
    ) -> Result<CheckedPattern, HirError> {
        let unit = self.program.interner.scalar(ScalarType::Unit);
        let Some(member) = self.select_pattern_member(expected, |ty| ty == unit)? else {
            self.emit_pattern_type_mismatch(file, node.range(), "unit pattern", expected)?;
            return self.recovery_pattern(file, node.range());
        };
        let id = self.allocate_pattern(HirPattern {
            span: self.sources.span(file, node.range())?,
            ty: member,
            kind: HirPatternKind::Literal(HirLiteral::Unit),
        })?;
        self.wrap_union_pattern(
            file,
            node.range(),
            expected,
            member,
            CheckedPattern {
                id,
                shape: PatternShape::Constructor {
                    key: PatternConstructor::Unit,
                    arguments: Vec::new(),
                },
                valid: true,
            },
        )
    }

    fn check_literal_pattern(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        expected: TypeId,
    ) -> Result<CheckedPattern, HirError> {
        let Some(token) = node.descendant_tokens().find(|token| {
            !token.kind().is_trivia() && !matches!(token.kind(), TokenKind::Minus | TokenKind::Nl)
        }) else {
            return self.recovery_pattern(file, node.range());
        };
        let (required, literal, key) = match token.kind() {
            TokenKind::True => (
                self.program.interner.scalar(ScalarType::Bool),
                HirLiteral::Bool(true),
                PatternConstructor::Bool(true),
            ),
            TokenKind::False => (
                self.program.interner.scalar(ScalarType::Bool),
                HirLiteral::Bool(false),
                PatternConstructor::Bool(false),
            ),
            TokenKind::CharLiteral => {
                let spelling = self.token_text(file, token)?.to_owned();
                let normalized = decode_char_literal(&spelling).unwrap_or_else(|| spelling.clone());
                let ty = self.program.interner.scalar(ScalarType::Char);
                (
                    ty,
                    HirLiteral::Char(spelling),
                    PatternConstructor::Literal {
                        ty,
                        value: normalized,
                    },
                )
            }
            TokenKind::RawStringLiteral
            | TokenKind::RawMultilineStringLiteral
            | TokenKind::StringStart
            | TokenKind::MultilineStringStart => {
                if contains_syntax_kind(node, SyntaxKind::Interpolation) {
                    self.emit_invalid_pattern(
                        file,
                        node.range(),
                        "interpolated strings are not patterns",
                    )?;
                    return self.recovery_pattern(file, node.range());
                }
                let spelling = self.source_text(file, node.range())?.to_owned();
                let normalized = decode_string_literal_pattern(&spelling, token.kind())
                    .unwrap_or_else(|| spelling.clone());
                let ty = self.program.interner.scalar(ScalarType::String);
                (
                    ty,
                    HirLiteral::String(spelling),
                    PatternConstructor::Literal {
                        ty,
                        value: normalized,
                    },
                )
            }
            TokenKind::IntegerLiteral | TokenKind::FloatLiteral => {
                return self.check_numeric_literal_pattern(file, node, token, expected);
            }
            _ => return self.recovery_pattern(file, node.range()),
        };
        let Some(member) = self.select_pattern_member(expected, |ty| ty == required)? else {
            self.emit_pattern_type_mismatch(file, node.range(), "literal pattern", expected)?;
            return self.recovery_pattern(file, node.range());
        };
        let id = self.allocate_pattern(HirPattern {
            span: self.sources.span(file, node.range())?,
            ty: member,
            kind: HirPatternKind::Literal(literal),
        })?;
        self.wrap_union_pattern(
            file,
            node.range(),
            expected,
            member,
            CheckedPattern {
                id,
                shape: PatternShape::Constructor {
                    key,
                    arguments: Vec::new(),
                },
                valid: true,
            },
        )
    }

    fn check_numeric_literal_pattern(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        token: SyntaxTokenRef<'_>,
        expected: TypeId,
    ) -> Result<CheckedPattern, HirError> {
        let spelling = self.source_text(file, node.range())?.to_owned();
        let negative = node
            .descendant_tokens()
            .any(|token| token.kind() == TokenKind::Minus);
        let token_spelling = self.token_text(file, token)?;
        let (member, literal, normalized) = match token.kind() {
            TokenKind::IntegerLiteral => {
                let explicit = integer_suffix(token_spelling);
                let member = if let Some(scalar) = explicit {
                    let ty = self.program.interner.scalar(scalar);
                    self.select_pattern_member(expected, |candidate| candidate == ty)?
                } else {
                    self.select_pattern_member(expected, |candidate| {
                        matches!(
                            self.program.interner.kind(candidate),
                            Ok(TypeKind::Scalar(scalar)) if is_integer_scalar(*scalar)
                        )
                    })?
                };
                let Some(member) = member else {
                    self.emit_pattern_type_mismatch(
                        file,
                        node.range(),
                        "integer literal pattern",
                        expected,
                    )?;
                    return self.recovery_pattern(file, node.range());
                };
                let TypeKind::Scalar(scalar) = self.program.interner.kind(member)? else {
                    unreachable!("integer pattern selection requires a scalar");
                };
                let Some(magnitude) = integer_magnitude(token_spelling) else {
                    self.emit_invalid_pattern(
                        file,
                        node.range(),
                        "integer pattern exceeds the intrinsic numeric domain",
                    )?;
                    return self.recovery_pattern(file, node.range());
                };
                let fits = if negative {
                    integer_shape(*scalar)
                        .is_some_and(|(signed, bits)| signed && magnitude <= (1_u128 << (bits - 1)))
                } else {
                    integer_fits_positive(magnitude, *scalar)
                };
                if !fits {
                    self.emit_invalid_pattern(
                        file,
                        node.range(),
                        "integer pattern is outside the scrutinee type's range",
                    )?;
                    return self.recovery_pattern(file, node.range());
                }
                let normalized = if negative && magnitude != 0 {
                    format!("-{magnitude}")
                } else {
                    magnitude.to_string()
                };
                (member, HirLiteral::Integer(spelling.clone()), normalized)
            }
            TokenKind::FloatLiteral => {
                let explicit = float_suffix(token_spelling);
                let member = if let Some(scalar) = explicit {
                    let ty = self.program.interner.scalar(scalar);
                    self.select_pattern_member(expected, |candidate| candidate == ty)?
                } else {
                    self.select_pattern_member(expected, |candidate| {
                        matches!(
                            self.program.interner.kind(candidate),
                            Ok(TypeKind::Scalar(scalar)) if is_float_scalar(*scalar)
                        )
                    })?
                };
                let Some(member) = member else {
                    self.emit_pattern_type_mismatch(
                        file,
                        node.range(),
                        "floating literal pattern",
                        expected,
                    )?;
                    return self.recovery_pattern(file, node.range());
                };
                let TypeKind::Scalar(scalar) = self.program.interner.kind(member)? else {
                    unreachable!("floating pattern selection requires a scalar");
                };
                if !float_is_representable(token_spelling, *scalar) {
                    self.emit_invalid_pattern(
                        file,
                        node.range(),
                        "floating pattern is outside the scrutinee type's range",
                    )?;
                    return self.recovery_pattern(file, node.range());
                }
                let normalized = normalize_float_pattern(token_spelling, negative, *scalar)
                    .unwrap_or_else(|| spelling.clone());
                (member, HirLiteral::Float(spelling.clone()), normalized)
            }
            _ => unreachable!("numeric pattern token selection is closed"),
        };
        let id = self.allocate_pattern(HirPattern {
            span: self.sources.span(file, node.range())?,
            ty: member,
            kind: HirPatternKind::Literal(literal),
        })?;
        self.wrap_union_pattern(
            file,
            node.range(),
            expected,
            member,
            CheckedPattern {
                id,
                shape: PatternShape::Constructor {
                    key: PatternConstructor::Literal {
                        ty: member,
                        value: normalized,
                    },
                    arguments: Vec::new(),
                },
                valid: true,
            },
        )
    }

    fn check_option_result_pattern(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        expected: TypeId,
        context: &mut BodyContext,
        pattern_context: PatternContext,
    ) -> Result<CheckedPattern, HirError> {
        let Some(token) = node.child_tokens().find(|token| {
            matches!(
                token.kind(),
                TokenKind::Some | TokenKind::None | TokenKind::Ok | TokenKind::Err
            )
        }) else {
            return self.recovery_pattern(file, node.range());
        };
        let predicate: fn(&TypeKind) -> bool = match token.kind() {
            TokenKind::Some | TokenKind::None => is_option_type,
            TokenKind::Ok | TokenKind::Err => is_result_type,
            _ => unreachable!("pattern constructor token selection is closed"),
        };
        let Some(member) = self.select_pattern_member(expected, |candidate| {
            self.program
                .interner
                .kind(candidate)
                .ok()
                .is_some_and(predicate)
        })?
        else {
            self.emit_pattern_type_mismatch(file, node.range(), "option/result pattern", expected)?;
            return self.recovery_pattern(file, node.range());
        };
        let (kind, shape, valid) = match (token.kind(), self.program.interner.kind(member)?.clone())
        {
            (TokenKind::None, TypeKind::Option(_)) => (
                HirPatternKind::OptionNone,
                PatternShape::Constructor {
                    key: PatternConstructor::OptionNone,
                    arguments: Vec::new(),
                },
                true,
            ),
            (TokenKind::Some, TypeKind::Option(item)) => {
                let Some(payload_node) = node
                    .child_nodes()
                    .find(|child| AstPattern::cast(*child).is_some())
                else {
                    return self.recovery_pattern(file, node.range());
                };
                let payload =
                    self.check_pattern(file, payload_node, item, context, pattern_context)?;
                (
                    HirPatternKind::OptionSome(payload.id),
                    PatternShape::Constructor {
                        key: PatternConstructor::OptionSome,
                        arguments: vec![payload.shape],
                    },
                    payload.valid,
                )
            }
            (TokenKind::Ok, TypeKind::Result { success, .. }) => {
                let Some(payload_node) = node
                    .child_nodes()
                    .find(|child| AstPattern::cast(*child).is_some())
                else {
                    return self.recovery_pattern(file, node.range());
                };
                let payload =
                    self.check_pattern(file, payload_node, success, context, pattern_context)?;
                (
                    HirPatternKind::ResultOk(payload.id),
                    PatternShape::Constructor {
                        key: PatternConstructor::ResultOk,
                        arguments: vec![payload.shape],
                    },
                    payload.valid,
                )
            }
            (TokenKind::Err, TypeKind::Result { error, .. }) => {
                let Some(payload_node) = node
                    .child_nodes()
                    .find(|child| AstPattern::cast(*child).is_some())
                else {
                    return self.recovery_pattern(file, node.range());
                };
                let payload =
                    self.check_pattern(file, payload_node, error, context, pattern_context)?;
                (
                    HirPatternKind::ResultErr(payload.id),
                    PatternShape::Constructor {
                        key: PatternConstructor::ResultErr,
                        arguments: vec![payload.shape],
                    },
                    payload.valid,
                )
            }
            _ => {
                self.emit_invalid_pattern(
                    file,
                    node.range(),
                    "pattern constructor does not match the selected type",
                )?;
                return self.recovery_pattern(file, node.range());
            }
        };
        let id = self.allocate_pattern(HirPattern {
            span: self.sources.span(file, node.range())?,
            ty: member,
            kind,
        })?;
        self.wrap_union_pattern(
            file,
            node.range(),
            expected,
            member,
            CheckedPattern { id, shape, valid },
        )
    }

    fn check_tuple_pattern(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        expected: TypeId,
        context: &mut BodyContext,
        pattern_context: PatternContext,
    ) -> Result<CheckedPattern, HirError> {
        let nodes = node
            .child_nodes()
            .filter(|child| AstPattern::cast(*child).is_some())
            .collect::<Vec<_>>();
        let Some(member) = self.select_pattern_member(expected, |candidate| {
            matches!(
                self.program.interner.kind(candidate),
                Ok(TypeKind::Tuple(items)) if items.len() == nodes.len()
            )
        })?
        else {
            self.emit_pattern_type_mismatch(file, node.range(), "tuple pattern", expected)?;
            return self.recovery_pattern(file, node.range());
        };
        let TypeKind::Tuple(types) = self.program.interner.kind(member)?.clone() else {
            unreachable!("tuple pattern selection requires a tuple");
        };
        let mut items = Vec::with_capacity(nodes.len());
        let mut shapes = Vec::with_capacity(nodes.len());
        let mut valid = true;
        for (node, ty) in nodes.into_iter().zip(types) {
            let item = self.check_pattern(file, node, ty, context, pattern_context)?;
            valid &= item.valid;
            items.push(item.id);
            shapes.push(item.shape);
        }
        let id = self.allocate_pattern(HirPattern {
            span: self.sources.span(file, node.range())?,
            ty: member,
            kind: HirPatternKind::Tuple(items),
        })?;
        self.wrap_union_pattern(
            file,
            node.range(),
            expected,
            member,
            CheckedPattern {
                id,
                shape: PatternShape::Constructor {
                    key: PatternConstructor::Tuple(shapes.len()),
                    arguments: shapes,
                },
                valid,
            },
        )
    }

    fn check_array_pattern(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        expected: TypeId,
        context: &mut BodyContext,
        pattern_context: PatternContext,
    ) -> Result<CheckedPattern, HirError> {
        let Some(member) = self.select_pattern_member(expected, |candidate| {
            matches!(
                self.program.interner.kind(candidate),
                Ok(TypeKind::Intrinsic {
                    constructor: IntrinsicType::Array,
                    ..
                })
            )
        })?
        else {
            self.emit_pattern_type_mismatch(file, node.range(), "array pattern", expected)?;
            return self.recovery_pattern(file, node.range());
        };
        let TypeKind::Intrinsic { arguments, .. } = self.program.interner.kind(member)?.clone()
        else {
            unreachable!("array pattern selection requires Array[T]");
        };
        let element_type = arguments[0];
        let prefix_nodes = node
            .child_nodes()
            .filter(|child| AstPattern::cast(*child).is_some())
            .collect::<Vec<_>>();
        let rest_node = node
            .child_nodes()
            .find(|child| child.kind() == SyntaxKind::ArrayRestPattern);
        let mut prefix = Vec::with_capacity(prefix_nodes.len());
        let mut shapes = Vec::with_capacity(prefix_nodes.len());
        let mut valid = true;
        for prefix_node in prefix_nodes {
            let checked =
                self.check_pattern(file, prefix_node, element_type, context, pattern_context)?;
            valid &= checked.valid;
            prefix.push(checked.id);
            shapes.push(checked.shape);
        }

        let rest = if let Some(rest_node) = rest_node {
            let borrowed = rest_node
                .child_tokens()
                .any(|token| token.kind() == TokenKind::Ref);
            if let Some(binding) = rest_node
                .child_tokens()
                .find(|token| token.kind() == TokenKind::Identifier)
            {
                let checked = if borrowed && pattern_context == PatternContext::Binding {
                    self.emit_invalid_pattern(
                        file,
                        rest_node.range(),
                        "borrow bindings are not allowed in `let` or `var` patterns",
                    )?;
                    self.recovery_pattern(file, rest_node.range())?
                } else {
                    self.check_pattern_binding_token(file, binding, member, context, borrowed)?
                };
                valid &= checked.valid;
                Some(checked.id)
            } else {
                Some(self.allocate_pattern(HirPattern {
                    span: self.sources.span(file, rest_node.range())?,
                    ty: member,
                    kind: HirPatternKind::Wildcard,
                })?)
            }
        } else {
            None
        };
        let id = self.allocate_pattern(HirPattern {
            span: self.sources.span(file, node.range())?,
            ty: member,
            kind: HirPatternKind::Array { prefix, rest },
        })?;
        self.wrap_union_pattern(
            file,
            node.range(),
            expected,
            member,
            CheckedPattern {
                id,
                shape: PatternShape::Array {
                    elements: Arc::from(shapes),
                    offset: 0,
                    has_rest: rest_node.is_some(),
                },
                valid,
            },
        )
    }

    fn check_constructor_pattern(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        expected: TypeId,
        context: &mut BodyContext,
        pattern_context: PatternContext,
    ) -> Result<CheckedPattern, HirError> {
        let Some(path) = self.pattern_path_info(file, node)? else {
            return self.recovery_pattern(file, node.range());
        };
        let Some(member) = self.select_pattern_member_checked(expected, |candidate| {
            self.pattern_path_matches_type(&path, candidate)
        })?
        else {
            self.emit_pattern_type_mismatch(file, node.range(), "constructor pattern", expected)?;
            return self.recovery_pattern(file, node.range());
        };
        let mut direct_patterns = node
            .child_nodes()
            .filter(|child| AstPattern::cast(*child).is_some());
        let path_node = direct_patterns.next();
        debug_assert!(
            path_node.is_some_and(|child| child.kind() == SyntaxKind::BindingPattern),
            "parsed constructor patterns begin with their binding-pattern path"
        );
        let payload_nodes = direct_patterns.collect::<Vec<_>>();

        if let Some((symbol, arguments, shape)) = self.nominal_instance(member)? {
            if let HirNominalShape::Enum { variants } = &shape
                && !path.suffix.is_empty()
            {
                let Some(variant_segment) = path.suffix.last() else {
                    unreachable!("an enum variant path has a final segment");
                };
                let Some(variant) = variants.iter().find(|variant| {
                    self.resolved
                        .member(variant.member())
                        .is_some_and(|member| {
                            member.name().as_str() == variant_segment.name.as_str()
                        })
                }) else {
                    self.emit_invalid_pattern(file, node.range(), "unknown enum variant")?;
                    return self.recovery_pattern(file, node.range());
                };
                self.record_member_reference(variant_segment.span, variant.member());
                let HirVariantPayload::Tuple(templates) = variant.payload() else {
                    self.emit_invalid_pattern(
                        file,
                        node.range(),
                        "this enum variant does not have a tuple payload",
                    )?;
                    return self.recovery_pattern(file, node.range());
                };
                if templates.len() != payload_nodes.len() {
                    self.emit_invalid_pattern(
                        file,
                        node.range(),
                        "enum variant pattern has the wrong payload arity",
                    )?;
                    return self.recovery_pattern(file, node.range());
                }
                let types = self.instantiate_types(&arguments, templates)?;
                let mut fields = Vec::with_capacity(types.len());
                let mut shapes = Vec::with_capacity(types.len());
                let mut valid = true;
                for (payload_node, ty) in payload_nodes.into_iter().zip(types) {
                    let payload =
                        self.check_pattern(file, payload_node, ty, context, pattern_context)?;
                    valid &= payload.valid;
                    fields.push(payload.id);
                    shapes.push(payload.shape);
                }
                let id = self.allocate_pattern(HirPattern {
                    span: self.sources.span(file, node.range())?,
                    ty: member,
                    kind: HirPatternKind::Variant {
                        variant: variant.member(),
                        fields,
                    },
                })?;
                return self.wrap_union_pattern(
                    file,
                    node.range(),
                    expected,
                    member,
                    CheckedPattern {
                        id,
                        shape: PatternShape::Constructor {
                            key: PatternConstructor::Variant(variant.member()),
                            arguments: shapes,
                        },
                        valid,
                    },
                );
            }

            if let HirNominalShape::Newtype { underlying } = shape {
                if payload_nodes.len() != 1 {
                    self.emit_invalid_pattern(
                        file,
                        node.range(),
                        "newtype pattern requires exactly one payload",
                    )?;
                    return self.recovery_pattern(file, node.range());
                }
                let underlying = self.instantiate_type(&arguments, underlying)?;
                let payload = self.check_pattern(
                    file,
                    payload_nodes[0],
                    underlying,
                    context,
                    pattern_context,
                )?;
                let id = self.allocate_pattern(HirPattern {
                    span: self.sources.span(file, node.range())?,
                    ty: member,
                    kind: HirPatternKind::Newtype {
                        constructor: symbol,
                        value: payload.id,
                    },
                })?;
                return self.wrap_union_pattern(
                    file,
                    node.range(),
                    expected,
                    member,
                    CheckedPattern {
                        id,
                        shape: PatternShape::Constructor {
                            key: PatternConstructor::Newtype(symbol),
                            arguments: vec![payload.shape],
                        },
                        valid: payload.valid,
                    },
                );
            }
        }

        if payload_nodes.len() != 1 {
            self.emit_invalid_pattern(
                file,
                node.range(),
                "a union member type pattern requires exactly one inner pattern",
            )?;
            return self.recovery_pattern(file, node.range());
        }
        let payload =
            self.check_pattern(file, payload_nodes[0], member, context, pattern_context)?;
        self.wrap_union_pattern(file, node.range(), expected, member, payload)
    }

    fn check_record_pattern(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        expected: TypeId,
        context: &mut BodyContext,
        pattern_context: PatternContext,
    ) -> Result<CheckedPattern, HirError> {
        let Some(path) = self.pattern_path_info(file, node)? else {
            return self.recovery_pattern(file, node.range());
        };
        let Some(member) = self.select_pattern_member_checked(expected, |candidate| {
            self.pattern_path_matches_type(&path, candidate)
        })?
        else {
            self.emit_pattern_type_mismatch(file, node.range(), "record pattern", expected)?;
            return self.recovery_pattern(file, node.range());
        };
        let Some((symbol, arguments, shape)) = self.nominal_instance(member)? else {
            self.emit_invalid_pattern(
                file,
                node.range(),
                "record pattern requires a nominal type",
            )?;
            return self.recovery_pattern(file, node.range());
        };

        match shape {
            HirNominalShape::Record { fields } if path.suffix.is_empty() => {
                let checked = self.check_record_fields(
                    file,
                    node,
                    &arguments,
                    &fields,
                    context,
                    pattern_context,
                )?;
                let id = self.allocate_pattern(HirPattern {
                    span: self.sources.span(file, node.range())?,
                    ty: member,
                    kind: HirPatternKind::Record {
                        owner: symbol,
                        fields: checked.fields,
                        has_rest: checked.has_rest,
                    },
                })?;
                self.wrap_union_pattern(
                    file,
                    node.range(),
                    expected,
                    member,
                    CheckedPattern {
                        id,
                        shape: PatternShape::Constructor {
                            key: PatternConstructor::Record(symbol),
                            arguments: checked.shapes,
                        },
                        valid: checked.valid,
                    },
                )
            }
            HirNominalShape::Enum { variants } if !path.suffix.is_empty() => {
                let Some(variant_segment) = path.suffix.last() else {
                    return self.recovery_pattern(file, node.range());
                };
                let Some(variant) = variants.iter().find(|variant| {
                    self.resolved
                        .member(variant.member())
                        .is_some_and(|member| {
                            member.name().as_str() == variant_segment.name.as_str()
                        })
                }) else {
                    self.emit_invalid_pattern(file, node.range(), "unknown enum variant")?;
                    return self.recovery_pattern(file, node.range());
                };
                self.record_member_reference(variant_segment.span, variant.member());
                let HirVariantPayload::Record(fields) = variant.payload() else {
                    self.emit_invalid_pattern(
                        file,
                        node.range(),
                        "this enum variant does not have a record payload",
                    )?;
                    return self.recovery_pattern(file, node.range());
                };
                let checked = self.check_record_fields(
                    file,
                    node,
                    &arguments,
                    fields,
                    context,
                    pattern_context,
                )?;
                let id = self.allocate_pattern(HirPattern {
                    span: self.sources.span(file, node.range())?,
                    ty: member,
                    kind: HirPatternKind::Variant {
                        variant: variant.member(),
                        fields: checked.ordered_patterns,
                    },
                })?;
                self.wrap_union_pattern(
                    file,
                    node.range(),
                    expected,
                    member,
                    CheckedPattern {
                        id,
                        shape: PatternShape::Constructor {
                            key: PatternConstructor::Variant(variant.member()),
                            arguments: checked.shapes,
                        },
                        valid: checked.valid,
                    },
                )
            }
            _ => {
                self.emit_invalid_pattern(
                    file,
                    node.range(),
                    "record pattern shape does not match its nominal declaration",
                )?;
                self.recovery_pattern(file, node.range())
            }
        }
    }

    fn check_record_fields(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        arguments: &[TypeId],
        declarations: &[HirField],
        context: &mut BodyContext,
        pattern_context: PatternContext,
    ) -> Result<CheckedRecordFields, HirError> {
        let field_nodes = node
            .child_nodes()
            .filter(|child| child.kind() == SyntaxKind::RecordPatternField)
            .collect::<Vec<_>>();
        let has_rest = node
            .child_nodes()
            .any(|child| child.kind() == SyntaxKind::RecordRestPattern);
        let mut by_member = BTreeMap::<MemberId, CheckedPattern>::new();
        let mut valid = true;
        for field_node in field_nodes {
            let Some(name_token) = field_node
                .child_tokens()
                .find(|token| token.kind() == TokenKind::Identifier)
            else {
                valid = false;
                continue;
            };
            let Some(name) = name_token
                .token()
                .normalized_identifier()
                .and_then(|name| MemberName::new(name).ok())
            else {
                valid = false;
                continue;
            };
            let Some(declaration) = declarations.iter().find(|field| {
                self.resolved
                    .member(field.member())
                    .is_some_and(|member| member.name() == &name)
            }) else {
                self.emit_invalid_pattern(file, field_node.range(), "unknown record field")?;
                valid = false;
                continue;
            };
            self.record_member_reference(
                self.sources.span(file, name_token.range())?,
                declaration.member(),
            );
            if by_member.contains_key(&declaration.member()) {
                self.emit_invalid_pattern(
                    file,
                    field_node.range(),
                    "record field appears more than once in this pattern",
                )?;
                valid = false;
                continue;
            }
            let ty = self.instantiate_type(arguments, declaration.ty())?;
            let checked = if let Some(pattern_node) = field_node
                .child_nodes()
                .find(|child| AstPattern::cast(*child).is_some())
            {
                self.check_pattern(file, pattern_node, ty, context, pattern_context)?
            } else {
                let borrowed = field_node
                    .child_tokens()
                    .any(|token| token.kind() == TokenKind::Ref);
                if borrowed && pattern_context == PatternContext::Binding {
                    self.emit_invalid_pattern(
                        file,
                        field_node.range(),
                        "borrow bindings are not allowed in `let` or `var` patterns",
                    )?;
                    self.recovery_pattern(file, field_node.range())?
                } else {
                    self.check_pattern_binding_token(file, name_token, ty, context, borrowed)?
                }
            };
            valid &= checked.valid;
            by_member.insert(declaration.member(), checked);
        }

        if !has_rest && by_member.len() != declarations.len() {
            self.emit_invalid_pattern(
                file,
                node.range(),
                "record pattern must name every field or end with `..`",
            )?;
            valid = false;
        }

        let mut fields = Vec::with_capacity(declarations.len());
        let mut ordered_patterns = Vec::with_capacity(declarations.len());
        let mut shapes = Vec::with_capacity(declarations.len());
        for declaration in declarations {
            let checked = if let Some(checked) = by_member.remove(&declaration.member()) {
                checked
            } else {
                let ty = self.instantiate_type(arguments, declaration.ty())?;
                let id = self.allocate_pattern(HirPattern {
                    span: self.sources.span(file, node.range())?,
                    ty,
                    kind: HirPatternKind::Wildcard,
                })?;
                CheckedPattern {
                    id,
                    shape: PatternShape::Wildcard,
                    valid: has_rest,
                }
            };
            fields.push(HirPatternField {
                member: declaration.member(),
                pattern: checked.id,
            });
            ordered_patterns.push(checked.id);
            shapes.push(checked.shape);
        }
        Ok(CheckedRecordFields {
            fields,
            ordered_patterns,
            shapes,
            valid,
            has_rest,
        })
    }

    fn check_pattern_binding_token(
        &mut self,
        file: FileId,
        token: SyntaxTokenRef<'_>,
        ty: TypeId,
        context: &mut BodyContext,
        borrowed: bool,
    ) -> Result<CheckedPattern, HirError> {
        if token.token().normalized_identifier() == Some("_") {
            self.emit_invalid_pattern(
                file,
                token.range(),
                "discard `_` cannot be used as a named pattern binding",
            )?;
            return self.recovery_pattern(file, token.range());
        }
        let Some(local) = self.resolved.local_at(file, token.range()) else {
            self.complete = false;
            return self.recovery_pattern(file, token.range());
        };
        context.locals.insert(local.id(), ty);
        context
            .local_permissions
            .entry(local.id())
            .or_insert(PlacePermission::Immutable);
        self.program.local_types.insert(local.id(), ty);
        let id = self.allocate_pattern(HirPattern {
            span: self.sources.span(file, token.range())?,
            ty,
            kind: if borrowed {
                HirPatternKind::BorrowBinding(local.id())
            } else {
                HirPatternKind::Binding(local.id())
            },
        })?;
        Ok(CheckedPattern {
            id,
            shape: PatternShape::Wildcard,
            valid: true,
        })
    }

    fn check_qualified_value_pattern(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        expected: TypeId,
    ) -> Result<CheckedPattern, HirError> {
        let Some(path) = self.pattern_path_info(file, node)? else {
            return self.recovery_pattern(file, node.range());
        };
        let Some(member) = self.select_pattern_member_checked(expected, |candidate| {
            self.pattern_path_matches_type(&path, candidate)
        })?
        else {
            self.emit_pattern_type_mismatch(file, node.range(), "enum variant pattern", expected)?;
            return self.recovery_pattern(file, node.range());
        };
        let Some((_, _, HirNominalShape::Enum { variants })) = self.nominal_instance(member)?
        else {
            self.emit_invalid_pattern(
                file,
                node.range(),
                "qualified pattern is not an enum variant",
            )?;
            return self.recovery_pattern(file, node.range());
        };
        let Some(variant_segment) = path.suffix.last() else {
            return self.recovery_pattern(file, node.range());
        };
        let Some(variant) = variants.iter().find(|variant| {
            self.resolved
                .member(variant.member())
                .is_some_and(|member| member.name().as_str() == variant_segment.name.as_str())
        }) else {
            self.emit_invalid_pattern(file, node.range(), "unknown enum variant")?;
            return self.recovery_pattern(file, node.range());
        };
        self.record_member_reference(variant_segment.span, variant.member());
        if !matches!(variant.payload(), HirVariantPayload::Unit) {
            self.emit_invalid_pattern(
                file,
                node.range(),
                "enum variant payload must be matched explicitly",
            )?;
            return self.recovery_pattern(file, node.range());
        }
        let id = self.allocate_pattern(HirPattern {
            span: self.sources.span(file, node.range())?,
            ty: member,
            kind: HirPatternKind::Variant {
                variant: variant.member(),
                fields: Vec::new(),
            },
        })?;
        self.wrap_union_pattern(
            file,
            node.range(),
            expected,
            member,
            CheckedPattern {
                id,
                shape: PatternShape::Constructor {
                    key: PatternConstructor::Variant(variant.member()),
                    arguments: Vec::new(),
                },
                valid: true,
            },
        )
    }

    fn pattern_path_info(
        &mut self,
        file: FileId,
        pattern: SyntaxNodeRef<'_>,
    ) -> Result<Option<PatternPathInfo>, HirError> {
        let Some(path) = pattern
            .child_nodes()
            .find(|child| child.kind() == SyntaxKind::BindingPattern)
        else {
            self.complete = false;
            return Ok(None);
        };
        let tokens = path
            .child_tokens()
            .filter(|token| token.kind() == TokenKind::Identifier)
            .collect::<Vec<_>>();
        let Some((resolved_index, resolved)) =
            tokens.iter().enumerate().find_map(|(index, token)| {
                let reference = self.resolved.reference(file, token.range())?;
                let resolved = match reference.entity() {
                    ResolvedEntity::Name(name) => Some(name.clone()),
                    ResolvedEntity::ContextualCandidates { type_name, .. } => {
                        Some(type_name.clone())
                    }
                    ResolvedEntity::Module(_) => None,
                }?;
                Some((index, resolved))
            })
        else {
            self.complete = false;
            return Ok(None);
        };
        let mut suffix = Vec::new();
        for token in tokens.iter().skip(resolved_index + 1) {
            let Some(name) = token.token().normalized_identifier() else {
                continue;
            };
            suffix.push(PatternPathSegment {
                name: Name::new(name).expect("resolved pattern paths contain ordinary identifiers"),
                span: self.sources.span(file, token.range())?,
            });
        }
        let brackets = path
            .child_nodes()
            .filter(|child| child.kind() == SyntaxKind::BracketPostfix)
            .collect::<Vec<_>>();
        if brackets.len() > 1 {
            self.emit_invalid_pattern(
                file,
                path.range(),
                "a pattern type path has at most one generic argument list",
            )?;
            return Ok(None);
        }
        let applied = if let Some(arguments) = brackets.first().copied() {
            let Some(arguments) = self.pattern_generic_arguments(file, arguments)? else {
                return Ok(None);
            };
            let Some(applied) =
                self.instantiate_pattern_type(file, path.range(), &resolved, arguments)?
            else {
                self.emit_invalid_pattern(
                    file,
                    path.range(),
                    "pattern type path cannot be instantiated with these arguments",
                )?;
                return Ok(None);
            };
            Some(applied)
        } else {
            None
        };
        Ok(Some(PatternPathInfo {
            resolved,
            suffix,
            applied,
        }))
    }

    fn pattern_generic_arguments(
        &mut self,
        file: FileId,
        bracket: SyntaxNodeRef<'_>,
    ) -> Result<Option<Vec<TypeId>>, HirError> {
        let mut arguments = Vec::new();
        for item in bracket
            .child_nodes()
            .filter(|child| child.kind() == SyntaxKind::BracketItem)
        {
            let ty = if let Some(type_node) = item
                .child_nodes()
                .find(|child| child.kind() == SyntaxKind::TypeExpr)
            {
                self.program.type_at(file, type_node.range())
            } else if let Some(expression) = item
                .child_nodes()
                .find(|child| AstExpression::cast(*child).is_some())
            {
                self.lower_pattern_type_expression(file, expression)?
            } else {
                None
            };
            let Some(ty) = ty else {
                self.emit_invalid_pattern(
                    file,
                    item.range(),
                    "generic pattern arguments must be types",
                )?;
                return Ok(None);
            };
            arguments.push(ty);
        }
        Ok(Some(arguments))
    }

    fn lower_pattern_type_expression(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
    ) -> Result<Option<TypeId>, HirError> {
        match node.kind() {
            SyntaxKind::PathExpr => {
                let Some(resolved) = self.resolved_type_name(file, node) else {
                    return Ok(None);
                };
                let arguments = if let Some(bracket) = node
                    .child_nodes()
                    .find(|child| child.kind() == SyntaxKind::BracketPostfix)
                {
                    let Some(arguments) = self.pattern_generic_arguments(file, bracket)? else {
                        return Ok(None);
                    };
                    arguments
                } else {
                    Vec::new()
                };
                self.instantiate_pattern_type(file, node.range(), &resolved, arguments)
            }
            SyntaxKind::PostfixExpr => {
                let Some(base) = node
                    .child_nodes()
                    .find(|child| AstExpression::cast(*child).is_some())
                else {
                    return Ok(None);
                };
                let Some(suffix) = node.child_nodes().find(|child| {
                    matches!(
                        child.kind(),
                        SyntaxKind::BracketPostfix | SyntaxKind::PropagateSuffix
                    )
                }) else {
                    return Ok(None);
                };
                if suffix.kind() == SyntaxKind::PropagateSuffix {
                    let Some(base) = self.lower_pattern_type_expression(file, base)? else {
                        return Ok(None);
                    };
                    return Ok(Some(self.program.interner.option(base)?));
                }
                if base.kind() != SyntaxKind::PathExpr {
                    return Ok(None);
                }
                let Some(resolved) = self.resolved_type_name(file, base) else {
                    return Ok(None);
                };
                let Some(arguments) = self.pattern_generic_arguments(file, suffix)? else {
                    return Ok(None);
                };
                self.instantiate_pattern_type(file, node.range(), &resolved, arguments)
            }
            SyntaxKind::TupleExpr => {
                let mut items = Vec::new();
                for item in node
                    .child_nodes()
                    .filter(|child| AstExpression::cast(*child).is_some())
                {
                    let Some(item) = self.lower_pattern_type_expression(file, item)? else {
                        return Ok(None);
                    };
                    items.push(item);
                }
                Ok(Some(self.program.interner.tuple(items)?))
            }
            SyntaxKind::GroupExpr => {
                let Some(inner) = node
                    .child_nodes()
                    .find(|child| AstExpression::cast(*child).is_some())
                else {
                    return Ok(None);
                };
                self.lower_pattern_type_expression(file, inner)
            }
            SyntaxKind::BinaryExpr
                if node
                    .child_tokens()
                    .any(|token| token.kind() == TokenKind::Pipe) =>
            {
                let mut members = Vec::new();
                for member in node
                    .child_nodes()
                    .filter(|child| AstExpression::cast(*child).is_some())
                {
                    let Some(member) = self.lower_pattern_type_expression(file, member)? else {
                        return Ok(None);
                    };
                    members.push(member);
                }
                Ok(Some(self.program.interner.union(members)?))
            }
            _ => Ok(None),
        }
    }

    fn resolved_type_name(&self, file: FileId, path: SyntaxNodeRef<'_>) -> Option<ResolvedName> {
        path.child_tokens()
            .filter(|token| token.kind() == TokenKind::Identifier)
            .find_map(
                |token| match self.resolved.reference(file, token.range())?.entity() {
                    ResolvedEntity::Name(name) => Some(name.clone()),
                    ResolvedEntity::ContextualCandidates { type_name, .. } => {
                        Some(type_name.clone())
                    }
                    ResolvedEntity::Module(_) => None,
                },
            )
    }

    fn instantiate_pattern_type(
        &mut self,
        _file: FileId,
        _range: TextRange,
        resolved: &ResolvedName,
        arguments: Vec<TypeId>,
    ) -> Result<Option<TypeId>, HirError> {
        match resolved {
            ResolvedName::Symbol(symbol) => {
                let Some(symbol_info) = self.resolved.symbol(*symbol) else {
                    return Ok(None);
                };
                if symbol_info.generic_arity() as usize != arguments.len() {
                    return Ok(None);
                }
                let Some(declaration) = self.program.declaration(*symbol) else {
                    return Ok(None);
                };
                let template = match declaration.kind() {
                    HirTypeDeclarationKind::Alias { target } => *target,
                    HirTypeDeclarationKind::Nominal(definition) => definition.self_type(),
                    HirTypeDeclarationKind::Trait => return Ok(None),
                };
                Ok(Some(
                    TypeSubstitution::new(arguments).apply(&mut self.program.interner, template)?,
                ))
            }
            ResolvedName::Prelude { name, .. } => {
                if let Some(scalar) = self.program.interner.named_scalar(name.as_str()) {
                    return Ok(arguments.is_empty().then_some(scalar));
                }
                let ty = match (name.as_str(), arguments.as_slice()) {
                    ("Option", [item]) => self.program.interner.option(*item)?,
                    ("Result", [success, error]) => {
                        self.program.interner.result(*success, *error)?
                    }
                    (name, arguments) => {
                        let constructor = match name {
                            "Array" => IntrinsicType::Array,
                            "Map" => IntrinsicType::Map,
                            "Set" => IntrinsicType::Set,
                            "Range" => IntrinsicType::Range,
                            "Ref" => IntrinsicType::Ref,
                            "Pointer" => IntrinsicType::Pointer,
                            "Join" => IntrinsicType::Join,
                            "Command" => IntrinsicType::Command,
                            "Pipeline" => IntrinsicType::Pipeline,
                            "NumericConversionError" => IntrinsicType::NumericConversionError,
                            _ => return Ok(None),
                        };
                        if constructor.arity() != arguments.len() {
                            return Ok(None);
                        }
                        self.program
                            .interner
                            .intrinsic(constructor, arguments.to_vec())?
                    }
                };
                Ok(Some(ty))
            }
            ResolvedName::External { .. }
            | ResolvedName::Local(_)
            | ResolvedName::Receiver
            | ResolvedName::ContextualSelf => Ok(None),
        }
    }

    fn pattern_path_matches_type(
        &self,
        path: &PatternPathInfo,
        candidate: TypeId,
    ) -> Result<bool, HirError> {
        if let Some(applied) = path.applied {
            return Ok(applied == candidate);
        }
        match &path.resolved {
            ResolvedName::Symbol(symbol) => {
                let Some(declaration) = self.program.declaration(*symbol) else {
                    return Ok(false);
                };
                match declaration.kind() {
                    HirTypeDeclarationKind::Alias { target } => Ok(*target == candidate),
                    HirTypeDeclarationKind::Nominal(definition) => {
                        let TypeKind::Nominal { identity, .. } =
                            self.program.interner.kind(candidate)?
                        else {
                            return Ok(false);
                        };
                        Ok(self
                            .resolved
                            .symbol(*symbol)
                            .is_some_and(|symbol| symbol.identity() == identity)
                            && matches!(
                                definition.shape(),
                                HirNominalShape::Newtype { .. }
                                    | HirNominalShape::Record { .. }
                                    | HirNominalShape::Enum { .. }
                            ))
                    }
                    HirTypeDeclarationKind::Trait => Ok(false),
                }
            }
            ResolvedName::Prelude { name, .. } => {
                if let Some(scalar) = self.program.interner.named_scalar(name.as_str()) {
                    return Ok(scalar == candidate);
                }
                Ok(
                    match (name.as_str(), self.program.interner.kind(candidate)?) {
                        ("Option", TypeKind::Option(_)) | ("Result", TypeKind::Result { .. }) => {
                            true
                        }
                        (name, TypeKind::Intrinsic { constructor, .. }) => {
                            constructor.as_str() == name
                        }
                        _ => false,
                    },
                )
            }
            ResolvedName::External { .. } => Ok(false),
            ResolvedName::Local(_) | ResolvedName::Receiver | ResolvedName::ContextualSelf => {
                Ok(false)
            }
        }
    }

    fn nominal_instance(
        &self,
        ty: TypeId,
    ) -> Result<Option<(SymbolId, Vec<TypeId>, HirNominalShape)>, HirError> {
        let TypeKind::Nominal {
            identity,
            arguments,
        } = self.program.interner.kind(ty)?
        else {
            return Ok(None);
        };
        let Some(symbol) = self
            .resolved
            .symbols()
            .find(|symbol| symbol.identity() == identity)
            .map(|symbol| symbol.id())
        else {
            return Ok(None);
        };
        let Some(declaration) = self.program.declaration(symbol) else {
            return Ok(None);
        };
        let HirTypeDeclarationKind::Nominal(definition) = declaration.kind() else {
            return Ok(None);
        };
        Ok(Some((
            symbol,
            arguments.clone(),
            definition.shape().clone(),
        )))
    }

    fn instantiate_type(
        &mut self,
        arguments: &[TypeId],
        template: TypeId,
    ) -> Result<TypeId, HirError> {
        Ok(
            TypeSubstitution::new(arguments.to_vec())
                .apply(&mut self.program.interner, template)?,
        )
    }

    fn instantiate_types(
        &mut self,
        arguments: &[TypeId],
        templates: &[TypeId],
    ) -> Result<Vec<TypeId>, HirError> {
        let substitution = TypeSubstitution::new(arguments.to_vec());
        templates
            .iter()
            .map(|template| {
                substitution
                    .apply(&mut self.program.interner, *template)
                    .map_err(HirError::from)
            })
            .collect()
    }

    fn discard_status_with_generics(
        &mut self,
        root: TypeId,
        generic_statuses: &BTreeMap<u32, DiscardStatus>,
    ) -> Result<DiscardStatus, HirError> {
        let by_identity = self.local_nominal_symbols();
        if self.discard_summaries.is_none() {
            self.discard_summaries = Some(self.compute_discard_summaries(&by_identity)?);
        }
        let summaries = self
            .discard_summaries
            .as_ref()
            .expect("discard summaries were initialized");
        let mut nodes = BTreeMap::<TypeId, DiscardNode>::new();
        let mut pending = vec![root];
        while let Some(ty) = pending.pop() {
            if nodes.contains_key(&ty) {
                continue;
            }
            let mut node = self.discard_node(ty, &by_identity, summaries, generic_statuses)?;
            node.dependencies.sort_unstable();
            node.dependencies.dedup();
            pending.extend(node.dependencies.iter().copied());
            nodes.insert(ty, node);
        }

        let mut statuses = nodes
            .iter()
            .map(|(ty, node)| (*ty, node.floor))
            .collect::<BTreeMap<_, _>>();
        let mut users = nodes
            .keys()
            .copied()
            .map(|ty| (ty, Vec::new()))
            .collect::<BTreeMap<_, Vec<TypeId>>>();
        for (user, node) in &nodes {
            for dependency in &node.dependencies {
                users
                    .get_mut(dependency)
                    .expect("all discard dependencies are indexed")
                    .push(*user);
            }
        }
        let mut changed = statuses
            .iter()
            .filter_map(|(ty, status)| (*status != DiscardStatus::Satisfied).then_some(*ty))
            .collect::<BTreeSet<_>>();
        while let Some(dependency) = changed.pop_first() {
            for user in &users[&dependency] {
                let node = &nodes[user];
                let next = node
                    .dependencies
                    .iter()
                    .fold(node.floor, |status, dependency| {
                        status.max(statuses[dependency])
                    });
                let current = statuses
                    .get_mut(user)
                    .expect("all discard graph users have a status");
                if next > *current {
                    *current = next;
                    changed.insert(*user);
                }
            }
        }
        Ok(statuses[&root])
    }

    fn local_nominal_symbols(&self) -> BTreeMap<SymbolIdentity, SymbolId> {
        self.program
            .declarations
            .iter()
            .filter_map(|(symbol, declaration)| {
                matches!(declaration.kind(), HirTypeDeclarationKind::Nominal(_))
                    .then(|| {
                        self.resolved
                            .symbol(*symbol)
                            .map(|resolved| (resolved.identity().clone(), *symbol))
                    })
                    .flatten()
            })
            .collect()
    }

    fn compute_discard_summaries(
        &self,
        by_identity: &BTreeMap<SymbolIdentity, SymbolId>,
    ) -> Result<BTreeMap<SymbolId, DiscardRequirement>, HirError> {
        let roots = self
            .program
            .declarations
            .iter()
            .filter_map(|(symbol, declaration)| {
                let HirTypeDeclarationKind::Nominal(definition) = declaration.kind() else {
                    return None;
                };
                Some((*symbol, nominal_discard_roots(definition.shape())))
            })
            .collect::<BTreeMap<_, _>>();
        let mut summaries = roots
            .keys()
            .copied()
            .map(|symbol| (symbol, DiscardRequirement::default()))
            .collect::<BTreeMap<_, _>>();
        let mut users = roots
            .keys()
            .copied()
            .map(|symbol| (symbol, BTreeSet::new()))
            .collect::<BTreeMap<_, BTreeSet<SymbolId>>>();
        for (user, types) in &roots {
            for dependency in self.nominal_references(types, by_identity)? {
                users.entry(dependency).or_default().insert(*user);
            }
        }

        let mut pending = roots.keys().copied().collect::<BTreeSet<_>>();
        while let Some(symbol) = pending.pop_first() {
            let next = self.discard_requirement(&roots[&symbol], by_identity, &summaries)?;
            if summaries[&symbol] == next {
                continue;
            }
            summaries.insert(symbol, next);
            pending.extend(users.get(&symbol).into_iter().flatten().copied());
        }
        Ok(summaries)
    }

    fn nominal_references(
        &self,
        roots: &[TypeId],
        by_identity: &BTreeMap<SymbolIdentity, SymbolId>,
    ) -> Result<BTreeSet<SymbolId>, HirError> {
        let mut references = BTreeSet::new();
        let mut visited = BTreeSet::new();
        let mut pending = roots.to_vec();
        while let Some(ty) = pending.pop() {
            if !visited.insert(ty) {
                continue;
            }
            match self.program.interner.kind(ty)? {
                TypeKind::Nominal {
                    identity,
                    arguments,
                } => {
                    if let Some(symbol) = by_identity.get(identity) {
                        references.insert(*symbol);
                    }
                    pending.extend(arguments.iter().copied());
                }
                TypeKind::Tuple(items) | TypeKind::Union(items) => {
                    pending.extend(items.iter().copied());
                }
                TypeKind::Option(item) => pending.push(*item),
                TypeKind::Result { success, error } => {
                    pending.push(*success);
                    pending.push(*error);
                }
                TypeKind::Intrinsic { arguments, .. } | TypeKind::Generated { arguments, .. } => {
                    pending.extend(arguments.iter().copied());
                }
                TypeKind::Cursor { collection, .. } => pending.push(*collection),
                TypeKind::Error
                | TypeKind::Scalar(_)
                | TypeKind::Function(_)
                | TypeKind::GenericParameter(_)
                | TypeKind::Inference(_)
                | TypeKind::OpaqueResult(_) => {}
            }
        }
        Ok(references)
    }

    fn discard_requirement(
        &self,
        roots: &[TypeId],
        by_identity: &BTreeMap<SymbolIdentity, SymbolId>,
        summaries: &BTreeMap<SymbolId, DiscardRequirement>,
    ) -> Result<DiscardRequirement, HirError> {
        let mut requirement = DiscardRequirement::default();
        let mut visited = BTreeSet::new();
        let mut pending = roots.to_vec();
        while let Some(ty) = pending.pop() {
            if !visited.insert(ty) {
                continue;
            }
            match self.program.interner.kind(ty)? {
                TypeKind::Error | TypeKind::Scalar(_) | TypeKind::Function(_) => {}
                TypeKind::Tuple(items) | TypeKind::Union(items) => {
                    pending.extend(items.iter().copied());
                }
                TypeKind::Option(item) => pending.push(*item),
                TypeKind::Result { success, error } => {
                    pending.push(*success);
                    pending.push(*error);
                }
                TypeKind::Intrinsic {
                    constructor: IntrinsicType::Join,
                    ..
                } => requirement.floor = DiscardStatus::Unsatisfied,
                TypeKind::Intrinsic {
                    constructor:
                        IntrinsicType::Array
                        | IntrinsicType::Map
                        | IntrinsicType::Set
                        | IntrinsicType::Range,
                    arguments,
                } => pending.extend(arguments.iter().copied()),
                TypeKind::Intrinsic {
                    constructor:
                        IntrinsicType::Ref
                        | IntrinsicType::Pointer
                        | IntrinsicType::Command
                        | IntrinsicType::Pipeline
                        | IntrinsicType::NumericConversionError,
                    ..
                } => {}
                TypeKind::Nominal {
                    identity,
                    arguments,
                } => {
                    let Some(symbol) = by_identity.get(identity) else {
                        requirement.floor = requirement.floor.max(DiscardStatus::Deferred);
                        continue;
                    };
                    let summary = &summaries[symbol];
                    requirement.floor = requirement.floor.max(summary.floor);
                    for position in &summary.parameters {
                        if let Some(argument) = arguments.get(*position as usize) {
                            pending.push(*argument);
                        } else {
                            requirement.floor = requirement.floor.max(DiscardStatus::Deferred);
                        }
                    }
                }
                TypeKind::GenericParameter(position) => {
                    requirement.parameters.insert(*position);
                }
                TypeKind::Inference(_)
                | TypeKind::OpaqueResult(_)
                | TypeKind::Generated { .. }
                | TypeKind::Cursor { .. } => {
                    requirement.floor = requirement.floor.max(DiscardStatus::Deferred);
                }
            }
        }
        Ok(requirement)
    }

    fn discard_node(
        &self,
        ty: TypeId,
        by_identity: &BTreeMap<SymbolIdentity, SymbolId>,
        summaries: &BTreeMap<SymbolId, DiscardRequirement>,
        generic_statuses: &BTreeMap<u32, DiscardStatus>,
    ) -> Result<DiscardNode, HirError> {
        let satisfied = |dependencies| DiscardNode {
            floor: DiscardStatus::Satisfied,
            dependencies,
        };
        let terminal = || DiscardNode {
            floor: DiscardStatus::Unsatisfied,
            dependencies: Vec::new(),
        };
        let deferred = || DiscardNode {
            floor: DiscardStatus::Deferred,
            dependencies: Vec::new(),
        };

        let node = match self.program.interner.kind(ty)?.clone() {
            TypeKind::Error | TypeKind::Scalar(_) | TypeKind::Function(_) => satisfied(Vec::new()),
            TypeKind::Tuple(items) | TypeKind::Union(items) => satisfied(items),
            TypeKind::Option(item) => satisfied(vec![item]),
            TypeKind::Result { success, error } => satisfied(vec![success, error]),
            TypeKind::Intrinsic {
                constructor: IntrinsicType::Join,
                ..
            } => terminal(),
            TypeKind::Intrinsic {
                constructor:
                    IntrinsicType::Array
                    | IntrinsicType::Map
                    | IntrinsicType::Set
                    | IntrinsicType::Range,
                arguments,
            } => satisfied(arguments),
            TypeKind::Intrinsic {
                constructor:
                    IntrinsicType::Ref
                    | IntrinsicType::Pointer
                    | IntrinsicType::Command
                    | IntrinsicType::Pipeline
                    | IntrinsicType::NumericConversionError,
                ..
            } => satisfied(Vec::new()),
            TypeKind::Nominal {
                identity,
                arguments,
            } => {
                let Some(symbol) = by_identity.get(&identity) else {
                    return Ok(deferred());
                };
                let summary = &summaries[symbol];
                let mut dependencies = Vec::with_capacity(summary.parameters.len());
                for position in &summary.parameters {
                    let Some(argument) = arguments.get(*position as usize) else {
                        return Ok(deferred());
                    };
                    dependencies.push(*argument);
                }
                DiscardNode {
                    floor: summary.floor,
                    dependencies,
                }
            }
            TypeKind::GenericParameter(position) => DiscardNode {
                floor: generic_statuses
                    .get(&position)
                    .copied()
                    .unwrap_or(DiscardStatus::Deferred),
                dependencies: Vec::new(),
            },
            TypeKind::Inference(_)
            | TypeKind::OpaqueResult(_)
            | TypeKind::Generated { .. }
            | TypeKind::Cursor { .. } => deferred(),
        };
        Ok(node)
    }

    fn require_discard(&mut self, span: Span, ty: TypeId) -> Result<(), HirError> {
        self.require_discard_with_generics(span, ty, &BTreeMap::new(), "`_ =`")
    }

    fn require_discard_with_generics(
        &mut self,
        span: Span,
        ty: TypeId,
        generic_statuses: &BTreeMap<u32, DiscardStatus>,
        context: &str,
    ) -> Result<(), HirError> {
        match self.discard_status_with_generics(ty, generic_statuses)? {
            DiscardStatus::Satisfied => {}
            DiscardStatus::Deferred => self.complete = false,
            DiscardStatus::Unsatisfied => {
                let actual = self.program.interner.canonical(ty)?;
                self.emit(
                    span,
                    "E1105",
                    format!("type `{actual}` does not satisfy `Discard` required by {context}"),
                    Vec::new(),
                    Some(("Discard".to_owned(), actual)),
                )?;
            }
        }
        Ok(())
    }

    fn select_pattern_member(
        &self,
        expected: TypeId,
        predicate: impl Fn(TypeId) -> bool,
    ) -> Result<Option<TypeId>, HirError> {
        if predicate(expected) {
            return Ok(Some(expected));
        }
        let TypeKind::Union(members) = self.program.interner.kind(expected)? else {
            return Ok(None);
        };
        let mut candidates = members
            .iter()
            .copied()
            .filter(|candidate| predicate(*candidate));
        let first = candidates.next();
        Ok(first.filter(|_| candidates.next().is_none()))
    }

    fn select_pattern_member_checked(
        &self,
        expected: TypeId,
        predicate: impl Fn(TypeId) -> Result<bool, HirError>,
    ) -> Result<Option<TypeId>, HirError> {
        if predicate(expected)? {
            return Ok(Some(expected));
        }
        let TypeKind::Union(members) = self.program.interner.kind(expected)? else {
            return Ok(None);
        };
        let mut matches = Vec::new();
        for candidate in members {
            if predicate(*candidate)? {
                matches.push(*candidate);
            }
        }
        Ok(if matches.len() == 1 {
            Some(matches[0])
        } else {
            None
        })
    }

    fn wrap_union_pattern(
        &mut self,
        file: FileId,
        range: TextRange,
        expected: TypeId,
        member: TypeId,
        pattern: CheckedPattern,
    ) -> Result<CheckedPattern, HirError> {
        if expected == member {
            return Ok(pattern);
        }
        let id = self.allocate_pattern(HirPattern {
            span: self.sources.span(file, range)?,
            ty: expected,
            kind: HirPatternKind::UnionMember {
                member,
                pattern: pattern.id,
            },
        })?;
        Ok(CheckedPattern {
            id,
            shape: PatternShape::Constructor {
                key: PatternConstructor::Union(member),
                arguments: vec![pattern.shape],
            },
            valid: pattern.valid,
        })
    }

    fn recovery_pattern(
        &mut self,
        file: FileId,
        range: TextRange,
    ) -> Result<CheckedPattern, HirError> {
        let id = self.allocate_pattern(HirPattern {
            span: self.sources.span(file, range)?,
            ty: self.program.interner.error(),
            kind: HirPatternKind::Recovery,
        })?;
        Ok(CheckedPattern {
            id,
            shape: PatternShape::Wildcard,
            valid: false,
        })
    }

    fn emit_invalid_pattern(
        &mut self,
        file: FileId,
        range: TextRange,
        message: &str,
    ) -> Result<(), HirError> {
        self.emit(
            self.sources.span(file, range)?,
            "E1202",
            message,
            Vec::new(),
            None,
        )
    }

    fn emit_pattern_type_mismatch(
        &mut self,
        file: FileId,
        range: TextRange,
        subject: &str,
        expected: TypeId,
    ) -> Result<(), HirError> {
        self.emit(
            self.sources.span(file, range)?,
            "E1202",
            format!(
                "{subject} is incompatible with `{}`",
                self.program.interner.canonical(expected)?
            ),
            Vec::new(),
            None,
        )
    }

    fn pattern_is_irrefutable(
        &mut self,
        pattern: &PatternShape,
        ty: TypeId,
        span: Span,
    ) -> Result<bool, HirError> {
        Ok(!self.pattern_vector_is_useful(
            &[vec![pattern.clone()]],
            vec![PatternShape::Wildcard],
            vec![ty],
            span,
        )?)
    }

    fn pattern_vector_is_useful(
        &mut self,
        matrix: &[Vec<PatternShape>],
        candidate: Vec<PatternShape>,
        types: Vec<TypeId>,
        span: Span,
    ) -> Result<bool, HirError> {
        let mut pending = vec![UsefulnessState {
            matrix: matrix.to_vec(),
            candidate,
            types,
        }];
        while let Some(state) = pending.pop() {
            self.consume_pattern_analysis_work(span, &state)?;
            debug_assert_eq!(state.candidate.len(), state.types.len());
            if state.candidate.is_empty() {
                if state.matrix.is_empty() {
                    return Ok(true);
                }
                continue;
            }

            let first = normalize_pattern_head(&state.candidate[0]);
            let remaining_candidate = &state.candidate[1..];
            let ty = state.types[0];
            let remaining_types = &state.types[1..];
            match first {
                PatternShape::Constructor { key, arguments } => {
                    let argument_types = self.pattern_constructor_arguments(&key, ty)?;
                    let specialized =
                        specialize_pattern_matrix(&state.matrix, &key, argument_types.len());
                    let mut next_candidate = arguments;
                    next_candidate.extend_from_slice(remaining_candidate);
                    let mut next_types = argument_types;
                    next_types.extend_from_slice(remaining_types);
                    pending.push(UsefulnessState {
                        matrix: specialized,
                        candidate: next_candidate,
                        types: next_types,
                    });
                }
                PatternShape::Wildcard => {
                    let complete = self.complete_pattern_constructors(ty)?;
                    if let Some(constructors) = complete {
                        if constructors.is_empty() {
                            continue;
                        }
                        let present = state
                            .matrix
                            .iter()
                            .filter_map(|row| match row.first().map(normalize_pattern_head) {
                                Some(PatternShape::Constructor { key, .. }) => Some(key),
                                Some(PatternShape::Wildcard | PatternShape::Array { .. })
                                | None => None,
                            })
                            .collect::<BTreeSet<_>>();
                        if constructors.iter().all(|(key, _)| present.contains(key)) {
                            let mut branches = Vec::with_capacity(constructors.len());
                            for (key, argument_types) in constructors {
                                let specialized = specialize_pattern_matrix(
                                    &state.matrix,
                                    &key,
                                    argument_types.len(),
                                );
                                let mut next_candidate =
                                    vec![PatternShape::Wildcard; argument_types.len()];
                                next_candidate.extend_from_slice(remaining_candidate);
                                let mut next_types = argument_types;
                                next_types.extend_from_slice(remaining_types);
                                branches.push(UsefulnessState {
                                    matrix: specialized,
                                    candidate: next_candidate,
                                    types: next_types,
                                });
                            }
                            pending.extend(branches.into_iter().rev());
                            continue;
                        }
                    }
                    pending.push(UsefulnessState {
                        matrix: default_pattern_matrix(&state.matrix),
                        candidate: remaining_candidate.to_vec(),
                        types: remaining_types.to_vec(),
                    });
                }
                PatternShape::Array { .. } => {
                    unreachable!("array pattern heads normalize to list constructors")
                }
            }
        }
        Ok(false)
    }

    fn consume_pattern_analysis_work(
        &mut self,
        span: Span,
        state: &UsefulnessState,
    ) -> Result<(), HirError> {
        let matrix_cells = state
            .matrix
            .iter()
            .try_fold(0_u64, |total, row| total.checked_add(row.len() as u64))
            .unwrap_or(u64::MAX);
        let work = matrix_cells
            .saturating_add(state.candidate.len() as u64)
            .saturating_add(1);
        if work > self.pattern_steps_remaining {
            return Err(HirError::PatternAnalysisLimit {
                file: span.file(),
                offset: span.range().start(),
            });
        }
        self.pattern_steps_remaining -= work;
        Ok(())
    }

    fn pattern_constructor_arguments(
        &mut self,
        key: &PatternConstructor,
        ty: TypeId,
    ) -> Result<Vec<TypeId>, HirError> {
        if let Some(constructors) = self.complete_pattern_constructors(ty)?
            && let Some((_, arguments)) = constructors
                .into_iter()
                .find(|(candidate, _)| candidate == key)
        {
            return Ok(arguments);
        }
        Ok(Vec::new())
    }

    fn complete_pattern_constructors(
        &mut self,
        ty: TypeId,
    ) -> Result<Option<CompletePatternConstructors>, HirError> {
        Ok(Some(match self.program.interner.kind(ty)?.clone() {
            TypeKind::Scalar(ScalarType::Never) => Vec::new(),
            TypeKind::Scalar(ScalarType::Unit) => {
                vec![(PatternConstructor::Unit, Vec::new())]
            }
            TypeKind::Scalar(ScalarType::Bool) => vec![
                (PatternConstructor::Bool(false), Vec::new()),
                (PatternConstructor::Bool(true), Vec::new()),
            ],
            TypeKind::Option(item) => vec![
                (PatternConstructor::OptionSome, vec![item]),
                (PatternConstructor::OptionNone, Vec::new()),
            ],
            TypeKind::Result { success, error } => vec![
                (PatternConstructor::ResultOk, vec![success]),
                (PatternConstructor::ResultErr, vec![error]),
            ],
            TypeKind::Tuple(items) => {
                vec![(PatternConstructor::Tuple(items.len()), items)]
            }
            TypeKind::Intrinsic {
                constructor: IntrinsicType::Array,
                arguments,
            } => vec![
                (PatternConstructor::ArrayEmpty, Vec::new()),
                (PatternConstructor::ArrayCons, vec![arguments[0], ty]),
            ],
            TypeKind::Union(members) => members
                .into_iter()
                .map(|member| (PatternConstructor::Union(member), vec![member]))
                .collect(),
            TypeKind::Nominal { .. } => {
                let Some((symbol, arguments, shape)) = self.nominal_instance(ty)? else {
                    return Ok(None);
                };
                match shape {
                    HirNominalShape::Newtype { underlying } => vec![(
                        PatternConstructor::Newtype(symbol),
                        vec![self.instantiate_type(&arguments, underlying)?],
                    )],
                    HirNominalShape::Record { fields } => {
                        let templates = fields.iter().map(HirField::ty).collect::<Vec<_>>();
                        vec![(
                            PatternConstructor::Record(symbol),
                            self.instantiate_types(&arguments, &templates)?,
                        )]
                    }
                    HirNominalShape::Enum { variants } => {
                        let mut constructors = Vec::with_capacity(variants.len());
                        for variant in variants {
                            let templates = match variant.payload() {
                                HirVariantPayload::Unit => Vec::new(),
                                HirVariantPayload::Tuple(types) => types.clone(),
                                HirVariantPayload::Record(fields) => {
                                    fields.iter().map(HirField::ty).collect()
                                }
                            };
                            constructors.push((
                                PatternConstructor::Variant(variant.member()),
                                self.instantiate_types(&arguments, &templates)?,
                            ));
                        }
                        constructors
                    }
                }
            }
            TypeKind::Error => return Ok(None),
            _ => return Ok(None),
        }))
    }

    fn check_control_transfer(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        context: &mut BodyContext,
    ) -> Result<HirExpressionId, HirError> {
        let never = self.program.interner.scalar(ScalarType::Never);
        let kind = match node.kind() {
            SyntaxKind::ReturnStmt => {
                let expression = node
                    .child_nodes()
                    .find(|child| AstExpression::cast(*child).is_some());
                let callable = context.callable;
                let value = match (expression, callable) {
                    (Some(expression), Some(callable)) => Some(self.check_expression(
                        file,
                        expression,
                        Some(callable.expectation()),
                        context,
                    )?),
                    (None, Some(callable))
                        if callable.success == self.program.interner.scalar(ScalarType::Unit) =>
                    {
                        None
                    }
                    (None, Some(callable)) => {
                        self.emit(
                            self.sources.span(file, node.range())?,
                            "E1205",
                            format!(
                                "this return must produce `{}`",
                                self.program.interner.canonical(callable.success)?
                            ),
                            Vec::new(),
                            None,
                        )?;
                        None
                    }
                    (_, None) => {
                        self.emit(
                            self.sources.span(file, node.range())?,
                            "E1205",
                            "`return` has no enclosing callable",
                            Vec::new(),
                            None,
                        )?;
                        None
                    }
                };
                HirExpressionKind::Return { value }
            }
            SyntaxKind::BreakStmt => {
                let target = context.loops.last().copied();
                if target.is_none() {
                    self.emit(
                        self.sources.span(file, node.range())?,
                        "E1205",
                        "`break` has no enclosing loop",
                        Vec::new(),
                        None,
                    )?;
                }
                HirExpressionKind::Break { target }
            }
            SyntaxKind::ContinueStmt => {
                let target = context.loops.last().copied();
                if target.is_none() {
                    self.emit(
                        self.sources.span(file, node.range())?,
                        "E1205",
                        "`continue` has no enclosing loop",
                        Vec::new(),
                        None,
                    )?;
                }
                HirExpressionKind::Continue { target }
            }
            _ => unreachable!("control transfer selection is closed"),
        };
        self.allocate_expression(HirExpression {
            span: self.sources.span(file, node.range())?,
            ty: never,
            category: HirValueCategory::Value,
            kind,
        })
    }

    fn check_fail(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        context: &mut BodyContext,
    ) -> Result<HirExpressionId, HirError> {
        let Some(error_node) = node
            .child_nodes()
            .find(|child| AstExpression::cast(*child).is_some())
        else {
            return self.recovery_expression(file, node.range());
        };
        let error = if let Some(callable) = context.callable
            && let Some(expected_error) = callable.error
        {
            self.check_error_with_expected_diagnostic(
                file,
                error_node,
                expected_error,
                context,
                "E1302",
                "`fail` error",
            )?
        } else {
            let error = self.check_expression(file, error_node, None, context)?;
            self.emit(
                self.sources.span(file, node.range())?,
                "E1302",
                "`fail` requires an enclosing callable with a direct error channel",
                context
                    .callable
                    .map(|callable| {
                        vec![(
                            "the enclosing callable is declared here",
                            callable.signature,
                        )]
                    })
                    .unwrap_or_default(),
                None,
            )?;
            error
        };
        self.allocate_expression(HirExpression {
            span: self.sources.span(file, node.range())?,
            ty: self.program.interner.scalar(ScalarType::Never),
            category: HirValueCategory::Value,
            kind: HirExpressionKind::Fail { error },
        })
    }

    fn check_for(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        context: &mut BodyContext,
    ) -> Result<HirStatement, HirError> {
        let header = node
            .child_nodes()
            .find(|child| child.kind() == SyntaxKind::ForHeader)
            .expect("parsed for statements have a header");
        let body_node = node
            .child_nodes()
            .find(|child| child.kind() == SyntaxKind::Block)
            .expect("parsed for statements have a body");
        let pattern_node = header
            .child_nodes()
            .find(|child| AstPattern::cast(*child).is_some());
        let source_node = header
            .child_nodes()
            .find(|child| AstExpression::cast(*child).is_some());
        let id = HirLoopId(self.next_loop_id);
        self.next_loop_id = self
            .next_loop_id
            .checked_add(1)
            .ok_or(HirError::NodeLimit {
                file,
                offset: node.range().start(),
            })?;
        let mut body_context = context.clone();
        body_context.loops.push(id);
        let kind = match (pattern_node, source_node) {
            (None, None) => HirForKind::Infinite,
            (None, Some(condition)) => {
                let condition = self.check_expression(
                    file,
                    condition,
                    Some(ExpressionExpectation::Direct(
                        self.program.interner.scalar(ScalarType::Bool),
                    )),
                    context,
                )?;
                HirForKind::Conditional { condition }
            }
            (Some(pattern_node), Some(source_node)) => {
                let source = self.check_expression(file, source_node, None, context)?;
                let source_type = self.expression_type(source);
                let element = self.iteration_element_type(source_type)?;
                let element = if let Some(element) = element {
                    element
                } else if self.iteration_requires_trait_resolution(source_type)? {
                    self.complete = false;
                    self.program.interner.error()
                } else {
                    if source_type != self.program.interner.error() {
                        self.emit(
                            self.sources.span(file, source_node.range())?,
                            "E1206",
                            format!(
                                "`{}` is not an intrinsic iterable source",
                                self.program.interner.canonical(source_type)?
                            ),
                            Vec::new(),
                            None,
                        )?;
                    }
                    self.program.interner.error()
                };
                let pattern = self.check_binding_pattern(
                    file,
                    pattern_node,
                    element,
                    &mut body_context,
                    PatternContext::For,
                )?;
                HirForKind::Iterate { pattern, source }
            }
            (Some(_), None) => {
                self.complete = false;
                HirForKind::Infinite
            }
        };
        let body = self.check_expression(
            file,
            body_node,
            Some(ExpressionExpectation::Direct(
                self.program.interner.scalar(ScalarType::Unit),
            )),
            &mut body_context,
        )?;
        Ok(HirStatement::For {
            span: self.sources.span(file, node.range())?,
            id,
            kind,
            body,
        })
    }

    fn iteration_element_type(&mut self, source: TypeId) -> Result<Option<TypeId>, HirError> {
        let kind = self.program.interner.kind(source)?.clone();
        let element = match kind {
            TypeKind::Intrinsic {
                constructor: IntrinsicType::Array | IntrinsicType::Set | IntrinsicType::Range,
                arguments,
            } => Some(arguments[0]),
            TypeKind::Intrinsic {
                constructor: IntrinsicType::Map,
                arguments,
            } => Some(self.program.interner.tuple(arguments)?),
            TypeKind::Scalar(ScalarType::String) => {
                Some(self.program.interner.scalar(ScalarType::Char))
            }
            TypeKind::Error => Some(self.program.interner.error()),
            _ => None,
        };
        Ok(element)
    }

    fn iteration_requires_trait_resolution(&self, source: TypeId) -> Result<bool, HirError> {
        Ok(matches!(
            self.program.interner.kind(source)?,
            TypeKind::Nominal { .. }
                | TypeKind::GenericParameter(_)
                | TypeKind::OpaqueResult(_)
                | TypeKind::Generated { .. }
                | TypeKind::Cursor { .. }
        ))
    }

    fn check_if(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        expected: Option<ExpressionExpectation>,
        context: &mut BodyContext,
    ) -> Result<HirExpressionId, HirError> {
        let children = node.child_nodes().collect::<Vec<_>>();
        let Some(condition_node) = children
            .iter()
            .copied()
            .find(|child| {
                AstExpression::cast(*child).is_some() && child.kind() != SyntaxKind::IfExpr
            })
            .or_else(|| {
                children
                    .iter()
                    .copied()
                    .find(|child| AstExpression::cast(*child).is_some())
            })
        else {
            return self.recovery_expression(file, node.range());
        };
        let Some(then_node) = children
            .iter()
            .copied()
            .find(|child| child.kind() == SyntaxKind::Block)
        else {
            return self.recovery_expression(file, node.range());
        };
        let then_position = children
            .iter()
            .position(|child| *child == then_node)
            .expect("the selected then block is a direct child");
        let else_node = children
            .iter()
            .copied()
            .skip(then_position + 1)
            .find(|child| matches!(child.kind(), SyntaxKind::Block | SyntaxKind::IfExpr));
        let condition = self.check_expression(
            file,
            condition_node,
            Some(ExpressionExpectation::Direct(
                self.program.interner.scalar(ScalarType::Bool),
            )),
            context,
        )?;
        let branch_expected = if else_node.is_some() {
            expected
        } else {
            Some(ExpressionExpectation::Direct(
                self.program.interner.scalar(ScalarType::Unit),
            ))
        };
        let mut then_context = context.clone();
        let mut then_branch =
            self.check_expression(file, then_node, branch_expected, &mut then_context)?;
        let mut else_branch = if let Some(else_node) = else_node {
            let mut else_context = context.clone();
            Some(self.check_expression(file, else_node, expected, &mut else_context)?)
        } else {
            None
        };
        let then_type = self.expression_type(then_branch);
        let condition_diverges = !self.expression_flow(condition).may_complete();
        let branches_diverge = else_branch.is_some_and(|else_branch| {
            !self.expression_flow(then_branch).may_complete()
                && !self.expression_flow(else_branch).may_complete()
        });
        let ty = if condition_diverges || branches_diverge {
            self.program.interner.scalar(ScalarType::Never)
        } else if let Some(current_else_branch) = else_branch {
            let else_type = self.expression_type(current_else_branch);
            if then_type == self.program.interner.error()
                || else_type == self.program.interner.error()
            {
                self.program.interner.error()
            } else if let Some(expected) = expected {
                expected.resulting_type()
            } else if then_type == else_type {
                then_type
            } else if then_type == self.program.interner.scalar(ScalarType::Never) {
                else_type
            } else if else_type == self.program.interner.scalar(ScalarType::Never) {
                then_type
            } else if self
                .program
                .interner
                .assignability(then_type, else_type)?
                .is_some()
            {
                then_branch = self.coerce_existing(then_branch, else_type)?;
                else_type
            } else if self
                .program
                .interner
                .assignability(else_type, then_type)?
                .is_some()
            {
                else_branch = Some(self.coerce_existing(current_else_branch, then_type)?);
                then_type
            } else {
                self.emit(
                    self.sources.span(file, node.range())?,
                    "E1101",
                    format!(
                        "if branches infer incompatible types `{}` and `{}`",
                        self.program.interner.canonical(then_type)?,
                        self.program.interner.canonical(else_type)?
                    ),
                    Vec::new(),
                    None,
                )?;
                self.program.interner.error()
            }
        } else {
            self.program.interner.scalar(ScalarType::Unit)
        };
        self.allocate_expression(HirExpression {
            span: self.sources.span(file, node.range())?,
            ty,
            category: HirValueCategory::Value,
            kind: HirExpressionKind::If {
                condition,
                then_branch,
                else_branch,
            },
        })
    }

    fn check_match(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        expected: Option<ExpressionExpectation>,
        context: &mut BodyContext,
    ) -> Result<HirExpressionId, HirError> {
        let Some(scrutinee_node) = node
            .child_nodes()
            .find(|child| AstExpression::cast(*child).is_some())
        else {
            return self.recovery_expression(file, node.range());
        };
        let scrutinee = self.check_expression(file, scrutinee_node, None, context)?;
        let scrutinee_type = self.expression_type(scrutinee);
        let mut coverage = Vec::<Vec<PatternShape>>::new();
        let mut arms = Vec::new();
        let mut coverage_valid = scrutinee_type != self.program.interner.error();

        for arm_node in node
            .child_nodes()
            .filter(|child| child.kind() == SyntaxKind::MatchArm)
        {
            let Some(pattern_node) = arm_node
                .child_nodes()
                .find(|child| AstPattern::cast(*child).is_some())
            else {
                coverage_valid = false;
                continue;
            };
            let mut arm_context = context.clone();
            let pattern = self.check_pattern(
                file,
                pattern_node,
                scrutinee_type,
                &mut arm_context,
                PatternContext::Match,
            )?;
            coverage_valid &= pattern.valid;
            let has_guard = arm_node
                .child_tokens()
                .any(|token| token.kind() == TokenKind::If);
            if pattern.valid {
                let useful = self.pattern_vector_is_useful(
                    &coverage,
                    vec![pattern.shape.clone()],
                    vec![scrutinee_type],
                    self.sources.span(file, pattern_node.range())?,
                )?;
                if !useful {
                    self.emit(
                        self.sources.span(file, pattern_node.range())?,
                        "E1203",
                        "this match arm is completely covered by previous unguarded arms",
                        Vec::new(),
                        None,
                    )?;
                }
                if !has_guard {
                    coverage.push(vec![pattern.shape.clone()]);
                }
            }

            let expression_nodes = arm_node
                .child_nodes()
                .filter(|child| AstExpression::cast(*child).is_some())
                .collect::<Vec<_>>();
            let (guard_node, body_node) = if has_guard {
                (
                    expression_nodes.first().copied(),
                    expression_nodes.get(1).copied(),
                )
            } else {
                (None, expression_nodes.first().copied())
            };
            let guard = if let Some(guard_node) = guard_node {
                Some(self.check_expression(
                    file,
                    guard_node,
                    Some(ExpressionExpectation::Direct(
                        self.program.interner.scalar(ScalarType::Bool),
                    )),
                    &mut arm_context,
                )?)
            } else {
                None
            };
            let body = if let Some(body_node) = body_node {
                self.check_expression(file, body_node, expected, &mut arm_context)?
            } else if let Some(transfer) = arm_node.child_nodes().find(|child| {
                matches!(
                    child.kind(),
                    SyntaxKind::ReturnStmt
                        | SyntaxKind::FailStmt
                        | SyntaxKind::BreakStmt
                        | SyntaxKind::ContinueStmt
                )
            }) {
                if transfer.kind() == SyntaxKind::FailStmt {
                    self.check_fail(file, transfer, &mut arm_context)?
                } else {
                    self.check_control_transfer(file, transfer, &mut arm_context)?
                }
            } else {
                self.complete = false;
                self.recovery_expression(file, arm_node.range())?
            };
            arms.push(HirMatchArm {
                pattern: pattern.id,
                guard,
                body,
            });
        }

        if coverage_valid
            && self.pattern_vector_is_useful(
                &coverage,
                vec![PatternShape::Wildcard],
                vec![scrutinee_type],
                self.sources.span(file, node.range())?,
            )?
        {
            self.emit(
                self.sources.span(file, node.range())?,
                "E1204",
                format!(
                    "match is not exhaustive for `{}`",
                    self.program.interner.canonical(scrutinee_type)?
                ),
                Vec::new(),
                None,
            )?;
        }

        let diverges = !self.expression_flow(scrutinee).may_complete()
            || !arms.iter().any(|arm| {
                arm.guard
                    .is_none_or(|guard| self.expression_flow(guard).may_complete())
                    && self.expression_flow(arm.body).may_complete()
            });
        let ty = if diverges {
            self.program.interner.scalar(ScalarType::Never)
        } else if let Some(expected) = expected {
            expected.resulting_type()
        } else {
            self.join_match_arm_types(file, node.range(), &mut arms)?
        };
        self.allocate_expression(HirExpression {
            span: self.sources.span(file, node.range())?,
            ty,
            category: HirValueCategory::Value,
            kind: HirExpressionKind::Match { scrutinee, arms },
        })
    }

    fn join_match_arm_types(
        &mut self,
        file: FileId,
        range: TextRange,
        arms: &mut [HirMatchArm],
    ) -> Result<TypeId, HirError> {
        let never = self.program.interner.scalar(ScalarType::Never);
        let error = self.program.interner.error();
        let mut joined = None;
        for index in 0..arms.len() {
            let ty = self.expression_type(arms[index].body);
            if ty == error {
                return Ok(error);
            }
            if ty == never {
                continue;
            }
            let Some(current) = joined else {
                joined = Some(ty);
                continue;
            };
            if ty == current {
                continue;
            }
            if self.program.interner.assignability(ty, current)?.is_some() {
                arms[index].body = self.coerce_existing(arms[index].body, current)?;
                continue;
            }
            if self.program.interner.assignability(current, ty)?.is_some() {
                for previous in &mut arms[..index] {
                    let previous_type = self.expression_type(previous.body);
                    if previous_type != never {
                        previous.body = self.coerce_existing(previous.body, ty)?;
                    }
                }
                joined = Some(ty);
                continue;
            }
            self.emit(
                self.sources.span(file, range)?,
                "E1101",
                format!(
                    "match arms infer incompatible types `{}` and `{}`",
                    self.program.interner.canonical(current)?,
                    self.program.interner.canonical(ty)?
                ),
                Vec::new(),
                None,
            )?;
            return Ok(error);
        }
        Ok(joined.unwrap_or(never))
    }

    fn coerce_existing(
        &mut self,
        value: HirExpressionId,
        expected: TypeId,
    ) -> Result<HirExpressionId, HirError> {
        let actual = self.expression_type(value);
        let assignability = self
            .program
            .interner
            .assignability(actual, expected)?
            .expect("branch joining only coerces a proven assignable value");
        if assignability == Assignability::Exact {
            return Ok(value);
        }
        let span = self.program.expressions[value.0 as usize].span;
        self.allocate_expression(HirExpression {
            span,
            ty: expected,
            category: HirValueCategory::Value,
            kind: HirExpressionKind::Coerce {
                kind: assignability,
                value,
            },
        })
    }

    fn check_prefix(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        expected: Option<ExpressionExpectation>,
        context: &mut BodyContext,
    ) -> Result<HirExpressionId, HirError> {
        let contextual = expected.map(ExpressionExpectation::contextual_type);
        let Some(operator_token) = node.child_tokens().find(|token| {
            matches!(
                token.kind(),
                TokenKind::Minus | TokenKind::Not | TokenKind::Tilde
            )
        }) else {
            return self.recovery_expression(file, node.range());
        };
        let Some(operand_node) = node
            .child_nodes()
            .find(|child| AstExpression::cast(*child).is_some())
        else {
            return self.recovery_expression(file, node.range());
        };
        if operator_token.kind() == TokenKind::Minus
            && operand_node.kind() == SyntaxKind::LiteralExpr
            && let Some(integer) = operand_node
                .descendant_tokens()
                .find(|token| token.kind() == TokenKind::IntegerLiteral)
        {
            return self.check_negative_integer(file, node, operand_node, integer, contextual);
        }
        let (operator, operand_expected) = match operator_token.kind() {
            TokenKind::Minus => (HirPrefixOperator::Negate, contextual),
            TokenKind::Not => (
                HirPrefixOperator::LogicalNot,
                Some(self.program.interner.scalar(ScalarType::Bool)),
            ),
            TokenKind::Tilde => (HirPrefixOperator::BitwiseNot, contextual),
            _ => unreachable!("prefix token selection is closed"),
        };
        let operand = self.check_expression(
            file,
            operand_node,
            operand_expected.map(ExpressionExpectation::Direct),
            context,
        )?;
        let operand_type = self.expression_type(operand);
        if operand_type == self.program.interner.error() {
            return self.recovery_expression(file, node.range());
        }
        let valid = match (operator, self.program.interner.kind(operand_type)?) {
            (HirPrefixOperator::LogicalNot, TypeKind::Scalar(ScalarType::Bool)) => true,
            (HirPrefixOperator::Negate, TypeKind::Scalar(scalar)) => {
                is_signed_integer_scalar(*scalar) || is_float_scalar(*scalar)
            }
            (HirPrefixOperator::BitwiseNot, TypeKind::Scalar(scalar)) => {
                is_integer_scalar(*scalar) || *scalar == ScalarType::Byte
            }
            _ => false,
        };
        if !valid {
            self.emit_invalid_operator(file, operator_token.range(), operand_type, None)?;
            return self.recovery_expression(file, node.range());
        }
        let ty = if operator == HirPrefixOperator::LogicalNot {
            self.program.interner.scalar(ScalarType::Bool)
        } else {
            operand_type
        };
        self.allocate_expression(HirExpression {
            span: self.sources.span(file, node.range())?,
            ty,
            category: HirValueCategory::Value,
            kind: HirExpressionKind::Prefix { operator, operand },
        })
    }

    fn check_binary(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        expected: Option<ExpressionExpectation>,
        context: &mut BodyContext,
    ) -> Result<HirExpressionId, HirError> {
        if let Some(update) = node
            .child_nodes()
            .find(|child| child.kind() == SyntaxKind::RecordUpdateBody)
        {
            return self.check_record_update(file, node, update, expected, context);
        }
        let contextual = expected.map(ExpressionExpectation::contextual_type);
        let operands = node
            .child_nodes()
            .filter(|child| AstExpression::cast(*child).is_some())
            .collect::<Vec<_>>();
        if operands.len() != 2 {
            return self.recovery_expression(file, node.range());
        }
        if let Some(operator) = node.child_tokens().find(|token| {
            matches!(
                token.kind(),
                TokenKind::DotDot | TokenKind::DotDotEq | TokenKind::In
            )
        }) {
            return match operator.kind() {
                TokenKind::DotDot | TokenKind::DotDotEq => self.check_range_expression(
                    file,
                    node.range(),
                    operands[0],
                    operands[1],
                    operator,
                    expected,
                    context,
                ),
                TokenKind::In => self.check_contains_expression(
                    file,
                    node.range(),
                    operands[0],
                    operands[1],
                    operator,
                    context,
                ),
                _ => unreachable!("the special binary token filter is closed"),
            };
        }
        let Some((operator_token, operator)) = node
            .child_tokens()
            .find_map(|token| binary_operator(token.kind()).map(|operator| (token, operator)))
        else {
            self.complete = false;
            return self.recovery_expression(file, node.range());
        };
        let bool_type = self.program.interner.scalar(ScalarType::Bool);
        let array_context = contextual.is_some_and(|ty| {
            self.program.interner.kind(ty).is_ok_and(is_array_type)
                && matches!(
                    operator,
                    HirBinaryOperator::Add
                        | HirBinaryOperator::Subtract
                        | HirBinaryOperator::Multiply
                        | HirBinaryOperator::Divide
                        | HirBinaryOperator::Remainder
                )
        });
        let (left_expected, right_from_left) = match operator {
            HirBinaryOperator::LogicalAnd | HirBinaryOperator::LogicalOr => {
                (Some(bool_type), false)
            }
            HirBinaryOperator::ShiftLeft | HirBinaryOperator::ShiftRight => (contextual, false),
            HirBinaryOperator::Less
            | HirBinaryOperator::LessEqual
            | HirBinaryOperator::Greater
            | HirBinaryOperator::GreaterEqual
            | HirBinaryOperator::Equal
            | HirBinaryOperator::NotEqual => (None, true),
            _ if array_context => (None, false),
            _ => (contextual, true),
        };
        let left = self.check_expression(
            file,
            operands[0],
            left_expected.map(ExpressionExpectation::Direct),
            context,
        )?;
        let left_type = self.expression_type(left);
        let lifted_array_candidate = matches!(
            operator,
            HirBinaryOperator::Add
                | HirBinaryOperator::Subtract
                | HirBinaryOperator::Multiply
                | HirBinaryOperator::Divide
                | HirBinaryOperator::Remainder
        ) && (self
            .program
            .interner
            .kind(left_type)
            .is_ok_and(is_array_type)
            || operands[1].kind() == SyntaxKind::BracketLiteralExpr);
        let right_expected = if array_context || lifted_array_candidate {
            None
        } else if right_from_left {
            Some(left_type)
        } else if matches!(
            operator,
            HirBinaryOperator::ShiftLeft | HirBinaryOperator::ShiftRight
        ) {
            None
        } else {
            left_expected
        };
        let right = self.check_expression(
            file,
            operands[1],
            right_expected.map(ExpressionExpectation::Direct),
            context,
        )?;
        let right_type = self.expression_type(right);
        if left_type == self.program.interner.error() || right_type == self.program.interner.error()
        {
            return self.recovery_expression(file, node.range());
        }
        let result = self.binary_result(operator, left_type, right_type)?;
        let Some(ty) = result else {
            self.emit_invalid_operator(file, operator_token.range(), left_type, Some(right_type))?;
            return self.recovery_expression(file, node.range());
        };
        self.allocate_expression(HirExpression {
            span: self.sources.span(file, node.range())?,
            ty,
            category: HirValueCategory::Value,
            kind: HirExpressionKind::Binary {
                operator,
                left,
                right,
            },
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn check_range_expression(
        &mut self,
        file: FileId,
        range: TextRange,
        start_node: SyntaxNodeRef<'_>,
        end_node: SyntaxNodeRef<'_>,
        operator: SyntaxTokenRef<'_>,
        expected: Option<ExpressionExpectation>,
        context: &mut BodyContext,
    ) -> Result<HirExpressionId, HirError> {
        let contextual_element = expected
            .map(ExpressionExpectation::contextual_type)
            .map(|expected| {
                self.select_pattern_member(expected, |candidate| {
                    matches!(
                        self.program.interner.kind(candidate),
                        Ok(TypeKind::Intrinsic {
                            constructor: IntrinsicType::Range,
                            arguments,
                        }) if arguments.len() == 1
                    )
                })
            })
            .transpose()?
            .flatten()
            .and_then(|range| match self.program.interner.kind(range) {
                Ok(TypeKind::Intrinsic { arguments, .. }) => arguments.first().copied(),
                _ => None,
            });
        let start = self.check_expression(
            file,
            start_node,
            contextual_element.map(ExpressionExpectation::Direct),
            context,
        )?;
        let start_type = self.expression_type(start);
        let end = self.check_expression(
            file,
            end_node,
            (start_type != self.program.interner.error())
                .then_some(ExpressionExpectation::Direct(start_type)),
            context,
        )?;
        let end_type = self.expression_type(end);
        if start_type == self.program.interner.error() || end_type == self.program.interner.error()
        {
            return self.recovery_expression(file, range);
        }
        let discrete = matches!(
            self.program.interner.kind(start_type)?,
            TypeKind::Scalar(scalar)
                if is_integer_scalar(*scalar) || *scalar == ScalarType::Char
        );
        if start_type != end_type || !discrete {
            self.emit(
                self.sources.span(file, operator.range())?,
                "E1102",
                "range endpoints must have one identical discrete integer or `Char` type",
                Vec::new(),
                None,
            )?;
            return self.recovery_expression(file, range);
        }
        let ty = self
            .program
            .interner
            .intrinsic(IntrinsicType::Range, vec![start_type])?;
        self.allocate_expression(HirExpression {
            span: self.sources.span(file, range)?,
            ty,
            category: HirValueCategory::Value,
            kind: HirExpressionKind::Range {
                kind: if operator.kind() == TokenKind::DotDotEq {
                    HirRangeKind::Inclusive
                } else {
                    HirRangeKind::Exclusive
                },
                start,
                end,
            },
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn check_contains_expression(
        &mut self,
        file: FileId,
        range: TextRange,
        item_node: SyntaxNodeRef<'_>,
        container_node: SyntaxNodeRef<'_>,
        operator: SyntaxTokenRef<'_>,
        context: &mut BodyContext,
    ) -> Result<HirExpressionId, HirError> {
        let empty_contextual_constructor = match container_node.kind() {
            SyntaxKind::BracketLiteralExpr
                if !container_node
                    .child_nodes()
                    .any(|child| AstExpression::cast(child).is_some())
                    && !container_node
                        .child_tokens()
                        .any(|token| token.kind() == TokenKind::Colon) =>
            {
                Some(IntrinsicType::Array)
            }
            SyntaxKind::SetLiteralExpr
                if !container_node
                    .child_nodes()
                    .any(|child| AstExpression::cast(child).is_some()) =>
            {
                Some(IntrinsicType::Set)
            }
            _ => None,
        };
        let (item, container) = if let Some(constructor) = empty_contextual_constructor {
            let item = self.check_expression(file, item_node, None, context)?;
            let item_type = self.expression_type(item);
            let expected_container = (item_type != self.program.interner.error())
                .then(|| {
                    self.program
                        .interner
                        .intrinsic(constructor, vec![item_type])
                })
                .transpose()?;
            let container = self.check_expression(
                file,
                container_node,
                expected_container.map(ExpressionExpectation::Direct),
                context,
            )?;
            (item, container)
        } else {
            let container = self.check_expression(file, container_node, None, context)?;
            let container_type = self.expression_type(container);
            let expected_item = self
                .containment_shape(container_type)?
                .map(|(_, item)| item);
            let item = self.check_expression(
                file,
                item_node,
                expected_item.map(ExpressionExpectation::Direct),
                context,
            )?;
            (item, container)
        };
        let item_type = self.expression_type(item);
        let container_type = self.expression_type(container);
        if item_type == self.program.interner.error()
            || container_type == self.program.interner.error()
        {
            return self.recovery_expression(file, range);
        }
        let Some((kind, expected_item)) = self.containment_shape(container_type)? else {
            self.emit(
                self.sources.span(file, operator.range())?,
                "E1102",
                "the right operand of `in` must be an array, map, set, range, or string",
                Vec::new(),
                None,
            )?;
            return self.recovery_expression(file, range);
        };
        if self
            .program
            .interner
            .assignability(item_type, expected_item)?
            .is_none()
        {
            self.emit(
                self.sources.span(file, item_node.range())?,
                "E1102",
                format!(
                    "membership expects `{}`, found `{}`",
                    self.program.interner.canonical(expected_item)?,
                    self.program.interner.canonical(item_type)?
                ),
                Vec::new(),
                None,
            )?;
            return self.recovery_expression(file, range);
        }
        if !matches!(
            self.program.interner.kind(expected_item)?,
            TypeKind::Scalar(_)
        ) {
            self.complete = false;
        }
        self.allocate_expression(HirExpression {
            span: self.sources.span(file, range)?,
            ty: self.program.interner.scalar(ScalarType::Bool),
            category: HirValueCategory::Value,
            kind: HirExpressionKind::Contains {
                kind,
                item,
                container,
            },
        })
    }

    fn containment_shape(
        &self,
        container: TypeId,
    ) -> Result<Option<(HirContainmentKind, TypeId)>, HirError> {
        let shape = match self.program.interner.kind(container)? {
            TypeKind::Intrinsic {
                constructor: IntrinsicType::Array,
                arguments,
            } => Some((HirContainmentKind::Array, arguments[0])),
            TypeKind::Intrinsic {
                constructor: IntrinsicType::Map,
                arguments,
            } => Some((HirContainmentKind::MapKey, arguments[0])),
            TypeKind::Intrinsic {
                constructor: IntrinsicType::Set,
                arguments,
            } => Some((HirContainmentKind::Set, arguments[0])),
            TypeKind::Intrinsic {
                constructor: IntrinsicType::Range,
                arguments,
            } => Some((HirContainmentKind::Range, arguments[0])),
            TypeKind::Scalar(ScalarType::String) => Some((
                HirContainmentKind::StringChar,
                self.program.interner.scalar(ScalarType::Char),
            )),
            _ => None,
        };
        Ok(shape)
    }

    fn check_record_update(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        update: SyntaxNodeRef<'_>,
        _expected: Option<ExpressionExpectation>,
        context: &mut BodyContext,
    ) -> Result<HirExpressionId, HirError> {
        let Some(base_node) = node
            .child_nodes()
            .find(|child| AstExpression::cast(*child).is_some())
        else {
            return self.recovery_expression(file, node.range());
        };
        let base = self.check_expression(file, base_node, None, context)?;
        let ty = self.expression_type(base);
        if ty == self.program.interner.error() {
            for item in update
                .child_nodes()
                .filter(|child| child.kind() == SyntaxKind::RecordUpdate)
            {
                if let Some(expression) = item
                    .child_nodes()
                    .find(|child| AstExpression::cast(*child).is_some())
                {
                    let _ = self.check_expression(file, expression, None, context)?;
                }
            }
            return self.recovery_expression(file, node.range());
        }
        let Some((owner, arguments, shape)) = self.nominal_instance(ty)? else {
            self.emit(
                self.sources.span(file, base_node.range())?,
                "E1102",
                format!(
                    "record update requires a record value, found `{}`",
                    self.program.interner.canonical(ty)?
                ),
                Vec::new(),
                None,
            )?;
            return self.recovery_expression(file, node.range());
        };
        let HirNominalShape::Record {
            fields: declarations,
        } = shape
        else {
            self.emit(
                self.sources.span(file, base_node.range())?,
                "E1102",
                "`with` is available only on nominal records",
                Vec::new(),
                None,
            )?;
            return self.recovery_expression(file, node.range());
        };
        let (fields, valid) = self.check_record_field_values(
            file,
            update,
            SyntaxKind::RecordUpdate,
            owner,
            &arguments,
            &declarations,
            false,
            context,
        )?;
        if !valid {
            return self.recovery_expression(file, node.range());
        }
        self.allocate_expression(HirExpression {
            span: self.sources.span(file, node.range())?,
            ty,
            category: HirValueCategory::Value,
            kind: HirExpressionKind::RecordUpdate { base, fields },
        })
    }

    fn binary_result(
        &mut self,
        operator: HirBinaryOperator,
        left: TypeId,
        right: TypeId,
    ) -> Result<Option<TypeId>, HirError> {
        if matches!(
            operator,
            HirBinaryOperator::Add
                | HirBinaryOperator::Subtract
                | HirBinaryOperator::Multiply
                | HirBinaryOperator::Divide
                | HirBinaryOperator::Remainder
        ) && (is_array_type(self.program.interner.kind(left)?)
            || is_array_type(self.program.interner.kind(right)?))
        {
            return self.lifted_array_binary_result(operator, left, right);
        }
        if left != right
            && !matches!(
                operator,
                HirBinaryOperator::ShiftLeft | HirBinaryOperator::ShiftRight
            )
        {
            return Ok(None);
        }
        let left_scalar = match self.program.interner.kind(left)? {
            TypeKind::Scalar(scalar) => Some(*scalar),
            _ => None,
        };
        let right_scalar = match self.program.interner.kind(right)? {
            TypeKind::Scalar(scalar) => Some(*scalar),
            _ => None,
        };
        let bool_type = self.program.interner.scalar(ScalarType::Bool);
        let valid = match operator {
            HirBinaryOperator::Multiply
            | HirBinaryOperator::Divide
            | HirBinaryOperator::Add
            | HirBinaryOperator::Subtract => left_scalar.is_some_and(is_arithmetic_scalar),
            HirBinaryOperator::Remainder => left_scalar.is_some_and(is_integer_scalar),
            HirBinaryOperator::ShiftLeft | HirBinaryOperator::ShiftRight => {
                left_scalar
                    .is_some_and(|scalar| is_integer_scalar(scalar) || scalar == ScalarType::Byte)
                    && right_scalar.is_some_and(is_integer_scalar)
            }
            HirBinaryOperator::BitwiseAnd
            | HirBinaryOperator::BitwiseXor
            | HirBinaryOperator::BitwiseOr => left_scalar
                .is_some_and(|scalar| is_integer_scalar(scalar) || scalar == ScalarType::Byte),
            HirBinaryOperator::Less
            | HirBinaryOperator::LessEqual
            | HirBinaryOperator::Greater
            | HirBinaryOperator::GreaterEqual => left_scalar.is_some_and(is_relational_scalar),
            HirBinaryOperator::Equal | HirBinaryOperator::NotEqual => {
                left_scalar.is_some_and(is_bootstrap_equatable_scalar)
            }
            HirBinaryOperator::LogicalAnd | HirBinaryOperator::LogicalOr => {
                left_scalar == Some(ScalarType::Bool)
            }
        };
        if !valid {
            return Ok(None);
        }
        Ok(Some(
            if matches!(
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
                bool_type
            } else {
                left
            },
        ))
    }

    fn lifted_array_binary_result(
        &mut self,
        operator: HirBinaryOperator,
        left: TypeId,
        right: TypeId,
    ) -> Result<Option<TypeId>, HirError> {
        let left_kind = self.program.interner.kind(left)?.clone();
        let right_kind = self.program.interner.kind(right)?.clone();
        let left_element = match left_kind {
            TypeKind::Intrinsic {
                constructor: IntrinsicType::Array,
                arguments,
            } => Some(arguments[0]),
            _ => None,
        };
        let right_element = match right_kind {
            TypeKind::Intrinsic {
                constructor: IntrinsicType::Array,
                arguments,
            } => Some(arguments[0]),
            _ => None,
        };
        let element_result = match (left_element, right_element) {
            (Some(left), Some(right)) => self.binary_result(operator, left, right)?,
            (Some(left), None) => self.binary_result(operator, left, right)?,
            (None, Some(right)) => self.binary_result(operator, left, right)?,
            (None, None) => return Ok(None),
        };
        element_result
            .map(|element| {
                self.program
                    .interner
                    .intrinsic(IntrinsicType::Array, vec![element])
                    .map_err(HirError::from)
            })
            .transpose()
    }

    fn check_postfix(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        expected: Option<ExpressionExpectation>,
        context: &mut BodyContext,
    ) -> Result<HirExpressionId, HirError> {
        let Some(base_node) = node
            .child_nodes()
            .find(|child| AstExpression::cast(*child).is_some())
        else {
            return self.recovery_expression(file, node.range());
        };
        let Some(suffix) = node.child_nodes().find(|child| {
            matches!(
                child.kind(),
                SyntaxKind::CallSuffix
                    | SyntaxKind::BracketPostfix
                    | SyntaxKind::MemberSuffix
                    | SyntaxKind::PropagateSuffix
            )
        }) else {
            return self.check_expression(file, base_node, None, context);
        };
        if suffix.kind() == SyntaxKind::CallSuffix {
            if let Some(call) =
                self.check_bootstrap_host_call(file, node.range(), base_node, suffix, context)?
            {
                return Ok(call);
            }
            if let Some(call) =
                self.check_prelude_runtime_call(file, node.range(), base_node, suffix, context)?
            {
                return Ok(call);
            }
            if let Some(call) = self.check_explicit_generic_call(
                file,
                node.range(),
                base_node,
                suffix,
                expected,
                context,
            )? {
                return Ok(call);
            }
            if let Some(call) =
                self.check_member_call(file, node.range(), base_node, suffix, expected, context)?
            {
                return Ok(call);
            }
            if let Some(constructor) = self.check_nominal_constructor_call(
                file,
                node.range(),
                base_node,
                suffix,
                expected,
                context,
            )? {
                return Ok(constructor);
            }
        }
        let base = self.check_expression(file, base_node, None, context)?;
        match suffix.kind() {
            SyntaxKind::CallSuffix => self.check_call(
                CallSite {
                    file,
                    range: node.range(),
                    suffix,
                    expected,
                },
                base,
                None,
                context,
            ),
            SyntaxKind::PropagateSuffix => {
                self.check_propagate(file, node.range(), base, suffix, context)
            }
            SyntaxKind::BracketPostfix => {
                self.project_bracket_expression(file, node.range(), base, suffix, context)
            }
            SyntaxKind::MemberSuffix => {
                let Some(token) = suffix
                    .child_tokens()
                    .find(|token| !token.kind().is_trivia() && token.kind() != TokenKind::Dot)
                else {
                    return self.recovery_expression(file, node.range());
                };
                self.project_member_expression(file, node.range(), base, token)
            }
            _ => unreachable!("postfix suffix selection is closed"),
        }
    }

    fn check_bootstrap_host_call(
        &mut self,
        file: FileId,
        range: TextRange,
        base: SyntaxNodeRef<'_>,
        suffix: SyntaxNodeRef<'_>,
        context: &mut BodyContext,
    ) -> Result<Option<HirExpressionId>, HirError> {
        if base.kind() != SyntaxKind::PathExpr {
            return Ok(None);
        }
        let identifiers = base
            .child_tokens()
            .filter(|token| token.kind() == TokenKind::Identifier)
            .collect::<Vec<_>>();
        let [module_token, function_token] = identifiers.as_slice() else {
            return Ok(None);
        };
        let Some(module_reference) = self.resolved.reference(file, module_token.range()) else {
            return Ok(None);
        };
        let ResolvedEntity::Module(module) = module_reference.entity() else {
            return Ok(None);
        };
        if module.package().as_str() != "toolchain:std:0.1-bootstrap"
            || module.path().as_str() != "console"
            || function_token.token().normalized_identifier() != Some("print")
        {
            return Ok(None);
        }
        let external_value = self
            .resolved
            .reference(file, function_token.range())
            .is_some_and(|reference| match reference.entity() {
                ResolvedEntity::Name(ResolvedName::External {
                    module: target,
                    namespace: Namespace::Value,
                    name,
                }) => target == module && name.as_str() == "print",
                ResolvedEntity::ContextualCandidates { value_name, .. } => matches!(
                    value_name,
                    ResolvedName::External {
                        module: target,
                        namespace: Namespace::Value,
                        name,
                    } if target == module && name.as_str() == "print"
                ),
                _ => false,
            });
        if !external_value {
            return Ok(None);
        }

        let arguments = suffix
            .child_nodes()
            .filter(|child| child.kind() == SyntaxKind::CallArgument)
            .collect::<Vec<_>>();
        let string_type = self.program.interner.scalar(ScalarType::String);
        let mut lowered = Vec::new();
        let mut invalid_shape = arguments.len() != 1;
        for argument in &arguments {
            let tokens = argument
                .child_tokens()
                .filter(|token| !token.kind().is_trivia())
                .collect::<Vec<_>>();
            invalid_shape |= tokens.iter().any(|token| {
                matches!(
                    token.kind(),
                    TokenKind::Colon
                        | TokenKind::Ellipsis
                        | TokenKind::Ref
                        | TokenKind::Mut
                        | TokenKind::Var
                )
            });
            if let Some(expression) = argument
                .child_nodes()
                .find(|child| AstExpression::cast(*child).is_some())
            {
                lowered.push(self.check_expression(
                    file,
                    expression,
                    Some(ExpressionExpectation::Direct(string_type)),
                    context,
                )?);
            }
        }
        if invalid_shape || lowered.len() != 1 {
            self.emit(
                self.sources.span(file, suffix.range())?,
                "E1102",
                "bootstrap `std.console.print` expects exactly one String value argument",
                Vec::new(),
                None,
            )?;
            return Ok(Some(self.recovery_expression(file, range)?));
        }
        Ok(Some(self.allocate_expression(HirExpression {
            span: self.sources.span(file, range)?,
            ty: self.program.interner.scalar(ScalarType::Unit),
            category: HirValueCategory::Value,
            kind: HirExpressionKind::BootstrapHostCall {
                function: HirBootstrapHostFunction::ConsolePrint,
                arguments: lowered,
            },
        })?))
    }

    fn check_prelude_runtime_call(
        &mut self,
        file: FileId,
        range: TextRange,
        base: SyntaxNodeRef<'_>,
        suffix: SyntaxNodeRef<'_>,
        context: &mut BodyContext,
    ) -> Result<Option<HirExpressionId>, HirError> {
        if base.kind() != SyntaxKind::PathExpr {
            return Ok(None);
        }
        let identifiers = base
            .child_tokens()
            .filter(|token| token.kind() == TokenKind::Identifier)
            .collect::<Vec<_>>();
        let [token] = identifiers.as_slice() else {
            return Ok(None);
        };
        let Some(reference) = self.resolved.reference(file, token.range()) else {
            return Ok(None);
        };
        let ResolvedEntity::Name(ResolvedName::Prelude {
            namespace: Namespace::Value,
            name,
        }) = reference.entity()
        else {
            return Ok(None);
        };
        if !matches!(name.as_str(), "panic" | "assert") {
            return Ok(None);
        }

        let bool_type = self.program.interner.scalar(ScalarType::Bool);
        let string_type = self.program.interner.scalar(ScalarType::String);
        let string_array = self
            .program
            .interner
            .intrinsic(IntrinsicType::Array, vec![string_type])?;
        let arguments = suffix
            .child_nodes()
            .filter(|child| child.kind() == SyntaxKind::CallArgument)
            .collect::<Vec<_>>();
        let mut condition = None;
        let mut message = None;
        let mut message_parts = Vec::new();
        let mut named_started = false;
        let mut spread_seen = false;

        for (index, argument) in arguments.iter().copied().enumerate() {
            let tokens = argument
                .child_tokens()
                .filter(|token| !token.kind().is_trivia())
                .collect::<Vec<_>>();
            let label = if tokens
                .get(1)
                .is_some_and(|token| token.kind() == TokenKind::Colon)
            {
                tokens
                    .first()
                    .and_then(|token| token.token().normalized_identifier())
                    .and_then(|name| Name::new(name).ok())
            } else {
                None
            };
            let spread = tokens
                .iter()
                .any(|token| token.kind() == TokenKind::Ellipsis);
            let has_mode = tokens.iter().any(|token| {
                matches!(
                    token.kind(),
                    TokenKind::Ref | TokenKind::Mut | TokenKind::Var
                )
            });
            if has_mode {
                self.emit(
                    self.sources.span(file, argument.range())?,
                    "E1407",
                    "prelude runtime arguments are passed by value",
                    Vec::new(),
                    None,
                )?;
            }
            if spread && (spread_seen || index + 1 != arguments.len()) {
                self.emit(
                    self.sources.span(file, argument.range())?,
                    "E1102",
                    "a variadic spread must be unique and final",
                    Vec::new(),
                    None,
                )?;
            }
            spread_seen |= spread;
            if label.is_some() {
                named_started = true;
            } else if named_started {
                self.emit(
                    self.sources.span(file, argument.range())?,
                    "E1102",
                    "positional arguments must precede named arguments",
                    Vec::new(),
                    None,
                )?;
            }

            let Some(expression) = argument
                .child_nodes()
                .find(|child| AstExpression::cast(*child).is_some())
            else {
                continue;
            };
            if name.as_str() == "panic" {
                let valid_label = label
                    .as_ref()
                    .is_none_or(|label| label.as_str() == "message");
                if !valid_label || spread || message.is_some() {
                    self.emit(
                        self.sources.span(file, argument.range())?,
                        "E1102",
                        "`panic` accepts exactly one non-spread `message` argument",
                        Vec::new(),
                        None,
                    )?;
                }
                let value = self.check_expression(
                    file,
                    expression,
                    Some(ExpressionExpectation::Direct(string_type)),
                    context,
                )?;
                if message.is_none() {
                    message = Some(value);
                }
                continue;
            }

            let target = match label.as_ref().map(Name::as_str) {
                Some("condition") => {
                    if spread || condition.is_some() {
                        self.emit(
                            self.sources.span(file, argument.range())?,
                            "E1102",
                            "`assert` condition must be one non-spread argument",
                            Vec::new(),
                            None,
                        )?;
                    }
                    AssertArgument::Condition
                }
                Some("messageParts") => {
                    if !spread {
                        self.emit(
                            self.sources.span(file, argument.range())?,
                            "E1102",
                            "named `messageParts` requires an array spread",
                            Vec::new(),
                            None,
                        )?;
                    }
                    AssertArgument::Message
                }
                Some(_) => {
                    self.emit(
                        self.sources.span(file, argument.range())?,
                        "E1102",
                        "`assert` has only `condition` and variadic `messageParts` parameters",
                        Vec::new(),
                        None,
                    )?;
                    AssertArgument::Message
                }
                None if condition.is_none() && !spread => AssertArgument::Condition,
                None => AssertArgument::Message,
            };
            match target {
                AssertArgument::Condition => {
                    let value = self.check_expression(
                        file,
                        expression,
                        Some(ExpressionExpectation::Direct(bool_type)),
                        context,
                    )?;
                    if condition.replace(value).is_some() {
                        self.emit(
                            self.sources.span(file, argument.range())?,
                            "E1102",
                            "`assert` condition is provided more than once",
                            Vec::new(),
                            None,
                        )?;
                    }
                }
                AssertArgument::Message => {
                    let expected = if spread { string_array } else { string_type };
                    let value = self.check_expression(
                        file,
                        expression,
                        Some(ExpressionExpectation::Direct(expected)),
                        context,
                    )?;
                    message_parts.push(HirAssertMessagePart { value, spread });
                }
            }
        }

        let span = self.sources.span(file, range)?;
        if name.as_str() == "panic" {
            let Some(message) = message else {
                self.emit(
                    self.sources.span(file, suffix.range())?,
                    "E1102",
                    "`panic` requires exactly one String argument",
                    Vec::new(),
                    None,
                )?;
                return Ok(Some(self.recovery_expression(file, range)?));
            };
            if arguments.len() != 1 {
                self.emit(
                    self.sources.span(file, suffix.range())?,
                    "E1102",
                    format!("`panic` expects one argument, found {}", arguments.len()),
                    Vec::new(),
                    None,
                )?;
            }
            return Ok(Some(self.allocate_expression(HirExpression {
                span,
                ty: self.program.interner.scalar(ScalarType::Never),
                category: HirValueCategory::Value,
                kind: HirExpressionKind::PreludePanic { message },
            })?));
        }

        let Some(condition) = condition else {
            self.emit(
                self.sources.span(file, suffix.range())?,
                "E1102",
                "`assert` requires a Bool condition",
                Vec::new(),
                None,
            )?;
            return Ok(Some(self.recovery_expression(file, range)?));
        };
        Ok(Some(self.allocate_expression(HirExpression {
            span,
            ty: self.program.interner.scalar(ScalarType::Unit),
            category: HirValueCategory::Value,
            kind: HirExpressionKind::PreludeAssert {
                condition,
                message_parts,
            },
        })?))
    }

    fn check_propagate(
        &mut self,
        file: FileId,
        range: TextRange,
        value: HirExpressionId,
        suffix: SyntaxNodeRef<'_>,
        context: &BodyContext,
    ) -> Result<HirExpressionId, HirError> {
        let value_type = self.expression_type(value);
        let kind = self.program.interner.kind(value_type)?.clone();
        match kind {
            TypeKind::Option(item) => {
                let compatible = context.callable.is_some_and(|callable| {
                    matches!(
                        self.program.interner.kind(callable.success),
                        Ok(TypeKind::Option(_))
                    )
                });
                if !compatible {
                    self.emit_incompatible_propagation(
                        self.sources.span(file, suffix.range())?,
                        value_type,
                        context.callable,
                        "option absence requires a direct option success type",
                    )?;
                    return self.recovery_expression(file, range);
                }
                self.allocate_expression(HirExpression {
                    span: self.sources.span(file, range)?,
                    ty: item,
                    category: HirValueCategory::Value,
                    kind: HirExpressionKind::PropagateOption { value },
                })
            }
            TypeKind::Result {
                success,
                error: produced_error,
            } => {
                let Some(callable) = context.callable else {
                    self.emit_incompatible_propagation(
                        self.sources.span(file, suffix.range())?,
                        value_type,
                        None,
                        "result propagation requires an enclosing fallible callable",
                    )?;
                    return self.recovery_expression(file, range);
                };
                let Some(expected_error) = callable.error else {
                    self.emit_incompatible_propagation(
                        self.sources.span(file, suffix.range())?,
                        value_type,
                        Some(callable),
                        "result propagation requires a direct enclosing error channel",
                    )?;
                    return self.recovery_expression(file, range);
                };
                let Some(error_coercion) =
                    self.error_assignability(produced_error, expected_error)?
                else {
                    self.emit_error_propagation_mismatch(
                        self.sources.span(file, suffix.range())?,
                        produced_error,
                        expected_error,
                        callable.signature,
                    )?;
                    return self.recovery_expression(file, range);
                };
                self.allocate_expression(HirExpression {
                    span: self.sources.span(file, range)?,
                    ty: success,
                    category: HirValueCategory::Value,
                    kind: HirExpressionKind::PropagateResult {
                        value,
                        error_coercion,
                    },
                })
            }
            TypeKind::Error => self.recovery_expression(file, range),
            _ => {
                self.emit_incompatible_propagation(
                    self.sources.span(file, suffix.range())?,
                    value_type,
                    context.callable,
                    "`?` requires a direct `Option` or `Result` operand",
                )?;
                self.recovery_expression(file, range)
            }
        }
    }

    fn check_explicit_generic_call(
        &mut self,
        file: FileId,
        range: TextRange,
        base_node: SyntaxNodeRef<'_>,
        suffix: SyntaxNodeRef<'_>,
        expected: Option<ExpressionExpectation>,
        context: &mut BodyContext,
    ) -> Result<Option<HirExpressionId>, HirError> {
        if base_node.kind() != SyntaxKind::PathExpr {
            return Ok(None);
        }
        let brackets = base_node
            .child_nodes()
            .filter(|child| child.kind() == SyntaxKind::BracketPostfix)
            .collect::<Vec<_>>();
        if brackets.is_empty() {
            return Ok(None);
        }
        if brackets.len() != 1 {
            self.emit(
                self.sources.span(file, base_node.range())?,
                "E1104",
                "a generic callable has exactly one type-argument list",
                Vec::new(),
                None,
            )?;
            return Ok(Some(self.recovery_expression(file, range)?));
        }
        let identifiers = base_node
            .child_tokens()
            .filter(|token| token.kind() == TokenKind::Identifier)
            .collect::<Vec<_>>();
        let Some((resolved_index, symbol)) =
            identifiers.iter().enumerate().find_map(|(index, token)| {
                let reference = self.resolved.reference(file, token.range())?;
                let ResolvedEntity::Name(ResolvedName::Symbol(symbol)) = reference.entity() else {
                    return None;
                };
                self.resolved
                    .symbol(*symbol)
                    .is_some_and(|symbol| symbol.kind() == SymbolKind::Function)
                    .then_some((index, *symbol))
            })
        else {
            return Ok(None);
        };
        if resolved_index + 1 != identifiers.len() {
            return Ok(None);
        }
        let id = HirCallableId::Symbol(symbol);
        let Some(callable) = self.callable(id).cloned() else {
            self.complete = false;
            return Ok(Some(self.recovery_expression(file, range)?));
        };
        if callable.generics.is_empty() {
            self.emit(
                self.sources.span(file, brackets[0].range())?,
                "E1104",
                "this callable does not declare generic parameters",
                Vec::new(),
                None,
            )?;
            return Ok(Some(self.recovery_expression(file, range)?));
        }
        let Some(arguments) = self.expression_generic_arguments(file, brackets[0])? else {
            return Ok(Some(self.recovery_expression(file, range)?));
        };
        if arguments.len() != callable.generics.len() {
            self.emit(
                self.sources.span(file, brackets[0].range())?,
                "E1104",
                format!(
                    "generic call expects {} type arguments, found {}",
                    callable.generics.len(),
                    arguments.len()
                ),
                Vec::new(),
                None,
            )?;
            return Ok(Some(self.recovery_expression(file, range)?));
        }
        if callable
            .generics
            .iter()
            .any(|parameter| !parameter.bounds.is_empty())
        {
            self.complete = false;
        }
        let function_type = TypeSubstitution::new(arguments.clone())
            .apply(&mut self.program.interner, callable.function_type)?;
        let callee = self.allocate_expression(HirExpression {
            span: self.sources.span(file, base_node.range())?,
            ty: function_type,
            category: HirValueCategory::Value,
            kind: HirExpressionKind::SpecializedFunction {
                callable: id,
                arguments,
            },
        })?;
        Ok(Some(self.check_call(
            CallSite {
                file,
                range,
                suffix,
                expected,
            },
            callee,
            None,
            context,
        )?))
    }

    #[allow(clippy::too_many_arguments)]
    fn check_member_call(
        &mut self,
        file: FileId,
        range: TextRange,
        base_node: SyntaxNodeRef<'_>,
        suffix: SyntaxNodeRef<'_>,
        expected: Option<ExpressionExpectation>,
        context: &mut BodyContext,
    ) -> Result<Option<HirExpressionId>, HirError> {
        if base_node.kind() == SyntaxKind::PathExpr {
            let tokens = base_node
                .child_tokens()
                .filter(|token| token.kind() == TokenKind::Identifier)
                .collect::<Vec<_>>();
            let Some((resolved_index, resolved)) =
                tokens.iter().enumerate().find_map(|(index, token)| {
                    let reference = self.resolved.reference(file, token.range())?;
                    let resolved = match reference.entity() {
                        ResolvedEntity::Name(name) => name.clone(),
                        ResolvedEntity::ContextualCandidates { type_name, .. } => type_name.clone(),
                        ResolvedEntity::Module(_) => return None,
                    };
                    Some((index, resolved))
                })
            else {
                return Ok(None);
            };
            if resolved_index + 1 < tokens.len() {
                let resolved_is_type = match &resolved {
                    ResolvedName::Symbol(symbol) => {
                        self.resolved.symbol(*symbol).is_some_and(|symbol| {
                            matches!(
                                symbol.kind(),
                                SymbolKind::Type
                                    | SymbolKind::Alias
                                    | SymbolKind::Enum
                                    | SymbolKind::NewtypeConstructor
                            )
                        })
                    }
                    ResolvedName::Prelude { namespace, .. }
                    | ResolvedName::External { namespace, .. } => *namespace == Namespace::Type,
                    ResolvedName::ContextualSelf => true,
                    ResolvedName::Local(_) | ResolvedName::Receiver => false,
                };
                let member_token = *tokens.last().expect("a qualified path has a final token");
                if resolved_is_type {
                    let Some(path) = self.expression_path_info(file, base_node)? else {
                        return Ok(None);
                    };
                    if path.suffix.len() != 1 {
                        return Ok(None);
                    }
                    let Some(owner_type) =
                        self.construction_type(file, base_node.range(), &path, expected)?
                    else {
                        return Ok(Some(self.recovery_expression(file, range)?));
                    };
                    let Some((owner, _, _)) = self.nominal_instance(owner_type)? else {
                        return Ok(None);
                    };
                    let Some(member) = self.callable_member(
                        file,
                        owner,
                        member_token,
                        &[MemberKind::AssociatedFunction, MemberKind::InherentMethod],
                    )?
                    else {
                        return Ok(None);
                    };
                    return self
                        .finish_resolved_member_call(
                            file,
                            range,
                            member_token,
                            member,
                            suffix,
                            None,
                            expected,
                            context,
                        )
                        .map(Some);
                }

                let receiver =
                    self.check_value_path(file, base_node, context, Some(member_token.range()))?;
                return self
                    .finish_value_member_call(
                        file,
                        range,
                        base_node.range(),
                        receiver,
                        member_token,
                        suffix,
                        expected,
                        context,
                    )
                    .map(Some);
            }
        }

        if base_node.kind() == SyntaxKind::PostfixExpr
            && let Some(member_suffix) = base_node
                .child_nodes()
                .find(|child| child.kind() == SyntaxKind::MemberSuffix)
        {
            let Some(receiver_node) = base_node
                .child_nodes()
                .find(|child| AstExpression::cast(*child).is_some())
            else {
                return Ok(Some(self.recovery_expression(file, range)?));
            };
            let Some(member_token) = member_suffix
                .child_tokens()
                .find(|token| !token.kind().is_trivia() && token.kind() != TokenKind::Dot)
            else {
                return Ok(Some(self.recovery_expression(file, range)?));
            };
            let receiver = self.check_expression(file, receiver_node, None, context)?;
            return self
                .finish_value_member_call(
                    file,
                    range,
                    base_node.range(),
                    receiver,
                    member_token,
                    suffix,
                    expected,
                    context,
                )
                .map(Some);
        }
        Ok(None)
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_value_member_call(
        &mut self,
        file: FileId,
        range: TextRange,
        member_range: TextRange,
        receiver: HirExpressionId,
        member_token: SyntaxTokenRef<'_>,
        suffix: SyntaxNodeRef<'_>,
        expected: Option<ExpressionExpectation>,
        context: &mut BodyContext,
    ) -> Result<HirExpressionId, HirError> {
        let receiver_type = self.expression_type(receiver);
        if receiver_type == self.program.interner.error() {
            return self.recovery_expression(file, range);
        }
        if let Some((owner, _, _)) = self.nominal_instance(receiver_type)?
            && let Some(member) =
                self.callable_member(file, owner, member_token, &[MemberKind::InherentMethod])?
        {
            return self.finish_resolved_member_call(
                file,
                range,
                member_token,
                member,
                suffix,
                Some(receiver),
                expected,
                context,
            );
        }
        let field = self.project_member_expression(file, member_range, receiver, member_token)?;
        self.check_call(
            CallSite {
                file,
                range,
                suffix,
                expected,
            },
            field,
            None,
            context,
        )
    }

    fn callable_member(
        &self,
        file: FileId,
        owner: SymbolId,
        token: SyntaxTokenRef<'_>,
        kinds: &[MemberKind],
    ) -> Result<Option<MemberId>, HirError> {
        let spelling = token
            .token()
            .normalized_identifier()
            .unwrap_or(self.token_text(file, token)?);
        let Ok(name) = MemberName::new(spelling) else {
            return Ok(None);
        };
        Ok(self
            .resolved
            .lookup_members(MemberOwner::Type(owner), &name)
            .into_iter()
            .flatten()
            .copied()
            .find(|member| {
                self.resolved
                    .member(*member)
                    .is_some_and(|member| kinds.contains(&member.kind()))
            }))
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_resolved_member_call(
        &mut self,
        file: FileId,
        range: TextRange,
        token: SyntaxTokenRef<'_>,
        member: MemberId,
        suffix: SyntaxNodeRef<'_>,
        receiver: Option<HirExpressionId>,
        expected: Option<ExpressionExpectation>,
        context: &mut BodyContext,
    ) -> Result<HirExpressionId, HirError> {
        let declaration = self
            .resolved
            .member(member)
            .expect("resolved member lookup returns indexed members");
        if declaration.visibility() == Visibility::Private {
            let MemberOwner::Type(owner) = declaration.owner() else {
                unreachable!("callable members have nominal owners");
            };
            let owner = self
                .resolved
                .symbol(owner)
                .expect("callable member owners remain indexed");
            let source = self.sources.get(file)?;
            if owner.identity().source_id() != source.source_id()
                || owner.identity().module() != source.module()
            {
                self.emit(
                    self.sources.span(file, token.range())?,
                    "E1501",
                    "this method is private to its declaring module",
                    vec![("the private method is declared here", declaration.span())],
                    None,
                )?;
                return self.recovery_expression(file, range);
            }
        }
        let id = HirCallableId::Member(member);
        let Some(callable) = self.callable(id).cloned() else {
            self.complete = false;
            return self.recovery_expression(file, range);
        };
        self.record_member_reference(self.sources.span(file, token.range())?, member);
        let callee = self.allocate_expression(HirExpression {
            span: self.sources.span(file, token.range())?,
            ty: callable.function_type,
            category: HirValueCategory::Value,
            kind: HirExpressionKind::Function(id),
        })?;
        self.check_call(
            CallSite {
                file,
                range,
                suffix,
                expected,
            },
            callee,
            receiver,
            context,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn check_nominal_constructor_call(
        &mut self,
        file: FileId,
        range: TextRange,
        base_node: SyntaxNodeRef<'_>,
        suffix: SyntaxNodeRef<'_>,
        expected: Option<ExpressionExpectation>,
        context: &mut BodyContext,
    ) -> Result<Option<HirExpressionId>, HirError> {
        let Some(path) = self.expression_path_info(file, base_node)? else {
            return Ok(None);
        };
        let Some(ty) = self.construction_type(file, base_node.range(), &path, expected)? else {
            return Ok(Some(self.recovery_expression(file, range)?));
        };
        if let TypeKind::Scalar(target) = self.program.interner.kind(ty)?.clone() {
            if !path.suffix.is_empty() {
                return Ok(None);
            }
            return self
                .check_numeric_conversion(file, range, base_node, suffix, target, context)
                .map(Some);
        }
        let Some((symbol, arguments, shape)) = self.nominal_instance(ty)? else {
            return Ok(None);
        };
        match shape {
            HirNominalShape::Newtype { underlying } if path.suffix.is_empty() => {
                let underlying = self.instantiate_type(&arguments, underlying)?;
                let (mut values, valid) = self.check_constructor_arguments(
                    file,
                    suffix,
                    &[underlying],
                    "newtype constructor",
                    context,
                )?;
                if !valid {
                    return Ok(Some(self.recovery_expression(file, range)?));
                }
                let value = values
                    .pop()
                    .expect("a valid newtype constructor has one value");
                Ok(Some(self.allocate_expression(HirExpression {
                    span: self.sources.span(file, range)?,
                    ty,
                    category: HirValueCategory::Value,
                    kind: HirExpressionKind::Newtype {
                        constructor: symbol,
                        value,
                    },
                })?))
            }
            HirNominalShape::Enum { variants } if path.suffix.len() == 1 => {
                let segment = &path.suffix[0];
                let Some(variant) = variants.iter().find(|variant| {
                    self.resolved
                        .member(variant.member())
                        .is_some_and(|member| member.name().as_str() == segment.name.as_str())
                }) else {
                    self.emit(
                        segment.span,
                        "E1102",
                        "unknown enum variant",
                        Vec::new(),
                        None,
                    )?;
                    return Ok(Some(self.recovery_expression(file, range)?));
                };
                self.record_member_reference(segment.span, variant.member());
                let HirVariantPayload::Tuple(templates) = variant.payload() else {
                    let required = match variant.payload() {
                        HirVariantPayload::Unit => "no payload and must be used without `()`",
                        HirVariantPayload::Record(_) => {
                            "a record payload and must be constructed with `{ ... }`"
                        }
                        HirVariantPayload::Tuple(_) => unreachable!(),
                    };
                    self.emit(
                        segment.span,
                        "E1102",
                        format!("this enum variant has {required}"),
                        Vec::new(),
                        None,
                    )?;
                    return Ok(Some(self.recovery_expression(file, range)?));
                };
                let types = self.instantiate_types(&arguments, templates)?;
                let (values, valid) = self.check_constructor_arguments(
                    file,
                    suffix,
                    &types,
                    "enum variant constructor",
                    context,
                )?;
                if !valid {
                    return Ok(Some(self.recovery_expression(file, range)?));
                }
                Ok(Some(self.allocate_expression(HirExpression {
                    span: self.sources.span(file, range)?,
                    ty,
                    category: HirValueCategory::Value,
                    kind: HirExpressionKind::Variant {
                        variant: variant.member(),
                        payload: HirVariantValue::Tuple(values),
                    },
                })?))
            }
            HirNominalShape::Newtype { .. } => {
                self.emit(
                    self.sources.span(file, base_node.range())?,
                    "E1102",
                    "a newtype constructor cannot have a member suffix",
                    Vec::new(),
                    None,
                )?;
                Ok(Some(self.recovery_expression(file, range)?))
            }
            HirNominalShape::Record { .. } => {
                self.emit(
                    self.sources.span(file, base_node.range())?,
                    "E1102",
                    "records use the `Name { ... }` constructor form",
                    Vec::new(),
                    None,
                )?;
                Ok(Some(self.recovery_expression(file, range)?))
            }
            HirNominalShape::Enum { .. } => {
                self.emit(
                    self.sources.span(file, base_node.range())?,
                    "E1102",
                    "an enum constructor must name exactly one variant",
                    Vec::new(),
                    None,
                )?;
                Ok(Some(self.recovery_expression(file, range)?))
            }
        }
    }

    fn check_numeric_conversion(
        &mut self,
        file: FileId,
        range: TextRange,
        constructor: SyntaxNodeRef<'_>,
        suffix: SyntaxNodeRef<'_>,
        target: ScalarType,
        context: &mut BodyContext,
    ) -> Result<HirExpressionId, HirError> {
        let arguments = suffix
            .child_nodes()
            .filter(|child| child.kind() == SyntaxKind::CallArgument)
            .collect::<Vec<_>>();
        let mut valid = true;
        if arguments.len() != 1 {
            self.emit(
                self.sources.span(file, suffix.range())?,
                "E1103",
                format!(
                    "numeric conversion constructor expects one value, found {}",
                    arguments.len()
                ),
                Vec::new(),
                None,
            )?;
            valid = false;
        }
        let mut values = Vec::with_capacity(arguments.len());
        for argument in arguments {
            let tokens = argument
                .child_tokens()
                .filter(|token| !token.kind().is_trivia())
                .collect::<Vec<_>>();
            if tokens.iter().any(|token| {
                matches!(
                    token.kind(),
                    TokenKind::Colon
                        | TokenKind::Ellipsis
                        | TokenKind::Ref
                        | TokenKind::Mut
                        | TokenKind::Var
                )
            }) {
                self.emit(
                    self.sources.span(file, argument.range())?,
                    "E1103",
                    "numeric conversions accept one positional value passed by value",
                    Vec::new(),
                    None,
                )?;
                valid = false;
            }
            let Some(expression) = argument
                .child_nodes()
                .find(|child| AstExpression::cast(*child).is_some())
            else {
                valid = false;
                continue;
            };
            let value = self.check_expression(file, expression, None, context)?;
            valid &= self.expression_type(value) != self.program.interner.error();
            values.push(value);
        }
        if !valid || values.len() != 1 {
            return self.recovery_expression(file, range);
        }
        let value = values[0];
        let source_type = self.expression_type(value);
        let source = match self.program.interner.kind(source_type)? {
            TypeKind::Scalar(source) => *source,
            TypeKind::Error => return self.recovery_expression(file, range),
            _ => {
                self.emit(
                    self.sources.span(file, range)?,
                    "E1103",
                    format!(
                        "cannot convert `{}` to `{target}` with a numeric constructor",
                        self.program.interner.canonical(source_type)?
                    ),
                    Vec::new(),
                    Some((
                        target.to_string(),
                        self.program.interner.canonical(source_type)?,
                    )),
                )?;
                return self.recovery_expression(file, range);
            }
        };
        let Some(conversion) = numeric_conversion(source, target) else {
            self.emit(
                self.sources.span(file, range)?,
                "E1103",
                format!("numeric conversion from `{source}` to `{target}` is not defined"),
                Vec::new(),
                Some((target.to_string(), source.to_string())),
            )?;
            return self.recovery_expression(file, range);
        };
        if conversion == NumericConversion::Identity {
            self.emit_warning(
                self.sources.span(file, constructor.range())?,
                "W1007",
                format!("conversion from `{source}` to the same type is redundant"),
            )?;
        }
        let target_type = self.program.interner.scalar(target);
        let ty = if conversion == NumericConversion::Checked {
            let error = self
                .program
                .interner
                .intrinsic(IntrinsicType::NumericConversionError, Vec::new())?;
            self.program.interner.result(target_type, error)?
        } else {
            target_type
        };
        self.allocate_expression(HirExpression {
            span: self.sources.span(file, range)?,
            ty,
            category: HirValueCategory::Value,
            kind: HirExpressionKind::NumericConversion {
                target,
                conversion,
                value,
            },
        })
    }

    fn check_constructor_arguments(
        &mut self,
        file: FileId,
        suffix: SyntaxNodeRef<'_>,
        expected: &[TypeId],
        subject: &str,
        context: &mut BodyContext,
    ) -> Result<(Vec<HirExpressionId>, bool), HirError> {
        let arguments = suffix
            .child_nodes()
            .filter(|child| child.kind() == SyntaxKind::CallArgument)
            .collect::<Vec<_>>();
        let mut valid = true;
        if arguments.len() != expected.len() {
            self.emit(
                self.sources.span(file, suffix.range())?,
                "E1102",
                format!(
                    "{subject} expects {} values, found {}",
                    expected.len(),
                    arguments.len()
                ),
                Vec::new(),
                None,
            )?;
            valid = false;
        }
        let mut values = Vec::with_capacity(arguments.len());
        for (index, argument) in arguments.into_iter().enumerate() {
            let tokens = argument
                .child_tokens()
                .filter(|token| !token.kind().is_trivia())
                .collect::<Vec<_>>();
            if tokens.iter().any(|token| {
                matches!(
                    token.kind(),
                    TokenKind::Colon
                        | TokenKind::Ellipsis
                        | TokenKind::Ref
                        | TokenKind::Mut
                        | TokenKind::Var
                )
            }) {
                self.emit(
                    self.sources.span(file, argument.range())?,
                    "E1102",
                    format!("{subject} accepts only positional values passed by value"),
                    Vec::new(),
                    None,
                )?;
                valid = false;
            }
            let Some(expression) = argument
                .child_nodes()
                .find(|child| AstExpression::cast(*child).is_some())
            else {
                valid = false;
                continue;
            };
            let value = self.check_expression(
                file,
                expression,
                expected
                    .get(index)
                    .copied()
                    .map(ExpressionExpectation::Direct),
                context,
            )?;
            valid &= self.expression_type(value) != self.program.interner.error();
            values.push(value);
        }
        Ok((values, valid))
    }

    fn check_call(
        &mut self,
        site: CallSite<'_>,
        callee: HirExpressionId,
        bound_receiver: Option<HirExpressionId>,
        context: &mut BodyContext,
    ) -> Result<HirExpressionId, HirError> {
        let CallSite {
            file,
            range,
            suffix,
            expected,
        } = site;
        let mut callee = callee;
        let callee_type = self.expression_type(callee);
        let mut function = match self.program.interner.kind(callee_type)?.clone() {
            TypeKind::Function(function) => function,
            TypeKind::Error => return self.recovery_expression(file, range),
            _ => {
                self.emit(
                    self.sources.span(file, suffix.range())?,
                    "E1102",
                    format!(
                        "value of type `{}` is not callable",
                        self.program.interner.canonical(callee_type)?
                    ),
                    Vec::new(),
                    None,
                )?;
                return self.recovery_expression(file, range);
            }
        };
        let generic_callable = match self
            .program
            .expression(callee)
            .expect("allocated callee expressions remain indexed")
            .kind()
        {
            HirExpressionKind::Function(id) => self
                .callable(*id)
                .filter(|callable| !callable.generics.is_empty())
                .cloned(),
            _ => None,
        };
        let mut inference = if let Some(callable) = generic_callable {
            let variables = (0..callable.generics.len())
                .map(|_| {
                    self.program
                        .interner
                        .fresh_inference()
                        .map_err(HirError::from)
                })
                .collect::<Result<Vec<_>, _>>()?;
            let inferred_type = TypeSubstitution::new(variables.clone())
                .apply(&mut self.program.interner, callable.function_type)?;
            function = match self.program.interner.kind(inferred_type)?.clone() {
                TypeKind::Function(function) => function,
                _ => unreachable!("callable signatures always lower to function types"),
            };
            let mut inference = GenericCallInference {
                callable: callable.id,
                variables,
                solver: InferenceContext::new(),
                contradiction: false,
            };
            if let Some(expectation) = expected {
                let contextual = match expectation {
                    ExpressionExpectation::Direct(ty) => ty,
                    ExpressionExpectation::CallableOutcome { full, success } => {
                        if matches!(
                            self.program.interner.kind(function.outcome())?,
                            TypeKind::Result { .. }
                        ) {
                            full
                        } else {
                            success
                        }
                    }
                };
                let _ = self.constrain_inference_assignment(
                    &mut inference.solver,
                    function.outcome(),
                    contextual,
                )?;
            }
            Some(inference)
        } else {
            None
        };
        let argument_nodes = suffix
            .child_nodes()
            .filter(|child| child.kind() == SyntaxKind::CallArgument)
            .collect::<Vec<_>>();
        let mut shape = self.call_shape(callee, &function, bound_receiver.is_some());
        let mut provided = vec![false; shape.fixed.len()];
        let mut next_positional = 0;
        let mut named_started = false;
        let mut spread_seen = false;
        let mut association_error = false;
        let mut arguments =
            Vec::with_capacity(argument_nodes.len() + usize::from(bound_receiver.is_some()));
        if let Some(receiver) = bound_receiver {
            let receiver_parameter = self
                .call_shape(callee, &function, false)
                .fixed
                .into_iter()
                .find(|parameter| parameter.receiver)
                .expect("a bound method call has a receiver parameter");
            let receiver_mode = receiver_parameter.mode;
            let receiver_type = self.expression_type(receiver);
            let resolved_expected = if let Some(inference) = &mut inference {
                match self.constrain_inference_assignment(
                    &mut inference.solver,
                    receiver_parameter.ty,
                    receiver_type,
                )? {
                    InferenceAssignment::Mismatch => {
                        inference.contradiction = true;
                        self.emit(
                            self.program.expressions[receiver.0 as usize].span,
                            "E1102",
                            "method receiver does not match its inferred owner type",
                            Vec::new(),
                            None,
                        )?;
                    }
                    InferenceAssignment::Applied | InferenceAssignment::Ambiguous => {}
                }
                self.resolve_inference_type(&inference.solver, receiver_parameter.ty)?
            } else {
                Some(receiver_parameter.ty)
            };
            self.check_method_receiver(receiver, receiver_mode, resolved_expected, context)?;
            arguments.push(HirCallArgument {
                label: None,
                mode: receiver_mode,
                spread: false,
                target: HirCallArgumentTarget::Receiver,
                value: receiver,
            });
        }
        let argument_count = argument_nodes.len();
        for (index, argument) in argument_nodes.into_iter().enumerate() {
            let tokens = argument
                .child_tokens()
                .filter(|token| !token.kind().is_trivia())
                .collect::<Vec<_>>();
            let label = if tokens
                .get(1)
                .is_some_and(|token| token.kind() == TokenKind::Colon)
            {
                tokens
                    .first()
                    .and_then(|token| token.token().normalized_identifier())
                    .and_then(|name| Name::new(name).ok())
            } else {
                None
            };
            let spread = tokens
                .iter()
                .any(|token| token.kind() == TokenKind::Ellipsis);
            let mode = if tokens.iter().any(|token| token.kind() == TokenKind::Ref) {
                ParameterMode::Ref
            } else if tokens.iter().any(|token| token.kind() == TokenKind::Mut) {
                ParameterMode::Mut
            } else if tokens.iter().any(|token| token.kind() == TokenKind::Var) {
                ParameterMode::Var
            } else {
                ParameterMode::Value
            };
            if spread && (spread_seen || index + 1 != argument_count) {
                self.emit(
                    self.sources.span(file, argument.range())?,
                    "E1102",
                    "a call spread must be unique and the final argument",
                    Vec::new(),
                    None,
                )?;
                association_error = true;
            }
            spread_seen |= spread;

            let mut target = HirCallArgumentTarget::Invalid;
            let mut expected_type = None;
            let mut expected_mode = ParameterMode::Value;
            let mut receiver_mode = None;
            if let Some(label) = &label {
                named_started = true;
                if let Some(fixed) = shape
                    .fixed
                    .iter()
                    .position(|parameter| parameter.name.as_ref() == Some(label))
                {
                    target = if shape.fixed[fixed].receiver {
                        receiver_mode = Some(shape.fixed[fixed].mode);
                        HirCallArgumentTarget::Receiver
                    } else {
                        HirCallArgumentTarget::Fixed(shape.fixed[fixed].index)
                    };
                    expected_type = Some(shape.fixed[fixed].ty);
                    expected_mode = if shape.fixed[fixed].receiver {
                        ParameterMode::Value
                    } else {
                        shape.fixed[fixed].mode
                    };
                    if provided[fixed] {
                        self.emit(
                            self.sources.span(file, argument.range())?,
                            "E1102",
                            format!("parameter `{label}` is provided more than once"),
                            Vec::new(),
                            None,
                        )?;
                        association_error = true;
                    }
                    provided[fixed] = true;
                    if spread {
                        self.emit(
                            self.sources.span(file, argument.range())?,
                            "E1102",
                            "only the variadic parameter accepts a named spread",
                            Vec::new(),
                            None,
                        )?;
                        association_error = true;
                    }
                } else if shape.variadic.as_ref().and_then(|(name, _)| name.as_ref()) == Some(label)
                {
                    let element = shape
                        .variadic
                        .as_ref()
                        .expect("the variadic name came from this shape")
                        .1;
                    if !spread {
                        self.emit(
                            self.sources.span(file, argument.range())?,
                            "E1102",
                            "a named variadic argument must use one array spread",
                            Vec::new(),
                            None,
                        )?;
                        association_error = true;
                        target = HirCallArgumentTarget::Invalid;
                        expected_type = None;
                    } else {
                        target = HirCallArgumentTarget::VariadicSpread;
                        expected_type = Some(
                            self.program
                                .interner
                                .intrinsic(IntrinsicType::Array, vec![element])?,
                        );
                    }
                } else {
                    self.emit(
                        self.sources.span(file, argument.range())?,
                        "E1102",
                        format!("callable has no named parameter `{label}`"),
                        Vec::new(),
                        None,
                    )?;
                    association_error = true;
                }
            } else {
                if named_started {
                    self.emit(
                        self.sources.span(file, argument.range())?,
                        "E1102",
                        "positional arguments must precede named arguments",
                        Vec::new(),
                        None,
                    )?;
                    association_error = true;
                }
                while next_positional < provided.len() && provided[next_positional] {
                    next_positional += 1;
                }
                if next_positional < shape.fixed.len() && !spread {
                    let fixed = next_positional;
                    next_positional += 1;
                    provided[fixed] = true;
                    target = if shape.fixed[fixed].receiver {
                        receiver_mode = Some(shape.fixed[fixed].mode);
                        HirCallArgumentTarget::Receiver
                    } else {
                        HirCallArgumentTarget::Fixed(shape.fixed[fixed].index)
                    };
                    expected_type = Some(shape.fixed[fixed].ty);
                    expected_mode = if shape.fixed[fixed].receiver {
                        ParameterMode::Value
                    } else {
                        shape.fixed[fixed].mode
                    };
                } else if let Some((_, element)) = &shape.variadic {
                    expected_mode = ParameterMode::Value;
                    if spread {
                        target = HirCallArgumentTarget::VariadicSpread;
                        expected_type = Some(
                            self.program
                                .interner
                                .intrinsic(IntrinsicType::Array, vec![*element])?,
                        );
                    } else {
                        target = HirCallArgumentTarget::VariadicElement;
                        expected_type = Some(*element);
                    }
                } else {
                    self.emit(
                        self.sources.span(file, argument.range())?,
                        "E1102",
                        "call provides more arguments than the callable accepts",
                        Vec::new(),
                        None,
                    )?;
                    association_error = true;
                }
            }
            if mode != expected_mode && target != HirCallArgumentTarget::Invalid {
                self.emit(
                    self.sources.span(file, argument.range())?,
                    "E1407",
                    "call argument mode does not match its parameter",
                    Vec::new(),
                    None,
                )?;
            }
            let Some(expression) = argument
                .child_nodes()
                .find(|child| AstExpression::cast(*child).is_some())
            else {
                continue;
            };
            let contextual_expected =
                if let (Some(inference), Some(expected_type)) = (&mut inference, expected_type) {
                    self.resolve_inference_type(&inference.solver, expected_type)?
                } else {
                    expected_type
                };
            let value = self.check_expression(
                file,
                expression,
                contextual_expected.map(ExpressionExpectation::Direct),
                context,
            )?;
            if let (Some(inference), Some(expected_type)) = (&mut inference, expected_type) {
                match self.constrain_inference_assignment(
                    &mut inference.solver,
                    expected_type,
                    self.expression_type(value),
                )? {
                    InferenceAssignment::Mismatch => {
                        inference.contradiction = true;
                        self.emit(
                            self.sources.span(file, argument.range())?,
                            "E1102",
                            "call argument contradicts the inferred parameter type",
                            Vec::new(),
                            None,
                        )?;
                    }
                    InferenceAssignment::Applied | InferenceAssignment::Ambiguous => {}
                }
            }
            if let Some(receiver_mode) = receiver_mode {
                self.check_method_receiver(value, receiver_mode, None, context)?;
            }
            arguments.push(HirCallArgument {
                label,
                mode: receiver_mode.unwrap_or(mode),
                spread,
                target,
                value,
            });
        }
        if !association_error {
            let missing = shape
                .fixed
                .iter()
                .zip(&provided)
                .filter_map(|(parameter, provided)| (!provided).then_some(parameter))
                .map(|parameter| {
                    parameter
                        .name
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| "<positional>".to_owned())
                })
                .collect::<Vec<_>>();
            if !missing.is_empty() {
                self.emit(
                    self.sources.span(file, suffix.range())?,
                    "E1102",
                    format!("call is missing fixed parameters: {}", missing.join(", ")),
                    Vec::new(),
                    None,
                )?;
                association_error = true;
            }
        }
        if let Some(mut generic) = inference {
            generic.contradiction |= association_error;
            let type_arguments = match generic.solver.finish(
                &mut self.program.interner,
                generic.variables.iter().copied(),
            ) {
                Ok(arguments) => arguments,
                Err(InferenceError::Type(error)) => return Err(error.into()),
                Err(
                    InferenceError::Unsolved(_)
                    | InferenceError::Mismatch { .. }
                    | InferenceError::RecursiveSolution { .. },
                ) => {
                    if !generic.contradiction {
                        self.emit(
                            self.sources.span(file, suffix.range())?,
                            "E1101",
                            "generic call does not provide enough unambiguous type information",
                            Vec::new(),
                            None,
                        )?;
                    }
                    return self.recovery_expression(file, range);
                }
            };
            if self.callable(generic.callable).is_some_and(|callable| {
                callable.generics.iter().any(|item| !item.bounds.is_empty())
            }) {
                self.complete = false;
            }
            let function_type = self
                .callable(generic.callable)
                .expect("an inferred callable remains indexed")
                .function_type;
            let final_type = TypeSubstitution::new(type_arguments.clone())
                .apply(&mut self.program.interner, function_type)?;
            let final_function = match self.program.interner.kind(final_type)?.clone() {
                TypeKind::Function(function) => function,
                _ => unreachable!("callable signatures always lower to function types"),
            };
            let callee_span = self.program.expressions[callee.0 as usize].span;
            callee = self.allocate_expression(HirExpression {
                span: callee_span,
                ty: final_type,
                category: HirValueCategory::Value,
                kind: HirExpressionKind::SpecializedFunction {
                    callable: generic.callable,
                    arguments: type_arguments,
                },
            })?;
            let full_shape = self.call_shape(callee, &final_function, false);
            for argument in &mut arguments {
                let final_expected = match argument.target {
                    HirCallArgumentTarget::Receiver => full_shape
                        .fixed
                        .iter()
                        .find(|parameter| parameter.receiver)
                        .map(|parameter| parameter.ty),
                    HirCallArgumentTarget::Fixed(index) => full_shape
                        .fixed
                        .iter()
                        .find(|parameter| parameter.index == index)
                        .map(|parameter| parameter.ty),
                    HirCallArgumentTarget::VariadicElement => {
                        full_shape.variadic.as_ref().map(|(_, element)| *element)
                    }
                    HirCallArgumentTarget::VariadicSpread => full_shape
                        .variadic
                        .as_ref()
                        .map(|(_, element)| {
                            self.program
                                .interner
                                .intrinsic(IntrinsicType::Array, vec![*element])
                        })
                        .transpose()?,
                    HirCallArgumentTarget::Invalid => None,
                };
                let Some(final_expected) = final_expected else {
                    continue;
                };
                let actual = self.expression_type(argument.value);
                if actual == self.program.interner.error() {
                    continue;
                }
                let Some(assignability) = self
                    .program
                    .interner
                    .assignability(actual, final_expected)?
                else {
                    if !generic.contradiction {
                        self.emit(
                            self.program.expressions[argument.value.0 as usize].span,
                            "E1102",
                            "call argument does not match the inferred parameter type",
                            Vec::new(),
                            None,
                        )?;
                        generic.contradiction = true;
                    }
                    continue;
                };
                if assignability != Assignability::Exact {
                    argument.value = self.coerce_existing(argument.value, final_expected)?;
                }
            }
            shape = self.call_shape(callee, &final_function, bound_receiver.is_some());
        }
        self.allocate_expression(HirExpression {
            span: self.sources.span(file, range)?,
            ty: shape.outcome,
            category: HirValueCategory::Value,
            kind: HirExpressionKind::Call { callee, arguments },
        })
    }

    fn resolve_inference_type(
        &mut self,
        solver: &InferenceContext,
        ty: TypeId,
    ) -> Result<Option<TypeId>, HirError> {
        match solver.resolve(&mut self.program.interner, ty) {
            Ok(resolved) => Ok(Some(resolved)),
            Err(InferenceError::Unsolved(_)) => Ok(None),
            Err(InferenceError::Type(error)) => Err(error.into()),
            Err(InferenceError::Mismatch { .. } | InferenceError::RecursiveSolution { .. }) => {
                Ok(None)
            }
        }
    }

    fn inference_head(
        &self,
        solver: &InferenceContext,
        mut ty: TypeId,
    ) -> Result<TypeId, HirError> {
        let mut visited = BTreeSet::new();
        loop {
            let TypeKind::Inference(inference) = self.program.interner.kind(ty)? else {
                return Ok(ty);
            };
            if !visited.insert(*inference) {
                return Ok(ty);
            }
            let Some(solution) = solver.solution(*inference) else {
                return Ok(ty);
            };
            ty = solution;
        }
    }

    fn try_inference_equation(
        &self,
        solver: &InferenceContext,
        left: TypeId,
        right: TypeId,
    ) -> Result<Option<InferenceContext>, HirError> {
        let mut candidate = solver.clone();
        match candidate.equate(&self.program.interner, left, right) {
            Ok(()) => Ok(Some(candidate)),
            Err(InferenceError::Type(error)) => Err(error.into()),
            Err(
                InferenceError::Mismatch { .. }
                | InferenceError::RecursiveSolution { .. }
                | InferenceError::Unsolved(_),
            ) => Ok(None),
        }
    }

    fn constrain_inference_assignment(
        &self,
        solver: &mut InferenceContext,
        expected: TypeId,
        actual: TypeId,
    ) -> Result<InferenceAssignment, HirError> {
        match self.program.interner.kind(actual)? {
            TypeKind::Error | TypeKind::Scalar(ScalarType::Never) => {
                return Ok(InferenceAssignment::Applied);
            }
            _ => {}
        }
        if let Some(candidate) = self.try_inference_equation(solver, expected, actual)? {
            *solver = candidate;
            return Ok(InferenceAssignment::Applied);
        }
        let head = self.inference_head(solver, expected)?;
        match self.program.interner.kind(head)?.clone() {
            TypeKind::Option(item) => {
                if let Some(candidate) = self.try_inference_equation(solver, item, actual)? {
                    *solver = candidate;
                    Ok(InferenceAssignment::Applied)
                } else {
                    Ok(InferenceAssignment::Mismatch)
                }
            }
            TypeKind::Union(members) => {
                let mut matches = members
                    .into_iter()
                    .filter_map(|member| {
                        self.try_inference_equation(solver, member, actual)
                            .transpose()
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                match matches.len() {
                    0 => Ok(InferenceAssignment::Mismatch),
                    1 => {
                        *solver = matches.pop().expect("one union candidate remains");
                        Ok(InferenceAssignment::Applied)
                    }
                    _ => Ok(InferenceAssignment::Ambiguous),
                }
            }
            _ => Ok(InferenceAssignment::Mismatch),
        }
    }

    fn call_shape(
        &self,
        callee: HirExpressionId,
        function: &crate::types::FunctionType,
        bound_receiver: bool,
    ) -> CallShape {
        let callable_id = match self.program.expressions[callee.0 as usize].kind() {
            HirExpressionKind::Function(id)
            | HirExpressionKind::SpecializedFunction { callable: id, .. } => Some(*id),
            _ => None,
        };
        if let Some(callable) = callable_id.and_then(|id| self.callable(id)) {
            let mut concrete = function.parameters().iter();
            let mut fixed = Vec::new();
            for (index, parameter) in callable.parameters.iter().enumerate() {
                if parameter.variadic_element.is_some() {
                    continue;
                }
                let concrete = concrete
                    .next()
                    .expect("callable HIR and function types retain matching fixed parameters");
                if bound_receiver && parameter.receiver {
                    continue;
                }
                fixed.push(CallParameterInfo {
                    index: u32::try_from(index).expect("call parameter counts fit in u32"),
                    name: (!parameter.discard)
                        .then_some(parameter.local)
                        .flatten()
                        .and_then(|local| self.resolved.local(local))
                        .map(|local| local.name().clone()),
                    mode: concrete.mode(),
                    ty: concrete.ty(),
                    receiver: parameter.receiver,
                });
            }
            let variadic = callable.parameters.iter().find_map(|parameter| {
                parameter.variadic_element?;
                let name = parameter
                    .local
                    .and_then(|local| self.resolved.local(local))
                    .map(|local| local.name().clone());
                Some((name, function.variadic()?))
            });
            return CallShape {
                fixed,
                variadic,
                outcome: function.outcome(),
            };
        }
        CallShape {
            fixed: function
                .parameters()
                .iter()
                .enumerate()
                .map(|(index, parameter)| CallParameterInfo {
                    index: u32::try_from(index).expect("call parameter counts fit in u32"),
                    name: None,
                    mode: parameter.mode(),
                    ty: parameter.ty(),
                    receiver: false,
                })
                .collect(),
            variadic: function.variadic().map(|element| (None, element)),
            outcome: function.outcome(),
        }
    }

    fn check_method_receiver(
        &mut self,
        receiver: HirExpressionId,
        mode: ParameterMode,
        expected: Option<TypeId>,
        context: &BodyContext,
    ) -> Result<(), HirError> {
        let receiver_expression = self
            .program
            .expression(receiver)
            .expect("allocated receiver expressions remain indexed");
        let receiver_span = receiver_expression.span();
        let receiver_type = receiver_expression.ty();
        if let Some(expected) = expected
            && self
                .program
                .interner
                .assignability(receiver_type, expected)?
                .is_none()
        {
            self.emit(
                receiver_span,
                "E1102",
                "method receiver does not match its declared owner type",
                Vec::new(),
                None,
            )?;
        }
        let permission = self.expression_place_permission(receiver, context);
        let allowed = match mode {
            ParameterMode::Value | ParameterMode::Ref => true,
            ParameterMode::Mut => matches!(
                permission,
                PlacePermission::MutRoot | PlacePermission::Replace
            ),
            ParameterMode::Var => permission == PlacePermission::Replace,
        };
        if !allowed {
            self.emit(
                receiver_span,
                "E1407",
                match mode {
                    ParameterMode::Mut => {
                        "a `mut self` method requires a mutable writable receiver place"
                    }
                    ParameterMode::Var => {
                        "a `var self` method requires a structurally replaceable receiver place"
                    }
                    ParameterMode::Value | ParameterMode::Ref => unreachable!(),
                },
                Vec::new(),
                None,
            )?;
        }
        Ok(())
    }

    fn expression_place_permission(
        &self,
        expression: HirExpressionId,
        context: &BodyContext,
    ) -> PlacePermission {
        let Some(expression) = self.program.expression(expression) else {
            return PlacePermission::Invalid;
        };
        match expression.kind() {
            HirExpressionKind::Local(local) => context
                .local_permissions
                .get(local)
                .copied()
                .unwrap_or(PlacePermission::Immutable),
            HirExpressionKind::Receiver => context.receiver_permission,
            HirExpressionKind::Field { base, .. }
            | HirExpressionKind::TupleField { base, .. }
            | HirExpressionKind::Index { base, .. }
            | HirExpressionKind::Slice { base, .. } => {
                self.expression_place_permission(*base, context).projected()
            }
            _ => PlacePermission::Immutable,
        }
    }

    fn contextual_numeric_scalar(
        &mut self,
        expected: Option<TypeId>,
        default: ScalarType,
        predicate: fn(ScalarType) -> bool,
        span: Span,
    ) -> Result<Option<ScalarType>, HirError> {
        let Some(expected) = expected else {
            return Ok(Some(default));
        };
        let expected = match self.program.interner.kind(expected)? {
            TypeKind::Option(item) => *item,
            _ => expected,
        };
        match self.program.interner.kind(expected)? {
            TypeKind::Scalar(scalar) if predicate(*scalar) => Ok(Some(*scalar)),
            TypeKind::Union(members) => {
                let default_type = self.program.interner.scalar(default);
                if members.contains(&default_type) {
                    return Ok(Some(default));
                }
                let candidates = members
                    .iter()
                    .filter_map(|member| match self.program.interner.kind(*member).ok()? {
                        TypeKind::Scalar(scalar) if predicate(*scalar) => Some(*scalar),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                if candidates.len() == 1 {
                    Ok(Some(candidates[0]))
                } else if candidates.len() > 1 {
                    self.emit_missing_context(
                        span,
                        "numeric literal has more than one contextual union member",
                    )?;
                    Ok(None)
                } else {
                    Ok(Some(default))
                }
            }
            _ => Ok(Some(default)),
        }
    }

    fn check_negative_integer(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'_>,
        operand_node: SyntaxNodeRef<'_>,
        token: SyntaxTokenRef<'_>,
        expected: Option<TypeId>,
    ) -> Result<HirExpressionId, HirError> {
        let spelling = self.token_text(file, token)?.to_owned();
        let Some(magnitude) = integer_magnitude(&spelling) else {
            self.emit(
                self.sources.span(file, token.range())?,
                "E1102",
                "integer literal exceeds the intrinsic numeric domain",
                Vec::new(),
                None,
            )?;
            return self.recovery_expression(file, node.range());
        };
        let scalar = if let Some(suffix) = integer_suffix(&spelling) {
            suffix
        } else {
            let Some(scalar) = self.contextual_numeric_scalar(
                expected,
                ScalarType::Int,
                is_integer_scalar,
                self.sources.span(file, token.range())?,
            )?
            else {
                return self.recovery_expression(file, node.range());
            };
            scalar
        };
        let Some((signed, bits)) = integer_shape(scalar) else {
            return self.recovery_expression(file, node.range());
        };
        let maximum_magnitude = 1_u128 << (bits - 1);
        if !signed || magnitude > maximum_magnitude {
            self.emit(
                self.sources.span(file, node.range())?,
                "E1102",
                format!("negative integer literal is not representable as `{scalar}`"),
                Vec::new(),
                Some((scalar.to_string(), "negative integer literal".into())),
            )?;
            return self.recovery_expression(file, node.range());
        }
        let ty = self.program.interner.scalar(scalar);
        let operand = self.allocate_expression(HirExpression {
            span: self.sources.span(file, operand_node.range())?,
            ty,
            category: HirValueCategory::Value,
            kind: HirExpressionKind::Literal(HirLiteral::Integer(spelling)),
        })?;
        self.allocate_expression(HirExpression {
            span: self.sources.span(file, node.range())?,
            ty,
            category: HirValueCategory::Value,
            kind: HirExpressionKind::Prefix {
                operator: HirPrefixOperator::Negate,
                operand,
            },
        })
    }

    fn callable(&self, id: HirCallableId) -> Option<&HirCallableSignature> {
        self.program
            .callables
            .iter()
            .find(|callable| callable.id == id)
    }

    fn expression_type(&self, id: HirExpressionId) -> TypeId {
        self.program.expressions[id.0 as usize].ty
    }

    fn expression_flow(&self, id: HirExpressionId) -> HirFlow {
        self.program.expression_flows[id.0 as usize]
    }

    fn expression_summary(&self, id: HirExpressionId) -> FlowSummary {
        FlowSummary {
            flow: self.program.expression_flows[id.0 as usize],
            breaks: self.program.expression_breaks[id.0 as usize]
                .iter()
                .copied()
                .collect(),
        }
    }

    fn expression_sequence(
        &self,
        expressions: impl IntoIterator<Item = HirExpressionId>,
    ) -> FlowSummary {
        expressions
            .into_iter()
            .fold(FlowSummary::completes(), |summary, expression| {
                summary.then(self.expression_summary(expression))
            })
    }

    fn assignment_target_summary(&self, target: &HirAssignmentTarget) -> FlowSummary {
        match &target.kind {
            HirAssignmentTargetKind::Place { place, .. } => self.expression_summary(*place),
            HirAssignmentTargetKind::Discard => FlowSummary::completes(),
            HirAssignmentTargetKind::Tuple(items) => items
                .iter()
                .fold(FlowSummary::completes(), |summary, item| {
                    summary.then(self.assignment_target_summary(item))
                }),
        }
    }

    fn statement_summary(&self, statement: &HirStatement) -> FlowSummary {
        match statement {
            HirStatement::Binding { value, .. }
            | HirStatement::Expression { value, .. }
            | HirStatement::Discard { value, .. } => self.expression_summary(*value),
            HirStatement::Assignment { target, value, .. } => self
                .assignment_target_summary(target)
                .then(self.expression_summary(*value)),
            HirStatement::For { id, kind, body, .. } => {
                let header = match kind {
                    HirForKind::Infinite => FlowSummary::completes(),
                    HirForKind::Conditional { condition } => self.expression_summary(*condition),
                    HirForKind::Iterate { source, .. } => self.expression_summary(*source),
                };
                if !header.flow.may_complete() {
                    return header;
                }

                let mut body = self.expression_summary(*body);
                let can_break = body.breaks.remove(id);
                let mut breaks = header.breaks;
                breaks.extend(body.breaks);
                let flow = match kind {
                    HirForKind::Infinite if !can_break => HirFlow::Diverges,
                    HirForKind::Infinite
                    | HirForKind::Conditional { .. }
                    | HirForKind::Iterate { .. } => HirFlow::MayComplete,
                };
                FlowSummary { flow, breaks }
            }
        }
    }

    fn summarize_expression(&self, expression: &HirExpression) -> FlowSummary {
        match &expression.kind {
            HirExpressionKind::Recovery
            | HirExpressionKind::Literal(_)
            | HirExpressionKind::Local(_)
            | HirExpressionKind::Constant(_)
            | HirExpressionKind::Function(_)
            | HirExpressionKind::SpecializedFunction { .. }
            | HirExpressionKind::Receiver => FlowSummary::completes(),
            HirExpressionKind::Tuple(items)
            | HirExpressionKind::Array(items)
            | HirExpressionKind::Set(items)
            | HirExpressionKind::InterpolatedString { values: items, .. } => {
                self.expression_sequence(items.iter().copied())
            }
            HirExpressionKind::Map(entries) => self.expression_sequence(
                entries
                    .iter()
                    .flat_map(|entry| [entry.key(), entry.value()]),
            ),
            HirExpressionKind::Newtype { value, .. } => self.expression_summary(*value),
            HirExpressionKind::NumericConversion { value, .. } => self.expression_summary(*value),
            HirExpressionKind::Record { fields, .. } => {
                self.expression_sequence(fields.iter().map(HirRecordFieldValue::value))
            }
            HirExpressionKind::Variant { payload, .. } => match payload {
                HirVariantValue::Unit => FlowSummary::completes(),
                HirVariantValue::Tuple(values) => self.expression_sequence(values.iter().copied()),
                HirVariantValue::Record(fields) => {
                    self.expression_sequence(fields.iter().map(HirRecordFieldValue::value))
                }
            },
            HirExpressionKind::RecordUpdate { base, fields } => self
                .expression_summary(*base)
                .then(self.expression_sequence(fields.iter().map(HirRecordFieldValue::value))),
            HirExpressionKind::Block { statements, tail } => {
                let summary = statements
                    .iter()
                    .fold(FlowSummary::completes(), |summary, statement| {
                        summary.then(self.statement_summary(statement))
                    });
                if let Some(tail) = tail {
                    summary.then(self.expression_summary(*tail))
                } else {
                    summary
                }
            }
            HirExpressionKind::Prefix { operand, .. }
            | HirExpressionKind::Field { base: operand, .. }
            | HirExpressionKind::TupleField { base: operand, .. }
            | HirExpressionKind::OptionSome { value: operand }
            | HirExpressionKind::ResultOk { value: operand }
            | HirExpressionKind::ResultErr { error: operand }
            | HirExpressionKind::Coerce { value: operand, .. } => self.expression_summary(*operand),
            HirExpressionKind::Binary {
                operator,
                left,
                right,
            } => {
                let mut left = self.expression_summary(*left);
                if !left.flow.may_complete() {
                    return left;
                }
                let right = self.expression_summary(*right);
                left.breaks.extend(right.breaks);
                left.flow = if matches!(
                    operator,
                    HirBinaryOperator::LogicalAnd | HirBinaryOperator::LogicalOr
                ) {
                    HirFlow::MayComplete
                } else {
                    right.flow
                };
                left
            }
            HirExpressionKind::Range { start, end, .. } => self
                .expression_summary(*start)
                .then(self.expression_summary(*end)),
            HirExpressionKind::Contains {
                item, container, ..
            } => self
                .expression_summary(*item)
                .then(self.expression_summary(*container)),
            HirExpressionKind::Index { base, index, .. } => self
                .expression_summary(*base)
                .then(self.expression_summary(*index)),
            HirExpressionKind::Slice {
                base,
                start,
                end,
                step,
            } => self.expression_sequence(
                std::iter::once(*base)
                    .chain(start.iter().copied())
                    .chain(end.iter().copied())
                    .chain(step.iter().copied()),
            ),
            HirExpressionKind::Call { callee, arguments } => {
                let mut summary = self.expression_sequence(
                    std::iter::once(*callee).chain(arguments.iter().map(HirCallArgument::value)),
                );
                if summary.flow.may_complete()
                    && expression.ty == self.program.interner.scalar(ScalarType::Never)
                {
                    summary.flow = HirFlow::Diverges;
                }
                summary
            }
            HirExpressionKind::PreludePanic { message } => {
                let mut summary = self.expression_summary(*message);
                if summary.flow.may_complete() {
                    summary.flow = HirFlow::Diverges;
                }
                summary
            }
            HirExpressionKind::PreludeAssert {
                condition,
                message_parts,
            } => self.expression_sequence(
                std::iter::once(*condition).chain(message_parts.iter().map(|part| part.value())),
            ),
            HirExpressionKind::BootstrapHostCall { arguments, .. } => {
                self.expression_sequence(arguments.iter().copied())
            }
            HirExpressionKind::PropagateOption { value }
            | HirExpressionKind::PropagateResult { value, .. } => {
                let mut summary = self.expression_summary(*value);
                if summary.flow.may_complete()
                    && expression.ty == self.program.interner.scalar(ScalarType::Never)
                {
                    summary.flow = HirFlow::Diverges;
                }
                summary
            }
            HirExpressionKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let mut condition = self.expression_summary(*condition);
                if !condition.flow.may_complete() {
                    return condition;
                }
                let then_branch = self.expression_summary(*then_branch);
                condition.breaks.extend(then_branch.breaks);
                if let Some(else_branch) = else_branch {
                    let else_branch = self.expression_summary(*else_branch);
                    condition.breaks.extend(else_branch.breaks);
                    condition.flow =
                        if then_branch.flow.may_complete() || else_branch.flow.may_complete() {
                            HirFlow::MayComplete
                        } else {
                            HirFlow::Diverges
                        };
                } else {
                    condition.flow = HirFlow::MayComplete;
                }
                condition
            }
            HirExpressionKind::Match { scrutinee, arms } => {
                let mut summary = self.expression_summary(*scrutinee);
                if !summary.flow.may_complete() {
                    return summary;
                }
                let mut may_complete = false;
                for arm in arms {
                    let guard = arm
                        .guard
                        .map(|guard| self.expression_summary(guard))
                        .unwrap_or_else(FlowSummary::completes);
                    summary.breaks.extend(guard.breaks.iter().copied());
                    if !guard.flow.may_complete() {
                        continue;
                    }
                    let body = self.expression_summary(arm.body);
                    summary.breaks.extend(body.breaks);
                    may_complete |= body.flow.may_complete();
                }
                summary.flow = if may_complete {
                    HirFlow::MayComplete
                } else {
                    HirFlow::Diverges
                };
                summary
            }
            HirExpressionKind::Return { value } => {
                let mut summary = value
                    .map(|value| self.expression_summary(value))
                    .unwrap_or_else(FlowSummary::completes);
                if summary.flow.may_complete() {
                    summary.flow = HirFlow::Diverges;
                }
                summary
            }
            HirExpressionKind::Fail { error } => {
                let mut summary = self.expression_summary(*error);
                if summary.flow.may_complete() {
                    summary.flow = HirFlow::Diverges;
                }
                summary
            }
            HirExpressionKind::Break { target } => match target {
                Some(target) => FlowSummary {
                    flow: HirFlow::Diverges,
                    breaks: BTreeSet::from([*target]),
                },
                None => FlowSummary::completes(),
            },
            HirExpressionKind::Continue { target } => {
                if target.is_some() {
                    FlowSummary::diverges()
                } else {
                    FlowSummary::completes()
                }
            }
        }
    }

    fn allocate_expression(
        &mut self,
        expression: HirExpression,
    ) -> Result<HirExpressionId, HirError> {
        self.check_node_budget(expression.span)?;
        let summary = self.summarize_expression(&expression);
        let index =
            u32::try_from(self.program.expressions.len()).map_err(|_| HirError::NodeLimit {
                file: expression.span.file(),
                offset: expression.span.range().start(),
            })?;
        let id = HirExpressionId(index);
        self.program.expressions.push(expression);
        self.program.expression_flows.push(summary.flow);
        self.program
            .expression_breaks
            .push(summary.breaks.into_iter().collect());
        Ok(id)
    }

    fn allocate_pattern(&mut self, pattern: HirPattern) -> Result<HirPatternId, HirError> {
        self.check_node_budget(pattern.span)?;
        let index =
            u32::try_from(self.program.patterns.len()).map_err(|_| HirError::NodeLimit {
                file: pattern.span.file(),
                offset: pattern.span.range().start(),
            })?;
        let id = HirPatternId(index);
        self.program.patterns.push(pattern);
        Ok(id)
    }

    fn record_member_reference(&mut self, span: Span, member: MemberId) {
        self.program
            .member_references
            .push(HirMemberReference { member, span });
    }

    fn check_node_budget(&self, span: Span) -> Result<(), HirError> {
        let used = self.program.expressions.len() as u64 + self.program.patterns.len() as u64;
        if used >= u64::from(self.max_nodes) {
            return Err(HirError::NodeLimit {
                file: span.file(),
                offset: span.range().start(),
            });
        }
        Ok(())
    }

    fn recovery_expression(
        &mut self,
        file: FileId,
        range: TextRange,
    ) -> Result<HirExpressionId, HirError> {
        self.allocate_expression(HirExpression {
            span: self.sources.span(file, range)?,
            ty: self.program.interner.error(),
            category: HirValueCategory::Value,
            kind: HirExpressionKind::Recovery,
        })
    }

    fn find_node(&self, span: Span, kind: Option<SyntaxKind>) -> Option<SyntaxNodeRef<'a>> {
        let parsed = self.parsed.get(&span.file())?;
        let mut pending = vec![parsed.cst().root_node()];
        while let Some(node) = pending.pop() {
            if node.range() == span.range() && kind.is_none_or(|kind| node.kind() == kind) {
                return Some(node);
            }
            pending.extend(node.child_nodes());
        }
        None
    }

    fn token_text<'s>(
        &'s self,
        file: FileId,
        token: SyntaxTokenRef<'_>,
    ) -> Result<&'s str, HirError> {
        self.source_text(file, token.range())
    }

    fn source_text(&self, file: FileId, range: TextRange) -> Result<&str, HirError> {
        let source = self.sources.get(file)?;
        let text = source
            .text()
            .expect("expression checking runs only after UTF-8 validation");
        Ok(&text[range.start() as usize..range.end() as usize])
    }

    fn emit_missing_context(&mut self, span: Span, message: &str) -> Result<(), HirError> {
        self.emit(span, "E1101", message, Vec::new(), None)
    }

    fn emit_invalid_operator(
        &mut self,
        file: FileId,
        range: TextRange,
        left: TypeId,
        right: Option<TypeId>,
    ) -> Result<(), HirError> {
        let left = self.program.interner.canonical(left)?;
        let message = if let Some(right) = right {
            format!(
                "operator is not defined for `{left}` and `{}`",
                self.program.interner.canonical(right)?
            )
        } else {
            format!("operator is not defined for `{left}`")
        };
        self.emit(
            self.sources.span(file, range)?,
            "E1102",
            message,
            Vec::new(),
            None,
        )
    }

    fn emit(
        &mut self,
        span: Span,
        code: &str,
        message: impl Into<String>,
        related: Vec<(&str, Span)>,
        expected_actual: Option<(String, String)>,
    ) -> Result<(), HirError> {
        self.emit_with_severity(
            Severity::Error,
            span,
            code,
            message,
            related,
            expected_actual,
        )
    }

    fn emit_warning(
        &mut self,
        span: Span,
        code: &str,
        message: impl Into<String>,
    ) -> Result<(), HirError> {
        self.emit_with_severity(Severity::Warning, span, code, message, Vec::new(), None)
    }

    fn emit_with_severity(
        &mut self,
        severity: Severity,
        span: Span,
        code: &str,
        message: impl Into<String>,
        related: Vec<(&str, Span)>,
        expected_actual: Option<(String, String)>,
    ) -> Result<(), HirError> {
        if self.diagnostics.len() >= self.max_diagnostics {
            return Err(HirError::DiagnosticLimit {
                file: span.file(),
                offset: span.range().start(),
            });
        }
        let mut diagnostic = Diagnostic::new(
            severity,
            DiagnosticCode::new(code)?,
            message,
            PrimaryLocation::Source(span),
        )?;
        if let Some((expected, actual)) = expected_actual {
            diagnostic = diagnostic.with_expected_actual(Some(expected), Some(actual));
        }
        for (message, span) in related {
            diagnostic = diagnostic.with_related(Related::new(message, span)?);
        }
        self.diagnostics.push(diagnostic);
        Ok(())
    }
}

fn is_option_type(kind: &TypeKind) -> bool {
    matches!(kind, TypeKind::Option(_))
}

fn is_result_type(kind: &TypeKind) -> bool {
    matches!(kind, TypeKind::Result { .. })
}

fn is_array_type(kind: &TypeKind) -> bool {
    matches!(
        kind,
        TypeKind::Intrinsic {
            constructor: IntrinsicType::Array,
            ..
        }
    )
}

fn specialize_pattern_matrix(
    matrix: &[Vec<PatternShape>],
    constructor: &PatternConstructor,
    arity: usize,
) -> Vec<Vec<PatternShape>> {
    let mut specialized = Vec::new();
    for row in matrix {
        let Some(first) = row.first() else {
            continue;
        };
        match normalize_pattern_head(first) {
            PatternShape::Wildcard => {
                let mut next = vec![PatternShape::Wildcard; arity];
                next.extend_from_slice(&row[1..]);
                specialized.push(next);
            }
            PatternShape::Constructor { key, arguments } if &key == constructor => {
                let mut next = arguments;
                next.extend_from_slice(&row[1..]);
                specialized.push(next);
            }
            PatternShape::Constructor { .. } => {}
            PatternShape::Array { .. } => {
                unreachable!("array pattern heads normalize to list constructors")
            }
        }
    }
    specialized
}

fn default_pattern_matrix(matrix: &[Vec<PatternShape>]) -> Vec<Vec<PatternShape>> {
    matrix
        .iter()
        .filter(|row| {
            row.first().is_some_and(|pattern| {
                matches!(normalize_pattern_head(pattern), PatternShape::Wildcard)
            })
        })
        .map(|row| row[1..].to_vec())
        .collect()
}

fn normalize_pattern_head(pattern: &PatternShape) -> PatternShape {
    let PatternShape::Array {
        elements,
        offset,
        has_rest,
    } = pattern
    else {
        return pattern.clone();
    };
    if *offset < elements.len() {
        PatternShape::Constructor {
            key: PatternConstructor::ArrayCons,
            arguments: vec![
                elements[*offset].clone(),
                PatternShape::Array {
                    elements: Arc::clone(elements),
                    offset: offset + 1,
                    has_rest: *has_rest,
                },
            ],
        }
    } else if *has_rest {
        PatternShape::Wildcard
    } else {
        PatternShape::Constructor {
            key: PatternConstructor::ArrayEmpty,
            arguments: Vec::new(),
        }
    }
}

fn binary_operator(token: TokenKind) -> Option<HirBinaryOperator> {
    Some(match token {
        TokenKind::Star => HirBinaryOperator::Multiply,
        TokenKind::Slash => HirBinaryOperator::Divide,
        TokenKind::Percent => HirBinaryOperator::Remainder,
        TokenKind::Plus => HirBinaryOperator::Add,
        TokenKind::Minus => HirBinaryOperator::Subtract,
        TokenKind::Shl => HirBinaryOperator::ShiftLeft,
        TokenKind::Shr => HirBinaryOperator::ShiftRight,
        TokenKind::Amp => HirBinaryOperator::BitwiseAnd,
        TokenKind::Caret => HirBinaryOperator::BitwiseXor,
        TokenKind::Pipe => HirBinaryOperator::BitwiseOr,
        TokenKind::Less => HirBinaryOperator::Less,
        TokenKind::LessEq => HirBinaryOperator::LessEqual,
        TokenKind::Greater => HirBinaryOperator::Greater,
        TokenKind::GreaterEq => HirBinaryOperator::GreaterEqual,
        TokenKind::EqEq => HirBinaryOperator::Equal,
        TokenKind::BangEq => HirBinaryOperator::NotEqual,
        TokenKind::And => HirBinaryOperator::LogicalAnd,
        TokenKind::Or => HirBinaryOperator::LogicalOr,
        _ => return None,
    })
}

fn assignment_operator(token: TokenKind) -> Option<HirAssignmentOperator> {
    Some(match token {
        TokenKind::Eq => HirAssignmentOperator::Assign,
        TokenKind::PlusEq => HirAssignmentOperator::Add,
        TokenKind::MinusEq => HirAssignmentOperator::Subtract,
        TokenKind::StarEq => HirAssignmentOperator::Multiply,
        TokenKind::SlashEq => HirAssignmentOperator::Divide,
        TokenKind::PercentEq => HirAssignmentOperator::Remainder,
        TokenKind::AmpEq => HirAssignmentOperator::BitwiseAnd,
        TokenKind::CaretEq => HirAssignmentOperator::BitwiseXor,
        TokenKind::PipeEq => HirAssignmentOperator::BitwiseOr,
        TokenKind::ShlEq => HirAssignmentOperator::ShiftLeft,
        TokenKind::ShrEq => HirAssignmentOperator::ShiftRight,
        _ => return None,
    })
}

fn assignment_write_kind(place: &CheckedPlace) -> HirWriteKind {
    if place.slice || place.permission == PlacePermission::MutRoot {
        HirWriteKind::PreserveExtent
    } else {
        HirWriteKind::Replace
    }
}

fn collect_assignment_places<'a>(
    target: &'a CheckedAssignmentTarget,
    output: &mut Vec<(&'a StaticPlace, Span)>,
) {
    match &target.kind {
        CheckedAssignmentTargetKind::Place(place)
            if matches!(
                place.permission,
                PlacePermission::MutRoot | PlacePermission::Replace
            ) && (!place.map_entry || place.permission == PlacePermission::Replace) =>
        {
            output.push((&place.key, target.span));
        }
        CheckedAssignmentTargetKind::Place(_) => {}
        CheckedAssignmentTargetKind::Discard => {}
        CheckedAssignmentTargetKind::Tuple(items) => {
            for item in items {
                collect_assignment_places(item, output);
            }
        }
    }
}

fn collect_assignment_target_expressions(
    target: &HirAssignmentTarget,
    output: &mut Vec<HirExpressionId>,
) {
    match target.kind() {
        HirAssignmentTargetKind::Place { place, .. } => output.push(*place),
        HirAssignmentTargetKind::Discard => {}
        HirAssignmentTargetKind::Tuple(items) => {
            for item in items {
                collect_assignment_target_expressions(item, output);
            }
        }
    }
}

fn nominal_discard_roots(shape: &HirNominalShape) -> Vec<TypeId> {
    match shape {
        HirNominalShape::Newtype { underlying } => vec![*underlying],
        HirNominalShape::Record { fields } => fields.iter().map(HirField::ty).collect(),
        HirNominalShape::Enum { variants } => variants
            .iter()
            .flat_map(|variant| match variant.payload() {
                HirVariantPayload::Unit => Vec::new(),
                HirVariantPayload::Tuple(types) => types.clone(),
                HirVariantPayload::Record(fields) => fields.iter().map(HirField::ty).collect(),
            })
            .collect(),
    }
}

fn static_places_overlap(left: &StaticPlace, right: &StaticPlace) -> bool {
    if left.root != right.root {
        return false;
    }
    for (left, right) in left.projections.iter().zip(&right.projections) {
        if left == right {
            continue;
        }
        return false;
    }
    true
}

fn direct_expression_child(node: SyntaxNodeRef<'_>) -> Option<SyntaxNodeRef<'_>> {
    node.child_nodes()
        .find(|child| AstExpression::cast(*child).is_some())
}

fn single_bracket_expression(bracket: SyntaxNodeRef<'_>) -> Option<SyntaxNodeRef<'_>> {
    let mut items = bracket
        .child_nodes()
        .filter(|child| child.kind() == SyntaxKind::BracketItem);
    let item = items.next()?;
    if items.next().is_some() {
        return None;
    }
    direct_expression_child(item)
}

fn integer_suffix(spelling: &str) -> Option<ScalarType> {
    [
        ("i16", ScalarType::Int16),
        ("i32", ScalarType::Int32),
        ("i64", ScalarType::Int),
        ("u16", ScalarType::UInt16),
        ("u32", ScalarType::UInt32),
        ("u64", ScalarType::UInt64),
        ("i8", ScalarType::Int8),
        ("u8", ScalarType::UInt8),
    ]
    .into_iter()
    .find_map(|(suffix, scalar)| spelling.ends_with(suffix).then_some(scalar))
}

fn float_suffix(spelling: &str) -> Option<ScalarType> {
    if spelling.ends_with("f32") {
        Some(ScalarType::Float32)
    } else if spelling.ends_with("f64") {
        Some(ScalarType::Float)
    } else {
        None
    }
}

fn decode_char_literal(spelling: &str) -> Option<String> {
    let body = spelling.strip_prefix('\'')?.strip_suffix('\'')?;
    let decoded = decode_escaped_text(body, false)?;
    (decoded.chars().count() == 1).then_some(decoded)
}

fn decode_string_literal_pattern(spelling: &str, kind: TokenKind) -> Option<String> {
    let (raw, multiline, opening, closing) = match kind {
        TokenKind::RawStringLiteral => (true, false, "r\"", "\""),
        TokenKind::RawMultilineStringLiteral => (true, true, "r\"\"\"", "\"\"\""),
        TokenKind::StringStart => (false, false, "\"", "\""),
        TokenKind::MultilineStringStart => (false, true, "\"\"\"", "\"\"\""),
        _ => return None,
    };
    let body = spelling.strip_prefix(opening)?.strip_suffix(closing)?;
    let body = if multiline {
        normalize_multiline_string(body)
    } else {
        body.to_owned()
    };
    if raw {
        Some(body)
    } else {
        decode_escaped_text(&body, true)
    }
}

fn normalize_static_literal(
    literal: &HirLiteral,
    ty: TypeId,
    scalar: Option<ScalarType>,
) -> Option<String> {
    match literal {
        HirLiteral::Unit => Some("unit".to_owned()),
        HirLiteral::Bool(value) => Some(value.to_string()),
        HirLiteral::Integer(value) => integer_magnitude(value).map(|value| value.to_string()),
        HirLiteral::Float(value) => normalize_float_pattern(value, false, scalar?),
        HirLiteral::Char(value) => decode_char_literal(value),
        HirLiteral::String(value) => {
            let kind = if value.starts_with("r\"\"\"") {
                TokenKind::RawMultilineStringLiteral
            } else if value.starts_with("r\"") {
                TokenKind::RawStringLiteral
            } else if value.starts_with("\"\"\"") {
                TokenKind::MultilineStringStart
            } else if value.starts_with('"') {
                TokenKind::StringStart
            } else {
                return None;
            };
            decode_string_literal_pattern(value, kind)
        }
        HirLiteral::None => Some(format!("none:{}", ty.index())),
    }
}

fn normalize_multiline_string(body: &str) -> String {
    let mut normalized = body.replace("\r\n", "\n");
    if normalized.starts_with('\n') {
        normalized.remove(0);
    }
    let line_start = normalized.rfind('\n').map_or(0, |index| index + 1);
    if !normalized[line_start..]
        .bytes()
        .all(|byte| matches!(byte, b' ' | b'\t'))
    {
        return normalized;
    }

    let prefix = normalized[line_start..].to_owned();
    normalized.truncate(if line_start == 0 { 0 } else { line_start - 1 });
    normalized
        .split('\n')
        .map(|line| {
            if line.bytes().all(|byte| matches!(byte, b' ' | b'\t')) {
                let common = line
                    .bytes()
                    .zip(prefix.bytes())
                    .take_while(|(left, right)| left == right)
                    .count();
                &line[common..]
            } else {
                line.strip_prefix(&prefix).unwrap_or(line)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn decode_escaped_text(body: &str, decode_braces: bool) -> Option<String> {
    let mut output = String::with_capacity(body.len());
    let mut characters = body.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\\' => {
                let escaped = characters.next()?;
                match escaped {
                    'n' => output.push('\n'),
                    'r' => output.push('\r'),
                    't' => output.push('\t'),
                    '\\' => output.push('\\'),
                    '\'' => output.push('\''),
                    '"' => output.push('"'),
                    '0' => output.push('\0'),
                    'u' => {
                        if characters.next()? != '{' {
                            return None;
                        }
                        let mut digits = String::new();
                        loop {
                            let digit = characters.next()?;
                            if digit == '}' {
                                break;
                            }
                            digits.push(digit);
                        }
                        if !(1..=6).contains(&digits.len()) {
                            return None;
                        }
                        let value = u32::from_str_radix(&digits, 16).ok()?;
                        output.push(char::from_u32(value)?);
                    }
                    _ => return None,
                }
            }
            '{' | '}' if decode_braces => {
                characters.next_if_eq(&character)?;
                output.push(character);
            }
            _ => output.push(character),
        }
    }
    Some(output)
}

fn integer_magnitude(spelling: &str) -> Option<u128> {
    let suffix_length = integer_suffix(spelling).map_or(0, |scalar| match scalar {
        ScalarType::Int8 | ScalarType::UInt8 => 2,
        _ => 3,
    });
    let body = &spelling[..spelling.len().checked_sub(suffix_length)?];
    let (radix, digits) = if let Some(digits) = body.strip_prefix("0b") {
        (2, digits)
    } else if let Some(digits) = body.strip_prefix("0o") {
        (8, digits)
    } else if let Some(digits) = body.strip_prefix("0x") {
        (16, digits)
    } else {
        (10, body)
    };
    u128::from_str_radix(&digits.replace('_', ""), radix).ok()
}

fn integer_fits_positive(value: u128, scalar: ScalarType) -> bool {
    let Some((signed, bits)) = integer_shape(scalar) else {
        return false;
    };
    let maximum = if signed {
        (1_u128 << (bits - 1)) - 1
    } else {
        (1_u128 << bits) - 1
    };
    value <= maximum
}

fn integer_shape(scalar: ScalarType) -> Option<(bool, u32)> {
    Some(match scalar {
        ScalarType::Int8 => (true, 8),
        ScalarType::Int16 => (true, 16),
        ScalarType::Int32 => (true, 32),
        ScalarType::Int => (true, 64),
        ScalarType::UInt8 => (false, 8),
        ScalarType::UInt16 => (false, 16),
        ScalarType::UInt32 => (false, 32),
        ScalarType::UInt64 => (false, 64),
        _ => return None,
    })
}

fn float_is_representable(spelling: &str, scalar: ScalarType) -> bool {
    let suffix_length = float_suffix(spelling).map_or(0, |_| 3);
    let normalized = spelling[..spelling.len() - suffix_length].replace('_', "");
    match scalar {
        ScalarType::Float32 => normalized.parse::<f32>().is_ok_and(f32::is_finite),
        ScalarType::Float => normalized.parse::<f64>().is_ok_and(f64::is_finite),
        _ => false,
    }
}

fn normalize_float_pattern(spelling: &str, negative: bool, scalar: ScalarType) -> Option<String> {
    let suffix_length = float_suffix(spelling).map_or(0, |_| 3);
    let mut normalized = spelling[..spelling.len().checked_sub(suffix_length)?].replace('_', "");
    if negative {
        normalized.insert(0, '-');
    }
    match scalar {
        ScalarType::Float32 => {
            let value = normalized.parse::<f32>().ok()?;
            let bits = if value == 0.0 { 0 } else { value.to_bits() };
            Some(format!("{bits:08x}"))
        }
        ScalarType::Float => {
            let value = normalized.parse::<f64>().ok()?;
            let bits = if value == 0.0 { 0 } else { value.to_bits() };
            Some(format!("{bits:016x}"))
        }
        _ => None,
    }
}

fn contains_syntax_kind(root: SyntaxNodeRef<'_>, kind: SyntaxKind) -> bool {
    let mut pending = root.child_nodes().collect::<Vec<_>>();
    while let Some(node) = pending.pop() {
        if node.kind() == kind {
            return true;
        }
        pending.extend(node.child_nodes());
    }
    false
}

fn is_integer_scalar(scalar: ScalarType) -> bool {
    integer_shape(scalar).is_some()
}

fn is_signed_integer_scalar(scalar: ScalarType) -> bool {
    integer_shape(scalar).is_some_and(|(signed, _)| signed)
}

fn is_float_scalar(scalar: ScalarType) -> bool {
    matches!(scalar, ScalarType::Float | ScalarType::Float32)
}

fn is_arithmetic_scalar(scalar: ScalarType) -> bool {
    is_integer_scalar(scalar) || is_float_scalar(scalar)
}

fn is_relational_scalar(scalar: ScalarType) -> bool {
    is_arithmetic_scalar(scalar)
        || matches!(
            scalar,
            ScalarType::Byte | ScalarType::Char | ScalarType::String
        )
}

fn is_bootstrap_equatable_scalar(scalar: ScalarType) -> bool {
    !matches!(scalar, ScalarType::Never)
}

fn strongly_connected_components(
    nodes: &[SymbolId],
    adjacency: &BTreeMap<SymbolId, Vec<SymbolId>>,
) -> Vec<Vec<SymbolId>> {
    let node_set = nodes.iter().copied().collect::<BTreeSet<_>>();
    let mut visited = BTreeSet::new();
    let mut finished = Vec::with_capacity(nodes.len());
    for root in nodes {
        if !visited.insert(*root) {
            continue;
        }
        let mut stack = vec![(*root, 0_usize)];
        while let Some((node, index)) = stack.last_mut() {
            let neighbors = adjacency.get(node).map(Vec::as_slice).unwrap_or_default();
            if let Some(next) = neighbors.get(*index).copied() {
                *index += 1;
                if node_set.contains(&next) && visited.insert(next) {
                    stack.push((next, 0));
                }
            } else {
                finished.push(*node);
                stack.pop();
            }
        }
    }
    let mut reverse = nodes
        .iter()
        .copied()
        .map(|node| (node, Vec::new()))
        .collect::<BTreeMap<_, _>>();
    for (from, targets) in adjacency {
        for target in targets {
            if node_set.contains(from) && node_set.contains(target) {
                reverse
                    .get_mut(target)
                    .expect("all SCC nodes have a reverse entry")
                    .push(*from);
            }
        }
    }
    visited.clear();
    let mut components = Vec::new();
    for root in finished.into_iter().rev() {
        if !visited.insert(root) {
            continue;
        }
        let mut component = Vec::new();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            component.push(node);
            for next in reverse[&node].iter().rev() {
                if visited.insert(*next) {
                    stack.push(*next);
                }
            }
        }
        component.sort_unstable();
        components.push(component);
    }
    components.sort_by_key(|component| component[0]);
    components
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::hir::{HirConstant, TypeLoweringLimits, lower_types};
    use crate::package::{Edition, PackageAlias, PackageGraph, PackageId, PackageNode};
    use crate::resolve::{ResolvedProgram, resolve};
    use crate::source::{LogicalPath, ModulePath, SourceId, SourceInput};
    use crate::syntax::{LexMode, ParseLimits, ParseMode, lex, parse};

    use super::*;

    fn check(source: &str) -> (SourceDatabase, ResolvedProgram, HirCheckOutput) {
        let mut sources = SourceDatabase::new();
        let file = sources
            .add(SourceInput::virtual_file(
                SourceId::new("root:expression-check").unwrap(),
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
        let resolved = resolve(&packages, &sources, [(file, &parsed)], 100).unwrap();
        let (resolved, diagnostics) = resolved.into_parts();
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        let lowered = lower_types(
            &packages,
            &sources,
            [(file, &parsed)],
            &resolved,
            TypeLoweringLimits {
                max_type_nodes: 100_000,
                max_diagnostics: 100,
            },
        )
        .unwrap();
        let (program, diagnostics) = lowered.into_parts();
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        let checked = check_expressions(
            &sources,
            [(file, &parsed)],
            &resolved,
            program,
            ExpressionCheckLimits {
                max_nodes: 100_000,
                max_pattern_steps: 100_000,
                max_diagnostics: 100,
            },
        )
        .unwrap();
        (sources, resolved, checked)
    }

    fn check_modules(inputs: &[(&str, &str, &str)]) -> HirCheckOutput {
        let mut sources = SourceDatabase::new();
        let mut parsed = Vec::new();
        for (module, path, source) in inputs {
            let file = sources
                .add(SourceInput::virtual_file(
                    SourceId::new("source:expression-check").unwrap(),
                    ModulePath::new(module).unwrap(),
                    LogicalPath::new(path).unwrap(),
                    Arc::<[u8]>::from(source.as_bytes()),
                ))
                .unwrap();
            let lexed = lex(&sources, file, LexMode::Module).unwrap();
            assert!(lexed.diagnostics().is_empty(), "{source}");
            let syntax = parse(
                &sources,
                file,
                lexed,
                ParseMode::Module,
                ParseLimits::default(),
            )
            .unwrap();
            assert!(syntax.diagnostics().is_empty(), "{source}");
            parsed.push((file, syntax));
        }
        let app = PackageId::new("pkg:expression-check").unwrap();
        let standard = PackageId::new("pkg:std").unwrap();
        let graph = PackageGraph::new(
            app.clone(),
            standard.clone(),
            [
                PackageNode::new(
                    app,
                    SourceId::new("source:expression-check").unwrap(),
                    PackageAlias::new("app").unwrap(),
                    Edition::V0_1,
                    inputs
                        .iter()
                        .map(|(module, _, _)| ModulePath::new(module).unwrap()),
                    [],
                )
                .unwrap(),
                PackageNode::new(
                    standard,
                    SourceId::new("source:std").unwrap(),
                    PackageAlias::new("tondoStd").unwrap(),
                    Edition::V0_1,
                    [],
                    [],
                )
                .unwrap(),
            ],
        )
        .unwrap();
        let resolved = resolve(
            &graph,
            &sources,
            parsed.iter().map(|(file, parsed)| (*file, parsed)),
            100,
        )
        .unwrap();
        let (resolved, diagnostics) = resolved.into_parts();
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        let lowered = lower_types(
            &graph,
            &sources,
            parsed.iter().map(|(file, parsed)| (*file, parsed)),
            &resolved,
            TypeLoweringLimits {
                max_type_nodes: 100_000,
                max_diagnostics: 100,
            },
        )
        .unwrap();
        let (program, diagnostics) = lowered.into_parts();
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        check_expressions(
            &sources,
            parsed.iter().map(|(file, parsed)| (*file, parsed)),
            &resolved,
            program,
            ExpressionCheckLimits {
                max_nodes: 100_000,
                max_pattern_steps: 100_000,
                max_diagnostics: 100,
            },
        )
        .unwrap()
    }

    fn codes(output: &HirCheckOutput) -> Vec<&str> {
        output
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.code().as_str())
            .collect()
    }

    fn only_body_root(output: &HirCheckOutput) -> HirExpressionId {
        let callable = output
            .program()
            .callables()
            .next()
            .expect("the fixture declares one callable");
        output
            .program()
            .body(callable.id())
            .expect("the callable body is checked")
            .root()
    }

    fn assignment_target_contains_coercion(
        target: &HirAssignmentTarget,
        expected: Assignability,
    ) -> bool {
        match target.kind() {
            HirAssignmentTargetKind::Place { coercion, .. } => *coercion == expected,
            HirAssignmentTargetKind::Discard => false,
            HirAssignmentTargetKind::Tuple(items) => items
                .iter()
                .any(|item| assignment_target_contains_coercion(item, expected)),
        }
    }

    #[test]
    fn constants_bindings_functions_and_inherent_methods_produce_typed_hir() {
        let (_, resolved, output) = check(
            "const Limit: Int8 = 12\n\
             const Twice = Limit + Limit\n\
             type Counter = { value: Int }\n\
             fn add(left: Int, right: Int): Int {\n\
                 let sum: Int = left + right\n\
                 sum\n\
             }\n\
             fn Counter.identity(self): Counter {\n\
                 self\n\
             }\n",
        );
        assert!(output.diagnostics().is_empty());
        assert!(output.is_complete());
        assert_eq!(output.program().bodies.len(), 2);
        assert_eq!(output.program().constants().count(), 2);
        let twice = resolved
            .symbols()
            .find(|symbol| symbol.name().as_str() == "Twice")
            .unwrap();
        let twice = output.program().constant(twice.id()).unwrap();
        assert_eq!(
            output
                .program()
                .interner()
                .canonical(twice.ty().unwrap())
                .unwrap(),
            "Int8"
        );
        assert!(
            output
                .program()
                .expressions()
                .all(|expression| { expression.ty() != output.program().interner().error() })
        );
    }

    #[test]
    fn expected_types_drive_literals_and_explicit_hir_coercions() {
        let (_, _, output) = check(
            "fn optional(): Int? { 42 }\n\
             fn union(): Int | String { 42 }\n",
        );
        assert!(output.diagnostics().is_empty());
        let coercions = output
            .program()
            .expressions()
            .filter_map(|expression| match expression.kind() {
                HirExpressionKind::Coerce { kind, .. } => Some(*kind),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(coercions.contains(&Assignability::OptionLift));
        assert!(coercions.contains(&Assignability::UnionInjection));
    }

    #[test]
    fn collection_literals_infer_or_use_one_contextual_intrinsic_type() {
        let (_, _, output) = check(
            "fn array(): Array[Int] { [1, 2, 3] }\n\
             fn map(): Map[String, Int?] { [\"one\": 1, \"none\": none] }\n\
             fn set(): Set[String] { Set[\"read\", \"write\"] }\n\
             fn empty(): (Array[Int], Map[String, Int], Set[Char]) {\n\
                 ([], [:], Set[])\n\
             }\n\
             fn nested(): Array[Array[UInt8]] { [[], [1, 2]] }\n\
             fn union_items(): Array[Int | String] | Bool { [1, \"two\"] }\n",
        );
        assert!(
            output.diagnostics().is_empty(),
            "{:#?}",
            output.diagnostics()
        );
        assert!(output.is_complete());

        let mut arrays = 0;
        let mut maps = 0;
        let mut sets = 0;
        for expression in output.program().expressions() {
            match expression.kind() {
                HirExpressionKind::Array(_) => arrays += 1,
                HirExpressionKind::Map(entries) => {
                    maps += 1;
                    assert!(entries.iter().all(|entry| {
                        output.program().expression(entry.key()).is_some()
                            && output.program().expression(entry.value()).is_some()
                    }));
                }
                HirExpressionKind::Set(_) => sets += 1,
                _ => {}
            }
        }
        assert_eq!(arrays, 6);
        assert_eq!(maps, 2);
        assert_eq!(sets, 2);
    }

    #[test]
    fn empty_and_heterogeneous_collection_literals_have_specific_type_errors() {
        for source in [
            "fn invalid() { [] }\n",
            "fn invalid() { [:] }\n",
            "fn invalid() { Set[] }\n",
        ] {
            let (_, _, output) = check(source);
            assert_eq!(codes(&output), ["E1101"], "{source}");
        }

        for source in [
            "fn invalid() { let value = [1, \"two\"]\n    _ = value\n}\n",
            "fn invalid() { let value = [\"one\": 1, 2: 2]\n    _ = value\n}\n",
            "fn invalid() { let value = Set[1, \"two\"]\n    _ = value\n}\n",
        ] {
            let (_, _, output) = check(source);
            assert_eq!(codes(&output), ["E1102"], "{source}");
        }
    }

    #[test]
    fn collection_literal_flow_preserves_left_to_right_element_evaluation() {
        let (_, _, output) = check(
            "fn stop(): Never {\n\
                 for {}\n\
             }\n\
             fn values(): Array[Int] { [stop(), 2] }\n",
        );
        assert_eq!(codes(&output), ["W1006"]);
        assert!(output.is_complete());
    }

    #[test]
    fn nominal_constructors_and_record_update_have_explicit_typed_hir() {
        let (_, _, output) = check(
            "type UserId = Int\n\
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
             fn make_id(): UserId { UserId(42) }\n",
        );
        assert!(
            output.diagnostics().is_empty(),
            "{:#?}",
            output.diagnostics()
        );
        assert!(output.is_complete());

        let mut newtypes = 0;
        let mut records = 0;
        let mut variants = [0; 3];
        let mut updates = 0;
        for expression in output.program().expressions() {
            match expression.kind() {
                HirExpressionKind::Newtype { .. } => newtypes += 1,
                HirExpressionKind::Record { fields, .. } => {
                    records += 1;
                    assert!(!fields.is_empty());
                }
                HirExpressionKind::Variant { payload, .. } => match payload {
                    HirVariantValue::Unit => variants[0] += 1,
                    HirVariantValue::Tuple(_) => variants[1] += 1,
                    HirVariantValue::Record(_) => variants[2] += 1,
                },
                HirExpressionKind::RecordUpdate { fields, .. } => {
                    updates += 1;
                    assert_eq!(fields.len(), 2);
                }
                _ => {}
            }
        }
        assert_eq!(newtypes, 1);
        assert_eq!(records, 1);
        assert_eq!(variants, [1, 1, 1]);
        assert_eq!(updates, 1);
        assert_eq!(output.program().member_references().count(), 10);
    }

    #[test]
    fn explicit_and_contextual_generic_nominal_construction_is_invariant() {
        let (_, _, output) = check(
            "type Box[T] = { value: T }\n\
             enum Choice[T] {\n\
                 Empty\n\
                 Value(T)\n\
                 Named { value: T }\n\
             }\n\
             fn explicit_box(): Box[Int] { Box[Int] { value: 1 } }\n\
             fn contextual_box(): Box[String] { Box { value: \"text\" } }\n\
             fn explicit_empty(): Choice[Int] { Choice[Int].Empty }\n\
             fn contextual_value(): Choice[String] { Choice.Value(\"text\") }\n\
             fn contextual_named(): Choice[UInt8] { Choice.Named { value: 1 } }\n",
        );
        assert!(
            output.diagnostics().is_empty(),
            "{:#?}",
            output.diagnostics()
        );
        assert!(output.is_complete());
        for body in output.program().bodies.values() {
            assert_ne!(
                output.program().expression(body.root()).unwrap().ty(),
                output.program().interner().error()
            );
        }
    }

    #[test]
    fn nominal_construction_and_with_reject_incomplete_or_wrong_shapes() {
        for source in [
            "type Pair = { left: Int, right: Int }\nfn invalid(): Pair { Pair { left: 1 } }\n",
            "type Pair = { left: Int }\nfn invalid(): Pair { Pair { left: 1, left: 2 } }\n",
            "type Pair = { left: Int }\nfn invalid(): Pair { Pair { left: 1, other: 2 } }\n",
            "enum Choice { Empty, Value(Int), Named { value: Int } }\nfn invalid(): Choice { Choice.Empty() }\n",
            "enum Choice { Empty, Value(Int), Named { value: Int } }\nfn invalid(): Choice { Choice.Value { value: 1 } }\n",
            "enum Choice { Empty, Value(Int), Named { value: Int } }\nfn invalid(): Choice { Choice.Named(1) }\n",
            "type Value = Int\nfn invalid(value: Value): Value { value with { value: 1 } }\n",
            "type Pair = { left: Int }\nfn invalid(pair: Pair): Pair { pair with { left: 1, left: 2 } }\n",
        ] {
            let (_, _, output) = check(source);
            assert_eq!(
                codes(&output),
                ["E1102"],
                "{source}\n{:#?}",
                output.diagnostics()
            );
        }
    }

    #[test]
    fn construction_update_access_and_methods_enforce_module_privacy() {
        let api = "pub type Account = {\n\
                       name: String\n\
                       priv secret: String\n\
                   }\n\
                   pub fn createAccount(name: String): Account {\n\
                       Account { name, secret: \"token\" }\n\
                   }\n\
                   fn Account.hidden(self): String { self.secret }\n\
                   pub fn Account.label(self): String { self.name }\n";
        let valid = check_modules(&[
            ("api", "api.to", api),
            (
                "main",
                "main.to",
                "import app.api\n\
                 fn use(value: api.Account): api.Account {\n\
                     _ = value.label()\n\
                     value with { name: \"next\" }\n\
                 }\n",
            ),
        ]);
        assert!(valid.diagnostics().is_empty(), "{:#?}", valid.diagnostics());
        assert!(valid.is_complete());

        for (body, expected) in [
            (
                "_ = api.Account { name: \"Ada\" }",
                "this record cannot be constructed outside its module",
            ),
            (
                "_ = api.Account { name: \"Ada\", secret: \"guess\" }",
                "cannot set a private field",
            ),
            (
                "let value = api.createAccount(\"Ada\")\n    _ = value with { secret: \"guess\" }",
                "cannot set a private field",
            ),
        ] {
            let main = format!("import app.api\nfn invalid() {{\n    {body}\n}}\n");
            let output = check_modules(&[("api", "api.to", api), ("main", "main.to", &main)]);
            assert_eq!(codes(&output), ["E1502"], "{body}");
            assert!(output.diagnostics()[0].message().contains(expected));
        }

        for body in [
            "let value = api.createAccount(\"Ada\")\n    _ = value.secret",
            "let value = api.createAccount(\"Ada\")\n    _ = value.hidden()",
        ] {
            let main = format!("import app.api\nfn invalid() {{\n    {body}\n}}\n");
            let output = check_modules(&[("api", "api.to", api), ("main", "main.to", &main)]);
            assert_eq!(codes(&output), ["E1501"], "{body}");
        }
    }

    #[test]
    fn numeric_conversion_constructors_encode_total_checked_and_identity_cases() {
        let (_, _, valid) = check(
            "fn widen(value: Int32): Int { Int(value) }\n\
             fn to_float(value: UInt64): Float32 { Float32(value) }\n\
             fn narrow(value: Int): Int8 ! NumericConversionError { Int8(value) }\n\
             fn narrow_with_propagation(value: Int): Int8 ! NumericConversionError {\n\
                 Int8(value)?\n\
             }\n\
             fn integer(value: Float): Int ! NumericConversionError { Int(value) }\n",
        );
        assert!(valid.diagnostics().is_empty(), "{:#?}", valid.diagnostics());
        assert!(valid.is_complete());
        let conversions = valid
            .program()
            .expressions()
            .filter_map(|expression| match expression.kind() {
                HirExpressionKind::NumericConversion { conversion, .. } => Some(*conversion),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            conversions
                .iter()
                .filter(|conversion| **conversion == NumericConversion::Total)
                .count(),
            2
        );
        assert_eq!(
            conversions
                .iter()
                .filter(|conversion| **conversion == NumericConversion::Checked)
                .count(),
            3
        );

        let (_, _, redundant) = check("fn same(value: Int): Int { Int(value) }\n");
        assert_eq!(codes(&redundant), ["W1007"]);
        assert_eq!(redundant.diagnostics()[0].severity(), Severity::Warning);
        assert!(redundant.is_complete());
    }

    #[test]
    fn numeric_conversion_constructors_reject_pairs_outside_the_closed_table() {
        for source in [
            "fn invalid(): Int { Int('a') }\n",
            "fn invalid(): Int { Int(\"one\") }\n",
            "fn invalid(): Int { Int() }\n",
            "fn invalid(): Int { Int(1, 2) }\n",
        ] {
            let (_, _, output) = check(source);
            assert_eq!(
                codes(&output),
                ["E1103"],
                "{source}\n{:#?}",
                output.diagnostics()
            );
        }

        let (_, _, nominal) = check(
            "type Small = Int8\n\
             fn invalid(value: Int): Small { Small(value) }\n",
        );
        assert_eq!(codes(&nominal), ["E1102"]);
    }

    #[test]
    fn mismatches_missing_context_and_uninitialized_bindings_are_specific() {
        let (_, _, mismatch) = check("fn invalid(): Int { \"text\" }\n");
        assert_eq!(codes(&mismatch), ["E1102"]);

        let (_, _, missing) = check("fn invalid() {\n    let value = none\n}\n");
        assert_eq!(codes(&missing), ["E1304"]);

        let (_, _, uninitialized) = check("fn invalid() {\n    var value: Int\n}\n");
        assert_eq!(codes(&uninitialized), ["E1109"]);
    }

    #[test]
    fn operators_calls_and_discard_rules_use_the_checked_types() {
        let (_, _, valid) = check(
            "fn add(left: Int, right: Int): Int { left + right }\n\
             fn main(): Int { add(20, 22) }\n",
        );
        assert!(valid.diagnostics().is_empty(), "{:#?}", valid.diagnostics());

        let (_, _, mixed) = check("fn invalid() {\n    let value = 1 + 2.0\n}\n");
        assert_eq!(codes(&mixed), ["E1102"]);

        let (_, _, discarded) = check("fn invalid() { 1\n() }\n");
        assert_eq!(codes(&discarded), ["E1303"]);

        let (_, _, optional) = check(
            "fn optional(): Int? { none }\n\
             fn invalid() {\n\
                 optional()\n\
                 ()\n\
             }\n",
        );
        assert_eq!(codes(&optional), ["E1303"]);
    }

    #[test]
    fn ranges_and_membership_have_closed_typed_hir_forms() {
        let (_, _, output) = check(
            "fn inspect(): Bool {\n\
                 let numbers = 0..10\n\
                 let letters = 'a'..='z'\n\
                 let ages = [\"Ada\": 37]\n\
                 let permissions = Set[\"read\", \"write\"]\n\
                 _ = 1 in []\n\
                 _ = 1 in Set[]\n\
                 5 in numbers and 'm' in letters and \"Ada\" in ages and\n\
                     \"read\" in permissions and 'x' in \"text\"\n\
             }\n",
        );
        assert!(
            output.diagnostics().is_empty(),
            "{:#?}",
            output.diagnostics()
        );
        assert!(output.is_complete());
        let ranges = output
            .program()
            .expressions()
            .filter_map(|expression| match expression.kind() {
                HirExpressionKind::Range { kind, .. } => Some(*kind),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(ranges, [HirRangeKind::Exclusive, HirRangeKind::Inclusive]);
        let containment = output
            .program()
            .expressions()
            .filter_map(|expression| match expression.kind() {
                HirExpressionKind::Contains { kind, .. } => Some(*kind),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            containment,
            [
                HirContainmentKind::Array,
                HirContainmentKind::Set,
                HirContainmentKind::Range,
                HirContainmentKind::Range,
                HirContainmentKind::MapKey,
                HirContainmentKind::Set,
                HirContainmentKind::StringChar,
            ]
        );
    }

    #[test]
    fn ranges_and_membership_reject_every_invalid_shape() {
        for source in [
            "fn invalid() {\n    _ = 0.0..1.0\n}\n",
            "fn invalid() {\n    _ = 0..'a'\n}\n",
            "fn invalid() {\n    _ = 1 in 2\n}\n",
            "fn invalid() {\n    _ = \"x\" in \"text\"\n}\n",
            "fn invalid() {\n    _ = 1 in [\"one\": 1]\n}\n",
        ] {
            let (_, _, output) = check(source);
            assert_eq!(
                codes(&output),
                ["E1102"],
                "{source}\n{:#?}",
                output.diagnostics()
            );
        }
    }

    #[test]
    fn membership_flow_observes_the_item_before_the_container() {
        let (_, _, output) = check(
            "fn stop(): Never {\n\
                 for {}\n\
             }\n\
             fn inspect(): Bool {\n\
                 stop() in [1]\n\
             }\n",
        );
        assert_eq!(codes(&output), ["W1006"]);
        assert!(output.is_complete());
    }

    #[test]
    fn named_and_variadic_calls_bind_parameters_without_reordering_evaluation() {
        let (_, _, output) = check(
            "fn connect(host: String, port: Int): String { host }\n\
             fn log(prefix: String, parts: ...String): Array[String] { parts }\n\
             fn main() {\n\
                 _ = connect(port: 8080, host: \"localhost\")\n\
                 _ = log(\"Info: \")\n\
                 _ = log(\"Info: \", \"server\", \" started\")\n\
                 let parts = [\"server\", \" started\"]\n\
                 _ = log(\"Info: \", ...parts)\n\
                 _ = log(prefix: \"Info: \", parts: ...parts)\n\
             }\n",
        );
        assert!(
            output.diagnostics().is_empty(),
            "{:#?}",
            output.diagnostics()
        );
        assert!(output.is_complete());

        let calls = output
            .program()
            .expressions()
            .filter_map(|expression| match expression.kind() {
                HirExpressionKind::Call { arguments, .. } => Some(arguments),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(calls.len(), 5);
        assert_eq!(calls[0][0].target(), HirCallArgumentTarget::Fixed(1));
        assert_eq!(calls[0][1].target(), HirCallArgumentTarget::Fixed(0));
        assert!(
            calls[2][1..]
                .iter()
                .all(|argument| argument.target() == HirCallArgumentTarget::VariadicElement)
        );
        assert_eq!(
            calls[3].last().unwrap().target(),
            HirCallArgumentTarget::VariadicSpread
        );
        assert_eq!(
            calls[4].last().unwrap().target(),
            HirCallArgumentTarget::VariadicSpread
        );
    }

    #[test]
    fn call_argument_association_rejects_every_ambiguous_shape() {
        let prelude = "fn connect(host: String, port: Int): String { host }\n";
        let variadic = "fn log(prefix: String, parts: ...String): Array[String] { parts }\n";
        for body in [
            "_ = connect(host: \"x\", 1)",
            "_ = connect(host: \"x\", socket: 1)",
            "_ = connect(host: \"x\", host: \"y\")",
            "_ = connect(\"x\")",
            "_ = connect(\"x\", 1, 2)",
        ] {
            let source = format!("{prelude}fn main() {{\n    {body}\n}}\n");
            let (_, _, output) = check(&source);
            assert_eq!(
                codes(&output),
                ["E1102"],
                "{body}\n{:#?}",
                output.diagnostics()
            );
        }

        for body in [
            "let values = [\"x\"]\n    _ = log(prefix: \"p\", \"x\")",
            "let values = [\"x\"]\n    _ = log(prefix: \"p\", parts: \"x\")",
            "let values = [\"x\"]\n    _ = log(\"p\", ...values, \"x\")",
        ] {
            let source = format!("{variadic}fn main() {{\n    {body}\n}}\n");
            let (_, _, output) = check(&source);
            assert_eq!(
                codes(&output),
                ["E1102"],
                "{body}\n{:#?}",
                output.diagnostics()
            );
        }
    }

    #[test]
    fn named_call_arguments_keep_textual_reachability_order() {
        let (_, _, output) = check(
            "fn choose(first: Int, second: Int): Int { first }\n\
             fn stop(): Never {\n\
                 for {}\n\
             }\n\
             fn main(): Int { choose(second: stop(), first: 1) }\n",
        );
        assert_eq!(codes(&output), ["W1006"]);
        assert!(output.is_complete());
    }

    #[test]
    fn explicit_discard_has_dedicated_hir_and_requires_discard_capability() {
        let (_, _, valid) = check(
            "fn release(\n\
                 number: Int,\n\
                 values: Array[String],\n\
                 command: Command,\n\
             ) {\n\
                 _ = number\n\
                 _ = values\n\
                 _ = command\n\
             }\n",
        );
        assert!(valid.diagnostics().is_empty(), "{:#?}", valid.diagnostics());
        assert!(valid.is_complete());
        let discards = valid
            .program()
            .expressions()
            .filter_map(|expression| match expression.kind() {
                HirExpressionKind::Block { statements, .. } => Some(
                    statements
                        .iter()
                        .filter(|statement| matches!(statement, HirStatement::Discard { .. }))
                        .count(),
                ),
                _ => None,
            })
            .sum::<usize>();
        assert_eq!(discards, 3);

        for source in [
            "fn invalid(task: Join[Int, Never]) {\n    _ = task\n}\n",
            "fn invalid(value: (Int, Join[Int, Never])) {\n    _ = value\n}\n",
            "fn invalid(value: Array[Join[Int, Never]]) {\n    _ = value\n}\n",
            "type Work = { task: Join[Int, Never] }\nfn invalid(value: Work) {\n    _ = value\n}\n",
            "type Box[T] = { value: T }\nfn invalid(value: Box[Join[Int, Never]]) {\n    _ = value\n}\n",
            "enum Work {\n    Idle\n    Running(Join[Int, Never])\n}\nfn invalid(value: Work) {\n    _ = value\n}\n",
        ] {
            let (_, _, invalid) = check(source);
            assert_eq!(codes(&invalid), ["E1105"], "{source}");
            assert!(invalid.diagnostics()[0].message().contains("Discard"));
        }
    }

    #[test]
    fn discard_parameters_require_capability_only_when_they_take_ownership() {
        let (_, _, concrete) = check("fn invalid(_: Join[Int, Never]) {}\n");
        assert_eq!(codes(&concrete), ["E1105"]);

        let (_, _, borrowed) = check(
            "fn inspect(\n\
                 _: ref Join[Int, Never],\n\
                 _: mut Join[Int, Never],\n\
                 _: var Join[Int, Never],\n\
             ) {}\n",
        );
        assert!(
            borrowed.diagnostics().is_empty(),
            "{:#?}",
            borrowed.diagnostics()
        );
        assert!(borrowed.is_complete());

        let (_, _, bounded) = check(
            "fn discard[T: Discard](_: Array[T]) {}\n\
             fn copied[T: Copy](_: T) {}\n\
             fn keyed[T: Key](_: T) {}\n",
        );
        assert!(
            bounded.diagnostics().is_empty(),
            "{:#?}",
            bounded.diagnostics()
        );
        assert!(!bounded.is_complete());

        let (_, _, unbounded) = check("fn invalid[T](_: Array[T]) {}\n");
        assert_eq!(codes(&unbounded), ["E1105"]);
    }

    #[test]
    fn discard_derivation_is_coinductive_and_applies_to_multiple_assignment_leaves() {
        let (_, _, recursive) = check(
            "type Node = { next: Node? }\n\
             fn release(value: Node) {\n\
                 _ = value\n\
             }\n",
        );
        assert!(
            recursive.diagnostics().is_empty(),
            "{:#?}",
            recursive.diagnostics()
        );
        assert!(recursive.is_complete());

        let (_, _, terminal_recursive) = check(
            "type Node = {\n\
                 next: Node?\n\
                 task: Join[Int, Never]\n\
             }\n\
             fn invalid(value: Node) {\n\
                 _ = value\n\
             }\n",
        );
        assert_eq!(codes(&terminal_recursive), ["E1105"]);

        let (_, _, leaf) = check(
            "fn invalid(\n\
                 pair: (Int, Join[Int, Never]),\n\
                 output: var Int,\n\
             ) {\n\
                 (output, _) = pair\n\
             }\n",
        );
        assert_eq!(codes(&leaf), ["E1105"]);

        let (_, _, transformed) = check(
            "type Chain[T] = {\n\
                 value: T\n\
                 next: Chain[T?]?\n\
             }\n\
             fn valid(value: Chain[Int]) {\n\
                 _ = value\n\
             }\n\
             fn invalid(value: Chain[Join[Int, Never]]) {\n\
                 _ = value\n\
             }\n",
        );
        assert_eq!(codes(&transformed), ["E1105"]);

        let (_, _, phantom) = check(
            "type Phantom[T] = { next: Phantom[T?]? }\n\
             fn release(value: Phantom[Join[Int, Never]]) {\n\
                 _ = value\n\
             }\n",
        );
        assert!(
            phantom.diagnostics().is_empty(),
            "{:#?}",
            phantom.diagnostics()
        );
        assert!(phantom.is_complete());
    }

    #[test]
    fn deep_discard_derivation_uses_a_worklist() {
        let mut source = String::new();
        for index in 0..512 {
            source.push_str(&format!(
                "type Node{index} = {{ next: Node{} }}\n",
                index + 1
            ));
        }
        source.push_str("type Node512 = { task: Join[Int, Never] }\n");
        source.push_str("fn invalid(value: Node0) {\n    _ = value\n}\n");
        let (_, _, output) = check(&source);
        assert_eq!(codes(&output), ["E1105"]);
    }

    #[test]
    fn simple_compound_and_multiple_assignments_are_typed() {
        let (_, _, output) = check(
            "fn update(borrowed: mut Int, replaced: var Int) {\n\
                 var left = 1\n\
                 var right = 2\n\
                 left = 3\n\
                 left += right\n\
                 borrowed = left\n\
                 replaced = borrowed\n\
                 (left, right) = (right, left)\n\
             }\n",
        );
        assert!(
            output.diagnostics().is_empty(),
            "{:#?}",
            output.diagnostics()
        );
        assert!(output.is_complete());
        let assignments = output
            .program()
            .expressions()
            .filter_map(|expression| match expression.kind() {
                HirExpressionKind::Block { statements, .. } => Some(
                    statements
                        .iter()
                        .filter(|statement| matches!(statement, HirStatement::Assignment { .. }))
                        .count(),
                ),
                _ => None,
            })
            .sum::<usize>();
        assert_eq!(assignments, 5);
        let writes = output
            .program()
            .expressions()
            .flat_map(|expression| match expression.kind() {
                HirExpressionKind::Block { statements, .. } => statements
                    .iter()
                    .filter_map(|statement| match statement {
                        HirStatement::Assignment { target, .. } => match target.kind() {
                            HirAssignmentTargetKind::Place { write, .. } => Some(*write),
                            _ => None,
                        },
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
                _ => Vec::new(),
            })
            .collect::<Vec<_>>();
        assert!(writes.contains(&HirWriteKind::PreserveExtent));
        assert!(writes.contains(&HirWriteKind::Replace));
    }

    #[test]
    fn assignment_rejects_immutable_duplicate_and_mismatched_destinations() {
        let (_, _, immutable) = check(
            "fn invalid() {\n\
                 let value = 1\n\
                 value = 2\n\
             }\n",
        );
        assert_eq!(codes(&immutable), ["E1411"]);

        let (_, _, duplicate) = check(
            "fn invalid() {\n\
                 var value = 0\n\
                 (value, value) = (1, 2)\n\
             }\n",
        );
        assert_eq!(codes(&duplicate), ["E1405"]);

        let (_, _, mismatch) = check(
            "fn invalid() {\n\
                 var number = 0\n\
                 var text = \"\"\n\
                 (number, text) = (1, 2)\n\
             }\n",
        );
        assert_eq!(codes(&mismatch), ["E1102"]);
    }

    #[test]
    fn assignment_supports_fields_tuple_slots_arrays_slices_and_maps() {
        let (_, _, output) = check(
            "type State = { count: Int, values: Array[Int] }\n\
             fn update(\n\
                 state: mut State,\n\
                 pair: var (Int, String),\n\
                 values: var Array[Int],\n\
                 replacement: Array[Int],\n\
                 entries: var Map[String, Int],\n\
             ) {\n\
                 state.count = 1\n\
                 state.values[0] = 2\n\
                 pair.0 = 3\n\
                 values[0] = pair.0\n\
                 values[1:3] = replacement\n\
                 values[::2] += 10\n\
                 entries[\"answer\"] = 42\n\
                 let item: Int = values[0]\n\
                 let view: Array[Int] = values[:]\n\
                 let found: Int? = entries[\"answer\"]\n\
             }\n",
        );
        assert!(
            output.diagnostics().is_empty(),
            "{:#?}",
            output.diagnostics()
        );
        assert!(output.is_complete());
        assert!(
            output
                .program()
                .expressions()
                .any(|expression| { matches!(expression.kind(), HirExpressionKind::Field { .. }) })
        );
        assert!(output.program().expressions().any(|expression| {
            matches!(expression.kind(), HirExpressionKind::TupleField { .. })
        }));
        assert!(output.program().expressions().any(|expression| {
            matches!(
                expression.kind(),
                HirExpressionKind::Index {
                    access: HirIndexAccess::MapEntry,
                    ..
                }
            )
        }));
        assert!(output.program().expressions().any(|expression| {
            matches!(
                expression.kind(),
                HirExpressionKind::Index {
                    access: HirIndexAccess::MapLookup,
                    ..
                }
            )
        }));
        assert!(
            output
                .program()
                .expressions()
                .any(|expression| { matches!(expression.kind(), HirExpressionKind::Slice { .. }) })
        );
        assert!(output.program().expressions().any(|expression| {
            matches!(
                expression.kind(),
                HirExpressionKind::Slice {
                    start: None,
                    end: None,
                    step: Some(_),
                    ..
                }
            )
        }));
    }

    #[test]
    fn every_scalar_compound_operator_and_array_arithmetic_are_closed() {
        let (_, _, output) = check(
            "fn update(number: var Int, values: var Array[Int], other: Array[Int]) {\n\
                 number += 1\n\
                 number -= 1\n\
                 number *= 2\n\
                 number /= 2\n\
                 number %= 3\n\
                 number &= 7\n\
                 number ^= 1\n\
                 number |= 8\n\
                 number <<= 1\n\
                 number >>= 1\n\
                 values += other\n\
                 values -= 1\n\
                 values *= 2\n\
                 values /= 2\n\
                 values %= 3\n\
                 let sum: Array[Int] = values + other\n\
                 let inverse: Array[Int] = 100 - values\n\
                 let inferred = values + 1\n\
                 let inferred_inverse = 100 - [1, 2]\n\
                 _ = inferred\n\
                 _ = inferred_inverse\n\
             }\n",
        );
        assert!(
            output.diagnostics().is_empty(),
            "{:#?}",
            output.diagnostics()
        );
        assert!(output.is_complete());
    }

    #[test]
    fn assignment_permissions_and_map_compound_policy_are_explicit() {
        let (_, _, immutable_field) = check(
            "type State = { count: Int }\n\
             fn invalid(state: State) {\n\
                 state.count = 1\n\
             }\n",
        );
        assert_eq!(codes(&immutable_field), ["E1411"]);

        let (_, _, borrowed_map) = check(
            "fn invalid(entries: mut Map[String, Int]) {\n\
                 entries[\"answer\"] = 42\n\
             }\n",
        );
        assert_eq!(codes(&borrowed_map), ["E1411"]);

        let (_, _, compound_map) = check(
            "fn invalid(entries: var Map[String, Int]) {\n\
                 entries[\"answer\"] += 1\n\
             }\n",
        );
        assert_eq!(codes(&compound_map), ["E1411"]);

        let (_, _, missing_field) = check(
            "type State = { count: Int }\n\
             fn invalid(state: var State) {\n\
                 state.missing = 1\n\
             }\n",
        );
        assert_eq!(codes(&missing_field), ["E1411"]);

        let (_, _, constant) = check(
            "const Limit: Int = 1\n\
             fn invalid() {\n\
                 Limit = 2\n\
             }\n",
        );
        assert_eq!(codes(&constant), ["E1411"]);
    }

    #[test]
    fn assignment_resolves_every_destination_before_the_rhs() {
        let (_, resolved, output) = check(
            "fn index(): Int { 0 }\n\
             fn value(): Int { 1 }\n\
             fn update(values: var Array[Int]) {\n\
                 values[index()] = value()\n\
             }\n",
        );
        assert!(
            output.diagnostics().is_empty(),
            "{:#?}",
            output.diagnostics()
        );
        let update = resolved
            .symbols()
            .find(|symbol| symbol.name().as_str() == "update")
            .expect("update is resolved");
        let body = output
            .program()
            .body(HirCallableId::Symbol(update.id()))
            .expect("update is checked");
        let HirExpressionKind::Block { statements, .. } = output
            .program()
            .expression(body.root())
            .expect("the body root exists")
            .kind()
        else {
            panic!("update has a block body");
        };
        let HirStatement::Assignment { target, value, .. } = &statements[0] else {
            panic!("the first statement is the assignment");
        };
        let HirAssignmentTargetKind::Place { place, .. } = target.kind() else {
            panic!("the assignment target is a place");
        };
        assert!(place.index() < value.index());
        let HirExpressionKind::Index { index, .. } = output
            .program()
            .expression(*place)
            .expect("the place expression exists")
            .kind()
        else {
            panic!("the place retains its index operation");
        };
        assert!(index.index() < value.index());
    }

    #[test]
    fn multiple_assignment_detects_static_place_overlap_without_rejecting_distinct_places() {
        let (_, _, duplicates) = check(
            "fn invalid(\n\
                 values: var Array[Int],\n\
                 entries: var Map[String, Int],\n\
                 index: Int,\n\
             ) {\n\
                 (values[index], values[index]) = (1, 2)\n\
                 (values[1], values[0x1]) = (3, 4)\n\
                 (entries[\"a\"], entries[\"\\u{61}\"]) = (5, 6)\n\
             }\n",
        );
        assert_eq!(codes(&duplicates), ["E1405", "E1405", "E1405"]);

        let (_, _, distinct) = check(
            "fn valid(values: var Array[Int]) {\n\
                 (values[0], values[1]) = (values[1], values[0])\n\
             }\n",
        );
        assert!(
            distinct.diagnostics().is_empty(),
            "{:#?}",
            distinct.diagnostics()
        );
        assert!(distinct.is_complete());

        let (_, _, prefix) = check(
            "type State = { count: Int }\n\
             fn invalid(state: var State, replacement: State) {\n\
                 (state, state.count) = (replacement, 1)\n\
             }\n",
        );
        assert_eq!(codes(&prefix), ["E1405"]);
    }

    #[test]
    fn nested_multiple_assignment_propagates_partial_context_and_leaf_coercions() {
        let (_, _, output) = check(
            "fn source(): (Int, String) { (1, \"text\") }\n\
             fn update(\n\
                 optional: var Int?,\n\
                 number: var Int,\n\
                 widened: var (Int | String),\n\
                 text: var String,\n\
             ) {\n\
                 (optional, _) = (none, 1)\n\
                 ((number, optional), _) = ((2, none), \"discarded\")\n\
                 (widened, text) = source()\n\
                 _ = 42\n\
             }\n",
        );
        assert!(
            output.diagnostics().is_empty(),
            "{:#?}",
            output.diagnostics()
        );
        assert!(output.is_complete());
        assert!(output.program().expressions().any(|expression| {
            matches!(
                expression.kind(),
                HirExpressionKind::Literal(HirLiteral::None)
            ) && matches!(
                output.program().interner().kind(expression.ty()).unwrap(),
                TypeKind::Option(_)
            )
        }));
        let has_leaf_injection = output.program().expressions().any(|expression| {
            let HirExpressionKind::Block { statements, .. } = expression.kind() else {
                return false;
            };
            statements.iter().any(|statement| {
                let HirStatement::Assignment { target, .. } = statement else {
                    return false;
                };
                assignment_target_contains_coercion(target, Assignability::UnionInjection)
            })
        });
        assert!(has_leaf_injection);
    }

    #[test]
    fn multiple_assignment_requires_one_matching_tuple_and_compound_is_single_place_only() {
        let (_, _, arity) = check(
            "fn invalid(left: var Int, right: var Int) {\n\
                 (left, right) = (1, 2, 3)\n\
             }\n",
        );
        assert_eq!(codes(&arity), ["E1102"]);

        let (_, _, not_tuple) = check(
            "fn invalid(left: var Int, right: var Int) {\n\
                 (left, right) = 1\n\
             }\n",
        );
        assert_eq!(codes(&not_tuple), ["E1102"]);

        let (_, _, compound) = check(
            "fn invalid(left: var Int, right: var Int) {\n\
                 (left, right) += (1, 2)\n\
             }\n",
        );
        assert_eq!(codes(&compound), ["E1411"]);
    }

    #[test]
    fn generic_record_and_newtype_assignment_fields_use_instantiated_types() {
        let (_, _, output) = check(
            "type Box[T] = { value: T }\n\
             type UserId = Int\n\
             fn update(boxed: var Box[String], id: var UserId) {\n\
                 boxed.value = \"updated\"\n\
                 id.value = 42\n\
                 let text: String = boxed.value\n\
                 let number: Int = id.value\n\
             }\n",
        );
        assert!(
            output.diagnostics().is_empty(),
            "{:#?}",
            output.diagnostics()
        );
        assert!(output.is_complete());
    }

    #[test]
    fn receiver_assignment_respects_self_mutability_modes() {
        let (_, _, output) = check(
            "type Counter = { value: Int }\n\
             fn Counter.increment(mut self) {\n\
                 self.value += 1\n\
             }\n\
             fn Counter.replace(mut self, other: Counter) {\n\
                 self = other\n\
             }\n\
             fn Counter.reset(var self, other: Counter) {\n\
                 self = other\n\
             }\n",
        );
        assert!(
            output.diagnostics().is_empty(),
            "{:#?}",
            output.diagnostics()
        );
        assert!(output.is_complete());

        let (_, _, immutable) = check(
            "type Counter = { value: Int }\n\
             fn Counter.invalid(self) {\n\
                 self.value = 1\n\
             }\n",
        );
        assert_eq!(codes(&immutable), ["E1411"]);
    }

    #[test]
    fn explicit_generic_calls_materialize_their_specialization() {
        let (_, _, output) = check(
            "fn identity[T](value: T): T { value }\n\
             fn main(): Int { identity[Int](1) }\n",
        );
        assert!(
            output.diagnostics().is_empty(),
            "{:#?}",
            output.diagnostics()
        );
        assert!(!output.is_complete());
        let arguments = output
            .program()
            .expressions()
            .find_map(|expression| match expression.kind() {
                HirExpressionKind::SpecializedFunction { arguments, .. } => Some(arguments),
                _ => None,
            })
            .expect("the explicit generic call has a specialized callee");
        assert_eq!(
            arguments,
            &[output.program().interner().scalar(ScalarType::Int)]
        );
    }

    #[test]
    fn generic_calls_infer_from_arguments_results_options_and_variadics() {
        let (_, _, output) = check(
            "fn identity[T](value: T): T { value }\n\
             fn make[T](): T {\n\
                 for {}\n\
             }\n\
             fn optional[T](value: T?): T? { value }\n\
             fn collect[T](values: ...T): Array[T] { values }\n\
             fn main(): Int8 {\n\
                 let text = identity(\"hello\")\n\
                 let made: String = make()\n\
                 let lifted = optional(1)\n\
                 let values = collect(1u16, 2u16)\n\
                 _ = text\n\
                 _ = made\n\
                 _ = lifted\n\
                 _ = values\n\
                 identity(1)\n\
             }\n",
        );
        assert!(
            output.diagnostics().is_empty(),
            "{:#?}",
            output.diagnostics()
        );
        assert!(!output.is_complete());
        let specializations = output
            .program()
            .expressions()
            .filter_map(|expression| match expression.kind() {
                HirExpressionKind::SpecializedFunction { arguments, .. } => {
                    Some(output.program().interner().canonical(arguments[0]).unwrap())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            specializations,
            ["String", "String", "Int", "UInt16", "Int8"]
        );
        assert!(output.program().expressions().any(|expression| matches!(
            expression.kind(),
            HirExpressionKind::Coerce {
                kind: Assignability::OptionLift,
                ..
            }
        )));
    }

    #[test]
    fn generic_call_inference_rejects_conflicts_and_ambiguity() {
        let (_, _, conflict) = check(
            "fn same[T](left: T, right: T): T { left }\n\
             fn invalid() {\n\
                 _ = same(1, \"two\")\n\
             }\n",
        );
        assert_eq!(codes(&conflict), ["E1102"]);

        let (_, _, unsolved) = check(
            "fn make[T](): T {\n\
                 for {}\n\
             }\n\
             fn invalid() {\n\
                 _ = make()\n\
             }\n",
        );
        assert_eq!(codes(&unsolved), ["E1101"]);

        let (_, _, ambiguous) = check(
            "fn choose[T, U](value: T): T { value }\n\
             fn invalid() {\n\
                 _ = choose(1)\n\
             }\n",
        );
        assert_eq!(codes(&ambiguous), ["E1101"]);
    }

    #[test]
    fn method_member_access_requires_an_immediate_call() {
        let (_, _, output) = check(
            "type Counter = { value: Int }\n\
             fn Counter.read(self): Int { self.value }\n\
             fn invalid(counter: Counter) {\n\
                 let method = counter.read\n\
             }\n",
        );
        assert_eq!(codes(&output), ["E1102"]);
    }

    #[test]
    fn inherent_and_associated_calls_desugar_to_explicit_receiver_arguments() {
        let (_, _, output) = check(
            "type Counter = { value: Int }\n\
             type Handler = { call: fn(Int): Int }\n\
             fn Counter.read(self): Int { self.value }\n\
             fn Counter.add(self, amount: Int): Int { self.value + amount }\n\
             fn Counter.set(mut self, value: Int) {\n\
                 self.value = value\n\
             }\n\
             fn Counter.replace(var self, next: Counter) {\n\
                 self = next\n\
             }\n\
             fn Counter.create(value: Int): Counter { Counter { value } }\n\
             fn identity(value: Int): Int { value }\n\
             fn invoke(handler: Handler): Int { handler.call(5) }\n\
             fn use(\n\
                 fixed: Counter,\n\
                 mutable: mut Counter,\n\
                 replaceable: var Counter,\n\
             ): Int {\n\
                 mutable.set(1)\n\
                 replaceable.replace(Counter.create(2))\n\
                 _ = invoke(Handler { call: identity })\n\
                 Counter.read(fixed) + fixed.add(amount: 3)\n\
             }\n",
        );
        assert!(
            output.diagnostics().is_empty(),
            "{:#?}",
            output.diagnostics()
        );
        assert!(output.is_complete());

        let receiver_arguments = output
            .program()
            .expressions()
            .filter_map(|expression| match expression.kind() {
                HirExpressionKind::Call { arguments, .. } => Some(
                    arguments
                        .iter()
                        .filter(|argument| argument.target() == HirCallArgumentTarget::Receiver)
                        .count(),
                ),
                _ => None,
            })
            .sum::<usize>();
        assert_eq!(receiver_arguments, 4);
    }

    #[test]
    fn mutable_method_receivers_require_the_declared_place_permission() {
        let source = "type Counter = { value: Int }\n\
                      fn Counter.set(mut self, value: Int) {\n\
                          self.value = value\n\
                      }\n\
                      fn Counter.replace(var self, next: Counter) {\n\
                          self = next\n\
                      }\n";
        for body in [
            "let value = Counter { value: 0 }\n    value.set(1)",
            "fn nested(value: mut Counter) { value.replace(Counter { value: 1 }) }",
            "Counter { value: 0 }.set(1)",
        ] {
            let fixture = if body.starts_with("fn ") {
                format!("{source}{body}\n")
            } else {
                format!("{source}fn invalid() {{\n    {body}\n}}\n")
            };
            let (_, _, output) = check(&fixture);
            assert_eq!(
                codes(&output),
                ["E1407"],
                "{body}\n{:#?}",
                output.diagnostics()
            );
        }
    }

    #[test]
    fn constant_cycles_are_reported_once_per_component() {
        let (_, _, output) = check(
            "const First: Int = Second\n\
             const Second: Int = First\n",
        );
        assert_eq!(codes(&output), ["E1902"]);
    }

    #[test]
    fn constant_cycles_and_topological_evaluation_are_file_order_independent() {
        let first = [
            (
                "main",
                "b.to",
                "\n\nconst Beta: Int = Alpha\nconst Use: Int = Alpha + 1\n",
            ),
            ("main", "a.to", "const Alpha: Int = Beta\n"),
            ("main", "d.to", "const Delta: Int = Gamma\n"),
            ("main", "c.to", "const Gamma: Int = Delta\n"),
            ("main", "self.to", "const SelfCycle: Int = SelfCycle\n"),
        ];
        let second = [first[4], first[2], first[0], first[3], first[1]];
        let forward = check_modules(&first);
        let permuted = check_modules(&second);
        assert_eq!(codes(&forward), ["E1902", "E1902", "E1902"]);
        assert_eq!(codes(&permuted), codes(&forward));

        let primary_offset = |output: &HirCheckOutput| match output.diagnostics()[0].location() {
            PrimaryLocation::Source(span) => span.range().start(),
            PrimaryLocation::Target(_) => panic!("constant cycles have source locations"),
        };
        assert_eq!(primary_offset(&forward), 6);
        assert_eq!(primary_offset(&permuted), 6);

        let acyclic_first = [
            ("main", "later.to", "const Answer: Int = Base + 2\n"),
            ("main", "base.to", "const Base: Int = 40\n"),
        ];
        let acyclic_second = [acyclic_first[1], acyclic_first[0]];
        for output in [
            check_modules(&acyclic_first),
            check_modules(&acyclic_second),
        ] {
            assert!(
                output.diagnostics().is_empty(),
                "{:#?}",
                output.diagnostics()
            );
            let mut values = output
                .program()
                .constants()
                .filter_map(|(_, constant)| constant.evaluated())
                .filter_map(|value| match value.kind() {
                    crate::hir::HirConstantValueKind::Integer(value) => Some(*value),
                    _ => None,
                })
                .collect::<Vec<_>>();
            values.sort_unstable();
            assert_eq!(values, [40, 42]);
        }
    }

    #[test]
    fn constant_evaluation_materializes_every_closed_bootstrap_value() {
        let (_, resolved, output) = check(
            "const Base: Int8 = 40\n\
             const Answer: Int8 = Base + 2\n\
             const Numbers: Array[Int] = [1, 2, 3, 4]\n\
             const Reversed: Array[Int] = Numbers[::-1]\n\
             const Lifted: Array[Int] = Numbers + 10\n\
             const Entries: Map[String, Int] = [\"one\": 1, \"two\": 2]\n\
             const Found: Int? = Entries[\"two\"]\n\
             const Permissions: Set[String] = Set[\"read\", \"write\"]\n\
             const Inside: Bool = 3 in 1..=3\n\
             const TextContains: Bool = 'ñ' in \"año\"\n\
             const Maybe: Int8? = some(Answer)\n\
             const Success: Int ! String = ok(7)\n\
             type UserId = Int\n\
             type User = { id: UserId, name: String }\n\
             enum Shape {\n\
                 Point\n\
                 Circle(Float)\n\
             }\n\
             const Id: UserId = UserId(9)\n\
             const Person: User = User { id: Id, name: \"Ada\" }\n\
             const Renamed: User = Person with { name: \"Grace\" }\n\
             const Form: Shape = Shape.Circle(2.5)\n\
             fn identity(value: Int): Int { value }\n\
             const Handler: fn(Int): Int = identity\n",
        );
        assert!(
            output.diagnostics().is_empty(),
            "{:#?}",
            output.diagnostics()
        );
        assert!(output.is_complete());
        let missing = output
            .program()
            .constants()
            .filter(|(_, constant)| constant.evaluated().is_none())
            .map(|(symbol, _)| {
                resolved
                    .symbol(*symbol)
                    .expect("constant symbol remains resolved")
                    .name()
                    .as_str()
                    .to_owned()
            })
            .collect::<Vec<_>>();
        assert!(missing.is_empty(), "constants not evaluated: {missing:?}");

        let answer = resolved
            .symbols()
            .find(|symbol| symbol.name().as_str() == "Answer")
            .and_then(|symbol| output.program().constant(symbol.id()))
            .and_then(HirConstant::evaluated)
            .expect("Answer is evaluated");
        assert!(matches!(
            answer.kind(),
            crate::hir::HirConstantValueKind::Integer(42)
        ));

        let reversed = resolved
            .symbols()
            .find(|symbol| symbol.name().as_str() == "Reversed")
            .and_then(|symbol| output.program().constant(symbol.id()))
            .and_then(HirConstant::evaluated)
            .expect("Reversed is evaluated");
        let crate::hir::HirConstantValueKind::Array(items) = reversed.kind() else {
            panic!("Reversed must be an evaluated array");
        };
        assert_eq!(
            items
                .iter()
                .map(|item| match item.kind() {
                    crate::hir::HirConstantValueKind::Integer(value) => *value,
                    other => panic!("unexpected reversed item: {other:?}"),
                })
                .collect::<Vec<_>>(),
            [4, 3, 2, 1]
        );
    }

    #[test]
    fn constant_evaluation_rejects_runtime_work_and_every_failing_pure_operation() {
        let (_, _, call) = check(
            "fn runtime(): Int { 1 }\n\
             const Invalid: Int = runtime()\n",
        );
        assert_eq!(codes(&call), ["E1901"]);

        let (_, _, interpolation) = check("const Invalid: String = \"value {1}\"\n");
        assert_eq!(codes(&interpolation), ["E1901"]);

        for source in [
            "const Invalid: Int8 = 127i8 + 1i8\n",
            "const Invalid: Int = 1 / 0\n",
            "const Invalid: Array[Int] = [1, 2] + [3]\n",
            "const Invalid: Int8 = 1i8 << 8\n",
            "const Invalid: Int = [1][2]\n",
            "const Invalid: Array[Int] = [1, 2][::0]\n",
            "const Invalid = Int8(128)\n",
            "const Invalid: Int8 = -128i8 / -1i8\n",
            "const Invalid: UInt8 = 0u8 - 1u8\n",
            "const Invalid = Int(0.5)\n",
            "const Invalid = Int(0.0 / 0.0)\n",
            "const Invalid = Float32(3.4028236e38)\n",
        ] {
            let (_, _, output) = check(source);
            assert_eq!(codes(&output), ["E1903"], "{source}");
        }
    }

    #[test]
    fn constant_logical_operators_short_circuit_before_a_failing_rhs() {
        let (_, resolved, output) = check(
            "const SafeAnd: Bool = false and (1 / 0 == 0)\n\
             const SafeOr: Bool = true or (1 / 0 == 0)\n",
        );
        assert!(
            output.diagnostics().is_empty(),
            "{:#?}",
            output.diagnostics()
        );
        let values = ["SafeAnd", "SafeOr"].map(|name| {
            resolved
                .symbols()
                .find(|symbol| symbol.name().as_str() == name)
                .and_then(|symbol| output.program().constant(symbol.id()))
                .and_then(HirConstant::evaluated)
                .and_then(|value| match value.kind() {
                    crate::hir::HirConstantValueKind::Bool(value) => Some(*value),
                    _ => None,
                })
                .expect("logical constant is evaluated")
        });
        assert_eq!(values, [false, true]);
    }

    #[test]
    fn constant_evaluation_covers_numeric_projection_conversion_and_array_edges() {
        let (_, resolved, output) = check(
            "type UserId = Int\n\
             type User = { id: UserId, name: String }\n\
             const Id: UserId = UserId(9)\n\
             const Person: User = User { id: Id, name: \"Ada\" }\n\
             const IdValue: Int = Id.value\n\
             const PersonName: String = Person.name\n\
             const Pair: (Int, String) = (1, \"two\")\n\
             const PairValue: String = Pair.1\n\
             const Last: Int = [1, 2, 3][-1]\n\
             const LargeStep: Array[Int] = [1, 2][1::9223372036854775807]\n\
             const MinimumStep: Array[Int] = [1, 2][1::-9223372036854775808]\n\
             const Nested: Array[Array[Int]] = [[1, 2], [3, 4]] + [[10, 20], [30, 40]]\n\
             const Negated: Int8 = -5i8\n\
             const Complement: UInt8 = ~0u8\n\
             const Shifted: Int8 = -2i8 >> 1u8\n\
             const MinimumRemainder: Int8 = -128i8 % -1i8\n\
             const Infinite: Float32 = 1.0f32 / 0.0f32\n\
             const Precise: Float32 = 1.0000000596046448031462006156289135105907917022705078125\n\
             const Ordered: Bool = \"a\" < \"b\" and 'a' <= 'a'\n\
             const Absent: Int? = [\"one\": 1][\"missing\"]\n\
             const CharInside: Bool = 'b' in 'a'..='c'\n\
             const Narrowed = Int8(12)\n\
             const Wide: Int = Int(12i8)\n\
             const Rounded: Float32 = Float32(18446744073709551615u64)\n\
             const Lifted: Int? = 1\n\
             const Empty: Int? = none\n\
             const Choice: Int | String = 1\n\
             const Failure: Int ! String = err(\"bad\")\n\
             fn identity[T](value: T): T { value }\n\
             const Handler: fn(Int): Int = identity[Int]\n",
        );
        assert!(
            output.diagnostics().is_empty(),
            "{:#?}",
            output.diagnostics()
        );
        assert!(
            !output.is_complete(),
            "generic bodies remain deferred to M4"
        );
        let missing = output
            .program()
            .constants()
            .filter(|(_, constant)| constant.evaluated().is_none())
            .map(|(symbol, _)| {
                resolved
                    .symbol(*symbol)
                    .expect("constant symbol remains resolved")
                    .name()
                    .as_str()
                    .to_owned()
            })
            .collect::<Vec<_>>();
        assert!(missing.is_empty(), "constants not evaluated: {missing:?}");

        let complement = resolved
            .symbols()
            .find(|symbol| symbol.name().as_str() == "Complement")
            .and_then(|symbol| output.program().constant(symbol.id()))
            .and_then(HirConstant::evaluated)
            .expect("Complement is evaluated");
        assert!(matches!(
            complement.kind(),
            crate::hir::HirConstantValueKind::Integer(255)
        ));

        let minimum_remainder = resolved
            .symbols()
            .find(|symbol| symbol.name().as_str() == "MinimumRemainder")
            .and_then(|symbol| output.program().constant(symbol.id()))
            .and_then(HirConstant::evaluated)
            .expect("MinimumRemainder is evaluated");
        assert!(matches!(
            minimum_remainder.kind(),
            crate::hir::HirConstantValueKind::Integer(0)
        ));

        let precise = resolved
            .symbols()
            .find(|symbol| symbol.name().as_str() == "Precise")
            .and_then(|symbol| output.program().constant(symbol.id()))
            .and_then(HirConstant::evaluated)
            .expect("Precise is evaluated");
        let crate::hir::HirConstantValueKind::Float(bits) = precise.kind() else {
            panic!("Precise must be an evaluated Float32");
        };
        assert_eq!((f64::from_bits(*bits) as f32).to_bits(), 0x3f80_0001);
    }

    #[test]
    fn compile_time_collection_duplicates_and_nan_comparisons_are_diagnosed() {
        let (_, _, map) = check(
            "const KnownKey: String = \"a\"\n\
             const Entries: Map[String, Int] = [KnownKey: 1, \"\\u{61}\": 2]\n",
        );
        assert_eq!(codes(&map), ["E1116"]);

        let (_, _, set) = check("const Values: Set[String] = Set[\"a\", \"\\u{61}\"]\n");
        assert_eq!(codes(&set), ["W1011"]);
        let value = set
            .program()
            .constants()
            .next()
            .and_then(|(_, constant)| constant.evaluated())
            .expect("a duplicate set constant is still evaluated");
        assert!(matches!(
            value.kind(),
            crate::hir::HirConstantValueKind::Set(items) if items.len() == 1
        ));

        let (_, _, dynamic) = check(
            "fn key(): String { \"a\" }\n\
             fn values(): Map[String, Int] { [key(): 1, key(): 2] }\n",
        );
        assert!(
            dynamic.diagnostics().is_empty(),
            "{:#?}",
            dynamic.diagnostics()
        );

        let (_, _, tagged_union) =
            check("const Entries: Map[Int8 | UInt8, Int] = [1i8: 1, 1u8: 2]\n");
        assert!(
            tagged_union.diagnostics().is_empty(),
            "union tags distinguish equal payloads: {:#?}",
            tagged_union.diagnostics()
        );

        let (_, _, nan) = check(
            "const Zero: Float = 0.0\n\
             const Nan: Float = Zero / Zero\n\
             const Known: Bool = Nan == Nan\n",
        );
        assert_eq!(codes(&nan), ["W1008"]);
    }

    #[test]
    fn numeric_context_handles_signed_minimum_unions_and_shift_rhs_types() {
        let (_, _, valid) = check(
            "fn minimum(): Int8 { -128 }\n\
             fn suffixed(): Int8 { -128i8 }\n\
             fn shifted(): Int8 { 1i8 << 2u32 }\n",
        );
        assert!(valid.diagnostics().is_empty(), "{:#?}", valid.diagnostics());

        let (_, _, overflow) = check("fn invalid(): Int8 { -129 }\n");
        assert_eq!(codes(&overflow), ["E1102"]);

        let (_, _, unsigned) = check("fn invalid(): UInt8 { -1 }\n");
        assert_eq!(codes(&unsigned), ["E1102"]);

        let (_, _, ambiguous) = check("fn invalid(): Int8 | UInt8 { 1 }\n");
        assert_eq!(codes(&ambiguous), ["E1101"]);
    }

    #[test]
    fn none_and_call_modes_require_their_direct_declared_context() {
        let (_, _, invalid_none) = check("fn invalid(): Int? | String { none }\n");
        assert_eq!(codes(&invalid_none), ["E1304"]);

        let (_, _, invalid_mode) = check(
            "fn inspect(value: ref Int) {}\n\
             fn main() {\n\
                 let value = 1\n\
                 inspect(value)\n\
             }\n",
        );
        assert_eq!(codes(&invalid_mode), ["E1407"]);

        let (_, _, valid_mode) = check(
            "fn inspect(value: ref Int) {}\n\
             fn main() {\n\
                 let value = 1\n\
                 inspect(ref value)\n\
             }\n",
        );
        assert!(valid_mode.diagnostics().is_empty());
    }

    #[test]
    fn if_and_return_propagate_expected_types_and_never() {
        let (_, _, output) = check(
            "fn choose(flag: Bool): Int {\n\
                 if flag { 1 } else { 2 }\n\
             }\n\
             fn early(flag: Bool): Int {\n\
                 if flag {\n\
                     return 1\n\
                 }\n\
                 2\n\
             }\n\
             fn direct(): Int {\n\
                 return 3\n\
             }\n",
        );
        assert!(
            output.diagnostics().is_empty(),
            "{:#?}",
            output.diagnostics()
        );
        assert!(output.is_complete());

        let (_, _, condition) = check("fn invalid() {\n    if 1 { () }\n}\n");
        assert_eq!(codes(&condition), ["E1102"]);

        let (_, _, branches) = check(
            "fn invalid(flag: Bool) {\n\
                 let value = if flag { 1 } else { \"text\" }\n\
             }\n",
        );
        assert_eq!(codes(&branches), ["E1101"]);

        let (_, _, missing) = check("fn invalid(): Int {\n    return\n}\n");
        assert_eq!(codes(&missing), ["E1205"]);

        let (_, _, joined) = check(
            "fn inferred(flag: Bool, wide: Int | String) {\n\
                 let value = if flag { 1 } else { wide }\n\
             }\n",
        );
        assert!(
            joined.diagnostics().is_empty(),
            "{:#?}",
            joined.diagnostics()
        );
        assert!(joined.program().expressions().any(|expression| {
            matches!(
                expression.kind(),
                HirExpressionKind::Coerce {
                    kind: Assignability::UnionInjection,
                    ..
                }
            )
        }));
    }

    #[test]
    fn all_three_for_forms_and_loop_transfers_are_typed() {
        let (_, _, output) = check(
            "fn loops(values: Array[Int], text: String) {\n\
                 for {\n\
                     break\n\
                 }\n\
                 for false {\n\
                     continue\n\
                 }\n\
                 for value in values {\n\
                     let copy = value\n\
                 }\n\
                 for character in text {\n\
                     let copy = character\n\
                 }\n\
             }\n",
        );
        assert!(
            output.diagnostics().is_empty(),
            "{:#?}",
            output.diagnostics()
        );
        assert!(output.is_complete());

        let (_, _, source) = check(
            "fn invalid() {\n\
                 for value in 42 { () }\n\
             }\n",
        );
        assert_eq!(codes(&source), ["E1206"]);

        let (_, _, deferred) = check(
            "type Cursor = { value: Int }\n\
             fn inspect(cursor: Cursor) {\n\
                 for value in cursor { () }\n\
             }\n",
        );
        assert!(deferred.diagnostics().is_empty());
        assert!(!deferred.is_complete());

        let (_, _, transfer) = check("fn invalid() {\n    break\n}\n");
        assert_eq!(codes(&transfer), ["E1205"]);
    }

    #[test]
    fn infinite_loop_flow_uses_only_reachable_breaks_for_that_loop() {
        let (_, _, infinite) = check("fn run(): Never {\n    for {}\n}\n");
        assert!(infinite.diagnostics().is_empty());
        let root = only_body_root(&infinite);
        assert_eq!(
            infinite.program().expression_flow(root),
            Some(HirFlow::Diverges)
        );
        assert_eq!(
            infinite.program().expression(root).unwrap().ty(),
            infinite.program().interner().scalar(ScalarType::Never)
        );

        let (_, _, escaping) = check("fn run() {\n    for {\n        break\n    }\n}\n");
        assert!(escaping.diagnostics().is_empty());
        let root = only_body_root(&escaping);
        assert_eq!(
            escaping.program().expression_flow(root),
            Some(HirFlow::MayComplete)
        );
        assert_eq!(
            escaping.program().expression(root).unwrap().ty(),
            escaping.program().interner().scalar(ScalarType::Unit)
        );

        let (_, _, unreachable) = check(
            "fn run(): Never {\n\
                 for {\n\
                     continue\n\
                     break\n\
                 }\n\
             }\n",
        );
        assert_eq!(codes(&unreachable), ["W1006"]);
        assert_eq!(unreachable.diagnostics()[0].severity(), Severity::Warning);
        let root = only_body_root(&unreachable);
        assert_eq!(
            unreachable.program().expression_flow(root),
            Some(HirFlow::Diverges)
        );

        let (_, _, after_return) = check(
            "fn run() {\n\
                 for {\n\
                     return\n\
                     break\n\
                 }\n\
             }\n",
        );
        assert_eq!(codes(&after_return), ["W1006"]);
        assert_eq!(
            after_return
                .program()
                .expression_flow(only_body_root(&after_return)),
            Some(HirFlow::Diverges)
        );

        let (_, _, nested) = check(
            "fn run(): Never {\n\
                 for {\n\
                     for {\n\
                         break\n\
                     }\n\
                 }\n\
             }\n",
        );
        assert!(nested.diagnostics().is_empty());
        let root = only_body_root(&nested);
        assert_eq!(
            nested.program().expression_flow(root),
            Some(HirFlow::Diverges)
        );
    }

    #[test]
    fn branch_flow_joins_if_and_match_paths_instead_of_their_contextual_types() {
        let (_, _, all_if) = check(
            "fn run(flag: Bool): Never {\n\
                 if flag {\n\
                     for {}\n\
                 } else {\n\
                     for {}\n\
                 }\n\
             }\n",
        );
        assert!(
            all_if.diagnostics().is_empty(),
            "{:#?}",
            all_if.diagnostics()
        );
        assert_eq!(
            all_if.program().expression_flow(only_body_root(&all_if)),
            Some(HirFlow::Diverges)
        );

        let (_, _, partial_if) = check(
            "fn run(flag: Bool) {\n\
                 if flag {\n\
                     for {}\n\
                 } else {\n\
                     ()\n\
                 }\n\
             }\n",
        );
        assert!(partial_if.diagnostics().is_empty());
        assert_eq!(
            partial_if
                .program()
                .expression_flow(only_body_root(&partial_if)),
            Some(HirFlow::MayComplete)
        );

        let (_, _, all_match) = check(
            "fn halt(): Never {\n\
                 for {}\n\
             }\n\
             fn run(flag: Bool): Never {\n\
                 match flag {\n\
                     true => halt()\n\
                     false => halt()\n\
                 }\n\
             }\n",
        );
        assert!(
            all_match.diagnostics().is_empty(),
            "{:#?}",
            all_match.diagnostics()
        );
        let match_expression = all_match
            .program()
            .expressions()
            .enumerate()
            .find_map(|(index, expression)| {
                matches!(expression.kind(), HirExpressionKind::Match { .. })
                    .then_some(HirExpressionId(index as u32))
            })
            .expect("the match expression is retained");
        assert_eq!(
            all_match.program().expression_flow(match_expression),
            Some(HirFlow::Diverges)
        );
        assert_eq!(
            all_match
                .program()
                .expression(match_expression)
                .unwrap()
                .ty(),
            all_match.program().interner().scalar(ScalarType::Never)
        );

        let (_, _, partial_match) = check(
            "fn halt(): Never {\n\
                 for {}\n\
             }\n\
             fn run(flag: Bool) {\n\
                 match flag {\n\
                     true => halt()\n\
                     false => ()\n\
                 }\n\
             }\n",
        );
        assert!(partial_match.diagnostics().is_empty());
        assert!(
            partial_match
                .program()
                .expressions()
                .enumerate()
                .any(|(index, expression)| {
                    matches!(expression.kind(), HirExpressionKind::Match { .. })
                        && partial_match
                            .program()
                            .expression_flow(HirExpressionId(index as u32))
                            == Some(HirFlow::MayComplete)
                })
        );
    }

    #[test]
    fn unreachable_code_warnings_follow_real_transfers_without_error_cascades() {
        let (_, _, output) = check(
            "fn after_return() {\n\
                 return\n\
                 let value = 1\n\
             }\n\
             fn after_fail(): Unit ! String {\n\
                 fail \"failed\"\n\
                 let value = 1\n\
             }\n\
             fn after_break() {\n\
                 for {\n\
                     break\n\
                     let value = 1\n\
                 }\n\
             }\n\
             fn after_continue(): Never {\n\
                 for {\n\
                     continue\n\
                     let value = 1\n\
                 }\n\
             }\n",
        );
        assert_eq!(codes(&output), ["W1006", "W1006", "W1006", "W1006"]);
        assert!(
            output
                .diagnostics()
                .iter()
                .all(|diagnostic| diagnostic.severity() == Severity::Warning)
        );

        let (_, _, conditional) = check(
            "fn run(flag: Bool) {\n\
                 if flag {\n\
                     return\n\
                 }\n\
                 let reachable = 1\n\
             }\n",
        );
        assert!(conditional.diagnostics().is_empty());

        let (_, _, invalid) = check(
            "fn run() {\n\
                 break\n\
                 let still_checked = 1\n\
             }\n",
        );
        assert_eq!(codes(&invalid), ["E1205"]);

        let (_, _, invalid_continue) = check(
            "fn run() {\n\
                 continue\n\
                 let still_checked = 1\n\
             }\n",
        );
        assert_eq!(codes(&invalid_continue), ["E1205"]);
    }

    #[test]
    fn contextual_never_coercions_preserve_divergent_flow() {
        let (_, _, output) = check(
            "fn halt(): Never {\n\
                 for {}\n\
             }\n\
             fn integer(): Int { halt() }\n",
        );
        assert!(output.diagnostics().is_empty());
        assert!(
            output
                .program()
                .expressions()
                .enumerate()
                .any(|(index, expression)| {
                    matches!(expression.kind(), HirExpressionKind::Coerce { .. })
                        && expression.ty() == output.program().interner().scalar(ScalarType::Int)
                        && output
                            .program()
                            .expression_flow(HirExpressionId(index as u32))
                            == Some(HirFlow::Diverges)
                })
        );
    }

    #[test]
    fn result_construction_fail_and_both_propagation_channels_are_typed() {
        let (_, resolved, output) = check(
            "fn source(): Int ! String { 1 }\n\
             fn multi(): Int ! (Bool | String) { 1 }\n\
             fn optional(): Int? { some(1) }\n\
             fn widened(flag: Bool): Int ! (Bool | String) {\n\
                 if flag {\n\
                     return source()?\n\
                 }\n\
                 fail true\n\
             }\n\
             fn explicit_ok(): Int ! String { ok(1) }\n\
             fn explicit_err(): Int ! String { err(\"bad\") }\n\
             fn forward(): Int ! String { source() }\n\
             fn wider(): Int ! (Bool | Char | String) { multi()? }\n\
             fn fail_wider(error: Bool | String): Int ! (Bool | Char | String) {\n\
                 fail error\n\
             }\n\
             fn unwrap_optional(): Int? { optional()? }\n\
             fn nested(): Int? ! String { optional()? }\n",
        );
        assert!(
            output.diagnostics().is_empty(),
            "{:#?}",
            output.diagnostics()
        );
        assert!(output.is_complete());

        let mut has_some = false;
        let mut has_ok = false;
        let mut has_err = false;
        let mut has_fail = false;
        let mut has_option_propagation = false;
        let mut has_widened_result_propagation = false;
        let mut has_union_error_widening = false;
        for expression in output.program().expressions() {
            match expression.kind() {
                HirExpressionKind::OptionSome { .. } => has_some = true,
                HirExpressionKind::ResultOk { .. } => has_ok = true,
                HirExpressionKind::ResultErr { .. } => has_err = true,
                HirExpressionKind::Fail { .. } => has_fail = true,
                HirExpressionKind::PropagateOption { .. } => has_option_propagation = true,
                HirExpressionKind::PropagateResult {
                    error_coercion: Assignability::UnionInjection,
                    ..
                } => has_widened_result_propagation = true,
                HirExpressionKind::PropagateResult {
                    error_coercion: Assignability::UnionWidening,
                    ..
                } => has_union_error_widening = true,
                _ => {}
            }
        }
        assert!(has_some);
        assert!(has_ok);
        assert!(has_err);
        assert!(has_fail);
        assert!(has_option_propagation);
        assert!(has_widened_result_propagation);
        assert!(has_union_error_widening);

        let forward = resolved
            .symbols()
            .find(|symbol| symbol.name().as_str() == "forward")
            .expect("forward is resolved");
        let root = output
            .program()
            .body(HirCallableId::Symbol(forward.id()))
            .expect("forward has a checked body")
            .root();
        let HirExpressionKind::Block {
            tail: Some(tail), ..
        } = output.program().expression(root).unwrap().kind()
        else {
            panic!("forward must retain its source block and tail");
        };
        assert!(matches!(
            output.program().expression(*tail).unwrap().kind(),
            HirExpressionKind::Call { .. }
        ));
    }

    #[test]
    fn reachability_warnings_follow_nested_evaluation_order_without_cascades() {
        let (_, _, output) = check(
            "fn halt(): Never {\n\
                 for {}\n\
             }\n\
             fn consume(first: Int, second: Int) {}\n\
             fn tupled() {\n\
                 let pair = (halt(), 1)\n\
             }\n\
             fn called() {\n\
                 consume(halt(), 1)\n\
             }\n\
             fn branched() {\n\
                 if halt() {\n\
                     return\n\
                     let nested = 1\n\
                 } else {\n\
                     ()\n\
                 }\n\
             }\n\
             fn assigned(values: var Array[Int]) {\n\
                 values[halt()] = 1\n\
             }\n",
        );
        assert_eq!(
            codes(&output),
            ["W1006", "W1006", "W1006", "W1006", "W1006"],
            "{:#?}",
            output.diagnostics()
        );
        assert!(
            output
                .diagnostics()
                .iter()
                .all(|diagnostic| diagnostic.severity() == Severity::Warning)
        );
    }

    #[test]
    fn a_divergent_loop_header_makes_the_loop_diverge_before_its_body() {
        let (_, _, output) = check(
            "fn halt(): Never {\n\
                 for {}\n\
             }\n\
             fn run(): Never {\n\
                 for halt() {\n\
                     ()\n\
                 }\n\
             }\n",
        );
        assert_eq!(codes(&output), ["W1006"]);
        let run = output
            .program()
            .callables()
            .nth(1)
            .expect("run is the second callable");
        let root = output.program().body(run.id()).unwrap().root();
        assert_eq!(
            output.program().expression_flow(root),
            Some(HirFlow::Diverges)
        );
    }

    #[test]
    fn result_channel_errors_use_the_normative_diagnostics() {
        let (_, _, fail_context) = check("fn invalid() {\n    fail \"bad\"\n}\n");
        assert_eq!(codes(&fail_context), ["E1302"]);

        let (_, _, fail_type) = check("fn invalid(): Int ! Bool {\n    fail \"bad\"\n}\n");
        assert_eq!(codes(&fail_type), ["E1302"]);

        let (_, _, result_context) = check(
            "fn source(): Int ! String { 1 }\n\
             fn invalid(): Int { source()? }\n",
        );
        assert_eq!(codes(&result_context), ["E1301"]);

        let (_, _, result_type) = check(
            "fn source(): Int ! String { 1 }\n\
             fn invalid(): Int ! Bool { source()? }\n",
        );
        assert_eq!(codes(&result_type), ["E1301"]);
        assert!(result_type.diagnostics()[0].message().contains("String"));
        assert!(result_type.diagnostics()[0].message().contains("Bool"));

        let (_, _, union_subset) = check(
            "fn source(): Int ! (Bool | String) { 1 }\n\
             fn invalid(): Int ! (Char | String) { source()? }\n",
        );
        assert_eq!(codes(&union_subset), ["E1301"]);
        assert!(union_subset.diagnostics()[0].message().contains("Bool"));

        let (_, _, option_context) = check(
            "fn optional(): Int? { none }\n\
             fn invalid(): Int { optional()? }\n",
        );
        assert_eq!(codes(&option_context), ["E1301"]);

        let (_, _, missing_constructor_context) =
            check("fn invalid() {\n    let value = ok(1)\n}\n");
        assert_eq!(codes(&missing_constructor_context), ["E1304"]);

        let (_, _, invalid_payload) = check("fn invalid(): Int ! Bool { err(\"bad\") }\n");
        assert_eq!(codes(&invalid_payload), ["E1304"]);
    }

    #[test]
    fn match_checks_finite_domains_guards_and_nested_payloads() {
        let (_, _, output) = check(
            "fn bool_value(value: Bool): Int {\n\
                 match value {\n\
                     true => 1\n\
                     false => 0\n\
                 }\n\
             }\n\
             fn optional(value: Bool?): Int {\n\
                 match value {\n\
                     some(true) => 1\n\
                     some(false) => 0\n\
                     none => -1\n\
                 }\n\
             }\n\
             fn guarded(value: Bool, enabled: Bool): Int {\n\
                 match value {\n\
                     true if enabled => 2\n\
                     true => 1\n\
                     false => 0\n\
                 }\n\
             }\n\
             fn result(value: Int ! String): Int {\n\
                 match value {\n\
                     ok(number) => number\n\
                     err(_) => 0\n\
                 }\n\
             }\n",
        );
        assert!(
            output.diagnostics().is_empty(),
            "{:#?}",
            output.diagnostics()
        );
        assert!(output.is_complete());
        assert!(
            output
                .program()
                .expressions()
                .any(|expression| matches!(expression.kind(), HirExpressionKind::Match { .. }))
        );
    }

    #[test]
    fn tuple_patterns_are_irrefutable_but_variant_patterns_are_not() {
        let (_, _, tuple) = check(
            "fn destructure(pair: (Int, String)): Int {\n\
                 let (number, _) = pair\n\
                 number\n\
             }\n",
        );
        assert!(tuple.diagnostics().is_empty(), "{:#?}", tuple.diagnostics());

        let (_, _, option) = check(
            "fn invalid(value: Int?) {\n\
                 let some(number) = value\n\
             }\n",
        );
        assert_eq!(codes(&option), ["E1201"]);
    }

    #[test]
    fn match_reports_invalid_unreachable_and_non_exhaustive_patterns() {
        let (_, _, invalid) = check(
            "fn invalid(value: Bool): Int {\n\
                 match value {\n\
                     some(_) => 1\n\
                     _ => 0\n\
                 }\n\
             }\n",
        );
        assert_eq!(codes(&invalid), ["E1202"]);

        let (_, _, invalid_payload) = check(
            "fn invalid(value: Int?): Int {\n\
                 match value {\n\
                     some(true) => 1\n\
                     none => 0\n\
                 }\n\
             }\n",
        );
        assert_eq!(codes(&invalid_payload), ["E1202"]);

        let (_, _, unreachable) = check(
            "fn invalid(value: Bool): Int {\n\
                 match value {\n\
                     _ => 0\n\
                     true => 1\n\
                 }\n\
             }\n",
        );
        assert_eq!(codes(&unreachable), ["E1203"]);

        let (_, _, missing) = check(
            "fn invalid(value: Bool): Int {\n\
                 match value {\n\
                     true => 1\n\
                 }\n\
             }\n",
        );
        assert_eq!(codes(&missing), ["E1204"]);

        let (_, _, guarded) = check(
            "fn invalid(value: Bool): Int {\n\
                 match value {\n\
                     true if value => 1\n\
                     false => 0\n\
                 }\n\
             }\n",
        );
        assert_eq!(codes(&guarded), ["E1204"]);
    }

    #[test]
    fn literal_pattern_coverage_uses_decoded_scalar_values() {
        let (_, _, character) = check(
            "fn inspect(value: Char): Int {\n\
                 match value {\n\
                     'a' => 0\n\
                     '\\u{61}' => 1\n\
                     _ => 2\n\
                 }\n\
             }\n",
        );
        assert_eq!(codes(&character), ["E1203"]);

        let (_, _, escaped_string) = check(
            "fn inspect(value: String): Int {\n\
                 match value {\n\
                     \"a\" => 0\n\
                     \"\\u{61}\" => 1\n\
                     _ => 2\n\
                 }\n\
             }\n",
        );
        assert_eq!(codes(&escaped_string), ["E1203"]);

        let (_, _, raw_string) = check(
            "fn inspect(value: String): Int {\n\
                 match value {\n\
                     \"{{\" => 0\n\
                     r\"{\" => 1\n\
                     _ => 2\n\
                 }\n\
             }\n",
        );
        assert_eq!(codes(&raw_string), ["E1203"]);

        assert_eq!(
            decode_string_literal_pattern(
                "\"\"\"\n    alpha\n    beta\n    \"\"\"",
                TokenKind::MultilineStringStart
            ),
            Some("alpha\nbeta".to_owned())
        );
        assert_eq!(
            decode_string_literal_pattern(
                "r\"\"\"\r\n    \\n\r\n    \"\"\"",
                TokenKind::RawMultilineStringLiteral
            ),
            Some("\\n".to_owned())
        );
    }

    #[test]
    fn nominal_and_union_patterns_use_instantiated_payload_types() {
        let (_, _, output) = check(
            "type Pair[T] = {\n\
                 first: T\n\
                 second: T\n\
             }\n\
             type UserId = Int\n\
             enum Choice[T] {\n\
                 Empty\n\
                 Item(T)\n\
                 Named { value: T }\n\
             }\n\
             fn record(pair: Pair[Int]): Int {\n\
                 let Pair { first, .. } = pair\n\
                 first\n\
             }\n\
             fn newtype(id: UserId): Int {\n\
                 let UserId(value) = id\n\
                 value\n\
             }\n\
             fn choice(subject: Choice[Int]): Int {\n\
                 match subject {\n\
                     Choice.Empty => 0\n\
                     Choice.Item(number) => number\n\
                     Choice.Named { value } => value\n\
                 }\n\
             }\n\
             fn union(value: Int | String): Int {\n\
                 match value {\n\
                     Int(number) => number\n\
                     String(_) => 0\n\
                 }\n\
             }\n",
        );
        assert!(
            output.diagnostics().is_empty(),
            "{:#?}",
            output.diagnostics()
        );
        assert!(output.is_complete());
        assert!(
            output
                .program()
                .patterns
                .iter()
                .any(|pattern| matches!(pattern.kind(), HirPatternKind::UnionMember { .. }))
        );
    }

    #[test]
    fn explicit_generic_pattern_paths_must_match_the_scrutinee_instance() {
        let (_, _, valid) = check(
            "alias Numbers[T] = Array[T]\n\
             type Box[T] = T\n\
             type Pair[T] = { value: T }\n\
             enum Choice[T] {\n\
                 Empty\n\
                 Item(T)\n\
                 Named { value: T }\n\
             }\n\
             fn unbox(boxed: Box[Array[Int?]]): Array[Int?] {\n\
                 let Box[Array[Int?]](value) = boxed\n\
                 value\n\
             }\n\
             fn field(pair: Pair[Int]): Int {\n\
                 let Pair[Int] { value } = pair\n\
                 value\n\
             }\n\
             fn inspect(subject: Choice[Int]): Int {\n\
                 match subject {\n\
                     Choice.Empty[Int] => 0\n\
                     Choice.Item[Int](number) => number\n\
                     Choice.Named[Int] { value } => value\n\
                 }\n\
             }\n\
             fn discriminate(input: Numbers[Int] | Option[String]): Int {\n\
                 match input {\n\
                     Numbers[Int](numbers) => 1\n\
                     Option[String](optional) => 0\n\
                 }\n\
             }\n",
        );
        assert!(valid.diagnostics().is_empty(), "{:#?}", valid.diagnostics());
        assert!(valid.is_complete());

        let (_, _, invalid) = check(
            "enum Choice[T] {\n\
                 Empty\n\
                 Item(T)\n\
             }\n\
             fn inspect(subject: Choice[Int]): Int {\n\
                 match subject {\n\
                     Choice.Item[String](text) => 1\n\
                     _ => 0\n\
                 }\n\
             }\n",
        );
        assert_eq!(codes(&invalid), ["E1202"]);
    }

    #[test]
    fn imported_nominal_pattern_paths_keep_the_type_and_variant_boundary() {
        let mut sources = SourceDatabase::new();
        let source_id = SourceId::new("root:imported-pattern-check").unwrap();
        let shapes = sources
            .add(SourceInput::virtual_file(
                source_id.clone(),
                ModulePath::new("shapes").unwrap(),
                LogicalPath::new("shapes.to").unwrap(),
                Arc::<[u8]>::from(
                    &b"pub type Pair[T] = { value: T }\npub enum Choice[T] {\n    Empty\n    Item(T)\n}\n"[..],
                ),
            ))
            .unwrap();
        let main = sources
            .add(SourceInput::virtual_file(
                source_id,
                ModulePath::new("main").unwrap(),
                LogicalPath::new("main.to").unwrap(),
                Arc::<[u8]>::from(
                    &b"import main.shapes\nfn field(pair: shapes.Pair[Int]): Int {\n    let shapes.Pair[Int] { value } = pair\n    value\n}\nfn inspect(subject: shapes.Choice[Int]): Int {\n    match subject {\n        shapes.Choice.Empty[Int] => 0\n        shapes.Choice.Item[Int](number) => number\n    }\n}\n"[..],
                ),
            ))
            .unwrap();
        let mut parsed = Vec::new();
        for file in [shapes, main] {
            let lexed = lex(&sources, file, LexMode::Module).unwrap();
            assert!(lexed.diagnostics().is_empty());
            let tree = parse(
                &sources,
                file,
                lexed,
                ParseMode::Module,
                ParseLimits::default(),
            )
            .unwrap();
            assert!(tree.diagnostics().is_empty(), "{:#?}", tree.diagnostics());
            parsed.push((file, tree));
        }
        let packages = PackageGraph::loose(&sources, main).unwrap();
        let resolved = resolve(
            &packages,
            &sources,
            parsed.iter().map(|(file, parsed)| (*file, parsed)),
            100,
        )
        .unwrap();
        let (resolved, diagnostics) = resolved.into_parts();
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        let lowered = lower_types(
            &packages,
            &sources,
            parsed.iter().map(|(file, parsed)| (*file, parsed)),
            &resolved,
            TypeLoweringLimits {
                max_type_nodes: 100_000,
                max_diagnostics: 100,
            },
        )
        .unwrap();
        let (program, diagnostics) = lowered.into_parts();
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        let checked = check_expressions(
            &sources,
            parsed.iter().map(|(file, parsed)| (*file, parsed)),
            &resolved,
            program,
            ExpressionCheckLimits {
                max_nodes: 100_000,
                max_pattern_steps: 100_000,
                max_diagnostics: 100,
            },
        )
        .unwrap();
        assert!(
            checked.diagnostics().is_empty(),
            "{:#?}",
            checked.diagnostics()
        );
        assert!(checked.is_complete());
    }

    #[test]
    fn nominal_pattern_shape_errors_and_duplicate_variants_are_diagnosed() {
        let (_, _, missing_field) = check(
            "type Pair = { first: Int, second: Int }\n\
             fn invalid(pair: Pair) {\n\
                 let Pair { first } = pair\n\
             }\n",
        );
        assert_eq!(codes(&missing_field), ["E1202"]);

        let (_, _, wrong_variant) = check(
            "enum Choice {\n\
                 Empty\n\
                 Item(Int)\n\
             }\n\
             fn invalid(value: Choice): Int {\n\
                 match value {\n\
                     Choice.Missing => 0\n\
                     _ => 1\n\
                 }\n\
             }\n",
        );
        assert_eq!(codes(&wrong_variant), ["E1202"]);

        let (_, _, duplicate) = check(
            "enum Choice {\n\
                 Empty\n\
                 Item(Int)\n\
             }\n\
             fn invalid(value: Choice): Int {\n\
                 match value {\n\
                     Choice.Empty => 0\n\
                     Choice.Empty => 1\n\
                     Choice.Item(_) => 2\n\
                 }\n\
             }\n",
        );
        assert_eq!(codes(&duplicate), ["E1203"]);
    }

    #[test]
    fn array_patterns_type_prefix_and_rest_and_prove_shape_coverage() {
        let (_, resolved, output) = check(
            "fn classify(values: Array[Int]): Int {\n\
                 match values {\n\
                     [] => 0\n\
                     [first, ..remaining] => first\n\
                 }\n\
             }\n\
             fn observe(items: Array[Bool]) {\n\
                 match items {\n\
                     [] => ()\n\
                     [true, ..] => ()\n\
                     [false, ..ref tail] => ()\n\
                 }\n\
             }\n",
        );
        assert!(
            output.diagnostics().is_empty(),
            "{:#?}",
            output.diagnostics()
        );
        assert!(output.is_complete());

        let remaining = resolved
            .locals()
            .find(|local| local.name().as_str() == "remaining")
            .expect("array rest binding is resolved");
        let remaining_type = output
            .program()
            .local_type(remaining.id())
            .expect("array rest binding is typed");
        assert!(matches!(
            output.program().interner().kind(remaining_type).unwrap(),
            TypeKind::Intrinsic {
                constructor: IntrinsicType::Array,
                arguments
            } if arguments.len() == 1
        ));
        assert!(output.program().patterns.iter().any(|pattern| {
            matches!(pattern.kind(), HirPatternKind::Array { rest: Some(_), .. })
        }));
        assert!(
            output
                .program()
                .patterns
                .iter()
                .any(|pattern| { matches!(pattern.kind(), HirPatternKind::BorrowBinding(_)) })
        );
    }

    #[test]
    fn array_patterns_report_refutability_invalid_shapes_and_coverage_gaps() {
        let (_, _, binding) = check(
            "fn invalid(values: Array[Int]) {\n\
                 let [first, ..rest] = values\n\
             }\n",
        );
        assert_eq!(codes(&binding), ["E1201"]);

        let (_, _, borrowed_binding) = check(
            "fn invalid(values: Array[Int]) {\n\
                 let [first, ..ref rest] = values\n\
             }\n",
        );
        assert_eq!(codes(&borrowed_binding), ["E1202"]);

        let (_, _, wrong_type) = check(
            "fn invalid(value: Int): Int {\n\
                 match value {\n\
                     [] => 0\n\
                     _ => 1\n\
                 }\n\
             }\n",
        );
        assert_eq!(codes(&wrong_type), ["E1202"]);

        let (_, _, redundant_borrow) = check(
            "fn invalid(value: Int): Int {\n\
                 match value {\n\
                     ref _ => 0\n\
                 }\n\
             }\n",
        );
        assert_eq!(codes(&redundant_borrow), ["E1202"]);

        let (_, _, non_exhaustive) = check(
            "fn invalid(values: Array[Int]): Int {\n\
                 match values {\n\
                     [] => 0\n\
                     [1, ..] => 1\n\
                 }\n\
             }\n",
        );
        assert_eq!(codes(&non_exhaustive), ["E1204"]);

        let (_, _, unreachable) = check(
            "fn invalid(values: Array[Int]): Int {\n\
                 match values {\n\
                     [_, ..] => 1\n\
                     [_, _] => 2\n\
                     [] => 0\n\
                 }\n\
             }\n",
        );
        assert_eq!(codes(&unreachable), ["E1203"]);
    }

    #[test]
    fn wide_array_patterns_use_the_analysis_worklist_instead_of_recursion() {
        let prefix = vec!["_"; 4_096].join(", ");
        let source = format!(
            "fn inspect(values: Array[Int]): Int {{\n    match values {{\n        [{prefix}] => 1\n        _ => 0\n    }}\n}}\n"
        );
        let (_, _, output) = check(&source);
        assert!(
            output.diagnostics().is_empty(),
            "{:#?}",
            output.diagnostics()
        );
        assert!(output.is_complete());
    }

    #[test]
    fn match_arms_type_guards_and_direct_control_transfers() {
        let (_, _, output) = check(
            "fn classify(value: Bool): Int {\n\
                 match value {\n\
                     true => return 1\n\
                     false => 2\n\
                 }\n\
             }\n\
             fn fallible(flag: Bool): Int ! String {\n\
                 match flag {\n\
                     true => fail \"bad\"\n\
                     false => 1\n\
                 }\n\
             }\n\
             fn looped(condition: Bool) {\n\
                 for {\n\
                     match condition {\n\
                         true => break\n\
                         false => continue\n\
                     }\n\
                 }\n\
             }\n",
        );
        assert!(
            output.diagnostics().is_empty(),
            "{:#?}",
            output.diagnostics()
        );
        assert!(output.is_complete());
        assert!(
            output.program().expressions().any(|expression| {
                matches!(expression.kind(), HirExpressionKind::Return { .. })
            })
        );
        assert!(
            output
                .program()
                .expressions()
                .any(|expression| { matches!(expression.kind(), HirExpressionKind::Fail { .. }) })
        );
        assert!(
            output
                .program()
                .expressions()
                .any(|expression| { matches!(expression.kind(), HirExpressionKind::Break { .. }) })
        );
        assert!(
            output.program().expressions().any(|expression| {
                matches!(expression.kind(), HirExpressionKind::Continue { .. })
            })
        );

        let (_, _, invalid_guard) = check(
            "fn invalid(value: Bool): Int {\n\
                 match value {\n\
                     true if 1 => 1\n\
                     _ => 0\n\
                 }\n\
             }\n",
        );
        assert_eq!(codes(&invalid_guard), ["E1102"]);
    }

    #[test]
    fn prelude_panic_and_variadic_assert_are_typed_as_runtime_operations() {
        let (_, _, output) = check(
            "fn stop(): Never { panic(\"stop\") }\n\
             fn verify(parts: Array[String]) {\n\
                 assert(true)\n\
                 assert(true, \"prefix\", ...parts)\n\
                 assert(condition: true, messageParts: ...parts)\n\
             }\n",
        );
        assert!(
            output.diagnostics().is_empty(),
            "{:#?}",
            output.diagnostics()
        );
        assert!(output.is_complete());

        let panic = output
            .program()
            .expressions()
            .enumerate()
            .find(|(_, expression)| {
                matches!(expression.kind(), HirExpressionKind::PreludePanic { .. })
            })
            .expect("panic call is retained as a dedicated HIR expression");
        assert_eq!(
            panic.1.ty(),
            output.program().interner().scalar(ScalarType::Never)
        );
        assert_eq!(
            output
                .program()
                .expression_flow(HirExpressionId(panic.0 as u32)),
            Some(HirFlow::Diverges)
        );

        let assertions = output
            .program()
            .expressions()
            .filter_map(|expression| match expression.kind() {
                HirExpressionKind::PreludeAssert { message_parts, .. } => Some(message_parts),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(assertions.len(), 3);
        assert!(assertions[0].is_empty());
        assert_eq!(assertions[1].len(), 2);
        assert!(!assertions[1][0].is_spread());
        assert!(assertions[1][1].is_spread());
        assert_eq!(assertions[2].len(), 1);
        assert!(assertions[2][0].is_spread());
    }

    #[test]
    fn prelude_panic_and_assert_reject_invalid_call_shapes() {
        for source in [
            "fn invalid() { panic() }\n",
            "fn invalid() { panic(1) }\n",
            "fn invalid() { assert() }\n",
            "fn invalid() { assert(1) }\n",
            "fn invalid() { assert(true, 1) }\n",
            "fn invalid(parts: Array[String]) { assert(true, ...parts, \"tail\") }\n",
        ] {
            let (_, _, output) = check(source);
            assert!(
                codes(&output).contains(&"E1102"),
                "source should fail with E1102: {source}\n{:#?}",
                output.diagnostics()
            );
        }
    }

    #[test]
    fn bootstrap_console_print_has_one_canonical_typed_call_shape() {
        let (_, _, output) = check(
            "import std.console\n\
             fn main() { console.print(\"Hello, Tondo!\") }\n",
        );
        assert!(
            output.diagnostics().is_empty(),
            "{:#?}",
            output.diagnostics()
        );
        assert!(output.is_complete());
        let calls = output
            .program()
            .expressions()
            .filter(|expression| {
                matches!(
                    expression.kind(),
                    HirExpressionKind::BootstrapHostCall {
                        function: HirBootstrapHostFunction::ConsolePrint,
                        ..
                    }
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].ty(),
            output.program().interner().scalar(ScalarType::Unit)
        );

        for source in [
            "import std.console\nfn invalid() { console.print() }\n",
            "import std.console\nfn invalid() { console.print(1) }\n",
            "import std.console\nfn invalid() { console.print(value: \"named\") }\n",
            "import std.console\nfn invalid(parts: Array[String]) { console.print(...parts) }\n",
        ] {
            let (_, _, output) = check(source);
            assert!(
                codes(&output).contains(&"E1102"),
                "source should fail with E1102: {source}\n{:#?}",
                output.diagnostics()
            );
        }
    }
}
